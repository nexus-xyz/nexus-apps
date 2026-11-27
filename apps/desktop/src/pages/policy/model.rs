use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::InputState;
use serde_json::{Map, Value, json};

use super::super::{Load, ui};

use super::*;

/// The slugs of the teams the person leads (`tenant.teams[]` with `role: lead`).
pub(super) fn led_teams(me: &Value) -> Vec<String> {
    me.get("teams")
        .and_then(Value::as_array)
        .map(|teams| {
            teams
                .iter()
                .filter(|team| team.get("role").and_then(Value::as_str) == Some("lead"))
                .filter_map(|team| team.get("slug")?.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// The levels a team's control applies to: the team itself, one of its people, or a key.
pub(super) const TEAM_LEVELS: [(&str, &str); 3] = [("team", "Team"), ("person", "Person"), ("key", "Key")];

/* ---------------------------------------------------------------- what Gateway answers */

/// Which rule a card edits, and which default model it sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Chat,
    Claude,
    Codex,
}

impl Kind {
    pub(super) const ALL: [Kind; 3] = [Kind::Chat, Kind::Claude, Kind::Codex];

    pub(super) fn key(self) -> &'static str {
        match self {
            Kind::Chat => "chat",
            Kind::Claude => "claude",
            Kind::Codex => "codex",
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Kind::Chat => "Organization",
            Kind::Claude => "Claude Code",
            Kind::Codex => "Codex",
        }
    }

    /// Where the place is named in the test sentence.
    pub(super) fn place(self) -> &'static str {
        match self {
            Kind::Chat => "Chat",
            other => other.title(),
        }
    }

    pub(super) fn path(self) -> &'static str {
        match self {
            Kind::Chat => "/v1/policies/organization",
            Kind::Claude => "/v1/policies/agents/claude",
            Kind::Codex => "/v1/policies/agents/codex",
        }
    }

    /// The surface a model must serve to be listed for this card.
    pub(super) fn surface(self) -> Option<&'static str> {
        match self {
            Kind::Chat => None,
            Kind::Claude => Some("anthropic_messages"),
            Kind::Codex => Some("openai_responses"),
        }
    }
}

/// A model of `GET /v1/models?status=supported`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Model {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) surfaces: Vec<String>,
}

pub(super) fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default()
}

pub(super) fn parse_catalog(value: &Value) -> Vec<Model> {
    value
        .get("data")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|entry| {
                    let id = entry.get("id")?.as_str()?.to_string();
                    Some(Model {
                        name: entry.get("name").and_then(Value::as_str).unwrap_or(&id).to_string(),
                        surfaces: strings(entry.get("surfaces")),
                        id,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One level's policy (`GET /v1/policies/...`): its own rule, and what it allows with what it inherits.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PolicyView {
    /// The stored rule as Gateway sent it, so a save keeps the parts this page does not edit.
    pub(super) rule: Value,
    pub(super) allow: Vec<String>,
    pub(super) deny: Vec<String>,
    pub(super) defaults: HashMap<String, String>,
    pub(super) custom: bool,
    pub(super) migrated: bool,
    /// The supported models allowed here, after every level.
    pub(super) allowed: Vec<String>,
    /// nexus/auto's model for each kind, and Gateway's note on it.
    pub(super) effective_defaults: HashMap<String, (Option<String>, Option<String>)>,
    /// Notes worth showing after a save.
    pub(super) notes: Vec<String>,
}

pub(super) fn parse_view(value: &Value) -> PolicyView {
    let rule = match value.get("rule") {
        Some(rule @ Value::Object(_)) => rule.clone(),
        _ => json!({}),
    };
    let defaults = rule
        .get("defaults")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(kind, model)| Some((kind.clone(), model.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let effective = value.get("effective");
    let effective_defaults = effective
        .and_then(|effective| effective.get("defaults"))
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(kind, entry)| {
                    let text = |key: &str| {
                        entry
                            .get(key)
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(String::from)
                    };
                    (kind.clone(), (text("model"), text("note")))
                })
                .collect()
        })
        .unwrap_or_default();
    PolicyView {
        allow: strings(rule.get("allow")),
        deny: strings(rule.get("deny")),
        defaults,
        custom: value.get("custom").and_then(Value::as_bool).unwrap_or(false),
        migrated: value.get("migrated").and_then(Value::as_bool).unwrap_or(false),
        allowed: strings(effective.and_then(|effective| effective.get("allowed"))),
        effective_defaults,
        notes: strings(value.get("notes")),
        rule,
    }
}

/// `POST /v1/policies/test`'s answer.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Decision {
    pub(super) allowed: bool,
    pub(super) requested: String,
    pub(super) model: Option<String>,
    pub(super) reason: String,
    pub(super) rule: String,
    pub(super) resolved_from: Option<String>,
    pub(super) warnings: Vec<String>,
}

pub(super) fn parse_decision(value: &Value) -> Decision {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    Decision {
        allowed: value.get("allowed").and_then(Value::as_bool).unwrap_or(false),
        requested: text("requested").unwrap_or_default(),
        model: text("model"),
        reason: text("reason").unwrap_or_default(),
        rule: value.get("rule").map(rule_ref).unwrap_or_default(),
        resolved_from: text("resolvedFrom"),
        warnings: strings(value.get("warnings")),
    }
}

/// Which rule decided, in words: `organization · deny list · openai/*`.
pub(super) fn rule_ref(rule: &Value) -> String {
    let level = ui::text(rule, "level");
    match level {
        "none" => return "no rule restricts it".into(),
        "catalog" => return "the model catalog".into(),
        _ => {}
    }
    let mut out = match level {
        "organization" => "organization".to_string(),
        "deployment" => "this Gateway deployment".to_string(),
        other => format!("{other} {}", ui::text(rule, "subject")).trim().to_string(),
    };
    let list = ui::text(rule, "list");
    if !list.is_empty() {
        out.push_str(&format!(" · {list} list"));
    }
    let pattern = ui::text(rule, "pattern");
    if !pattern.is_empty() {
        out.push_str(&format!(" · {pattern}"));
    }
    out
}

/// An operator control in force (`GET /v1/policies/controls`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Control {
    pub(super) id: String,
    pub(super) kind: String,
    pub(super) level: String,
    pub(super) subject: String,
    pub(super) models: Vec<String>,
    pub(super) reason: String,
    pub(super) created_by: Option<String>,
    pub(super) expires_at: String,
}

