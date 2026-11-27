//! `nexus-host serve | login | pair | status | proxy | stop | service | version`. The desktop app starts `serve` over SSH and reads
//! where it listens from its first line; a person runs `login` once (or the app runs it for them and shows the code).
//! The app reaches a running host through `ssh <server> nexus-host proxy` (the socket's bytes on standard input and
//! output), and `stop` is the first step of its "Update now". `local` is the desktop app's own service on this computer
//! (src/local.rs).

use std::path::PathBuf;

use nexus_client::Client;
use nexus_host::files::{Credentials, Lock, Paths};
use nexus_host::gateway::client_options;
use nexus_host::{Listen, Listening, Shell, VERSION, machine_name};
use serde_json::{Value, json};

/// Where `login` signs in when no Gateway is named and none was used before: the desktop app's default.
const DEFAULT_GATEWAY: &str = "https://api.nexus.inc";

const USAGE: &str = "Usage: nexus-host <command>

  serve [--port <n>]       Run the host. It listens on ~/.nexus/host/run/host.sock, or on
                           127.0.0.1:<n> with --port (0: any free port).
  login [--gateway <url>] [--pair]
                           Sign this machine in to Nexus with its own device token. With
                           --pair, also pair it: Nexus Desktop then lists it and reaches it
                           through Nexus, with no SSH. Open the link, or scan its QR code.
                           A paired machine stays paired when it signs in again.
  pair [--approver-code <code>] [--org-key-fingerprint <fingerprint>]
                           Pair this machine as one of your trusted devices, so its
                           sessions sync, encrypted, to Nexus UI. A trusted device
                           approves it in Settings › Devices by the code it prints.
                           Then confirm the approver's code: asked on a terminal, or
                           given with --approver-code. Sync runs when the host runs
                           with NEXUS_HOST_SESSION_SYNC=1.
                           --org-key-fingerprint confirms your org's recovery key:
                           the fingerprint computed from your vault on your
                           admin's own machine. Until then, this machine
                           locks nothing for that key.
  status [--json]          Version, sign-in and running sessions.
  proxy                    Connect standard input and output to the running host's socket
                           (or port): how the desktop app reaches it over SSH.
  stop                     Stop the running host the way quitting the app does, and wait.
  service install [--keep-running] | uninstall | status [--json]
                           Start the host on boot: a systemd user unit with linger on
                           Linux, a launchd agent on macOS. It runs <root>/current, so
                           an update does not rewrite it. install restarts a running host;
                           with --keep-running it leaves it, and the service runs the next.
  version                  The version.
  local --data <dir> --socket <path> [--worktrees <dir>] [--reviews <dir>] [--idle-exit <secs>]
                           The desktop app's own background service on this computer, in
                           the app's data folder (the app starts it; not for servers).

NEXUS_HOST_DIR moves ~/.nexus/host.";

fn home() -> PathBuf {
    nexus_client::config::home_dir(&|name| std::env::var(name).ok())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|at| args.get(at + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("");
    // A server's agents start this binary for the app's tools (`--mcp`, `--browser-mcp`, `--mcp-launch=`).
    if let Some(code) = nexus_host::mcp::serve(&args) {
        std::process::exit(code);
    }
    if command == "local" {
        std::process::exit(nexus_host::local::main(&args[1..]));
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => fail(&error.to_string()),
    };
    let result = match command {
        "serve" => runtime.block_on(serve(&args)),
        "login" => runtime.block_on(login(&args)),
        "pair" => runtime.block_on(pair(&args)),
        "status" => runtime.block_on(status(&args)),
        "proxy" => runtime.block_on(proxy()),
        "stop" => runtime.block_on(stop()),
        "service" => runtime.block_on(service(&args)),
        "version" | "--version" | "-V" => {
            println!("nexus-host {VERSION}");
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        _ => fail(USAGE),
    };
    if let Err(error) = result {
        fail(&error);
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1)
}

async fn serve(args: &[String]) -> Result<(), String> {
    if nix::unistd::geteuid().is_root() {
        eprintln!(
            "nexus-host: running as root. Agents get the same power as the host's user: run it as your own user instead."
        );
    }
    let listen = match flag(args, "--port") {
        Some(port) => Listen::Port(port.parse().map_err(|_| format!("Not a port: {port}"))?),
        None => Listen::Socket,
    };
    let (stop, stopped) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
            signal(SignalKind::hangup()),
        ) else {
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
            _ = hup.recv() => {}
        }
        let _ = stop.send(true);
    });
    let home = home();
    // The server's own logins and their limits: Auto and rotation on this server read them.
    nexus_host::local::set_plans(nexus_host::gateway::host_plans(Shell::Login));
    nexus_host::serve(
        Paths::from_env(&home),
        home,
        listen,
        stopped,
        Shell::Login,
        |listening| {
            // The first line is for the app's bootstrap script: where to point the tunnel.
            match listening {
                Listening::Socket(path) => println!("listening socket {}", path.display()),
                Listening::Port(port) => println!("listening port {port}"),
            }
        },
    )
    .await
}

