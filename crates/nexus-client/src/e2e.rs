//! End-to-end encryption of synced sessions: two key pairs
//! per device, a random content key per thread and epoch, each event sealed with that content key, and the content key
//! locked to each trusted holder's X25519 key. Keychain storage and sync live in the callers.
//!
//! - **Events**: AES-256-GCM under the content key, with a random 96-bit nonce per event. The associated data is
//!   [`EVENT_AAD_LABEL`], the thread id, the key's epoch and the event's zero-based sequence number, each
//!   length-prefixed. A stored event cannot be replayed into another thread, epoch or position. Sealed bytes are
//!   `nonce (12) || ciphertext || tag (16)`, with no version byte: the label versions the format.
//! - **Nonce budget**: one content key seals at most [`MAX_SEALS_PER_KEY`] events (NIST SP 800-38D's bound for random
//!   nonces). The next seal fails with [`E2eError::Exhausted`] instead of risking a repeated nonce; the runner rotates.
//!   The count lives in the [`ContentKey`] value, so a key restored after a restart counts again from zero.
//! - **Locked copies**: HPKE (RFC 9180) base mode, DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-256-GCM. The `info`
//!   is [`LOCK_INFO_LABEL`], the thread id, the epoch and the holder id, each length-prefixed, and the
//!   associated data is empty. Locked bytes are `enc (32) || ciphertext (32) || tag (16)`.
//! - **Signatures**: Ed25519 over the colon-separated text Gateway verifies (`nexus-state`'s `devices` module):
//!   [`approval_message`], [`thread_key_message`] and [`revocation_message`]. No field can hold a `:`.
//! - **The team key**: an organization's X25519 pair, a [`HolderKey`] every member's trusted holders
//!   hold. A team-visible thread's content key is locked to it as to any holder, with the team key's id as the holder
//!   id. Its private half is locked to each holder with the same HPKE suite. That `info` is [`TEAM_KEY_LOCK_LABEL`], the
//!   organization id, the epoch and the holder id. [`team_key_message`] and [`team_thread_key_message`] are signed.
//! - **The organization's recovery key**: under the `user_and_org` content policy, a thread's content key
//!   is locked to it as to any holder, with its id as the holder id, and [`org_thread_key_message`] is signed. A member
//!   pins it by [`org_key_fingerprint`], the form Gateway serves and Nexus UI shows.
//! - **Fingerprints**: the SHA-256 of [`FINGERPRINT_LABEL`], the X25519 key and the Ed25519 key, each
//!   length-prefixed; a recovery key's Ed25519 field is empty. [`short_code`] is its first 10 bytes in base32.
//! - **Recovery keys**: a person's passkey and printable recovery code each derive an X25519 key pair, a
//!   [`HolderKey`], with no Ed25519 key, so they never sign. The recovery code is 128 random bits shown as 26 Crockford
//!   base32 characters and a check symbol ([`RecoveryCode`]); HKDF-SHA256 turns its 16 bytes into the private key, with
//!   [`RECOVERY_CODE_SALT`] and the length-prefixed person id. A passkey's WebAuthn PRF output does the same, with
//!   [`PASSKEY_SALT`] and the length-prefixed person id and credential id; the PRF input is [`passkey_prf_input`].
//!   Gateway approves them with [`holder_approval_message`]. To reset a person's devices after every one is lost,
//!   Gateway seals a random challenge to each recovery key; [`HolderKey::open_recovery_challenge`] opens it, which
//!   proves the key. Its HPKE `info` is [`RECOVER_CHALLENGE_LABEL`], the person id and the holder id.
//! - **Handovers**: an approver hands the device it approves its own trusted holders and remembered
//!   revoked ids. It signs [`handover_message`] over the payload and seals the signature and payload to the new device
//!   with HPKE, `info` [`HANDOVER_LABEL`], the device id and the approver id. Gateway stores the sealed bytes unread.
//! - **Length prefixes**: each field is its byte length as 2 big-endian bytes, then the bytes. Thread ids are lowercase
//!   hyphenated UUIDs; holder ids are 1 to 128 of `A-Z a-z 0-9 _ -`, as Gateway mints them (`dev_…`); epochs and
//!   sequence numbers are 8 big-endian bytes.
//! - **Secrets** (both private keys, content keys and the AES round keys) are wiped from memory on drop. Ed25519 signs
//!   through `ring`, whose key pair is made for each signature and not wiped.
//!
//! Every byte of every format is pinned by `tests/e2e-vectors.json`, which the browser's side checks against too.

use std::sync::atomic::{AtomicU64, Ordering};

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use getrandom::SysRng;
use getrandom::rand_core::{CryptoRng, Rng, UnwrapErr};
use hpke::{Deserializable, Kem as _, OpModeR, OpModeS, Serializable};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use zeroize::Zeroizing;

type Kem = hpke::kem::X25519HkdfSha256;
type Kdf = hpke::kdf::HkdfSha256;
type LockAead = hpke::aead::AesGcm256;

