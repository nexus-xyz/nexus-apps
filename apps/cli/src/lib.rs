//! The `nexus` CLI as one Rust binary: a drop-in replacement for the TypeScript CLI (`@nexus/cli`), command for command
//! against the same `commands.json` catalog, with the same output, exit codes (0 success, 1 failure, 2 usage), config
//! file and headers. It is a client of the Nexus API (HTTP only), not an SDK.
//!
//! Arguments are read with the TypeScript CLI's own grammar (`parseArgv`: `--name value`, `--name=value`, a fixed
//! set of switches, everything else positional), not a derive parser: product commands (`nexus <product> <verb>
//! --<input> value`) are discovered from Gateway at run time, and unknown flags, `--help` anywhere and the error texts
//! must behave exactly as they do in TypeScript.

pub mod account;
pub mod after_the_fact;
pub mod agent_capabilities;
pub mod agent_chat;
pub mod agent_routes;
pub mod agent_share;
pub mod agents;
pub mod antigravity_usage;
pub mod apps;
pub mod args;
pub mod artifacts;
pub mod audit;
pub mod billing;
pub mod budgets;
pub mod byo;
pub mod catalog;
pub mod chat;
pub mod claude_transcript;
pub mod cli;
pub mod client;
#[cfg(unix)]
pub mod codex_remote;
pub mod completion;
pub mod connections;
pub mod design_systems;
pub mod device_key;
pub mod devices;
pub mod error;
pub mod exports;
pub mod images;
pub mod jobs;
pub mod json;
pub mod local_review;
pub mod machines;
pub mod mcp;
pub mod members;
pub mod models;
pub mod module_status;
pub mod notice;
pub mod notifications;
pub mod org;
pub mod pages;
pub mod phone_control;
pub mod picker;
#[cfg(feature = "plans")]
pub mod plans;
pub mod promotion_verbs;
pub mod provider_keys;
pub mod recipes;
pub mod recordings;
pub mod routines;
pub mod safe_text;
pub mod search;
pub mod service_accounts;
pub mod session_log;
pub mod sessions;
mod sessions_wait;
mod shared_memory;
pub mod skill_errors;
pub mod skill_library;
pub mod skills;
pub mod skills_delivery;
pub mod slack;
pub mod spend;
pub mod style;
pub mod teams;
pub mod text_diff;
pub mod tokens;
mod tui;
pub mod unanswered;
pub mod update;
pub mod usage_signals;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;

pub use cli::run_cli;
/// The config file and Nexus UI's links, shared with the other native clients (`nexus-client`).
pub use nexus_client::config;

/// This CLI's version: `<semver>+<git short sha>`, e.g. `0.2.0+abc1234` (build.rs).
pub const VERSION: &str = env!("NEXUS_CLI_VERSION");
/// The semver alone, as `commands.json` records it.
pub const SEMVER: &str = env!("NEXUS_CLI_SEMVER");

/// Where output goes: the terminal, or a test's buffers.
pub trait Io {
    /// One line on stdout.
    fn out(&self, line: &str);
    /// One line on stderr.
    fn err(&self, line: &str);
    /// Raw text on stdout without a newline (the chat answer as it streams); `false` when this output cannot stream.
    fn write(&self, text: &str) -> bool;
    /// A transient status line while waiting (a spinner on a terminal); `None` clears it.
    fn progress(&self, text: Option<&str>);
    /// All of stdin (`nexus auth login --with-token`); empty on a terminal.
    fn read_stdin(&self) -> String;
    /// One line of stdin without its line end (`nexus agent chat`); `None` at its end.
    fn read_line(&self) -> Option<String>;
    /// Open a URL in the browser; `false` when this run cannot (not a terminal, CI, a test).
    fn open_url(&self, url: &str) -> bool;
    /// Run the person's editor (`$VISUAL` or `$EDITOR`, split on spaces) on `path` and wait for it to end
    /// (`nexus skills create`). Answers its exit code; `Err` when it cannot start.
    fn edit(&self, command: &[String], path: &std::path::Path) -> std::io::Result<i32>;
}

/// Whether a link may reach the OS opener: `https://`, or `http://` on this machine (a local Nexus UI). Gateway
/// sends Stripe's links as Stripe made them, so any other scheme is printed, never opened. The link is parsed as a
/// browser reads it: no user part (`http://localhost:1@evil.test` is evil.test), no control character,
/// and it must parse.
pub fn openable(link: &str) -> bool {
    if link.chars().any(char::is_control) || !(link.starts_with("https://") || link.starts_with("http://")) {
        return false;
    }
    let Ok(parsed) = url::Url::parse(link) else {
        return false;
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return false;
    }
    match parsed.scheme() {
        "https" => parsed.host_str().is_some_and(|host| !host.is_empty()),
        "http" => matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    }
}

