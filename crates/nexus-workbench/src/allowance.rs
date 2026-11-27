//! The plan allowance by a Code composer: who pays for the session (Nexus Gateway or the person's Claude or ChatGPT
//! plan), how much of that plan's tightest shared limit is left, and when that is low enough to warn. The Nexus app's
//! `plan-allowance-model.ts`, `plan-allowance.tsx` and `plan-usage.ts` hold the same rules; Nexus GPUI reads them here.

use chrono::TimeZone;

use crate::event::{SessionAgent, SessionConnection, Via};
use crate::limit_stop::resets_on;
use crate::plans::{Limit, PlanUsage};

/// At or under this much left (percent), the composer warns: the plan is nearly out.
pub const LOW_ALLOWANCE: i64 = 15;

/// Who pays for the session's model calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Nexus Gateway: the organization's budget, whatever the agent.
    Nexus,
    /// The person's own plan for this agent (Claude or Codex).
    Plan(SessionAgent),
}

impl Source {
    /// The name the composer shows: `Nexus`, `Claude`, `Codex`.
    pub fn name(self) -> &'static str {
        match self {
            Source::Nexus => "Nexus",
            Source::Plan(agent) => crate::harness::harness(agent).plan_payer().unwrap_or("Nexus"),
        }
    }
}

/// Who pays, from the session's requested route and, once its agent runs, the route it reported. None: nothing to
/// show, as for an API key, which has no plan allowance.
pub fn source(agent: SessionAgent, via: Via, connection: Option<&SessionConnection>) -> Option<Source> {
    if connection.and_then(|connection| connection.via).unwrap_or(via) == Via::Nexus {
        return Some(Source::Nexus);
    }
    let api_key = connection
        .and_then(|connection| connection.account.as_ref())
        .is_some_and(|account| account.kind == "apiKey");
    (crate::harness::harness(agent).plan_payer().is_some() && !api_key).then_some(Source::Plan(agent))
}

/// Whole percent left of a limit, from 0 to 100; None when the agent reported no number.
pub fn left(limit: &Limit) -> Option<i64> {
    limit
        .used_percent
        .is_finite()
        .then(|| 100 - limit.used_percent.round().clamp(0., 100.) as i64)
}

/// Which login of the agent the composer reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Login<'a> {
    /// A new session's default: the active account, else the agent's own login.
    NewSession,
    /// A session's own: an extra account, or None for the agent's own login.
    Of(Option<&'a str>),
}

/// The signed-in plan for `login`. With `email` (the account the running agent reported), a plan for another email
/// is not the session's.
pub fn plan_for<'a>(
    plans: &'a [PlanUsage],
    agent: SessionAgent,
    login: Login<'_>,
    email: Option<&str>,
) -> Option<&'a PlanUsage> {
    let mut candidates = plans.iter().filter(|plan| plan.agent == agent && plan.signed_in);
    let chosen = match login {
        Login::Of(account) => candidates.find(|plan| plan.account.as_deref() == account),
        Login::NewSession => {
            let candidates: Vec<_> = candidates.collect();
            candidates
                .iter()
                .find(|plan| plan.active)
                .or_else(|| candidates.iter().find(|plan| plan.account.is_none()))
                .copied()
        }
    };
    match email {
        Some(email)
            if !chosen
                .and_then(|plan| plan.email.as_deref())
                .is_some_and(|known| known.eq_ignore_ascii_case(email)) =>
        {
            None
        }
        _ => chosen,
    }
}

/// The limit with the least left, and how much.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Allowance<'a> {
    pub limit: &'a Limit,
    pub left: i64,
}

