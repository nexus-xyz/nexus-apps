//! This computer's device key for end-to-end encrypted session sync: the X25519
//! and Ed25519 pairs of `nexus_client::e2e::DeviceKey`, kept in the OS keychain next to the sign-in (src/secrets.rs),
//! one entry per Gateway. The private keys never reach the webview. It gets the public keys, and asks for one of two
//! signatures: an approval, which this module signs only after it checks the new device's fingerprint against the code
//! the person typed, and a revocation. It signs no other text.

use std::sync::Mutex;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nexus_client::e2e::{self, DEVICE_SECRET_LEN, DeviceKey};
use serde::Serialize;
use zeroize::Zeroizing;

use crate::secrets::Vault;

/// One load-or-create at a time, so two windows that open Settings › Devices together end with one key.
static CREATING: Mutex<()> = Mutex::new(());

/// The keychain account of `gateway`'s device key, beside its sign-in (whose account is the URL itself).
pub fn account(gateway: &str) -> String {
    format!("{gateway}#e2e-device")
}

/// What the UI may know: the public keys, the fingerprint and its code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePublic {
    pub encryption_key: String,
    pub signing_key: String,
    pub fingerprint: String,
    pub code: String,
}

pub fn public(device: &DeviceKey) -> DevicePublic {
    let fingerprint = device.fingerprint();
    DevicePublic {
        encryption_key: e2e::b64u(&device.encryption_key()),
        signing_key: e2e::b64u(&device.signing_key()),
        fingerprint: e2e::b64u(&fingerprint),
        code: e2e::short_code(&fingerprint),
    }
}

/// The stored key, if any. An entry that is not a key (edited by hand) reads as an error, never as no key: making a new
/// one would silently orphan the device the person paired.
fn load(vault: &Vault, gateway: &str) -> Result<Option<DeviceKey>, String> {
    let Some(text) = vault.get(&account(gateway)).map(Zeroizing::new) else {
        return Ok(None);
    };
    let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(text.as_bytes()).unwrap_or_default());
    let secret: &[u8; DEVICE_SECRET_LEN] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| "The keychain holds a device key Nexus cannot read.".to_string())?;
    Ok(Some(DeviceKey::from_secret_bytes(secret)))
}

/// The key's text, as the keychain holds it: its secret bytes as base64url.
pub fn text(device: &DeviceKey) -> Zeroizing<String> {
    Zeroizing::new(URL_SAFE_NO_PAD.encode(&device.secret_bytes()[..]))
}

/// The stored key, or a new one made and stored now.
pub fn load_or_create(vault: &Vault, gateway: &str) -> Result<DeviceKey, String> {
    let _one = CREATING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(device) = load(vault, gateway)? {
        return Ok(device);
    }
    let device = DeviceKey::generate();
    vault
        .set(&account(gateway), &text(&device))
        .map_err(|error| format!("The keychain did not save this computer's device key: {error}"))?;
    Ok(device)
}

/// The stored key, without making one: session sync (src/session_sync.rs) syncs only after the person set up Devices.
pub fn stored_key(vault: &Vault, gateway: &str) -> Result<Option<DeviceKey>, String> {
    load(vault, gateway)
}

/// The stored key, or an error when there is none yet.
pub fn stored(vault: &Vault, gateway: &str) -> Result<DeviceKey, String> {
    load(vault, gateway)?.ok_or_else(|| "This computer has no device key yet.".to_string())
}

fn key32(text: &str) -> Result<[u8; 32], String> {
    URL_SAFE_NO_PAD
        .decode(text)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| "A device's key is not 32 bytes of base64url.".to_string())
}

/// The approval of `device_id` with these keys, only when their fingerprint's code is `code`.
pub fn sign_approval(
    device: &DeviceKey,
    device_id: &str,
    encryption_key: &str,
    signing_key: &str,
    code: &str,
) -> Result<String, String> {
    let encryption_key = key32(encryption_key)?;
    let signing_key = key32(signing_key)?;
    if e2e::short_code(&e2e::fingerprint(&encryption_key, Some(&signing_key))) != code {
        return Err("That device's keys do not match the code. Do not approve it.".to_string());
    }
    let message = e2e::approval_message(device_id, &encryption_key, &signing_key).map_err(|error| error.to_string())?;
    Ok(e2e::b64u(&device.sign(&message)))
}

/// The approval of a recovery key: its id, kind and X25519 key, over the v2 message. The webview derived the
/// key from the passkey or code the person holds, so there is no fingerprint to check here.
pub fn sign_holder_approval(
    device: &DeviceKey,
    holder_id: &str,
    kind: &str,
    encryption_key: &str,
) -> Result<String, String> {
    let kind = match e2e::HolderKind::parse(kind) {
        Some(kind) if kind != e2e::HolderKind::Device => kind,
        _ => return Err(format!("{kind} is not a recovery key's kind")),
    };
    let message =
        e2e::holder_approval_message(holder_id, kind, &key32(encryption_key)?).map_err(|error| error.to_string())?;
    Ok(e2e::b64u(&device.sign(&message)))
}

