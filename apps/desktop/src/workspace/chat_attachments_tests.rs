//! Saved Gateway messages go through the production native workspace, against a local fake Gateway.
use super::{Load, start};
use crate::gateway::{Event, Gateway, OpenedChat};
use crate::workspace::overlay::OverlaySlot;
use crate::workspace::{Auth, View, Workspace, machine_source::MachineSource};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt;
use gpui_kit::{Entity, Task, TestAppContext, VisualTestContext};
use serde_json::{Value, json};
use std::io::{Cursor, Read, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

struct Server {
    runtime: tokio::runtime::Runtime,
    url: String,
    reads: Arc<Mutex<Vec<String>>>,
    fail_file: Arc<AtomicBool>,
    messages: Arc<Mutex<Vec<Value>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let reads = Arc::new(Mutex::new(Vec::new()));
        let seen = reads.clone();
        let fail_file = Arc::new(AtomicBool::new(false));
        let failed = fail_file.clone();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let posted = messages.clone();
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut head = Vec::new();
                while !head.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut buffer = [0; 1024];
                    let count = stream.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    head.extend_from_slice(&buffer[..count]);
                }
                let boundary = head
                    .windows(4)
                    .position(|part| part == b"\r\n\r\n")
                    .expect("HTTP headers")
                    + 4;
                let headers = String::from_utf8_lossy(&head[..boundary]);
                let path = headers.split_whitespace().nth(1).unwrap_or("").to_string();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().expect("body length"))
                    })
                    .unwrap_or(0);
                let mut body = head[boundary..].to_vec();
                while body.len() < length {
                    let mut buffer = [0; 4096];
                    let count = stream.read(&mut buffer).expect("HTTP request body");
                    assert_ne!(count, 0, "complete request body");
                    body.extend_from_slice(&buffer[..count]);
                }
                if path == "/v1/chat/sessions/saved%20chat/messages" {
                    assert!(
                        headers
                            .lines()
                            .any(|line| { line.eq_ignore_ascii_case("authorization: Bearer attachment-fixture-key") })
                    );
                    posted
                        .lock()
                        .expect("posted messages")
                        .push(serde_json::from_slice(&body[..length]).expect("message JSON"));
                }
                if path == "/stop" {
                    break;
                }
                seen.lock().unwrap().push(path.clone());
                let (status, body) = match path.rsplit('/').next().unwrap_or("") {
                    "me" if path == "/v1/tenants/me" => (
                        "200 OK",
                        json!({
                            "tenant":{"id":"attachment-org","slug":"attachment-org","name":"Fixture organization","kind":"company"},
                            "user":{"id":"attachment-person","personId":"attachment-person","name":"Fixture","email":"attachments@example.test"},
                            "role":"member","permissions":[]
                        }),
                    ),
                    "image%20one" => ("200 OK", image()),
                    "file%2Ftwo" if !failed.load(Ordering::SeqCst) => (
                        "200 OK",
                        json!({"name":"notes.csv","mediaType":"text/csv","data":STANDARD.encode(b"x,y\n1,2")}),
                    ),
                    _ => (
                        "404 Not Found",
                        json!({"error":{"code":"not_found","message":"Attachment unavailable"}}),
                    ),
                };
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
            url,
            reads,
            fail_file,
            messages,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(mut stream) = std::net::TcpStream::connect(self.url.trim_start_matches("http://")) {
            let _ = stream.write_all(b"GET /stop HTTP/1.1\r\nhost: fake\r\n\r\n");
        }
        self.thread.take().unwrap().join().unwrap();
    }
}
fn image() -> Value {
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    json!({"name":"picture.png","mediaType":"image/png","data":STANDARD.encode(png.into_inner())})
}
fn chat() -> OpenedChat {
    OpenedChat::from_detail(
        "saved chat".into(),
        &json!({"messages":[
            {"id":"question","role":"user","content":"Read these","attachments":[
                {"id":"image one","name":"picture.png"}, {"id":"file/two","name":"notes.csv"}
            ]},
            {"id":"answer","role":"assistant","content":"Saved answer"}
        ]}),
    )
}
fn fixture<'a>(cx: &'a mut TestAppContext, server: &Server) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let gateway = Gateway::for_view_test_at(server.runtime.handle().clone(), &server.url);
    let (_, events) = futures::channel::mpsc::unbounded();
    fixture_on(cx, gateway, events, None)
}

