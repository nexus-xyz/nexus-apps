//! A short-lived draft. Credential inputs are cleared before their entities are released.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_providers::{KeyVendor, Secret, ServerSetting};

use super::store::Edit;
use super::{Check, Editor, Saved};
use crate::pages::ui;
use crate::palette::Palette;

#[derive(Clone)]
pub(super) enum Action {
    AddKey(KeyVendor),
    AddServer,
    EditServer(ServerSetting),
    Rotate(Saved),
    Clear(ServerSetting),
    Remove(Saved, String),
}

impl Action {
    fn title(&self) -> String {
        match self {
            Self::AddKey(vendor) => format!("Add {} API key", vendor.name()),
            Self::AddServer => "Add model server".into(),
            Self::EditServer(_) => "Edit model server".into(),
            Self::Rotate(Saved::Server(server)) if server.key.is_none() => "Add server key".into(),
            Self::Rotate(_) => "Replace API key".into(),
            Self::Clear(_) => "Clear server key".into(),
            Self::Remove(_, _) => "Remove provider".into(),
        }
    }

    fn submit_label(&self) -> &'static str {
        match self {
            Self::Remove(_, _) => "Remove",
            Self::Clear(_) => "Clear key",
            _ => "Save",
        }
    }

    fn saved(&self) -> Option<Saved> {
        match self {
            Self::AddKey(_) | Self::AddServer => None,
            Self::EditServer(server) | Self::Clear(server) => Some(Saved::Server(server.clone())),
            Self::Rotate(saved) | Self::Remove(saved, _) => Some(saved.clone()),
        }
    }
}

struct ServerFields {
    name: Entity<InputState>,
    address: Entity<InputState>,
    fixed_address: Option<String>,
    model: Entity<InputState>,
    models: Vec<String>,
    offers: Vec<String>,
    checked_address: Option<String>,
    offers_current: bool,
    filtered_offers: Vec<usize>,
    unknown_model: bool,
    _model_events: Subscription,
    _address_events: Subscription,
}

impl ServerFields {
    fn new(saved: Option<&ServerSetting>, offers: Vec<String>, window: &mut Window, cx: &mut Context<Editor>) -> Self {
        let name = cx
            .new(|cx| InputState::new(window, cx).default_value(saved.map_or("", |row| row.name.as_str()).to_owned()));
        let address = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("http://localhost:11434/v1")
                .default_value(saved.map_or("", |row| row.base_url.as_str()).to_owned())
        });
        let model = cx.new(|cx| InputState::new(window, cx).placeholder("Model ID"));
        let model_events = cx.subscribe_in(
            &model,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let typed = input.read(cx).value();
                    if let Some(fields) = this
                        .form
                        .as_mut()
                        .filter(|form| !form.saving)
                        .and_then(|form| form.server.as_mut())
                    {
                        fields.update_offers(typed.trim());
                        cx.notify();
                    }
                }
                InputEvent::PressEnter { .. } => this.add_typed_model(window, cx),
                _ => {}
            },
        );
        let address_events = cx.subscribe_in(&address, window, |this, input, event: &InputEvent, _, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            if let Some(fields) = this
                .form
                .as_mut()
                .filter(|form| !form.saving)
                .and_then(|form| form.server.as_mut())
            {
                let address = input.read(cx).value();
                fields.offers_current = matches_checked_address(fields.checked_address.as_deref(), &address);
                let typed = fields.model.read(cx).value();
                fields.update_offers(typed.trim());
                cx.notify();
            }
        });
        let mut fields = Self {
            name,
            address,
            fixed_address: saved.filter(|row| row.key.is_some()).map(|row| row.base_url.clone()),
            model,
            models: saved.map_or_else(Vec::new, |row| row.models.clone()),
            checked_address: saved.and_then(|row| nexus_workbench::provider::server_base_url(&row.base_url)),
            offers_current: saved.is_some(),
            offers,
            filtered_offers: vec![],
            unknown_model: false,
            _model_events: model_events,
            _address_events: address_events,
        };
        fields.update_offers("");
        fields
    }

    fn update_offers(&mut self, typed: &str) {
        if !self.offers_current {
            self.unknown_model = false;
            self.filtered_offers.clear();
            return;
        }
        self.unknown_model = !typed.is_empty() && absent_from_list(&self.offers, typed);
        self.filtered_offers = self
            .offers
            .iter()
            .enumerate()
            .filter(|(_, id)| id.contains(typed) && !self.models.contains(id))
            .map(|(index, _)| index)
            .collect();
    }

    fn add_model(&mut self, model: String, window: &mut Window, cx: &mut Context<Editor>) {
        if !model.is_empty() && !self.models.contains(&model) {
            self.models.push(model);
        }
        self.model.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.update_offers("");
    }
}

