use super::*;
use crate::model::Role;

fn message(text: &str) -> Message {
    Message {
        role: Role::User,
        content: text.into(),
        page: None,
        meta: Default::default(),
        web_sources: Vec::new(),
        stored: Default::default(),
    }
}

fn built(first: usize, texts: &[&str]) -> (Vec<Message>, Vec<Entry>) {
    let messages = texts.iter().map(|text| message(text)).collect();
    let entries = texts
        .iter()
        .enumerate()
        .map(|(at, text)| Entry::user(first + at, (*text).into()))
        .collect();
    (messages, entries)
}

#[test]
fn a_chat_left_unchanged_gives_back_its_entries_for_the_same_messages() {
    let mut kept = KeptChats::default();
    let (messages, mut entries) = built(1, &["a", "b"]);
    kept.shown("one", messages.clone(), &entries);
    kept.leave(Some("one"), &mut entries);
    assert!(entries.is_empty(), "the entries move into the keep");
    let back = kept.take("one", &messages).expect("kept");
    assert_eq!(back.iter().map(|entry| entry.id).collect::<Vec<_>>(), [1, 2]);
    assert!(kept.take("one", &messages).is_none(), "taken once");
}

#[test]
fn newer_messages_build_again() {
    let mut kept = KeptChats::default();
    let (messages, mut entries) = built(1, &["a"]);
    kept.shown("one", messages, &entries);
    kept.leave(Some("one"), &mut entries);
    let (newer, _) = built(9, &["a", "b"]);
    assert!(kept.take("one", &newer).is_none());
    assert!(kept.kept.is_empty(), "the stale copy goes");
}

#[test]
fn a_chat_changed_here_is_not_kept() {
    let mut kept = KeptChats::default();
    let (messages, mut entries) = built(1, &["a"]);
    kept.shown("one", messages.clone(), &entries);
    kept.mutated();
    kept.leave(Some("one"), &mut entries);
    assert_eq!(entries.len(), 1);
    // Rebuilt in place (new ids) or grown: not as built either.
    kept.shown("one", messages.clone(), &entries);
    entries.push(Entry::user(7, "c".into()));
    kept.leave(Some("one"), &mut entries);
    assert_eq!(entries.len(), 2);
    assert!(kept.take("one", &messages).is_none());
}

#[test]
fn only_the_last_chats_left_stay() {
    let mut kept = KeptChats::default();
    for at in 0..KEPT + 2 {
        let (messages, mut entries) = built(at * 10 + 1, &["a"]);
        let id = format!("chat-{at}");
        kept.shown(&id, messages, &entries);
        kept.leave(Some(&id), &mut entries);
    }
    assert_eq!(kept.kept.len(), KEPT);
    assert!(kept.take("chat-0", &[message("a")]).is_none());
    assert!(kept.take(&format!("chat-{}", KEPT + 1), &[message("a")]).is_some());
}

#[test]
fn a_fresh_read_confirms_the_shown_snapshot_without_new_entry_ids() {
    let mut kept = KeptChats::default();
    let (messages, entries) = built(1, &["a", "b"]);
    kept.shown("one", messages.clone(), &entries);
    assert!(kept.matches("one", &messages, &entries));
    assert!(!kept.matches("two", &messages, &entries));
    assert!(!kept.matches("one", &[message("changed")], &entries));
    let (_, rebuilt) = built(20, &["a", "b"]);
    assert!(!kept.matches("one", &messages, &rebuilt));
    kept.mutated();
    assert!(!kept.matches("one", &messages, &entries));
}

#[test]
fn streaming_and_arriving_entries_are_never_retained() {
    for arriving in [false, true] {
        let mut kept = KeptChats::default();
        let (messages, mut entries) = built(1, &["a"]);
        kept.shown("one", messages.clone(), &entries);
        entries[0].streaming = !arriving;
        entries[0].arriving = arriving;
        assert!(!kept.matches("one", &messages, &entries));
        kept.leave(Some("one"), &mut entries);
        assert!(kept.take("one", &messages).is_none());
        assert_eq!(entries.len(), 1);
    }
}

#[test]
fn leaving_the_organization_drops_shown_and_retained_snapshots() {
    let mut kept = KeptChats::default();
    let (messages, mut entries) = built(1, &["a"]);
    kept.shown("one", messages.clone(), &entries);
    kept.leave(Some("one"), &mut entries);
    let (_, shown) = built(10, &["a"]);
    kept.shown("two", messages.clone(), &shown);
    kept.clear();
    assert!(!kept.matches("two", &messages, &shown));
    assert!(kept.take("one", &messages).is_none());
}
