//! What a session's Connection pane says (desktop's `connection-model.ts` and `connection-view.tsx`), for every app that
//! draws it. Labels use the selected session's own evidence, never global account or model defaults: what is not
//! reported says so. [`crate::connection`] reads the observations; this module only words them.

use crate::driver;
use crate::event::{PermissionMode, SessionAgent, SessionConnection, StoredEvent, ThreadSummary, Via, WbEvent};
use crate::harness::harness;

/// A fact the runtime did not report.
pub const UNAVAILABLE: &str = "Not reported";
/// How often an open pane asks the live process again (the Nexus app's 15 s).
pub const REFRESH_SECS: u64 = 15;
/// What the pane says when that ask fails; the last reported details stay.
pub const REFRESH_FAILED: &str = "Connection refresh is unavailable. The last reported details remain below.";
pub const HOST_DISCONNECTED: &str =
    "The execution host is disconnected. Its saved observations are not live connection evidence.";
pub const MCP_NOTE: &str =
    "Enabled configuration does not prove a connected server. MCP changes apply on the next agent start.";
pub const SKILLS_NOTE: &str = "Discovery does not prove that a skill's full instructions were loaded or invoked.";
pub const RESTART_NOTE: &str =
    "Restart ends this process. Your next message resumes the conversation and reads configuration again.";

/// The session's connection: the live process's, else the last one its log reported (`lastConnection`).
pub fn last_connection<'a>(thread: &'a ThreadSummary, events: &'a [StoredEvent]) -> Option<&'a SessionConnection> {
    if let Some(connection) = thread.capabilities.as_ref().and_then(|caps| caps.connection.as_deref()) {
        return Some(connection);
    }
    let boundary = events
        .iter()
        .filter(|e| matches!(e.event, WbEvent::AgentSwitched { .. }))
        .map(|e| e.seq)
        .max()
        .unwrap_or(0);
    events
        .iter()
        .filter(|stored| stored.seq > boundary)
        .filter_map(|stored| match &stored.event {
            WbEvent::Connection { connection } => Some((stored.seq, connection.as_ref())),
            _ => None,
        })
        .max_by_key(|(seq, _)| *seq)
        .map(|(_, connection)| connection)
}

/// Where the session runs, as the pane names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub label: String,
    /// Its observations can be live: this computer, or a connected server.
    pub connected: bool,
}

/// The session's machine, when it is on one (`executionHost`): its name, and the host's own name while connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Machine<'a> {
    pub name: Option<&'a str>,
    pub connected_as: Option<&'a str>,
}

pub fn execution_host(machine: Option<Machine<'_>>) -> Host {
    match machine {
        None => Host {
            label: "Local".into(),
            connected: true,
        },
        Some(Machine {
            connected_as: Some(host),
            ..
        }) => Host {
            label: format!("Remote · {host}"),
            connected: true,
        },
        Some(Machine { name, .. }) => Host {
            label: format!("Remote · {}", name.unwrap_or("Host unavailable")),
            connected: false,
        },
    }
}

/// The model the process reported, else the one asked for, marked so (`observedModel`).
pub fn observed_model(thread: &ThreadSummary, connection: Option<&SessionConnection>) -> String {
    if let Some(model) = connection.and_then(|connection| connection.model.as_deref()) {
        return model.into();
    }
    match &thread.model {
        Some(model) => format!("{model} (requested)"),
        None => "Model not reported".into(),
    }
}

/// The details come from the running process (`connectionIsLive`): its generation is the live one, on a host that is
/// connected.
pub fn is_live(thread: &ThreadSummary, connection: Option<&SessionConnection>, host_connected: bool) -> bool {
    let live = thread
        .capabilities
        .as_ref()
        .and_then(|caps| caps.connection.as_ref())
        .map(|live| live.generation.as_str());
    connection.is_some_and(|connection| host_connected && live == Some(connection.generation.as_str()))
}

/// The payment route as people see it.
pub fn route(via: Via) -> &'static str {
    match via {
        Via::Nexus => "API",
        Via::Subscription => "Plan",
    }
}

/// A permission mode under the name people see (`PERMISSION_MODES`).
pub fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "Ask first",
        PermissionMode::Auto => "Auto",
        PermissionMode::AcceptEdits => "Auto-accept edits",
        PermissionMode::Plan => "Plan only",
        PermissionMode::BypassPermissions => "Full access",
    }
}

