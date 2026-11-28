use crate::{AgentEntry, AgentT, Billing, Identity, Install, Profile, Session, SignIn};

/// Grok's CLI (`grok`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grok;

impl AgentT for Grok {
    fn entry(&self) -> &'static AgentEntry {
        &GROK
    }
}

static GROK: AgentEntry = AgentEntry {
    identity: Identity {
        id: "grok",
        executable: "grok",
        name: "Grok",
        vendor: "xAI",
        plan: "Grok",
    },
    // https://docs.x.ai/build/overview
    install: Install {
        command: "curl -fsSL https://x.ai/cli/install.sh | bash",
        update: "grok update",
        dirs: &[],
    },
    // grok 1.0.92 has login and logout commands, but no noninteractive sign-in status command.
    sign_in: SignIn {
        login: "grok login",
        login_status: "grok",
        logout: "grok logout",
        status_args: None,
        status_timeout: std::time::Duration::ZERO,
        status_min_version: None,
        status_sign_in_prompt: None,
        token_var: None,
    },
    billing: Billing {
        configured_provider: None,
        gateway: None,
        accepts_subscription_usage: false,
        plan_overrides: &["XAI_API_KEY", "GROK_CLI_CHAT_PROXY_BASE_URL"],
    },
    // https://docs.x.ai/build/settings
    profile: Profile {
        var: Some("GROK_HOME"),
        dir: ".grok",
        skills: None,
        instructions: None,
    },
    session: Session {
        permission_settings: None,
        platforms: &[],
        cli_launcher: true,
        legacy_access: false,
        resume: "--resume {id}",
        example: "-p \"hello\"",
    },
};
