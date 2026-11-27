use crate::{AgentEntry, AgentT, Billing, GatewayBinding, Identity, Install, Platform, Profile, Session, SignIn};

/// OpenAI's Codex (`codex`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codex;

impl AgentT for Codex {
    fn entry(&self) -> &'static AgentEntry {
        &CODEX
    }
}

static CODEX: AgentEntry = AgentEntry {
    identity: Identity {
        id: "codex",
        executable: "codex",
        name: "Codex",
        vendor: "OpenAI",
        plan: "ChatGPT",
    },
    install: Install {
        command: "npm install -g @openai/codex",
        // Later: npm only. A Homebrew copy (`brew upgrade codex`) needs a choice by the binary's path.
        update: "npm i -g @openai/codex@latest",
        dirs: &[],
    },
    sign_in: SignIn {
        login: "codex login",
        login_status: "codex login status",
        logout: "codex logout",
        status_args: Some(&["login", "status"]),
        status_timeout: std::time::Duration::from_secs(5),
        status_min_version: None,
        status_sign_in_prompt: None,
        token_var: None,
    },
    billing: Billing {
        configured_provider: None,
        gateway: Some(GatewayBinding { provider: "openai" }),
        accepts_subscription_usage: true,
        plan_overrides: &["OPENAI_API_KEY"],
    },
    profile: Profile {
        var: Some("CODEX_HOME"),
        dir: ".codex",
        skills: Some("skills"),
        instructions: Some("AGENTS.md"),
    },
    session: Session {
        permission_settings: None,
        platforms: &[Platform::MacOs, Platform::Linux, Platform::Windows],
        cli_launcher: true,
        legacy_access: true,
        resume: "resume {id}",
        example: "exec \"hello\"",
    },
};
