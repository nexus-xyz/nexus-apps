use std::sync::Arc;

use super::KeySource;

/// The process-wide opt-in, read once at start. Sign-in state is not part of it; callers check the live scope per call.
#[derive(Clone, Default)]
pub enum LocalProvidersOptIn {
    #[default]
    Disabled,
    Enabled(LocalProviders),
}

#[derive(Clone)]
pub struct LocalProviders {
    pub keys: Arc<dyn KeySource>,
}

impl LocalProvidersOptIn {
    pub fn from_env(keys: impl FnOnce() -> Arc<dyn KeySource>) -> Self {
        if std::env::var("NEXUS_BYO_PROVIDERS")
            .is_ok_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "on" | "1" | "true"))
        {
            Self::Enabled(LocalProviders { keys: keys() })
        } else {
            Self::Disabled
        }
    }

    pub fn local_providers(&self) -> Option<&LocalProviders> {
        match self {
            Self::Disabled => None,
            Self::Enabled(local) => Some(local),
        }
    }

    pub fn enabled(&self) -> bool {
        self.local_providers().is_some()
    }
}
