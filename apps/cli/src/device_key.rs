//! This computer's device key for the CLI's session sync: the X25519 and Ed25519 pairs of
//! `nexus_client::e2e::DeviceKey`, one per Gateway, as the desktop app keeps its own (`src-tauri/src/device_key.rs`).
//!
//! The secret goes to the OS keychain when this computer has one the CLI can reach without a new dependency: macOS's
//! `security` and the Secret Service's `secret-tool` (GNOME Keyring, KWallet). Each takes the secret on stdin, never as
//! an argument. Elsewhere (Windows, a server without a keyring) it goes to `device-keys.json` beside the CLI's config,
//! 0600, where the CLI's own sign-in already is. That file always records where each key is, so a keychain that
//! cannot be read later is an error, never a reason to make a new key and orphan the paired device.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nexus_client::e2e::{DEVICE_SECRET_LEN, DeviceKey};
use serde_json::{Map, Value, json};

use crate::Context;
use crate::config::write_private;
use crate::error::{CliError, Result};

/// `NEXUS_DEVICE_KEY_STORE=file` keeps the key in the file even where a keychain is (tests, containers).
pub const STORE_VARIABLE: &str = "NEXUS_DEVICE_KEY_STORE";
const SERVICE: &str = "xyz.nexus.cli";

/// The keychain account of `gateway`'s device key, as the desktop app names its own.
fn account(gateway: &str) -> String {
    format!("{gateway}#e2e-device")
}

fn path(ctx: &Context) -> PathBuf {
    ctx.config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join("device-keys.json")
}

fn read_index(ctx: &Context) -> Result<Map<String, Value>> {
    match std::fs::read_to_string(path(ctx)) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .ok_or_else(|| unreadable(ctx)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(_) => Err(unreadable(ctx)),
    }
}

fn write_index(ctx: &Context, index: &Map<String, Value>) -> Result<()> {
    let text = serde_json::to_string_pretty(index).unwrap_or_default();
    write_private(&path(ctx), format!("{text}\n").as_bytes())
        .map_err(|error| CliError::new(format!("Could not save {}: {error}", path(ctx).display())).into())
}

fn unreadable(ctx: &Context) -> crate::error::Error {
    CliError::hint(
        format!("{} is not a file Nexus can read.", path(ctx).display()),
        "Do not delete it if this computer is paired: it says where the device key is.",
    )
    .into()
}

fn decode(text: &str) -> Option<DeviceKey> {
    let bytes = URL_SAFE_NO_PAD.decode(text.trim()).ok()?;
    let secret: &[u8; DEVICE_SECRET_LEN] = bytes.as_slice().try_into().ok()?;
    Some(DeviceKey::from_secret_bytes(secret))
}

/// The OS keychain the CLI can use here, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keychain {
    /// macOS: `security`.
    Mac,
    /// The Secret Service: `secret-tool`.
    SecretService,
}

impl Keychain {
    fn here(ctx: &Context) -> Option<Keychain> {
        if ctx.var(STORE_VARIABLE).as_deref() == Some("file") {
            return None;
        }
        if cfg!(target_os = "macos") {
            return Some(Keychain::Mac);
        }
        if cfg!(unix) && ctx.var("DBUS_SESSION_BUS_ADDRESS").is_some_and(|bus| !bus.is_empty()) {
            return Some(Keychain::SecretService);
        }
        None
    }

    fn get(self, gateway: &str) -> Option<String> {
        let account = account(gateway);
        let output = match self {
            Keychain::Mac => Command::new("/usr/bin/security")
                .args(["find-generic-password", "-s", SERVICE, "-a", &account, "-w"])
                .stderr(Stdio::null())
                .output(),
            Keychain::SecretService => Command::new("secret-tool")
                .args(["lookup", "service", SERVICE, "account", &account])
                .stderr(Stdio::null())
                .output(),
        }
        .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|text| !text.is_empty())
    }

    /// Stores `secret`, then reads it back: true only when the keychain holds it.
    fn set(self, gateway: &str, secret: &str) -> bool {
        let account = account(gateway);
        let (mut command, input) = match self {
            // `security -i` reads its commands from stdin, so the secret is in no process's arguments.
            Keychain::Mac => (
                Command::new("/usr/bin/security"),
                format!("add-generic-password -U -s {SERVICE} -a \"{account}\" -w {secret}\n"),
            ),
            Keychain::SecretService => {
                let mut command = Command::new("secret-tool");
                command.args([
                    "store",
                    "--label",
                    "Nexus CLI device key",
                    "service",
                    SERVICE,
                    "account",
                    &account,
                ]);
                (command, secret.to_string())
            }
        };
        if self == Keychain::Mac {
            command.arg("-i");
        }
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            return false;
        };
        let wrote = child
            .stdin
            .take()
            .is_some_and(|mut stdin| stdin.write_all(input.as_bytes()).is_ok());
        let stored = child.wait().is_ok_and(|status| status.success());
        wrote && stored && self.get(gateway).as_deref() == Some(secret)
    }

    fn delete(self, gateway: &str) {
        let account = account(gateway);
        let _ = match self {
            Keychain::Mac => Command::new("/usr/bin/security")
                .args(["delete-generic-password", "-s", SERVICE, "-a", &account])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status(),
            Keychain::SecretService => Command::new("secret-tool")
                .args(["clear", "service", SERVICE, "account", &account])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status(),
        };
    }
}

