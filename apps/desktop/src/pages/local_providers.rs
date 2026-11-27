//! Settings for credentials and model servers saved on this computer.

mod form;
mod store;
mod view;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;
use nexus_client::local_agents::{LocalAgent, Login};
use nexus_providers::{
    ApiKeySetting, Health, InstanceId, KeySource, KeyVendor, ProviderError, ProviderSettings, ServerSetting,
};
use nexus_workbench::event::SessionAgent;
use nexus_workbench::provider::{HealthContext, MachineAgent, MachineFacts, OpenAiCompatibleProvider, ProviderSet};

use form::Form;
use store::{LocalStore, Store};

pub(crate) fn start(cx: &mut App) {
    store::start(cx);
}

pub(crate) fn build(
    _device: super::DeviceCtx,
    agents: Option<&[LocalAgent]>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    let store = cx.try_global::<LocalStore>().map(|store| store.0.clone());
    cx.new(|cx| Editor::new(store, agents.map(machine_facts), window, cx))
        .into()
}

pub(crate) enum SetupForm {
    Key(KeyVendor),
    Server,
}

pub(crate) enum SetupEvent {
    Saved(InstanceId),
    Cancelled,
}

pub(crate) fn setup(form: SetupForm, window: &mut Window, cx: &mut App) -> Entity<Editor> {
    let store = cx.try_global::<LocalStore>().map(|store| store.0.clone());
    cx.new(|cx| {
        let mut editor = Editor::new(store, None, window, cx);
        editor.setup = true;
        let action = match form {
            SetupForm::Key(vendor) => form::Action::AddKey(vendor),
            SetupForm::Server => form::Action::AddServer,
        };
        editor.open_form(action, window, cx);
        editor
    })
}

pub(crate) fn read_for_picker(cx: &App) -> Task<Result<ProviderSettings, String>> {
    let file = cx.try_global::<LocalStore>().map(|store| store.0.read(cx).file.clone());
    cx.background_executor().spawn(async move {
        file.ok_or("Nexus could not open its local configuration folder.")?
            .read()
            .map_err(|error| error.to_string())
    })
}

pub(crate) fn validate_for_launch(
    id: InstanceId,
    agent: SessionAgent,
    model: Option<String>,
    still_local: impl Fn() -> bool + Send + 'static,
    cx: &App,
) -> Task<Result<(), String>> {
    let source = cx.try_global::<LocalStore>().map(|store| {
        let store = store.0.read(cx);
        (store.file.clone(), store.keys.clone())
    });
    cx.background_executor().spawn(async move {
        let (file, keys) = source.ok_or("Nexus could not open its local configuration folder.")?;
        let settings = file.read().map_err(|error| error.to_string())?;
        let providers = ProviderSet::from_settings(&settings, &MachineFacts::default(), None, true);
        let provider = providers
            .get(&id)
            .filter(|provider| provider.agents().contains(&agent))
            .ok_or("The saved provider is unavailable for this agent.")?;
        let key = match Saved::from_settings(&settings, provider.id()) {
            Some(Saved::Key(row)) => Some(row.key),
            Some(Saved::Server(row)) => {
                if model.as_ref().is_none_or(|model| !row.models.contains(model)) {
                    return Err("The saved server model changed.".into());
                }
                row.key
            }
            None => None,
        };
        if !still_local() {
            return Err("The account changed before the session could start.".into());
        }
        if let Some(key) = key
            && keys.read(&key).map_err(|error| error.to_string())?.is_none()
        {
            return Err("The provider's API key is missing.".into());
        }
        Ok(())
    })
}

#[cfg(test)]
pub(crate) fn install_test_store(path: &std::path::Path, keys: Arc<dyn KeySource>, cx: &mut App) {
    let store = cx.new(|_| Store::new(path, keys));
    cx.set_global(LocalStore(store));
}

#[cfg(test)]
pub(crate) fn submit_test_setup(
    editor: &Entity<Editor>,
    server: Option<(&str, &str)>,
    key: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) {
    editor.update(cx, |editor, cx| editor.submit_test_setup(server, key, window, cx));
}

pub(crate) fn cancel_edit(view: &AnyView, window: &mut Window, cx: &mut App) -> bool {
    let Ok(editor) = view.clone().downcast::<Editor>() else {
        return false;
    };
    editor.update(cx, |editor, cx| {
        if editor.form.is_none() {
            return false;
        }
        editor.cancel_form(window, cx);
        true
    })
}

fn machine_facts(agents: &[LocalAgent]) -> MachineFacts {
    MachineFacts {
        agents: agents
            .iter()
            .filter_map(|local| {
                Some(MachineAgent {
                    agent: SessionAgent::parse(&local.agent)?,
                    installed: local.installed,
                    login: local.login.unwrap_or(Login::Unknown),
                    accounts: vec![],
                })
            })
            .collect(),
    }
}

#[derive(Clone, PartialEq)]
enum Saved {
    Key(ApiKeySetting),
    Server(ServerSetting),
}

impl Saved {
    fn id(&self) -> &InstanceId {
        match self {
            Self::Key(key) => &key.id,
            Self::Server(server) => &server.id,
        }
    }

