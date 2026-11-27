//! Durable, sign-in-scoped recovery for sessions stopped by an app update. A claim is saved before Continue goes out.

use std::path::{Path, PathBuf};

use nexus_workbench::update_restart::Session;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub session: Session,
    #[serde(default)]
    pub continuing: bool,
    #[serde(skip)]
    pub error: Option<String>,
}

pub struct Recovery {
    file: Option<PathBuf>,
    entries: Vec<Entry>,
    pub hidden: bool,
    pub installing: bool,
    pub error: Option<String>,
}

impl gpui_kit::Global for Recovery {}

impl Recovery {
    pub fn load(file: Option<PathBuf>) -> Self {
        let read = file.as_deref().map(read).transpose();
        let (entries, error) = match read {
            Ok(entries) => (entries.unwrap_or_default(), None),
            Err(error) => (Vec::new(), Some(error)),
        };
        Self {
            file,
            entries,
            hidden: false,
            installing: false,
            error,
        }
    }

    pub fn pending(&self, scope: &str, identity: &str) -> Vec<Entry> {
        self.entries
            .iter()
            .filter(|entry| entry.session.scope == scope && entry.session.identity == identity)
            .cloned()
            .collect()
    }

    /// Refuse to interrupt work if it cannot survive the restart. Older deferred recovery stays intact.
    pub fn prepare(&mut self, sessions: &[Session]) -> Result<Vec<Entry>, String> {
        if self.installing {
            return Err("An update restart is already in progress.".into());
        }
        if self.entries.iter().any(|entry| entry.continuing) {
            return Err("Wait for the agents to finish continuing before restarting again.".into());
        }
        if sessions.is_empty() {
            self.installing = true;
            return Ok(self.entries.clone());
        }
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if !sessions.is_empty() && (self.file.is_none() || sessions.iter().any(|session| session.identity.is_empty())) {
            return Err("Nexus cannot save recovery for this sign-in. Finish your agents before restarting.".into());
        }
        let before = self.entries.clone();
        let mut entries = before.clone();
        for session in sessions {
            entries.retain(|entry| {
                entry.session.id != session.id
                    || entry.session.identity != session.identity
                    || entry.session.scope != session.scope
            });
            entries.push(Entry {
                session: session.clone(),
                continuing: false,
                error: None,
            });
        }
        self.save(&entries)?;
        self.entries = entries;
        self.installing = true;
        Ok(before)
    }

    /// Installation failed before agents stopped. Do not offer those still-running sessions as interrupted.
    pub fn cancel(&mut self, before: Vec<Entry>) {
        self.installing = false;
        if let Err(error) = self.save(&before) {
            self.error = Some(error);
        }
        self.entries = before;
    }

    pub fn claim(&mut self, scope: &str, identity: &str, selected: &[String]) -> Result<Vec<Entry>, String> {
        if self.installing {
            return Err("Wait for the update restart to finish.".into());
        }
        let mut entries = self.entries.clone();
        let mut claimed = Vec::new();
        for entry in &mut entries {
            if entry.session.scope == scope
                && entry.session.identity == identity
                && !entry.continuing
                && selected.contains(&entry.session.id)
            {
                entry.continuing = true;
                entry.error = None;
                claimed.push(entry.clone());
            }
        }
        self.save(&entries)?;
        self.entries = entries;
        Ok(claimed)
    }

    pub fn finish(&mut self, entry: &Entry, result: Result<(), String>) {
        let Some(index) = self.entries.iter().position(|pending| {
            pending.session.id == entry.session.id
                && pending.session.scope == entry.session.scope
                && pending.session.identity == entry.session.identity
        }) else {
            return;
        };
        match result {
            Ok(()) => {
                self.entries.remove(index);
            }
            Err(error) => {
                self.entries[index].continuing = false;
                self.entries[index].error = Some(error);
            }
        }
        if let Err(error) = self.save(&self.entries) {
            self.error = Some(error);
        }
    }

