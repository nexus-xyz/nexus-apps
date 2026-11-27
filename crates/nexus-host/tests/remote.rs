//! The desktop app's side (nexus-remote) against this host: the checks that need no real server.
//!
//! - The relay: a session runs through [`Remote`], the connection drops mid-turn, the relay reconnects by itself and
//!   catches up, and the webview's view ends up the whole event log, each event once, in order.
//! - The bootstrap: the same scripts the app runs over `ssh`, run by `sh` on this computer in a throwaway home
//!   ([`Target::Local`]): install the built `nexus-host` (the `NEXUS_HOST_BINARY` path), start it detached, read the
//!   token, reach it through `nexus-host proxy`, reconnect, and "Update now" (stop, database copied aside, start).

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::{TOKEN, root, start};
use futures::future::BoxFuture;
use nexus_host::files::{Lock, Paths};
use nexus_remote::client::Pushed;
use nexus_remote::install::{Installer, Source};
use nexus_remote::machine::{Backoff, Link, Opened, Remote, Sink, SocketLink, SshLink, Status};
use nexus_remote::ssh::Target;
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// What the app's sink gets: pushes and statuses, in order.
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

/// A socket link the test can cut off, as a network that is down: no connection opens while `open` is false.
struct Gated {
    link: SocketLink,
    open: Arc<AtomicBool>,
}

impl Link for Gated {
    fn open(&self) -> BoxFuture<'_, Result<Opened, String>> {
        Box::pin(async move {
            if !self.open.load(Ordering::SeqCst) {
                return Err("Network is unreachable".into());
            }
            self.link.open().await
        })
    }

    fn update(&self) -> BoxFuture<'_, Result<(), String>> {
        self.link.update()
    }
}

fn fast() -> Backoff {
    Backoff {
        first: Duration::from_millis(50),
        max: Duration::from_millis(200),
        healthy: Duration::from_secs(60),
    }
}

