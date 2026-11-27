use crate::{AgentEntry, AgentT, Billing, GatewayBinding, Identity, Install, Platform, Profile, Session, SignIn};

/// Anthropic's Claude Code (`claude`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claude;

impl AgentT for Claude {
    fn entry(&self) -> &'static AgentEntry {
        &CLAUDE_CODE
    }
}

static CLAUDE_CODE: AgentEntry = AgentEntry {
    identity: Identity {
        id: "claude",
        executable: "claude",
        name: "Claude Code",
        vendor: "Anthropic",
        plan: "Claude",
    },
    install: Install {
        // Anthropic's native installer, which puts `claude` in `~/.local/bin`.
        command: "curl -fsSL https://claude.ai/install.sh | bash",
        // Claude Code's own updater follows how it was installed: npm or its native installer.
        update: "claude update",
        dirs: &[".claude/local"],
    },
    sign_in: SignIn {
        login: "claude auth login",
        login_status: "claude auth status --text",
        logout: "claude auth logout",
        status_args: Some(&["auth", "status", "--json"]),
        status_timeout: std::time::Duration::from_secs(5),
        status_min_version: None,
        status_sign_in_prompt: None,
        token_var: Some("CLAUDE_CODE_OAUTH_TOKEN"),
    },
    billing: Billing {
        configured_provider: None,
        gateway: Some(GatewayBinding { provider: "anthropic" }),
        accepts_subscription_usage: true,
        plan_overrides: &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_CUSTOM_HEADERS",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
        ],
    },
    profile: Profile {
        var: Some("CLAUDE_CONFIG_DIR"),
        dir: ".claude",
        skills: Some("skills"),
        instructions: Some("CLAUDE.md"),
    },
    session: Session {
        permission_settings: None,
        platforms: &[Platform::MacOs, Platform::Linux, Platform::Windows],
        cli_launcher: true,
        legacy_access: true,
        resume: "--resume {id}",
        example: "-p \"hello\"",
    },
};
