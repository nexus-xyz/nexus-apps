//! The host over its socket, against the stand-in `claude`: the first end-to-end check.
//! A session is created, approved, sent to, left mid-turn by its client, and caught up on reconnect with
//! every event exactly once. A wrong token, another protocol and a browser's `Origin` are refused.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use nexus_host::client::{Connection, connect};
use nexus_host::files::Paths;
use nexus_host::server::PROTOCOL;
use nexus_host::{Listen, Listening, Shell};
use serde_json::{Value, json};
use tokio::net::UnixStream;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

mod common;

use common::{TOKEN, root, start};

async fn open(socket: &PathBuf) -> Connection<UnixStream> {
    let stream = UnixStream::connect(socket).await.unwrap_or_else(|e| panic!("{e}"));
    connect(stream, TOKEN).await.unwrap_or_else(|e| panic!("{e}"))
}

async fn call(connection: &mut Connection<UnixStream>, name: &str, args: Value, log: &mut Log) -> Value {
    let (answer, pushed) = connection.call(name, args).await;
    for push in pushed {
        log.live(&push);
    }
    answer.unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The session's events as the app keeps them: by `seq`, each once. A live event is kept only past the last `seq`
/// already held (the catch-up rule); a `seq` that comes twice is a duplicate.
#[derive(Default)]
struct Log {
    thread: String,
    events: BTreeMap<i64, Value>,
    duplicates: Vec<i64>,
}

impl Log {
    fn last(&self) -> i64 {
        self.events.keys().next_back().copied().unwrap_or(0)
    }

    fn add(&mut self, event: &Value) {
        let seq = event["seq"].as_i64().unwrap_or_default();
        if self.events.insert(seq, event.clone()).is_some() {
            self.duplicates.push(seq);
        }
    }

    fn live(&mut self, push: &Value) {
        let payload = &push["payload"];
        if push["event"] == "wb:event"
            && payload["threadId"] == json!(self.thread)
            && payload["seq"].as_i64().unwrap_or_default() > self.last()
        {
            self.add(payload);
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
}

/// Reads pushes until `done`, for at most ten seconds.
async fn until(connection: &mut Connection<UnixStream>, log: &mut Log, done: impl Fn(&Log) -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(10), async {
        while !done(log) {
            let push = connection.next_push().await.unwrap_or_else(|| panic!("closed"));
            log.live(&push);
        }
    })
    .await;
    assert!(waited.is_ok(), "timed out; have {:?}", log.kinds());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_goes_on_while_the_client_is_away_and_catches_up_exactly_once() {
    let host = start("session");
    let mut log = Log::default();
    let mut first = open(&host.socket).await;
    assert_eq!(first.info.version, "9.9.9");

    let project = call(&mut first, "wb_project_add", json!({ "path": host.repo }), &mut log).await;
    let created = call(
        &mut first,
        "wb_thread_create",
        json!({
            "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "use a tool", "images": [], "worktree": null,
        }),
        &mut log,
    )
    .await;
    log.thread = created["id"].as_str().unwrap_or_default().to_string();
    let thread = log.thread.clone();
    // What came before the id was known: the session's log so far, then live past it (the app opens a session so).
    let (snapshot, pushed) = first
        .call("wb_thread_events", json!({ "threadId": thread, "afterSeq": 0 }))
        .await;
    for event in snapshot
        .unwrap_or_else(|e| panic!("{e}"))
        .as_array()
        .into_iter()
        .flatten()
    {
        log.add(event);
    }
    for push in pushed {
        log.live(&push);
    }

    // The approval arrives live; the answer goes back over the socket.
    until(&mut first, &mut log, |l| l.kinds().contains(&"approval")).await;
    let approval = log
        .events
        .values()
        .find(|e| e["event"]["kind"] == "approval")
        .cloned()
        .unwrap_or_default();
    call(
        &mut first,
        "wb_approve",
        json!({ "threadId": thread, "requestId": approval["event"]["requestId"], "decision": "allow",
                "message": null, "suggestion": null, "answers": null }),
        &mut log,
    )
    .await;
    until(&mut first, &mut log, |l| l.turn_ends() == 1).await;

    // A turn that keeps working, with background work that ends on its own; the client leaves mid-turn.
    call(
        &mut first,
        "wb_send",
        json!({ "threadId": thread, "text": "slow background work", "images": [] }),
        &mut log,
    )
    .await;
    let before = log.last();
    until(&mut first, &mut log, |l| {
        l.events.range(before + 1..).any(|(_, e)| e["event"]["kind"] == "text")
    })
    .await;
    drop(first);
    let left_at = log.last();
    tokio::time::sleep(Duration::from_millis(800)).await;
    // No client: the agent still runs and the log still grows.
    let stored = host
        .api
        .thread_events(thread.clone(), left_at)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!stored.is_empty(), "nothing happened while the client was away");

    // Back: subscribed by the hello, then the log after the last seq, then live past it.
    let mut second = open(&host.socket).await;
    let (missed, pushed_meanwhile) = second
        .call("wb_thread_events", json!({ "threadId": thread, "afterSeq": left_at }))
        .await;
    let missed = missed.unwrap_or_else(|e| panic!("{e}"));
    let missed = missed.as_array().cloned().unwrap_or_default();
    assert!(missed.len() >= stored.len(), "{missed:?}");
    for event in &missed {
        log.add(event);
    }
    for push in pushed_meanwhile {
        log.live(&push);
    }

    // The same turn, still running: ordinary Send reaches it without another command.
    let before_send = log.last();
    call(
        &mut second,
        "wb_send",
        json!({ "threadId": thread, "text": "wrap up", "images": [] }),
        &mut log,
    )
    .await;
    let queued = |l: &Log| {
        l.events
            .range(before_send + 1..)
            .find(|(_, e)| e["event"]["kind"] == "user")
            .map(|(seq, _)| *seq)
    };
    until(&mut second, &mut log, |l| queued(l).is_some()).await;
    let seq = queued(&log).unwrap_or_default();
    until(&mut second, &mut log, |l| l.turn_ends() == 2).await;
    for kind in ["queueSteer", "read"] {
        assert_eq!(
            log.events
                .values()
                .filter(|e| e["event"]["kind"] == kind && e["event"]["seq"] == seq)
                .count(),
            1
        );
    }
    assert!(
        log.events.values().any(|e| e["event"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("Heard mid-turn: wrap up"))),
        "{:?}",
        log.kinds()
    );

    // Every event once, none missing: what the client holds is the whole log.
    assert!(log.duplicates.is_empty(), "duplicates: {:?}", log.duplicates);
    let (whole, _) = second
        .call("wb_thread_events", json!({ "threadId": thread, "afterSeq": 0 }))
        .await;
    let whole = whole.unwrap_or_else(|e| panic!("{e}"));
    let whole: BTreeMap<i64, Value> = whole
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e["seq"].as_i64() <= Some(log.last()))
        .map(|e| (e["seq"].as_i64().unwrap_or_default(), e))
        .collect();
    assert_eq!(log.events, whole);
    let seqs: Vec<i64> = log.events.keys().copied().collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");

    let (status, _) = second.call("host_status", Value::Null).await;
    let status = status.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((&status["machine"], &status["org"]), (&json!("box"), &json!("org")));

    call(&mut second, "wb_stop", json!({ "threadId": thread }), &mut log).await;
    let _ = host.stop.send(true);
    host.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&host.root);
}

