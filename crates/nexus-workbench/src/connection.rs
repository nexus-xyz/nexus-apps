//! Session-bound observations. Read-only native requests never create turns or expose raw configuration.
use super::*;
use crate::event::{McpHealth, RuntimeAccount, SessionConnection};
use crate::harness::{McpStatus, harness};

pub const FEATURE: &str = "session-connection";
const INSPECT_WAIT: Duration = Duration::from_secs(2);
struct PendingInspection<'a> {
    live: &'a Live,
    request: String,
}
impl Drop for PendingInspection<'_> {
    fn drop(&mut self) {
        self.live.state().task_asks.remove(&self.request);
        self.live.tell(Say::ForgetInspection {
            request: self.request.clone(),
        });
    }
}

/// Display the origin only. Credentials, paths, query parameters and fragments never reach the webview.
pub fn gateway_origin(raw: &str) -> Option<String> {
    let (scheme, rest) = raw.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    if authority.is_empty()
        || !authority
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-:[]".contains(&c))
    {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
}

fn label(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(|text| text.chars().filter(|c| !c.is_control()).take(256).collect::<String>())
        .filter(|text| !text.is_empty())
}
fn names(value: Option<&Value>) -> Option<Vec<String>> {
    let values = value?.as_array()?;
    Some(
        values
            .iter()
            .filter_map(|value| {
                label(if value.is_string() {
                    Some(value)
                } else {
                    value.get("name")
                })
            })
            .take(1000)
            .collect(),
    )
}

fn health(server: &Value, format: McpStatus) -> Option<McpHealth> {
    let name = label(server.get("name"))?;
    let status = match format.reported(server) {
        Some("connected" | "ready") => "connected",
        Some("starting" | "pending") => "pending",
        Some("failed") => "failed",
        Some("needs-auth" | "authenticationRequired") => "needs-auth",
        Some("disabled") => "disabled",
        Some("notStarted" | "cancelled" | "disconnected") => "disconnected",
        _ => "unavailable",
    };
    // Runtime errors can contain secrets. Keep actionable state explanations instead of raw diagnostics.
    let detail = match status {
        "failed" => Some("The runtime reported a connection failure. Check this server's settings.".into()),
        "needs-auth" => Some("This server requires authentication on the execution host.".into()),
        "unavailable" => Some("The runtime did not report connection health. Configuration or authentication alone does not prove a connection.".into()),
        _ => None,
    };
    Some(McpHealth {
        name,
        status: status.into(),
        detail,
    })
}

/// Only documented display fields survive. Raw MCP configuration and credentials are discarded here.
pub(crate) fn apply(snapshot: &mut SessionConnection, agent: SessionAgent, observation: &Value) {
    if let Some(account) = observation.get("account") {
        snapshot.account = account.as_object().and_then(|_| {
            let kind = label(account.get("type"))?;
            if !["apiKey", "chatgpt", "amazonBedrock"].contains(&kind.as_str()) {
                return None;
            }
            Some(RuntimeAccount {
                kind,
                email: label(account.get("email")),
                plan: label(account.get("planType")),
            })
        });
    }
    let format = harness(agent).mcp_status();
    if let Some(mcp) = observation.get("mcp") {
        snapshot.mcp = mcp.as_array().map(|servers| {
            servers
                .iter()
                .filter_map(|server| health(server, format))
                .take(1000)
                .collect()
        });
        snapshot.mcp_unavailable = snapshot
            .mcp
            .is_none()
            .then(|| "This runtime did not report MCP connection health.".into());
    }
    if let Some(server) = observation.get("mcpUpdate")
        && let Some(update) = health(server, format)
    {
        let servers = snapshot.mcp.get_or_insert_with(Vec::new);
        if let Some(previous) = servers.iter_mut().find(|previous| previous.name == update.name) {
            *previous = update;
        } else {
            servers.push(update);
        }
    }
    if observation.get("skills").is_some() {
        snapshot.skills = names(observation.get("skills"));
    }
    if observation.get("plugins").is_some() {
        snapshot.plugins = names(observation.get("plugins"));
    }
    if observation.get("provider").is_some() {
        snapshot.provider = label(observation.get("provider"));
    }
    if observation.get("effort").is_some() {
        snapshot.effort = label(observation.get("effort"));
    }
    if observation.get("permissions").is_some() {
        snapshot.permissions = label(observation.get("permissions"));
    }
    if observation.get("fastOff").is_some() {
        snapshot.fast_off = label(observation.get("fastOff"));
    }
}

impl Workbench {
    /// Persist safe observations for paged history. A replaced or stopping process cannot publish late answers.
    pub(crate) fn publish_connection(&self, thread_id: &str, live: &Live) -> Result<()> {
        let Some(current) = self.live(thread_id) else {
            return Ok(());
        };
        if !std::ptr::eq(current.as_ref(), live) {
            return Ok(());
        }
        let connection = {
            let mut state = live.state();
            if state.done || state.stop.is_some() {
                return Ok(());
            }
            let mut next = state.connection.clone();
            if let Some(previous) = &state.last_connection {
                next.reported_at = previous.reported_at;
                if &next == previous {
                    return Ok(());
                }
            }
            next.reported_at = now_ms();
            state.connection.reported_at = next.reported_at;
            state.last_connection = Some(next.clone());
            next
        };
        self.append(
            thread_id,
            WbEvent::Connection {
                connection: Box::new(connection),
            },
        )?;
        self.change_at(thread_id, false, |_| {})?;
        Ok(())
    }

    async fn inspect(
        &self,
        thread_id: &str,
        live: &Live,
        field: &'static str,
        cursor: Option<String>,
    ) -> Result<Value> {
        let request = format!("status-{}", live.next_request());
        let (tx, rx) = oneshot::channel();
        {
            let mut state = live.state();
            if !state.initialized || state.done || state.stop.is_some() {
                return Err("The agent is not running.".into());
            }
            state.task_asks.insert(request.clone(), tx);
        }
        let _pending = PendingInspection {
            live,
            request: request.clone(),
        };
        self.outs(
            thread_id,
            live,
            live.tell(Say::Inspect {
                request: request.clone(),
                field,
                cursor,
            }),
        )?;
        match tokio::time::timeout(INSPECT_WAIT, rx).await {
            Ok(Ok(answer)) => answer,
            _ => {
                live.state().task_asks.remove(&request);
                Err("The runtime did not report this metadata in time.".into())
            }
        }
    }

    /// Query the selected process only. Unsupported operations fail as unavailable, never as model prompts.
    pub async fn connection(&self, thread_id: &str) -> Result<SessionConnection> {
        let live = self
            .live(thread_id)
            .ok_or("The agent is not running. Last reported details remain available.")?;
        let (account_revision, mcp_revision, skills_revision) = {
            let mut state = live.state();
            state.account_revision += 1;
            state.mcp_revision += 1;
            state.skills_revision += 1;
            (state.account_revision, state.mcp_revision, state.skills_revision)
        };
        let cwd = self
            .record(thread_id)
            .ok()
            .and_then(|record| self.workdir(&record))
            .map(|path| path.to_string_lossy().into_owned());
        let mcp = async {
            let page_key = harness(live.agent).mcp_status().page_key;
            let mut all = Vec::new();
            let mut cursor = None;
            let mut cursors = HashSet::new();
            for _ in 0..20 {
                let result = self.inspect(thread_id, &live, "mcp", cursor.clone()).await?;
                let servers = result.get(page_key);
                let Some(servers) = servers.and_then(Value::as_array) else {
                    return Err("The runtime returned no MCP health observation.".into());
                };
                all.extend(servers.iter().cloned());
                let next = result
                    .get("nextCursor")
                    .and_then(Value::as_str)
                    .filter(|cursor| !cursor.is_empty())
                    .map(String::from);
                if next.is_none() {
                    return Ok::<_, String>(Value::Array(all));
                }
                if !cursors.insert(next.clone()) {
                    return Err("The runtime repeated its MCP page cursor.".into());
                }
                cursor = next;
            }
            Err("The runtime's MCP inventory is too large to read in one refresh.".into())
        };
        let features = harness(live.agent).session_features();
        let account = async {
            if !features.reads_account {
                return Err("Account identity is not reported by this adapter.".into());
            }
            self.inspect(thread_id, &live, "account", None).await
        };
        let skills = async {
            if !features.discovers_skills {
                return Ok(None);
            }
            let result = self.inspect(thread_id, &live, "skills", None).await?;
            let values = result
                .get("data")
                .and_then(Value::as_array)
                .and_then(|data| {
                    data.iter()
                        .find(|row| row.get("cwd").and_then(Value::as_str) == cwd.as_deref())
                })
                .and_then(|row| row.get("skills"));
            Ok::<_, String>(values.cloned())
        };
        let (mcp, account, skills) = tokio::join!(tokio::time::timeout(Duration::from_secs(5), mcp), account, skills);
        let observation = {
            let mut state = live.state();
            if state.mcp_revision == mcp_revision {
                match mcp {
                    Ok(Ok(servers)) => apply(&mut state.connection, live.agent, &serde_json::json!({"mcp":servers})),
                    _ => {
                        state.connection.mcp = None;
                        state.connection.mcp_unavailable = Some("MCP connection health is unavailable from this runtime. Try refreshing after the agent starts.".into());
                    }
                }
            }
            if state.account_revision == account_revision {
                match account {
                    Ok(account) => apply(
                        &mut state.connection,
                        live.agent,
                        &serde_json::json!({"account":account.get("account")}),
                    ),
                    Err(_) => state.connection.account = None,
                }
            }
            if state.skills_revision == skills_revision && features.discovers_skills {
                match skills {
                    Ok(Some(skills)) => apply(&mut state.connection, live.agent, &serde_json::json!({"skills":skills})),
                    _ => state.connection.skills = None,
                }
            }
            state.connection.clone()
        };
        if !self.live(thread_id).is_some_and(|current| Arc::ptr_eq(&current, &live)) || {
            let state = live.state();
            state.stop.is_some() || state.done
        } {
            return Err("The agent process changed. Refresh its connection details.".into());
        }
        self.publish_connection(thread_id, &live)?;
        Ok(observation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn synthetic_status_redacts_secrets_and_distinguishes_authentication_from_health() {
        assert_eq!(
            gateway_origin("https://synthetic:SECRET@gateway.example.test:8443/private?token=SECRET#fragment"),
            Some("https://gateway.example.test:8443".into())
        );
        assert!(gateway_origin("file:///synthetic").is_none());
        let mut snapshot = SessionConnection::default();
        apply(
            &mut snapshot,
            SessionAgent::Codex,
            &json!({
                "mcp":[{"name":"configured","runtimeStatus":null,"authStatus":"oAuth","tools":{"sample":{}}}],
                "mcpUpdate":{"name":"auth","status":"failed","failureReason":"reauthenticationRequired","error":"SECRET_DIAGNOSTIC"},
                "account":{"type":"chatgpt","email":"synthetic@example.test","planType":"pro","accessToken":"SECRET_TOKEN"},
                "skills":[{"name":"synthetic-check","path":"/private/synthetic"}],
                "provider":"openai"
            }),
        );
        let servers = snapshot.mcp.as_ref().expect("reported servers");
        assert_eq!(servers[0].status, "unavailable");
        assert_eq!(servers[1].status, "needs-auth");
        let display = serde_json::to_string(&snapshot).expect("safe snapshot");
        assert!(!display.contains("SECRET") && !display.contains("/private"));
        assert!(display.contains("synthetic@example.test"));
    }
    #[test]
    fn cancelled_synthetic_inspections_ignore_late_replies_in_both_adapters() {
        for agent in [SessionAgent::Claude, SessionAgent::Codex] {
            let mut protocol = driver::protocol(
                agent,
                &driver::Start {
                    mode: PermissionMode::Default,
                    model: None,
                    effort: None,
                    fast: false,
                    native_id: "synthetic-thread",
                    resume: false,
                    fork: false,
                    cwd: "/synthetic",
                    rewind: None,
                    rewind_required: false,
                    instructions: None,
                    prompt: None,
                    name: None,
                },
            );
            let outputs = protocol.say(Say::Inspect {
                request: "status-synthetic".into(),
                field: if agent == SessionAgent::Claude {
                    "mcp"
                } else {
                    "account"
                },
                cursor: None,
            });
            let line = outputs
                .iter()
                .find_map(|out| match out {
                    Out::Reply(line) => Some(line),
                    _ => None,
                })
                .expect("native inspection request");
            let wire: Value = serde_json::from_str(line).expect("JSON request");
            assert!(
                protocol
                    .say(Say::ForgetInspection {
                        request: "status-synthetic".into()
                    })
                    .is_empty()
            );
            let reply = if agent == SessionAgent::Claude {
                json!({"type":"control_response","response":{"subtype":"error","request_id":"status-synthetic","error":"synthetic late error"}})
            } else {
                json!({"id":wire["id"],"error":{"code":-32601,"message":"synthetic late error"}})
            };
            assert!(
                protocol.read(&reply.to_string()).is_empty(),
                "cancelled metadata cannot become activity or a notice"
            );
        }
    }
}
