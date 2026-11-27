//! Per-instance deferred HTTP requests exercise the real Gateway chat worker, without external services.
use super::{ChatChoices, Event, Gateway};
use crate::attachments::Draft;
use futures::{FutureExt as _, StreamExt as _};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

struct Reply {
    status: u16,
    body: Value,
}
struct Call {
    method: String,
    path: String,
    body: Value,
    authorization: Option<String>,
    reply: oneshot::Sender<Reply>,
    written: oneshot::Receiver<()>,
}
impl Call {
    async fn answer(self, status: u16, body: Value) {
        self.reply
            .send(Reply { status, body })
            .unwrap_or_else(|_| panic!("held HTTP fixture awaits its exact reply"));
        self.written
            .await
            .expect("held HTTP fixture completed its response attempt");
    }
}
struct Server {
    url: String,
    calls: mpsc::UnboundedReceiver<Call>,
    accept: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.accept.abort();
    }
}
impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("local fixture listener");
        let url = format!("http://{}", listener.local_addr().expect("local fixture address"));
        let (send, calls) = mpsc::unbounded_channel();
        let accept = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let send = send.clone();
                tokio::spawn(async move {
                    let mut request = vec![];
                    let (head, body) = loop {
                        let mut buffer = [0; 2048];
                        let Ok(count) = stream.read(&mut buffer).await else {
                            return;
                        };
                        if count == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..count]);
                        assert!(request.len() < 1024 * 1024, "bounded fixture request");
                        if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&request[..end]).into_owned();
                            let length = head
                                .lines()
                                .find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                                .unwrap_or(0);
                            if request.len() >= end + 4 + length {
                                break (head, request[end + 4..end + 4 + length].to_vec());
                            }
                        }
                    };
                    let mut first = head.lines().next().expect("HTTP request line").split_whitespace();
                    let method = first.next().expect("HTTP method").to_string();
                    let path = first.next().expect("HTTP path").to_string();
                    let authorization = head.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("authorization")
                            .then(|| value.trim().to_string())
                    });
                    let body = if body.is_empty() {
                        Value::Null
                    } else {
                        serde_json::from_slice(&body).expect("JSON chat request")
                    };
                    let (reply, read) = oneshot::channel();
                    let (done, written) = oneshot::channel();
                    if send
                        .send(Call {
                            method,
                            path,
                            body,
                            authorization,
                            reply,
                            written,
                        })
                        .is_err()
                    {
                        return;
                    }
                    let Ok(Reply { status, body }) = read.await else { return };
                    let body = body.to_string();
                    let response = format!(
                        "HTTP/1.1 {status} fixture\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    // A revoked client may already have closed its socket; the exact reply was still attempted.
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = done.send(());
                });
            }
        });
        Self { url, calls, accept }
    }
    async fn next(&mut self) -> Call {
        tokio::time::timeout(Duration::from_secs(3), self.calls.recv())
            .await
            .expect("real chat request arrives")
            .expect("fixture server stays open")
    }
    fn no_more(&mut self) {
        assert!(
            self.calls.try_recv().is_err(),
            "no subsequent chat mutation reached HTTP"
        );
    }
}
fn run(work: impl std::future::Future<Output = ()>) {
    nexus_client::install_tls();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("fixture runtime")
        .block_on(work);
}
fn gateway(server: &Server) -> (Gateway, futures::channel::mpsc::UnboundedReceiver<Event>) {
    let (gateway, events) = Gateway::at(tokio::runtime::Handle::current(), &server.url);
    gateway.set_token(Some("fixture-key-a".into()));
    (gateway, events)
}
fn current_execution(gateway: &Gateway) -> (Gateway, futures::channel::mpsc::UnboundedReceiver<Event>) {
    let authority = {
        let _client = gateway.inner.client.lock().expect("coherent fixture authority");
        crate::backend::ExecutionAuthority::from_read(
            gateway.inner.cache_epoch.clone(),
            gateway.inner.cache_epoch.load(Ordering::SeqCst),
        )
    };
    gateway
        .execution_window(&authority)
        .expect("current fixture execution authority")
}
fn handle(gateway: &Gateway, turn: u64) -> tokio::task::AbortHandle {
    gateway
        .side
        .answering
        .lock()
        .expect("side turn registry")
        .get(&turn)
        .expect("actual chat worker registered")
        .clone()
}
async fn finished(handle: &tokio::task::AbortHandle) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the exact worker completed or was canceled");
}
fn current(mut event: Event) -> Option<Event> {
    loop {
        match event {
            Event::ReadSnapshot {
                epoch,
                expected,
                revision,
                event: inner,
            } => {
                if epoch.load(Ordering::SeqCst) != expected
                    || revision.is_some_and(|(epoch, expected)| epoch.load(Ordering::SeqCst) != expected)
                {
                    return None;
                }
                event = *inner;
            }
            event => return Some(event),
        }
    }
}
fn drain(events: &mut futures::channel::mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
    let mut all = vec![];
    while let Some(Some(event)) = events.next().now_or_never() {
        all.push(event);
    }
    all
}
fn ask(gateway: &Gateway, turn: u64, session: Option<&str>, choices: ChatChoices) {
    gateway.ask(
        turn,
        session.map(str::to_string),
        "question".into(),
        None,
        Draft::default(),
        choices,
    );
}