pub(super) fn matches_checked_address(checked: Option<&str>, draft: &str) -> bool {
    checked.is_some_and(|checked| nexus_workbench::provider::server_base_url(draft).as_deref() == Some(checked))
}

pub(super) fn absent_from_list(offers: &[String], model: &str) -> bool {
    !offers.is_empty() && offers.binary_search_by(|id| id.as_str().cmp(model)).is_err()
}

pub(super) struct Form {
    action: Action,
    server: Option<ServerFields>,
    secret: Option<Entity<InputState>>,
    error: Option<&'static str>,
    saving: bool,
}

impl Form {
    fn new(action: Action, offers: Vec<String>, window: &mut Window, cx: &mut Context<Editor>) -> Self {
        let server = match &action {
            Action::AddServer => Some(ServerFields::new(None, vec![], window, cx)),
            Action::EditServer(server) => Some(ServerFields::new(Some(server), offers, window, cx)),
            _ => None,
        };
        let secret = matches!(action, Action::AddKey(_) | Action::AddServer | Action::Rotate(_))
            .then(|| cx.new(|cx| InputState::new(window, cx).masked(true)));
        let first = server.as_ref().map(|fields| &fields.name).or(secret.as_ref());
        if let Some(input) = first {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        Self {
            action,
            server,
            secret,
            error: None,
            saving: false,
        }
    }

    fn edit(&self, cx: &App) -> Result<Edit, &'static str> {
        let secret = self
            .secret
            .as_ref()
            .map(|input| input.read(cx).value())
            .and_then(|value| {
                let value = value.trim();
                (!value.is_empty()).then(|| Secret::new(value.to_owned()))
            });
        match &self.action {
            Action::AddKey(vendor) => Ok(Edit::AddKey(*vendor, secret.ok_or("Enter an API key.")?)),
            Action::Rotate(Saved::Key(key)) => Ok(Edit::RotateKey(key.clone(), secret.ok_or("Enter an API key.")?)),
            Action::Rotate(Saved::Server(server)) => {
                Ok(Edit::RotateServer(server.clone(), secret.ok_or("Enter an API key.")?))
            }
            Action::Clear(server) => Ok(Edit::ClearServer(server.clone())),
            Action::Remove(saved, _) => Ok(Edit::Remove(saved.clone())),
            Action::AddServer | Action::EditServer(_) => {
                let fields = self.server.as_ref().expect("server form");
                let name = fields.name.read(cx).value().trim().to_owned();
                if name.is_empty() {
                    return Err("Enter a server name.");
                }
                let address = fields
                    .fixed_address
                    .clone()
                    .unwrap_or_else(|| fields.address.read(cx).value().trim().to_owned());
                if address.is_empty() {
                    return Err("Enter a server address.");
                }
                let typed = fields.model.read(cx).value().trim().to_owned();
                let mut models = fields.models.clone();
                if !typed.is_empty() && !models.contains(&typed) {
                    models.push(typed);
                }
                match &self.action {
                    Action::EditServer(server) => Ok(Edit::UpdateServer {
                        expected: server.clone(),
                        name,
                        address,
                        models,
                    }),
                    _ => Ok(Edit::AddServer {
                        name,
                        address,
                        models,
                        key: secret,
                    }),
                }
            }
        }
    }

    pub(super) fn saved(&self) -> Option<Saved> {
        self.action.saved()
    }

    pub(super) fn saving(&self) -> bool {
        self.saving
    }

    pub(super) fn secret_input(&self) -> Option<&Entity<InputState>> {
        self.secret.as_ref()
    }

    #[cfg(test)]
    pub(super) fn server_inputs(&self) -> (Entity<InputState>, Entity<InputState>, Entity<InputState>) {
        let fields = self.server.as_ref().expect("server draft");
        (fields.name.clone(), fields.address.clone(), fields.model.clone())
    }

