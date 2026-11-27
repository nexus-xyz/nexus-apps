//! The Assistant's morning brief for a native shell: the agent API's `GET /agent/brief/today` read
//! strictly, as the kit's `agent-brief.ts`. The brief is the one Slack's DM got that morning. A shell shows it at the top
//! of today's thread: the greeting with the count, up to three lines, then the errands that need the person in one
//! line. The agent API writes the words. Nexus GPUI draws it
//! (`desktop-gpui/src/pages/agent/brief.rs`).
//!
//! The API answers 503 `brief_off` while the brief is off for it, and an older API answers 404: both read as
//! [`BriefRead::Off`], and the shell shows no brief.

use serde_json::Value;

use crate::agent_proactivity::{Why, why_of};

pub use crate::agent::BRIEF_PATH;
/// The most lines a brief has ("three lines, then the errands in one line").
pub const BRIEF_LINES: usize = 3;
const MAX_TEXT: usize = 400;

/// Today's brief, as a shell draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brief {
    /// The brief's local day, `YYYY-MM-DD`.
    pub day: String,
    /// When it was composed (ISO 8601).
    pub at: String,
    /// `Morning, Maya. Three things today:`
    pub head: String,
    pub lines: Vec<String>,
    /// The errands that need the person, in one line.
    pub errands: Option<String>,
    /// "Why am I seeing this?" An older API, or one without proactivity, gives none.
    pub why: Option<Why>,
}

/// How a read of the brief ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BriefRead {
    Brief(Box<Brief>),
    /// No brief today.
    None,
    /// The agent API has none to give.
    Off,
    Failed(u16),
}

fn text(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    (!text.trim().is_empty() && text.encode_utf16().count() <= MAX_TEXT).then(|| text.trim().to_string())
}

fn day(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(at, byte)| match at {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

/// An ISO 8601 time as the agent API writes it: `2026-11-18T07:58:00.000Z`, or with an offset.
fn time(text: &str) -> bool {
    let bytes = text.as_bytes();
    (14..=40).contains(&bytes.len())
        && text.is_char_boundary(10)
        && day(&text[..10])
        && bytes[10] == b'T'
        && bytes[11..13].iter().all(u8::is_ascii_digit)
        && bytes[13] == b':'
}

/// `GET /agent/brief/today`'s body, read strictly: `Some(Some(brief))`, `Some(None)` for none today, or None when it is
/// neither. A brief with a line that is not text, or more than three lines, is not one.
pub fn brief_of(body: &Value) -> Option<Option<Brief>> {
    let brief = body.get("brief")?;
    if brief.is_null() {
        return Some(None);
    }
    let day_text = brief.get("day")?.as_str().filter(|text| day(text))?;
    let at = brief.get("at")?.as_str().filter(|at| time(at))?;
    let head = text(brief.get("head")?)?;
    let lines = brief.get("lines")?.as_array()?;
    if lines.len() > BRIEF_LINES {
        return None;
    }
    let lines = lines.iter().map(text).collect::<Option<Vec<_>>>()?;
    let errands = match brief.get("errands") {
        None => None,
        Some(errands) => Some(text(errands)?),
    };
    // A reason that is not one is left out: the brief still shows.
    let why = brief.get("why").and_then(why_of);
    Some(Some(Brief {
        day: day_text.into(),
        at: at.into(),
        head,
        lines,
        errands,
        why,
    }))
}

/// A read of the brief, from its status and body: a 404 or a 503 `brief_off` is [`BriefRead::Off`], and a body that is
/// no brief failed.
pub fn brief_read(status: u16, body: Option<&Value>) -> BriefRead {
    let error = body.and_then(|body| body.get("error")).and_then(Value::as_str);
    if status == 404 || (status == 503 && error == Some("brief_off")) {
        return BriefRead::Off;
    }
    if !(200..300).contains(&status) {
        return BriefRead::Failed(status);
    }
    match body.and_then(brief_of) {
        Some(Some(brief)) => BriefRead::Brief(Box::new(brief)),
        Some(None) => BriefRead::None,
        None => BriefRead::Failed(502),
    }
}

/// The brief to show from a read: none while it is off, with none today, or on any failure. The chat never waits on it.
pub fn brief_to_show(status: u16, body: Option<&Value>) -> Option<Brief> {
    match brief_read(status, body) {
        BriefRead::Brief(brief) => Some(*brief),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_proactivity::{Kind, Level};
    use serde_json::json;

    fn body() -> Value {
        json!({ "brief": { "day": "2026-11-18", "at": "2026-11-18T07:58:00Z", "head": "Morning, Maya. Two things today:",
                "lines": ["8:40 to JFK tomorrow.", "Ana's birthday is Saturday."], "errands": "Dinner Friday needs you.",
                "why": { "kind": "brief", "level": "balanced", "lines": ["You get a brief each morning at Balanced."] } } })
    }

    #[test]
    fn reads_todays_brief_strictly() {
        let brief = brief_to_show(200, Some(&body())).expect("a brief");
        assert_eq!(brief.head, "Morning, Maya. Two things today:");
        assert_eq!(brief.lines.len(), 2);
        assert_eq!(brief.errands.as_deref(), Some("Dinner Friday needs you."));
        let why = brief.why.expect("why");
        assert_eq!((why.kind, why.level), (Kind::Brief, Level::Balanced));

        let mut no_why = body();
        no_why["brief"]["why"] = json!({ "kind": "brief" });
        assert_eq!(brief_to_show(200, Some(&no_why)).expect("still a brief").why, None);

        for (key, value) in [
            ("day", json!("18 Nov")),
            ("at", json!("now")),
            ("head", json!(" ")),
            ("lines", json!(["a", "b", "c", "d"])),
            ("lines", json!(["a", 1])),
            ("errands", json!("")),
        ] {
            let mut bad = body();
            bad["brief"][key] = value;
            assert_eq!(brief_read(200, Some(&bad)), BriefRead::Failed(502), "{key}");
        }
    }

    #[test]
    fn none_off_and_failures_show_no_brief() {
        assert_eq!(brief_read(200, Some(&json!({ "brief": null }))), BriefRead::None);
        assert_eq!(brief_read(404, None), BriefRead::Off);
        assert_eq!(brief_read(503, Some(&json!({ "error": "brief_off" }))), BriefRead::Off);
        assert_eq!(brief_read(500, None), BriefRead::Failed(500));
        assert_eq!(brief_to_show(503, Some(&json!({ "error": "brief_off" }))), None);
        assert_eq!(brief_to_show(200, Some(&json!({}))), None);
    }
}
