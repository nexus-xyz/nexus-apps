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

/// One read-change-write of the settings at a time in this process: the Settings pages, the pull request watch and
/// the cloud sign-ins each change the file from their own thread.
static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// [`update`] on the file at `file`, by an owner (`owns`) or not. The read, the change and the write run as one under
/// [`WRITES`]: without it, two changes at once each read the file before the other wrote, and the later write drops
/// the earlier change. Only the store's owner writes ([`writable`]), so that lock covers this app's writers. The
/// advisory lock on `<file>.lock` ([`hold`]) only guards against a second writer that does not check ownership, such
/// as an older build of this app.
fn update_at<T>(
    file: &Path,
    owns: bool,
    change: impl FnOnce(&mut Settings) -> Result<T, String>,
) -> Result<(Settings, T), String> {
    if !owns {
        return Err(READ_ONLY.into());
    }
    let _one = WRITES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _held = hold(file);
    let mut settings = settings::load(file);
    let answer = change(&mut settings)?;
    settings::save(file, &settings).map_err(|error| format!("Could not save the settings: {error}"))?;
    Ok((settings, answer))
}

/// How long a write waits for another process's hold on `<file>.lock` before it saves anyway.
const HOLD_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// The advisory lock on `<file>.lock`, held until dropped. None when it can't be taken within [`HOLD_WAIT`] (another
/// process holds it, or there is no folder yet): the change is saved anyway, as before the lock existed, rather than
/// lost or left waiting.
fn hold(file: &Path) -> Option<std::fs::File> {
    let mut name = file.as_os_str().to_owned();
    name.push(".lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(PathBuf::from(name))
        .ok()?;
    let until = std::time::Instant::now() + HOLD_WAIT;
    loop {
        match lock.try_lock() {
            Ok(()) => return Some(lock),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
}

/// Settings' Gateway switch, on the settings (the Nexus app's `point_at`): make `to` the Gateway, keeping `from`'s
/// organization for a switch back when it is signed in. Answers `to` as saved.
pub fn switch_gateway(settings: &mut Settings, from: &str, signed_in: bool, to: &str) -> Result<String, String> {
    let to = settings::gateway_address(to)?;
    settings.switch_gateway(from, signed_in, &to);
    Ok(to)
}

/// A full path on this system, for a test's folder setting: `/w` is not one on Windows.
#[cfg(test)]
pub const FULL_PATH: &str = if cfg!(windows) { r"C:\w" } else { "/w" };

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

    /// The Subsessions page's cross-project switch, saved as the page saves it, reads back from the file and reaches
    /// the engine's settings.
    #[test]
    fn the_cross_project_switch_is_saved_to_the_file() {
        let file = TempFile::new("cross-project-switch");
        for on in [false, true] {
            let (saved, ()) = file
                .update(|settings| {
                    settings.set_cross_project_subsessions(on);
                    Ok(())
                })
                .expect("saved");
            assert_eq!(saved.cross_project_subsessions, Some(on));
            let read = settings::load(file.path());
            assert_eq!(read.cross_project_subsessions, Some(on), "the file keeps it");
            assert_eq!(read.subsessions().cross_project_on, on);
        }
    }

    /// Changes made at once from several threads all reach the file: one read-change-write never drops another's
    /// change, so a write of the pull request watch can't drop the cross-project switch saved beside it.
    #[test]
    fn changes_at_once_from_several_threads_all_reach_the_file() {
        let file = TempFile::new("at-once");
        let path = file.path().to_path_buf();
        let writers: Vec<_> = (0..16)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    update_at(&path, true, |settings| {
                        if n == 0 {
                            settings.set_cross_project_subsessions(false);
                        }
                        settings.mcp_servers.push(McpServer {
                            name: format!("server-{n}"),
                            enabled: true,
                            command: Some("x".into()),
                            args: Vec::new(),
                            url: None,
                            secrets: Vec::new(),
                            tool_timeout_sec: None,
                        });
                        // Widen the window between the read and the write, as a slow disk would.
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        Ok(())
                    })
                    .expect("saved");
                })
            })
            .collect();
        for writer in writers {
            writer.join().expect("a writer");
        }
        let read = settings::load(&path);
        assert_eq!(read.mcp_servers.len(), 16, "every change is kept");
        assert_eq!(read.cross_project_subsessions, Some(false));
    }

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
                settings.set_worktree_root(WorktreeRoot::Worktrees, Some(super::FULL_PATH.into()))
            })
            .expect("update");
        let read = settings::load(file.path());
        assert_eq!(read, saved);
        assert_eq!(read.shortcut.as_deref(), Some("Alt+Shift+Space"));
        assert_eq!(read.panel_size, Some([520.0, 680.0]));
        assert_eq!(read.worktrees_dir.as_deref(), Some(super::FULL_PATH));
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
