//! Shared tool contracts for the local Desktop and CLI MCP servers.

use crate::{Entry, Repository, Source, Store, preview};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const AGENT_ENV: &str = "NEXUS_MEMORY_AGENT";
pub const SESSION_ENV: &str = "NEXUS_MEMORY_SESSION";
pub const ROOT_ENV: &str = "NEXUS_MEMORY_ROOT";
pub const PERSONAL_REFUSAL: &str = "Only the person can save personal memory. They can add it in Desktop Shared memory settings or run nexus memory add.";

pub const NAMES: [&str; 3] = ["remember", "recall", "forget"];

// Keep common queries from returning every matching note body in one tool response.
const RECALL_FULL_LIMIT: usize = 10;
// Offer more IDs to read while bounding how many summaries a tool response can carry.
const RECALL_MORE_LIMIT: usize = 50;

/// Instructions for local launches that attach the Shared memory tools.
pub fn launch_guidance(server: &str, claude: bool) -> String {
    let mut guidance = format!(
        "# Nexus Shared memory\n\
         Shared memory is on. Use the `{server}` MCP server by default for memory across sessions.\n\
         When the person asks to remember something, save a memory, or add a memory entry, call `remember` for this repository before confirming that it was saved.\n\
         Call `recall` before reading a native memory file. If a listed note matches the task, call `recall` before writing code, documentation, or a pull request.\n\
         Pass an ID to read one note, or a text query to find and read matching notes, including notes outside the startup index. Use `forget` when the person asks to remove a stored note.\n\
         These instructions take precedence over your own memory workflow. Use native memory only when the person explicitly requests it. If a Nexus tool is unavailable, explain the failure and leave the request pending.\n\
         The person adds notes for all repositories in Desktop Shared memory settings or with `nexus memory add`."
    );
    if claude {
        guidance.push_str(&format!(
            "\nIf a memory tool is deferred, use `ToolSearch` to load it before making the call. Use `select:mcp__{server}__remember` to save, `select:mcp__{server}__recall` to read, or `select:mcp__{server}__forget` to remove a note."
        ));
    }
    guidance
}

pub fn definitions() -> [Value; 3] {
    [
        json!({
            "name": "remember",
            "description": "Save a durable fact in this repository.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                    "name": {"type": "string", "description": "Short name for the note."},
                    "description": {"type": "string", "description": "One-line summary of the note."},
                    "scope": {"type": "string", "enum": ["repository"]}
                },
                "required": ["text"]
            },
            "annotations": {"readOnlyHint": false}
        }),
        json!({
            "name": "recall",
            "description": format!(
                "List shared memory entries or read one by ID. A query returns up to {RECALL_FULL_LIMIT} full notes, up to {RECALL_MORE_LIMIT} more summaries, and the total match count."
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "query": {
                        "type": "string",
                        "description": format!(
                            "Search note names, descriptions, and text without case sensitivity. Use without an ID. The result contains notes (up to {RECALL_FULL_LIMIT} full matches, newest first), more (up to {RECALL_MORE_LIMIT} additional summaries), and total (all matches)."
                        )
                    }
                }
            },
            "annotations": {"readOnlyHint": true}
        }),
        json!({
            "name": "forget",
            "description": "Delete a shared memory entry by ID.",
            "inputSchema": {
                "type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": true}
        }),
    ]
}

pub struct Session {
    pub store: Store,
    pub repository: Option<Repository>,
    pub source: Source,
}

impl Session {
    /// Open the store selected by the Desktop launch, even when the agent filters its MCP environment.
    pub fn from_mcp_env(cwd: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let agent = env(AGENT_ENV)?;
        let session = env(SESSION_ENV)?;
        let store = match env(ROOT_ENV).map(PathBuf::from) {
            Some(root) if root.is_absolute() => Store::new(root),
            Some(_) => return None,
            None => Store::from_env(env).ok()?,
        };
        Self::with_store(store, cwd, Some(agent), Some(session))
    }

