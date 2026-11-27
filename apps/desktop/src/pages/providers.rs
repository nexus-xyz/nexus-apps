//! Settings › Providers (`ui/.../settings/providers`, `provider-settings-view.tsx`): the providers Nexus calls, and
//! whose key pays for each. Owners and admins add, replace or remove the organization's own key, and turn a provider
//! on or off, which rewrites the organization's model policy. A saved key is never shown again: only its last four.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

use super::{Ctx, Load, Verb, fetch, palette, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

/// Owners and admins hold it; it lets a person see this page and change keys.
const PROVIDERS_PERMISSION: &str = "provider_keys.manage";
/// The On switch writes the organization's policy, which needs this one.
const POLICY_PERMISSION: &str = "policy.manage";

const HELP: &str = "Nexus uses your own key first. Without one, it uses the cheapest provider that serves the model.";

/// The row of provider `id` (`"anthropic"`), as a `SettingsFields` anchor: a setup line opens the page on it with
/// `Fix::open_at(PageId::Providers, anchor(id))`. Interned, so each provider leaks one short string once.
pub fn anchor(id: &str) -> &'static str {
    static ANCHORS: std::sync::OnceLock<std::sync::Mutex<Vec<&'static str>>> = std::sync::OnceLock::new();
    let name = format!("provider-{}", id.to_lowercase());
    let mut known = ANCHORS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(found) = known.iter().find(|known| **known == name) {
        return found;
    }
    let interned: &'static str = Box::leak(name.into_boxed_str());
    known.push(interned);
    interned
}

/// The fix key a saved key of provider `id` completes: what waits on it (a held message, a chat to retry) resumes.
pub fn key_added(id: &str) -> crate::setup::FixKey {
    format!("provider-key:{}", id.to_lowercase()).into()
}

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| ProvidersPage::new(ctx, window, cx));
    Some(view.into())
}

/// One provider from `GET /v1/providers`.
#[derive(Debug, Clone, PartialEq)]
struct Provider {
    id: String,
    name: String,
    /// Whether the Gateway calls it now.
    active: bool,
    /// Whether an organization may bring its own key.
    byok: bool,
    models: u64,
    /// `none`, `valid` or `rejected`; absent without a key.
    key: Option<String>,
    /// Absent until the Gateway reports it: the page then hides the switch.
    enabled: Option<bool>,
}

/// One stored key from `GET /v1/keys/providers`: only its last four characters.
#[derive(Debug, Clone, PartialEq)]
struct StoredKey {
    provider: String,
    /// A team's or a person's key has a scope; the organization's own has none.
    scoped: bool,
    last4: Option<String>,
    state: Option<String>,
    notice: Option<String>,
}

/// Who pays for a provider's calls.
#[derive(Debug, Clone, PartialEq)]
enum KeyCell {
    Nexus,
    Add,
    Own(Option<String>),
    Rejected(Option<String>),
    /// An admin disabled the stored key. `/v1/providers` says `none`, so the key list tells.
    Disabled(Option<String>, Option<String>),
}

