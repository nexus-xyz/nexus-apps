//! "Keep agents running when Nexus quits": this computer's Code sessions run in a background service, so a turn in
//! progress goes on after this build quits. Off by default; macOS and Linux only. The setting, its managed policy
//! (`AllowLocalCache` keeps it off) and the rules are `nexus_desktop_core::service`'s.
//!
//! The service is nexus-host's `local` mode. This build starts it from its own binary
//! (`nexus-desktop-gpui service <flags>`), and reaches it through nexus-remote's `LocalService`: the shared socket,
//! token, data folder and version check. So this app and an older Nexus install each reach a service the other
//! started, and a service of another version is replaced once it is idle.
//!
//! - **At launch** with the switch on, the runner (`code_run.rs`) does not take the store's lock: the service holds it.
//!   The runner connects to a running service, or starts one, and sends it the shared `wb_*` commands
//!   (`nexus_remote::engine`). The windows read the store as before. That is the take-back: the sessions the service kept
//!   running show here again, with what they did while this build was away.
//! - **At quit** the connection closes, and the service and its agents keep running. That is the hand-over.
//! - With the switch **off**, or the policy holding it off, nothing here runs. A service still running holds the
//!   store's lock, so this build reads only and offers Take over (`owner.rs`).
//! - Never two owners: when the Nexus app holds the lock itself (its switch off), this build does not start a
//!   service, which would be refused anyway (nexus-host takes the lock before it opens the store).
//!
//! - While the service runs the sessions, clone progress, outside access and the pull request watch reach them through
//!   it (`nexus_remote::engine`, `outside.rs`, `pr_watch.rs`). The service's agents get the built-in Nexus server, and
//!   Settings' MCP servers with their keychain values, which this build hands the service in each sign-in and after
//!   each save in Settings (`CodeRunner::apply_settings`); the service keeps them in memory only. They get the
//!   session browser's tools too, whose calls this build runs on the session's CEF tabs (`ServiceSink`). The
//!   service passes their pull request tools' and `request_secret`'s calls here, and this build answers them as on its
//!   own socket (`code_run.rs`'s `ServiceSink`). With session sync on, this build hands the service this computer's
//!   device key in each sign-in, and the service syncs the sessions (nexus-host's `local_sync`).
//! - Settings › App shows what the switch does now ([`note`]) and offers Restart now. Signing out stops the service
//!   with its agents (`CodeRunner::signed_out`).
//!
//! The switch takes effect at the next start. Turned off while this build uses the service, the service stops when
//! this build quits, with its agents.

use std::path::Path;

use nexus_desktop_core::settings::Settings;
use nexus_remote::machine::Status;

/// The shared app identifier: the service socket's folder in the runtime folder (`paths.rs` has its data folder).
pub const APP_ID: &str = "xyz.nexus.desktop";

/// Whether this launch runs Code sessions in the service: the switch is on, and the policy lets it be.
pub fn wanted(settings: &Settings, allow_local_cache: bool) -> bool {
    nexus_desktop_core::service::wanted(settings.keep_agents_running, allow_local_cache)
}

/// [`wanted`] with the Nexus app's settings and this computer's policy now.
pub fn wanted_now() -> bool {
    wanted(
        &crate::desktop_settings::load(),
        crate::managed::local_cache(crate::managed::policy()),
    )
}

/// The line under the switch in Settings › App. `enabled`: the switch as
/// saved; `active`: this launch runs the sessions in the service; `status`: its connection, while active.
pub fn note(allow_local_cache: bool, enabled: bool, active: bool, status: Option<&Status>) -> String {
    if let Some(reason) = nexus_desktop_core::service::unsupported(allow_local_cache) {
        return reason;
    }
    if enabled != active {
        return if enabled {
            "Restart Nexus to move this computer's sessions into the background service.".into()
        } else {
            "Restart Nexus to bring sessions back into the app. The background service stops with its agents.".into()
        };
    }
    if !active {
        return "Off: quitting Nexus stops every agent.".into();
    }
    match status {
        Some(Status::Connected { host, .. }) => {
            format!(
                "On: agents keep running when Nexus quits. Service version {}.",
                host.version
            )
        }
        Some(Status::Connecting { .. } | Status::Updating) => "On: starting the background service…".into(),
        Some(Status::Unreachable { error, .. }) => format!("On, but the background service is not running: {error}"),
        Some(Status::NeedsUpdate { .. }) => "On, but the background service runs another version.".into(),
        _ => "On: agents keep running when Nexus quits.".into(),
    }
}

