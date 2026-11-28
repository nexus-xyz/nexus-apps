//! The sessions tools an outside client calls: their definitions, which of them act, and the pure parts of their
//! answers (a session's summary, its latest turns, the project a start names). [`run`] runs the ones that read and
//! message sessions, on the app's workbench or its background service. Each app starts a session its own way.

use nexus_remote::engine::Engine;
use nexus_workbench::api::UpdateArgs;
use nexus_workbench::event::{PermissionMode, Project, StoredEvent, ThreadSummary, TurnStatus, WbEvent};
use nexus_workbench::harness::RunFacts;
use nexus_workbench::runs_as::runs_as;
use serde_json::{Map, Value, json};

use super::grants::Scope;

pub const LIST: &str = "list_sessions";
pub const SHOW: &str = "show_session";
pub const START: &str = "start_session";
pub const SEND: &str = "send_message";
pub const STOP: &str = "stop_session";
pub const NAME: &str = "name_session";

/// The most events `show_session` reads back for a session's latest turns.
const SHOW_EVENTS: usize = 400;

/// The most a message or a prompt may hold, in characters.
pub const MAX_TEXT: usize = 32_000;
/// The most of one turn's text `show_session` returns, in characters.
const MAX_TURN_TEXT: usize = 8_000;

/// Runs `list_sessions`, `show_session`, `send_message`, `stop_session` and `name_session` on `engine`: the app's
/// workbench, or the background service while it runs the sessions. None for any other tool: `start_session` is each
/// app's own.
pub async fn run(engine: &Engine, name: &str, args: &Map<String, Value>) -> Option<Result<Value, String>> {
    if ![LIST, SHOW, SEND, STOP, NAME].contains(&name) {
        return None;
    }
    Some(run_reading(engine, name, args).await)
}

async fn run_reading(engine: &Engine, name: &str, args: &Map<String, Value>) -> Result<Value, String> {
    let (threads, projects, facts) = engine.sessions().await?;
    let facts = facts.as_ref();
    let session = || {
        args.get("session_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let lookup = |word: &str| find(&threads, word).cloned();
    match name {
        LIST => {
            let project = match args.get("project").and_then(Value::as_str) {
                Some(query) => Some(find_project(&projects, query)?.id.clone()),
                None => None,
            };
            let archived = args.get("archived").and_then(Value::as_bool).unwrap_or(false);
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, 200) as usize;
            let sessions: Vec<Value> = listed(threads.clone(), project.as_deref(), archived, limit)
                .iter()
                .map(|thread| summary(thread, &projects, facts))
                .collect();
            Ok(json!({"sessions": sessions}))
        }
        SHOW => {
            let thread = lookup(&session())?;
            let id = thread.id.clone();
            let count = args.get("turns").and_then(Value::as_u64).unwrap_or(3).clamp(1, 20) as usize;
            let events = engine.latest_events(id, SHOW_EVENTS).await?;
            let mut shown = summary(&thread, &projects, facts);
            shown["turns"] = Value::Array(turns(&events, count));
            Ok(shown)
        }
        SEND => {
            let thread = lookup(&session())?;
            if let Some(project) = projects.iter().find(|project| project.id == thread.project_id) {
                in_sight(project)?;
            }
            let id = thread.id;
            let text = text_arg(args, "text")?;
            engine.send_files(&id, &text, vec![], vec![]).await?;
            Ok(json!({"sent": true, "sessionId": id}))
        }
        NAME => {
            let id = lookup(&session())?.id;
            // A string sets it; "" or null clears it. The workbench refuses an invalid name or another session's.
            let name = match args.get("name") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(name)) => name.clone(),
                Some(_) => return Err("name must be a string.".into()),
            };
            let thread = engine
                .thread_update(UpdateArgs {
                    thread_id: id,
                    name: Some(name),
                    ..UpdateArgs::default()
                })
                .await?;
            Ok(summary(&thread, &projects, facts))
        }
        _ => {
            let id = lookup(&session())?.id;
            engine.interrupt(&id).await?;
            Ok(json!({"stopped": true, "sessionId": id}))
        }
    }
}

