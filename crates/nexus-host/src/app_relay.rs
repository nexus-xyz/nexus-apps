//! The built-in Nexus server for the agents of the desktop app's own service ([`crate::local`]), as the app gives it
//! to the agents it starts itself: the app's binary with `--mcp`, signed in as the app, and the app's socket for its
//! pull request tools and `request_secret` (`nexus_workbench::app_socket`).
//!
//! The service's agents outlive the app, and the app may come back as another process on another port. So the
//! service binds the socket and hands out the credentials, one per agent run, ended with the run. It passes each
//! granted call to the app over the app's connection ([`CALL_EVENT`]). The app answers it as a call on its own socket
//! (`nexus_workbench::app_socket::REPLY_CALL`). With no app connected, the call answers at once that the app is not
//! open.
//!
//! The agents also get the MCP servers from the app's Settings, with their keychain values, which the app hands the
//! service in its sign-in and again when Settings saves the list ([`AppTools::set_servers`]).
//!
//! They get the session's browser server too (`nexus_browser`), on the same socket with a credential of its own. Its
//! calls carry no `tools`; the app runs them on the session's browser tabs, with the rules of its own agents.
//!
//! A credential works only while the service is signed in to the same organization as the same person, as on the
//! app's own socket. When a run ends, the app hears it ([`RUN_ENDED_EVENT`]) and closes the run's card and files.

use std::collections::HashMap;
use std::io::Write as _;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak, mpsc};
use std::time::Duration;

use nexus_workbench::app_socket::{
    self, CALL_EVENT, Checked, Grant, Grants, Launchables, RUN_ENDED_EVENT, Relayed, read_request, relayed,
};
use nexus_workbench::app_tools::SECRET_TTL;
use nexus_workbench::event::SessionAgent;
use nexus_workbench::mcp_servers::{self, BROWSER, BROWSER_WAIT, Handed, REPLY_WAIT, SECRET_TOOLS, SECRET_WAIT};
use nexus_workbench::secret_files;
use serde_json::{Value, json};

use crate::server::Hub;

pub(crate) async fn launch_saved_provider(
    local: &nexus_workbench::provider::LocalProviders,
    request: &nexus_workbench::LaunchRequest,
    program: PathBuf,
    env: HashMap<String, String>,
    tools: Option<&AppTools>,
    files: &crate::agent_files::AgentFiles,
    check_owner: &(dyn Fn() -> Result<(), String> + Sync),
) -> Result<nexus_workbench::Launch, String> {
    let settings = nexus_desktop_core::provider_settings::read_for_launch().await?;
    let Handed { servers, values } = tools.map(|tools| lock(&tools.servers).clone()).unwrap_or_default();
    let mut native = nexus_workbench::local_provider::NativeMcp::default();
    let mut request = request.clone();
    files.apply_local_mcp(request.agent, &mut request.args, &mut native)?;
    nexus_workbench::local_provider::prepare(
        &request,
        program,
        env,
        nexus_workbench::local_provider::Source {
            settings: &settings,
            local,
            check_owner,
        },
        nexus_workbench::local_provider::Mcp {
            program: tools.map(|tools| tools.program.as_path()),
            servers: &servers,
            values: &values,
            native,
        },
        |run, launchable| tools?.relay.as_ref()?.grant_local_launch(run, launchable),
    )
    .await
}

/// What the service is signed in to now: its scope (`<gateway>|<org>`) and the sign-in's identity.
pub type Current = Arc<dyn Fn() -> (String, String) + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The app's socket, served by the service, and the calls waiting for the app's answer.
pub struct AppRelay {
    address: SocketAddr,
    grants: Mutex<Grants>,
    waiting: Mutex<HashMap<u64, mpsc::Sender<Value>>>,
    next: AtomicU64,
    stop: Arc<AtomicBool>,
    hub: Hub,
    current: Current,
    /// On a person's own server: where `request_secret`'s files go, and the files written there, by run. The app hands
    /// the value back in its answer, and the agent reads the file here. Unset on the app's own service,
    /// whose app writes them on its own computer.
    secret_files: std::sync::OnceLock<PathBuf>,
    written: Mutex<Vec<(String, PathBuf)>>,
}