/// The webview's copy of one session: the snapshot, then what the relay delivers. Unlike the host's own test, the
/// relay itself must never deliver a `seq` it delivered before, nor one at or below what it already passed on.
#[derive(Default)]
struct View {
    thread: String,
    events: BTreeMap<i64, Value>,
    duplicates: Vec<i64>,
    /// Every `seq` the relay delivered for the session, in order.
    delivered: Vec<i64>,
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
                if push.event == "wb:event" && push.payload["threadId"] == json!(self.thread) {
                    let seq = push.payload["seq"].as_i64().unwrap_or_default();
                    self.delivered.push(seq);
                    // The webview's own rule for what came before its snapshot: keep past the last it holds.
                    if seq > self.last() {
                        self.add(&push.payload);
                    }
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

    fn connections(&self) -> usize {
        self.statuses
            .iter()
            .filter(|s| matches!(s, Status::Connected { .. }))
            .count()
    }
}

async fn until(heard: &mut mpsc::UnboundedReceiver<Heard>, view: &mut View, done: impl Fn(&View) -> bool) {
    // Connect runs `ensure` over ssh, which takes tens of seconds on a cold CI runner.
    let waited = tokio::time::timeout(Duration::from_secs(60), async {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_reconnects_by_itself_and_catches_up_exactly_once() {
    relay_catches_up(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_manual_relay_waits_for_retry_then_catches_up_exactly_once() {
    relay_catches_up(true).await;
}

async fn relay_catches_up(manual: bool) {
    let host = start(if manual { "manual-relay" } else { "relay" });
    let (tx, mut heard) = mpsc::unbounded_channel();
    let gate = Arc::new(AtomicBool::new(true));
    let start_remote = if manual { Remote::start_manual } else { Remote::start };
    let remote = start_remote(
        Arc::new(Gated {
            link: SocketLink {
                socket: host.socket.clone(),
                token: TOKEN.into(),
            },
            open: gate.clone(),
        }),
        Arc::new(Channel(tx)),
        fast(),
    );
    let mut view = View::default();
    until(&mut heard, &mut view, |v| v.connections() == 1).await;
    assert!(
        matches!(&view.statuses[view.statuses.len() - 1], Status::Connected { host, update: None } if host.version == "9.9.9")
    );

    let call = |name: &'static str, args: Value| {
        let remote = remote.clone();
        async move { remote.call(name, args).await.unwrap_or_else(|e| panic!("{name}: {e}")) }
    };
    let project = call("wb_project_add", json!({ "path": host.repo })).await;
    let created = call(
        "wb_thread_create",
        json!({
            "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "use a tool", "images": [], "worktree": null,
        }),
    )
    .await;
    view.thread = created["id"].as_str().unwrap_or_default().to_string();
    let thread = view.thread.clone();
    // The webview opens the session: its log so far through the relay, then live past it.
    let snapshot = call("wb_thread_events", json!({ "threadId": thread, "afterSeq": 0 })).await;
    for event in snapshot.as_array().into_iter().flatten() {
        view.add(event);
    }

    until(&mut heard, &mut view, |v| v.kinds().contains(&"approval")).await;
    let approval = view
        .events
        .values()
        .find(|e| e["event"]["kind"] == "approval")
        .cloned()
        .unwrap_or_default();
    call(
        "wb_approve",
        json!({ "threadId": thread, "requestId": approval["event"]["requestId"], "decision": "allow",
                "message": null, "suggestion": null, "answers": null }),
    )
    .await;
    until(&mut heard, &mut view, |v| v.turn_ends() == 1).await;

    let observed = call("wb_connection", json!({ "threadId": thread })).await;
    assert_eq!(observed["model"], "claude-fake-1");
    assert!(
        observed["generation"]
            .as_str()
            .is_some_and(|generation| !generation.is_empty())
    );
    assert_eq!(observed["mcp"][0]["status"], "connected");
    assert!(observed["account"].is_null());
    assert_eq!(view.turn_ends(), 1, "reading the remote process starts no turn");

    // A turn that keeps working; the network goes away mid-turn and the host goes on alone.
    call(
        "wb_send",
        json!({ "threadId": thread, "text": "slow background work", "images": [] }),
    )
    .await;
    let before = view.last();
    until(&mut heard, &mut view, |v| {
        v.events.range(before + 1..).any(|(_, e)| e["event"]["kind"] == "text")
    })
    .await;
    // The network goes: the connection drops and no new one can be made for a while.
    gate.store(false, Ordering::SeqCst);
    remote.drop_connection();
    let left_at = view.last();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let stored = host
        .api
        .thread_events(thread.clone(), left_at)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!stored.is_empty(), "nothing happened while the relay was away");
    // While away, calls fail at once rather than hang.
    assert!(remote.call("wb_overview", json!({})).await.is_err());

    // The network is back; the relay finds the host on its next try and catches up by itself.
    gate.store(true, Ordering::SeqCst);
    if manual {
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(matches!(remote.status(), Status::Unreachable { retry_in_secs: 0, .. }));
        assert!(remote.call("wb_overview", json!({})).await.is_err());
        remote.reconnect();
    }
    until(&mut heard, &mut view, |v| v.connections() == 2).await;
    let caught_up = stored.last().map(|e| e.seq).unwrap_or_default();
    until(&mut heard, &mut view, |v| v.last() >= caught_up).await;

    // Ordinary Send reaches the active turn after reconnect, without Send now.
    let before_send = view.last();
    call(
        "wb_send",
        json!({ "threadId": thread, "text": "wrap up", "images": [] }),
    )
    .await;
    let queued = |v: &View| {
        v.events
            .range(before_send + 1..)
            .find(|(_, e)| e["event"]["kind"] == "user")
            .map(|(seq, _)| *seq)
    };
    until(&mut heard, &mut view, |v| queued(v).is_some()).await;
    let seq = queued(&view).unwrap_or_default();
    until(&mut heard, &mut view, |v| v.turn_ends() == 2).await;
    for kind in ["queueSteer", "read"] {
        assert_eq!(
            view.events
                .values()
                .filter(|e| e["event"]["kind"] == kind && e["event"]["seq"] == seq)
                .count(),
            1
        );
    }
    assert!(view.events.values().any(|e| {
        e["event"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Heard mid-turn: wrap up"))
    }));

    // The relay never delivered a session's event twice or out of order...
    assert!(
        view.delivered.windows(2).all(|w| w[1] > w[0]),
        "relay order: {:?}",
        view.delivered
    );
    assert!(view.duplicates.is_empty(), "duplicates: {:?}", view.duplicates);
    // ...and the webview holds the whole log, gapless.
    let whole = call("wb_thread_events", json!({ "threadId": thread, "afterSeq": 0 })).await;
    let whole: BTreeMap<i64, Value> = whole
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e["seq"].as_i64() <= Some(view.last()))
        .map(|e| (e["seq"].as_i64().unwrap_or_default(), e))
        .collect();
    assert_eq!(view.events, whole);
    let seqs: Vec<i64> = view.events.keys().copied().collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");

    call("wb_stop", json!({ "threadId": thread })).await;
    remote.close();
    let _ = host.stop.send(true);
    host.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&host.root);
}

/// A video an agent saved on the server reaches the app through the relay in pieces, each one well under a WebSocket
/// frame, and comes out the same bytes. A file outside the session's folder does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_saved_video_reaches_the_app_in_pieces() {
    use base64::Engine as _;
    use nexus_workbench::output_media::{CHUNK, FEATURE};

    let host = start("media");
    let (tx, mut heard) = mpsc::unbounded_channel();
    let remote = Remote::start(
        Arc::new(SocketLink {
            socket: host.socket.clone(),
            token: TOKEN.into(),
        }),
        Arc::new(Channel(tx)),
        fast(),
    );
    let mut view = View::default();
    until(&mut heard, &mut view, |v| v.connections() == 1).await;
    assert!(
        matches!(&view.statuses[view.statuses.len() - 1], Status::Connected { host, .. } if host.features.iter().any(|f| f == FEATURE))
    );
    let call = |name: &'static str, args: Value| {
        let remote = remote.clone();
        async move { remote.call(name, args).await }
    };
    let project = call("wb_project_add", json!({ "path": host.repo }))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let created = call(
        "wb_thread_create",
        json!({
            "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "hi", "images": [], "worktree": null,
        }),
    )
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    let thread = created["id"].as_str().unwrap_or_default().to_string();

    let artifacts = host.repo.join(".nexus").join("artifacts");
    std::fs::create_dir_all(&artifacts).unwrap_or_else(|e| panic!("{e}"));
    let video: Vec<u8> = (0..CHUNK * 2 + 123).map(|n| (n % 253) as u8).collect();
    std::fs::write(artifacts.join("login.webm"), &video).unwrap_or_else(|e| panic!("{e}"));
    let mut bytes = Vec::new();
    let mut pieces = 0;
    while bytes.len() < video.len() {
        let chunk = call(
            "wb_file_media",
            json!({ "threadId": thread, "path": ".nexus/artifacts/login.webm", "offset": bytes.len() }),
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(chunk["status"], "ready");
        assert_eq!(chunk["mediaType"], "video/webm");
        assert_eq!(chunk["size"], video.len());
        let data = chunk["data"].as_str().unwrap_or_default();
        assert!(data.len() < 16 * 1024 * 1024, "a piece fits in one WebSocket frame");
        let piece = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(!piece.is_empty());
        bytes.extend(piece);
        pieces += 1;
    }
    assert_eq!(pieces, 3);
    assert!(bytes == video, "the pieces make the same video");

    std::fs::write(host.root.join("secret.png"), b"png").unwrap_or_else(|e| panic!("{e}"));
    let outside = call(
        "wb_file_media",
        json!({ "threadId": thread, "path": "../secret.png", "offset": 0 }),
    )
    .await
    .expect_err("a file outside the session's folder stays on the server");
    assert!(outside.contains("inside the project"), "{outside}");

    let _ = call("wb_stop", json!({ "threadId": thread })).await;
    remote.close();
    let _ = host.stop.send(true);
    host.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&host.root);
}

/// A 15 MB image previews on a server: whole, it is about 20 MB of base64, over a WebSocket frame; in pieces each
/// answer fits and the bytes come out the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_preview_reaches_the_app_in_pieces() {
    use base64::Engine as _;
    use nexus_workbench::output_files::PIECES_FEATURE;

