use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{CODE, Latch, Revoked, recognize};

const MESSAGE: &str = "This version of the Nexus app (0.11.0) was withdrawn. Update Nexus to continue.";

fn upgrade() -> Vec<(String, String)> {
    vec![("upgrade".into(), "nexus-desktop".into())]
}

/// Gateway's three bodies for the refusal (`desktop_revoked::refuse_revoked_desktop`, `error.rs`), as each route
/// family answers it.
fn bodies() -> [(&'static str, serde_json::Value); 3] {
    [
        (
            "core",
            serde_json::json!({ "error": {
                "message": MESSAGE, "type": "invalid_request_error", "code": CODE, "version": "0.11.0",
            }}),
        ),
        (
            "anthropic",
            serde_json::json!({ "type": "error", "error": {
                "type": "invalid_request_error", "message": MESSAGE,
                "nexus": { "version": "0.11.0", "code": CODE },
            }}),
        ),
        (
            "openai",
            serde_json::json!({ "error": {
                "message": MESSAGE, "type": "invalid_request_error", "code": CODE, "param": null,
                "nexus": { "version": "0.11.0" },
            }}),
        ),
    ]
}

#[test]
fn a_refusal_is_recognized_in_each_route_familys_shape() {
    for (family, body) in bodies() {
        let body = body.to_string();
        let expected = Some(Revoked {
            version: "0.11.0".into(),
            message: MESSAGE.into(),
        });
        // With Gateway's header, and from the body alone.
        assert_eq!(recognize(426, &upgrade(), Some(body.as_bytes())), expected, "{family}");
        assert_eq!(recognize(426, &[], Some(body.as_bytes())), expected, "{family}");
    }
}

#[test]
fn a_streams_head_is_enough_and_the_words_fall_back_to_ours() {
    let revoked = recognize(426, &[("Upgrade".into(), " Nexus-Desktop ".into())], None).expect("revoked");
    assert_eq!(revoked.version, crate::channel::VERSION);
    assert!(revoked.message.contains("was withdrawn"), "{}", revoked.message);
    // A body that is not JSON keeps the header's verdict.
    assert!(recognize(426, &upgrade(), Some(b"<html>")).is_some());
}

#[test]
fn an_ordinary_426_or_another_error_is_not_a_refusal() {
    let core = bodies()[0].1.to_string();
    assert_eq!(recognize(426, &[], None), None);
    assert_eq!(recognize(426, &[], Some(b"")), None);
    assert_eq!(
        recognize(426, &[], Some(br#"{"error":{"code":"upgrade_required"}}"#)),
        None
    );
    assert_eq!(recognize(426, &[("upgrade".into(), "websocket".into())], None), None);
    assert_eq!(recognize(426, &[("upgrade".into(), "h2c".into())], Some(b"{}")), None);
    for status in [200, 400, 401, 403, 404, 500, 502, 503] {
        assert_eq!(recognize(status, &upgrade(), Some(core.as_bytes())), None, "{status}");
    }
}

#[test]
fn the_latch_keeps_the_first_refusal_and_starts_one_update_check() {
    let checks = Arc::new(AtomicUsize::new(0));
    let latch = {
        let checks = checks.clone();
        Latch::new(move |_| {
            checks.fetch_add(1, Ordering::SeqCst);
        })
    };
    let mut changes = latch.subscribe();
    assert_eq!(latch.current(), None);
    let first = Revoked {
        version: "0.11.0".into(),
        message: "first".into(),
    };
    assert!(latch.note(first.clone()));
    assert!(changes.has_changed().unwrap_or(false));
    changes.mark_unchanged();
    for _ in 0..3 {
        assert!(!latch.note(Revoked {
            version: "0.11.0".into(),
            message: "later".into(),
        }));
    }
    assert_eq!(latch.current(), Some(first));
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    // The later refusals told no window.
    assert!(!changes.has_changed().unwrap_or(true));
}
