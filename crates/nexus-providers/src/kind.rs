//! The four ways a session is reached and paid, and the plain facts of each.

use serde::{Deserialize, Serialize};

/// How a session is reached and paid. The ids are stored and sent, and never change once shipped. `subscription` and
/// `nexus` equal the session's `via` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "subscription")]
    CliSubscription,
    #[serde(rename = "api_key")]
    ApiKey,
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    #[serde(rename = "nexus")]
    NexusCloud,
}

/// What Nexus knows of one kind as plain values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindEntry {
    /// The mode's name as people see it.
    pub name: &'static str,
    /// A turn calls a Nexus host.
    pub needs_nexus_host: bool,
    /// A person must be signed in to Nexus.
    pub needs_nexus_account: bool,
    /// Settings may hold several instances of it.
    pub many: bool,
}

static CLI_SUBSCRIPTION: KindEntry = KindEntry {
    name: "BYO subscription",
    needs_nexus_host: false,
    needs_nexus_account: false,
    many: true,
};

static API_KEY: KindEntry = KindEntry {
    name: "BYO key",
    needs_nexus_host: false,
    needs_nexus_account: false,
    many: true,
};

static OPENAI_COMPATIBLE: KindEntry = KindEntry {
    name: "BYO model",
    needs_nexus_host: false,
    needs_nexus_account: false,
    many: true,
};

static NEXUS_CLOUD: KindEntry = KindEntry {
    name: "Nexus Cloud",
    needs_nexus_host: true,
    needs_nexus_account: true,
    many: false,
};

impl ProviderKind {
    /// Every kind, in the order Settings lists them.
    pub const ALL: [ProviderKind; 4] = [
        ProviderKind::CliSubscription,
        ProviderKind::ApiKey,
        ProviderKind::OpenAiCompatible,
        ProviderKind::NexusCloud,
    ];

    /// Its entry. The match lists every kind, so a new kind does not build until it has one.
    pub fn entry(self) -> &'static KindEntry {
        match self {
            ProviderKind::CliSubscription => &CLI_SUBSCRIPTION,
            ProviderKind::ApiKey => &API_KEY,
            ProviderKind::OpenAiCompatible => &OPENAI_COMPATIBLE,
            ProviderKind::NexusCloud => &NEXUS_CLOUD,
        }
    }

    /// Its id, the same text serde writes.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::CliSubscription => "subscription",
            ProviderKind::ApiKey => "api_key",
            ProviderKind::OpenAiCompatible => "openai_compatible",
            ProviderKind::NexusCloud => "nexus",
        }
    }

    /// The kind named `id`, or `None` for an id this build does not know.
    pub fn parse(id: &str) -> Option<ProviderKind> {
        ProviderKind::ALL.into_iter().find(|kind| kind.as_str() == id)
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each variant's place in [`ProviderKind::ALL`]. The match lists every variant, so a new one does not build here
    /// until it has a place.
    fn place(kind: ProviderKind) -> usize {
        match kind {
            ProviderKind::CliSubscription => 0,
            ProviderKind::ApiKey => 1,
            ProviderKind::OpenAiCompatible => 2,
            ProviderKind::NexusCloud => 3,
        }
    }

    #[test]
    fn all_lists_every_variant() {
        let places: Vec<usize> = ProviderKind::ALL.map(place).to_vec();
        assert_eq!(places, (0..ProviderKind::ALL.len()).collect::<Vec<_>>());
    }

    /// A kind's serde value, its id, its text and its parse are one string, and each reads back as the kind.
    #[test]
    fn serde_value_is_the_id_parse_reads() {
        for kind in ProviderKind::ALL {
            let id = kind.as_str();
            assert_eq!(serde_json::to_value(kind).expect("serializes"), serde_json::json!(id));
            assert_eq!(
                serde_json::from_value::<ProviderKind>(serde_json::json!(id)).expect("deserializes"),
                kind
            );
            assert_eq!(ProviderKind::parse(id), Some(kind));
            assert_eq!(kind.to_string(), id);
        }
        assert_eq!(
            ProviderKind::ALL.map(ProviderKind::as_str),
            ["subscription", "api_key", "openai_compatible", "nexus"]
        );
        assert_eq!(ProviderKind::parse("x"), None);
    }

    /// Each entry states every field in a literal of its own: no struct update, no `Default`. A new field then fails to
    /// build until each kind decides it.
    #[test]
    fn every_entry_states_every_field() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/kind.rs");
        let source = std::fs::read_to_string(path).expect("readable");
        let code = source.split("#[cfg(test)]").next().expect("code part");
        let marker = ": KindEntry = KindEntry {";
        let literals: Vec<&str> = code.split(marker).skip(1).collect();
        assert_eq!(literals.len(), 4);
        for literal in literals {
            let body = literal.split("};").next().expect("literal body");
            for field in ["name:", "needs_nexus_host:", "needs_nexus_account:", "many:"] {
                assert_eq!(
                    body.lines().filter(|line| line.trim().starts_with(field)).count(),
                    1,
                    "{field} in {body}"
                );
            }
            assert!(!body.contains(".."), "{body}");
            assert!(!body.contains("Default"), "{body}");
        }
        assert!(!code.contains("Default"));
    }

    #[test]
    fn entries_state_the_modes() {
        assert_eq!(ProviderKind::NexusCloud.entry().name, "Nexus Cloud");
        assert!(ProviderKind::NexusCloud.entry().needs_nexus_host);
        assert!(ProviderKind::ApiKey.entry().many);
        assert!(!ProviderKind::NexusCloud.entry().many);
    }
}
