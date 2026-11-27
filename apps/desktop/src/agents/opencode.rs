use nexus_desktop_core::settings::Settings;

use super::{AgentUi, ServerSetup};
use crate::pages::subscriptions::login::Mode;

/// OpenCode uses the provider configured in OpenCode.
pub(super) struct Opencode;

impl AgentUi for Opencode {
    fn short_name(&self) -> &'static str {
        "OpenCode"
    }

    fn interrupt_title(&self) -> &'static str {
        "Stop this turn (Esc). The sub-agents it started may stop too."
    }

    fn fast_mode(&self) -> bool {
        false
    }

    /// OpenCode's provider owns billing and sign-in.
    fn reads_plan(&self) -> bool {
        false
    }

    fn sign_in_hint(&self) -> &'static str {
        "Configure a provider with `opencode auth login`."
    }

    fn hint_in_terminal(&self) -> bool {
        false
    }

    fn plan_name(&self, plan: Option<&str>) -> String {
        plan.map_or_else(|| "OpenCode".into(), String::from)
    }

    fn chosen_account(&self, _settings: &Settings) -> Option<String> {
        None
    }

    fn choose_account(&self, _settings: &mut Settings, _account: Option<String>) -> Result<(), String> {
        Err("Manage your provider in OpenCode.".into())
    }

    /// It has no extra accounts; the setting it reads is the shared one.
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
        Err("Configure a provider with `opencode auth login`.".into())
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

    fn server_setup(&self, _ssh: &str) -> Option<ServerSetup> {
        None
    }
}