impl AppRelay {
    /// Binds a port on 127.0.0.1 and answers on threads of its own, until [`AppRelay::close`].
    pub fn bind(hub: Hub, current: Current) -> Result<Arc<AppRelay>, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let relay = Arc::new(AppRelay {
            address,
            grants: Mutex::default(),
            waiting: Mutex::default(),
            next: AtomicU64::new(1),
            stop: stop.clone(),
            hub,
            current,
            secret_files: std::sync::OnceLock::new(),
            written: Mutex::default(),
        });
        let weak: Weak<AppRelay> = Arc::downgrade(&relay);
        std::thread::Builder::new()
            .name("nexus-host-app-socket".into())
            .spawn(move || {
                app_socket::serve(listener, stop, move |stream| {
                    if let Some(relay) = weak.upgrade() {
                        let _ = relay.answer(stream);
                    }
                });
            })
            .map_err(|error| error.to_string())?;
        Ok(relay)
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// A new credential for the run `run` of the session `thread`, in `scope`, signed in as `sign_in`.
    pub fn grant(&self, thread: &str, scope: &str, run: &str, sign_in: &str) -> String {
        lock(&self.grants).add(Grant {
            thread: thread.into(),
            scope: scope.into(),
            run: run.into(),
            sign_in: sign_in.into(),
        })
    }

    /// A new launch credential for the run `run`, which starts only `servers` (`nexus_desktop_mcp::launch`).
    pub fn grant_launch(&self, thread: &str, scope: &str, run: &str, sign_in: &str, servers: Launchables) -> String {
        let grant = Grant {
            thread: thread.into(),
            scope: scope.into(),
            run: run.into(),
            sign_in: sign_in.into(),
        };
        lock(&self.grants).add_launch(grant, servers)
    }

    fn grant_local_launch(&self, run: &str, servers: Launchables) -> Option<(String, String)> {
        let mut grants = lock(&self.grants);
        if self.stop.load(Ordering::SeqCst) {
            return None;
        }
        let token = grants.add_local_launch(run, servers);
        Some((self.address.to_string(), token))
    }

    pub(crate) fn end_local_run(&self, run: &str) {
        lock(&self.grants).end_run(run);
    }

    /// On a person's own server: `request_secret`'s files go in `base` (see [`AppRelay::secret_files`]).
    pub fn write_secrets_in(&self, base: PathBuf) {
        let _ = std::fs::remove_dir_all(&base);
        let _ = self.secret_files.set(base);
    }

    /// The run `run` ended: its credentials go, and the app hears it.
    pub fn end_run(&self, run: &str) {
        lock(&self.written).retain(|(of, path)| {
            if of == run {
                secret_files::remove(path);
            }
            of != run
        });
        if lock(&self.grants).end_run(run) {
            self.hub.push(RUN_ENDED_EVENT, &json!({ "run": run }));
        }
    }

    /// The app's answer to a call ([`REPLY_CALL`]'s arguments). A late answer, after the call gave up, is dropped.
    pub fn answered(&self, args: &Value) -> Result<(), String> {
        let id = args.get("id").and_then(Value::as_u64).ok_or("Say which call.")?;
        if let Some(waiter) = lock(&self.waiting).remove(&id) {
            let _ = waiter.send(args.get("reply").cloned().unwrap_or(Value::Null));
        }
        Ok(())
    }

    /// Stops listening; every credential goes.
    pub fn close(&self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        lock(&self.grants).clear();
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(self.address);
    }

    /// One connection: its request, checked against the credentials, and the app's answer.
    fn answer(&self, mut stream: TcpStream) -> Result<(), String> {
        let input = read_request(&stream)?;
        let (scope, sign_in) = (self.current)();
        // A launch credential's call is answered here, from the run's servers: the service holds their values.
        let checked = lock(&self.grants).check(&input, &scope, &sign_in);
        let reply = match checked {
            Checked::Answered(reply) => reply,
            Checked::Call(grant) => self.ask(grant, &input),
        };
        writeln!(stream, "{reply}").map_err(|error| error.to_string())
    }

    /// Passes a granted call to the app and waits for its answer, as long as the built-in server waits.
    fn ask(&self, grant: Grant, input: &Value) -> Value {
        let message = &input["message"];
        if self.hub.clients() == 0 {
            return relayed(
                message,
                Err("no Nexus app is connected to the background service".into()),
            );
        }
        let wait = if input["tools"] == SECRET_TOOLS {
            SECRET_WAIT + Duration::from_secs(20)
        } else if input["tools"].is_null() {
            // The browser's: the person may be asked to allow it.
            BROWSER_WAIT + Duration::from_secs(20)
        } else {
            REPLY_WAIT
        };
        let (thread, run) = (grant.thread.clone(), grant.run.clone());
        let call = Relayed {
            id: self.next.fetch_add(1, Ordering::SeqCst),
            grant,
            input: json!({ "tools": input["tools"], "message": message }),
        };
        let (waiter, answer) = mpsc::channel();
        lock(&self.waiting).insert(call.id, waiter);
        self.hub.push(CALL_EVENT, &call.payload());
        let reply = answer.recv_timeout(wait);
        lock(&self.waiting).remove(&call.id);
        match reply {
            Ok(reply) => self.deliver(&thread, &run, reply),
            Err(_) => relayed(message, Err("it took too long".into())),
        }
    }

