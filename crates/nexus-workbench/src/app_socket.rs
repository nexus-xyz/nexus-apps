//! The app's loopback socket, which the built-in `nexus` server relays tool calls to: a TCP port on 127.0.0.1 that the app binds, and a credential per agent run that reaches the agent only through
//! its environment ([`crate::mcp_servers::with_app_tools`]). No UI toolkit here; each app answers the calls itself.
//! While the background service runs the sessions, the service binds it instead and passes each call to the app
//! ([`Relayed`]).
//!
//! The protocol: one connection per call, one JSON line in and one out. The request is `{"token", "tools",
//! "message"}`: the run's credential, which tools the call is for ([`crate::mcp_servers::PULL_REQUEST_TOOLS`],
//! [`crate::secret_request::TOOLS`], or none for the browser), and one MCP JSON-RPC message. The reply is the
//! message's JSON-RPC answer.
//!
//! - [`serve`] and [`read_request`]: the server's loop and its read of one request. Nexus GPUI (src/app_socket.rs)
//!   serves with them.
//! - [`Grants`]: the credentials, one per agent run, scoped to the session, the organization and the sign-in.
//! - [`relay`]: the client side, which the built-in server uses (`mcp::relay_to_app`).
//! - [`Relayed`]: a call the background service (nexus-host's `app_relay`) passes to the app over its connection.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

/// The largest request line.
pub const LIMIT: u64 = 128 * 1024;
/// Connections answered at once. More wait in the system's queue until one ends.
const MAX_CONNECTIONS: usize = 8;

/// Accepts connections on `listener` until `stop` is set, and answers each on its own thread, at most
/// [`MAX_CONNECTIONS`] at once. A connection over the limit is closed unanswered.
pub fn serve<F>(listener: TcpListener, stop: Arc<AtomicBool>, answer: F)
where
    F: Fn(TcpStream) + Clone + Send + 'static,
{
    let active = Arc::new(AtomicUsize::new(0));
    for connection in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = connection else {
            // Out of file descriptors, say: accept fails at once again, so wait instead of spinning a core.
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let (answer, active) = (answer.clone(), active.clone());
        std::thread::spawn(move || {
            answer(stream);
            active.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

/// Reads the connection's one request line, at most [`LIMIT`] bytes, within 5 seconds. Sets the reply's write
/// timeout too.
pub fn read_request(stream: &TcpStream) -> Result<Value, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(12)))
        .map_err(|error| error.to_string())?;
    let mut line = String::new();
    let count = BufReader::new(stream.take(LIMIT + 1))
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    if count as u64 > LIMIT || !line.ends_with('\n') {
        return Err("Request too large.".into());
    }
    serde_json::from_str(&line).map_err(|error| error.to_string())
}

/// Sends one request to the app's socket, and returns the reply's line, waiting at most `wait` for it.
pub fn relay(address: &SocketAddr, request: &Value, wait: Duration) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(address, Duration::from_secs(3)).map_err(|error| error.to_string())?;
    stream.set_read_timeout(Some(wait)).map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|error| error.to_string())?;
    writeln!(stream, "{request}").map_err(|error| error.to_string())?;
    let mut reply = String::new();
    let read = BufReader::new(stream.take(20 * 1024 * 1024))
        .read_line(&mut reply)
        .map_err(|error| error.to_string())?;
    if read == 0 || !reply.ends_with('\n') {
        return Err("The connection closed without a complete reply.".into());
    }
    Ok(reply)
}

/// What a credential lets its holder do: call the tools of one run of one session, while the app is signed in to the
/// same organization as the same person.
#[derive(Debug, Clone, PartialEq)]
pub struct Grant {
    pub thread: String,
    /// `<gateway>|<org>`.
    pub scope: String,
    /// The run's Nexus agent session: its end revokes the credential.
    pub run: String,
    /// Who was signed in when the run started.
    pub sign_in: String,
}

#[derive(Debug)]
enum Holder {
    SignedIn(Grant),
    Local { run: String },
}

