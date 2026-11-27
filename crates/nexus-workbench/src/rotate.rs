//! Which Claude or ChatGPT account a session on the person's plan runs on, as claude-swap's `cswap auto` decides it:
//! the account it already runs on (a new session: the active one) while that is under [`ROTATE_AT`] of its session
//! and weekly limits, else the active account while it has room, else the signed-in account with the most left. The limits come from the plans' probes
//! ([`crate::plans`], at most every [`FRESH_FOR`]) and, in between, from each call's rate-limit report.
//!
//! Each agent has its own [`Accounts`]. A Codex session never moves once it has started: Codex keeps each account's
//! conversations apart ([`crate::accounts`]), so only a new Codex session takes another account.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::event::SessionAgent;
use crate::harness::harness;
use crate::plans::PlanUsage;

/// At this share (%) of its session or weekly limit, an account hands its sessions on: claude-swap's default.
pub const ROTATE_AT: f64 = 90.0;

/// How long the probes' answer stands before an agent start checks again.
pub const FRESH_FOR: Duration = Duration::from_secs(120);

/// Whether extra subscription accounts rotate automatically before a person chooses a setting.
pub fn auto_rotate_by_default(agent: SessionAgent) -> bool {
    harness(agent)
        .accounts()
        .is_some_and(|folders| folders.auto_rotate_by_default)
}

/// An account as last seen: its email, whether it is signed in, and how much of its limits it has used (0 to 100).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    pub email: Option<String>,
    pub signed_in: bool,
    pub session: Option<f64>,
    pub weekly: Option<f64>,
    /// The limits of one model family (`Weekly · Fable`): the family's name and how much is used.
    pub scoped: Vec<(String, f64)>,
}

impl Seen {
    /// The busier of the two limits, when either is known.
    pub fn used(&self) -> Option<f64> {
        match (self.session, self.weekly) {
            (Some(session), Some(weekly)) => Some(session.max(weekly)),
            (session, weekly) => session.or(weekly),
        }
    }

    /// The busiest limit a session on `model` meets: the plan's own, and a model family's limit that names it. `model`
    /// is the agent's id (`claude-fable-5-2`); None or `auto`: the plan's own limits only.
    pub fn used_for(&self, model: Option<&str>) -> Option<f64> {
        let name = model
            .filter(|model| !nexus_client::agents::is_auto_name(Some(model)))
            .map(crate::cycle::model_label);
        let family = name.and_then(|name| {
            self.scoped
                .iter()
                .filter(|(scope, _)| nexus_client::local_agents::names_model(std::slice::from_ref(scope), &name))
                .map(|(_, used)| *used)
                .reduce(f64::max)
        });
        match (self.used(), family) {
            (Some(own), Some(family)) => Some(own.max(family)),
            (own, family) => own.or(family),
        }
    }
}

/// What the workbench knows of one agent's accounts (by id; None: the agent's own login), and the active one.
#[derive(Debug)]
pub struct Accounts {
    /// Claude or Codex: whose plans these are.
    pub agent: SessionAgent,
    pub seen: HashMap<Option<String>, Seen>,
    /// Where new sessions start: the account the person chose, until a start moves off it.
    pub active: Option<String>,
    /// The person turned auto-rotation off: a session leaves its account only when that is signed out or gone.
    pub manual: bool,
    checked: Option<Instant>,
}

impl Default for Accounts {
    /// Claude's accounts, rotation on.
    fn default() -> Self {
        Accounts::new(SessionAgent::Claude)
    }
}

impl Accounts {
    /// `agent`'s accounts. Rotation is on for Claude. It is off for Codex until the person turns it on, so a Codex
    /// session starts on Codex's own login unless Settings says otherwise.
    pub fn new(agent: SessionAgent) -> Self {
        Accounts {
            agent,
            seen: HashMap::new(),
            active: None,
            manual: !auto_rotate_by_default(agent),
            checked: None,
        }
    }

    /// The probes' answer is missing or older than [`FRESH_FOR`].
    pub fn stale(&self) -> bool {
        self.checked.is_none_or(|at| at.elapsed() >= FRESH_FOR)
    }