fn session_route(agent: SessionAgent, via: Via) -> &'static str {
    match via {
        Via::Subscription => agent.configured_provider().unwrap_or_else(|| route(via)),
        Via::Nexus => route(via),
    }
}

/// The pane's one-line summary: agent, route and host.
pub fn headline(thread: &ThreadSummary, host: &Host) -> String {
    format!(
        "{} · {} · {}",
        driver::label(thread.agent),
        session_route(thread.agent, thread.via),
        host.label
    )
}

/// What the heading's tag says.
pub fn state(live: bool, connection: Option<&SessionConnection>) -> &'static str {
    match (live, connection) {
        (true, _) => "Current process",
        (false, Some(_)) => "Last reported",
        (false, None) => "Unavailable",
    }
}

/// The facts, in the pane's order, as (label, value).
pub fn facts(
    thread: &ThreadSummary,
    connection: Option<&SessionConnection>,
    host: &Host,
) -> Vec<(&'static str, String)> {
    let reported = |value: Option<&String>| value.cloned().unwrap_or_else(|| UNAVAILABLE.into());
    let attribution = connection.map(|connection| &connection.attribution);
    let account = connection.and_then(|connection| connection.account.as_ref());
    let pending = thread.pending.as_ref();
    vec![
        ("Execution host", host.label.clone()),
        ("Agent version", reported(connection.and_then(|c| c.version.as_ref()))),
        ("Reported model", reported(connection.and_then(|c| c.model.as_ref()))),
        (
            "Reported provider",
            reported(connection.and_then(|c| c.provider.as_ref())),
        ),
        (
            "Session route at launch",
            connection
                .and_then(|c| c.via)
                .map_or(UNAVAILABLE.into(), |via| session_route(thread.agent, via).into()),
        ),
        ("Saved route choice", session_route(thread.agent, thread.via).into()),
        (
            "Saved model choice",
            thread.model.clone().unwrap_or_else(|| "Organization default".into()),
        ),
        (
            "Pending model",
            match pending.and_then(|pending| pending.model.as_ref()) {
                None => "None".into(),
                Some(model) => model.clone().unwrap_or_else(|| "Organization default".into()),
            },
        ),
        (
            "Saved effort choice",
            thread
                .effort
                .map_or("Model default".into(), |effort| effort.as_str().into()),
        ),
        ("Reported effort", reported(connection.and_then(|c| c.effort.as_ref()))),
        (
            "Pending effort",
            match pending.and_then(|pending| pending.effort) {
                None => "None".into(),
                Some(effort) => effort.map_or("Model default".into(), |effort| effort.as_str().into()),
            },
        ),
        (
            "Pending permission mode",
            pending
                .and_then(|pending| pending.permission_mode)
                .map_or("None".into(), |mode| mode.as_str().into()),
        ),
        (
            "Saved permission mode",
            thread
                .agent
                .permission_settings()
                .unwrap_or_else(|| mode_label(thread.permission_mode))
                .into(),
        ),
        (
            "Reported permissions",
            reported(connection.and_then(|c| c.permissions.as_ref())),
        ),
        (
            "Vendor account",
            match account {
                Some(account) => account
                    .email
                    .clone()
                    .unwrap_or_else(|| format!("{} identity not reported", account.kind)),
                None => UNAVAILABLE.into(),
            },
        ),
        (
            "Vendor plan",
            reported(account.and_then(|account| account.plan.as_ref())),
        ),
        (
            "Configured account ID",
            reported(connection.and_then(|c| c.selected_account.as_ref())),
        ),
        (
            "Nexus organization",
            reported(attribution.and_then(|a| a.organization.as_ref())),
        ),
        ("Nexus team", reported(attribution.and_then(|a| a.team.as_ref()))),
        ("Gateway origin", reported(attribution.and_then(|a| a.gateway.as_ref()))),
        (
            "Payer class at launch",
            match attribution.and_then(|a| a.access.as_deref()) {
                Some("byok") => "BYOK".into(),
                Some("nexus") => "Nexus-supplied key".into(),
                Some("subscription") => thread.agent.configured_provider().unwrap_or("Vendor plan").into(),
                _ => UNAVAILABLE.into(),
            },
        ),
        ("Billing owner", "Billing owner not reported".into()),
    ]
}

