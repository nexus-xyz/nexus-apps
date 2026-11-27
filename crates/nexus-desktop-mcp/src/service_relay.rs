//! The app's side of the background service's socket (nexus-host's `app_relay`). The service's agents call the pull
//! request tools and `request_secret` on the service, which passes each call to the app that is connected. The
//! app answers such a call here as a call on its own socket, and tells the service the answer.
//!
//! - [`read`]: what a push from the service is: a call to answer, or a run that ended (its card and files go).
//! - [`reply`]: the arguments of `host_app_reply` for a call. A call whose organization is not the app's now is
//!   refused, as the app's own socket refuses it.

use serde_json::{Value, json};

use crate::app_socket::{CALL_EVENT, Grant, RUN_ENDED_EVENT, Relayed};

/// The reply's refusal, as the app's own socket says it.
const INVALID: &str = "This credential for Nexus's tools is invalid or expired.";

/// A push from the service for the app's tools.
#[derive(Debug, Clone, PartialEq)]
pub enum Pushed {
    /// A call to answer, then reply to with [`reply`].
    Call(Relayed),
    /// A run of the service ended: its open card closes and its files go.
    RunEnded(String),
}

/// What the push `event` with `payload` is for the app's tools; None for any other push.
pub fn read(event: &str, payload: &Value) -> Option<Pushed> {
    match event {
        CALL_EVENT => Relayed::from_payload(payload).map(Pushed::Call),
        RUN_ENDED_EVENT => payload["run"].as_str().map(|run| Pushed::RunEnded(run.to_string())),
        _ => None,
    }
}

/// The arguments of `host_app_reply` for `call`: `answer`'s reply while the app is signed in to the call's
/// organization (`scope`, `<gateway>|<org>`), else a refusal. `answer` runs the call as the app's own socket does.
pub fn reply(call: &Relayed, scope: &str, answer: impl FnOnce(&Grant, &Value) -> Value) -> Value {
    let reply = if call.grant.scope == scope {
        answer(&call.grant, &call.input)
    } else {
        json!({"jsonrpc": "2.0", "id": call.input["message"]["id"], "error": {"code": -32001, "message": INVALID}})
    };
    call.reply(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call() -> Relayed {
        Relayed {
            id: 4,
            grant: Grant {
                thread: "t1".into(),
                scope: "gw|acme".into(),
                run: "ns_1".into(),
                sign_in: "me".into(),
            },
            input: json!({"tools": "pull_request", "message": {"jsonrpc": "2.0", "id": 8}}),
        }
    }

    #[test]
    fn reads_the_services_pushes_for_the_apps_tools_only() {
        assert_eq!(read(CALL_EVENT, &call().payload()), Some(Pushed::Call(call())));
        assert_eq!(
            read(RUN_ENDED_EVENT, &json!({"run": "ns_1"})),
            Some(Pushed::RunEnded("ns_1".into()))
        );
        assert_eq!(read("wb:event", &json!({})), None);
    }

    #[test]
    fn answers_a_call_of_the_apps_organization_only() {
        let answered = reply(
            &call(),
            "gw|acme",
            |grant, input| json!({"id": input["message"]["id"], "thread": grant.thread}),
        );
        assert_eq!(answered, json!({"id": 4, "reply": {"id": 8, "thread": "t1"}}));
        let refused = reply(&call(), "gw|other", |_, _| panic!("not run for another organization"));
        assert_eq!(refused["reply"]["error"]["message"], INVALID);
        assert_eq!(refused["reply"]["id"], 8);
    }
}
