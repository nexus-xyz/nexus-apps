//! The coding agents Nexus launches. Each agent is a type of its own ([`Claude`], [`Codex`], [`Antigravity`]), and the
//! closed [`Agent`] enum names them in ids, serde and dispatch. Each agent's type implements [`AgentT`], which gives
//! its plain facts, its [`AgentEntry`].
//!
//! A session's agent is the workbench's `SessionAgent` (`nexus-workbench/src/event.rs`). It adds Nexus's own agent,
//! which only sessions run.
//!
//! A domain crate defines its own trait the same way, implements it for each agent's type, and reaches the
//! implementation with [`of!`]. A new variant then fails every domain's `of!` until that domain implements its trait
//! for the new type. Each agent is one file under `src/agents/`; copy the nearest sibling to add one.

mod agents;
mod entry;

pub use agents::{Antigravity, Claude, Codex, Cursor, Grok, Opencode};
pub use entry::{AgentEntry, Billing, GatewayBinding, Identity, Install, Platform, Profile, Session, SignIn};

use serde::{Deserialize, Serialize};

/// Declares the agents from one list: the [`Agent`] enum, [`Agent::ALL`] in the list's order, and the [`of!`] macro
/// with one arm per agent. Each name is both a variant and the agent's type in `agents/`, so no list can leave one
/// out. The leading `$` is passed in as a token, so this macro can write the metavariables of the `of!` it declares.
macro_rules! agents {
    ($d:tt $($agent:ident,)+) => {
        /// A coding agent, as `nexus <agent>` names it. Its id ([`Agent::as_str`]) names it in the API, in policies,
        /// in sessions and in stored rows, and serde writes the same id. A session's agent is the workbench's
        /// `SessionAgent` (`nexus-workbench/src/event.rs`). It adds Nexus's own agent, which only sessions run.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "lowercase")]
        pub enum Agent {
            $($agent,)+
        }

        impl Agent {
            /// Every agent, in the order Gateway's catalog lists them.
            pub const ALL: [Agent; [$(stringify!($agent)),+].len()] = [$(Agent::$agent),+];
        }

        /// The value of a domain's trait for an agent: `of!(agent, dyn GatewayAgent)` is the
        /// `&'static dyn GatewayAgent` of the agent's type. This crate's own trait is [`AgentT`]; a domain crate
        /// defines its trait the same way. The trait must be object safe, and it needs `Sync` as a supertrait, as
        /// `AgentT` has: the value is a `&'static` that Gateway holds across an `.await`. It does not build until the
        /// trait is implemented for every agent's type, and the error names the missing one.
        #[macro_export]
        macro_rules! of {
            ($d value:expr, dyn $d trait:path) => {
                match $d value {
                    $($d crate::Agent::$agent => &$d crate::$agent as &'static dyn $d trait,)+
                }
            };
        }
    };
}

agents! { $
    Claude,
    Codex,
    Antigravity,
    Cursor,
    Opencode,
    Grok,
}

/// The trait every coding agent's type implements in this crate (`src/agents/<agent>.rs`): it gives that agent's
/// entry. The closed enum that names the agents is [`Agent`]. Each domain crate defines its own trait the same way,
/// implements it for each agent's type, and reaches it with [`of!`].
pub trait AgentT: Sync {
    fn entry(&self) -> &'static AgentEntry;
}

/// The optional comma-separated list of Nexus CLI commands a client supports for agent setup.
pub const AGENT_LAUNCHERS_HEADER: &str = "x-nexus-agent-launchers";

impl Agent {
    pub fn entry(self) -> &'static AgentEntry {
        of!(self, dyn AgentT).entry()
    }

    /// The agent named `id` (`claude`).
    pub fn parse(id: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|agent| agent.as_str() == id)
    }

    pub fn as_str(self) -> &'static str {
        self.entry().identity.id
    }

    /// Every agent's id, joined by `sep` (`claude|codex`), for help and messages.
    pub fn ids(sep: &str) -> String {
        Agent::ALL.map(Agent::as_str).join(sep)
    }

    /// Its command: `nexus <command>` starts it, as the person types the program without Nexus (`agy`, so `nexus agy`).
    /// The id stays its name everywhere else: in the API, in sessions and in its variables.
    pub fn command(self) -> &'static str {
        self.entry().identity.executable
    }

    /// The agent `nexus <word>` starts. `nexus antigravity` starts nothing: its command is `agy`.
    pub fn from_command(word: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|agent| agent.command() == word)
    }

    /// The agent named by its id or its command, where a person may type either (`nexus agents use agy …`).
    pub fn parse_named(text: &str) -> Option<Agent> {
        Agent::parse(text).or_else(|| Agent::from_command(text))
    }

    /// The commands this build can launch, joined by `sep` (`claude|codex|agy`).
    pub fn commands(sep: &str) -> String {
        Agent::ALL
            .into_iter()
            .filter(|agent| agent.entry().session.has_cli_launcher())
            .map(Agent::command)
            .collect::<Vec<_>>()
            .join(sep)
    }
}

