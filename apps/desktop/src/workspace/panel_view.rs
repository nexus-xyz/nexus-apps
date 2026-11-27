//! The panel's window (`panel.rs`): a right-aligned
//! head with the organization pill (its switcher), ⋮ (New chat, Record on macOS, Settings), Open in the app window and
//! Close; under it the chat, or the panel's Settings. Above the composer, Move here names the chat the app window shows
//! and continues it in the panel. Empty, the chat shows "Hi <first name>," / "what's on your mind?" and three
//! suggestion chips that fill the composer and never send. The conversation and the
//! composer are the app window's own (`transcript.rs`, `composer.rs`).
//!
//! On Windows and Linux the panel opens with motion: the surface fades in and drops 6px in 160ms on
//! `--nx-ease-out`, and the chips follow 40ms apart. It closes at once. On macOS the window itself fades and drops
//! instead (`panel_motion.rs`), and the surface stays still.
//!
//! Settings (⋮, Ctrl+, or the tray's Settings) shows the choices both apps share and the updates, from Settings › App
//! (`pages/desktop_app.rs`), then the Gateway, the organization, what the organization manages and the version, and
//! Open in Nexus UI and Sign out. Esc or Done goes back to the chat.
//!
//! Signed out, the panel asks the person to sign in in the app window: the device sign-in opens the browser, which takes
//! the focus, and the panel closes when it loses the focus.

use crate::workspace::MenuTrigger;
use crate::workspace::menu_item_hover;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::code_dock::menu_box;
use super::{Auth, View, Workspace, apply_kit_theme, glyph};
use crate::model::SessionSummary;
use crate::motion;
use crate::palette::Palette;

/// The head's height and the round buttons in it (`.panel-head`, `.panel-icon`).
const HEAD: f32 = 52.;
const ICON_BUTTON: f32 = 26.;
/// The resize grip's square at the bottom-left corner (`.panel-grip`).
const GRIP: f32 = 20.;
/// How far the surface drops as the panel opens, and how far apart the chips arrive.
const DROP: f32 = 6.;
const CHIP_STEP: Duration = Duration::from_millis(40);
const OPEN_HINT: &str = if cfg!(target_os = "macos") {
    "⌘↩"
} else {
    "Ctrl+Enter"
};

/// The panel's Settings: the App page's choices, compact, while signed in (they need the account's context).
pub(crate) struct PanelSettings {
    page: Option<AnyView>,
}

/// A suggestion chip fills the composer with a prompt.
pub(super) struct Suggestion {
    pub label: &'static str,
    pub text: &'static str,
    pub icon: Lucide,
}

pub(super) const SUGGESTIONS: [Suggestion; 3] = [
    Suggestion {
        label: "Rewrite this message for Slack",
        text: "Rewrite this message for Slack: ",
        icon: Lucide::PenLine,
    },
    Suggestion {
        label: "Help me think through a problem",
        text: "Help me think through a problem: ",
        icon: Lucide::Lightbulb,
    },
    Suggestion {
        label: "Explain a topic to me",
        text: "Explain ",
        icon: Lucide::Sparkles,
    },
];

/// The chat Move here offers: the app window's (`main`), unless the panel shows it already (`current`) or an answer is
/// coming in (`busy`). Both windows share one sign-in, so one organization.
pub(super) fn movable<'a>(
    main: Option<&'a SessionSummary>,
    current: Option<&str>,
    busy: bool,
) -> Option<&'a SessionSummary> {
    main.filter(|main| !busy && current != Some(main.id.as_str()))
}

/// Which watches the panel keeps (`keep`: windows still open) and which windows it starts watching (`add`), given the
/// windows it watches and the app windows open now.
pub(super) fn watch_changes<K: PartialEq + Copy>(watched: &[K], open: &[K]) -> (Vec<K>, Vec<K>) {
    let keep = watched.iter().copied().filter(|id| open.contains(id)).collect();
    let add = open.iter().copied().filter(|id| !watched.contains(id)).collect();
    (keep, add)
}

