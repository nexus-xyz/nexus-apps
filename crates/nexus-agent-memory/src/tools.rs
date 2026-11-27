//! Shared tool contracts for the local Desktop and CLI MCP servers.

use crate::{Repository, Source, Store, preview};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const AGENT_ENV: &str = "NEXUS_MEMORY_AGENT";
pub const SESSION_ENV: &str = "NEXUS_MEMORY_SESSION";
pub const ROOT_ENV: &str = "NEXUS_MEMORY_ROOT";
pub const PERSONAL_REFUSAL: &str = "Only the person can save personal memory. They can add it in Desktop Shared memory settings or run nexus memory add.";

pub const NAMES: [&str; 3] = ["remember", "recall", "forget"];

/// Instructions for local launches that attach the Shared memory tools.
pub fn launch_guidance(server: &str, claude: bool) -> String {
    let mut guidance = format!(
        "# Nexus Shared memory\n\
         Shared memory is on. Use the `{server}` MCP server by default for memory across sessions.\n\
         When the person asks to remember something, save a memory, or add a memory entry, call `remember` for this repository before confirming that it was saved.\n\
         Use `recall` to find stored notes or read them in full, including notes omitted from the startup index. Use `forget` when the person asks to remove a stored note.\n\
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
                    "scope": {"type": "string", "enum": ["repository"]}
                },
                "required": ["text"]
            },
            "annotations": {"readOnlyHint": false}
        }),
        json!({
            "name": "recall",
            "description": "List shared memory entries or read the full text of one entry by ID.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}},
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
                self.store
                    .remember(scope, self.source.clone(), text)
                    .map(|entry| json!({"id": entry.id}))
            }
            "recall" => {
                let id = if args["id"].is_null() { None } else { Some(id()?) };
                return self
                    .store
                    .entries(repository)
                    .map(|entries| match id {
                        Some(id) => entries
                            .into_iter()
                            .find(|entry| entry.id == id)
                            .map(|entry| json!(entry)),
                        None => Some(json!(
                            entries
                                .into_iter()
                                .map(|entry| json!({
                                    "id": entry.id, "scope": entry.scope, "preview": preview(&entry.text)
                                }))
                                .collect::<Vec<_>>()
                        )),
                    })
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "That shared memory entry was not found.".into());
            }
            "forget" => self
                .store
                .forget(repository, id()?)
                .map(|deleted| json!({"deleted": deleted})),
            _ => return Err("Unknown shared memory tool.".into()),
        };
        result.map_err(|error| error.to_string())
    }
}
