//! The provider layer's shared values. A provider is how a coding agent's session is reached and paid: the person's
//! plan, their own key, a model server, or Nexus Cloud.
//!
//! This crate holds plain values only: the kinds, how a model is funded, a model offer, a credential, the settings
//! that name each provider and the errors. It does no I/O. The app reads the credential store and the network.

mod credential;
mod error;
mod funding;
mod kind;
mod offer;
mod settings;

pub use credential::{Credential, KeyRef, KeySource, KeyVendor, Secret, UsageSink};
pub use error::{Health, ProviderError};
pub use funding::{AccountHandle, Funding, Room};
pub use kind::{KindEntry, ProviderKind};
pub use offer::{CatalogModelId, ModelOffer};
pub use settings::{ApiKeySetting, InstanceId, ProviderSettings, ServerSetting};
