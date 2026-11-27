use nexus_desktop_core::settings::Settings;

use super::{AgentUi, ServerSetup};
use crate::pages::subscriptions::login::Mode;

/// Google's Antigravity (`agy`): one Google sign-in, which it makes on its own first start.
pub(super) struct Antigravity;

impl AgentUi for Antigravity {
    fn short_name(&self) -> &'static str {
        "Antigravity"
    }

    fn interrupt_title(&self) -> &'static str {
        "Stop this turn (Esc). The sub-agents it started may stop too."
    }

    fn fast_mode(&self) -> bool {
        false
    }

    fn reads_plan(&self) -> bool {
        true
    }

    fn sign_in_hint(&self) -> &'static str {
        "Sign in with Google, not a Gemini API key."
    }

    fn hint_in_terminal(&self) -> bool {
        false
    }

    /// "Google AI", or "Google Cloud · <project>" for a business sign-in (`plans::antigravity`).
    fn plan_name(&self, plan: Option<&str>) -> String {
        plan.map_or_else(|| "Google AI".into(), String::from)
    }

    fn chosen_account(&self, _settings: &Settings) -> Option<String> {
        None
    }

    fn choose_account(&self, _settings: &mut Settings, _account: Option<String>) -> Result<(), String> {
        Err("Antigravity has no plan accounts in Nexus.".into())
    }

    /// It has no extra accounts; the setting it reads is the shared one.
    fn switches_accounts(&self, settings: &Settings) -> bool {
        settings.rotate_accounts.unwrap_or(true)
    }

    fn set_switches_accounts(&self, settings: &mut Settings, on: bool) {
        settings.rotate_accounts = Some(on);
    }

    /// `/logout` does not undo its API-key setting, so Nexus runs no sign-out for it.
    fn sign_out_args(&self) -> Option<&'static [&'static str]> {
        None
    }

    fn banks_resets(&self) -> bool {
        false
    }

    fn login_args(&self, _email: Option<&str>, _mode: Mode) -> Result<Vec<String>, String> {
        Err("Nexus cannot sign in antigravity.".into())
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

    /// Its check opens a browser when signed out, and Nexus can stop that on Windows only with launchers on `PATH`
    /// (`local_agents::no_browser_dir`), so the check never runs there.
    fn windows_plan_note(&self) -> Option<&'static str> {
        Some("Nexus for desktop shows Google AI usage on macOS and Linux. On Windows, check it in agy with /usage.")
    }

    /// agy signs in on its first interactive start. Its settings file selects API-key mode with `modelProvider`.
    fn signs_in_itself(&self, api_key: bool, name: &str, plan: &str) -> Option<(String, String)> {
        let settings = format!(
            "~/{}/settings.json",
            nexus_client::agent_entries::Agent::Antigravity.entry().profile.dir
        );
        Some(if api_key {
            (
                format!("{name} is set to use an API key, not your {plan} plan"),
                format!("Remove \"modelProvider\" from {settings}, then check again."),
            )
        } else {
            (
                format!("{name} isn't signed in to your {plan} plan"),
                "Run `agy` in a terminal and sign in with Google, then check again.".into(),
            )
        })
    }

    /// Its Google sign-in stays on this computer: it runs on this computer only.
    fn server_setup(&self, _ssh: &str) -> Option<ServerSetup> {
        None
    }
}