    let host = start("preview");
    let (tx, mut heard) = mpsc::unbounded_channel();
    let remote = Remote::start(
        Arc::new(SocketLink {
            socket: host.socket.clone(),
            token: TOKEN.into(),
        }),
        Arc::new(Channel(tx)),
        fast(),
    );
    let mut view = View::default();
    until(&mut heard, &mut view, |v| v.connections() == 1).await;
    assert!(
        matches!(&view.statuses[view.statuses.len() - 1], Status::Connected { host, .. } if host.features.iter().any(|f| f == PIECES_FEATURE))
    );
    let call = |name: &'static str, args: Value| {
        let remote = remote.clone();
        async move { remote.call(name, args).await.unwrap_or_else(|e| panic!("{name}: {e}")) }
    };
    let project = call("wb_project_add", json!({ "path": host.repo })).await;
    let created = call(
        "wb_thread_create",
        json!({
            "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "hi", "images": [], "worktree": null,
        }),
    )
    .await;
    let thread = created["id"].as_str().unwrap_or_default().to_string();

    let image: Vec<u8> = (0..15 * 1024 * 1024).map(|n: u32| (n % 239) as u8).collect();
    std::fs::write(host.repo.join("large.png"), &image).unwrap_or_else(|e| panic!("{e}"));
    let mut bytes = Vec::new();
    loop {
        let piece = call(
            "wb_file_preview",
            json!({ "threadId": thread, "path": "large.png", "offset": bytes.len() }),
        )
        .await;
        assert_eq!(piece["status"], "ready");
        assert_eq!(piece["mediaType"], "image/png");
        assert_eq!(piece["size"], image.len());
        assert_eq!(piece["offset"], bytes.len());
        let data = piece["data"].as_str().unwrap_or_default();
        assert!(data.len() < 16 * 1024 * 1024, "a piece fits in one WebSocket frame");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(!decoded.is_empty());
        bytes.extend(decoded);
        if bytes.len() >= image.len() {
            break;
        }
    }
    assert!(bytes == image, "the pieces make the same image");

    let _ = remote.call("wb_stop", json!({ "threadId": thread })).await;
    remote.close();
    let _ = host.stop.send(true);
    host.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&host.root);
}