impl KeyCell {
    fn stored(&self) -> bool {
        matches!(self, KeyCell::Own(_) | KeyCell::Rejected(_) | KeyCell::Disabled(..))
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Row {
    id: String,
    name: String,
    active: bool,
    models: u64,
    enabled: Option<bool>,
    key: KeyCell,
}

fn parse_providers(value: &Value) -> Vec<Provider> {
    value
        .get("data")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|entry| {
                    let id = entry.get("id")?.as_str()?.to_string();
                    Some(Provider {
                        name: entry.get("name").and_then(Value::as_str).unwrap_or(&id).to_string(),
                        active: ui::text(entry, "status") == "active",
                        byok: entry.get("byok").and_then(Value::as_bool).unwrap_or(false),
                        models: entry.get("models").and_then(Value::as_u64).unwrap_or(0),
                        key: entry.get("key").and_then(Value::as_str).map(String::from),
                        enabled: entry.get("enabled").and_then(Value::as_bool),
                        id,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_keys(value: &Value) -> Vec<StoredKey> {
    let read = |entry: &Value| {
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(String::from)
        };
        Some(StoredKey {
            provider: entry.get("provider")?.as_str()?.to_string(),
            scoped: entry.get("scope").is_some_and(|scope| !scope.is_null()),
            last4: text("last4"),
            state: text("state"),
            notice: text("notice"),
        })
    };
    value
        .get("providers")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(read).collect())
        .unwrap_or_default()
}

fn rows(providers: &[Provider], keys: &[StoredKey]) -> Vec<Row> {
    providers
        .iter()
        .map(|provider| {
            let own = keys.iter().find(|key| key.provider == provider.id && !key.scoped);
            Row {
                id: provider.id.clone(),
                name: provider.name.clone(),
                active: provider.active,
                models: provider.models,
                enabled: provider.enabled,
                key: key_cell(provider, own),
            }
        })
        .collect()
}

fn key_cell(provider: &Provider, stored: Option<&StoredKey>) -> KeyCell {
    if !provider.byok {
        return KeyCell::Nexus;
    }
    let last4 = stored.and_then(|key| key.last4.clone());
    if stored.and_then(|key| key.state.as_deref()) == Some("disabled") {
        return KeyCell::Disabled(last4, stored.and_then(|key| key.notice.clone()));
    }
    match provider.key.as_deref() {
        Some("valid") => KeyCell::Own(last4),
        Some("rejected") => KeyCell::Rejected(last4),
        _ => KeyCell::Add,
    }
}

/// The organization's rule with provider `id` turned on or off, every other part kept. Off adds it to `deny`. On
/// removes it from `deny`, and adds it to `allow` when the organization uses an allow-list.
fn with_provider(rule: &Value, id: &str, on: bool) -> Value {
    let mut rule = match rule {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    let lists = rule.remove("providers").unwrap_or(Value::Null);
    let strings = |key: &str| -> Option<Vec<String>> {
        lists
            .get(key)
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
    };
    let allow = strings("allow");
    let deny: Vec<String> = strings("deny")
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry != id)
        .collect();
    let mut out = serde_json::Map::new();
    if let Some(mut allow) = allow {
        if on && !allow.iter().any(|entry| entry == id) {
            allow.push(id.to_string());
        }
        out.insert("allow".into(), json!(allow));
    }
    if on {
        if !deny.is_empty() {
            out.insert("deny".into(), json!(deny));
        }
    } else {
        let mut deny = deny;
        deny.push(id.to_string());
        out.insert("deny".into(), json!(deny));
    }
    if !out.is_empty() {
        rule.insert("providers".into(), Value::Object(out));
    }
    Value::Object(rule)
}

/// Gateway's refusal in plain words: a 403 reads as who may do it.
fn refusal(error: &str) -> String {
    let lower = error.to_lowercase();
    if lower.contains("owner or admins") || lower.contains("only the owner") || lower.contains("operator grant") {
        "Only organization admins can change this.".into()
    } else {
        error.to_string()
    }
}

fn permissions(value: &Value) -> Option<Vec<String>> {
    value
        .get("permissions")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
}

struct Message {
    text: SharedString,
    error: bool,
}

struct ProvidersPage {
    ctx: Ctx,
    /// The caller's permissions from `/v1/tenants/me`; `None` when Gateway lists none (then its 403 tells).
    permissions: Load<Option<Vec<String>>>,
    providers: Load<Vec<Provider>>,
    keys: Load<Vec<StoredKey>>,
    /// The organization's rule, for the On switch.
    rule: Load<Value>,
    /// The provider whose key form is open.
    editing: Option<String>,
    key_input: Entity<InputState>,
    saving: bool,
    form_error: Option<SharedString>,
    /// The provider whose switch is being saved: one write at a time, since each rewrites the whole rule.
    toggling: Option<String>,
    /// The provider whose Remove was pressed once.
    confirm_remove: Option<String>,
    removing: bool,
    message: Option<Message>,
    _subscription: Subscription,
}

impl ProvidersPage {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste the API key")
        });
        let subscription = cx.subscribe_in(&key_input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.save_key(window, cx);
            }
        });
        let mut page = ProvidersPage {
            ctx,
            permissions: Load::Loading,
            providers: Load::Loading,
            keys: Load::Loading,
            rule: Load::Loading,
            editing: None,
            key_input,
            saving: false,
            form_error: None,
            toggling: None,
            confirm_remove: None,
            removing: false,
            message: None,
            _subscription: subscription,
        };
        page.load(cx);
        page
    }

    fn can(&self, permission: &str) -> bool {
        match &self.permissions {
            Load::Ready(Some(list)) => list.iter().any(|entry| entry == permission),
            // Gateway did not say: let it answer, and show its refusal.
            _ => true,
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.permissions = Load::Loading;
        let request = self.ctx.gateway.query("/v1/tenants/me".into());
        super::read(cx, request, |this, answer, cx| {
            this.permissions = match answer {
                Ok(value) => Load::Ready(permissions(&value)),
                Err(error) => Load::Failed(error.into()),
            };
            if matches!(this.permissions, Load::Ready(_)) && this.can(PROVIDERS_PERMISSION) {
                this.load_lists(cx);
            }
        });
    }

    fn load_lists(&mut self, cx: &mut Context<Self>) {
        self.providers = Load::Loading;
        let request = self.ctx.gateway.query("/v1/providers".into());
        super::read(cx, request, |this, answer, _| {
            this.providers = match answer {
                Ok(value) => Load::Ready(parse_providers(&value)),
                Err(error) => Load::Failed(error.into()),
            };
        });
        self.load_keys(cx);
        if self.can(POLICY_PERMISSION) {
            self.load_rule(cx);
        } else {
            self.rule = Load::Failed("no permission".into());
        }
    }

    fn load_keys(&mut self, cx: &mut Context<Self>) {
        let request = self.ctx.gateway.query("/v1/keys/providers".into());
        super::read(cx, request, |this, answer, _| {
            // Without the key list the rows still show what `/v1/providers` says.
            this.keys = match answer {
                Ok(value) => Load::Ready(parse_keys(&value)),
                Err(error) => Load::Failed(error.into()),
            };
        });
    }

    fn load_rule(&mut self, cx: &mut Context<Self>) {
        let request = self.ctx.gateway.query("/v1/policies/organization".into());
        super::read(cx, request, |this, answer, _| {
            this.rule = match answer {
                Ok(value) => Load::Ready(value.get("rule").cloned().unwrap_or_else(|| json!({}))),
                Err(error) => Load::Failed(error.into()),
            };
        });
    }

    /// Refresh the providers and the keys after a change, without the loading state.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let request = self.ctx.gateway.query("/v1/providers".into());
        super::read(cx, request, |this, answer, _| {
            if let Ok(value) = answer {
                this.providers = Load::Ready(parse_providers(&value));
            }
        });
        self.load_keys(cx);
    }

    fn rows(&self) -> Vec<Row> {
        let Load::Ready(providers) = &self.providers else {
            return Vec::new();
        };
        let keys = match &self.keys {
            Load::Ready(keys) => keys.as_slice(),
            _ => &[],
        };
        rows(providers, keys)
    }

    fn open_form(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.as_deref() == Some(id.as_str()) {
            self.editing = None;
        } else {
            self.editing = Some(id);
            self.key_input.update(cx, |state, cx| {
                state.set_value("", window, cx);
                state.focus(window, cx);
            });
        }
        self.form_error = None;
        self.confirm_remove = None;
        cx.notify();
    }

    fn save_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(row) = self
            .editing
            .as_ref()
            .and_then(|id| self.rows().into_iter().find(|row| &row.id == id))
        else {
            return;
        };
        let key = self.key_input.read(cx).value().trim().to_string();
        if key.is_empty() {
            self.form_error = Some("Paste the key first.".into());
            cx.notify();
            return;
        }
        // A rejected key is replaced on its own route; anything else is added, which also replaces a stored one.
        let (path, body) = if matches!(row.key, KeyCell::Rejected(_)) {
            (format!("/v1/keys/providers/{}/replace", row.id), json!({ "key": key }))
        } else {
            (
                "/v1/keys/providers".to_string(),
                json!({ "provider": row.id, "key": key }),
            )
        };
        // The key leaves the field at once: it is never drawn again.
        self.key_input.update(cx, |state, cx| state.set_value("", window, cx));
        self.saving = true;
        self.form_error = None;
        self.message = None;
        let request = self.ctx.gateway.request(Verb::Post, path, Some(body));
        let window_handle = window.window_handle();
        fetch(cx, request, move |this, answer, cx| {
            this.saving = false;
            match answer {
                Ok(value) => {
                    // What waited on this key (a held send, a refused chat) goes on, outside this page's update.
                    let key = key_added(&row.id);
                    cx.defer(move |cx| {
                        let done = window_handle.update(cx, |_, window, cx| crate::setup::done(&key, window, cx));
                        if let Err(error) = done {
                            eprintln!("nexus-desktop-gpui: providers: key added: {error}");
                        }
                    });
                    let saved = value.get("providerKey").unwrap_or(&value);
                    let last4 = ui::text(saved, "last4");
                    let still = if matches!(row.key, KeyCell::Disabled(..)) {
                        format!(" It stays disabled until `nexus keys enable {}`.", row.id)
                    } else {
                        String::new()
                    };
                    let ending = if last4.is_empty() {
                        String::new()
                    } else {
                        format!(" ending {last4}")
                    };
                    this.message = Some(Message {
                        text: format!("Saved your {} key{ending}.{still}", row.name).into(),
                        error: false,
                    });
                    this.editing = None;
                    this.refresh(cx);
                }
                Err(error) => this.form_error = Some(refusal(&error).into()),
            }
        });
        cx.notify();
    }

    fn remove_key(&mut self, row: Row, cx: &mut Context<Self>) {
        if self.confirm_remove.as_deref() != Some(row.id.as_str()) {
            self.confirm_remove = Some(row.id);
            cx.notify();
            return;
        }
        self.confirm_remove = None;
        self.removing = true;
        self.message = None;
        let path = format!("/v1/keys/providers/{}/remove", row.id);
        let request = self.ctx.gateway.request(Verb::Post, path, Some(json!({})));
        fetch(cx, request, move |this, answer, cx| {
            this.removing = false;
            this.message = Some(match answer {
                Ok(_) => {
                    this.editing = None;
                    this.refresh(cx);
                    Message {
                        text: format!("Removed your {} key.", row.name).into(),
                        error: false,
                    }
                }
                Err(error) => Message {
                    text: refusal(&error).into(),
                    error: true,
                },
            });
        });
        cx.notify();
    }

    fn toggle(&mut self, row: Row, on: bool, cx: &mut Context<Self>) {
        let Load::Ready(rule) = &self.rule else {
            return;
        };
        if self.toggling.is_some() {
            return;
        }
        let body = with_provider(rule, &row.id, on);
        self.toggling = Some(row.id.clone());
        self.message = None;
        let request = self
            .ctx
            .gateway
            .request(Verb::Put, "/v1/policies/organization".into(), Some(body));
        fetch(cx, request, move |this, answer, cx| {
            this.toggling = None;
            match answer {
                Ok(value) => {
                    if let Some(rule) = value.get("rule") {
                        this.rule = Load::Ready(rule.clone());
                    }
                    this.message = Some(Message {
                        text: if on {
                            format!("{} is on.", row.name)
                        } else {
                            format!("{} is off. Calls no longer use it.", row.name)
                        }
                        .into(),
                        error: false,
                    });
                    this.refresh(cx);
                }
                Err(error) => {
                    this.message = Some(Message {
                        text: refusal(&error).into(),
                        error: true,
                    })
                }
            }
        });
        cx.notify();
    }

    /// The page's one notice for who can't change what `needs` guards: it names the owners and admins, and asks them.
    fn role_line(&self, needs: &'static str, palette: Palette, cx: &App) -> AnyElement {
        let org = match self.ctx.org_name.trim() {
            "" => "our organization".to_string(),
            name => name.to_string(),
        };
        let (what, message) = if needs == POLICY_PERMISSION {
            (
                "Owners and admins turn providers on or off.",
                format!("Could you turn a provider on or off for {org} in Nexus?"),
            )
        } else {
            (
                "Owners and admins change providers.",
                format!("Could you change {org}'s providers or keys in Nexus?"),
            )
        };
        let access = self.ctx.fixer.access(cx);
        crate::setup::role_notice(format!("providers:{needs}"), what, needs, message, &access)
            .render(&self.ctx.fixer, palette, cx)
            .into_any_element()
    }

    fn render_key_cell(&self, row: &Row, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = row.id.clone();
        let action = |label: &'static str, cx: &mut Context<Self>| {
            let id = id.clone();
            key_button(&id, label, palette, true)
                .on_click(cx.listener(move |this, _, window, cx| this.open_form(id.clone(), window, cx)))
        };
        let dots = |last4: &Option<String>| {
            last4.as_ref().map(|last4| {
                div()
                    .font_family(ui::MONO)
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child(format!("••••{last4}"))
            })
        };
        let cell = h_flex().gap(px(8.)).min_w(px(0.)).text_size(px(13.));
        match &row.key {
            KeyCell::Nexus => cell.child(ui::note("Nexus pays", palette)).into_any_element(),
            KeyCell::Add => cell.child(action(ADD_KEY, cx)).into_any_element(),
            KeyCell::Own(last4) => cell
                .child(div().text_color(palette.foreground).child("Your key"))
                .children(dots(last4))
                .child(action("Replace", cx))
                .into_any_element(),
            KeyCell::Rejected(last4) => cell
                .child(ui::badge("Rejected", palette).text_color(palette.destructive))
                .child(
                    div()
                        .min_w(px(0.))
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(format!("{} refused your key. Add a new one.", row.name)),
                )
                .children(dots(last4))
                .child(action("Replace", cx))
                .into_any_element(),
            KeyCell::Disabled(last4, notice) => cell
                .child(ui::badge("Disabled", palette))
                .children(dots(last4))
                .child(
                    div()
                        .min_w(px(0.))
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(disabled_notice(&row.id, notice.as_deref())),
                )
                .child(action("Replace", cx))
                .into_any_element(),
        }
    }

    fn render_form(&self, row: &Row, palette: Palette, cx: &mut Context<Self>) -> Div {
        let replacing = row.key.stored();
        let confirming = self.confirm_remove.as_deref() == Some(row.id.as_str());
        let remove_row = row.clone();
        v_flex().mt(px(16.)).gap(px(8.)).child(
            ui::card(palette)
                .child(ui::label(
                    if replacing {
                        format!("Replace the {} key", row.name)
                    } else {
                        format!("{} API key", row.name)
                    },
                    palette,
                ))
                .child(
                    h_flex()
                        .gap(px(8.))
                        .child(
                            h_flex()
                                .flex_1()
                                .h(px(32.))
                                .px(px(10.))
                                .rounded(px(8.))
                                .bg(palette.hover)
                                .text_size(px(13.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .child(Input::new(&self.key_input).appearance(false)),
                                ),
                        )
                        .child(
                            ui::primary_button(
                                "provider-save-key",
                                if self.saving { "Saving…" } else { "Save key" },
                                palette,
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.save_key(window, cx))),
                        )
                        .child(
                            ui::button("provider-cancel-key", "Cancel", palette).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.editing = None;
                                    this.form_error = None;
                                    this.confirm_remove = None;
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .when(replacing, |card| {
                    card.child(
                        h_flex()
                            .gap(px(8.))
                            .pt(px(4.))
                            .child(ui::note(
                                if confirming {
                                    "Remove it? Nexus pays for this provider's calls after."
                                } else {
                                    "Or remove the stored key."
                                },
                                palette,
                            ))
                            .child(
                                ui::danger_button(
                                    "provider-remove-key",
                                    if self.removing {
                                        "Removing…"
                                    } else if confirming {
                                        "Remove key"
                                    } else {
                                        "Remove"
                                    },
                                    palette,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| this.remove_key(remove_row.clone(), cx))),
                            ),
                    )
                })
                .when_some(self.form_error.clone(), |card, error| {
                    card.child(ui::error_line(error, palette))
                }),
        )
    }
}

