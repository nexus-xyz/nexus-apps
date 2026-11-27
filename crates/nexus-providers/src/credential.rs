//! What one launch runs on, and the secrets it carries.

use serde::{Deserialize, Serialize};

use crate::{ProviderError, ProviderKind};

/// A secret value: wiped on drop, never printed or serialized. It has no `Clone`, so a copy is a deliberate `expose`.
pub struct Secret(zeroize::Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Secret {
        Secret(zeroize::Zeroizing::new(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// The account name of a key in the OS credential store (`provider-key/<uuid>`). It is never the key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyRef(String);

impl KeyRef {
    pub fn new(account: String) -> KeyRef {
        KeyRef(account)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The company whose API a key opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyVendor {
    Anthropic,
    Openai,
}

impl KeyVendor {
    /// Its id, the same text serde writes.
    pub fn as_str(self) -> &'static str {
        match self {
            KeyVendor::Anthropic => "anthropic",
            KeyVendor::Openai => "openai",
        }
    }

    /// Its name in messages.
    pub fn name(self) -> &'static str {
        match self {
            KeyVendor::Anthropic => "Anthropic",
            KeyVendor::Openai => "OpenAI",
        }
    }
}

/// Where a plan session reports its usage: counts only, never content.
#[derive(Debug)]
pub struct UsageSink {
    pub core_url: String,
    pub token: Secret,
    pub session_id: String,
}

/// What one launch runs on, owned for one spawn and dropped after the child starts.
#[derive(Debug)]
pub enum Credential {
    /// The agent's own login, or an extra account's folder. A `usage` of `None` reports nowhere.
    AgentLogin {
        account_dir: Option<std::path::PathBuf>,
        usage: Option<UsageSink>,
    },
    VendorKey {
        vendor: KeyVendor,
        secret: Secret,
    },
    Server {
        /// Identifies the local session's provider inside the selected agent.
        provider_id: String,
        base_url: String,
        secret: Option<Secret>,
    },
    Gateway {
        /// Without a trailing slash.
        core_url: String,
        token: Secret,
        session_id: String,
        team: Option<String>,
    },
}

impl Credential {
    /// The kind of provider this credential belongs to.
    pub fn kind(&self) -> ProviderKind {
        match self {
            Credential::AgentLogin { .. } => ProviderKind::CliSubscription,
            Credential::VendorKey { .. } => ProviderKind::ApiKey,
            Credential::Server { .. } => ProviderKind::OpenAiCompatible,
            Credential::Gateway { .. } => ProviderKind::NexusCloud,
        }
    }
}

/// The OS credential store, as the app implements it. A key is read at each spawn and held nowhere.
pub trait KeySource: Send + Sync {
    fn read(&self, key: &KeyRef) -> Result<Option<Secret>, ProviderError>;
    fn write(&self, key: &KeyRef, secret: Secret) -> Result<(), ProviderError>;
    fn delete(&self, key: &KeyRef) -> Result<(), ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "sk-test-123";

    fn secret() -> Secret {
        Secret::new(TEXT.into())
    }

    fn sink() -> UsageSink {
        UsageSink {
            core_url: "https://core.example".into(),
            token: secret(),
            session_id: "s1".into(),
        }
    }

    #[test]
    fn secret_debug_is_redacted_and_expose_reads() {
        assert_eq!(format!("{:?}", secret()), "Secret(<redacted>)");
        assert_eq!(secret().expose(), TEXT);
    }

    #[test]
    fn debug_never_shows_a_secret() {
        let all = [
            Credential::AgentLogin {
                account_dir: None,
                usage: Some(sink()),
            },
            Credential::VendorKey {
                vendor: KeyVendor::Openai,
                secret: secret(),
            },
            Credential::Server {
                provider_id: "test-provider".into(),
                base_url: "http://localhost:1234".into(),
                secret: Some(secret()),
            },
            Credential::Gateway {
                core_url: "https://core.example".into(),
                token: secret(),
                session_id: "s1".into(),
                team: None,
            },
        ];
        for credential in &all {
            let text = format!("{credential:?}");
            assert!(!text.contains(TEXT), "{text}");
            assert!(text.contains("<redacted>") || matches!(credential, Credential::AgentLogin { .. }));
        }
        assert!(!format!("{:?}", sink()).contains(TEXT));
        assert_eq!(all.iter().map(Credential::kind).collect::<Vec<_>>(), ProviderKind::ALL);
    }

    #[test]
    fn vendor_serde_is_its_id() {
        for (vendor, id, name) in [
            (KeyVendor::Anthropic, "anthropic", "Anthropic"),
            (KeyVendor::Openai, "openai", "OpenAI"),
        ] {
            assert_eq!(serde_json::to_value(vendor).expect("serializes"), serde_json::json!(id));
            assert_eq!(vendor.as_str(), id);
            assert_eq!(vendor.name(), name);
        }
    }
}