    /// The app's answer as the agent gets it. A `request_secret` value the app handed back is written to a file here
    /// first, and the agent gets its path; it goes when the run ends or after `SECRET_TTL`. Without a folder for them
    /// (the app's own service), such a value is dropped and the call fails.
    fn deliver(&self, thread: &str, run: &str, mut reply: Value) -> Value {
        let Some((name, value)) = secret_files::take_value(&mut reply) else {
            return reply;
        };
        let failed = |text: String| json!({"jsonrpc": "2.0", "id": reply["id"], "result": {"content": [{"type": "text", "text": text}], "isError": true}});
        let Some(base) = self.secret_files.get() else {
            return failed("This Nexus host keeps no secret files.".into());
        };
        let path = match secret_files::write(base, thread, &value) {
            Ok(path) => path,
            Err(error) => return failed(error),
        };
        lock(&self.written).push((run.to_string(), path.clone()));
        let expired = path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SECRET_TTL);
            secret_files::remove(&expired);
        });
        secret_files::provided(&reply, &name, &path)
    }
}

impl Drop for AppRelay {
    fn drop(&mut self) {
        self.close();
    }
}

/// The servers for the service's agents: the app's binary for the built-in server, the socket the service serves for
/// it, and the servers from the app's Settings, as the app last handed them ([`AppTools::set_servers`]).
pub struct AppTools {
    local_memory: bool,
    pub program: PathBuf,
    pub relay: Option<Arc<AppRelay>>,
    servers: Mutex<Handed>,
}

/// What a run starts with: arguments to put before the agent's own, variables for its environment, and the enabled
/// servers' names (the session's attribution).
#[derive(Default)]
pub struct Launched {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub names: Vec<String>,
    /// For an agent that takes its servers in its session: the launcher's entries ([`mcp_servers::session_entries`]).
    pub session: mcp_servers::SessionMcp,
}

impl AppTools {
    pub fn new(program: PathBuf, relay: Arc<AppRelay>) -> AppTools {
        Self::with_relay(program, Some(relay))
    }

    pub(crate) fn with_relay(program: PathBuf, relay: Option<Arc<AppRelay>>) -> AppTools {
        AppTools {
            local_memory: false,
            program,
            relay,
            servers: Mutex::default(),
        }
    }

    pub(crate) fn with_local_memory(mut self, local_memory: bool) -> Self {
        self.local_memory = local_memory;
        self
    }

    /// Settings' servers and their values, as the app hands them in its sign-in and when Settings saves the list. Runs
    /// started from now on get them; a running agent keeps its own until it starts again, as in the app.
    pub fn set_servers(&self, servers: Handed) {
        *lock(&self.servers) = servers;
    }

    pub(crate) fn configure_local(&self, launch: &mut nexus_workbench::Launch, agent: SessionAgent) {
        let Handed { servers, values } = lock(&self.servers).clone();
        mcp_servers::configure_local(
            launch,
            agent,
            Some(&self.program),
            &servers,
            &values,
            |run, launchable| self.relay.as_ref()?.grant_local_launch(run, launchable),
        );
    }

