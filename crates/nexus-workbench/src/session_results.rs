//! Reading a batch of results back: the escaped frames [`crate::linked::results`] writes into a parent's log, as the
//! reports the parent's transcript shows, one card per linked session.
//!
//! The tests are in `session_results/tests.rs`. A partial or malformed batch reads as no batch,
//! so the app shows the original text and loses no report. Every value is decoded as text, never as markup.
//!
//! No I/O and no screen code here.

use crate::event::SessionAgent;
use crate::linked::results::{RESULTS_HEADER, ResultStatus};

#[cfg(test)]
mod tests;

/// One linked session's report in a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadResult {
    pub session_id: String,
    pub title: String,
    pub agent: SessionAgent,
    pub status: ResultStatus,
    pub text: String,
}

impl ReadResult {
    /// How its status reads on its card.
    pub fn status_label(&self) -> &'static str {
        match self.status {
            ResultStatus::Completed => "Completed",
            ResultStatus::Failed => "Failed",
            ResultStatus::Stopped => "Stopped",
        }
    }
}

/// Whether `text` opens with the header of a batch of results (`isResultBatch`).
pub fn is_result_batch(text: &str) -> bool {
    text.strip_prefix(RESULTS_HEADER)
        .is_some_and(|rest| rest.starts_with('\n'))
}

/// The reports of a batch (`sessionResults`), or None when `text` is not a whole, well-formed batch.
pub fn session_results(text: &str) -> Option<Vec<ReadResult>> {
    if !is_result_batch(text) {
        return None;
    }
    let mut rest = text[RESULTS_HEADER.len()..].trim();
    let mut results = Vec::new();
    while !rest.is_empty() {
        let (result, after) = frame(rest)?;
        results.push(result);
        rest = after.trim_start();
    }
    (!results.is_empty()).then_some(results)
}

/// The results a message from session `from` brings into session `on_screen` (`ownResults`), or None when it is not
/// a batch of results there. `parent_of` answers for a known session with its parent, and None for one it does not
/// know (deleted, or not read yet).
///
/// The core logs a batch in the parent's own log with `from` = its first child. A message from a known session that is
/// not a child of `on_screen` reads as a plain message with all its text. Of the frames, those of a known session that
/// is not a child of `on_screen` drop; a deleted session's frames stay, so a real batch stays a batch after its child
/// goes.
pub fn own_results<'a>(
    results: Option<Vec<ReadResult>>,
    from: &str,
    on_screen: Option<&str>,
    parent_of: impl Fn(&str) -> Option<Option<&'a str>>,
) -> Option<Vec<ReadResult>> {
    let on_screen = on_screen?;
    let strange = |id: &str| parent_of(id).is_some_and(|parent| parent != Some(on_screen));
    let results = results?;
    if strange(from) {
        return None;
    }
    let own: Vec<ReadResult> = results
        .into_iter()
        .filter(|result| !strange(&result.session_id))
        .collect();
    (!own.is_empty()).then_some(own)
}

/// One frame at the start of `text`, and what follows it.
fn frame(text: &str) -> Option<(ReadResult, &str)> {
    let rest = text.strip_prefix("<nexus-session-result session=\"")?;
    let (session_id, rest) = value(rest)?;
    let rest = rest.strip_prefix(" title=\"")?;
    let (title, rest) = value(rest)?;
    let rest = rest.strip_prefix(" agent=\"")?;
    let (agent, rest) = value(rest)?;
    // Every agent a frame is written for: the orchestra starts Nexus children too (`orchestra::tools::child_agent`).
    let agent = SessionAgent::parse(agent)?;
    let rest = rest.strip_prefix(" status=\"")?;
    let (status, rest) = value(rest)?;
    let status = match status {
        "completed" => ResultStatus::Completed,
        "stopped" => ResultStatus::Stopped,
        "failed" => ResultStatus::Failed,
        _ => return None,
    };
    let rest = rest.strip_prefix(" seq=\"")?;
    let (seq, rest) = value(rest)?;
    let digits = seq.bytes().all(|byte| byte.is_ascii_digit());
    if seq.is_empty() || seq.starts_with('0') || !digits {
        return None;
    }
    let rest = rest.strip_prefix('>')?;
    let end = rest.find(['<', '>'])?;
    let (body, rest) = rest.split_at(end);
    let rest = rest.strip_prefix("</nexus-session-result>")?;
    let result = ReadResult {
        session_id: decode(session_id),
        title: decode(title),
        agent,
        status,
        text: decode(body),
    };
    Some((result, rest))
}

/// An attribute's value up to its closing quote, holding no `<` or `>`, and what follows the quote.
fn value(text: &str) -> Option<(&str, &str)> {
    let end = text.find(['"', '<', '>'])?;
    let rest = text[end..].strip_prefix('"')?;
    Some((&text[..end], rest))
}

/// The four entities [`crate::linked::results::escape`] writes, read back in one pass.
fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = [("&amp;", '&'), ("&lt;", '<'), ("&gt;", '>'), ("&quot;", '"')]
            .into_iter()
            .find(|(entity, _)| rest.starts_with(entity));
        match entity {
            Some((entity, ch)) => {
                out.push(ch);
                rest = &rest[entity.len()..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
