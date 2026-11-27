use bytes::Bytes;
use futures::executor::block_on;
use nexus_client::CoreError;
use serde_json::json;

use super::*;

fn gateway_error(status: u16, code: Option<&str>) -> Error {
    CoreError {
        status,
        message: String::new(),
        code: code.map(String::from),
        interval: None,
        details: None,
    }
    .into()
}

fn rounds(used: u32, limit: u32) -> Rounds {
    Rounds { used, limit }
}

fn loaded(used: u32, limit: u32) -> StatusRead {
    Ok(Some((rounds(used, limit), "DeepSeek V4 Flash 0731".into())))
}

fn sse(events: &[Value]) -> Vec<nexus_client::Result<Bytes>> {
    events
        .iter()
        .map(|event| Ok(Bytes::from(format!("data: {event}\n\n"))))
        .collect()
}

fn event_stream(rounds: Option<(&str, &str)>) -> Vec<(String, String)> {
    let mut headers = vec![("content-type".to_string(), "text/event-stream".to_string())];
    if let Some((used, limit)) = rounds {
        headers.push((ROUNDS_USED_HEADER.to_string(), used.to_string()));
        headers.push((ROUNDS_LIMIT_HEADER.to_string(), limit.to_string()));
    }
    headers
}

fn round(status: u16, headers: &[(String, String)], body: Vec<nexus_client::Result<Bytes>>) -> (RoundRead, String) {
    let mut text = String::new();
    let read = block_on(read_round(status, headers, futures::stream::iter(body), |piece| {
        text.push_str(&piece)
    }));
    (read, text)
}

#[test]
fn guest_chat_is_on_only_when_the_switch_says_on() {
    assert!(flag_on(Some("on")));
    assert!(!flag_on(None));
    assert!(!flag_on(Some("off")));
    assert!(!flag_on(Some("1")));
}

#[test]
fn gateway_refusals_read_as_nexus_ui_reads_them() {
    let cases = [
        (503, Some("guest_chat_not_configured"), Refusal::Off),
        (404, None, Refusal::Off),
        (429, Some("guest_rounds_used"), Refusal::RoundsUsed),
        (429, Some("guest_address_limit"), Refusal::AddressLimit),
        (429, Some("guest_chat_paused"), Refusal::Paused),
        (400, Some("turnstile_failed"), Refusal::Turnstile),
        (400, Some("guest_request_invalid"), Refusal::Invalid),
        (429, Some("rate_limited"), Refusal::Failed),
        (502, Some("turnstile_unavailable"), Refusal::Failed),
    ];
    for (status, code, reason) in cases {
        assert_eq!(refusal(&gateway_error(status, code)), reason, "{status} {code:?}");
    }
}

#[test]
fn the_status_reads_the_rounds_the_address_limit_and_a_gateway_without_guest_chat() {
    let body =
        |enabled, used| json!({ "enabled": enabled, "rounds_used": used, "rounds_limit": 5, "model": "DeepSeek" });
    assert_eq!(
        read_status(Ok(body(true, 2))),
        Ok(Some((rounds(2, 5), "DeepSeek".into())))
    );
    assert_eq!(
        read_status(Ok(body(false, 5))),
        Ok(Some((rounds(5, 5), "DeepSeek".into())))
    );
    assert_eq!(read_status(Ok(body(false, 1))), Err(Refusal::AddressLimit));
    assert_eq!(read_status(Err(gateway_error(404, None))), Ok(None));
    assert_eq!(
        read_status(Err(gateway_error(503, Some("guest_chat_not_configured")))),
        Ok(None)
    );
    assert_eq!(read_status(Ok(json!({}))), Err(Refusal::Failed));
}

#[test]
fn a_round_streams_its_answer_and_reads_the_rounds_after_it() {
    let body = sse(&[
        json!({ "choices": [{ "delta": { "content": "Hel" } }] }),
        json!({ "choices": [{ "delta": { "content": "lo" } }] }),
        json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
    ]);
    let (read, text) = round(200, &event_stream(Some(("3", "5"))), body);
    assert_eq!(read, Ok(Some(rounds(3, 5))));
    assert_eq!(text, "Hello");
}

#[test]
fn a_round_without_its_last_chunk_or_with_an_error_event_fails() {
    let cut = sse(&[json!({ "choices": [{ "delta": { "content": "Hel" } }] })]);
    assert_eq!(round(200, &event_stream(None), cut).0, Err(Refusal::Failed));
    let broken = sse(&[json!({ "error": { "message": "provider down", "code": "upstream_error" } })]);
    assert_eq!(round(200, &event_stream(None), broken).0, Err(Refusal::Failed));
}

#[test]
fn a_refused_round_names_its_refusal() {
    let headers = vec![("content-type".to_string(), "application/json".to_string())];
    let body = vec![Ok(Bytes::from(
        json!({ "error": { "message": "used", "code": "guest_rounds_used" } }).to_string(),
    ))];
    assert_eq!(round(429, &headers, body).0, Err(Refusal::RoundsUsed));
}

#[test]
fn the_guest_id_is_made_once_and_kept() {
    let dir = crate::paths::test_root().join("guest-id-kept");
    std::fs::remove_dir_all(&dir).ok();
    let file = dir.join("guest-id");
    let made = kept_id(Some(file.clone()));
    assert!(uuid::Uuid::parse_str(&made).is_ok());
    assert_eq!(kept_id(Some(file.clone())), made);
    std::fs::write(&file, "not a uuid").unwrap();
    assert_ne!(kept_id(Some(file)), made);
}

