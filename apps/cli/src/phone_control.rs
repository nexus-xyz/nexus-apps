//! `nexus devices control [on|off]`: this computer's Phone control switch for its `nexus claude` and
//! `nexus codex` sessions, as the desktop app's Settings › Devices › Phone control is the app's. It is off until the
//! person turns it on. The CLI keeps it in its own session log (`session_log::log_path`), because the desktop app's
//! store belongs to the app. Each run reads it at every pass, so a change reaches running sessions too.

use nexus_workbench::session_sync::Identity;
use nexus_workbench::session_sync::control_loop::{cli_control, set_cli_control};
use nexus_workbench::store::Store;
use serde_json::json;

use crate::Context;
use crate::cli::Cmd;
use crate::client::Client;
use crate::error::{CliError, Result, usage};
use crate::json::s;

/// What a run with Phone control on lets the phone do.
pub const WHAT: &str = "Your phone can send a message to a `nexus codex` session, stop its turn and answer its approvals. `nexus claude` sessions stay read-only.";

pub fn control_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let wanted = match args.get(1).map(String::as_str) {
        None | Some("status") => None,
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some(other) => {
            return usage(
                format!("Unknown Phone control setting: {other}."),
                Some("Run `nexus devices control on`, `nexus devices control off` or `nexus devices control`."),
            );
        }
    };
    let identity = identity(&cmd.core, &cmd.config.url)?;
    let store = Store::open(&crate::session_log::log_path(cmd.ctx)).map_err(CliError::new)?;
    if let Some(enabled) = wanted {
        set_cli_control(&store, &identity, enabled).map_err(CliError::new)?;
    }
    let enabled = cli_control(&store, &identity).map_err(CliError::new)?;
    if cmd.json {
        return Ok(cmd.print(&json!({ "enabled": enabled })));
    }
    let style = cmd.style;
    if enabled {
        cmd.out(format!(
            "{} Phone control is on for this computer's terminal sessions.",
            style.ok
        ));
        cmd.out(style.dim(WHAT));
    } else {
        cmd.out(format!(
            "{} Phone control is off for this computer's terminal sessions.",
            style.info
        ));
        cmd.out(style.dim("Run `nexus devices control on` to turn it on."));
    }
    if !crate::session_log::enabled(cmd.ctx) {
        cmd.out(style.dim(&format!(
            "It needs session sync: {}=1, and this computer paired (`nexus devices pair`).",
            crate::session_log::SWITCH
        )));
    }
    Ok(0)
}

/// Whether Phone control is on for the person signed in to `gateway` with `core`. Any failure reads as off.
pub fn on(core: &Client, ctx: &Context, gateway: &str) -> bool {
    let Ok(identity) = identity(core, gateway) else {
        return false;
    };
    Store::open(&crate::session_log::log_path(ctx))
        .and_then(|store| cli_control(&store, &identity))
        .unwrap_or(false)
}

/// The session log's owner, as the run computes it: this Gateway and organization, and the signed-in person.
fn identity(core: &Client, gateway: &str) -> Result<Identity> {
    let whoami = core.get("/v1/auth/whoami")?;
    let tenant = s(&whoami, "tenantId").unwrap_or_default();
    let (Some("person"), Some(user)) = (s(&whoami, "kind"), s(&whoami, "userId").filter(|user| !user.is_empty()))
    else {
        return Err(CliError::hint(
            "Phone control needs a person's sign-in, and this one is not.",
            "Run `nexus login` as yourself.",
        )
        .into());
    };
    Ok(Identity {
        scope: crate::session_log::scope(gateway, tenant),
        user: user.to_string(),
    })
}
