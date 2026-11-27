//! The Assistant's watches for a native shell: the agent API's `/agent/watches` routes read
//! strictly, as the kit's `agent-watches.ts`, with the board's words. Nexus GPUI draws them
//! (`desktop-gpui/src/pages/assistant_views/watches.rs`, and the proposal card in the thread).
//!
//! - A watch is a standing instruction: a condition, how often it is checked, a limit, and where it tells you. It
//!   tells and asks; it never acts on its own.
//! - A watch starts by asking. The chat answers a notice (`topic: "watches"`, `state: "proposed"`) that carries the
//!   proposal ([`notice_proposal`]). Nothing is kept until Keep: `POST /agent/watches` with [`Proposal::keep_body`].
//! - The API answers 503 `watches_off` while it keeps none, and an older API 404: both read as [`Read::Off`].

use chrono::{DateTime, Local, Utc};
use serde_json::{Map, Value, json};

use crate::agent::Message;

pub const WATCHES_PATH: &str = "/agent/watches";
pub const MAX_DAYS: u32 = 30;
const BODY_MAX: usize = 4_096;

/// `/agent/watches/{id}` for a watch id (`w` and its number, 1 to 9999), None for any other id.
pub fn watch_path(id: &str) -> Option<String> {
    let digits = id.strip_prefix('w')?;
    let valid =
        (1..=4).contains(&digits.len()) && !digits.starts_with('0') && digits.bytes().all(|b| b.is_ascii_digit());
    valid.then(|| format!("{WATCHES_PATH}/{id}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Pull {
        repo: String,
        number: u64,
    },
    Issue {
        repo: String,
        number: u64,
    },
    /// The person's AI spend today over an amount, a decimal string.
    Spend {
        over: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    Often,
    Hourly,
    Daily,
}

impl Cadence {
    pub const ALL: [Cadence; 3] = [Cadence::Often, Cadence::Hourly, Cadence::Daily];

    pub fn word(self) -> &'static str {
        match self {
            Cadence::Often => "often",
            Cadence::Hourly => "hourly",
            Cadence::Daily => "daily",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Cadence::Often => "As it happens",
            Cadence::Hourly => "Every hour",
            Cadence::Daily => "Once a day",
        }
    }

    fn of(word: &str) -> Option<Cadence> {
        Cadence::ALL.into_iter().find(|each| each.word() == word)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Push,
    Thread,
}

impl Channel {
    pub const ALL: [Channel; 2] = [Channel::Push, Channel::Thread];

    pub fn word(self) -> &'static str {
        match self {
            Channel::Push => "push",
            Channel::Thread => "thread",
        }
    }
}

/// The channels, read strictly: a non-empty list of known ones, in their order, each once.
fn channels_of(value: &Value) -> Option<Vec<Channel>> {
    let list = value.as_array()?;
    if list.is_empty()
        || list
            .iter()
            .any(|each| !matches!(each.as_str(), Some("push" | "thread")))
    {
        return None;
    }
    Some(
        Channel::ALL
            .into_iter()
            .filter(|channel| list.iter().any(|each| each.as_str() == Some(channel.word())))
            .collect(),
    )
}

/// One time a watch fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firing {
    pub at: String,
    pub from: Option<String>,
    pub to: String,
    pub told: Vec<Channel>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Watch {
    pub id: String,
    pub target: Target,
    pub title: Option<String>,
    pub every: bool,
    pub cadence: Cadence,
    pub until: String,
    pub tells: Vec<Channel>,
    pub quiet_hours: bool,
    pub paused: bool,
    pub since: String,
    pub said: String,
    pub link: String,
    pub last_fired: Option<Firing>,
    /// When it fired for the last time (ISO 8601): it no longer checks, and its history stays to read.
    pub ended: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Detail {
    pub watch: Watch,
    pub history: Vec<Firing>,
}

/// What the chat proposes: the watch Keep would keep.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub target: Target,
    pub every: bool,
    pub said: String,
    pub cadence: Cadence,
    pub days: u32,
    pub tells: Vec<Channel>,
    pub quiet_hours: bool,
}

fn iso(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    DateTime::parse_from_rfc3339(text).ok()?;
    Some(text.to_string())
}

fn text(value: &Value, max: usize) -> Option<String> {
    let text = value.as_str()?;
    (!text.trim().is_empty() && text.len() <= max).then(|| text.to_string())
}

fn target_of(value: &Value) -> Option<Target> {
    let kind = value.get("kind")?.as_str()?;
    if kind == "spend" {
        let over = value.get("over")?.as_str()?;
        let (whole, fraction) = over.split_once('.').unwrap_or((over, ""));
        let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
        let valid = (1..=9).contains(&whole.len())
            && digits(whole)
            && fraction.len() <= 6
            && digits(fraction)
            && (over.find('.').is_none() || !fraction.is_empty());
        let zero = whole.bytes().chain(fraction.bytes()).all(|b| b == b'0');
        return (valid && !zero).then(|| Target::Spend { over: over.to_string() });
    }
    let repo = value.get("repo")?.as_str()?;
    let number = value.get("number")?.as_u64()?;
    let (owner, name) = repo.split_once('/')?;
    let part = |part: &str| {
        (1..=100).contains(&part.len())
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    };
    if !part(owner) || !part(name) || !(1..=9_999_999_999).contains(&number) {
        return None;
    }
    match kind {
        "pull" => Some(Target::Pull {
            repo: repo.into(),
            number,
        }),
        "issue" => Some(Target::Issue {
            repo: repo.into(),
            number,
        }),
        _ => None,
    }
}

fn firing_of(value: &Value) -> Option<Firing> {
    let told = value.get("told")?;
    let told = if told.as_array().is_some_and(|list| list.is_empty()) {
        Vec::new()
    } else {
        channels_of(told)?
    };
    let from = match value.get("from") {
        None | Some(Value::Null) => None,
        Some(from) => Some(text(from, 40)?),
    };
    Some(Firing {
        at: iso(value.get("at")?)?,
        from,
        to: text(value.get("to")?, 40)?,
        told,
    })
}

/// A watch, read strictly.
pub fn watch_of(value: &Value) -> Option<Watch> {
    let id = value.get("id")?.as_str()?;
    watch_path(id)?;
    let title = match value.get("title") {
        None | Some(Value::Null) => None,
        Some(title) => Some(text(title, 200)?.trim().to_string()),
    };
    let link = value.get("link")?.as_str()?;
    if !(link.starts_with("https://") || link.starts_with("http://")) {
        return None;
    }
    let last_fired = match value.get("lastFired")? {
        Value::Null => None,
        fired => Some(firing_of(fired)?),
    };
    Some(Watch {
        id: id.into(),
        target: target_of(value.get("target")?)?,
        title,
        every: value.get("every")?.as_bool()?,
        cadence: Cadence::of(value.get("cadence")?.as_str()?)?,
        until: iso(value.get("until")?)?,
        tells: channels_of(value.get("tells")?)?,
        quiet_hours: value.get("quietHours")?.as_bool()?,
        paused: value.get("paused")?.as_bool()?,
        since: iso(value.get("since")?)?,
        said: text(value.get("said")?, 8_000)?,
        link: link.into(),
        last_fired,
        ended: match value.get("ended") {
            None | Some(Value::Null) => None,
            Some(ended) => Some(iso(ended)?),
        },
    })
}

/// How a call ended. `Off`: the API keeps no watches.
#[derive(Debug, Clone, PartialEq)]
pub enum Read<T> {
    Ok(T),
    Off,
    Failed { status: u16, error: Option<String> },
}

fn read_with<T>(status: u16, body: Option<&Value>, read: impl FnOnce(&Value) -> Option<T>) -> Read<T> {
    let error = body.and_then(|body| body.get("error")).and_then(Value::as_str);
    if (status == 404 && matches!(error, None | Some("not_found"))) || (status == 503 && error == Some("watches_off")) {
        return Read::Off;
    }
    if !(200..300).contains(&status) {
        return Read::Failed {
            status,
            error: error.map(String::from),
        };
    }
    match body.and_then(read) {
        Some(value) => Read::Ok(value),
        None => Read::Failed {
            status: 502,
            error: None,
        },
    }
}

/// `GET /agent/watches`: the watches (one that does not read is left out) and the most a person keeps.
pub fn list_read(status: u16, body: Option<&Value>) -> Read<(Vec<Watch>, u64)> {
    read_with(status, body, |body| {
        let max = body.get("max")?.as_u64()?;
        Some((
            body.get("watches")?.as_array()?.iter().filter_map(watch_of).collect(),
            max,
        ))
    })
}

/// `GET /agent/watches/{id}`: the watch and every firing, newest first.
pub fn detail_read(status: u16, body: Option<&Value>) -> Read<Detail> {
    read_with(status, body, |body| {
        let watch = watch_of(body.get("watch")?)?;
        Some(Detail {
            watch,
            history: body.get("history")?.as_array()?.iter().filter_map(firing_of).collect(),
        })
    })
}

/// `POST`'s and `PATCH`'s answer: `{ watch }`.
pub fn watch_read(status: u16, body: Option<&Value>) -> Read<Watch> {
    read_with(status, body, |body| watch_of(body.get("watch")?))
}

/// `DELETE`'s answer: `{ deleted }`.
pub fn deleted_read(status: u16, body: Option<&Value>) -> Read<()> {
    read_with(status, body, |body| body.get("deleted")?.as_str().map(|_| ()))
}

/// A proposal, read strictly.
pub fn proposal_of(value: &Value) -> Option<Proposal> {
    let days = value.get("days")?.as_u64()?;
    if !(1..=MAX_DAYS as u64).contains(&days) {
        return None;
    }
    // Push is how Nexus tells the person: the thread alone would tell no one.
    let tells = channels_of(value.get("tells")?).filter(|tells| tells.contains(&Channel::Push))?;
    Some(Proposal {
        target: target_of(value.get("target")?)?,
        every: value.get("every")?.as_bool()?,
        said: text(value.get("said")?, 8_000)?,
        cadence: Cadence::of(value.get("cadence")?.as_str()?)?,
        days: days as u32,
        tells,
        quiet_hours: value.get("quietHours")?.as_bool()?,
    })
}

/// The proposal a chat message carries: a notice about watches in the `proposed` state.
pub fn notice_proposal(message: &Message) -> Option<Proposal> {
    let Message::Notice {
        topic, state, watch, ..
    } = message
    else {
        return None;
    };
    (topic == "watches" && state == "proposed")
        .then(|| proposal_of(watch))
        .flatten()
}

impl Proposal {
    /// `POST /agent/watches`'s body: the proposal as the person kept it.
    pub fn keep_body(&self) -> Option<String> {
        let body = json!({
            "target": target_json(&self.target),
            "every": self.every,
            "said": self.said,
            "cadence": self.cadence.word(),
            "days": self.days,
            "tells": self.tells.iter().map(|channel| channel.word()).collect::<Vec<_>>(),
            "quietHours": self.quiet_hours,
        })
        .to_string();
        (body.len() <= BODY_MAX).then_some(body)
    }
}

fn target_json(target: &Target) -> Value {
    match target {
        Target::Pull { repo, number } => json!({ "kind": "pull", "repo": repo, "number": number }),
        Target::Issue { repo, number } => json!({ "kind": "issue", "repo": repo, "number": number }),
        Target::Spend { over } => json!({ "kind": "spend", "over": over }),
    }
}

/// `PATCH /agent/watches/{id}`'s body: each field present is set. `days` counts from when it was kept.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Change {
    pub cadence: Option<Cadence>,
    pub days: Option<u32>,
    pub every: Option<bool>,
    pub tells: Option<Vec<Channel>>,
    pub quiet_hours: Option<bool>,
    pub paused: Option<bool>,
}