    fn from_settings(settings: &ProviderSettings, id: &InstanceId) -> Option<Self> {
        settings
            .api_keys
            .iter()
            .find(|key| &key.id == id)
            .cloned()
            .map(Self::Key)
            .or_else(|| {
                settings
                    .servers
                    .iter()
                    .find(|server| &server.id == id)
                    .cloned()
                    .map(Self::Server)
            })
    }

    async fn check(&self, keys: &Arc<dyn KeySource>) -> Result<Vec<String>, ProviderError> {
        let context = HealthContext {
            machine: &MachineFacts::default(),
            keys: Some(keys),
        };
        match self {
            Self::Key(key) => match nexus_workbench::provider::Provider::probe(
                &nexus_workbench::provider::ApiKeyProvider::new(key.clone()),
                &context,
            )
            .await
            {
                Health::Ready => Ok(vec![]),
                Health::Setup(error) => Err(error),
                Health::Unreachable { endpoint } => Err(ProviderError::Unreachable { endpoint }),
            },
            Self::Server(server) => OpenAiCompatibleProvider::new(server.clone()).check(&context).await,
        }
    }
}

struct Row {
    saved: Saved,
    name: String,
    setup: Option<String>,
    detail: String,
}

enum Check {
    Running(u64),
    Finished {
        result: Result<Vec<String>, ProviderError>,
        missing_models: bool,
    },
}

pub(crate) struct Editor {
    setup: bool,
    store: Option<Entity<Store>>,
    machine: Option<MachineFacts>,
    rows: Vec<Row>,
    checks: HashMap<InstanceId, Check>,
    next_check: u64,
    form: Option<Form>,
    draft_changed: bool,
    focus: FocusHandle,
    page: usize,
    _subscription: Option<Subscription>,
}

impl EventEmitter<SetupEvent> for Editor {}

impl Editor {
    pub(crate) fn setup_save_pending(&self) -> bool {
        self.setup && self.form.as_ref().is_some_and(Form::saving)
    }

    fn new(
        store: Option<Entity<Store>>,
        machine: Option<MachineFacts>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = store
            .as_ref()
            .map(|store| cx.observe_in(store, window, |this, _, window, cx| this.sync(window, cx)));
        if let Some(store) = &store {
            store.update(cx, Store::refresh);
            if machine.is_none() {
                let agents = cx.background_executor().spawn(async {
                    MachineFacts {
                        agents: [SessionAgent::Claude, SessionAgent::Codex, SessionAgent::Opencode]
                            .into_iter()
                            .map(|agent| MachineAgent {
                                agent,
                                installed: crate::code_run::find(agent).is_some(),
                                login: Login::Unknown,
                                accounts: vec![],
                            })
                            .collect(),
                    }
                });
                cx.spawn_in(window, async move |this, cx| {
                    let facts = agents.await;
                    this.update_in(cx, |this, window, cx| {
                        this.machine = Some(facts);
                        this.sync(window, cx);
                    })
                })
                .detach_and_log_err(cx);
            }
        }
        let owner = window.window_handle();
        cx.on_release(move |editor, cx| {
            let Some(form) = editor.form.take() else { return };
            // InputState needs a live window to reset its value and undo history. If the
            // owner went away, use another window. With no windows, the input is released.
            let windows = cx.windows();
            let window = windows
                .iter()
                .find(|window| window.window_id() == owner.window_id())
                .or(windows.first());
            if let Some(window) = window {
                let _ = window.update(cx, |_, window, cx| form.clear(window, cx));
            }
        })
        .detach();
        let mut editor = Self {
            setup: false,
            store,
            machine,
            rows: vec![],
            checks: HashMap::new(),
            next_check: 0,
            form: None,
            draft_changed: false,
            focus: cx.focus_handle(),
            page: 0,
            _subscription: subscription,
        };
        editor.sync(window, cx);
        editor
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else { return };
        let snapshot = store
            .read(cx)
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.as_ref().ok());
        let draft_changed = snapshot.is_some_and(|(settings, _)| {
            self.form
                .as_ref()
                .filter(|form| !form.saving())
                .and_then(Form::saved)
                .is_some_and(|original| Saved::from_settings(settings, original.id()).as_ref() != Some(&original))
        });
        let previous = std::mem::take(&mut self.rows);
        self.rows = match snapshot {
            Some((settings, _)) => {
                let machine = self.machine.as_ref().cloned().unwrap_or_default();
                let providers = ProviderSet::from_settings(settings, &machine, None, true);
                let context = HealthContext {
                    machine: &machine,
                    keys: None,
                };
                settings
                    .api_keys
                    .iter()
                    .cloned()
                    .map(Saved::Key)
                    .chain(settings.servers.iter().cloned().map(Saved::Server))
                    .filter_map(|saved| {
                        let provider = providers.get(saved.id())?;
                        let setup = match provider.local_health(&context) {
                            Health::Setup(ProviderError::AgentMissing(_)) if self.machine.is_none() => None,
                            Health::Setup(error) => Some(error.to_string()),
                            _ => None,
                        };
                        let detail = match &saved {
                            Saved::Key(_) => "API key saved".into(),
                            Saved::Server(server) if server.key.is_some() => {
                                format!("{} · API key saved", server.base_url)
                            }
                            Saved::Server(server) => server.base_url.clone(),
                        };
                        Some(Row {
                            name: provider.name(),
                            saved,
                            setup,
                            detail,
                        })
                    })
                    .collect()
            }
            None => vec![],
        };
        let previous: HashMap<_, _> = previous.iter().map(|row| (row.saved.id(), &row.saved)).collect();
        let current: HashMap<_, _> = self.rows.iter().map(|row| (row.saved.id(), &row.saved)).collect();
        self.checks
            .retain(|id, _| current.get(id).is_some_and(|row| previous.get(id) == Some(row)));
        if draft_changed {
            self.close_form(window, cx);
            self.draft_changed = true;
        }
        self.page = self.page.min(self.rows.len().saturating_sub(1) / view::PAGE_SIZE);
        cx.notify();
    }

    fn check(&mut self, saved: Saved, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let store = store.read(cx);
        let file = store.file.clone();
        let keys = store.keys.clone();
        let id = saved.id().clone();
        self.next_check += 1;
        let request = self.next_check;
        self.checks.insert(id.clone(), Check::Running(request));
        cx.notify();
        let saved_models = match &saved {
            Saved::Server(server) => server.models.clone(),
            Saved::Key(_) => vec![],
        };
        let check = crate::code_run::runtime().spawn(check_saved(file, keys, saved));
        cx.spawn(async move |this, cx| {
            let result = check.await.unwrap_or_else(|_| {
                Some(Err(ProviderError::Refused(
                    "The check could not finish. Try again.".into(),
                )))
            });
            this.update(cx, |this, cx| this.finish_check(id, request, result, &saved_models, cx))
        })
        .detach_and_log_err(cx);
    }
    fn finish_check(
        &mut self,
        id: InstanceId,
        request: u64,
        result: Option<Result<Vec<String>, ProviderError>>,
        saved_models: &[String],
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.checks.get(&id), Some(Check::Running(n)) if *n == request) {
            return;
        }
        let refresh = result.is_none() || matches!(result, Some(Err(ProviderError::Unreachable { .. })));
        if let Some(result) = result {
            let missing_models = result
                .as_ref()
                .is_ok_and(|offers| saved_models.iter().any(|id| form::absent_from_list(offers, id)));
            self.checks.insert(id, Check::Finished { result, missing_models });
        } else {
            self.checks.remove(&id);
        }
        // A deadline can end while the keychain still waits. Refresh the saved row without
        // delaying the UI, so an external edit or removal also discards the expired check.
        if refresh && let Some(store) = &self.store {
            store.update(cx, Store::refresh);
        }
        cx.notify();
    }
}

