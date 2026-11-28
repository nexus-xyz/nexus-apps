//! Each session agent's own words and controls in Nexus Desktop, one file per agent: its name on the cards, its plan
//! and sign-in controls, and what a server needs of it. [`AgentUi`] is this app's own trait, as `Harness` is the
//! workbench's and `LocalAgentT` nexus-local-agents'. [`ui`] is the one dispatch, so a new agent does not build here
//! until its file states each method. Facts that more than this app reads stay in the agent's `nexus-agents` entry or
//! its `Harness`.

use nexus_desktop_core::settings::Settings;
use nexus_workbench::event::SessionAgent;

use crate::pages::subscriptions::login::Mode;

mod antigravity;
mod claude;
mod codex;
mod cursor;
mod grok;
mod nexus;
mod opencode;
pub(crate) mod setup;

/// What a person runs on a server to set an agent up there. Nexus never sees these sign-ins.
pub(crate) struct ServerSetup {
    /// How to install it there.
    pub install: String,
    /// How to sign it in there.
    pub login: String,
    /// What the page says before the sign-in command.
    pub how: &'static str,
}

/// The trait every session agent implements, in `src/agents/<agent>.rs`. No method has a default body: a capability an
/// agent lacks is a stated value.
pub(crate) trait AgentUi: Sync {
    /// Its name on the Code cards and in "Signed … out of Claude" (`Claude`, where its label is `Claude Code`).
    fn short_name(&self) -> &'static str;
    /// The Stop button's tooltip while a turn runs (`interrupt.ts`'s `interruptTitle`).
    fn interrupt_title(&self) -> &'static str;
    /// Whether its models may run in fast mode.
    fn fast_mode(&self) -> bool;
    /// Whether Settings › Subscriptions reads its plan: it has a section there.
    fn reads_plan(&self) -> bool;
    /// What to pick in its sign-in so the login bills the plan.
    fn sign_in_hint(&self) -> &'static str;
    /// Whether the sign-in card repeats [`AgentUi::sign_in_hint`] above the terminal command.
    fn hint_in_terminal(&self) -> bool;
    /// The plan's name, from what its reading names (`plan`).
    fn plan_name(&self, plan: Option<&str>) -> String;
    /// The account new sessions start on, as Settings saved it.
    fn chosen_account(&self, settings: &Settings) -> Option<String>;
    /// Saves the account new sessions start on, or says why it has none.
    fn choose_account(&self, settings: &mut Settings, account: Option<String>) -> Result<(), String>;
    /// Whether its sessions take the account with the most left near a limit, as Settings saved it.
    fn switches_accounts(&self, settings: &Settings) -> bool;
    /// Saves whether its sessions switch accounts near a limit.
    fn set_switches_accounts(&self, settings: &mut Settings, on: bool);
    /// The arguments that sign its login out. None: Nexus has no sign-out to run for it.
    fn sign_out_args(&self) -> Option<&'static [&'static str]>;
    /// Whether its login banks limit resets that a press can use.
    fn banks_resets(&self) -> bool;
    /// The arguments of its sign-in, or why Nexus cannot sign it in.
    fn login_args(&self, email: Option<&str>, mode: Mode) -> Result<Vec<String>, String>;
    /// Whether its sign-in can take a device code in place of the browser's callback.
    fn device_code(&self) -> bool;
    /// Whether its sign-in takes a code pasted back from the browser.
    fn pastes_code(&self) -> bool;
    /// Whether a signed-in login with no plan tier has nothing to pay with.
    fn plan_needs_tier(&self) -> bool;
    /// Whether a signed-in extra account lets a new session start before its own login is read.
    fn extra_account_starts(&self) -> bool;
    /// What its section says on Windows in place of its plan's usage. None: the page reads its plan there too.
    fn windows_plan_note(&self) -> Option<&'static str>;
    /// The start blocker's title and text when its plan is not signed in, for an agent that signs in on its own (agy).
    /// None: the blocker offers Nexus's own sign-in. `api_key`: its login reads as an API key.
    fn signs_in_itself(&self, api_key: bool, name: &str, plan: &str) -> Option<(String, String)>;
    /// What a person runs on the server `ssh` to set it up there. None: a server check does not look for it.
    fn server_setup(&self, ssh: &str) -> Option<ServerSetup>;
}

/// The agent's [`AgentUi`]. A new session agent does not build here until it names its file.
pub(crate) fn ui(agent: SessionAgent) -> &'static dyn AgentUi {
    match agent {
        SessionAgent::Claude => &claude::Claude,
        SessionAgent::Codex => &codex::Codex,
        SessionAgent::Nexus => &nexus::Nexus,
        SessionAgent::Antigravity => &antigravity::Antigravity,
        SessionAgent::Cursor => &cursor::Cursor,
        SessionAgent::Grok => &grok::Grok,
        SessionAgent::Opencode => &opencode::Opencode,
    }
}

/// The agents Settings › Subscriptions reads, in the app's order.
pub(crate) fn plan_agents() -> Vec<SessionAgent> {
    SessionAgent::ALL
        .into_iter()
        .filter(|agent| ui(*agent).reads_plan())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No method of [`AgentUi`] has a default body: each agent's file writes every one.
    #[test]
    fn agent_ui_has_no_default_bodies() {
        let source = include_str!("mod.rs");
        let start = source
            .find("pub(crate) trait AgentUi: Sync {")
            .unwrap_or_else(|| panic!("no AgentUi trait"));
        let body = &source[start..];
        let end = body.find("\n}\n").unwrap_or_else(|| panic!("the trait does not close"));
        let body = &body[body.find('{').map_or(0, |at| at + 1)..end];
        for line in body.lines().map(str::trim).filter(|line| !line.starts_with("//")) {
            assert!(!line.contains('{'), "an AgentUi method has a default body: {line}");
        }
        assert!(body.matches("fn ").count() >= 10, "the trait's methods were not read");
    }

    #[test]
    fn the_plan_page_reads_claude_codex_and_antigravity_in_order() {
        assert_eq!(
            plan_agents(),
            [SessionAgent::Claude, SessionAgent::Codex, SessionAgent::Antigravity]
        );
    }

    #[test]
    fn short_names_and_stop_hints() {
        let names: Vec<&str> = SessionAgent::ALL.map(|agent| ui(agent).short_name()).to_vec();
        assert_eq!(
            names,
            ["Claude", "Codex", "Nexus", "Antigravity", "Cursor", "OpenCode", "Grok"]
        );
        for agent in SessionAgent::ALL {
            let title = ui(agent).interrupt_title();
            if agent == SessionAgent::Codex {
                assert_eq!(title, "Stop this turn (Esc). Codex may stop its sub-agents too.");
            } else {
                assert_eq!(title, "Stop this turn (Esc). The sub-agents it started may stop too.");
            }
            assert_eq!(ui(agent).fast_mode(), agent == SessionAgent::Claude, "{agent:?}");
        }
    }
}