/// Whether `link` has the origin (scheme, host and port) of one of `origins`: `nexus auth login` opens Gateway's
/// approval link only on its own Gateway or Nexus UI, else prints it (PT-12).
pub fn same_origin(link: &str, origins: &[&str]) -> bool {
    let Ok(link) = url::Url::parse(link) else {
        return false;
    };
    origins
        .iter()
        .filter_map(|origin| url::Url::parse(origin).ok())
        .any(|origin| origin.origin() == link.origin())
}

/// How an OS opens a link. No shell reads it: the link is one argument, as it came.
#[derive(Debug, PartialEq, Eq)]
pub enum LinkOpener<'a> {
    /// `open` (macOS) or `xdg-open`: the program, and the link as its only argument.
    Program(&'static str, &'a str),
    /// Windows: ShellExecuteW opens the link itself. Not `cmd /c start`, whose cmd.exe reads `&`, `|` and `^` in a
    /// link as command separators and expands `%NAME%`.
    ShellExecute(&'a str),
}

/// The opener for `os` (`std::env::consts::OS`).
pub fn link_opener<'a>(os: &str, link: &'a str) -> LinkOpener<'a> {
    match os {
        "macos" => LinkOpener::Program("open", link),
        "windows" => LinkOpener::ShellExecute(link),
        _ => LinkOpener::Program("xdg-open", link),
    }
}

/// Everything a run depends on besides its arguments.
pub struct Context {
    pub config_path: PathBuf,
    pub io: Box<dyn Io>,
    /// Colors and symbols; off: plain text.
    pub color: bool,
    /// The environment (the process's in the binary; a test's own).
    pub env: HashMap<String, String>,
    /// Delay between polls in milliseconds; tests set it to 0.
    pub poll_ms: Option<u64>,
    /// This machine's name, for the device login's label.
    pub hostname: Option<String>,
    pub cwd: PathBuf,
    /// Whether stdin is a terminal (`nexus claude`: Ctrl-C reaches the agent by itself).
    pub interactive: bool,
    /// The running binary (`nexus update`).
    pub exe_path: Option<PathBuf>,
    /// The terminal's width, when stdout is one: long rows give way with `…`. `None`: nothing is truncated.
    pub columns: Option<usize>,
    /// Whether stdin and stdout are both a terminal, so a picker can run (`nexus models pick`).
    pub picker: bool,
}

impl Context {
    pub fn var(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }

    pub fn env_fn(&self) -> impl Fn(&str) -> Option<String> + '_ {
        move |name| self.env.get(name).cloned()
    }
}

/// Stdout and stderr of the process.
#[derive(Debug, Default)]
pub struct Terminal {
    pub stdin_is_tty: bool,
    pub can_open: bool,
    pub spinner: Option<Spinner>,
}

impl Io for Terminal {
    fn out(&self, line: &str) {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{line}");
    }
    fn err(&self, line: &str) {
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{line}");
    }
    fn write(&self, text: &str) -> bool {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
        true
    }
    fn progress(&self, text: Option<&str>) {
        if let Some(spinner) = &self.spinner {
            spinner.set(text);
        }
    }
    fn read_stdin(&self) -> String {
        if self.stdin_is_tty {
            return String::new();
        }
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text);
        text
    }
    fn read_line(&self) -> Option<String> {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_string()),
        }
    }
    fn open_url(&self, url: &str) -> bool {
        if !self.can_open || !openable(url) {
            return false;
        }
        // Detached and silent: a failure only means the printed link is the way in.
        match link_opener(std::env::consts::OS, url) {
            LinkOpener::Program(program, link) => {
                let _ = std::process::Command::new(program)
                    .arg(link)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
            #[cfg(windows)]
            LinkOpener::ShellExecute(link) => {
                let _ = opener::open(link);
            }
            #[cfg(not(windows))]
            LinkOpener::ShellExecute(_) => {}
        }
        true
    }
    fn edit(&self, command: &[String], path: &std::path::Path) -> std::io::Result<i32> {
        let Some((program, args)) = command.split_first() else {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "no editor"));
        };
        let status = std::process::Command::new(program).args(args).arg(path).status()?;
        Ok(status.code().unwrap_or(1))
    }
}