    fn save(&self, entries: &[Entry]) -> Result<(), String> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        write(file, entries)
            .map_err(|error| format!("Couldn't save agent recovery: {error}. Try again before restarting."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, scope: &str) -> Session {
        Session {
            id: id.into(),
            scope: scope.into(),
            identity: "person".into(),
            last_user: 3,
            last_turn_end: 0,
        }
    }

    #[test]
    fn update_recovery_survives_relaunch_and_scopes_every_claim() {
        let root = std::env::temp_dir().join(format!("nexus-update-recovery-{}", uuid::Uuid::new_v4()));
        let file = root.join("recovery.json");
        let sessions = vec![
            session("first", "gateway|one"),
            session("second", "gateway|one"),
            session("other", "gateway|two"),
        ];
        let mut recovery = Recovery::load(Some(file.clone()));
        recovery.prepare(&sessions).unwrap();
        let mut reopened = Recovery::load(Some(file.clone()));
        assert_eq!(reopened.pending("gateway|one", "person").len(), 2);
        assert!(reopened.pending("gateway|one", "another-sign-in").is_empty());
        let selected = vec!["first".into(), "other".into()];
        let claimed = reopened.claim("gateway|one", "person", &selected).unwrap();
        assert_eq!(claimed.len(), 1, "another organization cannot be claimed");
        assert!(
            reopened.prepare(&sessions).is_err(),
            "a pending continuation cannot race another install"
        );
        assert!(
            reopened.claim("gateway|one", "person", &selected).unwrap().is_empty(),
            "windows share the same claim"
        );
        reopened.finish(&claimed[0], Ok(()));
        let mut reopened = Recovery::load(Some(file));
        let retry = reopened.claim("gateway|one", "person", &["second".into()]).unwrap();
        reopened.finish(&retry[0], Err("Project folder unavailable".into()));
        assert_eq!(reopened.pending("gateway|one", "person").len(), 1);
        assert!(reopened.pending("gateway|one", "person")[0].error.is_some());
        assert_eq!(
            reopened
                .claim("gateway|one", "person", &["second".into()])
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            Recovery::load(reopened.file.clone())
                .pending("gateway|one", "person")
                .len(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_install_restores_deferred_recovery_and_no_cache_refuses_to_stop_work() {
        let mut recovery = Recovery::load(None);
        assert!(recovery.prepare(&[session("working", "scope")]).is_err());
        let root = std::env::temp_dir().join(format!("nexus-update-recovery-{}", uuid::Uuid::new_v4()));
        let file = root.join("recovery.json");
        let mut recovery = Recovery::load(Some(file.clone()));
        recovery.prepare(&[session("older", "scope")]).unwrap();
        recovery.installing = false;
        let before = recovery.prepare(&[session("newer", "scope")]).unwrap();
        recovery.cancel(before);
        let restored = Recovery::load(Some(file));
        assert_eq!(restored.pending("scope", "person")[0].session.id, "older");
        assert_eq!(restored.pending("scope", "person").len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_writes_and_corrupt_recovery_never_claim_or_discard_work() {
        let root = std::env::temp_dir().join(format!("nexus-update-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("recovery.json");
        std::fs::write(&file, "bad json").unwrap();
        let mut recovery = Recovery::load(Some(file.clone()));
        assert!(recovery.prepare(&[session("first", "scope")]).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "bad json");
        std::fs::remove_file(&file).unwrap();
        std::fs::write(&file, "[]").unwrap();
        let mut recovery = Recovery::load(Some(file.clone()));
        recovery.prepare(&[session("first", "scope")]).unwrap();
        recovery.installing = false;
        // Replacing the file's parent with a file forces a deterministic write failure on every OS.
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::write(&root, "not a directory").unwrap();
        assert!(recovery.claim("scope", "person", &["first".into()]).is_err());
        assert!(!recovery.pending("scope", "person")[0].continuing);
        std::fs::remove_file(root).unwrap();
    }

    #[test]
    fn an_idle_restart_preserves_unreadable_recovery_without_writing_it() {
        let root = std::env::temp_dir().join(format!("nexus-update-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("recovery.json");
        std::fs::write(&file, "bad json").unwrap();
        let mut recovery = Recovery::load(Some(file.clone()));
        assert!(recovery.prepare(&[session("working", "scope")]).is_err());
        assert!(recovery.prepare(&[]).unwrap().is_empty());
        assert!(recovery.installing);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "bad json");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_idle_restart_preserves_deferred_recovery_after_a_write_failure() {
        let mut recovery = Recovery::load(None);
        recovery.entries.push(Entry {
            session: session("deferred", "scope"),
            continuing: false,
            error: None,
        });
        recovery.error = Some("Couldn't save agent recovery".into());
        assert!(recovery.prepare(&[session("working", "scope")]).is_err());
        let before = recovery.prepare(&[]).unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].session.id, "deferred");
        assert_eq!(recovery.pending("scope", "person")[0].session.id, "deferred");
        assert!(recovery.installing);
    }
}

fn read(file: &Path) -> Result<Vec<Entry>, String> {
    let text = match std::fs::read(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Couldn't read saved agent recovery: {error}.")),
    };
    let mut entries: Vec<Entry> =
        serde_json::from_slice(&text).map_err(|error| format!("Couldn't read saved agent recovery: {error}."))?;
    for entry in &mut entries {
        if entry.continuing {
            entry.continuing = false;
            entry.error = Some("The last continuation was interrupted. Check this session before retrying.".into());
        }
    }
    Ok(entries)
}

fn write(file: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let parent = file.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    let mut dir = std::fs::DirBuilder::new();
    dir.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        dir.mode(0o700);
    }
    dir.create(parent)?;
    nexus_desktop_core::atomic_file::write_atomic_private(file, &serde_json::to_vec(entries)?)
}
