use super::*;
use getrandom::rand_core::{TryCryptoRng, TryRng};
use serde::Deserialize;
use std::convert::Infallible;

const THREAD: &str = "0192f3a4-5b6c-7d8e-9fa0-b1c2d3e4f506";
const OTHER_THREAD: &str = "0192f3a4-0000-7000-8000-00000000ffff";
const DEVICE: &str = "dev_7Kx3mQ9pL2vN8rT4wY6zA1";

#[derive(Deserialize)]
struct Vectors {
    labels: Labels,
    hpke: Suite,
    events: Vec<EventVector>,
    locks: Vec<LockVector>,
    devices: Vec<DeviceVector>,
    recovery_keys: Vec<RecoveryVector>,
    signer_secret: String,
    approvals: Vec<ApprovalVector>,
    thread_keys: Vec<ThreadKeyVector>,
    revocations: Vec<RevocationVector>,
    salts: Salts,
    passkey_prf_label: String,
    passkey_prf_input: String,
    recovery_codes: Vec<RecoveryCodeVector>,
    recovery_code_typos: Vec<String>,
    passkeys: Vec<PasskeyVector>,
    holder_approvals: Vec<HolderApprovalVector>,
    recover_challenges: Vec<ChallengeVector>,
    handovers: Vec<HandoverVector>,
}

#[derive(Deserialize)]
struct HandoverVector {
    sealed_by: String,
    device_index: usize,
    device_id: String,
    approver_id: String,
    payload: String,
    info: String,
    message: String,
    sealed: String,
}

#[derive(Deserialize)]
struct ChallengeVector {
    holder_secret_key: String,
    holder_key: String,
    person_id: String,
    holder_id: String,
    challenge: String,
    info: String,
    sealed: String,
}

#[derive(Deserialize)]
struct Salts {
    recovery_code: String,
    passkey: String,
}

#[derive(Deserialize)]
struct RecoveryCodeVector {
    bytes: String,
    text: String,
    person_id: String,
    info: String,
    secret: String,
    encryption_key: String,
    encryption_key_b64u: String,
    fingerprint: String,
}

#[derive(Deserialize)]
struct PasskeyVector {
    prf_output: String,
    person_id: String,
    credential_id: String,
    info: String,
    secret: String,
    encryption_key: String,
    encryption_key_b64u: String,
    fingerprint: String,
}

#[derive(Deserialize)]
struct HolderApprovalVector {
    holder_id: String,
    kind: String,
    encryption_key: String,
    message: String,
    signature: String,
}

#[derive(Deserialize)]
struct Labels {
    event_aad: String,
    lock_info: String,
    fingerprint: String,
    recover_challenge: String,
    handover: String,
}

#[derive(Deserialize)]
struct Suite {
    mode: u8,
    kem_id: u16,
    kdf_id: u16,
    aead_id: u16,
}

#[derive(Deserialize)]
struct EventVector {
    content_key: String,
    epoch: u64,
    thread_id: String,
    seq: u64,
    nonce: String,
    plaintext: String,
    aad: String,
    sealed: String,
}

#[derive(Deserialize)]
struct LockVector {
    holder_secret_key: String,
    holder_key: String,
    holder_id: String,
    thread_id: String,
    epoch: u64,
    ikm_e: String,
    content_key: String,
    info: String,
    locked: String,
}

#[derive(Deserialize)]
struct DeviceVector {
    secret: String,
    encryption_key: String,
    signing_key: String,
    encryption_key_b64u: String,
    signing_key_b64u: String,
    fingerprint: String,
    short_code: String,
}

#[derive(Deserialize)]
struct RecoveryVector {
    encryption_key: String,
    fingerprint: String,
    short_code: String,
}

#[derive(Deserialize)]
struct ApprovalVector {
    device_id: String,
    encryption_key: String,
    signing_key: String,
    message: String,
    signature: String,
}

#[derive(Deserialize)]
struct ThreadKeyVector {
    thread_id: String,
    epoch: u64,
    key_version: i32,
    device_id: String,
    locked: String,
    message: String,
    signature: String,
}

#[derive(Deserialize)]
struct RevocationVector {
    device_id: String,
    message: String,
    signature: String,
}

fn vectors() -> Vectors {
    serde_json::from_str(include_str!("../../tests/e2e-vectors.json")).expect("e2e-vectors.json parses")
}

fn bytes(text: &str) -> Vec<u8> {
    hex::decode(text).expect("hex")
}

