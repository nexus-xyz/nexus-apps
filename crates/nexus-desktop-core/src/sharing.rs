//! Chat sharing's audience and policy rules, mirroring the web client's rules for native desktop views.
//! Gateway remains authoritative and checks the policy again on each write.

use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Audience {
    #[default]
    Private,
    Org,
    Public,
    Teams,
    People,
    External,
}

impl Audience {
    pub const ALL: [Self; 6] = [
        Self::Private,
        Self::Org,
        Self::Public,
        Self::Teams,
        Self::People,
        Self::External,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Org => "org",
            Self::Public => "public",
            Self::Teams => "teams",
            Self::People => "people",
            Self::External => "external",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|kind| kind.key() == key).unwrap_or_default()
    }

    pub fn outside(self) -> bool {
        matches!(self, Self::Public | Self::External)
    }

    /// Same order and defaults as `externalShareAllowed` in the kit.
    pub fn blocked(self, settings: &Value, role: &str) -> Option<&'static str> {
        if !self.outside() {
            return None;
        }
        let mode = settings["artifacts.external"].as_str().unwrap_or("public");
        if mode == "off" {
            Some("Your organization has turned off sharing outside it.")
        } else if self == Self::Public && mode != "public" {
            Some("Your organization allows sharing with named people only, not public links.")
        } else if settings["artifacts.external.who"] == "admins" && role != "owner" && role != "admin" {
            Some("Only owners and admins can share outside your organization.")
        } else {
            None
        }
    }
}

/// A share is a frozen version. Replacing revokes older audiences; Share latest preserves earlier versions.
pub fn body(kind: Audience, picks: &[String], expiry: &str, allow_continue: bool, replace: bool) -> Value {
    let mut body = json!({"audience": kind.key(), "allowContinue": allow_continue, "replace": replace});
    match kind {
        Audience::Teams => body["teams"] = json!(picks),
        Audience::People | Audience::External => body["people"] = json!(picks),
        _ => {}
    }
    if kind == Audience::Public && !expiry.trim().is_empty() {
        // Native input accepts the same duration syntax as the API, or an absolute ISO timestamp.
        let key = if expiry.contains('T') { "expiresAt" } else { "expiresIn" };
        body[key] = json!(expiry.trim());
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_policy_matches_the_kit() {
        for kind in [Audience::Org, Audience::Teams, Audience::People] {
            assert_eq!(kind.blocked(&json!({"artifacts.external":"off"}), "member"), None);
        }
        assert!(
            Audience::External
                .blocked(&json!({"artifacts.external":"off"}), "owner")
                .is_some()
        );
        assert!(
            Audience::Public
                .blocked(&json!({"artifacts.external":"named"}), "owner")
                .is_some()
        );
        assert_eq!(
            Audience::External.blocked(&json!({"artifacts.external":"named"}), "member"),
            None
        );
        assert!(
            Audience::External
                .blocked(&json!({"artifacts.external.who":"admins"}), "member")
                .is_some()
        );
        for role in ["owner", "admin"] {
            assert_eq!(
                Audience::Public.blocked(&json!({"artifacts.external.who":"admins"}), role),
                None
            );
        }
        assert_eq!(Audience::Public.blocked(&json!({}), "member"), None);
    }

    #[test]
    fn version_updates_preserve_audience_and_replacement_semantics() {
        let picks = vec!["eng".into()];
        assert_eq!(
            body(Audience::Teams, &picks, "7d", false, false),
            json!({"audience":"teams", "teams":["eng"], "allowContinue":false, "replace":false})
        );
        assert_eq!(
            body(Audience::Public, &[], "12h", true, true),
            json!({"audience":"public", "expiresIn":"12h", "allowContinue":true, "replace":true})
        );
        let at = "2027-01-01T00:00:00Z";
        assert_eq!(body(Audience::Public, &[], at, true, false)["expiresAt"], at);
        assert!(
            body(Audience::External, &picks, "1d", true, false)
                .get("expiresIn")
                .is_none()
        );
    }
}