    /// The probes' answer replaces what was known. An account whose check failed counts as signed in with room: a
    /// failed check is no reason to move. An answer with no plan of this agent (a host that probes only Claude) keeps
    /// what the calls reported.
    pub fn checked(&mut self, plans: &[PlanUsage]) {
        self.checked = Some(Instant::now());
        if !plans.iter().any(|plan| plan.agent == self.agent) {
            return;
        }
        let agent = self.agent;
        self.seen = plans
            .iter()
            .filter(|plan| plan.agent == agent)
            .map(|plan| {
                let limit = |label: &str| {
                    plan.limits
                        .iter()
                        .find(|limit| limit.label == label)
                        .map(|limit| limit.used_percent)
                };
                let scoped = plan
                    .limits
                    .iter()
                    .filter_map(|limit| {
                        let (_, scope) = limit.label.split_once('·')?;
                        let scope = scope.trim();
                        (!scope.is_empty()).then(|| (scope.to_string(), limit.used_percent))
                    })
                    .collect();
                let seen = Seen {
                    email: plan.email.clone(),
                    signed_in: plan.signed_in || plan.error.is_some(),
                    session: limit("Session"),
                    weekly: limit("Weekly"),
                    scoped,
                };
                (plan.account.clone(), seen)
            })
            .collect();
    }

    /// A call on `account` reported its limits. `refused`: it hit one.
    pub fn saw(&mut self, account: &Option<String>, session: Option<f64>, weekly: Option<f64>, refused: bool) {
        let seen = self.seen.entry(account.clone()).or_insert_with(|| Seen {
            signed_in: true,
            ..Seen::default()
        });
        seen.session = session.or(seen.session);
        seen.weekly = weekly.or(seen.weekly);
        if refused && seen.used().is_none_or(|used| used < 100.0) {
            seen.session = Some(100.0);
        }
    }

    /// `account` has used [`ROTATE_AT`] of a limit: a session on it may have to leave. A signed-out account is not
    /// near a limit: its sessions stay and say it is signed out, so they never move to someone else's login.
    pub fn near_limit(&self, account: &Option<String>) -> bool {
        self.near_limit_for(account, None)
    }

    /// [`Accounts::near_limit`] for a session on `model`: a model family's limit counts too.
    pub fn near_limit_for(&self, account: &Option<String>, model: Option<&str>) -> bool {
        self.seen
            .get(account)
            .is_some_and(|seen| seen.signed_in && seen.used_for(model).is_some_and(|used| used >= ROTATE_AT))
    }

    /// Where a session that would run on `preferred` runs: there while it is under [`ROTATE_AT`] (or not known at all;
    /// [`Accounts::manual`]: until it is used up and the person chose another account), else the active account while
    /// it has room, else the signed-in account with the most left, if it has more. A signed-out `preferred` stays: a
    /// work login that was lost must not quietly become a personal one. `gone`: `preferred` was removed.
    pub fn choose(&self, preferred: &Option<String>, gone: bool) -> Option<String> {
        self.choose_for(preferred, gone, None)
    }

    /// [`Accounts::choose`] for a session on `model`: a model family's limit (`Weekly · Fable`) counts as one of the
    /// account's limits, so a Fable session leaves an account whose Fable week is used up for one where Fable has room.
    pub fn choose_for(&self, preferred: &Option<String>, gone: bool, model: Option<&str>) -> Option<String> {
        let used = |seen: &Seen| seen.used_for(model);
        let current = self.seen.get(preferred);
        // The account the person chose takes a session that has to leave its own, while it has room.
        let active = self
            .seen
            .get(&self.active)
            .filter(|seen| self.active != *preferred && seen.signed_in)
            .filter(|seen| used(seen).is_none_or(|used| used < ROTATE_AT))
            .map(|_| self.active.clone());
        let stays = match current {
            None => true,
            Some(seen) if !seen.signed_in => true,
            Some(seen) if self.manual => active.is_none() || used(seen).is_none_or(|used| used < 100.0),
            Some(seen) => used(seen).is_none_or(|used| used < ROTATE_AT),
        };
        if stays && !gone {
            return preferred.clone();
        }
        if let Some(active) = active {
            return active;
        }
        let room = |seen: &Seen| 100.0 - used(seen).unwrap_or(0.0);
        let best = self
            .seen
            .iter()
            .filter(|(account, seen)| seen.signed_in && !(gone && *account == preferred))
            .max_by(|a, b| room(a.1).total_cmp(&room(b.1)));
        let Some((account, seen)) = best else {
            return if gone { None } else { preferred.clone() };
        };
        // A move only for more room than where it is.
        if !gone && current.is_some_and(|now| now.signed_in && room(seen) <= room(now)) {
            return preferred.clone();
        }
        account.clone()
    }