#[tokio::test]
async fn refuses_a_wrong_token_another_protocol_and_browsers() {
    let host = start("refuse");

    let stream = UnixStream::connect(&host.socket)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let wrong = connect(stream, "not-the-token").await.err();
    assert_eq!(wrong.as_deref(), Some("Wrong token."));

    // Another protocol: refused, but told which host this is.
    let stream = UnixStream::connect(&host.socket)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    use futures::SinkExt as _;
    let hello = json!({ "hello": { "token": TOKEN, "protocol": PROTOCOL + 1 } });
    let _ = ws
        .send(tokio_tungstenite::tungstenite::Message::text(hello.to_string()))
        .await;
    let answer = nexus_host::client::next_json(&mut ws).await.unwrap_or_default();
    assert!(
        answer["error"].as_str().is_some_and(|e| e.contains("update")),
        "{answer}"
    );
    assert_eq!(answer["hello"]["version"], "9.9.9");
    assert!(nexus_host::client::next_json(&mut ws).await.is_none());

    // A browser's upgrade carries Origin: refused before any hello.
    let stream = UnixStream::connect(&host.socket)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let mut request = "ws://localhost/"
        .into_client_request()
        .unwrap_or_else(|e| panic!("{e}"));
    request.headers_mut().insert(
        "origin",
        "https://evil.example".parse().unwrap_or_else(|e| panic!("{e:?}")),
    );
    let browser = tokio_tungstenite::client_async(request, stream).await;
    match browser {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => assert_eq!(response.status(), 403),
        other => panic!("not refused: {:?}", other.map(|_| ())),
    }

    // A request before the hello is not one.
    let stream = UnixStream::connect(&host.socket)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let _ = ws
        .send(tokio_tungstenite::tungstenite::Message::text(
            json!({ "id": 1, "call": "wb_overview" }).to_string(),
        ))
        .await;
    let answer = nexus_host::client::next_json(&mut ws).await.unwrap_or_default();
    assert!(answer.get("error").is_some(), "{answer}");

    let _ = host.stop.send(true);
    let _ = std::fs::remove_dir_all(&host.root);
}

