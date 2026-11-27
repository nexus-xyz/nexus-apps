//! Existing settings and terminal APIs, wired to the subscription account controls.
use nexus_desktop_core::{settings::Settings, terminal_window};
use nexus_workbench::{accounts, event::SessionAgent};
use std::path::{Path, PathBuf};

/// Validate before changing settings, so a removed account never becomes the saved default.
pub(super) fn select(
    settings: &mut Settings,
    root: &Path,
    agent: SessionAgent,
    account: Option<String>,
) -> Result<(), String> {
    if let Some(id) = &account {
        accounts::dir(root, agent, id)?;
    }
    crate::agents::ui(agent).choose_account(settings, account.clone())?;
    if !settings.chosen_accounts.iter().any(|name| name == agent.as_str()) {
        settings.chosen_accounts.push(agent.as_str().into());
    }
    // The window's organization keeps the choice: a switch back to it starts there again.
    if let Some(org) = super::current_org() {
        settings.remember_org_account(&org, agent.as_str(), account.as_deref());
    }
    Ok(())
}

pub(super) fn login_script(agent: SessionAgent, account: Option<&Path>, cache: &Path) -> Result<PathBuf, String> {
    let login = terminal_window::login_command(agent.as_str())
        .ok_or_else(|| format!("Nexus cannot sign in {}.", agent.as_str()))?;
    let directory = account.map(|dir| dir.to_string_lossy().into_owned());
    let env = directory
        .as_deref()
        .and_then(|dir| accounts::account_variable(agent).map(|variable| (variable, dir)));
    terminal_window::write_login_script(cache, agent.as_str(), env, &login)
}

pub(crate) fn connect(agent: SessionAgent, account: Option<&Path>) -> Result<(), String> {
    super::login::check_login_storage(agent)?;
    let script = login_script(agent, account, &crate::paths::cache_dir().join("resume"))?;
    terminal_window::open(&script)
}

#[cfg(test)]
mod tests {
    use super::{login_script, select};
    use crate::desktop_settings::TempFile;
    use nexus_workbench::event::SessionAgent;

    #[test]
    fn selecting_an_account_validates_its_folder_and_preserves_other_settings() {
        let file = TempFile::new("subscription-account");
        let root = file.path().with_extension("accounts");
        std::fs::create_dir_all(root.join("claude/a_123")).expect("temporary account");
        let (saved, ()) = file
            .update(|settings| {
                settings.rotate_accounts = Some(false);
                settings.gateway_url = Some("https://example.test".into());
                select(settings, &root, SessionAgent::Claude, Some("a_123".into()))
            })
            .expect("valid account saves");
        assert_eq!(saved.claude_account.as_deref(), Some("a_123"));
        assert_eq!(saved.rotate_accounts, Some(false));
        assert!(
            file.update(|settings| select(settings, &root, SessionAgent::Claude, Some("../other".into())))
                .is_err()
        );
        let (saved, ()) = file
            .update(|settings| {
                assert_eq!(settings.claude_account.as_deref(), Some("a_123"));
                select(settings, &root, SessionAgent::Claude, None)
            })
            .expect("default login saves");
        assert_eq!(saved.claude_account, None);
        // A ChatGPT account is Codex's choice, apart from Claude's.
        std::fs::create_dir_all(root.join("codex/a_456")).expect("temporary account");
        let (saved, ()) = file
            .update(|settings| select(settings, &root, SessionAgent::Codex, Some("a_456".into())))
            .expect("a ChatGPT account saves");
        assert_eq!(saved.codex_account.as_deref(), Some("a_456"));
        assert_eq!(saved.claude_account, None);
        assert!(
            file.update(|settings| select(settings, &root, SessionAgent::Codex, Some("a_123".into())))
                .is_err(),
            "a Claude account is not a ChatGPT one"
        );
        assert_eq!(saved.gateway_url.as_deref(), Some("https://example.test"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn login_scripts_use_the_selected_accounts_environment_without_running_a_terminal() {
        let file = TempFile::new("subscription-login");
        let cache = file.path().with_extension("scripts");
        for (agent, variable, command) in [
            (SessionAgent::Claude, "CLAUDE_CONFIG_DIR", "claude auth login"),
            (SessionAgent::Codex, "CODEX_HOME", "codex login"),
        ] {
            let account = cache.join("account with spaces");
            let path = login_script(agent, Some(&account), &cache).expect("temporary login script");
            let script = std::fs::read_to_string(path).expect("login script");
            assert!(script.contains(variable));
            assert!(script.contains("account with spaces"));
            assert!(script.contains(command));
        }
        assert!(login_script(SessionAgent::Nexus, None, &cache).is_err());
        let _ = std::fs::remove_dir_all(cache);
    }
}