/// The text for a disabled key, with the Gateway's own notice when it sends one.
fn disabled_notice(provider: &str, notice: Option<&str>) -> String {
    let notice = notice
        .map(String::from)
        .unwrap_or_else(|| format!("disabled: no call uses it until `nexus keys enable {provider}`"));
    format!("{notice}. The saved key is kept but never used.")
}

const ADD_KEY: &str = "Add key";

fn key_button(id: &str, label: &'static str, palette: Palette, enabled: bool) -> Stateful<Div> {
    let id = SharedString::from(format!("provider-key-{id}"));
    if enabled {
        ui::button(id, label, palette)
    } else {
        ui::disabled_button(id, label, palette)
    }
}

pub(super) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        v_flex()
            .child(key_button("unavailable", ADD_KEY, palette, false))
            .into_any_element()
    })
}

impl Render for ProvidersPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let column = v_flex().w_full().child(ui::lead(
            "The providers Nexus calls for your models, and whose key pays for each.",
            palette,
        ));
        match &self.permissions {
            Load::Loading => return column.child(ui::skeleton(4, palette)),
            Load::Failed(error) => {
                return column.child(ui::failed(error.clone(), palette)).child(
                    h_flex().justify_center().child(
                        ui::button("providers-retry", "Try again", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                    ),
                );
            }
            Load::Ready(_) => {}
        }
        if !self.can(PROVIDERS_PERMISSION) {
            return column.child(self.role_line(PROVIDERS_PERMISSION, palette, cx));
        }
        match &self.providers {
            Load::Loading => return column.child(ui::skeleton(4, palette)),
            Load::Failed(error) => {
                return column.child(ui::failed(error.clone(), palette)).child(
                    h_flex().justify_center().child(
                        ui::button("providers-retry", "Try again", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.load_lists(cx))),
                    ),
                );
            }
            Load::Ready(_) => {}
        }
        let rows = self.rows();
        let toggles = rows.iter().any(|row| row.enabled.is_some());
        // Who can't change the policy sees one notice and a locked state on each row, never a red line.
        let role_locked = toggles && !self.can(POLICY_PERMISSION);
        let toggles_off: Option<String> = match &self.rule {
            Load::Failed(error) if toggles && !role_locked => Some(format!(
                "The On switches are off: the organization's policy could not be loaded ({error})."
            )),
            _ => None,
        };
        let switches_live =
            !role_locked && toggles_off.is_none() && matches!(self.rule, Load::Ready(_)) && self.toggling.is_none();

        let mut column = column.child(ui::note(HELP, palette).mb(px(12.)));
        if role_locked {
            column = column.child(div().mb(px(12.)).child(self.role_line(POLICY_PERMISSION, palette, cx)));
        }
        if let Some(text) = toggles_off.clone() {
            column = column.child(ui::error_line(text, palette).mb(px(12.)));
        }
        if let Load::Failed(error) = &self.keys {
            column =
                column.child(ui::note(format!("Your stored keys could not be read: {error}"), palette).mb(px(12.)));
        }
        if rows.is_empty() {
            return column.child(ui::empty(
                "No providers yet",
                "Nexus Gateway lists no providers.",
                palette,
            ));
        }
        let head = ui::head_row(palette)
            .child(div().w(px(200.)).child("Provider"))
            .when(toggles, |head| head.child(div().w(px(56.)).child("On")))
            .child(div().flex_1().child("Key"))
            .child(div().w(px(60.)).text_right().child("Models"));
        let list = v_flex().child(head).children(rows.iter().map(|row| {
            let saving_this = self.toggling.as_deref() == Some(row.id.as_str());
            let switch = row.enabled.map(|on| {
                if role_locked {
                    return h_flex().child(crate::setup::locked::reason(if on { "On" } else { "Off" }, palette));
                }
                let toggle_row = row.clone();
                h_flex()
                    .gap(px(6.))
                    .child(
                        ui::switch(
                            SharedString::from(format!("provider-on-{}", row.id)),
                            on,
                            !switches_live,
                            palette,
                        )
                        .when(switches_live, |switch| {
                            switch.on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_row.clone(), !on, cx)))
                        }),
                    )
                    .when(saving_this, |cell| cell.child(ui::label("Saving…", palette)))
            });
            let line = ui::row(palette)
                .child(
                    ui::two_lines(
                        row.name.clone(),
                        if row.active {
                            row.id.clone()
                        } else {
                            "Not called yet".into()
                        },
                        palette,
                    )
                    .w(px(200.)),
                )
                .when(toggles, |line| line.child(div().w(px(56.)).children(switch)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(self.render_key_cell(row, palette, cx)),
                )
                .child(
                    div()
                        .w(px(60.))
                        .text_right()
                        .text_size(px(13.))
                        .text_color(palette.foreground)
                        .child(ui::count(row.models)),
                );
            // A setup line's "Add key" arrives on this row (`anchor`).
            self.ctx.settings_fields.field(anchor(&row.id), line, None, cx)
        }));
        let form = self
            .editing
            .as_ref()
            .and_then(|id| rows.iter().find(|row| &row.id == id))
            .map(|row| self.render_form(row, palette, cx));
        column
            .child(list)
            .children(form)
            .when_some(self.message.as_ref(), |column, message| {
                column.child(if message.error {
                    ui::error_line(message.text.clone(), palette).mt(px(12.))
                } else {
                    ui::note(message.text.clone(), palette).mt(px(12.)).text_color(palette.foreground)
                })
            })
            .child(
                ui::note(
                    "A key is used for your organization only. Calls on it still count against your budget and spend limits.",
                    palette,
                )
                .mt(px(12.)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The glob above brings GPUI's own `test` macro; this is the built-in one.
    use core::prelude::v1::test;

    /// A sample answer, parsed into the shape Gateway sends.
    fn sample(text: &str) -> Value {
        serde_json::from_str(text).expect("sample is JSON")
    }

    #[test]
    fn reads_providers_and_keys_into_rows() {
        let providers = parse_providers(&sample(
            r#"{"object": "list", "data": [{"id": "anthropic", "name": "Anthropic", "country": "US", "byok": true, "status": "active", "models": 12, "key": "valid", "enabled": true}, {"id": "openai", "name": "OpenAI", "country": "US", "byok": true, "status": "active", "models": 9, "key": "rejected"}, {"id": "deepseek", "name": "DeepSeek", "country": "CN", "byok": false, "status": "inactive", "models": 3}, {"id": "mistral", "name": "Mistral", "country": "FR", "byok": true, "status": "active", "models": 4, "key": "none"}, {"name": "no id"}]}"#,
        ));
        assert_eq!(providers.len(), 4);
        assert_eq!(providers[0].enabled, Some(true));
        assert_eq!(providers[1].enabled, None);
        let keys = parse_keys(&sample(
            r#"{"providers": [{"provider": "anthropic", "last4": "abcd", "createdAt": "2026-10-01T00:00:00Z", "state": "valid"}, {"provider": "openai", "last4": "wxyz", "createdAt": "2026-10-01T00:00:00Z", "state": "rejected"}, {"provider": "mistral", "last4": "1234", "state": "disabled", "createdAt": "2026-10-01T00:00:00Z"}, {"provider": "anthropic", "scope": "team", "scopeId": "t1", "last4": "team", "createdAt": "2026-10-01T00:00:00Z"}], "scopedKeys": []}"#,
        ));
        let rows = rows(&providers, &keys);
        assert_eq!(rows[0].key, KeyCell::Own(Some("abcd".into())));
        assert_eq!(rows[1].key, KeyCell::Rejected(Some("wxyz".into())));
        assert_eq!(rows[2].key, KeyCell::Nexus);
        assert!(!rows[2].active);
        assert_eq!(rows[3].key, KeyCell::Disabled(Some("1234".into()), None));
    }

    #[test]
    fn each_provider_row_has_one_anchor_a_setup_line_opens() {
        assert_eq!(anchor("anthropic"), "provider-anthropic");
        assert!(std::ptr::eq(anchor("Anthropic"), anchor("anthropic")), "interned once");
        assert_eq!(key_added("OpenAI").as_ref(), "provider-key:openai");
    }

    #[test]
    fn a_switch_keeps_the_rest_of_the_rule() {
        let rule = json!({ "allow": ["anthropic/*"], "defaults": { "chat": "anthropic/claude-opus-5-5" } });
        let off = with_provider(&rule, "openai", false);
        assert_eq!(off["providers"], json!({ "deny": ["openai"] }));
        assert_eq!(off["allow"], json!(["anthropic/*"]));
        let on = with_provider(&off, "openai", true);
        assert!(on.get("providers").is_none());
        assert_eq!(on["defaults"]["chat"], "anthropic/claude-opus-5-5");
        let listed = with_provider(&json!({ "providers": { "allow": ["anthropic"] } }), "openai", true);
        assert_eq!(listed["providers"], json!({ "allow": ["anthropic", "openai"] }));
    }

    #[test]
    fn a_refusal_reads_plainly() {
        assert_eq!(
            refusal("only the owner or admins can manage provider keys"),
            "Only organization admins can change this."
        );
        assert_eq!(refusal("provider refused the key"), "provider refused the key");
        assert_eq!(
            permissions(&json!({ "permissions": ["policy.manage"] })),
            Some(vec!["policy.manage".into()])
        );
        assert_eq!(permissions(&json!({})), None);
    }
}