fn array<const N: usize>(text: &str) -> [u8; N] {
    bytes(text).try_into().expect("length")
}

fn key(text: &str, epoch: u64) -> ContentKey {
    ContentKey::new(Zeroizing::new(array(text)), epoch)
}

/// The same key bytes at another epoch, as a hostile Gateway would serve them.
fn at_epoch(content: &ContentKey, epoch: u64) -> ContentKey {
    ContentKey::new(content.key.clone(), epoch)
}

/// A fixed X25519 private key for the X25519-only half of a vector: the Ed25519 seed is all zeros.
fn x25519_only(secret: &str) -> DeviceKey {
    let mut both = [0u8; DEVICE_SECRET_LEN];
    both[..KEY_LEN].copy_from_slice(&bytes(secret));
    DeviceKey::from_secret_bytes(&both)
}

/// Hands HPKE fixed bytes as its ephemeral key's input (`ikmE` in RFC 9180's test vectors), so a lock is reproducible.
struct FixedIkm<'a>(&'a [u8]);

impl TryRng for FixedIkm<'_> {
    type Error = Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        unreachable!("HPKE draws bytes")
    }
    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        unreachable!("HPKE draws bytes")
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        let (head, rest) = self.0.split_at(dst.len());
        dst.copy_from_slice(head);
        self.0 = rest;
        Ok(())
    }
}

impl TryCryptoRng for FixedIkm<'_> {}

fn flipped(data: &[u8], bit: usize) -> Vec<u8> {
    let mut out = data.to_vec();
    out[bit / 8] ^= 1 << (bit % 8);
    out
}

#[test]
fn the_vectors_name_this_suite_and_these_labels() {
    let v = vectors();
    assert_eq!(v.labels.event_aad, EVENT_AAD_LABEL);
    assert_eq!(v.labels.lock_info, LOCK_INFO_LABEL);
    assert_eq!(v.labels.fingerprint, FINGERPRINT_LABEL);
    assert_eq!(v.labels.recover_challenge, RECOVER_CHALLENGE_LABEL);
    assert_eq!(v.labels.handover, HANDOVER_LABEL);
    assert_eq!(
        (v.hpke.mode, v.hpke.kem_id, v.hpke.kdf_id, v.hpke.aead_id),
        (0, 0x0020, 0x0001, 0x0002)
    );
    assert_eq!(<Kem as hpke::Kem>::KEM_ID, v.hpke.kem_id);
}

#[test]
fn events_seal_and_open_byte_for_byte_as_the_vectors() {
    for v in vectors().events {
        let content = key(&v.content_key, v.epoch);
        let name = format!("{}@{}#{}", v.thread_id, v.epoch, v.seq);
        assert_eq!(
            hex::encode(event_aad(&v.thread_id, v.epoch, v.seq).expect("aad")),
            v.aad,
            "aad of {name}"
        );
        let sealed = content
            .seal_event_with_nonce(&array(&v.nonce), &v.thread_id, v.seq, &bytes(&v.plaintext))
            .expect("seal");
        assert_eq!(hex::encode(&sealed), v.sealed, "sealed {name}");
        assert_eq!(
            hex::encode(content.open_event(&v.thread_id, v.seq, &sealed).expect("open")),
            v.plaintext
        );
    }
}

#[test]
fn content_keys_lock_and_unlock_byte_for_byte_as_the_vectors() {
    for v in vectors().locks {
        let holder = x25519_only(&v.holder_secret_key);
        assert_eq!(hex::encode(holder.encryption_key()), v.holder_key);
        assert_eq!(
            hex::encode(lock_info(&v.thread_id, v.epoch, &v.holder_id).expect("info")),
            v.info
        );
        let ikm = bytes(&v.ikm_e);
        let locked = key(&v.content_key, v.epoch)
            .lock_with_rng(
                &v.thread_id,
                &v.holder_id,
                &holder.encryption_key(),
                &mut FixedIkm(&ikm),
            )
            .expect("lock");
        assert_eq!(hex::encode(&locked), v.locked, "locked for {}", v.holder_id);
        let opened = holder
            .unlock_content_key(&v.thread_id, v.epoch, &v.holder_id, &locked)
            .expect("unlock");
        assert_eq!(hex::encode(*opened.key), v.content_key);
        assert_eq!(opened.epoch(), v.epoch);
    }
}