async fn login(args: &[String]) -> Result<(), String> {
    let paths = Paths::from_env(&home());
    paths.create()?;
    let saved = Credentials::load(&paths.credentials());
    let gateway = flag(args, "--gateway")
        .or_else(|| saved.as_ref().map(|c| c.gateway.clone()))
        .or_else(|| std::env::var("NEXUS_URL").ok().filter(|url| !url.is_empty()))
        .unwrap_or_else(|| DEFAULT_GATEWAY.to_string());
    let gateway = gateway.trim_end_matches('/').to_string();
    let label = format!("Nexus Code host on {}", machine_name());
    let client = Client::new(client_options(&gateway, None));
    let ui = nexus_client::config::ui_url(&gateway, &|name| std::env::var(name).ok());
    let client_id = Some(nexus_client::device::DESKTOP_CLIENT_ID);
    let code = nexus_client::device::start(&client, &label, client_id, &ui)
        .await
        .map_err(|e| format!("Could not start the sign-in at {gateway}: {e}"))?;
    let pair = args.iter().any(|arg| arg == "--pair");
    let paired = paths.paired();
    let keep_paired = paired.exists();
    println!(
        "To sign this machine in to Nexus, open\n\n  {}\n\nand approve the code {}.",
        code.link, code.user_code
    );
    if pair && let Some(lines) = nexus_client::qr::lines(&code.link) {
        println!("\nOr scan it on your phone:\n");
        for line in lines {
            println!("  {line}");
        }
    }
    let grant = nexus_client::device::wait(&client, &code, || false)
        .await
        .map_err(|e| e.to_string())?;
    let credentials = Credentials {
        gateway,
        token: grant.token.clone(),
        org: grant.org(),
        label,
    };
    credentials.save(&paths.credentials())?;
    println!(
        "Signed in to {}.",
        credentials.org.as_deref().unwrap_or("your organization")
    );
    if pair || keep_paired {
        // The old pairing goes first, with the old token: removing it at Gateway revokes that token.
        if let (Some(old), Some(file)) = (&saved, nexus_host::cloud::CloudFile::load(&paired)) {
            let client = Client::new(client_options(&old.gateway, Some(old.token.clone())));
            if client
                .delete(&format!("/v1/cloud/machines/{}", file.machine_id))
                .await
                .is_err()
            {
                // The old token is already replaced in credentials.json, so nothing can revoke it from here.
                eprintln!("Could not remove the old server at Gateway. Remove it in Nexus Desktop.");
            }
        }
        let client = Client::new(client_options(&credentials.gateway, Some(credentials.token.clone())));
        nexus_host::cloud::pair(&client, &credentials.gateway, &machine_name(), &paired).await?;
        println!("Paired. Nexus Desktop lists this machine as {}.", machine_name());
    }
    if Lock::held(&paths.lock()) {
        println!("Restart the Nexus host for it to use this sign-in.");
    }
    Ok(())
}