const CHECK_DEADLINE: Duration = Duration::from_secs(10);

async fn check_saved(
    file: Arc<nexus_desktop_core::provider_settings::ProviderSettingsStore>,
    keys: Arc<dyn KeySource>,
    saved: Saved,
) -> Option<Result<Vec<String>, ProviderError>> {
    check_saved_with_deadline(file, keys, saved, CHECK_DEADLINE).await
}

async fn check_saved_with_deadline(
    file: Arc<nexus_desktop_core::provider_settings::ProviderSettingsStore>,
    keys: Arc<dyn KeySource>,
    saved: Saved,
    deadline: Duration,
) -> Option<Result<Vec<String>, ProviderError>> {
    let endpoint = match &saved {
        Saved::Key(key) => key.vendor.name().to_owned(),
        Saved::Server(server) => server.base_url.clone(),
    };
    tokio::time::timeout(deadline, check_current(file, keys, saved))
        .await
        .unwrap_or_else(|_| Some(Err(ProviderError::Unreachable { endpoint })))
}

// Use the credential and address shown on the row. An external edit or a second window can
// replace them while the endpoint answers.
async fn check_current(
    file: Arc<nexus_desktop_core::provider_settings::ProviderSettingsStore>,
    keys: Arc<dyn KeySource>,
    saved: Saved,
) -> Option<Result<Vec<String>, ProviderError>> {
    let read = |file: Arc<nexus_desktop_core::provider_settings::ProviderSettingsStore>| {
        let id = saved.id().clone();
        tokio::task::spawn_blocking(move || file.read().map(|settings| Saved::from_settings(&settings, &id)))
    };
    match read(file.clone()).await {
        Ok(Ok(Some(current))) if current == saved => {}
        Ok(Err(error)) => return Some(Err(error)),
        _ => return None,
    }
    let result = saved.check(&keys).await;
    match read(file).await {
        Ok(Ok(Some(current))) if current == saved => Some(result),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn fixture_store(dir: &std::path::Path, cx: &mut App) {
    let store = cx.new(|_| Store::new(dir, Arc::new(tests::Keys::default())));
    cx.set_global(LocalStore(store));
}

#[cfg(test)]
mod tests;
