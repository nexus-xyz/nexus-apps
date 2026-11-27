//! The app-wide local settings handle. All file and credential work runs off the UI thread.

use std::path::Path;
use std::sync::Arc;

use gpui_kit::*;
use nexus_desktop_core::provider_settings::{ExpectedProvider, ProviderSettingsStore};

use super::Saved;
use nexus_providers::{
    ApiKeySetting, InstanceId, KeySource, KeyVendor, ProviderError, ProviderSettings, Secret, ServerSetting,
};

type Snapshot = Result<(ProviderSettings, bool), ProviderError>;

pub(super) struct Store {
    pub(super) file: Arc<ProviderSettingsStore>,
    pub(super) keys: Arc<dyn KeySource>,
    pub(super) snapshot: Option<Snapshot>,
    pub(super) saving: bool,
    pub(super) error: Option<ProviderError>,
    refresh_after_save: bool,
    refresh: Option<Task<()>>,
}

pub(super) struct LocalStore(pub(super) Entity<Store>);
impl Global for LocalStore {}

pub(super) fn start(cx: &mut App) {
    let Some(local) = crate::local_providers_opt_in::get(cx).local_providers() else {
        return;
    };
    let keys = local.keys.clone();
    if let Some(dir) = nexus_desktop_core::settings::config_dir() {
        let store = cx.new(|_| Store::new(&dir, keys));
        store.update(cx, Store::startup_cleanup);
        cx.set_global(LocalStore(store));
    }
}

pub(super) enum Edit {
    AddKey(KeyVendor, Secret),
    AddServer {
        name: String,
        address: String,
        models: Vec<String>,
        key: Option<Secret>,
    },
    UpdateServer {
        expected: ServerSetting,
        name: String,
        address: String,
        models: Vec<String>,
    },
    RotateKey(ApiKeySetting, Secret),
    RotateServer(ServerSetting, Secret),
    ClearServer(ServerSetting),
    Remove(Saved),
}

impl Edit {
    pub(super) fn apply(
        self,
        file: &ProviderSettingsStore,
        keys: &dyn KeySource,
    ) -> Result<Option<InstanceId>, ProviderError> {
        match self {
            Self::AddKey(vendor, secret) => file.add_api_key(keys, vendor, secret).map(Some),
            Self::AddServer {
                name,
                address,
                models,
                key,
            } => file
                .add_server(name, &address, models, key.map(|secret| (keys, secret)))
                .map(Some),
            Self::UpdateServer {
                expected,
                name,
                address,
                models,
            } => file
                .edit_server_if_current(&expected, name, &address, models)
                .map(|()| None),
            Self::RotateKey(expected, secret) => file.rotate_api_key_if_current(keys, &expected, secret).map(|()| None),
            Self::RotateServer(expected, secret) => file
                .rotate_server_key_if_current(keys, &expected, secret)
                .map(|()| None),
            Self::ClearServer(expected) => file.clear_server_key_if_current(keys, &expected).map(|()| None),
            Self::Remove(saved) => file
                .remove_if_current(
                    keys,
                    match &saved {
                        Saved::Key(key) => ExpectedProvider::Key(key),
                        Saved::Server(server) => ExpectedProvider::Server(server),
                    },
                )
                .map(|()| None),
        }
    }
}

impl Store {
    pub(super) fn new(dir: &Path, keys: Arc<dyn KeySource>) -> Self {
        Self {
            file: Arc::new(ProviderSettingsStore::in_config(dir)),
            keys,
            snapshot: None,
            saving: false,
            error: None,
            refresh_after_save: false,
            refresh: None,
        }
    }

    pub(super) fn startup_cleanup(&mut self, cx: &mut Context<Self>) {
        let file = self.file.clone();
        let keys = self.keys.clone();
        let task = cx
            .background_executor()
            .spawn(async move { file.retry_key_cleanup(keys.as_ref()) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                eprintln!("nexus-desktop-gpui: local provider key cleanup: {error}");
            }
            this.update(cx, Store::refresh)
        })
        .detach_and_log_err(cx);
    }

    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            self.refresh_after_save = true;
            return;
        }
        let file = self.file.clone();
        let read = cx
            .background_executor()
            .spawn(async move { file.read_with_cleanup_status() });
        self.refresh = Some(cx.spawn(async move |this, cx| {
            let read_result = read.await;
            if let Err(error) = this.update(cx, |this, cx| {
                this.snapshot = Some(read_result);
                cx.notify();
            }) {
                eprintln!("nexus-desktop-gpui: local provider settings closed: {error}");
            }
        }));
    }

    pub(super) fn edit(
        &mut self,
        edit: Edit,
        cx: &mut Context<Self>,
    ) -> futures::channel::oneshot::Receiver<Result<Option<InstanceId>, ProviderError>> {
        let (finished, result) = futures::channel::oneshot::channel();
        if self.saving {
            return result;
        }
        self.refresh = None;
        self.saving = true;
        self.error = None;
        cx.notify();
        let file = self.file.clone();
        let keys = self.keys.clone();
        let save = cx.background_executor().spawn(async move {
            let result = edit.apply(&file, keys.as_ref());
            (result, file.read_with_cleanup_status())
        });
        cx.spawn(async move |this, cx| {
            let (outcome, snapshot) = save.await;
            this.update(cx, |this, cx| {
                this.saving = false;
                this.error = outcome.as_ref().err().cloned();
                this.snapshot = Some(snapshot);
                if std::mem::take(&mut this.refresh_after_save) {
                    this.refresh(cx);
                }
                cx.notify();
            })?;
            let _ = finished.send(outcome);
            Ok::<(), anyhow::Error>(())
        })
        .detach_and_log_err(cx);
        result
    }
}