    /// The session log's line when a session runs on `to` instead of `from`.
    pub fn moved(&self, from: &Option<String>, to: &Option<String>) -> String {
        let (own, plan) = harness(self.agent)
            .accounts()
            .map_or(("Claude Code's own login", "Claude"), |folders| {
                (folders.own_login, crate::accounts::plan(self.agent))
            });
        let name = |account: &Option<String>| {
            self.seen
                .get(account)
                .and_then(|seen| seen.email.clone())
                .unwrap_or_else(|| match account {
                    None => own.into(),
                    Some(_) => format!("another {plan} account"),
                })
        };
        let why = match self.seen.get(from) {
            Some(seen) if !seen.signed_in => "is signed out".to_string(),
            Some(seen) => match seen.used() {
                Some(used) => format!("has used {used:.0}% of its session or weekly limit"),
                None => "is near its limits".to_string(),
            },
            None => "is no longer on this computer".to_string(),
        };
        format!("Running on {}'s {plan} plan: {} {why}.", name(to), name(from))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plans::Limit;

    fn plan(account: Option<&str>, email: &str, session: f64, weekly: f64) -> PlanUsage {
        let limit = |label: &str, used: f64| Limit {
            models: Vec::new(),
            label: label.into(),
            used_percent: used,
            resets_at: None,
        };
        let mut plan = crate::plans::read_claude(
            account.map(String::from),
            &serde_json::json!({"account": {"email": email, "subscriptionType": "Claude Max"}}),
            Some(&serde_json::json!({"rate_limits_available": false})),
        );
        plan.limits = vec![limit("Session", session), limit("Weekly", weekly)];
        plan
    }

    fn accounts(plans: &[PlanUsage]) -> Accounts {
        let mut accounts = Accounts::default();
        accounts.checked(plans);
        accounts
    }

    #[test]
    fn stays_while_there_is_room_then_moves_to_the_most_left() {
        let own = None;
        let extra = Some("a_1".to_string());
        let mut known = accounts(&[
            plan(None, "work@example.com", 40.0, 20.0),
            plan(Some("a_1"), "me@example.com", 10.0, 5.0),
        ]);
        // Under the line: it stays, even with more room elsewhere.
        assert_eq!(known.choose(&own, false), own);
        // A call says the session limit is nearly used: the most left wins.
        known.saw(&own, Some(93.0), None, false);
        assert_eq!(known.choose(&own, false), extra);
        assert_eq!(
            known.moved(&own, &extra),
            "Running on me@example.com's Claude plan: work@example.com has used 93% of its session or weekly limit."
        );
        // The weekly limit counts too, and a refused call means used up.
        known.saw(&extra, None, Some(95.0), false);
        assert_eq!(known.choose(&extra, false), own);
        known.saw(&own, Some(10.0), None, true);
        assert_eq!(known.seen[&own].used(), Some(100.0));
    }

    #[test]
    fn stays_put_when_nothing_has_more_left_or_nothing_is_known() {
        let own = None;
        let full = accounts(&[
            plan(None, "a@x.com", 95.0, 0.0),
            plan(Some("a_1"), "b@x.com", 97.0, 0.0),
        ]);
        assert_eq!(full.choose(&own, false), own);
        // Never checked (its probe failed, or no probe ran): no reason to move.
        assert_eq!(
            Accounts::default().choose(&Some("a_9".into()), false),
            Some("a_9".into())
        );
    }

    #[test]
    fn manual_stays_at_the_limits_and_on_a_signed_out_account() {
        let mut signed_out = plan(Some("a_2"), "c@x.com", 0.0, 0.0);
        signed_out.signed_in = false;
        let mut known = accounts(&[
            plan(None, "a@x.com", 97.0, 0.0),
            plan(Some("a_1"), "b@x.com", 10.0, 0.0),
            signed_out,
        ]);
        known.manual = true;
        // At 97%, and after a refused call: it stays.
        assert_eq!(known.choose(&None, false), None);
        known.saw(&None, None, None, true);
        assert_eq!(known.choose(&None, false), None);
        // Signed out: it stays, and says so. Removed: it moves, to the account with the most left.
        assert_eq!(known.choose(&Some("a_2".into()), false), Some("a_2".into()));
        assert_eq!(known.choose(&Some("a_9".into()), true), Some("a_1".into()));
    }

    #[test]
    fn a_session_that_must_leave_goes_to_the_account_the_person_chose() {
        let mut known = accounts(&[
            plan(None, "a@x.com", 100.0, 0.0),
            plan(Some("a_1"), "b@x.com", 10.0, 0.0),
            plan(Some("a_2"), "c@x.com", 40.0, 0.0),
        ]);
        // Nothing chosen: the most left.
        assert_eq!(known.choose(&None, false), Some("a_1".into()));
        // Chosen and with room: it wins over the most left.
        known.active = Some("a_2".into());
        assert_eq!(known.choose(&None, false), Some("a_2".into()));
        // A session with room stays where it is.
        assert_eq!(known.choose(&Some("a_1".into()), false), Some("a_1".into()));
        // Rotation off: a used-up session still goes to the chosen account, one near its limit stays.
        known.manual = true;
        assert_eq!(known.choose(&None, false), Some("a_2".into()));
        known.saw(&None, Some(95.0), None, false);
        assert_eq!(known.choose(&None, false), None);
        // The chosen account near its own limit: the most left, as before.
        known.manual = false;
        known.saw(&Some("a_2".into()), Some(92.0), None, false);
        assert_eq!(known.choose(&None, false), Some("a_1".into()));
        assert!(known.near_limit(&None));
        assert!(!known.near_limit(&Some("a_1".into())));
        assert!(!known.near_limit(&Some("a_9".into())));
    }

    #[test]
    fn stays_on_a_signed_out_account_and_leaves_one_that_is_gone() {
        let mut signed_out = plan(Some("a_1"), "b@x.com", 0.0, 0.0);
        signed_out.signed_in = false;
        let known = accounts(&[plan(None, "a@x.com", 50.0, 0.0), signed_out]);
        // A lost login never quietly becomes another one: new and running sessions stay on it.
        assert_eq!(known.choose(&Some("a_1".into()), false), Some("a_1".into()));
        assert!(!known.near_limit(&Some("a_1".into())));
        // Removed from the computer: the best of the others, Claude Code's own login when nothing is known.
        assert_eq!(known.choose(&Some("a_2".into()), true), None);
        assert_eq!(Accounts::default().choose(&Some("a_2".into()), true), None);
    }

    #[test]
    fn codex_keeps_its_own_accounts_and_starts_with_rotation_off() {
        let codex_plan = |account: Option<&str>, email: &str, used: f64| PlanUsage {
            agent: SessionAgent::Codex,
            ..plan(account, email, used, 0.0)
        };
        let mut codex = Accounts::new(SessionAgent::Codex);
        assert!(codex.manual, "Codex rotation is off until the person turns it on");
        codex.checked(&[
            codex_plan(None, "work@example.com", 95.0),
            codex_plan(Some("a_1"), "me@example.com", 10.0),
        ]);
        // A Claude-only answer (a host that probes only Claude) keeps what Codex's own probe saw.
        codex.checked(&[plan(None, "claude@example.com", 0.0, 0.0)]);
        assert_eq!(codex.seen.len(), 2);
        assert_eq!(codex.choose(&None, false), None);
        codex.manual = false;
        assert_eq!(codex.choose(&None, false), Some("a_1".into()));
        assert_eq!(
            codex.moved(&None, &Some("a_1".into())),
            "Running on me@example.com's ChatGPT plan: work@example.com has used 95% of its session or weekly limit."
        );
        assert_eq!(
            Accounts::new(SessionAgent::Codex).moved(&Some("a_2".into()), &None),
            "Running on Codex's own login's ChatGPT plan: another ChatGPT account is no longer on this computer."
        );
    }

    #[test]
    fn a_model_family_limit_moves_only_sessions_of_that_family() {
        let scoped = |account: Option<&str>, email: &str, fable: f64| {
            let mut plan = plan(account, email, 10.0, 20.0);
            plan.limits.push(Limit {
                models: Vec::new(),
                label: "Weekly · Fable".into(),
                used_percent: fable,
                resets_at: None,
            });
            plan
        };
        let known = accounts(&[scoped(None, "own@x", 100.0), scoped(Some("a_1"), "extra@x", 5.0)]);
        let own = None;
        // A Fable session leaves the account whose Fable week is used up.
        assert_eq!(
            known.choose_for(&own, false, Some("claude-fable-5-2")),
            Some("a_1".into())
        );
        assert!(known.near_limit_for(&own, Some("claude-fable-5-2")));
        // An Opus session, Auto, or a session of unknown model stays: the plan's own limits have room.
        assert_eq!(known.choose_for(&own, false, Some("claude-opus-5-5")), own);
        assert_eq!(known.choose_for(&own, false, Some("auto")), own);
        assert_eq!(known.choose(&own, false), own);
        assert!(!known.near_limit_for(&own, Some("claude-opus-5-5")));
    }
}