impl Change {
    /// The body, or None when it changes nothing or a field is out of range.
    pub fn body(&self) -> Option<String> {
        let mut fields = Map::new();
        if let Some(cadence) = self.cadence {
            fields.insert("cadence".into(), json!(cadence.word()));
        }
        if let Some(days) = self.days {
            if !(1..=MAX_DAYS).contains(&days) {
                return None;
            }
            fields.insert("days".into(), json!(days));
        }
        if let Some(every) = self.every {
            fields.insert("every".into(), json!(every));
        }
        if let Some(tells) = &self.tells {
            if !tells.contains(&Channel::Push) {
                return None;
            }
            fields.insert(
                "tells".into(),
                json!(tells.iter().map(|channel| channel.word()).collect::<Vec<_>>()),
            );
        }
        if let Some(quiet) = self.quiet_hours {
            fields.insert("quietHours".into(), json!(quiet));
        }
        if let Some(paused) = self.paused {
            fields.insert("paused".into(), json!(paused));
        }
        (!fields.is_empty()).then(|| Value::Object(fields).to_string())
    }
}

// The board's words.

/// The list's title line: "PR #482", "Issue #12", or "Spend over $50".
pub fn name(target: &Target) -> String {
    match target {
        Target::Spend { over } => format!("Spend over ${over}"),
        Target::Pull { number, .. } => format!("PR #{number}"),
        Target::Issue { number, .. } => format!("Issue #{number}"),
    }
}

