//! The shared `wb_*` commands (`nexus_workbench::api`) driven by name with JSON arguments, as a remote host will drive
//! them, against the stand-in `claude` (src/bin/fake-claude.rs). These tests pin the wire format: the argument names
//! the webview sends (contract.ts's `Commands`), the JSON it reads back, and the `wb:*` events' names and payloads.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use nexus_workbench::api::{AgentInfo, Answer, Api, COMMANDS, Machine, Push, Reply, Request};
use nexus_workbench::event::{
    AgentCapabilities, Delta, Project, SessionAgent, StoredEvent, TaskDetail, TaskStatus, Usage, WbEvent,
};
use nexus_workbench::process::Spawn;
use nexus_workbench::store::Store;
use nexus_workbench::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd, Workbench};
use serde_json::{Value, json};

#[derive(Default)]
struct TestHost {
    emitted: Mutex<Vec<Emit>>,
    memory: PathBuf,
}

impl Host for TestHost {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launch, String>> {
        let env: HashMap<String, String> = [("FAKE_CLAUDE_DIR".to_string(), self.memory.to_string_lossy().to_string())]
            .into_iter()
            .collect();
        Box::pin(async move {
            Ok(Launch {
                session_mcp: Default::default(),
                attribution: None,
                spawn: Spawn {
                    program: PathBuf::from(env!("CARGO_BIN_EXE_fake-claude")),
                    args: request.args,
                    env,
                    cwd: request.cwd,
                },
                nexus_session: Some("ns_1".into()),
                local_run: None,
                local_provider: None,
                model: None,
            })
        })
    }

    fn ended(&self, _nexus_session: String, _end: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }

    fn price(&self, _model: &str, usage: &Usage) -> Option<f64> {
        Some(usage.output_tokens as f64 * 0.0001)
    }

    fn emit(&self, emit: Emit) {
        self.emitted.lock().unwrap_or_else(|p| p.into_inner()).push(emit);
    }
}

struct TestMachine {
    home: PathBuf,
    calls: std::sync::atomic::AtomicUsize,
}

impl Machine for TestMachine {
    fn tool_path(&self) -> String {
        std::env::var("PATH").unwrap_or_default()
    }

    fn home(&self) -> Result<PathBuf, String> {
        Ok(self.home.clone())
    }

    fn agent(&self, agent: SessionAgent) -> AgentInfo {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        AgentInfo {
            agent,
            path: (agent == SessionAgent::Claude).then(|| "/fake/nexus-claude".into()),
            version: (agent == SessionAgent::Claude).then(|| "2.1.287 (Claude Code)".into()),
            models: None,
            problem: None,
        }
    }

    fn subscriptions_allowed(&self) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
}

struct Setup {
    host: Arc<TestHost>,
    api: Api,
    machine: Arc<TestMachine>,
    repo: PathBuf,
    root: PathBuf,
}

