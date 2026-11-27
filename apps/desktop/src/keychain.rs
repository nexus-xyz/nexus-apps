//! The app's keychain: one per process, and every secret read and write goes through it. Each item is read
//! at most once per launch and held (`nexus_desktop_core::own_keychain`), so a caller never goes to the OS store on its
//! own.
//!
//! - **macOS:** items live under this app's own service, [`OWN_SERVICE`], which macOS lets it read without asking. An
//!   item the Nexus app saved (`xyz.nexus.desktop`) is read once and copied there. It shows a dialog only
//!   when the old item does not trust this app yet. The old item stays in place. `keychain-moves-<service>.json` in
//!   this build's data folder records which ones were tried.
//! - **Linux:** the owner-only file `vault.json` (`Vault::native`), which never asks for anything.
//! - **Windows:** no per-app rule, so the Nexus app's items are read in place, as before, and held.
//! - `NEXUS_DESKTOP_KEYCHAIN_SERVICE` names the old service instead (development, and the manual test with
//!   throwaway items). On macOS the own service is then that name with `-gpui` added.
//!   `NEXUS_DESKTOP_KEYCHAIN=memory`, `npm run measure` and tests keep secrets in memory only. `=file` and `=os`
//!   choose the store as `Vault::native` does; the OS store is always read through the own keychain.

use std::sync::{Arc, OnceLock};

use nexus_desktop_core::own_keychain::{OsStore, OwnKeychain, Unread};
use nexus_desktop_core::secrets::{Vault, keychain_services as services};

/// This app's own keychain service on macOS: its bundle id.
pub const OWN_SERVICE: &str = "xyz.nexus.desktop-gpui";

static VAULT: OnceLock<Vault> = OnceLock::new();

/// Where every secret is kept, for every caller.
pub fn vault() -> Vault {
    VAULT.get_or_init(open).clone()
}

fn open() -> Vault {
    if cfg!(test) || crate::measure::on() {
        return Vault::Memory(Default::default());
    }
    // `Vault::native` picks the store: the file on Linux, memory when asked. Only the OS store goes on from here.
    match Vault::native() {
        Vault::Os => {}
        other => return other,
    }
    let named = std::env::var("NEXUS_DESKTOP_KEYCHAIN_SERVICE").ok();
    let (service, old) = services(named.as_deref(), cfg!(target_os = "macos"));
    // One record per old service, so a test with throwaway items never marks the real ones as tried.
    let record = old
        .as_ref()
        .and_then(|old| Some(crate::paths::own_data_dir()?.join(format!("keychain-moves-{old}.json"))));
    Vault::Own(Arc::new(OwnKeychain::new(
        Box::new(OsStore),
        &service,
        old.as_deref(),
        record,
    )))
}

/// The plain sentence for a sign-in that could not be read.
fn sign_in_sentence(unread: &Unread) -> Option<String> {
    let sentence = match unread {
        Unread::NoKeychain => return None,
        Unread::Denied => {
            "The keychain did not let Nexus use the sign-in from the old Nexus app. \
                           Nexus will not ask for it again. Sign in to continue."
        }
        Unread::Failed(_) => "Nexus could not read your saved sign-in from the keychain. Sign in to continue.",
    };
    Some(sentence.into())
}

/// The plain sentence for a device key that could not be read.
fn device_key_sentence(unread: &Unread) -> String {
    match unread {
        Unread::NoKeychain => "Nexus could not find the system keychain for this computer's device key.".into(),
        Unread::Denied => "The keychain did not let Nexus read this computer's device key from the old Nexus app. \
                           Nexus made no new key, so your paired devices still match. \
                           Choose Try again to be asked once more."
            .into(),
        Unread::Failed(error) => format!("The keychain did not give this computer's device key: {error}"),
    }
}

/// `gateway`'s saved sign-in. An `Err` is the sentence that says why there is none.
pub fn token(gateway: &str) -> Result<Option<String>, String> {
    match vault() {
        Vault::Own(own) => match own.read(gateway) {
            Ok(secret) => Ok(secret),
            Err(unread) => sign_in_sentence(&unread).map_or(Ok(None), Err),
        },
        other => Ok(other.get(gateway)),
    }
}

