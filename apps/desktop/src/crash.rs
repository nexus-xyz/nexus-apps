//! Crash reports. The capture and
//! the file are `nexus_desktop_core::crash`'s: a panic is written to `last-crash.json` in this build's own data folder
//! ([`crate::paths::own_data_dir`]) before the process ends, and the next start takes it once. Only the first panic of a
//! run is kept. This build unwinds on a panic (`Cargo.toml`'s `dist` profile), so a panic in a background thread that
//! the app outlives is kept too.
//!
//! Once someone is signed in, the workspace asks whether to send it (`workspace/crash_prompt.rs`). With "Send crash and
//! error reports" on (Settings, Appearance), it goes without asking. It goes to `POST /v1/diagnostics/feedback` as the
//! category `crash`, built here as `@nexus/kit`'s `diagnostics.fromCrash` and Nexus UI's `crashReport` build it: the
//! message redacted ([`crate::redact`]), the panic's place in the code as its one frame, and no content.

use std::sync::OnceLock;

pub use nexus_desktop_core::crash::SeenCrash;
use serde_json::{Value, json};

/// The most characters a note may have (`MAX_FEEDBACK_CHARS`), as feedback's.
pub const MAX_NOTE_CHARS: usize = crate::feedback::MAX_CHARS;
const MAX_NAME_CHARS: usize = 120;
const MAX_FRAME_CHARS: usize = 300;
const MAX_VERSION_CHARS: usize = 40;

/// First thing at launch: the app's version for the core, and the panic hook. A folder that cannot be found keeps no
/// crash, and the app runs on.
pub fn install() {
    nexus_desktop_core::app::set_version(crate::channel::VERSION);
    // Before the signals: they queue nothing while usage data is off.
    crate::paths::apply_usage_data();
    if let Some(dir) = crate::paths::own_data_dir() {
        nexus_desktop_core::crash::install(&dir);
        // The usage signals' error queue: the last run's panic is queued for the Gateway.
        nexus_desktop_core::signals::install(&dir, nexus_desktop_core::signals::App::DesktopGpui);
        nexus_desktop_updates::report_failures_to(nexus_desktop_core::signals::update_failed);
    }
}

/// The crash the last run left, once.
pub fn take() -> Option<SeenCrash> {
    nexus_desktop_core::crash::take_installed()
}

/// The operating system and its version (`Fedora Linux 44`), read once. Blocking: run it off the UI thread.
pub fn os_version() -> String {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            nexus_desktop_core::crash::read_os_version()
                .map(|version| plain(&version, MAX_VERSION_CHARS))
                .filter(|version| !version.is_empty())
                .unwrap_or_else(|| "unknown".into())
        })
        .clone()
}

/// The report's platform, as the API names it.
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// The crash as a diagnostic report (`DiagnosticReport`): kind `crash`, fatal, its name and message redacted, and the
/// panic's place as its one frame.
pub fn report(crash: &SeenCrash, os_version: &str) -> Value {
    let name = plain(
        &crate::redact::redact(&crate::redact::first_line(&crash.name)),
        MAX_NAME_CHARS,
    );
    let occurred_at = chrono::DateTime::from_timestamp_millis(crash.occurred_at_ms as i64)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    json!({
        "kind": "crash",
        "fatal": true,
        "name": if name.is_empty() { "Crash".to_string() } else { name },
        "message": crate::redact::redact(&crate::redact::first_line(&crash.message)),
        "stack": crash.location.as_deref().and_then(frame).into_iter().collect::<Vec<_>>(),
        "occurredAt": occurred_at,
        "appVersion": version(&crash.app_version),
        "appBuild": null,
        "platform": platform(),
        "osVersion": version(os_version),
    })
}

/// What `POST /v1/diagnostics/feedback` takes: the crash, and the person's note (empty for none).
pub fn feedback(crash: &SeenCrash, os_version: &str, note: &str) -> Value {
    let report = report(crash, os_version);
    json!({
        "category": "crash",
        "message": note.trim().chars().take(MAX_NOTE_CHARS).collect::<String>(),
        "screen": null,
        "appVersion": report["appVersion"],
        "appBuild": null,
        "platform": platform(),
        "osVersion": report["osVersion"],
        "crash": report,
    })
}

/// "What the report holds", as the Nexus app's prompt shows it.
pub fn summary(report: &Value) -> String {
    let text = |key: &str| report.get(key).and_then(Value::as_str).unwrap_or_default();
    let message = match text("message") {
        "" => "(no message)",
        message => message,
    };
    let mut lines = vec![format!("{}: {message}", text("name"))];
    lines.extend(
        report
            .get("stack")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(String::from),
    );
    lines.push(String::new());
    lines.push(format!(
        "Nexus {} · {} {}",
        text("appVersion"),
        text("platform"),
        text("osVersion")
    ));
    lines.push(format!("At {}", text("occurredAt")));
    lines.push("With your name, email and organization, and your note if you write one.".into());
    lines.join("\n")
}