#[test]
fn devices_keys_and_fingerprints_match_the_vectors() {
    let v = vectors();
    for d in v.devices {
        let device = DeviceKey::from_secret_bytes(&array(&d.secret));
        assert_eq!(hex::encode(*device.secret_bytes()), d.secret);
        assert_eq!(hex::encode(device.encryption_key()), d.encryption_key);
        assert_eq!(hex::encode(device.signing_key()), d.signing_key);
        assert_eq!(b64u(&device.encryption_key()), d.encryption_key_b64u);
        assert_eq!(b64u(&device.signing_key()), d.signing_key_b64u);
        assert_eq!(hex::encode(device.fingerprint()), d.fingerprint);
        assert_eq!(short_code(&device.fingerprint()), d.short_code);
    }
    for r in v.recovery_keys {
        let print = fingerprint(&array(&r.encryption_key), None);
        assert_eq!(hex::encode(print), r.fingerprint);
        assert_eq!(short_code(&print), r.short_code);
    }
}

#[test]
fn signatures_match_the_vectors_and_gateways_messages() {
    let v = vectors();
    let signer = DeviceKey::from_secret_bytes(&array(&v.signer_secret));
    let check = |message: String, expected: &str, signature: &str| {
        assert_eq!(message, expected);
        assert_eq!(hex::encode(signer.sign(&message)), signature, "{message}");
        assert!(verify(&signer.signing_key(), &message, &bytes(signature)));
    };
    for a in v.approvals {
        let message = approval_message(&a.device_id, &array(&a.encryption_key), &array(&a.signing_key));
        check(message.expect("message"), &a.message, &a.signature);
    }
    for t in v.thread_keys {
        assert_eq!(key_version(t.epoch), Ok(t.key_version));
        let message = thread_key_message(&t.thread_id, t.epoch, &t.device_id, &bytes(&t.locked));
        check(message.expect("message"), &t.message, &t.signature);
    }
    for r in v.revocations {
        check(
            revocation_message(&r.device_id).expect("message"),
            &r.message,
            &r.signature,
        );
    }
}

#[test]
fn recovery_codes_derive_their_holders_as_the_vectors() {
    let v = vectors();
    assert_eq!(v.salts.recovery_code, RECOVERY_CODE_SALT);
    for r in v.recovery_codes {
        let code = RecoveryCode::from_bytes(&array(&r.bytes));
        assert_eq!(*code.display(), r.text);
        let parsed = RecoveryCode::parse(&r.text).expect("the printed code parses");
        assert_eq!(hex::encode(parsed.bytes()), r.bytes);
        // As typed: lower case, no hyphens, spaces between groups.
        let typed = r.text.to_lowercase().replace('-', " ");
        assert_eq!(
            hex::encode(RecoveryCode::parse(&typed).expect("typed").bytes()),
            r.bytes
        );
        assert_eq!(hex::encode(length_prefixed(&[r.person_id.as_bytes()])), r.info);
        assert_eq!(
            hex::encode(*hkdf_sha256(code.bytes(), RECOVERY_CODE_SALT, &bytes(&r.info))),
            r.secret
        );
        let holder = HolderKey::from_recovery_code(&code, &r.person_id).expect("holder");
        assert_eq!(hex::encode(holder.encryption_key()), r.encryption_key);
        assert_eq!(b64u(&holder.encryption_key()), r.encryption_key_b64u);
        assert_eq!(hex::encode(holder.fingerprint()), r.fingerprint);
        // Another person's code gives another key.
        let other = HolderKey::from_recovery_code(&code, "user_someone_else").expect("holder");
        assert_ne!(other.encryption_key(), holder.encryption_key());
    }
    for typo in v.recovery_code_typos {
        assert_eq!(RecoveryCode::parse(&typo).err(), Some(E2eError::Code), "{typo}");
    }
}

#[test]
fn passkeys_derive_their_holders_as_the_vectors() {
    let v = vectors();
    assert_eq!(v.salts.passkey, PASSKEY_SALT);
    assert_eq!(v.passkey_prf_label, PASSKEY_PRF_LABEL);
    assert_eq!(hex::encode(passkey_prf_input()), v.passkey_prf_input);
    for p in v.passkeys {
        let credential = bytes(&p.credential_id);
        assert_eq!(
            hex::encode(length_prefixed(&[p.person_id.as_bytes(), &credential])),
            p.info
        );
        assert_eq!(
            hex::encode(*hkdf_sha256(&bytes(&p.prf_output), PASSKEY_SALT, &bytes(&p.info))),
            p.secret
        );
        let holder = HolderKey::from_passkey_prf(&array(&p.prf_output), &p.person_id, &credential).expect("holder");
        assert_eq!(hex::encode(holder.encryption_key()), p.encryption_key);
        assert_eq!(b64u(&holder.encryption_key()), p.encryption_key_b64u);
        assert_eq!(hex::encode(holder.fingerprint()), p.fingerprint);
    }
}