fn fixture_on<'a>(
    cx: &'a mut TestAppContext,
    gateway: Gateway,
    events: futures::channel::mpsc::UnboundedReceiver<Event>,
    account: Option<Event>,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = Task::ready(());
        workspace.runner = None;
        workspace.auth = Auth::SignedIn;
        workspace.view = View::Chat;
        if let Some(account) = account {
            workspace.account_view_fixture = true;
            workspace.handle(account, window, cx);
            // Install the production draft model in memory before any authenticated render/open.
            crate::workspace::drafts::fixture_memory_scope(&workspace, cx);
        }
        workspace
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    (workspace, cx)
}

struct AccountGuard {
    epoch: Arc<AtomicU64>,
    expected: u64,
    revision: Option<(Arc<AtomicU64>, u64)>,
    marker: String,
}
impl AccountGuard {
    fn assert_current(&self, workspace: &Workspace) {
        assert!(matches!(workspace.auth, Auth::SignedIn));
        let account = workspace.account.as_ref().expect("actual Account read accepted");
        assert_eq!(account.org, "attachment-org");
        assert_eq!(account.person_id.as_deref(), Some("attachment-person"));
        assert_eq!(workspace.gateway.marker().as_deref(), Some(self.marker.as_str()));
        assert_eq!(
            self.epoch.load(Ordering::SeqCst),
            self.expected,
            "same authenticated transport epoch"
        );
        if let Some((revision, expected)) = &self.revision {
            assert_eq!(
                revision.load(Ordering::SeqCst),
                *expected,
                "Account proof was current when accepted"
            );
        }
    }
}
fn authenticated_fixture<'a>(
    cx: &'a mut TestAppContext,
    server: &Server,
) -> (Entity<Workspace>, &'a mut VisualTestContext, AccountGuard) {
    let (gateway, mut events) =
        Gateway::for_authenticated_view_test_at(server.runtime.handle().clone(), &server.url, "attachment-fixture-key");
    let marker = gateway.marker().expect("explicit fixture credential");
    gateway.load_account();
    let snapshot = server.runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(3), events.next())
            .await
            .expect("loopback Account response")
            .expect("actual Gateway event")
    });
    let Event::ReadSnapshot {
        epoch,
        expected,
        revision,
        event,
    } = &snapshot
    else {
        panic!("Account must arrive in the real Gateway read envelope")
    };
    assert!(matches!(&**event, Event::Account(_)));
    let guard = AccountGuard {
        epoch: epoch.clone(),
        expected: *expected,
        revision: revision.clone(),
        marker,
    };
    let (workspace, cx) = fixture_on(cx, gateway, events, Some(snapshot));
    workspace.read_with(cx, |workspace, _| guard.assert_current(workspace));
    (workspace, cx, guard)
}
fn open(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
    workspace.update_in(cx, |this, window, cx| {
        this.current = Some("saved chat".into());
        this.opening = true;
        this.handle(Event::Opened(Ok(chat())), window, cx);
    });
}
// Pump both executors on this test thread: GPUI's deterministic scheduler rejects wakes
// from Tokio worker threads, and its simulated timer cannot wait for real network I/O.
fn wait_for(
    server: &Server,
    workspace: &Entity<Workspace>,
    cx: &mut VisualTestContext,
    predicate: impl Fn(&Workspace) -> bool,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        cx.run_until_parked();
        if workspace.read_with(cx, |this, _| predicate(this)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "saved attachments did not finish loading"
        );
        server.runtime.block_on(async {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        });
    }
}

#[gpui_kit::test]
fn saved_chat_restores_images_and_files_after_reopening(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let server = Server::new();
    let (workspace, cx) = fixture(cx, &server);
    for _ in 0..3 {
        open(&workspace, cx);
        wait_for(&server, &workspace, cx, |this| !this.entries[0].attachments.is_empty());
        workspace.read_with(cx, |this, _| {
            let question = &this.entries[0];
            assert_eq!(question.attachments.images.len(), 1);
            assert_eq!(question.attachments.files.len(), 1);
            assert!(question.attachments.images[0].thumbnail.is_some());
            assert_eq!(question.attachments.files[0].bytes.as_slice(), b"x,y\n1,2");
            assert_eq!(question.stored.attachments, chat().messages[0].stored.attachments);
        });
        workspace.update_in(cx, |this, _, cx| {
            let files = &this.entries[0].attachments;
            let body = crate::attachments::chat_body("Read these", None, files);
            assert_eq!(
                body["images"][0]["data"],
                format!("data:image/png;base64,{}", image()["data"].as_str().unwrap())
            );
            assert_eq!(
                body["files"][0]["data"],
                format!("data:text/csv;base64,{}", STANDARD.encode(b"x,y\n1,2"))
            );
            let images = files.images.clone();
            this.open_image(&images, 0, cx);
            assert!(this.overlay.image_viewer().is_some(), "restored images open full size");
            this.close_image(cx);
            // Opening another session discards its view; the next iteration must read Gateway again.
            this.current = Some("another chat".into());
            this.entries.clear();
        });
    }
    let reads = server.reads.lock().unwrap();
    assert_eq!(reads.iter().filter(|path| path.contains("/attachments/")).count(), 6);
    assert!(
        reads
            .iter()
            .any(|path| path == "/v1/chat/sessions/saved%20chat/attachments/image%20one")
    );
    assert!(
        reads
            .iter()
            .any(|path| path == "/v1/chat/sessions/saved%20chat/attachments/file%2Ftwo")
    );
}