impl Holder {
    fn run(&self) -> &str {
        match self {
            Self::SignedIn(grant) => &grant.run,
            Self::Local { run } => run,
        }
    }
}

/// The credentials handed out, by token, and the servers a launch credential may start. In memory only: the app's
/// restart ends every run.
#[derive(Debug, Default)]
pub struct Grants {
    grants: HashMap<String, Holder>,
    launches: HashMap<String, Launchables>,
}

impl Grants {
    /// A new credential for `grant`.
    pub fn add(&mut self, grant: Grant) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        self.grants.insert(token.clone(), Holder::SignedIn(grant));
        token
    }

    /// A new credential for `grant` that starts `servers` through the launcher (`nexus-desktop-mcp`'s `launch`), and
    /// does nothing else.
    pub fn add_launch(&mut self, grant: Grant, servers: Launchables) -> String {
        let token = self.add(grant);
        self.launches.insert(token.clone(), servers);
        token
    }

    /// Gives one local run a launch-only token.
    pub fn add_local_launch(&mut self, run: &str, servers: Launchables) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        self.grants.insert(token.clone(), Holder::Local { run: run.into() });
        self.launches.insert(token.clone(), servers);
        token
    }

    /// The grant of `token`, while the app's scope and sign-in are still the grant's.
    pub fn authorize(&self, token: &str, scope: &str, sign_in: &str) -> Option<Grant> {
        match self.grants.get(token) {
            Some(Holder::SignedIn(grant)) if grant.scope == scope && grant.sign_in == sign_in => Some(grant.clone()),
            _ => None,
        }
    }

    /// One request on the socket, checked: a launch credential only launches ([`Grants::launch`]), answered here, and
    /// any other credential only calls tools, which the app answers. Each works only while the app's scope and
    /// sign-in are the grant's.
    pub fn check(&mut self, input: &Value, scope: &str, sign_in: &str) -> Checked {
        let token = input["token"].as_str().unwrap_or_default();
        let message = &input["message"];
        let launching = input["tools"] == LAUNCH_TOOLS;
        let local = matches!(self.grants.get(token), Some(Holder::Local { .. }));
        let signed_in = self.authorize(token, scope, sign_in);
        let may_launch = self.launches.contains_key(token);
        if launching && may_launch && (local || signed_in.is_some()) {
            return Checked::Answered(self.launch(token, message));
        }
        match signed_in {
            Some(grant) if !launching && !may_launch => Checked::Call(grant),
            _ => Checked::Answered(json!({"jsonrpc": "2.0", "id": message["id"],
                "error": {"code": -32001, "message": INVALID}})),
        }
    }

    /// The answer to a [`LAUNCH_TOOLS`] call with an authorized `token`: the named server's command, arguments and
    /// environment, values included, once. The agent stores the credential in plain text, so a second ask (anything
    /// else that read it while the run lives) gets nothing; the agent starts each server once per run. A credential
    /// that was not made for launching starts nothing.
    pub fn launch(&mut self, token: &str, message: &Value) -> Value {
        let name = message["params"]["name"].as_str().unwrap_or_default();
        match self.launches.get_mut(token).and_then(|servers| servers.remove(name)) {
            Some(server) => json!({"jsonrpc": "2.0", "id": message["id"], "result": server}),
            None => json!({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32602,
                "message": format!("This run has no MCP server named {name} left to start.")}}),
        }
    }

    /// The run `run` ended: its credentials go. Says whether it had any.
    pub fn end_run(&mut self, run: &str) -> bool {
        let before = self.grants.len();
        self.grants.retain(|_, grant| grant.run() != run);
        let grants = &self.grants;
        self.launches.retain(|token, _| grants.contains_key(token));
        self.grants.len() != before
    }

    /// Every credential goes; returns the signed-in runs so their tools can be closed.
    pub fn clear(&mut self) -> Vec<String> {
        self.launches.clear();
        let mut runs: Vec<String> = self
            .grants
            .drain()
            .filter_map(|(_, holder)| match holder {
                Holder::SignedIn(grant) => Some(grant.run),
                Holder::Local { .. } => None,
            })
            .collect();
        runs.sort();
        runs.dedup();
        runs
    }
}