/// Its id, as the API and sessions name it. Its command is [`Agent::command`].
impl std::fmt::Display for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each variant's place in [`Agent::ALL`]. `agents!` writes `ALL` from the enum's own list; this pins the order
    /// Gateway's catalog lists them in. The match lists every variant, so a new one does not build here until it has a
    /// place.
    fn place(agent: Agent) -> usize {
        match agent {
            Agent::Claude => 0,
            Agent::Codex => 1,
            Agent::Antigravity => 2,
            Agent::Cursor => 3,
            Agent::Opencode => 4,
            Agent::Grok => 5,
        }
    }

    #[test]
    fn all_lists_every_variant() {
        let places: Vec<usize> = Agent::ALL.map(place).to_vec();
        assert_eq!(places, (0..Agent::ALL.len()).collect::<Vec<_>>());
        assert_eq!(Agent::ALL[place(Agent::Antigravity)], Agent::Antigravity);
    }

    /// An agent's serde value, its id, its text and its parse are one string, and each reads back as the agent.
    #[test]
    fn serde_value_is_the_id_parse_reads() {
        for agent in Agent::ALL {
            let id = agent.as_str();
            assert_eq!(serde_json::to_value(agent).expect("serializes"), serde_json::json!(id));
            assert_eq!(
                serde_json::from_value::<Agent>(serde_json::json!(id)).expect("deserializes"),
                agent
            );
            assert_eq!(Agent::parse(id), Some(agent));
            assert_eq!(agent.to_string(), id);
        }
        assert_eq!(
            Agent::ALL.map(Agent::as_str),
            ["claude", "codex", "antigravity", "cursor", "opencode", "grok"]
        );
    }

    /// A command reads back as its agent, and so does either name where a person may type one.
    #[test]
    fn commands_read_back_as_their_agents() {
        for agent in Agent::ALL {
            assert_eq!(Agent::from_command(agent.command()), Some(agent));
            assert_eq!(Agent::parse_named(agent.command()), Some(agent));
            assert_eq!(Agent::parse_named(agent.as_str()), Some(agent));
        }
        assert_eq!(Agent::from_command("antigravity"), None);
        assert_eq!(Agent::ids("|"), "claude|codex|antigravity|cursor|opencode|grok");
        assert_eq!(Agent::commands("|"), "claude|codex|agy|cursor-agent");
    }

    /// Each agent's entry states every field in its own file, at every level: no struct update (`..CLAUDE_CODE`), no
    /// `Default`, no entry or part copied from another (`static X: AgentEntry = CODEX;`, `identity: CODEX_IDENTITY,`).
    /// Each part is a literal of its own (`identity: Identity {`), once. A new field then fails to build until each agent
    /// decides it.
    #[test]
    fn every_entry_states_every_field() {
        const PARTS: [(&str, &str); 6] = [
            ("identity", "Identity"),
            ("install", "Install"),
            ("sign_in", "SignIn"),
            ("billing", "Billing"),
            ("profile", "Profile"),
            ("session", "Session"),
        ];
        let types = [
            "AgentEntry",
            "Identity",
            "Install",
            "SignIn",
            "Billing",
            "Profile",
            "Session",
        ];
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/agents");
        let mut read = 0;
        for file in std::fs::read_dir(&dir).expect("src/agents") {
            let path = file.expect("an entry").path();
            let source = std::fs::read_to_string(&path).expect("readable");
            read += 1;
            let mut parts = [0; PARTS.len()];
            for line in source.lines().map(str::trim).filter(|line| !line.starts_with("//")) {
                let update = line.starts_with("..") || line.contains("{ ..") || line.contains(", ..");
                let copied = types
                    .iter()
                    .any(|ty| line.contains(&format!(": {ty} =")) && !line.contains(&format!(": {ty} = {ty} {{")));
                assert!(
                    !update && !copied && !line.contains("Default"),
                    "{}: {line}",
                    path.display()
                );
                for (count, (field, ty)) in parts.iter_mut().zip(PARTS) {
                    if line.starts_with(&format!("{field}:")) {
                        assert_eq!(line, format!("{field}: {ty} {{"), "{}", path.display());
                        *count += 1;
                    }
                }
            }
            assert_eq!(parts, [1; PARTS.len()], "each part once in {}", path.display());
        }
        assert_eq!(read, Agent::ALL.len(), "one file per agent in {}", dir.display());
    }
}
