//! The client against a fake Codex app-server: the messages Codex 0.169 sends and takes, as `codex app-server`
//! answered them on a real Unix socket (two clients, one thread).

use futures::{SinkExt, StreamExt};
use tokio::io::DuplexStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;

use super::*;

const THREAD: &str = "019a-thread";

struct Fake(WebSocketStream<DuplexStream>);

impl Fake {
    async fn next(&mut self) -> Value {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), self.0.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => return serde_json::from_str(&text).expect("JSON"),
                Ok(Some(Ok(_))) => continue,
                other => panic!("the client sent nothing: {other:?}"),
            }
        }
    }
    async fn send(&mut self, value: Value) {
        self.0
            .send(Message::Text(value.to_string().into()))
            .await
            .expect("send");
    }
    /// Takes the client's next request, checks its method, and answers it.
    async fn answer(&mut self, method: &str, result: Value) -> Value {
        let request = self.next().await;
        assert_eq!(request["method"], method, "{request}");
        self.send(json!({"id": request["id"], "result": result})).await;
        request
    }
}

async fn connected() -> (Arc<Client>, Fake, mpsc::UnboundedReceiver<WbEvent>) {
    let (near, far) = tokio::io::duplex(1 << 16);
    let mut fake = Fake(WebSocketStream::from_raw_socket(far, Role::Server, None).await);
    let (events, read) = mpsc::unbounded_channel();
    let ws = WebSocketStream::from_raw_socket(near, Role::Client, None).await;
    let client = tokio::spawn(Client::start(ws, events));
    fake.answer("initialize", json!({"userAgent": "fake"})).await;
    assert_eq!(fake.next().await, json!({"method": "initialized"}));
    let client = client.await.expect("task").expect("initialized");
    assert!(!client.ready(), "read-only until it attaches");
    let attaching = {
        let client = client.clone();
        tokio::spawn(async move { client.attach(THREAD).await })
    };
    let resume = fake.answer("thread/resume", json!({"thread": {"id": THREAD}})).await;
    assert_eq!(resume["params"], json!({"threadId": THREAD}));
    attaching.await.expect("task").expect("attached");
    assert!(client.ready());
    (client, fake, read)
}

async fn event(read: &mut mpsc::UnboundedReceiver<WbEvent>) -> WbEvent {
    tokio::time::timeout(Duration::from_secs(5), read.recv())
        .await
        .expect("an event")
        .expect("open")
}

fn perform(client: &Arc<Client>, action: Action) -> tokio::task::JoinHandle<Result<(), String>> {
    let client = client.clone();
    tokio::spawn(async move { client.perform("t_local", None, action).await })
}

fn approve(request_id: &str, decision: Decision) -> Action {
    Action::Approve {
        request_id: request_id.into(),
        decision,
        message: None,
        answers: None,
    }
}

fn approval(id: u64, item: &str, thread: &str) -> Value {
    json!({"method": COMMAND, "id": id, "params": {"threadId": thread, "turnId": "turn_1", "itemId": item,
        "command": "/bin/zsh -lc 'echo hi'", "availableDecisions": ["accept", "acceptForSession", "cancel"]}})
}

#[tokio::test]
async fn the_phone_answers_an_approval_the_tui_also_sees() {
    let (client, mut fake, mut read) = connected().await;
    fake.send(json!({"method": "turn/started", "params": {"threadId": THREAD, "turn": {"id": "turn_1"}}}))
        .await;
    // Another thread's request (a sub-agent's, or another client's) is the TUI's alone.
    fake.send(approval(7, "exec-other", "019a-other")).await;
    fake.send(approval(0, "exec-1", THREAD)).await;
    let WbEvent::Approval {
        request_id,
        tool,
        input,
        suggestions,
        ..
    } = event(&mut read).await
    else {
        panic!("an approval");
    };
    assert_eq!(request_id, "exec-1#0");
    assert_eq!(tool, "Bash");
    assert_eq!(input, json!({"command": "/bin/zsh -lc 'echo hi'"}));
    assert_eq!(suggestions[0].raw, json!("acceptForSession"));

    let answered = perform(&client, approve(&request_id, Decision::AllowSession));
    assert_eq!(
        fake.next().await,
        json!({"id": 0, "result": {"decision": "acceptForSession"}})
    );
    answered.await.expect("task").expect("answered");
    assert!(matches!(event(&mut read).await,
        WbEvent::Notice { text } if text == "Your phone allowed Bash for this session."));
    fake.send(json!({"method": "serverRequest/resolved", "params": {"threadId": THREAD, "requestId": 0}}))
        .await;
    assert!(matches!(event(&mut read).await,
        WbEvent::ApprovalResolved { request_id, decision: Decision::AllowSession, .. } if request_id == "exec-1#0"));
    assert!(
        perform(&client, approve("exec-1#0", Decision::Deny))
            .await
            .expect("task")
            .is_err(),
        "an answered request takes no second answer"
    );
}

