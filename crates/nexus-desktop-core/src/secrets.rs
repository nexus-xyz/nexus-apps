//! The credential, in the OS's credential store only (`keyring`): Keychain on macOS, Credential Manager on Windows,
//! the Secret Service on Linux. One entry per Gateway (service `xyz.nexus.desktop`, account the Gateway URL), so
//! pointing the app at another Gateway never sends it this one's token. Never written to disk by the app, never sent
//! to the webview. `NEXUS_DESKTOP_KEYCHAIN_SERVICE` names another service (development: a throwaway entry that a
//! rebuilt, ad-hoc signed app can read without a Keychain prompt, deleted afterwards).

use keyring::{Entry, Error};

pub const SERVICE: &str = "xyz.nexus.desktop";

/// The own service and the old one to copy from: on macOS, an older Nexus install's service (or `named`) with
/// `-gpui` added, copying from it; elsewhere that service itself, with nothing to copy.
pub fn keychain_services(named: Option<&str>, macos: bool) -> (String, Option<String>) {
    let old = named.filter(|named| !named.is_empty()).unwrap_or(SERVICE);
    if macos {
        (format!("{old}-gpui"), Some(old.to_string()))
    } else {
        (old.to_string(), None)
    }
}

/// The service the entries are kept under: `NEXUS_DESKTOP_KEYCHAIN_SERVICE`, else [`SERVICE`].
fn service() -> String {
    std::env::var("NEXUS_DESKTOP_KEYCHAIN_SERVICE")
        .ok()
        .filter(|service| !service.is_empty())
        .unwrap_or_else(|| SERVICE.to_string())
}

/// Where tokens live: the OS store, or memory (tests, and `NEXUS_DESKTOP_KEYCHAIN=memory` for measurements that must
/// not touch the person's keychain).
#[derive(Debug, Clone)]
pub enum Vault {
    Os,
    /// Nexus GPUI's keychain: its own items, each read once per launch and held (src/own_keychain.rs).
    Own(std::sync::Arc<crate::own_keychain::OwnKeychain>),
    /// A JSON file in the config folder, readable by its owner only. Never asks the person for anything: the Linux
    /// Secret Service asks for a password when its keyring is locked or missing, and a fresh install often has neither.
    File(std::path::PathBuf),
    Memory(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>),
}

impl Vault {
    pub fn from_env() -> Vault {
        if std::env::var("NEXUS_DESKTOP_KEYCHAIN").as_deref() == Ok("memory") {
            Vault::Memory(Default::default())
        } else {
            Vault::Os
        }
    }

    /// The store a shell without an older Nexus install's items uses: the OS store on macOS and Windows, where the
    /// person is asked nothing for an app signed with a stable identity, and the file on Linux. `NEXUS_DESKTOP_KEYCHAIN` overrides:
    /// `memory`, `os` or `file`.
    pub fn native() -> Vault {
        let file = || {
            crate::settings::config_dir()
                .map(|dir| Vault::File(dir.join("vault.json")))
                .unwrap_or_else(|| Vault::Memory(Default::default()))
        };
        match std::env::var("NEXUS_DESKTOP_KEYCHAIN").as_deref() {
            Ok("memory") => Vault::Memory(Default::default()),
            Ok("os") => Vault::Os,
            Ok("file") => file(),
            _ if cfg!(target_os = "linux") => file(),
            _ => Vault::Os,
        }
    }

    pub fn get(&self, gateway: &str) -> Option<String> {
        self.try_get(gateway).ok().flatten()
    }

    /// Like [`Vault::get`], but a store that fails to answer is an `Err`, not "no entry". For callers that would
    /// make a new secret on `None`, and so orphan the one that is there.
    pub fn try_get(&self, gateway: &str) -> Result<Option<String>, String> {
        match self {
            Vault::File(path) => Ok(read_file(path)?.remove(gateway)),
            Vault::Os => match Entry::new(&service(), gateway).and_then(|entry| entry.get_password()) {
                Ok(token) => Ok(Some(token)),
                Err(Error::NoEntry) => Ok(None),
                Err(error) => Err(format!("The keychain did not answer: {error}")),
            },
            Vault::Own(own) => own.read(gateway).map_err(|unread| unread.to_string()),
            Vault::Memory(map) => Ok(map
                .lock()
                .map_err(|_| "memory vault poisoned".to_string())?
                .get(gateway)
                .cloned()),
        }
    }

