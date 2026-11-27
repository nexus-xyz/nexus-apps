//! The update file's minisign signature, in the updater's format (`<file>.sig`): the base64 of a minisign signature
//! file. Its trusted comment carries `version:<x.y.z>` (`--app-version`), so a manifest cannot pair a new version
//! number with an older file. The cache and Nexus GPUI check downloads here.
//!
//! The key is a list: the base64 of one or more minisign key files, separated by whitespace. A release build lists the
//! active key, then the next key, whose secret half stays offline. A rotation signs with the next key, so builds that
//! trust it update without a gap.
use base64::{Engine, engine::general_purpose::STANDARD};
use minisign_verify::{PublicKey, Signature};
use semver::Version;

use crate::Failure;

/// Whether `bytes` are the file one of `pubkey`'s owners signed for `version`. `pubkey` lists the base64 of one or
/// more minisign key files; `signature` is the base64 of a minisign signature file.
pub fn verify(bytes: &[u8], signature: &str, pubkey: &str, version: &str) -> Result<(), Failure> {
    let keys = keys(pubkey)
        .ok_or_else(|| Failure::new("missing_key", "This build's update verification key is invalid.", false))?;
    let signature = decode(signature)
        .and_then(|signature| Signature::decode(&signature).ok())
        .ok_or_else(failed)?;
    if !keys.iter().any(|key| key.verify(bytes, &signature, true).is_ok()) {
        return Err(failed());
    }
    // The global signature authenticates the trusted comment. Read the version only after verification.
    version_matches(signature.trusted_comment(), version).ok_or_else(failed)
}

/// Whether `signature` (a `.sig` in the updater's format) signs `bytes` with one of `pubkey`'s keys, and its trusted
/// comment names this `version` and this `file`. The file's name keeps another file signed for the same version from
/// passing as it. The Nexus host (nexus-remote's install.rs) is checked this way.
pub fn verify_signed_file(pubkey: &str, signature: &str, version: &str, file: &str, bytes: &[u8]) -> bool {
    let (Some(keys), Some(signature)) = (
        keys(pubkey),
        decode(signature).and_then(|signature| Signature::decode(&signature).ok()),
    ) else {
        return false;
    };
    if !keys.iter().any(|key| key.verify(bytes, &signature, true).is_ok()) {
        return false;
    }
    // The global signature authenticates the trusted comment. Read it only after verification.
    let comment = signature.trusted_comment();
    version_matches(comment, version).is_some()
        && comment
            .split('\t')
            .any(|field| field.strip_prefix("file:") == Some(file))
}

/// The keys `pubkey` lists, or None when it lists none or any of them is invalid: a build with a broken key must
/// say so, not quietly trust fewer keys.
fn keys(pubkey: &str) -> Option<Vec<PublicKey>> {
    let keys = pubkey
        .split_whitespace()
        .map(|key| decode(key).and_then(|key| PublicKey::decode(&key).ok()))
        .collect::<Option<Vec<_>>>()?;
    (!keys.is_empty()).then_some(keys)
}

fn decode(value: &str) -> Option<String> {
    String::from_utf8(STANDARD.decode(value.trim()).ok()?).ok()
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
    /// A second disposable key, as a release's next key. Its secret half was not kept.
    const NEXT: &str = include_str!("../fixtures/next.pub");
    const NEXT_SIGNATURE: &str = include_str!("../fixtures/update.txt.next.sig");

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

    #[test]
    fn either_listed_key_verifies() {
        let both = format!("{KEY} {NEXT}");
        let next_first = format!("{NEXT}\n{KEY}");
        for keys in [both.as_str(), next_first.as_str()] {
            assert_eq!(verify(BYTES, SIGNATURE, keys, "1.0.0"), Ok(()));
            assert_eq!(verify(BYTES, NEXT_SIGNATURE, keys, "1.0.0"), Ok(()));
            assert!(verify_signed_file(keys, NEXT_SIGNATURE, "1.0.0", "update.txt", BYTES));
            let tampered = verify(b"tampered", NEXT_SIGNATURE, keys, "1.0.0").expect_err("tampered");
            assert_eq!(tampered.code, "verification_failed");
        }
        // A key that is not listed is not trusted.
        let unlisted = verify(BYTES, NEXT_SIGNATURE, KEY, "1.0.0").expect_err("unlisted");
        assert_eq!(unlisted.code, "verification_failed");
        assert!(!verify_signed_file(KEY, NEXT_SIGNATURE, "1.0.0", "update.txt", BYTES));
    }

    #[test]
    fn one_invalid_key_disables_the_list() {
        let broken = format!("{KEY} not-a-key");
        assert_eq!(
            verify(BYTES, SIGNATURE, &broken, "1.0.0").expect_err("broken").code,
            "missing_key"
        );
        assert_eq!(
            verify(BYTES, SIGNATURE, " \n ", "1.0.0").expect_err("blank").code,
            "missing_key"
        );
        assert!(!verify_signed_file(&broken, SIGNATURE, "1.0.0", "update.txt", BYTES));
    }
}
