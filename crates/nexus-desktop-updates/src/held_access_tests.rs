use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const GATEWAY: &str = "https://api.nexus.inc";
const TOKEN: &str = "nxc_test_employee";
const PUBLIC: &str = "0.11.0";
const INTERNAL: &str = "0.11.0-internal.42";

/// What the fake access endpoint answers.
#[derive(Clone, Copy)]
enum Answer {
    Status(u16, &'static str),
    /// Read the request, then never answer: the client's deadline must end it.
    Silent,
}

/// A fake `GET /v1/desktop/download-access` on this computer that answers `answer` to every request and counts them.
async fn fake(answer: Answer) -> (String, Arc<AtomicUsize>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("its address");
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            counted.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut request = [0; 2048];
                let _ = socket.read(&mut request).await;
                match answer {
                    Answer::Status(status, body) => {
                        let response = format!(
                            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                    }
                    Answer::Silent => tokio::time::sleep(Duration::from_secs(30)).await,
                }
            });
        }
    });
    (format!("http://{address}/v1/desktop/download-access"), calls)
}

async fn ask(
    held: &Held,
    endpoint: &str,
    installed: &str,
    manual: bool,
    now: Instant,
) -> Result<(Channel, Option<HeaderValue>), Failure> {
    let check = Check {
        endpoint,
        wait: Duration::from_millis(300),
    };
    channel_with(held, &check, GATEWAY, Some(TOKEN), installed, manual, now).await
}

fn channel_of(result: &Result<(Channel, Option<HeaderValue>), Failure>) -> Result<Channel, &'static str> {
    result
        .as_ref()
        .map(|(channel, _)| *channel)
        .map_err(|failure| failure.code)
}

#[tokio::test]
async fn every_answer_for_a_public_and_an_internal_build() {
    let unavailable = Err("employee_verification_unavailable");
    let cases = [
        // (what the endpoint answers, a public build's channel, an internal build's)
        (
            Answer::Status(200, r#"{"allowed":true}"#),
            Ok(Channel::Internal),
            Ok(Channel::Internal),
        ),
        (
            Answer::Status(200, r#"{"allowed":false}"#),
            Ok(Channel::Stable),
            Err("employee_access_required"),
        ),
        (
            Answer::Status(401, "{}"),
            Ok(Channel::Stable),
            Err("employee_sign_in_required"),
        ),
        (
            Answer::Status(403, "{}"),
            Ok(Channel::Stable),
            Err("employee_access_required"),
        ),
        (
            Answer::Status(503, r#"{"error":"employee_verification_unavailable"}"#),
            Ok(Channel::Stable),
            unavailable,
        ),
        (Answer::Silent, Ok(Channel::Stable), unavailable),
        (Answer::Status(200, "not json"), Ok(Channel::Stable), unavailable),
    ];
    for (answer, public, internal) in cases {
        let (endpoint, _) = fake(answer).await;
        let now = Instant::now();
        let result = ask(&Held::default(), &endpoint, PUBLIC, false, now).await;
        assert_eq!(channel_of(&result), public);
        assert_eq!(
            result
                .as_ref()
                .ok()
                .and_then(|(_, header)| header.is_some().then_some(())),
            (public == Ok(Channel::Internal)).then_some(()),
            "a credential only with internal access"
        );
        let result = ask(&Held::default(), &endpoint, INTERNAL, false, now).await;
        assert_eq!(channel_of(&result), internal);
        assert!(!format!("{result:?}").contains(TOKEN), "the token never shows");
    }
}

#[tokio::test]
async fn a_held_answer_spares_gateway_until_it_expires() {
    let (endpoint, calls) = fake(Answer::Status(200, r#"{"allowed":true}"#)).await;
    let held = Held::default();
    let start = Instant::now();
    for minute in [0, 1, 10, 59] {
        let now = start + Duration::from_secs(minute * 60);
        let result = ask(&held, &endpoint, INTERNAL, false, now).await;
        assert_eq!(channel_of(&result), Ok(Channel::Internal));
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "focus, wake and launch within the hour ask once"
    );
    // A manual check keeps an allowed answer too.
    let _ = ask(&held, &endpoint, INTERNAL, true, start + Duration::from_secs(30 * 60)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let _ = ask(&held, &endpoint, INTERNAL, false, start + HOLD).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2, "past the hour it asks again");
}

#[tokio::test]
async fn an_outage_holds_five_minutes_and_a_manual_check_asks_again() {
    let (endpoint, calls) = fake(Answer::Status(503, "{}")).await;
    let held = Held::default();
    let start = Instant::now();
    for seconds in [0, 30, 240] {
        let result = ask(&held, &endpoint, PUBLIC, false, start + Duration::from_secs(seconds)).await;
        assert_eq!(channel_of(&result), Ok(Channel::Stable));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let _ = ask(&held, &endpoint, INTERNAL, true, start + Duration::from_secs(60)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "Check for updates asks again after an outage"
    );
    let _ = ask(
        &held,
        &endpoint,
        PUBLIC,
        false,
        start + Duration::from_secs(60) + HOLD_OUTAGE,
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn another_sign_in_is_never_answered_from_the_held_one() {
    let (endpoint, calls) = fake(Answer::Status(200, r#"{"allowed":true}"#)).await;
    let held = Held::default();
    let check = Check {
        endpoint: &endpoint,
        wait: Duration::from_millis(300),
    };
    let now = Instant::now();
    let first = channel_with(&held, &check, GATEWAY, Some(TOKEN), INTERNAL, false, now).await;
    assert_eq!(channel_of(&first), Ok(Channel::Internal));
    let signed_out = channel_with(&held, &check, GATEWAY, None, INTERNAL, false, now).await;
    assert_eq!(channel_of(&signed_out), Err("employee_sign_in_required"));
    let public = channel_with(&held, &check, GATEWAY, None, PUBLIC, false, now).await;
    assert_eq!(channel_of(&public), Ok(Channel::Stable));
    let other = channel_with(&held, &check, GATEWAY, Some("nxc_other"), INTERNAL, false, now).await;
    assert_eq!(channel_of(&other), Ok(Channel::Internal));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the signed-out checks need no call; the other token does"
    );
}