/// The repository's short name, as "· cortex-sim" after the name; none for spend.
pub fn place(target: &Target) -> Option<&str> {
    match target {
        Target::Pull { repo, .. } | Target::Issue { repo, .. } => repo.rsplit('/').next(),
        Target::Spend { .. } => None,
    }
}

/// "When": the condition it waits for.
pub fn when(target: &Target, every: bool) -> String {
    match (target, every) {
        (Target::Spend { over }, _) => format!(
            "Your AI spend today passes ${over}{}",
            if every { ", once a day" } else { "" }
        ),
        (Target::Pull { .. }, true) => "Each change, until it merges".into(),
        (Target::Pull { .. }, false) => "It merges or closes".into(),
        (Target::Issue { .. }, true) => "Each change of state".into(),
        (Target::Issue { .. }, false) => "It changes state".into(),
    }
}

/// "Checks": how often, and what it reads.
pub fn checks(target: &Target, cadence: Cadence) -> String {
    format!(
        "{} · {}",
        cadence.label(),
        if matches!(target, Target::Spend { .. }) {
            "Nexus usage"
        } else {
            "GitHub"
        }
    )
}

/// The days left at `now` (epoch milliseconds), rounded up, never below 0.
fn days_left(until: &str, now: i64) -> i64 {
    let end = DateTime::parse_from_rfc3339(until)
        .map(|at| at.timestamp_millis())
        .unwrap_or(now);
    ((end - now).max(0) + 86_399_999) / 86_400_000
}

