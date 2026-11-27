//! Which of the person's listed holders may receive a thread's content key.
//! Gateway serves the list, and the list is not trusted. A holder is trusted only through a chain of Ed25519
//! approvals, in Gateway's `nexus-device-approval:v1` format for a device and `v2` for a recovery key, back to this
//! device or a device this computer verified before. Revocations are remembered, so a holder Gateway lists again as
//! trusted stays refused.
//!
//! A recovery key (a passkey or a recovery code) has no signing key. It receives copies like a device, and
//! it never approves anything, so no chain passes through it.
//!
//! The first refresh trusts only this device and its confirmed approver. Each shell
//! that pairs a runner shows the approver's code and records the person's yes ([`Memory::anchors`]). Devices above the
//! approver come from its handover: the holders it trusts and the revoked ids it remembers, signed by it and
//! sealed to this device. It counts only when the confirmed approver signed it. Without one, only approvals down from
//! these two count.
//!
//! When Gateway's list says `trust: "gateway"`, Gateway vouches for the person's devices instead ([`verify_gateway`]):
//! no approval chain, no approver to confirm and no handover.

use std::collections::{HashMap, HashSet};

use nexus_client::e2e::{self, DeviceKey, OpenedHandover};
use serde::{Deserialize, Serialize};

pub(crate) const NOT_TRUSTED: &str = "session sync: this device is not a trusted device";
pub(crate) const CHAIN_BROKEN: &str = "session sync: this device's approval chain does not verify";
/// A teammate's root is not the one this computer pinned, or Gateway lists no single root to pin.
pub(crate) const ROOT_CHANGED: &str = "session sync: a teammate's root device changed; a member must confirm it";
/// This device has an approver, and the person has not confirmed it on this computer. Nothing is locked.
pub(crate) const APPROVER_UNCONFIRMED: &str = "session sync: confirm this device's approver by its code before it syncs (nexus devices pair, or Settings › Devices)";

/// One entry of `GET /v1/devices`, as Gateway lists it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Listed {
    pub id: String,
    /// `device`, `passkey` or `recovery_code`. Absent from an older Gateway: a device.
    #[serde(default = "device_kind")]
    pub kind: String,
    pub encryption_key: String,
    /// Null for a recovery key.
    #[serde(default)]
    pub signing_key: Option<String>,
    pub state: String,
    #[serde(default)]
    pub approved_by: Option<String>,
    #[serde(default)]
    pub approval_signature: Option<String>,
    /// The approver's trusted list, sealed to this device. Absent from a Gateway before it.
    #[serde(default)]
    pub handover: Option<String>,
}

fn device_kind() -> String {
    "device".to_string()
}

/// A holder's public keys. A recovery key has no signing key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Keys {
    pub encryption: [u8; 32],
    pub signing: Option<[u8; 32]>,
}

/// A device that may receive a copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Holder {
    pub id: String,
    pub keys: Keys,
}

/// What this computer remembers between refreshes: every revoked id it has seen, every device it has verified, and the
/// approver the person confirmed for each of its own device ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Memory {
    pub revoked: HashSet<String>,
    pub verified: HashMap<String, Keys>,
    /// This device's id to the approver the person confirmed by its code. Only a confirmation writes it, never a refresh.
    pub anchors: HashMap<String, Holder>,
}

/// The trusted devices: this one first.
#[derive(Debug, Clone)]
pub(crate) struct Trusted {
    pub me: Holder,
    pub holders: Vec<Holder>,
}

pub(crate) fn key32(text: &str) -> Option<[u8; 32]> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(text).ok()?;
    bytes.try_into().ok()
}

/// The holder's keys, when they fit its kind: a device has a signing key, and a recovery key has none.
fn keys_of(device: &Listed) -> Option<Keys> {
    let encryption = key32(&device.encryption_key)?;
    let signing = match (e2e::HolderKind::parse(&device.kind)?, device.signing_key.as_deref()) {
        (e2e::HolderKind::Device, Some(text)) => Some(key32(text)?),
        (e2e::HolderKind::Device, None) => return None,
        (_, None) => None,
        (_, Some(_)) => return None,
    };
    Some(Keys { encryption, signing })
}

/// Whether `approver`, a device, signed `device`'s id and keys, as Gateway's approval message spells them for its kind.
fn approved(device: &Listed, keys: &Keys, approver: &Keys) -> bool {
    use base64::Engine as _;
    let (Some(approver_key), Some(signature)) = (
        approver.signing,
        device
            .approval_signature
            .as_deref()
            .and_then(|text| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(text).ok()),
    ) else {
        return false;
    };
    let message = match (keys.signing, e2e::HolderKind::parse(&device.kind)) {
        (Some(signing), _) => e2e::approval_message(&device.id, &keys.encryption, &signing),
        (None, Some(kind)) => e2e::holder_approval_message(&device.id, kind, &keys.encryption),
        (None, None) => return false,
    };
    message.is_ok_and(|message| e2e::verify(&approver_key, &message, &signature))
}

