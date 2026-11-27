//! Which `nexus` runs Nexus's agent. The workbench starts it as `nexus __agent` ([`crate::nexus`]), which
//! only the native CLI has. The JavaScript CLI (`lib/nexus.mjs`, all that production published before the native CLI)
//! reads `__agent` as a product name and fails. So a `nexus` that is found is not yet ready: it must first answer
//! `nexus __agent --handshake` with the protocol [`PROTOCOL`]. A newer protocol is one this app cannot drive.
//!
//! The candidates, in order: each `nexus` on the agents' `PATH` (`nexus.exe`, `nexus.cmd`, `nexus.bat` on Windows),
//! then the installer's own (`<NEXUS_INSTALL_DIR or ~/.nexus>/bin/nexus`, then its `lib/nexus`), which an app started
//! from the dock may not have on `PATH`. The first that answers runs the agent. One that does not answer is passed
//! over, so another `nexus` earlier on `PATH` (the JavaScript CLI from npm, the Exchange's from cargo) hides no good one.
//!
//! The handshake runs again at each check, with no cache, so a `nexus` replaced at the same path counts at once. It
//! runs with an empty config of its own, no key, and a Gateway URL nothing listens on, so no candidate signs in or
//! sends a request.
//!
//! [`install_from`] installs the native CLI from the signed-in Gateway (`/cli/version?target=…`) where the installer
//! puts it, so `nexus update` keeps it current. The download is checked against its SHA-256 and its handshake before
//! anything is replaced.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::process;

/// The protocol of `nexus __agent` this workbench drives: the lines the CLI documents as
/// protocol 1. It drives no other.
pub const PROTOCOL: u32 = 1;

/// How long a candidate has to answer the handshake, or to print its version.
const ANSWER: Duration = Duration::from_secs(5);

/// How long the handshake has once more when it did not answer within [`ANSWER`]. The first start of a large binary
/// just written can take that long, while macOS checks it.
const SLOW_ANSWER: Duration = Duration::from_secs(20);

/// A Gateway URL nothing listens on (the discard port): a candidate that tries a request fails at once.
const NOWHERE: &str = "http://127.0.0.1:9";

/// How long [`select`] keeps a check's answer for a `nexus` that has not changed. A hung one costs a check's whole
/// wait (25 s), so it is not paid on every page load and session start; a change to the file, or to a file its script
/// names, checks it again at once.
const KEPT: Duration = Duration::from_secs(5 * 60);

