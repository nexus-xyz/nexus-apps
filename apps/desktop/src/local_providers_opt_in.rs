use gpui_kit::{App, Global};
#[cfg(test)]
use nexus_workbench::provider::LocalProviders;
use nexus_workbench::provider::LocalProvidersOptIn;
use std::sync::Arc;

struct OptIn(LocalProvidersOptIn);
impl Global for OptIn {}

pub(crate) fn start(cx: &mut App) {
    install(cx, from_env());
}

pub(crate) fn install(cx: &mut App, mode: LocalProvidersOptIn) {
    cx.set_global(OptIn(mode));
}

pub(crate) fn get(cx: &App) -> &LocalProvidersOptIn {
    cx.try_global::<OptIn>()
        .map_or(&LocalProvidersOptIn::Disabled, |opt_in| &opt_in.0)
}

pub(crate) fn from_env() -> LocalProvidersOptIn {
    LocalProvidersOptIn::from_env(|| Arc::new(nexus_desktop_core::provider_keys::OsProviderKeys))
}

#[cfg(test)]
pub(crate) fn enabled_for_test() -> LocalProvidersOptIn {
    LocalProvidersOptIn::Enabled(LocalProviders {
        keys: Arc::new(nexus_desktop_core::provider_keys::OsProviderKeys),
    })
}
