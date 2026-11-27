//! [`Machines`] against a stand-in Nexus host over an in-memory stream: the same WebSocket, hello, calls and pushes as
//! nexus-host's server, with no SSH and no network. It covers the connection's life (connect, a host of another
//! protocol, a dropped link, a removed server) and a session's (start it, follow its events, catch up after a drop).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::{SinkExt as _, StreamExt as _};
use nexus_remote::machine::Opened;
use nexus_workbench::api::{HostInfo, PROTOCOL};
use tokio_tungstenite::tungstenite::Message;

use super::*;

/// The stand-in host's sessions: what `wb_overview` and `wb_thread_page` read, and the pushes it sends.
#[derive(Default)]
pub(crate) struct Host {
    pub(crate) threads: Vec<Value>,
    /// The texts `wb_send` took, in order.
    pub(crate) sent: Vec<String>,
    /// The next `wb_send` is refused by the host (and not taken).
    pub(crate) refuse_send: bool,
    /// What `wb_cloud_setup` answers, in turn (the last one stays); none: an older host, which does not know it.
    pub(crate) setup: Vec<Value>,
    /// How many `wb_cloud_setup_retry` calls came.
    pub(crate) setup_retries: u32,
    /// Project `p1`'s commands (`wb_project_scripts`, as a JSON list) and its setup script.
    pub(crate) scripts: Value,
    pub(crate) project_setup: Option<String>,
    /// The app's hand-offs it took: `host_mcp_servers` and `host_app_reply`, with their arguments.
    pub(crate) handed: Vec<(String, Value)>,
    events: Vec<Value>,
    /// The connected client's push channel.
    client: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
}

/// A Nexus host in memory: the cloud machines' tests (`cloud/tests.rs`) reach theirs through it too.
pub(crate) struct Fake {
    /// The protocol its hello says.
    protocol: u32,
    /// No connection opens while false: a network that is down.
    pub(crate) up: AtomicBool,
    pub(crate) opened: AtomicU32,
    pub(crate) host: Arc<Mutex<Host>>,
    /// The next `wb_overview` reply waits for `release`: a read that lands after a newer answer.
    late_overview: AtomicBool,
    release: tokio::sync::Notify,
    /// How many `wb_overview` calls came.
    overviews: AtomicU32,
    /// What its hello says the host does.
    features: Vec<String>,
}

impl Fake {
    pub(crate) fn new(protocol: u32) -> Arc<Fake> {
        Fake::with_features(protocol, &[nexus_workbench::clone::FEATURE])
    }

    /// A host whose hello names `features`.
    pub(crate) fn with_features(protocol: u32, features: &[&str]) -> Arc<Fake> {
        Arc::new(Fake {
            protocol,
            up: AtomicBool::new(true),
            opened: AtomicU32::new(0),
            host: Arc::default(),
            late_overview: AtomicBool::new(false),
            release: tokio::sync::Notify::new(),
            overviews: AtomicU32::new(0),
            features: features.iter().map(|feature| feature.to_string()).collect(),
        })
    }

    /// The host pushes `event` for session `thread`, numbered after its last.
    pub(crate) fn push_event(&self, thread: &str, event: Value) {
        let Ok(mut host) = self.host.lock() else { return };
        let seq = host.events.len() as i64 + 1;
        let stored = json!({ "threadId": thread, "seq": seq, "at": seq, "event": event });
        host.events.push(stored.clone());
        if let Some(client) = &host.client {
            let _ = client.send(json!({ "event": "wb:event", "payload": stored }));
        }
    }
}

impl Fake {
    /// The host pushes `event` with `payload` to the connected app.
    pub(crate) fn push(&self, event: &str, payload: Value) {
        let Ok(host) = self.host.lock() else { return };
        if let Some(client) = &host.client {
            let _ = client.send(json!({ "event": event, "payload": payload }));
        }
    }

    /// The host streams `text` of message `id` in session `thread`.
    fn push_delta(&self, thread: &str, id: &str, text: &str, offset: Option<u64>) {
        let Ok(host) = self.host.lock() else { return };
        let delta = json!({ "threadId": thread, "id": id, "text": text, "parent": null, "offset": offset });
        if let Some(client) = &host.client {
            let _ = client.send(json!({ "event": "wb:delta", "payload": delta }));
        }
    }
}

fn thread(id: &str, status: &str) -> Value {
    json!({
        "id": id, "projectId": "p1", "title": "Fix the build", "agent": "claude", "model": null,
        "permissionMode": "default", "via": "nexus", "status": status, "costUsd": 0.0, "nexusSessions": [],
        "contextTokens": null, "contextWindow": null, "createdAt": 1, "updatedAt": 2, "archived": false,
    })
}