    /// Like [`Vault::get`], but on macOS never from an older Nexus install's items, so it never shows a keychain
    /// dialog. For a secret this app can make again at no cost, such as the notification device token.
    pub fn get_own(&self, account: &str) -> Option<String> {
        match self {
            Vault::Own(own) => own.peek(account),
            other => other.get(account),
        }
    }

    pub fn set(&self, gateway: &str, token: &str) -> Result<(), String> {
        match self {
            Vault::File(path) => edit_file(path, |map| {
                map.insert(gateway.to_string(), token.to_string());
            }),
            Vault::Os => Entry::new(&service(), gateway)
                .and_then(|entry| entry.set_password(token))
                .map_err(|error| format!("The keychain did not save the sign-in: {error}")),
            Vault::Own(own) => own
                .write(gateway, token)
                .map_err(|error| format!("The keychain did not save the sign-in: {error}")),
            Vault::Memory(map) => {
                map.lock()
                    .map_err(|_| "memory vault poisoned".to_string())?
                    .insert(gateway.to_string(), token.to_string());
                Ok(())
            }
        }
    }

    /// Delete the entry; one that is not there is fine.
    pub fn delete(&self, gateway: &str) -> Result<(), String> {
        match self {
            Vault::File(path) => edit_file(path, |map| {
                map.remove(gateway);
            }),
            Vault::Os => match Entry::new(&service(), gateway).and_then(|entry| entry.delete_credential()) {
                Ok(()) | Err(Error::NoEntry) => Ok(()),
                Err(error) => Err(format!("The keychain did not delete the sign-in: {error}")),
            },
            Vault::Own(own) => own
                .delete(gateway)
                .map_err(|error| format!("The keychain did not delete the sign-in: {error}")),
            Vault::Memory(map) => {
                map.lock()
                    .map_err(|_| "memory vault poisoned".to_string())?
                    .remove(gateway);
                Ok(())
            }
        }
    }

    /// `gateway`'s token. When it has none and `retired` has one, the token moves to `gateway` (the same Gateway
    /// under a new host), so the person stays signed in. A failed write keeps the token where it is and returns it
    /// anyway: the next start tries the move again.
    pub fn get_or_adopt(&self, gateway: &str, retired: &str) -> Option<String> {
        if let Some(token) = self.get(gateway) {
            return Some(token);
        }
        let token = self.get(retired)?;
        if self.set(gateway, &token).is_ok() {
            // A leftover entry is harmless: `gateway` has the token now, and it wins on every later call.
            let _ = self.delete(retired);
        }
        Some(token)
    }
}

type FileMap = std::collections::BTreeMap<String, String>;

/// Serializes the read-modify-write of every file vault in this process.
static FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The file's entries: none when the file is not there, an `Err` for anything else (unreadable, not valid JSON), so a
/// hand edit or a permission problem never reads as an empty vault that the next write then replaces.
fn read_file(path: &std::path::Path) -> Result<FileMap, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("Nexus could not read its saved sign-ins ({}): {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileMap::new()),
        Err(error) => Err(format!(
            "Nexus could not read its saved sign-ins ({}): {error}",
            path.display()
        )),
    }
}

/// Apply `change` and write the file back: owner-only from the first byte, replaced whole so a crash never leaves half.
fn edit_file(path: &std::path::Path, change: impl FnOnce(&mut FileMap)) -> Result<(), String> {
    let _guard = FILE_LOCK.lock().map_err(|_| "file vault poisoned".to_string())?;
    let fail = |error: std::io::Error| format!("Nexus could not save the sign-in: {error}");
    let mut map = read_file(path)?;
    change(&mut map);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let text = serde_json::to_string(&map).map_err(|error| error.to_string())?;
    crate::atomic_file::write_atomic_private(path, text.as_bytes()).map_err(fail)
}

