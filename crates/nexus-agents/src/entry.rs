//! What Nexus knows of one agent as plain values: its names, its plan, how to install and sign it in, and what a client
//! needs of it on this machine. A fact here is a plain value that more than one domain reads. A value that one domain
//! alone reads, or that needs a domain's type, stays in that domain: Gateway's typed route and its plan's stored id are
//! the catalog's.
//!
//! An [`AgentEntry`] holds one struct per concern, so a reader finds a value by what it is for.

/// What Nexus knows of one agent, one struct per concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentEntry {
    pub identity: Identity,
    pub install: Install,
    pub sign_in: SignIn,
    pub billing: Billing,
    pub profile: Profile,
    pub session: Session,
}

/// Who the agent is: its names, and the plan and vendor it runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    /// Its name in the API and in sessions, and the key of its variables (`NEXUS_CLAUDE_BIN`).
    pub id: &'static str,
    /// The program the finders look for on `PATH` (`claude`), and its command (`nexus claude`). It may differ from `id`.
    pub executable: &'static str,
    /// Its name in messages (`Claude Code`).
    pub name: &'static str,
    /// Who sells the plan its subscription route runs on (`Anthropic`).
    pub vendor: &'static str,
    /// The plan its subscription route runs on (`Claude`).
    pub plan: &'static str,
}

/// How the agent gets onto the person's machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Install {
    /// How to install it: the vendor's own package.
    pub command: &'static str,
    /// How to update an installed copy to the vendor's latest release, with the vendor's own command. Nexus Desktop
    /// runs it only when the person presses Update.
    pub update: &'static str,
    /// Where its own installer puts it under the home folder, beyond `PATH` and the common folders.
    pub dirs: &'static [&'static str],
}

/// How the agent signs in to its plan, and how a client checks that sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignIn {
    /// How to sign it in to its plan, once, in words a person reads.
    pub login: &'static str,
    /// How to check that sign-in, in words a person reads.
    pub login_status: &'static str,
    /// How to sign it out, when an API key would bill the API instead of the plan.
    pub logout: &'static str,
    /// The arguments that make it report its sign-in, as `local_agents` reads it. `None` for an agent without such a
    /// command: it is never run for a sign-in check, and setup and launch say the sign-in is not checked.
    pub status_args: Option<&'static [&'static str]>,
    /// How long that check may take.
    pub status_timeout: std::time::Duration,
    /// The oldest version whose check is known to answer without a model turn; `None`: any version. An older or
    /// unknown version is not run, and the sign-in is not checked.
    pub status_min_version: Option<(u64, u64, u64)>,
    /// What the check prints when, signed out, it starts a sign-in and opens the browser for it. Such a check runs with
    /// no-op browser launchers (and not where those can't be put in place: Windows), stops as soon as it prints this,
    /// and reads as signed out.
    pub status_sign_in_prompt: Option<&'static str>,
    /// The variable that carries a long-lived sign-in token from the vendor's own command (`claude setup-token`), for a
    /// machine its own sign-in can't reach, such as a cloud machine. `None`: Nexus passes it no such token.
    pub token_var: Option<&'static str>,
}

/// Who pays for the agent's calls: Gateway, or the person's plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Billing {
    /// The payer label when an agent uses a provider configured outside Nexus.
    pub configured_provider: Option<&'static str>,
    /// How it runs through Gateway. `None` for an agent that runs only on the person's own plan: both CLIs then
    /// refuse the Gateway route (`route_unsupported`), and the offline catalog marks it unavailable.
    pub gateway: Option<GatewayBinding>,
    /// Whether Gateway accepts usage reports for a run on the person's own plan. Without them Gateway refuses the report
    /// (`usage_unsupported`), and the CLI shows the run's usage as unavailable.
    pub accepts_subscription_usage: bool,
    /// What would make it bill something other than the person's plan: removed for a subscription run and for its
    /// status check.
    pub plan_overrides: &'static [&'static str],
}

/// Where the agent keeps its configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    /// The variable that moves its configuration folder. `None` for an agent whose folder no variable moves: it is
    /// always `dir` under the home folder.
    pub var: Option<&'static str>,
    /// Its configuration folder under the home folder when `var` is unset.
    pub dir: &'static str,
    /// The folder in its configuration folder that it reads skills from. `None` while where it reads skills from is
    /// not established: Nexus delivers it no skills.
    pub skills: Option<&'static str>,
    /// The file in its configuration folder that it reads the person's own instructions from in every session
    /// (`CLAUDE.md`). `None` while that file is not established: Nexus writes it none.
    pub instructions: Option<&'static str>,
}