/// A one-line spinner on stderr while a run or a login is polled: only on a terminal, never with --json.
#[derive(Debug)]
pub struct Spinner {
    state: std::sync::Arc<std::sync::Mutex<SpinnerState>>,
    color: bool,
}

#[derive(Debug, Default)]
struct SpinnerState {
    text: Option<String>,
    running: bool,
    generation: u64,
}

/// Brand v5.0's terminal spinner: braille dots turning, 100 ms a frame.
const FRAMES: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];

impl Spinner {
    pub fn new(color: bool) -> Spinner {
        Spinner {
            state: Default::default(),
            color,
        }
    }

    fn set(&self, text: Option<&str>) {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match text {
            None => {
                if state.running {
                    state.running = false;
                    state.generation += 1;
                    let mut stderr = std::io::stderr().lock();
                    let _ = write!(stderr, "\r\x1b[2K");
                    let _ = stderr.flush();
                }
                state.text = None;
            }
            Some(text) => {
                state.text = Some(text.to_string());
                if !state.running {
                    state.running = true;
                    state.generation += 1;
                    let generation = state.generation;
                    let shared = self.state.clone();
                    let color = self.color;
                    std::thread::spawn(move || spin(shared, generation, color));
                }
            }
        }
    }
}

fn spin(state: std::sync::Arc<std::sync::Mutex<SpinnerState>>, generation: u64, color: bool) {
    let started = std::time::Instant::now();
    let mut frame = 0usize;
    loop {
        {
            let state = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.running || state.generation != generation {
                return;
            }
            let elapsed = format!("{}s", style::to_fixed(started.elapsed().as_secs_f64(), 1));
            let glyph = FRAMES[frame % FRAMES.len()];
            frame += 1;
            let text = state.text.clone().unwrap_or_default();
            let line = if color {
                format!("\r\x1b[2K{glyph} {text} \x1b[2m{elapsed}\x1b[22m")
            } else {
                format!("\r\x1b[2K{glyph} {text} {elapsed}")
            };
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(line.as_bytes());
            let _ = stderr.flush();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Output kept in memory, for tests.
#[derive(Debug, Default)]
pub struct Captured {
    pub out: RefCell<Vec<String>>,
    pub err: RefCell<Vec<String>>,
    pub written: RefCell<String>,
    pub stdin: String,
    /// The lines of `stdin` that `read_line` has given.
    pub lines_read: RefCell<usize>,
    pub opened: RefCell<Vec<String>>,
    /// Whether `open_url` succeeds.
    pub can_open: bool,
    /// Whether `write` streams.
    pub can_stream: bool,
    /// The text the editor saves (`edit`); `None`: it saves the file as it found it.
    pub edit_saves: Option<String>,
    /// The editor's exit code.
    pub edit_exit: i32,
    /// Each editor run: the command and the file's text when it opened.
    pub edited: RefCell<Vec<(Vec<String>, String)>>,
}

/// A shared handle on `Captured`, so a test keeps reading after the context takes it.
#[derive(Debug, Clone, Default)]
pub struct Capture(pub std::rc::Rc<Captured>);

impl Io for Capture {
    fn out(&self, line: &str) {
        self.0.out.borrow_mut().push(line.to_string());
    }
    fn err(&self, line: &str) {
        self.0.err.borrow_mut().push(line.to_string());
    }
    fn write(&self, text: &str) -> bool {
        if self.0.can_stream {
            self.0.written.borrow_mut().push_str(text);
        }
        self.0.can_stream
    }
    fn progress(&self, _text: Option<&str>) {}
    fn read_stdin(&self) -> String {
        self.0.stdin.clone()
    }
    fn read_line(&self) -> Option<String> {
        let mut read = self.0.lines_read.borrow_mut();
        let line = self.0.stdin.lines().nth(*read)?;
        *read += 1;
        Some(line.to_string())
    }
    fn open_url(&self, url: &str) -> bool {
        if self.0.can_open {
            self.0.opened.borrow_mut().push(url.to_string());
        }
        self.0.can_open
    }
    fn edit(&self, command: &[String], path: &std::path::Path) -> std::io::Result<i32> {
        self.0
            .edited
            .borrow_mut()
            .push((command.to_vec(), std::fs::read_to_string(path)?));
        if let Some(text) = &self.0.edit_saves {
            std::fs::write(path, text)?;
        }
        Ok(self.0.edit_exit)
    }
}
