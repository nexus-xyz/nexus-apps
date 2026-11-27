//! Proactivity for a native shell: the agent API's `GET` and `PUT /agent/proactivity` read strictly,
//! as the kit's `agent-proactivity.ts`, the dial's words, and "Why am I seeing this?" with its two ways out, as Nexus
//! UI's `agent-proactivity.tsx`. Nexus GPUI draws it (`desktop-gpui/src/pages/assistant_views/proactivity.rs`).
//!
//! - One dial: Quiet, Balanced (the default) and Hands-on.
//! - Quiet hours: nothing proactive is sent from `start` to `end` in the person's time zone; it waits.
//! - "Fewer like this" lowers the dial one step; at Quiet, it turns that kind off. "Turn off" turns that kind off at
//!   every step.
//!
//! The API answers 503 `proactivity_off` while it has no store for it, and an older API answers 404: both read as
//! [`Read::Off`], and the shell shows no dial and no "Why" actions.

use serde_json::{Map, Value, json};

/// `GET` reads the person's setting; `PUT` with a [`Change`] sets it and answers the setting as it is now.
pub const PROACTIVITY_PATH: &str = "/agent/proactivity";
/// The longest `PUT` body a shell sends: every kind muted, quiet hours with a long zone, and some room.
const CHANGE_BODY_MAX: usize = 320;
/// The longest time zone name the agent API takes.
const ZONE_MAX: usize = 64;
/// Quiet hours when the person first turns them on (Nexus UI's `DEFAULT_QUIET_HOURS`).
pub const DEFAULT_QUIET_HOURS: (&str, &str) = ("22:00", "07:00");

/// A step of the dial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Quiet,
    Balanced,
    HandsOn,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Quiet, Level::Balanced, Level::HandsOn];

    pub fn word(self) -> &'static str {
        match self {
            Level::Quiet => "quiet",
            Level::Balanced => "balanced",
            Level::HandsOn => "hands_on",
        }
    }

    fn of(word: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|level| level.word() == word)
    }

    /// The dial's label.
    pub fn label(self) -> &'static str {
        match self {
            Level::Quiet => "Quiet",
            Level::Balanced => "Balanced",
            Level::HandsOn => "Hands-on",
        }
    }

    /// What this step sends.
    pub fn does(self) -> &'static str {
        match self {
            Level::Quiet => "Only what you asked for and what's urgent. No brief.",
            Level::Balanced => "A morning brief, reminders, follow-ups that matter.",
            Level::HandsOn => "Also an evening wrap-up, ideas and nudges it notices.",
        }
    }

    /// About how often.
    pub fn per_day(self) -> &'static str {
        match self {
            Level::Quiet => "≈ 2 a day",
            Level::Balanced => "≈ 6 a day",
            Level::HandsOn => "≈ 12 a day",
        }
    }

    /// One step down; None at Quiet.
    fn lower(self) -> Option<Level> {
        match self {
            Level::Quiet => None,
            Level::Balanced => Some(Level::Quiet),
            Level::HandsOn => Some(Level::Balanced),
        }
    }
}

/// What a proactive message is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Watch,
    Brief,
    FollowUp,
    Reminder,
    Renewal,
    Birthday,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Watch,
        Kind::Brief,
        Kind::FollowUp,
        Kind::Reminder,
        Kind::Renewal,
        Kind::Birthday,
    ];

    pub fn word(self) -> &'static str {
        match self {
            Kind::Watch => "watch",
            Kind::Brief => "brief",
            Kind::FollowUp => "follow-up",
            Kind::Reminder => "reminder",
            Kind::Renewal => "renewal",
            Kind::Birthday => "birthday",
        }
    }

    fn of(word: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.word() == word)
    }

    /// Its name in "Turn off …" (the kit's `KIND_NAMES`).
    pub fn name(self) -> &'static str {
        match self {
            Kind::Watch => "watches",
            Kind::Brief => "the morning brief",
            Kind::FollowUp => "follow-ups",
            Kind::Reminder => "reminders",
            Kind::Renewal => "renewals",
            Kind::Birthday => "birthdays",
        }
    }
}

/// Quiet hours: `HH:MM` to `HH:MM`, 24-hour, in an IANA time zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuietHours {
    pub start: String,
    pub end: String,
    pub timezone: String,
}

/// `GET /agent/proactivity`'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub level: Level,
    /// The person chose the step; else it follows their saved `agentProactivity`.
    pub own_level: bool,
    pub muted: Vec<Kind>,
    pub quiet_hours: Option<QuietHours>,
    /// More than one push in this many minutes becomes one.
    pub batch_minutes: u64,
}