/// Answers one call as nexus-host's workbench would, for the calls these tests make.
fn answer(fake: &Fake, call: &str, args: &Value) -> Result<Value, String> {
    let Ok(mut host) = fake.host.lock() else {
        return Err("poisoned".into());
    };
    match call {
        "host_status" => Ok(json!({ "sessions": [] })),
        "wb_overview" => Ok(json!({
            "projects": [{ "id": "p1", "name": "api", "path": "/srv/api", "branch": "main", "addedAt": 1 }],
            "threads": host.threads, "agents": [], "persistent": true, "agentSubscriptions": false,
        })),
        "wb_thread_create" => {
            // A request id names the session, as nexus-host's workbench does (`t_<request id>`).
            let id = args["requestId"]
                .as_str()
                .map_or_else(|| "t1".to_string(), |request| format!("t_{request}"));
            let summary = thread(&id, "running");
            host.threads.push(summary.clone());
            if let Some(client) = &host.client {
                let _ = client.send(json!({ "event": "wb:thread", "payload": summary }));
            }
            Ok(summary)
        }
        // The page before `beforeSeq` (the latest without it), `limit` events long, as nexus-workbench's `page.rs`.
        "wb_thread_page" => {
            let before = args["beforeSeq"].as_i64().unwrap_or(i64::MAX);
            let limit = args["limit"].as_u64().map_or(usize::MAX, |limit| limit as usize);
            let below: Vec<Value> = host
                .events
                .iter()
                .filter(|event| event["seq"].as_i64().unwrap_or(0) < before)
                .cloned()
                .collect();
            let page = below[below.len().saturating_sub(limit)..].to_vec();
            let more = page.first().is_some_and(|first| first["seq"].as_i64().unwrap_or(0) > 1);
            Ok(json!({ "events": page, "carried": [], "more": more }))
        }
        "wb_thread_events" => {
            let after = args["afterSeq"].as_i64().unwrap_or(0);
            Ok(Value::Array(
                host.events
                    .iter()
                    .filter(|event| event["seq"].as_i64().unwrap_or(0) > after)
                    .cloned()
                    .collect(),
            ))
        }
        "wb_send" if std::mem::take(&mut host.refuse_send) => Err("The host refused the message.".into()),
        "wb_send" => {
            host.sent.push(args["text"].as_str().unwrap_or_default().to_string());
            Ok(Value::Null)
        }
        "wb_project_add" => Ok(json!({
            "id": "p2", "name": "web", "path": args["path"], "branch": null, "addedAt": 2,
        })),
        // A clone of a repository git cannot read fails with git's words, as nexus-workbench's `clone.rs` says them.
        "wb_project_clone" if args["source"] == "owner/missing" => {
            Err("Couldn't find the repository owner/missing on GitHub.".into())
        }
        "wb_project_clone" => Ok(json!({
            "id": "p3", "name": "app", "path": format!("{}/app", args["parent"].as_str().unwrap_or_default()),
            "branch": "main", "addedAt": 3,
        })),
        // Git is receiving objects for request `r1`; any other request has said nothing yet.
        "wb_project_clone_progress" if args["requestId"] == "r1" => {
            Ok(json!({ "stage": "Receiving objects", "percent": 45 }))
        }
        "wb_project_clone_progress" => Ok(Value::Null),
        "wb_worktree_cleanup" => Ok(json!({
            "at": 5, "kept": [],
            "removed": [{ "threadId": "t1", "title": "Fix the build", "path": "/w/t1", "branch": "nexus/t1", "rule": "archived" }],
        })),
        "wb_changes" | "wb_diff" => Ok(json!({
            "diff": "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-a\n+b\n",
            "truncated": false,
        })),
        "wb_changed_turns" => Ok(json!([4, 9])),
        "wb_diff_targets" => Ok(json!({
            "base": "origin/main",
            "commits": [{ "id": "a1b2c3d4e5", "short": "a1b2c3d", "subject": "Fix", "author": "Ada", "time": 1 }],
        })),
        "wb_git_state" => Ok(json!({
            "path": "/srv/api", "branch": "main", "branches": [], "repos": null, "upstream": null, "base": null,
            "uncommitted": { "files": 1, "insertions": 1, "deletions": 1 },
        })),
        "wb_files" => Ok(json!({ "files": ["README.md", "src/a.rs"], "truncated": false })),
        "wb_file_read" => {
            Ok(json!({ "path": args["path"], "text": "b\n", "binary": false, "truncated": false, "size": 2 }))
        }
        // A five-byte image, in pieces of three bytes when asked from an offset; a Markdown file whole.
        "wb_file_preview" if args["path"] == "logo.png" => {
            let bytes = [1u8, 2, 3, 4, 5];
            let offset = args["offset"].as_u64();
            let from = offset.unwrap_or(0) as usize;
            let to = if offset.is_some() { (from + 3).min(5) } else { 5 };
            let mut preview = json!({
                "path": "/srv/api/logo.png", "mediaType": "image/png", "size": 5, "outsideProject": false,
                "text": null, "data": base64_of(&bytes[from..to]), "truncated": false, "status": "ready",
            });
            if let Some(offset) = offset {
                preview["offset"] = offset.into();
            }
            Ok(preview)
        }
        "wb_file_preview" => Ok(json!({
            "path": format!("/srv/api/{}", args["path"].as_str().unwrap_or_default()), "mediaType": "text/markdown",
            "size": 7, "outsideProject": false, "text": "# Notes", "data": null, "truncated": false, "status": "ready",
        })),
        "wb_commit" => Ok(json!({
            "path": "/srv/api", "branch": "main", "branches": [], "repos": null, "upstream": null, "base": null,
            "uncommitted": { "files": 0, "insertions": 0, "deletions": 0 },
        })),
        "wb_cloud_setup" if !host.setup.is_empty() => {
            let view = host.setup[0].clone();
            if host.setup.len() > 1 {
                host.setup.remove(0);
            }
            Ok(view)
        }
        // A project's commands and setup script, kept as nexus-workbench's `scripts.rs` and `setup.rs` keep them.
        "wb_project_scripts" if host.scripts.is_null() => Ok(json!([])),
        "wb_project_scripts" => Ok(host.scripts.clone()),
        "wb_project_scripts_set" => {
            host.scripts = args["scripts"].clone();
            Ok(host.scripts.clone())
        }
        "wb_project_setup" => Ok(json!(host.project_setup)),
        "wb_project_setup_set" => {
            host.project_setup = args["script"].as_str().map(str::to_string);
            Ok(json!(host.project_setup))
        }
        // An older host: it does not know a project's pin and archive.
        "wb_project_update" => Err("Unknown command wb_project_update.".into()),
        "wb_cloud_setup_retry" if !host.setup.is_empty() => {
            host.setup_retries += 1;
            Ok(host.setup[0].clone())
        }
        "host_mcp_servers" | "host_app_reply" => {
            host.handed.push((call.to_string(), args.clone()));
            Ok(Value::Null)
        }
        other => Err(format!("{other} is not served here")),
    }
}