#[gpui_kit::test]
fn unavailable_attachments_keep_available_files_and_block_incomplete_retries(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let server = Server::new();
    server.fail_file.store(true, Ordering::SeqCst);
    let (workspace, cx, authority) = authenticated_fixture(cx, &server);
    open(&workspace, cx);
    workspace.update_in(cx, |this, _, cx| {
        authority.assert_current(this);
        this.retry(cx);
        authority.assert_current(this);
        assert!(server.messages.lock().expect("message requests").is_empty());
        assert!(this.asking.is_none(), "a pending read must not send a text-only retry");
        assert_eq!(this.entries.len(), 2);
    });
    wait_for(&server, &workspace, cx, |this| {
        matches!(this.entries[0].attachment_load, Load::Failed(_))
    });
    workspace.update_in(cx, |this, _, cx| {
        let question = &this.entries[0];
        assert_eq!(question.attachments.images.len(), 1);
        assert!(question.attachments.files.is_empty());
        assert_eq!(question.stored.attachments.len(), 2);
        let Load::Failed(errors) = &question.attachment_load else {
            panic!("missing attachment error")
        };
        assert!(errors[0].contains("notes.csv"));
        authority.assert_current(this);
        this.retry(cx);
        authority.assert_current(this);
        assert!(server.messages.lock().expect("message requests").is_empty());
        assert!(this.asking.is_none(), "unavailable files must not be silently omitted");
        assert_eq!(this.entries.len(), 2);
    });
    server.fail_file.store(false, Ordering::SeqCst);
    // GPUI only records selector bounds when its dependency has debug assertions.
    #[cfg(debug_assertions)]
    {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let retry = cx
            .debug_bounds("reload-chat-attachments")
            .expect("the recovery control is visible");
        cx.simulate_click(retry.center(), gpui_kit::Modifiers::none());
    }
    #[cfg(not(debug_assertions))]
    workspace.update_in(cx, |this, _, cx| this.reload_chat_attachments(this.entries[0].id, cx));
    wait_for(&server, &workspace, cx, |this| this.entries[0].attachment_load.ready());
    workspace.update_in(cx, |this, _, cx| {
        assert_eq!(
            this.entries[0].attachments.images.len(),
            1,
            "retry must not duplicate images"
        );
        assert_eq!(this.entries[0].attachments.files.len(), 1);
        authority.assert_current(this);
        this.retry(cx);
        assert!(this.asking.is_some());
        assert_eq!(this.entries[0].attachments.images.len(), 1);
        assert_eq!(this.entries[0].attachments.files.len(), 1);
    });
    wait_for(&server, &workspace, cx, |_| {
        !server.messages.lock().expect("message requests").is_empty()
    });
    // Consume the fake server's terminal refusal before dropping the test scheduler.
    wait_for(&server, &workspace, cx, |workspace| workspace.asking.is_none());
    workspace.read_with(cx, |workspace, _| {
        // Writes advance cache revision, but never change the authenticated epoch or credential.
        assert_eq!(workspace.gateway.marker().as_deref(), Some(authority.marker.as_str()));
        assert_eq!(authority.epoch.load(Ordering::SeqCst), authority.expected);
    });
    let messages = server.messages.lock().expect("message requests");
    assert_eq!(messages.len(), 1, "exactly one complete retry reached HTTP");
    assert_eq!(messages[0]["content"], "Read these");
    assert_eq!(
        messages[0]["images"][0]["data"],
        format!("data:image/png;base64,{}", image()["data"].as_str().expect("PNG bytes"))
    );
    assert_eq!(
        messages[0]["files"][0]["data"],
        format!("data:text/csv;base64,{}", STANDARD.encode(b"x,y\n1,2"))
    );
}