/// A panic's place as a frame, `at panic (<file>:<line>:<column>)`. A path outside this crate is absolute and names the
/// person's folders, so only the crate's folder and what follows stay (`nexus-workbench/src/lib.rs:3546:13`).
fn frame(location: &str) -> Option<String> {
    let location = location.replace('\\', "/");
    let relative = if location.starts_with('/') || location.get(1..2) == Some(":") {
        match location.rfind("/src/") {
            Some(at) => {
                let start = location[..at].rfind('/').map_or(0, |slash| slash + 1);
                location[start..].to_string()
            }
            None => location.rsplit('/').next().unwrap_or_default().to_string(),
        }
    } else {
        location
    };
    let frame = plain(
        &crate::redact::redact_shapes(&format!("at panic ({relative})")),
        MAX_FRAME_CHARS,
    );
    // The Gateway's frame shape: no spaces or brackets in the place, and a line and a column.
    let place = frame.strip_prefix("at panic (")?.strip_suffix(')')?;
    let mut parts = place.rsplitn(3, ':');
    let numbers = [parts.next()?, parts.next()?];
    let file = parts.next()?;
    let valid = !file.is_empty()
        && !file.contains(|c: char| c.is_whitespace() || c == '(' || c == ')')
        && numbers
            .iter()
            .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    valid.then_some(frame)
}

fn version(text: &str) -> String {
    let version = plain(text, MAX_VERSION_CHARS);
    if version.is_empty() { "unknown".into() } else { version }
}

fn plain(text: &str, max: usize) -> String {
    crate::redact::plain(text, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crash(location: Option<&str>) -> SeenCrash {
        SeenCrash {
            name: "panic in nexus-code".into(),
            message: "called `Option::unwrap()` on a `None` value in /home/ana/acme/notes.md".into(),
            location: location.map(String::from),
            occurred_at_ms: 1_762_400_000_000,
            app_version: "0.1.2".into(),
        }
    }

    #[test]
    fn the_report_names_the_place_and_never_the_persons_folders() {
        let inside = report(&crash(Some("src/workspace/mod.rs:12:5")), "Fedora Linux 44");
        assert_eq!(inside["kind"], "crash");
        assert_eq!(inside["stack"], json!(["at panic (src/workspace/mod.rs:12:5)"]));
        assert_eq!(inside["occurredAt"], "2025-11-06T03:33:20.000Z");
        let message = inside["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains("/home/ana") && !message.contains("Option::unwrap"),
            "{message}"
        );
        let outside = report(
            &crash(Some("/home/ana/src/app/crates/nexus-workbench/src/lib.rs:3546:13")),
            "x",
        );
        assert_eq!(
            outside["stack"],
            json!(["at panic (nexus-workbench/src/lib.rs:3546:13)"])
        );
        assert_eq!(report(&crash(Some("not a place")), "x")["stack"], json!([]));
    }

    #[test]
    fn feedback_carries_the_crash_and_the_trimmed_note() {
        let sent = feedback(&crash(None), "", "  I had just opened a session  ");
        assert_eq!(sent["category"], "crash");
        assert_eq!(sent["message"], "I had just opened a session");
        assert_eq!(sent["osVersion"], "unknown");
        assert_eq!(sent["crash"]["appVersion"], "0.1.2");
        assert!(summary(&sent["crash"]).starts_with("panic in nexus-code: called [quoted]"));
    }

    /// The real capture: a child copy of this test binary panics with the hook installed (`NEXUS_GPUI_TEST_PANIC`, set
    /// only here), and the next "start" takes the crash once.
    #[test]
    fn a_panic_is_kept_for_the_next_start() {
        const NAME: &str = "crash::tests::a_panic_is_kept_for_the_next_start";
        if let Some(dir) = std::env::var_os("NEXUS_GPUI_TEST_PANIC") {
            nexus_desktop_core::app::set_version("9.9.9");
            nexus_desktop_core::crash::install(std::path::Path::new(&dir));
            panic!("forced for the crash report test");
        }
        let dir = std::env::temp_dir().join(format!("nexus-gpui-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let status = std::process::Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", NAME, "--test-threads=1"])
            .env("NEXUS_GPUI_TEST_PANIC", &dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("the child runs");
        assert!(!status.success(), "the child panicked");
        let seen = nexus_desktop_core::crash::take(&dir).expect("the panic was kept");
        assert_eq!(seen.message, "forced for the crash report test");
        assert_eq!(seen.app_version, "9.9.9");
        // The path is relative to the workspace root: `src/crash.rs` here, `apps/desktop/src/crash.rs` in the
        // open-source workspace.
        assert!(
            seen.location
                .as_deref()
                .is_some_and(|place| place.starts_with("src/crash.rs:") || place.contains("/src/crash.rs:"))
        );
        assert_eq!(nexus_desktop_core::crash::take(&dir), None, "asked about once");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