#[test]
fn recovery_challenges_open_as_the_vectors() {
    let v = vectors();
    assert_eq!(v.recover_challenges.len(), 2);
    for c in v.recover_challenges {
        let holder = HolderKey::from_secret_bytes(&array(&c.holder_secret_key));
        assert_eq!(hex::encode(holder.encryption_key()), c.holder_key);
        assert_eq!(
            hex::encode(recover_challenge_info(&c.person_id, &c.holder_id).expect("info")),
            c.info
        );
        let sealed = bytes(&c.sealed);
        let opened = holder
            .open_recovery_challenge(&c.person_id, &c.holder_id, &sealed)
            .expect("opens");
        assert_eq!(hex::encode(*opened), c.challenge);
        // Bound to its person and holder: another of either does not open it.
        assert_eq!(
            holder
                .open_recovery_challenge("user_other", &c.holder_id, &sealed)
                .err(),
            Some(E2eError::Open)
        );
        assert_eq!(
            holder.open_recovery_challenge(&c.person_id, "dev_other", &sealed).err(),
            Some(E2eError::Open)
        );
        assert_eq!(
            holder
                .open_recovery_challenge(&c.person_id, &c.holder_id, &flipped(&sealed, 300))
                .err(),
            Some(E2eError::Open)
        );
    }
}

#[test]
fn handovers_open_as_the_vectors_whichever_side_sealed_them() {
    let v = vectors();
    let signer = DeviceKey::from_secret_bytes(&array(&v.signer_secret));
    let sealers: Vec<&str> = v.handovers.iter().map(|h| h.sealed_by.as_str()).collect();
    assert_eq!(sealers, ["nexus-client", "kit"]);
    for h in &v.handovers {
        let device = DeviceKey::from_secret_bytes(&array(&v.devices[h.device_index].secret));
        assert_eq!(
            hex::encode(handover_info(&h.device_id, &h.approver_id).expect("info")),
            h.info
        );
        assert_eq!(
            handover_message(&h.device_id, &h.approver_id, h.payload.as_bytes()).expect("message"),
            h.message
        );
        let opened = device
            .open_handover(&h.device_id, &h.approver_id, &bytes(&h.sealed))
            .expect("opens");
        assert_eq!(opened.payload, h.payload.as_bytes());
        assert!(opened.signed_by(&signer.signing_key()), "{}", h.sealed_by);
    }
}

#[test]
fn a_handover_opens_only_for_its_device_and_approver() {
    let (approver, device) = (DeviceKey::generate(), DeviceKey::generate());
    let payload = br#"{"version":1,"holders":[],"revoked":["dev_gone"]}"#;
    let sealed = approver
        .seal_handover("dev_c", "dev_b", &device.encryption_key(), payload)
        .expect("sealed");
    let opened = device.open_handover("dev_c", "dev_b", &sealed).expect("opens");
    assert_eq!(opened.payload, payload);
    assert!(opened.signed_by(&approver.signing_key()));
    assert!(!opened.signed_by(&device.signing_key()));
    // Bound to the device, the approver and the device's key, and to every byte.
    for (device_id, approver_id) in [("dev_x", "dev_b"), ("dev_c", "dev_x"), ("dev_b", "dev_c")] {
        assert_eq!(
            device.open_handover(device_id, approver_id, &sealed).err(),
            Some(E2eError::Open)
        );
    }
    assert_eq!(
        approver.open_handover("dev_c", "dev_b", &sealed).err(),
        Some(E2eError::Open)
    );
    assert_eq!(
        device.open_handover("dev_c", "dev_b", &flipped(&sealed, 400)).err(),
        Some(E2eError::Open)
    );
    assert_eq!(
        device.open_handover("dev_c", "dev_b", &sealed[..100]).err(),
        Some(E2eError::Open)
    );
    assert_eq!(handover_message("dev:c", "dev_b", payload).err(), Some(E2eError::Field));
    assert_eq!(
        seal_handover(
            "dev_c",
            "dev_b",
            &device.encryption_key(),
            &[0; SIGNATURE_LEN],
            &vec![b' '; MAX_HANDOVER_PAYLOAD + 1]
        )
        .err(),
        Some(E2eError::Seal)
    );
}

