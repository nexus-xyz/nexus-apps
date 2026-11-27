use super::*;
use crate::attachments::persisted_attachment;
struct Temporary(std::path::PathBuf);
impl Temporary {
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn temporary() -> Temporary {
    let path = std::env::temp_dir().join(format!("nexus-draft-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).expect("isolated test directory");
    Temporary(path)
}
fn file(name: &str, bytes: &[u8]) -> Attachment {
    persisted_attachment(name.into(), None, Arc::new(bytes.to_vec()))
}
fn png() -> Attachment {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 2)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("test PNG");
    persisted_attachment("shot.png".into(), Some("image/png"), Arc::new(bytes.into_inner()))
}
fn change(key: &str, revision: u64, draft: ParkedDraft) -> Change {
    Change {
        key: key.into(),
        revision,
        draft,
    }
}
#[test]
fn complete_drafts_survive_restart_without_the_original_files() {
    let temp = temporary();
    let root = temp.path().join("scope");
    let original = temp.path().join("original.bin");
    std::fs::write(&original, [0, 255, 128, 1]).expect("source file");
    let image = png();
    let image_bytes = image.bytes.clone();
    let attachment = file("../original.bin", &std::fs::read(&original).expect("source bytes"));
    let previous_id = attachment.id;
    let store = DiskStore::new(root.clone(), true);
    store
        .write(vec![change(
            "session",
            1,
            ParkedDraft {
                text: "keep my prompt".into(),
                images: vec![image],
                files: vec![attachment],
                problem: None,
            },
        )])
        .expect("save complete draft");
    std::fs::remove_file(original).expect("source disappears");
    drop(store);
    let reopened = DiskStore::new(root, true)
        .read("session")
        .expect("read disk")
        .expect("saved draft");
    assert_eq!(reopened.text, "keep my prompt");
    assert_eq!(reopened.images[0].bytes.as_slice(), image_bytes.as_slice());
    assert_eq!(reopened.images[0].media_type, Some("image/png"));
    assert!(reopened.images[0].thumbnail.is_some());
    assert_eq!(reopened.files[0].bytes.as_slice(), [0, 255, 128, 1]);
    assert_eq!(reopened.files[0].name, "../original.bin");
    assert_ne!(
        reopened.files[0].id, previous_id,
        "restored ids cannot collide with new attachments"
    );
    assert!(reopened.problem.is_none());
}
#[test]
fn disabled_policy_performs_no_disk_reads_or_writes() {
    let temp = temporary();
    let root = temp.path().join("never-created");
    let store = DiskStore::new(root.clone(), false);
    let draft = ParkedDraft {
        files: vec![file("secret.bin", b"private bytes")],
        ..Default::default()
    };
    assert!(
        store
            .write(vec![change("a", 1, draft.clone())])
            .expect("disabled write")
            .is_empty()
    );
    assert!(store.read("a").expect("disabled read").is_none());
    assert!(!root.exists());
    store.enable(true);
    store
        .write(vec![change("a", 2, draft)])
        .expect("policy permits persistence");
    store.enable(false);
    assert!(store.read("a").expect("cached bytes remain inaccessible").is_none());
    store
        .write(vec![change("a", 3, ParkedDraft::default())])
        .expect("disabled clear");
    store.enable(true);
    assert!(store.read("a").expect("original cache unchanged").is_some());
}
#[test]
fn late_saves_cannot_overwrite_edits_or_resurrect_cleared_drafts() {
    let temp = temporary();
    let store = DiskStore::new(temp.path().join("scope"), true);
    let text = |text: &str| ParkedDraft {
        text: text.into(),
        ..Default::default()
    };
    store
        .write(vec![change("a", 2, text("new")), change("b", 1, text("other chat"))])
        .expect("new saves");
    store
        .write(vec![change("a", 1, text("old"))])
        .expect("late save ignored");
    assert_eq!(store.read("a").expect("read").expect("a").text, "new");
    assert_eq!(store.read("b").expect("read").expect("b").text, "other chat");
    store
        .write(vec![change("a", 3, ParkedDraft::default())])
        .expect("clear");
    store
        .write(vec![change("a", 2, text("resurrect"))])
        .expect("stale write");
    assert!(store.read("a").expect("cleared").is_none());
}
#[test]
fn shared_blobs_live_until_their_last_draft_is_cleared() {
    let temp = temporary();
    let root = temp.path().join("scope");
    let store = DiskStore::new(root.clone(), true);
    let draft = ParkedDraft {
        files: vec![file("one.bin", b"same bytes")],
        ..Default::default()
    };
    store
        .write(vec![change("a", 1, draft.clone()), change("b", 1, draft)])
        .expect("shared content");
    let count = || {
        std::fs::read_dir(root.join("attachments"))
            .expect("blob directory")
            .count()
    };
    assert_eq!(count(), 1);
    store
        .write(vec![change("a", 2, ParkedDraft::default())])
        .expect("clear first");
    assert_eq!(count(), 1);
    assert_eq!(
        store.read("b").expect("read second").expect("second draft").files[0]
            .bytes
            .as_slice(),
        b"same bytes"
    );
    store
        .write(vec![change("b", 2, ParkedDraft::default())])
        .expect("clear last");
    assert_eq!(count(), 0);
}
#[test]
fn missing_or_invalid_attachment_references_keep_text_and_report_the_problem() {
    let temp = temporary();
    let root = temp.path().join("scope");
    let store = DiskStore::new(root.clone(), true);
    store
        .write(vec![change(
            "a",
            1,
            ParkedDraft {
                text: "keep text".into(),
                files: vec![file("file.bin", b"bytes")],
                ..Default::default()
            },
        )])
        .expect("save");
    let index = root.join("drafts.json");
    let mut json: serde_json::Value = serde_json::from_slice(&std::fs::read(&index).expect("index")).expect("JSON");
    json["drafts"]["a"]["files"][0]["blob"] = serde_json::json!("../outside");
    std::fs::write(&index, serde_json::to_vec(&json).expect("JSON")).expect("tampered test index");
    let restored = store.read("a").expect("text survives").expect("draft");
    assert_eq!(restored.text, "keep text");
    assert!(restored.files.is_empty());
    assert!(restored.problem.is_some());
    assert!(!blob_name("../outside"));
    assert!(!blob_name(&"A".repeat(64)));
}
#[test]
fn identity_stores_never_read_another_identitys_draft() {
    let temp = temporary();
    let scope = |marker| {
        nexus_workbench::composer_drafts::storage_scope("https://gateway.test", "org", Some(marker)).expect("identity")
    };
    let a = DiskStore::new(temp.path().join(scope("alice")), true);
    let b = DiskStore::new(temp.path().join(scope("bob")), true);
    a.write(vec![change(
        "same-session-id",
        1,
        ParkedDraft {
            text: "Alice's draft".into(),
            files: vec![file("private.bin", b"alice")],
            ..Default::default()
        },
    )])
    .expect("save Alice");
    assert!(b.read("same-session-id").expect("Bob has no draft").is_none());
    assert_eq!(
        a.read("same-session-id")
            .expect("Alice read")
            .expect("Alice draft")
            .files[0]
            .bytes
            .as_slice(),
        b"alice"
    );
}
#[cfg(unix)]
#[test]
fn indexes_and_attachment_bytes_are_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let temp = temporary();
    let root = temp.path().join("scope");
    let store = DiskStore::new(root.clone(), true);
    store
        .write(vec![change(
            "a",
            1,
            ParkedDraft {
                files: vec![file("secret.bin", b"bytes")],
                ..Default::default()
            },
        )])
        .expect("save");
    for file in [
        root.join("drafts.json"),
        root.join("attachments").join(digest(b"bytes")),
    ] {
        assert_eq!(
            std::fs::metadata(file).expect("saved file").permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(
        std::fs::metadata(root).expect("scope directory").permissions().mode() & 0o777,
        0o700
    );
}
