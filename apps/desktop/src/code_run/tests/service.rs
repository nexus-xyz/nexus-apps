//! "Keep agents running when Nexus quits" (`service.rs`) with a temp store and no network.
//!
//! The setting off and the policy lock need nothing more. The tests with a stand-in service run nexus-host's own
//! binary (`nexus-host local`, the service the app runs) and its stand-in `claude`, built from this workspace's
//! `crates/nexus-host`, so they are ignored unless both are named:
//!
//! ```sh
//! cargo build -p nexus-host --bins
//! NEXUS_HOST_BIN=…/target/debug/nexus-host NEXUS_FAKE_CLAUDE_BIN=…/target/debug/fake-claude \
//!   cargo test service -- --include-ignored
//! ```

use std::io::{Read as _, Write as _};

use nexus_remote::local::LocalService;
use nexus_workbench::event::ThreadStatus;

use super::*;
use crate::owner::Owner;

#[cfg(target_os = "linux")]
mod first_connection;
mod handoff;

/// The stand-in service's sign-in token.
const TOKEN: &str = "nxc_the_apps_own_token";
const LOCAL_SCOPE: &str = "|";

/// A fresh folder for one test, short: a Unix socket's path has a length limit.
///
/// In memory (`/dev/shm`) when the system has it. The stand-in service syncs its new store to disk 4 times before it
/// listens. On a CI node whose disk other jobs keep busy, those syncs took longer than the 20 seconds a service has to
/// start. These tests check the hand-over, not the disk.
fn root(name: &str) -> PathBuf {
    let shm = PathBuf::from("/dev/shm");
    let base = if shm.is_dir() && tempfile_in(&shm) {
        shm
    } else {
        std::env::temp_dir()
    };
    let root = base.join(format!("ngs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).expect("a temporary folder");
    root
}

/// Whether this process can make a folder in `dir`.
fn tempfile_in(dir: &Path) -> bool {
    let probe = dir.join(format!("ngs-probe-{}", std::process::id()));
    let made = std::fs::create_dir_all(&probe).is_ok();
    let _ = std::fs::remove_dir(&probe);
    made
}

/// What each thread of process `pid` waits in, from `/proc`: its name, its kernel wait and its system call number
/// (`fsync` is 74 and `fdatasync` 75 on x86_64). Empty off Linux.
fn threads_of(pid: i32) -> String {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return String::new();
    };
    tasks
        .flatten()
        .map(|task| {
            let read = |file: &str| {
                std::fs::read_to_string(task.path().join(file))
                    .map(|text| text.trim().to_string())
                    .unwrap_or_else(|_| "?".into())
            };
            let syscall = read("syscall");
            let number = syscall.split_whitespace().next().unwrap_or("?").to_string();
            format!("{} wchan={} syscall={number}", read("comm"), read("wchan"))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The loops on the default settings, with the switch as `keep_running` says now.
fn switch(keep_running: Arc<dyn Fn() -> bool + Send + Sync>) -> Loops {
    Loops {
        keep_running,
        ..quick(Duration::from_secs(600))
    }
}

/// A built binary, named in `var`. The ignored tests run only with it.
fn built(var: &str) -> PathBuf {
    let path = std::env::var_os(var).map(PathBuf::from);
    path.filter(|path| path.is_file())
        .unwrap_or_else(|| panic!("{var} names no built binary: see this file's first lines"))
}

/// Gateway's stand-in on a loopback port: every call answered with the same small JSON. Its address.
fn gateway() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let url = format!("http://{}", listener.local_addr().expect("its address"));
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let mut buffer = [0u8; 16384];
            let _ = stream.read(&mut buffer);
            let body = r#"{"id":"ns_local","data":[]}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
}

/// The stand-in service in `root`: nexus-host's binary in `local` mode, on `gateway`, its agent the stand-in `claude`.
fn stand_in(root: &Path, gateway: &str, scope: &str) -> LocalService {
    let memory = root.join("memory");
    let _ = std::fs::create_dir_all(&memory);
    let _ = std::fs::create_dir_all(root.join("home"));
    let sign_in = json!({ "gateway": gateway, "token": TOKEN, "scope": scope });
    LocalService {
        // Through `sh`, so the log ends with how the service ended: a signal (a kill, a crash) writes nothing itself.
        program: PathBuf::from("/bin/sh"),
        command: vec![
            "-c".into(),
            r#""$@"; echo "nexus-host ended with status $?" >&2"#.into(),
            "sh".into(),
            built("NEXUS_HOST_BIN").to_string_lossy().into_owned(),
            "local".into(),
        ],
        data: root.join("data"),
        socket: root.join("s.sock"),
        worktrees: Some(root.join("wt")),
        reviews: Some(root.join("rv")),
        idle_exit: Duration::from_secs(600),
        app_program: None,
        env: vec![
            ("HOME".into(), root.join("home").to_string_lossy().into_owned()),
            (
                "NEXUS_CLAUDE_BIN".into(),
                built("NEXUS_FAKE_CLAUDE_BIN").to_string_lossy().into_owned(),
            ),
            ("FAKE_CLAUDE_DIR".into(), memory.to_string_lossy().into_owned()),
            // Only capture the service's synthetic app grant. Other inherited secrets stay redacted.
            ("FAKE_RECORD_MCP_APP_TOKEN".into(), "1".into()),
            ("FAKE_CLAUDE_SLOW_MS".into(), "3000".into()),
        ],
        version: None,
        sign_in: Arc::new(move || Some(sign_in.clone())),
    }
}

/// Stops the stand-in service when the test ends, passed or not, by its pid.
struct StopService(LocalService);

impl Drop for StopService {
    fn drop(&mut self) {
        let local = self.0.clone();
        let _ = runtime().block_on(async move { local.stop().await });
    }
}

/// The session's events, as the windows read them from the store.
fn kinds(data: &Path, thread: &str) -> Vec<String> {
    let Ok(store) = Store::open(&data.join("workbench.sqlite3")) else {
        return Vec::new();
    };
    store
        .events_after(thread, 0)
        .unwrap_or_default()
        .iter()
        .filter_map(|event| serde_json::to_value(&event.event).ok())
        .filter_map(|event| event["kind"].as_str().map(String::from))
        .collect()
}

fn turn_ends(data: &Path, thread: &str) -> usize {
    kinds(data, thread).iter().filter(|kind| *kind == "turnEnd").count()
}

fn status(data: &Path, thread: &str) -> Option<ThreadStatus> {
    let store = Store::open(&data.join("workbench.sqlite3")).ok()?;
    store.thread(thread).ok().flatten().map(|record| record.summary.status)
}

/// What the stand-in service left behind, for a failure's message: its log, its lock's holder and its folder.
fn report(local: &LocalService) -> String {
    let log = std::fs::read_to_string(local.log()).unwrap_or_else(|error| format!("(no log: {error})"));
    let files: Vec<String> = std::fs::read_dir(&local.data)
        .map(|dir| {
            dir.flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    format!(
        "service pid {:?} [{}]; socket {} ({} bytes, exists {}); data {files:?}; log:\n{log}",
        local.running(),
        local.running().map(threads_of).unwrap_or_default(),
        local.socket.display(),
        local.socket.as_os_str().len(),
        local.socket.exists(),
    )
}

/// The `--mcp-config=` JSON in the arguments a stand-in `claude` recorded.
fn mcp_config(started: &Value) -> Value {
    let config = started["args"]
        .as_array()
        .and_then(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .find_map(|arg| arg.strip_prefix("--mcp-config="))
        })
        .expect("an --mcp-config argument");
    serde_json::from_str(config).expect("the config is JSON")
}

/// The answer to the launch of server `name`, asked as the launcher asks: with the launch credential and the socket's
/// address that the recorded agent has in its environment.
fn launch_answer(started: &Value, name: &str) -> Value {
    use nexus_desktop_mcp::mcp::{APP_ADDRESS_VAR, APP_TOKEN_VAR};

    let address: std::net::SocketAddr = started["env"][APP_ADDRESS_VAR]
        .as_str()
        .and_then(|address| address.parse().ok())
        .expect("the socket's address in the agent's environment");
    let token = started["env"][APP_TOKEN_VAR]
        .as_str()
        .expect("the launch credential in the agent's environment");
    let request = json!({"token": token, "tools": nexus_workbench::app_socket::LAUNCH_TOOLS,
                         "message": {"jsonrpc": "2.0", "id": 1, "method": "launch", "params": {"name": name}}});
    let reply = nexus_desktop_mcp::app_socket::relay(&address, &request, Duration::from_secs(10)).expect("a reply");
    serde_json::from_str(reply.trim()).expect("json")
}

/// The next event the runner sends that `pick` takes, within 20 seconds; `local` says why on a timeout.
fn next<T>(
    events: &mut UnboundedReceiver<CodeEvent>,
    local: &LocalService,
    pick: impl Fn(CodeEvent) -> Option<T>,
) -> T {
    next_with_timeout(events, local, Duration::from_secs(20), pick)
}

fn next_with_timeout<T>(
    events: &mut UnboundedReceiver<CodeEvent>,
    local: &LocalService,
    timeout: Duration,
    pick: impl Fn(CodeEvent) -> Option<T>,
) -> T {
    let until = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < until {
        match events.try_recv() {
            Ok(event) => {
                if let CodeEvent::Failed(error) | CodeEvent::StartFailed { error, .. } = &event {
                    eprintln!("runner: {error}");
                }
                if let Some(found) = pick(event) {
                    return found;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    panic!("the runner did not send the event in time: {}", report(local));
}

#[test]
fn the_switch_off_keeps_sessions_here_and_starts_no_service() {
    let root = root("off");
    let data = root.join("data");
    let (runner, _events) = CodeRunner::start_with(data.clone(), GATEWAY, "g|o", switch(Arc::new(|| false)), |_| {
        panic!("no service with the switch off")
    })
    .expect("the store opens")
    .expect("free, so taken");
    assert!(crate::service::store_held(&data), "this build holds the store's lock");
    assert!(runner.api.remote().is_none());
    // Quitting stops this build's agents as before: no service is left.
    runner.shutdown();
    drop(runner);
    assert!(!crate::service::store_held(&data));
    assert!(!data.join("run").join("host.lock").exists(), "no service ever ran");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_policy_lock_keeps_the_switch_off() {
    let on = Settings {
        keep_agents_running: Some(true),
        ..Settings::default()
    };
    // `AllowLocalCache` off: the switch saved on counts as off.
    let keep_running = Arc::new(move || crate::service::wanted(&on, false));
    let root = root("policy");
    let data = root.join("data");
    let (runner, _events) = CodeRunner::start_with(data.clone(), GATEWAY, "g|o", switch(keep_running), |_| {
        panic!("no service while the policy keeps nothing on this computer")
    })
    .expect("the store opens")
    .expect("free, so taken");
    assert!(runner.api.remote().is_none());
    assert!(crate::service::store_held(&data));
    drop(runner);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_nexus_app_holding_the_store_itself_starts_no_second_owner() {
    let root = root("inproc");
    let data = root.join("data");
    // Another app with its switch off holds the lock in its own process.
    let app = StoreOwner::take(&data).expect("the Nexus app's hold");
    let local = LocalService {
        program: PathBuf::from("/nonexistent/nexus-host"),
        command: vec!["local".into()],
        data: data.clone(),
        socket: root.join("s.sock"),
        worktrees: None,
        reviews: None,
        idle_exit: Duration::from_secs(1),
        app_program: None,
        env: Vec::new(),
        version: None,
        sign_in: Arc::new(|| None),
    };
    let started = CodeRunner::start_with(data.clone(), GATEWAY, "g|o", switch(Arc::new(|| true)), |_| Ok(local));
    assert!(matches!(started, Ok(None)), "read-only, and Take over offered");
    let owner = Owner::Unknown.tried(Err(Refused::Busy), std::time::Instant::now());
    assert!(owner.can_take_over());
    drop(app);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "needs NEXUS_HOST_BIN and NEXUS_FAKE_CLAUDE_BIN: see this file's first lines"]
fn a_service_that_owns_the_store_leaves_this_build_read_only() {
    let root = root("busy");
    let data = root.join("data");
    let gateway = gateway();
    let local = stand_in(&root, &gateway, &format!("{gateway}|acme"));
    let _stop = StopService(local.clone());
    // Started by another app, say: it holds the store's lock.
    let start = local.clone();
    runtime()
        .block_on(async move { start.start().await })
        .unwrap_or_else(|error| panic!("the stand-in service starts: {error}; {}", report(&local)));
    assert!(local.running().is_some());
    // The switch off here: this build tries the lock, is refused, and reads only.
    let started = CodeRunner::start_with(data.clone(), GATEWAY, "g|o", switch(Arc::new(|| false)), |_| {
        panic!("no service with the switch off")
    });
    assert!(matches!(started, Ok(None)));
    let owner = Owner::Unknown.tried(Err(Refused::Busy), std::time::Instant::now());
    assert_eq!(owner, Owner::Elsewhere { waited: false });
    assert!(owner.can_take_over());
    let pid = local.running();
    drop(started);
    assert_eq!(local.running(), pid, "this build never stops the service");
    drop(_stop);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "needs NEXUS_HOST_BIN and NEXUS_FAKE_CLAUDE_BIN: see this file's first lines"]
fn a_turn_goes_on_after_quit_and_the_next_launch_takes_it_back() {
    let root = root("hand");
    let data = root.join("data");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("a project folder");
    let gateway = gateway();
    let scope = format!("{gateway}|acme");
    let local = stand_in(&root, &gateway, &scope);
    let _stop = StopService(local.clone());
    let keep = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let keep_running: Arc<dyn Fn() -> bool + Send + Sync> = {
        let keep = keep.clone();
        Arc::new(move || keep.load(std::sync::atomic::Ordering::SeqCst))
    };

    // Launch with the switch on: no lock here, the service starts and owns the store.
    let service = local.clone();
    let (runner, mut events) =
        CodeRunner::start_with(data.clone(), &gateway, &scope, switch(keep_running.clone()), |_| {
            Ok(service)
        })
        .expect("the runner starts")
        .expect("no other owner");
    assert!(runner.api.remote().is_some());
    runner.add_project(repo.clone(), None);
    let project = next(&mut events, &local, |event| match event {
        CodeEvent::ProjectAdded { id, .. } => Some(id),
        _ => None,
    });
    let pid = local.running().expect("the service runs");
    assert!(crate::service::store_held(&data), "the service holds the store's lock");
    runner.create(
        "draft-0".into(),
        NewSession {
            project_id: project,
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Default.as_str().to_string(),
            effort: None,
            via: Via::Nexus,
            provider: None,
            worktree: false,
            review: None,
            base: None,
            text: "slow".into(),
            attachments: Default::default(),
        },
    );
    let thread = next(&mut events, &local, |event| match event {
        CodeEvent::Created { id, .. } => Some(id),
        _ => None,
    });
    assert!(
        wait_for(Duration::from_secs(20), || matches!(
            status(&data, &thread),
            Some(ThreadStatus::Running)
        )),
        "the turn runs in the service: {:?}",
        kinds(&data, &thread)
    );
    assert_eq!(turn_ends(&data, &thread), 0, "the turn ended before the quit");

    // Quit mid-turn: the hand-over. The connection closes; the service and its agent go on.
    runner.shutdown();
    drop(runner);
    drop(events);
    assert_eq!(local.running(), Some(pid), "the service outlives the quit");
    assert!(
        wait_for(Duration::from_secs(20), || turn_ends(&data, &thread) == 1),
        "the turn ended in the service, with no app: {:?}",
        kinds(&data, &thread)
    );

    // The next launch: the take-back. The same service, its session as it left it, and new work goes there.
    let service = local.clone();
    let (runner, mut events) =
        CodeRunner::start_with(data.clone(), &gateway, &scope, switch(keep_running), |_| Ok(service))
            .expect("the runner starts again")
            .expect("the service is reached, not refused");
    assert_eq!(local.running(), Some(pid), "reattached, not started over");
    runner.send(thread.clone(), "again".into(), Default::default(), false);
    assert!(
        wait_for(Duration::from_secs(20), || turn_ends(&data, &thread) == 2),
        "a message after the take-back runs in the service: {:?}",
        kinds(&data, &thread)
    );
    while events.try_recv().is_ok() {}

    // The switch turned off since the launch: quitting stops the service and its agents.
    keep.store(false, std::sync::atomic::Ordering::SeqCst);
    runner.shutdown();
    drop(runner);
    assert_eq!(local.running(), None, "the service stopped with the quit");
    assert!(
        !crate::service::store_held(&data),
        "the store is free for the next owner"
    );
    drop(_stop);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "needs NEXUS_HOST_BIN and NEXUS_FAKE_CLAUDE_BIN: see this file's first lines"]
fn outside_access_and_the_pull_request_watch_reach_the_services_sessions() {
    use nexus_desktop_mcp::outside::{Host as _, tools};

    let root = root("tools");
    let data = root.join("data");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("a project folder");
    let gateway = gateway();
    let scope = format!("{gateway}|acme");
    let local = stand_in(&root, &gateway, &scope);
    let _stop = StopService(local.clone());
    let service = local.clone();
    let (runner, mut events) =
        CodeRunner::start_with(data.clone(), &gateway, &scope, switch(Arc::new(|| true)), |_| {
            Ok(service)
        })
        .expect("the runner starts")
        .expect("no other owner");
    runner.add_project(repo.clone(), None);
    let project = next(&mut events, &local, |event| match event {
        CodeEvent::ProjectAdded { id, .. } => Some(id),
        _ => None,
    });
    runner.create(
        "draft-0".into(),
        NewSession {
            project_id: project,
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Default.as_str().to_string(),
            effort: None,
            via: Via::Nexus,
            provider: None,
            worktree: false,
            review: None,
            base: None,
            text: "hello".into(),
            attachments: Default::default(),
        },
    );
    let thread = next(&mut events, &local, |event| match event {
        CodeEvent::Created { id, .. } => Some(id),
        _ => None,
    });
    assert!(wait_for(Duration::from_secs(20), || turn_ends(&data, &thread) == 1));

    // Outside access: its tools list, show and message the service's session.
    let outside = crate::outside::GpuiOutside {
        api: runner.api.clone(),
        runtime: runtime(),
        org: Arc::new(|| "acme".into()),
        client: Arc::new(|| None),
        changed: Arc::new(|| {}),
    };
    let listed = outside.call(tools::LIST, &Map::new()).expect("list_sessions");
    assert!(
        listed["sessions"]
            .as_array()
            .is_some_and(|sessions| sessions.iter().any(|session| session["id"] == json!(thread))),
        "{listed}"
    );
    let mut args = Map::new();
    args.insert("session_id".into(), json!(thread));
    let shown = outside.call(tools::SHOW, &args).expect("show_session");
    assert!(
        shown["turns"].as_array().is_some_and(|turns| !turns.is_empty()),
        "{shown}"
    );
    args.insert("text".into(), json!("again"));
    outside.call(tools::SEND, &args).expect("send_message");
    assert!(
        wait_for(Duration::from_secs(20), || turn_ends(&data, &thread) == 2),
        "the message reached the service's agent: {:?}",
        kinds(&data, &thread)
    );

    // The pull request watch: it reads the sessions from the store and tells the agent through the service.
    let watched = crate::pr_watch::Service {
        remote: runner.api.remote().cloned().expect("over the service"),
        data: data.clone(),
        scope: Arc::new({
            let scope = scope.clone();
            move || scope.clone()
        }),
    };
    assert!(watched.threads().expect("the sessions").iter().any(|t| t.id == thread));
    assert_eq!(watched.folder(&thread).ok(), Some(repo.clone()));
    runtime()
        .block_on(watched.queue(&thread, "the checks failed"))
        .expect("queued through the service");
    assert!(
        wait_for(Duration::from_secs(20), || turn_ends(&data, &thread) == 3),
        "the watch's message reached the agent: {:?}",
        kinds(&data, &thread)
    );
    runtime()
        .block_on(watched.settle_merged(&thread, 7, false))
        .expect("wb_settle_merged answers");
    runtime()
        .block_on(watched.wake(&thread))
        .expect("not snoozed: nothing to do");

    // Signing out stops the service with its agents.
    runner.signed_out();
    assert!(
        wait_for(Duration::from_secs(20), || local.running().is_none()),
        "the service stopped at sign-out"
    );
    drop(runner);
    drop(_stop);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "needs NEXUS_HOST_BIN and NEXUS_FAKE_CLAUDE_BIN: see this file's first lines"]
fn the_services_agents_reach_this_builds_tools_through_it() {
    use nexus_desktop_mcp::mcp::{APP_ADDRESS_VAR, APP_TOKEN_VAR, relay_to_app};

    let root = root("builtin");
    let data = root.join("data");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("a project folder");
    let gateway = gateway();
    let scope = format!("{gateway}|acme");
    let record = root.join("launch.json");
    let mut local = stand_in(&root, &gateway, &scope);
    local.app_program = Some(PathBuf::from("/opt/nexus/nexus-desktop-gpui"));
    local
        .env
        .push(("FAKE_LAUNCH_RECORD".into(), record.to_string_lossy().into_owned()));
    let _stop = StopService(local.clone());
    let service = local.clone();
    let (runner, mut events) =
        CodeRunner::start_with(data.clone(), &gateway, &scope, switch(Arc::new(|| true)), |_| {
            Ok(service)
        })
        .expect("the runner starts")
        .expect("no other owner");
    runner.add_project(repo.clone(), None);
    let project = next(&mut events, &local, |event| match event {
        CodeEvent::ProjectAdded { id, .. } => Some(id),
        _ => None,
    });
    runner.create(
        "draft-0".into(),
        NewSession {
            project_id: project,
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Default.as_str().to_string(),
            effort: None,
            via: Via::Nexus,
            provider: None,
            worktree: false,
            review: None,
            base: None,
            text: "hello".into(),
            attachments: Default::default(),
        },
    );
    let thread = next(&mut events, &local, |event| match event {
        CodeEvent::Created { id, .. } => Some(id),
        _ => None,
    });

    // The service's agent starts the built-in server through the launcher, and has the service's socket and a launch
    // credential only in its environment.
    assert!(
        wait_for(Duration::from_secs(20), || record.exists()),
        "the agent did not start: {}",
        report(&local)
    );
    let started: Value = serde_json::from_str(&std::fs::read_to_string(&record).expect("the record")).expect("json");
    assert_eq!(
        mcp_config(&started)["mcpServers"]["nexus"],
        json!({"type": "stdio", "command": "/opt/nexus/nexus-desktop-gpui", "args": ["--mcp-launch=nexus"], "env": {}})
    );
    let address: std::net::SocketAddr = started["env"][APP_ADDRESS_VAR]
        .as_str()
        .and_then(|address| address.parse().ok())
        .expect("the service's socket");
    let launch_token = started["env"][APP_TOKEN_VAR]
        .as_str()
        .expect("its launch credential")
        .to_string();
    assert!(
        started["env"].get("NEXUS_MCP_TOKEN").is_none(),
        "the built-in server's token is not in the agent's environment"
    );

    // The tool credential comes in the launch answer, with the server's own token.
    let launched = launch_answer(&started, "nexus");
    assert_eq!(launched["result"]["strip_credentials"], "no_credentials", "{launched}");
    assert_eq!(launched["result"]["env"]["NEXUS_MCP_TOKEN"], TOKEN, "{launched}");
    let token = launched["result"]["env"][APP_TOKEN_VAR]
        .as_str()
        .expect("its tool credential")
        .to_string();
    assert_ne!(token, launch_token);
    assert!(
        !started["args"].to_string().contains(&token) && !started["env"].to_string().contains(&token),
        "the tool credential is not in the agent's arguments or environment"
    );
    // The launch credential only launches, and only once per server.
    assert!(launch_answer(&started, "nexus")["error"].is_object());
    let refused = relay_to_app(
        &address,
        &launch_token,
        &json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
                "params": {"name": "unwatch_pull_request", "arguments": {}}}),
    );
    assert!(refused.to_string().contains("invalid or expired"), "{refused}");

    // A pull request tool: the service passes the call to this build, whose watch answers it.
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                      "params": {"name": "unwatch_pull_request", "arguments": {}}});
    let answer = relay_to_app(&address, &token, &call);
    assert_eq!(answer["id"], 1);
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    assert!(answer.to_string().contains("not watching"), "{answer}");

    // `request_secret`: the card opens in this build, and the value goes to a private file on this computer.
    let ask = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                     "params": {"name": "request_secret", "arguments": {"name": "Stripe key", "reason": "To deploy."}}});
    let asking = std::thread::spawn(move || relay_to_app(&address, &token, &ask));
    assert!(
        wait_for(Duration::from_secs(20), || runner.secret_request(&thread).is_some()),
        "the card did not open"
    );
    let card = runner.secret_request(&thread).expect("the card");
    futures::executor::block_on(runner.answer_secret(card.id, Some("sk_test_value".into())))
        .expect("answered")
        .expect("written");
    let answer = asking.join().expect("the call");
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    assert!(
        !answer.to_string().contains("sk_test_value"),
        "the value never reaches the agent"
    );
    let text = answer["result"]["content"][0]["text"].as_str().unwrap_or_default();
    let path = text
        .split_once("in the file ")
        .and_then(|(_, rest)| rest.split_once(", readable"))
        .map(|(path, _)| PathBuf::from(path))
        .unwrap_or_else(|| panic!("no file in {text}"));
    assert_eq!(std::fs::read_to_string(&path).ok().as_deref(), Some("sk_test_value"));

    drop(runner);
    drop(_stop);
    let _ = std::fs::remove_dir_all(root);
}

/// Settings saved while the service runs the sessions reach it: a new session's worktree goes under the folder chosen
/// now, and Open in editor finds the session's folder through the service.
#[test]
#[ignore = "needs NEXUS_HOST_BIN and NEXUS_FAKE_CLAUDE_BIN: see this file's first lines"]
fn the_service_takes_new_worktree_folders_and_opens_its_sessions_in_an_editor() {
    let root = root("roots");
    let data = root.join("data");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("a project folder");
    let git = |args: &[&str]| {
        let done = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .current_dir(&repo)
            .output()
            .expect("git runs");
        assert!(
            done.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&done.stderr)
        );
    };
    git(&["init", "--quiet", "--initial-branch", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").expect("a file");
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "one"]);
    let gateway = gateway();
    let scope = format!("{gateway}|acme");
    let local = stand_in(&root, &gateway, &scope);
    let _stop = StopService(local.clone());
    let service = local.clone();
    let (runner, mut events) =
        CodeRunner::start_with(data.clone(), &gateway, &scope, switch(Arc::new(|| true)), |_| {
            Ok(service)
        })
        .expect("the runner starts")
        .expect("no other owner");
    runner.add_project(repo.clone(), None);
    let project = next(&mut events, &local, |event| match event {
        CodeEvent::ProjectAdded { id, .. } => Some(id),
        _ => None,
    });

    // Settings saves new folders (`Workspace::settings_saved`): the service makes the next worktree there.
    let chosen = root.join("chosen-wt");
    let settings = nexus_desktop_core::settings::Settings {
        worktrees_dir: Some(chosen.to_string_lossy().into_owned()),
        reviews_dir: Some(root.join("chosen-rv").to_string_lossy().into_owned()),
        ..Default::default()
    };
    runner.apply_settings(&settings);
    // The hand-over is a call of its own: give it a moment before the session that depends on it.
    std::thread::sleep(Duration::from_millis(300));
    runner.create(
        "draft-0".into(),
        NewSession {
            project_id: project,
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Default.as_str().to_string(),
            effort: None,
            via: Via::Nexus,
            provider: None,
            worktree: true,
            review: None,
            base: None,
            text: "hello".into(),
            attachments: Default::default(),
        },
    );
    let thread = next(&mut events, &local, |event| match event {
        CodeEvent::Created { id, .. } => Some(id),
        _ => None,
    });

    // Open in editor asks the service where the session's folder is.
    let folder = futures::executor::block_on(runner.editor_folder(&thread, None))
        .unwrap_or_else(|_| Err("the runner stopped".into()))
        .unwrap_or_else(|error| panic!("the session's folder: {error}: {}", report(&local)));
    assert!(
        folder.starts_with(&chosen),
        "the worktree goes under the folder Settings chose: {}",
        folder.display()
    );
    assert!(folder.is_dir(), "{}", folder.display());

    drop(runner);
    drop(_stop);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "needs NEXUS_HOST_BIN and NEXUS_FAKE_CLAUDE_BIN: see this file's first lines"]
fn the_services_agents_get_the_mcp_servers_from_settings() {
    use nexus_desktop_core::secrets::Vault;
    use nexus_desktop_core::settings::McpServer;

    let root = root("mcp");
    let data = root.join("data");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("a project folder");
    let gateway = gateway();
    let scope = format!("{gateway}|acme");
    let record = root.join("launch.json");
    let mut local = stand_in(&root, &gateway, &scope);
    local.app_program = Some(PathBuf::from("/opt/nexus/nexus-desktop-gpui"));
    local
        .env
        .push(("FAKE_LAUNCH_RECORD".into(), record.to_string_lossy().into_owned()));
    // The sign-in as this build hands it (`GpuiHost::sign_in`): Settings' servers, with the keychain's values.
    let server = |name: &str, enabled: bool, command: Option<&str>, url: Option<&str>, secrets: &[&str]| McpServer {
        name: name.into(),
        enabled,
        command: command.map(String::from),
        args: Vec::new(),
        url: url.map(String::from),
        secrets: secrets.iter().map(|secret| secret.to_string()).collect(),
        tool_timeout_sec: None,
        plugin: None,
    };
    let servers = [
        server("db", true, Some("db-mcp"), None, &["DB_TOKEN"]),
        server("off", false, Some("off-mcp"), None, &["OFF_TOKEN"]),
    ];
    let vault = Vault::Memory(Default::default());
    vault
        .set(
            nexus_desktop_mcp::mcp::VAULT_ACCOUNT,
            r#"{"db":{"DB_TOKEN":"s3cret_db"},"off":{"OFF_TOKEN":"s3cret_off"}}"#,
        )
        .expect("the stand-in keychain");
    let mcp = serde_json::to_value(nexus_desktop_mcp::mcp::for_service(&servers, &vault)).expect("json");
    let sign_in = json!({ "gateway": gateway, "token": TOKEN, "scope": scope, "mcp": mcp });
    local.sign_in = Arc::new(move || Some(sign_in.clone()));
    let _stop = StopService(local.clone());
    let service = local.clone();
    let (runner, mut events) =
        CodeRunner::start_with(data.clone(), &gateway, &scope, switch(Arc::new(|| true)), |_| {
            Ok(service)
        })
        .expect("the runner starts")
        .expect("no other owner");
    runner.add_project(repo.clone(), None);
    let project = next(&mut events, &local, |event| match event {
        CodeEvent::ProjectAdded { id, .. } => Some(id),
        _ => None,
    });
    let session = |text: &str| NewSession {
        project_id: project.clone(),
        agent: SessionAgent::Claude,
        model: None,
        permission_mode: PermissionMode::Default.as_str().to_string(),
        effort: None,
        via: Via::Nexus,
        provider: None,
        worktree: false,
        review: None,
        base: None,
        text: text.into(),
        attachments: Default::default(),
    };
    let started = |local: &LocalService| -> Value {
        assert!(
            wait_for(Duration::from_secs(20), || record.exists()),
            "the agent did not start: {}",
            report(local)
        );
        serde_json::from_str(&std::fs::read_to_string(&record).expect("the record")).expect("json")
    };

    // The service's agent starts the enabled server and the built-in one through the launcher. The value is only in the
    // launch answer, not in the agent's arguments or environment.
    runner.create("draft-0".into(), session("hello"));
    let first = started(&local);
    let args = first["args"].to_string();
    let launcher = |name: &str| {
        json!({"type": "stdio", "command": "/opt/nexus/nexus-desktop-gpui",
               "args": [format!("--mcp-launch={name}")], "env": {}})
    };
    let config = mcp_config(&first);
    assert_eq!(config["mcpServers"]["db"], launcher("db"), "{args}");
    assert_eq!(
        config["mcpServers"]["nexus"],
        launcher("nexus"),
        "the built-in server too: {args}"
    );
    assert!(
        config["mcpServers"].get("off").is_none(),
        "a server switched off stays out: {args}"
    );
    assert!(
        !args.contains("db-mcp") && !args.contains("${DB_TOKEN}") && !args.contains("s3cret"),
        "neither the command nor a value goes in the arguments: {args}"
    );
    assert!(first["env"].get("DB_TOKEN").is_none() && first["env"].get("OFF_TOKEN").is_none());
    assert!(
        !first["env"].to_string().contains("s3cret"),
        "a value is not in the environment"
    );
    let db = launch_answer(&first, "db");
    assert_eq!(db["result"]["command"], "db-mcp", "{db}");
    assert_eq!(db["result"]["env"]["DB_TOKEN"], "s3cret_db", "{db}");
    assert_eq!(db["result"]["strip_credentials"], "no_credentials", "{db}");
    assert!(
        launch_answer(&first, "off")["error"].is_object(),
        "a server switched off does not launch"
    );

    // Settings saves another list (`apply_settings`): the next agent the service starts gets it.
    std::fs::remove_file(&record).expect("the first record goes");
    let handing = runner
        .hand_mcp_servers(vec![server("docs", true, None, Some("https://docs.example/mcp"), &[])])
        .expect("the service runs the sessions");
    runtime().block_on(handing).expect("handed");
    runner.create("draft-0".into(), session("again"));
    let second = started(&local);
    let args = second["args"].to_string();
    assert!(args.contains("https://docs.example/mcp"), "{args}");
    assert!(!args.contains("db-mcp"), "the old list is gone: {args}");
    assert!(mcp_config(&second)["mcpServers"].get("db").is_none());
    assert!(second["env"].get("DB_TOKEN").is_none());
    assert!(
        !second["env"].to_string().contains("s3cret"),
        "a value is not in the environment"
    );
    assert!(
        launch_answer(&second, "db")["error"].is_object(),
        "the old list's server does not launch"
    );

    drop(runner);
    drop(_stop);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "needs NEXUS_HOST_BIN and NEXUS_FAKE_CLAUDE_BIN: see this file's first lines"]
fn the_services_agents_drive_this_builds_session_browser() {
    use nexus_desktop_mcp::browser::{ADDRESS, NAME, TOKEN as BROWSER_TOKEN};

    let root = root("browser");
    let data = root.join("data");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("a project folder");
    let gateway = gateway();
    let scope = format!("{gateway}|acme");
    let record = root.join("launch.json");
    let mut local = stand_in(&root, &gateway, &scope);
    local.app_program = Some(PathBuf::from("/opt/nexus/nexus-desktop-gpui"));
    local
        .env
        .push(("FAKE_LAUNCH_RECORD".into(), record.to_string_lossy().into_owned()));
    let _stop = StopService(local.clone());
    // The windows' stand-in: each browser call they would answer comes here.
    let (posted, calls) = std::sync::mpsc::channel::<crate::side::browser::agent::Call>();
    let browser: crate::app_socket::BrowserPost = Arc::new(move |call| posted.send(call).is_ok());
    let loops = Loops {
        browser,
        ..switch(Arc::new(|| true))
    };
    let service = local.clone();
    let (runner, mut events) = CodeRunner::start_with(data.clone(), &gateway, &scope, loops, |_| Ok(service))
        .expect("the runner starts")
        .expect("no other owner");
    runner.add_project(repo.clone(), None);
    let project = next(&mut events, &local, |event| match event {
        CodeEvent::ProjectAdded { id, .. } => Some(id),
        _ => None,
    });
    runner.create(
        "draft-0".into(),
        NewSession {
            project_id: project,
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Default.as_str().to_string(),
            effort: None,
            via: Via::Nexus,
            provider: None,
            worktree: false,
            review: None,
            base: None,
            text: "hello".into(),
            attachments: Default::default(),
        },
    );
    let thread = next(&mut events, &local, |event| match event {
        CodeEvent::Created { id, .. } => Some(id),
        _ => None,
    });

    // The service's agent starts the browser server through the launcher. Its address and credential are in the launch
    // answer, not in the agent's arguments or environment.
    assert!(
        wait_for(Duration::from_secs(20), || record.exists()),
        "the agent did not start: {}",
        report(&local)
    );
    let started: Value = serde_json::from_str(&std::fs::read_to_string(&record).expect("the record")).expect("json");
    assert_eq!(
        mcp_config(&started)["mcpServers"][NAME],
        json!({"type": "stdio", "command": "/opt/nexus/nexus-desktop-gpui",
               "args": [format!("--mcp-launch={NAME}")], "env": {}})
    );
    assert!(started["env"].get(ADDRESS).is_none() && started["env"].get(BROWSER_TOKEN).is_none());
    let launched = launch_answer(&started, NAME);
    let address: std::net::SocketAddr = launched["result"]["env"][ADDRESS]
        .as_str()
        .and_then(|address| address.parse().ok())
        .expect("the service's socket");
    let token = launched["result"]["env"][BROWSER_TOKEN]
        .as_str()
        .expect("its credential")
        .to_string();
    assert!(
        !started["args"].to_string().contains(&token) && !started["env"].to_string().contains(&token),
        "the browser credential is not in the agent's arguments or environment"
    );

    // A browser tool, as `--browser-mcp` relays it (no `tools`): the service passes it here, to the session's windows.
    let call = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                      "params": {"name": "browser_tabs", "arguments": {}}});
    let asking = std::thread::spawn(move || {
        nexus_desktop_mcp::app_socket::relay(
            &address,
            &json!({"token": token, "message": call}),
            Duration::from_secs(30),
        )
    });
    let reached = calls
        .recv_timeout(Duration::from_secs(20))
        .unwrap_or_else(|_| panic!("the call did not reach the windows: {}", report(&local)));
    assert_eq!(reached.thread, thread);
    assert_eq!(reached.run, "ns_local", "the run Gateway's stand-in opened");
    assert_eq!(reached.message["params"]["name"], "browser_tabs");
    // This test signs nothing in here (no keychain): the call does not hold, as on this build's own socket.
    assert!(!(reached.valid)(), "a call holds only for the person signed in here");
    reached
        .reply
        .send(json!({"jsonrpc": "2.0", "id": 3, "result": {"content": [], "isError": false}}))
        .expect("answered");
    let answer: Value = serde_json::from_str(asking.join().expect("the call").expect("a reply").trim()).expect("json");
    assert_eq!(answer["id"], 3);
    assert_eq!(answer["result"]["isError"], false, "{answer}");

    // A forged credential never reaches the windows.
    let forged = nexus_desktop_mcp::app_socket::relay(
        &address,
        &json!({"token": "made-up", "message": {"jsonrpc": "2.0", "id": 4, "method": "tools/list"}}),
        Duration::from_secs(10),
    )
    .expect("a refusal");
    assert!(forged.contains("invalid or expired"), "{forged}");
    assert!(calls.try_recv().is_err());

    drop(runner);
    drop(_stop);
    let _ = std::fs::remove_dir_all(root);
}

/// This build's own launch of Claude Code (`GpuiHost::launch`, no service): every stdio server starts through the
/// launcher, Claude's environment has the app socket's address and a launch credential and no Settings value, and the
/// launch credential answers the launch of `db` with its value.
///
/// `find` takes `NEXUS_CLAUDE_BIN` from the process environment, so the test runs only when it names a file (the
/// stand-in `claude` or any other: the launch never starts it):
/// `NEXUS_CLAUDE_BIN=target/debug/fake-claude cargo test the_apps_own_claude -- --include-ignored`.
#[test]
#[ignore = "needs NEXUS_CLAUDE_BIN naming a file: see this test's comment"]
fn the_apps_own_claude_gets_launcher_entries_and_the_launch_credential_answers_for_settings_servers() {
    use nexus_desktop_core::settings::{McpServer, Settings};
    use nexus_desktop_mcp::mcp::{APP_ADDRESS_VAR, APP_TOKEN_VAR, VAULT_ACCOUNT};

    /// Puts Settings and the keychain's stand-in (both of this test process) back as they were.
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(file) = crate::desktop_settings::file() {
                let _ = std::fs::remove_file(file);
            }
            let _ = crate::keychain::vault().set(VAULT_ACCOUNT, "{}");
        }
    }

    let found = nexus_workbench::agent_launch::find(
        SessionAgent::Claude,
        &agent_env(),
        &std::env::current_dir().unwrap_or_default(),
    );
    assert!(
        found.is_some(),
        "NEXUS_CLAUDE_BIN names no file: see this test's comment"
    );

    let root = root("own-claude");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("a project folder");
    let gateway = gateway();
    let scope = format!("{gateway}|acme");

    // Settings with a server `db` and its value in the keychain's stand-in, as `GpuiHost::launch` reads them.
    let _restore = Restore;
    let server = |name: &str, enabled: bool, command: &str, secret: &str| McpServer {
        name: name.into(),
        enabled,
        command: Some(command.into()),
        args: Vec::new(),
        url: None,
        secrets: vec![secret.into()],
        tool_timeout_sec: None,
        plugin: None,
    };
    let settings = Settings {
        mcp_servers: vec![
            server("db", true, "db-mcp", "DB_TOKEN"),
            server("off", false, "off-mcp", "OFF_TOKEN"),
        ],
        ..Settings::default()
    };
    let file = crate::desktop_settings::file().expect("the test's settings folder");
    nexus_desktop_core::settings::save(&file, &settings).expect("Settings saved");
    crate::keychain::vault()
        .set(
            VAULT_ACCOUNT,
            r#"{"db":{"DB_TOKEN":"s3cret_db"},"off":{"OFF_TOKEN":"s3cret_off"}}"#,
        )
        .expect("the stand-in keychain");

    let client = Client::new(crate::gateway::client_options(&gateway, Some(TOKEN.into())));
    let loops = Loops {
        client: Arc::new(move || Some(client.clone())),
        ..switch(Arc::new(|| false))
    };
    let (runner, _events) = CodeRunner::start_in(root.join("data"), &gateway, &scope, loops)
        .expect("the store opens")
        .expect("free, so taken");
    let launch = runtime()
        .block_on(runner.host.launch(LaunchRequest {
            thread_id: "thread".into(),
            agent: SessionAgent::Claude,
            cwd: repo.clone(),
            model: None,
            via: Via::Nexus,
            required_payer: None,
            provider: None,
            account_dir: None,
            args: Vec::new(),
            scope: scope.clone().into(),
            prompt_counts: None,
        }))
        .unwrap_or_else(|error| panic!("the launch: {error}"));
    let started = json!({"args": launch.spawn.args, "env": launch.spawn.env});
    let program = std::env::current_exe()
        .expect("this binary")
        .to_string_lossy()
        .into_owned();

    // Every stdio server starts through the launcher, and none of Settings' values is in the arguments or environment.
    let launcher =
        |name: &str| json!({"type": "stdio", "command": program, "args": [format!("--mcp-launch={name}")], "env": {}});
    let config = mcp_config(&started);
    assert_eq!(config["mcpServers"]["nexus"], launcher("nexus"), "{config}");
    assert_eq!(config["mcpServers"]["db"], launcher("db"), "{config}");
    assert_eq!(
        config["mcpServers"][nexus_desktop_mcp::browser::NAME],
        launcher(nexus_desktop_mcp::browser::NAME),
        "{config}"
    );
    assert!(
        config["mcpServers"].get("off").is_none(),
        "a server switched off stays out: {config}"
    );
    assert!(launch.session_mcp.skipped.is_empty(), "every server started");
    let args = started["args"].to_string();
    assert!(
        !args.contains("db-mcp") && !args.contains("${DB_TOKEN}") && !args.contains("s3cret"),
        "{args}"
    );
    let env = started["env"].to_string();
    assert!(!env.contains("s3cret"), "no Settings value in the agent's environment");
    for variable in ["DB_TOKEN", "OFF_TOKEN", "NEXUS_MCP_TOKEN", "NEXUS_BROWSER_TOKEN"] {
        assert!(
            started["env"].get(variable).is_none(),
            "{variable} is in the agent's environment"
        );
    }
    assert!(started["env"][APP_ADDRESS_VAR].is_string() && started["env"][APP_TOKEN_VAR].is_string());

    // The launch credential answers for `db` with its command and value, and once.
    let db = launch_answer(&started, "db");
    assert_eq!(db["result"]["command"], "db-mcp", "{db}");
    assert_eq!(db["result"]["env"]["DB_TOKEN"], "s3cret_db", "{db}");
    assert_eq!(db["result"]["strip_credentials"], "no_credentials", "{db}");
    assert!(
        launch_answer(&started, "db")["error"].is_object(),
        "a server launches once"
    );
    assert!(
        launch_answer(&started, "off")["error"].is_object(),
        "a server switched off does not launch"
    );

    drop(runner);
    let _ = std::fs::remove_dir_all(root);
}