/// What a launch credential's call says in `tools`: the launcher asks for one server's command and environment.
pub const LAUNCH_TOOLS: &str = "launch";

/// The answer to a credential the socket refuses.
pub const INVALID: &str = "This credential for Nexus's tools is invalid or expired.";

/// A request as [`Grants::check`] found it.
#[derive(Debug, PartialEq)]
pub enum Checked {
    /// A tools call for the app, under this grant.
    Call(Grant),
    /// Answered already: a launch, or a refusal.
    Answered(Value),
}

/// The servers one launch credential may start, by name ([`crate::mcp_servers::Launchable`]).
pub type Launchables = std::collections::BTreeMap<String, crate::mcp_servers::Launchable>;

/// A relayed call's answer: the app's reply, or a tool error that says the app did not give one.
pub fn relayed(message: &Value, reply: Result<String, String>) -> Value {
    reply
        .and_then(|reply| serde_json::from_str(reply.trim()).map_err(|error| error.to_string()))
        .unwrap_or_else(|error: String| {
            let text = format!("Nexus for desktop did not answer: {error}. Is the app still open?");
            json!({"jsonrpc": "2.0", "id": message["id"],
                   "result": {"content": [{"type": "text", "text": text}], "isError": true}})
        })
}

/// The push that carries a [`Relayed`] call from the background service to the app.
pub const CALL_EVENT: &str = "host:app_call";
/// The app's answer to a [`Relayed`] call: `{ id, reply }`.
pub const REPLY_CALL: &str = "host_app_reply";
/// The push that says a run of the service ended (`{ run }`): the app closes its open card and deletes its files.
pub const RUN_ENDED_EVENT: &str = "host:app_run_ended";

/// A call the background service took on its socket and passes to the app: the grant it checked the credential
/// against, and the request without the credential (`{ tools, message }`). The app answers it as a call on its own
/// socket, if the grant's scope is still the app's.
#[derive(Debug, Clone, PartialEq)]
pub struct Relayed {
    pub id: u64,
    pub grant: Grant,
    pub input: Value,
}

impl Relayed {
    /// The [`CALL_EVENT`] push's payload.
    pub fn payload(&self) -> Value {
        json!({
            "id": self.id,
            "thread": self.grant.thread,
            "scope": self.grant.scope,
            "run": self.grant.run,
            "signIn": self.grant.sign_in,
            "tools": self.input["tools"],
            "message": self.input["message"],
        })
    }

    /// The call in a [`CALL_EVENT`] payload; None when it is not one.
    pub fn from_payload(payload: &Value) -> Option<Relayed> {
        let text = |name: &str| payload.get(name).and_then(Value::as_str).map(String::from);
        Some(Relayed {
            id: payload.get("id")?.as_u64()?,
            grant: Grant {
                thread: text("thread")?,
                scope: text("scope")?,
                run: text("run")?,
                sign_in: text("signIn").unwrap_or_default(),
            },
            input: json!({ "tools": payload["tools"], "message": payload["message"] }),
        })
    }