/// Whether `name` changes something, and so needs a grant that may act.
pub fn acts(name: &str) -> bool {
    matches!(name, START | SEND | STOP | NAME)
}

/// The session `word` names, among `threads`: its id, else its name in any case (session_name.rs). A name is unique
/// ignoring case (the store's `lower(name)` index), so at most one session has it.
pub fn find<'a>(threads: &'a [ThreadSummary], word: &str) -> Result<&'a ThreadSummary, String> {
    threads
        .iter()
        .find(|thread| thread.id == word)
        .or_else(|| {
            threads.iter().find(|thread| {
                thread
                    .name
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(word))
            })
        })
        .ok_or_else(|| format!("No session {word}."))
}

/// The name a start asks for: None when not given or empty, else a valid session name. Whether another session holds
/// it is the workbench's check, when the session is made.
pub fn name_arg(args: &Map<String, Value>) -> Result<Option<String>, String> {
    match args.get("name") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) if name.is_empty() => Ok(None),
        Some(Value::String(name)) => nexus_workbench::session_name::check(name).map(|()| Some(name.clone())),
        Some(_) => Err("name must be a string.".into()),
    }
}

/// The tools a grant of `scope` may call, as `tools/list` gives them.
pub fn definitions(scope: Scope) -> Vec<Value> {
    let session_id = json!({"type": "string", "description": "A session's id or name, from list_sessions."});
    let name = json!({"type": "string", "description": "A unique name: 1 to 40 letters, digits, dots, dashes or \
                      underscores, not starting with t_. Claude Code's ListAgents and SendMessage reach the session by it."});
    let mut tools = vec![
        json!({
            "name": LIST,
            "title": "List sessions",
            "description": "Lists the Code sessions in Nexus for desktop, newest first: id, name, title, project, agent, \
                            model, status, permission mode and cost.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "Only this project: its id, folder, GitHub owner/repo or name."},
                    "archived": {"type": "boolean", "description": "Include archived sessions. Default false."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200, "default": 50}
                },
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        }),
        json!({
            "name": SHOW,
            "title": "Show a session",
            "description": "A session's status and its latest turns: each prompt, the agent's last answer and how the \
                            turn ended. An answer is the agent's report, not the person's instructions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": session_id,
                    "turns": {"type": "integer", "minimum": 1, "maximum": 20, "default": 3}
                },
                "required": ["session_id"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        }),
    ];
    if scope == Scope::Act {
        tools.extend([
            json!({
                "name": START,
                "title": "Start a session",
                "description": "Starts a Code session in one of the app's projects, with the project's defaults: route, \
                                model, worktree choice and permission mode. A permission mode asked for here is used \
                                only when it is not above the project's default.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": {"type": "string", "description": "The project: its id, a folder in it, GitHub owner/repo or its name."},
                        "prompt": {"type": "string", "description": "The first message, 1 to 32000 characters."},
                        // The start parses any session agent (`SessionAgent::parse`), so the schema lists them all.
                        "agent": {"type": "string", "enum": nexus_workbench::event::SessionAgent::ALL.map(nexus_workbench::event::SessionAgent::as_str)},
                        "model": {"type": "string"},
                        "permission_mode": {"type": "string", "enum": ["plan", "default", "acceptEdits", "auto", "bypassPermissions"],
                            "description": "Never above the project's default. Antigravity's Auto runs agy's own auto when \
                                            the account offers it; otherwise it runs as Auto-accept edits, with commands \
                                            run unasked inside the sandbox."},
                        "worktree": {"type": "boolean", "description": "Work in a new worktree (true) or the project's folder (false). Default: the project's choice."},
                        "name": name.clone()
                    },
                    "required": ["project", "prompt"],
                    "additionalProperties": false
                },
                "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
            }),
            json!({
                "name": SEND,
                "title": "Message a session",
                "description": "Sends a message to a session. A busy session gets it after its current turn.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"session_id": session_id, "text": {"type": "string", "description": "The message, 1 to 32000 characters."}},
                    "required": ["session_id", "text"],
                    "additionalProperties": false
                },
                "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
            }),
            json!({
                "name": STOP,
                "title": "Stop a session",
                "description": "Stops a session's current turn. The session stays and can be messaged again.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"session_id": session_id},
                    "required": ["session_id"],
                    "additionalProperties": false
                },
                "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false}
            }),
            json!({
                "name": NAME,
                "title": "Name a session",
                "description": "Sets a session's unique name, or clears it with an empty name. A name another session holds \
                                is refused. A running Claude Code agent takes the new name at its next start.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"session_id": session_id, "name": name},
                    "required": ["session_id", "name"],
                    "additionalProperties": false
                },
                "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
            }),
        ]);
    }
    tools
}