/// The first field of every event's associated data. Another use of the content key takes another label.
pub const EVENT_AAD_LABEL: &str = "nexus-event-v1";
/// The first field of HPKE's `info` when a content key is locked to a holder.
pub const LOCK_INFO_LABEL: &str = "nexus-thread-key-v1";
/// The first field of HPKE's `info` when a team key's private half is locked to a holder.
pub const TEAM_KEY_LOCK_LABEL: &str = "nexus-team-key-v1";
/// The first field a holder's fingerprint hashes.
pub const FINGERPRINT_LABEL: &str = "nexus-holder-fingerprint-v1";
/// The first field of HPKE's `info` when Gateway seals a recovery challenge to a recovery key.
pub const RECOVER_CHALLENGE_LABEL: &str = "nexus-recover-challenge-v1";
/// A recovery challenge: 32 random bytes.
pub const RECOVER_CHALLENGE_LEN: usize = 32;
/// The HKDF salt that turns a recovery code's 16 bytes into its holder's X25519 private key.
pub const RECOVERY_CODE_SALT: &str = "nexus-recovery-code-v1";
/// The HKDF salt that turns a passkey's PRF output into its holder's X25519 private key.
pub const PASSKEY_SALT: &str = "nexus-passkey-v1";
/// The text whose SHA-256 is the passkey's PRF input, WebAuthn's `prf.eval.first` ([`passkey_prf_input`]).
pub const PASSKEY_PRF_LABEL: &str = "nexus-passkey-prf-v1";
/// The first field of HPKE's `info` when an approver seals its trusted list to the device it approves.
pub const HANDOVER_LABEL: &str = "nexus-device-handover-v1";
/// The longest handover payload a device seals: its sealed form fits Gateway's 65,536 base64url characters.
pub const MAX_HANDOVER_PAYLOAD: usize = 48_000;
/// A recovery code's random bytes: 128 bits.
pub const RECOVERY_CODE_BYTES: usize = 16;
/// The most events one content key seals: 2^32, NIST SP 800-38D's bound for random 96-bit nonces.
pub const MAX_SEALS_PER_KEY: u64 = 1 << 32;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
/// A device's stored secret: its X25519 private key, then its Ed25519 seed.
pub const DEVICE_SECRET_LEN: usize = 2 * KEY_LEN;
/// A locked content key: HPKE's encapsulated key (an X25519 public key), then the sealed content key and its tag.
pub const LOCKED_LEN: usize = KEY_LEN + KEY_LEN + TAG_LEN;
/// An Ed25519 signature.
pub const SIGNATURE_LEN: usize = 64;
/// The fingerprint bytes a short code carries: 10 bytes, 16 base32 characters.
const SHORT_CODE_BYTES: usize = 10;
const MAX_HOLDER_ID_LEN: usize = 128;
/// The longest person id a derivation binds, in bytes.
const MAX_PERSON_ID_LEN: usize = 128;
/// The longest WebAuthn credential id, in bytes (WebAuthn Level 3, §4).
const MAX_CREDENTIAL_ID_LEN: usize = 1023;
/// Crockford's base32 digits, then the five extra symbols of its mod-37 check symbol.
const CROCKFORD: &[u8; 37] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ*~$=U";
/// A recovery code's characters: 26 digits for 128 bits, then the check symbol.
const RECOVERY_CODE_DIGITS: usize = 26;
/// How a recovery code is grouped for reading: 27 characters.
const RECOVERY_CODE_GROUPS: [usize; 6] = [5, 5, 5, 4, 4, 4];

/// Why an end-to-end encryption step failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum E2eError {
    /// The bytes did not open: the wrong key, the wrong thread, epoch, event number or holder, or changed since they
    /// were sealed. Deliberately one answer for every cause.
    Open,
    /// The holder's X25519 key cannot be encrypted to (a low-order point).
    PublicKey,
    /// The plaintext is beyond what AES-GCM can seal under one nonce (64 GiB).
    Seal,
    /// The content key has sealed [`MAX_SEALS_PER_KEY`] events. Rotate it.
    Exhausted,
    /// A thread id is not a lowercase hyphenated UUID, a holder id is not 1 to 128 of `A-Z a-z 0-9 _ -`, an epoch is
    /// too large for Gateway's key version, or a person id or credential id is empty or too long.
    Field,
    /// The text is not a recovery code: a wrong length, a character outside Crockford's base32, or a check symbol that
    /// does not match. Caught before any network call.
    Code,
}

impl std::fmt::Display for E2eError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            E2eError::Open => "the encrypted data could not be opened",
            E2eError::PublicKey => "the holder's public key is not a usable X25519 key",
            E2eError::Seal => "the data is too long to encrypt",
            E2eError::Exhausted => "the content key has sealed all the events it may; rotate it",
            E2eError::Field => "a thread id, holder id or epoch is not in the form the format binds",
            E2eError::Code => "that is not a recovery code: check each character",
        })
    }
}

impl std::error::Error for E2eError {}

/// The operating system's random source. Panics only if the OS cannot give randomness, as RustCrypto's own
/// `generate()` does.
fn os_rng() -> UnwrapErr<SysRng> {
    UnwrapErr(SysRng)
}

/// This device's two key pairs, made on the device: X25519 receives locked content keys, Ed25519 signs. The
/// private keys are wiped from memory on drop.
pub struct DeviceKey {
    secret: <Kem as hpke::Kem>::PrivateKey,
    public: <Kem as hpke::Kem>::PublicKey,
    signing_seed: Zeroizing<[u8; KEY_LEN]>,
    signing_public: [u8; KEY_LEN],
}

impl DeviceKey {
    pub fn generate() -> DeviceKey {
        let mut secret = Zeroizing::new([0u8; DEVICE_SECRET_LEN]);
        os_rng().fill_bytes(&mut secret[..]);
        DeviceKey::from_secret_bytes(&secret)
    }

    /// A stored device: the X25519 private key (RFC 7748's 32 bytes, as RFC 9180 §7.1.2 serializes it), then the
    /// Ed25519 seed (RFC 8032's 32-byte private key). [`DeviceKey::secret_bytes`] writes them.
    pub fn from_secret_bytes(bytes: &[u8; DEVICE_SECRET_LEN]) -> DeviceKey {
        let (x25519, ed25519) = bytes.split_at(KEY_LEN);
        let secret =
            <Kem as hpke::Kem>::PrivateKey::from_bytes(x25519).expect("any 32 bytes are an X25519 private key");
        let public = Kem::sk_to_pk(&secret);
        let mut signing_seed = Zeroizing::new([0u8; KEY_LEN]);
        signing_seed.copy_from_slice(ed25519);
        let mut device = DeviceKey {
            secret,
            public,
            signing_seed,
            signing_public: [0; KEY_LEN],
        };
        let signing_public = device.signing_pair().public_key().as_ref().to_vec();
        device.signing_public.copy_from_slice(&signing_public);
        device
    }