    /// The arguments of [`REPLY_CALL`] that answer it with `reply`.
    pub fn reply(&self, reply: Value) -> Value {
        json!({ "id": self.id, "reply": reply })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_relay_sends_one_line_and_returns_the_reply_line() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let app = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).expect("request");
            let request: Value = serde_json::from_str(&line).expect("json");
            writeln!(&stream, "{}", json!({"seen": request["tools"]})).expect("reply");
        });
        let reply = relay(
            &address,
            &json!({"token": "t", "tools": crate::mcp_servers::PULL_REQUEST_TOOLS, "message": {}}),
            crate::mcp_servers::REPLY_WAIT,
        )
        .expect("reply");
        assert_eq!(reply, "{\"seen\":\"pull_request\"}\n");
        app.join().expect("fake app");
        // Nothing listens any more: the caller gets an error, not a hang.
        assert!(relay(&address, &json!({}), crate::mcp_servers::REPLY_WAIT).is_err());
    }

    #[test]
    fn the_server_answers_each_request_line_and_stops_when_told() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let server = std::thread::spawn(move || {
            serve(listener, stopped, |stream| {
                let reply = match read_request(&stream) {
                    Ok(request) => json!({"echo": request["message"]}),
                    Err(error) => json!({"error": error}),
                };
                let _ = writeln!(&stream, "{reply}");
            })
        });
        let reply = relay(&address, &json!({"message": 7}), crate::mcp_servers::REPLY_WAIT).expect("reply");
        assert_eq!(reply, "{\"echo\":7}\n");
        stop.store(true, Ordering::SeqCst);
        // The loop sees the flag at its next connection.
        let _ = TcpStream::connect(address);
        server.join().expect("server");
    }

    #[test]
    fn a_relayed_call_goes_and_comes_back_without_its_credential() {
        let call = Relayed {
            id: 7,
            grant: Grant {
                thread: "t".into(),
                scope: "gw|acme".into(),
                run: "ns_1".into(),
                sign_in: "person".into(),
            },
            input: json!({"tools": crate::mcp_servers::PULL_REQUEST_TOOLS, "message": {"id": 3}}),
        };
        let payload = call.payload();
        assert!(payload.get("token").is_none());
        assert_eq!(Relayed::from_payload(&payload), Some(call.clone()));
        assert_eq!(call.reply(json!({"id": 3})), json!({"id": 7, "reply": {"id": 3}}));
        assert_eq!(Relayed::from_payload(&json!({"thread": "t"})), None);
    }

    #[test]
    fn credentials_are_scoped_and_end_with_their_run() {
        let mut grants = Grants::default();
        let grant = |run: &str| Grant {
            thread: "t".into(),
            scope: "one".into(),
            run: run.into(),
            sign_in: "person-one".into(),
        };
        let token = grants.add(grant("run"));
        let other = grants.add(grant("other"));
        assert_eq!(grants.authorize(&token, "one", "person-one"), Some(grant("run")));
        assert_eq!(grants.authorize(&token, "two", "person-one"), None);
        assert_eq!(grants.authorize(&token, "one", "person-two"), None);
        assert_eq!(grants.authorize("wrong", "one", "person-one"), None);
        assert!(grants.end_run("run"));
        assert!(!grants.end_run("run"));
        assert_eq!(grants.authorize(&token, "one", "person-one"), None);
        assert!(grants.authorize(&other, "one", "person-one").is_some());
        assert_eq!(grants.clear(), ["other"]);
        assert_eq!(grants.authorize(&other, "one", "person-one"), None);
    }

    /// A launch credential answers only its own servers, each once, and stops with its run; a tool credential starts
    /// nothing.
    #[test]
    fn a_launch_credential_starts_its_own_servers_once_until_its_run_ends() {
        let mut grants = Grants::default();
        let grant = Grant {
            thread: "t".into(),
            scope: "one".into(),
            run: "run".into(),
            sign_in: "person".into(),
        };
        let server = crate::mcp_servers::Launchable {
            command: "db-mcp".into(),
            args: vec!["--stdio".into()],
            env: std::collections::BTreeMap::from([("DB_TOKEN".into(), "s1".into())]),
            inherited: crate::mcp_servers::InheritedEnv::AppCredentialOnly,
        };
        let launchables = Launchables::from([("db".into(), server.clone()), ("cache".into(), server)]);
        let token = grants.add_launch(grant.clone(), launchables);
        let tools = grants.add(grant.clone());
        let ask = |grants: &mut Grants, token: &str, tools: Value, name: &str| {
            let message = json!({"jsonrpc": "2.0", "id": 4, "method": "launch", "params": {"name": name}});
            grants.check(
                &json!({"token": token, "tools": tools, "message": message}),
                "one",
                "person",
            )
        };
        let answered = |checked: Checked| match checked {
            Checked::Answered(answer) => answer,
            Checked::Call(_) => panic!("not answered"),
        };
        let answer = answered(ask(&mut grants, &token, json!(LAUNCH_TOOLS), "db"));
        assert_eq!(answer["result"]["command"], "db-mcp");
        assert_eq!(answer["result"]["env"]["DB_TOKEN"], "s1");
        // Once: anything else holding the stored credential gets nothing for a server already started.
        let again = answered(ask(&mut grants, &token, json!(LAUNCH_TOOLS), "db"));
        assert!(
            again["result"].is_null() && again["error"]["message"] != INVALID,
            "{again}"
        );
        assert!(answered(ask(&mut grants, &token, json!(LAUNCH_TOOLS), "other"))["error"].is_object());
        // A launch credential calls no tools, and a tools credential launches nothing.
        assert_eq!(
            answered(ask(&mut grants, &token, Value::Null, "db"))["error"]["message"],
            INVALID
        );
        assert_eq!(
            answered(ask(&mut grants, &tools, json!(LAUNCH_TOOLS), "db"))["error"]["message"],
            INVALID
        );
        assert_eq!(ask(&mut grants, &tools, Value::Null, "db"), Checked::Call(grant));
        // Another sign-in, or the run's end, starts nothing, even a server not started yet.
        let other = grants.check(
            &json!({"token": token, "tools": LAUNCH_TOOLS, "message": {"id": 4, "params": {"name": "cache"}}}),
            "one",
            "someone else",
        );
        assert_eq!(answered(other)["error"]["message"], INVALID);
        grants.end_run("run");
        assert_eq!(
            answered(ask(&mut grants, &token, json!(LAUNCH_TOOLS), "cache"))["error"]["message"],
            INVALID
        );
    }

    #[test]
    fn local_launcher_survives_sign_in_and_only_starts_its_own_servers() {
        use crate::mcp_servers::{InheritedEnv, Launchable};

        let mut grants = Grants::default();
        let server = Launchable {
            command: "db-mcp".into(),
            args: Vec::new(),
            env: Default::default(),
            inherited: InheritedEnv::NoCredentials,
        };
        let token = grants.add_local_launch("local_first", Launchables::from([("db".into(), server.clone())]));
        let other = grants.add_local_launch("local_other", Launchables::from([("db".into(), server)]));
        let answer = |grants: &mut Grants, token: &str, tools: Value| {
            let input = json!({"token": token, "tools": tools, "message": {"id": 1, "params": {"name": "db"}}});
            let Checked::Answered(reply) = grants.check(&input, "gateway|team", "signed-in") else {
                panic!("a local credential must never authorize a tool call");
            };
            reply
        };
        assert!(grants.authorize(&token, "", "").is_none());
        assert!(grants.authorize(&token, "gateway|team", "signed-in").is_none());
        assert_eq!(answer(&mut grants, &token, Value::Null)["error"]["message"], INVALID);
        assert_eq!(
            answer(&mut grants, &token, json!("pull_request"))["error"]["message"],
            INVALID
        );
        assert_eq!(
            answer(&mut grants, &token, json!(LAUNCH_TOOLS))["result"]["command"],
            "db-mcp"
        );
        assert_eq!(
            answer(&mut grants, &token, json!(LAUNCH_TOOLS))["error"]["code"],
            -32602
        );
        assert!(grants.end_run("local_first"));
        assert!(!grants.end_run("local_first"));
        assert_eq!(
            answer(&mut grants, &token, json!(LAUNCH_TOOLS))["error"]["message"],
            INVALID
        );
        assert_eq!(
            answer(&mut grants, &other, json!(LAUNCH_TOOLS))["result"]["command"],
            "db-mcp"
        );
        grants.clear();
        assert_eq!(
            answer(&mut grants, &other, json!(LAUNCH_TOOLS))["error"]["message"],
            INVALID
        );
    }
}