#[tokio::test]
async fn an_approval_the_tui_answered_first_reads_as_its_answer_and_refuses_the_phone() {
    let (client, mut fake, mut read) = connected().await;
    fake.send(json!({"method": "turn/started", "params": {"threadId": THREAD, "turn": {"id": "turn_1"}}}))
        .await;
    fake.send(approval(1, "exec-2", THREAD)).await;
    assert!(matches!(event(&mut read).await, WbEvent::Approval { .. }));
    fake.send(json!({"method": "serverRequest/resolved", "params": {"threadId": THREAD, "requestId": 1}}))
        .await;
    fake.send(json!({"method": "item/completed", "params": {"threadId": THREAD,
        "item": {"type": "commandExecution", "id": "exec-2", "status": "declined"}}}))
        .await;
    assert!(matches!(event(&mut read).await,
        WbEvent::ApprovalResolved { request_id, decision: Decision::Deny, .. } if request_id == "exec-2#1"));
    assert!(
        perform(&client, approve("exec-2#1", Decision::Allow))
            .await
            .expect("task")
            .is_err()
    );

    // The turn ends before anyone answers: the request expired.
    fake.send(approval(2, "exec-3", THREAD)).await;
    assert!(matches!(event(&mut read).await, WbEvent::Approval { .. }));
    fake.send(json!({"method": "turn/completed", "params": {"threadId": THREAD,
        "turn": {"id": "turn_1", "status": "interrupted"}}}))
        .await;
    fake.send(json!({"method": "serverRequest/resolved", "params": {"threadId": THREAD, "requestId": 2}}))
        .await;
    assert!(matches!(event(&mut read).await,
        WbEvent::ApprovalResolved { request_id, decision: Decision::Expired, .. } if request_id == "exec-3#2"));
}

#[tokio::test]
async fn a_message_starts_or_steers_a_turn_and_stop_interrupts_it() {
    let (client, mut fake, mut read) = connected().await;
    let sent = perform(&client, Action::Send { text: "fix it".into() });
    let start = fake.answer("turn/start", json!({"turn": {"id": "turn_1"}})).await;
    assert_eq!(start["params"]["threadId"], THREAD);
    assert_eq!(start["params"]["input"][0]["text"], "fix it");
    sent.await.expect("task").expect("sent");
    assert!(matches!(event(&mut read).await, WbEvent::Notice { text } if text == "Your phone sent a message."));

    fake.send(json!({"method": "turn/started", "params": {"threadId": THREAD, "turn": {"id": "turn_1"}}}))
        .await;
    // The notification and the next command race; the turn is known once the client has read it.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let steered = perform(
        &client,
        Action::Send {
            text: "and the docs".into(),
        },
    );
    let steer = fake.answer("turn/steer", json!({"turnId": "turn_1"})).await;
    assert_eq!(steer["params"]["expectedTurnId"], "turn_1");
    steered.await.expect("task").expect("steered");

    let stopped = perform(&client, Action::Stop);
    let interrupt = fake.answer("turn/interrupt", json!({})).await;
    assert_eq!(interrupt["params"], json!({"threadId": THREAD, "turnId": "turn_1"}));
    stopped.await.expect("task").expect("stopped");
    fake.send(json!({"method": "turn/completed", "params": {"threadId": THREAD,
        "turn": {"id": "turn_1", "status": "interrupted"}}}))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Nothing to stop is no error, and sends nothing.
    perform(&client, Action::Stop).await.expect("task").expect("idle stop");

    // Codex refuses: the phone's command is rejected.
    let refused = perform(&client, Action::Send { text: "again".into() });
    let request = fake.next().await;
    fake.send(json!({"id": request["id"], "error": {"code": -32600, "message": "busy"}}))
        .await;
    assert!(refused.await.expect("task").is_err());

    drop(fake);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!client.ready(), "a closed app-server takes no command");
}