    /// The bytes [`DeviceKey::from_secret_bytes`] reads, for the keychain.
    pub fn secret_bytes(&self) -> Zeroizing<[u8; DEVICE_SECRET_LEN]> {
        let mut bytes = Zeroizing::new([0u8; DEVICE_SECRET_LEN]);
        self.secret.write_exact(&mut bytes[..KEY_LEN]);
        bytes[KEY_LEN..].copy_from_slice(&self.signing_seed[..]);
        bytes
    }

    /// The X25519 key holders lock content keys to: 32 bytes.
    pub fn encryption_key(&self) -> [u8; KEY_LEN] {
        self.public.to_bytes().into()
    }

    /// The Ed25519 key that verifies this device's signatures: 32 bytes.
    pub fn signing_key(&self) -> [u8; KEY_LEN] {
        self.signing_public
    }

    /// This device's fingerprint, for its QR code.
    pub fn fingerprint(&self) -> [u8; KEY_LEN] {
        fingerprint(&self.encryption_key(), Some(&self.signing_key()))
    }

    /// The Ed25519 signature over `message`: one of [`approval_message`], [`thread_key_message`] or
    /// [`revocation_message`].
    pub fn sign(&self, message: &str) -> [u8; SIGNATURE_LEN] {
        let mut signature = [0u8; SIGNATURE_LEN];
        signature.copy_from_slice(self.signing_pair().sign(message.as_bytes()).as_ref());
        signature
    }

    fn signing_pair(&self) -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&self.signing_seed[..]).expect("any 32 bytes are an Ed25519 seed")
    }

    /// Opens a content key that [`ContentKey::lock_for`] locked to this device as `holder_id`, for `thread_id` at
    /// `epoch`. The key it returns seals at that epoch.
    pub fn unlock_content_key(
        &self,
        thread_id: &str,
        epoch: u64,
        holder_id: &str,
        locked: &[u8],
    ) -> Result<ContentKey, E2eError> {
        unlock(&self.secret, thread_id, epoch, holder_id, locked)
    }

    /// Signs `payload` as approver `approver_id` for the new device `device_id`, then seals the signature and the
    /// payload to that device's X25519 key ([`seal_handover`]). The ids come in the order of every handover function:
    /// the device, then its approver.
    pub fn seal_handover(
        &self,
        device_id: &str,
        approver_id: &str,
        device_key: &[u8; KEY_LEN],
        payload: &[u8],
    ) -> Result<Vec<u8>, E2eError> {
        let signature = self.sign(&handover_message(device_id, approver_id, payload)?);
        seal_handover(device_id, approver_id, device_key, &signature, payload)
    }

    /// Opens a handover sealed to this device as `device_id` by `approver_id`. It does not check the signature: the
    /// caller checks it against the approver it trusts ([`OpenedHandover::signed_by`]). Anyone who knows this device's
    /// public key can seal one, Gateway included.
    pub fn open_handover(&self, device_id: &str, approver_id: &str, sealed: &[u8]) -> Result<OpenedHandover, E2eError> {
        if sealed.len() < KEY_LEN + SIGNATURE_LEN + TAG_LEN || sealed.len() > handover_sealed_len(MAX_HANDOVER_PAYLOAD)
        {
            return Err(E2eError::Open);
        }
        let (enc, ciphertext) = sealed.split_at(KEY_LEN);
        let enc = <Kem as hpke::Kem>::EncappedKey::from_bytes(enc).map_err(|_| E2eError::Open)?;
        let info = handover_info(device_id, approver_id)?;
        let opened =
            hpke::single_shot_open::<LockAead, Kdf, Kem>(&OpModeR::Base, &self.secret, &enc, &info, ciphertext, &[])
                .map_err(|_| E2eError::Open)?;
        let (signature, payload) = opened.split_at(SIGNATURE_LEN);
        let mut handover = OpenedHandover {
            device_id: device_id.to_string(),
            approver_id: approver_id.to_string(),
            signature: [0; SIGNATURE_LEN],
            payload: payload.to_vec(),
        };
        handover.signature.copy_from_slice(signature);
        Ok(handover)
    }

    /// Opens the team key's private half that [`HolderKey::lock_team_key_for`] locked to this device as `holder_id`,
    /// for organization `tenant_id` at `epoch`.
    pub fn unlock_team_key(
        &self,
        tenant_id: &str,
        epoch: i32,
        holder_id: &str,
        locked: &[u8],
    ) -> Result<HolderKey, E2eError> {
        let secret = open_locked(&self.secret, &team_lock_info(tenant_id, epoch, holder_id)?, locked)?;
        Ok(HolderKey::from_secret_bytes(&secret))
    }
}

/// A recovery key: an X25519 key pair derived from a passkey or a recovery code, with no Ed25519 key. It
/// receives locked copies and never signs. The private key is wiped from memory on drop.
pub struct HolderKey {
    secret: <Kem as hpke::Kem>::PrivateKey,
    public: <Kem as hpke::Kem>::PublicKey,
}

impl HolderKey {
    /// A new random pair: a team key's epoch.
    pub fn generate() -> HolderKey {
        let mut secret = Zeroizing::new([0u8; KEY_LEN]);
        os_rng().fill_bytes(&mut secret[..]);
        HolderKey::from_secret_bytes(&secret)
    }

    /// Locks this pair's private half, a team key's epoch, to holder `holder_id`'s X25519 key, for organization
    /// `tenant_id` at `epoch` (HPKE base mode). Only that holder opens it ([`DeviceKey::unlock_team_key`]).
    pub fn lock_team_key_for(
        &self,
        tenant_id: &str,
        epoch: i32,
        holder_id: &str,
        holder_key: &[u8; KEY_LEN],
    ) -> Result<Vec<u8>, E2eError> {
        let mut secret = Zeroizing::new([0u8; KEY_LEN]);
        self.secret.write_exact(&mut secret[..]);
        seal_locked(
            &team_lock_info(tenant_id, epoch, holder_id)?,
            &secret,
            holder_key,
            &mut os_rng(),
        )
    }

