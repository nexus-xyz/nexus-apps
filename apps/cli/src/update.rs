//! `nexus update` (`update.ts`) for the native binary: install the version the configured Gateway publishes for
//! this platform, the way the installer laid it out.
//!
//!   nexus update [--json]             download the binary, verify its sha256, check it starts, swap it in atomically
//!   nexus update --check [--json]     only say whether a newer version is available (exit 0 either way)
//!   nexus update --rollback [--json]  go back to the version before the last update (again: undo the rollback)
//!
//! The installer puts the binary at `<dir>/lib/nexus` and a wrapper at `<dir>/bin/nexus` that `exec`s it with the
//! Gateway's URL (`<dir>` is `NEXUS_INSTALL_DIR`, default `~/.nexus`). An update writes the new binary to a temporary file
//! in `lib/`, keeps the current one as `lib/nexus.previous`, and renames the new one over `lib/nexus`, so the CLI is
//! never half written. A binary not installed that way (cargo, a copy) is left alone, with how to update it instead.
//! The Gateway is asked with `GET /cli/version?target=<os>-<arch>`; a Gateway that does not serve native builds answers
//! without `target` (it publishes the JavaScript bundle only), and nothing is changed. No request carries a
//! credential.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nexus_client::base::version::Version;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::args::Args;
use crate::catalog::UPDATE_COMMANDS;
use crate::client::{CLI_VERSION_HEADER, http_client};
use crate::config::write_private;
use crate::error::{CliError, Result};
use crate::json::{pretty, s};
use crate::notice::update_cache_path;
use crate::style::{Style, iso, kv, now_ms, pad_end, pairs};
use crate::{Context, VERSION};

/// The marker the gateway and `--rollback` read a binary's version from (the JavaScript bundle's first-line form).
#[used]
pub static VERSION_MARKER: &str = concat!("/* @nexus/cli ", env!("NEXUS_CLI_VERSION"), " */");

/// This build's platform as the installer and Gateway name it: `darwin-arm64`, `linux-x64`, …
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

pub use nexus_client::base::version::binary_version;

#[derive(Debug, Clone, PartialEq)]
pub struct Published {
    pub version: String,
    pub sha256: String,
    pub url: String,
    pub minimum: String,
    /// The platform the binary is for; `None` from a Gateway that publishes only the JavaScript bundle.
    pub target: Option<String>,
}