/// This device's entry, and its approver as Gateway lists it when the approval verifies with the approver's listed
/// keys: what the person confirms by its code. No approver for a root. [`NOT_TRUSTED`] when no single trusted
/// entry has `me`'s keys; [`CHAIN_BROKEN`] when the approver is missing, is not a device, or did not sign.
pub(crate) fn listed_approver<'a>(
    listed: &'a [Listed],
    me: &Keys,
) -> Result<(&'a Listed, Option<Holder>), &'static str> {
    let mine = listed
        .iter()
        .filter(|device| keys_of(device).as_ref() == Some(me))
        .collect::<Vec<_>>();
    let [mine] = mine.as_slice() else {
        return Err(NOT_TRUSTED);
    };
    if mine.state != "trusted" || me.signing.is_none() {
        return Err(NOT_TRUSTED);
    }
    let Some(approver_id) = mine.approved_by.as_deref() else {
        return Ok((mine, None));
    };
    let approver = listed
        .iter()
        .find(|device| device.id == approver_id)
        .ok_or(CHAIN_BROKEN)?;
    let keys = keys_of(approver)
        .filter(|keys| keys.signing.is_some())
        .ok_or(CHAIN_BROKEN)?;
    if !approved(mine, me, &keys) {
        return Err(CHAIN_BROKEN);
    }
    Ok((
        mine,
        Some(Holder {
            id: approver.id.clone(),
            keys,
        }),
    ))
}

/// A memory whose anchor is this device's listed approver, as if the person confirmed it: for tests that read as an
/// approved device.
#[cfg(test)]
pub(crate) fn confirmed(listed: &[Listed], me: &Keys) -> Memory {
    let mut memory = Memory::default();
    if let Ok((mine, Some(approver))) = listed_approver(listed, me) {
        memory.anchors.insert(mine.id.clone(), approver);
    }
    memory
}

/// The devices to lock for. `me` is this device's keys. Updates `memory` with the revocations and verified devices it
/// saw; the caller stores it before it locks a key. Refuses when this device is not trusted, or its own chain is forged.
#[cfg(test)]
pub(crate) fn verify(listed: &[Listed], me: &Keys, memory: &mut Memory) -> Result<Trusted, &'static str> {
    verify_with(listed, me, memory, None)
}

/// [`verify`], with the handover this device opened ([`open_handover`]). It counts on every refresh while the approver
/// the person confirmed signed it: its revoked ids join `memory`, and its holders are trusted while Gateway lists them
/// trusted with the same keys.
pub(crate) fn verify_with(
    listed: &[Listed],
    me: &Keys,
    memory: &mut Memory,
    handover: Option<&OpenedHandover>,
) -> Result<Trusted, &'static str> {
    let by_id: HashMap<&str, &Listed> = listed.iter().map(|device| (device.id.as_str(), device)).collect();
    for device in listed.iter().filter(|device| device.state == "revoked") {
        memory.revoked.insert(device.id.clone());
    }
    let mine = listed
        .iter()
        .filter(|device| keys_of(device).as_ref() == Some(me))
        .collect::<Vec<_>>();
    let [mine] = mine.as_slice() else {
        return Err(NOT_TRUSTED);
    };
    let (handed, handed_revoked) = handover
        .and_then(|handover| accepted(handover, &mine.id, memory.anchors.get(&mine.id)))
        .unwrap_or_default();
    memory.revoked.extend(handed_revoked);
    let usable = |device: &Listed| device.state == "trusted" && !memory.revoked.contains(&device.id);
    let Some(my_signing) = me.signing else {
        return Err(NOT_TRUSTED);
    };
    if !usable(mine) || e2e::approval_message(&mine.id, &me.encryption, &my_signing).is_err() {
        return Err(NOT_TRUSTED);
    }
    let mut trusted: HashMap<String, Keys> = HashMap::from([(mine.id.clone(), *me)]);
    // Devices verified before stay trusted while Gateway lists them trusted with the same keys (an approval stands
    // after its approver is revoked).
    for (id, keys) in &memory.verified {
        if let Some(device) = by_id.get(id.as_str())
            && usable(device)
            && keys_of(device).as_ref() == Some(keys)
        {
            trusted.insert(id.clone(), *keys);
        }
    }
    // The first refresh trusts this device and the approver the person confirmed by its code on this computer:
    // same id and keys as listed, and its signature over this device. Nothing above it: Gateway can rewrite the
    // approver's own approval, so devices above it come only from the approver's signed handover, below.
    // Otherwise they are reached only through approvals down from these two. A root has no approver to confirm.
    if memory.verified.is_empty()
        && let Some(approver_id) = mine.approved_by.as_deref()
    {
        let approver = by_id.get(approver_id).ok_or(APPROVER_UNCONFIRMED)?;
        let approver_keys = keys_of(approver).ok_or(APPROVER_UNCONFIRMED)?;
        let anchored = memory
            .anchors
            .get(&mine.id)
            .is_some_and(|anchor| anchor.id == approver_id && anchor.keys == approver_keys);
        if !anchored {
            return Err(APPROVER_UNCONFIRMED);
        }
        if !approved(mine, me, &approver_keys) {
            return Err(CHAIN_BROKEN);
        }
        if usable(approver) {
            trusted.insert(approver.id.clone(), approver_keys);
        }
    }
    // The confirmed approver's handover: holders it trusts, at the keys Gateway lists now.
    for holder in &handed {
        if let Some(device) = by_id.get(holder.id.as_str())
            && usable(device)
            && keys_of(device).as_ref() == Some(&holder.keys)
        {
            trusted.insert(holder.id.clone(), holder.keys);
        }
    }
    grow(listed, &mut trusted, usable);
    for (id, keys) in &trusted {
        memory.verified.insert(id.clone(), *keys);
    }
    let me = Holder {
        id: mine.id.clone(),
        keys: *me,
    };
    let mut holders = vec![me.clone()];
    // Gateway's order, so copies and tests are stable.
    holders.extend(listed.iter().filter(|device| device.id != me.id).filter_map(|device| {
        trusted.get(&device.id).map(|keys| Holder {
            id: device.id.clone(),
            keys: *keys,
        })
    }));
    Ok(Trusted { me, holders })
}

