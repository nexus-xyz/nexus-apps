//! What an administrator decides for Nexus on this computer, read once at launch through `nexus-desktop-core`
//! (`settings::managed`): macOS managed preferences, Windows Group Policy and the managed JSON file, merged into one
//! `Policy`. This build applies these keys:
//!
//! - `GatewayURL` (forced) and the JSON file's `lockUrl`: the Gateway is locked. Sign-in and Settings › Gateways show it
//!   and offer no other.
//! - `Organization` with `AllowOrganizationSwitching` off: only that organization is listed, and the app moves to it
//!   after sign-in.
//! - `AllowLocalCache` off: the web pages' profile stays in memory, and Keep agents running stays off.
//! - `LaunchAtLogin` and `GlobalShortcutEnabled`: shown as set by the organization on Settings › App.
//!
//! `ScreenCaptureEnabled` has nothing to act on here: this build has no screen recording.

use std::sync::OnceLock;

use nexus_desktop_core::policy::{Policy, key};
use serde_json::Value;

/// The note beside a control the organization set.
pub const SET_BY_ORG: &str = "Set by your organization";

static POLICY: OnceLock<Policy> = OnceLock::new();

/// This computer's policy, read on first use and kept until the app quits.
pub fn policy() -> &'static Policy {
    POLICY.get_or_init(|| nexus_desktop_core::settings::managed().0)
}

/// The Gateway the organization locked this computer to, if it did.
pub fn locked_gateway(policy: &Policy) -> Option<String> {
    policy
        .managed
        .as_ref()
        .filter(|managed| managed.lock_url)
        .and_then(|managed| managed.url.clone())
        .map(|url| url.trim_end_matches('/').to_string())
}

/// Why a Gateway switch is refused, when the organization locked the Gateway.
pub const GATEWAY_LOCKED: &str = "Your organization set the Gateway for this computer.";

/// Whether the organization keeps data on this computer (`AllowLocalCache`, on by default).
pub fn local_cache(policy: &Policy) -> bool {
    policy.allow_local_cache
}

/// An organization of `GET /v1/orgs`, as the policy reads it.
fn ids(org: &Value) -> (&str, Option<&str>) {
    (
        org.get("id").and_then(Value::as_str).unwrap_or_default(),
        org.get("slug").and_then(Value::as_str),
    )
}

/// Whether the switcher lists `org`: every one while switching is allowed; else the
/// pinned one, or without a pin the current one.
pub fn listed(policy: &Policy, org: &Value) -> bool {
    if policy.allow_organization_switching {
        return true;
    }
    let (id, slug) = ids(org);
    let current = org.get("current").and_then(Value::as_bool) == Some(true);
    policy.may_switch_to(id, slug) || (policy.organization.is_none() && current)
}

/// After a sign-in, where the pinned organization sends the person, from
/// `GET /v1/orgs`'s answer: `Ok(None)` when there is no pin or the person is in it already, `Ok(Some(id))` to switch,
/// and an error when this account is not a member.
pub fn pinned_target(policy: &Policy, orgs: &Value) -> Result<Option<String>, String> {
    let Some(pinned) = policy.organization.as_deref() else {
        return Ok(None);
    };
    let list = orgs.get("orgs").and_then(Value::as_array).cloned().unwrap_or_default();
    let target = list
        .iter()
        .find(|org| {
            let (id, slug) = ids(org);
            id == pinned || slug == Some(pinned)
        })
        .ok_or_else(|| {
            format!("Your organization set this computer to {pinned}, which this account is not a member of.")
        })?;
    if target.get("current").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    Ok(Some(ids(target).0.to_string()))
}