/// The plan's shared limit (Session or Weekly) with the least left. Model-specific weeks stay in Settings. None when
/// the plan failed, a reading is past its reset (it must refresh before it can warn again), or no number was read.
pub fn limiting(plan: Option<&PlanUsage>, now: i64) -> Option<Allowance<'_>> {
    let plan = plan.filter(|plan| plan.error.is_none())?;
    let limits: Vec<&Limit> = plan
        .limits
        .iter()
        .filter(|limit| limit.label == "Session" || limit.label == "Weekly")
        .collect();
    if limits.iter().any(|limit| limit.resets_at.is_some_and(|at| at <= now)) {
        return None;
    }
    limits
        .into_iter()
        .filter_map(|limit| left(limit).map(|left| Allowance { limit, left }))
        .min_by_key(|allowance| allowance.left)
}

/// The limit a session on `model` works against. A plan whose limits name their models (Antigravity's model groups):
/// the tightest limit of the model's group, else of every group when the model is unknown or in none. Any other plan:
/// [`limiting`]. None in the same cases as [`limiting`].
pub fn limiting_for<'a>(plan: Option<&'a PlanUsage>, model: Option<&str>, now: i64) -> Option<Allowance<'a>> {
    let read = plan.filter(|plan| plan.error.is_none())?;
    let grouped: Vec<&Limit> = read.limits.iter().filter(|limit| !limit.models.is_empty()).collect();
    if grouped.is_empty() {
        return limiting(plan, now);
    }
    let of_model: Vec<&Limit> = model
        .map(|model| {
            grouped
                .iter()
                .copied()
                .filter(|limit| nexus_client::local_agents::names_model(&limit.models, model))
                .collect()
        })
        .unwrap_or_default();
    let limits = if of_model.is_empty() { grouped } else { of_model };
    if limits.iter().any(|limit| limit.resets_at.is_some_and(|at| at <= now)) {
        return None;
    }
    limits
        .into_iter()
        .filter_map(|limit| left(limit).map(|left| Allowance { limit, left }))
        .min_by_key(|allowance| allowance.left)
}

/// The limit as the composer names it: `5-hour limit`, `Weekly limit`, `Weekly limit · Fable`.
pub fn limit_label(label: &str) -> String {
    if label == "Session" {
        return "5-hour limit".into();
    }
    match label.strip_prefix("Weekly") {
        Some(rest) if rest.is_empty() || !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_') => {
            format!("Weekly limit{rest}")
        }
        _ => label.to_string(),
    }
}

/// How long until `at`: `42 min`, `3 h 13 min`, `1 d 13 h`. `at` and `now` are epoch ms.
pub fn resets_in(at: i64, now: i64) -> String {
    let minutes = ((at - now) as f64 / 60_000.).ceil().max(1.) as i64;
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return if minutes % 60 > 0 {
            format!("{hours} h {} min", minutes % 60)
        } else {
            format!("{hours} h")
        };
    }
    if hours % 24 > 0 {
        format!("{} d {} h", hours / 24, hours % 24)
    } else {
        format!("{} d", hours / 24)
    }
}

/// When the limit resets, for the warning and the line's tooltip: `Resets in 3 h · today at 3:40 PM.`
pub fn reset_text<Tz: TimeZone>(allowance: &Allowance<'_>, now: i64, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    match allowance.limit.resets_at {
        Some(at) => format!("Resets in {} · {}.", resets_in(at, now), resets_on(at, now, zone)),
        None => "Reset time not reported.".into(),
    }
}

/// What the agent's plans read, as the composer knows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading<'a> {
    /// Not read yet.
    Checking,
    /// The read failed.
    Failed,
    /// Read; None when the session's plan has no usable limit.
    Read(Option<Allowance<'a>>),
}

/// A provider plan is meaningful only when that plan pays for the session.
/// Do not consult it for Nexus-routed calls, including their loading and failure states.
pub fn reading<'a>(source: Source, plan: impl FnOnce() -> Reading<'a>) -> Reading<'a> {
    if source == Source::Nexus {
        Reading::Read(None)
    } else {
        plan()
    }
}

