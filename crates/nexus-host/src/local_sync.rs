//! Session sync in the desktop app's own service ([`crate::local`]). The service runs on the app's computer, so it
//! syncs as that computer's device: the app hands it the device key from its keychain in each sign-in, only while the
//! app's own sync would run (its switch, and the managed `AllowLocalCache`). The key stays in memory. The content keys'
//! store is the app's (`session-sync-keys.sqlite3` in its data folder), as when the app runs the sessions.
//!
//! The backend is `nexus_workbench::session_sync::slot`'s, as the app resolves its own: one per sign-in and device, built
//! in the background, dropped at once for another. With no device key, nothing syncs.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use nexus_client::Client;
use nexus_client::e2e::{self, DeviceKey};
use nexus_workbench::session_sync::Backend;
use nexus_workbench::session_sync::slot::{self, Slot};
use zeroize::Zeroizing;

/// The device key the app handed over, and the backend for it.
#[derive(Default)]
pub struct LocalSync {
    /// The key's text, and its fingerprint for the slot's key.
    device: RwLock<Option<(Zeroizing<String>, String)>>,
    slot: Arc<Slot>,
    /// The content keys' store: the app's data folder's.
    keys: RwLock<Option<PathBuf>>,
}

impl LocalSync {
    /// Where the content keys' store is.
    pub fn keys_at(&self, path: PathBuf) {
        *self.keys.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(path);
    }

    /// The device key of the app's sign-in, as text; None when the app syncs nothing. A key that is not one counts as
    /// none.
    pub fn set_device(&self, text: Option<&str>) {
        let device = text.and_then(|text| {
            let key = crate::session_sync::key_from_text(text)?;
            Some((Zeroizing::new(text.trim().to_string()), e2e::b64u(&key.fingerprint())))
        });
        *self.device.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = device;
    }

    /// The backend for the sign-in on `client` (`token`, in `scope`), or None: no device key, not yet resolved, or
    /// refused.
    pub fn current(&self, client: Option<Client>, scope: &str, token: &str) -> Option<Arc<dyn Backend>> {
        let (text, fingerprint) = self
            .device
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()?;
        let client = client?;
        let keys = self
            .keys
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()?;
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let identity = nexus_workbench::session_choice::sign_in_identity(token);
        let (held, scope) = (self.slot.clone(), scope.to_string());
        self.slot.current(
            &slot::sign_in(&scope, &format!("{identity}\n{fingerprint}")),
            |task| {
                runtime.spawn(task);
            },
            move || {
                Box::pin(async move {
                    let device: DeviceKey = crate::session_sync::key_from_text(&text)?;
                    let keys = held.keys(|| Some(keys))?;
                    slot::connect(client, scope, device, keys, true).await
                })
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nothing_syncs_without_the_apps_device_key() {
        let sync = LocalSync::default();
        let dir = std::env::temp_dir().join(format!("nh-local-sync-{}", std::process::id()));
        sync.keys_at(dir.join(slot::KEYS_FILE));
        let client = || Some(Client::new(nexus_client::ClientOptions::new("http://127.0.0.1:9")));
        assert!(sync.current(client(), "gw|acme", "nxc_t").is_none());
        // A text that is not a key is no key.
        sync.set_device(Some("not a key"));
        assert!(sync.current(client(), "gw|acme", "nxc_t").is_none());
        assert!(!dir.exists(), "no store was opened");
    }
}
