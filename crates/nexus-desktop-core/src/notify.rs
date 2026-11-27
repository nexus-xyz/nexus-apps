//! System notifications for the Code workbench, without the toolkit: which moments are worth one (a session that needs
//! the person, or finished a turn), their text, the sub-agent an approval names ([`AgentNames`]), when to stay quiet
//! and which page a click opens. The shell shows them through its own [`Notifier`].

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use nexus_workbench::event::{TaskStatus, TurnStatus, WbEvent};
use serde_json::Value;

/// How a shell shows a session's notification. Shared by the modules that notify (pull request watches, secret
/// requests) so they need no toolkit.
pub trait Notifier: Send + Sync {
    /// Whether the person is looking at `thread_id` now: the main window is focused with it on screen.
    fn looking_at(&self, thread_id: &str) -> bool;
    /// Shows `body` under the session's `title`, unless the person turned notifications off or is looking at the
    /// session ([`wanted`]). `machine` is the remote machine the session runs on (None: this computer).
    fn show(&self, machine: Option<&str>, thread_id: &str, title: &str, body: &str);
}

/// What to say about an event, if it is worth a notification. `asker`: the name of the sub-agent an approval comes
/// from, when it comes from one.
pub fn message(event: &WbEvent, asker: Option<&str>) -> Option<String> {
    let said = match event {
        WbEvent::Approval { tool, .. } if tool == "AskUserQuestion" => "Has a question for you",
        WbEvent::Approval { .. } => "Needs your approval",
        WbEvent::TurnEnd {
            status: TurnStatus::Completed,
            ..
        } => "Finished",
        WbEvent::TurnEnd {
            status: TurnStatus::Error,
            ..
        } => "Stopped with an error",
        _ => return None,
    };
    Some(match (event, asker) {
        (WbEvent::Approval { tool, .. }, Some(asker)) if tool == "AskUserQuestion" => {
            format!("The sub-agent \u{201c}{asker}\u{201d} has a question for you")
        }
        (WbEvent::Approval { .. }, Some(asker)) => format!("The sub-agent \u{201c}{asker}\u{201d} needs your approval"),
        _ => said.to_string(),
    })
}

/// Sessions whose sub-agents' names are kept: the ones that named a sub-agent most recently. An approval comes from a
/// session that is running, so an older one is not asked about again.
const KEPT_SESSIONS: usize = 32;
/// Names kept per session. A crew larger than this names its later sub-agents generically.
const KEPT_NAMES: usize = 512;
/// A name longer than this is cut, with an ellipsis, so a notification stays one line.
const NAME_CHARS: usize = 60;

/// `text` trimmed, and cut to `NAME_CHARS` characters with an ellipsis.
fn short_name(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(NAME_CHARS) {
        Some((cut, _)) => format!("{}\u{2026}", text[..cut].trim_end()),
        None => text.to_string(),
    }
}

#[derive(Default)]
struct Names {
    by_session: HashMap<String, HashMap<String, String>>,
    /// Session ids, the one that named a sub-agent least recently first.
    recent: VecDeque<String>,
}

impl Names {
    /// The names of `thread_id`'s sub-agents, made the most recent; the least recent session is let go past
    /// `KEPT_SESSIONS`.
    fn session(&mut self, thread_id: &str) -> &mut HashMap<String, String> {
        if let Some(at) = self.recent.iter().position(|id| id == thread_id) {
            self.recent.remove(at);
        }
        self.recent.push_back(thread_id.to_string());
        while self.recent.len() > KEPT_SESSIONS {
            if let Some(oldest) = self.recent.pop_front() {
                self.by_session.remove(&oldest);
            }
        }
        self.by_session.entry(thread_id.to_string()).or_default()
    }
}

/// Sub-agents' names by session, learned from the events as they stream (an Agent call's description, a task's first
/// report that says what it does), so an approval can say which sub-agent asks. An approval names its sub-agent by
/// call id or task id. Bounded: `KEPT_SESSIONS` sessions, `KEPT_NAMES` names each.
#[derive(Default)]
pub struct AgentNames(Mutex<Names>);

impl AgentNames {
    /// Notes a sub-agent's name from `event`, and returns the name of the sub-agent an approval comes from.
    pub fn read(&self, thread_id: &str, event: &WbEvent) -> Option<String> {
        // Most events name no sub-agent: they cost neither the lock nor an allocation.
        let relevant = match event {
            WbEvent::Tool { name, .. } => name == "Agent" || name == "Task",
            WbEvent::Task {
                status: TaskStatus::Running,
                ..
            } => true,
            WbEvent::Approval { parent, .. } => parent.is_some(),
            _ => false,
        };
        if !relevant {
            return None;
        }
        let mut all = self.0.lock().ok()?;
        if let WbEvent::Approval {
            parent: Some(parent), ..
        } = event
        {
            return all.by_session.get(thread_id)?.get(parent).cloned();
        }
        let names = all.session(thread_id);
        match event {
            WbEvent::Tool { id, name, input, .. } if name == "Agent" || name == "Task" => {
                let said = input
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                if !said.is_empty() && (names.len() < KEPT_NAMES || names.contains_key(id)) {
                    names.insert(id.clone(), short_name(said));
                }
                None
            }
            // The first running report that says what the task does; its last one says how it went.
            WbEvent::Task {
                task_id,
                tool_use_id,
                status: TaskStatus::Running,
                description,
                ..
            } if !description.trim().is_empty() && (names.len() < KEPT_NAMES || names.contains_key(task_id)) => {
                let said = tool_use_id.as_ref().and_then(|call| names.get(call).cloned());
                names
                    .entry(task_id.clone())
                    .or_insert_with(|| said.unwrap_or_else(|| short_name(description)));
                None
            }
            _ => None,
        }
    }
}