#[test]
fn a_handover_anyone_sealed_opens_but_only_its_signer_signed_it() {
    // HPKE base mode does not name the sender: Gateway knows the device's public key and can seal a list of its own.
    let (approver, device, gateway) = (DeviceKey::generate(), DeviceKey::generate(), DeviceKey::generate());
    let payload = br#"{"version":1,"holders":[],"revoked":[]}"#;
    let forged = gateway
        .seal_handover("dev_c", "dev_b", &device.encryption_key(), payload)
        .expect("sealed");
    let opened = device.open_handover("dev_c", "dev_b", &forged).expect("opens");
    assert!(!opened.signed_by(&approver.signing_key()));
    assert!(opened.signed_by(&gateway.signing_key()));
    // The approver's real signature for another device, or another payload, does not carry over.
    let other_device = approver.sign(&handover_message("dev_d", "dev_b", payload).expect("message"));
    let other_payload = approver.sign(&handover_message("dev_c", "dev_b", b"{}").expect("message"));
    for signature in [other_device, other_payload] {
        let sealed = seal_handover("dev_c", "dev_b", &device.encryption_key(), &signature, payload).expect("sealed");
        let opened = device.open_handover("dev_c", "dev_b", &sealed).expect("opens");
        assert!(!opened.signed_by(&approver.signing_key()));
    }
}

#[test]
fn holder_approvals_match_the_vectors() {
    let v = vectors();
    let signer = DeviceKey::from_secret_bytes(&array(&v.signer_secret));
    for a in v.holder_approvals {
        let kind = HolderKind::parse(&a.kind).expect("kind");
        let message = holder_approval_message(&a.holder_id, kind, &array(&a.encryption_key)).expect("message");
        assert_eq!(message, a.message);
        assert_eq!(hex::encode(signer.sign(&message)), a.signature);
    }
    assert_eq!(
        holder_approval_message("dev_1", HolderKind::Device, &[1; KEY_LEN]),
        Err(E2eError::Field),
        "a device's approval is v1"
    );
}

#[test]
fn a_recovery_code_round_trips_and_refuses_what_is_not_one() {
    for _ in 0..64 {
        let code = RecoveryCode::generate();
        let text = code.display();
        assert_eq!(text.len(), 32, "27 characters and 5 hyphens: {}", *text);
        assert_eq!(RecoveryCode::parse(&text).expect("parses").bytes(), code.bytes());
    }
    // Crockford's look-alikes read as digits.
    let code = RecoveryCode::from_bytes(&[0; RECOVERY_CODE_BYTES]);
    assert_eq!(*code.display(), "00000-00000-00000-0000-0000-0000");
    assert!(RecoveryCode::parse("OOOOO-OOOOO-OOOOO-oooo-0000-0000").is_ok());
    for bad in [
        "",
        "U0000-00000-00000-0000-0000-0000",
        "00000-00000-00000-0000-0000-000!",
        "00000",
    ] {
        assert_eq!(RecoveryCode::parse(bad).err(), Some(E2eError::Code), "{bad}");
    }
    // A holder locked for opens with the derived key, for its holder id only.
    let holder = HolderKey::from_recovery_code(&RecoveryCode::generate(), "user_ada").expect("holder");
    let content = ContentKey::generate(0);
    let locked = content
        .lock_for(THREAD, "dev_recovery", &holder.encryption_key())
        .expect("lock");
    let opened = holder
        .unlock_content_key(THREAD, 0, "dev_recovery", &locked)
        .expect("unlock");
    let sealed = content.seal_event(THREAD, 0, b"hello").expect("seal");
    assert_eq!(opened.open_event(THREAD, 0, &sealed).expect("open"), b"hello");
    assert!(holder.unlock_content_key(THREAD, 0, DEVICE, &locked).is_err());
    assert_eq!(
        HolderKey::from_recovery_code(&RecoveryCode::generate(), "")
            .err()
            .map(|_| ()),
        Some(())
    );
}

