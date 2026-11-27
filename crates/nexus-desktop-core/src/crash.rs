//! Crash reports, without the toolkit. A panic is written to `<app data>/last-crash.json` before the process
//! ends (release builds abort on a panic, after the hook runs). At the next start the shell takes it and asks the person
//! whether to send it. Only the first panic of a run is kept: it is the cause, and what follows is its fallout. The
//! file holds the panic's message, its place in Nexus's code, the thread's name and the app's version
//! (`crate::app::version`); the UI redacts the message before anything leaves the computer.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const FILE: &str = "last-crash.json";

/// Where the crash file is: set once, at setup.
static DIR: OnceLock<PathBuf> = OnceLock::new();

/// A panic, as the next start reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeenCrash {
    /// `panic`, with the thread when it has a name (`panic in tokio-runtime-worker`).
    pub name: String,
    pub message: String,
    /// `crates/nexus-workbench/src/lib.rs:3546:13`.
    pub location: Option<String>,
    pub occurred_at_ms: u64,
    /// The version that crashed, which an update since may have replaced.
    pub app_version: String,
}

/// Keep the next panic in `dir`, after the hook already set (Rust's own, which prints it) runs.
pub fn install(dir: &Path) {
    if DIR.set(dir.to_path_buf()).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let thread = std::thread::current().name().map(str::to_string);
        let crash = SeenCrash {
            name: thread.map_or_else(|| "panic".into(), |thread| format!("panic in {thread}")),
            message,
            location: info
                .location()
                .map(|place| format!("{}:{}:{}", place.file(), place.line(), place.column())),
            occurred_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_millis() as u64),
            app_version: crate::app::version().to_string(),
        };
        keep(&crash);
        previous(info);
    }));
}

/// Written at once and never over an earlier panic of the run; a failure is ignored, since the process is ending.
fn keep(crash: &SeenCrash) {
    let Some(dir) = DIR.get() else { return };
    let path = dir.join(FILE);
    if path.exists() {
        return;
    }
    if let Ok(json) = serde_json::to_vec(crash) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(path, json);
    }
}

/// The most of a crash file read: a panic's message is short, so a larger file is not one this app wrote.
const MAX_FILE_BYTES: u64 = 64 * 1024;

/// The crash the last run left, removed so it is asked about once. `None` without one, or with one unreadable or
/// larger than [`MAX_FILE_BYTES`].
pub fn take(dir: &Path) -> Option<SeenCrash> {
    let path = dir.join(FILE);
    let size = std::fs::metadata(&path).ok()?.len();
    let bytes = (size <= MAX_FILE_BYTES).then(|| std::fs::read(&path).ok()).flatten();
    let _ = std::fs::remove_file(&path);
    serde_json::from_slice(&bytes?).ok()
}

/// The crash the last run left, read without taking it: the client errors queue it once (`signals`), and the prompt
/// still asks about it.
pub fn peek(dir: &Path) -> Option<SeenCrash> {
    let path = dir.join(FILE);
    let size = std::fs::metadata(&path).ok()?.len();
    if size > MAX_FILE_BYTES {
        return None;
    }
    serde_json::from_slice(&std::fs::read(&path).ok()?).ok()
}

/// The crash the last run left in the folder [`install`] was given; see [`take`].
pub fn take_installed() -> Option<SeenCrash> {
    take(DIR.get()?)
}

/// The operating system's version (`15.1`, `10.0.22631`, `Ubuntu 24.04`), for reports and feedback. Blocking: it may
/// run a system command.
#[cfg(target_os = "macos")]
pub fn read_os_version() -> Option<String> {
    let out = nexus_workbench::process::command("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    let version = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!version.is_empty()).then_some(version)
}

#[cfg(target_os = "windows")]
pub fn read_os_version() -> Option<String> {
    // "Microsoft Windows [Version 10.0.22631.4460]"
    // System32's, by its full path: a `cmd.exe` found first on the search path is not run.
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let out = nexus_workbench::process::command(std::path::Path::new(&root).join("System32").join("cmd.exe"))
        .args(["/C", "ver"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let start = text.find("Version ")? + "Version ".len();
    let version: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!version.is_empty()).then_some(version)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn read_os_version() -> Option<String> {
    let release = std::fs::read_to_string("/etc/os-release").ok()?;
    let field = |key: &str| {
        release
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .map(|value| value.trim_matches('"').to_string())
    };
    match (field("NAME="), field("VERSION_ID=")) {
        (Some(name), Some(version)) => Some(format!("{name} {version}")),
        (name, version) => name.or(version),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crash(message: &str) -> SeenCrash {
        SeenCrash {
            name: "panic in main".into(),
            message: message.into(),
            location: Some("src/lib.rs:1:2".into()),
            occurred_at_ms: 1,
            app_version: "0.21.0".into(),
        }
    }

    #[test]
    fn a_crash_is_taken_once() {
        let dir = std::env::temp_dir().join(format!("nexus-crash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(FILE), serde_json::to_vec(&crash("boom")).expect("json")).expect("write");
        assert_eq!(take(&dir), Some(crash("boom")));
        assert_eq!(take(&dir), None);
        std::fs::write(dir.join(FILE), b"not json").expect("write");
        assert_eq!(take(&dir), None, "an unreadable file is dropped, not asked about");
        assert!(!dir.join(FILE).exists());
        let mut large = crash("boom");
        large.message = "x".repeat(MAX_FILE_BYTES as usize);
        std::fs::write(dir.join(FILE), serde_json::to_vec(&large).expect("json")).expect("write");
        assert_eq!(take(&dir), None, "a file this app did not write is dropped unread");
        assert!(!dir.join(FILE).exists());
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