/// Whether a process holds the store's lock (`run/workbench.lock` in `data`), checked without taking it.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn store_held(data: &Path) -> bool {
    let Ok(file) = std::fs::File::open(data.join("run").join("workbench.lock")) else {
        return false;
    };
    matches!(file.try_lock_shared(), Err(std::fs::TryLockError::WouldBlock))
}

/// Run the service from this binary: `nexus-desktop-gpui service <flags>`. Returns its exit code, or None when this
/// process is not the service. Called first in `main`.
pub fn serve() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("service") {
        return None;
    }
    #[cfg(unix)]
    {
        let args: Vec<String> = args.collect();
        // Agy in the service runs with Settings' Sandbox switch, read each time it starts, as in the app.
        nexus_host::local::set_antigravity_sandbox(std::sync::Arc::new(|| {
            crate::desktop_settings::load().antigravity_sandboxed()
        }));
        nexus_host::local::set_subsessions_source(|| crate::desktop_settings::load().subsessions());
        // The service runs the person's hooks (Settings › Hooks); it shows no notifications.
        if let Some(source) = nexus_desktop_core::hooks::FromSettings::in_config_dir(None) {
            nexus_host::local::set_hooks(source);
        }
        // The service's rotation reads the plans as Settings does: it skips a signed-out account and moves near a
        // limit.
        nexus_host::local::set_plans(std::sync::Arc::new(|| {
            crate::pages::subscriptions::probe_all().unwrap_or_default()
        }));
        Some(nexus_host::local::main(&args))
    }
    #[cfg(not(unix))]
    {
        eprintln!("nexus-desktop-gpui: the background service runs on macOS and Linux only.");
        Some(2)
    }
}

#[cfg(unix)]
pub use unix::*;

#[cfg(unix)]
mod unix {
    use std::path::PathBuf;

    use nexus_remote::local::{LocalService, SignIn};

    use super::*;

    /// The service as this build starts and reaches it: the shared data folder, socket and version, and this
    /// build's binary. `worktrees` and `reviews` are the folders new work goes in (Settings, Worktrees).
    pub fn local_service(
        data: PathBuf,
        worktrees: PathBuf,
        reviews: PathBuf,
        sign_in: SignIn,
    ) -> Result<LocalService, String> {
        // The same binary serves the built-in Nexus server the service's agents get.
        let program = nexus_desktop_core::service::program()?;
        Ok(LocalService {
            app_program: Some(program.clone()),
            program,
            command: vec!["service".into()],
            socket: nexus_desktop_core::service::socket_here(APP_ID, &data),
            data,
            worktrees: Some(worktrees),
            reviews: Some(reviews),
            idle_exit: nexus_host::local::IDLE_EXIT,
            env: Vec::new(),
            version: Some(nexus_host::VERSION.to_string()),
            sign_in,
        })
    }

    /// Whether the Nexus app holds the store in its own process: the lock is held, and not by a service.
    pub fn held_in_process(local: &LocalService) -> bool {
        store_held(&local.data) && local.running().is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_and_the_policy_decide() {
        let on = Settings {
            keep_agents_running: Some(true),
            ..Settings::default()
        };
        assert_eq!(wanted(&on, true), cfg!(unix));
        assert!(!wanted(&on, false), "AllowLocalCache off keeps it off");
        assert!(!wanted(&Settings::default(), true), "off by default");
        let off = Settings {
            keep_agents_running: Some(false),
            ..Settings::default()
        };
        assert!(!wanted(&off, true));
    }

    #[test]
    fn the_note_says_what_the_switch_does_now() {
        if !cfg!(unix) {
            return;
        }
        assert!(note(false, false, false, None).contains("organization"));
        assert!(note(true, true, false, None).starts_with("Restart Nexus to move"));
        assert!(note(true, false, true, None).starts_with("Restart Nexus to bring"));
        assert_eq!(note(true, false, false, None), "Off: quitting Nexus stops every agent.");
        let unreachable = Status::Unreachable {
            error: "no socket".into(),
            retry_in_secs: 1,
        };
        assert_eq!(
            note(true, true, true, Some(&unreachable)),
            "On, but the background service is not running: no socket"
        );
        assert_eq!(
            note(true, true, true, Some(&Status::Connecting { attempt: 1 })),
            "On: starting the background service…"
        );
    }

    #[test]
    fn a_held_lock_reads_as_held_and_a_free_one_does_not() {
        let data = std::env::temp_dir().join(format!("nexus-gpui-held-{}", std::process::id()));
        assert!(!store_held(&data), "no lock file yet");
        let owner = nexus_workbench::owner::StoreOwner::take(&data).expect("the lock is free");
        assert!(store_held(&data));
        drop(owner);
        assert!(!store_held(&data));
        let _ = std::fs::remove_dir_all(data);
    }
}