/// The texts of `nexus-state`'s `approval_message`, `thread_key_message` and `revocation_message`, field for field.
#[test]
fn messages_are_gateways_colon_separated_texts() {
    let (enc, sig) = ([1u8; KEY_LEN], [2u8; KEY_LEN]);
    assert_eq!(
        approval_message("dev_1", &enc, &sig).expect("message"),
        format!("nexus-device-approval:v1:dev_1:{}:{}", b64u(&enc), b64u(&sig))
    );
    assert_eq!(
        thread_key_message(THREAD, 1, "dev_1", &[0, 0, 0]).expect("message"),
        format!("nexus-thread-key:v1:{THREAD}:2:dev_1:AAAA")
    );
    assert_eq!(
        revocation_message("dev_1").expect("message"),
        "nexus-device-revoke:v1:dev_1"
    );
    for bad in ["", "dev:1", "dev 1", "dév", &"d".repeat(129)] {
        assert_eq!(revocation_message(bad), Err(E2eError::Field), "{bad:?}");
    }
    assert_eq!(thread_key_message("thread-a", 0, "dev_1", &[0]), Err(E2eError::Field));
}

#[test]
fn key_versions_are_epochs_plus_one() {
    assert_eq!(key_version(0), Ok(1));
    assert_eq!(key_version(i32::MAX as u64 - 1), Ok(i32::MAX));
    assert_eq!(key_version(i32::MAX as u64), Err(E2eError::Field));
    assert_eq!(key_version(u64::MAX), Err(E2eError::Field));
    assert_eq!(epoch_of(1), Some(0));
    assert_eq!(epoch_of(i32::MAX), Some(i32::MAX as u64 - 1));
    assert_eq!(epoch_of(0), None);
    assert_eq!(epoch_of(-1), None);
}

#[test]
fn a_signature_verifies_only_for_its_key_and_message() {
    let device = DeviceKey::generate();
    let message = revocation_message(DEVICE).expect("message");
    let signature = device.sign(&message);
    assert!(verify(&device.signing_key(), &message, &signature));
    assert!(!verify(&DeviceKey::generate().signing_key(), &message, &signature));
    assert!(!verify(
        &device.signing_key(),
        "nexus-device-revoke:v1:dev_2",
        &signature
    ));
    assert!(!verify(&device.signing_key(), &message, &flipped(&signature, 3)));
    assert!(!verify(&device.signing_key(), &message, &signature[..63]));
}

#[test]
fn a_short_code_is_sixteen_base32_characters_of_ten_bytes() {
    let mut print = [0u8; KEY_LEN];
    assert_eq!(short_code(&print), "AAAAAAAAAAAAAAAA");
    print[..10].fill(0xff);
    assert_eq!(short_code(&print), "7777777777777777");
    print[10..].fill(0x55);
    assert_eq!(short_code(&print), "7777777777777777", "only the first 10 bytes count");
    // RFC 4648 §10: "fooba" is MZXW6YTB.
    print[..10].copy_from_slice(b"foobafooba");
    assert_eq!(short_code(&print), "MZXW6YTBMZXW6YTB");
    assert_ne!(
        fingerprint(&[1; KEY_LEN], None),
        fingerprint(&[1; KEY_LEN], Some(&[0; KEY_LEN])),
        "a recovery key's empty field is not a zero key"
    );
}

#[test]
fn a_new_device_and_thread_round_trip() {
    let device = DeviceKey::generate();
    let restored = DeviceKey::from_secret_bytes(&device.secret_bytes());
    assert_eq!(restored.encryption_key(), device.encryption_key());
    assert_eq!(restored.signing_key(), device.signing_key());

    let content = ContentKey::generate(3);
    let locked = content
        .lock_for(THREAD, DEVICE, &device.encryption_key())
        .expect("lock");
    assert_eq!(locked.len(), LOCKED_LEN);
    let opened = restored.unlock_content_key(THREAD, 3, DEVICE, &locked).expect("unlock");
    assert_eq!(*opened.key, *content.key);
    assert_eq!(opened.epoch(), 3);

    let first = content.seal_event(THREAD, 0, b"hello").expect("seal");
    let again = content.seal_event(THREAD, 0, b"hello").expect("seal");
    assert_ne!(first, again, "every seal draws a fresh nonce");
    assert_eq!(first.len(), NONCE_LEN + 5 + TAG_LEN);
    assert_eq!(opened.open_event(THREAD, 0, &first).expect("open"), b"hello");
    assert_ne!(*ContentKey::generate(3).key, *content.key);
    assert_ne!(DeviceKey::generate().encryption_key(), device.encryption_key());
    assert_ne!(DeviceKey::generate().signing_key(), device.signing_key());
}