/// A client that says hello and then stops reading (a laptop asleep with its tunnel open) is dropped once it has taken
/// no bytes for the stall limit, instead of holding its connection until the network gives up.
#[tokio::test]
async fn drops_a_client_that_stops_reading() {
    use nexus_workbench::Emit;
    use nexus_workbench::event::Delta;
    let host = start("stall");
    let (near, far) = UnixStream::pair().unwrap_or_else(|e| panic!("{e}"));
    let (_stop, stopped) = watch::channel(false);
    let served = tokio::spawn(nexus_host::server::connection_with(
        far,
        host.server.clone(),
        stopped,
        Duration::from_millis(300),
    ));
    // Hello answered: subscribed. From here on the client reads nothing.
    let quiet = connect(near, TOKEN).await.unwrap_or_else(|e| panic!("{e}"));
    // Far more than the socket's buffers hold, and far fewer than the push queue (which would close it as lagging).
    for n in 0..100 {
        host.hub.publish(&Emit::Delta(Delta {
            thread_id: "t".into(),
            id: format!("d{n}"),
            text: "x".repeat(64 * 1024),
            parent: None,
            offset: None,
        }));
    }
    let ended = tokio::time::timeout(Duration::from_secs(10), served).await;
    assert!(ended.is_ok(), "the host still holds a client that stopped reading");
    drop(quiet);
    let _ = host.stop.send(true);
    let _ = std::fs::remove_dir_all(&host.root);
}