#[test]
fn credential_change_or_signout_aborts_every_side_before_created_and_aba_cannot_revive_them() {
    run(async {
        for replacement in [Some("fixture-key-b".to_string()), None] {
            let mut server = Server::start().await;
            let (first, _) = gateway(&server);
            let (second, _) = first.window();
            ask(&first, 1, None, ChatChoices::default());
            ask(&second, 1, None, ChatChoices::default());
            let a = server.next().await;
            let b = server.next().await;
            assert_eq!((a.method.as_str(), a.path.as_str()), ("POST", "/v1/chat/sessions"));
            assert_eq!((b.method.as_str(), b.path.as_str()), ("POST", "/v1/chat/sessions"));
            let a_handle = handle(&first, 1);
            let b_handle = handle(&second, 1);
            first.set_token(replacement);
            first.set_token(Some("fixture-key-a".into()));
            finished(&a_handle).await;
            finished(&b_handle).await;
            assert!(first.side.answering.lock().expect("first side").is_empty());
            assert!(second.side.answering.lock().expect("second side").is_empty());
            a.answer(200, json!({"id":"old-a"})).await;
            b.answer(200, json!({"id":"old-b"})).await;
            server.no_more();
        }
    });
}

#[test]
fn same_credential_refresh_preserves_real_create_and_message_dispatch() {
    run(async {
        let mut server = Server::start().await;
        let (gateway, mut events) = gateway(&server);
        ask(&gateway, 7, None, ChatChoices::default());
        let create = server.next().await;
        let worker = handle(&gateway, 7);
        gateway.set_token(Some("fixture-key-a".into()));
        assert!(!worker.is_finished(), "same credential does not cancel its answer");
        create.answer(200, json!({"id":"named"})).await;
        let message = server.next().await;
        assert_eq!(
            (message.method.as_str(), message.path.as_str()),
            ("POST", "/v1/chat/sessions/named/messages")
        );
        assert_eq!(message.body["stream"], true);
        // A nonretryable refusal ends the actual worker; no fake stream result is injected.
        message
            .answer(
                400,
                json!({"error":{"code":"fixture_refusal","message":"fixture refusal"}}),
            )
            .await;
        finished(&worker).await;
        let events = drain(&mut events).into_iter().filter_map(current).collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::Created { turn:7, id, .. } if id=="named"))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::AnswerFailed { turn: 7, .. }))
        );
        server.no_more();
    });
}

#[test]
fn create_continuation_checks_the_epoch_before_publishing_created_or_posting_a_message() {
    run(async {
        let mut server = Server::start().await;
        let (gateway, mut events) = gateway(&server);
        ask(&gateway, 9, None, ChatChoices::default());
        let create = server.next().await;
        let worker = handle(&gateway, 9);
        // Isolate the real interval after auth invalidation but before abort fanout: continuation itself must fence.
        gateway.inner.cache_epoch.fetch_add(1, Ordering::SeqCst);
        create.answer(200, json!({"id":"stale-created"})).await;
        finished(&worker).await;
        let events = drain(&mut events);
        assert!(
            !events.is_empty(),
            "the actual continuation finished and emitted its scoped refusal"
        );
        assert!(events.into_iter().all(|event| current(event).is_none()));
        server.no_more();
    });
}