/// The first word of the account's name ("Ada Lovelace" is "Ada"); none when there is no name, or it is an email.
pub(super) fn first_name(name: &str) -> Option<&str> {
    name.split_whitespace().next().filter(|first| !first.contains('@'))
}

/// The empty panel's greeting on two lines: "Hi Ada," / "what's on your mind?".
pub(super) fn greeting(name: &str) -> [String; 2] {
    let first = match first_name(name) {
        Some(first) => format!("Hi {first},"),
        None => "Hi,".to_string(),
    };
    [first, "what's on your mind?".to_string()]
}

impl Workspace {
    /// The chat the panel shows, for its next window (`panel.rs`): `None` on a new chat.
    pub(crate) fn panel_chat(&self) -> Option<SessionSummary> {
        let id = self.current.clone()?;
        Some(SessionSummary {
            id,
            title: self.title.to_string(),
            day: None,
            at_ms: None,
        })
    }

    /// The chat this app window shows, for the panel's Move here: none on any other page, or in the panel.
    pub(crate) fn shown_chat(&self) -> Option<SessionSummary> {
        if self.panel || !matches!(self.view, View::Chat) {
            return None;
        }
        self.panel_chat()
    }

    /// Watch the app windows for the chat they show: Move here offers it. Only a
    /// change of chat redraws the panel. Called as the panel opens and each time an app window opens (`windows.rs`):
    /// a window already watched keeps its watch, a closed one drops it, and a new one gets one.
    pub(crate) fn watch_main_chat(&mut self, cx: &mut Context<Self>) {
        let windows = crate::windows::app_workspaces(cx);
        let ids: Vec<EntityId> = windows.iter().map(Entity::entity_id).collect();
        let watched: Vec<EntityId> = self.main_watch.iter().map(|(id, _)| *id).collect();
        let (keep, add) = watch_changes(&watched, &ids);
        self.main_watch.retain(|(id, _)| keep.contains(id));
        for workspace in windows.iter().filter(|workspace| add.contains(&workspace.entity_id())) {
            let watch = cx.observe(workspace, |this, _, cx| this.main_chat_changed(cx));
            self.main_watch.push((workspace.entity_id(), watch));
        }
        self.main_chat_changed(cx);
    }

    /// The app window focused last may show another chat: Move here follows it.
    fn main_chat_changed(&mut self, cx: &mut Context<Self>) {
        let chat = crate::windows::app_chat(cx);
        if chat != self.main_chat {
            self.main_chat = chat;
            cx.notify();
        }
    }

    /// The chat Move here offers: the app window's, unless the panel shows it already or an answer is coming in.
    fn movable(&self) -> Option<&SessionSummary> {
        movable(self.main_chat.as_ref(), self.current.as_deref(), self.asking.is_some())
    }

