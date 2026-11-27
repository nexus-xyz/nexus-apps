//! One process owns a local session store. Recovery must not interrupt agents in another running build.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

/// Held until the app or host exits. The operating system releases it after a crash too.
pub struct StoreOwner {
    file: File,
}

impl StoreOwner {
    /// Claim the data folder before opening its store, recovering sessions or ending lost Gateway sessions.
    pub fn take(data: &Path) -> Result<Self, String> {
        let run = data.join("run");
        std::fs::create_dir_all(&run).map_err(|error| format!("Could not open the Code sessions folder: {error}"))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(run.join("workbench.lock"))
            .map_err(|error| format!("Could not open the Code sessions lock: {error}"))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(TryLockError::WouldBlock) => Err(
                "Another Nexus app or background service owns these Code sessions. Close it before opening Code in this build."
                    .into(),
            ),
            Err(TryLockError::Error(error)) => Err(format!("Could not lock the Code sessions: {error}")),
        }
    }

    /// Claim ownership before startup can stop a service or recover the store.
    pub fn start<T>(data: &Path, setup: impl FnOnce() -> T) -> Result<(Self, T), String> {
        let owner = Self::take(data)?;
        let result = setup();
        Ok((owner, result))
    }
}

impl Drop for StoreOwner {
    fn drop(&mut self) {
        // A child can briefly inherit this open file description between fork and exec on Unix.
        // Closing our descriptor alone then leaves the lock held until that child executes or exits.
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::StoreOwner;

    #[test]
    fn one_owner_per_folder_until_the_owner_exits() {
        let root = std::env::temp_dir().join(format!("nexus-store-owner-{}", uuid::Uuid::new_v4()));
        let owner = StoreOwner::take(&root).expect("first owner");
        assert!(StoreOwner::take(&root).is_err());
        let other = StoreOwner::take(&root.join("another-build")).expect("separate data folder");
        drop(owner);
        let replacement = StoreOwner::take(&root).expect("lock released without deleting its file");
        drop((replacement, other));
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn a_second_build_cannot_run_service_startup_before_claiming_ownership() {
        let root = std::env::temp_dir().join(format!("nexus-store-start-{}", uuid::Uuid::new_v4()));
        let owner = StoreOwner::take(&root).expect("running build");
        let mut stopped_service = false;
        let second = StoreOwner::start(&root, || stopped_service = true);
        assert!(second.is_err());
        assert!(!stopped_service, "a rejected build must not stop the running service");
        drop(owner);
        let (next, ()) = StoreOwner::start(&root, || {
            assert!(StoreOwner::take(&root).is_err(), "startup keeps its ownership claim");
        })
        .expect("start after the previous build exits");
        drop(next);
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn an_inherited_descriptor_does_not_keep_a_departed_owners_lock() {
        let root = std::env::temp_dir().join(format!("nexus-store-inherited-{}", uuid::Uuid::new_v4()));
        let owner = StoreOwner::take(&root).expect("first owner");
        let inherited = owner.file.try_clone().expect("inherited open file description");
        drop(owner);
        let next = StoreOwner::take(&root).expect("the previous owner explicitly released its lock");
        drop((next, inherited));
        std::fs::remove_dir_all(root).expect("remove fixture");
    }
}