#[test]
fn web_search_patch_continuation_checks_the_epoch_before_posting_a_message() {
    run(async {
        let mut server = Server::start().await;
        let (gateway, mut events) = gateway(&server);
        ask(
            &gateway,
            11,
            Some("existing"),
            ChatChoices {
                patch_web_search: true,
                web_search: true,
                ..ChatChoices::default()
            },
        );
        // The session was never read here, so the client reads its version before the edit.
        let read = server.next().await;
        assert_eq!(
            (read.method.as_str(), read.path.as_str()),
            ("GET", "/v1/chat/sessions/existing")
        );
        read.answer(200, json!({"id":"existing"})).await;
        let patch = server.next().await;
        assert_eq!(
            (patch.method.as_str(), patch.path.as_str()),
            ("PATCH", "/v1/chat/sessions/existing")
        );
        assert_eq!(patch.body, json!({"webSearch":true}));
        let worker = handle(&gateway, 11);
        gateway.inner.cache_epoch.fetch_add(1, Ordering::SeqCst);
        patch.answer(200, json!({})).await;
        finished(&worker).await;
        let events = drain(&mut events);
        assert!(!events.is_empty(), "the actual patch continuation completed");
        assert!(events.into_iter().all(|event| current(event).is_none()));
        server.no_more();
    });
}

#[test]
fn gateway_switch_invalidates_old_scope_and_cancels_its_pending_create() {
    run(async {
        let mut server = Server::start().await;
        let (gateway, _) = gateway(&server);
        ask(&gateway, 13, None, ChatChoices::default());
        let create = server.next().await;
        let worker = handle(&gateway, 13);
        let epoch = gateway.inner.cache_epoch.clone();
        let expected = epoch.load(Ordering::SeqCst);
        let (_next, _events) = gateway.switched("http://127.0.0.1:1");
        assert_ne!(epoch.load(Ordering::SeqCst), expected);
        // A handle arriving after the switch's abort fanout still carries the old scope.
        let late = tokio::spawn(std::future::pending::<()>());
        gateway.side.register(14, late.abort_handle(), &epoch, expected);
        assert!(
            late.await
                .expect_err("old-Gateway late registration canceled")
                .is_cancelled()
        );
        finished(&worker).await;
        create.answer(200, json!({"id":"old-gateway"})).await;
        server.no_more();
    });
}

#[test]
fn late_aba_registration_is_canceled_and_old_cleanup_cannot_remove_a_replacement_turn() {
    run(async {
        let server = Server::start().await;
        let (gateway, _) = gateway(&server);
        let epoch = gateway.inner.cache_epoch.clone();
        let expected = epoch.load(Ordering::SeqCst);
        let stale = tokio::spawn(std::future::pending::<()>());
        gateway.set_token(Some("fixture-key-b".into()));
        gateway.set_token(Some("fixture-key-a".into()));
        gateway.side.register(17, stale.abort_handle(), &epoch, expected);
        assert!(
            stale
                .await
                .expect_err("stale registered worker canceled")
                .is_cancelled()
        );

        let expected = epoch.load(Ordering::SeqCst);
        let old = tokio::spawn(std::future::pending::<()>());
        let old_id = old.id();
        gateway.side.register(17, old.abort_handle(), &epoch, expected);
        let replacement = tokio::spawn(std::future::pending::<()>());
        let replacement_id = replacement.id();
        gateway.side.register(17, replacement.abort_handle(), &epoch, expected);
        gateway.side.finished(17, old_id);
        assert_eq!(handle(&gateway, 17).id(), replacement_id);
        assert!(
            old.await
                .expect_err("replacement cancels original worker")
                .is_cancelled()
        );
        gateway.set_token(None);
        assert!(
            replacement
                .await
                .expect_err("replacement remains revocable")
                .is_cancelled()
        );
    });
}

#[test]
fn new_epoch_client_capture_waits_for_abort_fanout_and_its_real_request_survives() {
    run(async {
        let mut server = Server::start().await;
        let (gateway, mut events) = gateway(&server);
        // Synchronous phase: hold no mutex guard across an async await.
        let (capture_was_locked, returned_during_fanout) = {
            let side = gateway.side.clone();
            let registry = side.answering.lock().expect("hold exact Side fanout");
            let epoch = gateway.inner.cache_epoch.clone();
            let expected = epoch.load(Ordering::SeqCst);
            let transition_gateway = gateway.clone();
            let transition = std::thread::spawn(move || transition_gateway.set_token(Some("fixture-key-b".into())));
            let started_at = std::time::Instant::now();
            while epoch.load(Ordering::SeqCst) == expected {
                assert!(
                    started_at.elapsed() < Duration::from_secs(3),
                    "transition reaches held fanout"
                );
                std::thread::yield_now();
            }
            let capture_was_locked = gateway.inner.client.try_lock().is_err();
            let (started, start) = std::sync::mpsc::channel();
            let (returned, returned_read) = std::sync::mpsc::channel();
            let asking_gateway = gateway.clone();
            let asking = std::thread::spawn(move || {
                started.send(()).expect("new ask starts");
                ask(&asking_gateway, 19, None, ChatChoices::default());
                returned.send(()).expect("new ask returns");
            });
            start
                .recv_timeout(Duration::from_secs(3))
                .expect("actual new-client ask attempted");
            let returned_during_fanout = returned_read.try_recv().is_ok();
            drop(registry);
            transition.join().expect("credential transition completes");
            asking.join().expect("new-client ask completes registration");
            (capture_was_locked, returned_during_fanout)
        };
        assert!(
            capture_was_locked,
            "client capture must stay locked throughout abort fanout"
        );
        assert!(
            !returned_during_fanout,
            "the new ask cannot register inside the old abort fanout"
        );
        let worker = handle(&gateway, 19);
        let create = server.next().await;
        assert_eq!(create.authorization.as_deref(), Some("Bearer fixture-key-b"));
        create.answer(200, json!({"id":"new-epoch"})).await;
        let message = server.next().await;
        assert_eq!(message.path, "/v1/chat/sessions/new-epoch/messages");
        assert_eq!(message.authorization.as_deref(), Some("Bearer fixture-key-b"));
        message
            .answer(
                400,
                json!({"error":{"code":"fixture_refusal","message":"fixture refusal"}}),
            )
            .await;
        finished(&worker).await;
        let events = drain(&mut events).into_iter().filter_map(current).collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::Created { turn:19,id,.. } if id=="new-epoch"))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::AnswerFailed { turn: 19, .. }))
        );
        server.no_more();
    });
}

