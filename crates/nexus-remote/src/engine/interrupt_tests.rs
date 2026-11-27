//! The real service route must recheck authority after its held connection becomes ready.
use super::*;
use crate::machine::{Backoff, Link, Opened, Sink};
use futures::{SinkExt, StreamExt, future::BoxFuture};
use nexus_workbench::api::{Machine, PROTOCOL, Request};
use nexus_workbench::{
    Config, Emit, Host, Launch, LaunchRequest, ProcessEnd,
    event::{SessionAgent, Usage},
    store::Store,
};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::{io::DuplexStream, sync::Notify};
use tokio_tungstenite::tungstenite::Message;

struct IsolatedHost;
impl Host for IsolatedHost {
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, crate::Result<Launch>> {
        Box::pin(async { Err("No agent process belongs to this service fixture.".into()) })
    }
    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
    fn price(&self, _: &str, _: &Usage) -> Option<f64> {
        None
    }
    fn emit(&self, _: Emit) {}
}
impl Machine for IsolatedHost {
    fn tool_path(&self) -> String {
        String::new()
    }
    fn home(&self) -> crate::Result<std::path::PathBuf> {
        Err("No user home in this fixture.".into())
    }
    fn agent(&self, agent: SessionAgent) -> AgentInfo {
        AgentInfo {
            agent,
            path: None,
            version: None,
            models: None,
            problem: None,
        }
    }
    fn subscriptions_allowed(&self) -> BoxFuture<'static, bool> {
        Box::pin(async { false })
    }
}
struct HeldLink {
    stream: Mutex<Option<DuplexStream>>,
    opened: Notify,
    release: Notify,
}
impl Link for HeldLink {
    fn open(&self) -> BoxFuture<'_, crate::Result<Opened>> {
        Box::pin(async move {
            self.opened.notify_one();
            self.release.notified().await;
            let stream = self
                .stream
                .lock()
                .expect("per-instance stream")
                .take()
                .expect("one connection");
            Ok(Opened {
                stream: Box::new(stream),
                token: "service-fixture-token".into(),
                version: None,
                keep: None,
                sign_in: None,
            })
        })
    }
    fn update(&self) -> BoxFuture<'_, crate::Result<()>> {
        Box::pin(async { Err("No installer in this fixture.".into()) })
    }
}
#[derive(Default)]
struct Connected(Notify);
impl Sink for Connected {
    fn push(&self, _: &crate::client::Pushed) {}
    fn status(&self, status: &Status) {
        if matches!(status, Status::Connected { .. }) {
            self.0.notify_one();
        }
    }
}

#[tokio::test(start_paused = true)]
async fn reconnect_wait_rechecks_frozen_authority_before_real_service_enqueue() {
    let (client, server) = tokio::io::duplex(16 * 1024);
    let link = Arc::new(HeldLink {
        stream: Mutex::new(Some(client)),
        opened: Notify::new(),
        release: Notify::new(),
    });
    let connected = Arc::new(Connected::default());
    let calls = Arc::new(Mutex::new(Vec::<Request>::new()));
    let seen = calls.clone();
    let host = tokio::spawn(async move {
        let mut socket = tokio_tungstenite::accept_async(server)
            .await
            .expect("actual WebSocket upgrade");
        let frame = socket.next().await.expect("client hello").expect("valid hello frame");
        let hello: Value = serde_json::from_str(frame.to_text().expect("hello text")).expect("hello JSON");
        assert_eq!(hello["hello"]["token"], "service-fixture-token");
        socket.send(Message::text(json!({"hello": {
            "version": "fixture", "protocol": PROTOCOL, "machine": "isolated-service", "org": null, "features": []
        }}).to_string())).await.expect("actual host hello");
        while let Some(frame) = socket.next().await {
            let frame = frame.expect("valid command frame");
            if frame.is_close() {
                break;
            }
            if !frame.is_text() {
                continue;
            }
            let request: Request =
                serde_json::from_str(frame.to_text().expect("command text")).expect("actual command wire shape");
            seen.lock().expect("recorded commands").push(request.clone());
            socket
                .send(Message::text(json!({"id": request.id, "ok": null}).to_string()))
                .await
                .expect("actual command reply");
        }
    });
    let remote = Remote::start_manual(link.clone(), connected.clone(), Backoff::default());
    tokio::time::timeout(Duration::from_secs(3), link.opened.notified())
        .await
        .expect("actual service connection reached its held link");
    let wb = Workbench::new(
        Store::in_memory().expect("isolated in-memory store"),
        Arc::new(IsolatedHost),
        Config::default(),
    )
    .expect("unused local API");
    let engine = Engine::service(Arc::new(Api::new(wb, Arc::new(IsolatedHost))), remote.clone());
    let epoch = Arc::new(AtomicU64::new(1));
    let captured = epoch.clone();
    let old = move || captured.load(Ordering::SeqCst) == 1;
    let mut command = Box::pin(engine.interrupt_guarded("original-session", &old));
    assert!(
        futures::poll!(&mut command).is_pending(),
        "actual guarded route must wait for the held service connection"
    );
    assert!(calls.lock().expect("no queued wire calls").is_empty());
    epoch.fetch_add(2, Ordering::SeqCst); // Credentials can return to the same value, but this proof stays revoked.
    link.release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), connected.0.notified())
        .await
        .expect("released real service connection became ready");
    tokio::time::advance(Duration::from_millis(50)).await;
    let error = tokio::time::timeout(Duration::from_secs(3), command)
        .await
        .expect("held command completed after readiness")
        .expect_err("revocation during readiness must refuse the actual command");
    assert!(error.contains("source authority expired"));
    assert!(
        calls.lock().expect("no stale command").is_empty(),
        "revoked readiness cannot enqueue a Stop"
    );

    let expected = epoch.load(Ordering::SeqCst);
    let captured = epoch.clone();
    let fresh = move || captured.load(Ordering::SeqCst) == expected;
    tokio::time::timeout(
        Duration::from_secs(3),
        engine.interrupt_guarded("original-session", &fresh),
    )
    .await
    .expect("fresh service reply arrived")
    .expect("fresh guarded Stop still works through the real service");
    tokio::time::timeout(Duration::from_secs(3), engine.interrupt("ordinary-session"))
        .await
        .expect("ordinary service reply arrived")
        .expect("ordinary Stop keeps its existing behavior");
    let commands = calls.lock().expect("actual dispatched commands").clone();
    assert_eq!(commands.len(), 2);
    assert_eq!(
        commands[0],
        Request {
            id: 1,
            call: "wb_interrupt".into(),
            args: json!({"threadId":"original-session"})
        }
    );
    assert_eq!(
        commands[1],
        Request {
            id: 2,
            call: "wb_interrupt".into(),
            args: json!({"threadId":"ordinary-session"})
        }
    );
    assert!(!old(), "the old proof was never recaptured");
    remote.close();
    host.abort();
    let _ = host.await;
}