fn delayed(entry: &mut super::Entry, epoch: &Arc<AtomicU64>, cx: &mut gpui_kit::Context<Workspace>) {
    let queries = [
        image(),
        json!({"name":"notes.csv","mediaType":"text/csv","data":STANDARD.encode(b"x,y\n1,2")}),
    ]
    .into_iter()
    .map(|value| crate::backend::Query {
        cached: None,
        cached_generation: None,
        refresh: Box::pin(async move { Ok(value) }),
        generation: Some((epoch.clone(), epoch.load(Ordering::SeqCst))),
    })
    .collect();
    start(entry, queries, cx);
}

#[gpui_kit::test]
fn late_attachment_results_cannot_replace_a_reopened_chat_or_a_new_accounts_entry(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let server = Server::new();
    let (workspace, cx) = fixture(cx, &server);
    let epoch = Arc::new(AtomicU64::new(0));
    workspace.update_in(cx, |this, _, cx| {
        let mut old = super::Entry::from_message(1, chat().messages.remove(0), cx);
        delayed(&mut old, &epoch, cx);
        this.entries = vec![super::Entry::from_message(2, chat().messages.remove(0), cx)];
    });
    cx.run_until_parked();
    workspace.read_with(cx, |this, _| assert!(this.entries[0].attachments.is_empty()));
    workspace.update_in(cx, |this, _, cx| {
        delayed(&mut this.entries[0], &epoch, cx);
        epoch.fetch_add(1, Ordering::SeqCst);
    });
    cx.run_until_parked();
    workspace.read_with(cx, |this, _| assert!(this.entries[0].attachments.is_empty()));
}

#[gpui_kit::test]
fn account_change_after_the_attachment_response_discards_decoded_bytes(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let server = Server::new();
    let (workspace, cx) = fixture(cx, &server);
    let epoch = Arc::new(AtomicU64::new(0));
    workspace.update_in(cx, |this, _, cx| {
        let mut entry = super::Entry::from_message(1, chat().messages.remove(0), cx);
        entry.stored.attachments.truncate(1);
        let at_response = epoch.clone();
        let query = crate::backend::Query {
            cached: None,
            cached_generation: None,
            generation: Some((epoch.clone(), 0)),
            refresh: Box::pin(async move {
                let response = image();
                // Query::poll already accepted this account. Change it after the response,
                // before background decoding can commit the bytes to the transcript.
                at_response.fetch_add(1, Ordering::SeqCst);
                Ok(response)
            }),
        };
        start(&mut entry, vec![query], cx);
        this.entries = vec![entry];
    });
    cx.run_until_parked();
    assert_eq!(epoch.load(Ordering::SeqCst), 1, "the response was read");
    workspace.read_with(cx, |this, _| assert!(this.entries[0].attachments.is_empty()));
}

#[gpui_kit::test]
fn attachment_results_follow_a_chat_that_keeps_running_after_navigation(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(Vec::new());
    let server = Server::new();
    let (workspace, cx) = fixture(cx, &server);
    let epoch = Arc::new(AtomicU64::new(0));
    workspace.update_in(cx, |this, _, cx| {
        let mut entry = super::Entry::from_message(1, chat().messages.remove(0), cx);
        delayed(&mut entry, &epoch, cx);
        this.entries = vec![entry];
        this.current = Some("saved chat".into());
        this.asking = Some(1);
        assert!(this.detach_live_chat());
        this.current = Some("other chat".into());
        this.entries = vec![super::Entry::user(2, "Other prompt".into())];
    });
    cx.run_until_parked();
    workspace.update_in(cx, |this, _, _| {
        assert!(this.entries[0].attachments.is_empty());
        let saved = this.chat_entry_mut(1).unwrap();
        assert_eq!(saved.attachments.images.len(), 1);
        assert_eq!(saved.attachments.files.len(), 1);
        assert!(saved.attachment_load.ready());
    });
}

#[test]
fn malformed_images_are_reported_without_hiding_the_next_file() {
    let refs = chat().messages.remove(0).stored.attachments;
    let (draft, load) = super::restore(vec![
        (
            refs[0].clone(),
            Ok(json!({"mediaType":"image/png","data":"aW52YWxpZA=="})),
        ),
        (
            refs[1].clone(),
            Ok(json!({"mediaType":"text/csv","data":STANDARD.encode(b"x,y")})),
        ),
    ]);
    assert!(draft.images.is_empty());
    assert_eq!(
        draft.files[0].name, "notes.csv",
        "the saved reference supplies a missing payload name"
    );
    assert!(matches!(load, Load::Failed(errors) if errors[0].contains("picture.png")));
}