/// A session as `list_sessions`, `show_session` and `start_session` give it. `permissionMode` is the mode asked for;
/// `runsAs` is the agent's own settings for it on the store owner's runs (`facts`), null when the owner did not say.
pub fn summary(thread: &ThreadSummary, projects: &[Project], facts: Option<&RunFacts>) -> Value {
    let project = projects.iter().find(|project| project.id == thread.project_id);
    let runs_as = facts.map(|facts| runs_as(thread.agent, thread.permission_mode, facts));
    json!({
        "id": thread.id,
        "name": thread.name,
        "title": thread.title,
        "project": project.map(|project| json!({"id": project.id, "name": project.name, "path": project.path})),
        "agent": thread.agent,
        "model": thread.model,
        "permissionMode": thread.permission_mode,
        "runsAs": runs_as,
        "status": thread.status,
        "costUsd": thread.cost_usd,
        "worktree": thread.worktree.as_ref().map(|worktree| json!({"path": worktree.path, "branch": worktree.branch})),
        "archived": thread.archived,
        "updatedAt": thread.updated_at,
    })
}

/// The sessions `list_sessions` returns, newest first: those in `project` (any when none), archived ones only when
/// asked. Side chats are left out, as the sidebar leaves them out.
pub fn listed(threads: Vec<ThreadSummary>, project: Option<&str>, archived: bool, limit: usize) -> Vec<ThreadSummary> {
    let mut threads: Vec<_> = threads
        .into_iter()
        .filter(|thread| !thread.side_chat)
        .filter(|thread| archived || !thread.archived)
        .filter(|thread| project.is_none_or(|id| thread.project_id == id))
        .collect();
    threads.sort_by_key(|thread| std::cmp::Reverse(thread.updated_at));
    threads.truncate(limit);
    threads
}

fn cut(text: &str) -> String {
    match text.char_indices().nth(MAX_TURN_TEXT) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

/// The last `count` turns in `events`: each prompt, the agent's last top-level answer, and how the turn ended
/// (`running` while it has not; `approval` while it waits for the person).
pub fn turns(events: &[StoredEvent], count: usize) -> Vec<Value> {
    struct Turn {
        prompt: String,
        answer: String,
        status: &'static str,
        error: Option<String>,
    }
    let mut turns: Vec<Turn> = Vec::new();
    for stored in events {
        match &stored.event {
            WbEvent::User { text, .. } => turns.push(Turn {
                prompt: text.clone(),
                answer: String::new(),
                status: "running",
                error: None,
            }),
            WbEvent::Text { text, parent: None, .. } => {
                if let Some(turn) = turns.last_mut() {
                    turn.answer = text.clone();
                }
            }
            WbEvent::Approval { parent: None, .. } => {
                if let Some(turn) = turns.last_mut() {
                    turn.status = "approval";
                }
            }
            WbEvent::ApprovalResolved { .. } => {
                if let Some(turn) = turns.last_mut().filter(|turn| turn.status == "approval") {
                    turn.status = "running";
                }
            }
            WbEvent::TurnEnd { status, error, .. } => {
                if let Some(turn) = turns.last_mut() {
                    turn.status = match status {
                        TurnStatus::Completed => "completed",
                        TurnStatus::Interrupted => "interrupted",
                        TurnStatus::Error => "error",
                    };
                    turn.error = error.clone();
                }
            }
            _ => {}
        }
    }
    let skip = turns.len().saturating_sub(count);
    turns
        .into_iter()
        .skip(skip)
        .map(|turn| json!({"prompt": cut(&turn.prompt), "answer": cut(&turn.answer), "status": turn.status, "error": turn.error}))
        .collect()
}

/// The project a start or a list names: the workbench's lookup, shared with `start_session`'s `project`.
pub use nexus_workbench::project_lookup::{find_project, in_sight};

/// A permission mode asked for in a start, read by name. None when it was not given.
pub fn asked_mode(value: &Value) -> Result<Option<PermissionMode>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(name) => PermissionMode::parse(name)
            .map(Some)
            .ok_or_else(|| format!("Unknown permission mode {name}.")),
        _ => Err("permission_mode must be a string.".into()),
    }
}

