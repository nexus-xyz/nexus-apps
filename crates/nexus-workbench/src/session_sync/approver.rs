//! The approver a person confirms for this device. Gateway lists who approved
//! this device, and Gateway is not trusted for it: a hostile Gateway can approve a new device from a device whose keys
//! it holds. So each shell that pairs a runner shows the listed approver's name and code, and the person compares them
//! with the approver's own screen. A yes is recorded in the key store. The runner's first refresh trusts that approver
//! only (`trust::verify`). A root device has no approver, so it has nothing to confirm.

use nexus_client::e2e::{self, DeviceKey};
use serde_json::Value;

use super::keystore::KeyStore;
use super::trust::{self, Holder, Keys, Listed};

/// This device's approver, as the person compares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approver {
    pub id: String,
    pub name: String,
    /// The 16-character code of its fingerprint.
    pub code: String,
}

/// Where this device's approval stands on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approval {
    /// A first device: trusted with no approver. It has nothing to confirm.
    Root,
    /// The person confirmed this approver on this computer.
    Confirmed(Approver),
    /// Sync waits until the person confirms this approver.
    Unconfirmed(Approver),
}

/// Why a confirmation was not recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmError {
    /// Gateway's device list is not in the expected form.
    InvalidList,
    /// No single trusted device in the list has this device's keys.
    NotTrusted,
    /// Gateway lists another approver for this device, or none.
    OtherApprover,
    /// The listed approval does not verify with the approver's listed keys.
    NotVerified,
    /// The key store failed.
    Store,
}

impl ConfirmError {
    pub fn message(self) -> &'static str {
        match self {
            ConfirmError::InvalidList => "Gateway's device list is not in the expected form.",
            ConfirmError::NotTrusted => "This device is not a trusted device yet.",
            ConfirmError::OtherApprover => "Gateway lists another approver for this device. Compare the code again.",
            ConfirmError::NotVerified => "This device's approval does not verify. Do not trust it; pair it again.",
            ConfirmError::Store => "The session sync key store failed.",
        }
    }
}

impl std::fmt::Display for ConfirmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ConfirmError {}

/// Whose memory in the key store: one person on one Gateway.
pub fn owner(gateway: &str, person: &str) -> String {
    format!("{}|{person}", gateway.trim_end_matches('/'))
}