/// The `OnScreen` key of a session. This computer's sessions are keyed by their id; a remote machine's by machine and
/// id together, so two machines (or one of them and this computer) never share a key.
pub fn screen_key(machine: Option<&str>, thread_id: &str) -> String {
    match machine {
        Some(machine) => format!("machine:{machine}:{thread_id}"),
        None => thread_id.to_string(),
    }
}

/// Whether a moment of `thread_id` is shown: notifications are on, and the person is not already looking at it.
pub fn wanted(enabled: bool, main_focused: bool, on_screen: Option<&str>, thread_id: &str) -> bool {
    enabled && !(main_focused && on_screen == Some(thread_id))
}

/// The page of a session in the main window: `/app/<org>/code/<id>`, or the home when no organization is known.
pub fn session_path(org: Option<&str>, thread_id: &str) -> String {
    let enc = nexus_client::url::enc;
    match org.filter(|org| !org.is_empty()) {
        Some(org) => format!("/app/{}/code/{}", enc(org), enc(thread_id)),
        None => "/app".into(),
    }
}

/// The page a notification opens: the session's, at `request` when it is a question.
pub fn request_path(org: Option<&str>, thread_id: &str, request: Option<&str>) -> String {
    let path = session_path(org, thread_id);
    match request.filter(|_| path != "/app") {
        Some(request) => format!("{path}?question={}", nexus_client::url::enc(request)),
        None => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn approval(tool: &str) -> WbEvent {
        WbEvent::Approval {
            request_id: "r".into(),
            tool_use_id: None,
            tool: tool.into(),
            input: json!({}),
            description: None,
            suggestions: vec![],
            parent: None,
        }
    }

    fn end(status: TurnStatus) -> WbEvent {
        WbEvent::TurnEnd {
            turn_id: "t".into(),
            status,
            cost_usd: None,
            duration_ms: 1,
            error: None,
            cursor: None,
        }
    }

    #[test]
    fn only_moments_that_need_the_person() {
        assert_eq!(message(&approval("Bash"), None).as_deref(), Some("Needs your approval"));
        assert_eq!(
            message(&approval("AskUserQuestion"), None).as_deref(),
            Some("Has a question for you")
        );
        assert_eq!(message(&end(TurnStatus::Completed), None).as_deref(), Some("Finished"));
        assert_eq!(
            message(&end(TurnStatus::Error), None).as_deref(),
            Some("Stopped with an error")
        );
        // The person stopped it themselves.
        assert_eq!(message(&end(TurnStatus::Interrupted), None), None);
        assert_eq!(message(&WbEvent::Notice { text: "x".into() }, None), None);
    }

    #[test]
    fn a_question_notification_opens_the_exact_request() {
        assert_eq!(
            request_path(Some("acme"), "t-1", Some("question#7")),
            "/app/acme/code/t-1?question=question%237"
        );
        assert_eq!(request_path(Some("acme"), "t-1", None), "/app/acme/code/t-1");
        assert_eq!(request_path(None, "t-1", Some("q")), "/app");
    }

    #[test]
    fn a_sub_agents_approval_names_it() {
        let names = AgentNames::default();
        let call = WbEvent::Tool {
            id: "toolu_A".into(),
            name: "Agent".into(),
            input: json!({ "description": "Read the auth tests", "prompt": "go" }),
            parent: None,
        };
        assert_eq!(names.read("t", &call), None);
        let mut asks = approval("Bash");
        if let WbEvent::Approval { parent, .. } = &mut asks {
            *parent = Some("toolu_A".into());
        }
        let asker = names.read("t", &asks);
        assert_eq!(asker.as_deref(), Some("Read the auth tests"));
        assert_eq!(
            message(&asks, asker.as_deref()).as_deref(),
            Some("The sub-agent \u{201c}Read the auth tests\u{201d} needs your approval")
        );
        // Another session knows no such sub-agent; the lead's own approval names none.
        assert_eq!(names.read("other", &asks), None);
        assert_eq!(names.read("t", &approval("Bash")), None);
    }

    fn agent_call(id: &str, description: &str) -> WbEvent {
        WbEvent::Tool {
            id: id.into(),
            name: "Agent".into(),
            input: json!({ "description": description }),
            parent: None,
        }
    }

    fn asks_from(parent: &str) -> WbEvent {
        let mut asks = approval("Bash");
        if let WbEvent::Approval { parent: from, .. } = &mut asks {
            *from = Some(parent.into());
        }
        asks
    }

    #[test]
    fn a_task_takes_its_agent_calls_name_and_the_message_quotes_it() {
        let names = AgentNames::default();
        names.read("t", &agent_call("toolu_A", "Read the auth tests"));
        let report = WbEvent::Task {
            task_id: "a1".into(),
            tool_use_id: Some("toolu_A".into()),
            task_type: Some("local_agent".into()),
            agent_type: None,
            status: TaskStatus::Running,
            description: "a different, longer description".into(),
            tool_count: None,
            total_tokens: None,
            duration_ms: None,
            detail: None,
        };
        names.read("t", &report);
        // An approval that names the task id gets the call's name, in curly quotes.
        let asker = names.read("t", &asks_from("a1"));
        assert_eq!(asker.as_deref(), Some("Read the auth tests"));
        let mut question = asks_from("a1");
        if let WbEvent::Approval { tool, .. } = &mut question {
            *tool = "AskUserQuestion".into();
        }
        assert_eq!(
            message(&question, asker.as_deref()).as_deref(),
            Some("The sub-agent \u{201c}Read the auth tests\u{201d} has a question for you")
        );
    }

    #[test]
    fn names_are_kept_for_the_most_recent_sessions_only() {
        let names = AgentNames::default();
        for session in 0..=KEPT_SESSIONS {
            names.read(&format!("t{session}"), &agent_call("A", "worker"));
        }
        // The first session was let go; the last ones still name their sub-agent.
        assert_eq!(names.read("t0", &asks_from("A")), None);
        assert_eq!(
            names.read(&format!("t{KEPT_SESSIONS}"), &asks_from("A")).as_deref(),
            Some("worker")
        );
        let held = names
            .0
            .lock()
            .map(|all| (all.by_session.len(), all.recent.len()))
            .unwrap_or_default();
        assert_eq!(held, (KEPT_SESSIONS, KEPT_SESSIONS));
    }

    #[test]
    fn a_session_keeps_at_most_kept_names_and_a_long_name_is_cut() {
        let names = AgentNames::default();
        for agent in 0..KEPT_NAMES + 5 {
            names.read("t", &agent_call(&format!("A{agent}"), "worker"));
        }
        assert_eq!(names.read("t", &asks_from(&format!("A{}", KEPT_NAMES + 1))), None);
        assert_eq!(names.read("t", &asks_from("A0")).as_deref(), Some("worker"));
        let long = "x".repeat(200);
        let names = AgentNames::default();
        names.read("t", &agent_call("L", &long));
        let cut = names.read("t", &asks_from("L")).unwrap_or_default();
        assert_eq!(cut.chars().count(), NAME_CHARS + 1);
        assert!(cut.ends_with('\u{2026}'));
    }

    #[test]
    fn a_task_known_only_by_its_id_is_named_by_its_first_report() {
        let names = AgentNames::default();
        let report = |status: TaskStatus, description: &str| WbEvent::Task {
            task_id: "a1".into(),
            tool_use_id: None,
            task_type: Some("local_agent".into()),
            agent_type: None,
            status,
            description: description.into(),
            tool_count: None,
            total_tokens: None,
            duration_ms: None,
            detail: None,
        };
        names.read("t", &report(TaskStatus::Running, "Map the callers"));
        names.read("t", &report(TaskStatus::Completed, "Found three callers."));
        let mut asks = approval("Bash");
        if let WbEvent::Approval { parent, .. } = &mut asks {
            *parent = Some("a1".into());
        }
        assert_eq!(names.read("t", &asks).as_deref(), Some("Map the callers"));
    }

    #[test]
    fn quiet_only_when_turned_off_or_already_looking_at_the_session() {
        // Away from the app: always, whatever is on screen.
        assert!(wanted(true, false, Some("a"), "a"));
        assert!(wanted(true, false, None, "a"));
        // In the app, on another session or another page.
        assert!(wanted(true, true, Some("b"), "a"));
        assert!(wanted(true, true, None, "a"));
        // In the app, on that session.
        assert!(!wanted(true, true, Some("a"), "a"));
        // Turned off in Settings.
        assert!(!wanted(false, false, None, "a"));
    }

    #[test]
    fn a_remote_session_never_shares_a_key_with_a_local_one() {
        assert_eq!(screen_key(None, "t-1"), "t-1");
        assert_eq!(screen_key(Some("m1"), "t-1"), "machine:m1:t-1");
        assert_ne!(screen_key(Some("m1"), "t-1"), screen_key(Some("m2"), "t-1"));
    }

    #[test]
    fn opens_the_session_in_its_organization() {
        assert_eq!(session_path(Some("acme"), "t-1"), "/app/acme/code/t-1");
        assert_eq!(session_path(Some("a b"), "x/y"), "/app/a%20b/code/x%2Fy");
        assert_eq!(session_path(None, "t-1"), "/app");
        assert_eq!(session_path(Some(""), "t-1"), "/app");
    }
}
