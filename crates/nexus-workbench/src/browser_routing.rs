//! Agent browsing goes through the session's Nexus browser, the `nexus_browser` server, as commands go
//! through the shared terminal ([`crate::terminal`]). Every agent gets [`GUIDANCE`]. An agent that needs a capability
//! those tools lack records a bypass first: `browser_bypass` names the capability and the reason, and the conversation
//! shows them ([`Workbench::observe_browser_routing`]).
//!
//! - Claude Code: a `PreToolUse` hook callback, registered in `initialize` ([`HOOK`]), refuses outside browser tools
//!   while the run has the Nexus browser tools and no bypass ([`Gate`]). Hooks run in every permission mode.
//! - Codex and the Nexus agent: the guidance, and a notice when they use an outside browser tool without a bypass.
//!
//! An outside browser tool is an MCP tool whose server's name says browser automation, or whose own name starts with
//! `browser_` ([`outside_tool`]). Commands that start a browser from the terminal are not intercepted.

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::event::WbEvent;
use crate::mcp_servers::BROWSER;
use crate::{Live, Workbench};

/// What every agent's system prompt says about browsing ([`crate::driver::instructions`]).
pub const GUIDANCE: &str = include_str!("browser_routing/instructions.txt").trim_ascii_end();
/// The Nexus browser's tool that records a bypass.
pub const BYPASS_TOOL: &str = "browser_bypass";
/// The id of the Claude Code hook callback that guards browser tools.
pub const HOOK: &str = "nexus_browser_routing";
/// Server names that mean browser automation, lowercase.
const BROWSER_WORDS: [&str; 9] = [
    "browser",
    "playwright",
    "puppeteer",
    "chrome",
    "chromium",
    "firefox",
    "selenium",
    "webdriver",
    "stagehand",
];
const MAX_CAPABILITY: usize = 200;
const MIN_REASON: usize = 12;
const MAX_REASON: usize = 1000;
/// What a refused outside browser tool answers.
pub const REFUSED: &str = "Use the Nexus browser tools (mcp__nexus_browser__*) for browser tasks. If they cannot do this \
                           step, call mcp__nexus_browser__browser_bypass with the missing capability and the reason \
                           first, then retry.";

/// `browser_bypass` as `tools/list` gives it.
pub fn bypass_definition() -> Value {
    json!({
        "name": BYPASS_TOOL,
        "description": "Record why the Nexus browser tools cannot do a browser step, before you use another browser \
                        (another MCP server's browser tools, a script or a headless browser). The conversation shows \
                        the capability and the reason. Waiting for Allow, declined access, the person's take-over or \
                        preference never justify a bypass.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "capability": {"type": "string", "maxLength": MAX_CAPABILITY,
                    "description": "What the Nexus browser tools lack, such as \"network request log\"."},
                "reason": {"type": "string", "maxLength": MAX_REASON,
                    "description": "Why the step needs it, in one or two sentences."}
            },
            "required": ["capability", "reason"],
            "additionalProperties": false
        }
    })
}

/// A recorded bypass: what the Nexus browser lacks and why the step needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bypass {
    pub capability: String,
    pub reason: String,
}

/// The bypass `input` asks for, or why it is refused.
pub fn bypass(input: &Value) -> Result<Bypass, String> {
    let field = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
    };
    let (capability, reason) = (field("capability"), field("reason"));
    if capability.is_empty() || capability.chars().count() > MAX_CAPABILITY {
        return Err(format!(
            "Name the missing capability in at most {MAX_CAPABILITY} characters."
        ));
    }
    let length = reason.chars().count();
    if !(MIN_REASON..=MAX_REASON).contains(&length) {
        return Err(format!(
            "Give the technical reason in {MIN_REASON} to {MAX_REASON} characters."
        ));
    }
    Ok(Bypass {
        capability: capability.into(),
        reason: reason.into(),
    })
}

/// What `browser_bypass` answers once its input holds.
pub fn bypass_answer(input: &Value) -> Result<Value, String> {
    bypass(input).map(|_| {
        json!({"recorded": true, "note": "The conversation shows this bypass. Use the other browser for this step, then \
                                          go back to the Nexus browser tools."})
    })
}

/// Whether `name` is the Nexus browser's bypass tool, as an agent names it.
pub fn is_bypass(name: &str) -> bool {
    split(name).is_some_and(|(server, tool)| server == BROWSER && tool == BYPASS_TOOL)
}

/// Whether `name` is one of the Nexus browser's own tools, as an agent names it.
pub fn is_nexus_tool(name: &str) -> bool {
    split(name).is_some_and(|(server, _)| server == BROWSER)
}

/// Whether `name` is a browser tool outside the Nexus browser.
pub fn outside_tool(name: &str) -> bool {
    let Some((server, tool)) = split(name) else {
        return false;
    };
    if server == BROWSER {
        return false;
    }
    let server = server.to_ascii_lowercase();
    BROWSER_WORDS.iter().any(|word| server.contains(word)) || tool.to_ascii_lowercase().starts_with("browser_")
}

/// `mcp__<server>__<tool>` as its server and tool.
fn split(name: &str) -> Option<(&str, &str)> {
    name.strip_prefix("mcp__")?.split_once("__")
}

