//! A secret's file, as `request_secret` delivers it: 0600 in a 0700 folder per session, a random name, the agent told
//! only its path. The desktop apps write it on their own computer; `nexus-host` writes it on a person's own server from
//! the value the app hands back in its answer (`app_relay`), so the agent there reads it.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::app_tools::SECRET_TTL;

/// The field of a relayed `request_secret` answer that carries the person's value to a server's host, which writes it
/// and puts the file's path in its place ([`take_value`]). Only an app connected over SSH sends it.
pub const VALUE_FIELD: &str = "nexusSecret";

/// The secret's name and value in a relayed answer, taken out of it. None: the answer carries none.
pub fn take_value(reply: &mut Value) -> Option<(String, String)> {
    let carried = reply.as_object_mut()?.remove(VALUE_FIELD)?;
    let name = carried["name"].as_str()?.to_string();
    let value = carried["value"].as_str()?.to_string();
    Some((name, value))
}

/// The answer the agent gets once the file is written: `reply` with its text naming the file.
pub fn provided(reply: &Value, name: &str, path: &Path) -> Value {
    serde_json::json!({"jsonrpc": "2.0", "id": reply["id"],
        "result": {"content": [{"type": "text", "text": provided_text(name, path)}], "isError": false}})
}

/// A folder that only this user can enter: made with mode 0700, or set to it. Refuses a link in its place.
fn private_dir(dir: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(|error| error.to_string())?;
    let meta = std::fs::symlink_metadata(dir).map_err(|error| error.to_string())?;
    if !meta.is_dir() {
        return Err(format!("{} is not a folder.", dir.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The session's folder under `base`: the session id, with any character but a letter, a digit or `-` dropped.
pub fn session_dir(base: &Path, thread: &str) -> PathBuf {
    let name: String = thread
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(64)
        .collect();
    base.join(if name.is_empty() { "session".into() } else { name })
}

/// Writes `value` to a new file with a random name, mode 0600, in the session's folder under `base`. The error says
/// what failed, never the value.
pub fn write(base: &Path, thread: &str, value: &str) -> Result<PathBuf, String> {
    let failed = |error: std::io::Error| format!("Nexus could not write the secret's file: {}", error.kind());
    private_dir(base).map_err(|_| "Nexus could not make the secrets folder.".to_string())?;
    let dir = session_dir(base, thread);
    private_dir(&dir).map_err(|_| "Nexus could not make the session's secrets folder.".to_string())?;
    let path = dir.join(uuid::Uuid::new_v4().simple().to_string());
    let mut options = std::fs::OpenOptions::new();
    // A new file only: never one that is there already, or a link.
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&path).map_err(failed)?;
    if let Err(error) = file.write_all(value.as_bytes()).and_then(|()| file.flush()) {
        drop(file);
        remove(&path);
        return Err(failed(error));
    }
    Ok(path)
}

/// Deletes a file, and its folder when that is then empty. One already gone is fine.
pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent() {
        // Fails while the folder holds another file: it stays.
        let _ = std::fs::remove_dir(dir);
    }
}

/// What the agent reads when the person provided the secret: the path and how to use it. Never the value.
pub fn provided_text(name: &str, path: &Path) -> String {
    let path = path.display();
    format!(
        "The person provided {name}. It is in the file {path}, readable only by this user. It is deleted when this \
         agent run ends, or {} minutes from now. Read it only inside a shell command, for example \
         `VALUE=\"$(cat '{path}')\" the-command`, or pass the path to a --from-file option. Never print it, echo it, \
         or copy it into files in the repository.",
        SECRET_TTL.as_secs() / 60
    )
}