/// "Until": it fires once, or its end date.
pub fn until_text(target: &Target, every: bool, until: &str, now: i64) -> String {
    let days = days_left(until, now);
    let left = format!("{days} {}", if days == 1 { "day" } else { "days" });
    match (every, target) {
        (true, Target::Pull { .. }) => format!("It merges, or {left}"),
        (true, _) => left,
        (false, _) => format!("It fires once, or {left}"),
    }
}

/// "Tells you": "Push and the Assistant's thread".
pub fn tells_text(tells: &[Channel]) -> String {
    let line = tells
        .iter()
        .map(|channel| {
            if *channel == Channel::Push {
                "Push"
            } else {
                "the Assistant’s thread"
            }
        })
        .collect::<Vec<_>>()
        .join(" and ");
    let mut chars = line.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// The line under a watch in the list: its condition, short; "Paused" when it is.
pub fn list_line(watch: &Watch) -> String {
    if watch.ended.is_some() {
        "Ended".into()
    } else if watch.paused {
        "Paused".into()
    } else {
        when(&watch.target, watch.every)
    }
}

/// What a firing saw, as the history says it.
pub fn firing_what(target: &Target, firing: &Firing) -> String {
    if matches!(target, Target::Spend { .. }) {
        return format!("Spend reached ${}", firing.to);
    }
    match firing.to.as_str() {
        "merged" => "Merged".into(),
        "closed" => "Closed".into(),
        "open" => "Reopened".into(),
        other => other.into(),
    }
}

/// Where a firing told you: "Told you · push" or "Held: nothing was sent".
pub fn firing_told(firing: &Firing) -> String {
    if firing.told.is_empty() {
        return "Held: nothing was sent".into();
    }
    format!(
        "Told you · {}",
        firing
            .told
            .iter()
            .map(|channel| channel.word())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// A short day and time on this computer's clock: "Nov 14 · 9:41 AM".
pub fn when_text(at: &str) -> String {
    DateTime::parse_from_rfc3339(at)
        .map(|at| at.with_timezone(&Local).format("%b %-d · %-I:%M %p").to_string())
        .unwrap_or_default()
}

/// A short day: "Nov 14".
pub fn day_text(at: &str) -> String {
    DateTime::parse_from_rfc3339(at)
        .map(|at| at.with_timezone(&Local).format("%b %-d").to_string())
        .unwrap_or_default()
}

/// The days from `since` to `until`, rounded, within 1 and 30.
pub fn days_between(since: &str, until: &str) -> u32 {
    let at = |text: &str| {
        DateTime::parse_from_rfc3339(text)
            .map(|at| at.with_timezone(&Utc).timestamp_millis())
            .unwrap_or(0)
    };
    (((at(until) - at(since)) as f64 / 86_400_000.0).round() as i64).clamp(1, MAX_DAYS as i64) as u32
}

/// The API's error codes, in the app's words.
pub fn error_text(error: Option<&str>) -> &'static str {
    match error {
        Some("watch_already") => "That already happened, so there is nothing to watch.",
        Some("watches_full") => "You keep the most watches already. Delete one first.",
        Some("link_needed") => {
            "Link Nexus in Slack first: a watch checks and tells you through that link, and it stops only through it."
        }
        Some("tools_needed") => "Turn on Nexus's tools where you linked it, so it can read GitHub as you.",
        Some("insights_needed") => "Turn on spend answers where you linked Nexus, so it can read your usage.",
        Some("push_needed") => "Push is how Nexus tells you, so it stays on. The thread can be added to it.",
        Some("watch_unreadable") => "Nexus could not read that now. Try again in a moment.",
        Some("watch_gone") => "That watch is gone.",
        _ => "Nexus could not do that now. Try again in a moment.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull() -> Value {
        json!({
            "id": "w1", "target": { "kind": "pull", "repo": "acme/cortex-sim", "number": 482 }, "title": "Decoder",
            "every": true, "cadence": "hourly", "until": "2026-11-24T12:00:00.000Z", "tells": ["thread", "push"], "quietHours": true,
            "paused": false, "since": "2026-11-10T12:00:00.000Z", "said": "watch it", "link": "https://github.com/acme/cortex-sim/pull/482",
            "lastFired": { "at": "2026-11-12T10:00:00.000Z", "from": "open", "to": "merged", "told": ["push"] }
        })
    }

    #[test]
    fn reads_a_watch_strictly_and_orders_its_channels() {
        let watch = watch_of(&pull()).expect("a watch");
        assert_eq!(watch.tells, [Channel::Push, Channel::Thread]);
        assert_eq!(watch.last_fired.as_ref().expect("a firing").to, "merged");
        for (field, bad) in [
            ("id", json!("x1")),
            ("cadence", json!("never")),
            ("tells", json!([])),
            ("link", json!("ftp://x")),
            ("until", json!("soon")),
            ("paused", json!("no")),
        ] {
            let mut value = pull();
            value[field] = bad;
            assert!(watch_of(&value).is_none(), "{field}");
        }
        let mut ended = pull();
        ended["ended"] = json!("2026-11-12T10:00:00.000Z");
        let ended = watch_of(&ended).expect("a watch");
        assert_eq!(list_line(&ended), "Ended");
        let mut spend = pull();
        spend["target"] = json!({ "kind": "spend", "over": "0.00" });
        assert!(watch_of(&spend).is_none(), "zero spend");
        spend["target"] = json!({ "kind": "spend", "over": "50.5" });
        assert_eq!(
            watch_of(&spend).expect("a watch").target,
            Target::Spend { over: "50.5".into() }
        );
    }

    #[test]
    fn off_for_404_and_watches_off_and_a_failure_otherwise() {
        let list = json!({ "watches": [pull(), { "id": "bad" }], "max": 10 });
        assert_eq!(
            list_read(200, Some(&list)),
            Read::Ok((vec![watch_of(&pull()).expect("a watch")], 10))
        );
        assert_eq!(list_read(404, None), Read::Off);
        assert_eq!(list_read(503, Some(&json!({ "error": "watches_off" }))), Read::Off);
        assert_eq!(
            list_read(503, Some(&json!({ "error": "x" }))),
            Read::Failed {
                status: 503,
                error: Some("x".into())
            }
        );
        assert_eq!(
            list_read(200, Some(&json!({}))),
            Read::Failed {
                status: 502,
                error: None
            }
        );
        assert!(
            matches!(detail_read(200, Some(&json!({ "watch": pull(), "history": [{ "x": 1 }] }))), Read::Ok(Detail { history, .. }) if history.is_empty())
        );
    }

    #[test]
    fn keeps_what_the_card_proposed_and_changes_only_what_is_set() {
        let proposal = proposal_of(&json!({ "target": { "kind": "issue", "repo": "a/b", "number": 3 }, "every": false, "said": "x", "cadence": "daily", "days": 7, "tells": ["push"], "quietHours": false })).expect("a proposal");
        assert_eq!(
            serde_json::from_str::<Value>(&proposal.keep_body().expect("a keep body")).expect("json")["days"],
            7
        );
        assert!(proposal_of(&json!({ "target": { "kind": "issue", "repo": "a/b", "number": 3 }, "every": false, "said": "x", "cadence": "daily", "days": 31, "tells": ["push"], "quietHours": false })).is_none());
        assert!(proposal_of(&json!({ "target": { "kind": "issue", "repo": "a/b", "number": 3 }, "every": false, "said": "x", "cadence": "daily", "days": 7, "tells": ["thread"], "quietHours": false })).is_none(), "thread alone");
        assert_eq!(Change::default().body(), None);
        assert_eq!(
            Change {
                days: Some(0),
                ..Change::default()
            }
            .body(),
            None
        );
        for tells in [vec![], vec![Channel::Thread]] {
            assert_eq!(
                Change {
                    tells: Some(tells),
                    ..Change::default()
                }
                .body(),
                None,
                "push stays on"
            );
        }
        assert_eq!(
            Change {
                paused: Some(true),
                cadence: Some(Cadence::Daily),
                ..Change::default()
            }
            .body()
            .expect("a change body"),
            r#"{"cadence":"daily","paused":true}"#
        );
    }

    #[test]
    fn says_the_boards_words() {
        let watch = watch_of(&pull()).expect("a watch");
        assert_eq!(
            (name(&watch.target), place(&watch.target)),
            ("PR #482".to_string(), Some("cortex-sim"))
        );
        assert_eq!(when(&watch.target, true), "Each change, until it merges");
        assert_eq!(checks(&watch.target, Cadence::Hourly), "Every hour · GitHub");
        let now = DateTime::parse_from_rfc3339("2026-11-20T12:00:00Z")
            .expect("a time")
            .timestamp_millis();
        assert_eq!(
            until_text(&watch.target, true, &watch.until, now),
            "It merges, or 4 days"
        );
        assert_eq!(
            until_text(&watch.target, false, &watch.until, now),
            "It fires once, or 4 days"
        );
        assert_eq!(tells_text(&watch.tells), "Push and the Assistant’s thread");
        let firing = watch.last_fired.expect("a firing");
        assert_eq!(
            (firing_what(&watch.target, &firing), firing_told(&firing)),
            ("Merged".to_string(), "Told you · push".to_string())
        );
        assert_eq!(days_between(&watch.since, &watch.until), 14);
        assert_eq!(watch_path("w12").as_deref(), Some("/agent/watches/w12"));
        assert_eq!(
            (watch_path("w0"), watch_path("w12345"), watch_path("../w1")),
            (None, None, None)
        );
    }
}
