//! `host_provision`: what the desktop app puts on a cloud machine once
//! per connection through Gateway. It carries what the SSH start's PREPARE script carried: the agents' credentials
//! (`agent.env`), their MCP servers (`mcp.json`), the block of instructions in each agent's instructions file
//! ([`crate::cloud_agent::CloudAgent::instructions`]), git's name and email, and the person's GitHub token. The app
//! builds it and the host applies it (nexus-host's `provision`). The call is idempotent.
//!
//! `{ "github": { "token", "name", "email" } | null, "agentEnv", "mcp", "instructions" }`:
//!
//! - `agentEnv`: the file's text. An empty text removes the file. Null keeps the file as it is: a device without the
//!   Claude token must not remove the one another device wrote.
//! - `mcp`: the file's text. Null removes the file.
//! - `instructions`: the block's Markdown. Null removes the block.
//! - `github.token`: null leaves the machine's `gh` sign-in as it is. `name` and `email` go to git's global config.

use serde::{Deserialize, Serialize};

/// The host call's name.
pub const CALL: &str = "host_provision";

/// The first line of the app's block in an agent's instructions file. The rest of the file is the person's.
pub const BLOCK_BEGIN: &str =
    "<!-- nexus-cloud:begin. Written by the Nexus app on every connect: changes inside this block are replaced. -->";
/// Its last line.
pub const BLOCK_END: &str = "<!-- nexus-cloud:end -->";

/// The call's arguments.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProvision {
    #[serde(default)]
    pub github: Option<Github>,
    #[serde(default)]
    pub agent_env: Option<String>,
    #[serde(default)]
    pub mcp: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
}

/// The person's GitHub sign-in and git identity.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Github {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

/// `Some(..)` for a value that is there, so a secret never reaches a log through `{:?}`.
fn presence(value: &Option<String>) -> Option<&'static str> {
    value.as_ref().map(|_| "..")
}

/// Says which parts are there, never their values: `agent_env` holds tokens, `mcp` server secrets.
impl std::fmt::Debug for HostProvision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostProvision")
            .field("github", &self.github)
            .field("agent_env", &presence(&self.agent_env))
            .field("mcp", &presence(&self.mcp))
            .field("instructions", &presence(&self.instructions))
            .finish()
    }
}

/// The token shows only as there or not.
impl std::fmt::Debug for Github {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Github")
            .field("token", &presence(&self.token))
            .field("name", &self.name)
            .field("email", &self.email)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn debug_never_shows_the_token_or_the_agent_env() {
        let provision = HostProvision {
            github: Some(Github {
                token: Some("tok_secret".into()),
                name: Some("Ada".into()),
                email: None,
            }),
            agent_env: Some("CLAUDE_CODE_OAUTH_TOKEN=env_secret\n".into()),
            mcp: Some("{\"key\":\"mcp_secret\"}".into()),
            instructions: None,
        };
        let shown = format!("{provision:?}");
        for secret in ["tok_secret", "env_secret", "mcp_secret"] {
            assert!(!shown.contains(secret), "{shown}");
        }
        assert!(shown.contains("Ada"), "{shown}");
    }

    #[test]
    fn reads_the_contracts_shape() {
        let read: HostProvision = serde_json::from_value(json!({
            "github": { "token": "gho_1", "name": "Ada", "email": "ada@example.com" },
            "agentEnv": "CLAUDE_CODE_OAUTH_TOKEN=x\n",
            "mcp": null,
            "instructions": "# Hi",
        }))
        .unwrap_or_default();
        assert_eq!(read.github.and_then(|g| g.token).as_deref(), Some("gho_1"));
        assert_eq!(read.agent_env.as_deref(), Some("CLAUDE_CODE_OAUTH_TOKEN=x\n"));
        assert_eq!(read.mcp, None);
        let empty: HostProvision = serde_json::from_value(json!({ "github": null })).unwrap_or_default();
        assert_eq!(empty, HostProvision::default());
        assert_eq!(
            serde_json::to_value(HostProvision::default()).unwrap_or_default(),
            json!({ "github": null, "agentEnv": null, "mcp": null, "instructions": null })
        );
    }
}
