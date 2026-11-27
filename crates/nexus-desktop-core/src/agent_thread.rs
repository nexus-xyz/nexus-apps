//! The Assistant thread's quick replies and link cards, read strictly as the kit's
//! `agent-chat.ts` reads them (`answerSuggestions`, `answerSessions`, `noticeErrand`), with the cards' words as Nexus UI
//! writes them (`agent-session-card.tsx`, `agent-errand-card.tsx`). Nexus GPUI draws them
//! (`desktop-gpui/src/pages/agent/thread_links.rs`).
//!
//! Each reader drops an entry that is not one and keeps the rest, so a bad entry never drops the answer. A link is
//! http(s) only, or the desktop's own `nexus://new?` draft link for a desktop session: a `javascript:` URL never
//! becomes one.

use serde_json::Value;

use crate::agent::{Message, control, js_len, safe_url};
use crate::agent_errands::{Errand, Status, errand_line, errand_of};

/// The most quick replies one answer shows, and the longest one (the kit's `AGENT_MAX_SUGGESTIONS`,
/// `AGENT_SUGGESTION_CHARS`).
pub const MAX_SUGGESTIONS: usize = 3;
pub const SUGGESTION_CHARS: usize = 40;

/// The line before an opened errand's card.
pub const OPENED_ERRAND: &str = "I opened an errand for this.";

/// An answer's quick replies: plain text on one line, at most [`SUGGESTION_CHARS`] each, each once (ignoring
/// case), at most [`MAX_SUGGESTIONS`]. Anything else is left out.
pub fn answer_suggestions(message: &Message) -> Vec<String> {
    let Message::Answer { suggestions, .. } = message else {
        return Vec::new();
    };
    let Some(items) = suggestions.as_array() else {
        return Vec::new();
    };
    let mut found: Vec<String> = Vec::new();
    for item in items {
        let Some(reply) = item.as_str() else { continue };
        if reply.chars().any(control) {
            continue;
        }
        let reply = reply.trim();
        let seen = found.iter().any(|seen| seen.to_lowercase() == reply.to_lowercase());
        if reply.is_empty() || js_len(reply) > SUGGESTION_CHARS || seen {
            continue;
        }
        found.push(reply.to_string());
        if found.len() == MAX_SUGGESTIONS {
            break;
        }
    }
    found
}

/// The quick replies under a reply (Nexus UI's `lastSuggestions`): its last message's, when that is an answer.
pub fn last_suggestions(messages: &[Message]) -> Vec<String> {
    messages.last().map(answer_suggestions).unwrap_or_default()
}

/// Where a session lives: it syncs end-to-end encrypted (its title names its agent), or it is set up for this app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionWhere {
    Synced,
    Desktop,
}

/// A session's state, drawn by shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Running,
    Done,
    Failed,
    Stopped,
    Ready,
}

impl SessionState {
    fn of(value: &str) -> Option<SessionState> {
        Some(match value {
            "running" => SessionState::Running,
            "done" => SessionState::Done,
            "failed" => SessionState::Failed,
            "stopped" => SessionState::Stopped,
            "ready" => SessionState::Ready,
            _ => return None,
        })
    }

    /// The state as the card's line says it (the kit's `sessionStateText`).
    pub fn text(self) -> &'static str {
        match self {
            SessionState::Running => "Running",
            SessionState::Done => "Done",
            SessionState::Failed => "Failed",
            SessionState::Stopped => "Stopped",
            SessionState::Ready => "Ready to open",
        }
    }
}

/// One of the person's coding sessions, as the link card shows it (the kit's `AgentSession`).
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub place: SessionWhere,
    pub state: SessionState,
    pub title: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub detail: Option<String>,
    /// Open: a web page, or the desktop's `nexus://new?…` link, which fills a draft and never sends it.
    pub url: String,
    pub id: Option<String>,
}

/// The most session cards one answer shows, and the longest title.
const MAX_SESSIONS: usize = 3;
const SESSION_TEXT_CHARS: usize = 120;
/// The longest desktop link the card opens.
const DESKTOP_LINK_CHARS: usize = 100_000;

/// Plain text of at most `limit` characters on one line (the kit's `cardText`), or None for none.
fn card_text(value: Option<&Value>, limit: usize) -> Option<String> {
    let text = value?.as_str()?.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = text.chars().take(limit).collect();
    let cut = cut.trim();
    (!cut.is_empty()).then(|| cut.to_string())
}

/// A GitHub-style `owner/name`, as the kit's `SESSION_REPO` admits it.
fn repo_name(value: &str) -> bool {
    let Some((owner, name)) = value.split_once('/') else {
        return false;
    };
    let owner_ok = owner.len() <= 39
        && owner.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let name_ok = (1..=100).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    owner_ok && name_ok
}

