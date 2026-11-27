//! What the switcher says of an account's Assistant, from the agent API's answers.
use super::{Org, Peek, Reply, age_text, key_of, peek_of, state_of};
use serde_json::{Value, json};

fn reply(status: u16, body: Value) -> Reply {
    Reply {
        status,
        body: Some(body),
    }
}

fn errand(status: &str, archived: bool) -> Value {
    json!({ "id": "i1", "goal": "Renew the lease", "status": status, "archived": archived, "pinned": false,
        "at": "2026-11-17T09:00:00Z" })
}

#[test]
fn an_age_reads_in_minutes_then_hours() {
    assert_eq!(age_text(0), "0 min ago");
    assert_eq!(age_text(59 * 60_000), "59 min ago");
    assert_eq!(age_text(3 * 60 * 60_000 + 59_000), "3 h ago");
}

#[test]
fn an_errand_that_waits_on_the_person_says_needs_you() {
    let waiting = reply(200, json!({ "errands": [errand("needs-you", false)] }));
    let paused = reply(200, json!({ "paused": false }));
    let peek = peek_of(Some(&waiting), Some(&paused), 2, 10).expect("a state");
    assert_eq!(
        peek,
        Peek {
            needs_you: true,
            unread: 2,
            paused: false,
            off: false,
            at: 10
        }
    );
    // An archived one does not.
    let archived = reply(200, json!({ "errands": [errand("needs-you", true)] }));
    assert!(
        !peek_of(Some(&archived), Some(&paused), 0, 10)
            .expect("a state")
            .needs_you
    );
}

#[test]
fn a_pause_and_a_missing_agent_api_read_as_paused_and_not_turned_on() {
    let none = reply(200, json!({ "errands": [] }));
    let paused = reply(200, json!({ "paused": true, "pausedAt": "2026-11-17T14:02:00Z" }));
    assert!(peek_of(Some(&none), Some(&paused), 0, 1).expect("a state").paused);
    let off = reply(404, json!({ "error": "not_found" }));
    let peek = peek_of(Some(&off), Some(&off), 0, 1).expect("a state");
    assert!(peek.off && !peek.needs_you && !peek.paused);
}

#[test]
fn an_account_that_did_not_answer_keeps_its_last_state() {
    assert_eq!(peek_of(None, None, 0, 1), None);
    // One answer is enough: the other read failed.
    let none = reply(200, json!({ "errands": [] }));
    assert!(peek_of(Some(&none), None, 0, 1).is_some());
}

#[test]
fn a_switcher_row_says_nothing_at_rest() {
    let palette = crate::palette::Palette::expected_at_launch();
    assert!(state_of(&Peek::default(), palette).is_none());
    assert!(
        state_of(
            &Peek {
                unread: 3,
                ..Peek::default()
            },
            palette
        )
        .is_some()
    );
    assert!(
        state_of(
            &Peek {
                paused: true,
                ..Peek::default()
            },
            palette
        )
        .is_some()
    );
    assert!(
        state_of(
            &Peek {
                off: true,
                ..Peek::default()
            },
            palette
        )
        .is_some()
    );
}

#[test]
fn an_account_is_known_by_its_slug_else_its_id() {
    let org = |slug: Option<&str>| Org {
        id: "o1".into(),
        slug: slug.map(String::from),
        name: "Acme".into(),
        personal: false,
        current: false,
        logo: None,
        role: None,
        provider_org_id: Some("org_provider".into()),
    };
    assert_eq!(key_of(&org(Some("acme"))), "acme");
    assert_eq!(key_of(&org(None)), "o1");
}