/// Whether `gateway` has a sign-in this app already holds, without ever showing a dialog: Settings › Gateways lists
/// every known Gateway, and must not ask once per Gateway for the Nexus app's items.
pub fn has_sign_in(gateway: &str) -> bool {
    match vault() {
        Vault::Own(own) => own.peek(gateway).is_some(),
        other => other.get(gateway).is_some(),
    }
}

/// Why the saved sign-in for `gateway` could not be read at launch, for the sign-in screen. Reads only the held answer,
/// so the UI thread never waits on the keychain.
pub fn sign_in_notice(gateway: &str) -> Option<String> {
    let Vault::Own(own) = vault() else {
        return None;
    };
    own.held(gateway)?.err().and_then(|unread| sign_in_sentence(&unread))
}

/// Keep `token` as `gateway`'s sign-in. A failure is logged: the window stays signed in, the next start does not.
pub fn save_token(gateway: &str, token: &str) {
    if let Err(error) = vault().set(gateway, token) {
        eprintln!(
            "nexus-desktop-gpui: keychain: the sign-in was not saved, so the next start asks to sign in: {error}"
        );
    }
}

/// Remove `gateway`'s sign-in. A failure is logged.
pub fn forget_token(gateway: &str) {
    if let Err(error) = vault().delete(gateway) {
        eprintln!("nexus-desktop-gpui: keychain: the sign-in was not removed: {error}");
    }
}

/// This computer's device key for `gateway`, as stored text, or None when there is none yet. An `Err` is never "none":
/// a new key would orphan the device the person paired.
pub fn device_key(gateway: &str) -> Result<Option<String>, String> {
    let account = nexus_desktop_core::device_key::account(gateway);
    match vault() {
        Vault::Own(own) => own.read(&account).map_err(|unread| device_key_sentence(&unread)),
        other => other.try_get(&account),
    }
}

/// The person chose Try again: a device key the keychain refused is asked for once more.
pub fn retry_device_key(gateway: &str) {
    if let Vault::Own(own) = vault() {
        own.retry(&nexus_desktop_core::device_key::account(gateway));
    }
}

pub fn save_device_key(gateway: &str, text: &str) -> Result<(), String> {
    vault().set(&nexus_desktop_core::device_key::account(gateway), text)
}

pub fn forget_device_key(gateway: &str) -> Result<(), String> {
    vault().delete(&nexus_desktop_core::device_key::account(gateway))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_never_touch_the_persons_keychain() {
        assert!(matches!(vault(), Vault::Memory(_)));
    }

    #[test]
    fn macos_copies_from_the_old_service_and_the_others_read_it_in_place() {
        assert_eq!(
            services(None, true),
            (OWN_SERVICE.to_string(), Some("xyz.nexus.desktop".to_string()))
        );
        assert_eq!(
            services(Some("xyz.nexus.desktop.test"), true),
            (
                "xyz.nexus.desktop.test-gpui".to_string(),
                Some("xyz.nexus.desktop.test".to_string())
            )
        );
        assert_eq!(services(None, false), ("xyz.nexus.desktop".to_string(), None));
        assert_eq!(services(Some(""), false), ("xyz.nexus.desktop".to_string(), None));
        assert_eq!(services(Some("dev"), false), ("dev".to_string(), None));
    }

    #[test]
    fn a_missing_keychain_is_silent_at_sign_in() {
        assert_eq!(sign_in_sentence(&Unread::NoKeychain), None);
        assert!(sign_in_sentence(&Unread::Failed("locked".into())).is_some());
        assert!(device_key_sentence(&Unread::NoKeychain).contains("device key"));
    }

    #[test]
    fn the_sentences_say_what_happened_without_the_error_text() {
        let denied = sign_in_sentence(&Unread::Denied).expect("a denied prompt shows a notice");
        assert!(denied.contains("old Nexus app") && denied.contains("Sign in"));
        let device = device_key_sentence(&Unread::Denied);
        assert!(device.contains("Try again") && device.contains("no new key"));
    }
}