/// With the service running the sessions, its agents get the shared terminal tools, and this window runs and reads a
/// session's commands through the service with the person's authority.
#[test]
#[ignore = "needs NEXUS_HOST_BIN and NEXUS_FAKE_CLAUDE_BIN: see this file's first lines"]
fn the_services_sessions_share_their_terminal_with_this_window() {
    let root = root("terminal");
    let data = root.join("data");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("a project folder");
    let gateway = gateway();
    let scope = format!("{gateway}|acme");
    let record = root.join("launch.json");
    let mut local = stand_in(&root, &gateway, &scope);
    local
        .env
        .push(("FAKE_LAUNCH_RECORD".into(), record.to_string_lossy().into_owned()));
    let _stop = StopService(local.clone());
    let service = local.clone();
    let (runner, mut events) =
        CodeRunner::start_with(data.clone(), &gateway, &scope, switch(Arc::new(|| true)), |_| {
            Ok(service)
        })
        .expect("the runner starts")
        .expect("no other owner");
    runner.add_project(repo.clone(), None);
    let project = next(&mut events, &local, |event| match event {
        CodeEvent::ProjectAdded { id, .. } => Some(id),
        _ => None,
    });
    runner.create(
        "draft-0".into(),
        NewSession {
            project_id: project,
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Default.as_str().to_string(),
            effort: None,
            via: Via::Nexus,
            provider: None,
            worktree: false,
            review: None,
            base: None,
            text: "hello".into(),
            attachments: Default::default(),
        },
    );
    let thread = next(&mut events, &local, |event| match event {
        CodeEvent::Created { id, .. } => Some(id),
        _ => None,
    });

    // The service's agent starts with the terminal tools, and without its own shell.
    assert!(
        wait_for(Duration::from_secs(20), || record.exists()),
        "the agent did not start: {}",
        report(&local)
    );
    let launch: Value = serde_json::from_str(&std::fs::read_to_string(&record).expect("the record")).expect("json");
    let args = launch["args"].to_string();
    assert!(args.contains("terminal_run"), "the terminal tools: {args}");
    assert!(args.contains("--disallowedTools=Bash"), "no native shell: {args}");

    // The person runs a command from this window: the service runs it, and its output comes back here.
    let terminal = |op: &str, args: Value| {
        futures::executor::block_on(runner.terminal(thread.clone(), op.into(), args))
            .unwrap_or_else(|_| Err("the runner stopped".into()))
    };
    let ran = terminal("run", json!({ "command": "echo from-the-service", "request_id": "r1" }))
        .unwrap_or_else(|error| panic!("the person's command: {error}: {}", report(&local)));
    let id = ran["id"].as_str().expect("the command's id").to_string();
    let output = std::cell::RefCell::new(String::new());
    assert!(
        wait_for(Duration::from_secs(20), || {
            if let Ok(read) = terminal("read", json!({ "id": id, "after": 0 })) {
                *output.borrow_mut() = read["output"].as_str().unwrap_or_default().to_string();
            }
            output.borrow().contains("from-the-service")
        }),
        "the command's output: {:?}: {}",
        output.borrow(),
        report(&local)
    );
    let listed = terminal("list", json!({})).expect("the session's commands");
    assert!(listed.to_string().contains(&id), "{listed}");

    drop(runner);
    drop(_stop);
    let _ = std::fs::remove_dir_all(root);
}

