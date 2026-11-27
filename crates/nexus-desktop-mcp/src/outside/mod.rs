//! Outside access: scripts, the `nexus` CLI and other agents list, start, read, message and stop Code
//! sessions through a socket only this person can open. Off until the person turns it on (Settings, MCP servers,
//! "Outside access").
//!
//! - The endpoint ([`socket`]): a Unix socket, 0600 in a 0700 folder: `$XDG_RUNTIME_DIR/<app id>/sessions.sock` on
//!   Linux, `<app data>/run/sessions.sock` on macOS, the folder the background service's socket uses too.
//!   No TCP port, so nothing on the network or in another account reaches it. Windows has none yet.
//! - The protocol: one JSON line in, one JSON line out, per connection. `{"client", "connect": "read"|"act"}` asks the
//!   person for access; the app shows a prompt, and the reply is `{"token", "scope"}` once they approve, or an error.
//!   `{"token", "message"}` carries one MCP JSON-RPC message (`initialize`, `tools/list`, `tools/call`…).
//! - Grants ([`grants`]): one per client, its scope read or act, listed in Settings with its last use, revocable.
//!   Nothing runs for a client before the person approves it. A read grant lists and calls only the reading tools.
//! - The tools ([`tools`], run by the app's [`Host`]): a start goes through the app's own New session path, so it takes the
//!   project's route, model, worktree choice and permission mode; a mode asked for is used only when it is lower.

pub mod grants;
#[cfg(unix)]
pub mod socket;
pub mod tools;

use grants::{Grants, Scope};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

/// How long a consent prompt waits for the person.
pub const CONSENT_WAIT: Duration = Duration::from_secs(120);
/// At most this many prompts wait at once, so a client cannot bury the app in them.
const MAX_PENDING: usize = 3;
/// The longest client name.
const MAX_CLIENT: usize = 64;

/// A client asking for access, as the prompt shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentRequest {
    pub id: String,
    pub client: String,
    pub scope: Scope,
    pub at: i64,
}

/// What the endpoint needs from the app: the prompt, and the tools run for a granted client.
pub trait Host: Send + Sync {
    /// Shows the person `request`. The answer comes back through [`Outside::answer`].
    fn ask(&self, request: &ConsentRequest);
    /// Runs tool `name`. The caller has already checked the grant's scope.
    fn call(&self, name: &str, args: &Map<String, Value>) -> Result<Value, String>;
    /// A prompt was answered: Settings shows the grants again.
    fn changed(&self) {}
}

/// The prompts waiting for the person, by id, each with where its answer goes.
type Waiting = HashMap<String, (ConsentRequest, mpsc::Sender<Option<Scope>>)>;

/// The endpoint's state: the grants, and the prompts waiting for the person.
pub struct Outside {
    grants: Mutex<Grants>,
    pending: Mutex<Waiting>,
    consent_wait: Duration,
}

