use nexus_desktop_core::settings::Settings;

use super::{AgentUi, ServerSetup};
use crate::pages::subscriptions::login::Mode;

/// OpenAI's Codex.
pub(super) struct Codex;

impl AgentUi for Codex {
    fn short_name(&self) -> &'static str {
        "Codex"
    }

    fn interrupt_title(&self) -> &'static str {
        "Stop this turn (Esc). Codex may stop its sub-agents too."
    }

    fn fast_mode(&self) -> bool {
        false
    }

    fn reads_plan(&self) -> bool {
        true
    }

    fn sign_in_hint(&self) -> &'static str {
        "Sign in with ChatGPT, not an API key."
    }

    /// Its terminal sign-in asks how to sign in, so the card says which to pick.
    fn hint_in_terminal(&self) -> bool {
        true
    }

    /// Codex names only the tier (`plus`), shown as `ChatGPT Plus`.
    fn plan_name(&self, plan: Option<&str>) -> String {
        match plan.filter(|tier| !tier.is_empty()) {
            Some(tier) => format!("ChatGPT {}", crate::pages::subscriptions::tier_name(tier)),
            None => "ChatGPT".into(),
        }
    }

    fn chosen_account(&self, settings: &Settings) -> Option<String> {
        settings.codex_account.clone()
    }

    fn choose_account(&self, settings: &mut Settings, account: Option<String>) -> Result<(), String> {
        settings.codex_account = account;
        Ok(())
    }

    /// Off unless Settings turned it on.
    fn switches_accounts(&self, settings: &Settings) -> bool {
        settings.rotate_codex_accounts.unwrap_or(false)
    }

    fn set_switches_accounts(&self, settings: &mut Settings, on: bool) {
        settings.rotate_codex_accounts = Some(on);
    }

    fn sign_out_args(&self) -> Option<&'static [&'static str]> {
        Some(&["logout"])
    }

    fn banks_resets(&self) -> bool {
        true
    }

    fn login_args(&self, _email: Option<&str>, mode: Mode) -> Result<Vec<String>, String> {
        Ok(match mode {
            Mode::Browser => vec!["login".into()],
            Mode::DeviceCode => vec!["login".into(), "--device-auth".into()],
        })
    }

    fn device_code(&self) -> bool {
        true
    }

    fn pastes_code(&self) -> bool {
        false
    }

    /// Codex reads a ChatGPT login with no plan tier as signed in; it has nothing to pay with.
    fn plan_needs_tier(&self) -> bool {
        true
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

    fn server_setup(&self, ssh: &str) -> Option<ServerSetup> {
        let commands = crate::machine_view::server_commands(ssh);
        Some(ServerSetup {
            install: commands.codex_install,
            login: commands.codex_login,
            how: "To use your ChatGPT plan there, run this and follow its link:",
        })
    }
}
