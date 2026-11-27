//! Where this build reads the Code sessions' store. Usually that is the file in the data folder (`workbench.sqlite3`).
//! With the managed policy's `AllowLocalCache` off, the runner keeps the store in memory
//! (`nexus_workbench::app_store`). The sidebar, Settings and the other readers then read that store through
//! [`open_in`], and nothing of the sessions is written to disk.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nexus_workbench::store::Store;

/// The runner's in-memory store and its data folder, while the runner holds it.
static MEMORY: Mutex<Option<(PathBuf, Store)>> = Mutex::new(None);

fn memory() -> std::sync::MutexGuard<'static, Option<(PathBuf, Store)>> {
    MEMORY.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The runner's hold on its in-memory store for the readers. The readers lose it when this is dropped.
pub struct Kept {
    data: PathBuf,
}

impl Drop for Kept {
    fn drop(&mut self) {
        let mut memory = memory();
        if memory.as_ref().is_some_and(|(data, _)| *data == self.data) {
            *memory = None;
        }
    }
}

/// Lets the readers of `data` read `store` when it is in memory. None for a store on disk: they open its file.
pub fn keep(data: &Path, store: &Store) -> Result<Option<Kept>, String> {
    if store.persistent() {
        return Ok(None);
    }
    *memory() = Some((data.to_path_buf(), store.reading()?));
    Ok(Some(Kept {
        data: data.to_path_buf(),
    }))
}

/// The store the readers of the data folder `data` read. None when there is none yet, or when the policy keeps
/// nothing on disk and this build does not hold the store: the Nexus app then keeps it in its own memory.
pub fn open_in(data: &Path) -> Result<Option<Store>, String> {
    if let Some((kept, store)) = memory().as_ref()
        && kept == data
    {
        return store.reading().map(Some);
    }
    if !crate::managed::local_cache(crate::managed::policy()) {
        return Ok(None);
    }
    nexus_workbench::reader::open_in(data)
}

/// [`open_in`] for reads only: no schema work and no write, so the UI thread never waits for a running session's write
/// lock.
pub fn open_reading_in(data: &Path) -> Result<Option<Store>, String> {
    if let Some((kept, store)) = memory().as_ref()
        && kept == data
    {
        return store.reading().map(Some);
    }
    if !crate::managed::local_cache(crate::managed::policy()) {
        return Ok(None);
    }
    nexus_workbench::reader::open_reading_in(data)
}

/// [`open_in`] on this computer's data folder.
pub fn open() -> Result<Option<Store>, String> {
    match crate::paths::data_dir() {
        Some(data) => open_in(&data),
        None => Ok(None),
    }
}