    /// Move here continues the app window's chat in the panel.
    fn move_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(chat) = self.movable().cloned() {
            self.open(&chat, window, cx);
            self.moving = Some(chat.id);
        }
    }

    /// Clear the Move here marker once the selected chat loads.
    pub(super) fn moved(&mut self, chat: &crate::gateway::OpenedChat) {
        if self.moving.as_deref() == Some(chat.id.as_str()) {
            self.moving = None;
        }
    }

    /// The Move here row above the composer (`.panel-move`): the app window's chat by name, and Move here.
    pub(super) fn render_move_here(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.panel {
            return None;
        }
        let chat = self.movable()?;
        Some(
            h_flex()
                .id("panel-move")
                .w_full()
                .mb(px(6.))
                .px(px(10.))
                .h(px(34.))
                .gap(px(8.))
                .rounded(px(10.))
                .bg(palette.hover)
                .text_size(px(12.5))
                .child(glyph(Lucide::MessageSquareShare, palette.muted).size(px(15.)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(palette.foreground)
                        .child(chat.title.clone()),
                )
                .child(
                    div()
                        .id("panel-move-button")
                        .flex_none()
                        .px(px(10.))
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .bg(palette.background)
                        .text_color(palette.strong)
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.selected))
                        .tooltip(|window, cx| Tooltip::new("Continue the app window's chat here").build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.move_here(window, cx)))
                        .child("Move here"),
                )
                .into_any_element(),
        )
    }

    /// The panel's window opened: on the chat it left (`resume`), or a new one, with the composer focused.
    pub(crate) fn panel_opened(&mut self, resume: Option<SessionSummary>, window: &mut Window, cx: &mut Context<Self>) {
        self.watch_main_chat(cx);
        match resume {
            // The sign-in may come back after the window opens: the chat opens then (`shell.rs`).
            Some(chat) => self.go_tray(super::shell::TrayPlace::Session(chat), window, cx),
            None => self.focus_composer(window, cx),
        }
    }

    /// The panel's New chat (⋮, Ctrl+N, the tray's New Chat): out of Settings, on a new chat.
    pub(crate) fn panel_new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.close_panel_menu();
        self.panel_settings = None;
        // A question on its way keeps its chat.
        if self.asking.is_none() {
            self.new_chat(window, cx);
        }
        cx.notify();
    }

    /// The panel's Settings (⋮, Ctrl+, or the tray's Settings).
    pub(crate) fn open_panel_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.close_panel_menu();
        self.overlay.close_org_menu();
        if self.panel_settings.is_none() {
            let device = self.device_page_ctx(cx);
            let page = Some(crate::pages::desktop_app::build_panel(device, cx));
            self.panel_settings = Some(PanelSettings { page });
        }
        cx.notify();
    }

    /// Done or Esc in the panel's Settings: back to the chat, the composer focused.
    pub(super) fn close_panel_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel_settings = None;
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Open in the app window: the panel's chat continues in the app window, and the panel closes.
    pub(super) fn open_in_app(&mut self, cx: &mut Context<Self>) {
        crate::panel::open_in_app(self.panel_chat(), cx);
    }

    /// Fill the composer and place the caret after the prompt.
    fn suggest(&mut self, chip: &'static Suggestion, window: &mut Window, cx: &mut Context<Self>) {
        let text = chip.text;
        self.composer.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.focus(window, cx);
        });
        self.composer_empty = false;
        cx.notify();
    }

    pub(super) fn render_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::for_choice(self.theme, window.appearance()).solid();
        cx.set_global(crate::pages::Theme(palette));
        apply_kit_theme(Palette::is_dark(self.theme, window.appearance()), window, cx);
        self.sync_placeholder(window, cx);
        let focused = self.composer.focus_handle(cx).is_focused(window);
        self.composer_focused = focused;
        let body = match self.auth {
            _ if self.panel_settings.is_some() => self.render_panel_settings(palette, cx).into_any_element(),
            Auth::SignedIn if matches!(self.view, View::Chat) && self.entries.is_empty() && !self.opening => {
                self.render_panel_empty(palette, cx).into_any_element()
            }
            Auth::SignedIn => self.render_chat(palette, cx).into_any_element(),
            Auth::Checking | Auth::Restoring => div().flex_1().into_any_element(),
            Auth::SignedOut { .. } | Auth::Waiting { .. } | Auth::Local => {
                if self.local_mode.opt_in.enabled() {
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .px(px(24.))
                        .text_color(palette.muted)
                        .child("Quick Chat requires an account.")
                        .into_any_element()
                } else {
                    self.render_panel_signed_out(palette, cx).into_any_element()
                }
            }
        };
        let surface = v_flex()
            .id("panel")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &super::NewChat, window, cx| this.panel_new_chat(window, cx)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.file_drag = false;
                this.attach_paths(paths.paths().to_vec(), cx);
            }))
            .size_full()
            .bg(palette.background)
            .text_color(palette.foreground)
            .child(self.render_panel_head(palette, cx))
            .child(v_flex().flex_1().min_h(px(0.)).child(body))
            .when(
                self.overlay.model_menu() || self.overlay.attach_menu() || self.overlay.mic_menu(),
                |root| {
                    // A click outside the open model, "+" or microphone menu closes it, as in the app window.
                    root.child(
                        deferred(
                            div()
                                .id("menu-backdrop")
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .occlude()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.overlay.close_model_menu();
                                        this.overlay.close_attach_menu();
                                        this.close_mic_menu();
                                        cx.notify();
                                    }),
                                ),
                        )
                        .with_priority(0),
                    )
                },
            );
        div()
            .relative()
            .size_full()
            .bg(palette.background)
            .child(if crate::panel_motion::WINDOW {
                surface.into_any_element()
            } else {
                surface
                    .with_animation(
                        "panel-open",
                        Animation::new(motion::UI).with_easing(motion::ease_out),
                        |surface, delta| surface.opacity(delta).relative().top(px(-DROP * (1.0 - delta))),
                    )
                    .into_any_element()
            })
            .when(crate::panel::GRIP, |root| root.child(render_grip(palette)))
    }

    /// The top row, right-aligned: the organization pill, ⋮, Open in the app window, Close. Its plain space moves the
    /// window.
    fn render_panel_head(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let signed_in = matches!(self.auth, Auth::SignedIn);
        h_flex()
            .flex_none()
            .h(px(HEAD))
            .px(px(10.))
            .gap(px(4.))
            .child(
                super::without_text_selection(div().id("panel-drag"))
                    .flex_1()
                    .h_full()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move()),
            )
            .when(signed_in && self.account.is_some(), |head| {
                head.child(self.render_panel_org(palette, cx))
            })
            .child(
                div()
                    .relative()
                    .child(
                        head_button("panel-more", Lucide::EllipsisVertical, palette)
                            .when(self.overlay.panel_menu(), |button| button.bg(palette.selected))
                            .tooltip(|window, cx| Tooltip::new("More").build(window, cx))
                            .menu_trigger(
                                self.overlay.panel_menu(),
                                cx,
                                |this| this.overlay.panel_menu(),
                                |this, _, cx| {
                                    this.toggle_overlay(Overlay::PanelMenu);
                                    this.overlay.close_org_menu();
                                    cx.notify();
                                },
                            ),
                    )
                    .children(
                        motion::presence("panel-menu", self.overlay.panel_menu(), cx)
                            .map(|phase| self.render_panel_menu(signed_in, phase, palette, cx)),
                    ),
            )
            .when(signed_in, |head| {
                head.child(
                    head_button("panel-open-in-app", Lucide::AppWindow, palette)
                        .tooltip(|window, cx| {
                            Tooltip::new(format!("Open in the app window ({OPEN_HINT})")).build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.open_in_app(cx))),
                )
            })
            .child(
                head_button("panel-close", Lucide::X, palette)
                    .tooltip(|window, cx| Tooltip::new("Close (Esc)").build(window, cx))
                    .on_click(|_, _, cx| crate::panel::hide_later(cx)),
            )
    }

    /// The organization pill (`OrgSwitcher variant="pill"`): its mark and name; a click opens the switcher under it.
    fn render_panel_org(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let (name, mark) = self.org_mark(palette);
        div()
            .relative()
            .min_w(px(0.))
            .child(
                h_flex()
                    .id("panel-org")
                    .h(px(ICON_BUTTON + 2.))
                    .max_w(px(180.))
                    .pl(px(6.))
                    .pr(px(8.))
                    .gap(px(6.))
                    .rounded_full()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.foreground)
                    .bg(palette.hover)
                    .cursor_pointer()
                    .when(self.overlay.org_menu().is_some(), |pill| pill.bg(palette.selected))
                    .hover(|style| style.bg(palette.selected))
                    .tooltip(|window, cx| Tooltip::new("Switch organization").build(window, cx))
                    .menu_trigger(
                        self.overlay.org_menu().is_some(),
                        cx,
                        |this| this.overlay.org_menu().is_some(),
                        |this, _, cx| {
                            this.overlay.close_panel_menu();
                            this.toggle_org_menu(cx);
                        },
                    )
                    .child(
                        div()
                            .flex_none()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(mark),
                    )
                    .child(div().min_w(px(0.)).truncate().child(name))
                    .child(glyph(Lucide::ChevronDown, palette.muted).size(px(12.))),
            )
            .children(self.render_org_menu(0., ICON_BUTTON + 6., palette, cx))
    }

    /// The ⋮ menu (`HeadMenu` in `panel.tsx`): New chat when signed in, Record your screen for an agent (macOS), and
    /// Settings, each with its keys.
    fn render_panel_menu(
        &self,
        signed_in: bool,
        phase: motion::Phase,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = |id: &'static str, icon: Lucide, label: &'static str, keys: &'static str| {
            h_flex()
                .id(id)
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .text_size(px(13.))
                .cursor_pointer()
                .hover(|style| menu_item_hover(style, palette))
                .child(glyph(icon, palette.icon).size(px(15.)))
                .child(div().flex_1().child(label))
                .when(!keys.is_empty(), |row| {
                    row.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(crate::shortcut_label::label(keys)),
                    )
                })
        };
        let menu = menu_box("panel-menu", 200., palette)
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.overlay.close_panel_menu();
                cx.notify();
            }))
            .when(signed_in, |menu| {
                menu.child(
                    row("panel-menu-new", Lucide::SquarePen, "New chat", "secondary-n")
                        .on_click(cx.listener(|this, _, window, cx| this.panel_new_chat(window, cx))),
                )
            })
            // "Record your screen for an agent" (macOS, when the organization allows it).
            .when(crate::record::available(), |menu| {
                menu.child(
                    row(
                        "panel-menu-record",
                        Lucide::CircleDot,
                        "Record your screen for an agent",
                        "",
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.overlay.close_panel_menu();
                        cx.notify();
                        crate::record::toggle(cx);
                    })),
                )
            })
            .child(
                row("panel-menu-settings", Lucide::Settings, "Settings", "secondary-,")
                    .on_click(cx.listener(|this, _, window, cx| this.open_panel_settings(window, cx))),
            );
        div()
            .absolute()
            .right_0()
            .top(px(ICON_BUTTON + 4.))
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopRight)
                        .snap_to_window_with_margin(px(8.))
                        .child(phase.menu("panel-menu", -4., menu)),
                )
                .with_priority(1),
            )
            .into_any_element()
    }

    /// The panel's Settings: a heading with Done, the App choices, then the facts and the links (`PanelSettings`).
    fn render_panel_settings(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let policy = crate::managed::policy();
        let page = self.panel_settings.as_ref().and_then(|settings| settings.page.clone());
        let fact = |label: &'static str, value: SharedString| {
            h_flex()
                .gap(px(8.))
                .text_size(px(12.5))
                .child(div().flex_none().text_color(palette.muted).child(label))
                .child(
                    div()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(palette.foreground)
                        .child(value),
                )
        };
        let note = |text: SharedString| div().text_size(px(12.5)).text_color(palette.muted).child(text);
        // What the organization manages on this computer: nobody here can change it, so a lock and quiet text.
        let managed = |text: SharedString| {
            h_flex()
                .gap(px(6.))
                .text_size(px(12.5))
                .text_color(palette.muted)
                .child(glyph(Lucide::Lock, palette.icon).size(px(13.)))
                .child(text)
        };
        let forced = crate::managed::forced_names(policy);
        let signed_in = matches!(self.auth, Auth::SignedIn);
        let gateway_note = if crate::managed::locked_gateway(policy).is_some() {
            format!(" · {}", crate::managed::SET_BY_ORG)
        } else {
            String::new()
        };
        v_flex()
            .id("panel-settings")
            .size_full()
            .overflow_y_scroll()
            .px(px(20.))
            .pb(px(20.))
            .gap(px(14.))
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_size(px(20.)).text_color(palette.strong).child("Settings"))
                    .child(
                        div()
                            .id("panel-settings-done")
                            .text_size(px(13.))
                            .text_color(palette.foreground)
                            .cursor_pointer()
                            .hover(|style| style.underline())
                            .on_click(cx.listener(|this, _, window, cx| this.close_panel_settings(window, cx)))
                            .child("Done"),
                    ),
            )
            .children(page)
            .child(
                v_flex()
                    .gap(px(6.))
                    .child(fact("Gateway", format!("{}{gateway_note}", self.gateway.url()).into()))
                    .children(
                        self.account
                            .as_ref()
                            .map(|account| fact("Organization", account.org_name.clone().into())),
                    )
                    .children(
                        (!policy.allow_local_cache)
                            .then(|| managed("Your organization turned off saving on this computer.".into())),
                    )
                    .children(
                        (!forced.is_empty())
                            .then(|| managed(format!("Set by your organization: {}.", forced.join(", ")).into())),
                    )
                    .child(note(format!("Nexus {}", crate::channel::VERSION).into())),
            )
            .child(h_flex().gap(px(16.)).text_size(px(13.)).when(signed_in, |links| {
                links
                    .child(
                        div()
                            .id("panel-open-ui")
                            .cursor_pointer()
                            .hover(|style| style.underline())
                            .on_click(cx.listener(|this, _, _, cx| this.open_in_browser("", cx)))
                            .child("Open in Nexus UI"),
                    )
                    .child(
                        div()
                            .id("panel-sign-out")
                            .cursor_pointer()
                            .hover(|style| style.underline())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel_settings = None;
                                this.request_sign_out();
                                cx.notify();
                            }))
                            .child("Sign out"),
                    )
            }))
    }

    /// A new chat: the greeting and the chips, the composer at the foot.
    fn render_panel_empty(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.account.as_ref().map(|account| account.name.as_str()).unwrap_or("");
        let [first, second] = greeting(name);
        let chips = SUGGESTIONS.iter().enumerate().map(|(index, chip)| {
            let delay = CHIP_STEP * (index as u32 + 1);
            let total = delay + motion::UI_LONG;
            let chip = h_flex()
                .id(("panel-chip", index))
                .h(px(36.))
                .px(px(14.))
                .gap(px(8.))
                .rounded_full()
                .bg(palette.hover)
                .text_size(px(13.))
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| style.bg(palette.selected).text_color(palette.strong))
                .active(|style| style.opacity(0.8))
                .on_click(cx.listener(move |this, _, window, cx| this.suggest(chip, window, cx)))
                .child(glyph(chip.icon, palette.muted))
                .child(chip.label);
            // Each chip waits its turn, then rises in as a message does.
            let (delay, total) = (delay.as_secs_f32(), total.as_secs_f32());
            chip.with_animation(
                ("panel-chip-in", index),
                Animation::new(Duration::from_secs_f32(total))
                    .with_easing(move |t| motion::ease_out(((t * total - delay) / (total - delay)).clamp(0., 1.))),
                |chip, delta| chip.opacity(delta).relative().top(px(DROP * (1.0 - delta))),
            )
        });
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .flex_1()
                    .justify_center()
                    .items_start()
                    .gap(px(20.))
                    .px(px(24.))
                    .child(
                        v_flex()
                            .text_size(px(24.))
                            .line_height(px(30.))
                            .text_color(palette.strong)
                            .child(first)
                            .child(second),
                    )
                    .child(v_flex().items_start().gap(px(8.)).children(chips))
                    .when_some(self.notice.clone(), |column, notice| {
                        column.child(div().text_sm().text_color(palette.destructive).child(notice))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(10.))
                    .pt(px(4.))
                    .pb(px(10.))
                    .children(self.render_move_here(palette, cx))
                    .child(self.render_composer(palette, cx)),
            )
    }

    /// Signed out: the sign-in is in the app window.
    fn render_panel_signed_out(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .justify_center()
            .items_start()
            .gap(px(16.))
            .px(px(24.))
            .child(
                div()
                    .text_size(px(24.))
                    .line_height(px(30.))
                    .text_color(palette.strong)
                    .child("Sign in to ask Nexus"),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .child("Sign in in the app window. The panel uses the same account."),
            )
            .child(
                div()
                    .id("panel-sign-in")
                    .px(px(14.))
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(palette.primary)
                    .text_color(palette.primary_foreground)
                    .text_size(px(13.))
                    .cursor_pointer()
                    .active(|style| style.opacity(0.8))
                    .on_click(cx.listener(|this, _, _, cx| this.open_in_app(cx)))
                    .child("Sign in"),
            )
    }
}