/// Pairs this machine's device key (`nexus_host::session_sync::pair`) on the Gateway it is signed in to. The approver's
/// code is confirmed by `--approver-code`, else by a yes on a terminal. `--org-key-fingerprint` confirms the org's
/// recovery key (`session_sync::confirm_org_key`).
async fn pair(args: &[String]) -> Result<(), String> {
    use nexus_host::session_sync::{self, SWITCH};
    use std::io::{BufRead as _, IsTerminal as _, Write as _};
    let paths = Paths::from_env(&home());
    paths.create()?;
    let credentials = Credentials::load(&paths.credentials())
        .ok_or("This machine is not signed in to Nexus: run `nexus-host login` first.")?;
    let client = Client::new(client_options(&credentials.gateway, Some(credentials.token.clone())));
    let name = session_sync::device_name(&machine_name());
    let poll = std::time::Duration::from_secs(3);
    let typed = flag(args, "--approver-code");
    let mut confirm = move |shown: &nexus_workbench::session_sync::approver::Approver| match &typed {
        Some(typed) => session_sync::same_code(typed, &shown.code),
        None if std::io::stdin().is_terminal() => {
            print!("Does \"{}\" show this code? [y/N] ", shown.name);
            let _ = std::io::stdout().flush();
            let mut answer = String::new();
            let _ = std::io::stdin().lock().read_line(&mut answer);
            matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
        }
        None => {
            println!("Run `nexus-host pair --approver-code <the code it shows>` to confirm it.");
            false
        }
    };
    session_sync::pair(
        &client,
        &paths,
        &name,
        poll,
        &mut |line: &str| println!("{line}"),
        &mut confirm,
    )
    .await?;
    // The org's recovery key: pinned only by the flag, on a server with no screen to confirm on.
    session_sync::confirm_org_key(
        &client,
        &paths,
        flag(args, session_sync::ORG_KEY_FLAG).as_deref(),
        &mut |line: &str| println!("{line}"),
    )
    .await?;
    if !session_sync::enabled() {
        println!("Session sync starts when the host runs with {SWITCH}=1.");
    } else if Lock::held(&paths.lock()) {
        println!(
            "The running host starts syncing within {} seconds.",
            session_sync::WATCH.as_secs()
        );
    }
    Ok(())
}

/// How long `status` waits for the running host's own answer.
const LIVE_STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

async fn status(args: &[String]) -> Result<(), String> {
    let paths = Paths::from_env(&home());
    let credentials = Credentials::load(&paths.credentials());
    let running = Lock::held(&paths.lock());
    let mut status = json!({
        "version": VERSION,
        "gateway": credentials.as_ref().map(|c| c.gateway.clone()),
        "org": credentials.as_ref().and_then(|c| c.org.clone()),
        "signedIn": credentials.is_some(),
        "running": running,
        "sessions": [],
    });
    if running {
        // A host that holds its lock but does not answer (stuck, or still starting) must not hold `status` too: the
        // app's install script runs `status --json` to learn whether a host runs, and waits on it.
        let asked = tokio::time::timeout(LIVE_STATUS_TIMEOUT, live_status(&paths))
            .await
            .unwrap_or_else(|_| Err("The running host did not answer in time.".into()));
        match asked {
            Ok(live) => {
                status["sessions"] = live["sessions"].clone();
                // A cloud machine's idle timer (its idle script) reads these to decide when to sleep the
                // machine. Copied only when the running host reports them, so an older host leaves them out and the
                // timer keeps the machine awake.
                for key in ["runningTurns", "idleSince"] {
                    if let Some(value) = live.get(key) {
                        status[key] = value.clone();
                    }
                }
            }
            Err(error) => status["error"] = Value::String(error),
        }
    }
    if args.iter().any(|arg| arg == "--json") {
        println!("{status}");
        return Ok(());
    }
    println!("nexus-host {VERSION}");
    match &credentials {
        Some(c) => println!(
            "Signed in to {} at {}.",
            c.org.as_deref().unwrap_or("an organization"),
            c.gateway
        ),
        None => println!("Not signed in: run `nexus-host login`."),
    }
    if !running {
        println!("Not running.");
        return Ok(());
    }
    let sessions = status["sessions"].as_array().cloned().unwrap_or_default();
    println!("Running, {} active session(s).", sessions.len());
    for session in sessions {
        println!(
            "  {}  {}  {}",
            session["id"].as_str().unwrap_or(""),
            session["status"].as_str().unwrap_or(""),
            session["title"].as_str().unwrap_or("")
        );
    }
    if let Some(error) = status["error"].as_str() {
        println!("Could not ask the running host: {error}");
    }
    Ok(())
}

