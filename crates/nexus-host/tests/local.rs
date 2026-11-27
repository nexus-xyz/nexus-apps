//! The desktop app's own background service, started as a child process the way the app starts it
//! (nexus-remote's `LocalService`, here with `nexus-host local`), its agents the stand-in `claude`, and Gateway a
//! stand-in on a loopback port. No network.
//!
//! - A turn outlives the app: the app's relay goes away mid-turn, the service finishes the turn alone, and a new relay
//!   (the app launched again) reattaches to the same service and reads the whole log, each event once, in order.
//! - Agents start on the app's Gateway sign-in, handed over in the hello.
//! - Stopping the service ends its agents and their children: nothing is left running.
//! - The socket is private, a wrong token is refused, and a server's host refuses the app's sign-in.
//! - The service says where a session's folder is, for the app's "Open in".
//! - An idle service of another version is replaced, and a service with no app stops by itself.
//! - Its agents get the built-in Nexus server, and their calls of the app's tools reach the app and come back.
//! - Given the app's device key, it syncs as the app's computer; without one, it syncs nothing.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nexus_remote::client::{self, HelloError, Pushed};
use nexus_remote::local::LocalService;
use nexus_remote::machine::{Backoff, Remote, Sink, Status};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

const APP_TOKEN: &str = "nxc_the_apps_own_token";

