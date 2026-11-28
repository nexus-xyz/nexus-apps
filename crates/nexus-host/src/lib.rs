//! `nexus-host`: the desktop app's Code workbench with no window, for a person's own server.
//! It runs the same `nexus-workbench` as the app and answers the same `wb_*` commands
//! over a WebSocket on a private Unix socket, which the app reaches through an SSH tunnel ([`server`]). Agents keep
//! running while no app is connected; the app catches up from the event log when it comes back.
//!
//! - [`files`]: `~/.nexus/host` (the token, the host's own Gateway sign-in, the store, the socket, the lock).
//! - [`gateway`]: the [`nexus_workbench::Host`] that starts agents through Gateway with the host's token, and its
//!   [`nexus_workbench::api::Machine`].
//! - [`server`]: the hello, the requests and the pushed events. [`client`]: the same from the other end.
//! - [`cloud`]: a cloud machine's heartbeat to Gateway, which decides when the machine sleeps.
//! - [`cloud_history`]: a cloud machine's session history, sent to Gateway.
//! - [`relay`]: a cloud machine's line out to Gateway, which the desktop app reaches it through (no SSH).
//!   [`provision`]: the app's setup of the machine on each connection (`host_provision`).
//! - [`session_sync`]: the server as a trusted device, and its sessions synced, encrypted, through Gateway.
//!   [`local_sync`]: the desktop app's service syncs as the app's computer.
//! - [`local`]: the same host as the desktop app's own background service on this computer, so its agents keep
//!   running when the app quits. [`app_relay`]: its agents' built-in Nexus server and the app's socket.

pub mod agent_files;
pub mod app_relay;
pub mod client;
pub mod cloud;
pub mod cloud_history;
pub mod files;
pub mod gateway;
pub mod local;
pub mod local_sync;
pub mod mcp;
pub mod provision;
pub mod relay;
pub mod server;
pub mod service;
pub mod session_runner;
pub mod session_sync;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nexus_workbench::api::Api;
/// The shell [`serve`] hands the workbench and the agents ([`nexus_workbench::process::Shell`]).
pub use nexus_workbench::process::Shell;
use nexus_workbench::store::Store;
use nexus_workbench::{Config, Workbench};
use tokio::sync::watch;

use crate::files::{Credentials, Lock, Paths};
use crate::gateway::{HeadlessHost, HeadlessMachine};
use crate::server::{HostInfo, Hub, PROTOCOL, Server};

pub type Result<T> = std::result::Result<T, String>;

/// The desktop app's version: an app asks for a host of exactly its own (build.rs).
pub const VERSION: &str = env!("NEXUS_HOST_VERSION");

/// The machine's name, as the approval page and the hello show it.
pub fn machine_name() -> String {
    nix::unistd::gethostname()
        .ok()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this server".into())
}

/// How `serve` listens.
#[derive(Debug, Clone, PartialEq)]
pub enum Listen {
    /// The private socket, `run/host.sock`.
    Socket,
    /// A loopback port (0: any free one, written to `run/host.port`), for an SSH that does not forward sockets.
    Port(u16),
}

/// Where the host listens, once it does.
#[derive(Debug, Clone, PartialEq)]
pub enum Listening {
    Socket(PathBuf),
    Port(u16),
}

/// What kind of host `serve` runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Mode {
    /// A person's own server: signed in with `nexus-host login`, its worktrees in the home folder.
    #[default]
    Server,
    /// The desktop app's own service on this computer ([`local`]).
    Local(local::Options),
}

/// Runs the host until `stop` turns true: one per person (the lock), the sessions of the organization it is signed in
/// to, the sessions a crash left open in Gateway ended first, idle agents stopped after 30 minutes. `ready` hears where
/// it listens. On stop it stops every agent the way quitting the app does (each turn shows "Continue" later). `shell`
/// runs terminal commands and gives the agents their `PATH`: the login shell, or `/bin/sh` in tests ([`Shell`]).
pub async fn serve(
    paths: Paths,
    home: PathBuf,
    listen: Listen,
    stop: watch::Receiver<bool>,
    shell: Shell,
    ready: impl FnOnce(&Listening),
) -> Result<()> {
    serve_as(Mode::Server, paths, home, listen, stop, shell, ready).await
}

