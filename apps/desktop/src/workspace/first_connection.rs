//! First signed-out entry and the handoff to the local new-session form.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_providers::{InstanceId, KeyVendor, ProviderKind};
use nexus_workbench::provider::ProviderUi as _;
use serde::{Deserialize, Serialize};

use super::{Auth, View, Workspace, local_mode::Entry};
use crate::pages::{local_providers, ui};
use crate::palette::Palette;

pub(super) enum Page {
    Choices,
    KeyVendor,
    Form {
        editor: Entity<local_providers::Editor>,
        _events: Subscription,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Progress {
    Provider(InstanceId),
    Done,
}

impl Progress {
    fn load() -> Option<Self> {
        crate::paths::first_connection().and_then(|value| serde_json::from_value(value).ok())
    }

    fn save(&self) {
        match serde_json::to_value(self) {
            Ok(value) => crate::paths::save_first_connection(value),
            Err(error) => eprintln!("nexus-desktop-gpui: save first connection: {error}"),
        }
    }
}

#[derive(Default)]
struct ProviderFormOwner(Option<WeakEntity<Workspace>>);

impl Global for ProviderFormOwner {}

impl ProviderFormOwner {
    fn claim(&mut self, window: &Entity<Workspace>) -> bool {
        if self
            .0
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .is_some_and(|owner| &owner != window)
        {
            return false;
        }
        self.0 = Some(window.downgrade());
        true
    }

    fn release(&mut self, window: &Entity<Workspace>) -> bool {
        if self
            .0
            .as_ref()
            .is_some_and(|owner| owner.entity_id() == window.entity_id())
        {
            self.0 = None;
            return true;
        }
        false
    }
}

impl Workspace {
    pub(super) fn continue_without_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.byo_entry() || !matches!(self.auth, Auth::SignedOut { .. }) || self.gateway.marker().is_some() {
            return;
        }
        self.enter_local(Entry::Chosen, window, cx);
    }

    pub(super) fn open_first_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.local_mode.remembered {
            return;
        }
        match Progress::load() {
            Some(Progress::Done) => {}
            Some(Progress::Provider(id)) => self.claim_first_provider(id, cx),
            None => self.show_first_connection(Page::Choices, window, cx),
        }
    }

    fn claim_first_provider(&mut self, id: InstanceId, cx: &mut Context<Self>) {
        if self.local_mode.pending_first_provider.is_some() {
            return;
        }
        let entity = cx.entity();
        let claimed = cx.default_global::<ProviderFormOwner>().claim(&entity);
        self.code_form.begin_first_provider(claimed.then_some(id));
    }

    fn select_saved_first_provider(&mut self, id: &InstanceId, cx: &mut Context<Self>) {
        if Progress::load().is_none() {
            Progress::Provider(id.clone()).save();
            let entity = cx.entity();
            cx.default_global::<ProviderFormOwner>().claim(&entity);
        }
        // A form that was already open may finish after another window saved its own provider.
        self.code_form.begin_first_provider(Some(id.clone()));
        self.clear_failed_provider_draft();
    }

    pub(super) fn failed_provider_draft(&self) -> Option<&str> {
        let project = self.code_form.project.as_ref().filter(|_| self.is_local())?;
        self.local_mode.failed_provider_drafts.get(project).map(String::as_str)
    }

    pub(super) fn clear_failed_provider_draft(&mut self) {
        if let Some(project) = &self.code_form.project {
            self.local_mode.failed_provider_drafts.remove(project);
        }
    }