/// A `PUT`'s body: each field present is set; `Some(None)` resets it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    pub level: Option<Option<Level>>,
    pub muted: Option<Vec<Kind>>,
    pub quiet_hours: Option<Option<QuietHours>>,
}

impl Change {
    pub fn level(level: Level) -> Change {
        Change {
            level: Some(Some(level)),
            ..Change::default()
        }
    }

    pub fn muted(muted: Vec<Kind>) -> Change {
        Change {
            muted: Some(muted),
            ..Change::default()
        }
    }

    pub fn quiet_hours(hours: Option<QuietHours>) -> Change {
        Change {
            quiet_hours: Some(hours),
            ..Change::default()
        }
    }

    /// The `PUT`'s body: the fields present alone.
    pub fn body(&self) -> String {
        let mut fields = Map::new();
        if let Some(level) = &self.level {
            fields.insert("level".into(), level.map_or(Value::Null, |level| level.word().into()));
        }
        if let Some(muted) = &self.muted {
            fields.insert(
                "muted".into(),
                muted.iter().map(|kind| kind.word()).collect::<Vec<_>>().into(),
            );
        }
        if let Some(hours) = &self.quiet_hours {
            fields.insert(
                "quietHours".into(),
                hours.as_ref().map_or(
                    Value::Null,
                    |hours| json!({ "start": hours.start, "end": hours.end, "timezone": hours.timezone }),
                ),
            );
        }
        Value::Object(fields).to_string()
    }
}

/// `HH:MM`, 24-hour.
fn clock(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 5
        && bytes[2] == b':'
        && bytes[..2].iter().chain(&bytes[3..]).all(u8::is_ascii_digit)
        && matches!(bytes[0], b'0'..=b'2')
        && (bytes[0] < b'2' || bytes[1] <= b'3')
        && bytes[3] <= b'5'
}

fn zone(text: &str) -> bool {
    !text.is_empty() && text.len() <= ZONE_MAX
}

fn quiet_hours_of(value: &Value) -> Option<Option<QuietHours>> {
    if value.is_null() {
        return Some(None);
    }
    let start = value.get("start")?.as_str()?;
    let end = value.get("end")?.as_str()?;
    let timezone = value.get("timezone")?.as_str()?;
    (clock(start) && clock(end) && zone(timezone)).then(|| {
        Some(QuietHours {
            start: start.into(),
            end: end.into(),
            timezone: timezone.into(),
        })
    })
}

fn kinds_of(value: &Value) -> Option<Vec<Kind>> {
    value
        .as_array()?
        .iter()
        .map(|kind| kind.as_str().and_then(Kind::of))
        .collect()
}

/// `GET /agent/proactivity`'s body, read strictly; None when it is not one.
pub fn view_of(body: &Value) -> Option<View> {
    let level = Level::of(body.get("level")?.as_str()?)?;
    let own_level = body.get("ownLevel")?.as_bool()?;
    if !matches!(
        body.get("agentProactivity").and_then(Value::as_str),
        Some("always" | "ask_first" | "never")
    ) {
        return None;
    }
    Some(View {
        level,
        own_level,
        muted: kinds_of(body.get("muted")?)?,
        quiet_hours: quiet_hours_of(body.get("quietHours")?)?,
        batch_minutes: body.get("batchMinutes")?.as_u64()?,
    })
}

/// How a read or a change ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    View(View),
    /// The agent API has none to give: no dial, and no "Why" actions.
    Off,
    Failed(u16),
}

/// A read from its status and body: a 404 or a 503 `proactivity_off` is [`Read::Off`], a body that is no view failed.
pub fn read_of(status: u16, body: Option<&Value>) -> Read {
    let error = body.and_then(|body| body.get("error")).and_then(Value::as_str);
    if status == 404 || (status == 503 && error == Some("proactivity_off")) {
        return Read::Off;
    }
    if !(200..300).contains(&status) {
        return Read::Failed(status);
    }
    match body.and_then(view_of) {
        Some(view) => Read::View(view),
        None => Read::Failed(502),
    }
}

/// "Fewer like this": the dial one step down, or at Quiet, that kind turned off.
pub fn fewer_like_this(view: &View, kind: Kind) -> Change {
    match view.level.lower() {
        Some(lower) => Change::level(lower),
        None => turn_off(view, kind),
    }
}

/// "Turn off": that kind off at every step, with the ones already off.
pub fn turn_off(view: &View, kind: Kind) -> Change {
    Change::muted(
        Kind::ALL
            .into_iter()
            .filter(|each| *each == kind || view.muted.contains(each))
            .collect(),
    )
}