/// Claude Code's side of the routing, for one run ([`crate::claude::Translator`]).
#[derive(Debug, Default)]
pub struct Gate {
    /// The run has the Nexus browser tools (its `init` lists them).
    nexus_tools: bool,
    /// The agent recorded a bypass.
    bypassed: bool,
}

impl Gate {
    /// The tools `init` lists.
    pub fn init(&mut self, tools: &[String]) {
        self.nexus_tools = tools.iter().any(|tool| is_nexus_tool(tool));
    }

    /// A tool call the agent made.
    pub fn call(&mut self, name: &str, input: &Value) {
        if is_bypass(name) && bypass(input).is_ok() {
            self.bypassed = true;
        }
    }

    /// The hook's answer for a call of `tool`: Claude Code's `PreToolUse` output.
    pub fn decide(&self, tool: &str) -> Value {
        if self.nexus_tools && !self.bypassed && outside_tool(tool) {
            json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny",
                "permissionDecisionReason": REFUSED}})
        } else {
            json!({})
        }
    }
}

/// What the workbench saw of one run's browsing.
#[derive(Debug, Default)]
pub(crate) struct RunState {
    bypassed: bool,
    /// Outside tools already noted, so a retry adds no second notice.
    noted: HashSet<String>,
}

impl Workbench {
    /// Notes a run's bypass, and an outside browser tool used without one.
    pub(crate) fn observe_browser_routing(&self, thread: &str, live: &Live, event: &WbEvent) -> Result<(), String> {
        let WbEvent::Tool { name, input, .. } = event else {
            return Ok(());
        };
        let text = {
            let mut state = live.state();
            let state = &mut state.browser;
            if is_bypass(name) {
                let Ok(bypass) = bypass(input) else {
                    return Ok(());
                };
                state.bypassed = true;
                format!(
                    "Browser bypass: the agent will use another browser. Missing in the Nexus browser: {}. Reason: {}",
                    bypass.capability, bypass.reason
                )
            } else if outside_tool(name) && !state.bypassed && state.noted.insert(name.clone()) {
                format!(
                    "Browser routing: the agent asked for {name} outside the Nexus browser without recording a bypass \
                     reason."
                )
            } else {
                return Ok(());
            }
        };
        self.append(thread, WbEvent::Notice { text }).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_tools_are_other_servers_browser_tools() {
        for name in [
            "mcp__playwright__browser_navigate",
            "mcp__chrome-devtools__take_screenshot",
            "mcp__puppeteer__puppeteer_click",
            "mcp__claude-in-chrome__navigate",
            "mcp__my_server__browser_click",
            "mcp__Browserbase__open",
        ] {
            assert!(outside_tool(name), "{name}");
        }
        for name in [
            "mcp__nexus_browser__browser_open",
            "mcp__nexus_browser__browser_bypass",
            "mcp__nexus_app__terminal_run",
            "mcp__linear__save_issue",
            "WebFetch",
            "Bash",
            "browser_open",
        ] {
            assert!(!outside_tool(name), "{name}");
        }
        assert!(is_bypass("mcp__nexus_browser__browser_bypass"));
        assert!(!is_bypass("mcp__other__browser_bypass"));
    }

    #[test]
    fn a_bypass_needs_a_capability_and_a_reason() {
        let ok = json!({"capability": "network log", "reason": "The step must read the failed request's headers."});
        assert_eq!(bypass(&ok).expect("a bypass").capability, "network log");
        assert!(
            bypass_answer(&ok).expect("recorded")["recorded"]
                .as_bool()
                .unwrap_or_default()
        );
        for input in [
            json!({"reason": "The step must read the failed request's headers."}),
            json!({"capability": "network log", "reason": "because"}),
            json!({"capability": " ", "reason": "The step must read the failed request's headers."}),
            json!({"capability": "x".repeat(MAX_CAPABILITY + 1), "reason": "The step must read the headers."}),
        ] {
            assert!(bypass(&input).is_err(), "{input}");
        }
        assert_eq!(bypass_definition()["name"], BYPASS_TOOL);
    }

    #[test]
    fn the_gate_refuses_outside_tools_until_a_bypass() {
        let mut gate = Gate::default();
        let outside = "mcp__playwright__browser_navigate";
        // Without the Nexus browser tools there is nothing to route to.
        assert_eq!(gate.decide(outside), json!({}));
        gate.init(&["Bash".into(), "mcp__nexus_browser__browser_open".into()]);
        assert_eq!(gate.decide(outside)["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(gate.decide("mcp__nexus_browser__browser_open"), json!({}));
        assert_eq!(gate.decide("Bash"), json!({}));
        gate.call(
            "mcp__nexus_browser__browser_bypass",
            &json!({"capability": "x", "reason": "short"}),
        );
        assert_eq!(gate.decide(outside)["hookSpecificOutput"]["permissionDecision"], "deny");
        gate.call(
            "mcp__nexus_browser__browser_bypass",
            &json!({"capability": "network log", "reason": "The step must read the failed request's headers."}),
        );
        assert_eq!(gate.decide(outside), json!({}));
    }
}