/// What a Gateway publishes for this platform, checked for shape. Never sends a credential.
pub fn fetch_published(core_url: &str, timeout: Duration) -> Result<Published> {
    let base = core_url.strip_suffix('/').unwrap_or(core_url);
    let unreachable = || {
        CliError::hint(
            format!("Cannot reach Nexus Gateway at {base}."),
            "Check your connection and the Gateway URL (`nexus auth status`).",
        )
    };
    let response = http_client()
        .get(format!("{base}/cli/version?target={}", target()))
        .header(CLI_VERSION_HEADER, VERSION)
        .timeout(timeout)
        .send()
        .map_err(|_| unreachable())?;
    let status = response.status().as_u16();
    let body: Value = response
        .text()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(json!({}));
    if status == 404 {
        if body.pointer("/error/code").and_then(Value::as_str) == Some("cli_target_unavailable") {
            return Err(CliError::hint(
                format!(
                    "Nexus Gateway at {base} does not publish a nexus binary for {}.",
                    target()
                ),
                format!("Reinstall instead: `curl -fsSL {base}/install.sh | bash`."),
            )
            .into());
        }
        return Err(CliError::hint(
            format!("Nexus Gateway at {base} does not publish CLI versions yet."),
            format!("Reinstall instead: `curl -fsSL {base}/install.sh | bash`."),
        )
        .into());
    }
    if !(200..300).contains(&status) {
        return Err(CliError::hint(
            format!("Nexus Gateway at {base} answered {status} for /cli/version."),
            "Try again later.",
        )
        .into());
    }
    let version = s(&body, "version").filter(|v| Version::parse(v).is_some());
    let sha = s(&body, "sha256")
        .filter(|v| v.len() == 64 && v.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    let url = s(&body, "url");
    let minimum = s(&body, "minimum").filter(|v| Version::parse(v).is_some());
    let (Some(version), Some(sha), Some(url), Some(minimum)) = (version, sha, url, minimum) else {
        return Err(CliError::hint(
            format!("Nexus Gateway at {base} sent an unreadable /cli/version."),
            "Try again later; if it persists, tell your Gateway's administrator.",
        )
        .into());
    };
    let absolute = reqwest::Url::parse(&format!("{base}/"))
        .and_then(|root| root.join(url))
        .map(|u| u.to_string())
        .unwrap_or_else(|_| url.to_string());
    Ok(Published {
        version: version.to_string(),
        sha256: sha.to_string(),
        url: absolute,
        minimum: minimum.to_string(),
        target: s(&body, "target").map(String::from),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Layout {
    Installed { lib: PathBuf, previous: PathBuf },
    Elsewhere { how: String, hint: String },
}

fn windows_binary(name: &str) -> bool {
    name.strip_suffix(".exe")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The installed Windows launcher delegates to the selected immutable executable.
#[cfg(windows)]
pub fn launch_windows_install() -> std::io::Result<Option<i32>> {
    let exe = std::env::current_exe()?;
    let Some(bin) = exe.parent() else { return Ok(None) };
    if exe.file_name().and_then(|n| n.to_str()) != Some("nexus.exe")
        || bin.file_name().and_then(|n| n.to_str()) != Some("bin")
    {
        return Ok(None);
    }
    let lib = bin.parent().unwrap().join("lib");
    let state = match std::fs::read_to_string(lib.join("windows-active")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        other => other?,
    };
    let active = state
        .lines()
        .next()
        .filter(|s| windows_binary(s))
        .ok_or_else(|| std::io::Error::other("invalid installed Windows CLI version"))?;
    let mut command = std::process::Command::new(lib.join(active));
    command.args(std::env::args_os().skip(1));
    for (key, file) in [("NEXUS_URL", "windows-gateway"), ("NEXUS_UI_URL", "windows-ui")] {
        if std::env::var_os(key).is_none() {
            let value = std::fs::read_to_string(lib.join(file))?;
            if !value.trim().is_empty() {
                command.env(key, value.trim());
            }
        }
    }
    Ok(Some(command.status()?.code().unwrap_or(1)))
}

fn windows_layout(path: &Path) -> Option<Layout> {
    let folder = path.parent()?;
    let name = path.file_name()?.to_str()?;
    if !windows_binary(name) || folder.file_name()? != "lib" || !folder.parent()?.join("bin/nexus.exe").is_file() {
        return None;
    }
    let state = std::fs::read_to_string(folder.join("windows-active")).ok()?;
    let mut lines = state.lines();
    if lines.next()? != name {
        return None;
    }
    let previous = lines
        .next()
        .filter(|name| windows_binary(name))
        .unwrap_or("no-previous-version");
    Some(Layout::Installed {
        lib: path.to_owned(),
        previous: folder.join(previous),
    })
}

// A running Windows executable stays mapped. Switch both version names with one rename.
fn select_windows_binary(current: &Path, next: &Path) -> std::io::Result<()> {
    let folder = current.parent().expect("installed binary directory");
    let state = folder.join("windows-active");
    let old = current.file_name().unwrap().to_string_lossy();
    let new = next.file_name().unwrap().to_string_lossy();
    let temp = folder.join(format!(".windows-active-{}", uuid::Uuid::new_v4()));
    std::fs::write(&temp, format!("{new}\n{old}\n"))?;
    let result = std::fs::rename(&temp, state);
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

/// Whether the running binary is the installer's (`<dir>/lib/nexus`, started by `<dir>/bin/nexus`); otherwise how it
/// seems to have been installed and how to update it.
pub fn install_layout(exe: Option<&Path>, core_url: &str) -> Layout {
    let base = core_url.strip_suffix('/').unwrap_or(core_url);
    let reinstall = format!("or install it with Nexus's installer: `curl -fsSL {base}/install.sh | bash`");
    let Some(exe) = exe else {
        return Layout::Elsewhere {
            how: "This nexus is not running from a file.".into(),
            hint: format!("Install it with Nexus's installer: `curl -fsSL {base}/install.sh | bash`."),
        };
    };
    let path = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
    if let Some(layout) = windows_layout(&path) {
        return layout;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let parent = path.parent();
    let dir = parent.and_then(Path::parent);
    if name == "nexus"
        && parent.and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("lib")
        && let Some(dir) = dir
    {
        let wrapper = dir.join("bin").join("nexus");
        let starts = std::fs::read_to_string(&wrapper)
            .ok()
            .and_then(|text| wrapper_target(&text))
            .and_then(|target| std::fs::canonicalize(target).ok())
            .is_some_and(|target| target == path);
        if starts {
            return Layout::Installed {
                previous: path.with_file_name("nexus.previous"),
                lib: path,
            };
        }
    }
    let shown = path.display().to_string();
    if shown.contains("/.cargo/bin/") || shown.contains("/target/release/") || shown.contains("/target/debug/") {
        return Layout::Elsewhere {
            how: format!("This nexus is a build from source ({shown})."),
            hint: format!("Pull and rebuild it from source, {reinstall}."),
        };
    }
    let wrapper = dir
        .map(|dir| dir.join("bin").join("nexus"))
        .unwrap_or_else(|| PathBuf::from("bin/nexus"));
    Layout::Elsewhere {
        how: format!(
            "This nexus ({shown}) was not installed by Nexus's installer: there is no {} wrapper starting it.",
            wrapper.display()
        ),
        hint: format!("Update it the way you installed it, {reinstall}."),
    }
}

/// The binary the installer's wrapper starts: `exec "<path>"`.
fn wrapper_target(text: &str) -> Option<String> {
    let at = text.find("exec \"")? + "exec \"".len();
    let end = text[at..].find('"')? + at;
    Some(text[at..end].to_string())
}

pub fn update_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus update [options]", style.bold("Usage")),
        String::new(),
    ];
    lines.extend(
        UPDATE_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 22)))),
    );
    lines.push(String::new());
    lines.push(style.dim(
        "Updates come from the Gateway you are signed in to (GET /cli/version). Without the installer (npm, a source",
    ));
    lines.push(style.dim("build), nothing is changed. A daily notice says when a new version is out; NEXUS_NO_UPDATE_CHECK=1 silences it."));
    lines
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn order(a: &str, b: &str) -> std::cmp::Ordering {
    nexus_client::base::version::compare_versions(a, b).unwrap_or(std::cmp::Ordering::Equal)
}

pub fn update_command(ctx: &Context, core_url: &str, args: &Args, json: bool, style: &Style) -> Result<i32> {
    let out = |line: String| ctx.io.out(&line);
    let print = |value: Value| {
        ctx.io.out(&pretty(&value));
        0
    };
    if args.has("help") {
        if json {
            let commands: Vec<Value> = UPDATE_COMMANDS
                .iter()
                .map(|(usage, description)| json!({ "usage": format!("nexus update {usage}"), "description": description }))
                .collect();
            return Ok(print(
                json!({ "usage": "nexus update [--check] [--rollback] [--json]", "commands": commands }),
            ));
        }
        for line in update_help(style) {
            out(line);
        }
        return Ok(0);
    }
    let layout = install_layout(ctx.exe_path.as_deref(), core_url);
    if args.has("rollback") {
        return rollback(ctx, &layout, json, style);
    }

    let published = fetch_published(core_url, Duration::from_secs(15))?;
    let base = core_url.strip_suffix('/').unwrap_or(core_url);
    if published.target.as_deref() != Some(target().as_str()) {
        return Err(CliError::hint(
            format!(
                "Nexus Gateway at {base} publishes only the JavaScript CLI ({}), not a nexus binary for {}. Nothing was changed.",
                published.version,
                target()
            ),
            "Ask your Gateway's administrator to publish the native CLI (NEXUS_CLI_BINARIES), or keep this version.",
        )
        .into());
    }
    let ordering = order(&published.version, VERSION);
    let supported = order(VERSION, &published.minimum).is_ge();
    let installed = matches!(layout, Layout::Installed { .. });
    // The same release in another build counts as an update only when the bytes differ.
    let same_bytes = match &layout {
        Layout::Installed { lib, .. } => std::fs::read(lib)
            .map(|b| sha256_hex(&b) == published.sha256)
            .unwrap_or(false),
        Layout::Elsewhere { .. } => false,
    };
    let available = ordering.is_gt() || (ordering.is_eq() && published.version != VERSION && !same_bytes);

    if args.has("check") {
        if json {
            return Ok(print(json!({
                "current": VERSION, "latest": published.version, "minimum": published.minimum,
                "updateAvailable": available, "supported": supported, "core": core_url, "installer": installed,
            })));
        }
        if available {
            out(format!(
                "{} nexus {} is available (you have {VERSION}). Run {}.",
                style.info,
                published.version,
                style.cyan("nexus update")
            ));
        } else if ordering.is_lt() {
            out(format!(
                "{} You have nexus {VERSION}, newer than the {} Nexus Gateway at {core_url} publishes.",
                style.ok, published.version
            ));
        } else {
            out(format!(
                "{} nexus {VERSION} is up to date {}",
                style.ok,
                style.dim(&format!("(Nexus Gateway at {core_url})"))
            ));
        }
        if !supported {
            out(format!(
                "  {} This Gateway needs {} or later; until you update, it refuses this CLI.",
                style.arrow, published.minimum
            ));
        }
        if let (Layout::Elsewhere { how, hint }, true) = (&layout, available) {
            out(format!("  {}", style.dim(&format!("{how} {hint}"))));
        }
        return Ok(0);
    }

    if !available {
        if json {
            return Ok(print(json!({
                "updated": false, "current": VERSION, "latest": published.version,
                "reason": if ordering.is_lt() { "newer_than_core" } else { "up_to_date" },
            })));
        }
        out(if ordering.is_lt() {
            format!(
                "{} You have nexus {VERSION}, newer than the {} Nexus Gateway at {core_url} publishes. Nothing to do.",
                style.ok, published.version
            )
        } else {
            format!("{} nexus {VERSION} is up to date.", style.ok)
        });
        return Ok(0);
    }
    let (lib, previous) = match &layout {
        Layout::Installed { lib, previous } => (lib.clone(), previous.clone()),
        Layout::Elsewhere { how, hint } => {
            return Err(CliError::hint(format!("{how} Nothing was changed."), hint.clone()).into());
        }
    };

    let code = download(&published.url)?;
    let actual = sha256_hex(&code);
    if actual != published.sha256 {
        return Err(CliError::hint(
            format!(
                "The downloaded CLI does not match its checksum (expected {}, got {actual}); nothing was changed.",
                published.sha256
            ),
            "Try again; if it persists, tell your Gateway's administrator.",
        )
        .into());
    }
    let folder = lib.parent().map(Path::to_path_buf).unwrap_or_default();
    let windows = lib.file_name().and_then(|s| s.to_str()).is_some_and(windows_binary);
    let suffix = if windows { ".exe" } else { "" };
    let temp = folder.join(format!(".nexus-update-{}-{}{suffix}", std::process::id(), random_hex()));
    let destination = if windows {
        folder.join(format!("{actual}.exe"))
    } else {
        lib.clone()
    };
    let installed_result = (|| -> std::result::Result<(), CliError> {
        let fail = |error: std::io::Error| {
            CliError::hint(
                format!(
                    "Could not install the new CLI in {}: {error}. Nothing was changed.",
                    folder.display()
                ),
                "Check that you can write to that folder.",
            )
        };
        write_executable(&temp, &code, &lib).map_err(fail)?;
        let reported = starts_as(&temp);
        if reported.as_deref() != Some(published.version.as_str()) {
            return Err(CliError::hint(
                format!(
                    "The downloaded CLI reports {}, not {}; nothing was changed.",
                    reported.as_deref().unwrap_or("no version"),
                    published.version
                ),
                "Try again; if it persists, tell your Gateway's administrator.",
            ));
        }
        if windows {
            if destination.exists() {
                if sha256_hex(&std::fs::read(&destination).map_err(fail)?) != actual {
                    return Err(CliError::hint(
                        "Installed executable checksum mismatch.",
                        "Reinstall the Windows CLI.",
                    ));
                }
                std::fs::remove_file(&temp).map_err(fail)?;
            } else {
                std::fs::rename(&temp, &destination).map_err(fail)?;
            }
            return select_windows_binary(&lib, &destination).map_err(fail);
        }
        // Keep the current version for --rollback (written aside, then renamed), then swap.
        let keep = PathBuf::from(format!("{}.{}.tmp", previous.display(), random_hex()));
        std::fs::copy(&lib, &keep).map_err(fail)?;
        std::fs::rename(&keep, &previous).map_err(fail)?;
        std::fs::rename(&temp, &lib).map_err(fail)?;
        Ok(())
    })();
    if let Err(error) = installed_result {
        let _ = std::fs::remove_file(&temp);
        return Err(error.into());
    }
    let previous = if windows { lib } else { previous };
    let lib = destination;
    refresh_cache(&update_cache_path(&ctx.config_path), core_url, &published.version);
    // Tab completion needs no manual step: each update turns it on, and a failure never fails the update.
    let completion =
        (ctx.var("NEXUS_NO_COMPLETION").as_deref() != Some("1")).then(|| crate::completion::install(&ctx.env));
    if json {
        return Ok(print(json!({
            "updated": true, "from": VERSION, "to": published.version,
            "path": lib.display().to_string(), "previous": previous.display().to_string(),
        })));
    }
    out(format!(
        "{} Updated nexus {VERSION} → {}",
        style.ok,
        style.bold(&published.version)
    ));
    out(String::new());
    for line in pairs(
        style,
        &[
            kv("Installed", lib.display().to_string()),
            kv(
                "Previous",
                format!(
                    "{} {}",
                    previous.display(),
                    style.dim("(nexus update --rollback restores it)")
                ),
            ),
            kv("From", core_url),
        ],
    ) {
        out(line);
    }
    match completion {
        Some(Ok(done)) if done.iter().any(|setup| setup.new) => {
            let shells: Vec<&str> = done.iter().map(|setup| setup.shell).collect();
            out(String::new());
            out(format!(
                "{} Tab completion is on for {}: open a new terminal to use it.",
                style.ok,
                shells.join(", ")
            ));
        }
        Some(Err(error)) => {
            out(String::new());
            out(style.dim(&format!(
                "Tab completion was not turned on ({error}); run `nexus completion install`."
            )));
        }
        _ => {}
    }
    Ok(0)
}

fn download(url: &str) -> Result<Vec<u8>> {
    let response = http_client()
        .get(url)
        .header(CLI_VERSION_HEADER, VERSION)
        .timeout(Duration::from_secs(120))
        .send()
        .map_err(|_| {
            CliError::hint(
                format!("Could not download the CLI from {url}; nothing was changed."),
                "Check your connection and try again.",
            )
        })?;
    if !response.status().is_success() {
        return Err(CliError::hint(
            format!(
                "Could not download the CLI from {url} (HTTP {}); nothing was changed.",
                response.status().as_u16()
            ),
            "Try again later.",
        )
        .into());
    }
    let mut bytes = Vec::new();
    let mut response = response;
    response.read_to_end(&mut bytes).map_err(|_| {
        CliError::hint(
            format!("Could not download the CLI from {url}; nothing was changed."),
            "Check your connection and try again.",
        )
    })?;
    Ok(bytes)
}

fn random_hex() -> String {
    hex::encode(rand::random::<[u8; 4]>())
}

/// Write the new binary with the current one's permissions (executable).
fn write_executable(path: &Path, bytes: &[u8], like: &Path) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(like)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o755)
            | 0o100;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = like;
    Ok(())
}