#[test]
fn execution_side_cannot_borrow_replacement_authority_even_after_aba_or_gateway_switch() {
    run(async {
        for transition in ["credential", "aba", "gateway"] {
            let mut server = Server::start().await;
            let (gateway, _) = gateway(&server);
            let (execution, mut old_events) = current_execution(&gateway);
            assert_eq!(
                execution.marker(),
                gateway.marker(),
                "fresh execution authority is current"
            );
            let original_marker = gateway.marker();
            let pinned = execution
                .side
                .execution_epoch
                .expect("dedicated executor pins authority");
            let current_gateway = match transition {
                "gateway" => gateway.switched(&server.url).0,
                "aba" => {
                    gateway.set_token(Some("fixture-key-b".into()));
                    gateway.send_all(|| Event::SignedIn);
                    gateway.set_token(Some("fixture-key-a".into()));
                    gateway.send_all(|| Event::SignedIn);
                    gateway.clone()
                }
                _ => {
                    gateway.set_token(Some("fixture-key-b".into()));
                    gateway.send_all(|| Event::SignedIn);
                    gateway.clone()
                }
            };
            assert_ne!(gateway.inner.cache_epoch.load(Ordering::SeqCst), pinned);
            // No auth event has been consumed. Marker observation is immediately and permanently revoked.
            assert_eq!(
                execution.marker(),
                None,
                "owner can observe revocation before draining auth events"
            );
            assert!(
                gateway.marker().is_some(),
                "ordinary Side still exposes its current credential"
            );
            if transition == "aba" {
                assert_eq!(
                    gateway.marker(),
                    original_marker,
                    "ordinary marker intentionally has ABA equality"
                );
            }
            // Simulates an owner's successful precheck immediately BEFORE the transition, then its delayed
            // queued dispatch. Admission is synchronous: no task may be spawned with the replacement client.
            ask(
                &execution,
                31,
                Some("old-session"),
                ChatChoices {
                    patch_web_search: true,
                    ..Default::default()
                },
            );
            assert!(
                execution
                    .side
                    .answering
                    .lock()
                    .expect("old executor registry")
                    .is_empty()
            );
            assert!(
                drain(&mut old_events)
                    .into_iter()
                    .filter_map(current)
                    .all(|event| !matches!(event, Event::Created { .. } | Event::Asked { .. }))
            );
            server.no_more();

            // A fresh executor still dispatches a real create and message with its coherently captured client.
            current_gateway.set_token(Some("fixture-key-current".into()));
            let (fresh, _) = current_execution(&current_gateway);
            ask(&fresh, 32, None, ChatChoices::default());
            let worker = handle(&fresh, 32);
            let create = server.next().await;
            assert_eq!(create.path, "/v1/chat/sessions");
            assert_eq!(create.authorization.as_deref(), Some("Bearer fixture-key-current"));
            create.answer(200, json!({"id":"fresh-owner"})).await;
            let message = server.next().await;
            assert_eq!(message.path, "/v1/chat/sessions/fresh-owner/messages");
            assert_eq!(message.authorization.as_deref(), Some("Bearer fixture-key-current"));
            message
                .answer(
                    400,
                    json!({"error":{"code":"fixture_refusal","message":"fixture refusal"}}),
                )
                .await;
            finished(&worker).await;
            server.no_more();
        }
    });
}