fn start_local_turn(
    runner: &CodeRunner,
    events: &mut UnboundedReceiver<CodeEvent>,
    diagnostic: &LocalService,
    repo: PathBuf,
) -> String {
    runner.add_project(repo, None);
    let project = next(events, diagnostic, |event| match event {
        CodeEvent::ProjectAdded { id, .. } => Some(id),
        _ => None,
    });
    runner.create(
        "local-subscription".into(),
        NewSession {
            project_id: project,
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: PermissionMode::Default.as_str().into(),
            effort: None,
            via: Via::Subscription,
            provider: None,
            worktree: false,
            review: None,
            base: None,
            text: "Reply with OK.".into(),
            attachments: Default::default(),
        },
    );
    next(events, diagnostic, |event| match event {
        CodeEvent::Created { id, .. } => Some(id),
        CodeEvent::StartFailed { error, .. } => panic!("could not create a local Code session: {error}"),
        _ => None,
    })
}

fn subscription_turn_statuses(data: &Path, id: &str) -> Vec<nexus_workbench::event::TurnStatus> {
    Store::open(&data.join("workbench.sqlite3"))
        .and_then(|store| store.events_after(id, 0))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|event| match event.event {
            WbEvent::TurnEnd { status, .. } => Some(status),
            _ => None,
        })
        .collect()
}

