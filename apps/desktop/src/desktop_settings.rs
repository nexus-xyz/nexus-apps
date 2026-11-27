//! The Nexus app's `settings.json`, which this build shares: the Gateway, the organization, the MCP servers, the
//! worktree folders and rules, the servers, the cloud sign-ins' hints and the App choices. It reads and writes the file
//! through `nexus-desktop-core`'s `settings`, so every app that reads the file keeps one schema.
//!
//! Only the store's owner writes (`owner::owns_store`). While the Nexus app runs, it
//! holds the settings in memory and its next save would undo a change made here, so [`update`] refuses and the
//! Settings pages say why in one line ([`READ_ONLY`]).

use std::path::{Path, PathBuf};

use nexus_desktop_core::settings::{self, Settings};

/// A saved settings change, shared by open settings views.
pub(crate) struct Changed {
    pub settings: Settings,
    pub revision: u64,
}

impl Changed {
    pub fn revision(cx: &gpui_kit::App) -> u64 {
        cx.try_global::<Self>().map_or(0, |changed| changed.revision)
    }

    pub fn publish(settings: &Settings, cx: &mut gpui_kit::App) {
        cx.set_global(Self {
            settings: settings.clone(),
            revision: Self::revision(cx) + 1,
        });
    }
}

impl gpui_kit::Global for Changed {}

/// Why a Settings page of this computer does not change anything now.
pub const READ_ONLY: &str =
    "Read-only while the Nexus app runs. Quit it, or take over from Code, to change these here.";

/// `settings.json` in the Nexus app's config folder; None when the OS has no config folder.
pub fn file() -> Option<PathBuf> {
    crate::paths::person_dir(Path::new("config").join(settings::APP_IDENTIFIER), settings::config_dir)
        .map(|dir| settings::path(&dir))
}

/// The saved settings; the defaults when there are none.
pub fn load() -> Settings {
    file().map(|file| settings::load(&file)).unwrap_or_default()
}

/// Whether this build may write the settings now.
pub fn writable() -> bool {
    crate::owner::owns_store()
}

/// Read the file again, change it, and save it, as the Nexus app does (`AppState::update_settings`). `change` checks
/// its input first and refuses with a reason for the person; then nothing is saved. Answers the settings as saved and
/// what `change` answered.
pub fn update<T>(change: impl FnOnce(&mut Settings) -> Result<T, String>) -> Result<(Settings, T), String> {
    let file = file().ok_or("This computer has no config folder.")?;
    update_at(&file, writable(), change)
}

/// [`update`] on the file at `file`, by an owner (`owns`) or not.
fn update_at<T>(
    file: &Path,
    owns: bool,
    change: impl FnOnce(&mut Settings) -> Result<T, String>,
) -> Result<(Settings, T), String> {
    if !owns {
        return Err(READ_ONLY.into());
    }
    let mut settings = settings::load(file);
    let answer = change(&mut settings)?;
    settings::save(file, &settings).map_err(|error| format!("Could not save the settings: {error}"))?;
    Ok((settings, answer))
}

/// Settings' Gateway switch, on the settings (the Nexus app's `point_at`): make `to` the Gateway, keeping `from`'s
/// organization for a switch back when it is signed in. Answers `to` as saved.
pub fn switch_gateway(settings: &mut Settings, from: &str, signed_in: bool, to: &str) -> Result<String, String> {
    let to = settings::gateway_address(to)?;
    settings.switch_gateway(from, signed_in, &to);
    Ok(to)
}

/// A temporary `settings.json` for a test, removed when dropped.
#[cfg(test)]
pub struct TempFile(PathBuf);

#[cfg(test)]
impl TempFile {
    pub fn new(name: &str) -> TempFile {
        let dir = std::env::temp_dir().join(format!("nexus-gpui-settings-{name}-{}", std::process::id()));
        TempFile(dir.join(settings::FILE))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// [`update`] on this file, as its owner.
    pub fn update<T>(&self, change: impl FnOnce(&mut Settings) -> Result<T, String>) -> Result<(Settings, T), String> {
        update_at(&self.0, true, change)
    }
}

#[cfg(test)]
impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(dir) = self.0.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_desktop_core::settings::{McpServer, WorktreeRoot};

    #[test]
    fn load_edit_save_load_keeps_every_field() {
        let file = TempFile::new("round-trip");
        // Fields this build never edits survive its saves.
        let first = Settings {
            gateway_url: Some("https://gw.example.test".into()),
            org: Some("acme".into()),
            kind: Some("cli".into()),
            shortcut: Some("Alt+Shift+Space".into()),
            panel_size: Some([520.0, 680.0]),
            ..Settings::default()
        };
        settings::save(file.path(), &first).expect("save");
        let (saved, ()) = file
            .update(|settings| {
                settings.mcp_servers.push(McpServer {
                    name: "linear".into(),
                    enabled: true,
                    command: Some("npx".into()),
                    args: vec!["linear-mcp".into()],
                    url: None,
                    secrets: vec!["LINEAR_KEY".into()],
                    tool_timeout_sec: None,
                });
                settings.set_worktree_root(WorktreeRoot::Worktrees, Some("/w".into()))
            })
            .expect("update");
        let read = settings::load(file.path());
        assert_eq!(read, saved);
        assert_eq!(read.shortcut.as_deref(), Some("Alt+Shift+Space"));
        assert_eq!(read.panel_size, Some([520.0, 680.0]));
        assert_eq!(read.worktrees_dir.as_deref(), Some("/w"));
        assert_eq!(read.mcp_servers.len(), 1);
    }

    #[test]
    fn a_refused_change_saves_nothing() {
        let file = TempFile::new("refused");
        let refused =
            file.update(|settings| settings.set_worktree_root(WorktreeRoot::Reviews, Some("relative".into())));
        assert!(refused.is_err());
        assert!(!file.path().exists());
    }

    #[test]
    fn only_the_owner_writes() {
        let file = TempFile::new("not-owner");
        let refused = update_at(file.path(), false, |settings| {
            settings.org = Some("acme".into());
            Ok(())
        });
        assert_eq!(refused.map(|_| ()), Err(READ_ONLY.to_string()));
        assert!(!file.path().exists());
    }
}
