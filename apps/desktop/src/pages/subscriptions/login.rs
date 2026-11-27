//! Nexus runs the agent's own sign-in, so the person does not type a command: `claude auth login --claudeai` and
//! `codex login` (or `codex login --device-auth`), with the account's folder in the environment. The login opens the
//! browser itself. Nexus reads what it prints, line by line: the sign-in link, Codex's one-time code, and the last
//! line it said, for an error. Unknown output is never fatal: the page waits for the login to end, or for the account
//! to read as signed in.
//!
//! Checked on 2026-11-17 against Claude Code 2.1.323 and codex-cli 0.169.0, with temporary folders, no terminal and
//! stdin closed. Claude's login keeps waiting for a pasted code after it prints its link; Codex's browser login serves
//! a callback on port 1455, and its device login prints a link and a code that expires in 15 minutes.
//!
//! A link is offered or opened only when it is https on a vendor's sign-in host ([`vetted`]). A pasted code goes to
//! the login's stdin once, and Nexus keeps no copy.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use nexus_workbench::accounts;
use nexus_workbench::event::SessionAgent;
use nexus_workbench::plans::PlanUsage;
use nexus_workbench::process::{self, Spawn};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// How long a sign-in may wait for the person.
pub(crate) const DEADLINE: Duration = Duration::from_secs(15 * 60);
/// A Codex browser login that fails this soon could not serve its callback: Nexus tries the device code instead.
const EARLY: Duration = Duration::from_secs(20);
/// How long to read what a login printed after it ended.
const DRAIN: Duration = Duration::from_millis(500);
/// What Codex's device code lasts when its line does not say.
const CODE_MINUTES: u64 = 15;

/// The hosts a sign-in link may point to, and their subdomains.
const HOSTS: [&str; 3] = ["claude.com", "claude.ai", "auth.openai.com"];

/// How the login reaches the browser. Claude has only the browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The login opens the browser, and the vendor calls it back.
    Browser,
    /// Codex prints a link and a one-time code to enter there.
    DeviceCode,
}

/// What the login said or did.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// The login started, in this mode. A Codex login that falls back to the device code starts again.
    Started(Mode),
    /// A vetted sign-in link.
    Link(String),
    /// Codex's one-time code, and when it expires.
    Code { code: String, expires_at: Instant },
    /// A line that is not the login's usual chatter: the last one explains a failure.
    Line(String),
    /// The login ended.
    Exited(Exit),
}

/// How the login ended.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Exit {
    /// Its exit code (None: killed by a signal).
    Status(Option<i32>),
    /// Cancelled, or the page closed.
    Cancelled,
    /// The deadline passed first.
    TimedOut,
    /// It could not start.
    NotStarted(String),
}

impl Exit {
    pub(crate) fn succeeded(&self) -> bool {
        *self == Exit::Status(Some(0))
    }
}

/// One sign-in to run.
#[derive(Clone, Debug)]
pub(crate) struct Options {
    pub agent: SessionAgent,
    /// The agent's program.
    pub program: PathBuf,
    /// Arguments before the agent's own: a wrapper's (a test runs its fake through `/bin/sh`). Empty for the agent.
    pub program_args: Vec<String>,
    /// The environment, without what bills elsewhere (`plan_env`).
    pub env: HashMap<String, String>,
    /// The extra account's folder; None for the agent's own login.
    pub dir: Option<PathBuf>,
    pub cwd: PathBuf,
    /// Sign in again on a known email: Claude pre-fills it.
    pub email: Option<String>,
    pub mode: Mode,
    pub deadline: Duration,
}