pub(super) fn parse_controls(value: &Value) -> Vec<Control> {
    value
        .get("controls")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|entry| {
                    Some(Control {
                        id: entry.get("id")?.as_str()?.to_string(),
                        kind: ui::text(entry, "kind").to_string(),
                        level: ui::text(entry, "subjectLevel").to_string(),
                        subject: ui::text(entry, "subject").to_string(),
                        models: strings(entry.get("models")),
                        reason: ui::text(entry, "reason").to_string(),
                        created_by: entry.get("createdBy").and_then(Value::as_str).map(String::from),
                        expires_at: ui::text(entry, "expiresAt").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Someone or something to pick: a person by user id, a team by slug (and its id, which stored controls name).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Choice {
    pub(super) value: String,
    pub(super) id: String,
    pub(super) label: String,
}

pub(super) fn parse_members(value: &Value) -> Vec<Choice> {
    value
        .get("members")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|member| {
                    let id = member.get("userId")?.as_str()?.to_string();
                    let label = [ui::text(member, "name"), ui::text(member, "email")]
                        .into_iter()
                        .find(|text| !text.is_empty())
                        .unwrap_or(&id)
                        .to_string();
                    Some(Choice {
                        value: id.clone(),
                        id,
                        label,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn parse_teams(value: &Value) -> Vec<Choice> {
    value
        .get("teams")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|team| {
                    let slug = team.get("slug")?.as_str()?.to_string();
                    Some(Choice {
                        id: team.get("id").and_then(Value::as_str).unwrap_or(&slug).to_string(),
                        label: team.get("name").and_then(Value::as_str).unwrap_or(&slug).to_string(),
                        value: slug,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn permissions(value: &Value) -> Option<Vec<String>> {
    value
        .get("permissions")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
}

/* ---------------------------------------------------------------- the rule logic, as the web page's */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    All,
    Only,
    Except,
    /// Patterns no chip can show: the patterns editor.
    Advanced,
}

/// A provider wildcard (`deepseek/*`): one chip stands for the whole provider.
pub(super) fn is_provider_wildcard(pattern: &str) -> bool {
    let Some(provider) = pattern.strip_suffix("/*") else {
        return false;
    };
    let mut chars = provider.chars();
    chars.next().is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub(super) fn mode_of(allow: &[String], deny: &[String], known: &dyn Fn(&str) -> bool) -> Mode {
    let exact = |list: &[String]| {
        list.iter()
            .all(|pattern| known(pattern) || is_provider_wildcard(pattern))
    };
    match (allow.is_empty(), deny.is_empty()) {
        (true, true) => Mode::All,
        (false, true) if exact(allow) => Mode::Only,
        (true, false) if exact(deny) => Mode::Except,
        _ => Mode::Advanced,
    }
}

/// Patterns typed one per line or split by commas.
pub(super) fn lines(text: &str) -> Vec<String> {
    text.split(['\n', ','])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect()
}

/// `anthropic/claude-opus-5-5` reads as `claude-opus-5-5` on a chip.
pub(super) fn short_id(id: &str) -> &str {
    id.split_once('/').map(|(_, rest)| rest).unwrap_or(id)
}

/// What the form holds: the lists and the default it would save.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Draft {
    pub(super) allow: Vec<String>,
    pub(super) deny: Vec<String>,
    pub(super) default_model: String,
    /// Only these: an empty allow list is sent too, and allows nothing.
    pub(super) only: bool,
}

impl Draft {
    /// The rule to send: the stored one with its lists and this kind's default replaced, the rest kept.
    pub(super) fn rule(&self, view: &PolicyView, kind: Kind) -> Value {
        let mut rule: Map<String, Value> = view.rule.as_object().cloned().unwrap_or_default();
        rule.remove("allow");
        rule.remove("deny");
        let mut defaults: Map<String, Value> = rule
            .remove("defaults")
            .and_then(|defaults| defaults.as_object().cloned())
            .unwrap_or_default();
        defaults.remove(kind.key());
        if !self.default_model.is_empty() {
            defaults.insert(kind.key().into(), json!(self.default_model));
        }
        defaults.retain(|_, value| value.as_str().is_some_and(|s| !s.is_empty()));
        if !defaults.is_empty() {
            rule.insert("defaults".into(), Value::Object(defaults));
        }
        if !self.allow.is_empty() || self.only {
            rule.insert("allow".into(), json!(self.allow));
        }
        if !self.deny.is_empty() {
            rule.insert("deny".into(), json!(self.deny));
        }
        Value::Object(rule)
    }

    /// Whether it differs from what is stored, as the web page compares (order does not count).
    pub(super) fn differs(&self, view: &PolicyView, kind: Kind) -> bool {
        let sorted = |list: &[String]| {
            let mut list = list.to_vec();
            list.sort();
            list
        };
        sorted(&self.allow) != sorted(&view.allow)
            || sorted(&self.deny) != sorted(&view.deny)
            || self.default_model != view.defaults.get(kind.key()).cloned().unwrap_or_default()
    }
}

/* ---------------------------------------------------------------- time, for controls */

pub(super) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub(super) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// An ISO time in UTC: `2026-11-05T14:00:00Z`.
pub(super) fn iso_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Seconds since 1970 from an ISO time in UTC (`2026-11-05T14:00:00.000Z`).
pub(super) fn parse_iso(at: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| at.get(range)?.parse::<i64>().ok();
    let days = days_from_civil(number(0..4)?, number(5..7)?, number(8..10)?);
    Some(days * 86_400 + number(11..13)? * 3600 + number(14..16)? * 60 + number(17..19)?)
}

pub(super) fn time_left(expires_at: &str, now: i64) -> String {
    let Some(at) = parse_iso(expires_at) else {
        return String::new();
    };
    let secs = at - now;
    if secs <= 0 {
        return "expired".into();
    }
    if secs < 60 {
        return "expires in under a minute".into();
    }
    let minutes = (secs + 59) / 60;
    if minutes < 60 {
        return format!("expires in {minutes}m");
    }
    let hours = (minutes + 59) / 60;
    if hours <= 24 {
        return format!("expires in {hours}h");
    }
    format!("expires in {}d", (hours + 23) / 24)
}

/// Gateway's refusal in plain words: a 403 reads as who may do it.
pub(super) fn refusal(error: &str) -> String {
    let lower = error.to_lowercase();
    if lower.contains("override_needs_admin") || lower.contains("override a model") {
        "Only an owner or admin can override a model.".into()
    } else if lower.contains("owner or admins") || lower.contains("only the owner") || lower.contains("operator grant")
    {
        "Only organization admins can change this.".into()
    } else {
        error.to_string()
    }
}

/* ---------------------------------------------------------------- the page's state */

pub(super) struct Outcome {
    pub(super) text: SharedString,
    pub(super) error: bool,
    pub(super) notes: Vec<String>,
}

/// A changed draft needs a fresh review before it can be sent.
pub(super) fn review_ready(reviewed: &mut Option<Draft>, current: &Draft) -> bool {
    if reviewed.as_ref() == Some(current) {
        return true;
    }
    *reviewed = Some(current.clone());
    false
}

/// One level's card.
pub(super) struct Card {
    pub(super) kind: Kind,
    pub(super) view: Load<PolicyView>,
    pub(super) mode: Mode,
    pub(super) picked: Vec<String>,
    pub(super) allow: Entity<InputState>,
    pub(super) deny: Entity<InputState>,
    pub(super) default_model: String,
    /// The pattern fields need the view's lists on the next frame (setting them needs the window).
    pub(super) reset: bool,
    pub(super) saving: bool,
    pub(super) outcome: Option<Outcome>,
    /// The "+ Add model" list is open.
    pub(super) adding: bool,
    /// The default model list is open.
    pub(super) picking_default: bool,
    pub(super) reviewed: Option<Draft>,
}

#[derive(PartialEq, Eq)]
pub(super) struct CardDraft {
    pub(super) mode: Mode,
    pub(super) picked: Vec<String>,
    pub(super) allow: String,
    pub(super) deny: String,
    pub(super) default_model: String,
}
impl Card {
    pub(super) fn draft(&self, cx: &App) -> CardDraft {
        let (allow, deny) = if self.reset {
            match &self.view {
                Load::Ready(view) => (view.allow.join(", "), view.deny.join(", ")),
                _ => (String::new(), String::new()),
            }
        } else {
            (
                self.allow.read(cx).value().to_string(),
                self.deny.read(cx).value().to_string(),
            )
        };
        CardDraft {
            mode: self.mode,
            picked: self.picked.clone(),
            allow,
            deny,
            default_model: self.default_model.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ControlKind {
    Pause,
    Limit,
    Override,
}

impl ControlKind {
    pub(super) fn key(self) -> &'static str {
        match self {
            ControlKind::Pause => "pause",
            ControlKind::Limit => "limit",
            ControlKind::Override => "override",
        }
    }

    pub(super) fn icon(self) -> Lucide {
        match self {
            ControlKind::Pause => Lucide::Pause,
            ControlKind::Limit => Lucide::Gauge,
            ControlKind::Override => Lucide::Shuffle,
        }
    }

    pub(super) fn action(self) -> &'static str {
        match self {
            ControlKind::Pause => "Pause",
            ControlKind::Limit => "Limit",
            ControlKind::Override => "Override",
        }
    }

    pub(super) fn note(self) -> &'static str {
        match self {
            ControlKind::Pause => "Stops every model request it covers until it ends.",
            ControlKind::Limit => "Allows only the models you pick until it ends.",
            ControlKind::Override => "Allows the models you pick even where a policy denies them, until it ends.",
        }
    }
}

pub(super) const LEVELS: [(&str, &str); 4] = [
    ("organization", "Organization"),
    ("team", "Team"),
    ("person", "Person"),
    ("key", "Key"),
];

pub(super) const EXPIRY: [(i64, &str); 3] = [(3600, "In 1 hour"), (86_400, "In 24 hours"), (7 * 86_400, "In 7 days")];
/// `ControlForm::expiry` for "At a time…": the end is the typed one (`custom_end`).
pub(super) const AT_A_TIME: i64 = 0;
/// What a typed end time looks like, in the computer's time zone (the web's `datetime-local`, no seconds).
pub(super) const END_FORMAT: &str = "%Y-%m-%d %H:%M";

/// A typed end time, read in the computer's time zone, as seconds since 1970. None when it can't be read or is not
/// in the future (the web's `presetExpiry`). A time a clock change skips takes the earliest reading.
pub(super) fn custom_end(text: &str, now: i64) -> Option<i64> {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let text = text.trim().replacen('T', " ", 1);
    let naive = NaiveDateTime::parse_from_str(&text, END_FORMAT).ok()?;
    let at = Local.from_local_datetime(&naive).earliest()?.timestamp();
    (at > now).then_some(at)
}

/// The computer's time zone as the end field's note names it: `UTC−03:00`.
pub(super) fn local_zone() -> String {
    let offset = chrono::Local::now().offset().local_minus_utc();
    let sign = if offset < 0 { '−' } else { '+' };
    let minutes = offset.abs() / 60;
    format!("UTC{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

/// The end the form sends: now plus a preset, or the typed time.
pub(super) fn control_end(expiry: i64, typed: &str, now: i64) -> Option<i64> {
    if expiry == AT_A_TIME {
        custom_end(typed, now)
    } else {
        Some(now + expiry)
    }
}

/// The form that sets a control.
pub(super) struct ControlForm {
    pub(super) kind: ControlKind,
    pub(super) level: &'static str,
    pub(super) subject: String,
    pub(super) models: Vec<String>,
    pub(super) expiry: i64,
    pub(super) busy: bool,
    pub(super) confirm: bool,
    pub(super) problem: Option<SharedString>,
}