/// The real `serve`: its private folder and socket, one host at a time, the hello, and a clean stop.
#[tokio::test]
async fn serve_keeps_its_files_private_and_stops_cleanly() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = root("serve");
    let paths = Paths::at(root.join("h"));
    let (stop, stopped) = watch::channel(false);
    let (ready, listening) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(nexus_host::serve(
        paths.clone(),
        root.join("home"),
        Listen::Socket,
        stopped,
        Shell::sh(),
        move |at: &Listening| {
            let _ = ready.send(at.clone());
        },
    ));
    let listening = listening.await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(listening, Listening::Socket(paths.socket()));
    let mode = |p: PathBuf| {
        std::fs::metadata(p)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0)
    };
    assert_eq!(mode(paths.dir.clone()), 0o700);
    assert_eq!(mode(paths.token()), 0o600);
    assert_eq!(mode(paths.socket()), 0o600);

    // A second host for the same person stops, after the lock's wait for a probe.
    let (_, second_stop) = watch::channel(false);
    let second = nexus_host::serve(
        paths.clone(),
        root.join("home"),
        Listen::Socket,
        second_stop,
        Shell::sh(),
        |_| {},
    )
    .await;
    assert_eq!(second, Err("A Nexus host is already running for this user.".into()));

    let token = std::fs::read_to_string(paths.token()).unwrap_or_default();
    let stream = UnixStream::connect(paths.socket())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let mut connection = connect(stream, token.trim()).await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(connection.info.protocol, PROTOCOL);
    assert_eq!(connection.info.org, None);
    let (overview, _) = connection.call("wb_overview", json!({})).await;
    let overview = overview.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(overview["persistent"], true);
    assert_eq!(overview["agentSubscriptions"], false);
    let (status, _) = connection.call("host_status", Value::Null).await;
    assert_eq!(status.map(|s| s["sessions"].clone()), Ok(json!([])));

    let _ = stop.send(true);
    let served = tokio::time::timeout(Duration::from_secs(10), serving).await;
    assert!(matches!(served, Ok(Ok(Ok(())))), "{served:?}");
    assert!(!paths.socket().exists());
    // The client hears the host leave.
    assert!(connection.next_push().await.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

/// A host that holds its lock and socket but never answers (stuck) does not hang `status --json`: the app's install
/// script runs it to learn whether a host runs. It still says the host runs, with why it could not ask.
#[tokio::test]
async fn status_does_not_wait_forever_on_a_host_that_does_not_answer() {
    let root = root("mute");
    let paths = Paths::at(root.join("h"));
    std::fs::create_dir_all(paths.run()).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(paths.token(), TOKEN).unwrap_or_else(|e| panic!("{e}"));
    let _lock = nexus_host::files::Lock::take(&paths.lock()).unwrap_or_else(|e| panic!("{e}"));
    // Accepts connections and never reads or writes a byte.
    let listener = tokio::net::UnixListener::bind(paths.socket()).unwrap_or_else(|e| panic!("{e}"));
    let mute = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    let status = nexus_workbench::process::async_command(env!("CARGO_BIN_EXE_nexus-host"))
        .args(["status", "--json"])
        .env("NEXUS_HOST_DIR", &paths.dir)
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(Duration::from_secs(20), status)
        .await
        .unwrap_or_else(|_| panic!("status waited on a host that does not answer"))
        .unwrap_or_else(|e| panic!("{e}"));
    let status: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(status["running"], true);
    // The timeout path, not a fast connect error (which is a string too).
    assert_eq!(status["error"], "The running host did not answer in time.", "{status}");
    mute.abort();
    let _ = std::fs::remove_dir_all(&root);
}

/// The session's log so far, then live past it (the app opens a session so).
async fn follow(connection: &mut Connection<UnixStream>, log: &mut Log) {
    let (snapshot, pushed) = connection
        .call("wb_thread_events", json!({ "threadId": log.thread, "afterSeq": 0 }))
        .await;
    for event in snapshot.unwrap_or_default().as_array().into_iter().flatten() {
        log.add(event);
    }
    for push in pushed {
        log.live(&push);
    }
}

/// Starts a session as the app does: `log.thread` is its id.
async fn start_session(connection: &mut Connection<UnixStream>, repo: &PathBuf, via: &str, text: &str, log: &mut Log) {
    let project = call(connection, "wb_project_add", json!({ "path": repo }), log).await;
    let created = call(
        connection,
        "wb_thread_create",
        json!({
            "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": via, "text": text, "images": [], "worktree": null,
        }),
        log,
    )
    .await;
    log.thread = created["id"].as_str().unwrap_or_default().to_string();
}

fn said(log: &Log, needle: &str) -> bool {
    log.events
        .values()
        .any(|e| e["event"]["text"].as_str().is_some_and(|t| t.contains(needle)))
}

/// `agent.env` in the host's folder reaches the agent as the plan's token only on the subscription route, and a token
/// changed in the file applies to the next session with no restart. The stand-in says whether each is set, never what.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_env_reaches_the_agent_on_the_plan_route_only() {
    let host = start("agentenv");
    let file = host.root.join("agent.env");
    let write = |text: &str, mode: u32| {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::write(&file, text).unwrap_or_else(|e| panic!("{e}"));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap_or_else(|e| panic!("{e}"));
    };
    let ask = |via: &'static str| {
        let (repo, socket) = (host.repo.clone(), host.socket.clone());
        async move {
            let mut log = Log::default();
            let mut connection = open(&socket).await;
            start_session(&mut connection, &repo, via, "tokens", &mut log).await;
            follow(&mut connection, &mut log).await;
            until(&mut connection, &mut log, |l| l.turn_ends() == 1).await;
            log
        }
    };
    write(
        "# cloud\nCLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-fake\nGH_TOKEN=ghp_fake\n",
        0o600,
    );
    let plan = ask("subscription").await;
    assert!(said(&plan, "Claude token set. GitHub token set."), "{:?}", plan.events);
    let gateway = ask("nexus").await;
    assert!(
        said(&gateway, "Claude token unset. GitHub token set."),
        "{:?}",
        gateway.events
    );

    // Replaced and read fresh; a file others can read is refused whole; removed means nothing.
    write("GH_TOKEN=ghp_new\n", 0o600);
    let changed = ask("subscription").await;
    assert!(
        said(&changed, "Claude token unset. GitHub token set."),
        "{:?}",
        changed.events
    );
    write("CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-fake\nGH_TOKEN=ghp_fake\n", 0o644);
    let refused = ask("subscription").await;
    assert!(
        said(&refused, "Claude token unset. GitHub token unset."),
        "{:?}",
        refused.events
    );
    let _ = std::fs::remove_file(&file);
    let gone = ask("subscription").await;
    assert!(
        said(&gone, "Claude token unset. GitHub token unset."),
        "{:?}",
        gone.events
    );

    let _ = host.stop.send(true);
    host.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&host.root);
}

