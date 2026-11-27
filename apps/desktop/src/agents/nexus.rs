use nexus_desktop_core::settings::Settings;

use super::{AgentUi, ServerSetup};
use crate::pages::subscriptions::login::Mode;

/// Nexus's own agent: it runs on the organization's Gateway budget, with no plan and no sign-in of its own.
pub(super) struct Nexus;

impl AgentUi for Nexus {
    fn short_name(&self) -> &'static str {
        "Nexus"
    }

    fn interrupt_title(&self) -> &'static str {
        "Stop this turn (Esc). The sub-agents it started may stop too."
    }

    fn fast_mode(&self) -> bool {
        false
    }

    fn reads_plan(&self) -> bool {
        false
    }

    fn sign_in_hint(&self) -> &'static str {
        "Sign in to Nexus."
    }

    fn hint_in_terminal(&self) -> bool {
        false
    }

    fn plan_name(&self, _plan: Option<&str>) -> String {
        "Nexus".into()
    }

    fn chosen_account(&self, _settings: &Settings) -> Option<String> {
        None
    }

    fn choose_account(&self, _settings: &mut Settings, _account: Option<String>) -> Result<(), String> {
        Err("Nexus runs on your organization's budget, not a plan account.".into())
    }

    /// It has no plan accounts; the setting it reads is the shared one.
    fn switches_accounts(&self, settings: &Settings) -> bool {
        settings.rotate_accounts.unwrap_or(true)
    }

    fn set_switches_accounts(&self, settings: &mut Settings, on: bool) {
        settings.rotate_accounts = Some(on);
    }

    fn sign_out_args(&self) -> Option<&'static [&'static str]> {
        None
    }

    fn banks_resets(&self) -> bool {
        false
    }

    fn login_args(&self, _email: Option<&str>, _mode: Mode) -> Result<Vec<String>, String> {
        Err("Nexus cannot sign in nexus.".into())
    }

    fn device_code(&self) -> bool {
        false
    }

    fn pastes_code(&self) -> bool {
        false
    }

    fn plan_needs_tier(&self) -> bool {
        false
    }

    fn extra_account_starts(&self) -> bool {
        false
    }

    fn windows_plan_note(&self) -> Option<&'static str> {
        None
    }

    fn signs_in_itself(&self, _api_key: bool, _name: &str, _plan: &str) -> Option<(String, String)> {
        None
    }

    /// A server installs the nexus CLI from the Servers page's own steps, not as an agent.
    fn server_setup(&self, _ssh: &str) -> Option<ServerSetup> {
        None
    }
}
