//! The sessions as a reader finds them in the store, while another process runs them: the desktop apps read the store
//! that their background service (nexus-host's `local` mode) holds. A reader has no agents, so it sees no live fields.

use std::path::{Path, PathBuf};

use crate::event::ThreadSummary;
use crate::store::{Result, Store};

/// The event log's file in a data folder.
pub const STORE_FILE: &str = "workbench.sqlite3";

/// Where a session works.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionFolder {
    pub path: PathBuf,
    /// What its worktree's branch was made from (`origin/main`); None in the project's own folder.
    pub base: Option<String>,
}

/// The store in the data folder `dir`; None when there is none yet.
pub fn open_in(dir: &Path) -> Result<Option<Store>> {
    let path = dir.join(STORE_FILE);
    if !path.exists() {
        return Ok(None);
    }
    Store::open(&path).map(Some)
}

/// [`open_in`] for reads only ([`Store::open_reading`]): what a reader on a UI thread opens.
pub fn open_reading_in(dir: &Path) -> Result<Option<Store>> {
    let path = dir.join(STORE_FILE);
    if !path.exists() {
        return Ok(None);
    }
    Store::open_reading(&path).map(Some)
}

/// The sessions of `scope` (`<gateway>|<org>`) as they are stored.
pub fn summaries(store: &Store, scope: &str) -> Result<Vec<ThreadSummary>> {
    Ok(store.threads(scope)?.into_iter().map(|record| record.summary).collect())
}

/// The sessions of `scope` in the store in the data folder `dir`.
pub fn summaries_in(dir: &Path, scope: &str) -> Result<Vec<ThreadSummary>> {
    let store = open_in(dir)?.ok_or("No Code sessions on this computer yet.")?;
    summaries(&store, scope)
}

/// Where a session works, as `Workbench::workdir` finds it: its worktree (a side chat's, its session's), else its
/// project's folder. An error when its worktree was taken off the disk.
pub fn folder(store: &Store, thread_id: &str) -> Result<SessionFolder> {
    let thread = |id: &str| store.thread(id)?.ok_or_else(|| "This session is gone.".to_string());
    let record = thread(thread_id)?;
    let worktree = match (&record.summary.side_chat, &record.summary.source_thread_id) {
        (true, Some(source)) => thread(source)?.summary.worktree,
        _ => record.summary.worktree.clone(),
    };
    match worktree {
        Some(worktree) if worktree.removed => Err("This session's worktree was removed.".into()),
        Some(worktree) => Ok(SessionFolder {
            path: worktree.path.into(),
            base: Some(worktree.base),
        }),
        None => {
            let project = store
                .project(&record.summary.project_id)?
                .ok_or_else(|| "This session's project is gone.".to_string())?;
            Ok(SessionFolder {
                path: project.path.into(),
                base: None,
            })
        }
    }
}