/// Claude Code asks macOS to save a new login in the default keychain. If there is none,
/// starting OAuth can raise a system-wide reset prompt. A fresh lookup sees repairs on retry.
pub(super) fn check_login_storage(agent: SessionAgent) -> Result<(), String> {
    if cfg!(target_os = "macos") && agent == SessionAgent::Claude {
        use std::process::Stdio;
        let status = process::command("/usr/bin/security")
            .args(["default-keychain", "-d", "user"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !status.is_ok_and(|status| status.success()) {
            return Err("This Mac can’t save a Claude sign-in right now.".into());
        }
    }
    Ok(())
}

/// The login's arguments.
pub(crate) fn args(agent: SessionAgent, email: Option<&str>, mode: Mode) -> Result<Vec<String>, String> {
    crate::agents::ui(agent).login_args(email, mode)
}

/// What to start: the account's folder goes in the agent's variable; the agent's own login keeps its default.
pub(crate) fn command(options: &Options, mode: Mode) -> Result<Spawn, String> {
    let mut env = options.env.clone();
    if let Some(dir) = &options.dir
        && let Some(variable) = accounts::account_variable(options.agent)
    {
        env.insert(variable.into(), dir.to_string_lossy().into_owned());
    }
    let mut args = options.program_args.clone();
    args.extend(self::args(options.agent, options.email.as_deref(), mode)?);
    Ok(Spawn {
        program: options.program.clone(),
        args,
        env,
        cwd: options.cwd.clone(),
    })
}

// ---------------------------------------------------------------- reading what the login prints

/// `text` without terminal escapes (colors, cursor moves, titles) and control characters. A carriage return keeps
/// what follows it, as a terminal would show it.
pub(crate) fn strip_ansi(text: &str) -> String {
    let text = text.trim_end_matches(['\r', '\n']);
    let text = text.rsplit('\r').find(|part| !part.trim().is_empty()).unwrap_or("");
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            if !c.is_control() || c == '\t' {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            // CSI: parameters, then one final byte from '@' to '~'.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC '\'.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next_if_eq(&'\\');
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `url` when it is https on a sign-in host, with no user or port of its own; None otherwise.
pub(crate) fn vetted(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    if parsed.port().is_some() {
        return None;
    }
    let host = match parsed.host()? {
        url::Host::Domain(host) => host.to_ascii_lowercase(),
        _ => return None,
    };
    HOSTS
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
        .then(|| parsed.to_string())
}

/// The first web address in `line`, without the punctuation a sentence puts after it.
fn find_url(line: &str) -> Option<&str> {
    let start = line.find("https://").or_else(|| line.find("http://"))?;
    let rest = &line[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some(rest[..end].trim_end_matches(['.', ',', ')', '\'', '"', '>', ']']))
}

/// A device code as Codex prints it: `NJSZ-6I2YS`.
fn code_like(text: &str) -> bool {
    (6..=24).contains(&text.len())
        && text.contains('-')
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && text.chars().any(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
}

/// The minutes in "expires in 15 minutes".
fn expiry_minutes(lower: &str) -> Option<u64> {
    let after = &lower[lower.find("expires in ")? + "expires in ".len()..];
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .ok()
        .filter(|_| after[digits.len()..].trim_start().starts_with("min"))
}

/// The login's usual chatter: none of it explains a failure.
fn chatter(lower: &str) -> bool {
    const SAID: [&str; 12] = [
        "opening browser",
        "if the browser didn't open",
        "paste code here",
        "starting local login server",
        "if your browser did not open",
        "on a remote or headless machine",
        "open this link in your browser",
        "enter this one-time code",
        "welcome to codex",
        "warning:",
        "follow these steps",
        "never share this code",
    ];
    SAID.iter().any(|said| {
        lower
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
            .starts_with(said)
    })
}

/// Whether Codex could not serve its browser callback: its port is taken, or it could not bind.
pub(crate) fn port_error(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("address already in use")
        || lower.contains("addrinuse")
        || lower.contains("failed to bind")
        || (lower.contains("port") && (lower.contains("in use") || lower.contains("bind")))
}

/// Reads a login's output a line at a time. Pure: the time comes in, so a test feeds the transcripts it recorded.
#[derive(Debug, Default)]
pub(crate) struct Parser {
    /// After "Enter this one-time code (expires in N minutes)": the code is the next line, and lasts N minutes.
    code_for: Option<u64>,
}

impl Parser {
    pub(crate) fn feed(&mut self, raw: &str, now: Instant) -> Vec<Event> {
        let clean = strip_ansi(raw);
        // Claude's prompt has no line break, so its next line starts after it.
        let line = clean
            .trim()
            .strip_prefix("Paste code here if prompted >")
            .unwrap_or(clean.trim())
            .trim();
        if line.is_empty() {
            return vec![];
        }
        if let Some(minutes) = self.code_for
            && code_like(line)
        {
            self.code_for = None;
            return vec![Event::Code {
                code: line.into(),
                expires_at: now + Duration::from_secs(minutes * 60),
            }];
        }
        let lower = line.to_lowercase();
        if lower.contains("one-time code") {
            self.code_for = Some(expiry_minutes(&lower).unwrap_or(CODE_MINUTES));
            return vec![];
        }
        if let Some(url) = find_url(line) {
            // A link that is not a sign-in host's is dropped, never offered.
            return vetted(url).map(Event::Link).into_iter().collect();
        }
        if chatter(&lower) {
            return vec![];
        }
        vec![Event::Line(line.into())]
    }
}

// ---------------------------------------------------------------- what the sign-in did

/// What a read of the login just signed in means.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Finish {
    /// Signed in to a plan.
    Done,
    /// A new account signed in to a login another row already holds: this email.
    Duplicate(String),
    /// Signed in with no plan to pay with (a Codex API key or a free account): this name.
    NoPlan(String),
    /// Not signed in yet: keep waiting.
    NotYet,
}

/// Two logins of one agent that are the same person's: both signed in, the same email in any case. The same comparison
/// as `duplicates`, which flags a copy that is already on the page.
pub(crate) fn same_login(a: &PlanUsage, b: &PlanUsage) -> bool {
    a.agent == b.agent
        && a.signed_in
        && b.signed_in
        && a.email
            .as_deref()
            .zip(b.email.as_deref())
            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
}

/// What `read` (the signing-in login) means, beside the other logins on the page. `new`: the account was made for
/// this sign-in, so a copy goes. `ended`: the login exited 0, so not signed in now means no plan, not "not yet".
pub(crate) fn finish(read: &PlanUsage, others: &[PlanUsage], new: bool, ended: bool) -> Finish {
    let name = || read.email.clone().unwrap_or_else(|| "This account".into());
    if read.signed_in {
        if new
            && let Some(other) = others
                .iter()
                .find(|other| other.account != read.account && same_login(other, read))
        {
            return Finish::Duplicate(other.email.clone().unwrap_or_else(name));
        }
        // Codex reads a ChatGPT login with no plan tier as signed in; it has nothing to pay with.
        if crate::agents::ui(read.agent).plan_needs_tier() && read.plan.as_deref().is_none_or(str::is_empty) {
            return Finish::NoPlan(name());
        }
        return Finish::Done;
    }
    if ended { Finish::NoPlan(name()) } else { Finish::NotYet }
}

// ---------------------------------------------------------------- running it

/// What the page asks of a running login.
#[derive(PartialEq)]
pub(crate) enum Command {
    /// The code the browser showed. Its `Debug` never shows it, so it never reaches a log.
    Code(String),
    /// Codex: start again with the device code.
    DeviceCode,
    Cancel,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Command::Code(_) => "Code(…)",
            Command::DeviceCode => "DeviceCode",
            Command::Cancel => "Cancel",
        })
    }
}

/// A running sign-in. Dropping it ends the login, as [`Runner::cancel`] does.
pub(crate) struct Runner {
    commands: UnboundedSender<Command>,
}

impl Runner {
    /// Starts the login on its own thread. Its events end with one [`Event::Exited`].
    #[cfg(test)]
    pub(crate) fn start(options: Options) -> (Runner, UnboundedReceiver<Event>) {
        Runner::spawn(move || Ok(options))
    }

    /// Starts the login on its own thread, with the options `prepare` finds there (where the agent is, its folder).
    /// A `prepare` that fails ends it as [`Exit::NotStarted`].
    pub(crate) fn spawn(
        prepare: impl FnOnce() -> Result<Options, String> + Send + 'static,
    ) -> (Runner, UnboundedReceiver<Event>) {
        let (commands, receive) = unbounded();
        let (send, events) = unbounded();
        std::thread::spawn(move || {
            let options = match prepare() {
                Ok(options) => options,
                Err(error) => {
                    let _ = send.unbounded_send(Event::Exited(Exit::NotStarted(error)));
                    return;
                }
            };
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
            match runtime {
                Ok(runtime) => runtime.block_on(run(options, receive, send)),
                Err(error) => {
                    let _ = send.unbounded_send(Event::Exited(Exit::NotStarted(error.to_string())));
                }
            }
        });
        (Runner { commands }, events)
    }

    /// A runner with no login behind it: the test reads what the page asks of it.
    #[cfg(test)]
    pub(crate) fn fake() -> (Runner, UnboundedReceiver<Command>) {
        let (commands, receive) = unbounded();
        (Runner { commands }, receive)
    }

    /// Writes the code to the login's stdin, once. Nexus keeps no copy.
    pub(crate) fn submit_code(&self, code: String) {
        let _ = self.commands.unbounded_send(Command::Code(code));
    }

    /// Codex: "Use a code instead".
    pub(crate) fn device_code(&self) {
        let _ = self.commands.unbounded_send(Command::DeviceCode);
    }

    pub(crate) fn cancel(&self) {
        let _ = self.commands.unbounded_send(Command::Cancel);
    }
}

enum Attempt {
    Restart,
    Done(Exit),
}

async fn run(options: Options, mut commands: UnboundedReceiver<Command>, events: UnboundedSender<Event>) {
    let deadline = tokio::time::Instant::now() + options.deadline;
    let mut mode = if crate::agents::ui(options.agent).device_code() {
        options.mode
    } else {
        Mode::Browser
    };
    loop {
        match attempt(&options, mode, deadline, &mut commands, &events).await {
            Attempt::Restart => mode = Mode::DeviceCode,
            Attempt::Done(exit) => {
                let _ = events.unbounded_send(Event::Exited(exit));
                return;
            }
        }
    }
}

/// Kills the login's process group, then reaps it.
async fn stop(child: &mut tokio::process::Child) {
    process::kill(child.id());
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
}

/// Sends what `line` says. True when Codex's browser login cannot serve its callback.
fn read_line(parser: &mut Parser, line: &str, fallback: bool, events: &UnboundedSender<Event>) -> bool {
    let said = parser.feed(line, Instant::now());
    let port = fallback
        && said
            .iter()
            .any(|event| matches!(event, Event::Line(line) if port_error(line)));
    for event in said {
        let _ = events.unbounded_send(event);
    }
    port
}

/// Reads what is left in one of the login's pipes after it ended.
async fn drain<R: tokio::io::AsyncBufRead + Unpin>(
    lines: Option<&mut tokio::io::Lines<R>>,
    parser: &mut Parser,
    events: &UnboundedSender<Event>,
) {
    let Some(lines) = lines else { return };
    while let Ok(Ok(Some(line))) = tokio::time::timeout(DRAIN, lines.next_line()).await {
        read_line(parser, &line, false, events);
    }
}

async fn attempt(
    options: &Options,
    mode: Mode,
    deadline: tokio::time::Instant,
    commands: &mut UnboundedReceiver<Command>,
    events: &UnboundedSender<Event>,
) -> Attempt {
    let spawn = match command(options, mode) {
        Ok(spawn) => spawn,
        Err(error) => return Attempt::Done(Exit::NotStarted(error)),
    };
    let mut child = match process::spawn(&spawn) {
        Ok(child) => child,
        Err(error) => return Attempt::Done(Exit::NotStarted(error.to_string())),
    };
    let _ = events.unbounded_send(Event::Started(mode));
    let started = Instant::now();
    let fallback = crate::agents::ui(options.agent).device_code() && mode == Mode::Browser;
    let mut stdin = child.stdin.take();
    let mut out = child.stdout.take().map(|out| BufReader::new(out).lines());
    let mut err = child.stderr.take().map(|err| BufReader::new(err).lines());
    let mut parser = Parser::default();
    loop {
        tokio::select! {
            line = async { out.as_mut().expect("guarded").next_line().await }, if out.is_some() => match line {
                Ok(Some(line)) => if read_line(&mut parser, &line, fallback, events) {
                    stop(&mut child).await;
                    return Attempt::Restart;
                },
                _ => out = None,
            },
            line = async { err.as_mut().expect("guarded").next_line().await }, if err.is_some() => match line {
                Ok(Some(line)) => if read_line(&mut parser, &line, fallback, events) {
                    stop(&mut child).await;
                    return Attempt::Restart;
                },
                _ => err = None,
            },
            command = commands.next() => match command {
                Some(Command::Code(code)) => {
                    let mut bytes = code.into_bytes();
                    bytes.push(b'\n');
                    if let Some(stdin) = stdin.as_mut() {
                        let _ = stdin.write_all(&bytes).await;
                        let _ = stdin.flush().await;
                    }
                    bytes.fill(0);
                }
                Some(Command::DeviceCode) if crate::agents::ui(options.agent).device_code() => {
                    stop(&mut child).await;
                    return Attempt::Restart;
                }
                Some(Command::DeviceCode) => {}
                Some(Command::Cancel) | None => {
                    stop(&mut child).await;
                    return Attempt::Done(Exit::Cancelled);
                }
            },
            status = child.wait() => {
                // What it printed last may still be in the pipes.
                drain(out.as_mut(), &mut parser, events).await;
                drain(err.as_mut(), &mut parser, events).await;
                let code = status.ok().and_then(|status| status.code());
                if fallback && code != Some(0) && started.elapsed() < EARLY {
                    return Attempt::Restart;
                }
                return Attempt::Done(Exit::Status(code));
            },
            _ = tokio::time::sleep_until(deadline) => {
                stop(&mut child).await;
                return Attempt::Done(Exit::TimedOut);
            },
        }
    }
}

#[cfg(test)]
mod tests;