/// Serves one connection: the hello, then calls and pushes, until the client goes.
pub(crate) async fn serve(fake: Arc<Fake>, stream: tokio::io::DuplexStream) {
    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let Some(Ok(Message::Text(hello))) = ws.next().await else {
        return;
    };
    let hello: Value = serde_json::from_str(hello.as_str()).unwrap_or_default();
    let info = HostInfo {
        version: HOST_VERSION.into(),
        protocol: fake.protocol,
        machine: "box".into(),
        org: Some("Acme".into()),
        features: fake.features.clone(),
    };
    if hello["hello"]["token"] != "secret" || fake.protocol != PROTOCOL {
        let reply = json!({ "hello": info, "error": "This host speaks another protocol." });
        let _ = ws.send(Message::text(reply.to_string())).await;
        return;
    }
    // Subscribed before the hello is answered, as nexus-host's server is: a push sent once the client reads the hello
    // as connected reaches it.
    let (pushes, mut pushed) = tokio::sync::mpsc::unbounded_channel();
    if let Ok(mut host) = fake.host.lock() {
        host.client = Some(pushes);
    }
    let _ = ws.send(Message::text(json!({ "hello": info }).to_string())).await;
    let mut held = None;
    loop {
        tokio::select! {
            message = ws.next() => {
                let Some(Ok(Message::Text(text))) = message else { return };
                let request: Value = serde_json::from_str(text.as_str()).unwrap_or_default();
                let call = request["call"].as_str().unwrap_or_default();
                let reply = match answer(&fake, call, &request["args"]) {
                    Ok(value) => json!({ "id": request["id"], "ok": value }),
                    Err(error) => json!({ "id": request["id"], "error": error }),
                };
                if call == "wb_overview" {
                    fake.overviews.fetch_add(1, Ordering::SeqCst);
                }
                if call == "wb_overview" && fake.late_overview.swap(false, Ordering::SeqCst) {
                    held = Some(reply);
                    continue;
                }
                if ws.send(Message::text(reply.to_string())).await.is_err() {
                    return;
                }
            }
            () = fake.release.notified(), if held.is_some() => {
                if let Some(late) = held.take()
                    && ws.send(Message::text(late.to_string())).await.is_err()
                {
                    return;
                }
            }
            push = pushed.recv() => {
                let Some(push) = push else { return };
                if ws.send(Message::text(push.to_string())).await.is_err() {
                    return;
                }
            }
        }
    }
}

