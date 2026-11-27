//! Session sync on this computer: the workbench's `GatewayBackend` for the person signed in now, with this
//! computer's device key from the keychain ([`crate::device_key`]). It is on by default. These gates
//! remain: the managed `AllowLocalCache`, a sign-in, the device key, device trust, the confirmed approver, the
//! organization's `sessionContent` and the per-computer Phone control switch. Nexus GPUI uses it from its
//! workbench host's `session_sync`, and runs it only while it owns the Code sessions store.
//!
//! The backend is built for one sign-in and never changes: a new sign-in, Gateway or organization drops it at once and
//! builds another in the background. Until then no thread binds to sync, and the engine makes no call. Without a device
//! key (Settings › Devices makes it) or the managed `AllowLocalCache`, nothing syncs. While the background service runs
//! the sessions, the apps hand it the device key instead ([`service_device_key`]), and it syncs as this computer.

use zeroize::Zeroizing;

use crate::secrets::Vault;

/// Whether sync may run at all on this computer: the managed `AllowLocalCache`. The other gates apply later.
pub fn wanted(allow_local_cache: bool) -> bool {
    allow_local_cache
}

/// This computer's device key as the background service takes it in the app's sign-in (`deviceKey`, nexus-host's
/// `local_sync`): only while the app's own sync would run, and once Settings › Devices made the key.
pub fn service_device_key(vault: &Vault, gateway: &str, allow_local_cache: bool) -> Option<Zeroizing<String>> {
    if !wanted(allow_local_cache) {
        return None;
    }
    let device = crate::device_key::stored_key(vault, gateway).ok()??;
    Some(crate::device_key::text(&device))
}

/// The backend's slot and its resolution: `nexus_workbench`'s, which the background service uses too.
pub use nexus_workbench::session_sync::slot::{KEYS_FILE, RETRY, Resolving, Slot, Task, connect, sign_in};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_managed_policy_stops_sync_here() {
        assert!(wanted(true));
        assert!(!wanted(false));
    }

    /// No environment switch: the service gets the key once Settings › Devices made it, unless the policy forbids sync.
    #[test]
    fn the_service_gets_the_device_key_unless_the_policy_forbids_sync() {
        const GATEWAY: &str = "http://127.0.0.1:8787";
        let vault = Vault::Memory(Default::default());
        assert!(service_device_key(&vault, GATEWAY, true).is_none(), "no key yet");
        let made = crate::device_key::load_or_create(&vault, GATEWAY).expect("made");
        let handed = service_device_key(&vault, GATEWAY, true).expect("handed over");
        assert_eq!(handed.as_str(), crate::device_key::text(&made).as_str());
        assert!(
            service_device_key(&vault, GATEWAY, false).is_none(),
            "the policy forbids sync"
        );
    }
}
