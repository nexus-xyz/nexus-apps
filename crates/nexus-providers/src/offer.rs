//! A model an agent can run on one funding.

use crate::{Funding, Room};

/// A model id from Gateway's catalog (`anthropic/claude-opus-5.5`). It is built only where Gateway's answer is read, so
/// a string that is not from the catalog cannot stand for one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CatalogModelId(String);

impl CatalogModelId {
    /// The id as Gateway gave it, or `None` for an empty id.
    pub fn from_gateway(id: &str) -> Option<CatalogModelId> {
        (!id.is_empty()).then(|| CatalogModelId(id.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One model an agent can run, and who would pay for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOffer {
    pub agent: nexus_agents::Agent,
    /// The agent's own id for the model (`claude-opus-5-5`).
    pub agent_model: String,
    /// `None` when Gateway's catalog lacks the model.
    pub catalog: Option<CatalogModelId>,
    /// The display name that plan limits name (`Opus 5.5`).
    pub name: String,
    pub funding: Funding,
    /// `Unknown` unless the funding is a plan with a reading.
    pub room: Room,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_id_reads_only_a_non_empty_id() {
        assert_eq!(CatalogModelId::from_gateway(""), None);
        let id = CatalogModelId::from_gateway("anthropic/claude-opus-5.5").expect("an id");
        assert_eq!(id.as_str(), "anthropic/claude-opus-5.5");
    }
}