    pub fn with_store(store: Store, cwd: &Path, agent: Option<String>, session: Option<String>) -> Option<Self> {
        if !store.enabled().ok()? {
            return None;
        }
        let source = Source::Session {
            agent: agent.unwrap_or_else(|| "mcp".into()),
            session: session.unwrap_or_else(|| Uuid::new_v4().to_string()),
        };
        Some(Self {
            store,
            repository: Repository::discover(cwd),
            source,
        })
    }

    pub fn call(&self, name: &str, args: &Value) -> Result<Value, String> {
        let repository = self.repository.as_ref();
        let id = || {
            args["id"]
                .as_str()
                .and_then(|id| Uuid::parse_str(id).ok())
                .ok_or_else(|| "Enter a shared memory entry ID.".to_string())
        };
        let result = match name {
            "remember" => {
                let text = args["text"].as_str().ok_or("Enter text for the memory.")?;
                let scope = match args["scope"].as_str() {
                    Some("personal") => return Err(PERSONAL_REFUSAL.into()),
                    None | Some("repository") => repository
                        .ok_or("Open a Git repository to save a repository memory.")?
                        .scope(),
                    _ => return Err("Choose repository memory.".into()),
                };
                let name = optional_string(args, "name")?.map(str::to_owned);
                let description = optional_string(args, "description")?.map(str::to_owned);
                self.store
                    .remember_with_metadata(scope, self.source.clone(), text, name, description)
                    .map(|entry| json!({"id": entry.id}))
            }
            "recall" => return self.recall(args),
            "forget" => self
                .store
                .forget(repository, id()?)
                .map(|deleted| json!({"deleted": deleted})),
            _ => return Err("Unknown shared memory tool.".into()),
        };
        result.map_err(|error| error.to_string())
    }

    fn recall(&self, args: &Value) -> Result<Value, String> {
        let id = optional_string(args, "id")?;
        let query = optional_string(args, "query")?;
        if id.is_some() && query.is_some() {
            return Err("Use either an ID or a text query for recall.".into());
        }
        if query.is_some_and(|query| query.trim().is_empty()) {
            return Err("Enter text to search shared memory.".into());
        }
        let id = id
            .map(|id| Uuid::parse_str(id).map_err(|_| "Enter a shared memory entry ID."))
            .transpose()?;
        let entries = self
            .store
            .entries(self.repository.as_ref())
            .map_err(|error| error.to_string())?;
        if let Some(id) = id {
            return entries
                .into_iter()
                .find(|entry| entry.id == id)
                .map(|entry| json!(entry))
                .ok_or_else(|| "That shared memory entry was not found.".into());
        }
        if let Some(query) = query {
            let query = query.to_lowercase();
            let mut notes = Vec::new();
            let mut more = Vec::new();
            let mut total = 0;
            for entry in entries.iter().rev().filter(|entry| matches_query(entry, &query)) {
                total += 1;
                if notes.len() < RECALL_FULL_LIMIT {
                    notes.push(entry);
                } else if more.len() < RECALL_MORE_LIMIT {
                    more.push(json!({
                        "id": entry.id,
                        "name": entry.summary_name(),
                        "description": entry.summary_description()
                    }));
                }
            }
            return Ok(json!({"notes": notes, "more": more, "total": total}));
        }
        Ok(json!(
            entries
                .iter()
                .map(|entry| json!({
                    "id": entry.id,
                    "scope": entry.scope,
                    "preview": preview(&entry.text),
                    "name": entry.summary_name(),
                    "description": entry.summary_description()
                }))
                .collect::<Vec<_>>()
        ))
    }
}

fn optional_string<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(format!("Enter {key} as text.")),
    }
}

fn matches_query(entry: &Entry, query: &str) -> bool {
    let name = entry
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| entry.display_name())
        .to_lowercase();
    let description = entry.description.as_deref().unwrap_or_default().to_lowercase();
    let text = entry.text.to_lowercase();
    query
        .split_whitespace()
        .all(|word| name.contains(word) || description.contains(word) || text.contains(word))
}
