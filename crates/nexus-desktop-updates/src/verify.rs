//! The update file's minisign signature, in the updater's format (`<file>.sig`): the base64 of a minisign signature
//! file. Its trusted comment carries `version:<x.y.z>` (`--app-version`), so a manifest cannot pair a new version
//! number with an older file. The cache and Nexus GPUI check downloads here.
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use semver::Version;

use crate::Failure;

/// Whether `bytes` are the file `pubkey`'s owner signed for `version`. Both `pubkey` and `signature` are base64 of
/// a minisign key file and a minisign signature file.
pub fn verify(bytes: &[u8], signature: &str, pubkey: &str, version: &str) -> Result<(), Failure> {
    let decode = |value: &str| String::from_utf8(STANDARD.decode(value.trim()).ok()?).ok();
    let key = decode(pubkey)
        .and_then(|key| PublicKey::decode(&key).ok())
        .ok_or_else(|| Failure::new("missing_key", "This build's update verification key is invalid.", false))?;
    let signature = decode(signature)
        .and_then(|signature| Signature::decode(&signature).ok())
        .ok_or_else(failed)?;
    key.verify(bytes, &signature, true).map_err(|_| failed())?;
    // The global signature authenticates the trusted comment. Read the version only after verification.
    version_matches(signature.trusted_comment(), version).ok_or_else(failed)
}

/// Whether `signature` (a `.sig` in the updater's format) signs `bytes` with `pubkey`, and its trusted comment names
/// this `version` and this `file`. The file's name keeps another file signed for the same version from passing as it.
/// The Nexus host (nexus-remote's install.rs) is checked this way.
pub fn verify_signed_file(pubkey: &str, signature: &str, version: &str, file: &str, bytes: &[u8]) -> bool {
    let decode = |value: &str| String::from_utf8(STANDARD.decode(value.trim()).ok()?).ok();
    let (Some(key), Some(signature)) = (
        decode(pubkey).and_then(|key| PublicKey::decode(&key).ok()),
        decode(signature).and_then(|signature| Signature::decode(&signature).ok()),
    ) else {
        return false;
    };
    if key.verify(bytes, &signature, true).is_err() {
        return false;
    }
    // The global signature authenticates the trusted comment. Read it only after verification.
    let comment = signature.trusted_comment();
    version_matches(comment, version).is_some()
        && comment
            .split('\t')
            .any(|field| field.strip_prefix("file:") == Some(file))
}

/// The failure for an update that does not verify.
fn failed() -> Failure {
    Failure::new(
        "verification_failed",
        "The update could not be verified. Nothing was installed. Contact support or check again later.",
        false,
    )
}

pub(crate) fn version_matches(trusted_comment: &str, announced: &str) -> Option<()> {
    let signed = trusted_comment
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"))?;
    let signed = Version::parse(signed.trim_start_matches('v')).ok()?;
    let expected = Version::parse(announced).ok()?;
    (signed == expected).then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BYTES: &[u8] = include_bytes!("../fixtures/update.txt");
    const KEY: &str = include_str!("../fixtures/test.pub");
    const SIGNATURE: &str = include_str!("../fixtures/update.txt.sig");

    #[test]
    fn accepts_only_the_signed_bytes_and_version() {
        assert_eq!(verify(BYTES, SIGNATURE, KEY, "1.0.0"), Ok(()));
        let tampered = verify(b"tampered update", SIGNATURE, KEY, "1.0.0").expect_err("tampered");
        assert_eq!(tampered.code, "verification_failed");
        let relabeled = verify(BYTES, SIGNATURE, KEY, "2.0.0").expect_err("relabeled");
        assert_eq!(relabeled.code, "verification_failed");
        assert_eq!(
            verify(BYTES, "not base64", KEY, "1.0.0").expect_err("garbage").code,
            "verification_failed"
        );
        assert_eq!(
            verify(BYTES, SIGNATURE, "", "1.0.0").expect_err("no key").code,
            "missing_key"
        );
    }

    #[test]
    fn a_signed_file_must_match_its_key_version_and_name() {
        let check = |key, signature, version, file, bytes| verify_signed_file(key, signature, version, file, bytes);
        assert!(check(KEY, SIGNATURE, "1.0.0", "update.txt", BYTES));
        assert!(!check(KEY, SIGNATURE, "1.0.1", "update.txt", BYTES));
        assert!(!check(KEY, SIGNATURE, "1.0.0", "another.txt", BYTES));
        assert!(!check(KEY, SIGNATURE, "1.0.0", "update.txt", b"tampered"));
        assert!(!check(KEY, "", "1.0.0", "update.txt", BYTES));
        assert!(!check("", SIGNATURE, "1.0.0", "update.txt", BYTES));
    }
}
