//! The settings that name each configured provider. They hold no secret.

use serde::{Deserialize, Serialize};

use crate::{KeyRef, KeyVendor};

/// One configured provider's stable id. It is never an index.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstanceId(String);

impl InstanceId {
    pub fn new(id: String) -> InstanceId {
        InstanceId(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The agent's own login (`subscription:claude:own`).
    pub fn subscription_own(agent: nexus_agents::Agent) -> InstanceId {
        InstanceId(format!("subscription:{}:own", agent.as_str()))
    }

    /// An extra account of the agent (`subscription:claude:<account>`).
    pub fn subscription_account(agent: nexus_agents::Agent, account: &str) -> InstanceId {
        InstanceId(format!("subscription:{}:{account}", agent.as_str()))
    }

    /// Nexus Cloud, which has one instance.
    pub fn nexus() -> InstanceId {
        InstanceId("nexus".to_string())
    }
}

/// Which providers the person configured. It holds no secret: a key is a [`KeyRef`] into the OS credential store. A
/// `default` that names an instance that is gone reads as unset.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettings {
    #[serde(default)]
    pub default: Option<InstanceId>,
    #[serde(default)]
    pub api_keys: Vec<ApiKeySetting>,
    #[serde(default)]
    pub servers: Vec<ServerSetting>,
}

/// One saved vendor key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeySetting {
    pub id: InstanceId,
    pub vendor: KeyVendor,
    pub key: KeyRef,
}

/// One model server that speaks the OpenAI API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSetting {
    pub id: InstanceId,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub key: Option<KeyRef>,
    #[serde(default)]
    pub models: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_agents::Agent;

    fn full() -> ProviderSettings {
        ProviderSettings {
            default: Some(InstanceId::new("k1".into())),
            api_keys: vec![ApiKeySetting {
                id: InstanceId::new("k1".into()),
                vendor: KeyVendor::Anthropic,
                key: KeyRef::new("provider-key/abc".into()),
            }],
            servers: vec![ServerSetting {
                id: InstanceId::new("s1".into()),
                name: "Local".into(),
                base_url: "http://localhost:1234/v1".into(),
                key: Some(KeyRef::new("provider-key/def".into())),
                models: vec!["llama".into()],
            }],
        }
    }

    #[test]
    fn instance_ids() {
        assert_eq!(
            InstanceId::subscription_own(Agent::Claude).as_str(),
            "subscription:claude:own"
        );
        assert_eq!(
            InstanceId::subscription_account(Agent::Claude, "a_1700000000000").as_str(),
            "subscription:claude:a_1700000000000"
        );
        assert_eq!(InstanceId::nexus().as_str(), "nexus");
    }

    #[test]
    fn settings_round_trip() {
        let settings = full();
        let json = serde_json::to_string(&settings).expect("serializes");
        assert_eq!(
            serde_json::from_str::<ProviderSettings>(&json).expect("reads"),
            settings
        );
    }

    #[test]
    fn settings_read_camel_case_keys() {
        let json = r#"{"default":"k1","apiKeys":[{"id":"k1","vendor":"openai","key":"provider-key/abc"}],
            "servers":[{"id":"s1","name":"Local","baseUrl":"http://localhost:1234/v1"}]}"#;
        let expected = ProviderSettings {
            default: Some(InstanceId::new("k1".into())),
            api_keys: vec![ApiKeySetting {
                id: InstanceId::new("k1".into()),
                vendor: KeyVendor::Openai,
                key: KeyRef::new("provider-key/abc".into()),
            }],
            servers: vec![ServerSetting {
                id: InstanceId::new("s1".into()),
                name: "Local".into(),
                base_url: "http://localhost:1234/v1".into(),
                key: None,
                models: vec![],
            }],
        };
        assert_eq!(serde_json::from_str::<ProviderSettings>(json).expect("reads"), expected);
    }

    #[test]
    fn empty_object_is_the_default() {
        assert_eq!(
            serde_json::from_str::<ProviderSettings>("{}").expect("reads"),
            ProviderSettings::default()
        );
    }
}
