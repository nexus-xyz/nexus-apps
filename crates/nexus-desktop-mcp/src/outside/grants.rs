//! The grants of outside access: one per client the person approved, with its scope and when it was last used. The
//! file keeps a SHA-256 of each token, never the token: the client holds that. Revoking deletes the grant, so the next
//! request with its token is refused.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a client may do: read sessions, or read them and act on them (start, message, stop).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read,
    Act,
}

impl Scope {
    pub fn parse(value: &str) -> Option<Scope> {
        match value {
            "read" => Some(Scope::Read),
            "act" => Some(Scope::Act),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub id: String,
    /// The name the client gave. Clients name themselves, so the consent prompt says so.
    pub client: String,
    pub scope: Scope,
    /// Hex SHA-256 of the token. Never sent to the webview.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    token_hash: String,
    pub created_at: i64,
    #[serde(default)]
    pub last_used_at: Option<i64>,
}

/// A grant as Settings shows it: no hash.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantView {
    pub id: String,
    pub client: String,
    pub scope: Scope,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    grants: Vec<Grant>,
}

/// The grants, and the file they are saved in (none in tests).
#[derive(Debug, Default)]
pub struct Grants {
    path: Option<PathBuf>,
    grants: Vec<Grant>,
}

use nexus_workbench::orchestra::grant::sha256_hex as hash;

pub fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

impl Grants {
    /// The grants saved at `path`; none when it is missing or does not read.
    pub fn load(path: PathBuf) -> Grants {
        let grants = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<File>(&bytes).ok())
            .map(|file| file.grants)
            .unwrap_or_default();
        Grants {
            path: Some(path),
            grants,
        }
    }

    pub fn views(&self) -> Vec<GrantView> {
        self.grants
            .iter()
            .map(|grant| GrantView {
                id: grant.id.clone(),
                client: grant.client.clone(),
                scope: grant.scope,
                created_at: grant.created_at,
                last_used_at: grant.last_used_at,
            })
            .collect()
    }

    /// A new grant for `client` with a new token, which only the caller sees. It replaces the client's earlier grant.
    pub fn add(&mut self, client: &str, scope: Scope) -> Result<String, String> {
        let token = nexus_workbench::orchestra::grant::new_token();
        self.grants.retain(|grant| grant.client != client);
        self.grants.push(Grant {
            id: uuid::Uuid::new_v4().simple().to_string(),
            client: client.into(),
            scope,
            token_hash: hash(&token),
            created_at: now_s(),
            last_used_at: None,
        });
        self.save()?;
        Ok(token)
    }

    pub fn revoke(&mut self, id: &str) -> Result<bool, String> {
        let before = self.grants.len();
        self.grants.retain(|grant| grant.id != id);
        if self.grants.len() == before {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    /// The grant that holds `token`, marked used now. None: no token, or a revoked or unknown one.
    pub fn authorize(&mut self, token: &str) -> Option<Grant> {
        if token.is_empty() {
            return None;
        }
        let wanted = hash(token);
        let grant = self
            .grants
            .iter_mut()
            .find(|grant| nexus_workbench::orchestra::grant::same_token(&grant.token_hash, &wanted))?;
        let now = now_s();
        // Saved at most once a minute: last use is for the person's eyes, not an audit log.
        let stale = grant.last_used_at.is_none_or(|at| now - at >= 60);
        grant.last_used_at = Some(now);
        let found = grant.clone();
        if stale {
            let _ = self.save();
        }
        Some(found)
    }

    /// Writes the file private to the person (0600), through a temporary file and a rename.
    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(&File {
            grants: self.grants.clone(),
        })
        .map_err(|error| error.to_string())?;
        write_private(path, &bytes).map_err(|error| format!("Could not save outside access: {error}"))
    }
}

pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    nexus_desktop_core::atomic_file::write_atomic_private(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_reaches_its_grant_until_it_is_revoked() {
        let mut grants = Grants::default();
        let token = grants.add("Claude Code", Scope::Read).expect("add");
        assert!(grants.authorize("").is_none());
        assert!(grants.authorize("wrong").is_none());
        let grant = grants.authorize(&token).expect("granted");
        assert_eq!((grant.client.as_str(), grant.scope), ("Claude Code", Scope::Read));
        assert!(grant.last_used_at.is_some());
        assert!(grants.revoke(&grant.id).expect("revoke"));
        assert!(grants.authorize(&token).is_none());
    }

    #[test]
    fn a_new_grant_replaces_the_clients_earlier_one() {
        let mut grants = Grants::default();
        let first = grants.add("nexus CLI", Scope::Read).expect("add");
        let second = grants.add("nexus CLI", Scope::Act).expect("add");
        assert!(grants.authorize(&first).is_none());
        assert_eq!(grants.authorize(&second).map(|grant| grant.scope), Some(Scope::Act));
        assert_eq!(grants.views().len(), 1);
    }

    #[test]
    fn the_file_is_private_and_holds_no_token() {
        let dir = std::env::temp_dir().join(format!("nexus-outside-grants-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("outside-access.json");
        let mut grants = Grants::load(path.clone());
        let token = grants.add("script", Scope::Act).expect("add");
        let text = std::fs::read_to_string(&path).expect("saved");
        assert!(!text.contains(&token));
        assert!(text.contains(&hash(&token)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let mut again = Grants::load(path);
        assert!(again.authorize(&token).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
