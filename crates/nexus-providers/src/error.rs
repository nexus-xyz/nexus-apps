//! Why a provider cannot run, in words a person reads.

use crate::{KeyVendor, ProviderKind};

/// Why a provider cannot run. Each message says what does not work and what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    KeyMissing(KeyVendor),
    KeychainUnread(String),
    AgentMissing(nexus_agents::Agent),
    SignedOut(nexus_agents::Agent),
    WrongLogin(nexus_agents::Agent),
    NotOffered {
        agent: nexus_agents::Agent,
        kind: ProviderKind,
    },
    Policy(String),
    BadBaseUrl(String),
    Unreachable {
        endpoint: String,
    },
    /// A launch step refused, with the text it gave the person.
    Refused(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::KeyMissing(vendor) => write!(
                f,
                "No {} key is saved on this computer. Add one in Settings under Local providers.",
                vendor.name()
            ),
            ProviderError::KeychainUnread(text) | ProviderError::Policy(text) | ProviderError::Refused(text) => {
                f.write_str(text)
            }
            ProviderError::AgentMissing(agent) => {
                let entry = agent.entry();
                write!(
                    f,
                    "{} is not installed on this computer. Install it with `{}`, then try again.",
                    entry.identity.name, entry.install.command
                )
            }
            ProviderError::SignedOut(agent) => {
                let entry = agent.entry();
                write!(
                    f,
                    "{} is not signed in. Sign in with `{}`, then try again.",
                    entry.identity.name, entry.sign_in.login
                )
            }
            ProviderError::WrongLogin(agent) => {
                let entry = agent.entry();
                write!(
                    f,
                    "{} is signed in with an API key, not a plan. Sign in to your plan with `{}`, or use the key as a key provider.",
                    entry.identity.name, entry.sign_in.login
                )
            }
            ProviderError::NotOffered { agent, kind } => {
                write!(
                    f,
                    "{} does not run on {}.",
                    agent.entry().identity.name,
                    kind.entry().name
                )
            }
            ProviderError::BadBaseUrl(url) => {
                write!(
                    f,
                    "`{url}` is not a server address. Use an http:// or https:// address."
                )
            }
            ProviderError::Unreachable { endpoint } => write!(
                f,
                "{endpoint} did not answer. Check that it is running and reachable, then try again."
            ),
        }
    }
}

impl std::error::Error for ProviderError {}

/// Whether a provider can run now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    Ready,
    Setup(ProviderError),
    Unreachable { endpoint: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_agents::Agent;

    fn all() -> Vec<ProviderError> {
        vec![
            ProviderError::KeyMissing(KeyVendor::Openai),
            ProviderError::KeychainUnread("The keychain is locked.".into()),
            ProviderError::AgentMissing(Agent::Claude),
            ProviderError::SignedOut(Agent::Codex),
            ProviderError::WrongLogin(Agent::Claude),
            ProviderError::NotOffered {
                agent: Agent::Claude,
                kind: ProviderKind::ApiKey,
            },
            ProviderError::Policy("Your organization blocks this provider.".into()),
            ProviderError::BadBaseUrl("ftp://x".into()),
            ProviderError::Unreachable {
                endpoint: "localhost:1234".into(),
            },
            ProviderError::Refused("The launch was refused.".into()),
        ]
    }

    #[test]
    fn every_message_is_a_plain_sentence() {
        for error in all() {
            let text = error.to_string();
            assert!(!text.is_empty());
            assert!(text.ends_with('.'), "{text}");
            assert!(!text.contains("  "), "{text}");
        }
    }

    #[test]
    fn agent_messages_name_the_agent() {
        for agent in Agent::ALL {
            let name = agent.entry().identity.name;
            for error in [
                ProviderError::AgentMissing(agent),
                ProviderError::SignedOut(agent),
                ProviderError::WrongLogin(agent),
                ProviderError::NotOffered {
                    agent,
                    kind: ProviderKind::NexusCloud,
                },
            ] {
                assert!(error.to_string().starts_with(name), "{error}");
            }
        }
        assert_eq!(
            ProviderError::NotOffered {
                agent: Agent::Claude,
                kind: ProviderKind::ApiKey
            }
            .to_string(),
            "Claude Code does not run on BYO key."
        );
    }
}