/// The quiet lines under the facts, in the pane's order.
pub fn notes(thread: &ThreadSummary, connection: Option<&SessionConnection>) -> Vec<&'static str> {
    let via = connection.and_then(|connection| connection.via).unwrap_or(thread.via);
    let api_key = connection
        .and_then(|connection| connection.account.as_ref())
        .is_some_and(|account| account.kind == "apiKey");
    let mut notes = Vec::new();
    let configured_provider = thread.agent.configured_provider().is_some();
    if via == Via::Subscription && api_key && !configured_provider {
        notes
            .push("The runtime reports API-key authentication for this Plan session. Its payment route is unverified.");
    }
    if thread.pending.is_some() {
        notes.push(
            "Settings changes are pending. Requested settings do not prove which settings the running turn uses.",
        );
    }
    notes.push(match via {
        Via::Nexus => {
            "API configures model calls through Nexus Gateway. This route alone does not identify the provider key or \
             billing owner."
        }
        Via::Subscription if configured_provider => {
            "The agent uses its configured provider. Model calls go directly to that provider."
        }
        Via::Subscription => {
            "Plan intends vendor model calls outside Gateway's model proxy. A reported provider or API-key account can \
             differ. Nexus still records session and usage metadata."
        }
    });
    notes.push(
        "The payer class describes the Gateway session at launch. It does not prove which key pays for later calls.",
    );
    if !configured_provider {
        notes.push("Plan only controls agent permissions. It is separate from the Plan payment route.");
    }
    notes
}

/// The MCP heading's tag.
pub fn mcp_state(connection: Option<&SessionConnection>, live: bool) -> &'static str {
    match connection.and_then(|connection| connection.mcp.as_ref()) {
        None => "Unavailable",
        Some(_) if live => "Reported by this process",
        Some(_) => "Last reported",
    }
}

/// What an MCP list says when there is no list to show.
pub fn mcp_empty(connection: Option<&SessionConnection>) -> Option<String> {
    match connection.and_then(|connection| connection.mcp.as_ref()) {
        Some(servers) if servers.is_empty() => Some("This runtime reported no MCP servers.".into()),
        Some(_) => None,
        None => Some(
            connection
                .and_then(|connection| connection.mcp_unavailable.clone())
                .unwrap_or_else(|| "MCP connection health is unavailable from this runtime.".into()),
        ),
    }
}