    /// A holder from its X25519 private key (RFC 7748's 32 bytes).
    pub fn from_secret_bytes(bytes: &[u8; KEY_LEN]) -> HolderKey {
        let secret = <Kem as hpke::Kem>::PrivateKey::from_bytes(bytes).expect("any 32 bytes are an X25519 private key");
        let public = Kem::sk_to_pk(&secret);
        HolderKey { secret, public }
    }

    /// The recovery code's holder for `person_id` (`users.id`): HKDF-SHA256 with the code's 16 bytes as input keying
    /// material, [`RECOVERY_CODE_SALT`] as salt and the length-prefixed person id as info. The code has 128 bits of
    /// entropy, so no slow KDF is needed.
    pub fn from_recovery_code(code: &RecoveryCode, person_id: &str) -> Result<HolderKey, E2eError> {
        check_person_id(person_id)?;
        let info = length_prefixed(&[person_id.as_bytes()]);
        Ok(HolderKey::from_secret_bytes(&hkdf_sha256(
            &code.0[..],
            RECOVERY_CODE_SALT,
            &info,
        )))
    }

    /// The passkey's holder for `person_id` (`users.id`): HKDF-SHA256 with the WebAuthn PRF output (32 bytes, for the
    /// input [`passkey_prf_input`]) as input keying material, [`PASSKEY_SALT`] as salt, and the length-prefixed person
    /// id and credential id (the raw `rawId` bytes) as info.
    pub fn from_passkey_prf(
        prf_output: &[u8; KEY_LEN],
        person_id: &str,
        credential_id: &[u8],
    ) -> Result<HolderKey, E2eError> {
        check_person_id(person_id)?;
        if credential_id.is_empty() || credential_id.len() > MAX_CREDENTIAL_ID_LEN {
            return Err(E2eError::Field);
        }
        let info = length_prefixed(&[person_id.as_bytes(), credential_id]);
        Ok(HolderKey::from_secret_bytes(&hkdf_sha256(
            prf_output,
            PASSKEY_SALT,
            &info,
        )))
    }

    /// The X25519 key holders lock content keys to: 32 bytes.
    pub fn encryption_key(&self) -> [u8; KEY_LEN] {
        self.public.to_bytes().into()
    }

    /// Its fingerprint, with an empty Ed25519 field.
    pub fn fingerprint(&self) -> [u8; KEY_LEN] {
        fingerprint(&self.encryption_key(), None)
    }

    /// Opens the recovery challenge Gateway sealed to this holder as `holder_id`, for `person_id`. Only this
    /// holder's private key opens it, and only for that person and holder id.
    pub fn open_recovery_challenge(
        &self,
        person_id: &str,
        holder_id: &str,
        sealed: &[u8],
    ) -> Result<Zeroizing<[u8; RECOVER_CHALLENGE_LEN]>, E2eError> {
        let info = recover_challenge_info(person_id, holder_id)?;
        if sealed.len() != KEY_LEN + RECOVER_CHALLENGE_LEN + TAG_LEN {
            return Err(E2eError::Open);
        }
        let (enc, ciphertext) = sealed.split_at(KEY_LEN);
        let enc = <Kem as hpke::Kem>::EncappedKey::from_bytes(enc).map_err(|_| E2eError::Open)?;
        let opened = Zeroizing::new(
            hpke::single_shot_open::<LockAead, Kdf, Kem>(&OpModeR::Base, &self.secret, &enc, &info, ciphertext, &[])
                .map_err(|_| E2eError::Open)?,
        );
        let mut challenge = Zeroizing::new([0u8; RECOVER_CHALLENGE_LEN]);
        challenge.copy_from_slice(&opened);
        Ok(challenge)
    }

    /// Opens a content key locked to this holder as `holder_id`, as [`DeviceKey::unlock_content_key`] does.
    pub fn unlock_content_key(
        &self,
        thread_id: &str,
        epoch: u64,
        holder_id: &str,
        locked: &[u8],
    ) -> Result<ContentKey, E2eError> {
        unlock(&self.secret, thread_id, epoch, holder_id, locked)
    }
}

/// HPKE base-mode open of a locked content key with `secret`, bound to the lock's `info`.
fn unlock(
    secret: &<Kem as hpke::Kem>::PrivateKey,
    thread_id: &str,
    epoch: u64,
    holder_id: &str,
    locked: &[u8],
) -> Result<ContentKey, E2eError> {
    let key = open_locked(secret, &lock_info(thread_id, epoch, holder_id)?, locked)?;
    Ok(ContentKey::new(key, epoch))
}

/// HPKE base-mode open of 32 locked bytes with `secret`, bound to `info`.
fn open_locked(
    secret: &<Kem as hpke::Kem>::PrivateKey,
    info: &[u8],
    locked: &[u8],
) -> Result<Zeroizing<[u8; KEY_LEN]>, E2eError> {
    if locked.len() != LOCKED_LEN {
        return Err(E2eError::Open);
    }
    let (enc, sealed) = locked.split_at(KEY_LEN);
    let enc = <Kem as hpke::Kem>::EncappedKey::from_bytes(enc).map_err(|_| E2eError::Open)?;
    let opened = hpke::single_shot_open::<LockAead, Kdf, Kem>(&OpModeR::Base, secret, &enc, info, sealed, &[])
        .map_err(|_| E2eError::Open)?;
    let opened = Zeroizing::new(opened);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    // The length check above leaves exactly a 32-byte plaintext once the tag verifies.
    key.copy_from_slice(&opened);
    Ok(key)
}