    fn show_first_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible = self.connection_open();
        self.local_mode.connection = None;
        if visible {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    fn show_first_connection(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.local_mode.connection = Some(page);
        cx.notify();
    }

    pub(super) fn connection_open(&self) -> bool {
        self.is_local() && matches!(self.view, View::NewCode) && self.local_mode.connection.is_some()
    }

    pub(super) fn connection_save_pending(&self, cx: &App) -> bool {
        matches!(&self.local_mode.connection, Some(Page::Form { editor, .. }) if editor.read(cx).setup_save_pending())
    }

    fn choose_connection(&mut self, kind: ProviderKind, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connection_open()
            || !matches!(self.local_mode.connection, Some(Page::Choices))
            || self.gateway.marker().is_some()
        {
            return;
        }
        match kind {
            ProviderKind::CliSubscription => self.dismiss_first_connection(window, cx),
            ProviderKind::ApiKey => self.show_first_connection(Page::KeyVendor, window, cx),
            ProviderKind::OpenAiCompatible => self.open_connection_form(local_providers::SetupForm::Server, window, cx),
            ProviderKind::NexusCloud => {
                self.leave_local(Auth::SignedOut { error: None }, window, cx);
                self.submit_browser_sign_in(window, cx);
            }
        }
    }

    fn open_connection_form(&mut self, form: local_providers::SetupForm, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connection_open() || self.gateway.marker().is_some() {
            return;
        }
        let key = matches!(form, local_providers::SetupForm::Key(_));
        let editor = local_providers::setup(form, window, cx);
        let events = cx.subscribe_in(&editor, window, move |this, editor, event, window, cx| {
            if !matches!(&this.local_mode.connection, Some(Page::Form { editor: current, .. }) if current == editor)
                || !(this.is_local() || this.local_mode.signing_in)
                || this.gateway.marker().is_some()
            {
                return;
            }
            match event {
                local_providers::SetupEvent::Saved(id) => {
                    this.select_saved_first_provider(id, cx);
                    this.show_first_session(window, cx);
                    this.load_local_provider_choices(cx);
                }
                local_providers::SetupEvent::Cancelled => {
                    if this.connection_open() {
                        this.show_first_connection(if key { Page::KeyVendor } else { Page::Choices }, window, cx);
                    } else {
                        this.local_mode.connection = None;
                    }
                }
            }
        });
        self.local_mode.connection = Some(Page::Form {
            editor,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn first_connection_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.window_handle.window_id() != window.window_handle().window_id()
            || !self.byo_entry()
            || !(matches!(self.auth, Auth::SignedOut { .. }) || self.connection_open())
            || key.modifiers.control
            || key.modifiers.alt
            || key.modifiers.platform
        {
            return false;
        }
        match key.key.as_str() {
            "tab" => {
                if key.modifiers.shift {
                    window.focus_prev(cx);
                } else {
                    window.focus_next(cx);
                }
                true
            }
            "escape" if !key.modifiers.shift => self.connection_back(window, cx),
            _ => false,
        }
    }

    pub(super) fn connection_back(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.connection_open() {
            return false;
        }
        match &self.local_mode.connection {
            Some(Page::Choices) => self.dismiss_first_connection(window, cx),
            Some(Page::KeyVendor) => self.show_first_connection(Page::Choices, window, cx),
            Some(Page::Form { editor, .. }) => {
                let view = editor.clone().into();
                local_providers::cancel_edit(&view, window, cx);
            }
            _ => return false,
        }
        true
    }

    fn dismiss_first_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if Progress::load().is_none() {
            Progress::Done.save();
        }
        self.show_first_session(window, cx);
    }

    pub(super) fn release_first_provider(&mut self, cx: &mut Context<Self>) {
        if self.local_mode.pending_first_provider.is_some() {
            return;
        }
        let entity = cx.entity();
        cx.default_global::<ProviderFormOwner>().release(&entity);
    }

    pub(super) fn clear_first_provider(&mut self, cx: &mut Context<Self>) {
        if self.code_form.finish_first_provider() {
            self.release_first_provider(cx);
        }
    }

    pub(super) fn release_pending_first_provider(&mut self, draft: &str, cx: &mut Context<Self>) {
        if self.local_mode.pending_first_provider.as_deref() == Some(draft) {
            self.local_mode.pending_first_provider = None;
            self.release_first_provider(cx);
        }
    }

    pub(super) fn record_first_local_turn(&self) {
        if Progress::load() != Some(Progress::Done) {
            Progress::Done.save();
        }
    }

    pub(super) fn render_first_connection(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(page) = &self.local_mode.connection else {
            return div().into_any_element();
        };
        let back = |cx: &mut Context<Self>| {
            focusable_connection_control(
                super::sign_in_parts::button(
                    "connection-back",
                    super::sign_in_parts::Button::Ghost,
                    "Back",
                    None,
                    palette,
                ),
                palette.strong,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.connection_back(window, cx);
            }))
        };
        let body = v_flex().w_full().max_w(px(500.)).gap(px(12.));
        let body = match page {
            Page::Choices => body
                .child(super::sign_in_parts::heading("Choose a connection", palette))
                .children(ProviderKind::ALL.map(|kind| {
                    let (title, icon) = match kind {
                        ProviderKind::CliSubscription => ("CLI subscription", Lucide::Terminal),
                        ProviderKind::ApiKey => ("API key", Lucide::Key),
                        ProviderKind::OpenAiCompatible => ("OpenAI-compatible server", Lucide::Server),
                        ProviderKind::NexusCloud => ("Nexus Cloud", Lucide::Cloud),
                    };
                    connection_row(kind.as_str(), title, kind.summary(), icon, palette)
                        .on_click(cx.listener(move |this, _, window, cx| this.choose_connection(kind, window, cx)))
                })),
            Page::KeyVendor => body
                .child(super::sign_in_parts::heading("Choose an API key provider", palette))
                .children(
                    [
                        (KeyVendor::Anthropic, "Runs Claude Code."),
                        (KeyVendor::Openai, "Runs Codex."),
                    ]
                    .map(|(vendor, detail)| {
                        connection_row(vendor.name(), vendor.name(), detail, Lucide::Key, palette).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.open_connection_form(local_providers::SetupForm::Key(vendor), window, cx);
                            }),
                        )
                    }),
                )
                .child(back(cx)),
            Page::Form { editor, .. } => body.child(editor.clone()),
        };
        v_flex()
            .id("first-connection")
            .size_full()
            .overflow_y_scroll()
            .items_center()
            .px(px(24.))
            .py(px(32.))
            .child(body.flex_none())
            .into_any_element()
    }
}

fn connection_row(
    id: &'static str,
    title: &'static str,
    detail: &'static str,
    icon: Lucide,
    palette: Palette,
) -> Stateful<Div> {
    focusable_connection_control(
        h_flex()
            .id(id)
            .role(Role::Button)
            .w_full()
            .items_start()
            .gap(px(12.))
            .px(px(16.))
            .py(px(12.))
            .rounded(px(10.))
            .border_1()
            .border_color(palette.border)
            .bg(palette.raised)
            .hover(move |row| row.bg(palette.hover))
            .cursor_pointer()
            .child(Icon::new(icon).size(px(16.)).text_color(palette.muted))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(3.))
                    .child(div().text_color(palette.foreground).child(title))
                    .child(ui::note(detail, palette)),
            ),
        palette.strong,
    )
}

pub(super) fn focusable_connection_control(control: Stateful<Div>, ring: Hsla) -> Stateful<Div> {
    control
        .role(Role::Button)
        .tab_index(0)
        .border_1()
        .focus_visible(move |style| style.border_color(ring))
}

#[cfg(test)]
#[path = "first_connection_tests.rs"]
pub(crate) mod tests;
