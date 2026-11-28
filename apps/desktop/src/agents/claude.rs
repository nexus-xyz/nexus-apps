use nexus_desktop_core::settings::Settings;

use super::{AgentUi, ServerSetup};
use crate::pages::subscriptions::login::Mode;

/// Claude Code.
pub(super) struct Claude;

impl AgentUi for Claude {
    fn short_name(&self) -> &'static str {
        "Claude"
    }

    fn interrupt_title(&self) -> &'static str {
        "Stop this turn (Esc). The sub-agents it started may stop too."
    }

    fn reads_plan(&self) -> bool {
        true
    }

    fn sign_in_hint(&self) -> &'static str {
        "Choose your Claude account, not Console."
    }

    fn hint_in_terminal(&self) -> bool {
        false
    }

    /// Claude Code names its plans (`Claude Team`).
    fn plan_name(&self, plan: Option<&str>) -> String {
        plan.map_or_else(|| "Claude".into(), String::from)
    }

    fn chosen_account(&self, settings: &Settings) -> Option<String> {
        settings.claude_account.clone()
    }

    fn choose_account(&self, settings: &mut Settings, account: Option<String>) -> Result<(), String> {
        settings.claude_account = account;
        Ok(())
    }

    /// On unless Settings turned it off.
    fn switches_accounts(&self, settings: &Settings) -> bool {
        settings.rotate_accounts.unwrap_or(true)
    }

    fn set_switches_accounts(&self, settings: &mut Settings, on: bool) {
        settings.rotate_accounts = Some(on);
    }

    fn sign_out_args(&self) -> Option<&'static [&'static str]> {
        Some(&["auth", "logout"])
    }

    fn banks_resets(&self) -> bool {
        false
    }

    /// `--claudeai` signs in to the plan, not Console. A known email is filled in.
    fn login_args(&self, email: Option<&str>, _mode: Mode) -> Result<Vec<String>, String> {
        let mut args: Vec<String> = ["auth", "login", "--claudeai"].map(String::from).into();
        if let Some(email) = email.filter(|email| !email.is_empty()) {
            args.extend(["--email".into(), email.into()]);
        }
        Ok(args)
    }

    fn device_code(&self) -> bool {
        false
    }

    fn pastes_code(&self) -> bool {
        true
    }

    fn plan_needs_tier(&self) -> bool {
        false
    }

    fn extra_account_starts(&self) -> bool {
        true
    }

    fn windows_plan_note(&self) -> Option<&'static str> {
        None
    }

    fn signs_in_itself(&self, _api_key: bool, _name: &str, _plan: &str) -> Option<(String, String)> {
        None
    }

    fn server_setup(&self, ssh: &str) -> Option<ServerSetup> {
        let commands = crate::machine_view::server_commands(ssh);
        Some(ServerSetup {
            install: commands.claude_install,
            login: commands.claude_login,
            how: "To use your Claude plan there, run this, then type /login and follow its link:",
        })
    }
}
