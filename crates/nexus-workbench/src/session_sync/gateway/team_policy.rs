//! Machine-local opt-in and administrator-admitted member roots. Gateway cannot write this file.
use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use super::super::trust::{Holder, Keys, key32};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    session_team_sync: Vec<Policy>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Policy {
    gateway: String,
    tenant_id: String,
    members: Vec<Admission>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Admission {
    person_id: String,
    device_id: String,
    encryption_key: String,
    signing_key: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct TeamPolicy {
    pub roots: HashMap<String, Holder>,
}

impl TeamPolicy {
    pub fn load(path: &Path, gateway: &str, tenant: &str) -> Option<Self> {
        Self::parse(&std::fs::read(path).ok()?, gateway, tenant)
    }

    fn parse(bytes: &[u8], gateway: &str, tenant: &str) -> Option<Self> {
        if tenant.is_empty() {
            return None;
        }
        let config: Config = serde_json::from_slice(bytes).ok()?;
        let mut matches = config.session_team_sync.into_iter().filter(|policy| {
            policy.gateway.trim_end_matches('/') == gateway.trim_end_matches('/') && policy.tenant_id == tenant
        });
        let policy = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        let mut roots = HashMap::new();
        let mut devices = std::collections::HashSet::new();
        for member in policy.members {
            if member.person_id.is_empty() || member.device_id.is_empty() || !devices.insert(member.device_id.clone()) {
                return None;
            }
            let root = Holder {
                id: member.device_id,
                keys: Keys {
                    encryption: key32(&member.encryption_key)?,
                    signing: Some(key32(&member.signing_key)?),
                },
            };
            if roots.insert(member.person_id, root).is_some() {
                return None;
            }
        }
        Some(Self { roots })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_one_local_policy_for_this_gateway_and_org_enables_team_sync() {
        let policy = json!({ "gateway": "https://gateway.test", "tenantId": "acme", "members": [] });
        let parse =
            |value: serde_json::Value| TeamPolicy::parse(value.to_string().as_bytes(), "https://gateway.test/", "acme");
        assert!(parse(json!({ "multiplayerSessions": true })).is_none());
        assert!(parse(json!({ "sessionTeamSync": [policy.clone()] })).is_some());
        assert!(parse(json!({ "sessionTeamSync": [policy.clone(), policy] })).is_none());
        assert!(
            TeamPolicy::parse(
                br#"{"sessionTeamSync":[{"gateway":"https://evil.test","tenantId":"acme","members":[]}]}"#,
                "https://gateway.test",
                "acme"
            )
            .is_none()
        );
    }

    #[test]
    fn member_admission_requires_exact_keys_and_rejects_ambiguous_roots() {
        let key = nexus_client::e2e::b64u(&[1; 32]);
        let member = json!({ "personId": "ada", "deviceId": "dev_ada", "encryptionKey": key, "signingKey": key });
        let parse = |members| {
            TeamPolicy::parse(
                json!({ "sessionTeamSync": [{ "gateway": "g", "tenantId": "t", "members": members }] })
                    .to_string()
                    .as_bytes(),
                "g",
                "t",
            )
        };
        let policy = parse(json!([member.clone()])).expect("local admission");
        assert_eq!(policy.roots["ada"].keys.encryption, [1; 32]);
        assert!(!policy.roots.contains_key("invented"));
        assert!(parse(json!([member.clone(), member])).is_none());
        assert!(
            parse(json!([{ "personId": "ada", "deviceId": "dev_ada", "encryptionKey": "short", "signingKey": key }]))
                .is_none()
        );
    }
}