/// This device's id in `devices` (`GET /v1/devices`'s `devices`), and its listed approver when the approval verifies.
fn listed(devices: &[Value], me: &DeviceKey) -> Result<(String, Option<(Holder, Approver)>), ConfirmError> {
    let listed: Vec<Listed> =
        serde_json::from_value(Value::Array(devices.to_vec())).map_err(|_| ConfirmError::InvalidList)?;
    let keys = Keys {
        encryption: me.encryption_key(),
        signing: Some(me.signing_key()),
    };
    let (mine, approver) = trust::listed_approver(&listed, &keys).map_err(|error| match error {
        trust::NOT_TRUSTED => ConfirmError::NotTrusted,
        _ => ConfirmError::NotVerified,
    })?;
    let approver = approver.map(|holder| {
        let name = devices
            .iter()
            .find(|device| device.get("id").and_then(Value::as_str) == Some(holder.id.as_str()))
            .and_then(|device| device.get("name").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        let code = e2e::short_code(&e2e::fingerprint(&holder.keys.encryption, holder.keys.signing.as_ref()));
        let shown = Approver {
            id: holder.id.clone(),
            name,
            code,
        };
        (holder, shown)
    });
    Ok((mine.id.clone(), approver))
}

/// This device's approver as Gateway lists it, when its approval verifies; none for a root. It reads no key store.
pub fn approver_of(devices: &[Value], me: &DeviceKey) -> Result<Option<Approver>, ConfirmError> {
    Ok(listed(devices, me)?.1.map(|(_, shown)| shown))
}

/// This device's approval as Gateway lists it, and whether the person confirmed that approver on this computer.
pub fn approval(keys: &KeyStore, owner: &str, devices: &[Value], me: &DeviceKey) -> Result<Approval, ConfirmError> {
    let (mine, approver) = listed(devices, me)?;
    let Some((holder, approver)) = approver else {
        return Ok(Approval::Root);
    };
    let memory = keys.memory(owner).map_err(|_| ConfirmError::Store)?;
    Ok(if memory.anchors.get(&mine) == Some(&holder) {
        Approval::Confirmed(approver)
    } else {
        Approval::Unconfirmed(approver)
    })
}

/// Records that the person confirmed `approver` (a device id) by its code. It is recorded only when Gateway lists that
/// device as this device's approver and its approval signature verifies with its listed keys.
pub fn confirm(
    keys: &KeyStore,
    owner: &str,
    devices: &[Value],
    me: &DeviceKey,
    approver: &str,
) -> Result<Approver, ConfirmError> {
    let (mine, listed) = listed(devices, me)?;
    let Some((holder, shown)) = listed.filter(|(holder, _)| holder.id == approver) else {
        return Err(ConfirmError::OtherApprover);
    };
    keys.confirm_approver(owner, &mine, &holder)
        .map_err(|_| ConfirmError::Store)?;
    Ok(shown)
}

/// What this device hands `device` (its id and listed X25519 key, base64url) when it approves it: the holders
/// this device trusts and the revoked ids it remembers, signed by `me` and sealed to `device`, base64url for
/// `approveDevice`'s `handover`. None when this device does not trust its own directory, or the list is too long to
/// seal: the approval then carries no handover. It reads the key store and writes nothing.
pub fn handover(
    keys: &KeyStore,
    owner: &str,
    devices: &[Value],
    me: &DeviceKey,
    device: &str,
    encryption_key: &str,
) -> Result<Option<String>, ConfirmError> {
    let listed: Vec<Listed> =
        serde_json::from_value(Value::Array(devices.to_vec())).map_err(|_| ConfirmError::InvalidList)?;
    let device_key = trust::key32(encryption_key).ok_or(ConfirmError::InvalidList)?;
    let mut memory = keys.memory(owner).map_err(|_| ConfirmError::Store)?;
    let me_keys = Keys {
        encryption: me.encryption_key(),
        signing: Some(me.signing_key()),
    };
    let opened = trust::open_handover(&listed, &me_keys, me);
    let Ok(trusted) = trust::verify_with(&listed, &me_keys, &mut memory, opened.as_ref()) else {
        return Ok(None);
    };
    let payload = trust::handover_payload(&trusted, &memory.revoked);
    Ok(me
        .seal_handover(device, &trusted.me.id, &device_key, &payload)
        .ok()
        .map(|sealed| e2e::b64u(&sealed)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(id: &str, key: &DeviceKey, state: &str, approver: Option<(&str, &DeviceKey)>) -> Value {
        let signature = approver.map(|(_, by)| {
            let message =
                e2e::approval_message(id, &key.encryption_key(), &key.signing_key()).expect("approval message");
            e2e::b64u(&by.sign(&message))
        });
        json!({
            "id": id,
            "name": format!("Device {id}"),
            "encryptionKey": e2e::b64u(&key.encryption_key()),
            "signingKey": e2e::b64u(&key.signing_key()),
            "state": state,
            "approvedBy": approver.map(|(by, _)| by),
            "approvalSignature": signature,
        })
    }

    #[test]
    fn a_confirmation_is_recorded_only_for_the_listed_approver_whose_signature_verifies() {
        let (a, b, f) = (DeviceKey::generate(), DeviceKey::generate(), DeviceKey::generate());
        let devices = [
            row("dev_a", &a, "trusted", None),
            row("dev_f", &f, "trusted", None),
            row("dev_b", &b, "trusted", Some(("dev_a", &a))),
        ];
        let keys = KeyStore::in_memory().expect("store");
        let owner = owner("https://gw.test/", "user_ada");
        assert_eq!(owner, "https://gw.test|user_ada");
        let code = e2e::short_code(&a.fingerprint());
        let shown = Approver {
            id: "dev_a".into(),
            name: "Device dev_a".into(),
            code,
        };
        assert_eq!(
            approval(&keys, &owner, &devices, &b),
            Ok(Approval::Unconfirmed(shown.clone()))
        );
        // Another device than the listed approver is refused, and nothing is recorded.
        assert_eq!(
            confirm(&keys, &owner, &devices, &b, "dev_f"),
            Err(ConfirmError::OtherApprover)
        );
        assert!(keys.memory(&owner).expect("memory").anchors.is_empty());
        assert_eq!(confirm(&keys, &owner, &devices, &b, "dev_a"), Ok(shown.clone()));
        assert_eq!(approval(&keys, &owner, &devices, &b), Ok(Approval::Confirmed(shown)));
        // Gateway later lists F as B's approver, signed by F: the confirmation does not carry over.
        let swapped = [
            devices[0].clone(),
            devices[1].clone(),
            row("dev_b", &b, "trusted", Some(("dev_f", &f))),
        ];
        assert!(matches!(
            approval(&keys, &owner, &swapped, &b),
            Ok(Approval::Unconfirmed(Approver { id, .. })) if id == "dev_f"
        ));
        // A root has nothing to confirm.
        assert_eq!(approval(&keys, &owner, &devices, &a), Ok(Approval::Root));
        assert_eq!(approver_of(&devices, &a), Ok(None));
        assert!(matches!(approver_of(&devices, &b), Ok(Some(Approver { id, .. })) if id == "dev_a"));
    }

    #[test]
    fn an_approver_hands_over_what_it_trusts_and_the_revocations_it_remembers() {
        let (a, b, x, new) = (
            DeviceKey::generate(),
            DeviceKey::generate(),
            DeviceKey::generate(),
            DeviceKey::generate(),
        );
        let devices = [
            row("dev_a", &a, "trusted", None),
            row("dev_b", &b, "trusted", Some(("dev_a", &a))),
            row("dev_x", &x, "revoked", Some(("dev_a", &a))),
        ];
        let keys = KeyStore::in_memory().expect("store");
        let new_key = e2e::b64u(&new.encryption_key());
        let sealed = handover(&keys, "o", &devices, &a, "dev_new", &new_key)
            .expect("read")
            .expect("a handover");
        let sealed = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, sealed).expect("b64");
        let opened = new.open_handover("dev_new", "dev_a", &sealed).expect("opens");
        assert!(opened.signed_by(&a.signing_key()));
        let payload: Value = serde_json::from_slice(&opened.payload).expect("json");
        let ids: Vec<&str> = payload["holders"]
            .as_array()
            .expect("holders")
            .iter()
            .filter_map(|holder| holder["id"].as_str())
            .collect();
        assert_eq!(ids, ["dev_a", "dev_b"]);
        assert_eq!(payload["revoked"], json!(["dev_x"]));
        // It reads the key store and writes nothing.
        assert!(keys.memory("o").expect("memory").revoked.is_empty());
        // A device that does not trust its own directory hands over nothing.
        let pending = [row("dev_a", &a, "trusted", None), row("dev_b", &b, "pending", None)];
        assert_eq!(handover(&keys, "o", &pending, &b, "dev_new", &new_key), Ok(None));
    }

    #[test]
    fn a_forged_or_pending_approval_is_never_recorded() {
        let (a, b, mallory) = (DeviceKey::generate(), DeviceKey::generate(), DeviceKey::generate());
        let keys = KeyStore::in_memory().expect("store");
        let mut forged = row("dev_b", &b, "trusted", Some(("dev_m", &mallory)));
        forged["approvedBy"] = json!("dev_a");
        let devices = [row("dev_a", &a, "trusted", None), forged];
        assert_eq!(
            confirm(&keys, "o", &devices, &b, "dev_a"),
            Err(ConfirmError::NotVerified)
        );
        let pending = [row("dev_a", &a, "trusted", None), row("dev_b", &b, "pending", None)];
        assert_eq!(
            confirm(&keys, "o", &pending, &b, "dev_a"),
            Err(ConfirmError::NotTrusted)
        );
        assert_eq!(
            confirm(&keys, "o", &[json!({ "id": "dev_x" })], &b, "dev_a"),
            Err(ConfirmError::InvalidList)
        );
        assert!(keys.memory("o").expect("memory").anchors.is_empty());
    }
}