/// The running host's socket (or port) on standard input and output, until either side closes: the desktop app's
/// tunnel, `ssh <server> nexus-host proxy`. Works with any version of the host, which is how the app reaches an older
/// one to learn that it needs an update.
async fn proxy() -> Result<(), String> {
    let paths = Paths::from_env(&home());
    if !Lock::held(&paths.lock()) {
        return Err("The Nexus host is not running.".into());
    }
    let mut stdio = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
    let copied = if let Ok(port) = std::fs::read_to_string(paths.port()) {
        let port: u16 = port.trim().parse().map_err(|_| "Unreadable port file.".to_string())?;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .map_err(|e| format!("Could not reach the Nexus host: {e}"))?;
        tokio::io::copy_bidirectional(&mut stdio, &mut stream).await
    } else {
        let mut stream = tokio::net::UnixStream::connect(paths.socket())
            .await
            .map_err(|e| format!("Could not reach the Nexus host: {e}"))?;
        tokio::io::copy_bidirectional(&mut stdio, &mut stream).await
    };
    // The other side going away is how a proxy ends.
    let _ = copied;
    Ok(())
}

/// Sends the running host SIGTERM (it stops its agents as quitting the app does) and waits up to a minute for it to
/// let go of its lock.
async fn stop() -> Result<(), String> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    let paths = Paths::from_env(&home());
    // A pid that cannot be read while the lock is held (a host from before pids were written, or a read between
    // truncate and write) is not "not running": wait a moment, then say so.
    let mut holder = Lock::holder(&paths.lock());
    for _ in 0..10 {
        if holder.is_some() || !Lock::held(&paths.lock()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        holder = Lock::holder(&paths.lock());
    }
    let Some(pid) = holder else {
        if Lock::held(&paths.lock()) {
            return Err("A Nexus host is running but its process id cannot be read.".into());
        }
        println!("Not running.");
        return Ok(());
    };
    kill(Pid::from_raw(pid), Signal::SIGTERM).map_err(|e| format!("Could not stop the Nexus host ({pid}): {e}"))?;
    for _ in 0..600 {
        if !Lock::held(&paths.lock()) {
            println!("Stopped.");
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(format!("The Nexus host ({pid}) did not stop within a minute."))
}

/// `service install | uninstall | status [--json]` (src/service.rs). `install` stops a host the service does not run,
/// so only one host holds the root; the service then starts it again.
async fn service(args: &[String]) -> Result<(), String> {
    use nexus_host::service::{Service, describe};
    let home = home();
    let paths = Paths::from_env(&home);
    let mut service = Service {
        root: paths.dir.clone(),
        home: home.clone(),
        path: String::new(),
    };
    let lines = match args.get(1).map(String::as_str) {
        Some("install") => {
            service.check_home()?;
            service.check_ours()?;
            let version = service.version_dir(std::env::current_exe().ok().as_deref())?;
            // The login shell's PATH, as a host started over SSH has: agents find the same tools.
            service.path = nexus_workbench::process::agent_path(
                std::env::var("PATH").ok().as_deref(),
                Some(home.as_path()),
                &Shell::Login,
            );
            let outside = Lock::held(&paths.lock()) && !service.active();
            if outside && args.iter().any(|arg| arg == "--keep-running") {
                // The app's "Start on boot": the running host's sessions go on.
                service.enable(&version)?
            } else {
                if outside {
                    println!("Stopping the running Nexus host, so that the service runs it from now on.");
                    stop().await?;
                }
                service.install(&version)?
            }
        }
        Some("uninstall") => service.uninstall()?,
        Some("status") => {
            let status = service.status();
            if args.iter().any(|arg| arg == "--json") {
                vec![status.to_string()]
            } else {
                describe(&status)
            }
        }
        _ => return Err(USAGE.into()),
    };
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

/// Asks the running host, through its socket or port, for `host_status`.
async fn live_status(paths: &Paths) -> Result<Value, String> {
    let token = std::fs::read_to_string(paths.token()).map_err(|e| e.to_string())?;
    let token = token.trim();
    if let Ok(port) = std::fs::read_to_string(paths.port()) {
        let port: u16 = port.trim().parse().map_err(|_| "Unreadable port file.".to_string())?;
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .map_err(|e| e.to_string())?;
        let mut connection = nexus_host::client::connect(stream, token).await?;
        connection.call("host_status", Value::Null).await.0
    } else {
        let stream = tokio::net::UnixStream::connect(paths.socket())
            .await
            .map_err(|e| e.to_string())?;
        let mut connection = nexus_host::client::connect(stream, token).await?;
        connection.call("host_status", Value::Null).await.0
    }
}
