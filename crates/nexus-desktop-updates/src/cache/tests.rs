use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const BYTES: &[u8] = include_bytes!("../../fixtures/update.txt");
const KEY: &str = include_str!("../../fixtures/test.pub");
const SIGNATURE: &str = include_str!("../../fixtures/update.txt.sig");

struct Scratch(Cache);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(Cache::new(std::env::temp_dir().join(format!(
            "nexus-update-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ))))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0.directory);
    }
}

fn artifact() -> Artifact {
    Artifact {
        version: "1.0.0".into(),
        target: "test-platform".into(),
        url: "https://downloads.example.test/update".into(),
        signature: SIGNATURE.trim().into(),
    }
}

#[test]
fn a_completed_download_survives_a_new_cache_instance() {
    let scratch = Scratch::new();
    assert!(scratch.0.load(&artifact(), KEY).is_none());
    scratch.0.save(&artifact(), BYTES).expect("save verified download");
    let restarted = Cache::new(scratch.0.directory.clone());
    assert_eq!(restarted.load(&artifact(), KEY).as_deref(), Some(BYTES));
    assert!(!scratch.0.directory.join("artifact.part").exists());
    assert!(!scratch.0.directory.join("release.json.part").exists());
}

#[test]
fn corrupted_bytes_are_discarded_before_reuse() {
    let scratch = Scratch::new();
    scratch.0.save(&artifact(), BYTES).expect("save");
    fs::write(scratch.0.directory.join("artifact"), b"tampered update").expect("tamper");
    assert!(scratch.0.load(&artifact(), KEY).is_none());
    assert!(!scratch.0.directory.join("artifact").exists());
    assert!(!scratch.0.directory.join("release.json").exists());
}

#[test]
fn a_saved_signature_cannot_relabel_an_old_artifact() {
    let scratch = Scratch::new();
    let mut relabeled = artifact();
    relabeled.version = "2.0.0".into();
    scratch.0.save(&relabeled, BYTES).expect("save relabeled metadata");
    assert!(scratch.0.load(&relabeled, KEY).is_none());
}

#[test]
fn a_changed_trusted_comment_fails_the_global_signature_check() {
    let scratch = Scratch::new();
    let mut relabeled = artifact();
    let decoded = String::from_utf8(STANDARD.decode(SIGNATURE.trim()).expect("fixture signature")).expect("text");
    relabeled.signature = STANDARD.encode(decoded.replace("version:1.0.0", "version:2.0.0"));
    relabeled.version = "2.0.0".into();
    scratch.0.save(&relabeled, BYTES).expect("save altered signature");
    assert!(scratch.0.load(&relabeled, KEY).is_none());
}

#[test]
fn a_trusted_comment_without_a_version_is_refused() {
    assert!(version_matches("timestamp:123\tfile:update.tar.gz", "1.0.0").is_none());
    assert!(version_matches("timestamp:123\tversion:", "1.0.0").is_none());
    assert!(version_matches("timestamp:123\tversion:1.0.0", "1.0.0").is_some());
}

#[test]
fn malformed_metadata_or_a_missing_artifact_starts_a_fresh_download() {
    let scratch = Scratch::new();
    scratch.0.save(&artifact(), BYTES).expect("save");
    fs::write(scratch.0.directory.join("release.json"), b"partial JSON").expect("partial metadata");
    assert!(scratch.0.load(&artifact(), KEY).is_none());
    scratch.0.save(&artifact(), BYTES).expect("save again");
    fs::remove_file(scratch.0.directory.join("artifact")).expect("missing artifact");
    assert!(scratch.0.load(&artifact(), KEY).is_none());
}

#[test]
fn a_different_release_target_url_signature_or_key_cannot_reuse_the_cache() {
    let scratch = Scratch::new();
    for field in ["version", "target", "url", "signature", "key"] {
        scratch.0.save(&artifact(), BYTES).expect("save");
        let mut expected = artifact();
        let mut key = KEY;
        match field {
            "version" => expected.version = "1.0.1".into(),
            "target" => expected.target = "another-platform".into(),
            "url" => expected.url.push_str("-new"),
            "signature" => expected.signature = "invalid".into(),
            "key" => key = "invalid",
            _ => unreachable!(),
        }
        assert!(scratch.0.load(&expected, key).is_none(), "{field}");
    }
}

#[test]
fn interrupted_or_oversized_writes_are_not_reused() {
    let scratch = Scratch::new();
    scratch.0.save(&artifact(), BYTES).expect("save");
    fs::remove_file(scratch.0.directory.join("release.json")).expect("interrupt metadata publication");
    fs::write(scratch.0.directory.join("artifact.part"), b"partial").expect("partial artifact");
    assert!(scratch.0.load(&artifact(), KEY).is_none());
    assert!(!scratch.0.directory.join("artifact.part").exists());

    scratch.0.save(&artifact(), BYTES).expect("save again");
    fs::File::create(scratch.0.directory.join("artifact"))
        .expect("artifact")
        .set_len(MAX_ARTIFACT_BYTES + 1)
        .expect("oversized sparse file");
    assert!(scratch.0.load(&artifact(), KEY).is_none());

    scratch.0.save(&artifact(), BYTES).expect("save metadata again");
    fs::OpenOptions::new()
        .write(true)
        .open(scratch.0.directory.join("release.json"))
        .expect("metadata")
        .set_len(MAX_METADATA_BYTES + 1)
        .expect("oversized metadata");
    assert!(scratch.0.load(&artifact(), KEY).is_none());
}

#[test]
fn a_read_error_preserves_the_download_for_a_later_attempt() {
    let scratch = Scratch::new();
    scratch.0.save(&artifact(), BYTES).expect("save");
    let metadata = scratch.0.directory.join("release.json");
    fs::remove_file(&metadata).expect("replace metadata temporarily");
    fs::create_dir(&metadata).expect("cause a read error");
    assert!(scratch.0.load(&artifact(), KEY).is_none());
    assert!(scratch.0.directory.join("artifact").exists());
    fs::remove_dir(&metadata).expect("restore metadata");
    fs::write(metadata, serde_json::to_vec(&artifact()).expect("metadata")).expect("restore");
    assert_eq!(scratch.0.load(&artifact(), KEY).as_deref(), Some(BYTES));
}

#[test]
fn a_new_download_replaces_the_old_one_and_clear_removes_it() {
    let scratch = Scratch::new();
    scratch.0.save(&artifact(), BYTES).expect("save");
    let mut newer = artifact();
    newer.url.push_str("-new");
    scratch.0.save(&newer, BYTES).expect("replace");
    assert_eq!(scratch.0.load(&newer, KEY).as_deref(), Some(BYTES));
    assert_eq!(fs::read_dir(&scratch.0.directory).expect("directory").count(), 2);
    scratch.0.clear().expect("clear installed release");
    scratch.0.clear().expect("already cleared");
    assert_eq!(fs::read_dir(&scratch.0.directory).expect("empty directory").count(), 0);
}

#[test]
fn a_cache_write_failure_does_not_create_a_ready_artifact() {
    let scratch = Scratch::new();
    fs::write(&scratch.0.directory, b"not a directory").expect("block cache writes");
    assert!(scratch.0.save(&artifact(), BYTES).is_err());
    assert!(scratch.0.load(&artifact(), KEY).is_none());
    fs::remove_file(&scratch.0.directory).expect("cleanup");
}