fn assert_subscription_turns(data: &Path, id: &str, count: usize, phase: &str) {
    use nexus_workbench::event::TurnStatus;

    let expected = vec![TurnStatus::Completed; count];
    wait_for(Duration::from_secs(20), || {
        subscription_turn_statuses(data, id).len() >= count
    });
    assert_eq!(subscription_turn_statuses(data, id), expected, "{phase}");
}

fn assert_local_turn(data: &Path, id: &str) {
    let store = Store::open(&data.join("workbench.sqlite3")).expect("store");
    let record = store.thread(id).expect("read thread").expect("thread");
    assert!(record.summary.nexus_sessions.is_empty());
    assert_eq!(record.summary.via, Via::Subscription);
    let events = store.events_after(id, 0).expect("events");
    assert!(events.iter().any(|event| matches!(
        event.event,
        WbEvent::Session {
            nexus_session_id: None,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event.event,
        WbEvent::TurnEnd {
            status: nexus_workbench::event::TurnStatus::Completed,
            ..
        }
    )));
}

#[test]
#[ignore = "needs NEXUS_HOST_BIN and NEXUS_FAKE_CLAUDE_BIN: see this file's first lines"]
fn signed_out_service_finishes_a_subscription_turn_and_starts_its_mcp_server() {
    let root = root("local-byo");
    let data = root.join("data");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("project folder");
    let trap = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback trap");
    trap.set_nonblocking(true).expect("non-blocking trap");
    let gateway = format!("http://{}", trap.local_addr().expect("trap address"));
    let record = root.join("launch.json");
    let (script, marker) = local_mcp_fixture(&root);
    let mut local = stand_in(&root, &gateway, LOCAL_SCOPE);
    local.app_program = Some(built("NEXUS_HOST_BIN"));
    local.env.push(("NEXUS_BYO_PROVIDERS".into(), "1".into()));
    local
        .env
        .push(("FAKE_LAUNCH_RECORD".into(), record.to_string_lossy().into_owned()));
    local.env.extend([
        ("ANTHROPIC_AUTH_TOKEN".into(), "prior-token".into()),
        ("ANTHROPIC_BASE_URL".into(), gateway.clone()),
        (
            "ANTHROPIC_CUSTOM_HEADERS".into(),
            "x-nexus-session: prior-session".into(),
        ),
        ("CLAUDE_CODE_ENABLE_TELEMETRY".into(), "1".into()),
        ("OTEL_LOGS_EXPORTER".into(), "otlp".into()),
        ("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL".into(), "http/json".into()),
        (
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT".into(),
            format!("{gateway}/v1/agents/sessions/old/usage"),
        ),
        (
            "OTEL_EXPORTER_OTLP_LOGS_HEADERS".into(),
            "Authorization=Bearer prior-session".into(),
        ),
    ]);
    local.env.push(("FAKE_CLAUDE_INIT_MCP".into(), "db".into()));
    let hello = json!({
        "localMcp": {
            "servers": [{"name": "db", "command": "python3", "args": ["-u", script, marker], "secrets": ["DB_TOKEN"]}],
            "values": {"db": {"DB_TOKEN": "db-own"}},
        },
        "accounts": [],
    });
    let mcp = hello["localMcp"].clone();
    let pending_account = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let pending = pending_account.clone();
    local.sign_in = Arc::new(move || {
        let mut hello = hello.clone();
        if pending.load(std::sync::atomic::Ordering::SeqCst) {
            let payload = hello.as_object_mut().expect("hello body");
            let mcp = payload.remove("localMcp").expect("local MCP");
            payload.insert("localAccountPendingMcp".into(), mcp);
        }
        Some(hello)
    });
    let stop = StopService(local.clone());
    let service = local.clone();
    let (runner, mut events) =
        CodeRunner::start_with(data.clone(), &gateway, LOCAL_SCOPE, switch(Arc::new(|| true)), |_| {
            Ok(service)
        })
        .expect("runner starts")
        .expect("runner owns store");
    let id = start_local_turn(&runner, &mut events, &local, repo);
    assert!(
        wait_for(Duration::from_secs(20), || record.exists()),
        "the agent did not launch: {}",
        report(&local)
    );
    let started: Value = serde_json::from_slice(&std::fs::read(record).expect("launch record")).expect("JSON");
    assert!(mcp_config(&started)["mcpServers"].get("nexus").is_none());
    assert!(mcp_config(&started)["mcpServers"].get("nexus_browser").is_none());
    for name in [
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_CUSTOM_HEADERS",
        "CLAUDE_CODE_ENABLE_TELEMETRY",
        "OTEL_LOGS_EXPORTER",
        "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL",
        "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
        "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
        "DB_TOKEN",
    ] {
        assert!(started["env"].get(name).is_none(), "{name} reached the agent");
    }
    assert_subscription_turns(&data, &id, 1, "the first service turn");
    next(&mut events, &local, |event| match event {
        CodeEvent::TurnEnded(thread) if thread == id => Some(()),
        _ => None,
    });
    assert_local_turn(&data, &id);
    assert!(
        marker.is_file(),
        "Claude's Settings server did not initialize through the launcher"
    );
    assert_eq!(
        launch_answer(&started, "db")["error"]["code"],
        -32602,
        "a server starts once"
    );
    runner.stop(id.clone());
    assert!(
        wait_for(
            Duration::from_secs(20),
            || launch_answer(&started, "db")["error"]["message"] == nexus_workbench::app_socket::INVALID
        ),
        "the process end did not revoke the grant"
    );
    assert_eq!(
        trap.accept().expect_err("Gateway saw a connection").kind(),
        std::io::ErrorKind::WouldBlock
    );
    pending_account.store(true, std::sync::atomic::Ordering::SeqCst);
    runner.refresh_local_service_account();
    let remote = runner.api.remote().expect("background service");
    assert!(
        wait_for(Duration::from_secs(20), || matches!(
            remote.status(),
            nexus_remote::machine::Status::Connected { .. }
        ) && !remote.is_held()),
        "the service did not reconnect with the pending-account hello: {}",
        report(&local)
    );
    runner.send(
        id.clone(),
        "Continue while the account is pending.".into(),
        Default::default(),
        false,
    );
    assert_subscription_turns(&data, &id, 2, "the service turn after reconnecting");
    next(&mut events, &local, |event| match event {
        CodeEvent::TurnEnded(thread) if thread == id => Some(()),
        _ => None,
    });
    runtime().block_on(runner.api.stop(&id)).expect("stop the local agent");

    let scope = format!("{gateway}|fixture");
    runtime()
        .block_on(runner.api.remote().expect("background service").call(
            "host_sign_in",
            json!({"gateway":gateway, "token":"fixture-key", "scope":scope, "mcp":mcp}),
        ))
        .expect("sign in the service");
    runner.send(
        id.clone(),
        "Continue the same subscription.".into(),
        Default::default(),
        false,
    );
    assert_subscription_turns(&data, &id, 3, "the signed-in service turn");
    next(&mut events, &local, |event| match event {
        CodeEvent::TurnEnded(thread) if thread == id => Some(()),
        _ => None,
    });
    assert_local_turn(&data, &id);
    drop(runner);
    drop(stop);
}

#[test]
#[ignore = "needs NEXUS_HOST_BIN and NEXUS_FAKE_CLAUDE_BIN: see this file's first lines"]
fn signed_out_gpui_finishes_a_subscription_turn_with_gateway_closed() {
    const CHILD: &str = "NEXUS_TEST_GPUI_LOCAL_SUBSCRIPTION";
    if std::env::var_os(CHILD).is_none() {
        let exe = std::env::current_exe().expect("GPUI test executable");
        let profile = root("own-local-byo-profile");
        let memory = profile.join("memory");
        std::fs::create_dir_all(&memory).expect("the fake Claude conversation store");
        let output = nexus_workbench::process::command(exe)
            .args([
                "--ignored",
                "--exact",
                "code_run::tests::service::signed_out_gpui_finishes_a_subscription_turn_with_gateway_closed",
            ])
            .env(CHILD, "1")
            .env("XDG_CONFIG_HOME", profile.join("config"))
            .env("XDG_DATA_HOME", profile.join("data"))
            .env("XDG_CACHE_HOME", profile.join("cache"))
            .env("NEXUS_BYO_PROVIDERS", "1")
            .env("FAKE_CLAUDE_DIR", memory)
            .env("FAKE_CLAUDE_INIT_MCP", "db")
            .env("FAKE_CLAUDE_MCP_LAUNCHER", built("NEXUS_HOST_BIN"))
            .env("NEXUS_CLAUDE_BIN", built("NEXUS_FAKE_CLAUDE_BIN"))
            .output()
            .expect("run isolated GPUI test");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let mode = crate::local_providers_opt_in::from_env();
    assert!(mode.enabled());
    let root = root("own-local-byo");
    let data = root.join("data");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("project folder");
    let (script, marker) = local_mcp_fixture(&root);
    let settings = Settings {
        mcp_servers: serde_json::from_value(json!([{
            "name": "db", "command": "python3", "args": ["-u", script, marker], "secrets": ["DB_TOKEN"]
        }]))
        .expect("the Settings MCP fixture"),
        ..Settings::default()
    };
    let file = crate::desktop_settings::file().expect("the test's Settings file");
    nexus_desktop_core::settings::save(&file, &settings).expect("save Settings");
    crate::keychain::vault()
        .set(nexus_desktop_mcp::mcp::VAULT_ACCOUNT, r#"{"db":{"DB_TOKEN":"db-own"}}"#)
        .expect("seed the test keychain");
    let (runner, mut events) = CodeRunner::start_in(
        data.clone(),
        GATEWAY,
        LOCAL_SCOPE,
        Loops {
            local_providers_opt_in: mode,
            ..quick(Duration::from_secs(600))
        },
    )
    .expect("opens the store")
    .expect("takes the store");
    let diagnostic = stand_in(&root, GATEWAY, LOCAL_SCOPE);
    let id = start_local_turn(&runner, &mut events, &diagnostic, repo);
    assert_subscription_turns(&data, &id, 1, "the first GPUI turn");
    next(&mut events, &diagnostic, |event| match event {
        CodeEvent::TurnEnded(thread) if thread == id => Some(()),
        _ => None,
    });
    assert_local_turn(&data, &id);
    assert!(
        marker.is_file(),
        "Claude's Settings server did not initialize through the launcher"
    );
    runner.stop(id.clone());
    assert!(wait_for(Duration::from_secs(10), || !runner
        .api
        .workbench()
        .is_live(&id)));
    let gateway = std::net::TcpListener::bind("127.0.0.1:0").expect("the account endpoint");
    gateway.set_nonblocking(true).expect("nonblocking account listener");
    let gateway_url = format!("http://{}", gateway.local_addr().expect("account address"));
    let (requested, waiting) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let account_server = std::thread::spawn(move || {
        use std::io::BufRead as _;

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut connection = loop {
            match gateway.accept() {
                Ok((connection, _)) => break connection,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("the account read did not reach Gateway: {error}"),
            }
        };
        connection.set_nonblocking(false).expect("blocking account stream");
        connection
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("account request deadline");
        let mut request_line = String::new();
        std::io::BufReader::new(&connection)
            .read_line(&mut request_line)
            .expect("account request");
        assert!(request_line.starts_with("GET /v1/tenants/me "), "{request_line}");
        requested.send(()).expect("the test waits for the account request");
        released
            .recv_timeout(Duration::from_secs(25))
            .expect("the test releases the response");
        connection
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .expect("account error response");
    });
    let client = nexus_client::Client::new(nexus_client::ClientOptions::new(&gateway_url))
        .with_api_key(Some("fixture-key".into()));
    let account_client = client.clone();
    runner.set_gateway(&gateway_url, Arc::new(move || Some(client.clone())));
    let hello = runner
        .host
        .service_session()
        .expect("a Local service can reconnect while the account is pending");
    assert!(hello.get("localAccountPendingMcp").is_some());
    assert!(hello.get("token").is_none());
    let account_read = runtime().spawn(async move { account_client.get("/v1/tenants/me").await });
    waiting
        .recv_timeout(Duration::from_secs(10))
        .expect("the account read is pending");

    let mut failed_account = Some((release, account_read, account_server));
    for (completed, phase) in [
        (2, "the pending account read"),
        (3, "the failed account read"),
        (4, "the adopted organization"),
    ] {
        if completed == 4 {
            runner.set_scope(&format!("{gateway_url}|fixture"));
        } else {
            assert_eq!(runner.host.scope(), LOCAL_SCOPE);
        }
        runner.send(
            id.clone(),
            format!("Continue during {phase}."),
            Default::default(),
            false,
        );
        assert_subscription_turns(&data, &id, completed, phase);
        next(&mut events, &diagnostic, |event| match event {
            CodeEvent::TurnEnded(thread) if thread == id => Some(()),
            _ => None,
        });
        if let Some((release, account_read, account_server)) = failed_account.take() {
            release.send(()).expect("release the failed account response");
            assert!(runtime().block_on(account_read).expect("account task").is_err());
            account_server.join().expect("account endpoint");
        }
        runner.stop(id.clone());
        assert!(wait_for(Duration::from_secs(10), || !runner
            .api
            .workbench()
            .is_live(&id)));
    }
    assert_local_turn(&data, &id);
    runner.shutdown();
}

fn local_mcp_fixture(root: &Path) -> (PathBuf, PathBuf) {
    let marker = root.join("mcp-initialized");
    let script = root.join("mcp.py");
    std::fs::write(
        &script,
        r#"
import json
import os
from pathlib import Path
import sys

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    if request["method"] == "initialize":
        assert os.environ.get("NEXUS_MCP_APP_TOKEN") is None
        assert os.environ["DB_TOKEN"] == "db-own"
        Path(sys.argv[1]).write_text("initialized")
        result = {
            "protocolVersion": request["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "local-subscription-check", "version": "1.0"},
        }
    else:
        result = {"tools": []} if request["method"] == "tools/list" else {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
"#,
    )
    .expect("write the test MCP server");
    (script, marker)
}

/// Run explicitly with the installed Claude Code binary and a personal subscription login.
#[test]
#[ignore = "manual: set NEXUS_REAL_CLAUDE_BIN to an installed Claude Code"]
fn manual_signed_out_claude_subscription_turn() {
    let Some(claude) = std::env::var_os("NEXUS_REAL_CLAUDE_BIN").map(PathBuf::from) else {
        return;
    };
    assert!(
        claude.is_file(),
        "NEXUS_REAL_CLAUDE_BIN must name the Claude Code executable"
    );
    let root = root("real-local-byo");
    let data = root.join("data");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("project folder");
    let (script, marker) = local_mcp_fixture(&root);

    let mut local = stand_in(&root, GATEWAY, LOCAL_SCOPE);
    local.app_program = Some(built("NEXUS_HOST_BIN"));
    local
        .env
        .retain(|(key, _)| !matches!(key.as_str(), "HOME" | "NEXUS_CLAUDE_BIN") && !key.starts_with("FAKE_CLAUDE_"));
    local
        .env
        .push(("NEXUS_CLAUDE_BIN".into(), claude.to_string_lossy().into_owned()));
    local.env.push(("NEXUS_BYO_PROVIDERS".into(), "1".into()));
    let hello = json!({
        "localMcp": {
            "servers": [{
                "name": "local-subscription-check",
                "command": "python3",
                "args": ["-u", script.to_str().expect("script path"), marker.to_str().expect("marker path")],
                "secrets": ["DB_TOKEN"],
            }],
            "values": {"local-subscription-check": {"DB_TOKEN": "db-own"}},
        },
        "accounts": [],
    });
    local.sign_in = Arc::new(move || Some(hello.clone()));
    let stop = StopService(local.clone());
    let service = local.clone();
    let (runner, mut events) =
        CodeRunner::start_with(data.clone(), GATEWAY, LOCAL_SCOPE, switch(Arc::new(|| true)), |_| {
            Ok(service)
        })
        .expect("runner starts")
        .expect("runner owns store");
    let id = start_local_turn(&runner, &mut events, &local, repo);
    assert!(
        wait_for(Duration::from_secs(90), || turn_ends(&data, &id) >= 1),
        "the Claude Code turn did not finish"
    );
    assert_local_turn(&data, &id);
    assert!(
        marker.is_file(),
        "the Settings MCP server did not initialize through the launcher"
    );
    runner.stop(id);
    drop(runner);
    drop(stop);
    println!("Claude Code completed a subscription turn and the Settings MCP server initialized.");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "needs the built background service and fake Claude binary"]
fn signed_out_saved_server_runs_in_each_launch_owner() {
    const CHILD: &str = "NEXUS_TEST_SAVED_SERVER_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let probe = root("server-stage");
        let fake = probe.join("opencode.py");
        std::fs::write(&fake, SAVED_SERVER_AGENT).expect("write the fixture agent");
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).expect("make the fixture executable");
        for owner in ["gpui", "service"] {
            let output = nexus_workbench::process::command(std::env::current_exe().expect("test binary"))
                .args([
                    "--ignored",
                    "--exact",
                    "code_run::tests::service::signed_out_saved_server_runs_in_each_launch_owner",
                ])
                .env(CHILD, owner)
                .env("NEXUS_BYO_PROVIDERS", "on")
                .env("XDG_CONFIG_HOME", probe.join(owner))
                .env("NEXUS_OPENCODE_BIN", &fake)
                .output()
                .expect("run the isolated test");
            assert!(
                output.status.success(),
                "{owner}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
        return;
    }

    let root = root("saved-server");
    let data = root.join("data");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("project folder");
    let gateway = std::net::TcpListener::bind("127.0.0.1:0").expect("Gateway trap");
    gateway.set_nonblocking(true).expect("nonblocking Gateway trap");
    let gateway_url = format!("http://{}", gateway.local_addr().expect("Gateway address"));
    let (address, model_turn) = loopback_model_turn(false);
    let model = "team/model";
    let config = nexus_desktop_core::settings::config_dir().expect("config directory");
    let providers = nexus_desktop_core::provider_settings::ProviderSettingsStore::in_config(&config);
    let provider = providers
        .add_server("Local test server".into(), &address, vec![model.into()], None)
        .expect("save the provider");
    let mut local = stand_in(&root, &gateway_url, LOCAL_SCOPE);
    local.sign_in = Arc::new(|| Some(json!({"localMcp":{"servers":[],"values":{}},"accounts":[]})));
    local.app_program = Some(built("NEXUS_HOST_BIN"));
    let use_service = std::env::var(CHILD).as_deref() == Ok("service");
    let mode = crate::local_providers_opt_in::from_env();
    let (runner, mut events, _stop) = if use_service {
        let service = local.clone();
        let (runner, events) = CodeRunner::start_with(
            data.clone(),
            &gateway_url,
            LOCAL_SCOPE,
            Loops {
                local_providers_opt_in: mode,
                ..switch(Arc::new(|| true))
            },
            |_| Ok(service),
        )
        .expect("start service runner")
        .expect("runner owns store");
        (runner, events, Some(StopService(local.clone())))
    } else {
        let (runner, events) = CodeRunner::start_in(
            data.clone(),
            &gateway_url,
            LOCAL_SCOPE,
            Loops {
                local_providers_opt_in: mode,
                ..quick(Duration::from_secs(600))
            },
        )
        .expect("start GPUI runner")
        .expect("runner owns store");
        (runner, events, None)
    };
    runner.add_project(repo, None);
    let project = next(&mut events, &local, |event| match event {
        CodeEvent::ProjectAdded { id, .. } => Some(id),
        _ => None,
    });
    runner.create(
        "saved-server".into(),
        NewSession {
            project_id: project,
            agent: SessionAgent::Opencode,
            model: None,
            permission_mode: PermissionMode::Default.as_str().into(),
            effort: None,
            via: Via::Subscription,
            provider: Some(provider.clone()),
            worktree: false,
            review: None,
            base: None,
            text: "Reply with OK.".into(),
            attachments: Default::default(),
        },
    );
    let id = next(&mut events, &local, |event| match event {
        CodeEvent::Created { id, .. } => Some(id),
        CodeEvent::StartFailed { error, .. } => panic!("saved server did not start: {error}"),
        _ => None,
    });
    assert!(
        wait_for(Duration::from_secs(20), || turn_ends(&data, &id) == 1),
        "saved server did not finish its turn"
    );
    assert_local_turn(&data, &id);
    let store = Store::open(&data.join("workbench.sqlite3")).expect("store");
    let summary = store.thread(&id).expect("row").expect("saved session").summary;
    assert_eq!(summary.provider, Some(provider));
    assert_eq!(summary.model.as_deref(), Some(model));
    assert!(store.events_after(&id, 0).expect("events").iter().any(|event| matches!(
        &event.event, WbEvent::Text { text, .. } if text == "OK"
    )));
    assert_eq!(
        gateway.accept().expect_err("Gateway saw a connection").kind(),
        std::io::ErrorKind::WouldBlock
    );
    model_turn.join().expect("model server");
    runner.shutdown();
}

#[cfg(target_os = "linux")]
fn loopback_model_turn(keyed: bool) -> (String, std::thread::JoinHandle<()>) {
    let server = std::net::TcpListener::bind("127.0.0.1:0").expect("model server");
    server.set_nonblocking(true).expect("nonblocking model server");
    let addr = server.local_addr().expect("model server address");
    let model_turn = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut stream = loop {
            match server.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("accept model request: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("request deadline");
        let mut buffer = [0; 8192];
        let n = stream.read(&mut buffer).expect("model request");
        let request = std::str::from_utf8(&buffer[..n]).expect("HTTP request");
        assert!(request.starts_with("POST /v1/chat/completions "), "{request}");
        let auth = request
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("authorization:"));
        assert_eq!(
            auth.map(|line| line.split_once(':').unwrap().1.trim()),
            keyed.then_some("Bearer nexus-fixture-key")
        );
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
            .expect("model response");
    });
    (format!("http://{addr}/v1"), model_turn)
}

#[cfg(target_os = "linux")]
const SAVED_SERVER_AGENT: &str = r#"#!/usr/bin/env python3
import json, os, sys, urllib.request
config = json.loads(os.environ['OPENCODE_CONFIG_CONTENT'])
provider, model = config['model'].split('/', 1)
assert provider.startswith('nexus_byo_') and model == 'session'
assert config['provider'][provider]['models'][model]['id'] == 'team/model'
key = os.environ.get('NEXUS_OPENCODE_API_KEY')
active_model = 'ambient/other-model'
def send(message):
    print(json.dumps({'jsonrpc': '2.0', **message}), flush=True)
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get('method')
    if 'id' not in msg:
        continue
    reply = {'id': msg['id']}
    if method == 'initialize':
        send({**reply, 'result': {'protocolVersion': 1, 'agentCapabilities': {}}})
    elif method == 'session/new':
        send({**reply, 'result': {'sessionId': 'saved-server-fixture', 'configOptions': [
            {'id': 'model', 'currentValue': active_model}
        ]}})
    elif method == 'session/set_config_option':
        active_model = msg['params']['value']
        send({**reply, 'result': {'configOptions': [{'id': 'model', 'currentValue': active_model}]}})
    elif method == 'session/prompt':
        assert active_model == config['model']
        base = config['provider'][provider]['options']['baseURL']
        req = urllib.request.Request(base + '/chat/completions', data=b'{"model":"team/model"}', method='POST')
        if key:
            req.add_header('Authorization', 'Bearer ' + key)
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req, timeout=5) as res:
            text = res.read().decode()
        send({'method': 'session/update', 'params': {'sessionId': 'saved-server-fixture', 'update': {
            'sessionUpdate': 'agent_message_chunk', 'content': {'type': 'text', 'text': text}}}})
        send({**reply, 'result': {'stopReason': 'end_turn'}})
    else:
        send({**reply, 'result': {}})
"#;