fn setup(name: &str) -> Setup {
    let root = std::env::temp_dir().join(format!("nexus-workbench-api-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("repo");
    let memory = root.join("memory");
    let _ = std::fs::create_dir_all(&repo);
    let _ = std::fs::create_dir_all(&memory);
    let host = Arc::new(TestHost {
        memory,
        ..TestHost::default()
    });
    let store = Store::open(&root.join("workbench.sqlite3")).unwrap_or_else(|e| panic!("{e}"));
    let config = Config {
        scope: "gw|org".into(),
        shell: nexus_workbench::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(store, host.clone(), config).unwrap_or_else(|e| panic!("{e}"));
    let machine = Arc::new(TestMachine {
        home: root.join("home"),
        calls: Default::default(),
    });
    Setup {
        host,
        api: Api::new(workbench, machine.clone()),
        machine,
        repo,
        root,
    }
}

#[tokio::test]
async fn concurrent_catalog_refreshes_share_discovery_and_leave_sessions_alone() {
    let s = setup("catalog-cache");
    let (a, b) = tokio::join!(s.api.agent_models(), s.api.agent_models());
    assert_eq!(a.expect("first catalog"), b.expect("shared catalog"));
    let overview = s.api.overview().await.expect("overview");
    assert!(overview.projects.is_empty() && overview.threads.is_empty());
    // The overview and both refreshes share one look per agent. Each cached read also checks Nexus's agent.
    assert_eq!(
        s.machine.calls.load(std::sync::atomic::Ordering::SeqCst),
        SessionAgent::ALL.len() + 2
    );
}

/// Codex's discovery ends at once; Antigravity's waits until `gate` opens, as a slow `agy models` does.
struct SlowDiscovery {
    gate: Arc<tokio::sync::Notify>,
}

fn one_model(id: &str) -> Vec<nexus_workbench::agent_models::Model> {
    vec![nexus_workbench::agent_models::Model {
        id: id.into(),
        name: id.into(),
        efforts: Vec::new(),
        resolved_id: None,
    }]
}

impl Machine for SlowDiscovery {
    fn tool_path(&self) -> String {
        String::new()
    }

    fn home(&self) -> Result<PathBuf, String> {
        Err("no home".into())
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
        Box::pin(async { true })
    }

    fn discover_models(
        &self,
        agent: &AgentInfo,
        _path: &str,
    ) -> BoxFuture<'static, Option<Vec<nexus_workbench::agent_models::Model>>> {
        let (agent, gate) = (agent.agent, self.gate.clone());
        Box::pin(async move {
            match agent {
                SessionAgent::Codex => Some(one_model("gpt-9")),
                SessionAgent::Antigravity => {
                    gate.notified().await;
                    Some(one_model("gemini-9-pro"))
                }
                #[cfg(feature = "cursor")]
                SessionAgent::Cursor => None,
                #[cfg(feature = "opencode")]
                SessionAgent::Opencode => None,
                SessionAgent::Claude | SessionAgent::Nexus => None,
            }
        })
    }
}

fn models_of(agents: &[AgentInfo], agent: SessionAgent) -> Option<Vec<String>> {
    agents
        .iter()
        .find(|one| one.agent == agent)
        .and_then(|one| one.models.as_ref())
        .map(|models| models.iter().map(|model| model.id.clone()).collect())
}

#[tokio::test]
async fn slow_discovery_does_not_hold_back_fast_agents_models() {
    let s = setup("catalog-each");
    let gate = Arc::new(tokio::sync::Notify::new());
    let api = Api::new(
        s.api.workbench().clone(),
        Arc::new(SlowDiscovery { gate: gate.clone() }),
    );
    let mut lists: Vec<Vec<AgentInfo>> = Vec::new();
    // Antigravity's discovery ends only once Codex's models have been handed over: waiting for every agent first
    // would never end, and the timeout fails the test.
    let all = tokio::time::timeout(
        Duration::from_secs(10),
        api.agent_models_each(|agents| {
            if models_of(&agents, SessionAgent::Codex).is_some() {
                gate.notify_one();
            }
            lists.push(agents);
        }),
    )
    .await
    .expect("a slow discovery held back the others")
    .expect("catalog");
    let first = lists
        .iter()
        .find(|agents| models_of(agents, SessionAgent::Codex).is_some())
        .expect("Codex's models");
    assert_eq!(models_of(first, SessionAgent::Antigravity), None);
    // One list per agent's discovery, in the same order each time; the last is the whole catalog.
    assert_eq!(lists.len(), SessionAgent::ALL.len());
    assert_eq!(lists.last(), Some(&all));
    assert_eq!(
        models_of(&all, SessionAgent::Antigravity),
        Some(vec!["gemini-9-pro".to_string()])
    );
    assert_eq!(models_of(&all, SessionAgent::Codex), Some(vec!["gpt-9".to_string()]));
    // The cache answers the next read with one list, and `agent_models` with the same catalog.
    let mut cached = 0;
    let again = api.agent_models_each(|_| cached += 1).await.expect("cached");
    assert_eq!((cached, &again), (1, &all));
    assert_eq!(api.agent_models().await.expect("cached"), all);
}

async fn call(api: &Api, name: &str, args: Value) -> Value {
    api.call(name, args).await.unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// Waits (up to ten seconds) until the session's events, read through `wb_thread_events`, satisfy `done`.
async fn wait_for(api: &Api, thread_id: &str, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    for _ in 0..400 {
        let events = call(api, "wb_thread_events", json!({ "threadId": thread_id, "afterSeq": 0 })).await;
        let events = events.as_array().cloned().unwrap_or_default();
        if done(&events) {
            return events;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out");
}

fn kinds(events: &[Value]) -> Vec<&str> {
    events.iter().filter_map(|e| e["event"]["kind"].as_str()).collect()
}

fn turn_ends(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .filter(|e| e["event"]["kind"] == "turnEnd")
        .filter_map(|e| e["event"]["status"].as_str())
        .collect()
}

#[tokio::test]
async fn file_attachments_reach_the_sessions_computer_through_both_commands() {
    let s = setup("files");
    let project = call(&s.api, "wb_project_add", json!({ "path": s.repo })).await;
    let created = call(
        &s.api,
        "wb_thread_create_files",
        json!({
            "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "Read this", "images": [], "worktree": null,
            "files": [{ "name": "notes.bin", "data": "AP8q" }]
        }),
    )
    .await;
    let id = created["id"].as_str().unwrap_or_default();
    wait_for(&s.api, id, |events| turn_ends(events).len() == 1).await;
    call(
        &s.api,
        "wb_send_files",
        json!({ "threadId": id, "text": "Read this too", "images": [],
        "files": [{ "name": "notes.txt", "data": "aGk=" }] }),
    )
    .await;
    let events = wait_for(&s.api, id, |events| turn_ends(events).len() == 2).await;
    let users: Vec<_> = events.iter().filter(|event| event["event"]["kind"] == "user").collect();
    assert_eq!(users.len(), 2);
    assert!(
        users[0]["event"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("notes.bin")
    );
    assert!(
        users[1]["event"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("notes.txt")
    );
    // Both messages' files sit in the session's own folder, one folder a message.
    let root = s.root.join("home/.nexus/attachments");
    let session = root.join(id);
    let mut contents = Vec::new();
    for folder in std::fs::read_dir(&session).unwrap_or_else(|e| panic!("{e}")).flatten() {
        for file in std::fs::read_dir(folder.path())
            .unwrap_or_else(|e| panic!("{e}"))
            .flatten()
        {
            contents.push(std::fs::read(file.path()).unwrap_or_default());
        }
    }
    contents.sort();
    assert_eq!(contents, [vec![0, 255, 42], b"hi".to_vec()]);
    assert_eq!(std::fs::read_dir(&root).map(Iterator::count).unwrap_or_default(), 1);
    // Deleting the session deletes its files.
    call(&s.api, "wb_thread_delete", json!({ "threadId": id })).await;
    assert!(!session.exists());
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

/// A file sent in pieces through the shared commands lands in the session's folder, whole, and a cancelled one leaves nothing.
#[tokio::test]
async fn a_file_sent_in_pieces_is_saved_whole_and_a_cancelled_one_leaves_nothing() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let s = setup("pieces");
    let project = call(&s.api, "wb_project_add", json!({ "path": s.repo })).await;
    let whole: Vec<u8> = (0..9_000u32).map(|n| (n % 253) as u8).collect();
    // The pieces are for the session the creation request makes, so only its first message takes them.
    let request = uuid::Uuid::new_v4().to_string();
    let thread = nexus_workbench::creation_thread_id(&request).unwrap_or_default();
    let piece = |id: &str, from: usize, to: usize| json!({ "uploadId": id, "size": whole.len(), "offset": from, "data": STANDARD.encode(&whole[from..to]), "threadId": thread });
    let first = call(&s.api, "wb_attachment_chunk", piece("u1", 0, 4_000)).await;
    assert_eq!(first, json!({ "received": 4_000, "done": false }));
    call(&s.api, "wb_attachment_chunk", piece("u1", 4_000, 8_000)).await;
    let last = call(&s.api, "wb_attachment_chunk", piece("u1", 8_000, 9_000)).await;
    assert_eq!(last, json!({ "received": 9_000, "done": true }));
    // Begun and cancelled: no file stays.
    call(&s.api, "wb_attachment_chunk", piece("u2", 0, 4_000)).await;
    call(
        &s.api,
        "wb_attachment_cancel",
        json!({ "uploadId": "u2", "threadId": thread }),
    )
    .await;
    let created = call(
        &s.api,
        "wb_thread_create_files",
        json!({
            "requestId": request, "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "Watch this", "images": [], "worktree": null,
            "files": [{ "name": "clip.bin", "upload": "u1" }]
        }),
    )
    .await;
    let id = created["id"].as_str().unwrap_or_default();
    wait_for(&s.api, id, |events| turn_ends(events).len() == 1).await;
    let root = s.root.join("home/.nexus/attachments");
    let saved: Vec<_> = std::fs::read_dir(root.join(id))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|batch| std::fs::read_dir(batch.path()).into_iter().flatten().flatten())
        .collect();
    assert_eq!(saved.len(), 1);
    assert_eq!(std::fs::read(saved[0].path()).unwrap_or_default(), whole);
    let waiting: Vec<_> = std::fs::read_dir(root.join(".uploads"))
        .into_iter()
        .flatten()
        .flatten()
        .collect();
    assert!(waiting.is_empty(), "{waiting:?}");
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

/// The file viewer and project search read the session's folder over the wire, and nothing outside it.
#[tokio::test]
async fn a_sessions_files_are_read_and_searched_inside_its_folder_only() {
    let s = setup("project-files");
    std::fs::create_dir_all(s.repo.join("src")).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(s.repo.join("src/main.rs"), "fn main() {\n    println!(\"Hello\");\n}\n")
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(s.root.join("outside.txt"), "hello from outside\n").unwrap_or_else(|e| panic!("{e}"));
    let project = call(&s.api, "wb_project_add", json!({ "path": s.repo })).await;
    let created = call(
        &s.api,
        "wb_thread_create",
        json!({
            "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "hi", "images": [], "worktree": null,
        }),
    )
    .await;
    let id = created["id"].as_str().unwrap_or_default();
    wait_for(&s.api, id, |events| turn_ends(events).len() == 1).await;

    let file = call(&s.api, "wb_file_read", json!({ "threadId": id, "path": "src/main.rs" })).await;
    assert_eq!(
        file,
        json!({ "path": "src/main.rs", "text": "fn main() {\n    println!(\"Hello\");\n}\n", "binary": false,
                "truncated": false, "size": 37 })
    );
    let refused = s
        .api
        .call("wb_file_read", json!({ "threadId": id, "path": "../outside.txt" }))
        .await;
    assert!(refused.is_err());
    let preview = call(
        &s.api,
        "wb_file_preview",
        json!({ "threadId": id, "path": "../outside.txt" }),
    )
    .await;
    assert_eq!(preview["text"], "hello from outside\n");
    assert_eq!(preview["outsideProject"], true);
    assert_eq!(preview["status"], "ready");
    assert!(
        s.api
            .call(
                "wb_file_preview",
                json!({ "threadId": "missing", "path": "../outside.txt" })
            )
            .await
            .is_err()
    );

    let found = call(&s.api, "wb_file_search", json!({ "threadId": id, "query": "hello" })).await;
    assert_eq!(
        found,
        json!({ "matches": [{ "path": "src/main.rs", "line": 2, "column": 15, "start": 1,
                              "text": "    println!(\"Hello\");" }], "truncated": false })
    );
    let exact = call(
        &s.api,
        "wb_file_search",
        json!({ "threadId": id, "query": "hello", "caseSensitive": true }),
    )
    .await;
    assert_eq!(exact["matches"], json!([]));
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[tokio::test]
async fn a_session_that_never_starts_leaves_no_attachments_folder() {
    let s = setup("files-failed");
    let project = call(&s.api, "wb_project_add", json!({ "path": s.repo })).await;
    // Files are saved before the arguments are checked: an unknown agent fails after them.
    let created = s
        .api
        .call(
            "wb_thread_create_files",
            json!({
                "projectId": project["id"], "agent": "nope", "model": null, "permissionMode": "default",
                "effort": null, "via": "nexus", "text": "Read this", "images": [], "worktree": null,
                "files": [{ "name": "notes.txt", "data": "aGk=" }]
            }),
        )
        .await;
    assert!(created.is_err());
    let root = s.root.join("home/.nexus/attachments");
    assert_eq!(std::fs::read_dir(&root).map(Iterator::count).unwrap_or_default(), 0);
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

/// A mentioned session reaches the agent as a short block after the message (its title and last exchanges), on the
/// first message and on later ones, while the log and the title keep the message as typed.
#[tokio::test]
async fn mentioned_sessions_reach_the_agent_but_not_the_log() {
    let s = setup("mentions");
    let project = call(&s.api, "wb_project_add", json!({ "path": s.repo })).await;
    let create = |text: &str| {
        json!({
            "projectId": project["id"], "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": text, "images": [], "worktree": null
        })
    };
    let first = call(&s.api, "wb_thread_create", create("Fix the login")).await;
    let first_id = first["id"].as_str().unwrap_or_default().to_string();
    wait_for(&s.api, &first_id, |events| turn_ends(events).len() == 1).await;

    let typed = format!("recall Go on from [Fix the login](session:{first_id}) and [Gone](session:t_gone)");
    let second = call(&s.api, "wb_thread_create", create(&typed)).await;
    assert_eq!(second["title"], "recall Go on from Fix the login and Gone");
    let id = second["id"].as_str().unwrap_or_default().to_string();
    let events = wait_for(&s.api, &id, |events| turn_ends(events).len() == 1).await;
    let heard = |events: &[Value]| {
        events
            .iter()
            .filter(|e| e["event"]["kind"] == "text")
            .filter_map(|e| e["event"]["text"].as_str())
            .find(|text| text.contains("Heard:"))
            .unwrap_or_default()
            .to_string()
    };
    let users = |events: &[Value]| {
        events
            .iter()
            .filter(|e| e["event"]["kind"] == "user")
            .filter_map(|e| e["event"]["text"].as_str().map(String::from))
            .collect::<Vec<_>>()
    };
    let said = heard(&events);
    assert!(said.contains(&format!("{typed}\n\n<mentions>\n")), "{said}");
    assert!(
        said.contains(&format!("<session id=\"{first_id}\">\nTitle: Fix the login\n")),
        "{said}"
    );
    assert!(said.contains("Person: Fix the login\nAgent: "), "{said}");
    assert!(said.contains("<session id=\"t_gone\" unavailable=\"true\"/>"), "{said}");
    assert_eq!(users(&events), std::slice::from_ref(&typed));

    let again = format!("recall And [it](session:{first_id}) again");
    call(
        &s.api,
        "wb_send",
        json!({ "threadId": id, "text": again, "images": [] }),
    )
    .await;
    let events = wait_for(&s.api, &id, |events| turn_ends(events).len() == 2).await;
    assert_eq!(users(&events), [typed, again]);
    assert!(
        events.iter().any(|e| e["event"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("Heard: recall And") && t.contains("Title: Fix the login"))),
        "{events:?}"
    );
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

/// Create, approve, send, interrupt and the session's events, all through `Api::call`.
#[tokio::test]
async fn drives_a_session_through_the_shared_commands() {
    let s = setup("session");
    let project = call(&s.api, "wb_project_add", json!({ "path": s.repo })).await;
    assert_eq!(keys(&project), ["id", "name", "path", "branch", "addedAt"]);
    let project_id = project["id"].as_str().unwrap_or_default().to_string();

    let created = call(
        &s.api,
        "wb_thread_create",
        json!({
            "projectId": project_id, "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "use a tool", "images": [], "worktree": null,
        }),
    )
    .await;
    // ThreadSummary, as contract.ts reads it (`pending` only when something is pending, `pinned` only when pinned,
    // `orchestration` only when on: this computer's switch is on by default).
    assert_eq!(
        keys(&created),
        [
            "id",
            "projectId",
            "title",
            "agent",
            "model",
            "permissionMode",
            "effort",
            "via",
            "account",
            "status",
            "costUsd",
            "nexusSessions",
            "contextTokens",
            "contextWindow",
            "createdAt",
            "updatedAt",
            "listedAt",
            "actedAt",
            "archived",
            "worktree",
            "orchestration",
        ]
    );
    assert_eq!(
        (&created["agent"], &created["permissionMode"], &created["via"]),
        (&json!("claude"), &json!("default"), &json!("nexus"))
    );
    let id = created["id"].as_str().unwrap_or_default().to_string();

    // The approval, then the person's answer.
    let events = wait_for(&s.api, &id, |e| kinds(e).contains(&"approval")).await;
    let approval = events
        .iter()
        .find(|e| e["event"]["kind"] == "approval")
        .cloned()
        .unwrap_or_default();
    assert_eq!(keys(&approval), ["threadId", "seq", "at", "event"]);
    assert_eq!(
        keys(&approval["event"]),
        [
            "kind",
            "requestId",
            "toolUseId",
            "tool",
            "input",
            "description",
            "suggestions"
        ]
    );
    let answered = call(
        &s.api,
        "wb_approve",
        json!({
            "threadId": id, "requestId": approval["event"]["requestId"], "decision": "allow",
            "message": null, "suggestion": null, "answers": null,
        }),
    )
    .await;
    assert_eq!(answered, Value::Null);
    let events = wait_for(&s.api, &id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(turn_ends(&events), ["completed"]);
    assert!(kinds(&events).contains(&"approvalResolved"));

    // A message that works until interrupted.
    let sent = call(
        &s.api,
        "wb_send",
        json!({ "threadId": id, "text": "slow work", "images": [] }),
    )
    .await;
    assert_eq!(sent, Value::Null);
    let last = events.last().and_then(|e| e["seq"].as_i64()).unwrap_or_default();
    wait_for(&s.api, &id, |e| {
        e.iter()
            .any(|e| e["seq"].as_i64() > Some(last) && e["event"]["kind"] == "text")
    })
    .await;
    assert_eq!(
        call(&s.api, "wb_interrupt", json!({ "threadId": id })).await,
        Value::Null
    );
    let events = wait_for(&s.api, &id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), ["completed", "interrupted"]);

    // `afterSeq` gives only what came after.
    let later = call(&s.api, "wb_thread_events", json!({ "threadId": id, "afterSeq": last })).await;
    let later = later.as_array().cloned().unwrap_or_default();
    let seqs: Vec<i64> = later.iter().filter_map(|e| e["seq"].as_i64()).collect();
    assert_eq!(seqs.first(), Some(&(last + 1)));
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");

    // What was pushed live is what the log gives back: same names, same payloads.
    let pushed: Vec<Value> = s
        .host
        .emitted
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|emit| serde_json::to_value(Push::new(emit)).unwrap_or_default())
        .collect();
    let stored: Vec<&Value> = pushed
        .iter()
        .filter(|p| p["event"] == "wb:event")
        .map(|p| &p["payload"])
        .collect();
    for event in &events {
        assert!(stored.contains(&event), "not pushed: {event}");
    }
    assert!(
        pushed
            .iter()
            .any(|p| p["event"] == "wb:thread" && p["payload"]["id"] == json!(id))
    );
    assert!(
        pushed
            .iter()
            .any(|p| p["event"] == "wb:delta" && p["payload"]["threadId"] == json!(id))
    );
    assert!(
        pushed
            .iter()
            .any(|p| p["event"] == "wb:projects" && p["payload"].is_array())
    );

    // `restart: true` ends the agent and says so in the log.
    assert_eq!(
        call(&s.api, "wb_stop", json!({ "threadId": id, "restart": true })).await,
        Value::Null
    );
    wait_for(&s.api, &id, |events| {
        events
            .iter()
            .any(|e| e.to_string().contains(nexus_workbench::RESTARTED))
    })
    .await;
    assert_eq!(call(&s.api, "wb_stop", json!({ "threadId": id })).await, Value::Null);
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn overview_answers_as_the_code_page_reads_it() {
    let s = setup("overview");
    let overview = call(&s.api, "wb_overview", json!({})).await;
    let agents = [
        json!({ "agent": "claude", "path": "/fake/nexus-claude", "version": "2.1.287 (Claude Code)" }),
        json!({ "agent": "codex", "path": null, "version": null }),
        json!({ "agent": "nexus", "path": null, "version": null }),
        json!({ "agent": "antigravity", "path": null, "version": null }),
        #[cfg(feature = "cursor")]
        json!({ "agent": "cursor", "path": null, "version": null }),
        #[cfg(feature = "opencode")]
        json!({ "agent": "opencode", "path": null, "version": null }),
    ];
    assert_eq!(
        overview,
        json!({
            "projects": [],
            "threads": [],
            "agents": agents,
            "persistent": true,
            "agentSubscriptions": true,
            "executionIdentities": [],
            "features": ["orchestration", "session-commands", "session-connection", "orchestration-controls", "session-start-choice", "project-clone", "project-dirs", "diff-sources", "project-files", "file-previews", "file-preview-pieces", "inline-media", "project-pin-archive", "side-chats", "move-to-worktree"],
            // The fixture's Sandbox switch is off, and no agy has run.
            "runFacts": { "sandbox": false, "autoOffered": null },
        })
    );
    // No arguments at all reads as `{}`.
    let default = call(&s.api, "wb_project_default", Value::Null).await;
    assert_eq!(default["name"], "nexus");
    assert!(s.root.join("home/nexus").is_dir());
    let _ = std::fs::remove_dir_all(&s.root);
}

#[test]
fn pushes_name_each_event_and_carry_its_payload() {
    let event = Emit::Event(StoredEvent {
        thread_id: "t1".into(),
        seq: 3,
        at: 1700,
        event: WbEvent::Notice { text: "Hi".into() },
    });
    assert_eq!(
        serde_json::to_value(Push::new(&event)).unwrap_or_default(),
        json!({ "event": "wb:event", "payload": { "threadId": "t1", "seq": 3, "at": 1700, "event": { "kind": "notice", "text": "Hi" } } })
    );
    let delta = Emit::Delta(Delta {
        thread_id: "t1".into(),
        id: "m1".into(),
        text: "Hel".into(),
        parent: None,
        offset: None,
    });
    assert_eq!(
        serde_json::to_value(Push::new(&delta)).unwrap_or_default(),
        json!({ "event": "wb:delta", "payload": { "threadId": "t1", "id": "m1", "text": "Hel", "parent": null } })
    );
    let projects = Emit::Projects(vec![Project {
        id: "p1".into(),
        name: "repo".into(),
        path: "/r".into(),
        branch: None,
        remotes: vec![],
        host: Default::default(),
        added_at: 5,
        orchestration: false,
        pinned: false,
        archived: false,
    }]);
    assert_eq!(
        serde_json::to_value(Push::new(&projects)).unwrap_or_default(),
        json!({ "event": "wb:projects", "payload": [{ "id": "p1", "name": "repo", "path": "/r", "branch": null, "addedAt": 5 }] })
    );
    // An emit alone is its payload: what a push carries under `Emit::name`.
    assert_eq!(
        serde_json::to_value(&delta).unwrap_or_default(),
        json!({ "threadId": "t1", "id": "m1", "text": "Hel", "parent": null })
    );
}

#[tokio::test]
async fn requests_and_replies_keep_their_shape() {
    let s = setup("request");
    let request: Request =
        serde_json::from_value(json!({ "id": 7, "call": "wb_search", "args": { "query": "anything" } }))
            .unwrap_or_else(|e| panic!("{e}"));
    let reply = s.api.answer(request).await;
    assert_eq!(
        serde_json::to_value(&reply).unwrap_or_default(),
        json!({ "id": 7, "ok": [] })
    );

    let unknown: Request =
        serde_json::from_value(json!({ "id": 8, "call": "wb_pick_folder" })).unwrap_or_else(|e| panic!("{e}"));
    let reply = s.api.answer(unknown).await;
    assert_eq!(
        serde_json::to_value(&reply).unwrap_or_default(),
        json!({ "id": 8, "error": "Unknown command wb_pick_folder." })
    );
    let back: Reply = serde_json::from_value(json!({ "id": 9, "ok": null })).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(back.answer, Answer::Ok(Value::Null));

    // Arguments are read by the webview's (camelCase) names.
    let wrong = s
        .api
        .call("wb_send", json!({ "thread_id": "t", "text": "x", "images": [] }))
        .await;
    assert_eq!(
        wrong,
        Err("Invalid arguments for wb_send: missing field `threadId`.".into())
    );
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A file of this crate's folder: a source, or `contract.ts`, the wire contract.
fn read(path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The `Commands` of contract.ts, by name.
fn contract_commands() -> Vec<String> {
    let text = read("contract.ts");
    let start = text
        .find("export interface Commands {")
        .unwrap_or_else(|| panic!("no Commands in contract.ts"));
    let body = &text[start..];
    let body = &body[..body.find("\n}").unwrap_or(body.len())];
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("wb_").map(|rest| (line, rest)))
        .filter_map(|(_, rest)| rest.split_once(':').map(|(name, _)| format!("wb_{name}")))
        .collect()
}

/// What the app answers itself: a window on this computer (pickers, editors, notifications) or the app's own
/// settings and keychain. A host answers them "Unknown command" until it gets its own.
const APP_ONLY: &[&str] = &[
    "wb_pull_watch",
    "wb_pull_watches",
    "wb_pull_watch_send",
    "wb_pull_watch_auto",
    "wb_pull_watch_agent_off",
    "wb_pull_page",
    "wb_pull_reply",
    "wb_pull_resolve",
    "wb_pull_merge",
    "wb_pull_auto_off",
    "wb_secret_requests",
    "wb_secret_answer",
    "wb_browser",
    "wb_browser_requests",
    "wb_browser_decline",
    "wb_preview_probe",
    "wb_nexus_install",
    "wb_pick_folder",
    "wb_editors",
    "wb_open_in",
    "wb_open_file",
    "wb_open_output",
    "wb_notifications",
    "wb_notifications_set",
    "wb_hooks",
    "wb_hooks_set",
    "wb_path_notifications",
    "wb_path_notifications_set",
    "wb_viewing",
    "wb_mcp_servers",
    "wb_mcp_servers_set",
    "wb_plans",
    "wb_account_add",
    "wb_account_remove",
    "wb_account_use",
    "wb_rotation",
    "wb_rotation_set",
    "wb_worktree_roots",
    "wb_worktree_roots_set",
    "wb_worktree_cleanup_settings",
    "wb_worktree_cleanup_set",
    "wb_worktree_cleanup_run",
    "wb_branch_prefix_set",
    "wb_draft_write",
];

#[test]
fn every_contract_command_is_shared_or_named_as_the_apps() {
    let contract = contract_commands();
    assert!(contract.len() > 40, "{contract:?}");
    for name in &contract {
        assert!(
            COMMANDS.contains(&name.as_str()) != APP_ONLY.contains(&name.as_str()),
            "{name}: shared or the app's, not both or neither"
        );
    }
    for name in COMMANDS.iter().chain(APP_ONLY) {
        assert!(contract.iter().any(|c| c == name), "{name} is not in contract.ts");
    }
}

/// contract.ts without its comments.
fn contract_types() -> String {
    let mut text = read("contract.ts");
    while let Some(start) = text.find("/*") {
        let end = text[start..].find("*/").map_or(text.len(), |end| start + end + 2);
        text.replace_range(start..end, "");
    }
    text
}

/// The names of the fields a TypeScript object type's body declares, at its top level.
fn field_names(body: &str) -> BTreeSet<String> {
    body.split([';', '\n'])
        .filter_map(|part| part.split_once(':'))
        .map(|(name, _)| name.trim().trim_end_matches('?'))
        .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .map(String::from)
        .collect()
}

/// The fields of `WbEvent`'s member of this kind.
fn member_fields(types: &str, kind: &str) -> BTreeSet<String> {
    let events = &types[types.find("export type WbEvent =").expect("no WbEvent in contract.ts")..];
    let at = events
        .find(&format!("kind: \"{kind}\";"))
        .unwrap_or_else(|| panic!("no {kind} in WbEvent"));
    let open = events[..at].rfind('{').expect("a member opens with {");
    let mut depth = 0;
    let close = events[open..]
        .char_indices()
        .find_map(|(i, c)| {
            depth += match c {
                '{' => 1,
                '}' => -1,
                _ => 0,
            };
            (depth == 0).then_some(open + i)
        })
        .expect("a member closes with }");
    field_names(&events[open + 1..close])
}

/// The fields a JSON object has.
fn json_fields(value: &Value) -> BTreeSet<String> {
    value.as_object().expect("an object").keys().cloned().collect()
}

/// Snapshot and logical-message fields use the same camelCase/null wire contract as TypeScript.
#[test]
fn stream_snapshot_fields_match_the_contract() {
    use nexus_workbench::stream_snapshot::{Message, Snapshot};

    let types = contract_types();
    let mut snapshot = Snapshot {
        thread_id: "side".into(),
        summary: serde_json::from_value(json!({
            "id": "side", "projectId": "project", "title": "Side", "agent": "claude",
            "model": null, "permissionMode": "default", "via": "nexus", "status": "idle",
            "costUsd": 0.0, "nexusSessions": [], "createdAt": 100, "updatedAt": 100,
            "archived": false, "sideChat": true, "sourceThreadId": "parent"
        }))
        .expect("valid snapshot contract summary"),
        incarnation: Some("opaque-run".into()),
        revision: 0,
        through_seq: 0,
        events: vec![],
        more: false,
        messages: vec![Message {
            id: "main".into(),
            parent: None,
            text: "partial".into(),
        }],
    };
    let value = serde_json::to_value(&snapshot).expect("snapshot contract serializes");
    for (name, object) in [
        ("ThreadStreamSnapshot", &value),
        ("ThreadStreamMessage", &value["messages"][0]),
    ] {
        let start = types
            .find(&format!("export interface {name} {{"))
            .expect("snapshot interface");
        let body = &types[start..];
        let body = &body[body.find('{').expect("interface opens") + 1..body.find("\n}").expect("interface closes")];
        assert_eq!(json_fields(object), field_names(body), "{name}");
    }
    assert_eq!(value["incarnation"], "opaque-run");
    assert!(value["messages"][0]["parent"].is_null());
    snapshot.messages[0].parent = Some("child-call".into());
    let value = serde_json::to_value(&snapshot).expect("live child snapshot serializes");
    assert_eq!(value["incarnation"], "opaque-run");
    assert_eq!(value["messages"][0]["parent"], "child-call");
    snapshot.incarnation = None;
    snapshot.messages.clear();
    let value = serde_json::to_value(&snapshot).expect("inactive snapshot serializes");
    assert!(value["incarnation"].is_null());
    assert_eq!(value["messages"], json!([]));
}

/// Every event kind the core writes is a member of contract.ts's `WbEvent`, and the other way round.
#[test]
fn every_event_kind_is_in_the_contract() {
    let source = read("src/event.rs");
    let start = source.find("pub enum WbEvent {").expect("no WbEvent in event.rs");
    let body = &source[start..];
    let body = &body[..body.find("\n}").expect("WbEvent ends")];
    let core: BTreeSet<String> = body
        .lines()
        .filter_map(|line| line.strip_prefix("    "))
        .filter(|line| line.starts_with(|c: char| c.is_ascii_uppercase()))
        .map(|line| {
            let name: String = line.chars().take_while(char::is_ascii_alphanumeric).collect();
            name[..1].to_lowercase() + &name[1..]
        })
        .collect();
    let types = contract_types();
    let events = &types[types.find("export type WbEvent =").expect("no WbEvent in contract.ts")..];
    let end = events[1..].find("\nexport ").map_or(events.len(), |end| end + 1);
    let contract: BTreeSet<String> = events[..end]
        .split("kind: \"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(kind, _)| kind.to_string()))
        .collect();
    assert!(core.len() > 20, "{core:?}");
    assert_eq!(core, contract);
}

/// The fields the agent tree added to events (`parent`, the task's `detail`) are the ones contract.ts declares.
#[test]
fn every_agent_tree_field_is_in_the_contract() {
    let types = contract_types();
    let detail = TaskDetail {
        parent_task_id: Some("a1".into()),
        depth: Some(2),
        model: Some("m".into()),
        name: Some("Ada".into()),
        activity: Some("Reading".into()),
        backgrounded: Some(true),
        run_id: Some("run1".into()),
        progress: Some(true),
        reason: Some("process_ended".into()),
    };
    let start = types
        .find("export interface TaskDetail {")
        .expect("no TaskDetail in contract.ts");
    let body = &types[start..];
    let body = &body[body.find('{').unwrap_or(0) + 1..body.find("\n}").unwrap_or(body.len())];
    let detail_json = serde_json::to_value(&detail).expect("a detail serializes");
    assert_eq!(json_fields(&detail_json), field_names(body), "TaskDetail");
    let parent = Some("t1".to_string());
    for event in [
        WbEvent::ToolResult {
            images: vec![nexus_workbench::event::Image {
                media_type: "image/png".into(),
                data: "aGk=".into(),
                name: None,
            }],
            id: "t2".into(),
            output: String::new(),
            is_error: false,
            truncated: false,
            parent: parent.clone(),
        },
        WbEvent::Approval {
            request_id: "r".into(),
            tool_use_id: None,
            tool: "Bash".into(),
            input: json!({}),
            description: None,
            suggestions: vec![],
            parent: parent.clone(),
        },
        WbEvent::Usage {
            model: "m".into(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: 0.0,
            parent: parent.clone(),
        },
        WbEvent::Task {
            task_id: "a2".into(),
            tool_use_id: None,
            task_type: None,
            agent_type: None,
            status: TaskStatus::Running,
            description: String::new(),
            tool_count: None,
            total_tokens: None,
            duration_ms: None,
            detail: Some(Box::new(detail.clone())),
        },
    ] {
        let value = serde_json::to_value(&event).expect("an event serializes");
        let kind = value["kind"].as_str().expect("a kind").to_string();
        assert_eq!(json_fields(&value), member_fields(&types, &kind), "{kind}");
    }
    // What the running agent accepts, on the session's summary.
    let interface = |name: &str| {
        let start = types
            .find(&format!("export interface {name} {{"))
            .unwrap_or_else(|| panic!("no {name} in contract.ts"));
        let body = &types[start..];
        field_names(&body[body.find('{').unwrap_or(0) + 1..body.find("\n}").unwrap_or(body.len())])
    };
    let capabilities = AgentCapabilities {
        connection: Some(Box::default()),
        commands: Some(vec!["compact".into()]),
        compact: true,
        version: Some("2.1.292".into()),
        stop: vec!["agent".into()],
        background: vec!["shell".into()],
        output: vec!["shell".into()],
        message: vec!["agent".into()],
        interrupt_spares_agents: true,
    };
    let capabilities = serde_json::to_value(&capabilities).expect("capabilities serialize");
    assert_eq!(
        json_fields(&capabilities),
        interface("AgentCapabilities"),
        "AgentCapabilities"
    );
    assert!(
        interface("ThreadSummary").contains("capabilities"),
        "ThreadSummary.capabilities"
    );
    assert!(
        interface("ThreadSummary").contains("atDepthLimit"),
        "ThreadSummary.atDepthLimit"
    );
}

/// Each shared command takes the arguments contract.ts gives it: a call with them fails, if at all, for what it asked
/// (no such session), never on the arguments.
#[tokio::test]
async fn every_shared_command_takes_the_contracts_arguments() {
    let s = setup("arguments");
    let t = json!("no-such-session");
    let p = json!("no-such-project");
    let calls = [
        ("wb_computer_list", json!({})),
        ("wb_computer_info", json!({})),
        ("wb_computer_name_set", json!({ "name": "Test computer" })),
        (
            "wb_computer_join_begin",
            json!({ "link": "nexus://connect#invite=example" }),
        ),
        ("wb_computer_join_poll", json!({ "inviteId": "invite" })),
        ("wb_computer_join_allow", json!({ "inviteId": "invite" })),
        (
            "wb_computer_request",
            json!({ "computerId": "computer", "requestId": "request", "operation": { "kind": "describeProjects" } }),
        ),
        (
            "wb_computer_request_get",
            json!({ "computerId": "computer", "requestId": "request" }),
        ),
        ("wb_local_history", json!({ "limit": 10 })),
        ("wb_synced_thread_page", json!({ "threadId": t, "limit": 10 })),
        (
            "wb_synced_command",
            json!({ "threadId": t, "requestId": "request", "action": { "kind": "stop" } }),
        ),
        (
            "wb_synced_command_get",
            json!({ "threadId": t, "requestId": "request" }),
        ),
        ("wb_phone_invitation_begin", json!({})),
        ("wb_phone_invitation_poll", json!({ "inviteId": "invite" })),
        (
            "wb_phone_invitation_approve",
            json!({ "inviteId": "invite", "deviceId": "device", "code": "code" }),
        ),
        ("wb_phone_invitation_cancel", json!({ "inviteId": "invite" })),
        ("wb_code_control", json!({})),
        ("wb_code_control_set", json!({ "enabled": false })),
        ("wb_overview", json!({})),
        ("wb_threads", json!({})),
        ("wb_agent_models", json!({})),
        ("wb_project_add", json!({ "path": s.repo })),
        (
            "wb_project_clone",
            json!({ "source": "not a repository", "parent": "~/code", "requestId": "r" }),
        ),
        ("wb_project_clone_progress", json!({ "requestId": "r" })),
        ("wb_dirs", json!({ "path": s.repo })),
        ("wb_cloud_setup", json!({})),
        ("wb_cloud_setup_retry", json!({})),
        ("wb_project_default", json!({})),
        ("wb_project_remove", json!({ "projectId": p })),
        ("wb_project_setup", json!({ "projectId": p })),
        (
            "wb_project_update",
            json!({ "projectId": p, "pinned": true, "archived": false }),
        ),
        ("wb_session_start_preference", json!({ "projectId": p })),
        ("wb_session_start_overrides", json!({ "clear": false })),
        ("wb_project_orchestration_set", json!({ "projectId": p, "on": true })),
        (
            "wb_session_start_preference_set",
            json!({ "projectId": p, "preference": "ask" }),
        ),
        (
            "wb_session_start_answer",
            json!({ "threadId": t, "requestId": "r", "choice": "here", "remember": false }),
        ),
        ("wb_project_setup_set", json!({ "projectId": p, "script": null })),
        ("wb_project_scripts", json!({ "projectId": p })),
        (
            "wb_project_scripts_set",
            json!({ "projectId": p, "scripts": [{ "id": "dev", "name": "Dev", "command": "npm run dev", "shortcut": null }] }),
        ),
        ("wb_project_defaults", json!({ "projectId": p })),
        ("wb_project_claude_trust", json!({ "projectId": p })),
        (
            "wb_project_claude_trust_set",
            json!({ "projectId": p, "trusted": true }),
        ),
        (
            "wb_project_defaults_set",
            json!({ "projectId": p, "defaults": { "agent": "codex", "model": "gpt-5", "mode": "plan", "workspace": "worktree" } }),
        ),
        ("wb_schedules", json!({})),
        (
            "wb_schedule_save",
            json!({ "scheduleId": null, "schedule": { "projectId": p, "title": null, "prompt": "Triage", "agent": "claude",
                    "model": null, "permissionMode": "plan", "effort": null, "via": "nexus", "worktree": true,
                    "when": { "kind": "weekly", "days": [1, 3], "minute": 540 }, "enabled": true,
                    "maxRunsPerDay": 4, "catchUp": false } }),
        ),
        ("wb_schedule_delete", json!({ "scheduleId": "s_none" })),
        ("wb_schedule_run", json!({ "scheduleId": "s_none" })),
        ("wb_schedule_webhook", json!({ "scheduleId": "s_none", "on": false })),
        (
            "wb_thread_create",
            json!({ "projectId": p, "agent": "claude", "model": null, "permissionMode": "plan", "effort": "high",
                    "via": "subscription", "text": "hi", "images": [{ "mediaType": "image/png", "data": "AA==", "name": null }],
                    "worktree": { "branch": null, "base": null } }),
        ),
        ("wb_pull_requests", json!({ "projectId": p })),
        ("wb_worktree_bases", json!({ "projectId": p })),
        ("wb_files", json!({ "threadId": t })),
        ("wb_files", json!({ "projectId": p })),
        ("wb_file_preview", json!({ "threadId": t, "path": "src/main.rs" })),
        (
            "wb_file_preview",
            json!({ "threadId": t, "path": "shot.png", "offset": 0 }),
        ),
        (
            "wb_file_media",
            json!({ "threadId": t, "path": "shot.png", "offset": 0 }),
        ),
        ("wb_file_read", json!({ "threadId": t, "path": "src/main.rs" })),
        ("wb_file_search", json!({ "threadId": t, "query": "main" })),
        (
            "wb_file_search",
            json!({ "threadId": t, "query": "main", "caseSensitive": true }),
        ),
        ("wb_mention_prs", json!({ "threadId": t })),
        ("wb_mention_prs", json!({ "projectId": p })),
        ("wb_pull_request", json!({ "threadId": t, "title": "x", "body": "y" })),
        ("wb_git_state", json!({ "threadId": t })),
        ("wb_git_state", json!({ "threadId": t, "repo": "inner" })),
        ("wb_commit", json!({ "threadId": t, "message": "m", "push": true })),
        ("wb_draft_context", json!({ "threadId": t, "kind": "commit" })),
        (
            "wb_draft_context",
            json!({ "threadId": t, "repo": "inner", "kind": "pullRequest" }),
        ),
        ("wb_switch_branch", json!({ "threadId": t, "branch": "main" })),
        ("wb_git_fetch", json!({ "threadId": t })),
        (
            "wb_git_pull",
            json!({ "threadId": t, "repo": "inner", "rebase": false }),
        ),
        ("wb_branch_create", json!({ "threadId": t, "branch": "feature" })),
        ("wb_changed_turns", json!({ "threadId": t })),
        ("wb_changes", json!({ "threadId": t, "turn": null })),
        ("wb_turn_files", json!({ "threadId": t, "turn": 4 })),
        ("wb_diff", json!({ "threadId": t, "source": { "kind": "branch" } })),
        (
            "wb_diff",
            json!({ "threadId": t, "source": { "kind": "commit", "id": "abc1234" } }),
        ),
        ("wb_diff", json!({ "threadId": t, "source": { "kind": "uncommitted" } })),
        ("wb_diff_targets", json!({ "threadId": t })),
        ("wb_revert_preview", json!({ "threadId": t, "turn": 4 })),
        ("wb_revert_to_turn", json!({ "threadId": t, "turn": 4 })),
        ("wb_rewind_message", json!({ "threadId": t, "seq": 1 })),
        ("wb_worktree_remove", json!({ "threadId": t })),
        ("wb_worktrees", json!({})),
        ("wb_worktree_remove_orphan", json!({ "path": "/nowhere" })),
        (
            "wb_worktree_cleanup",
            json!({ "rules": { "merged": false, "archived": true, "idleDays": 14 } }),
        ),
        ("wb_move_to_worktree", json!({ "threadId": t })),
        ("wb_thread_delete", json!({ "threadId": t })),
        ("wb_thread_fork", json!({ "threadId": t, "turn": null })),
        ("wb_send_back", json!({ "threadId": t })),
        ("wb_side_chat", json!({ "threadId": t })),
        ("wb_side_chat_close", json!({ "threadId": t })),
        ("wb_thread_events", json!({ "threadId": t, "afterSeq": 0 })),
        ("wb_thread_stream", json!({ "threadId": t, "afterSeq": 0 })),
        (
            "wb_thread_page",
            json!({ "threadId": t, "beforeSeq": null, "limit": 400 }),
        ),
        ("wb_search", json!({ "query": "words" })),
        ("wb_send", json!({ "threadId": t, "text": "hi", "images": [] })),
        (
            "wb_approve",
            json!({ "threadId": t, "requestId": "r", "decision": "allowSession", "message": null, "suggestion": 0,
                    "answers": { "Which?": "A" } }),
        ),
        (
            "wb_attachment_chunk",
            json!({ "uploadId": "u", "size": 1, "offset": 0, "data": "AA==", "sha256": null }),
        ),
        ("wb_attachment_cancel", json!({ "uploadId": "u" })),
        ("wb_queue_cancel", json!({ "threadId": t, "seq": 2 })),
        ("wb_queue_edit", json!({ "threadId": t, "seq": 2, "text": "x" })),
        ("wb_queue_move", json!({ "threadId": t, "seq": 2, "before": null })),
        ("wb_queue_hold", json!({ "threadId": t, "seq": 2, "on": true })),
        ("wb_queue_steer", json!({ "threadId": t, "seq": 2 })),
        ("wb_interrupt", json!({ "threadId": t })),
        ("wb_task_stop", json!({ "threadId": t, "taskId": "a1" })),
        ("wb_task_background", json!({ "threadId": t, "toolUseId": "toolu_1" })),
        ("wb_task_output", json!({ "threadId": t, "taskId": "b1" })),
        ("wb_connection", json!({ "threadId": t })),
        ("wb_compact", json!({ "threadId": t, "instructions": null })),
        ("wb_stop", json!({ "threadId": t })),
        ("wb_setup_stop", json!({ "threadId": t })),
        (
            "wb_thread_switch_agent",
            json!({"threadId": "missing", "expectedAgent": "claude", "agent": "codex", "model": null, "effort": null, "via": "nexus", "account": null}),
        ),
        (
            "wb_thread_update",
            json!({ "threadId": t, "title": "T", "model": "", "permissionMode": "acceptEdits", "effort": "", "archived": true, "pinned": true, "snoozedUntil": 0, "keepListed": false }),
        ),
        ("wb_settle_merged", json!({ "threadId": t, "number": 7 })),
        (
            "wb_check_failed",
            json!({ "threadId": t, "name": "build", "url": null }),
        ),
        (
            "wb_link_pull_request",
            json!({ "threadId": t, "url": "https://github.com/example/repo/pull/7" }),
        ),
    ];
    for name in COMMANDS {
        assert!(
            calls.iter().any(|(called, _)| called == name),
            "{name} is not called here"
        );
    }
    for (name, args) in calls {
        if let Err(error) = s.api.call(name, args).await {
            assert!(!error.starts_with("Invalid arguments"), "{name}: {error}");
            assert!(!error.starts_with("Unknown command"), "{name}: {error}");
        }
    }
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A machine with no setup of its own answers `idle`, with the fields contract.ts's `CloudSetup` declares.
#[tokio::test]
async fn the_machines_own_setup_answers_as_the_contract_reads_it() {
    let s = setup("cloud-setup");
    let types = contract_types();
    let start = types
        .find("export interface CloudSetup {")
        .expect("no CloudSetup in contract.ts");
    let body = &types[start..];
    let body = &body[body.find('{').expect("{") + 1..body.find("\n}").expect("}")];
    for name in ["wb_cloud_setup", "wb_cloud_setup_retry"] {
        let view = call(&s.api, name, json!({})).await;
        assert_eq!(view["state"], "idle", "{name}: {view}");
        assert_eq!(json_fields(&view), field_names(body), "{name}");
    }
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// Gives no token: Gateway withdrew the grant (it ran out). Counts the asks.
struct Withdrawn(Arc<std::sync::atomic::AtomicUsize>);

impl nexus_workbench::cloud_setup::TokenSource for Withdrawn {
    fn token(&self) -> BoxFuture<'static, Result<String, nexus_workbench::cloud_setup::TokenError>> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async {
            Err(nexus_workbench::cloud_setup::TokenError::Withdrawn(
                "grant ended".into(),
            ))
        })
    }
}

/// Try again after `.agents/setup` failed, with its clone kept, runs on that clone without asking Gateway for a token:
/// after the grant's hour Gateway refuses one, and the setup must not go `withdrawn` and leave the app a taken folder.
#[tokio::test]
async fn try_again_after_a_failed_agents_setup_needs_no_token() {
    let s = setup("cloud-setup-retry");
    let clone = s.machine.home.join("web");
    std::fs::create_dir_all(&clone).unwrap_or_else(|e| panic!("{e}"));
    let init = nexus_workbench::process::command("git")
        .args(["init", "--quiet"])
        .current_dir(&clone)
        .status()
        .is_ok_and(|status| status.success());
    assert!(init, "git init");
    let file = s.root.join("host/setup.json");
    std::fs::create_dir_all(s.root.join("host")).unwrap_or_else(|e| panic!("{e}"));
    let saved = json!({ "repo": "acme/web", "state": "failed", "path": clone, "reason": ".agents/setup failed", "cloned": true });
    std::fs::write(&file, saved.to_string()).unwrap_or_else(|e| panic!("{e}"));
    let asks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    s.api
        .begin_cloud_setup("acme/web".into(), file, Arc::new(Withdrawn(asks.clone())));
    assert_eq!(call(&s.api, "wb_cloud_setup", json!({})).await["state"], "failed");
    call(&s.api, "wb_cloud_setup_retry", json!({})).await;
    let mut view = Value::Null;
    for _ in 0..100 {
        view = call(&s.api, "wb_cloud_setup", json!({})).await;
        if view["state"] != "setup" && view["state"] != "cloning" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(view["state"], "ready", "{view}");
    assert_eq!(view["project"]["path"], json!(clone), "{view}");
    assert_eq!(
        asks.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a token was asked for"
    );
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A schedule goes in as contract.ts's `ScheduleInput` and comes back as its `Schedule`, with no webhook token in the
/// list.
#[tokio::test]
async fn schedules_are_kept_over_the_wire() {
    let s = setup("schedules");
    let project = call(&s.api, "wb_project_add", json!({ "path": s.repo })).await;
    let saved = call(
        &s.api,
        "wb_schedule_save",
        json!({ "schedule": { "projectId": project["id"], "prompt": "Triage the new issues", "agent": "claude",
                "permissionMode": "acceptEdits", "via": "nexus", "when": { "kind": "interval", "minutes": 60 } } }),
    )
    .await;
    assert_eq!(saved["title"], "Triage the new issues");
    assert_eq!(
        (
            saved["enabled"].clone(),
            saved["catchUp"].clone(),
            saved["worktree"].clone()
        ),
        (json!(true), json!(true), json!(true))
    );
    assert_eq!(saved["maxRunsPerDay"], 12);
    assert!(saved["nextRunAt"].as_i64().is_some());
    let mut fields = keys(&saved);
    fields.sort_unstable();
    assert_eq!(
        fields,
        [
            "agent",
            "anchor",
            "catchUp",
            "createdAt",
            "createdBy",
            "day",
            "effort",
            "enabled",
            "id",
            "maxRunsPerDay",
            "model",
            "nextRunAt",
            "permissionMode",
            "projectId",
            "prompt",
            "runs",
            "runsOnDay",
            "title",
            "via",
            "webhook",
            "when",
            "worktree"
        ]
    );
    let changed = call(
        &s.api,
        "wb_schedule_save",
        json!({ "scheduleId": saved["id"], "schedule": { "projectId": project["id"], "prompt": "Triage", "agent": "codex",
                "permissionMode": "plan", "via": "nexus", "enabled": false, "when": { "kind": "manual" } } }),
    )
    .await;
    assert_eq!(
        (changed["id"].clone(), changed["agent"].clone()),
        (saved["id"].clone(), json!("codex"))
    );
    assert_eq!(changed["nextRunAt"], Value::Null);
    let list = call(&s.api, "wb_schedules", json!({})).await;
    assert_eq!(list, json!([changed]));
    assert!(!list.to_string().contains("token"));
    call(&s.api, "wb_schedule_delete", json!({ "scheduleId": saved["id"] })).await;
    assert_eq!(call(&s.api, "wb_schedules", json!({})).await, json!([]));
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A project's commands go in and come back over the wire as contract.ts's `ProjectScript` list, in their order; a
/// refused list leaves the saved one as it was, and closing the project takes them away.
#[tokio::test]
async fn project_commands_are_kept_per_project_over_the_wire() {
    let s = setup("scripts");
    let project = call(&s.api, "wb_project_add", json!({ "path": s.repo })).await;
    let p = project["id"].clone();
    assert_eq!(
        call(&s.api, "wb_project_scripts", json!({ "projectId": p })).await,
        json!([])
    );
    let scripts = json!([
        { "id": "test", "name": " Test ", "command": "cargo test\n", "shortcut": "Mod+Alt+KeyT" },
        { "id": "dev", "name": "Dev server", "command": "npm run dev" },
    ]);
    let saved = call(
        &s.api,
        "wb_project_scripts_set",
        json!({ "projectId": p, "scripts": scripts }),
    )
    .await;
    let expected = json!([
        { "id": "test", "name": "Test", "command": "cargo test", "shortcut": "Mod+Alt+KeyT" },
        { "id": "dev", "name": "Dev server", "command": "npm run dev" },
    ]);
    assert_eq!(saved, expected);
    assert_eq!(
        call(&s.api, "wb_project_scripts", json!({ "projectId": p })).await,
        expected
    );

    let clash = json!([
        { "id": "a", "name": "A", "command": "ls", "shortcut": "Mod+Alt+KeyL" },
        { "id": "b", "name": "B", "command": "pwd", "shortcut": "Mod+Alt+KeyL" },
    ]);
    let refused = s
        .api
        .call("wb_project_scripts_set", json!({ "projectId": p, "scripts": clash }))
        .await
        .err()
        .unwrap_or_default();
    assert_eq!(refused, "“B” and “A” have the same key.");
    assert_eq!(
        call(&s.api, "wb_project_scripts", json!({ "projectId": p })).await,
        expected
    );
    let gone = s
        .api
        .call(
            "wb_project_scripts_set",
            json!({ "projectId": "no-such-project", "scripts": [] }),
        )
        .await
        .err()
        .unwrap_or_default();
    assert_eq!(gone, "That project is no longer open.");

    call(&s.api, "wb_project_remove", json!({ "projectId": p })).await;
    let again = call(&s.api, "wb_project_add", json!({ "path": s.repo })).await;
    assert_eq!(
        call(&s.api, "wb_project_scripts", json!({ "projectId": again["id"] })).await,
        json!([])
    );
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A new session that does not say gets subsessions from the computer's switch, on by default, never from its
/// project's saved flag. One that says keeps its own choice.
#[tokio::test]
async fn a_new_session_gets_subsessions_from_the_computers_switch_not_its_project() {
    let s = setup("subsessions-switch");
    let project = call(&s.api, "wb_project_add", json!({ "path": s.repo })).await;
    let p = project["id"].clone();
    async fn create(api: &Api, project: &Value, orchestration: Option<bool>) -> bool {
        let mut args = json!({
            "projectId": project, "agent": "claude", "model": null, "permissionMode": "default",
            "effort": null, "via": "nexus", "text": "hi", "images": [], "worktree": null,
        });
        if let Some(on) = orchestration {
            args["orchestration"] = json!(on);
        }
        let created = call(api, "wb_thread_create", args).await;
        created["orchestration"].as_bool().unwrap_or(false)
    }

    // The project's flag is off, as every project's is until set: the computer's switch, on, decides.
    assert_eq!(project.get("orchestration"), None);
    assert!(create(&s.api, &p, None).await, "on by default");
    assert!(
        !create(&s.api, &p, Some(false)).await,
        "a start that says keeps its choice"
    );

    // The person turned subsessions off on this computer; the project's old flag, on, is not read.
    call(
        &s.api,
        "wb_project_orchestration_set",
        json!({ "projectId": p, "on": true }),
    )
    .await;
    s.api.workbench().set_subsessions(nexus_workbench::Subsessions {
        on: false,
        ..Default::default()
    });
    assert!(!create(&s.api, &p, None).await, "off on this computer");
    assert!(
        create(&s.api, &p, Some(true)).await,
        "a start that says keeps its choice"
    );
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// `wb_project_update` pins and archives a project over the wire as contract.ts's `Project` says: each field only when
/// set, one change at a time, pushed to the app in `wb:projects`. The core says so in its features.
#[tokio::test]
async fn a_project_is_pinned_and_archived_over_the_wire() {
    let s = setup("project-update");
    let overview = call(&s.api, "wb_overview", json!({})).await;
    assert!(
        overview["features"]
            .as_array()
            .is_some_and(|features| features.contains(&json!("project-pin-archive")))
    );
    let project = call(&s.api, "wb_project_add", json!({ "path": s.repo })).await;
    let p = project["id"].clone();
    assert_eq!((project.get("pinned"), project.get("archived")), (None, None));

    let pinned = call(&s.api, "wb_project_update", json!({ "projectId": p, "pinned": true })).await;
    assert_eq!((pinned["pinned"].clone(), pinned.get("archived")), (json!(true), None));
    let archived = call(&s.api, "wb_project_update", json!({ "projectId": p, "archived": true })).await;
    assert_eq!(
        (archived["pinned"].clone(), archived["archived"].clone()),
        (json!(true), json!(true)),
        "a field left out stays"
    );
    let pushed: Vec<Value> = s
        .host
        .emitted
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|emit| serde_json::to_value(Push::new(emit)).unwrap_or_default())
        .filter(|push| push["event"] == "wb:projects")
        .collect();
    assert_eq!(pushed.last().map(|push| push["payload"][0].clone()), Some(archived));
    let overview = call(&s.api, "wb_overview", json!({})).await;
    assert_eq!(overview["projects"][0]["archived"], json!(true), "still listed");

    let back = call(
        &s.api,
        "wb_project_update",
        json!({ "projectId": p, "pinned": false, "archived": false }),
    )
    .await;
    assert_eq!((back.get("pinned"), back.get("archived")), (None, None));
    let gone = s
        .api
        .call(
            "wb_project_update",
            json!({ "projectId": "no-such-project", "pinned": true }),
        )
        .await
        .err()
        .unwrap_or_default();
    assert_eq!(gone, "That project is no longer open.");
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// `wb_project_clone` clones into the chosen folder under the machine's home, opens the clone as a project, and stops
/// answering its progress once it ends.
#[tokio::test]
async fn a_cloned_repository_opens_as_a_project() {
    let s = setup("clone");
    let git = |args: &[&str]| {
        let out = nexus_workbench::process::command("git")
            .arg("-C")
            .arg(&s.repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "--quiet", "--initial-branch", "main"]);
    git(&["commit", "--quiet", "--allow-empty", "-m", "one"]);
    let source = format!("file://{}", s.repo.display());
    let args = json!({ "source": source, "parent": "~/code", "requestId": "c1" });
    let project = call(&s.api, "wb_project_clone", args.clone()).await;
    let dest = s.root.join("home/code/repo");
    assert_eq!(project["path"], dest.to_string_lossy().as_ref());
    assert_eq!(project["branch"], "main");
    assert!(dest.join(".git").is_dir());
    let progress = call(&s.api, "wb_project_clone_progress", json!({ "requestId": "c1" })).await;
    assert_eq!(progress, Value::Null);
    // The folder is taken now: a second clone into it is refused, and the first is left as it is.
    let again = s.api.call("wb_project_clone", args).await.err().unwrap_or_default();
    assert!(again.contains("already exists"), "{again}");
    assert!(dest.join(".git").is_dir());
    s.api.workbench().shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_local_first_message_cannot_be_created_in_a_new_account_scope() {
    const LOCAL_SCOPE: &str = "|";

    let s = setup("create-wrong-scope");
    let workbench = s.api.workbench();
    workbench.set_scope(LOCAL_SCOPE);
    let project = workbench.add_project(&s.root).expect("add a project");
    workbench.set_scope("https://gateway.example|new-account");
    let error = s
        .api
        .call(
            "wb_thread_create_bound",
            json!({
                "projectId": project.id,
                "agent": "claude",
                "permissionMode": "default",
                "via": "subscription",
                "text": "This prompt belongs to a local session",
                "images": [],
                "source": {"scope": LOCAL_SCOPE, "identity": null},
            }),
        )
        .await
        .expect_err("the new account must never receive the original draft");
    assert_eq!(error, "The account changed before the session could start.");
    assert!(workbench.threads().expect("account sessions").is_empty());
    workbench.set_scope(LOCAL_SCOPE);
    assert!(workbench.threads().expect("local sessions").is_empty());
}