/// Gateway's stand-in: every call answered with the same small JSON; what was asked, with its `Authorization`, kept.
struct Gateway {
    url: String,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl Gateway {
    async fn start() -> Gateway {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let url = format!("http://{}", listener.local_addr().unwrap_or_else(|e| panic!("{e}")));
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        tokio::spawn({
            let seen = seen.clone();
            async move {
                while let Ok((stream, _)) = listener.accept().await {
                    tokio::spawn(answer(stream, seen.clone()));
                }
            }
        });
        Gateway { url, seen }
    }

    /// The `Authorization` of each `POST /v1/agents/sessions`, the call that opens an agent's Nexus session.
    fn session_opens(&self) -> Vec<String> {
        self.calls("POST /v1/agents/sessions ")
    }

    /// The `Authorization` of each call whose request line starts with `start`.
    fn calls(&self, start: &str) -> Vec<String> {
        self.seen
            .lock()
            .map(|seen| {
                seen.iter()
                    .filter(|(line, _)| line.starts_with(start))
                    .map(|(_, auth)| auth.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn answer(mut stream: tokio::net::TcpStream, seen: Arc<Mutex<Vec<(String, String)>>>) {
    let mut request = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let Ok(read) = stream.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            return;
        }
        request.extend_from_slice(&chunk[..read]);
        if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&request[..head_end]).to_string();
    let header = |name: &str| {
        head.lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_string())
            })
            .unwrap_or_default()
    };
    let length: usize = header("content-length").parse().unwrap_or(0);
    while request.len() < head_end + length {
        let Ok(read) = stream.read(&mut chunk).await else {
            return;
        };
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
    }
    let line = head.lines().next().unwrap_or_default().to_string();
    if let Ok(mut seen) = seen.lock() {
        seen.push((line, header("authorization")));
    }
    let body = r#"{"id":"ns_local","data":[]}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// The app's service in `root`, on `gateway`, signed in as the app is.
fn service(root: &Path, gateway: &str, idle_exit: Duration, version: Option<&str>) -> LocalService {
    let scope = format!("{gateway}|acme");
    let sign_in = json!({ "gateway": gateway, "token": APP_TOKEN, "scope": scope });
    let memory = root.join("memory");
    let _ = std::fs::create_dir_all(&memory);
    let _ = std::fs::create_dir_all(root.join("home"));
    LocalService {
        program: PathBuf::from(env!("CARGO_BIN_EXE_nexus-host")),
        command: vec!["local".into()],
        data: root.join("data"),
        // Short: a Unix socket's path has a length limit.
        socket: root.join("run").join("s.sock"),
        worktrees: Some(root.join("wt")),
        reviews: Some(root.join("rv")),
        idle_exit,
        app_program: None,
        env: vec![
            ("HOME".into(), root.join("home").to_string_lossy().into_owned()),
            // No login shell to ask for `PATH`: the child never starts the developer's shell (as `nexus-remote`'s probe tests).
            ("SHELL".into(), String::new()),
            ("NEXUS_CLAUDE_BIN".into(), env!("CARGO_BIN_EXE_fake-claude").into()),
            ("FAKE_CLAUDE_DIR".into(), memory.to_string_lossy().into_owned()),
            ("FAKE_CLAUDE_SLOW_MS".into(), "2500".into()),
        ],
        version: version.map(String::from),
        sign_in: Arc::new(move || Some(sign_in.clone())),
    }
}

/// Stops the service when the test ends, passed or not.
struct StopService(LocalService);

impl Drop for StopService {
    fn drop(&mut self) {
        if let Some(pid) = self.0.running() {
            let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGTERM);
            for _ in 0..300 {
                if self.0.running().is_none() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Whether `pid` is a live process (a zombie waiting to be reaped is not).
fn alive(pid: i32) -> bool {
    if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
        return false;
    }
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')),
        Err(_) => true,
    }
}

enum Heard {
    Push(Pushed),
    Status(Status),
}

struct Channel(mpsc::UnboundedSender<Heard>);

impl Sink for Channel {
    fn push(&self, push: &Pushed) {
        let _ = self.0.send(Heard::Push(push.clone()));
    }
    fn status(&self, status: &Status) {
        let _ = self.0.send(Heard::Status(status.clone()));
    }
}

fn fast() -> Backoff {
    Backoff {
        first: Duration::from_millis(50),
        max: Duration::from_millis(200),
        healthy: Duration::from_secs(60),
    }
}

/// The webview's copy of one session, as in tests/remote.rs.
#[derive(Default)]
struct View {
    thread: String,
    events: BTreeMap<i64, Value>,
    duplicates: Vec<i64>,
    statuses: Vec<Status>,
}

impl View {
    fn last(&self) -> i64 {
        self.events.keys().next_back().copied().unwrap_or(0)
    }

    fn add(&mut self, event: &Value) {
        let seq = event["seq"].as_i64().unwrap_or_default();
        if self.events.insert(seq, event.clone()).is_some() {
            self.duplicates.push(seq);
        }
    }

    fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::Status(status) => self.statuses.push(status),
            Heard::Push(push) => {
                if push.event == "wb:event"
                    && push.payload["threadId"] == json!(self.thread)
                    && push.payload["seq"].as_i64().unwrap_or_default() > self.last()
                {
                    self.add(&push.payload);
                }
            }
        }
    }

    fn kinds(&self) -> Vec<&str> {
        self.events
            .values()
            .filter_map(|e| e["event"]["kind"].as_str())
            .collect()
    }

    fn turn_ends(&self) -> usize {
        self.kinds().iter().filter(|k| **k == "turnEnd").count()
    }

    fn connected(&self) -> usize {
        self.statuses
            .iter()
            .filter(|s| matches!(s, Status::Connected { .. }))
            .count()
    }
}

async fn until(heard: &mut mpsc::UnboundedReceiver<Heard>, view: &mut View, done: impl Fn(&View) -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        while !done(view) {
            let next = heard.recv().await.unwrap_or_else(|| panic!("the sink closed"));
            view.heard(next);
        }
    })
    .await;
    assert!(
        waited.is_ok(),
        "timed out; have {:?}, statuses {:?}",
        view.kinds(),
        view.statuses
    );
}