/// The limits on a standalone host's subsessions. It reads no Settings and often runs unattended, so it keeps a
/// ceiling of its own: the limits hosts had before limits became optional in the app, its $20 spend cap included.
pub const SERVER_LIMITS: nexus_workbench::Limits = nexus_workbench::Limits {
    max_depth: Some(1),
    max_live_children: Some(4),
    max_live_linked: Some(8),
    max_child_worktrees: Some(16),
    max_starts: Some(8),
    max_auto_turns: Some(12),
    max_idle_waits: Some(30),
    crew_spend_usd: Some(20.0),
};

/// The subsession settings a host runs with: the app's on its service (the defaults until the app hands them), and
/// subsessions on with [`SERVER_LIMITS`] on a standalone host.
fn subsession_settings(mode: &Mode, from_app: Option<nexus_workbench::Subsessions>) -> nexus_workbench::Subsessions {
    match mode {
        Mode::Local(_) => from_app.unwrap_or_default(),
        Mode::Server => nexus_workbench::Subsessions {
            limits: SERVER_LIMITS,
            ..Default::default()
        },
    }
}

/// [`serve`] as a server or as the desktop app's service. The service has no sign-in of its own: the app hands it
/// its own in each hello, and the lost sessions are ended once it has one. It also stops by itself once no app has
/// been connected and no session has been busy for its `idle_exit`.
pub async fn serve_as(
    mode: Mode,
    paths: Paths,
    home: PathBuf,
    listen: Listen,
    stop: watch::Receiver<bool>,
    shell: Shell,
    ready: impl FnOnce(&Listening),
) -> Result<()> {
    paths.create()?;
    let _lock = Lock::take(&paths.lock())?;
    let _ownership = nexus_workbench::owner::StoreOwner::take(&paths.dir)?;
    let token = files::token(&paths.token())?;
    let local = match &mode {
        Mode::Local(options) => Some(options.clone()),
        Mode::Server => None,
    };
    let credentials = match local {
        Some(_) => None,
        None => Credentials::load(&paths.credentials()),
    };
    let local_providers_opt_in = match &mode {
        Mode::Local(_) => nexus_workbench::provider::LocalProvidersOptIn::from_env(|| {
            Arc::new(nexus_desktop_core::provider_keys::OsProviderKeys)
        }),
        Mode::Server => nexus_workbench::provider::LocalProvidersOptIn::Disabled,
    };
    let hub = Hub::new();
    let host = Arc::new(HeadlessHost::new(
        credentials.as_ref(),
        paths.dir.clone(),
        hub.clone(),
        shell.clone(),
        local.is_none(),
        local_providers_opt_in,
    ));
    let store = Store::open(&paths.store())?;
    let org = credentials.as_ref().and_then(|c| c.org.clone());
    let cloud_file = local
        .is_none()
        .then(|| cloud::CloudFile::load(&paths.cloud()))
        .flatten();
    // The Desktop service follows the app's settings. A standalone host has none, so it keeps its own ceiling.
    let subsessions = local.as_ref().and_then(|_| local::subsessions());
    let settings = subsession_settings(&mode, subsessions);
    let config = Config {
        orchestration: settings.limits,
        subsessions: settings.on,
        subsessions_first_start: settings.first_start,
        cross_agent_off: settings.cross_agent_off,
        cross_project_on: settings.cross_project_on,
        // As the desktop app scopes its sessions: the Gateway and the organization (the one launches are checked against).
        scope: host.scope(),
        worktrees: Some(
            local
                .as_ref()
                .and_then(|l| l.worktrees.clone())
                .unwrap_or_else(|| home.join("nexus-worktrees")),
        ),
        reviews: Some(
            local
                .as_ref()
                .and_then(|l| l.reviews.clone())
                .unwrap_or_else(|| home.join("nexus-reviews")),
        ),
        accounts: Some(paths.accounts()),
        attachments: Some(paths.attachments()),
        instructions: Some(paths.dir.join("instructions")),
        transcripts: Some(paths.dir.join("transcripts")),
        memory: local.as_ref().and_then(|_| nexus_agent_memory::Store::local().ok()),
        // The app's service serves its schedules' webhooks, as the app does with the service off. A server has none.
        trigger_port: local
            .as_ref()
            .map(|_| nexus_workbench::schedules::trigger::DEFAULT_PORT),
        shell: shell.clone(),
        // The app's service and a person's own server offer their sessions the shared terminal, as the app does when
        // it runs them: the app's window shows the commands and sends the person's actions (`host_terminal`). A cloud
        // machine does not yet: the app reaches it through Gateway, which does not carry those actions.
        terminal: cloud_file.is_none(),
        ..Config::default()
    };
    if let Some(cloud) = cloud_file {
        store.set_computer_origin(&nexus_workbench::computer::Origin::Cloud {
            machine_id: cloud.machine_id,
            name: None,
        })?;
    }
    // Read before the workbench takes the store: a cloud machine's history upload reads the log on its own connection.
    let log = store.reader();
    // And for the native sessions' crew members: each runs in its lead's session ([`session_runner`]).
    let lineage = store.reader();
    let workbench = Workbench::new(store, host.clone(), config)?;
    // The app's service runs the app's hooks, which only the app's binary hands it. A server runs none.
    if let Some(source) = local.as_ref().and_then(|_| local::hooks()) {
        workbench.set_hooks(source);
    }
    let machine = Arc::new(HeadlessMachine {
        home: home.clone(),
        host: host.clone(),
    });
    // The person's own shells, on a server or a cloud machine. The app's own service opens none: the app runs its own.
    let shells = local.is_none().then(|| {
        let machine = machine.clone();
        nexus_workbench::shell_pty::Shells::new(
            shell.clone(),
            Arc::new(move || nexus_workbench::api::Machine::tool_path(machine.as_ref())),
            nexus_workbench::shell_pty::Limits::default(),
        )
    });
    let mut features = workbench.features();
    if subsessions.is_some() {
        features.push(nexus_workbench::orchestra::LIMITS_FEATURE.into());
    }
    if shells.is_some() {
        features.push(nexus_workbench::shell_pty::FEATURE.into());
    }
    // A server runs native sessions (`/v1/sessions`) when Gateway starts them on it: it holds each one's runner lease,
    // as this machine. The app's own service runs none.
    let runners = local.is_none().then(|| {
        let machine_id = cloud::CloudFile::load(&paths.paired())
            .or_else(|| cloud::CloudFile::load(&paths.cloud()))
            .map(|file| file.machine_id)
            .unwrap_or_else(machine_name);
        let parent: session_runner::Parent = Arc::new(move |thread: &str| {
            lineage
                .thread(thread)
                .ok()
                .flatten()
                .and_then(|record| record.summary.parent_thread_id)
        });
        let runners = Arc::new(
            session_runner::SessionRunners::new(format!("nexus-host:{machine_id}"))
                .saved_in(paths.native_sessions())
                .with_lineage(parent),
        );
        host.set_session_runners(runners.clone());
        runners
    });
    let server = Arc::new(Server {
        api: Api::new(workbench.clone(), machine.clone()),
        hub: hub.clone(),
        token,
        info: HostInfo {
            version: VERSION.into(),
            protocol: PROTOCOL,
            machine: machine_name(),
            org,
            features,
        },
        shells: shells.clone(),
        local: local.as_ref().map(|_| host.clone()),
        provision: local
            .is_none()
            .then(|| provision::Places::new(paths.dir.clone(), home.clone())),
        runners,
        tools_host: Some(host.clone()),
    });
    // On a cloud machine (`cloud.json`), clones are shallow and blobless. The app's own service clones whole.
    if local.is_none() {
        nexus_workbench::clone::blobless_when(Some(paths.cloud()));
        // A host that closed mid-deepen left its projects shallow: each known one picks its history up again now.
        let tool_path = nexus_workbench::api::Machine::tool_path(machine.as_ref());
        for project in workbench.projects().unwrap_or_default() {
            nexus_workbench::clone::resume_deepen_if_blobless(project.path.into(), tool_path.clone());
        }
    }
    // The app's service and a person's own server: their agents get the app's tools, as the app's own do. The service
    // runs the app's binary for them; a server runs this one (`mcp.rs`). A cloud machine gets none: Gateway relays its
    // app, which hands it no tools.
    let program = match local.as_ref() {
        Some(local) => local.app_program.clone(),
        None if cloud::CloudFile::load(&paths.cloud()).is_none() => std::env::current_exe().ok(),
        None => None,
    };
    let relay = match program {
        Some(program) => {
            let signed_in = Arc::downgrade(&host);
            let current: app_relay::Current =
                Arc::new(move || signed_in.upgrade().map(|host| host.signed_in()).unwrap_or_default());
            let relay = match app_relay::AppRelay::bind(hub.clone(), current) {
                Ok(relay) => Some(relay),
                Err(error) if nexus_workbench::Host::local_providers_opt_in(host.as_ref()).enabled() => {
                    eprintln!("nexus-host: the app tools socket: {error}");
                    None
                }
                Err(error) => return Err(error),
            };
            // A server's agents read `request_secret`'s files here; the app's service's read them where the app is.
            if local.is_none()
                && let Some(relay) = &relay
            {
                relay.write_secrets_in(paths.dir.join("secret-files"));
            }
            host.set_app_tools(
                app_relay::AppTools::with_relay(program, relay.clone()).with_local_memory(local.is_some()),
            );
            relay
        }
        None => None,
    };

    // The service's own stop, when it has been idle long enough, joins the caller's.
    let (quit, mut stop) = {
        let mut asked = stop;
        let (quit, stop) = watch::channel(false);
        let forward = quit.clone();
        tokio::spawn(async move {
            let _ = asked.wait_for(|stopped| *stopped).await;
            let _ = forward.send(true);
        });
        (quit, stop)
    };

    let (listening, accept) = match listen {
        Listen::Socket => {
            let socket = paths.socket();
            // Ours: we hold the lock, so a socket or port file left here is from a host that died. A stale port file
            // would send `nexus-host status` to a dead port.
            let _ = std::fs::remove_file(&socket);
            let _ = std::fs::remove_file(paths.port());
            let listener = tokio::net::UnixListener::bind(&socket)
                .map_err(|e| format!("Could not listen on {}: {e}", socket.display()))?;
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| format!("Could not protect {}: {e}", socket.display()))?;
            }
            let accept = tokio::spawn(server::serve_unix(listener, server.clone(), stop.clone()));
            (Listening::Socket(socket), accept)
        }
        Listen::Port(port) => {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                .await
                .map_err(|e| format!("Could not listen on 127.0.0.1:{port}: {e}"))?;
            let port = listener.local_addr().map_err(|e| e.to_string())?.port();
            nexus_client::config::write_private(&paths.port(), format!("{port}\n").as_bytes())
                .map_err(|e| e.to_string())?;
            let accept = tokio::spawn(server::serve_tcp(listener, server.clone(), stop.clone()));
            (Listening::Port(port), accept)
        }
    };
    ready(&listening);

    let upkeep = tokio::spawn({
        let workbench = workbench.clone();
        let signed_in = local.is_none();
        let shells = shells.clone();
        async move {
            // The app's service ends them once the app has signed it in (`Server::sign_in`).
            if signed_in {
                workbench.end_lost_sessions().await;
            }
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                workbench.reap_idle();
                // Shells an app left open and stopped reading.
                if let Some(shells) = shells.clone() {
                    let reaped = tokio::task::spawn_blocking(move || shells.reap()).await;
                    if let Err(error) = reaped {
                        eprintln!("nexus-host: reaping shells: {error}");
                    }
                }
                // The host owns its sessions, so it continues them when their plan's limit resets, and starts its
                // schedules' runs. The app's service also binds its webhooks' listener here.
                let now = nexus_workbench::now_ms();
                workbench.continue_at_resets(now).await;
                workbench.run_schedules(now).await;
            }
        }
    });

    // A cloud machine's heartbeat. A server without `cloud.json` sends nothing; the app's own service never does.
    let heartbeat = local.is_none().then(|| {
        let server = server.clone();
        let sources = cloud::Sources {
            cloud: paths.cloud(),
            credentials: paths.credentials(),
            memory_events: cloud::MEMORY_EVENTS.into(),
            vmstat: cloud::VMSTAT.into(),
        };
        tokio::spawn(async move {
            loop {
                cloud::beat(&server, &sources).await;
                tokio::time::sleep(cloud::HEARTBEAT).await;
            }
        })
    });

    // A cloud machine's own setup at boot: the repository `cloud.json` names, cloned with no app connected.
    // A server without `cloud.json` looks for one, as the line out does; the app's own service never sets up.
    let setup = local.is_none().then(|| {
        let sources = cloud::Sources {
            cloud: paths.cloud(),
            credentials: paths.credentials(),
            memory_events: cloud::MEMORY_EVENTS.into(),
            vmstat: cloud::VMSTAT.into(),
        };
        tokio::spawn(cloud::begin_setup(server.clone(), host.clone(), sources, paths.setup()))
    });

    // A cloud machine's session history, sent to Gateway so the apps read it while the machine sleeps. A
    // server without `cloud.json` looks for one, as the setup does; the app's own service never sends it.
    let history = local
        .is_none()
        .then(|| tokio::spawn(cloud_history::begin(log, paths.cloud(), paths.credentials())));

    // A cloud machine's line out to Gateway. A server without `cloud.json` looks for one every 2 seconds: the cloud provider may write
    // the file after the host starts, and the app waits only 30 seconds for the machine to come online.
    let line_out = local.is_none().then(|| {
        // A paired server dials out as a cloud machine does, and does nothing else a cloud machine does.
        let paired = paths.paired();
        let sources = relay::Sources {
            cloud: if paired.exists() { paired } else { paths.cloud() },
            credentials: paths.credentials(),
        };
        tokio::spawn(relay::run(server.clone(), sources, stop.clone()))
    });

    // Session sync, on a server behind its switch. The app's own service syncs as the app's computer, with the device
    // key the app hands it while the app's own sync would run ([`local_sync`]): the worker asks for it on each pass.
    let syncing = match (&local, session_sync::enabled(), host.client()) {
        (Some(_), _, _) => {
            host.local_sync().keys_at(paths.session_sync_keys());
            Some((None, workbench.clone()))
        }
        (None, false, _) => None,
        (None, true, None) => {
            eprintln!("{}", session_sync::NOT_SIGNED_IN);
            None
        }
        (None, true, Some(client)) => {
            let watcher = tokio::spawn(host.sync().watch(client, paths.clone(), host.scope(), stop.clone()));
            Some((Some(watcher), workbench.clone()))
        }
    }
    .map(|(watcher, worker)| {
        let computer = server.clone();
        let worker = tokio::spawn(async move {
            tokio::join!(
                worker.run_session_sync_until_shutdown(),
                computer.api.run_computer_control_until_shutdown()
            );
        });
        (watcher, worker)
    });

    let idle =
        local.map(|options| tokio::spawn(local::exit_when_idle(server.clone(), options.idle_exit, quit.clone())));

    let _ = stop.wait_for(|stopped| *stopped).await;
    let _ = accept.await;
    upkeep.abort();
    if let Some(heartbeat) = heartbeat {
        heartbeat.abort();
    }
    if let Some(line_out) = line_out {
        line_out.abort();
    }
    if let Some(history) = history {
        history.abort();
    }
    if let Some(setup) = setup {
        setup.abort();
    }
    if let Some(idle) = idle {
        idle.abort();
    }
    workbench.shutdown().await;
    if let Some(shells) = shells {
        // Nothing an app left open outlives the host.
        if let Err(error) = tokio::task::spawn_blocking(move || shells.close_all()).await {
            eprintln!("nexus-host: closing shells: {error}");
        }
    }
    if let Some(relay) = relay {
        relay.close();
    }
    if let Some((watcher, worker)) = syncing {
        if let Some(watcher) = watcher {
            watcher.abort();
        }
        // The workbench's shutdown stopped the worker; an upload in flight stays staged for the next start.
        let _ = worker.await;
    }
    match listening {
        Listening::Socket(socket) => {
            let _ = std::fs::remove_file(socket);
        }
        Listening::Port(_) => {
            let _ = std::fs::remove_file(paths.port());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A standalone host reads no Settings, so its subsessions stop at its own ceiling, spend included. The app's
    /// service follows the app: no limit until the person sets one.
    #[test]
    fn a_server_host_keeps_a_ceiling_and_the_app_service_follows_the_app() {
        let server = subsession_settings(&Mode::Server, None);
        assert!(server.on);
        assert_eq!(server.limits, SERVER_LIMITS);
        assert_eq!(server.limits.crew_spend_usd, Some(20.0));
        assert_eq!(server.limits.max_auto_turns, Some(12));

        let local = Mode::Local(local::Options {
            data: "data".into(),
            socket: "socket".into(),
            worktrees: None,
            reviews: None,
            idle_exit: local::IDLE_EXIT,
            app_program: None,
        });
        assert_eq!(
            subsession_settings(&local, None).limits,
            nexus_workbench::Limits::default()
        );
        let set = nexus_workbench::Subsessions {
            limits: nexus_workbench::Limits {
                max_starts: Some(3),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(subsession_settings(&local, Some(set)), set);
    }
}