/// The policy keys an administrator forced, in words, for Settings › App.
pub fn forced_names(policy: &Policy) -> Vec<&'static str> {
    let name = |forced: &str| match forced {
        key::GATEWAY_URL => Some("Gateway"),
        key::NEXUS_UI_URL => Some("Nexus UI address"),
        key::ORGANIZATION => Some("Organization"),
        key::ALLOW_ORGANIZATION_SWITCHING => Some("Switching organizations"),
        key::ALLOW_LOCAL_CACHE => Some("Saving on this computer"),
        key::GLOBAL_SHORTCUT_ENABLED => Some("Global shortcut"),
        key::LAUNCH_AT_LOGIN => Some("Open at login"),
        key::SCREEN_CAPTURE_ENABLED => Some("Screen recordings"),
        _ => None,
    };
    let mut names: Vec<&'static str> = policy.forced.iter().filter_map(|forced| name(forced)).collect();
    if locked_gateway(policy).is_some() && !names.contains(&"Gateway") {
        names.insert(0, "Gateway");
    }
    names
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use nexus_client::config::ManagedConfig;
    use nexus_desktop_core::policy::{Source, Value as Pref};
    use serde_json::json;
    use std::collections::BTreeMap;

    /// A policy source from a map, every key forced: a configuration profile or Group Policy in a test.
    pub struct Fake(pub BTreeMap<&'static str, Pref>);

    impl Source for Fake {
        fn value(&self, key: &str) -> Option<Pref> {
            self.0.get(key).cloned()
        }
        fn is_forced(&self, key: &str) -> bool {
            self.0.contains_key(key)
        }
        fn describe(&self) -> String {
            "test".into()
        }
    }

    pub fn policy_of(prefs: &[(&'static str, Pref)], file: Option<ManagedConfig>) -> Policy {
        Policy::merge(&Fake(prefs.iter().cloned().collect()), file, "managed.json")
    }

    #[test]
    fn a_forced_gateway_is_locked() {
        let policy = policy_of(
            &[(key::GATEWAY_URL, Pref::Text("https://nexus.acme.test/".into()))],
            None,
        );
        assert_eq!(locked_gateway(&policy).as_deref(), Some("https://nexus.acme.test"));
        assert_eq!(forced_names(&policy), vec!["Gateway"]);
        // The JSON file's lockUrl locks too; a URL without it is a default, not a lock.
        let file = ManagedConfig {
            url: Some("https://nexus.acme.test".into()),
            lock_url: true,
            ..ManagedConfig::default()
        };
        assert!(locked_gateway(&policy_of(&[], Some(file.clone()))).is_some());
        let unlocked = ManagedConfig {
            lock_url: false,
            ..file
        };
        assert_eq!(locked_gateway(&policy_of(&[], Some(unlocked))), None);
        assert_eq!(locked_gateway(&policy_of(&[], None)), None);
    }

    #[test]
    fn a_pinned_organization_is_the_only_one_listed_and_the_app_moves_to_it() {
        let orgs = json!({ "orgs": [
            { "id": "org_personal", "kind": "personal", "current": true },
            { "id": "org_acme", "slug": "acme" },
            { "id": "org_other", "slug": "other" }
        ]});
        let all = orgs["orgs"].as_array().cloned().unwrap_or_default();
        let pinned = policy_of(
            &[
                (key::ORGANIZATION, Pref::Text("acme".into())),
                (key::ALLOW_ORGANIZATION_SWITCHING, Pref::Bool(false)),
            ],
            None,
        );
        let shown: Vec<&str> = all
            .iter()
            .filter(|org| listed(&pinned, org))
            .filter_map(|org| org["id"].as_str())
            .collect();
        assert_eq!(shown, vec!["org_acme"]);
        assert_eq!(pinned_target(&pinned, &orgs), Ok(Some("org_acme".into())));
        let there = json!({ "orgs": [{ "id": "org_acme", "slug": "acme", "current": true }] });
        assert_eq!(pinned_target(&pinned, &there), Ok(None));
        assert!(pinned_target(&pinned, &json!({ "orgs": [] })).is_err());

        // Switching off with no pin keeps the current organization only; switching on lists every one.
        let off = policy_of(&[(key::ALLOW_ORGANIZATION_SWITCHING, Pref::Bool(false))], None);
        assert_eq!(all.iter().filter(|org| listed(&off, org)).count(), 1);
        let free = policy_of(&[], None);
        assert_eq!(all.iter().filter(|org| listed(&free, org)).count(), 3);
        assert_eq!(pinned_target(&free, &orgs), Ok(None));
    }

    #[test]
    fn local_cache_launch_at_login_and_the_shortcut_follow_the_policy() {
        let policy = policy_of(
            &[
                (key::ALLOW_LOCAL_CACHE, Pref::Number(0)),
                (key::LAUNCH_AT_LOGIN, Pref::Bool(true)),
                (key::GLOBAL_SHORTCUT_ENABLED, Pref::Text("false".into())),
            ],
            None,
        );
        assert!(!local_cache(&policy));
        assert_eq!(policy.launch_at_login, Some(true));
        assert!(!policy.global_shortcut_enabled);
        assert_eq!(
            forced_names(&policy),
            vec!["Saving on this computer", "Global shortcut", "Open at login"]
        );
        assert!(local_cache(&policy_of(&[], None)));
    }
}