/// How a client runs one of the agent's conversations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    /// The settings label if the agent owns its tool permissions.
    pub permission_settings: Option<&'static str>,
    /// Whether Gateway offers this agent's subscription setup to clients that name its executable.
    pub cli_launcher: bool,
    /// Whether clients without launcher negotiation can render this agent on their access page.
    pub legacy_access: bool,
    /// Where Nexus supports local sessions. An empty list disables local sessions.
    pub platforms: &'static [Platform],
    /// The arguments that resume a conversation, with `{id}` for its id (`--resume {id}`).
    pub resume: &'static str,
    /// The arguments of a one-prompt run, for help text (`-p "hello"`).
    pub example: &'static str,
}

impl Session {
    /// Whether the CLI publishes the launcher. Gateway can register setup before clients support a platform.
    pub fn has_cli_launcher(&self) -> bool {
        self.cli_launcher && !self.platforms.is_empty()
    }

    pub fn supports_platform(&self, platform: Option<Platform>) -> bool {
        platform.is_some_and(|platform| self.platforms.contains(&platform))
    }
}

/// A platform Nexus supports for local sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
}

impl Platform {
    /// The platform of this build, if Nexus supports local sessions on it.
    pub fn current() -> Option<Self> {
        match std::env::consts::OS {
            "macos" => Some(Self::MacOs),
            "linux" => Some(Self::Linux),
            "windows" => Some(Self::Windows),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::MacOs => "macOS",
            Self::Linux => "Linux",
            Self::Windows => "Windows",
        }
    }
}

/// An agent's route through Gateway, as a client names it. Gateway's own route is `nexus_catalog::GatewayRoute`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayBinding {
    /// The provider Gateway calls for it (`anthropic`).
    pub provider: &'static str,
}

impl AgentEntry {
    /// Why Nexus refuses a local launch on this platform.
    pub fn platform_refusal(&self, platform: Option<Platform>) -> Option<String> {
        if self.session.supports_platform(platform) {
            return None;
        }
        let name = self.identity.name;
        let platforms: Vec<_> = self.session.platforms.iter().map(|platform| platform.name()).collect();
        Some(if platforms.is_empty() {
            format!("Nexus does not support local {name} sessions.")
        } else {
            format!("{name} sessions in Nexus require {}.", platforms.join(" or "))
        })
    }

    /// Why its `gateway` route cannot run when it has none: shown on that route in `GET /v1/agents/access`.
    pub fn plan_only_reason(&self) -> String {
        if self.billing.configured_provider.is_some() {
            return format!("{} uses its configured provider.", self.identity.name);
        }
        format!(
            "{} runs only on your {} plan through Nexus for now",
            self.identity.name, self.identity.plan
        )
    }

    /// Why a request for its Gateway route is refused when it has none, and the command that runs it.
    pub fn route_unsupported(&self) -> String {
        if self.billing.configured_provider.is_some() {
            return self.plan_only_reason();
        }
        let reason = format!("{} runs only on your {} plan", self.identity.name, self.identity.plan);
        if self.session.has_cli_launcher() {
            format!("{reason}: run `nexus {} --subscription`", self.identity.executable)
        } else {
            reason
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Platform::{Linux, MacOs, Windows};
    use crate::Agent;

    #[test]
    fn cursor_platform_support_and_refusal_come_from_its_entry() {
        let cursor = Agent::Cursor.entry();
        for platform in [MacOs, Linux] {
            assert_eq!(cursor.platform_refusal(Some(platform)), None);
        }
        assert_eq!(
            cursor.platform_refusal(Some(Windows)).as_deref(),
            Some("Cursor sessions in Nexus require macOS or Linux.")
        );
        for agent in [Agent::Claude, Agent::Codex, Agent::Antigravity] {
            for platform in [MacOs, Linux, Windows] {
                assert_eq!(agent.entry().platform_refusal(Some(platform)), None);
            }
        }
    }

    #[test]
    fn no_local_platforms_excludes_an_agent_and_explains_the_refusal() {
        let mut entry = *Agent::Claude.entry();
        entry.session.platforms = &[];
        for platform in [MacOs, Linux, Windows] {
            assert!(!entry.session.supports_platform(Some(platform)));
            assert_eq!(
                entry.platform_refusal(Some(platform)).as_deref(),
                Some("Nexus does not support local Claude Code sessions.")
            );
        }
    }

    /// An agent without a Gateway route names its plan in both messages, and the command that runs it there. The
    /// messages read only the agent's identity, so Claude Code's entry stands for a plan-only agent here.
    #[test]
    fn plan_only_agent_messages_name_the_plan() {
        let plan_only = Agent::Claude.entry();
        assert_eq!(
            plan_only.plan_only_reason(),
            "Claude Code runs only on your Claude plan through Nexus for now"
        );
        assert_eq!(
            plan_only.route_unsupported(),
            "Claude Code runs only on your Claude plan: run `nexus claude --subscription`"
        );
        assert_eq!(
            Agent::Antigravity.entry().route_unsupported(),
            "Antigravity runs only on your Google AI plan: run `nexus agy --subscription`"
        );
    }
}