/// The line under the composer: `Claude · 62% left · 5-hour limit`, `Nexus · Budget and spend`.
pub fn line(source: Source, reading: Reading<'_>) -> String {
    let name = source.name();
    if source == Source::Nexus {
        return format!("{name} · Budget and spend");
    }
    match reading {
        Reading::Read(Some(allowance)) => format!(
            "{name} · {}% left · {}",
            allowance.left,
            limit_label(&allowance.limit.label)
        ),
        Reading::Checking => format!("{name} · Checking plan usage…"),
        Reading::Failed | Reading::Read(None) => format!("{name} · Usage unavailable"),
    }
}

/// Whether the composer warns: at or under [`LOW_ALLOWANCE`] left, unless the session's usage-limit stop already
/// says so.
pub fn warns(allowance: Option<&Allowance<'_>>, limited: bool) -> bool {
    allowance.is_some_and(|allowance| allowance.left <= LOW_ALLOWANCE) && !limited
}

/// The warning over the composer: `6% left in your Claude 5-hour limit`, `Your Claude weekly limit is used up`.
pub fn warning(source: Source, allowance: &Allowance<'_>) -> String {
    let label = limit_label(&allowance.limit.label).to_lowercase();
    let name = source.name();
    if allowance.left == 0 {
        format!("Your {name} {label} is used up")
    } else {
        format!("{}% left in your {name} {label}", allowance.left)
    }
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;
    use crate::event::RuntimeAccount;

    fn plan(account: Option<&str>) -> PlanUsage {
        PlanUsage {
            agent: SessionAgent::Codex,
            account: account.map(str::to_string),
            email: Some("member@example.com".into()),
            plan: Some("plus".into()),
            signed_in: true,
            active: false,
            limits: vec![
                Limit {
                    models: Vec::new(),
                    label: "Session".into(),
                    used_percent: 28.,
                    resets_at: Some(10_000),
                },
                Limit {
                    models: Vec::new(),
                    label: "Weekly".into(),
                    used_percent: 64.,
                    resets_at: Some(20_000),
                },
            ],
            resets: None,
            error: None,
            google_cloud: false,
            unavailable: false,
            fetched_at: None,
        }
    }

    fn connection(via: Option<Via>, account: Option<&str>) -> SessionConnection {
        SessionConnection {
            via,
            account: account.map(|kind| RuntimeAccount {
                kind: kind.into(),
                ..RuntimeAccount::default()
            }),
            ..SessionConnection::default()
        }
    }

    fn limit(label: &str, used_percent: f64, resets_at: Option<i64>) -> Limit {
        Limit {
            models: Vec::new(),
            label: label.into(),
            used_percent,
            resets_at,
        }
    }

    #[test]
    fn accepts_a_remote_hosts_plan_reading_without_losing_account_or_reset() {
        let original = plan(Some("team-account"));
        let json = serde_json::to_value(&original).expect("the allowance fixture has the expected reading");
        assert_eq!(json["signedIn"], true);
        assert_eq!(json["limits"][0]["resetsAt"], 10_000);
        let received: PlanUsage = serde_json::from_value(json).expect("the allowance fixture has the expected reading");
        assert_eq!(received, original);
        assert_eq!(
            limiting(Some(&received), 0)
                .expect("the allowance fixture has the expected reading")
                .left,
            36
        );
    }

    /// An Antigravity plan's limits are per model group (agy 1.5.8's `/usage`): a session works against its model's
    /// group, and with no known model against the tightest group.
    #[test]
    fn antigravity_session_reads_its_models_group() {
        let groups = nexus_client::local_agents::read_antigravity_usage(include_str!(
            "../../nexus-client/tests/fixtures/agy-1.5.8-usage.json"
        ))
        .unwrap_or_default();
        let mut usage = PlanUsage {
            signed_in: true,
            limits: crate::plans::antigravity_limits(&groups),
            ..plan(None)
        };
        usage.agent = SessionAgent::Antigravity;
        assert_eq!(
            usage
                .limits
                .iter()
                .map(|limit| limit.label.as_str())
                .collect::<Vec<_>>(),
            // agy names the groups "Gemini Models" and "Claude and GPT models"; the labels drop the "Models".
            ["Weekly · Gemini", "Weekly · Claude and GPT"]
        );
        let reset = chrono::DateTime::parse_from_rfc3339("2026-11-23T16:28:00Z").map(|at| at.timestamp_millis());
        assert_eq!(usage.limits[0].resets_at, reset.ok());
        let gemini = limiting_for(Some(&usage), Some("gemini-3.8-flash"), 0);
        assert_eq!(gemini.map(|allowance| allowance.left), Some(96));
        let opus = limiting_for(Some(&usage), Some("claude-opus-4-6-thinking"), 0);
        assert_eq!(
            opus.map(|allowance| (allowance.left, allowance.limit.label.as_str())),
            Some((100, "Weekly · Claude and GPT"))
        );
        // No model known: the tightest group.
        assert_eq!(
            limiting_for(Some(&usage), None, 0).map(|allowance| allowance.limit.label.as_str()),
            Some("Weekly · Gemini")
        );
        assert_eq!(limit_label("Weekly · Gemini"), "Weekly limit · Gemini");
        // A plan without groups reads as before.
        assert_eq!(
            limiting_for(Some(&plan(None)), Some("opus"), 0).map(|allowance| allowance.left),
            limiting(Some(&plan(None)), 0).map(|allowance| allowance.left)
        );
    }

    #[test]
    fn distinguishes_nexus_gateway_usage_from_each_providers_plan_using_the_observed_route() {
        assert_eq!(
            source(SessionAgent::Claude, Via::Subscription, None),
            Some(Source::Plan(SessionAgent::Claude))
        );
        assert_eq!(
            source(SessionAgent::Codex, Via::Subscription, None),
            Some(Source::Plan(SessionAgent::Codex))
        );
        assert_eq!(source(SessionAgent::Nexus, Via::Nexus, None), Some(Source::Nexus));
        assert_eq!(source(SessionAgent::Claude, Via::Nexus, None), Some(Source::Nexus));
        assert_eq!(source(SessionAgent::Codex, Via::Nexus, None), Some(Source::Nexus));
        let on_gateway = connection(Some(Via::Nexus), None);
        assert_eq!(
            source(SessionAgent::Claude, Via::Subscription, Some(&on_gateway)),
            Some(Source::Nexus)
        );
        let on_plan = connection(Some(Via::Subscription), None);
        assert_eq!(
            source(SessionAgent::Codex, Via::Nexus, Some(&on_plan)),
            Some(Source::Plan(SessionAgent::Codex))
        );
        assert_eq!(source(SessionAgent::Nexus, Via::Subscription, None), None);
        assert_eq!(
            source(SessionAgent::Antigravity, Via::Subscription, None),
            Some(Source::Plan(SessionAgent::Antigravity))
        );
        let api_key = connection(Some(Via::Subscription), Some("apiKey"));
        assert_eq!(source(SessionAgent::Claude, Via::Subscription, Some(&api_key)), None);
        // A connection that has not reported its route yet keeps the requested one.
        let unreported = connection(None, None);
        assert_eq!(
            source(SessionAgent::Claude, Via::Nexus, Some(&unreported)),
            Some(Source::Nexus)
        );
    }

    #[test]
    fn keeps_explicit_accounts_distinct_from_the_active_default_and_other_providers() {
        let own = plan(None);
        let active = PlanUsage {
            active: true,
            ..plan(Some("extra"))
        };
        let other = PlanUsage {
            agent: SessionAgent::Claude,
            active: true,
            ..plan(None)
        };
        let plans = vec![other, active.clone(), own.clone()];
        assert_eq!(plan_for(&plans, SessionAgent::Codex, Login::Of(None), None), Some(&own));
        assert_eq!(
            plan_for(&plans, SessionAgent::Codex, Login::Of(Some("extra")), None),
            Some(&active)
        );
        assert_eq!(
            plan_for(&plans, SessionAgent::Codex, Login::NewSession, None),
            Some(&active)
        );
        assert_eq!(
            plan_for(&plans, SessionAgent::Codex, Login::Of(Some("missing")), None),
            None
        );
        let signed_out = vec![PlanUsage {
            signed_in: false,
            ..plan(None)
        }];
        assert_eq!(plan_for(&signed_out, SessionAgent::Codex, Login::Of(None), None), None);
        assert_eq!(
            plan_for(
                &plans,
                SessionAgent::Codex,
                Login::Of(None),
                Some("different@example.com")
            ),
            None
        );
        assert_eq!(
            plan_for(&plans, SessionAgent::Codex, Login::Of(None), Some("MEMBER@example.com")),
            Some(&own)
        );
        // With no active account, a new session reads the agent's own login.
        let plain = vec![plan(Some("extra")), own.clone()];
        assert_eq!(
            plan_for(&plain, SessionAgent::Codex, Login::NewSession, None),
            Some(&own)
        );
    }

    #[test]
    fn shows_the_tightest_shared_window_not_an_average_or_another_models_week() {
        let mut account = plan(None);
        account.limits.push(limit("Weekly · Another model", 99., Some(20_000)));
        let tightest = limiting(Some(&account), 0).expect("the allowance fixture has the expected reading");
        assert_eq!((tightest.limit.label.as_str(), tightest.left), ("Weekly", 36));
        account.limits[0].used_percent = 94.;
        let tightest = limiting(Some(&account), 0).expect("the allowance fixture has the expected reading");
        assert_eq!((tightest.limit.label.as_str(), tightest.left), ("Session", 6));
        account.limits[1].used_percent = 100.;
        assert_eq!(limiting(Some(&account), 0).map(|allowance| allowance.left), Some(0));
    }

    #[test]
    fn does_not_treat_expired_failed_or_unreported_readings_as_exhausted_allowances() {
        assert_eq!(limiting(Some(&plan(None)), 10_000), None);
        let failed = PlanUsage {
            error: Some("Refresh failed".into()),
            ..plan(None)
        };
        assert_eq!(limiting(Some(&failed), 0), None);
        let empty = PlanUsage {
            limits: vec![],
            ..plan(None)
        };
        assert_eq!(limiting(Some(&empty), 0), None);
        assert_eq!(limiting(None, 0), None);
        assert_eq!(left(&limit("Weekly", f64::NAN, None)), None);
        assert_eq!(left(&limit("Weekly", f64::INFINITY, None)), None);
    }

    #[test]
    fn keeps_a_providers_unknown_reset_time_distinct_from_zero_allowance() {
        let account = PlanUsage {
            limits: vec![limit("Weekly", 91., None)],
            ..plan(None)
        };
        assert_eq!(limiting(Some(&account), 0).map(|allowance| allowance.left), Some(9));
        assert_eq!(left(&limit("Weekly", -20., None)), Some(100));
        assert_eq!(left(&limit("Weekly", 110., None)), Some(0));
    }

    #[test]
    fn says_how_long_until_a_limit_resets() {
        let minute = 60_000;
        assert_eq!(resets_in(0, 0), "1 min");
        assert_eq!(resets_in(42 * minute, 0), "42 min");
        assert_eq!(resets_in(60 * minute, 0), "1 h");
        assert_eq!(resets_in(193 * minute, 0), "3 h 13 min");
        assert_eq!(resets_in(37 * 60 * minute, 0), "1 d 13 h");
        assert_eq!(resets_in(6 * 24 * 60 * minute, 0), "6 d");
        assert_eq!(resets_in(-5, 0), "1 min");
    }

    #[test]
    fn names_the_limits_the_way_the_settings_page_reads_them() {
        assert_eq!(limit_label("Session"), "5-hour limit");
        assert_eq!(limit_label("Weekly"), "Weekly limit");
        assert_eq!(limit_label("Weekly · Fable"), "Weekly limit · Fable");
        assert_eq!(limit_label("Monthly"), "Monthly");
        assert_eq!(limit_label("Weeklyish"), "Weeklyish");
    }

    #[test]
    fn writes_the_line_under_the_composer() {
        let account = plan(None);
        let tightest = limiting(Some(&account), 0);
        let claude = Source::Plan(SessionAgent::Claude);
        assert_eq!(line(Source::Nexus, Reading::Checking), "Nexus · Budget and spend");
        assert_eq!(
            line(claude, Reading::Read(tightest)),
            "Claude · 36% left · Weekly limit"
        );
        assert_eq!(line(claude, Reading::Checking), "Claude · Checking plan usage…");
        assert_eq!(line(claude, Reading::Failed), "Claude · Usage unavailable");
        assert_eq!(
            line(Source::Plan(SessionAgent::Codex), Reading::Read(None)),
            "Codex · Usage unavailable"
        );
    }

    #[test]
    fn nexus_routing_never_reads_or_warns_on_a_low_personal_plan() {
        for agent in [SessionAgent::Claude, SessionAgent::Codex] {
            let low = PlanUsage {
                agent,
                limits: vec![limit("Session", 94., Some(10_000))],
                ..plan(None)
            };
            let provider = Reading::Read(limiting(Some(&low), 0));
            assert!(matches!(provider,Reading::Read(Some(allowance)) if warns(Some(&allowance),false)));
            let observed = connection(Some(Via::Nexus), None);
            let source = source(agent, Via::Subscription, Some(&observed)).expect("observed payer");
            let mut consulted = false;
            let read = reading(source, || {
                consulted = true;
                provider
            });
            assert!(!consulted, "Nexus does not consult the provider account");
            assert_eq!(read, Reading::Read(None));
            assert_eq!(line(source, read), "Nexus · Budget and spend");
            let Reading::Read(allowance) = read else {
                panic!("Nexus has no provider reading")
            };
            assert!(!warns(allowance.as_ref(), false));
            assert_eq!(reading(Source::Plan(agent), || provider), provider);
        }
    }

    #[test]
    fn warns_at_fifteen_percent_left_unless_the_limit_stop_already_shows() {
        let at = |used: f64| PlanUsage {
            limits: vec![limit("Session", used, Some(10_000))],
            ..plan(None)
        };
        let (low, edge, fine, out) = (at(94.), at(85.), at(84.), at(100.));
        let claude = Source::Plan(SessionAgent::Claude);
        let low = limiting(Some(&low), 0).expect("the allowance fixture has the expected reading");
        assert!(warns(Some(&low), false));
        assert!(!warns(Some(&low), true));
        assert!(warns(limiting(Some(&edge), 0).as_ref(), false));
        assert!(!warns(limiting(Some(&fine), 0).as_ref(), false));
        assert!(!warns(None, false));
        assert_eq!(warning(claude, &low), "6% left in your Claude 5-hour limit");
        let out = limiting(Some(&out), 0).expect("the allowance fixture has the expected reading");
        assert_eq!(
            warning(Source::Plan(SessionAgent::Codex), &out),
            "Your Codex 5-hour limit is used up"
        );
    }

    #[test]
    fn says_when_the_limit_resets() {
        let zone = FixedOffset::west_opt(3 * 3600).expect("the allowance fixture has the expected reading");
        let now = zone
            .with_ymd_and_hms(2026, 10, 28, 12, 0, 0)
            .single()
            .expect("the allowance fixture has the expected reading")
            .timestamp_millis();
        let at = zone
            .with_ymd_and_hms(2026, 10, 28, 15, 40, 0)
            .single()
            .expect("the allowance fixture has the expected reading")
            .timestamp_millis();
        let known = limit("Session", 90., Some(at));
        let allowance = Allowance {
            limit: &known,
            left: 10,
        };
        assert_eq!(
            reset_text(&allowance, now, &zone),
            "Resets in 3 h 40 min · today at 3:40 PM."
        );
        let unknown = limit("Weekly", 90., None);
        let allowance = Allowance {
            limit: &unknown,
            left: 10,
        };
        assert_eq!(reset_text(&allowance, now, &zone), "Reset time not reported.");
    }
}