#[test]
fn an_event_opens_only_with_its_key_thread_epoch_and_number() {
    let content = ContentKey::generate(2);
    let sealed = content.seal_event(THREAD, 7, b"the diff").expect("seal");
    assert_eq!(
        content.open_event(OTHER_THREAD, 7, &sealed),
        Err(E2eError::Open),
        "another thread"
    );
    assert_eq!(
        content.open_event(THREAD, 8, &sealed),
        Err(E2eError::Open),
        "another position"
    );
    assert_eq!(
        content.open_event(THREAD, 7 + (1 << 32), &sealed),
        Err(E2eError::Open),
        "the high bytes count"
    );
    for epoch in [1, 3, 2 + (1 << 32)] {
        assert_eq!(
            at_epoch(&content, epoch).open_event(THREAD, 7, &sealed),
            Err(E2eError::Open),
            "epoch {epoch}"
        );
    }
    assert_eq!(
        ContentKey::generate(2).open_event(THREAD, 7, &sealed),
        Err(E2eError::Open),
        "another key"
    );
    assert_eq!(content.open_event(THREAD, 7, &sealed).expect("open"), b"the diff");
}

#[test]
fn a_thread_id_is_a_lowercase_hyphenated_uuid() {
    let content = ContentKey::generate(0);
    let upper = THREAD.to_uppercase();
    let simple = THREAD.replace('-', "");
    let braced = format!("{{{THREAD}}}");
    for bad in ["", "thr_a", upper.as_str(), simple.as_str(), braced.as_str()] {
        assert_eq!(content.seal_event(bad, 0, b"x"), Err(E2eError::Field), "{bad:?}");
        assert_eq!(content.open_event(bad, 0, &[0; 40]), Err(E2eError::Field), "{bad:?}");
        assert!(matches!(
            content.lock_for(bad, DEVICE, &DeviceKey::generate().encryption_key()),
            Err(E2eError::Field)
        ));
    }
}

#[test]
fn every_flipped_bit_or_cut_byte_fails_to_open_an_event() {
    let content = ContentKey::generate(0);
    let sealed = content.seal_event(THREAD, 3, b"tool output").expect("seal");
    for bit in 0..sealed.len() * 8 {
        assert_eq!(
            content.open_event(THREAD, 3, &flipped(&sealed, bit)),
            Err(E2eError::Open),
            "bit {bit}"
        );
    }
    for len in 0..sealed.len() {
        assert_eq!(
            content.open_event(THREAD, 3, &sealed[..len]),
            Err(E2eError::Open),
            "{len} bytes"
        );
    }
    let mut longer = sealed.clone();
    longer.push(0);
    assert_eq!(content.open_event(THREAD, 3, &longer), Err(E2eError::Open));
}

#[test]
fn an_exhausted_key_refuses_to_seal() {
    let content = ContentKey::generate(0);
    content.seals.store(MAX_SEALS_PER_KEY - 1, Ordering::Relaxed);
    let last = content.seal_event(THREAD, 0, b"last").expect("the last seal");
    assert_eq!(content.seal_event(THREAD, 1, b"one more"), Err(E2eError::Exhausted));
    assert_eq!(
        content.seal_event(THREAD, 1, b"one more"),
        Err(E2eError::Exhausted),
        "it stays refused"
    );
    assert_eq!(
        content.open_event(THREAD, 0, &last).expect("opening still works"),
        b"last"
    );
}

#[test]
fn a_locked_key_opens_only_on_its_holder_for_its_thread_epoch_and_id() {
    let device = DeviceKey::generate();
    let locked = ContentKey::generate(1)
        .lock_for(THREAD, DEVICE, &device.encryption_key())
        .expect("lock");
    let refused = |result: Result<ContentKey, E2eError>| matches!(result, Err(E2eError::Open));
    assert!(
        refused(DeviceKey::generate().unlock_content_key(THREAD, 1, DEVICE, &locked)),
        "another device"
    );
    assert!(
        refused(device.unlock_content_key(OTHER_THREAD, 1, DEVICE, &locked)),
        "another thread"
    );
    assert!(
        refused(device.unlock_content_key(THREAD, 0, DEVICE, &locked)),
        "another epoch"
    );
    assert!(
        refused(device.unlock_content_key(THREAD, 1 + (1 << 32), DEVICE, &locked)),
        "the high bytes count"
    );
    assert!(
        refused(device.unlock_content_key(THREAD, 1, "dev_other", &locked)),
        "another holder id"
    );
    for bit in 0..locked.len() * 8 {
        assert!(
            refused(device.unlock_content_key(THREAD, 1, DEVICE, &flipped(&locked, bit))),
            "bit {bit}"
        );
    }
    for len in [0, KEY_LEN, LOCKED_LEN - 1, LOCKED_LEN + 1] {
        let mut cut = locked.clone();
        cut.resize(len, 0);
        assert!(
            refused(device.unlock_content_key(THREAD, 1, DEVICE, &cut)),
            "{len} bytes"
        );
    }
    assert!(matches!(
        device.unlock_content_key(THREAD, 1, "dev:1", &locked),
        Err(E2eError::Field)
    ));
    assert!(device.unlock_content_key(THREAD, 1, DEVICE, &locked).is_ok());
}