async fn call(remote: &Remote, name: &str, args: Value) -> Value {
    remote.call(name, args).await.unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[tokio::test]
async fn a_service_process_cannot_open_another_builds_session_store() {
    let root = common::root("svc-owner");
    let data = root.join("data");
    let owner = nexus_workbench::owner::StoreOwner::take(&data).expect("desktop owns its store");
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        nexus_workbench::process::async_command(env!("CARGO_BIN_EXE_nexus-host"))
            .env("SHELL", "")
            .args(["local", "--data"])
            .arg(&data)
            .arg("--socket")
            .arg(root.join("service.sock"))
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("second process exits promptly")
    .expect("run service");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Another Nexus app or background service owns"));
    assert!(
        !data.join("workbench.sqlite3").exists(),
        "refused before opening the database"
    );
    drop(owner);
    let replacement = nexus_workbench::owner::StoreOwner::take(&data).expect("failed child releases all locks");
    drop(replacement);
    std::fs::remove_dir_all(root).expect("remove fixture");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_outlives_the_app_and_the_service_leaves_nothing_behind() {
    let root = common::root("svc");
    let repo = root.join("repo");
    let _ = std::fs::create_dir_all(&repo);
    let gateway = Gateway::start().await;
    let local = service(&root, &gateway.url, Duration::from_secs(600), None);
    let _stop = StopService(local.clone());

    // The app launches: it starts the service and connects.
    let (tx, mut heard) = mpsc::unbounded_channel();
    let app = Remote::start(Arc::new(local.clone()), Arc::new(Channel(tx)), fast());
    let mut view = View::default();
    until(&mut heard, &mut view, |v| v.connected() == 1).await;
    let pid = local.running().unwrap_or_else(|| panic!("no service runs"));

    let project = call(&app, "wb_project_add", json!({ "path": repo })).await;
    let created = call(
        &app,
        "wb_thread_create",
        json!({
            "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "slow keepchild", "images": [], "worktree": null,
        }),
    )
    .await;
    view.thread = created["id"].as_str().unwrap_or_default().to_string();
    let thread = view.thread.clone();
    // The app's "Open in" finds the session's folder on this computer.
    let folder = call(&app, "host_folder", json!({ "threadId": thread })).await;
    assert_eq!(folder, project["path"]);
    let snapshot = call(&app, "wb_thread_events", json!({ "threadId": thread, "afterSeq": 0 })).await;
    for event in snapshot.as_array().into_iter().flatten() {
        view.add(event);
    }
    until(&mut heard, &mut view, |v| v.kinds().contains(&"text")).await;
    assert_eq!(view.turn_ends(), 0, "the turn ended before the app quit");

    // The app quits mid-turn: its connection goes, the service and the turn go on.
    app.close();
    drop(heard);
    let before_quit = view.events.clone();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(local.running(), Some(pid));

    // The app launches again: a new relay and an empty webview reattach to the same service.
    let (tx, mut heard) = mpsc::unbounded_channel();
    let app = Remote::start(Arc::new(local.clone()), Arc::new(Channel(tx)), fast());
    let mut again = View {
        thread: thread.clone(),
        ..View::default()
    };
    until(&mut heard, &mut again, |v| v.connected() == 1).await;
    assert_eq!(
        local.running(),
        Some(pid),
        "the service was started over, not reattached"
    );
    let snapshot = call(&app, "wb_thread_events", json!({ "threadId": thread, "afterSeq": 0 })).await;
    for event in snapshot.as_array().into_iter().flatten() {
        again.add(event);
    }
    until(&mut heard, &mut again, |v| v.turn_ends() == 1).await;

    // Nothing lost or shown twice: what the first launch saw is the start of the log, and the log is gapless.
    assert!(again.duplicates.is_empty(), "duplicates: {:?}", again.duplicates);
    for (seq, event) in &before_quit {
        assert_eq!(again.events.get(seq), Some(event), "event {seq} changed");
    }
    let seqs: Vec<i64> = again.events.keys().copied().collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");
    let whole = call(&app, "wb_thread_events", json!({ "threadId": thread, "afterSeq": 0 })).await;
    let whole: BTreeMap<i64, Value> = whole
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e["seq"].as_i64() <= Some(again.last()))
        .map(|e| (e["seq"].as_i64().unwrap_or_default(), e))
        .collect();
    assert_eq!(again.events, whole);

    // The agent started on the app's own Gateway sign-in.
    assert_eq!(gateway.session_opens(), vec![format!("Bearer {APP_TOKEN}")]);
    // The app handed over no device key, so the service never asked who it syncs as.
    assert!(gateway.calls("GET /v1/auth/whoami").is_empty());

    // Stopping the service (the switch turned off, or a sign-out) ends the agent and what it started.
    let pids = std::fs::read_to_string(repo.join("keepchild.pids")).unwrap_or_default();
    let pids: Vec<i32> = pids.split_whitespace().filter_map(|p| p.parse().ok()).collect();
    assert_eq!(pids.len(), 2, "the agent did not start its child");
    assert!(pids.iter().all(|pid| alive(*pid)), "{pids:?} should run until the stop");
    app.close();
    assert_eq!(local.stop().await, Ok(true));
    assert_eq!(local.running(), None);
    let gone = tokio::time::timeout(Duration::from_secs(10), async {
        while pids.iter().any(|pid| alive(*pid)) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(gone.is_ok(), "left running: {pids:?}");
    // `stop` returns once the service lets go of its lock, the last thing `serve_as` does; the process then drops its
    // runtime and exits. Wait for that exit, within a deadline, rather than read it in the same instant.
    let exited = tokio::time::timeout(Duration::from_secs(10), async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(exited.is_ok(), "the service {pid} is still running after its stop");
    assert!(!local.socket.exists());
    drop(_stop);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_this_person_with_the_token_gets_in() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = common::root("svc-lock");
    let gateway = Gateway::start().await;
    let local = service(&root, &gateway.url, Duration::from_secs(600), None);
    let _stop = StopService(local.clone());
    local.start().await.unwrap_or_else(|e| panic!("{e}"));

    let mode = |path: &Path| {
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0)
    };
    assert_eq!(mode(&local.socket), 0o600);
    assert_eq!(mode(local.socket.parent().unwrap_or(&root)), 0o700);
    assert_eq!(mode(&local.data.join("token")), 0o600);

    let stream = tokio::net::UnixStream::connect(&local.socket)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let refused = client::open(Box::new(stream), "not-the-token-not-the-token-not-the-token", None).await;
    assert!(
        matches!(&refused, Err(HelloError::Failed(error)) if error == "Wrong token."),
        "{:?}",
        refused.err()
    );

    // A server's host never takes the app's sign-in: it has its own device token.
    let server = common::start("svc-srv");
    let stream = tokio::net::UnixStream::connect(&server.socket)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let sign_in = json!({ "gateway": gateway.url, "token": APP_TOKEN, "scope": "gw|org" });
    let refused = client::open(Box::new(stream), common::TOKEN, Some(&sign_in)).await;
    assert!(
        matches!(&refused, Err(HelloError::Failed(error)) if error.contains("nexus-host login")),
        "{:?}",
        refused.err()
    );
    let _ = server.stop.send(true);
    let _ = std::fs::remove_dir_all(&server.root);

    assert_eq!(local.stop().await, Ok(true));
    drop(_stop);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_older_service_is_replaced_and_one_with_no_app_stops_by_itself() {
    let root = common::root("svc-idle");
    let gateway = Gateway::start().await;
    // The app is another version than the running service: a service it finds idle is started over with its own.
    let local = service(&root, &gateway.url, Duration::from_secs(2), Some("0.0.0-the-app"));
    let _stop = StopService(local.clone());
    local.start().await.unwrap_or_else(|e| panic!("{e}"));
    let first = local.running().unwrap_or_else(|| panic!("no service runs"));

    let (tx, mut heard) = mpsc::unbounded_channel();
    let app = Remote::start(Arc::new(local.clone()), Arc::new(Channel(tx)), fast());
    let mut view = View::default();
    until(&mut heard, &mut view, |v| v.connected() == 1).await;
    assert!(view.statuses.contains(&Status::Updating), "{:?}", view.statuses);
    let second = local
        .running()
        .unwrap_or_else(|| panic!("no service runs after the update"));
    assert_ne!(second, first);
    // Once: the build here is still not the app's version, and it is kept rather than replaced in a loop.
    assert!(
        matches!(app.status(), Status::Connected { update: Some(ref v), .. } if v == "0.0.0-the-app"),
        "{:?}",
        app.status()
    );

    // The app goes, and nothing is busy: the service stops by itself.
    app.close();
    let stopped = tokio::time::timeout(Duration::from_secs(15), async {
        while local.running().is_some() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(stopped.is_ok(), "the idle service kept running");
    drop(_stop);
    let _ = std::fs::remove_dir_all(&root);
}

/// Waits up to 20 seconds for the stand-in `claude` to write what it was started with.
async fn launched(record: &Path) -> Value {
    let read = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(record) {
                return serde_json::from_str::<Value>(&text).unwrap_or_else(|e| panic!("{e}"));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    read.unwrap_or_else(|_| panic!("the agent did not start"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn its_agents_reach_the_apps_tools_through_it() {
    use nexus_workbench::app_socket::{self, CALL_EVENT, REPLY_CALL, Relayed};
    use nexus_workbench::mcp_servers::{APP_ADDRESS_VAR, APP_TOKEN_VAR, PULL_REQUEST_TOOLS, REPLY_WAIT};

    let root = common::root("svc-tools");
    let repo = root.join("repo");
    let _ = std::fs::create_dir_all(&repo);
    let gateway = Gateway::start().await;
    let record = root.join("launch.json");
    let mut local = service(&root, &gateway.url, Duration::from_secs(600), None);
    local.app_program = Some(PathBuf::from("/opt/nexus/nexus-desktop"));
    local
        .env
        .push(("FAKE_LAUNCH_RECORD".into(), record.to_string_lossy().into_owned()));
    local.env.push(("FAKE_RECORD_SECRETS".into(), "1".into()));
    let _stop = StopService(local.clone());

    let (tx, mut heard) = mpsc::unbounded_channel();
    let app = Remote::start(Arc::new(local.clone()), Arc::new(Channel(tx)), fast());
    let mut view = View::default();
    until(&mut heard, &mut view, |v| v.connected() == 1).await;
    let project = call(&app, "wb_project_add", json!({ "path": repo })).await;
    let created = call(
        &app,
        "wb_thread_create",
        json!({
            "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "hello", "images": [], "worktree": null,
        }),
    )
    .await;
    let thread = created["id"].as_str().unwrap_or_default().to_string();

    // The agent starts with the built-in server: the app's binary, and the service's socket in its environment only.
    let started = launched(&record).await;
    let config = started["args"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|arg| arg.contains("\"nexus\""))
        .and_then(|arg| arg.strip_prefix("--mcp-config="))
        .map(|config| serde_json::from_str::<Value>(config).unwrap_or_else(|e| panic!("{e}")))
        .unwrap_or_else(|| panic!("no built-in server in {:?}", started["args"]));
    assert_eq!(config["mcpServers"]["nexus"]["command"], "/opt/nexus/nexus-desktop");
    assert_eq!(config["mcpServers"]["nexus"]["args"], json!(["--mcp-launch=nexus"]));
    let address: std::net::SocketAddr = started["env"][APP_ADDRESS_VAR]
        .as_str()
        .and_then(|address| address.parse().ok())
        .unwrap_or_else(|| panic!("no app socket in the agent's environment"));
    assert!(address.ip().is_loopback());
    // The agent's variable is the launch credential. The built-in server's tool credential is in the launch answer.
    let launch_token = started["env"][APP_TOKEN_VAR].as_str().unwrap_or_default().to_string();
    assert!(!launch_token.is_empty());
    let launch = json!({"token": launch_token, "tools": app_socket::LAUNCH_TOOLS,
                        "message": {"jsonrpc": "2.0", "id": 1, "method": "launch", "params": {"name": "nexus"}}});
    let launched = tokio::task::spawn_blocking(move || app_socket::relay(&address, &launch, REPLY_WAIT))
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
    let launched: Value = serde_json::from_str(launched.trim()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(launched["result"]["args"], json!(["--mcp"]), "{launched}");
    let credential = launched["result"]["env"][APP_TOKEN_VAR]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!credential.is_empty() && credential != launch_token, "{launched}");

    // The built-in server relays a pull request tool's call: the service passes it to the app, and the app's answer
    // comes back.
    let message = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "watch_pull_request"}});
    let request = json!({"token": credential, "tools": PULL_REQUEST_TOOLS, "message": message});
    let asked = tokio::task::spawn_blocking(move || app_socket::relay(&address, &request, REPLY_WAIT));
    let relayed = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match heard.recv().await {
                Some(Heard::Push(push)) if push.event == CALL_EVENT => return Relayed::from_payload(&push.payload),
                Some(_) => {}
                None => return None,
            }
        }
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| panic!("the app was not asked"));
    assert_eq!(relayed.grant.thread, thread);
    assert_eq!(relayed.grant.scope, format!("{}|acme", gateway.url));
    assert_eq!(relayed.input["message"], message);
    let answer = json!({"jsonrpc": "2.0", "id": 9, "result": {"content": [{"type": "text", "text": "watching"}]}});
    call(&app, REPLY_CALL, relayed.reply(answer.clone())).await;
    let reply = asked
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(serde_json::from_str::<Value>(reply.trim()).ok(), Some(answer));

    // A credential the service did not hand out gets nothing from the app.
    let forged = json!({"token": "forged", "tools": PULL_REQUEST_TOOLS, "message": message});
    let refused = tokio::task::spawn_blocking(move || app_socket::relay(&address, &forged, REPLY_WAIT))
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(refused.contains("invalid or expired"), "{refused}");

    app.close();
    assert_eq!(local.stop().await, Ok(true));
    drop(_stop);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn given_the_apps_device_key_it_syncs_as_this_computer() {
    use base64::Engine as _;

    let root = common::root("svc-sync");
    let repo = root.join("repo");
    let _ = std::fs::create_dir_all(&repo);
    let gateway = Gateway::start().await;
    let device = nexus_client::e2e::DeviceKey::generate();
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&device.secret_bytes()[..]);
    let mut local = service(&root, &gateway.url, Duration::from_secs(600), None);
    let sign_in = json!({
        "gateway": gateway.url, "token": APP_TOKEN, "scope": format!("{}|acme", gateway.url), "deviceKey": key,
    });
    local.sign_in = Arc::new(move || Some(sign_in.clone()));
    let _stop = StopService(local.clone());

    let (tx, mut heard) = mpsc::unbounded_channel();
    let app = Remote::start(Arc::new(local.clone()), Arc::new(Channel(tx)), fast());
    let mut view = View::default();
    until(&mut heard, &mut view, |v| v.connected() == 1).await;
    let project = call(&app, "wb_project_add", json!({ "path": repo })).await;
    call(
        &app,
        "wb_thread_create",
        json!({
            "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "hello", "images": [], "worktree": null,
        }),
    )
    .await;

    // Its sync connects on the app's sign-in: Gateway is asked who that credential is. The stand-in's answer is not a
    // person, so nothing more is sent.
    let asked = tokio::time::timeout(Duration::from_secs(20), async {
        while gateway.calls("GET /v1/auth/whoami").is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(asked.is_ok(), "the service never connected its sync");
    assert_eq!(gateway.calls("GET /v1/auth/whoami")[0], format!("Bearer {APP_TOKEN}"));
    // The content keys' store is the app's, in its data folder.
    assert!(local.data.join("session-sync-keys.sqlite3").exists());

    app.close();
    assert_eq!(local.stop().await, Ok(true));
    drop(_stop);
    let _ = std::fs::remove_dir_all(&root);
}