/// An inventory (configured MCP, skills, plugins): its names, or the line that stands for them.
pub fn inventory(names: Option<&[String]>, unavailable: &'static str) -> Result<Vec<String>, &'static str> {
    match names {
        None => Err(unavailable),
        Some([]) => Err("None reported."),
        Some(names) => Ok(names.to_vec()),
    }
}

/// The inventories, in the pane's order, as (heading, names or the line in their place).
pub fn inventories(
    thread: &ThreadSummary,
    connection: Option<&SessionConnection>,
) -> [(&'static str, Result<Vec<String>, &'static str>); 3] {
    [
        (
            "Enabled by Desktop at agent start",
            inventory(
                connection.and_then(|c| c.attribution.configured_mcp.as_deref()),
                "The execution host did not report Desktop-injected MCP configuration.",
            ),
        ),
        (
            if harness(thread.agent).session_features().discovers_skills {
                "Discovered skills"
            } else {
                "Reported skills"
            },
            inventory(
                connection.and_then(|c| c.skills.as_deref()),
                "Skill discovery is unavailable from this runtime.",
            ),
        ),
        (
            "Reported plugins",
            inventory(
                connection.and_then(|c| c.plugins.as_deref()),
                "Plugin inventory is unavailable from this runtime.",
            ),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{AgentCapabilities, RuntimeAccount, SessionAttribution};

    fn thread(agent: &str, via: &str) -> ThreadSummary {
        serde_json::from_value(serde_json::json!({
            "id": "fixture", "projectId": "p", "title": "", "agent": agent, "model": null,
            "permissionMode": "plan", "via": via, "status": "idle", "costUsd": 0.0, "nexusSessions": [],
            "contextTokens": null, "contextWindow": null, "createdAt": 0, "updatedAt": 0, "archived": false,
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    fn fact<'a>(facts: &'a [(&str, String)], label: &str) -> &'a str {
        facts
            .iter()
            .find(|(name, _)| *name == label)
            .map_or("", |(_, value)| value.as_str())
    }

    #[test]
    fn shows_unavailable_identity_and_inventory_instead_of_defaults() {
        let thread = thread("codex", "nexus");
        let connection = last_connection(&thread, &[]);
        let host = execution_host(None);
        let facts = facts(&thread, connection, &host);
        assert_eq!(fact(&facts, "Agent version"), UNAVAILABLE);
        assert_eq!(fact(&facts, "Billing owner"), "Billing owner not reported");
        assert_eq!(fact(&facts, "Saved permission mode"), "Plan only");
        assert_eq!(mcp_state(connection, false), "Unavailable");
        let [_, _, plugins] = inventories(&thread, connection);
        assert_eq!(plugins.1, Err("Plugin inventory is unavailable from this runtime."));
        assert!(
            notes(&thread, connection)
                .iter()
                .any(|note| note.contains("This route alone does not identify the provider key or billing owner"))
        );
        assert_eq!(state(false, connection), "Unavailable");
        assert_eq!(headline(&thread, &host), "Codex · API · Local");
    }

    #[test]
    fn exposes_an_api_key_account_on_plan_without_claiming_vendor_plan_payment() {
        let mut thread = thread("codex", "subscription");
        thread.capabilities = Some(Box::new(AgentCapabilities {
            connection: Some(Box::new(SessionConnection {
                generation: "synthetic".into(),
                via: Some(Via::Subscription),
                provider: Some("synthetic-provider".into()),
                attribution: SessionAttribution {
                    access: Some("byok".into()),
                    ..SessionAttribution::default()
                },
                account: Some(RuntimeAccount {
                    kind: "apiKey".into(),
                    email: None,
                    plan: None,
                }),
                ..SessionConnection::default()
            })),
            ..AgentCapabilities::default()
        }));
        let connection = last_connection(&thread, &[]);
        let facts = facts(&thread, connection, &execution_host(None));
        assert_eq!(fact(&facts, "Reported provider"), "synthetic-provider");
        assert_eq!(fact(&facts, "Payer class at launch"), "BYOK");
        assert_eq!(fact(&facts, "Vendor account"), "apiKey identity not reported");
        assert!(notes(&thread, connection)[0].contains("Its payment route is unverified"));
        assert_eq!(mcp_state(connection, true), "Unavailable");
        assert!(is_live(&thread, connection, true));
        assert!(!is_live(&thread, connection, false));
        assert_eq!(state(true, connection), "Current process");
    }

    #[test]
    fn the_last_reported_connection_comes_from_the_log_when_no_process_runs() {
        let thread = thread("claude", "nexus");
        let event = |seq: i64, generation: &str| StoredEvent {
            thread_id: "fixture".into(),
            seq,
            at: 0,
            event: WbEvent::Connection {
                connection: Box::new(SessionConnection {
                    generation: generation.into(),
                    model: Some("claude-x".into()),
                    mcp: Some(vec![]),
                    ..SessionConnection::default()
                }),
            },
        };
        let events = [event(9, "newer"), event(3, "older")];
        let connection = last_connection(&thread, &events);
        assert_eq!(connection.map(|c| c.generation.as_str()), Some("newer"));
        assert!(!is_live(&thread, connection, true));
        assert_eq!(state(false, connection), "Last reported");
        assert_eq!(observed_model(&thread, connection), "claude-x");
        assert_eq!(mcp_state(connection, false), "Last reported");
        assert_eq!(
            mcp_empty(connection).as_deref(),
            Some("This runtime reported no MCP servers.")
        );
        let mut asked = thread.clone();
        asked.model = Some("opus".into());
        assert_eq!(observed_model(&asked, None), "opus (requested)");
        assert_eq!(observed_model(&thread, None), "Model not reported");
    }

    #[test]
    fn a_server_is_live_only_while_connected() {
        let connected = execution_host(Some(Machine {
            name: Some("box"),
            connected_as: Some("box.local"),
        }));
        assert_eq!(connected.label, "Remote · box.local");
        assert!(connected.connected);
        let gone = execution_host(Some(Machine {
            name: None,
            connected_as: None,
        }));
        assert_eq!(gone.label, "Remote · Host unavailable");
        assert!(!gone.connected);
        assert_eq!(inventory(Some(&[]), "x"), Err("None reported."));
    }
}