/// HPKE base-mode seal of 32 bytes to `holder_key`, bound to `info`: `enc (32) || ciphertext (32) || tag (16)`.
fn seal_locked(
    info: &[u8],
    plaintext: &[u8; KEY_LEN],
    holder_key: &[u8; KEY_LEN],
    rng: &mut impl CryptoRng,
) -> Result<Vec<u8>, E2eError> {
    seal_bytes(info, plaintext, holder_key, rng)
}

/// HPKE base-mode seal of `plaintext` to `holder_key`, bound to `info`: `enc (32) || ciphertext || tag (16)`.
fn seal_bytes(
    info: &[u8],
    plaintext: &[u8],
    holder_key: &[u8; KEY_LEN],
    rng: &mut impl CryptoRng,
) -> Result<Vec<u8>, E2eError> {
    let public = <Kem as hpke::Kem>::PublicKey::from_bytes(holder_key).map_err(|_| E2eError::PublicKey)?;
    let (enc, sealed) =
        hpke::single_shot_seal_with_rng::<LockAead, Kdf, Kem>(&OpModeS::Base, &public, info, plaintext, &[], rng)
            .map_err(|error| match error {
                // The only way encapsulation fails: an all-zero shared secret, from a low-order public key.
                hpke::HpkeError::EncapError => E2eError::PublicKey,
                _ => E2eError::Seal,
            })?;
    let mut locked = Vec::with_capacity(KEY_LEN + sealed.len());
    locked.extend_from_slice(&enc.to_bytes());
    locked.extend_from_slice(&sealed);
    Ok(locked)
}

/// HKDF-SHA256 (RFC 5869) to 32 bytes.
fn hkdf_sha256(ikm: &[u8], salt: &str, info: &[u8]) -> Zeroizing<[u8; KEY_LEN]> {
    struct Len;
    impl ring::hkdf::KeyType for Len {
        fn len(&self) -> usize {
            KEY_LEN
        }
    }
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, salt.as_bytes())
        .extract(ikm)
        .expand(&[info], Len)
        .and_then(|okm| okm.fill(&mut out[..]))
        .expect("32 bytes is within HKDF-SHA256's output");
    out
}

/// The passkey's WebAuthn PRF input, `prf.eval.first`: the SHA-256 of [`PASSKEY_PRF_LABEL`].
pub fn passkey_prf_input() -> [u8; KEY_LEN] {
    let mut digest = [0u8; KEY_LEN];
    digest.copy_from_slice(ring::digest::digest(&ring::digest::SHA256, PASSKEY_PRF_LABEL.as_bytes()).as_ref());
    digest
}

/// A printable recovery code: 128 bits from the operating system's random source, wiped from memory on drop.
/// It reads as 26 Crockford base32 digits, most significant first, then Crockford's mod-37 check symbol, in groups of
/// five and four: `XXXXX-XXXXX-XXXXX-XXXX-XXXX-XXXX`.
pub struct RecoveryCode(Zeroizing<[u8; RECOVERY_CODE_BYTES]>);

impl RecoveryCode {
    pub fn generate() -> RecoveryCode {
        let mut bytes = Zeroizing::new([0u8; RECOVERY_CODE_BYTES]);
        os_rng().fill_bytes(&mut bytes[..]);
        RecoveryCode(bytes)
    }

    pub fn from_bytes(bytes: &[u8; RECOVERY_CODE_BYTES]) -> RecoveryCode {
        RecoveryCode(Zeroizing::new(*bytes))
    }

    pub fn bytes(&self) -> &[u8; RECOVERY_CODE_BYTES] {
        &self.0
    }

    /// The code as typed: any case, hyphens and spaces ignored, `O` read as `0` and `I` and `L` as `1`, as Crockford
    /// says. Refuses anything else with [`E2eError::Code`], a wrong check symbol included.
    pub fn parse(text: &str) -> Result<RecoveryCode, E2eError> {
        let symbols: Vec<u8> = text
            .bytes()
            .filter(|byte| !matches!(byte, b'-' | b' '))
            .map(|byte| match byte.to_ascii_uppercase() {
                b'O' => b'0',
                b'I' | b'L' => b'1',
                other => other,
            })
            .collect();
        let [digits @ .., check] = symbols.as_slice() else {
            return Err(E2eError::Code);
        };
        if digits.len() != RECOVERY_CODE_DIGITS {
            return Err(E2eError::Code);
        }
        let mut value: u128 = 0;
        for (place, symbol) in digits.iter().enumerate() {
            let digit = CROCKFORD[..32]
                .iter()
                .position(|candidate| candidate == symbol)
                .ok_or(E2eError::Code)? as u128;
            // 26 digits carry 130 bits: the first is at most 7, or the value passes 128 bits.
            if place == 0 && digit > 7 {
                return Err(E2eError::Code);
            }
            value = (value << 5) | digit;
        }
        if CROCKFORD[(value % 37) as usize] != *check {
            return Err(E2eError::Code);
        }
        Ok(RecoveryCode(Zeroizing::new(value.to_be_bytes())))
    }

    /// The code as printed: 27 characters in groups, joined by hyphens.
    pub fn display(&self) -> Zeroizing<String> {
        let value = u128::from_be_bytes(*self.0);
        let mut symbols = Zeroizing::new(Vec::with_capacity(RECOVERY_CODE_DIGITS + 1));
        for place in (0..RECOVERY_CODE_DIGITS).rev() {
            symbols.push(CROCKFORD[((value >> (5 * place)) & 31) as usize]);
        }
        symbols.push(CROCKFORD[(value % 37) as usize]);
        let mut out = Zeroizing::new(String::with_capacity(32));
        let mut at = 0;
        for (index, size) in RECOVERY_CODE_GROUPS.iter().enumerate() {
            if index > 0 {
                out.push('-');
            }
            out.extend(symbols[at..at + size].iter().map(|byte| char::from(*byte)));
            at += size;
        }
        out
    }
}

