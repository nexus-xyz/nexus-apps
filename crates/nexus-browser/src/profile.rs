//! Which website profile a session's browser uses: one per Nexus sign-in, so conversations share their sign-in's
//! website logins and cookies, and other sign-ins, other apps and a cloud machine's pages never see them. Nexus
//! GPUI (`src/side/browser/`) keeps the engine's files.

use sha2::{Digest, Sha256};

/// One website profile: its id, and whether it is saved on this computer. A profile that is not saved lives in memory
/// (a cloud machine's, or any when the organization keeps nothing on this computer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Profile {
    pub id: [u8; 16],
    pub persistent: bool,
}

impl Profile {
    /// The profile of `scope` in `app` (the app's identifier, so two apps never share one).
    pub fn new(app: &str, scope: &str, persistent: bool) -> Profile {
        Profile {
            id: identifier(app, scope),
            persistent,
        }
    }

    /// The profile's folder name: its id as a UUID.
    pub fn folder(&self) -> String {
        uuid::Uuid::from_bytes(self.id).to_string()
    }
}

/// A sign-in's scope: its Gateway, organization and a fingerprint of its credential
/// (`nexus_desktop_core::secrets::marker`), never the credential.
pub fn scope(gateway: &str, org: &str, marker: &str) -> String {
    format!("{gateway}|{org}|{marker}")
}

/// A cloud machine's scope: its pages get their own profile, never saved.
pub fn cloud_scope(machine: &str) -> String {
    format!("cloud|{machine}")
}

fn identifier(app: &str, scope: &str) -> [u8; 16] {
    let mut hash = Sha256::new();
    hash.update(b"nexus-browser-profile-v1\0");
    hash.update(app.as_bytes());
    hash.update(b"\0");
    hash.update(scope.as_bytes());
    let mut id = [0; 16];
    id.copy_from_slice(&hash.finalize()[..16]);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_survive_restarts_and_separate_accounts_and_apps() {
        let id = identifier("nexus", "gateway|org|person-a");
        assert_eq!(identifier("nexus", "gateway|org|person-a"), id);
        assert_ne!(identifier("nexus", "gateway|org|person-b"), id);
        assert_ne!(identifier("nexus-review", "gateway|org|person-a"), id);
    }

    #[test]
    fn a_sign_in_is_its_gateway_organization_and_credential() {
        assert_eq!(
            scope("https://api.nexus.inc", "org_1", "desktop:00ff"),
            "https://api.nexus.inc|org_1|desktop:00ff"
        );
        let a = Profile::new("app", &scope("g", "org_1", "m1"), true);
        assert_ne!(
            a,
            Profile::new("app", &scope("g", "org_2", "m1"), true),
            "another organization"
        );
        assert_ne!(
            a,
            Profile::new("app", &scope("g", "org_1", "m2"), true),
            "another credential"
        );
        assert_ne!(
            a,
            Profile::new("app", &scope("h", "org_1", "m1"), true),
            "another Gateway"
        );
        assert_ne!(a.id, Profile::new("app", &cloud_scope("m_1"), false).id);
        assert_eq!(a.folder().len(), 36);
    }
}