fn error(code: &str, message: &str) -> Value {
    json!({"error": {"code": code, "message": message}})
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// A client's name: 1 to 64 characters, no control characters.
fn client_name(value: &Value) -> Option<String> {
    let name = value.as_str()?.trim();
    (!name.is_empty() && name.chars().count() <= MAX_CLIENT && !name.chars().any(char::is_control))
        .then(|| name.to_string())
}

impl Outside {
    pub fn new(grants: Grants) -> Outside {
        Outside {
            grants: Mutex::new(grants),
            pending: Mutex::new(HashMap::new()),
            consent_wait: CONSENT_WAIT,
        }
    }

    pub fn grants(&self) -> std::sync::MutexGuard<'_, Grants> {
        self.grants.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn waiting(&self) -> std::sync::MutexGuard<'_, Waiting> {
        self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The prompts waiting for the person, oldest first.
    pub fn pending(&self) -> Vec<ConsentRequest> {
        let mut pending: Vec<ConsentRequest> = self.waiting().values().map(|(request, _)| request.clone()).collect();
        pending.sort_by_key(|request| request.at);
        pending
    }

    /// The person's answer to prompt `id`: a scope approves (never wider than the one asked for), None denies.
    pub fn answer(&self, id: &str, scope: Option<Scope>) -> bool {
        match self.waiting().remove(id) {
            Some((request, reply)) => {
                let scope = scope.map(|scope| {
                    if request.scope == Scope::Read {
                        Scope::Read
                    } else {
                        scope
                    }
                });
                reply.send(scope).is_ok()
            }
            None => false,
        }
    }

    /// Denies every waiting prompt: outside access was turned off.
    pub fn deny_all(&self) {
        for (_, (_, reply)) in self.waiting().drain() {
            let _ = reply.send(None);
        }
    }

    /// One request line's answer. Blocks while a consent prompt waits for the person.
    pub fn handle(&self, host: &dyn Host, request: &Value) -> Value {
        if let Some(asked) = request.get("connect") {
            return self.connect(host, &request["client"], asked);
        }
        let message = &request["message"];
        let id = message.get("id").cloned();
        let token = request["token"].as_str().unwrap_or_default();
        let Some(grant) = self.grants().authorize(token) else {
            return rpc_error(
                &id.unwrap_or(Value::Null),
                -32001,
                "This client has no access to Nexus sessions, or it was revoked. Ask for access again: the person approves it in Nexus.",
            );
        };
        // A notification has no answer.
        let Some(id) = id else {
            return Value::Null;
        };
        let result = match message["method"].as_str().unwrap_or_default() {
            "initialize" => json!({
                "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "nexus_sessions", "version": nexus_desktop_core::app::version()}
            }),
            "ping" => json!({}),
            "tools/list" => json!({"tools": tools::definitions(grant.scope)}),
            "tools/call" => {
                let name = message["params"]["name"].as_str().unwrap_or_default();
                let args = message["params"]["arguments"].as_object().cloned().unwrap_or_default();
                let answer = if tools::acts(name) && grant.scope != Scope::Act {
                    Err(format!(
                        "{} may only read sessions. To let it start, message or stop them, approve it again with read and act.",
                        grant.client
                    ))
                } else {
                    host.call(name, &args)
                };
                match answer {
                    Ok(value) => {
                        json!({"content": [{"type": "text", "text": value.to_string()}], "structuredContent": value, "isError": false})
                    }
                    Err(message) => json!({"content": [{"type": "text", "text": message}], "isError": true}),
                }
            }
            _ => return rpc_error(&id, -32601, "Method not found"),
        };
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    }

    fn connect(&self, host: &dyn Host, client: &Value, asked: &Value) -> Value {
        let Some(client) = client_name(client) else {
            return error("bad_request", "Name the client: 1 to 64 characters.");
        };
        let Some(scope) = asked.as_str().and_then(Scope::parse) else {
            return error("bad_request", "Ask for read or act.");
        };
        let (reply, answer) = mpsc::channel();
        let request = ConsentRequest {
            id: uuid::Uuid::new_v4().simple().to_string(),
            client: client.clone(),
            scope,
            at: grants::now_s(),
        };
        {
            let mut waiting = self.waiting();
            if waiting.len() >= MAX_PENDING || waiting.values().any(|(other, _)| other.client == client) {
                return error(
                    "busy",
                    "Another request for access is waiting for the person. Try again later.",
                );
            }
            waiting.insert(request.id.clone(), (request.clone(), reply));
        }
        host.ask(&request);
        let decided = answer.recv_timeout(self.consent_wait);
        self.waiting().remove(&request.id);
        let reply = match decided {
            Ok(Some(scope)) => match self.grants().add(&client, scope) {
                Ok(token) => json!({"token": token, "scope": scope}),
                Err(message) => error("failed", &message),
            },
            Ok(None) => error("denied", "The person denied access."),
            Err(_) => error("denied", "Nobody answered the request for access in Nexus."),
        };
        host.changed();
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Condvar};

    /// A fake app: it records the prompts and the tool calls, and answers every call with its name.
    #[derive(Default)]
    pub(super) struct Fake {
        pub asked: Mutex<Vec<ConsentRequest>>,
        pub calls: Mutex<Vec<String>>,
        shown: Condvar,
    }

    impl Fake {
        fn wait_for_prompts(&self, count: usize) -> Vec<ConsentRequest> {
            let (asked, waited) = self
                .shown
                .wait_timeout_while(self.asked.lock().expect("lock"), Duration::from_secs(5), |asked| {
                    asked.len() < count
                })
                .expect("prompt signal");
            assert!(
                !waited.timed_out(),
                "expected {count} shown prompts, got {}",
                asked.len()
            );
            asked.clone()
        }
    }

    impl Host for Fake {
        fn ask(&self, request: &ConsentRequest) {
            self.asked.lock().expect("lock").push(request.clone());
            self.shown.notify_all();
        }
        fn call(&self, name: &str, _: &Map<String, Value>) -> Result<Value, String> {
            self.calls.lock().expect("lock").push(name.into());
            Ok(json!({"called": name}))
        }
    }

    pub(super) fn call(token: &str, name: &str) -> Value {
        json!({"token": token, "message": {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": {}}}})
    }

    /// Asks for `scope` on a thread and answers the prompt with `answer` once it shows.
    fn connect(outside: &Arc<Outside>, fake: &Arc<Fake>, scope: &str, answer: Option<Scope>) -> Value {
        let (o, f, scope) = (outside.clone(), fake.clone(), scope.to_string());
        let asking = std::thread::spawn(move || o.handle(&*f, &json!({"client": "test", "connect": scope})));
        // The prompt shows, and nothing has run.
        assert_eq!(fake.wait_for_prompts(1).len(), 1);
        assert!(fake.calls.lock().expect("lock").is_empty());
        let id = outside.pending()[0].id.clone();
        assert!(outside.answer(&id, answer));
        asking.join().expect("join")
    }

    #[test]
    fn a_request_without_a_token_or_with_a_wrong_one_is_refused() {
        let outside = Outside::new(Grants::default());
        let fake = Fake::default();
        for token in ["", "nope"] {
            let reply = outside.handle(&fake, &call(token, tools::LIST));
            assert_eq!(reply["error"]["code"], -32001);
        }
        assert!(fake.calls.lock().expect("lock").is_empty());
    }

    #[test]
    fn the_prompt_blocks_until_the_person_approves_and_a_denial_grants_nothing() {
        let outside = Arc::new(Outside::new(Grants::default()));
        let fake = Arc::new(Fake::default());
        let denied = connect(&outside, &fake, "act", None);
        assert_eq!(denied["error"]["code"], "denied");
        assert!(outside.grants().views().is_empty());

        fake.asked.lock().expect("lock").clear();
        let approved = connect(&outside, &fake, "act", Some(Scope::Act));
        let token = approved["token"].as_str().expect("token").to_string();
        assert_eq!(approved["scope"], "act");
        let reply = outside.handle(&*fake, &call(&token, tools::STOP));
        assert_eq!(reply["result"]["isError"], false);
        assert_eq!(*fake.calls.lock().expect("lock"), [tools::STOP]);
    }

    #[test]
    fn a_read_grant_cannot_act_and_cannot_be_widened_by_the_answer() {
        let outside = Arc::new(Outside::new(Grants::default()));
        let fake = Arc::new(Fake::default());
        // The client asked to read: an answer of act still grants read only.
        let approved = connect(&outside, &fake, "read", Some(Scope::Act));
        assert_eq!(approved["scope"], "read");
        let token = approved["token"].as_str().expect("token").to_string();
        for name in [tools::START, tools::SEND, tools::STOP, tools::NAME] {
            let reply = outside.handle(&*fake, &call(&token, name));
            assert_eq!(reply["result"]["isError"], true, "{name}");
        }
        assert!(fake.calls.lock().expect("lock").is_empty());
        let listed = outside.handle(&*fake, &call(&token, tools::LIST));
        assert_eq!(listed["result"]["isError"], false);
        let tools = outside.handle(
            &*fake,
            &json!({"token": token, "message": {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}}),
        );
        assert_eq!(tools["result"]["tools"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn a_revoked_grant_is_refused() {
        let outside = Arc::new(Outside::new(Grants::default()));
        let fake = Arc::new(Fake::default());
        let token = connect(&outside, &fake, "act", Some(Scope::Act))["token"]
            .as_str()
            .expect("token")
            .to_string();
        let id = outside.grants().views()[0].id.clone();
        assert!(outside.grants().revoke(&id).expect("revoke"));
        let reply = outside.handle(&*fake, &call(&token, tools::LIST));
        assert_eq!(reply["error"]["code"], -32001);
        assert!(fake.calls.lock().expect("lock").is_empty());
    }

    #[test]
    fn an_unanswered_prompt_times_out_as_a_denial_and_prompts_are_capped() {
        let mut outside = Outside::new(Grants::default());
        outside.consent_wait = Duration::from_millis(20);
        let fake = Fake::default();
        let reply = outside.handle(&fake, &json!({"client": "slow", "connect": "read"}));
        assert_eq!(reply["error"]["code"], "denied");
        assert!(outside.pending().is_empty());

        let outside = Arc::new(Outside::new(Grants::default()));
        let fake = Arc::new(Fake::default());
        let waiting: Vec<_> = (0..MAX_PENDING)
            .map(|n| {
                let (o, f) = (outside.clone(), fake.clone());
                std::thread::spawn(move || o.handle(&*f, &json!({"client": format!("c{n}"), "connect": "read"})))
            })
            .collect();
        fake.wait_for_prompts(MAX_PENDING);
        let busy = outside.handle(&*fake, &json!({"client": "another", "connect": "read"}));
        assert_eq!(busy["error"]["code"], "busy");
        outside.deny_all();
        for thread in waiting {
            assert_eq!(thread.join().expect("join")["error"]["code"], "denied");
        }
    }

    #[test]
    fn a_client_must_name_itself() {
        let outside = Outside::new(Grants::default());
        let fake = Fake::default();
        for client in [json!(""), json!("a\u{7}b"), json!("x".repeat(65)), Value::Null] {
            let reply = outside.handle(&fake, &json!({"client": client, "connect": "read"}));
            assert_eq!(reply["error"]["code"], "bad_request");
        }
        assert!(fake.asked.lock().expect("lock").is_empty());
    }
}
