//! Gateway's device trust (`trust: "gateway"`): Gateway vouches for the person's devices, so no approval is checked.
use super::*;
use crate::session_sync::DeviceTrust;
use e2e::DeviceKey;
use serde_json::json;

fn keys(key: &DeviceKey) -> Keys {
    Keys {
        encryption: key.encryption_key(),
        signing: Some(key.signing_key()),
    }
}

/// A device as Gateway lists it under its own trust: no approver and no signature.
fn row(id: &str, key: &DeviceKey, state: &str) -> Listed {
    Listed {
        id: id.into(),
        kind: "device".into(),
        encryption_key: e2e::b64u(&key.encryption_key()),
        signing_key: Some(e2e::b64u(&key.signing_key())),
        state: state.into(),
        approved_by: None,
        approval_signature: None,
        handover: None,
    }
}

/// A device that `approver` approved, as Gateway lists it under approvals.
fn approved(id: &str, key: &DeviceKey, approver: (&str, &DeviceKey)) -> Listed {
    let message = e2e::approval_message(id, &key.encryption_key(), &key.signing_key()).expect("message");
    Listed {
        approved_by: Some(approver.0.into()),
        approval_signature: Some(e2e::b64u(&approver.1.sign(&message))),
        ..row(id, key, "trusted")
    }
}

fn recovery_code(id: &str) -> Listed {
    let key = e2e::HolderKey::from_recovery_code(&e2e::RecoveryCode::generate(), "user_ada").expect("holder");
    Listed {
        id: id.into(),
        kind: "recovery_code".into(),
        encryption_key: e2e::b64u(&key.encryption_key()),
        signing_key: None,
        state: "trusted".into(),
        approved_by: None,
        approval_signature: None,
        handover: None,
    }
}

fn ids(trusted: &Trusted) -> Vec<&str> {
    trusted.holders.iter().map(|holder| holder.id.as_str()).collect()
}

#[test]
fn gateway_trust_holds_every_trusted_device_but_recovery_keys_and_revoked_ids() {
    let (a, b, c, d, x) = (
        DeviceKey::generate(),
        DeviceKey::generate(),
        DeviceKey::generate(),
        DeviceKey::generate(),
        DeviceKey::generate(),
    );
    let list = [
        row("dev_a", &a, "trusted"),
        row("dev_b", &b, "trusted"),
        row("dev_c", &c, "pending"),
        row("dev_d", &d, "revoked"),
        recovery_code("dev_code"),
        // Revoked before on this computer; Gateway lists it as trusted again.
        row("dev_x", &x, "trusted"),
    ];
    let mut memory = Memory::default();
    memory.revoked.insert("dev_x".into());
    let trusted = verify_gateway(&list, &keys(&a), &mut memory).expect("trusted");
    assert_eq!(ids(&trusted), ["dev_a", "dev_b"]);
    assert_eq!(trusted.me.id, "dev_a");
    assert!(memory.revoked.contains("dev_d") && memory.revoked.contains("dev_x"));
    assert!(
        memory.verified.is_empty(),
        "what Gateway vouched for is not remembered as verified"
    );
    // Under approvals the same list trusts only this device: B, the recovery key and X have no approval.
    let trusted = verify(&list, &keys(&a), &mut Memory::default()).expect("root");
    assert_eq!(ids(&trusted), ["dev_a"]);
}

#[test]
fn gateway_trust_needs_no_approver_confirmation() {
    let (a, b) = (DeviceKey::generate(), DeviceKey::generate());
    let list = [row("dev_a", &a, "trusted"), approved("dev_b", &b, ("dev_a", &a))];
    // Under approvals B locks nothing until the person confirms A by its code.
    assert_eq!(
        verify(&list, &keys(&b), &mut Memory::default()).expect_err("waits"),
        APPROVER_UNCONFIRMED
    );
    let mut memory = Memory::default();
    let trusted = verify_gateway(&list, &keys(&b), &mut memory).expect("no confirmation");
    assert_eq!(ids(&trusted), ["dev_b", "dev_a"]);
    assert!(memory.anchors.is_empty());
}

#[test]
fn gateway_trust_refuses_this_device_unless_gateway_lists_it_trusted() {
    let (a, b) = (DeviceKey::generate(), DeviceKey::generate());
    let pending = [row("dev_a", &a, "trusted"), row("dev_b", &b, "pending")];
    assert_eq!(
        verify_gateway(&pending, &keys(&b), &mut Memory::default()).expect_err("pending"),
        NOT_TRUSTED
    );
    let absent = [row("dev_a", &a, "trusted")];
    assert_eq!(
        verify_gateway(&absent, &keys(&b), &mut Memory::default()).expect_err("absent"),
        NOT_TRUSTED
    );
    let listed = [row("dev_a", &a, "trusted"), row("dev_b", &b, "trusted")];
    let mut revoked = Memory::default();
    revoked.revoked.insert("dev_b".into());
    assert_eq!(
        verify_gateway(&listed, &keys(&b), &mut revoked).expect_err("revoked here"),
        NOT_TRUSTED
    );
}

#[test]
fn only_gateway_in_the_list_reads_as_gateway_trust() {
    assert_eq!(DeviceTrust::of(&json!({ "devices": [] })), DeviceTrust::Approvals);
    assert_eq!(
        DeviceTrust::of(&json!({ "devices": [], "trust": "approvals" })),
        DeviceTrust::Approvals
    );
    assert_eq!(
        DeviceTrust::of(&json!({ "devices": [], "trust": "gateway" })),
        DeviceTrust::Gateway
    );
    for other in [json!("Gateway"), json!(true), json!(null)] {
        assert_eq!(
            DeviceTrust::of(&json!({ "devices": [], "trust": other })),
            DeviceTrust::Approvals
        );
    }
}
