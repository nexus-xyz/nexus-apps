//! A server's check, for "Test connection" and its settings: what nexus-remote's probe read there, in the shape
//! Nexus GPUI's Servers page shows.

use nexus_client::agent_entries::Agent as ClientAgent;
use nexus_local_agents::agents::local;
use nexus_local_agents::{LocalAgent, read_version};
use nexus_remote::probe::Probe;
use serde::Serialize;

/// What a server has: the system, the host's Nexus sign-in, and Claude Code and Codex there in the same shape as this
/// computer's (`local_agents`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCheck {
    pub os: String,
    pub arch: String,
    /// The Nexus host runs there (Linux, x86-64 or arm64).
    pub supported: bool,
    /// The host there is signed in to Nexus.
    pub nexus_signed_in: bool,
    /// The host starts when the server boots; None where the server has no service manager the host uses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starts_on_boot: Option<bool>,
    pub agents: Vec<LocalAgent>,
    /// The server was slow and some checks did not finish: why, for the person. The rest is what it read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete: Option<String>,
}

impl ServerCheck {
    pub fn of(probe: &Probe) -> ServerCheck {
        let agents = nexus_remote::probe::AGENTS
            .into_iter()
            .filter_map(ClientAgent::parse)
            .map(
                |agent| match probe.agents.iter().find(|found| found.agent == agent.as_str()) {
                    None => LocalAgent {
                        agent: agent.as_str().into(),
                        installed: false,
                        path: None,
                        version: None,
                        login: None,
                        plan: None,
                    },
                    Some(found) => {
                        let (login, plan) = local(agent).read_status(&found.status, "");
                        LocalAgent {
                            agent: agent.as_str().into(),
                            installed: true,
                            path: Some(found.path.clone()),
                            version: read_version(&found.version),
                            login: Some(login),
                            plan,
                        }
                    }
                },
            )
            .collect();
        ServerCheck {
            os: probe.os.clone(),
            arch: probe.arch.clone(),
            supported: probe.supported(),
            nexus_signed_in: probe.nexus,
            starts_on_boot: probe.boot,
            agents,
            incomplete: probe.incomplete.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_server_check_reads_the_agents_answers() {
        let probe = Probe {
            os: "Linux".into(),
            arch: "x86_64".into(),
            nexus: true,
            boot: Some(false),
            agents: vec![nexus_remote::probe::AgentProbe {
                agent: "claude".into(),
                path: "/usr/local/bin/claude".into(),
                version: "2.1.300 (Claude Code)".into(),
                status: r#"{"loggedIn": true, "authMethod": "claude.ai", "subscriptionType": "max"}"#.into(),
            }],
            incomplete: None,
        };
        let check = serde_json::to_value(ServerCheck::of(&probe)).unwrap_or_default();
        assert!(check.get("incomplete").is_none());
        let slow = Probe {
            incomplete: Some("box was slow to answer.".into()),
            ..probe.clone()
        };
        let slow = serde_json::to_value(ServerCheck::of(&slow)).unwrap_or_default();
        assert_eq!(slow["incomplete"], "box was slow to answer.");
        assert_eq!(check["supported"], true);
        assert_eq!(check["nexusSignedIn"], true);
        assert_eq!(check["startsOnBoot"], false);
        assert_eq!(check["agents"][0]["login"], "plan");
        assert_eq!(check["agents"][0]["version"], "2.1.300");
        assert_eq!(check["agents"][1], json!({ "agent": "codex", "installed": false }));
    }
    #[test]
    fn server_status_uses_probe_order_and_shared_sign_in_readers() {
        let probe = Probe {
            agents: vec![
                nexus_remote::probe::AgentProbe {
                    agent: "antigravity".into(),
                    ..Default::default()
                },
                nexus_remote::probe::AgentProbe {
                    agent: "codex".into(),
                    path: "/bin/codex".into(),
                    version: "codex-cli 0.163.0".into(),
                    status: "Logged in using ChatGPT".into(),
                },
            ],
            ..Default::default()
        };
        let check = ServerCheck::of(&probe);
        assert_eq!(
            check
                .agents
                .iter()
                .map(|agent| agent.agent.as_str())
                .collect::<Vec<_>>(),
            nexus_remote::probe::AGENTS
        );
        assert!(!check.agents[0].installed);
        assert_eq!(check.agents[1].login, Some(nexus_local_agents::Login::Plan));
        assert_eq!(check.agents[1].version.as_deref(), Some("0.163.0"));
        assert_eq!(check.agents[1].plan, None);
    }
}
