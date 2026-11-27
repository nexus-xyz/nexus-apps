use crate::{AgentEntry, AgentT, Billing, Identity, Install, Platform, Profile, Session, SignIn};

/// Cursor's CLI (`cursor-agent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor;

impl AgentT for Cursor {
    fn entry(&self) -> &'static AgentEntry {
        &CURSOR
    }
}

/// Cursor's `cursor-agent`, installed with Cursor's own script (<https://cursor.com/docs/cli/installation>). Its check is
/// `status --format json`, which reads the stored sign-in without a model turn and exits 0 signed in or not; its
/// `status` field says which (cursor-agent 2026.11.17). No Gateway route: it runs only on the person's Cursor plan.
static CURSOR: AgentEntry = AgentEntry {
    identity: Identity {
        id: "cursor",
        executable: "cursor-agent",
        name: "Cursor",
        vendor: "Cursor",
        plan: "Cursor",
    },
    install: Install {
        command: "curl https://cursor.com/install -fsS | bash",
        // Its installer links it in `~/.local/bin`, which `local_agents` already searches.
        update: "curl https://cursor.com/install -fsS | bash",
        dirs: &[],
    },
    sign_in: SignIn {
        login: "cursor-agent login",
        login_status: "cursor-agent status",
        logout: "cursor-agent logout",
        status_args: Some(&["status", "--format", "json"]),
        // It reads its own files: about 0.9 s on 2026-11-19.
        status_timeout: std::time::Duration::from_secs(10),
        status_min_version: None,
        status_sign_in_prompt: None,
        token_var: None,
    },
    billing: Billing {
        configured_provider: None,
        gateway: None,
        accepts_subscription_usage: false,
        // Each one signs it in with a key or a token instead of the person's own sign-in, or sends its calls elsewhere.
        plan_overrides: &[
            "CURSOR_API_KEY",
            "CURSOR_AUTH_TOKEN",
            "CURSOR_AUTH_TOKEN_FILE",
            "CURSOR_API_ENDPOINT",
        ],
    },
    profile: Profile {
        // Unset, it is `$XDG_CONFIG_HOME/cursor` when that is set, else `~/.cursor`.
        var: Some("CURSOR_CONFIG_DIR"),
        dir: ".cursor",
        skills: None,
        instructions: None,
    },
    session: Session {
        permission_settings: None,
        platforms: &[Platform::MacOs, Platform::Linux],
        cli_launcher: true,
        legacy_access: false,
        resume: "--resume {id}",
        example: "-p \"hello\"",
    },
};