/// The devices to lock for when Gateway vouches for the person's devices (`trust: "gateway"`). Every device Gateway lists
/// as trusted is a holder, except a recovery key and an id this computer saw revoked. Updates `memory`'s revocations
/// only: what Gateway vouched for is not remembered as verified, so a return to approvals trusts nothing from it.
/// Refuses when Gateway does not list this device as trusted.
pub(crate) fn verify_gateway(listed: &[Listed], me: &Keys, memory: &mut Memory) -> Result<Trusted, &'static str> {
    for device in listed.iter().filter(|device| device.state == "revoked") {
        memory.revoked.insert(device.id.clone());
    }
    // A device's keys: a recovery key has no signing key.
    let usable = |device: &Listed| {
        keys_of(device)
            .filter(|keys| keys.signing.is_some() && device.state == "trusted" && !memory.revoked.contains(&device.id))
    };
    let mine = listed
        .iter()
        .filter(|device| keys_of(device).as_ref() == Some(me))
        .collect::<Vec<_>>();
    let [mine] = mine.as_slice() else {
        return Err(NOT_TRUSTED);
    };
    let Some(my_signing) = me.signing else {
        return Err(NOT_TRUSTED);
    };
    if usable(mine).is_none() || e2e::approval_message(&mine.id, &me.encryption, &my_signing).is_err() {
        return Err(NOT_TRUSTED);
    }
    let me = Holder {
        id: mine.id.clone(),
        keys: *me,
    };
    let mut holders = vec![me.clone()];
    // Gateway's order, so copies and tests are stable.
    holders.extend(listed.iter().filter(|device| device.id != me.id).filter_map(|device| {
        usable(device).map(|keys| Holder {
            id: device.id.clone(),
            keys,
        })
    }));
    Ok(Trusted { me, holders })
}

/// A handover's payload: the approver's trusted holders and every revoked id it remembers. The kit writes
/// and reads the same JSON (`kit/src/handover.ts`). Version 2 excludes trust imported from legacy handovers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoverPayload {
    version: u8,
    holders: Vec<HandoverHolder>,
    revoked: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HandoverHolder {
    id: String,
    encryption_key: String,
    signing_key: Option<String>,
}

/// What this device hands a device it approves: the holders it trusts now and every revoked id it remembers, as the
/// payload the approver signs ([`DeviceKey::seal_handover`]).
pub(crate) fn handover_payload(trusted: &Trusted, revoked: &HashSet<String>) -> Vec<u8> {
    let mut revoked: Vec<String> = revoked.iter().cloned().collect();
    revoked.sort();
    let payload = HandoverPayload {
        version: 2,
        holders: trusted
            .holders
            .iter()
            .map(|holder| HandoverHolder {
                id: holder.id.clone(),
                encryption_key: e2e::b64u(&holder.keys.encryption),
                signing_key: holder.keys.signing.map(|key| e2e::b64u(&key)),
            })
            .collect(),
        revoked,
    };
    serde_json::to_vec(&payload).expect("a handover serializes")
}

/// The handover Gateway lists on this device, opened with its key. Not trusted yet: [`verify_with`] checks who signed it.
pub(crate) fn open_handover(listed: &[Listed], me: &Keys, device: &DeviceKey) -> Option<OpenedHandover> {
    use base64::Engine as _;
    let mine = listed.iter().find(|device| keys_of(device).as_ref() == Some(me))?;
    let sealed = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(mine.handover.as_deref()?)
        .ok()?;
    device
        .open_handover(&mine.id, mine.approved_by.as_deref()?, &sealed)
        .ok()
}