/// Where a session card's Open goes (the kit's `sessionUrl`): an http(s) page, or for a desktop session the desktop's
/// `nexus://new?…` draft link and no other `nexus:` link.
pub fn session_url(url: Option<&Value>, place: SessionWhere) -> Option<String> {
    let url = url?.as_str()?;
    match place {
        SessionWhere::Desktop => {
            (url.starts_with("nexus://new?") && js_len(url) <= DESKTOP_LINK_CHARS).then(|| url.to_string())
        }
        SessionWhere::Synced => safe_url(url),
    }
}

/// An answer's session cards: a known place and state, a title, an allowed link, plain text. An entry that is not
/// one is left out, each link shows once, and at most three show.
pub fn answer_sessions(message: &Message) -> Vec<Session> {
    let Message::Answer { sessions, .. } = message else {
        return Vec::new();
    };
    let Some(items) = sessions.as_array() else {
        return Vec::new();
    };
    let mut found: Vec<Session> = Vec::new();
    for entry in items.iter().filter(|entry| entry.is_object()) {
        let place = match entry.get("where").and_then(Value::as_str) {
            Some("synced") => SessionWhere::Synced,
            Some("desktop") => SessionWhere::Desktop,
            _ => continue,
        };
        let Some(state) = entry.get("state").and_then(Value::as_str).and_then(SessionState::of) else {
            continue;
        };
        let Some(title) = card_text(entry.get("title"), SESSION_TEXT_CHARS) else {
            continue;
        };
        let Some(url) = session_url(entry.get("url"), place) else {
            continue;
        };
        if found.iter().any(|seen| seen.url == url) {
            continue;
        }
        let repo = entry
            .get("repo")
            .and_then(Value::as_str)
            .filter(|repo| repo_name(repo))
            .map(String::from);
        found.push(Session {
            place,
            state,
            title,
            repo,
            branch: card_text(entry.get("branch"), 100),
            detail: card_text(entry.get("detail"), 80),
            url,
            id: card_text(entry.get("id"), 128),
        });
        if found.len() == MAX_SESSIONS {
            break;
        }
    }
    found
}

/// The card's line under its title (the kit's `sessionCardLine`): the state, then the branch or a detail, then the
/// repository's name, as "Done · on decoder-gru" and "cortex-sim". The view greys the repository.
#[derive(Debug, Clone, PartialEq)]
pub struct CardLine {
    pub state: &'static str,
    pub detail: Option<String>,
    pub repo: Option<String>,
}

impl CardLine {
    /// The state and the detail, as one string: "Done · on decoder-gru".
    pub fn lead(&self) -> String {
        match &self.detail {
            Some(detail) => format!("{} · {detail}", self.state),
            None => self.state.to_string(),
        }
    }
}

pub fn session_card_line(session: &Session) -> CardLine {
    let detail = match &session.branch {
        Some(branch) => Some(format!("on {branch}")),
        None => session.detail.clone(),
    };
    CardLine {
        state: session.state.text(),
        detail,
        repo: session
            .repo
            .as_deref()
            .and_then(|repo| repo.rsplit('/').next())
            .map(String::from),
    }
}

/// The errand an "I opened an errand" notice carries (`topic: "errands"`, `state: "opened"`), read strictly; None for
/// any other message.
pub fn notice_errand(message: &Message) -> Option<Errand> {
    let Message::Notice {
        topic, state, errand, ..
    } = message
    else {
        return None;
    };
    (topic == "errands" && state == "opened")
        .then(|| errand_of(errand))
        .flatten()
}

/// The errand card's line (Nexus UI's `errandCardLine`): what it needs or how it goes; a failed one says why, when the
/// agent said.
pub fn errand_card_line(errand: &Errand) -> String {
    match (&errand.status, &errand.detail) {
        (Status::Failed, Some(detail)) => detail.clone(),
        _ => errand_line(errand),
    }
}