/// A fingerprint of a token for the webview (its query cache's scope): tells two credentials apart, reveals nothing
/// usable. Never the token.
pub fn marker(token: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "nexus-desktop".hash(&mut hasher);
    token.hash(&mut hasher);
    format!("desktop:{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_vault_and_markers() {
        let vault = Vault::Memory(Default::default());
        assert_eq!(vault.get("http://g"), None);
        vault.set("http://g", "nxc_1").expect("set");
        assert_eq!(vault.get("http://g").as_deref(), Some("nxc_1"));
        vault.delete("http://g").expect("delete");
        vault.delete("http://g").expect("delete again");
        assert_eq!(vault.get("http://g"), None);
        assert_eq!(marker("nxc_1"), marker("nxc_1"));
        assert_ne!(marker("nxc_1"), marker("nxc_2"));
        assert!(!marker("nxc_1").contains("nxc_1"));
    }

    #[test]
    fn a_sign_in_on_the_retired_host_moves_to_the_new_one() {
        let vault = Vault::Memory(Default::default());
        let retired = "https://api.nexus.xyz";
        vault.set(retired, "nxc_old").expect("set");
        assert_eq!(
            vault.get_or_adopt("https://api.nexus.inc", retired).as_deref(),
            Some("nxc_old")
        );
        assert_eq!(vault.get("https://api.nexus.inc").as_deref(), Some("nxc_old"));
        assert_eq!(vault.get(retired), None);
        vault.set(retired, "nxc_stale").expect("set");
        assert_eq!(
            vault.get_or_adopt("https://api.nexus.inc", retired).as_deref(),
            Some("nxc_old"),
            "a token already on the new host wins"
        );
        assert_eq!(
            vault.get(retired).as_deref(),
            Some("nxc_stale"),
            "the stale retired entry stays; the new host wins on every call"
        );
        assert_eq!(vault.get_or_adopt("https://other.test", "https://none.test"), None);
    }

    #[test]
    fn file_vault_round_trips_and_is_owner_only() {
        let dir = std::env::temp_dir().join(format!("nexus-vault-test-{}", std::process::id()));
        let path = dir.join("vault.json");
        let vault = Vault::File(path.clone());
        assert_eq!(vault.get("a"), None);
        vault.set("a", "one").expect("set");
        vault.set("b", "two").expect("set");
        assert_eq!(vault.get("a").as_deref(), Some("one"));
        vault.delete("a").expect("delete");
        vault.delete("a").expect("delete again");
        assert_eq!(vault.get("a"), None);
        assert_eq!(vault.get("b").as_deref(), Some("two"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(dir).ok();
    }

    /// The OS store itself (Keychain, Credential Manager, Secret Service): writes, reads and deletes one entry under a
    /// test account. Ignored by default (CI has no keychain); run with `cargo test -- --ignored keychain`.
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let vault = Vault::Os;
        let account = format!("nexus-desktop-test-{}", std::process::id());
        vault.set(&account, "nxc_test_token").expect("set");
        assert_eq!(vault.get(&account).as_deref(), Some("nxc_test_token"));
        vault.delete(&account).expect("delete");
        assert_eq!(vault.get(&account), None);
    }

    #[test]
    fn a_file_vault_that_cannot_be_read_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("nexus-vault-unreadable-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("vault.json");
        let vault = Vault::File(path.clone());
        assert_eq!(vault.try_get("g"), Ok(None));
        vault.set("g", "nxc_1").expect("set");
        assert_eq!(vault.try_get("g"), Ok(Some("nxc_1".to_string())));

        std::fs::write(&path, "{ not json").expect("write");
        assert!(vault.try_get("g").is_err());
        assert_eq!(vault.get("g"), None);
        assert!(vault.set("h", "nxc_2").is_err());
        assert!(vault.delete("g").is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{ not json");

        // A leftover temp file with loose permissions does not leak into the new vault.
        std::fs::write(&path, "{}").expect("reset");
        std::fs::write(path.with_extension("tmp"), "old").expect("tmp");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path.with_extension("tmp"), std::fs::Permissions::from_mode(0o644))
                .expect("chmod");
        }
        vault.set("g", "nxc_3").expect("set");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