/// Stops whatever host runs in `home` when the test ends, passed or not.
struct StopHost {
    binary: PathBuf,
    home: PathBuf,
}

impl Drop for StopHost {
    fn drop(&mut self) {
        let _ = nexus_workbench::process::command(&self.binary)
            .arg("stop")
            .env("HOME", &self.home)
            .env_remove("NEXUS_HOST_DIR")
            .output();
    }
}

/// A file larger than one piece goes through the real connection in pieces, and the host's session folder
/// ends with the whole file. A cancelled upload leaves nothing behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_attached_in_pieces_reaches_the_host_over_the_connection() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use nexus_workbench::attachments::CHUNK_BYTES;
    use sha2::{Digest, Sha256};

    let host = start("pieces");
    let (tx, mut heard) = mpsc::unbounded_channel();
    let remote = Remote::start(
        Arc::new(SocketLink {
            socket: host.socket.clone(),
            token: TOKEN.into(),
        }),
        Arc::new(Channel(tx)),
        fast(),
    );
    let mut view = View::default();
    until(&mut heard, &mut view, |v| v.connections() == 1).await;
    let call = |name: &'static str, args: Value| {
        let remote = remote.clone();
        async move { remote.call(name, args).await.unwrap_or_else(|e| panic!("{name}: {e}")) }
    };
    // Two whole pieces and a short one.
    let whole: Vec<u8> = (0..(CHUNK_BYTES * 2 + 12_345)).map(|n| (n % 251) as u8).collect();
    let sha: String = Sha256::digest(&whole).iter().map(|b| format!("{b:02x}")).collect();
    // The pieces are for the session the creation request makes.
    let request = "5b0e1c7a-3f2d-4c8e-9a61-2d7f4b9e0c13".to_string();
    let thread = nexus_workbench::creation_thread_id(&request).unwrap_or_default();
    let mut offset = 0;
    let mut last = Value::Null;
    while offset < whole.len() {
        let end = (offset + CHUNK_BYTES).min(whole.len());
        last = call(
            "wb_attachment_chunk",
            json!({ "uploadId": "video1", "size": whole.len(), "offset": offset, "sha256": sha,
                    "data": STANDARD.encode(&whole[offset..end]), "threadId": thread }),
        )
        .await;
        offset = end;
    }
    assert_eq!(last, json!({ "received": whole.len(), "done": true }));
    // Another upload is begun, then cancelled by the person.
    call(
        "wb_attachment_chunk",
        json!({ "uploadId": "video2", "size": whole.len(), "offset": 0, "data": STANDARD.encode(&whole[..CHUNK_BYTES]),
                "threadId": thread }),
    )
    .await;
    call(
        "wb_attachment_cancel",
        json!({ "uploadId": "video2", "threadId": thread }),
    )
    .await;

    let project = call("wb_project_add", json!({ "path": host.repo })).await;
    let created = call(
        "wb_thread_create_files",
        json!({
            "requestId": request, "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "Watch this", "images": [], "worktree": null,
            "files": [{ "name": "clip.mp4", "upload": "video1" }],
        }),
    )
    .await;
    let id = created["id"].as_str().unwrap_or_default().to_string();
    let attachments = host.root.join("home/.nexus/attachments");
    let saved: Vec<PathBuf> = std::fs::read_dir(attachments.join(&id))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|batch| std::fs::read_dir(batch.path()).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .collect();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert!(saved[0].ends_with("1-clip.mp4"));
    assert!(std::fs::read(&saved[0]).is_ok_and(|bytes| bytes == whole));
    // Nothing waits in the upload folder: the first moved into the session, the second was cancelled.
    let waiting: Vec<_> = std::fs::read_dir(attachments.join(".uploads"))
        .into_iter()
        .flatten()
        .flatten()
        .collect();
    assert!(waiting.is_empty(), "{waiting:?}");

    let _ = host.stop.send(true);
    host.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&host.root);
}