/// A round button in the head (`.panel-icon`).
fn head_button(id: &'static str, icon: Lucide, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(ICON_BUTTON))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|style| style.bg(palette.selected))
        .active(|style| style.opacity(0.8))
        .child(glyph(icon, palette.muted).size(px(15.)))
}

/// The grip at the bottom-left: a press resizes the panel from that corner
/// (`panel::grip`). The edges resize it too; the grip makes the whole mark a handle.
fn render_grip(palette: Palette) -> impl IntoElement {
    div()
        .id("panel-grip")
        .absolute()
        .left_0()
        .bottom_0()
        .size(px(GRIP))
        .pl(px(4.))
        .pt(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::ResizeUpRightDownLeft)
        .group("panel-grip")
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            cx.stop_propagation();
            crate::panel::grip(window);
        })
        .child(
            svg()
                .path(crate::assets::PANEL_GRIP)
                .size(px(10.))
                .text_color(palette.faint)
                .group_hover("panel-grip", |style| style.text_color(palette.strong)),
        )
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn the_greeting_has_the_first_name_or_none() {
        assert_eq!(greeting("Ada Lovelace"), ["Hi Ada,", "what's on your mind?"]);
        assert_eq!(greeting("  Ana "), ["Hi Ana,", "what's on your mind?"]);
        assert_eq!(greeting("ana@example.com"), ["Hi,", "what's on your mind?"]);
        assert_eq!(greeting(""), ["Hi,", "what's on your mind?"]);
    }

    #[test]
    fn move_here_offers_the_app_windows_chat_unless_the_panel_has_it_or_is_busy() {
        let chat = |id: &str| SessionSummary {
            id: id.into(),
            title: format!("Chat {id}"),
            day: None,
            at_ms: None,
        };
        let main = chat("a");
        assert_eq!(movable(Some(&main), None, false), Some(&main));
        assert_eq!(movable(Some(&main), Some("b"), false), Some(&main));
        // The panel shows it already.
        assert_eq!(movable(Some(&main), Some("a"), false), None);
        // An answer is coming in.
        assert_eq!(movable(Some(&main), None, true), None);
        // The app window shows no chat.
        assert_eq!(movable(None, None, false), None);
    }

    #[test]
    fn an_app_window_opened_after_the_panel_is_watched() {
        // The panel opened over windows 1 and 2; window 3 opens: only it gets a new watch.
        assert_eq!(watch_changes(&[1, 2], &[1, 2, 3]), (vec![1, 2], vec![3]));
        // Window 1 closed: its watch goes, the others stay.
        assert_eq!(watch_changes(&[1, 2, 3], &[2, 3]), (vec![2, 3], vec![]));
        // The panel opens with no watch yet: every window gets one.
        assert_eq!(watch_changes(&[], &[4, 5]), (vec![], vec![4, 5]));
        // Nothing changed: no new watch, so an open is not observed twice.
        assert_eq!(watch_changes(&[4, 5], &[4, 5]), (vec![4, 5], vec![]));
    }

    #[test]
    fn a_chip_fills_the_composer_with_room_to_finish() {
        assert_eq!(SUGGESTIONS.len(), 3);
        for chip in &SUGGESTIONS {
            assert!(chip.text.ends_with(' '), "{}", chip.text);
        }
    }
}