/// The text in `args[key]`: 1 to [`MAX_TEXT`] characters.
pub fn text_arg(args: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    let text = args.get(key).and_then(Value::as_str).unwrap_or_default();
    if text.trim().is_empty() {
        return Err(format!("{key} is required."));
    }
    if text.chars().count() > MAX_TEXT {
        return Err(format!("{key} is longer than {MAX_TEXT} characters."));
    }
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: &str, path: &str, remotes: &[&str]) -> Project {
        Project {
            id: id.into(),
            name: id.into(),
            path: path.into(),
            branch: Some("main".into()),
            remotes: remotes.iter().map(|remote| remote.to_string()).collect(),
            added_at: 0,
            orchestration: false,
            pinned: false,
            archived: false,
            host: Default::default(),
        }
    }

    fn thread(id: &str, project: &str, updated_at: i64) -> ThreadSummary {
        serde_json::from_value(json!({
            "id": id, "projectId": project, "title": id, "agent": "claude", "model": null,
            "permissionMode": "default", "via": "nexus", "status": "idle", "costUsd": 0.0, "nexusSessions": [],
            "contextTokens": null, "contextWindow": null, "createdAt": 0, "updatedAt": updated_at, "archived": false,
        }))
        .expect("a summary")
    }

    #[test]
    fn the_list_leaves_out_side_chats_and_archived_sessions() {
        let mut side = thread("side", "p1", 9);
        side.side_chat = true;
        side.source_thread_id = Some("a".into());
        let mut old = thread("old", "p1", 1);
        old.archived = true;
        let threads = vec![thread("a", "p1", 5), side, old, thread("b", "p2", 7)];
        let ids = |listed: Vec<ThreadSummary>| listed.into_iter().map(|thread| thread.id).collect::<Vec<_>>();
        assert_eq!(ids(listed(threads.clone(), None, false, 50)), ["b", "a"]);
        assert_eq!(ids(listed(threads.clone(), None, true, 50)), ["b", "a", "old"]);
        assert_eq!(ids(listed(threads.clone(), Some("p1"), false, 50)), ["a"]);
        assert_eq!(ids(listed(threads, None, false, 1)), ["b"]);
    }

    #[test]
    fn a_read_grant_lists_no_tool_that_acts() {
        let read: Vec<String> = definitions(Scope::Read)
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(read, [LIST, SHOW]);
        assert!(read.iter().all(|name| !acts(name)));
        assert_eq!(definitions(Scope::Act).len(), 6);
        assert!(acts(START) && acts(SEND) && acts(STOP) && acts(NAME));
    }

    /// The outside start finds a project by the workbench's lookup: its name too, and an archived one is refused.
    #[test]
    fn a_start_finds_its_project_by_the_workbench_lookup() {
        let mut projects = [project("p1", "/home/a/nexus", &["git@github.com:acme/nexus.git"])];
        assert_eq!(find_project(&projects, "P1").map(|p| &p.id), Ok(&"p1".to_string()));
        projects[0].archived = true;
        let refusal = find_project(&projects, "acme/nexus")
            .and_then(in_sight)
            .expect_err("archived");
        assert!(
            refusal.contains("p1 is archived") && refusal.contains("Unarchive"),
            "{refusal}"
        );
    }

    #[test]
    fn turns_give_each_prompt_its_last_answer_and_its_end() {
        let event = |seq, event| StoredEvent {
            thread_id: "t".into(),
            seq,
            at: 0,
            event,
        };
        let text = |id: &str, text: &str| WbEvent::Text {
            id: id.into(),
            text: text.into(),
            parent: None,
        };
        let events = [
            event(
                1,
                WbEvent::User {
                    text: "first".into(),
                    images: vec![],
                    from: None,
                },
            ),
            event(2, text("a", "thinking aloud")),
            event(3, text("b", "done")),
            event(
                4,
                WbEvent::TurnEnd {
                    turn_id: "1".into(),
                    status: TurnStatus::Completed,
                    cost_usd: None,
                    duration_ms: 1,
                    error: None,
                    cursor: None,
                },
            ),
            event(
                5,
                WbEvent::User {
                    text: "second".into(),
                    images: vec![],
                    from: None,
                },
            ),
        ];
        let all = turns(&events, 3);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0]["answer"], "done");
        assert_eq!(all[0]["status"], "completed");
        assert_eq!(all[1]["status"], "running");
        assert_eq!(turns(&events, 1)[0]["prompt"], "second");
    }

    #[test]
    fn a_session_is_found_by_its_id_then_its_name() {
        let mut named = thread("t_1", "p1", 1);
        named.name = Some("orchestrator".into());
        let mut other = thread("t_2", "p1", 2);
        other.name = Some("worker".into());
        let threads = vec![named, other];
        let id = |word: &str| find(&threads, word).map(|thread| thread.id.clone());
        assert_eq!(id("orchestrator"), Ok("t_1".to_string()));
        assert_eq!(id("worker"), Ok("t_2".to_string()));
        assert_eq!(id("t_2"), Ok("t_2".to_string()));
        // A name is unique ignoring case (the store's lower(name) index), so any case finds it.
        assert_eq!(id("Orchestrator"), Ok("t_1".to_string()));
        assert_eq!(id("WORKER"), Ok("t_2".to_string()));
        assert_eq!(id("orchestrators"), Err("No session orchestrators.".to_string()));
        // The list and a show give the name.
        assert_eq!(summary(&threads[0], &[], None)["name"], "orchestrator");
        assert_eq!(summary(&threads[1], &[], None)["name"], "worker");
    }

    #[test]
    fn a_summary_says_what_the_agent_runs_with() {
        let mut agy = thread("t_1", "p1", 1);
        agy.agent = nexus_workbench::event::SessionAgent::Antigravity;
        agy.permission_mode = PermissionMode::Auto;
        let facts = RunFacts {
            sandbox: true,
            auto_offered: Some(false),
        };
        let shown = summary(&agy, &[], Some(&facts));
        assert_eq!(shown["permissionMode"], "auto");
        assert_eq!(shown["runsAs"], runs_as(agy.agent, PermissionMode::Auto, &facts));
        // The facts reach the agent's table: the Sandbox switch is on.
        assert_eq!(shown["runsAs"]["--sandbox"], "on");
        let claude = thread("t_2", "p1", 1);
        assert_eq!(
            summary(&claude, &[], Some(&facts))["runsAs"],
            json!({"--permission-mode": "default"})
        );
        // A service too old to give its run facts: the field is there, null.
        assert_eq!(summary(&claude, &[], None)["runsAs"], Value::Null);
    }

    #[test]
    fn a_start_takes_a_valid_name_or_none() {
        let args = |value: Value| json!({"name": value}).as_object().cloned().unwrap_or_default();
        assert_eq!(name_arg(&Map::new()), Ok(None));
        assert_eq!(name_arg(&args(json!(""))), Ok(None));
        assert_eq!(name_arg(&args(json!("orchestrator"))), Ok(Some("orchestrator".into())));
        assert!(name_arg(&args(json!("t_abc"))).is_err());
        assert!(name_arg(&args(json!("two words"))).is_err());
        assert!(name_arg(&args(json!(7))).is_err());
        let start = definitions(Scope::Act)
            .into_iter()
            .find(|tool| tool["name"] == START)
            .unwrap_or_default();
        assert_eq!(start["inputSchema"]["properties"]["name"]["type"], "string");
    }

    #[test]
    fn a_mode_is_read_by_name_only() {
        assert_eq!(asked_mode(&Value::Null), Ok(None));
        assert_eq!(asked_mode(&json!("plan")), Ok(Some(PermissionMode::Plan)));
        assert!(asked_mode(&json!("root")).is_err());
        assert!(asked_mode(&json!(1)).is_err());
    }
}
