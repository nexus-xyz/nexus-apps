//! What a phone's approval answer remembers, and the notice each phone command leaves in the session. The phone sends a
//! decision only (`allowSession`, no rule), so the computer picks the rule. The notices name the action and never echo
//! a message, an answer or a tool's input.

use serde_json::{Value, json};

use crate::event::{Decision, PLAN_TOOL, PermissionSuggestion, QUESTION_TOOL};

pub const SENT: &str = "Your phone sent a message.";
pub const STOPPED: &str = "Your phone stopped the turn.";

/// The notice for a phone message that waited while this computer slept. `sent_at`: when the phone sent it,
/// in Unix milliseconds, shown in this computer's time zone.
pub(super) fn waited_notice(sent_at: i64) -> String {
    use chrono::TimeZone as _;
    waited_text(
        chrono::Local
            .timestamp_millis_opt(sent_at)
            .single()
            .map(|at| at.format("%H:%M").to_string()),
    )
}

fn waited_text(time: Option<String>) -> String {
    match time {
        Some(time) => format!("Your phone's message from {time} arrived while this computer was asleep."),
        None => "Your phone's message arrived while this computer was asleep.".into(),
    }
}

/// The rule a phone's "Allow for this session" keeps: the agent's own answer for the rest of this session, never a
/// change to a settings file and never "everything, without asking". None allows once.
///
/// - Codex and the Nexus agent: their `acceptForSession`.
/// - Claude Code: its first allow suggestion for the `session` destination (an `acceptEdits` mode at most), else its
///   first `addRules` allow suggestion, kept for this session instead of the settings file it names.
/// - Other agents (ACP's `allow_always`) offer no session-only answer: the call is allowed once.
pub(super) fn session_rule(suggestions: &[PermissionSuggestion]) -> Option<Value> {
    let raws = || suggestions.iter().map(|suggestion| &suggestion.raw);
    if let Some(raw) = raws().find(|raw| raw.as_str() == Some("acceptForSession")) {
        return Some(raw.clone());
    }
    let allows = |raw: &Value| raw.get("behavior").and_then(Value::as_str) != Some("deny");
    let kind = |raw: &Value| raw.get("type").and_then(Value::as_str).map(str::to_string);
    let session = raws().find(|raw| {
        raw.get("destination").and_then(Value::as_str) == Some("session")
            && allows(raw)
            && match kind(raw).as_deref() {
                Some("setMode") => raw.get("mode").and_then(Value::as_str) == Some("acceptEdits"),
                Some("addRules" | "addDirectories") => true,
                _ => false,
            }
    });
    if let Some(raw) = session {
        return Some(raw.clone());
    }
    raws()
        .find(|raw| kind(raw).as_deref() == Some("addRules") && allows(raw))
        .map(|raw| {
            let mut rule = raw.clone();
            rule["destination"] = json!("session");
            rule
        })
}

/// The notice for a phone's answer to an approval of `tool`. `remembered`: a rule is kept for this session.
pub fn approval_notice(tool: &str, decision: Decision, remembered: bool) -> String {
    let allowed = matches!(decision, Decision::Allow | Decision::AllowSession);
    match tool {
        QUESTION_TOOL if allowed => "Your phone answered the question.".into(),
        QUESTION_TOOL => "Your phone declined the question.".into(),
        PLAN_TOOL if allowed => "Your phone approved the plan.".into(),
        PLAN_TOOL => "Your phone rejected the plan.".into(),
        _ => {
            let tool = shown_tool(tool);
            match (allowed, remembered) {
                (true, true) => format!("Your phone allowed {tool} for this session."),
                (true, false) => format!("Your phone allowed {tool}."),
                (false, _) => format!("Your phone denied {tool}."),
            }
        }
    }
}

/// A tool's name as the notice shows it: short, with a plain word for none.
fn shown_tool(tool: &str) -> String {
    const MAX: usize = 64;
    let tool = tool.trim();
    if tool.is_empty() {
        return "a tool".into();
    }
    if tool.chars().count() <= MAX {
        return tool.into();
    }
    let short: String = tool.chars().take(MAX - 1).collect();
    format!("{short}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered(raws: &[Value]) -> Vec<PermissionSuggestion> {
        raws.iter()
            .map(|raw| PermissionSuggestion {
                kind: raw.get("type").and_then(Value::as_str).unwrap_or("").into(),
                label: String::new(),
                raw: raw.clone(),
            })
            .collect()
    }

    #[test]
    fn codex_and_the_nexus_agent_keep_their_session_answer() {
        let codex = offered(&[
            json!({"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["echo", "hi"]}}),
            json!("acceptForSession"),
        ]);
        assert_eq!(session_rule(&codex), Some(json!("acceptForSession")));
        let only_permanent = offered(&[json!({"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["ls"]}})]);
        assert_eq!(session_rule(&only_permanent), None, "never a rule from now on");
    }

    #[test]
    fn claude_code_keeps_a_session_rule_and_never_writes_a_settings_file() {
        let session = json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "echo:*"}],
            "behavior": "allow", "destination": "session"});
        let local = json!({"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "npm test:*"}],
            "behavior": "allow", "destination": "localSettings"});
        assert_eq!(session_rule(&offered(&[local.clone(), session.clone()])), Some(session));
        let mut kept = local.clone();
        kept["destination"] = json!("session");
        assert_eq!(session_rule(&offered(&[local])), Some(kept));
        let edits = json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"});
        assert_eq!(session_rule(&offered(std::slice::from_ref(&edits))), Some(edits));
    }

    #[test]
    fn nothing_broader_than_this_kind_of_request_is_kept() {
        let everything = json!({"type": "setMode", "mode": "bypassPermissions", "destination": "session"});
        let deny = json!({"type": "addRules", "rules": [{"toolName": "Bash"}], "behavior": "deny",
            "destination": "session"});
        let acp = json!("allow_always_option");
        assert_eq!(session_rule(&offered(&[everything, deny, acp])), None);
        assert_eq!(session_rule(&[]), None);
    }

    #[test]
    fn a_waited_message_says_when_the_phone_sent_it() {
        assert_eq!(
            waited_text(Some("14:02".into())),
            "Your phone's message from 14:02 arrived while this computer was asleep."
        );
        assert_eq!(
            waited_text(None),
            "Your phone's message arrived while this computer was asleep."
        );
        assert!(waited_notice(1_800_000_000_000).starts_with("Your phone's message from "));
    }

    #[test]
    fn notices_name_the_action_only() {
        assert_eq!(
            approval_notice("Bash", Decision::Allow, false),
            "Your phone allowed Bash."
        );
        assert_eq!(
            approval_notice("Bash", Decision::AllowSession, true),
            "Your phone allowed Bash for this session."
        );
        assert_eq!(
            approval_notice("Edit", Decision::Deny, false),
            "Your phone denied Edit."
        );
        assert_eq!(
            approval_notice(QUESTION_TOOL, Decision::Allow, false),
            "Your phone answered the question."
        );
        assert_eq!(
            approval_notice(PLAN_TOOL, Decision::Allow, false),
            "Your phone approved the plan."
        );
        let long = "t".repeat(200);
        assert_eq!(
            approval_notice(&long, Decision::Allow, false).chars().count(),
            "Your phone allowed .".len() + 64
        );
    }
}
