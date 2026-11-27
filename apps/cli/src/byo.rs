//! The CLI without a Nexus account: coding agents run on the person's own plan or key, and a command that needs an
//! account says how to get one. There is no guest chat: a terminal cannot run the abuse check guests need.
//!
//! Behind `NEXUS_BYO_PROVIDERS`, off by default: with it off, every command says what it said before.

use crate::Context;
use crate::config::{Config, DEFAULT_URL, ui_url};
use crate::error::CliError;

/// The switch, the same one Nexus for desktop's own-plan and own-key sessions use.
pub const VARIABLE: &str = "NEXUS_BYO_PROVIDERS";

/// Whether the switch is on: `on`, `1` or `true`.
pub fn enabled(ctx: &Context) -> bool {
    ctx.var(VARIABLE)
        .is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "on" | "1" | "true"))
}

/// Where a person creates an account: Nexus UI of the Gateway the CLI talks to (`https://nexus.inc/app` in production).
pub fn signup_url(ctx: &Context, config: &Config) -> String {
    config.ui_url.clone().unwrap_or_else(|| {
        ui_url(
            if config.url.is_empty() {
                DEFAULT_URL
            } else {
                &config.url
            },
            &ctx.env_fn(),
        )
    })
}

const OWN_PLAN: &str = "Without one, `nexus claude`, `nexus codex` and the other coding agents run on your own plan or key (`nexus agents`).";

/// `what` needs an account: sign in, or create one.
pub fn account_required(what: &str, ctx: &Context, config: &Config) -> CliError {
    CliError::hint(
        format!("{what} needs a Nexus account."),
        format!(
            "Sign in with `nexus auth login`, or create an account at {}. {OWN_PLAN}",
            signup_url(ctx, config)
        ),
    )
    .with_code("account_required")
}

/// `nexus chat` and `nexus agent ask|chat` without an account.
pub fn chat_requires_account(ctx: &Context, config: &Config) -> CliError {
    CliError::hint(
        "Chat needs a Nexus account, or a coding agent on your own plan or key.",
        format!(
            "Sign in with `nexus auth login`, or create an account at {}. Or set up a coding agent: `nexus agents` shows what this computer has, then `nexus claude` or `nexus codex` runs it on your own plan or key.",
            signup_url(ctx, config)
        ),
    )
    .with_code("account_required")
}