fn holder(home: &Path) -> Option<i32> {
    Lock::holder(&Paths::from_env(home).lock())
}

/// Copy an executable through a child `cp`, never in this process. Tests here run in parallel threads, and another
/// test's fork inherits any descriptor this process holds open for writing until its exec. Running the copy while a
/// child still holds one fails with ETXTBSY (`Text file busy`), as `process::write_fake_script` says for scripts.
fn copy_executable(from: &Path, to: &Path) {
    let copied = nexus_workbench::process::command("cp")
        .arg(from)
        .arg(to)
        .status()
        .unwrap_or_else(|e| panic!("could not run cp {} {}: {e}", from.display(), to.display()));
    assert!(copied.success(), "cp {} {}: {copied}", from.display(), to.display());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bootstrap_installs_starts_reaches_and_updates_the_host() {
    let root = root("boot");
    let home = root.join("home");
    let _ = std::fs::create_dir_all(&home);
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_nexus-host"));
    let _stop = StopHost {
        binary: binary.clone(),
        home: home.clone(),
    };
    let installer = Installer {
        target: Target::Local {
            // `SHELL` empty: the host asks no login shell for `PATH`, so the developer's shell never starts.
            env: vec![
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("SHELL".into(), String::new()),
            ],
        },
        source: Source::Local { path: binary.clone() },
        fetch: None,
    };

    // The first time: copied over (as from the laptop), started detached, its token read.
    let ready = installer.ensure().await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(ready.version, nexus_host::VERSION);
    assert!(ready.dir.starts_with("dev-"), "{}", ready.dir);
    assert!(["systemd", "nohup"].contains(&ready.started.as_str()), "{ready:?}");
    let dir = home.join(".nexus/host");
    assert!(dir.join(&ready.dir).join("nexus-host").exists());
    let token = std::fs::read_to_string(dir.join("token")).unwrap_or_default();
    assert_eq!(ready.token, token.trim());
    let first = holder(&home).unwrap_or_else(|| panic!("no host runs"));
    // Again: nothing to install or start.
    let again = installer.ensure().await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(again.started, "already");
    assert_eq!(holder(&home), Some(first));

    // Through `nexus-host proxy`, as over ssh.
    let (tx, mut heard) = mpsc::unbounded_channel();
    let remote = Remote::start(
        Arc::new(SshLink {
            installer: installer.clone(),
        }),
        Arc::new(Channel(tx)),
        fast(),
    );
    let mut view = View::default();
    until(&mut heard, &mut view, |v| v.connections() == 1).await;
    let status = remote.status();
    assert!(
        matches!(&status, Status::Connected { host, update: None } if host.version == nexus_host::VERSION),
        "{status:?}"
    );
    let overview = remote
        .call("wb_overview", json!({}))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(overview["persistent"], true);
    let project = remote
        .call("wb_project_add", json!({ "path": root }))
        .await
        .unwrap_or_else(|e| panic!("{e}"));

    // The ssh child dies (the laptop slept): back by itself, same host.
    remote.drop_connection();
    until(&mut heard, &mut view, |v| v.connections() == 2).await;
    assert_eq!(holder(&home), Some(first));

    // "Update now": the host restarts (its database copied aside first) and keeps what it had.
    remote.update().await.unwrap_or_else(|e| panic!("{e}"));
    until(&mut heard, &mut view, |v| v.connections() == 3).await;
    let second = holder(&home).unwrap_or_else(|| panic!("no host runs after the update"));
    assert_ne!(second, first);
    assert!(
        dir.join(format!("workbench.sqlite3.before-{}", ready.dir)).exists(),
        "no copy of the database"
    );
    let projects = remote
        .call("wb_overview", json!({}))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        projects["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|p| p["id"] == project["id"]),
        "{projects}"
    );

    remote.close();
    drop(_stop);
    assert_eq!(holder(&home), None);
    let _ = std::fs::remove_dir_all(&root);
}

/// An app whose own version is not published (Nexus GPUI from main): the host already running on the machine serves,
/// not the newest installed one, and it is never silently replaced, so the connection is made once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unpublished_version_connects_to_the_running_host_and_keeps_it() {
    use nexus_remote::install::Fetch;
    let root = root("unpublished");
    let home = root.join("home");
    let _ = std::fs::create_dir_all(&home);
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_nexus-host"));
    let _stop = StopHost {
        binary: binary.clone(),
        home: home.clone(),
    };
    let target = Target::Local {
        env: vec![
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("SHELL".into(), String::new()),
        ],
    };
    // A host runs there already, from a released folder as a cloud image's unit starts it at boot. (A local build
    // never stands in for a release, so this one is not the `dev-` copy the local installer would make.)
    let running_dir = home.join(".nexus/host/0.0.1+running");
    let _ = std::fs::create_dir_all(&running_dir);
    copy_executable(&binary, &running_dir.join("nexus-host"));
    let mut serve = nexus_workbench::process::command(running_dir.join("nexus-host"));
    serve
        .arg("serve")
        .env("HOME", &home)
        .env_remove("NEXUS_HOST_DIR")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = serve.spawn().unwrap_or_else(|e| panic!("{e}"));
    for _ in 0..100 {
        if holder(&home).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let first = holder(&home).unwrap_or_else(|| panic!("no host runs"));
    let _ = child.try_wait();
    // Another host is installed after it, so it is the newest folder.
    let newer = home.join(".nexus/host/0.0.1+newer");
    let _ = std::fs::create_dir_all(&newer);
    copy_executable(&binary, &newer.join("nexus-host"));

    let fetch: Fetch = Arc::new(|url: String, _max| {
        Box::pin(async move {
            Err(format!(
                "Could not download {url}: HTTP status client error (404 Not Found)"
            ))
        })
    });
    let installer = Installer {
        target,
        source: Source::release("https://d.test", "9.9.9+abc", None),
        fetch: Some(fetch),
    };
    let ready = installer.ensure().await.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(ready.dir, "0.0.1+running");
    assert_eq!(ready.started, "already");
    assert!(!ready.exact);

    let (tx, mut heard) = mpsc::unbounded_channel();
    let remote = Remote::start(Arc::new(SshLink { installer }), Arc::new(Channel(tx)), fast());
    let mut view = View::default();
    until(&mut heard, &mut view, |v| v.connections() == 1).await;
    let status = remote.status();
    assert!(
        matches!(&status, Status::Connected { host, update: None } if host.version == nexus_host::VERSION),
        "{status:?}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    while let Ok(next) = heard.try_recv() {
        view.heard(next);
    }
    assert_eq!(view.connections(), 1, "{:?}", view.statuses);
    assert!(!view.statuses.iter().any(|s| matches!(s, Status::Updating)));
    assert_eq!(holder(&home), Some(first));

    remote.close();
    drop(_stop);
    let _ = std::fs::remove_dir_all(&root);
}

/// A machine nobody can reach says so, and keeps trying.
#[tokio::test]
async fn an_unreachable_machine_says_why() {
    let installer = Installer {
        target: Target::Local {
            env: vec![
                ("HOME".into(), "/nonexistent/nexus-remote-test".into()),
                ("SHELL".into(), String::new()),
            ],
        },
        source: Source::Local {
            path: PathBuf::from("/nonexistent/nexus-host"),
        },
        fetch: None,
    };
    let (tx, mut heard) = mpsc::unbounded_channel();
    let remote = Remote::start(Arc::new(SshLink { installer }), Arc::new(Channel(tx)), fast());
    let mut view = View::default();
    until(&mut heard, &mut view, |v| {
        v.statuses
            .iter()
            .any(|s| matches!(s, Status::Unreachable { error, .. } if error.contains("local Nexus host")))
    })
    .await;
    remote.close();
}

/// A host of another protocol: "needs an update", calls refused with that reason, and no retry loop until updated.
#[tokio::test]
async fn a_host_of_another_protocol_needs_an_update() {
    use futures::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;
    let root = root("proto");
    let socket = root.join("old.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap_or_else(|e| panic!("{e}"));
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    tokio::spawn({
        let accepted = accepted.clone();
        async move {
            while let Ok((stream, _)) = listener.accept().await {
                accepted.fetch_add(1, Ordering::SeqCst);
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    continue;
                };
                let _ = ws.next().await;
                let refusal = json!({
                    "error": "This Nexus host speaks protocol 0, not 1: update it.",
                    "hello": { "version": "0.1.0", "protocol": 0, "machine": "old", "org": null },
                });
                let _ = ws.send(Message::text(refusal.to_string())).await;
                let _ = ws.close(None).await;
            }
        }
    });
    let (tx, mut heard) = mpsc::unbounded_channel();
    let remote = Remote::start(
        Arc::new(SocketLink {
            socket,
            token: TOKEN.into(),
        }),
        Arc::new(Channel(tx)),
        fast(),
    );
    let mut view = View::default();
    until(&mut heard, &mut view, |v| {
        v.statuses
            .iter()
            .any(|s| matches!(s, Status::NeedsUpdate { host, .. } if host.version == "0.1.0"))
    })
    .await;
    let refused = remote.call("wb_overview", json!({})).await;
    assert_eq!(
        refused,
        Err("This machine runs an older Nexus host: update it first.".into())
    );
    // It waits for "Update now" instead of knocking again and again.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    remote.close();
    let _ = std::fs::remove_dir_all(&root);
}

/// A release: the server downloads the `.gz` beside its manifest, checks it and unpacks it; when it cannot reach that
/// URL, this computer downloads it, checks it, and copies it over to be unpacked there.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_is_downloaded_by_the_server_or_copied_from_here() {
    use nexus_remote::install::{Fetch, sha256_hex};
    let root = root("release");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_nexus-host"));
    // `uname` says Linux, so the release path runs the same way on a Mac and on CI.
    let stand_ins = root.join("bin");
    let _ = std::fs::create_dir_all(&stand_ins);
    let uname = stand_ins.join("uname");
    // Written from a child shell, as copy_executable copies: never open for writing in this process.
    nexus_workbench::process::write_fake_script(
        &uname,
        "#!/bin/sh\nif [ \"$1\" = -s ]; then echo Linux; else echo x86_64; fi\n",
    );
    let path = format!("{}:{}", stand_ins.display(), std::env::var("PATH").unwrap_or_default());
    let folder = root.join("downloads/desktop/stable/9.9.9");
    let _ = std::fs::create_dir_all(&folder);
    let gz = folder.join("nexus-host.gz");
    let packed = nexus_workbench::process::command("sh")
        .arg("-c")
        .arg("gzip -c \"$1\" > \"$2\"")
        .arg("sh")
        .arg(&binary)
        .arg(&gz)
        .status();
    assert!(packed.is_ok_and(|s| s.success()));
    let bytes = std::fs::read(&gz).unwrap_or_default();
    let sha = sha256_hex(&bytes);
    for arch in ["x86_64", "aarch64"] {
        let copied = std::fs::copy(&gz, folder.join(format!("nexus-host_9.9.9_linux-{arch}.gz")));
        assert!(copied.is_ok(), "{copied:?}");
    }

    for (case, downloads) in [
        ("server", format!("file://{}", root.join("downloads").display())),
        ("here", "http://127.0.0.1:9".to_string()),
    ] {
        let home = root.join(case);
        let _ = std::fs::create_dir_all(&home);
        let _stop = StopHost {
            binary: binary.clone(),
            home: home.clone(),
        };
        let manifest = json!({ "downloads": {
            "linux-x86_64-nexus-host": { "url": "unused", "sha256": sha },
            "linux-aarch64-nexus-host": { "url": "unused", "sha256": sha },
        }});
        let fetched = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let fetch: Fetch = {
            let (bytes, fetched) = (bytes.clone(), fetched.clone());
            Arc::new(move |asked: String, _max| {
                if let Ok(mut f) = fetched.lock() {
                    f.push(asked.clone());
                }
                let answer = if asked.ends_with("/desktop/stable/9.9.9/latest.json") {
                    Ok(manifest.to_string().into_bytes())
                } else {
                    Ok(bytes.clone())
                };
                Box::pin(async move { answer })
            })
        };
        let installer = Installer {
            target: Target::Local {
                env: vec![
                    ("HOME".into(), home.to_string_lossy().into_owned()),
                    ("PATH".into(), path.clone()),
                    ("SHELL".into(), String::new()),
                ],
            },
            source: Source::release(&downloads, "9.9.9+abc", None),
            fetch: Some(fetch),
        };
        let ready = installer.ensure().await.unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(ready.dir, "9.9.9+abc");
        assert_eq!(ready.version, nexus_host::VERSION);
        assert!(home.join(".nexus/host/9.9.9+abc/nexus-host").exists());
        let asked = fetched.lock().map(|f| f.len()).unwrap_or_default();
        // The manifest only, or the manifest and the host when the server could not download it.
        assert_eq!(asked, if case == "server" { 1 } else { 2 }, "{case}");
    }

    // A file that does not match its manifest is refused here, never copied over.
    let home = root.join("bad");
    let manifest = json!({ "downloads": {
        "linux-x86_64-nexus-host": { "url": "http://127.0.0.1:9/x.gz", "sha256": "00" },
        "linux-aarch64-nexus-host": { "url": "http://127.0.0.1:9/x.gz", "sha256": "00" },
    }});
    let fetch: Fetch = Arc::new(move |asked: String, _max| {
        let answer = if asked.ends_with("latest.json") {
            manifest.to_string().into_bytes()
        } else {
            b"not the host".to_vec()
        };
        Box::pin(async move { Ok(answer) })
    });
    let installer = Installer {
        target: Target::Local {
            env: vec![
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("PATH".into(), path.clone()),
                ("SHELL".into(), String::new()),
            ],
        },
        source: Source::release("http://127.0.0.1:9", "9.9.9+abc", None),
        fetch: Some(fetch),
    };
    let refused = installer.ensure().await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.contains("does not match its manifest")),
        "{refused:?}"
    );
    assert!(!home.join(".nexus/host/9.9.9+abc/nexus-host").exists());
    let _ = std::fs::remove_dir_all(&root);
}