/// The word a screen reader hears for an errand's state.
pub fn errand_state_word(status: Status) -> &'static str {
    match status {
        Status::Running => "Running",
        Status::NeedsYou => "Needs you",
        Status::Done => "Done",
        Status::Failed => "Failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn answer(extra: Value) -> Message {
        let mut body = json!({ "kind": "answer", "text": "Shall I?" });
        body.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        serde_json::from_value(body).expect("an answer")
    }

    #[test]
    fn quick_replies_are_at_most_three_plain_short_lines_each_once() {
        let message = answer(json!({ "suggestions": [
            " Tell them ", "tell them", "", "two\nlines", 7, "x".repeat(41), "x".repeat(40), "Not now", "Later"
        ]}));
        assert_eq!(
            answer_suggestions(&message),
            vec!["Tell them".to_string(), "x".repeat(40), "Not now".to_string()]
        );
        for odd in [
            json!({}),
            json!({ "suggestions": "Tell them" }),
            json!({ "suggestions": null }),
        ] {
            assert!(answer_suggestions(&answer(odd)).is_empty());
        }
        // Only the reply's last message, and only an answer.
        let notice: Message = serde_json::from_value(json!({ "kind": "notice", "text": "x" })).expect("a notice");
        let asks = answer(json!({ "suggestions": ["Yes"] }));
        assert_eq!(
            last_suggestions(&[notice.clone(), asks.clone()]),
            vec!["Yes".to_string()]
        );
        assert!(last_suggestions(&[asks, notice]).is_empty());
        assert!(last_suggestions(&[]).is_empty());
    }

    #[test]
    fn a_session_card_is_read_strictly() {
        let message = answer(json!({ "sessions": [
            { "where": "desktop", "state": "ready", "title": "  Fix   the decoder ", "url": "nexus://new?prompt=fix",
              "repo": "acme/cortex-sim", "branch": "decoder-gru" },
            { "where": "synced", "state": "done", "title": "Codex session", "url": "https://nexus.test/app/s/1", "detail": "2 files" },
            { "where": "synced", "state": "done", "title": "Again", "url": "https://nexus.test/app/s/1" },
            { "where": "synced", "state": "running", "title": "Bad", "url": "javascript:alert(1)" },
            { "where": "synced", "state": "running", "title": "Desktop link from sync", "url": "nexus://new?prompt=x" },
            { "where": "desktop", "state": "ready", "title": "Other nexus link", "url": "nexus://settings" },
            { "where": "cloud", "state": "running", "title": "Unknown place", "url": "https://nexus.test" },
            { "where": "synced", "state": "paused", "title": "Unknown state", "url": "https://nexus.test" },
            { "where": "synced", "state": "done", "title": "  ", "url": "https://nexus.test/2" },
            { "where": "synced", "state": "failed", "title": "Bad repo", "url": "https://nexus.test/3", "repo": "-x/y" },
            { "where": "synced", "state": "stopped", "title": "Fourth", "url": "https://nexus.test/4" },
        ]}));
        let sessions = answer_sessions(&message);
        assert_eq!(sessions.len(), 3, "at most three");
        assert_eq!(sessions[0].title, "Fix the decoder");
        assert_eq!(sessions[0].place, SessionWhere::Desktop);
        assert_eq!(sessions[0].url, "nexus://new?prompt=fix");
        let line = session_card_line(&sessions[0]);
        assert_eq!(
            (line.lead(), line.repo.as_deref()),
            ("Ready to open · on decoder-gru".into(), Some("cortex-sim"))
        );
        assert_eq!(session_card_line(&sessions[1]).lead(), "Done · 2 files");
        assert_eq!(sessions[2].title, "Bad repo");
        assert_eq!(
            sessions[2].repo, None,
            "a repository that is not owner/name is left out"
        );
        assert!(answer_sessions(&answer(json!({ "sessions": {} }))).is_empty());
    }

    #[test]
    fn an_opened_errand_notice_carries_its_card() {
        let notice = |topic: &str, state: &str, errand: Value| -> Message {
            serde_json::from_value(
                json!({ "kind": "notice", "text": "x", "topic": topic, "state": state, "errand": errand }),
            )
            .expect("a notice")
        };
        let row = json!({ "id": "i3", "goal": "Book the dentist", "status": "running", "steps": 2, "at": "2026-11-18T09:00:00Z" });
        let errand = notice_errand(&notice("errands", "opened", row.clone())).expect("the errand");
        assert_eq!(
            (errand.id.as_str(), errand_card_line(&errand)),
            ("i3", "2 steps so far".into())
        );
        assert_eq!(notice_errand(&notice("errands", "closed", row.clone())), None);
        assert_eq!(notice_errand(&notice("memory", "opened", row)), None);
        assert_eq!(
            notice_errand(&notice("errands", "opened", json!({ "id": "../x" }))),
            None
        );
        let plain: Message = serde_json::from_value(json!({ "kind": "notice", "text": "Saved." })).expect("a notice");
        assert_eq!(notice_errand(&plain), None);

        // A failed errand says why, when the agent said; else the list's own line.
        let failed = errand_of(
            &json!({ "id": "i4", "goal": "g", "status": "failed", "detail": "The site was down", "at": "t" }),
        )
        .expect("an errand");
        assert_eq!(errand_card_line(&failed), "The site was down");
        let failed = errand_of(&json!({ "id": "i4", "goal": "g", "status": "failed", "at": "t" })).expect("an errand");
        assert_eq!(errand_card_line(&failed), "It did not finish");
    }
}
