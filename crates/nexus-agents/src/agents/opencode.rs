use crate::{AgentEntry, AgentT, Billing, Identity, Install, Platform, Profile, Session, SignIn};

/// OpenCode's CLI (`opencode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opencode;

impl AgentT for Opencode {
    fn entry(&self) -> &'static AgentEntry {
        &OPENCODE
    }
}

static OPENCODE: AgentEntry = AgentEntry {
    identity: Identity {
        id: "opencode",
        executable: "opencode",
        name: "OpenCode",
        vendor: "OpenCode",
        plan: "configured provider",
    },
    // https://opencode.ai/docs/
    install: Install {
        command: "curl -fsSL https://opencode.ai/install | bash",
        update: "opencode upgrade",
        dirs: &[".opencode/bin"],
    },
    // https://opencode.ai/docs/cli/#auth
    sign_in: SignIn {
        login: "opencode auth login",
        login_status: "opencode auth list",
        logout: "opencode auth logout",
        status_args: None,
        status_timeout: std::time::Duration::ZERO,
        status_min_version: None,
        status_sign_in_prompt: None,
        token_var: None,
    },
    billing: Billing {
        configured_provider: Some("OpenCode provider"),
        gateway: None,
        accepts_subscription_usage: false,
        plan_overrides: &[],
    },
    // https://opencode.ai/docs/config/
    profile: Profile {
        var: Some("OPENCODE_CONFIG_DIR"),
        dir: ".config/opencode",
        skills: None,
        instructions: None,
    },
    session: Session {
        permission_settings: Some("OpenCode settings"),
        platforms: &[Platform::Linux],
        cli_launcher: false,
        legacy_access: false,
        resume: "--session {id}",
        example: "run \"hello\"",
    },
};