/// `host_status` says how many turns run and since when the host has had none: null while one runs (here, one waiting
/// on an approval), the moment the last ended after.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_status_reports_idle_time() {
    let before_start = unix_ms();
    let host = start("idle");
    let mut connection = open(&host.socket).await;
    let mut log = Log::default();

    let status = call(&mut connection, "host_status", Value::Null, &mut log).await;
    assert_eq!(status["runningTurns"], 0);
    assert_eq!(status["activeWork"], false);
    let at_start = status["idleSince"].as_i64().unwrap_or_default();
    assert!(at_start >= before_start && at_start <= unix_ms(), "{status}");
    // Older apps read the same fields as before.
    assert!(status["sessions"].as_array().is_some_and(Vec::is_empty));
    // The relay's own reads are not the person using the machine.
    call(&mut connection, "wb_overview", Value::Null, &mut log).await;
    let status = call(&mut connection, "host_status", Value::Null, &mut log).await;
    assert!(status["interactedAt"].is_null(), "{status}");

    let started_at = unix_ms();
    start_session(&mut connection, &host.repo, "nexus", "use a tool", &mut log).await;
    let thread = log.thread.clone();
    follow(&mut connection, &mut log).await;
    until(&mut connection, &mut log, |l| l.kinds().contains(&"approval")).await;
    let status = call(&mut connection, "host_status", Value::Null, &mut log).await;
    assert_eq!(status["runningTurns"], 1, "{status}");
    assert_eq!(status["activeWork"], true);
    assert!(status["idleSince"].is_null(), "{status}");
    let interacted = status["interactedAt"].as_i64().unwrap_or_default();
    assert!(interacted >= started_at && interacted <= unix_ms(), "{status}");
    // For a cloud machine's sleep, the agent waiting for the person's answer is idle.
    let beat = host.server.heartbeat().unwrap_or_else(|e| panic!("{e}"));
    assert!(!beat.busy, "{beat:?}");
    assert!(beat.agent_idle_since.is_some_and(|t| t >= started_at), "{beat:?}");
    assert_eq!(beat.interacted_at, Some(interacted));

    let approval = log
        .events
        .values()
        .find(|e| e["event"]["kind"] == "approval")
        .cloned()
        .unwrap_or_default();
    let approved_at = unix_ms();
    call(
        &mut connection,
        "wb_approve",
        json!({ "threadId": thread, "requestId": approval["event"]["requestId"], "decision": "allow",
                "message": null, "suggestion": null, "answers": null }),
        &mut log,
    )
    .await;
    until(&mut connection, &mut log, |l| l.turn_ends() == 1).await;
    // The session's status settles just after its turn end is stored.
    let mut idle = Value::Null;
    for _ in 0..50 {
        let status = call(&mut connection, "host_status", Value::Null, &mut log).await;
        if status["runningTurns"] == 0 {
            idle = status;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(idle["activeWork"], false);
    let since = idle["idleSince"].as_i64().unwrap_or_default();
    assert!(
        since >= approved_at && since <= unix_ms(),
        "{idle} (approved at {approved_at})"
    );

    let _ = host.stop.send(true);
    host.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&host.root);
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or_default())
        .unwrap_or_default()
}