    /// What the run `run` of `thread` starts with, as the app's own agents start: the built-in server on the sign-in
    /// `gateway` and `token`, with a new credential for the socket (unless Settings has a server named nexus), then
    /// Settings' servers, then the session's browser server with a credential of its own (unless Settings has a server
    /// named `nexus_browser`).
    pub fn launch(
        &self,
        agent: SessionAgent,
        thread: &str,
        scope: &str,
        run: &str,
        gateway: &str,
        token: &str,
    ) -> Launched {
        let Some(relay) = self.relay.as_ref() else {
            return Launched::default();
        };
        let in_session = mcp_servers::in_session(agent);
        if !in_session && !nexus_workbench::harness::harness(agent).app_tools().offered() {
            return Launched::default();
        }
        let Handed {
            mut servers,
            mut values,
        } = lock(&self.servers).clone();
        let program = self.program.to_string_lossy();
        let address = relay.address().to_string();
        let sign_in = nexus_workbench::session_choice::sign_in_identity(token);
        if let Some((mut server, mut own)) = mcp_servers::builtin(&servers, &program, gateway, token) {
            let credential = relay.grant(thread, scope, run, &sign_in);
            mcp_servers::with_app_tools(&mut server, &mut own, &address, &credential);
            if self.local_memory {
                mcp_servers::with_shared_memory(&mut server, &mut own, agent, thread);
            }
            servers.insert(0, server);
            values.extend(own);
        }
        if !mcp_servers::named(&servers, BROWSER) {
            let credential = relay.grant(thread, scope, run, &sign_in);
            if let Some((server, own)) = mcp_servers::browser(&servers, &program, &address, &credential) {
                servers.push(server);
                values.extend(own);
            }
        }
        // Claude Code starts each local server through the launcher, which keeps the token out of it (`mcp_launch`).
        let launched = mcp_servers::through_launcher_or_builtin(agent, &program, &servers, &values, |launchable| {
            let token = relay.grant_launch(thread, scope, run, &sign_in, launchable);
            Some((address.clone(), token))
        });
        let left_out = launched
            .as_ref()
            .map(|launched| launched.left_out.clone())
            .unwrap_or_default();
        let (args, env) = match launched {
            Some(launched) => (launched.args, launched.env),
            None => mcp_servers::launch(agent, &servers, &values),
        };
        let session = match mcp_servers::session(agent, &servers, &values) {
            Some(session) if in_session => {
                let token = relay.grant_launch(thread, scope, run, &sign_in, session.launchable.clone());
                mcp_servers::session_entries(&program, &address, &token, &session)
            }
            // The servers that did not start, for the session's notice.
            _ => mcp_servers::SessionMcp {
                servers: Vec::new(),
                skipped: left_out.clone(),
            },
        };
        let names = servers
            .iter()
            .filter(|server| server.enabled && !left_out.contains(&server.name))
            .map(|server| server.name.clone())
            .collect();
        Launched {
            args,
            env,
            names,
            session,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_workbench::app_socket::INVALID;
    use nexus_workbench::mcp_servers::{APP_ADDRESS_VAR, APP_TOKEN_VAR, PULL_REQUEST_TOOLS};

    fn signed_in(scope: &'static str, sign_in: &'static str) -> Current {
        Arc::new(move || (scope.to_string(), sign_in.to_string()))
    }

    /// A server's host writes the secret the app handed back, tells the agent only the file's path, and deletes it
    /// when the run ends.
    #[test]
    fn a_servers_secret_is_written_here_and_goes_with_its_run() {
        let base = std::env::temp_dir().join(format!("nh-secrets-{}", std::process::id()));
        let relay = AppRelay::bind(Hub::new(), signed_in("gw|acme", "me")).expect("bind");
        relay.write_secrets_in(base.clone());
        let reply = json!({"jsonrpc": "2.0", "id": 9, "result": {"content": [{"type": "text", "text": ""}], "isError": false},
            secret_files::VALUE_FIELD: {"name": "API key", "value": "sk-123"}});
        let delivered = relay.deliver("thr_1", "run_1", reply);
        assert!(delivered.get(secret_files::VALUE_FIELD).is_none());
        assert_eq!(delivered["result"]["isError"], false);
        let text = delivered["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .to_string();
        assert!(!text.contains("sk-123"), "{text}");
        let path = lock(&relay.written)[0].1.clone();
        assert!(text.contains(&path.display().to_string()), "{text}");
        assert_eq!(std::fs::read_to_string(&path).expect("file"), "sk-123");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        relay.end_run("run_1");
        assert!(!path.exists());
        assert!(lock(&relay.written).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The app's own service writes no secret files: a value handed back there is dropped, and the call fails.
    #[test]
    fn a_host_without_a_secrets_folder_keeps_no_value() {
        let relay = AppRelay::bind(Hub::new(), signed_in("gw|acme", "me")).expect("bind");
        let reply = json!({"jsonrpc": "2.0", "id": 3, secret_files::VALUE_FIELD: {"name": "k", "value": "v"}});
        let delivered = relay.deliver("thr_1", "run_1", reply);
        assert_eq!(delivered["result"]["isError"], true);
        assert!(!delivered.to_string().contains("\"v\""));
        assert!(lock(&relay.written).is_empty());
        // Any other answer passes as it is.
        let plain = json!({"jsonrpc": "2.0", "id": 4, "result": {"ok": true}});
        assert_eq!(relay.deliver("thr_1", "run_1", plain.clone()), plain);
    }

    fn call(address: SocketAddr, token: &str) -> Value {
        let request = json!({"token": token, "tools": PULL_REQUEST_TOOLS, "message": {"jsonrpc": "2.0", "id": 5}});
        let reply = app_socket::relay(&address, &request, REPLY_WAIT).expect("a reply");
        serde_json::from_str(reply.trim()).expect("json")
    }

    #[test]
    fn a_call_with_no_app_connected_says_so_at_once() {
        let relay = AppRelay::bind(Hub::new(), signed_in("gw|acme", "me")).expect("bind");
        let token = relay.grant("t1", "gw|acme", "ns_1", "me");
        let answer = call(relay.address(), &token);
        assert_eq!(answer["id"], 5);
        assert_eq!(answer["result"]["isError"], true);
        let text = answer["result"]["content"][0]["text"].as_str().unwrap_or_default();
        assert!(text.contains("no Nexus app is connected"), "{text}");
    }

    #[test]
    fn a_granted_call_goes_to_the_app_and_its_answer_comes_back() {
        let hub = Hub::new();
        let mut pushes = hub.subscribe();
        let relay = AppRelay::bind(hub, signed_in("gw|acme", "me")).expect("bind");
        let token = relay.grant("t1", "gw|acme", "ns_1", "me");
        let address = relay.address();
        let caller = std::thread::spawn(move || call(address, &token));
        // The app: it reads the push and answers it.
        let push = pushes.blocking_recv().expect("the call's push");
        let push: Value = serde_json::from_str(&push).expect("json");
        assert_eq!(push["event"], CALL_EVENT);
        assert!(
            push["payload"].get("token").is_none(),
            "the credential stays in the service"
        );
        let call = Relayed::from_payload(&push["payload"]).expect("a relayed call");
        assert_eq!((call.grant.thread.as_str(), call.grant.run.as_str()), ("t1", "ns_1"));
        assert_eq!(call.input["message"]["id"], 5);
        relay
            .answered(&call.reply(json!({"jsonrpc": "2.0", "id": 5, "result": {"ok": true}})))
            .expect("answered");
        assert_eq!(caller.join().expect("caller")["result"]["ok"], true);
    }

    #[test]
    fn a_credential_works_only_for_its_sign_in_and_ends_with_its_run() {
        let hub = Hub::new();
        let mut pushes = hub.subscribe();
        let relay = AppRelay::bind(hub, signed_in("gw|other", "me")).expect("bind");
        let token = relay.grant("t1", "gw|acme", "ns_1", "me");
        assert_eq!(call(relay.address(), &token)["error"]["message"], INVALID);
        assert_eq!(call(relay.address(), "made-up")["error"]["message"], INVALID);
        relay.end_run("ns_1");
        let push: Value = serde_json::from_str(&pushes.blocking_recv().expect("the end's push")).expect("json");
        assert_eq!(push, json!({"event": RUN_ENDED_EVENT, "payload": {"run": "ns_1"}}));
        // A run with no credential left says nothing more.
        relay.end_run("ns_1");
        assert!(pushes.try_recv().is_err());
    }

    #[test]
    fn an_agent_without_app_tools_gets_no_servers_secrets_or_grants() {
        let hub = Hub::new();
        let mut pushes = hub.subscribe();
        let relay = AppRelay::bind(hub, signed_in("gw|acme", "me")).expect("bind");
        let tools = AppTools::new("/opt/Nexus/nexus-desktop".into(), relay.clone());
        tools.set_servers(handed(
            json!([{"name": "db", "command": "db-mcp", "secrets": ["DB_TOKEN"]}]),
            json!({"db": {"DB_TOKEN": "fixture-secret"}}),
        ));
        for agent in SessionAgent::ALL.into_iter().filter(|&agent| {
            !nexus_workbench::harness::harness(agent).app_tools().offered() && !mcp_servers::in_session(agent)
        }) {
            let launch = tools.launch(agent, "thread", "gw|acme", "run", "https://gw", "test-token");
            assert!(launch.args.is_empty());
            assert!(launch.env.is_empty());
            assert!(launch.names.is_empty());
            assert_eq!(launch.session, mcp_servers::SessionMcp::default());
        }
        relay.end_run("run");
        assert!(pushes.try_recv().is_err(), "no grant to end");
    }

    /// agy takes its servers in `session/new` and writes their environment to disk: it gets only the launcher, the
    /// socket and a launch credential. The launcher gets each server, values included, from the service.
    #[test]
    fn antigravity_gets_launcher_entries_and_the_service_answers_the_launcher() {
        let sign_in = nexus_workbench::session_choice::sign_in_identity("test-token");
        let current: Current = Arc::new(move || ("gw|acme".to_string(), sign_in.clone()));
        let tools_relay = AppRelay::bind(Hub::new(), current).expect("bind");
        let tools = AppTools::new("/opt/Nexus/nexus-desktop".into(), tools_relay.clone());
        tools.set_servers(handed(
            json!([{"name": "db", "command": "db-mcp", "secrets": ["DB_TOKEN"]},
                   {"name": "docs", "url": "https://mcp.example.test"}]),
            json!({"db": {"DB_TOKEN": "fixture-secret"}}),
        ));
        let launch = tools.launch(
            SessionAgent::Antigravity,
            "thread",
            "gw|acme",
            "run",
            "https://gw",
            "test-token",
        );
        assert!(launch.args.is_empty() && launch.env.is_empty());
        assert_eq!(launch.session.skipped, ["docs"]);
        let entries = serde_json::to_string(&launch.session.servers).expect("json");
        assert!(!entries.contains("fixture-secret") && !entries.contains("test-token"));
        let names: Vec<_> = launch
            .session
            .servers
            .iter()
            .map(|server| server["name"].clone())
            .collect();
        assert_eq!(names, [json!("db"), json!("nexus"), json!(BROWSER)]);
        let token = launch.session.servers[0]["env"][1]["value"]
            .as_str()
            .expect("the launch credential");
        let ask = |name: &str| {
            let request = nexus_desktop_launch_request(token, name);
            let reply = app_socket::relay(&tools_relay.address(), &request, REPLY_WAIT).expect("reply");
            serde_json::from_str::<Value>(&reply).expect("json")
        };
        assert_eq!(ask("db")["result"]["env"]["DB_TOKEN"], "fixture-secret");
        let nexus = ask("nexus");
        assert_eq!(nexus["result"]["args"], json!(["--mcp"]));
        assert_eq!(nexus["result"]["env"]["NEXUS_MCP_TOKEN"], "test-token");
        tools_relay.end_run("run");
        assert_eq!(ask("db")["error"]["message"], INVALID);
    }

    /// The launcher's request (`nexus_desktop_mcp::launch::request`, which depends on this crate's workbench).
    fn nexus_desktop_launch_request(token: &str, name: &str) -> Value {
        json!({"token": token, "tools": app_socket::LAUNCH_TOOLS,
               "message": {"jsonrpc": "2.0", "id": 1, "method": "launch", "params": {"name": name}}})
    }

    /// A relay signed in as the sign-in of `token`, in the scope `gw|acme`.
    fn signed_in_as(token: &str) -> Current {
        let sign_in = nexus_workbench::session_choice::sign_in_identity(token);
        Arc::new(move || ("gw|acme".to_string(), sign_in.clone()))
    }

    /// The launcher's ask for the server `name`, on `launch_token`, as the relay answers it.
    fn ask_launch(relay: &AppRelay, launch_token: &str, name: &str) -> Value {
        let request = nexus_desktop_launch_request(launch_token, name);
        let reply = app_socket::relay(&relay.address(), &request, REPLY_WAIT).expect("reply");
        serde_json::from_str::<Value>(reply.trim()).expect("json")
    }

    fn env_value(env: &[(String, String)], name: &str) -> Option<String> {
        env.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone())
    }

    /// Claude's `--mcp-config=` argument as JSON.
    fn claude_config(args: &[String]) -> Value {
        assert_eq!(args.len(), 1, "{args:?}");
        serde_json::from_str(args[0].strip_prefix("--mcp-config=").expect("flag")).expect("json")
    }

    /// The entry Claude gets for a stdio server `name`: the launcher, with no variables of its own.
    fn launcher_entry(program: &str, name: &str) -> Value {
        json!({"type": "stdio", "command": program, "args": [format!("--mcp-launch={name}")], "env": {}})
    }

    #[test]
    fn the_agent_gets_the_built_in_server_with_its_credential_only_in_the_launch_answer() {
        let relay = AppRelay::bind(Hub::new(), signed_in_as("nx_tok")).expect("bind");
        let tools = AppTools::new("/opt/Nexus/nexus-desktop".into(), relay.clone());
        let Launched { args, env, names, .. } =
            tools.launch(SessionAgent::Claude, "t1", "gw|acme", "ns_1", "https://gw", "nx_tok");
        assert_eq!(names, vec!["nexus".to_string(), BROWSER.to_string()]);
        let config = claude_config(&args);
        assert_eq!(
            config["mcpServers"]["nexus"],
            launcher_entry("/opt/Nexus/nexus-desktop", "nexus")
        );
        assert_eq!(env_value(&env, APP_ADDRESS_VAR), Some(relay.address().to_string()));
        let launch_token = env_value(&env, APP_TOKEN_VAR).expect("a launch credential");
        assert!(!args[0].contains(&launch_token) && !args[0].contains("nx_tok"));
        assert!(!env.iter().any(|(_, value)| value == "nx_tok"), "{env:?}");

        // The launcher's answer has the built-in server's own start: the Gateway, the token and a tool credential.
        let nexus = ask_launch(&relay, &launch_token, "nexus");
        assert_eq!(nexus["result"]["args"], json!(["--mcp"]));
        assert_eq!(nexus["result"]["env"]["NEXUS_MCP_TOKEN"], "nx_tok");
        assert_eq!(nexus["result"]["env"][APP_ADDRESS_VAR], relay.address().to_string());
        let credential = nexus["result"]["env"][APP_TOKEN_VAR]
            .as_str()
            .expect("a tool credential")
            .to_string();
        assert_ne!(credential, launch_token);
        // The tool credential is the run's, for the sign-in of the token it starts on.
        let sign_in = nexus_workbench::session_choice::sign_in_identity("nx_tok");
        assert!(
            lock(&relay.grants)
                .authorize(&credential, "gw|acme", &sign_in)
                .is_some()
        );
    }

    fn handed(servers: Value, values: Value) -> Handed {
        serde_json::from_value(json!({ "servers": servers, "values": values })).expect("handed")
    }

    #[test]
    fn the_agent_gets_the_servers_the_app_handed_with_their_values_only_in_the_launch_answer() {
        let relay = AppRelay::bind(Hub::new(), signed_in_as("nx_tok")).expect("bind");
        let tools = AppTools::new("/opt/Nexus/nexus-desktop".into(), relay.clone());
        tools.set_servers(handed(
            json!([{"name": "db", "command": "db-mcp", "args": ["--ro"], "secrets": ["DB_TOKEN"]}]),
            json!({"db": {"DB_TOKEN": "s3cret"}}),
        ));
        let Launched { args, env, names, .. } =
            tools.launch(SessionAgent::Claude, "t1", "gw|acme", "ns_1", "https://gw", "nx_tok");
        assert_eq!(names, vec!["nexus".to_string(), "db".to_string(), BROWSER.to_string()]);
        let config = claude_config(&args);
        assert_eq!(
            config["mcpServers"]["db"],
            launcher_entry("/opt/Nexus/nexus-desktop", "db")
        );
        assert!(!args[0].contains("s3cret"));
        assert!(!env.iter().any(|(_, value)| value == "s3cret"), "{env:?}");
        let launch_token = env_value(&env, APP_TOKEN_VAR).expect("a launch credential");
        let db = ask_launch(&relay, &launch_token, "db");
        assert_eq!(db["result"]["command"], "db-mcp");
        assert_eq!(db["result"]["args"], json!(["--ro"]));
        assert_eq!(db["result"]["env"]["DB_TOKEN"], "s3cret");

        // Codex takes them as `-c` overrides, before its own options.
        let codex = tools.launch(SessionAgent::Codex, "t1", "gw|acme", "ns_2", "https://gw", "nx_tok");
        assert!(
            codex
                .args
                .iter()
                .any(|arg| arg.starts_with("mcp_servers.db={command=\"db-mcp\""))
        );

        // The person's own server named nexus wins over the built-in one, as in the app.
        tools.set_servers(handed(json!([{"name": "Nexus", "url": "https://mine/mcp"}]), json!({})));
        let mine = tools.launch(SessionAgent::Claude, "t1", "gw|acme", "ns_3", "https://gw", "nx_tok");
        assert_eq!(mine.names, vec!["Nexus".to_string(), BROWSER.to_string()]);
        let config = claude_config(&mine.args);
        assert_eq!(config["mcpServers"]["Nexus"]["type"], "http");
        assert!(config["mcpServers"].get("nexus").is_none());
        // No built-in server starts: the launch credential (the browser's) has none to give.
        let launch_token = env_value(&mine.env, APP_TOKEN_VAR).expect("a launch credential");
        assert_eq!(
            ask_launch(&relay, &launch_token, "nexus")["error"]["code"],
            json!(-32602)
        );

        // A new list replaces the old one for the next run.
        tools.set_servers(Handed::default());
        assert_eq!(
            tools
                .launch(SessionAgent::Claude, "t1", "gw|acme", "ns_4", "https://gw", "nx_tok")
                .names,
            vec!["nexus".to_string(), BROWSER.to_string()]
        );
    }

    #[test]
    fn the_agent_gets_the_browser_server_and_its_calls_wait_for_the_app() {
        use nexus_workbench::mcp_servers::{BROWSER_ADDRESS_VAR, BROWSER_ARG, BROWSER_TOKEN_VAR};

        let hub = Hub::new();
        let mut pushes = hub.subscribe();
        // The service is signed in as the token the run starts on.
        let relay = AppRelay::bind(hub, signed_in_as("nx_tok")).expect("bind");
        let tools = AppTools::new("/opt/Nexus/nexus-desktop".into(), relay.clone());
        let Launched { args, env, .. } =
            tools.launch(SessionAgent::Claude, "t1", "gw|acme", "ns_1", "https://gw", "nx_tok");
        let config = claude_config(&args);
        assert_eq!(
            config["mcpServers"][BROWSER],
            launcher_entry("/opt/Nexus/nexus-desktop", BROWSER)
        );
        assert!(env_value(&env, BROWSER_TOKEN_VAR).is_none(), "{env:?}");
        // The browser's own start, with its credential, is in the launch answer.
        let launch_token = env_value(&env, APP_TOKEN_VAR).expect("a launch credential");
        let browser = ask_launch(&relay, &launch_token, BROWSER);
        assert_eq!(browser["result"]["args"], json!([BROWSER_ARG]));
        assert_eq!(
            browser["result"]["env"][BROWSER_ADDRESS_VAR],
            relay.address().to_string()
        );
        let credential = browser["result"]["env"][BROWSER_TOKEN_VAR]
            .as_str()
            .expect("a credential")
            .to_string();
        assert_ne!(credential, launch_token, "a credential of its own");
        assert!(!args[0].contains(&credential));

        // A browser call names no tools: it goes to the app as one, and the app's answer comes back.
        let address = relay.address();
        let caller = std::thread::spawn(move || {
            let request = json!({"token": credential, "message": {"jsonrpc": "2.0", "id": 9, "method": "tools/list"}});
            let reply = app_socket::relay(&address, &request, REPLY_WAIT).expect("a reply");
            serde_json::from_str::<Value>(reply.trim()).expect("json")
        });
        // Bounded: a refused call pushes nothing, and its reply says why.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let push = loop {
            match pushes.try_recv() {
                Ok(push) => break push,
                Err(_) if caller.is_finished() || std::time::Instant::now() >= deadline => {
                    panic!("no call reached the app: {:?}", caller.join().ok())
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        let push: Value = serde_json::from_str(&push).expect("json");
        let call = Relayed::from_payload(&push["payload"]).expect("a relayed call");
        assert!(call.input["tools"].is_null(), "{}", call.input);
        assert_eq!((call.grant.thread.as_str(), call.grant.run.as_str()), ("t1", "ns_1"));
        relay
            .answered(&call.reply(json!({"jsonrpc": "2.0", "id": 9, "result": {"tools": []}})))
            .expect("answered");
        assert_eq!(caller.join().expect("caller")["result"]["tools"], json!([]));

        // The person's own server of that name wins, and no credential is made for it.
        tools.set_servers(handed(
            json!([{"name": "Nexus_Browser", "url": "https://mine/mcp"}]),
            json!({}),
        ));
        let mine = tools.launch(SessionAgent::Claude, "t1", "gw|acme", "ns_2", "https://gw", "nx_tok");
        assert_eq!(mine.names, vec!["nexus".to_string(), "Nexus_Browser".to_string()]);
        let launch_token = env_value(&mine.env, APP_TOKEN_VAR).expect("a launch credential");
        assert_eq!(
            ask_launch(&relay, &launch_token, BROWSER)["error"]["code"],
            json!(-32602)
        );
    }

    /// Claude Code passes its whole environment to a stdio server, so each one starts through the launcher, and no
    /// server's own value is in Claude's arguments or environment.
    #[test]
    fn claude_gets_launcher_entries_and_no_server_values() {
        let relay = AppRelay::bind(Hub::new(), signed_in_as("nx_tok")).expect("bind");
        let program = "/opt/Nexus/nexus-desktop";
        let tools = AppTools::new(program.into(), relay.clone());
        tools.set_servers(handed(
            json!([{"name": "db", "command": "db-mcp", "secrets": ["DB_TOKEN"]},
                   {"name": "docs", "url": "https://mcp.example.test", "secrets": ["Authorization"]}]),
            json!({"db": {"DB_TOKEN": "s3cret"}, "docs": {"Authorization": "Bearer hdr-value"}}),
        ));
        let Launched { args, env, .. } =
            tools.launch(SessionAgent::Claude, "t1", "gw|acme", "ns_1", "https://gw", "nx_tok");
        let config = claude_config(&args);
        let servers = config["mcpServers"].as_object().expect("servers");
        let mut names: Vec<_> = servers.keys().cloned().collect();
        names.sort();
        assert_eq!(names, ["db", "docs", "nexus", BROWSER]);
        for (name, entry) in servers {
            if name == "docs" {
                // The remote server keeps its header reference: nexus, db, docs are enabled servers 0, 1, 2.
                let remote = json!({"type": "http", "url": "https://mcp.example.test",
                                    "headers": {"Authorization": "${NEXUS_MCP_2_0}"}});
                assert_eq!(entry, &remote);
            } else {
                assert_eq!(entry, &launcher_entry(program, name), "{name}");
            }
        }
        let mut env_names: Vec<_> = env.iter().map(|(key, _)| key.as_str()).collect();
        env_names.sort();
        let mut expected = [APP_ADDRESS_VAR, APP_TOKEN_VAR, "NEXUS_MCP_2_0"];
        expected.sort();
        assert_eq!(env_names, expected);
        assert_eq!(env_value(&env, "NEXUS_MCP_2_0").as_deref(), Some("Bearer hdr-value"));
        let everything = format!("{args:?} {env:?}");
        assert!(
            !everything.contains("s3cret") && !everything.contains("nx_tok"),
            "{everything}"
        );

        // The launch answer for db has the server's own value, and the launcher removes Claude's credentials.
        let launch_token = env_value(&env, APP_TOKEN_VAR).expect("a launch credential");
        let db = ask_launch(&relay, &launch_token, "db");
        assert_eq!(db["result"]["command"], "db-mcp");
        assert_eq!(db["result"]["env"]["DB_TOKEN"], "s3cret");
        assert_eq!(db["result"]["strip_credentials"], "no_credentials");
        // Once: a second ask gets an error.
        let again = ask_launch(&relay, &launch_token, "db");
        assert!(again.get("result").is_none(), "{again}");
        assert!(
            again["error"]["message"].as_str().unwrap_or_default().contains("db"),
            "{again}"
        );
    }
}
