//! Provider keys use the app's OS store and are never held by the app's cached vault.

use nexus_providers::{KeyRef, KeySource, ProviderError, Secret};

pub struct OsProviderKeys;

fn service() -> String {
    let named = std::env::var("NEXUS_DESKTOP_KEYCHAIN_SERVICE").ok();
    super::secrets::keychain_services(named.as_deref(), cfg!(target_os = "macos")).0
}

fn store_unavailable(operation: &str) -> ProviderError {
    #[cfg(target_os = "linux")]
    let message = format!("Install or unlock a keyring to {operation} this provider key.");
    #[cfg(not(target_os = "linux"))]
    let message = format!("Unlock the OS key store to {operation} this provider key.");
    ProviderError::KeychainUnread(message)
}

fn check_ref(key: &KeyRef) -> Result<&str, ProviderError> {
    key.as_str()
        .strip_prefix("provider-key/")
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .map(|_| key.as_str())
        .ok_or_else(|| ProviderError::Refused("This provider has an invalid key reference.".into()))
}

impl KeySource for OsProviderKeys {
    fn read(&self, key: &KeyRef) -> Result<Option<Secret>, ProviderError> {
        os::read(&service(), check_ref(key)?).map_err(|_| store_unavailable("read"))
    }

    fn write(&self, key: &KeyRef, secret: Secret) -> Result<(), ProviderError> {
        os::write(&service(), check_ref(key)?, &secret).map_err(|_| store_unavailable("save"))
    }

    fn delete(&self, key: &KeyRef) -> Result<(), ProviderError> {
        os::delete(&service(), check_ref(key)?).map_err(|_| store_unavailable("delete"))
    }
}

#[cfg(not(target_os = "linux"))]
mod os {
    use super::*;

    pub fn read(service: &str, key: &str) -> keyring::Result<Option<Secret>> {
        match keyring::Entry::new(service, key)?.get_password() {
            Ok(secret) => Ok(Some(Secret::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn write(service: &str, key: &str, secret: &Secret) -> keyring::Result<()> {
        keyring::Entry::new(service, key)?.set_password(secret.expose())
    }

    pub fn delete(service: &str, key: &str) -> keyring::Result<()> {
        match keyring::Entry::new(service, key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

// The general keyring backend unlocks collections by invoking a prompt. Provider keys
// use the existing D-Bus client directly so a missing or locked collection fails.
#[cfg(target_os = "linux")]
#[path = "provider_keys_linux.rs"]
mod os;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_key_references_are_restricted_to_their_own_namespace() {
        let valid = format!("provider-key/{}", uuid::Uuid::new_v4());
        let key = KeyRef::new(valid.clone());
        assert_eq!(check_ref(&key).expect("provider key"), valid);
        for invalid in [
            "",
            "nexus-device-key",
            "provider-key/",
            "provider-key/not-a-uuid",
            "other/ffffffff-ffff-ffff-ffff-ffffffffffff",
        ] {
            assert!(check_ref(&KeyRef::new(invalid.into())).is_err(), "{invalid}");
        }
    }

    #[test]
    fn provider_keys_share_the_desktop_service_derivation() {
        use super::super::secrets::keychain_services;
        assert_eq!(keychain_services(None, true).0, "xyz.nexus.desktop-gpui");
        assert_eq!(keychain_services(None, false).0, "xyz.nexus.desktop");
        assert_eq!(keychain_services(Some("desktop.dev"), true).0, "desktop.dev-gpui");
        assert_eq!(keychain_services(Some("desktop.dev"), false).0, "desktop.dev");
        assert_eq!(keychain_services(Some(""), false).0, "xyz.nexus.desktop");
    }

    #[test]
    #[ignore = "requires an unlocked OS keychain and a test service override"]
    fn os_provider_key_round_trip() {
        let name = std::env::var("NEXUS_DESKTOP_KEYCHAIN_SERVICE").expect("set a test service override");
        assert!(!name.is_empty() && name != super::super::secrets::SERVICE);
        assert_ne!(service(), "xyz.nexus.desktop-gpui");
        let key = KeyRef::new(format!("provider-key/{}", uuid::Uuid::new_v4()));
        let keys = OsProviderKeys;
        assert!(keys.read(&key).expect("read missing entry").is_none());
        keys.write(&key, Secret::new("nexus-test-provider-key".into()))
            .expect("save");
        let value = keys.read(&key);
        keys.delete(&key).expect("delete");
        assert_eq!(
            value.expect("read entry").expect("saved entry").expose(),
            "nexus-test-provider-key"
        );
        assert!(keys.read(&key).expect("read deleted entry").is_none());
    }
}