#[test]
fn a_low_order_public_key_is_refused() {
    // The all-zero point and the point of order 8 from RFC 7748's list: X25519 with either gives an all-zero secret,
    // which RFC 9180 §7.1.4 says to refuse.
    let zero = [0u8; KEY_LEN];
    let order_eight = array("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800");
    for public in [zero, order_eight] {
        assert!(matches!(
            ContentKey::generate(0).lock_for(THREAD, DEVICE, &public),
            Err(E2eError::PublicKey)
        ));
    }
}

#[test]
fn a_team_key_opens_on_its_holder_and_then_opens_a_thread_locked_to_it() {
    // A member's device opens the team key, then the thread key locked to the team key.
    let (tenant, team_id) = ("ten_acme", "tmk_one");
    let device = DeviceKey::generate();
    let team = HolderKey::generate();
    let locked = team
        .lock_team_key_for(tenant, 2, DEVICE, &device.encryption_key())
        .expect("lock the team key");
    let opened = device
        .unlock_team_key(tenant, 2, DEVICE, &locked)
        .expect("the holder opens it");
    assert_eq!(opened.encryption_key(), team.encryption_key());
    let refused = |result: Result<HolderKey, E2eError>| matches!(result, Err(E2eError::Open));
    assert!(
        refused(DeviceKey::generate().unlock_team_key(tenant, 2, DEVICE, &locked)),
        "another device"
    );
    assert!(
        refused(device.unlock_team_key("ten_other", 2, DEVICE, &locked)),
        "another organization"
    );
    assert!(
        refused(device.unlock_team_key(tenant, 1, DEVICE, &locked)),
        "another epoch"
    );
    assert!(
        refused(device.unlock_team_key(tenant, 2, "dev_other", &locked)),
        "another holder id"
    );
    assert!(matches!(
        device.unlock_team_key(tenant, 0, DEVICE, &locked),
        Err(E2eError::Field)
    ));

    let content = ContentKey::generate(3);
    let sealed = content.seal_event(THREAD, 0, b"for the team").expect("seal");
    let thread_copy = content
        .lock_for(THREAD, team_id, &team.encryption_key())
        .expect("lock to the team key");
    let thread_key = opened
        .unlock_content_key(THREAD, 3, team_id, &thread_copy)
        .expect("the team key opens the thread key");
    assert_eq!(
        thread_key.open_event(THREAD, 0, &sealed).expect("open"),
        b"for the team"
    );
    // The next epoch's team key opens nothing locked to this one.
    assert!(
        HolderKey::generate()
            .unlock_content_key(THREAD, 3, team_id, &thread_copy)
            .is_err()
    );
}

#[test]
fn team_messages_are_gateways_texts() {
    assert_eq!(
        team_key_message("ten_1", 2, "dev_1", &[0, 0, 0]).expect("message"),
        "nexus-team-key:v1:ten_1:2:dev_1:AAAA"
    );
    assert_eq!(
        team_thread_key_message(THREAD, 1, "tmk_1", &[0, 0, 0]).expect("message"),
        format!("nexus-team-thread-key:v1:{THREAD}:2:tmk_1:AAAA")
    );
    assert!(team_key_message("ten:1", 2, "dev_1", &[]).is_err());
}

#[test]
fn org_key_messages_and_fingerprints_are_gateways() {
    assert_eq!(
        org_thread_key_message(THREAD, 1, "ork_1", &[0, 0, 0]).expect("message"),
        format!("nexus-org-thread-key:v1:{THREAD}:2:ork_1:AAAA")
    );
    assert!(org_thread_key_message(THREAD, 1, "ork:1", &[]).is_err());
    // Gateway's fixed vector (`devices::org_key`'s test), computed apart with Python's hashlib.
    assert_eq!(
        org_key_fingerprint(&[0; KEY_LEN]),
        "e10e33546b772652b9faffb62b970e29735472a6eba7a68fa7257edb7b8f8dce"
    );
    assert_ne!(org_key_fingerprint(&[7; KEY_LEN]), org_key_fingerprint(&[8; KEY_LEN]));
}
