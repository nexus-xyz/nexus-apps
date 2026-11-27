use super::*;
fn attachment(name: &str) -> Attachment {
    crate::attachments::persisted_attachment(name.into(), None, Arc::new(name.as_bytes().to_vec()))
}
fn scope() -> ScopeStore {
    ScopeStore::new(
        Some(Arc::new(DiskStore::new(
            std::path::PathBuf::from("unused-test-path"),
            true,
        ))),
        true,
    )
}
#[test]
fn delayed_reads_never_replace_an_edit_or_clear() {
    let mut store = scope();
    let (key, version) = store.begin_read("a", false).expect("initial read");
    store.put(
        "a",
        ParkedDraft {
            text: "typed meanwhile".into(),
            ..Default::default()
        },
        false,
    );
    assert!(!store.restore(
        &key,
        version,
        Some(ParkedDraft {
            text: "old disk value".into(),
            ..Default::default()
        })
    ));
    assert_eq!(store.get("a").text, "typed meanwhile");
    store.put("a", ParkedDraft::default(), true);
    assert!(!store.restore(
        &key,
        version,
        Some(ParkedDraft {
            text: "resurrect".into(),
            ..Default::default()
        })
    ));
    assert!(store.get("a").is_empty());
}
#[test]
fn accepting_a_snapshot_keeps_later_text_and_attachments() {
    let sent_image = attachment("sent image");
    let later_image = attachment("later image");
    let sent_file = attachment("sent file");
    let later_file = attachment("later file");
    let submitted = ParkedDraft {
        text: "sent".into(),
        images: vec![sent_image.clone()],
        files: vec![sent_file.clone()],
        problem: None,
    };
    let current = ParkedDraft {
        text: "sent\n\nnext".into(),
        images: vec![sent_image, later_image.clone()],
        files: vec![sent_file, later_file.clone()],
        problem: None,
    };
    let left = after_send(current, &submitted);
    assert_eq!(left.text, "next");
    assert_eq!(left.images[0].id, later_image.id);
    assert_eq!(left.files[0].id, later_file.id);
    assert_eq!(left.images.len(), 1);
    assert_eq!(left.files.len(), 1);
}
#[test]
fn stale_preparation_tickets_do_not_restore_a_submitted_draft() {
    let mut store = scope();
    store.put(
        "a",
        ParkedDraft {
            text: "sent".into(),
            ..Default::default()
        },
        false,
    );
    let ticket = store.generation("a");
    store.put("a", ParkedDraft::default(), true);
    assert_ne!(store.generation("a"), ticket);
    assert!(store.get("a").is_empty());
}
#[test]
fn pending_session_rename_keeps_bytes_and_preparation_ownership() {
    let mut store = scope();
    let bytes = attachment("pending file");
    store.put(
        "draft-1",
        ParkedDraft {
            text: "follow-up".into(),
            files: vec![bytes.clone()],
            ..Default::default()
        },
        false,
    );
    let ticket = store.generation("draft-1");
    assert!(store.changes().is_empty(), "passing ids are not persisted");
    store.rename("draft-1", "thread-1");
    assert_eq!(store.get("thread-1").files[0].bytes.as_slice(), bytes.bytes.as_slice());
    assert_eq!(
        store.generation("draft-1"),
        ticket,
        "late preparation follows the renamed session"
    );
    let changes = store.changes();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].key, "thread-1");
    store.put("thread-1", ParkedDraft::default(), true);
    assert_ne!(store.generation("draft-1"), ticket);
}
#[test]
fn another_windows_newer_attachments_survive_a_local_text_edit() {
    let stale = attachment("removed elsewhere");
    let foreign = attachment("added elsewhere");
    let local = attachment("added here");
    let merged = merge_local(
        ParkedDraft {
            text: "other text".into(),
            images: vec![foreign.clone()],
            ..Default::default()
        },
        ParkedDraft {
            text: "typed here".into(),
            images: vec![stale.clone(), local.clone()],
            ..Default::default()
        },
        "before",
        &[(true, stale.id)],
    );
    assert_eq!(merged.text, "typed here");
    assert_eq!(
        merged.images.iter().map(|a| a.id).collect::<Vec<_>>(),
        [foreign.id, local.id]
    );
}
#[test]
fn old_write_acknowledgements_cannot_clear_newer_dirty_edits() {
    let mut store = scope();
    let old = store.put(
        "a",
        ParkedDraft {
            text: "old".into(),
            ..Default::default()
        },
        false,
    );
    let new = store.put(
        "a",
        ParkedDraft {
            text: "new".into(),
            ..Default::default()
        },
        false,
    );
    store.acknowledge(&[("a".into(), old)], true);
    assert!(store.slots["a"].dirty);
    store.acknowledge(&[("a".into(), new)], true);
    assert!(!store.slots["a"].dirty);
}
#[test]
fn disabling_policy_invalidates_a_pending_read_even_after_reenable() {
    let mut store = scope();
    let (key, revision) = store.begin_read("a", false).expect("read");
    store.enable(false);
    assert!(!store.can_restore(&key, revision));
    store.enable(true);
    assert!(!store.restore(
        &key,
        revision,
        Some(ParkedDraft {
            text: "stale".into(),
            ..Default::default()
        })
    ));
    assert!(store.begin_read("a", false).is_some());
}

#[test]
fn detached_first_answer_keeps_reused_home_edits_separate() {
    let mut store = ScopeStore::new(None, false);
    let old = ParkedDraft {
        text: "follow-up".into(),
        ..Default::default()
    };
    let revision = store.put("home", old.clone(), false);
    store.put(
        "home",
        ParkedDraft {
            text: "new conversation".into(),
            ..Default::default()
        },
        false,
    );
    store.promote_snapshot("home", "named-chat", revision, old);
    assert_eq!(store.get("home").text, "new conversation");
    assert_eq!(store.get("named-chat").text, "follow-up");
}