/// One HTTP request a stand-in Gateway took: its first line, `Authorization` and JSON body. It answers 204.
async fn take_one(listener: tokio::net::TcpListener) -> (String, String, Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut stream, _) = listener.accept().await.unwrap_or_else(|e| panic!("{e}"));
    let mut request = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let read = stream.read(&mut chunk).await.unwrap_or_default();
        assert!(read > 0, "the request ended early");
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
        let read = stream.read(&mut chunk).await.unwrap_or_default();
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
    }
    let _ = stream
        .write_all(b"HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n")
        .await;
    let body = serde_json::from_slice(&request[head_end..]).unwrap_or(Value::Null);
    (
        head.lines().next().unwrap_or_default().to_string(),
        header("authorization"),
        body,
    )
}

/// On a cloud machine (Gateway wrote `cloud.json` and the host's key), the heartbeat posts the sessions' activity
/// with that key. Without `cloud.json` it sends nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cloud_machine_tells_gateway_its_activity() {
    use nexus_host::cloud::{Sources, beat};
    use nexus_host::files::Credentials;

    let host = start("beat");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let url = format!("http://{}", listener.local_addr().unwrap_or_else(|e| panic!("{e}")));
    let sources = Sources {
        cloud: host.root.join("cloud.json"),
        credentials: host.root.join("credentials"),
        memory_events: host.root.join("memory.events"),
        vmstat: host.root.join("vmstat"),
    };
    Credentials {
        gateway: "http://127.0.0.1:1".into(),
        token: "nxs_machine".into(),
        org: Some("org".into()),
        label: "cloud".into(),
    }
    .save(&sources.credentials)
    .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&sources.vmstat, "oom_kill 3\n").unwrap_or_else(|e| panic!("{e}"));

    // Not a cloud machine yet: nothing is sent (the credentials' Gateway would refuse the connection).
    beat(&host.server, &sources).await;

    std::fs::write(
        &sources.cloud,
        json!({ "machineId": "m_1", "gatewayUrl": url }).to_string(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    host.hub.interacted(1_700_000_000_000);
    let taken = tokio::spawn(take_one(listener));
    beat(&host.server, &sources).await;
    let (line, auth, body) = taken.await.unwrap_or_else(|e| panic!("{e}"));
    assert!(line.starts_with("POST /v1/cloud/machines/m_1/activity "), "{line}");
    assert_eq!(auth, "Bearer nxs_machine");
    assert_eq!(body["busy"], false, "{body}");
    assert_eq!(body["interactedAt"], "2023-11-14T22:13:20.000Z", "{body}");
    assert!(body["agentIdleSince"].is_string(), "{body}");
    assert_eq!(body["oomKills"], 3, "{body}");

    let _ = host.stop.send(true);
    host.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&host.root);
}