/// The signature over a locked copy this device stores for itself, after a recovery (`nexus-thread-key:v1`).
pub fn sign_thread_key(
    device: &DeviceKey,
    thread_id: &str,
    epoch: u64,
    device_id: &str,
    locked: &str,
) -> Result<String, String> {
    let locked = URL_SAFE_NO_PAD
        .decode(locked)
        .map_err(|_| "The locked key is not base64url.".to_string())?;
    let message = e2e::thread_key_message(thread_id, epoch, device_id, &locked).map_err(|error| error.to_string())?;
    Ok(e2e::b64u(&device.sign(&message)))
}

pub fn sign_revocation(device: &DeviceKey, device_id: &str) -> Result<String, String> {
    let message = e2e::revocation_message(device_id).map_err(|error| error.to_string())?;
    Ok(e2e::b64u(&device.sign(&message)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATEWAY: &str = "http://127.0.0.1:8787";

    #[test]
    fn the_key_is_made_once_and_read_back() {
        let vault = Vault::Memory(Default::default());
        let made = public(&load_or_create(&vault, GATEWAY).expect("made"));
        let again = public(&load_or_create(&vault, GATEWAY).expect("read"));
        assert_eq!(made, again);
        assert_eq!(made.code.len(), 16);
        assert!(vault.get(GATEWAY).is_none(), "the sign-in's entry is untouched");
        let other = public(&load_or_create(&vault, "https://api.nexus.inc").expect("made"));
        assert_ne!(made, other, "one key per Gateway");
        vault.delete(&account(GATEWAY)).expect("forget");
        assert_ne!(public(&load_or_create(&vault, GATEWAY).expect("new")), made);
    }

    #[test]
    fn an_unreadable_entry_is_an_error_not_a_new_key() {
        let vault = Vault::Memory(Default::default());
        vault.set(&account(GATEWAY), "not a key").expect("set");
        assert!(load_or_create(&vault, GATEWAY).is_err());
        assert_eq!(vault.get(&account(GATEWAY)).as_deref(), Some("not a key"));
    }

    #[test]
    fn it_signs_an_approval_only_for_the_code_of_the_keys() {
        let approver = DeviceKey::generate();
        let new = DeviceKey::generate();
        let (encryption_key, signing_key) = (e2e::b64u(&new.encryption_key()), e2e::b64u(&new.signing_key()));
        let code = e2e::short_code(&new.fingerprint());
        let signature =
            sign_approval(&approver, "dev_abcdefghijkl", &encryption_key, &signing_key, &code).expect("signed");
        let message =
            e2e::approval_message("dev_abcdefghijkl", &new.encryption_key(), &new.signing_key()).expect("message");
        let bytes = URL_SAFE_NO_PAD.decode(signature).expect("base64url");
        assert!(e2e::verify(&approver.signing_key(), &message, &bytes));

        let other = DeviceKey::generate();
        let swapped = e2e::b64u(&other.encryption_key());
        assert!(sign_approval(&approver, "dev_abcdefghijkl", &swapped, &signing_key, &code).is_err());
        assert!(
            sign_approval(
                &approver,
                "dev_abcdefghijkl",
                &encryption_key,
                &signing_key,
                "AAAAAAAAAAAAAAAA"
            )
            .is_err()
        );
        assert!(sign_approval(&approver, "dev:bad", &encryption_key, &signing_key, &code).is_err());
    }

    #[test]
    fn it_signs_the_revocation_text() {
        let device = DeviceKey::generate();
        let signature = sign_revocation(&device, "dev_abcdefghijkl").expect("signed");
        let bytes = URL_SAFE_NO_PAD.decode(signature).expect("base64url");
        assert!(e2e::verify(
            &device.signing_key(),
            "nexus-device-revoke:v1:dev_abcdefghijkl",
            &bytes
        ));
    }

    #[test]
    fn it_signs_a_recovery_keys_approval_and_a_copy() {
        let device = DeviceKey::generate();
        let holder = e2e::HolderKey::from_recovery_code(&e2e::RecoveryCode::generate(), "user_ada").expect("holder");
        let key = e2e::b64u(&holder.encryption_key());
        let signature = sign_holder_approval(&device, "dev_abcdefghijkl", "recovery_code", &key).expect("signed");
        let message = e2e::holder_approval_message(
            "dev_abcdefghijkl",
            e2e::HolderKind::RecoveryCode,
            &holder.encryption_key(),
        )
        .expect("message");
        let bytes = URL_SAFE_NO_PAD.decode(signature).expect("base64url");
        assert!(e2e::verify(&device.signing_key(), &message, &bytes));
        assert!(sign_holder_approval(&device, "dev_abcdefghijkl", "device", &key).is_err());

        let thread = "0192f3a4-5b6c-7d8e-9fa0-b1c2d3e4f506";
        let signature = sign_thread_key(&device, thread, 0, "dev_abcdefghijkl", "AAAA").expect("signed");
        let message = e2e::thread_key_message(thread, 0, "dev_abcdefghijkl", &[0, 0, 0]).expect("message");
        let bytes = URL_SAFE_NO_PAD.decode(signature).expect("base64url");
        assert!(e2e::verify(&device.signing_key(), &message, &bytes));
    }
}