/// The holders and revoked ids of `handover`, when `anchor`, the approver the person confirmed for `mine`, signed it
/// for this device. A list Gateway made up, one another device signed, or one from an approver the person did not
/// confirm is ignored: this device then trusts as if it had none.
fn accepted(handover: &OpenedHandover, mine: &str, anchor: Option<&Holder>) -> Option<(Vec<Holder>, Vec<String>)> {
    let anchor = anchor?;
    if handover.device_id != mine
        || handover.approver_id != anchor.id
        || !anchor.keys.signing.is_some_and(|key| handover.signed_by(&key))
    {
        return None;
    }
    let payload: HandoverPayload = serde_json::from_slice(&handover.payload).ok()?;
    if payload.version != 2 {
        return None;
    }
    let holders = payload
        .holders
        .into_iter()
        .map(|holder| {
            let keys = Keys {
                encryption: key32(&holder.encryption_key)?,
                signing: match holder.signing_key {
                    Some(text) => Some(key32(&text)?),
                    None => None,
                },
            };
            Some(Holder { id: holder.id, keys })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((holders, payload.revoked))
}

/// Every other holder needs an approval by a trusted, unrevoked device. A device approved by no one is a replaced root,
/// one approved only by a revoked device is refused, and a recovery key approves nothing.
fn grow(listed: &[Listed], trusted: &mut HashMap<String, Keys>, usable: impl Fn(&Listed) -> bool) {
    loop {
        let mut grew = false;
        for device in listed {
            if trusted.contains_key(&device.id) || !usable(device) {
                continue;
            }
            let (Some(keys), Some(approver)) = (
                keys_of(device),
                device.approved_by.as_deref().and_then(|id| trusted.get(id)),
            ) else {
                continue;
            };
            if approved(device, &keys, approver) {
                trusted.insert(device.id.clone(), keys);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
}

/// A teammate's holders, rooted only in the exact device an administrator admitted locally.
pub(crate) fn member(
    listed: &[Listed],
    pinned: &Holder,
    memory: &mut Memory,
) -> Result<(Holder, Vec<Holder>), &'static str> {
    for device in listed.iter().filter(|device| device.state == "revoked") {
        memory.revoked.insert(device.id.clone());
    }
    let is_root = |device: &&Listed| {
        device.state == "trusted"
            && !memory.revoked.contains(&device.id)
            && device.approved_by.is_none()
            && keys_of(device).is_some_and(|keys| keys.signing.is_some())
    };
    let root = listed
        .iter()
        .filter(is_root)
        .find(|device| device.id == pinned.id && keys_of(device) == Some(pinned.keys))
        .ok_or(ROOT_CHANGED)?;
    let root = Holder {
        id: root.id.clone(),
        keys: keys_of(root).ok_or(ROOT_CHANGED)?,
    };
    let mut trusted = HashMap::from([(root.id.clone(), root.keys)]);
    grow(listed, &mut trusted, |device| {
        device.state == "trusted" && !memory.revoked.contains(&device.id)
    });
    let holders = listed
        .iter()
        .filter_map(|device| {
            trusted.get(&device.id).map(|keys| Holder {
                id: device.id.clone(),
                keys: *keys,
            })
        })
        .collect();
    Ok((root, holders))
}

#[cfg(test)]
mod gateway_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use e2e::DeviceKey;

    struct Device {
        id: String,
        key: DeviceKey,
    }

    fn device(id: &str) -> Device {
        Device {
            id: id.into(),
            key: DeviceKey::generate(),
        }
    }

    fn keys(device: &Device) -> Keys {
        Keys {
            encryption: device.key.encryption_key(),
            signing: Some(device.key.signing_key()),
        }
    }

    fn listed(device: &Device, state: &str, approver: Option<&Device>) -> Listed {
        let signature = approver.map(|approver| {
            let message = e2e::approval_message(&device.id, &device.key.encryption_key(), &device.key.signing_key())
                .expect("message");
            e2e::b64u(&approver.key.sign(&message))
        });
        Listed {
            id: device.id.clone(),
            kind: "device".into(),
            encryption_key: e2e::b64u(&device.key.encryption_key()),
            signing_key: Some(e2e::b64u(&device.key.signing_key())),
            state: state.into(),
            approved_by: approver.map(|approver| approver.id.clone()),
            approval_signature: signature,
            handover: None,
        }
    }

    fn ids(trusted: &Trusted) -> Vec<&str> {
        trusted.holders.iter().map(|holder| holder.id.as_str()).collect()
    }

    #[test]
    fn a_root_and_the_devices_it_approved_are_trusted() {
        let (a, b, c) = (device("dev_a"), device("dev_b"), device("dev_c"));
        let list = [
            listed(&a, "trusted", None),
            listed(&b, "trusted", Some(&a)),
            listed(&c, "trusted", Some(&b)),
        ];
        let mut memory = Memory::default();
        let trusted = verify(&list, &keys(&a), &mut memory).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_a", "dev_b", "dev_c"]);
        assert_eq!(memory.verified.len(), 3);
        // From the newest device, once the person confirmed its approver B: B, and nothing above B.
        let trusted = verify(&list, &keys(&c), &mut confirmed(&list, &keys(&c))).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_c", "dev_b"]);
    }

    #[test]
    fn an_honest_root_above_the_confirmed_approver_gets_no_keys_without_a_handover() {
        // The cost of the strict first refresh: A really approved B, and B approved C. C's person confirmed B. A is
        // above B, so with no handover from B C locks nothing for A. A device B approves later,
        // and a device C approves, are trusted: they are reached downward.
        let (a, b, c, d, e) = (
            device("dev_a"),
            device("dev_b"),
            device("dev_c"),
            device("dev_d"),
            device("dev_e"),
        );
        let list = [
            listed(&a, "trusted", None),
            listed(&b, "trusted", Some(&a)),
            listed(&c, "trusted", Some(&b)),
            listed(&d, "trusted", Some(&b)),
            listed(&e, "trusted", Some(&c)),
        ];
        let mut memory = anchored(&c, &b);
        let trusted = verify(&list, &keys(&c), &mut memory).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_c", "dev_b", "dev_d", "dev_e"]);
        assert!(
            !memory.verified.contains_key("dev_a"),
            "the root is not remembered either"
        );
    }

    /// `device`'s row with a handover of `holders` and `revoked`, signed by `signer` as approver `approver_id` and sealed
    /// to `device`: what Gateway lists after the approval.
    fn handed(
        row: Listed,
        device: &Device,
        approver_id: &str,
        signer: &Device,
        holders: &[&Device],
        revoked: &[&str],
    ) -> Listed {
        let trusted = Trusted {
            me: Holder {
                id: signer.id.clone(),
                keys: keys(signer),
            },
            holders: holders
                .iter()
                .map(|holder| Holder {
                    id: holder.id.clone(),
                    keys: keys(holder),
                })
                .collect(),
        };
        let revoked = revoked.iter().map(|id| id.to_string()).collect();
        let sealed = signer
            .key
            .seal_handover(
                &device.id,
                approver_id,
                &device.key.encryption_key(),
                &handover_payload(&trusted, &revoked),
            )
            .expect("sealed");
        Listed {
            handover: Some(e2e::b64u(&sealed)),
            ..row
        }
    }

    /// The trust `device` reads from `list` with `memory`, opening the handover listed on it.
    fn verify_opened(list: &[Listed], device: &Device, memory: &mut Memory) -> Result<Trusted, &'static str> {
        let opened = open_handover(list, &keys(device), &device.key);
        verify_with(list, &keys(device), memory, opened.as_ref())
    }

    #[test]
    fn the_confirmed_approver_s_handover_brings_the_devices_above_it_and_its_revocations() {
        // A (the root) approved B, the phone P and X. X was revoked, and B remembers it; Gateway lists X as trusted
        // again. B approves C and hands over its list. C's person confirmed B.
        let (a, b, c, p, x) = (
            device("dev_a"),
            device("dev_b"),
            device("dev_c"),
            device("dev_p"),
            device("dev_x"),
        );
        let rows = [
            listed(&a, "trusted", None),
            listed(&b, "trusted", Some(&a)),
            listed(&p, "trusted", Some(&a)),
            listed(&x, "trusted", Some(&a)),
        ];
        let c_row = handed(
            listed(&c, "trusted", Some(&b)),
            &c,
            "dev_b",
            &b,
            &[&b, &a, &p],
            &["dev_x"],
        );
        let list = [rows.to_vec(), vec![c_row]].concat();
        let mut memory = anchored(&c, &b);
        let trusted = verify_opened(&list, &c, &mut memory).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_c", "dev_a", "dev_b", "dev_p"]);
        assert!(memory.revoked.contains("dev_x"));
        // Without the list, the first refresh trusts B and nothing above it.
        let plain = [rows.to_vec(), vec![listed(&c, "trusted", Some(&b))]].concat();
        let trusted = verify_opened(&plain, &c, &mut anchored(&c, &b)).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_c", "dev_b"]);
        // A later refresh keeps the list's devices and revocation, whatever Gateway lists.
        let trusted = verify_opened(&plain, &c, &mut memory).expect("remembered");
        assert_eq!(ids(&trusted), ["dev_c", "dev_a", "dev_b", "dev_p"]);
    }

    #[test]
    fn a_handover_gateway_made_or_another_device_signed_is_ignored() {
        // Gateway knows C's public key, so it can seal a list: F's own, naming F and revoking A, or the root A's real
        // signature over a list for C. C's person confirmed B, so neither counts.
        let (a, b, c, f) = (device("dev_a"), device("dev_b"), device("dev_c"), device("dev_f"));
        for signer in [&f, &a] {
            let list = [
                listed(&a, "trusted", None),
                listed(&b, "trusted", Some(&a)),
                listed(&f, "trusted", None),
                handed(
                    listed(&c, "trusted", Some(&b)),
                    &c,
                    "dev_b",
                    signer,
                    &[&f],
                    &["dev_a", "dev_b"],
                ),
            ];
            let mut memory = anchored(&c, &b);
            let trusted = verify_opened(&list, &c, &mut memory).expect("trusted");
            assert_eq!(ids(&trusted), ["dev_c", "dev_b"], "signed by {}", signer.id);
            assert!(memory.revoked.is_empty());
            assert!(!memory.verified.contains_key("dev_f"));
        }
    }

    #[test]
    fn a_handover_from_an_approver_the_person_did_not_confirm_is_ignored() {
        // Gateway approves C from F, a device whose keys it holds, and F hands C a list naming F's own devices. C's
        // person confirmed B: C locks nothing, and F's list counts for nothing.
        let (a, b, c, f, g) = (
            device("dev_a"),
            device("dev_b"),
            device("dev_c"),
            device("dev_f"),
            device("dev_g"),
        );
        let list = [
            listed(&a, "trusted", None),
            listed(&b, "trusted", Some(&a)),
            listed(&f, "trusted", None),
            listed(&g, "trusted", Some(&f)),
            handed(listed(&c, "trusted", Some(&f)), &c, "dev_f", &f, &[&f, &g], &["dev_b"]),
        ];
        let mut memory = anchored(&c, &b);
        assert_eq!(
            verify_opened(&list, &c, &mut memory).expect_err("refused"),
            APPROVER_UNCONFIRMED
        );
        assert!(memory.verified.is_empty() && memory.revoked.is_empty());
        // Once C trusts B, Gateway rewrites C's approval to name F, with F's list: still nothing from F.
        let honest = [
            listed(&a, "trusted", None),
            listed(&b, "trusted", Some(&a)),
            listed(&c, "trusted", Some(&b)),
        ];
        verify_opened(&honest, &c, &mut memory).expect("trusted");
        let trusted = verify_opened(&list, &c, &mut memory).expect("remembered");
        assert_eq!(ids(&trusted), ["dev_c", "dev_b"]);
        assert!(!memory.revoked.contains("dev_b"));
    }

    #[test]
    fn handover_v2_refuses_a_confirmed_approver_s_legacy_handover() {
        let (f, b, c) = (device("dev_f"), device("dev_b"), device("dev_c"));
        let mut memory = Memory::default();
        memory.anchors.insert(
            c.id.clone(),
            Holder {
                id: b.id.clone(),
                keys: keys(&b),
            },
        );
        let payload = serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "holders": [{ "id": f.id, "encryptionKey": e2e::b64u(&f.key.encryption_key()), "signingKey": e2e::b64u(&f.key.signing_key()) }],
            "revoked": [],
        })).expect("legacy payload");
        let sealed = b
            .key
            .seal_handover(&c.id, &b.id, &c.key.encryption_key(), &payload)
            .expect("legacy handover");
        let list = [
            listed(&f, "trusted", None),
            listed(&b, "trusted", Some(&f)),
            Listed {
                handover: Some(e2e::b64u(&sealed)),
                ..listed(&c, "trusted", Some(&b))
            },
        ];
        let trusted = verify_opened(&list, &c, &mut memory).expect("confirmed direct approver");
        assert_eq!(ids(&trusted), ["dev_c", "dev_b"]);
        assert!(!memory.verified.contains_key("dev_f"));
    }

    #[test]
    fn a_handover_names_this_device_s_trusted_holders_and_every_revoked_id() {
        let (a, b) = (device("dev_a"), device("dev_b"));
        let trusted = Trusted {
            me: Holder {
                id: a.id.clone(),
                keys: keys(&a),
            },
            holders: vec![Holder {
                id: b.id.clone(),
                keys: keys(&b),
            }],
        };
        let payload = handover_payload(&trusted, &HashSet::from(["dev_y".to_string(), "dev_x".to_string()]));
        let text = String::from_utf8(payload).expect("utf-8");
        assert_eq!(
            text,
            format!(
                r#"{{"version":2,"holders":[{{"id":"dev_b","encryptionKey":"{}","signingKey":"{}"}}],"revoked":["dev_x","dev_y"]}}"#,
                e2e::b64u(&b.key.encryption_key()),
                e2e::b64u(&b.key.signing_key())
            )
        );
    }

    #[test]
    fn an_approver_s_rewritten_approval_brings_in_no_one() {
        // One step up: the person confirmed B. Gateway rewrites B's own approval to name F, a device whose keys it
        // holds, and F's signature over B's keys verifies. F gets nothing, nor does anything F approved.
        let (a, b, c, f, g) = (
            device("dev_a"),
            device("dev_b"),
            device("dev_c"),
            device("dev_f"),
            device("dev_g"),
        );
        let list = [
            listed(&a, "trusted", None),
            listed(&f, "trusted", None),
            listed(&b, "trusted", Some(&f)),
            listed(&c, "trusted", Some(&b)),
            listed(&g, "trusted", Some(&f)),
        ];
        let mut memory = anchored(&c, &b);
        let trusted = verify(&list, &keys(&c), &mut memory).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_c", "dev_b"]);
        assert!(!memory.verified.contains_key("dev_f") && !memory.verified.contains_key("dev_g"));
    }

    /// The approver the person confirmed for `device`.
    fn anchored(device: &Device, approver: &Device) -> Memory {
        let mut memory = Memory::default();
        memory.anchors.insert(
            device.id.clone(),
            Holder {
                id: approver.id.clone(),
                keys: keys(approver),
            },
        );
        memory
    }

    #[test]
    fn an_approved_device_locks_for_no_one_until_the_person_confirms_its_approver() {
        let (a, b) = (device("dev_a"), device("dev_b"));
        let list = [listed(&a, "trusted", None), listed(&b, "trusted", Some(&a))];
        let mut memory = Memory::default();
        assert_eq!(
            verify(&list, &keys(&b), &mut memory).expect_err("waits"),
            APPROVER_UNCONFIRMED
        );
        assert!(memory.verified.is_empty(), "nothing is remembered as verified");
        let mut memory = anchored(&b, &a);
        let trusted = verify(&list, &keys(&b), &mut memory).expect("confirmed");
        assert_eq!(ids(&trusted), ["dev_b", "dev_a"]);
        // A root has no approver to confirm.
        let trusted = verify(&list, &keys(&a), &mut Memory::default()).expect("root");
        assert_eq!(ids(&trusted), ["dev_a", "dev_b"]);
    }

    #[test]
    fn an_approver_gateway_made_is_refused_though_its_signature_verifies() {
        // The person confirmed A. Gateway lists B as approved by F, a device whose keys it holds; F signed B.
        let (a, b, f) = (device("dev_a"), device("dev_b"), device("dev_f"));
        let list = [
            listed(&a, "trusted", None),
            listed(&f, "trusted", None),
            listed(&b, "trusted", Some(&f)),
        ];
        let mut memory = anchored(&b, &a);
        assert_eq!(
            verify(&list, &keys(&b), &mut memory).expect_err("refused"),
            APPROVER_UNCONFIRMED
        );
        assert!(memory.verified.is_empty());
        // F listed under A's id: the id matches the anchor, the keys do not.
        let mut posing = listed(&f, "trusted", None);
        posing.id = a.id.clone();
        let mut signed = listed(&b, "trusted", Some(&f));
        signed.approved_by = Some(a.id.clone());
        assert_eq!(
            verify(&[posing, signed], &keys(&b), &mut anchored(&b, &a)).expect_err("refused"),
            APPROVER_UNCONFIRMED
        );
        // An anchor for another of this computer's device ids does not count.
        let list = [listed(&a, "trusted", None), listed(&b, "trusted", Some(&a))];
        let mut other = anchored(&device("dev_old"), &a);
        assert_eq!(
            verify(&list, &keys(&b), &mut other).expect_err("refused"),
            APPROVER_UNCONFIRMED
        );
    }

    #[test]
    fn a_device_verified_before_keeps_its_trust_without_an_anchor() {
        let (a, b, c) = (device("dev_a"), device("dev_b"), device("dev_c"));
        let list = [listed(&a, "trusted", None), listed(&b, "trusted", Some(&a))];
        let mut memory = Memory::default();
        memory.verified.insert(a.id.clone(), keys(&a));
        memory.verified.insert(b.id.clone(), keys(&b));
        let trusted = verify(&list, &keys(&b), &mut memory).expect("remembered");
        assert_eq!(ids(&trusted), ["dev_b", "dev_a"]);
        // It grows from there as before: a device A approves later is trusted.
        let list = [
            listed(&a, "trusted", None),
            listed(&b, "trusted", Some(&a)),
            listed(&c, "trusted", Some(&a)),
        ];
        let trusted = verify(&list, &keys(&b), &mut memory).expect("remembered");
        assert_eq!(ids(&trusted), ["dev_b", "dev_a", "dev_c"]);
    }

    #[test]
    fn the_listed_approver_is_what_the_person_confirms() {
        let (a, b, mallory) = (device("dev_a"), device("dev_b"), device("dev_m"));
        let list = [listed(&a, "trusted", None), listed(&b, "trusted", Some(&a))];
        let (mine, approver) = listed_approver(&list, &keys(&b)).expect("listed");
        assert_eq!(mine.id, "dev_b");
        assert_eq!(
            approver,
            Some(Holder {
                id: "dev_a".into(),
                keys: keys(&a)
            })
        );
        let (mine, approver) = listed_approver(&list, &keys(&a)).expect("root");
        assert_eq!((mine.id.as_str(), approver), ("dev_a", None));
        let mut forged = listed(&b, "trusted", Some(&mallory));
        forged.approved_by = Some(a.id.clone());
        assert_eq!(
            listed_approver(&[list[0].clone(), forged], &keys(&b)).expect_err("forged"),
            CHAIN_BROKEN
        );
        let missing = [listed(&b, "trusted", Some(&a))];
        assert_eq!(listed_approver(&missing, &keys(&b)).expect_err("missing"), CHAIN_BROKEN);
        let pending = [list[0].clone(), listed(&b, "pending", None)];
        assert_eq!(listed_approver(&pending, &keys(&b)).expect_err("pending"), NOT_TRUSTED);
    }

    /// A recovery code's holder, approved by `approver` over the v2 message.
    fn recovery(id: &str, state: &str, approver: Option<&Device>) -> (e2e::HolderKey, Listed) {
        let key = e2e::HolderKey::from_recovery_code(&e2e::RecoveryCode::generate(), "user_ada").expect("holder");
        let signature = approver.map(|approver| {
            let message = e2e::holder_approval_message(id, e2e::HolderKind::RecoveryCode, &key.encryption_key())
                .expect("message");
            e2e::b64u(&approver.key.sign(&message))
        });
        let listed = Listed {
            id: id.into(),
            kind: "recovery_code".into(),
            encryption_key: e2e::b64u(&key.encryption_key()),
            signing_key: None,
            state: state.into(),
            approved_by: approver.map(|approver| approver.id.clone()),
            approval_signature: signature,
            handover: None,
        };
        (key, listed)
    }

    #[test]
    fn a_recovery_key_approved_by_a_trusted_device_receives_copies() {
        let (a, b) = (device("dev_a"), device("dev_b"));
        let (code, code_listed) = recovery("dev_code", "trusted", Some(&a));
        let list = [listed(&a, "trusted", None), code_listed.clone()];
        let trusted = verify(&list, &keys(&a), &mut Memory::default()).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_a", "dev_code"]);
        assert_eq!(trusted.holders[1].keys.encryption, code.encryption_key());
        assert_eq!(trusted.holders[1].keys.signing, None);
        // A device's v1 approval over the same id does not stand for it, and a signing key on it is refused.
        let mut v1 = code_listed.clone();
        v1.approval_signature = Some(e2e::b64u(
            &a.key
                .sign(&e2e::approval_message("dev_code", &code.encryption_key(), &[0; 32]).expect("message")),
        ));
        let mut signs = code_listed.clone();
        signs.signing_key = Some(e2e::b64u(&b.key.signing_key()));
        for holder in [v1, signs] {
            let trusted = verify(
                &[listed(&a, "trusted", None), holder],
                &keys(&a),
                &mut Memory::default(),
            )
            .expect("trusted");
            assert_eq!(ids(&trusted), ["dev_a"]);
        }
    }

    #[test]
    fn a_recovery_key_approves_nothing() {
        let a = device("dev_a");
        let (code, mut code_listed) = recovery("dev_code", "trusted", Some(&a));
        let b = device("dev_b");
        // Gateway lists B as approved by the recovery key: whatever the signature, no chain passes through it.
        let mut by_code = listed(&b, "trusted", Some(&a));
        by_code.approved_by = Some("dev_code".into());
        let list = [listed(&a, "trusted", None), code_listed.clone(), by_code];
        let trusted = verify(&list, &keys(&a), &mut Memory::default()).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_a", "dev_code"]);
        // Nor is it ever this device.
        code_listed.state = "trusted".into();
        let me = Keys {
            encryption: code.encryption_key(),
            signing: None,
        };
        assert_eq!(
            verify(&[listed(&a, "trusted", None), code_listed], &me, &mut Memory::default()).expect_err("refused"),
            NOT_TRUSTED
        );
    }

    #[test]
    fn after_the_way_back_the_new_root_and_its_reapproved_recovery_key_are_trusted() {
        let (a, b) = (device("dev_a"), device("dev_b"));
        // A is lost and revoked; B recovered as the new root and approved the recovery key again.
        let (_, code_listed) = recovery("dev_code", "trusted", Some(&b));
        let list = [listed(&a, "revoked", None), listed(&b, "trusted", None), code_listed];
        let trusted = verify(&list, &keys(&b), &mut Memory::default()).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_b", "dev_code"]);
    }

    #[test]
    fn a_forged_approval_is_refused() {
        let (a, b, mallory) = (device("dev_a"), device("dev_b"), device("dev_m"));
        // Gateway lists a device as approved by A, but the signature is Mallory's own.
        let mut forged = listed(&b, "trusted", Some(&mallory));
        forged.approved_by = Some(a.id.clone());
        let list = [listed(&a, "trusted", None), forged];
        let trusted = verify(&list, &keys(&a), &mut Memory::default()).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_a"]);
        // A signature over other keys does not carry over to swapped keys.
        let mut swapped = listed(&b, "trusted", Some(&a));
        swapped.encryption_key = e2e::b64u(&mallory.key.encryption_key());
        let trusted = verify(
            &[listed(&a, "trusted", None), swapped],
            &keys(&a),
            &mut Memory::default(),
        )
        .expect("trusted");
        assert_eq!(ids(&trusted), ["dev_a"]);
    }

    #[test]
    fn unapproved_and_pending_devices_are_refused() {
        let (a, b, root) = (device("dev_a"), device("dev_b"), device("dev_r"));
        let mut memory = Memory::default();
        verify(&[listed(&a, "trusted", None)], &keys(&a), &mut memory).expect("trusted");
        // A second root (the replaced trust root) and a pending device get nothing.
        let list = [
            listed(&a, "trusted", None),
            listed(&root, "trusted", None),
            listed(&b, "pending", None),
        ];
        let trusted = verify(&list, &keys(&a), &mut memory).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_a"]);
        let mut unsigned = listed(&b, "trusted", Some(&a));
        unsigned.approval_signature = None;
        let trusted = verify(&[listed(&a, "trusted", None), unsigned], &keys(&a), &mut memory).expect("trusted");
        assert_eq!(ids(&trusted), ["dev_a"]);
    }

    #[test]
    fn a_revocation_is_remembered_after_gateway_lists_the_device_again() {
        let (a, b, c) = (device("dev_a"), device("dev_b"), device("dev_c"));
        let mut memory = Memory::default();
        let list = [listed(&a, "trusted", None), listed(&b, "trusted", Some(&a))];
        assert_eq!(
            ids(&verify(&list, &keys(&a), &mut memory).expect("trusted")),
            ["dev_a", "dev_b"]
        );
        let list = [listed(&a, "trusted", None), listed(&b, "revoked", Some(&a))];
        assert_eq!(ids(&verify(&list, &keys(&a), &mut memory).expect("trusted")), ["dev_a"]);
        assert!(memory.revoked.contains("dev_b"));
        // Gateway lists B as trusted again, and lists C approved by B: both stay refused.
        let list = [
            listed(&a, "trusted", None),
            listed(&b, "trusted", Some(&a)),
            listed(&c, "trusted", Some(&b)),
        ];
        assert_eq!(ids(&verify(&list, &keys(&a), &mut memory).expect("trusted")), ["dev_a"]);
    }

    #[test]
    fn an_approval_stands_after_its_approver_is_revoked() {
        let (a, b, c) = (device("dev_a"), device("dev_b"), device("dev_c"));
        let mut memory = Memory::default();
        let list = [
            listed(&a, "trusted", None),
            listed(&b, "trusted", Some(&a)),
            listed(&c, "trusted", Some(&b)),
        ];
        verify(&list, &keys(&a), &mut memory).expect("trusted");
        let list = [
            listed(&a, "trusted", None),
            listed(&b, "revoked", Some(&a)),
            listed(&c, "trusted", Some(&b)),
        ];
        assert_eq!(
            ids(&verify(&list, &keys(&a), &mut memory).expect("trusted")),
            ["dev_a", "dev_c"]
        );
    }

    #[test]
    fn a_teammates_root_requires_exact_local_admission_and_a_changed_root_waits() {
        let (root, phone, other, mallory) = (device("dev_r"), device("dev_p"), device("dev_o"), device("dev_m"));
        let list = [listed(&root, "trusted", None), listed(&phone, "trusted", Some(&root))];
        // First use: Gateway's single root is pinned, and the chain from it verifies.
        let admitted = Holder {
            id: root.id.clone(),
            keys: keys(&root),
        };
        let (pin, holders) = member(&list, &admitted, &mut Memory::default()).expect("admitted");
        assert_eq!(pin.id, "dev_r");
        assert_eq!(
            holders.iter().map(|holder| holder.id.as_str()).collect::<Vec<_>>(),
            ["dev_r", "dev_p"]
        );
        // A forged approval gets nothing.
        let mut forged = listed(&mallory, "trusted", Some(&mallory));
        forged.approved_by = Some(root.id.clone());
        let (_, holders) = member(&[list[0].clone(), forged], &pin, &mut Memory::default()).expect("pinned");
        assert_eq!(holders.len(), 1);
        // A new root (the way back) is not the pinned one: it waits for a member to confirm it.
        let replaced = [listed(&other, "trusted", None), listed(&phone, "trusted", Some(&other))];
        assert_eq!(
            member(&replaced, &pin, &mut Memory::default()).expect_err("waits"),
            ROOT_CHANGED
        );
        // The same id with other keys is a changed root too.
        let mut swapped = listed(&other, "trusted", None);
        swapped.id = root.id.clone();
        assert_eq!(
            member(&[swapped], &pin, &mut Memory::default()).expect_err("waits"),
            ROOT_CHANGED
        );
        // An extra unrelated root cannot change the locally admitted one.
        let two = [listed(&root, "trusted", None), listed(&other, "trusted", None)];
        assert_eq!(member(&two, &pin, &mut Memory::default()).expect("admitted").1.len(), 1);
    }

    #[test]
    fn a_teammates_revoked_device_stays_refused_when_gateway_restores_it() {
        let (root, phone) = (device("dev_r"), device("dev_p"));
        let list = [listed(&root, "trusted", None), listed(&phone, "trusted", Some(&root))];
        let mut memory = Memory::default();
        let pin = Holder {
            id: root.id.clone(),
            keys: keys(&root),
        };
        member(&list, &pin, &mut memory).expect("admitted");
        let revoked = [list[0].clone(), listed(&phone, "revoked", Some(&root))];
        member(&revoked, &pin, &mut memory).expect("revocation");
        let (_, holders) = member(&list, &pin, &mut memory).expect("replayed directory");
        assert_eq!(
            holders.iter().map(|holder| holder.id.as_str()).collect::<Vec<_>>(),
            ["dev_r"]
        );
    }

    #[test]
    fn this_device_must_be_trusted_and_its_own_chain_must_verify() {
        let (a, b, mallory) = (device("dev_a"), device("dev_b"), device("dev_m"));
        let pending = [listed(&a, "trusted", None), listed(&b, "pending", None)];
        assert_eq!(
            verify(&pending, &keys(&b), &mut Memory::default()).expect_err("refused"),
            NOT_TRUSTED
        );
        let revoked = [listed(&a, "trusted", None), listed(&b, "revoked", Some(&a))];
        assert_eq!(
            verify(&revoked, &keys(&b), &mut Memory::default()).expect_err("refused"),
            NOT_TRUSTED
        );
        assert_eq!(
            verify(&[listed(&a, "trusted", None)], &keys(&b), &mut Memory::default()).expect_err("refused"),
            NOT_TRUSTED
        );
        let mut forged = listed(&b, "trusted", Some(&mallory));
        forged.approved_by = Some(a.id.clone());
        let list = [listed(&a, "trusted", None), forged];
        // Even with A confirmed, Mallory's signature does not stand for A's.
        assert_eq!(
            verify(&list, &keys(&b), &mut anchored(&b, &a)).expect_err("refused"),
            CHAIN_BROKEN
        );
    }
}
