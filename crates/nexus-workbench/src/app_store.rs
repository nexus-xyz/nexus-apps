//! The store a desktop app opens in its data folder, as the managed policy allows (`AllowLocalCache`). Nexus GPUI
//! opens it here.
//!
//! - Allowed: the event log on disk (`workbench.sqlite3`). When it does not open, the sessions stay in memory.
//! - Not allowed: everything in memory. Only the open Nexus sessions' ids are written down
//!   (`workbench-sessions.sqlite3`, no content), so a crash can still end them in Gateway at the next launch.

use std::path::Path;

use crate::reader::STORE_FILE;
use crate::store::{Result, Store};

/// The ledger of open Nexus sessions, beside the store, while the store is in memory.
pub const LEDGER_FILE: &str = "workbench-sessions.sqlite3";

/// The store in the data folder `data`: on disk when `allow_local_cache`, else in memory with its ledger.
pub fn open(data: &Path, allow_local_cache: bool) -> Result<Store> {
    if !allow_local_cache {
        return in_memory(data);
    }
    Store::open(&data.join(STORE_FILE)).or_else(|error| {
        eprintln!("nexus-workbench: workbench store: {error}; keeping sessions in memory");
        in_memory(data)
    })
}

/// Everything in memory but the ledger; nothing at all on disk when the ledger does not open either.
fn in_memory(data: &Path) -> Result<Store> {
    Store::in_memory_with_ledger(&data.join(LEDGER_FILE)).or_else(|error| {
        eprintln!("nexus-workbench: workbench session ledger: {error}; a crash will leave Nexus sessions open");
        Store::in_memory()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-app-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn with_the_cache_off_only_the_ledger_is_written() -> Result<()> {
        let dir = folder("off");
        let store = open(&dir, false)?;
        assert!(!store.persistent());
        let project = store.add_project(&dir, 1)?;
        assert_eq!(store.projects()?.len(), 1, "the project is kept, in memory");
        drop(store);
        assert_eq!(files(&dir), vec![LEDGER_FILE.to_string()], "no event log on disk");
        // A restart finds nothing: the project left with the app.
        assert!(open(&dir, false)?.project(&project.id)?.is_none());
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn with_the_cache_on_the_event_log_is_on_disk() -> Result<()> {
        let dir = folder("on");
        let store = open(&dir, true)?;
        assert!(store.persistent());
        let project = store.add_project(&dir, 1)?;
        drop(store);
        assert!(dir.join(STORE_FILE).exists());
        assert!(
            open(&dir, true)?.project(&project.id)?.is_some(),
            "kept across a restart"
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