/// Where a key is: the keychain, or the file.
fn entry_of(index: &Map<String, Value>, gateway: &str) -> Option<Value> {
    index.get(gateway).cloned()
}

/// The stored key for `gateway`, if any. A key this computer recorded and cannot read is an error.
pub fn stored(ctx: &Context, gateway: &str) -> Result<Option<DeviceKey>> {
    let index = read_index(ctx)?;
    let Some(entry) = entry_of(&index, gateway) else {
        return Ok(None);
    };
    let text = match entry.get("store").and_then(Value::as_str) {
        Some("file") => entry.get("secret").and_then(Value::as_str).map(String::from),
        Some("keychain") => {
            let keychain = match entry.get("keychain").and_then(Value::as_str) {
                Some("mac") => Keychain::Mac,
                _ => Keychain::SecretService,
            };
            keychain.get(gateway)
        }
        _ => None,
    };
    let Some(device) = text.as_deref().and_then(decode) else {
        return Err(CliError::hint(
            "Nexus cannot read this computer's device key.",
            "Unlock your keychain and try again. If the key is gone, run `nexus devices pair --new` and revoke the old device.",
        )
        .into());
    };
    Ok(Some(device))
}

/// The stored key, or a new one stored now. True when it is new.
pub fn load_or_create(ctx: &Context, gateway: &str) -> Result<(DeviceKey, bool)> {
    if let Some(device) = stored(ctx, gateway)? {
        return Ok((device, false));
    }
    let device = DeviceKey::generate();
    let secret = URL_SAFE_NO_PAD.encode(&device.secret_bytes()[..]);
    let mut index = read_index(ctx)?;
    let entry = match Keychain::here(ctx).filter(|keychain| keychain.set(gateway, &secret)) {
        Some(keychain) => json!({
            "store": "keychain",
            "keychain": if keychain == Keychain::Mac { "mac" } else { "secret-service" },
        }),
        None => json!({ "store": "file", "secret": secret }),
    };
    index.insert(gateway.to_string(), entry);
    write_index(ctx, &index)?;
    Ok((device, true))
}

/// Deletes the key (after this device was revoked): the next pairing makes a new one.
pub fn forget(ctx: &Context, gateway: &str) -> Result<()> {
    let mut index = read_index(ctx)?;
    if let Some(entry) = index.remove(gateway) {
        if entry.get("store").and_then(Value::as_str) == Some("keychain") {
            let keychain = match entry.get("keychain").and_then(Value::as_str) {
                Some("mac") => Keychain::Mac,
                _ => Keychain::SecretService,
            };
            keychain.delete(gateway);
        }
        write_index(ctx, &index)?;
    }
    Ok(())
}

/// Where `gateway`'s key is kept, for `nexus devices`: `keychain` or `file`.
pub fn kept_in(ctx: &Context, gateway: &str) -> Option<String> {
    let index = read_index(ctx).ok()?;
    let entry = entry_of(&index, gateway)?;
    entry.get("store").and_then(Value::as_str).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &Path) -> Context {
        Context {
            config_path: dir.join("config.json"),
            io: Box::new(crate::Capture::default()),
            color: false,
            env: [(STORE_VARIABLE.to_string(), "file".to_string())].into(),
            poll_ms: Some(0),
            hostname: None,
            cwd: dir.to_path_buf(),
            interactive: false,
            exe_path: None,
            columns: None,
            picker: false,
        }
    }

    #[test]
    fn a_key_is_made_once_per_gateway_and_kept_private() {
        let dir = std::env::temp_dir().join(format!("nexus-device-key-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let ctx = ctx(&dir);
        assert!(stored(&ctx, "https://a.example").expect("read").is_none());
        let (made, new) = load_or_create(&ctx, "https://a.example").expect("make");
        assert!(new);
        let (again, new) = load_or_create(&ctx, "https://a.example").expect("load");
        assert!(!new);
        assert_eq!(again.fingerprint(), made.fingerprint());
        let (other, _) = load_or_create(&ctx, "https://b.example").expect("make");
        assert_ne!(other.fingerprint(), made.fingerprint(), "one key per Gateway");
        assert_eq!(kept_in(&ctx, "https://a.example").as_deref(), Some("file"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path(&ctx)).expect("file").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        forget(&ctx, "https://a.example").expect("forget");
        assert!(stored(&ctx, "https://a.example").expect("read").is_none());
        assert!(stored(&ctx, "https://b.example").expect("read").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_recorded_key_that_cannot_be_read_is_an_error_never_a_new_key() {
        let dir = std::env::temp_dir().join(format!("nexus-device-key-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let ctx = ctx(&dir);
        let index = json!({ "https://a.example": { "store": "file", "secret": "not a key" } });
        std::fs::write(path(&ctx), index.to_string()).expect("write");
        assert!(stored(&ctx, "https://a.example").is_err());
        assert!(load_or_create(&ctx, "https://a.example").is_err());
        std::fs::write(path(&ctx), "{ broken").expect("write");
        assert!(stored(&ctx, "https://a.example").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