    pub(super) fn clear(&self, window: &mut Window, cx: &mut App) {
        if let Some(secret) = &self.secret {
            secret.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    pub(super) fn render(&self, setup: bool, palette: Palette, cx: &mut Context<Editor>) -> Div {
        let detail = match &self.action {
            Action::Rotate(Saved::Server(server)) => Some(server.base_url.clone()),
            Action::Clear(server) => Some(format!("Clear the key for {}? The server stays saved.", server.name)),
            Action::Remove(_, name) => Some(format!("Remove {name} from this computer?")),
            _ => None,
        };
        let secret_label = if matches!(self.action, Action::AddServer) {
            "API key (optional)"
        } else {
            "API key"
        };
        let title = self.action.title();
        let heading = if setup {
            div()
                .text_size(px(28.))
                .line_height(px(34.))
                .text_center()
                .text_color(palette.strong)
                .child(title)
        } else {
            ui::section_head(title, palette)
        };
        v_flex()
            .gap(px(12.))
            .child(heading)
            .children(detail.map(|text| ui::note(text, palette)))
            .children(
                self.server
                    .as_ref()
                    .map(|fields| fields.render(self.saving, palette, cx)),
            )
            .children(
                self.secret_input()
                    .map(|key| field(secret_label, key, self.saving, palette)),
            )
            .children(self.error.map(|error| ui::error_line(error, palette)))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(
                        ui::button("cancel-provider-edit", "Cancel", palette).when(!self.saving, |button| {
                            button.on_click(cx.listener(|this, _, window, cx| this.cancel_form(window, cx)))
                        }),
                    )
                    .child(if self.saving {
                        ui::disabled_button("save-provider-edit", "Saving…", palette)
                    } else {
                        ui::primary_button("save-provider-edit", self.action.submit_label(), palette)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                    }),
            )
    }
}

fn field(label: &'static str, input: &Entity<InputState>, disabled: bool, palette: Palette) -> Div {
    v_flex()
        .min_w_0()
        .gap(px(4.))
        .child(ui::label(label, palette))
        .child(Input::new(input).disabled(disabled).w_full())
}

impl ServerFields {
    fn render(&self, saving: bool, palette: Palette, cx: &mut Context<Editor>) -> Div {
        let selected = self.models.iter().enumerate().map(|(index, id)| {
            let label = if self.offers_current && absent_from_list(&self.offers, id) {
                format!("Remove {id} (missing from server list)")
            } else {
                format!("Remove {id}")
            };
            if saving {
                return ui::disabled_button(("remove-model", index), label, palette)
                    .max_w_full()
                    .truncate();
            }
            ui::button(("remove-model", index), label, palette)
                .max_w_full()
                .truncate()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(fields) = this
                        .form
                        .as_mut()
                        .filter(|form| !form.saving)
                        .and_then(|form| form.server.as_mut())
                    {
                        fields.models.remove(index);
                        let typed = fields.model.read(cx).value();
                        fields.update_offers(typed.trim());
                        cx.notify();
                    }
                }))
        });
        let count = if saving { 0 } else { self.filtered_offers.len() };
        let suggestions = (count > 0).then(|| {
            uniform_list(
                "server-model-offers",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    let Some(fields) = this.form.as_ref().and_then(|form| form.server.as_ref()) else {
                        return vec![];
                    };
                    range
                        .filter_map(|index| {
                            let id = fields.offers.get(*fields.filtered_offers.get(index)?)?.clone();
                            Some(
                                ui::ghost_button(("select-model", index), id.clone(), palette)
                                    .w_full()
                                    .justify_start()
                                    .truncate()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.add_model(id.clone(), window, cx);
                                    })),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .h(px(ui::CONTROL_HEIGHT * count.min(5) as f32))
        });
        v_flex()
            .gap(px(12.))
            .child(field("Name", &self.name, saving, palette))
            .child(if let Some(address) = &self.fixed_address {
                v_flex()
                    .gap(px(4.))
                    .child(ui::label("Server address", palette))
                    .child(ui::note(address.clone(), palette))
                    .child(ui::note("Clear the saved key to change this address.", palette))
            } else {
                field("Server address", &self.address, saving, palette)
            })
            .child(
                v_flex()
                    .gap(px(6.))
                    .child(ui::label("Models", palette))
                    .child(h_flex().flex_wrap().gap(px(6.)).children(selected))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&self.model).disabled(saving).w_full()),
                            )
                            .child(if saving {
                                ui::disabled_button("add-model", "Add ID", palette)
                            } else {
                                ui::button("add-model", "Add ID", palette)
                                    .on_click(cx.listener(|this, _, window, cx| this.add_typed_model(window, cx)))
                            }),
                    )
                    .when(self.unknown_model, |body| {
                        body.child(ui::error_line("The server did not list this model ID.", palette))
                    })
                    .children(suggestions),
            )
    }
}