/// A thread's content key at one epoch: 256 random bits, wiped from memory on drop. It counts its seals (see
/// [`MAX_SEALS_PER_KEY`]).
pub struct ContentKey {
    key: Zeroizing<[u8; KEY_LEN]>,
    epoch: u64,
    seals: AtomicU64,
}

impl ContentKey {
    /// A new key for `epoch`: 0 for a thread's first key, one more at each rotation.
    pub fn generate(epoch: u64) -> ContentKey {
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        os_rng().fill_bytes(&mut key[..]);
        ContentKey::new(key, epoch)
    }

    fn new(key: Zeroizing<[u8; KEY_LEN]>, epoch: u64) -> ContentKey {
        ContentKey {
            key,
            epoch,
            seals: AtomicU64::new(0),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Seals event `seq` of `thread_id`, with a fresh random nonce. Fails with [`E2eError::Exhausted`] once this key
    /// has sealed [`MAX_SEALS_PER_KEY`] events.
    pub fn seal_event(&self, thread_id: &str, seq: u64, plaintext: &[u8]) -> Result<Vec<u8>, E2eError> {
        let mut nonce = [0u8; NONCE_LEN];
        os_rng().fill_bytes(&mut nonce);
        self.seal_event_with_nonce(&nonce, thread_id, seq, plaintext)
    }

    fn seal_event_with_nonce(
        &self,
        nonce: &[u8; NONCE_LEN],
        thread_id: &str,
        seq: u64,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, E2eError> {
        let aad = event_aad(thread_id, self.epoch, seq)?;
        // Every attempt counts, refused ones too, so the count only grows. 2^64 attempts never happen.
        if self.seals.fetch_add(1, Ordering::Relaxed) >= MAX_SEALS_PER_KEY {
            return Err(E2eError::Exhausted);
        }
        let ciphertext = self
            .cipher()
            .encrypt(
                &Nonce::from(*nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| E2eError::Seal)?;
        let mut sealed = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        sealed.extend_from_slice(nonce);
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }

    /// Opens event `seq` of `thread_id`. Fails unless it was sealed with this key, at this key's epoch, for exactly
    /// that thread and number.
    pub fn open_event(&self, thread_id: &str, seq: u64, sealed: &[u8]) -> Result<Vec<u8>, E2eError> {
        let aad = event_aad(thread_id, self.epoch, seq)?;
        if sealed.len() < NONCE_LEN + TAG_LEN {
            return Err(E2eError::Open);
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
        let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| E2eError::Open)?;
        self.cipher()
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| E2eError::Open)
    }

    /// Locks this key to holder `holder_id`'s X25519 key, for `thread_id` at this key's epoch (HPKE base mode). Only
    /// that holder's private key opens it, and only for the same thread, epoch and holder id.
    pub fn lock_for(&self, thread_id: &str, holder_id: &str, holder_key: &[u8; KEY_LEN]) -> Result<Vec<u8>, E2eError> {
        self.lock_with_rng(thread_id, holder_id, holder_key, &mut os_rng())
    }

    fn lock_with_rng(
        &self,
        thread_id: &str,
        holder_id: &str,
        holder_key: &[u8; KEY_LEN],
        rng: &mut impl CryptoRng,
    ) -> Result<Vec<u8>, E2eError> {
        seal_locked(
            &lock_info(thread_id, self.epoch, holder_id)?,
            &self.key,
            holder_key,
            rng,
        )
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(&(*self.key).into())
    }
}

/// A holder's fingerprint: the SHA-256 of [`FINGERPRINT_LABEL`], its X25519 key and its Ed25519 key, each
/// length-prefixed. A recovery key has no Ed25519 key: pass `None`, and that field is empty.
pub fn fingerprint(encryption_key: &[u8; KEY_LEN], signing_key: Option<&[u8; KEY_LEN]>) -> [u8; KEY_LEN] {
    let signing: &[u8] = signing_key.map_or(&[], |key| &key[..]);
    let bytes = length_prefixed(&[FINGERPRINT_LABEL.as_bytes(), encryption_key, signing]);
    let mut digest = [0u8; KEY_LEN];
    digest.copy_from_slice(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
    digest
}

/// The organization recovery key's fingerprint, as Gateway serves it (`nexus-state`'s `devices::org_key::fingerprint`)
/// and Nexus UI shows it: the lower-case hex SHA-256 of [`FINGERPRINT_LABEL`] and the key's 32 bytes, joined with no
/// length prefix. It is not [`fingerprint`]'s form. A member compares it with the one the organization publishes from
/// its vault, so a client computes it from the key it is about to lock for, never takes Gateway's.
pub fn org_key_fingerprint(encryption_key: &[u8; KEY_LEN]) -> String {
    let mut bytes = FINGERPRINT_LABEL.as_bytes().to_vec();
    bytes.extend_from_slice(encryption_key);
    ring::digest::digest(&ring::digest::SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The code a terminal shows instead of a QR code: the fingerprint's first 10 bytes as 16 base32 characters
/// (RFC 4648's upper-case alphabet, no padding).
pub fn short_code(fingerprint: &[u8; KEY_LEN]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let bits = fingerprint[..SHORT_CODE_BYTES]
        .iter()
        .fold(0u128, |bits, byte| (bits << 8) | u128::from(*byte));
    (0..16)
        .rev()
        .map(|place| char::from(ALPHABET[((bits >> (5 * place)) & 31) as usize]))
        .collect()
}

/// Whether `signature` is `signing_key`'s Ed25519 signature over `message`, as Gateway checks it.
pub fn verify(signing_key: &[u8; KEY_LEN], message: &str, signature: &[u8]) -> bool {
    UnparsedPublicKey::new(&ED25519, signing_key)
        .verify(message.as_bytes(), signature)
        .is_ok()
}

/// Bytes as Gateway reads and stores them: base64url without padding. Keys, locked copies and signatures travel this
/// way, and so do sealed events.
pub fn b64u(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// What an approver signs for a new device: Gateway's `approval_message`,
/// `nexus-device-approval:v1:{deviceId}:{encryptionKey}:{signingKey}`, with both keys in base64url.
pub fn approval_message(
    device_id: &str,
    encryption_key: &[u8; KEY_LEN],
    signing_key: &[u8; KEY_LEN],
) -> Result<String, E2eError> {
    check_holder_id(device_id)?;
    Ok(format!(
        "nexus-device-approval:v1:{device_id}:{}:{}",
        b64u(encryption_key),
        b64u(signing_key)
    ))
}

/// A key holder's kind in the directory: a device, or one of a person's recovery keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HolderKind {
    Device,
    Passkey,
    RecoveryCode,
}

impl HolderKind {
    /// The kind as Gateway names it.
    pub fn as_str(self) -> &'static str {
        match self {
            HolderKind::Device => "device",
            HolderKind::Passkey => "passkey",
            HolderKind::RecoveryCode => "recovery_code",
        }
    }

    pub fn parse(text: &str) -> Option<HolderKind> {
        match text {
            "device" => Some(HolderKind::Device),
            "passkey" => Some(HolderKind::Passkey),
            "recovery_code" => Some(HolderKind::RecoveryCode),
            _ => None,
        }
    }
}

/// What an approver signs for a recovery key: Gateway's `holder_approval_message`,
/// `nexus-device-approval:v2:{holderId}:{kind}:{encryptionKey}:`, the key in base64url and the Ed25519 field empty. A
/// device's approval stays [`approval_message`] (v1), so neither signature stands for the other.
pub fn holder_approval_message(
    holder_id: &str,
    kind: HolderKind,
    encryption_key: &[u8; KEY_LEN],
) -> Result<String, E2eError> {
    check_holder_id(holder_id)?;
    if kind == HolderKind::Device {
        return Err(E2eError::Field);
    }
    Ok(format!(
        "nexus-device-approval:v2:{holder_id}:{}:{}:",
        kind.as_str(),
        b64u(encryption_key)
    ))
}

/// What the device that stores a locked copy signs: Gateway's `thread_key_message`,
/// `nexus-thread-key:v1:{threadId}:{keyVersion}:{deviceId}:{lockedKey}`, with the copy in base64url. Gateway's key
/// version is the epoch plus one ([`key_version`]).
pub fn thread_key_message(thread_id: &str, epoch: u64, device_id: &str, locked: &[u8]) -> Result<String, E2eError> {
    check_thread_id(thread_id)?;
    check_holder_id(device_id)?;
    let version = key_version(epoch)?;
    Ok(format!(
        "nexus-thread-key:v1:{thread_id}:{version}:{device_id}:{}",
        b64u(locked)
    ))
}

/// What the device that stores a team key's copy signs: Gateway's `team_key_message`,
/// `nexus-team-key:v1:{orgId}:{epoch}:{holderId}:{lockedKey}`, with the copy in base64url.
pub fn team_key_message(tenant_id: &str, epoch: i32, holder_id: &str, locked: &[u8]) -> Result<String, E2eError> {
    check_holder_id(tenant_id)?;
    check_holder_id(holder_id)?;
    Ok(format!(
        "nexus-team-key:v1:{tenant_id}:{epoch}:{holder_id}:{}",
        b64u(locked)
    ))
}

/// What the device that locks a thread's content key to the team key signs: Gateway's `team_thread_key_message`,
/// `nexus-team-thread-key:v1:{threadId}:{keyVersion}:{teamKeyId}:{lockedKey}`.
pub fn team_thread_key_message(
    thread_id: &str,
    epoch: u64,
    team_key_id: &str,
    locked: &[u8],
) -> Result<String, E2eError> {
    check_thread_id(thread_id)?;
    check_holder_id(team_key_id)?;
    let version = key_version(epoch)?;
    Ok(format!(
        "nexus-team-thread-key:v1:{thread_id}:{version}:{team_key_id}:{}",
        b64u(locked)
    ))
}

/// What the device that locks a thread's content key to the organization's recovery key signs: Gateway's
/// `org_thread_key_message`, `nexus-org-thread-key:v1:{threadId}:{keyVersion}:{orgKeyId}:{lockedKey}`.
pub fn org_thread_key_message(
    thread_id: &str,
    epoch: u64,
    org_key_id: &str,
    locked: &[u8],
) -> Result<String, E2eError> {
    check_thread_id(thread_id)?;
    check_holder_id(org_key_id)?;
    let version = key_version(epoch)?;
    Ok(format!(
        "nexus-org-thread-key:v1:{thread_id}:{version}:{org_key_id}:{}",
        b64u(locked)
    ))
}

/// What a revoker signs: Gateway's `revocation_message`, `nexus-device-revoke:v1:{deviceId}`.
pub fn revocation_message(device_id: &str) -> Result<String, E2eError> {
    check_holder_id(device_id)?;
    Ok(format!("nexus-device-revoke:v1:{device_id}"))
}

/// What an approver signs over the trusted list it hands the device it approves:
/// `nexus-device-handover:v1:{deviceId}:{approverId}:{payload}`, the payload in base64url. Gateway never reads it.
pub fn handover_message(device_id: &str, approver_id: &str, payload: &[u8]) -> Result<String, E2eError> {
    check_holder_id(device_id)?;
    check_holder_id(approver_id)?;
    Ok(format!(
        "nexus-device-handover:v1:{device_id}:{approver_id}:{}",
        b64u(payload)
    ))
}

/// The HPKE `info` of a handover: [`HANDOVER_LABEL`], the new device's id and the approver's id, length-prefixed.
pub fn handover_info(device_id: &str, approver_id: &str) -> Result<Vec<u8>, E2eError> {
    check_holder_id(device_id)?;
    check_holder_id(approver_id)?;
    Ok(length_prefixed(&[
        HANDOVER_LABEL.as_bytes(),
        device_id.as_bytes(),
        approver_id.as_bytes(),
    ]))
}

/// Seals an approver's `signature` over [`handover_message`] and the `payload` to the new device's X25519 key (HPKE base
/// mode, [`handover_info`], an empty associated data): `enc (32) || ciphertext (64 + payload) || tag (16)`. The
/// plaintext is the signature, then the payload.
pub fn seal_handover(
    device_id: &str,
    approver_id: &str,
    device_key: &[u8; KEY_LEN],
    signature: &[u8; SIGNATURE_LEN],
    payload: &[u8],
) -> Result<Vec<u8>, E2eError> {
    if payload.len() > MAX_HANDOVER_PAYLOAD {
        return Err(E2eError::Seal);
    }
    let info = handover_info(device_id, approver_id)?;
    let mut plaintext = Zeroizing::new(Vec::with_capacity(SIGNATURE_LEN + payload.len()));
    plaintext.extend_from_slice(signature);
    plaintext.extend_from_slice(payload);
    seal_bytes(&info, &plaintext, device_key, &mut os_rng())
}

/// A sealed handover's length for a payload of `payload` bytes.
fn handover_sealed_len(payload: usize) -> usize {
    KEY_LEN + SIGNATURE_LEN + payload + TAG_LEN
}

/// A handover this device opened ([`DeviceKey::open_handover`]): the approver's signature and the payload it signed.
/// Not trusted until [`OpenedHandover::signed_by`] says the approver this device trusts signed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedHandover {
    pub device_id: String,
    pub approver_id: String,
    pub signature: [u8; SIGNATURE_LEN],
    pub payload: Vec<u8>,
}

impl OpenedHandover {
    /// Whether `signing_key` signed this payload for this device and approver.
    pub fn signed_by(&self, signing_key: &[u8; KEY_LEN]) -> bool {
        handover_message(&self.device_id, &self.approver_id, &self.payload)
            .is_ok_and(|message| verify(signing_key, &message, &self.signature))
    }
}

/// Gateway's `keyVersion` for `epoch`: Gateway counts from 1, the epoch from 0.
pub fn key_version(epoch: u64) -> Result<i32, E2eError> {
    epoch
        .checked_add(1)
        .and_then(|version| i32::try_from(version).ok())
        .ok_or(E2eError::Field)
}

/// The epoch of Gateway's `keyVersion`, or `None` for a version below 1.
pub fn epoch_of(key_version: i32) -> Option<u64> {
    u64::try_from(key_version).ok()?.checked_sub(1)
}

/// An event's associated data.
fn event_aad(thread_id: &str, epoch: u64, seq: u64) -> Result<Vec<u8>, E2eError> {
    check_thread_id(thread_id)?;
    Ok(length_prefixed(&[
        EVENT_AAD_LABEL.as_bytes(),
        thread_id.as_bytes(),
        &epoch.to_be_bytes(),
        &seq.to_be_bytes(),
    ]))
}

/// The HPKE `info` for a recovery challenge: the label, the person id and the holder id, length-prefixed.
pub fn recover_challenge_info(person_id: &str, holder_id: &str) -> Result<Vec<u8>, E2eError> {
    check_person_id(person_id)?;
    check_holder_id(holder_id)?;
    Ok(length_prefixed(&[
        RECOVER_CHALLENGE_LABEL.as_bytes(),
        person_id.as_bytes(),
        holder_id.as_bytes(),
    ]))
}

/// The HPKE `info` for a locked copy.
fn lock_info(thread_id: &str, epoch: u64, holder_id: &str) -> Result<Vec<u8>, E2eError> {
    check_thread_id(thread_id)?;
    check_holder_id(holder_id)?;
    Ok(length_prefixed(&[
        LOCK_INFO_LABEL.as_bytes(),
        thread_id.as_bytes(),
        &epoch.to_be_bytes(),
        holder_id.as_bytes(),
    ]))
}

/// The HPKE `info` for a team key's copy: the label, the organization id, the epoch (from 1, 8 big-endian bytes) and
/// the holder id, length-prefixed.
fn team_lock_info(tenant_id: &str, epoch: i32, holder_id: &str) -> Result<Vec<u8>, E2eError> {
    check_holder_id(tenant_id)?;
    check_holder_id(holder_id)?;
    let epoch = u64::try_from(epoch)
        .ok()
        .filter(|epoch| *epoch >= 1)
        .ok_or(E2eError::Field)?;
    Ok(length_prefixed(&[
        TEAM_KEY_LOCK_LABEL.as_bytes(),
        tenant_id.as_bytes(),
        &epoch.to_be_bytes(),
        holder_id.as_bytes(),
    ]))
}

/// Each field as its length in 2 big-endian bytes, then its bytes. Every caller's fields are checked to be short.
fn length_prefixed(fields: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(fields.iter().map(|field| 2 + field.len()).sum());
    for field in fields {
        let len = u16::try_from(field.len()).expect("a bound field is under 64 KiB");
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(field);
    }
    out
}

/// A thread id in its one form: a lowercase hyphenated UUID, so one thread never has two encodings.
fn check_thread_id(id: &str) -> Result<(), E2eError> {
    match uuid::Uuid::try_parse(id) {
        Ok(uuid) if uuid.hyphenated().to_string() == id => Ok(()),
        _ => Err(E2eError::Field),
    }
}

/// A person id a derivation binds: 1 to 128 bytes.
fn check_person_id(id: &str) -> Result<(), E2eError> {
    if (1..=MAX_PERSON_ID_LEN).contains(&id.len()) {
        Ok(())
    } else {
        Err(E2eError::Field)
    }
}

/// A holder id as Gateway mints it (`dev_…`): 1 to 128 of `A-Z a-z 0-9 _ -`, so it holds no `:`.
fn check_holder_id(id: &str) -> Result<(), E2eError> {
    let fits = (1..=MAX_HOLDER_ID_LEN).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if fits { Ok(()) } else { Err(E2eError::Field) }
}

#[cfg(test)]
mod tests;