#[test]
fn the_chat_counts_its_rounds_and_ends_on_the_sign_in_card_at_the_limit() {
    let mut chat = Chat::new(true);
    assert_eq!(chat.ask("hi"), None, "no round before the status");
    chat.status_read(loaded(3, 5));
    assert_eq!(
        chat.note().as_deref(),
        Some("You are signed out. You have 5 rounds on DeepSeek V4 Flash 0731, and this chat is not saved.")
    );

    let sent = chat.ask("  first  ").unwrap();
    assert_eq!(
        sent,
        vec![Message {
            role: Role::User,
            content: "first".into()
        }]
    );
    assert_eq!(chat.ask("again"), None, "one round at a time");
    chat.text("one ");
    chat.text("answer");
    chat.finish(Ok(Some(rounds(4, 5))));
    assert_eq!(chat.thread.len(), 2);
    assert_eq!(chat.thread[1].content, "one answer");
    assert_eq!(
        chat.note().as_deref(),
        Some("1 round left. Sign in to keep going with this chat.")
    );

    let sent = chat.ask("second").unwrap();
    assert_eq!(sent.len(), 3, "the whole conversation goes each round");
    chat.finish(Ok(Some(rounds(5, 5))));
    assert_eq!(chat.end(), Some(Refusal::RoundsUsed));
    assert_eq!(chat.note(), None);
    assert_eq!(chat.ask("third"), None);
    assert_eq!(card(Refusal::RoundsUsed).unwrap().0, "You used your free rounds");
}

#[test]
fn the_middle_rounds_say_how_many_are_used() {
    let mut chat = Chat::new(true);
    chat.status_read(loaded(0, 5));
    chat.ask("hi").unwrap();
    chat.finish(Ok(Some(rounds(1, 5))));
    assert_eq!(
        chat.note().as_deref(),
        Some("1 of 5 rounds used. This chat disappears when you close the app. Sign in to keep it.")
    );
}

#[test]
fn a_refusal_with_a_card_ends_the_chat_and_another_takes_the_question_back() {
    let mut chat = Chat::new(true);
    chat.status_read(loaded(0, 5));
    chat.ask("hi").unwrap();
    chat.finish(Err(Refusal::Turnstile));
    assert!(chat.thread.is_empty());
    assert!(!chat.busy());
    assert_eq!(chat.end(), None);
    assert_eq!(
        chat.note().as_deref(),
        Some("The check that you are a person did not pass. Try again.")
    );
    chat.ask("hi").unwrap();
    chat.finish(Err(Refusal::Failed));
    assert_eq!(chat.note().as_deref(), Some("The answer did not arrive. Try again."));
    chat.ask("hi").unwrap();
    chat.finish(Err(Refusal::Paused));
    assert_eq!(chat.end(), Some(Refusal::Paused));
    assert_eq!(card(Refusal::Paused).unwrap().0, "Guest chat is resting for today");
}

#[test]
fn guest_chat_shows_as_off_without_a_token_or_a_guest_route() {
    let mut chat = Chat::new(false);
    assert_eq!(chat.end(), None, "nothing ends while the status loads");
    chat.status_read(loaded(0, 5));
    assert_eq!(chat.end(), Some(Refusal::Off));
    assert_eq!(chat.ask("hi"), None);

    let mut chat = Chat::new(true);
    chat.status_read(Ok(None));
    assert_eq!(chat.end(), Some(Refusal::Off));
    assert_eq!(card(Refusal::Off).unwrap().0, "Sign in to chat with Nexus");

    let mut chat = Chat::new(true);
    chat.status_read(Err(Refusal::AddressLimit));
    assert_eq!(chat.end(), Some(Refusal::AddressLimit));

    let mut chat = Chat::new(true);
    chat.status_read(Err(Refusal::Failed));
    assert_eq!(chat.end(), Some(Refusal::Off));
}

#[test]
fn the_challenge_page_hands_over_a_token_or_an_error_in_the_fragment() {
    let site = "https://nexus.test/";
    assert_eq!(challenge_url(site), "https://nexus.test/app/guest/challenge");
    let handoff_at = handoff_url(site);
    assert_eq!(handoff_at, "https://nexus.test/app/guest/challenge/token");
    assert_eq!(
        handoff(&format!("{handoff_at}#0.ab_c-D%2Bx%3D")),
        Handoff::Token("0.ab_c-D+x=".into())
    );
    assert_eq!(
        handoff(&format!("{handoff_at}#error=300030")),
        Handoff::Error("300030".into())
    );
    assert_eq!(
        handoff(&format!("{handoff_at}#error=")),
        Handoff::Error("unknown".into())
    );
    assert_eq!(handoff(&format!("{handoff_at}#")), Handoff::Error("empty".into()));
    assert_eq!(handoff(&handoff_at), Handoff::Error("empty".into()));
}

#[test]
fn a_kept_token_goes_once_and_not_after_four_minutes() {
    let at = Instant::now();
    let kept = Kept::new("t".into(), at);
    assert!(kept.fresh(at + TOKEN_MAX_AGE - Duration::from_secs(1)));
    assert!(!kept.fresh(at + TOKEN_MAX_AGE));
    assert_eq!(kept.take(at + Duration::from_secs(60)), Some("t".into()));
    assert_eq!(Kept::new("t".into(), at).take(at + TOKEN_MAX_AGE), None);
}

#[test]
fn a_challenge_page_without_guest_mode_shows_guest_chat_as_off() {
    let mut chat = Chat::new(true);
    chat.status_read(loaded(0, 5));
    assert_eq!(chat.end(), None);
    chat.no_token();
    assert_eq!(chat.end(), Some(Refusal::Off));
}