struct FakeLink(Arc<Fake>);

impl Link for FakeLink {
    fn open(&self) -> BoxFuture<'_, Result<Opened, String>> {
        Box::pin(async move {
            if !self.0.up.load(Ordering::SeqCst) {
                return Err("ssh: connect to host box port 22: Network is unreachable".into());
            }
            self.0.opened.fetch_add(1, Ordering::SeqCst);
            let (client, server) = tokio::io::duplex(1 << 16);
            tokio::spawn(serve(self.0.clone(), server));
            Ok(Opened {
                stream: Box::new(client),
                token: "secret".into(),
                version: None,
                keep: None,
                sign_in: None,
            })
        })
    }

    fn update(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

pub(crate) fn settings(servers: &[(&str, &str)]) -> Settings {
    Settings {
        machines: servers
            .iter()
            .map(|(id, ssh)| MachineSetting {
                id: id.to_string(),
                name: id.to_string(),
                kind: "ssh".into(),
                ssh: ssh.to_string(),
                cloud: None,
            })
            .collect(),
        ..Settings::default()
    }
}

pub(crate) fn machines(fake: &Arc<Fake>) -> Arc<Machines> {
    let fake = fake.clone();
    let links: Links = Arc::new(move |_: &MachineSetting| Arc::new(FakeLink(fake.clone())) as Arc<dyn Link>);
    let fast = Backoff {
        first: Duration::from_millis(20),
        max: Duration::from_millis(50),
        healthy: Duration::from_secs(60),
    };
    Machines::new(links, fast, tokio::runtime::Handle::current())
}

/// Waits up to five seconds for `done`.
pub(crate) async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

fn status(machines: &Machines) -> Status {
    machines
        .servers()
        .first()
        .map(|server| server.status.clone())
        .unwrap_or(Status::Closed)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_settings_connect_ssh_without_loading_cloud_machines() {
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    let mut saved = settings(&[("m1", "me@box"), ("m2", "cloud")]);
    saved.machines[1].kind = "cloud".into();
    machines.sync_device(&saved);
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;
    assert_eq!(machines.servers().len(), 1);
    assert!(machines.cloud().is_empty());

    saved.machines.remove(0);
    machines.sync_device(&saved);
    assert!(machines.servers().is_empty());
    assert!(machines.cloud().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connects_and_lists_the_servers_projects_under_it() {
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    let mut changes = machines.subscribe();
    machines.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;
    until("the overview", || !machines.projects().is_empty()).await;
    let projects = machines.projects();
    assert_eq!(projects[0].name, "api");
    assert_eq!(projects[0].machine.as_deref(), Some("m1"));
    assert_eq!(machines.machine_of("p1").as_deref(), Some("m1"));
    assert!(changes.try_recv().is_ok_and(|change| change.machine == "m1"));
    let Status::Connected { host, update } = status(&machines) else {
        unreachable!()
    };
    assert_eq!((host.org.as_deref(), update), (Some("Acme"), None));
    // The server leaves the list: its connection closes, and nothing of it stays.
    machines.sync(&settings(&[]));
    assert!(machines.servers().is_empty() && machines.projects().is_empty());
    assert_eq!(machines.machine_of("p1"), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_of_another_protocol_needs_an_update_and_refuses_calls() {
    let fake = Fake::new(PROTOCOL + 1);
    let machines = machines(&fake);
    machines.sync(&settings(&[("m1", "me@box")]));
    until("needs update", || {
        matches!(status(&machines), Status::NeedsUpdate { .. })
    })
    .await;
    let view = crate::machine_view::status_view(&status(&machines));
    assert_eq!(
        (view.label, view.action),
        ("Needs update", Some(crate::machine_view::Action::Update))
    );
    let refused = machines.call("m1", "wb_overview", json!({})).await;
    assert_eq!(
        refused.ok(),
        Some(Err(
            "This machine runs an older Nexus host: update it first.".to_string()
        ))
    );
    machines.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn starts_a_session_there_and_follows_its_events_through_a_drop() {
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    machines.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;

    // This computer's commands stay here.
    let local = machines.call("m1", "wb_viewing", json!({ "threadId": null })).await;
    assert_eq!(
        local.ok(),
        Some(Err("wb_viewing is about this computer, not a machine.".to_string()))
    );

    let created = machines
        .call(
            "m1",
            "wb_thread_create",
            json!({ "projectId": "p1", "agent": "claude", "text": "Fix the build", "permissionMode": "default", "via": "nexus" }),
        )
        .await;
    assert_eq!(
        created.ok().and_then(Result::ok).map(|t| t["id"].clone()),
        Some(json!("t1"))
    );
    assert_eq!(machines.machine_of("t1").as_deref(), Some("m1"));
    assert_eq!(machines.servers()[0].busy, 1, "a running session makes the server busy");
    fake.push_event("t1", json!({ "kind": "user", "text": "Fix the build" }));

    // A window opens it: the first page, then each event as it is pushed.
    assert!(
        machines.transcript("m1", "t1", false).is_none(),
        "the first page is on its way"
    );
    until("the first page", || machines.transcript("m1", "t1", false).is_some()).await;
    fake.push_event(
        "t1",
        json!({ "kind": "text", "id": "a1", "text": "On it.", "parent": null }),
    );
    until("the pushed answer", || {
        machines
            .transcript("m1", "t1", false)
            .is_some_and(|read| read.is_ok_and(|read| read.last == 2))
    })
    .await;

    // The link drops, an event lands meanwhile, and the relay catches it up once, in order.
    fake.up.store(false, Ordering::SeqCst);
    if let Ok(remote) = machines.remote("m1") {
        remote.drop_connection();
    }
    until("offline", || matches!(status(&machines), Status::Unreachable { .. })).await;
    let offline = crate::machine_view::status_view(&status(&machines));
    assert_eq!(offline.label, "Offline");
    assert!(
        offline
            .detail
            .is_some_and(|detail| detail.contains("Click Retry to reconnect"))
    );
    if let Ok(mut host) = fake.host.lock() {
        host.client = None;
    }
    fake.push_event(
        "t1",
        json!({ "kind": "text", "id": "a2", "text": "Done.", "parent": null }),
    );
    fake.up.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(matches!(
        status(&machines),
        Status::Unreachable { retry_in_secs: 0, .. }
    ));
    let _ = machines.reconnect("m1");
    until("caught up", || {
        machines
            .transcript("m1", "t1", false)
            .is_some_and(|read| read.is_ok_and(|read| read.last == 3))
    })
    .await;
    let Some(Ok(Transcript { blocks, .. })) = machines.transcript("m1", "t1", false) else {
        unreachable!()
    };
    assert_eq!(
        blocks,
        // One answer: its two texts read as one block, as this computer's sessions do.
        vec![
            Block::User {
                seq: 1,
                at: 1,
                text: "Fix the build".into(),
                from: None,
                rewound: false,
                images: 0,
                image_data: Vec::new(),
            },
            Block::Text("On it.\n\nDone.".into()),
        ]
    );
    assert!(fake.opened.load(Ordering::SeqCst) >= 2);
    machines.shutdown();
    until("closed", || status(&machines) == Status::Closed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_session_there_opens_at_its_end_and_reads_older_pages_on_request() {
    let fake = Fake::new(PROTOCOL);
    // Seed history before connecting. Live pushes can arrive after the first page and legitimately enlarge it.
    let total = code::EVENTS + 100;
    for n in 1..=total {
        fake.push_event("t1", json!({ "kind": "user", "text": format!("Message {n}") }));
    }
    let machines = machines(&fake);
    machines.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;

    assert!(
        machines.transcript("m1", "t1", false).is_none(),
        "the first page is on its way"
    );
    until("the first page", || machines.transcript("m1", "t1", false).is_some()).await;
    let Some(Ok(latest)) = machines.transcript("m1", "t1", false) else {
        unreachable!()
    };
    assert_eq!(latest.blocks.len(), code::EVENTS);
    assert_eq!(latest.start, Some(101));
    assert!(latest.more, "the first 100 messages are before the page");

    // Near the top, the page before: the session from its first message.
    let _ = machines.transcript("m1", "t1", true);
    until("the older page", || {
        machines
            .transcript("m1", "t1", false)
            .is_some_and(|read| read.is_ok_and(|read| read.start == Some(1)))
    })
    .await;
    let Some(Ok(all)) = machines.transcript("m1", "t1", false) else {
        unreachable!()
    };
    assert_eq!(all.blocks.len(), total);
    assert!(!all.more);
    assert!(matches!(&all.blocks[0], Block::User { text, .. } if text == "Message 1"));
    machines.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_server_or_destination_is_refused() {
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    let missing = machines.call("nope", "wb_overview", json!({})).await;
    assert_eq!(missing.ok(), Some(Err("No such machine.".to_string())));
    let test = machines.test("-oProxyCommand=evil").await;
    assert!(test.is_ok_and(|check| check.is_err_and(|error| error.contains("is not an SSH host"))));
    // A destination ssh would read as an option is never connected.
    machines.sync(&settings(&[("m1", "-oProxyCommand=evil")]));
    assert!(machines.servers().is_empty());
}

/// The connect's read of the projects and sessions lands after a newer answer: what that answer added stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_project_added_before_the_first_read_lands_stays() {
    let fake = Fake::new(PROTOCOL);
    fake.late_overview.store(true, Ordering::SeqCst);
    let machines = machines(&fake);
    machines.sync(&settings(&[("m1", "me@box")]));
    // The read is asked for first; its reply waits until the add's answer is kept.
    until("the first read asked", || fake.overviews.load(Ordering::SeqCst) == 1).await;
    let added = machines
        .call("m1", "wb_project_add", json!({ "path": "/srv/web" }))
        .await;
    assert!(matches!(added, Ok(Ok(_))), "{added:?}");
    fake.release.notify_one();
    let on = || machines.projects_on("m1");
    until("the first read", || on().iter().any(|project| project.id == "p1")).await;
    let ids: Vec<String> = on().into_iter().map(|project| project.id).collect();
    assert_eq!(ids, ["p1", "p2"]);
    assert_eq!(machines.machine_of("p2").as_deref(), Some("m1"));
    machines.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_folder_added_there_is_one_of_its_projects_at_once() {
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    machines.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;
    let added = machines
        .call("m1", "wb_project_add", json!({ "path": "/srv/web" }))
        .await;
    assert_eq!(
        added.ok().and_then(Result::ok).map(|p| p["id"].clone()),
        Some(json!("p2"))
    );
    assert_eq!(machines.machine_of("p2").as_deref(), Some("m1"));
    let web = machines.projects().into_iter().find(|project| project.id == "p2");
    assert_eq!(
        web.map(|project| (project.name, project.machine)),
        Some(("web".into(), Some("m1".into())))
    );
    machines.shutdown();
}

/// A repository cloned there (`server_folder.rs`): the host says it clones, git's progress reads as a stage and a
/// percentage, the clone joins the server's projects at once, and a failed clone answers git's reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repository_cloned_there_is_one_of_its_projects_at_once() {
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    assert!(!clones(&Status::Connecting { attempt: 0 }));
    machines.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;
    assert!(clones(&status(&machines)), "the host says project-clone");

    assert_eq!(
        machines.clone_progress("m1", "r1").await,
        Some(("Receiving objects".to_string(), Some(45)))
    );
    assert_eq!(machines.clone_progress("m1", "r2").await, None, "git said nothing yet");
    assert_eq!(machines.clone_progress("nope", "r1").await, None, "no such server");

    let cloned = machines.clone_project("m1", "owner/app", "/srv", "r1").await;
    assert_eq!(cloned.as_deref(), Ok("p3"));
    assert_eq!(machines.machine_of("p3").as_deref(), Some("m1"));
    let app = machines.projects().into_iter().find(|project| project.id == "p3");
    assert_eq!(
        app.map(|project| (project.name, project.machine)),
        Some(("app".into(), Some("m1".into())))
    );
    let on = machines
        .projects_on("m1")
        .into_iter()
        .find(|project| project.id == "p3");
    assert_eq!(on.map(|project| project.path), Some("/srv/app".to_string()));

    let failed = machines.clone_project("m1", "owner/missing", "/srv", "r3").await;
    assert_eq!(
        failed,
        Err("Couldn't find the repository owner/missing on GitHub.".to_string())
    );
    machines.shutdown();
}

#[test]
fn a_hosts_clone_progress_reads_as_a_stage_and_a_percentage() {
    assert_eq!(
        clone_stage(&json!({ "stage": "Resolving deltas", "percent": null })),
        Some(("Resolving deltas".to_string(), None))
    );
    assert_eq!(
        clone_stage(&json!({ "stage": "Receiving objects", "percent": 300 })),
        Some(("Receiving objects".to_string(), None)),
        "a percentage out of range is dropped"
    );
    assert_eq!(clone_stage(&Value::Null), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_servers_answer_streams_until_its_text_lands() {
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    let mut changes = machines.subscribe();
    machines.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;
    fake.push_delta("t-stream", "a1", "Look", None);
    fake.push_delta("t-stream", "a1", "ing at it", Some(4));
    until("the streamed text", || {
        crate::stream::text("t-stream").as_deref() == Some("Looking at it")
    })
    .await;
    // A streamed piece only redraws: nothing is read again.
    let mut streamed = false;
    while let Ok(change) = changes.try_recv() {
        streamed |= change.streamed;
    }
    assert!(streamed);
    fake.push_event(
        "t-stream",
        json!({ "kind": "text", "id": "a1", "text": "Looking at it.", "parent": null }),
    );
    until("the final text", || crate::stream::text("t-stream").is_none()).await;
    machines.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connected_server_sweeps_its_own_worktrees() {
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    machines.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;
    let connected = machines.connected_now();
    assert_eq!(
        connected.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        vec!["m1"]
    );
    let rules = nexus_workbench::cleanup::Rules::default();
    let mut last = nexus_remote::cleanup::LastCleanup::default();
    for (name, remote) in &connected {
        last.add_answer(Some(name), nexus_remote::cleanup::sweep(name, remote, &rules).await);
    }
    assert_eq!(last.removed.len(), 1);
    assert_eq!(last.removed[0].machine.as_deref(), Some("m1"));
    assert!(last.problems.is_empty());
    // Offline, it is not asked.
    fake.up.store(false, Ordering::SeqCst);
    if let Ok(remote) = machines.remote("m1") {
        remote.drop_connection();
    }
    until("offline", || !matches!(status(&machines), Status::Connected { .. })).await;
    assert!(machines.connected_now().is_empty());
    machines.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_servers_session_reads_its_changes_and_files_there() {
    use nexus_workbench::changes_source::ChangesSource as Source;
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    machines.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;
    let place = crate::side::server::ServerPlace {
        machines: machines.clone(),
        machine: "m1".into(),
        thread: Some("t1".into()),
        project: None,
    };
    let (patch, truncated) = place.diff(&Source::Session).await.expect("the session's diff");
    assert!(patch.contains("src/a.rs") && !truncated);
    for source in [
        Source::Turn(9),
        Source::Uncommitted,
        Source::Branch,
        Source::Commit("a1b2c3d4e5".into()),
    ] {
        assert!(place.diff(&source).await.is_ok(), "{source:?}");
    }
    // The picker's turns, base branch and commits come from the server too.
    assert_eq!(place.turns().await, vec![4, 9]);
    let targets = place.targets().await.expect("the picker's targets");
    assert_eq!(targets.base.as_deref(), Some("origin/main"));
    assert_eq!(targets.commits.len(), 1);
    assert_eq!(place.uncommitted().await.map(|stat| stat.files), Some(1));
    assert_eq!(
        place.files().await,
        Ok((vec!["README.md".to_string(), "src/a.rs".to_string()], false))
    );
    assert_eq!(place.status().await.get("src/a.rs"), Some(&'M'));
    // This host is older than image and PDF previews: the file comes as plain text, and the viewer says so.
    let (preview, previews) = place.preview("src/a.rs").await.expect("the file's preview");
    assert_eq!(
        (preview.text.as_deref(), preview.media_type.as_str()),
        (Some("b\n"), "text/plain")
    );
    assert!(!previews);
    let committed = machines
        .call(
            "m1",
            "wb_commit",
            json!({ "threadId": "t1", "repo": null, "message": "m", "push": false }),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .map(serde_json::from_value::<nexus_workbench::GitState>);
    assert!(matches!(committed, Some(Ok(_))), "{committed:?}");
    // A project there lists its files, and its changes show in its sessions.
    let project = crate::side::server::ServerPlace {
        thread: None,
        project: Some("p1".into()),
        ..place
    };
    assert!(project.files().await.is_ok());
    assert!(project.diff(&Source::Session).await.is_err());
    assert!(project.turns().await.is_empty());
    machines.shutdown();
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_servers_file_previews_there_in_pieces() {
    use nexus_workbench::output_files::{FEATURE, PIECES_FEATURE, Status as Shown};
    let fake = Fake::with_features(PROTOCOL, &[FEATURE, PIECES_FEATURE]);
    let machines = machines(&fake);
    machines.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;
    let place = crate::side::server::ServerPlace {
        machines: machines.clone(),
        machine: "m1".into(),
        thread: Some("t1".into()),
        project: None,
    };
    let (image, previews) = place.preview("logo.png").await.expect("the image's preview");
    assert!(previews);
    assert_eq!((image.status, image.offset), (Shown::Ready, None));
    assert_eq!(image.bytes(), Some(vec![1, 2, 3, 4, 5]), "both pieces, joined");
    let (notes, _) = place.preview("NOTES.md").await.expect("the Markdown preview");
    assert_eq!(
        (notes.media_type.as_str(), notes.text.as_deref()),
        ("text/markdown", Some("# Notes"))
    );
    // Without pieces, the image comes whole.
    let fake = Fake::with_features(PROTOCOL, &[FEATURE]);
    let whole = self::machines(&fake);
    whole.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&whole), Status::Connected { .. })).await;
    let place = crate::side::server::ServerPlace {
        machines: whole.clone(),
        ..place
    };
    let (image, _) = place.preview("logo.png").await.expect("the whole image");
    assert_eq!(image.bytes(), Some(vec![1, 2, 3, 4, 5]));
    machines.shutdown();
    whole.shutdown();
}

/// Events of a session Gateway holds up to `gateway`, shown as its Gateway-read first page, on a machine `fake` that
/// holds `total` events: the machine's own latest page is then read (as on connecting).
async fn lagging_gateway(total: i64, gateway: i64) -> (Arc<Machines>, Arc<Fake>) {
    let fake = Fake::new(PROTOCOL);
    for n in 1..=total {
        fake.push_event("t1", json!({ "kind": "user", "text": format!("Message {n}") }));
    }
    let machines = machines(&fake);
    machines.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;
    machines.with_server("m1", |server| {
        let followed = server.followed.entry("t1".into()).or_default();
        let held = (1..=gateway)
            .map(|n| {
                (
                    n,
                    n,
                    WbEvent::Notice {
                        text: format!("gateway {n}"),
                    },
                )
            })
            .collect();
        followed.take_page(held, false, false);
        followed.history = Some(history::HistoryRead::Kept);
        followed.gateway_last = gateway;
    });
    machines.relay_page("m1", "t1", None);
    (machines, fake)
}

fn held(machines: &Machines) -> (Vec<i64>, bool, bool) {
    machines
        .with_server("m1", |server| {
            let followed = &server.followed["t1"];
            (followed.events.keys().copied().collect(), followed.more, followed.gap)
        })
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gateway_that_lags_the_machine_by_more_than_a_page_is_bridged_page_by_page() {
    let total = (code::EVENTS * 2 + 100) as i64;
    let (machines, _fake) = lagging_gateway(total, 5).await;
    until("the bridge", || held(&machines).0.len() as i64 == total).await;
    let (seqs, more, gap) = held(&machines);
    assert_eq!(
        seqs,
        (1..=total).collect::<Vec<_>>(),
        "no event between Gateway's last and the machine's page is lost"
    );
    assert!(!more && !gap);
    machines.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hole_too_long_to_bridge_is_marked_so_scrolling_up_loads_it_from_the_machine() {
    let total = (code::EVENTS * (history::BRIDGE_PAGES + 2)) as i64;
    let (machines, _fake) = lagging_gateway(total, 5).await;
    until("the cap", || held(&machines).2).await;
    let (seqs, more, gap) = held(&machines);
    assert!(more && gap);
    assert!(
        seqs.iter().all(|seq| *seq > 5),
        "Gateway's copy below the hole is dropped, not shown next to it"
    );
    assert_eq!(seqs.last(), Some(&total));
    // Scrolling up reads the machine's page before the first event held, not Gateway's.
    assert!(!machines.read_history("m1", "t1", seqs.first().copied()));
    machines.shutdown();
}

/// Settings' MCP servers reach a server when it connects, and every connected one when they change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_mcp_servers_reach_each_server_that_connects() {
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    let handed = json!({"servers": [{"name": "db", "command": "db-mcp"}], "values": {}});
    machines.hand_mcp_servers(handed.clone());
    machines.sync(&settings(&[("m1", "me@box")]));
    until("the hand-off", || {
        fake.host.lock().is_ok_and(|host| {
            host.handed
                .iter()
                .any(|(call, args)| call == "host_mcp_servers" && *args == handed)
        })
    })
    .await;
    let changed = json!({"servers": [], "values": {}});
    machines.hand_mcp_servers(changed.clone());
    until("the change", || {
        fake.host
            .lock()
            .is_ok_and(|host| host.handed.iter().any(|(_, args)| *args == changed))
    })
    .await;
}

/// A run of the server that ended is kept, so its browser calls stop, and its push is not a session's event.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_servers_run_end_is_the_app_tools_not_a_sessions_event() {
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    machines.sync(&settings(&[("m1", "me@box")]));
    until("connected", || matches!(status(&machines), Status::Connected { .. })).await;
    fake.push(nexus_workbench::app_socket::RUN_ENDED_EVENT, json!({"run": "run_9"}));
    until("the run's end", || machines.server_run_ended("run_9")).await;
}