/// Quiet hours from two typed `HH:MM` times, or why not (Nexus UI's `quietHoursOf`).
pub fn quiet_hours_typed(start: &str, end: &str, timezone: &str) -> Result<QuietHours, &'static str> {
    let (start, end) = (start.trim(), end.trim());
    if !clock(start) || !clock(end) {
        return Err("Enter both times as hours and minutes.");
    }
    if start == end {
        return Err("Quiet hours start and end at different times.");
    }
    Ok(QuietHours {
        start: start.into(),
        end: end.into(),
        timezone: timezone.into(),
    })
}

/// Whether `body` is a `PUT /agent/proactivity` body as a native view writes it (Nexus UI route's `changeOf`): an object
/// with some of `level`, `muted` and `quietHours`, each of its shape, and nothing else. Anything else never reaches the
/// agent API.
pub fn change_body(body: Option<&str>) -> bool {
    let Some(body) = body.filter(|body| body.len() <= CHANGE_BODY_MAX) else {
        return false;
    };
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    !fields.is_empty()
        && fields.iter().all(|(key, value)| match key.as_str() {
            "level" => value.is_null() || value.as_str().and_then(Level::of).is_some(),
            "muted" => kinds_of(value).is_some(),
            "quietHours" => quiet_hours_of(value).is_some(),
            _ => false,
        })
}

/// "Why am I seeing this?": what the message is about, the step it came at, and the reasons in its language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Why {
    pub kind: Kind,
    pub level: Level,
    pub lines: Vec<String>,
}

/// A message's `why`, read strictly: one to four lines of text.
pub fn why_of(value: &Value) -> Option<Why> {
    let kind = Kind::of(value.get("kind")?.as_str()?)?;
    let level = Level::of(value.get("level")?.as_str()?)?;
    let lines = value.get("lines")?.as_array()?;
    if lines.is_empty() || lines.len() > 4 {
        return None;
    }
    let lines = lines
        .iter()
        .map(|line| {
            let line = line.as_str()?;
            (!line.trim().is_empty() && line.chars().count() <= 400).then(|| line.trim().to_string())
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Why { kind, level, lines })
}

/// What a change says once saved, as the dial's status.
pub fn level_text(view: &View) -> String {
    format!("Proactive is set to {}.", view.level.label())
}

/// What "Fewer like this" says once done.
pub fn fewer_text(saved: &View, kind: Kind) -> String {
    if saved.muted.contains(&kind) {
        format!("Done: no more {}.", kind.name())
    } else {
        format!("Done: {}", level_text(saved))
    }
}

/// What a change that did not land says: off at the agent API, or any other failure.
pub fn unsaved_text(read: &Read) -> &'static str {
    match read {
        Read::Off => "Proactivity is not on for Nexus here yet.",
        _ => "That was not saved. Try again in a moment.",
    }
}

/// The kinds off at every step, as the dial says them: "Off at every step: watches, birthdays.".
pub fn muted_text(muted: &[Kind]) -> Option<String> {
    (!muted.is_empty()).then(|| {
        format!(
            "Off at every step: {}.",
            muted.iter().map(|kind| kind.name()).collect::<Vec<_>>().join(", ")
        )
    })
}

