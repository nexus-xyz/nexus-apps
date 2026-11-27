//! The SSH key the app makes for each cloud machine (one key per machine, made by the app, kept in its data
//! folder). Made by the system `ssh-keygen` (the same OpenSSH the app
//! already runs as `ssh`), so the app carries no key-generation code of its own: ed25519, no passphrase (the app has
//! no way to ask for one at connect time), readable only by this user.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use nexus_workbench::process::async_command;

use crate::{Error, Result};

/// A key pair on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyPair {
    /// The private key file (what `ssh -i` reads).
    pub private: PathBuf,
    /// The public key, one line, `ssh-ed25519 AAAA… nexus-cloud` (what the provider is given).
    pub public: String,
}

/// The `.pub` beside a private key.
fn public_path(private: &Path) -> PathBuf {
    let mut name = private.as_os_str().to_owned();
    name.push(".pub");
    PathBuf::from(name)
}

/// `ssh-keygen`, or `NEXUS_SSH_KEYGEN` (another OpenSSH, as `NEXUS_SSH` is for `ssh`; debug builds only).
fn keygen_program() -> PathBuf {
    nexus_remote::development_setting("NEXUS_SSH_KEYGEN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ssh-keygen"))
}

/// The key pair at `private` (and `private.pub`), made first when there is none. A private key without its public
/// half is an error rather than replaced: something else wrote there, and the machine may already trust it.
pub async fn load_or_generate(private: &Path) -> Result<KeyPair> {
    let public = public_path(private);
    let exists = tokio::fs::try_exists(private).await.unwrap_or(false);
    if !exists {
        generate(private).await?;
    }
    let line = tokio::fs::read_to_string(&public)
        .await
        .map_err(|e| Error::Other(format!("Could not read the SSH key {}: {e}", public.display())))?;
    let line = line.trim().to_string();
    if !line.starts_with("ssh-ed25519 ") {
        return Err(Error::Other(format!(
            "{} is not an ed25519 public key.",
            public.display()
        )));
    }
    protect(private).await?;
    Ok(KeyPair {
        private: private.to_path_buf(),
        public: line,
    })
}

/// `ssh-keygen -t ed25519 -N "" -C nexus-cloud -f <private>`.
async fn generate(private: &Path) -> Result<()> {
    if let Some(parent) = private.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::Other(format!("Could not create {}: {e}", parent.display())))?;
    }
    // A stale `.pub` would make ssh-keygen ask whether to overwrite; nothing answers it here.
    let _ = tokio::fs::remove_file(public_path(private)).await;
    let output = async_command(keygen_program())
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "nexus-cloud", "-f"])
        .arg(private)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| Error::Other(format!("Could not run ssh-keygen: {e}. Is OpenSSH installed?")))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::Other(format!("ssh-keygen could not make a key: {said}")));
    }
    Ok(())
}

/// The private key readable only by this user (ssh refuses one others can read). ssh-keygen already writes it so on
/// Unix; this holds for a key copied in by hand too. On Windows ssh-keygen sets the file's access list itself.
async fn protect(private: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(private, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|e| Error::Other(format!("Could not protect {}: {e}", private.display())))?;
    }
    #[cfg(not(unix))]
    let _ = private;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn makes_a_private_ed25519_key_once() {
        if nexus_workbench::process::command(keygen_program())
            .arg("-?")
            .stderr(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipped: no ssh-keygen on this computer");
            return;
        }
        let dir = std::env::temp_dir().join(format!("nexus-cloud-keys-{}", std::process::id()));
        let private = dir.join("machines").join("bx_23456789");
        let first = load_or_generate(&private).await.unwrap_or_else(|e| panic!("{e}"));
        assert!(first.public.starts_with("ssh-ed25519 "));
        assert!(first.public.ends_with(" nexus-cloud"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&private)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0);
            assert_eq!(mode, 0o600);
        }
        let again = load_or_generate(&private).await.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(again, first);
        let _ = std::fs::remove_file(public_path(&private));
        assert!(load_or_generate(&private).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