impl Editor {
    pub(super) fn open_form(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.as_ref().is_some_and(Form::saving)
            || (!self.setup && self.store.as_ref().is_none_or(|store| store.read(cx).saving))
        {
            return;
        }
        let mut offers = vec![];
        if let Some(saved) = action.saved() {
            match self.checks.get(saved.id()) {
                Some(Check::Finished { result: Ok(models), .. }) => offers = models.clone(),
                Some(Check::Running(_)) => {
                    self.checks.remove(saved.id());
                }
                _ => {}
            }
        }
        if let Some(form) = self.form.take() {
            form.clear(window, cx);
        }
        self.draft_changed = false;
        self.form = Some(Form::new(action, offers, window, cx));
        if self
            .form
            .as_ref()
            .is_some_and(|form| form.secret.is_none() && form.server.is_none())
        {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    fn add_typed_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(fields) = self.form.as_ref().and_then(|form| form.server.as_ref()) {
            let typed = fields.model.read(cx).value().trim().to_owned();
            self.add_model(typed, window, cx);
        }
    }

    fn add_model(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(fields) = self
            .form
            .as_mut()
            .filter(|form| !form.saving)
            .and_then(|form| form.server.as_mut())
        {
            fields.add_model(id, window, cx);
            cx.notify();
        }
    }

    pub(super) fn close_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_form(window, cx);
        window.focus(&self.focus, cx);
    }

    fn clear_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = self.form.take() {
            form.clear(window, cx);
        }
        cx.notify();
    }

    pub(super) fn cancel_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.as_ref().is_some_and(Form::saving) {
            return;
        }
        self.close_form(window, cx);
        if self.setup {
            cx.emit(super::SetupEvent::Cancelled);
        }
    }

    #[cfg(test)]
    pub(super) fn submit_test_setup(
        &mut self,
        server: Option<(&str, &str)>,
        key: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let form = self.form.as_ref().expect("setup form");
        if let Some((address, model)) = server {
            let fields = form.server.as_ref().expect("server fields");
            fields
                .name
                .update(cx, |input, cx| input.set_value("Test server", window, cx));
            fields
                .address
                .update(cx, |input, cx| input.set_value(address, window, cx));
            fields.model.update(cx, |input, cx| input.set_value(model, window, cx));
        }
        if let Some(key) = key {
            form.secret
                .as_ref()
                .expect("key field")
                .update(cx, |input, cx| input.set_value(key, window, cx));
        }
        self.submit(window, cx);
    }

    pub(super) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone().filter(|store| !store.read(cx).saving) else {
            return;
        };
        let Some(form) = &mut self.form else { return };
        if form.saving {
            return;
        }
        let edit = match form.edit(cx) {
            Ok(edit) => edit,
            Err(error) => {
                form.error = Some(error);
                cx.notify();
                return;
            }
        };
        form.error = None;
        form.saving = true;
        let saved = store.update(cx, |store, cx| store.edit(edit, cx));
        cx.spawn_in(window, async move |this, cx| {
            let provider = saved.await.ok().and_then(Result::ok);
            this.update_in(cx, |this, window, cx| {
                if let Some(provider) = provider {
                    if this.setup {
                        this.clear_form(window, cx);
                        if let Some(id) = provider {
                            cx.emit(super::SetupEvent::Saved(id));
                        }
                    } else {
                        this.close_form(window, cx);
                    }
                } else if let Some(form) = &mut this.form {
                    form.saving = false;
                    this.sync(window, cx);
                }
            })
        })
        .detach_and_log_err(cx);
        cx.notify();
    }
}
