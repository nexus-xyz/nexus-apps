//! `nexus`: the CLI's entry point (`main.ts`). Runs one command, then, at most once a day, the update notice.

use std::io::IsTerminal as _;

use nexus_cli::config::{DEFAULT_URL, default_config_path, load_config};
use nexus_cli::notice::{notice_suppressed, published_version, update_cache_path, update_notice};
use nexus_cli::style::{now_ms, should_color};
use nexus_cli::{Context, Spinner, Terminal, VERSION, run_cli};

fn main() {
    #[cfg(windows)]
    match nexus_cli::update::launch_windows_install() {
        Ok(Some(code)) => std::process::exit(code),
        Err(error) => {
            eprintln!("Could not start the installed Windows CLI: {error}");
            std::process::exit(1);
        }
        Ok(None) => {}
    }
    // Keep the version marker in the binary: Gateway and `nexus update --rollback` read it (`update::binary_version`).
    std::hint::black_box(nexus_cli::update::VERSION_MARKER);
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let env: std::collections::HashMap<String, String> = std::env::vars().collect();
    let get = |name: &str| env.get(name).cloned();
    let stdout_tty = std::io::stdout().is_terminal();
    let stderr_tty = std::io::stderr().is_terminal();
    let stdin_tty = std::io::stdin().is_terminal();
    let color = should_color(stdout_tty, &get, &argv);
    let config_path = default_config_path(&get);
    let ci = get("CI").is_some_and(|ci| !ci.is_empty());
    let spinner = (stderr_tty && !argv.iter().any(|arg| arg == "--json")).then(|| Spinner::new(color));

    let context = Context {
        config_path: config_path.clone(),
        io: Box::new(Terminal {
            stdin_is_tty: stdin_tty,
            can_open: stdout_tty && !ci,
            spinner,
        }),
        color,
        env: env.clone(),
        poll_ms: None,
        hostname: hostname(),
        cwd: std::env::current_dir().unwrap_or_default(),
        interactive: stdin_tty,
        exe_path: std::env::current_exe().ok(),
        columns: stdout_tty
            .then(|| crossterm::terminal::size().ok())
            .flatten()
            .map(|(columns, _)| usize::from(columns))
            .filter(|columns| *columns > 0),
        picker: stdin_tty && stdout_tty,
    };
    // The usage signals go from the binary only, each on its own thread; no command waits for one.
    nexus_cli::usage_signals::enable();
    // Completion, the agent protocol and flags alone stay local and instant: they read no config and send nothing.
    if !nexus_cli::usage_signals::local_only(&argv) {
        let saved = load_config(&config_path, &get);
        if let Some(key) = saved.api_key.as_deref().filter(|key| !key.is_empty()) {
            let _ = nexus_cli::usage_signals::flush_errors(&config_path, &saved.url, key);
            if stdin_tty && stdout_tty && nexus_cli::usage_signals::counts_as_command(&argv) {
                let _ = nexus_cli::usage_signals::command_beat(&config_path, &saved.url, key);
            }
        }
    }
    let code = run_cli(&argv, &context);

    // After the command's output, at most once a day, one line when the Gateway publishes a newer CLI.
    if !notice_suppressed(&argv, &get, stdout_tty) {
        let saved = load_config(&config_path, &get);
        let core = if saved.url.is_empty() {
            DEFAULT_URL.to_string()
        } else {
            saved.url
        };
        if let Some(notice) = update_notice(
            &update_cache_path(&config_path),
            &core,
            VERSION,
            now_ms(),
            &published_version,
        ) {
            eprintln!(
                "{}",
                if color {
                    format!("\x1b[2m{notice}\x1b[22m")
                } else {
                    notice
                }
            );
        }
    }
    std::process::exit(code);
}

/// This machine's name, for the device login's label (`os.hostname()`).
fn hostname() -> Option<String> {
    let output = std::process::Command::new("hostname").output().ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}