/// Run a downloaded binary with `--version --json`: its version if it starts within 20 s, else `None`.
fn starts_as(path: &Path) -> Option<String> {
    let mut child = std::process::Command::new(path)
        .args(["--version", "--json"])
        .env("NEXUS_NO_UPDATE_CHECK", "1")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait().ok()? {
            Some(status) if status.success() => break,
            Some(_) => return None,
            None if Instant::now() > deadline => {
                let _ = child.kill();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut text = String::new();
    child.stdout.take()?.read_to_string(&mut text).ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()?
        .get("version")?
        .as_str()
        .map(String::from)
}

/// `--rollback`: swap `nexus` and `nexus.previous`, each by an atomic rename.
fn rollback(ctx: &Context, layout: &Layout, json: bool, style: &Style) -> Result<i32> {
    let (lib, previous) = match layout {
        Layout::Installed { lib, previous } => (lib, previous),
        Layout::Elsewhere { how, hint } => {
            return Err(CliError::hint(format!("{how} Nothing was changed."), hint.clone()).into());
        }
    };
    if !previous.exists() {
        return Err(CliError::hint(
            "There is no previous version to roll back to.",
            "`nexus update --rollback` works after an update by `nexus update`.",
        )
        .into());
    }
    let current = std::fs::read(lib)
        .ok()
        .and_then(|b| binary_version(&b))
        .unwrap_or_else(|| VERSION.to_string());
    let before = std::fs::read(previous)
        .ok()
        .and_then(|b| binary_version(&b))
        .unwrap_or_else(|| "the previous version".to_string());
    let aside = PathBuf::from(format!("{}.{}.tmp", lib.display(), random_hex()));
    let folder = lib.parent().map(Path::to_path_buf).unwrap_or_default();
    let windows = lib.file_name().and_then(|s| s.to_str()).is_some_and(windows_binary);
    let swapped = if windows {
        select_windows_binary(lib, previous)
    } else {
        std::fs::copy(lib, &aside)
            .and_then(|_| std::fs::rename(previous, lib))
            .and_then(|_| std::fs::rename(&aside, previous))
    };
    if let Err(error) = swapped {
        let _ = std::fs::remove_file(&aside);
        return Err(CliError::hint(
            format!("Could not roll back in {}: {error}.", folder.display()),
            "Check that you can write to that folder.",
        )
        .into());
    }
    let (lib, previous) = if windows { (previous, lib) } else { (lib, previous) };
    if json {
        ctx.io.out(&pretty(&json!({
            "rolledBack": true, "from": current, "to": before,
            "path": lib.display().to_string(), "previous": previous.display().to_string(),
        })));
        return Ok(0);
    }
    ctx.io.out(&format!(
        "{} Rolled back nexus {current} → {}",
        style.ok,
        style.bold(&before)
    ));
    ctx.io.out(&format!(
        "  {} {} again undoes it; {} installs the latest.",
        style.arrow,
        style.cyan("nexus update --rollback"),
        style.cyan("nexus update")
    ));
    Ok(0)
}

fn refresh_cache(path: &Path, core_url: &str, latest: &str) {
    let text = format!(
        "{}\n",
        pretty(&json!({ "checkedAt": iso(now_ms()), "coreUrl": core_url, "latest": latest }))
    );
    let _ = write_private(path, text.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_a_binary_declares() {
        let mut code = vec![0u8; 5000];
        code.extend_from_slice(b"junk /* @nexus/cli nope */ /* @nexus/cli 0.3.0+abc1234 */ tail");
        assert_eq!(binary_version(&code).as_deref(), Some("0.3.0+abc1234"));
        assert_eq!(binary_version(b"nothing here"), None);
        assert!(VERSION_MARKER.contains(VERSION));
    }

    #[test]
    fn windows_switch_keeps_the_running_executable_and_a_rollback_target() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let lib = root.join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/nexus.exe"), b"launcher").unwrap();
        let old_name = format!("{}.exe", sha256_hex(b"old"));
        let new_name = format!("{}.exe", sha256_hex(b"new"));
        let old = lib.join(&old_name);
        let new = lib.join(&new_name);
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        std::fs::write(lib.join("windows-active"), &old_name).unwrap();
        select_windows_binary(&old, &new).unwrap();
        assert_eq!(
            windows_layout(&new),
            Some(Layout::Installed {
                lib: new.clone(),
                previous: old.clone()
            })
        );
        assert_eq!(windows_layout(&old), None);
        select_windows_binary(&new, &old).unwrap();
        assert_eq!(
            windows_layout(&old),
            Some(Layout::Installed {
                lib: old.clone(),
                previous: new
            })
        );
        assert_eq!(std::fs::read(old).unwrap(), b"old");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn knows_the_installers_layout() {
        let dir = std::env::temp_dir().join(format!("nexus-layout-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("lib")).expect("lib");
        std::fs::create_dir_all(dir.join("bin")).expect("bin");
        let lib = dir.join("lib/nexus");
        std::fs::write(&lib, b"binary").expect("write");
        let lib = std::fs::canonicalize(&lib).expect("real");
        assert!(matches!(
            install_layout(Some(&lib), "http://core.test"),
            Layout::Elsewhere { .. }
        ));
        std::fs::write(
            dir.join("bin/nexus"),
            format!(
                "#!/bin/sh\nNEXUS_URL=\"${{NEXUS_URL:-x}}\" exec \"{}\" \"$@\"\n",
                lib.display()
            ),
        )
        .expect("wrapper");
        assert_eq!(
            install_layout(Some(&lib), "http://core.test"),
            Layout::Installed {
                previous: lib.with_file_name("nexus.previous"),
                lib: lib.clone(),
            }
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