/// The dial's note under quiet hours.
pub fn batch_text(view: &View) -> String {
    format!(
        "More than one push in {} minutes becomes one. Approvals and what you asked for always come.",
        view.batch_minutes
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Value {
        json!({ "level": "balanced", "ownLevel": false, "agentProactivity": "ask_first", "muted": ["birthday"],
                "quietHours": { "start": "22:00", "end": "07:00", "timezone": "America/Sao_Paulo" }, "batchMinutes": 15 })
    }

    #[test]
    fn reads_the_setting_strictly() {
        let view = view_of(&body()).expect("a view");
        assert_eq!(view.level, Level::Balanced);
        assert_eq!(view.muted, [Kind::Birthday]);
        assert_eq!(view.quiet_hours.as_ref().map(|hours| hours.end.as_str()), Some("07:00"));
        assert_eq!(view.batch_minutes, 15);
        let mut off = body();
        off["quietHours"] = Value::Null;
        assert_eq!(view_of(&off).expect("no quiet hours").quiet_hours, None);
        for (key, value) in [
            ("level", json!("loud")),
            ("muted", json!(["spam"])),
            (
                "quietHours",
                json!({ "start": "24:00", "end": "07:00", "timezone": "UTC" }),
            ),
            (
                "quietHours",
                json!({ "start": "22:00", "end": "07:60", "timezone": "UTC" }),
            ),
            ("batchMinutes", json!(-1)),
            ("agentProactivity", json!("sometimes")),
        ] {
            let mut bad = body();
            bad[key] = value;
            assert_eq!(view_of(&bad), None, "{key}");
        }
    }

    #[test]
    fn off_and_failures_read_apart() {
        assert_eq!(read_of(404, None), Read::Off);
        assert_eq!(read_of(503, Some(&json!({ "error": "proactivity_off" }))), Read::Off);
        assert_eq!(read_of(503, Some(&json!({ "error": "other" }))), Read::Failed(503));
        assert_eq!(read_of(200, Some(&json!({}))), Read::Failed(502));
        assert!(matches!(read_of(200, Some(&body())), Read::View(_)));
    }

    #[test]
    fn fewer_like_this_lowers_the_dial_then_turns_the_kind_off() {
        let mut view = view_of(&body()).expect("a view");
        view.level = Level::HandsOn;
        assert_eq!(fewer_like_this(&view, Kind::Brief), Change::level(Level::Balanced));
        view.level = Level::Quiet;
        assert_eq!(
            fewer_like_this(&view, Kind::Brief),
            Change::muted(vec![Kind::Brief, Kind::Birthday])
        );
        assert_eq!(turn_off(&view, Kind::Birthday), Change::muted(vec![Kind::Birthday]));
    }

    #[test]
    fn a_change_writes_only_its_fields_and_the_check_reads_it_back() {
        assert_eq!(Change::level(Level::HandsOn).body(), r#"{"level":"hands_on"}"#);
        assert_eq!(Change::quiet_hours(None).body(), r#"{"quietHours":null}"#);
        let hours = quiet_hours_typed("22:00", "07:00", "Europe/Lisbon").expect("hours");
        let set = Change::quiet_hours(Some(hours)).body();
        assert_eq!(
            set,
            r#"{"quietHours":{"start":"22:00","end":"07:00","timezone":"Europe/Lisbon"}}"#
        );
        let every = Change::muted(Kind::ALL.to_vec()).body();
        for body in [&set, &every, r#"{"level":null}"#, r#"{"muted":[]}"#] {
            assert!(change_body(Some(body)), "{body}");
        }
        for body in [
            "{}",
            r#"{"level":"loud"}"#,
            r#"{"muted":["spam"]}"#,
            r#"{"quietHours":{"start":"9:00","end":"07:00","timezone":"UTC"}}"#,
            r#"{"level":"quiet","extra":1}"#,
            "[]",
        ] {
            assert!(!change_body(Some(body)), "{body}");
        }
        assert!(!change_body(None));
    }

    #[test]
    fn typed_quiet_hours_say_why_not() {
        assert_eq!(
            quiet_hours_typed("22", "07:00", "UTC"),
            Err("Enter both times as hours and minutes.")
        );
        assert_eq!(
            quiet_hours_typed("07:00", "07:00", "UTC"),
            Err("Quiet hours start and end at different times.")
        );
    }

    #[test]
    fn why_reads_one_to_four_lines() {
        let why = why_of(&json!({ "kind": "brief", "level": "balanced", "lines": [" You asked for it. "] }));
        assert_eq!(
            why,
            Some(Why {
                kind: Kind::Brief,
                level: Level::Balanced,
                lines: vec!["You asked for it.".into()]
            })
        );
        assert_eq!(
            why_of(&json!({ "kind": "brief", "level": "balanced", "lines": [] })),
            None
        );
        assert_eq!(
            why_of(&json!({ "kind": "brief", "level": "balanced", "lines": [" "] })),
            None
        );
        assert_eq!(
            why_of(&json!({ "kind": "spam", "level": "balanced", "lines": ["a"] })),
            None
        );
    }

    #[test]
    fn the_words_follow_the_web() {
        let view = view_of(&body()).expect("a view");
        assert_eq!(level_text(&view), "Proactive is set to Balanced.");
        assert_eq!(fewer_text(&view, Kind::Birthday), "Done: no more birthdays.");
        assert_eq!(fewer_text(&view, Kind::Brief), "Done: Proactive is set to Balanced.");
        assert_eq!(
            muted_text(&view.muted).as_deref(),
            Some("Off at every step: birthdays.")
        );
        assert_eq!(
            batch_text(&view),
            "More than one push in 15 minutes becomes one. Approvals and what you asked for always come."
        );
        assert_eq!(unsaved_text(&Read::Off), "Proactivity is not on for Nexus here yet.");
    }
}