async fn accepted_account(
    gateway: &Gateway,
    events: &mut futures::channel::mpsc::UnboundedReceiver<Event>,
    server: &mut Server,
) -> crate::backend::ExecutionAuthority {
    gateway.load_account();
    let request = server.next().await;
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("GET", "/v1/tenants/me")
    );
    request
        .answer(
            200,
            json!({
                "tenant":{"id":"org-a","slug":"org-a","name":"Fixture organization","kind":"company"},
                "user":{"id":"person-a","personId":"person-a","name":"A","email":"a@example.test"},
                "role":"member","permissions":[]
            }),
        )
        .await;
    let event = tokio::time::timeout(Duration::from_secs(3), events.next())
        .await
        .expect("actual Account read publishes its envelope")
        .expect("Account receiver stays live");
    let Event::ReadSnapshot {
        epoch,
        expected,
        revision,
        event,
    } = event
    else {
        panic!("no unrequested event may stand in for an Account proof");
    };
    assert!(
        matches!(*event, Event::Account(_)),
        "only the requested Account read may publish a snapshot"
    );
    assert!(revision.is_none_or(|(epoch, expected)| epoch.load(Ordering::SeqCst) == expected));
    let authority = crate::backend::ExecutionAuthority::from_read(epoch, expected);
    assert!(authority.current(), "proof is accepted before transition");
    authority
}

#[test]
fn account_read_proof_fences_first_factory_admission_across_aba_and_foreign_gateway() {
    run(async {
        for transition in ["credential", "aba", "gateway"] {
            let mut server = Server::start().await;
            let (gateway, mut events) = gateway(&server);
            let proof = accepted_account(&gateway, &mut events, &mut server).await;
            let original_marker = gateway.marker();
            let original_epoch = gateway.inner.cache_epoch.load(Ordering::SeqCst);
            let (foreign, _) = Gateway::at(tokio::runtime::Handle::current(), &server.url);
            foreign.set_token(Some("fixture-key-a".into()));
            assert_eq!(foreign.inner.cache_epoch.load(Ordering::SeqCst), original_epoch);
            assert_eq!(foreign.marker(), original_marker);
            assert!(
                foreign.execution_window(&proof).is_none(),
                "same numeric generation, URL and credential on a different Inner cannot authorize"
            );

            // The UI accepted the actual Account reply, then credentials changed BEFORE the first owner factory.
            // No SignedIn has been consumed and no executor exists yet.
            let current_gateway = match transition {
                "gateway" => gateway.switched(&server.url).0,
                "aba" => {
                    gateway.set_token(Some("fixture-key-b".into()));
                    gateway.send_all(|| Event::SignedIn);
                    gateway.set_token(Some("fixture-key-a".into()));
                    gateway.send_all(|| Event::SignedIn);
                    assert_eq!(gateway.marker(), original_marker);
                    gateway.clone()
                }
                _ => {
                    gateway.set_token(Some("fixture-key-b".into()));
                    gateway.send_all(|| Event::SignedIn);
                    gateway.clone()
                }
            };
            let sides_before = gateway.sides().len();
            assert!(!proof.current());
            assert!(
                gateway.execution_window(&proof).is_none(),
                "old proof never borrows the new epoch"
            );
            assert!(
                current_gateway.execution_window(&proof).is_none(),
                "new Gateway never relabels old proof"
            );
            assert_eq!(
                gateway.sides().len(),
                sides_before,
                "refusal allocates no execution Side"
            );
            server.no_more();

            current_gateway.set_token(Some("fixture-key-current".into()));
            let (reader, mut read_events) = current_gateway.window();
            let fresh_proof = accepted_account(&reader, &mut read_events, &mut server).await;
            let (fresh, _) = current_gateway
                .execution_window(&fresh_proof)
                .expect("fresh accepted proof");
            ask(&fresh, 41, None, ChatChoices::default());
            let worker = handle(&fresh, 41);
            let create = server.next().await;
            assert_eq!(create.path, "/v1/chat/sessions");
            assert_eq!(create.authorization.as_deref(), Some("Bearer fixture-key-current"));
            create.answer(200, json!({"id":"proof-owner"})).await;
            let message = server.next().await;
            assert_eq!(message.path, "/v1/chat/sessions/proof-owner/messages");
            assert_eq!(message.authorization.as_deref(), Some("Bearer fixture-key-current"));
            message
                .answer(
                    400,
                    json!({"error":{"code":"fixture_refusal","message":"fixture refusal"}}),
                )
                .await;
            finished(&worker).await;
            server.no_more();
        }
    });
}
