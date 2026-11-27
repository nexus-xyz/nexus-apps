use crate::{AgentEntry, AgentT, Billing, Identity, Install, Platform, Profile, Session, SignIn};

/// Google's Antigravity (`agy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Antigravity;

impl AgentT for Antigravity {
    fn entry(&self) -> &'static AgentEntry {
        &ANTIGRAVITY
    }
}

/// Google's `agy`. Google installs it with its own script (<https://antigravity.google/docs/install/>), not an npm
/// package. It has no login command: it signs in on its first interactive start, so both steps a person reads are
/// `agy`. Its check is `/usage` in print mode, which answers the plan's quota without a model turn (agy 1.1.11's
/// changelog), and so only for a signed-in plan; signed out, agy asks for a sign-in and opens the browser for it. Its
/// settings file selects API-key mode with `modelProvider` (`local_agents::antigravity_login`), read before the check;
/// `/logout` does not undo it, so signing out is removing that key. No Gateway route: it runs only on the person's
/// Google AI plan.
static ANTIGRAVITY: AgentEntry = AgentEntry {
    identity: Identity {
        id: "antigravity",
        executable: "agy",
        name: "Antigravity",
        vendor: "Google",
        plan: "Google AI",
    },
    install: Install {
        command: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
        // Its installer replaces an older copy with the latest release.
        update: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
        // Its installer puts it in `~/.local/bin`, which `local_agents` already searches.
        dirs: &[],
    },
    sign_in: SignIn {
        login: "agy",
        login_status: "agy",
        logout: "remove \"modelProvider\" from ~/.gemini/antigravity-cli/settings.json",
        status_args: Some(&["-p", "/usage", "--output-format", "json"]),
        // It asks Google: about 3.6 s on 2026-11-16.
        status_timeout: std::time::Duration::from_secs(15),
        // An older agy sends `/usage` to the model as a prompt.
        status_min_version: Some((1, 1, 11)),
        status_sign_in_prompt: Some("Authentication required"),
        token_var: None,
    },
    billing: Billing {
        configured_provider: None,
        gateway: None,
        accepts_subscription_usage: true,
        // Each one makes `agy` bill a Gemini API key, Vertex or ADC, or send its calls to another endpoint.
        plan_overrides: &[
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GOOGLE_CLOUD_PROJECT",
            "CLOUDSDK_CORE_PROJECT",
            "AGY_ADC_AUTH",
            "GOOGLE_GEMINI_BASE_URL",
        ],
    },
    profile: Profile {
        var: None,
        dir: ".gemini/antigravity-cli",
        // Where `agy` reads skills from is not established.
        skills: None,
        instructions: None,
    },
    session: Session {
        permission_settings: None,
        platforms: &[Platform::MacOs, Platform::Linux, Platform::Windows],
        cli_launcher: true,
        legacy_access: true,
        resume: "--conversation {id}",
        example: "-p \"hello\"",
    },
};