/// A file up to this size is hashed whole: a wrapper or a stand-in script. A larger one, a binary, is known by its
/// size, modification time and inode.
const SCRIPT: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runtime {
    /// It answered the handshake: it runs the agent.
    Ready {
        path: PathBuf,
        version: String,
        protocol: u32,
    },
    /// A `nexus` that cannot run the agent.
    Unusable {
        path: PathBuf,
        why: Unusable,
        version: Option<String>,
    },
    /// No `nexus` at all.
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unusable {
    /// The JavaScript CLI, which has no `__agent`.
    JavaScript,
    /// A native CLI from before the handshake, or with a protocol older than [`PROTOCOL`].
    Outdated,
    /// Another program by that name, one that does not start or answer in time, one that answers the handshake and
    /// then fails, or a native CLI with a protocol newer than [`PROTOCOL`].
    Other,
    /// Not checked: the check's private folder could not be made, and a check without it would read the person's own
    /// config, and sign in with it.
    Unchecked,
}

impl Runtime {
    /// The program to start, when it runs the agent.
    pub fn ready(&self) -> Option<&Path> {
        match self {
            Runtime::Ready { path, .. } => Some(path),
            _ => None,
        }
    }
}

/// `NEXUS_INSTALL_DIR`, else `~/.nexus`: where the installer (`/install.sh`) and [`install`] put the CLI.
pub fn install_dir(env: &HashMap<String, String>) -> Option<PathBuf> {
    let set = |name: &str| env.get(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    set("NEXUS_INSTALL_DIR").or_else(|| {
        set("HOME")
            .or_else(|| set("USERPROFILE"))
            .map(|home| home.join(".nexus"))
    })
}

/// The installer's program in `dir`: its wrapper, then the binary the wrapper starts.
fn installed(dir: &Path, windows: bool) -> Vec<PathBuf> {
    if windows {
        vec![dir.join("bin").join("nexus.exe")]
    } else {
        vec![dir.join("bin").join("nexus"), dir.join("lib").join("nexus")]
    }
}

/// Every `nexus` that could run the agent, in the order they are tried, each once.
pub fn candidates(env: &HashMap<String, String>, windows: bool) -> Vec<PathBuf> {
    let (separator, names): (char, &[&str]) = if windows {
        (';', &["nexus.exe", "nexus.cmd", "nexus.bat"])
    } else {
        (':', &["nexus"])
    };
    let on_path = env
        .get("PATH")
        .or_else(|| env.get("Path"))
        .map(String::as_str)
        .unwrap_or_default()
        .split(separator)
        .filter(|dir| !dir.is_empty())
        .flat_map(|dir| names.iter().map(move |name| Path::new(dir).join(name)));
    let theirs = install_dir(env).map(|dir| installed(&dir, windows)).unwrap_or_default();
    let mut seen = Vec::new();
    let mut found = Vec::new();
    for candidate in on_path.chain(theirs) {
        if !runnable(&candidate) {
            continue;
        }
        let real = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if !seen.contains(&real) {
            seen.push(real);
            found.push(candidate);
        }
    }
    found
}

/// A file this user may run (the executable bit, outside Windows).
fn runnable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

/// The first candidate that runs the agent; else the first one found, and why it cannot; else [`Runtime::Missing`].
pub fn select(env: &HashMap<String, String>) -> Runtime {
    let mut first = Runtime::Missing;
    for candidate in candidates(env, cfg!(windows)) {
        let checked = checked(&candidate, env);
        if checked.ready().is_some() {
            return checked;
        }
        if first == Runtime::Missing {
            first = checked;
        }
    }
    first
}

/// [`check`], or its answer from the last [`KEPT`] when neither `program`, the files its script names, nor `env` changed.
fn checked(program: &Path, env: &HashMap<String, String>) -> Runtime {
    type Seen = HashMap<PathBuf, (String, Instant, Runtime)>;
    static SEEN: LazyLock<Mutex<Seen>> = LazyLock::new(Mutex::default);
    let Some(identity) = identity(program, env) else {
        return check(program, env);
    };
    if let Some((seen, at, runtime)) = SEEN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(program)
        && *seen == identity
        && at.elapsed() < KEPT
    {
        return runtime.clone();
    }
    let runtime = check(program, env);
    // A check that could not run is not kept: the next one may.
    if !matches!(
        runtime,
        Runtime::Unusable {
            why: Unusable::Unchecked,
            ..
        }
    ) {
        SEEN.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(program.to_path_buf(), (identity, Instant::now(), runtime.clone()));
    }
    runtime
}

/// What a check of `program` depends on, hashed: `env`, `program`, and each absolute path its script quotes (the
/// installer's wrapper names `lib/nexus` or `lib/nexus.mjs`). None when `program` cannot be read.
fn identity(program: &Path, env: &HashMap<String, String>) -> Option<String> {
    let mut hash = Sha256::new();
    let mut names: Vec<_> = env.iter().collect();
    names.sort();
    for (name, value) in names {
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(value.as_bytes());
        hash.update([0]);
    }
    let script = stamp(program, &mut hash)?;
    for quoted in script.as_deref().map(quoted_paths).unwrap_or_default() {
        let _ = stamp(Path::new(quoted), &mut hash);
    }
    Some(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Adds `path`, its size, modification time and inode to `hash`, and its contents when it is a script's size; returns
/// those contents. None when `path` cannot be read; the hash then says so.
fn stamp(path: &Path, hash: &mut Sha256) -> Option<Option<String>> {
    hash.update(path.as_os_str().as_encoded_bytes());
    let Ok(metadata) = std::fs::metadata(path) else {
        hash.update(b"\0missing");
        return None;
    };
    hash.update(metadata.len().to_le_bytes());
    if let Ok(modified) = metadata.modified() {
        hash.update(format!("{modified:?}").as_bytes());
    }
    #[cfg(unix)]
    hash.update(std::os::unix::fs::MetadataExt::ino(&metadata).to_le_bytes());
    if metadata.len() > SCRIPT {
        return Some(None);
    }
    let Ok(bytes) = std::fs::read(path) else {
        hash.update(b"\0unreadable");
        return None;
    };
    hash.update(&bytes);
    Some(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// The double-quoted absolute paths in a script, at most 8.
fn quoted_paths(text: &str) -> Vec<&str> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .filter(|quoted| {
            let bytes = quoted.as_bytes();
            quoted.starts_with('/')
                || (bytes.len() > 2
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && matches!(bytes[2], b'\\' | b'/'))
        })
        .take(8)
        .collect()
}

/// A folder only this user can open, for one check's empty config, removed when the check ends. A fixed name in the
/// temp folder could already be another user's folder or link: on Linux, servers included, that is the shared `/tmp`.
struct Scratch(PathBuf);

impl Scratch {
    /// A new folder under a random name, mode 700. Creating it fails if the name exists, so it is never one that
    /// someone else made.
    fn new() -> std::io::Result<Scratch> {
        let dir = std::env::temp_dir().join(format!("nexus-agent-handshake-{}", uuid::Uuid::new_v4().simple()));
        let mut folder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut folder, 0o700);
        folder.create(&dir)?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The environment a check runs in: the agents' own, without a key or a Gateway to sign in to, with no update check
/// and no color, and with `config` (in a [`Scratch`] folder) as its empty config.
fn check_env(env: &HashMap<String, String>, config: &Path) -> HashMap<String, String> {
    let mut env = env.clone();
    env.retain(|name, _| !name.starts_with("NEXUS_"));
    env.insert("NEXUS_CONFIG".into(), config.to_string_lossy().into_owned());
    env.insert("NEXUS_URL".into(), NOWHERE.into());
    env.insert("NEXUS_NO_UPDATE_CHECK".into(), "1".into());
    env.insert("NO_COLOR".into(), "1".into());
    env
}

/// What `program` prints for `args` within `limit`: its exit and its output, or None when it did not start or ran
/// over.
fn run(program: &Path, args: &[&str], env: &HashMap<String, String>, limit: Duration) -> Option<(bool, String)> {
    let mut command = process::command(program);
    command
        .args(args)
        .env_clear()
        .envs(env)
        .current_dir(std::env::temp_dir());
    process::stdout_within(command, limit).map(|(status, stdout)| (status.success(), stdout))
}

/// Whether `program` runs the agent: its answer to `__agent --handshake`, and when it has none, what it is.
pub fn check(program: &Path, env: &HashMap<String, String>) -> Runtime {
    check_in(program, env, Scratch::new())
}

/// [`check`] with its private folder. Without one it does not run `program`: the CLI would read the person's own
/// config, whose saved Gateway and key win over `NEXUS_URL`, and send that key to it.
fn check_in(program: &Path, env: &HashMap<String, String>, scratch: std::io::Result<Scratch>) -> Runtime {
    let Ok(scratch) = scratch else {
        return Runtime::Unusable {
            path: program.to_path_buf(),
            why: Unusable::Unchecked,
            version: None,
        };
    };
    let env = check_env(env, &scratch.0.join("config.json"));
    let handshake = |limit| run(program, &["__agent", "--handshake"], &env, limit);
    let answer = handshake(ANSWER).or_else(|| handshake(SLOW_ANSWER));
    let lines: Vec<Value> = answer
        .as_ref()
        .map(|(_, stdout)| {
            stdout
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
                .collect()
        })
        .unwrap_or_default();
    let unusable = |why: Unusable, version: Option<String>| Runtime::Unusable {
        path: program.to_path_buf(),
        why,
        version,
    };
    if let Some(handshake) = lines.iter().find(|line| line["type"] == "handshake") {
        let version = handshake["version"].as_str().map(String::from);
        let protocol = handshake["protocol"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        let ok = answer.as_ref().is_some_and(|(ok, _)| *ok);
        return match protocol {
            PROTOCOL if ok => Runtime::Ready {
                path: program.to_path_buf(),
                version: version.unwrap_or_else(|| "unknown".into()),
                protocol,
            },
            // It speaks the protocol and then fails: not too old, but not one that runs.
            PROTOCOL => unusable(Unusable::Other, version),
            newer if newer > PROTOCOL => unusable(Unusable::Other, version),
            _ => unusable(Unusable::Outdated, version),
        };
    }
    let printed = version_of(program, &env);
    let version = printed
        .as_deref()
        .and_then(|line| line.strip_prefix("nexus "))
        .map(String::from);
    if javascript(program) {
        return unusable(Unusable::JavaScript, version);
    }
    // A native `__agent` from before the handshake answers in its lines (it is not signed in); an older binary still
    // says it is nexus. One whose handshake never answered is not called old: it may be slow, or hung.
    let native = answer.is_some() && (version.is_some() || lines.iter().any(|line| line["type"] == "error"));
    unusable(if native { Unusable::Outdated } else { Unusable::Other }, version)
}

/// `program --version`'s first line.
fn version_of(program: &Path, env: &HashMap<String, String>) -> Option<String> {
    let (ok, stdout) = run(program, &["--version"], env, ANSWER)?;
    let line = stdout.lines().map(str::trim).find(|line| !line.is_empty())?;
    ok.then(|| line.to_string())
}

/// Whether `program` is the JavaScript CLI: the installer's wrapper (`exec node "…/lib/nexus.mjs"`), npm's script or
/// shim, or the bundle itself (`#!/usr/bin/env node`).
fn javascript(program: &Path) -> bool {
    use std::io::Read as _;
    let mut head = Vec::new();
    let read = std::fs::File::open(program).and_then(|file| file.take(64 * 1024).read_to_end(&mut head));
    if read.is_err() {
        return false;
    }
    let text = String::from_utf8_lossy(&head);
    let script = text.starts_with("#!")
        || program
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat"));
    script
        && [".mjs", "node_modules", "env node", "exec node ", "node.exe"]
            .iter()
            .any(|sign| text.contains(sign))
}

/// What is wrong, said for a person: None when it runs, or when there is no `nexus` at all.
pub fn problem(runtime: &Runtime) -> Option<String> {
    let Runtime::Unusable { path, why, version } = runtime else {
        return None;
    };
    let at = path.display();
    let version = version.as_deref().map(|v| format!(" ({v})")).unwrap_or_default();
    Some(match why {
        Unusable::JavaScript => {
            format!("The nexus at {at} is the JavaScript CLI{version}. Nexus's agent needs the native CLI.")
        }
        Unusable::Outdated => {
            format!("The nexus at {at}{version} is too old for Nexus's agent, which needs agent protocol {PROTOCOL}.")
        }
        Unusable::Other => format!("{at} is not a Nexus CLI this app can run Nexus's agent on."),
        Unusable::Unchecked => format!(
            "The nexus at {at} could not be checked: this app could not make a private folder in the temp folder."
        ),
    })
}

/// The agent list's entry for Nexus's agent: the `nexus` that runs it, or why the one found cannot.
pub fn info(runtime: &Runtime) -> crate::api::AgentInfo {
    let (path, version) = match runtime {
        Runtime::Ready { path, version, .. } => (Some(path.display().to_string()), Some(version.clone())),
        _ => (None, None),
    };
    crate::api::AgentInfo {
        agent: crate::event::SessionAgent::Nexus,
        path,
        version,
        models: None,
        problem: problem(runtime),
    }
}

/// Why a session of Nexus's agent cannot start, and `fix`: what installs a native CLI where this runs.
pub fn refusal(runtime: &Runtime, fix: &str) -> String {
    let what =
        problem(runtime).unwrap_or_else(|| "Nexus's agent needs the native nexus CLI. None is installed.".into());
    format!("{what} {fix}")
}

/// What the desktop says to do: the Install action on the harness menu's Nexus card.
pub const INSTALL_HERE: &str =
    "Open the harness menu and choose Install on the Nexus card. It installs the native CLI from your Gateway.";

/// What `nexus-host` says to do. It runs on a server, or as this computer's background service ("Keep agents running
/// when Nexus quits"), where the desktop's Install action works too.
pub fn install_there(gateway: &str) -> String {
    format!(
        "On this computer, open the harness menu and choose Install on the Nexus card. On a server, run `{}` there.",
        crate::nexus::install_hint(gateway)
    )
}

/// This platform as the Gateway names its builds: `darwin-arm64`, `linux-x64`, `win32-x64` (as the CLI does).
pub fn target() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Gets a URL: its status and its body. The app's is an HTTP client; a test's serves what it wants.
pub type Fetch = Arc<dyn Fn(String) -> BoxFuture<'static, Result<(u16, Vec<u8>), String>> + Send + Sync>;

/// Installs the native CLI the Gateway at `gateway` publishes for this platform ([`install`]), and returns it ready.
/// One install at a time: two would swap the same files, and one would fail on the other's move.
pub async fn install_from(gateway: &str, env: HashMap<String, String>, fetch: Fetch) -> Result<Runtime, String> {
    static INSTALLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = INSTALLING.lock().await;
    let base = gateway.trim_end_matches('/').to_string();
    let target = target();
    let (status, body) = fetch(format!("{base}/cli/version?target={target}"))
        .await
        .map_err(|error| format!("Could not reach Nexus Gateway at {base}: {error}"))?;
    let published: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if status == 404 && published.pointer("/error/code").and_then(Value::as_str) == Some("cli_target_unavailable") {
        return Err(format!(
            "Nexus Gateway at {base} does not publish the native nexus CLI for {target} yet, so Nexus's agent cannot \
             be installed. Use Claude Code or Codex until it does."
        ));
    }
    if status != 200 {
        return Err(format!(
            "Nexus Gateway at {base} answered {status} when asked for the native nexus CLI. Try again later."
        ));
    }
    let text = |key: &str| published.get(key).and_then(Value::as_str).unwrap_or("");
    let (sha256, url) = (text("sha256").to_string(), text("url").to_string());
    let well_formed = text("target") == target
        && sha256.len() == 64
        && sha256.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        && (url.starts_with("https://") || url.starts_with(&format!("{base}/")));
    if !well_formed {
        return Err(format!(
            "Nexus Gateway at {base} described the native nexus CLI in a form this app does not read. Nothing was \
             changed."
        ));
    }
    let (status, binary) = fetch(url.clone())
        .await
        .map_err(|error| format!("Could not download {url}: {error}. Nothing was changed."))?;
    if status != 200 {
        return Err(format!(
            "Could not download {url} (HTTP {status}). Nothing was changed."
        ));
    }
    if sha256_hex(&binary) != sha256 {
        return Err(format!(
            "The download from {url} does not match its checksum. Nothing was changed."
        ));
    }
    let dir = install_dir(&env).ok_or("There is no home folder to install the nexus CLI in.")?;
    tokio::task::spawn_blocking(move || install(&dir, &binary, &base, &env, cfg!(windows)))
        .await
        .map_err(|error| error.to_string())?
}

/// Puts `binary` where the installer does in `dir`, once it answers the handshake: `lib/nexus`, started by the
/// wrapper `bin/nexus` (`bin/nexus.exe` alone on Windows). The binary it replaces stays as `nexus.previous`
/// (`nexus update --rollback` reads it). The installer's JavaScript wrapper is turned to the native binary, keeping its
/// URLs; a `bin/nexus` that is not the installer's is left as it is, and `lib/nexus` is the program.
pub fn install(
    dir: &Path,
    binary: &[u8],
    gateway: &str,
    env: &HashMap<String, String>,
    windows: bool,
) -> Result<Runtime, String> {
    let failed = |error: std::io::Error| format!("Could not install the nexus CLI in {}: {error}.", dir.display());
    let bin = dir.join("bin");
    let lib = if windows { bin.clone() } else { dir.join("lib") };
    std::fs::create_dir_all(&bin).map_err(failed)?;
    std::fs::create_dir_all(&lib).map_err(failed)?;
    let (name, previous) = if windows {
        ("nexus.exe", "nexus.previous.exe")
    } else {
        ("nexus", "nexus.previous")
    };
    let staged = lib.join(format!(
        "nexus.{}.tmp{}",
        uuid::Uuid::new_v4().simple(),
        if windows { ".exe" } else { "" }
    ));
    write_executable(&staged, binary).map_err(failed)?;
    let checked = check_written(&staged, env);
    if checked.ready().is_none() {
        let _ = std::fs::remove_file(&staged);
        let why = problem(&checked).unwrap_or_default();
        return Err(format!(
            "The nexus CLI the Gateway publishes cannot run Nexus's agent yet. {why} Nothing was changed."
        ));
    }
    let program = lib.join(name);
    let previous = lib.join(previous);
    let kept = program.exists();
    if let Err(error) = swap(&staged, &program, &previous, kept, windows) {
        let _ = std::fs::remove_file(&staged);
        return Err(failed(error));
    }
    let start = if windows {
        program
    } else {
        wrap(dir, &program, gateway).map_err(failed)?
    };
    // `wrap` may have just written `bin/nexus`, so it can be busy as the staged binary was.
    match check_written(&start, env) {
        ready @ Runtime::Ready { .. } => Ok(ready),
        other => Err(format!(
            "The native nexus CLI is installed at {}, but it does not start. {}",
            start.display(),
            problem(&other).unwrap_or_default()
        )),
    }
}

/// Puts `staged` at `program`, keeping the binary it replaces as `previous`. On Unix the old one is copied to `previous`
/// first, and the new one then takes `program`'s name in one rename, so `program` is never missing, even if the app
/// stops in between. Windows cannot replace a running `.exe`, only rename it, so there the old one moves to `previous`
/// first and comes back when the new one cannot take its place.
fn swap(staged: &Path, program: &Path, previous: &Path, kept: bool, windows: bool) -> std::io::Result<()> {
    if !windows {
        if kept {
            // Copied under a name of its own, then renamed: `previous` may be running, and cannot be written over.
            let copy = previous.with_file_name(format!("nexus.previous.{}.tmp", uuid::Uuid::new_v4().simple()));
            if let Err(error) = std::fs::copy(program, &copy).and_then(|_| std::fs::rename(&copy, previous)) {
                let _ = std::fs::remove_file(&copy);
                return Err(error);
            }
        }
        return std::fs::rename(staged, program);
    }
    if kept {
        let _ = std::fs::remove_file(previous);
        std::fs::rename(program, previous)?;
    }
    std::fs::rename(staged, program).inspect_err(|_| {
        if kept {
            let _ = std::fs::rename(previous, program);
        }
    })
}

/// [`check`] of a file `install` just wrote, again after a moment when the first try fails: the file can be busy
/// (ETXTBSY) for an instant, because a process another thread of this app forks holds its write handle until it execs.
fn check_written(written: &Path, env: &HashMap<String, String>) -> Runtime {
    let first = check(written, env);
    if first.ready().is_some() {
        return first;
    }
    std::thread::sleep(Duration::from_millis(200));
    check(written, env)
}

/// Writes an executable file (mode 755) and closes it.
fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// The wrapper `bin/nexus` that starts `program`, as the installer writes it, and the program to start for the agent.
/// A file there that is not the installer's is left alone, and `program` is started directly.
fn wrap(dir: &Path, program: &Path, gateway: &str) -> std::io::Result<PathBuf> {
    let wrapper = dir.join("bin").join("nexus");
    let lib = program.display().to_string();
    let real = |path: &Path| std::fs::canonicalize(path).ok();
    let text = match std::fs::read_to_string(&wrapper) {
        Ok(text)
            if quoted_after(&text, "exec \"").is_some_and(|started| real(Path::new(&started)) == real(program)) =>
        {
            return Ok(wrapper);
        }
        // The installer's JavaScript wrapper, beside this binary: the same lines, starting the native binary instead.
        Ok(text) => match quoted_after(&text, "exec node \"") {
            Some(bundle) if Path::new(&bundle).parent().and_then(real) == program.parent().and_then(real) => {
                text.replace(&format!("exec node \"{bundle}\""), &format!("exec \"{lib}\""))
            }
            _ => return Ok(program.to_path_buf()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The URL as the default, as the installer bakes it in; one a shell would read differently is left out.
            let url = if gateway.contains(['"', '$', '`', '\\', '\n']) {
                String::new()
            } else {
                format!("NEXUS_URL=\"${{NEXUS_URL:-{gateway}}}\" ")
            };
            format!("#!/bin/sh\n{url}exec \"{lib}\" \"$@\"\n")
        }
        // Not text, so not the installer's wrapper (a binary there, say).
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => return Ok(program.to_path_buf()),
        Err(error) => return Err(error),
    };
    let staged = wrapper.with_file_name(format!("nexus.{}.tmp", uuid::Uuid::new_v4().simple()));
    write_executable(&staged, text.as_bytes())?;
    std::fs::rename(&staged, &wrapper)?;
    Ok(wrapper)
}

/// The double-quoted text right after `prefix`.
fn quoted_after(text: &str, prefix: &str) -> Option<String> {
    let at = text.find(prefix)? + prefix.len();
    let end = text[at..].find('"')? + at;
    Some(text[at..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn the_installer_folder_follows_nexus_install_dir_then_home() {
        assert_eq!(
            install_dir(&env(&[("HOME", "/home/a"), ("NEXUS_INSTALL_DIR", "/opt/nexus")])),
            Some(PathBuf::from("/opt/nexus"))
        );
        assert_eq!(
            install_dir(&env(&[("HOME", "/home/a")])),
            Some(PathBuf::from("/home/a/.nexus"))
        );
        assert_eq!(
            install_dir(&env(&[("USERPROFILE", "C:\\Users\\a")])),
            Some(PathBuf::from("C:\\Users\\a").join(".nexus"))
        );
        assert_eq!(install_dir(&env(&[])), None);
    }

    #[test]
    fn a_check_has_no_key_no_gateway_and_an_empty_config() {
        let agents = env(&[
            ("PATH", "/bin"),
            ("NEXUS_API_KEY", "secret"),
            ("NEXUS_GATEWAY", "prod"),
            ("NEXUS_URL", "https://gw.example"),
            ("NEXUS_CONFIG", "/home/a/.nexus/factory/config.json"),
        ]);
        let checked = check_env(&agents, Path::new("/private/check/config.json"));
        assert_eq!(checked.get("PATH").map(String::as_str), Some("/bin"));
        assert!(!checked.contains_key("NEXUS_API_KEY") && !checked.contains_key("NEXUS_GATEWAY"));
        assert_eq!(checked.get("NEXUS_URL").map(String::as_str), Some(NOWHERE));
        assert_eq!(
            checked.get("NEXUS_CONFIG").map(String::as_str),
            Some("/private/check/config.json")
        );
    }

    /// Without its private folder a check runs nothing: the CLI would read the person's own config and sign in.
    #[cfg(unix)]
    #[test]
    fn a_check_without_its_folder_runs_nothing() {
        let dir = std::env::temp_dir().join(format!("nexus-runtime-unchecked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        let program = dir.join("nexus");
        let ran = dir.join("ran");
        crate::process::write_fake_script(&program, &format!("#!/bin/sh\ntouch '{}'\n", ran.display()));
        let checked = check_in(
            &program,
            &env(&[("PATH", "/bin")]),
            Err(std::io::Error::other("no temp folder")),
        );
        assert!(
            matches!(
                checked,
                Runtime::Unusable {
                    why: Unusable::Unchecked,
                    ..
                }
            ),
            "{checked:?}"
        );
        assert!(!ran.exists(), "the program ran");
        assert!(problem(&checked).is_some_and(|text| text.contains("could not be checked")));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A check is known by the program, the files its script quotes, and the environment: a change to any of them
    /// checks it again.
    #[cfg(unix)]
    #[test]
    fn a_check_is_kept_until_what_it_read_changes() {
        let dir = std::env::temp_dir().join(format!("nexus-runtime-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lib")).unwrap_or_else(|e| panic!("{e}"));
        let lib = dir.join("lib/nexus");
        std::fs::write(&lib, "one").unwrap_or_else(|e| panic!("{e}"));
        let wrapper = dir.join("nexus");
        crate::process::write_fake_script(&wrapper, &format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", lib.display()));
        let agents = env(&[("PATH", "/bin")]);
        let first = identity(&wrapper, &agents);
        assert!(first.is_some());
        assert_eq!(identity(&wrapper, &agents), first, "nothing changed");
        assert_ne!(
            identity(&wrapper, &env(&[("PATH", "/usr/bin")])),
            first,
            "the environment changed"
        );
        // `nexus update` and Install replace lib/nexus by a rename, behind an unchanged wrapper.
        let replacement = dir.join("lib/nexus.new");
        std::fs::write(&replacement, "two").unwrap_or_else(|e| panic!("{e}"));
        std::fs::rename(&replacement, &lib).unwrap_or_else(|e| panic!("{e}"));
        assert_ne!(
            identity(&wrapper, &agents),
            first,
            "the file the wrapper starts changed"
        );
        assert_eq!(identity(&dir.join("absent"), &agents), None);
        assert_eq!(
            quoted_paths(r#"NEXUS_URL="${NEXUS_URL:-https://a}" exec node "/h/.nexus/lib/nexus.mjs" "$@" "C:\n.exe""#),
            vec!["/h/.nexus/lib/nexus.mjs", "C:\\n.exe"]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Each check's folder is its own, this user's only, and gone when the check ends.
    #[test]
    fn each_check_gets_a_private_folder_it_removes() {
        let (one, two) = match (Scratch::new(), Scratch::new()) {
            (Ok(one), Ok(two)) => (one, two),
            (one, two) => panic!("{:?} {:?}", one.err(), two.err()),
        };
        assert_ne!(one.0, two.0);
        assert!(one.0.is_dir() && !one.0.ends_with("nexus-agent-handshake"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&one.0)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0);
            assert_eq!(mode, 0o700);
        }
        std::fs::write(one.0.join("config.json"), "{}").unwrap_or_else(|e| panic!("{e}"));
        let gone = one.0.clone();
        drop(one);
        assert!(!gone.exists());
    }

    #[test]
    fn problems_say_what_was_found_and_what_it_needs() {
        let javascript = Runtime::Unusable {
            path: PathBuf::from("/home/a/.nexus/bin/nexus"),
            why: Unusable::JavaScript,
            version: Some("0.21.0+d72f8fc".into()),
        };
        assert_eq!(
            problem(&javascript).as_deref(),
            Some(
                "The nexus at /home/a/.nexus/bin/nexus is the JavaScript CLI (0.21.0+d72f8fc). Nexus's agent needs \
                 the native CLI."
            )
        );
        let old = Runtime::Unusable {
            path: PathBuf::from("/usr/local/bin/nexus"),
            why: Unusable::Outdated,
            version: Some("0.20.0".into()),
        };
        assert_eq!(
            problem(&old).as_deref(),
            Some(
                "The nexus at /usr/local/bin/nexus (0.20.0) is too old for Nexus's agent, which needs agent protocol 1."
            )
        );
        assert_eq!(problem(&Runtime::Missing), None);
        assert!(refusal(&Runtime::Missing, INSTALL_HERE).starts_with("Nexus's agent needs the native nexus CLI."));
        assert!(
            install_there("https://gw.example/")
                .ends_with("run `curl -fsSL https://gw.example/install.sh | bash` there.")
        );
    }

    #[test]
    fn javascript_is_told_by_its_script() {
        let dir = std::env::temp_dir().join(format!("nexus-runtime-js-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        let file = |name: &str, text: &str| {
            let path = dir.join(name);
            std::fs::write(&path, text).unwrap_or_else(|e| panic!("{e}"));
            path
        };
        let wrapper =
            "#!/bin/sh\nNEXUS_URL=\"${NEXUS_URL:-https://gw.example}\" exec node \"/h/.nexus/lib/nexus.mjs\" \"$@\"\n";
        assert!(javascript(&file("installer", wrapper)));
        assert!(javascript(&file(
            "bundle",
            "#!/usr/bin/env node\n/* @nexus/cli 0.21.0 */\n"
        )));
        assert!(javascript(&file(
            "nexus.cmd",
            "@\"%_prog%\" \"%dp0%\\node_modules\\@nexus\\cli\\dist\\nexus.js\" %*\r\n"
        )));
        assert!(!javascript(&file(
            "native",
            "#!/bin/sh\nexec \"/h/.nexus/lib/nexus\" \"$@\"\n"
        )));
        assert!(!javascript(&file("binary", "\u{7f}ELF node_modules")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_platform_is_named_as_the_gateway_names_its_builds() {
        let target = target();
        assert!(
            ["darwin-arm64", "darwin-x64", "linux-x64", "linux-arm64", "win32-x64"].contains(&target.as_str()),
            "{target}"
        );
    }
}
