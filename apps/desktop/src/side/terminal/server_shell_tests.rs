//! A server shell against a real host's shells (`nexus_workbench::shell_pty`) in this process, with no network.

use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use nexus_workbench::process::Shell;
use nexus_workbench::shell_pty::{Limits, Shells};

use super::*;

/// The host's shells, answering as a server would; every `fail_every`th read is lost on the way, as on a dropped
/// connection.
struct Fake {
    shells: Shells,
    runtime: tokio::runtime::Runtime,
    fail_every: usize,
    reads: AtomicUsize,
}

impl Fake {
    fn new(fail_every: usize) -> Arc<Fake> {
        Arc::new(Fake {
            shells: Shells::new(Shell::sh(), Arc::new(String::new), Limits::default()),
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("a runtime"),
            fail_every,
            reads: AtomicUsize::new(0),
        })
    }
}

impl ShellHost for Fake {
    fn call(&self, call: &'static str, args: Value) -> BoxFuture<'static, Result<Value, String>> {
        if call == shell_pty::READ
            && self.fail_every > 0
            && self.reads.fetch_add(1, Ordering::Relaxed) % self.fail_every == 1
        {
            return Box::pin(async { Err("Lost the connection to the machine.".to_string()) });
        }
        let shells = self.shells.clone();
        let task = self.runtime.spawn(async move {
            shells
                .answer(call, args, |_, project| {
                    assert_eq!(project, Some("p1"));
                    Ok(std::env::temp_dir())
                })
                .await
        });
        Box::pin(async move { task.await.map_err(|e| e.to_string())? })
    }
}

#[derive(Default)]
struct Seen {
    text: String,
    exited: Option<Option<u32>>,
    problems: Vec<Option<String>>,
}

fn start(fake: &Arc<Fake>, command: Option<&str>) -> (ServerShell, Arc<Mutex<Seen>>) {
    start_on(fake.clone(), "p1", command)
}

fn start_on(host: Arc<dyn ShellHost>, project: &str, command: Option<&str>) -> (ServerShell, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let target = ServerTarget {
        host,
        thread: None,
        project: Some(project.into()),
        command: command.map(str::to_string),
    };
    let heard = seen.clone();
    let shell = ServerShell::start(&target, 80, 24, move |event| {
        let mut seen = heard.lock().unwrap();
        match event {
            Event::Output { bytes, .. } => seen.text.push_str(&String::from_utf8_lossy(&bytes)),
            Event::Exited(code) => seen.exited = Some(code),
            Event::Problem(problem) => seen.problems.push(problem),
        }
    });
    (shell, seen)
}

fn wait_for(seen: &Arc<Mutex<Seen>>, done: impl Fn(&Seen) -> bool) {
    let started = Instant::now();
    while !done(&seen.lock().unwrap()) {
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "gave up on {:?}",
            seen.lock().unwrap().text
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn typing_reaches_the_server_in_order_and_its_output_comes_back() {
    let fake = Fake::new(0);
    let (shell, seen) = start(&fake, None);
    // Quoted apart: only the output, not the echo of the typed line, holds each marker.
    for n in 1..=20 {
        shell.input(format!("echo \"n-\"\"{n}-\"\n").into_bytes());
    }
    shell.resize(120, 30);
    shell.input(b"echo \"size:$(stty size)\"\n".to_vec());
    wait_for(&seen, |seen| seen.text.contains("size:30 120"));
    let text = seen.lock().unwrap().text.clone();
    let places: Vec<usize> = (1..=20)
        .map(|n| {
            text.find(&format!("n-{n}-\r\n"))
                .unwrap_or_else(|| panic!("n-{n} in {text:?}"))
        })
        .collect();
    assert!(places.windows(2).all(|pair| pair[0] < pair[1]), "{text:?}");
    drop(shell);
}

#[test]
fn a_lost_read_reads_on_from_its_cursor_without_loss_or_repeats() {
    let fake = Fake::new(2);
    let (_shell, seen) = start(
        &fake,
        Some("for i in 1 2 3 4 5 6; do echo line-$i; sleep 0.15; done; exit 4"),
    );
    wait_for(&seen, |seen| seen.exited.is_some());
    let seen = seen.lock().unwrap();
    for i in 1..=6 {
        assert_eq!(
            seen.text.matches(&format!("line-{i}\r\n")).count(),
            1,
            "{:?}",
            seen.text
        );
    }
    assert_eq!(seen.exited, Some(Some(4)));
    assert!(seen.problems.first().is_some_and(Option::is_some), "the loss shows");
    assert_eq!(seen.problems.last(), Some(&None), "and clears once reads come back");
}

#[test]
fn dropping_the_shell_closes_it_on_the_server() {
    let fake = Fake::new(0);
    let (shell, seen) = start(&fake, None);
    shell.input(b"echo ready\n".to_vec());
    wait_for(&seen, |seen| seen.text.contains("ready\r\n"));
    assert_eq!(fake.shells.len(), 1);
    drop(shell);
    let started = Instant::now();
    while !fake.shells.is_empty() {
        assert!(started.elapsed() < Duration::from_secs(5), "still open");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A real host through its forwarded socket, as the app's own client reaches it (`nexus_remote::client`).
struct RealHost {
    connection: Arc<nexus_remote::client::Connection>,
    runtime: tokio::runtime::Runtime,
}

impl ShellHost for RealHost {
    fn call(&self, call: &'static str, args: Value) -> BoxFuture<'static, Result<Value, String>> {
        let connection = self.connection.clone();
        let task = self.runtime.spawn(async move { connection.call(call, args).await });
        Box::pin(async move { task.await.map_err(|e| e.to_string())? })
    }
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
}

/// This screen's shell on a real server, run by hand: the host and the forwarded socket as in nexus-host's
/// `tests/shell_e2e.rs`. `NEXUS_E2E_COMMAND` runs a command (a project command) instead of an interactive shell,
/// `NEXUS_E2E_SHELL` runs that as the interactive shell instead of the login shell.
#[test]
#[ignore = "needs a real host: see nexus-host's tests/shell_e2e.rs"]
fn a_server_shell_on_a_real_host() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let (connection, host, _pushes) = runtime.block_on(async {
        let stream = tokio::net::UnixStream::connect(env("NEXUS_E2E_SOCKET"))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        nexus_remote::client::open(Box::new(stream), &env("NEXUS_E2E_TOKEN"), None)
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    });
    println!("== hello from {} {}: {:?}", host.machine, host.version, host.features);
    assert!(host.features.iter().any(|f| f == shell_pty::FEATURE));
    let project = runtime
        .block_on(connection.call("wb_project_add", json!({ "path": env("NEXUS_E2E_REPO") })))
        .unwrap_or_else(|e| panic!("{e}"));
    let real = Arc::new(RealHost {
        connection: Arc::new(connection),
        runtime,
    });
    let command = std::env::var("NEXUS_E2E_COMMAND").ok();
    let interactive = command.is_none();
    let command = command.or_else(|| std::env::var("NEXUS_E2E_SHELL").ok());
    let (shell, seen) = start_on(real, project["id"].as_str().unwrap_or_default(), command.as_deref());
    if interactive {
        shell.resize(100, 30);
        shell.input(b"pwd; echo $TERM; tput cols; exit 3\n".to_vec());
    }
    wait_for(&seen, |seen| seen.exited.is_some());
    let seen = seen.lock().unwrap();
    println!(
        "== {}: exited {:?}, output:\n{}",
        command.as_deref().unwrap_or("the login shell"),
        seen.exited,
        format!("{:?}", seen.text).replace("\\r\\n", "\n")
    );
    assert!(seen.text.contains(&env("NEXUS_E2E_REPO")));
}
