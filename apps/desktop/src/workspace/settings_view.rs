//! Settings' sections drawn by the workspace itself (`ui/src/components/nexus-ui/settings/`): Preferences' Appearance,
//! Notifications and Shortcuts, Keys and Account. Appearance is this computer's (`paths::AppSettings`); notification
//! channels and API keys live on Gateway. The shell around them, its nav and its stacked pages are `settings_shell.rs`.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::settings_shell::{RowStatus, Shell};
use super::{ThemeChoice, View, Workspace, glyph};
use crate::i18n::tr;
use crate::pages::{PageId, ui};
use crate::palette::Palette;
use crate::settings::{ApiToken, ChannelPreferences, ChannelSetting, GROUPS, NewToken};

// The theme cards live beside this file, so the Appearance section stays one call here.
#[path = "theme_cards.rs"]
pub(super) mod theme_cards;
#[path = "transparency_tune.rs"]
pub(super) mod transparency_tune;

/// The channels a person turns on per group. The Inbox is not one: it always gets every group.
const CHANNELS: [(&str, &str, Lucide); 2] = [("email", "Email", Lucide::Mail), ("push", "Push", Lucide::Smartphone)];
/// A channel column's width in the notifications card.
const CELL: f32 = 84.;

/// A section of Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsSection {
    Appearance,
    Notifications,
    ApiKeys,
    Shortcuts,
    Account,
    LocalProviders,
    GatewayLink(super::settings_registry::Dest),
    /// A page drawn by `pages/`.
    Page(PageId),
}

impl SettingsSection {
    pub(super) fn label(self) -> &'static str {
        match self {
            SettingsSection::Appearance => "Appearance",
            SettingsSection::Notifications => "Notifications",
            SettingsSection::ApiKeys => "API keys",
            SettingsSection::Shortcuts => "Keyboard shortcuts",
            SettingsSection::Account => "Account",
            SettingsSection::LocalProviders => "Local providers",
            SettingsSection::GatewayLink(dest) => dest.title(),
            SettingsSection::Page(id) => id.label(),
        }
    }
}

/// What Settings shows.
pub(super) struct SettingsState {
    pub notifications: ChannelPreferences,
    /// "Your sessions" (`notify.rs`), as `settings.json` last said, and why a change did not save.
    pub session_notifications: bool,
    pub session_note: Option<SharedString>,
    /// "Your organization" (`path_notices.rs`), as `settings.json` last said, and why a change did not save.
    pub path_notifications: bool,
    pub path_note: Option<SharedString>,
    pub tokens: Option<Result<Vec<ApiToken>, SharedString>>,
    pub token_label: Entity<InputState>,
    /// The key just made, shown once until the person leaves the section.
    pub new_token: Option<NewToken>,
    pub token_note: Option<SharedString>,
    pub creating: bool,
    /// The key whose Revoke was pressed once, waiting for the second press.
    pub confirm_revoke: Option<String>,
    /// When Copy was last pressed on the new key's secret: its check shows for a moment (`api_keys_view.rs`).
    pub copied: Option<std::time::Instant>,
    /// The install ID usage data had, shown once after it was turned off (PRIVACY.md, "The install ID").
    pub last_install_id: Option<String>,
    /// The signed-in apps opened to show each of their sign-ins, by `api_keys_view::App::key`.
    pub open_apps: std::collections::HashSet<String>,
    pub scroll: ScrollHandle,
    pub shortcuts: Entity<super::shortcuts_page::ShortcutsPage>,
    /// The nav, search, stacked pages and row feedback (`settings_shell.rs`).
    pub shell: Shell,
    /// Transparency's Fine-tune: open or not, and its sliders (`transparency_tune.rs`).
    pub tune: transparency_tune::Tune,
    _subscription: Subscription,
}

impl SettingsState {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> SettingsState {
        let token_label =
            cx.new(|cx| InputState::new(window, cx).placeholder("Name, such as \u{201c}CI deploy bot\u{201d}"));
        let subscription = cx.subscribe_in(&token_label, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.create_token(window, cx);
            }
        });
        let scroll = ScrollHandle::new();
        let shell = Shell::new(window, scroll.clone(), cx);
        SettingsState {
            notifications: ChannelPreferences::default(),
            session_notifications: true,
            session_note: None,
            path_notifications: true,
            path_note: None,
            tokens: None,
            token_label,
            new_token: None,
            token_note: None,
            creating: false,
            confirm_revoke: None,
            copied: None,
            last_install_id: None,
            open_apps: Default::default(),
            scroll,
            shortcuts: cx.new(|cx| super::shortcuts_page::ShortcutsPage::new(window, cx)),
            shell,
            tune: Default::default(),
            _subscription: subscription,
        }
    }

    pub(super) fn clear_account(&mut self) {
        self.shell.account_ctx = None;
        self.shell.views.retain(|(section, _)| {
            matches!(section, super::SettingsSection::LocalProviders)
                || matches!(section, super::SettingsSection::Page(id) if matches!(id.builder(), crate::pages::Builder::Device(_)))
        });
        self.shell.status.clear();
        self.shell.fields.clear();
        self.notifications = ChannelPreferences::default();
        self.tokens = None;
        self.new_token = None;
        self.token_note = None;
        self.creating = false;
        self.confirm_revoke = None;
        self.copied = None;
        self.open_apps.clear();
    }
}

/// The theme saved on this computer; System when none is.
pub(super) fn saved_theme() -> ThemeChoice {
    ThemeChoice::saved()
}

impl Workspace {
    /// Pick the theme and keep it for the next launch.
    pub(super) fn set_theme(&mut self, choice: ThemeChoice, cx: &mut Context<Self>) {
        self.theme = choice;
        let theme = match choice {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        };
        // The theme applies at once. A failed save says so: the next launch opens with the saved one.
        let status = match crate::paths::save_app_settings(&crate::paths::AppSettings {
            theme: Some(theme.into()),
        }) {
            Ok(()) => RowStatus::Saved(std::time::Instant::now()),
            Err(error) => RowStatus::Failed(format!("Applied until Nexus quits, but not kept. {error}").into()),
        };
        self.set_row_status("theme", status, cx);
    }

    /// Pick a Transparency preset (`transparency_tune::set_frost`).
    pub(super) fn set_transparency(&mut self, level: crate::frost::Transparency, cx: &mut Context<Self>) {
        self.set_frost(level.frost(), cx);
    }

    pub(super) fn channels_loaded(&mut self, channels: Result<Vec<ChannelSetting>, String>, cx: &mut Context<Self>) {
        if matches!(self.settings.shell.status.get("channels"), Some(RowStatus::Saving)) {
            let status = match &channels {
                Ok(_) => RowStatus::Saved(std::time::Instant::now()),
                Err(error) => RowStatus::Failed(format!("Not saved: {error}").into()),
            };
            self.set_row_status("channels", status, cx);
        }
        self.settings.notifications.finish(channels);
    }

    pub(super) fn refresh_channels(&mut self, cx: &mut Context<Self>) {
        if let Some(account) = self.account_gateway()
            && self.settings.notifications.begin_read()
        {
            account.load_notification_settings();
            cx.notify();
        }
    }

    pub(super) fn tokens_loaded(&mut self, tokens: Result<Vec<ApiToken>, String>) {
        self.settings.confirm_revoke = None;
        match tokens {
            Ok(tokens) => self.settings.tokens = Some(Ok(tokens)),
            Err(error) if matches!(self.settings.tokens, Some(Ok(_))) => {
                self.settings.token_note = Some(error.into());
            }
            Err(error) => self.settings.tokens = Some(Err(error.into())),
        }
    }

    pub(super) fn token_created(
        &mut self,
        made: Result<NewToken, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.creating = false;
        match made {
            Ok(made) => {
                // A one-time secret belongs only on the API keys section, including a late response.
                self.settings.new_token = matches!(self.view, View::Settings(SettingsSection::ApiKeys)).then_some(made);
                self.settings.token_note = None;
                self.settings.copied = None;
                self.settings
                    .token_label
                    .update(cx, |state, cx| state.set_value("", window, cx));
            }
            Err(error) => self.settings.token_note = Some(format!("The key was not made: {error}").into()),
        }
    }

    pub(super) fn create_token(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if self.settings.creating {
            return;
        }
        let label = self.settings.token_label.read(cx).value().trim().to_string();
        self.settings.creating = true;
        self.settings.token_note = None;
        self.settings.new_token = None;
        account_gateway.create_token(label);
        cx.notify();
    }

    fn toggle_channel(&mut self, group: &str, channel: &str, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(on) = self.settings.notifications.begin_change(group, channel) else {
            return;
        };
        account_gateway.set_notification_setting(group.to_string(), channel.to_string(), on);
        self.set_row_status("channels", RowStatus::Saving, cx);
    }

    pub(super) fn render_appearance(
        &self,
        account: Option<&crate::pages::Ctx>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .child(ui::lead(tr("How Nexus looks, and what it sends when something breaks."), palette).mb(px(0.)))
            .child(ui::section_head(tr("Theme"), palette))
            .child(note(
                tr("The theme, text size and motion are kept on this computer."),
                palette,
            ))
            .child(self.settings.shell.fields.field(
                "theme",
                theme_cards::render(self.theme, palette, &self.settings.shell.fields, cx),
                None,
                cx,
            ))
            .children(self.row_status_tag("theme", palette).map(|tag| tag.mt(px(8.))))
            .children(self.row_failure("theme").map(|why| error_line(why, palette).mt(px(8.))))
            .child(
                self.settings
                    .shell
                    .fields
                    .field("language", self.render_language(palette, cx), None, cx),
            )
            .child(
                self.settings
                    .shell
                    .fields
                    .field("transparency", self.render_transparency(palette, cx), None, cx),
            )
            .child(
                self.settings
                    .shell
                    .fields
                    .field("text-size", self.render_text_size(palette, cx), None, cx),
            )
            .child(
                self.settings
                    .shell
                    .fields
                    .field("reduce-motion", self.render_reduce_motion(palette, cx), None, cx),
            )
            .child(self.settings.shell.fields.field(
                "assistant-sounds",
                self.render_assistant_sounds(palette, cx),
                None,
                cx,
            ))
            .child(self.render_chat_width(palette, cx))
            .child(self.render_privacy(account, palette, cx))
    }

    /// Language (`i18n.rs`): the interface's language, or the computer's. Kept on this computer, in `settings.json`.
    /// Each language shows its own name, so a person who cannot read the current one still finds theirs.
    fn render_language(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let chosen = crate::i18n::chosen();
        let choices = std::iter::once((None, tr("Same as this computer"))).chain(
            crate::i18n::LANGUAGES
                .iter()
                .map(|&(code, name)| (Some(code), name.into())),
        );
        let chips = choices.enumerate().map(|(index, (code, name))| {
            ui::chip(("settings-language", index), name, chosen.as_deref() == code, palette).on_click(cx.listener(
                move |this, _, _, cx| {
                    let status = match crate::i18n::set(code, cx) {
                        Ok(()) => RowStatus::Saved(std::time::Instant::now()),
                        Err(error) => RowStatus::Failed(format!("{} {error}", tr("Not changed.")).into()),
                    };
                    this.set_row_status("language", status, cx);
                },
            ))
        });
        v_flex()
            .pt(px(14.))
            .gap(px(10.))
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).child(tr("Language")))
                    .child(note(
                        tr("The language of menus, buttons and settings. Agents reply in the language you write in."),
                        palette,
                    )),
            )
            .child(h_flex().flex_wrap().gap(px(8.)).children(chips))
            .children(self.row_status_tag("language", palette))
            .children(self.row_failure("language").map(|why| error_line(why, palette)))
    }

    /// Text size: the Nexus app's zoom, 50% to 200%, shown as the body text's size (`zoom.rs`).
    /// Transparency (`frost.rs`): how much of the desktop shows through the window, under the theme.
    fn render_transparency(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let detail = if crate::frost::system_solid() {
            tr(
                "How much of your desktop shows through the window, blurred. Your system's reduce transparency setting \
                keeps the window solid.",
            )
        } else if crate::frost::can_blur() {
            tr("How much of your desktop shows through the window, blurred.")
        } else {
            tr(
                "How much of your desktop shows through the window, blurred. This desktop can't blur behind windows, \
                so Nexus stays opaque here.",
            )
        };
        v_flex()
            .pt(px(14.))
            .gap(px(10.))
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).child(tr("Transparency")))
                    .child(note(detail, palette)),
            )
            .child(theme_cards::render_transparency(
                crate::frost::chosen().preset(),
                palette,
                &self.settings.shell.fields,
                cx,
            ))
            .child(self.render_tune("settings", palette, cx))
            .children(self.row_status_tag("transparency", palette))
            .children(self.row_failure("transparency").map(|why| error_line(why, palette)))
    }

    fn render_text_size(&self, palette: Palette, _cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let zoom = gpui::ui_zoom();
        let step = |id: &'static str, label: &'static str, active: bool, by: f32| {
            div()
                .id(id)
                .flex_none()
                .w(px(28.))
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .bg(palette.hover)
                .text_size(px(15.))
                .when(active, |button| {
                    ui::focusable(button, palette)
                        .text_color(palette.strong)
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.selected))
                        .on_click(move |_, _, cx| crate::zoom::set(gpui::ui_zoom() + by, cx))
                })
                .when(!active, |button| button.text_color(palette.faint))
                .child(label)
        };
        h_flex()
            .pt(px(14.))
            .gap(px(16.))
            .child(
                v_flex()
                    .flex_1()
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).child(tr("Text size")))
                    .child(note(tr("Scales the app. Code and terminal follow it."), palette)),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .when(zoom != 1., |row| {
                        row.child(
                            ui::ghost_button("settings-text-reset", tr("Reset"), palette)
                                .on_click(|_, _, cx| crate::zoom::set(1., cx)),
                        )
                    })
                    .child(step("settings-text-smaller", "−", zoom > crate::zoom::MIN, -0.1))
                    .child(
                        div()
                            .w(px(48.))
                            .text_center()
                            .text_size(px(13.))
                            .text_color(palette.strong)
                            .child(crate::zoom::label(zoom)),
                    )
                    .child(step("settings-text-larger", "+", zoom < crate::zoom::MAX, 0.1)),
            )
    }

    /// Reduce motion: every animation shows its end, and a repeating one holds still (`zoom.rs`). The switch is the
    /// app's own; the system's setting applies too, as the Nexus app says.
    fn render_reduce_motion(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let on = crate::paths::reduce_motion();
        let detail = if crate::zoom::system_reduces_motion() {
            tr("Shows things in place, with no slides or breathing dots. Your system asks for it too.")
        } else {
            tr("Shows things in place, with no slides or breathing dots. Off, your system's setting still applies.")
        };
        let row = h_flex()
            .pt(px(14.))
            .gap(px(16.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).child(tr("Reduce motion")))
                    .child(note(detail, palette)),
            )
            .children(self.row_status_tag("reduce-motion", palette))
            .child(
                ui::switch("settings-reduce-motion", on, false, palette).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let status = match crate::zoom::set_reduce_motion(!on, cx) {
                            Ok(()) => RowStatus::Saved(std::time::Instant::now()),
                            Err(error) => RowStatus::Failed(format!("{} {error}", tr("Not changed.")).into()),
                        };
                        this.set_row_status("reduce-motion", status, cx);
                    },
                )),
            );
        v_flex().child(row).children(
            self.row_failure("reduce-motion")
                .map(|why| error_line(why, palette).mt(px(6.))),
        )
    }

    /// Assistant sounds: the soft sounds when you send and when Nexus replies (`sounds.rs`). Kept on this computer.
    fn render_assistant_sounds(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let on = crate::paths::assistant_sounds();
        let row = h_flex()
            .pt(px(14.))
            .gap(px(16.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).child(tr("Assistant sounds")))
                    .child(note(
                        tr(
                            "A soft sound when you send and when Nexus replies, while Nexus is in front. Quiet under \
                            Reduce motion.",
                        ),
                        palette,
                    )),
            )
            .children(self.row_status_tag("assistant-sounds", palette))
            .child(
                ui::switch("settings-assistant-sounds", on, false, palette).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let status = match crate::paths::save_assistant_sounds(!on) {
                            Ok(()) => RowStatus::Saved(std::time::Instant::now()),
                            Err(error) => RowStatus::Failed(format!("{} {error}", tr("Not changed.")).into()),
                        };
                        this.set_row_status("assistant-sounds", status, cx);
                    },
                )),
            );
        v_flex().child(row).children(
            self.row_failure("assistant-sounds")
                .map(|why| error_line(why, palette).mt(px(6.))),
        )
    }

    /// Privacy: "Send usage data" (off until chosen, PRIVACY.md) and "Send crash and error reports", as
    /// Nexus UI's Appearance page has it. Both kept on this computer.
    fn render_privacy(
        &self,
        account: Option<&crate::pages::Ctx>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let on = crate::paths::error_reports();
        let usage = crate::paths::usage_data() == Some(true);
        v_flex()
            .child(ui::section_head("Privacy", palette))
            .child(
                setting_row(
                    "Send usage data",
                    "How often Nexus is used, its version and errors by name, never your chats, code, files or keys. \
                     PRIVACY.md lists every field.",
                    palette,
                )
                .children(self.row_status_tag("usage-data", palette))
                .child(
                    ui::switch("settings-usage-data", usage, false, palette).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let status = match crate::paths::save_usage_data(!usage) {
                                Ok(forgotten) => {
                                    this.settings.last_install_id = forgotten;
                                    RowStatus::Saved(std::time::Instant::now())
                                }
                                Err(error) => RowStatus::Failed(format!("{} {error}", tr("Not changed.")).into()),
                            };
                            this.set_row_status("usage-data", status, cx);
                        },
                    )),
                ),
            )
            .children(self.row_failure("usage-data").map(|why| error_line(why, palette)))
            .children(crate::paths::install_id().map(|id| {
                install_id_line(
                    format!("{} {id}", tr("Install ID:")),
                    id,
                    "settings-copy-install-id",
                    palette,
                    cx,
                )
            }))
            .children(self.settings.last_install_id.clone().map(|id| {
                install_id_line(
                    format!(
                        "{} {id}",
                        tr("Install ID, now deleted. Copy it to ask privacy@nexus.inc to delete its data:")
                    ),
                    id,
                    "settings-copy-last-install-id",
                    palette,
                    cx,
                )
            }))
            .child(
                setting_row(
                    "Send crash and error reports",
                    "When Nexus crashes, it sends what failed and where in its code, never your chats, code or keys, \
                     with your name and email. Off, it asks after each crash.",
                    palette,
                )
                .children(self.row_status_tag("error-reports", palette))
                .child(
                    ui::switch("settings-error-reports", on, false, palette).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let status = match crate::paths::save_error_reports(!on) {
                                Ok(()) => RowStatus::Saved(std::time::Instant::now()),
                                Err(error) => RowStatus::Failed(format!("{} {error}", tr("Not changed.")).into()),
                            };
                            this.set_row_status("error-reports", status, cx);
                        },
                    )),
                ),
            )
            .children(self.row_failure("error-reports").map(|why| error_line(why, palette)))
            .children(self.render_presence_preference(account, palette, cx).map(|body| {
                self.settings
                    .shell
                    .fields
                    .field(super::team_presence::SHOW_ONLINE.0, body, None, cx)
            }))
    }

    /// "Your sessions": the Nexus app's `sessionNotifications`, written only while this build owns the store.
    fn toggle_session_notifications(&mut self, cx: &mut Context<Self>) {
        if !crate::desktop_settings::writable() {
            return;
        }
        let on = !self.settings.session_notifications;
        match save_session_notifications(on) {
            Ok(()) => {
                self.settings.session_notifications = on;
                self.settings.session_note = None;
                self.set_row_status("session-notifications", RowStatus::Saved(std::time::Instant::now()), cx);
            }
            // The switch keeps the saved value; the line says why the change was not kept.
            Err(error) => self.settings.session_note = Some(error.into()),
        }
        cx.notify();
    }

    /// "Your organization": the Nexus app's `pathNotifications`, a mute for this computer only, written only
    /// while this build owns the store.
    fn toggle_path_notifications(&mut self, cx: &mut Context<Self>) {
        if !crate::desktop_settings::writable() {
            return;
        }
        let on = !self.settings.path_notifications;
        match save_path_notifications(on) {
            Ok(()) => {
                self.settings.path_notifications = on;
                self.settings.path_note = None;
                self.set_row_status("path-notifications", RowStatus::Saved(std::time::Instant::now()), cx);
            }
            Err(error) => self.settings.path_note = Some(error.into()),
        }
        cx.notify();
    }

    /// "On this computer": the two switches `settings.json` keeps, in one card. While the Nexus app runs they are
    /// read-only, with the reason rather than a dead switch.
    fn render_this_computer(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let locked = !crate::desktop_settings::writable();
        let session = self.computer_row(
            "session-notifications",
            "Your sessions",
            "A session finishes, needs approval or asks a question. Not while you're looking at it.",
            self.settings.session_notifications,
            locked,
            palette,
            cx.listener(|this, _, _, cx| this.toggle_session_notifications(cx)),
        );
        let organization = self.computer_row(
            "path-notifications",
            "Your organization",
            "Spending limits, access requests, billing and security, as system notifications.",
            self.settings.path_notifications,
            locked,
            palette,
            cx.listener(|this, _, _, cx| this.toggle_path_notifications(cx)),
        );
        v_flex()
            .child(card_head("On this computer", Lucide::Laptop, "Saved on this computer", palette))
            .when(locked, |column| {
                column.child(
                    note(
                        "Read-only while the Nexus app runs: it owns these two. Take over in Computer to change them here.",
                        palette,
                    )
                    .mb(px(8.)),
                )
            })
            .child(ui::card_frame(palette).child(session).child(ui::card_rule(palette)).child(organization))
            .when_some(self.settings.session_note.clone(), |column, error| {
                column.child(error_line(error, palette).mt(px(8.)))
            })
            .when_some(self.settings.path_note.clone(), |column, error| {
                column.child(error_line(error, palette).mt(px(8.)))
            })
    }

    /// One switch of "On this computer": its name and what it covers, its save status and the switch.
    #[allow(clippy::too_many_arguments)]
    fn computer_row(
        &self,
        anchor: &'static str,
        title: &'static str,
        detail: &'static str,
        on: bool,
        locked: bool,
        palette: Palette,
        toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Div {
        ui::card_row()
            .child(ui::setting_lines(title, detail, palette).flex_1())
            .children(self.row_status_tag(anchor, palette))
            .child(
                ui::switch(SharedString::from(format!("settings-{anchor}")), on, locked, palette)
                    .when(!locked, |switch| switch.on_click(toggle)),
            )
    }

    /// "Email and phone": each group's Email and Push, as Gateway keeps them for the account. The Inbox always gets
    /// every group, so it has no column; a group no channel carries (the person's own sessions) has no row.
    fn render_channels(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let head = h_flex().gap(px(8.)).child(div().flex_1().child(card_head(
            "Email and phone",
            Lucide::Cloud,
            "Saved to your account",
            palette,
        )));
        if self.is_local() {
            let cell_width = px(52.);
            let mut card = ui::card_frame(palette).child(
                ui::card_row()
                    .child(div().flex_1().child(ui::requires_sign_in(palette)))
                    .children(CHANNELS.map(|(_, label, _)| {
                        div()
                            .w(cell_width)
                            .text_center()
                            .child(ui::setting_detail(label, palette))
                    })),
            );
            for (group, label) in GROUPS.into_iter().filter(|(group, _)| *group != "my_sessions") {
                card = card.child(ui::card_rule(palette)).child(
                    ui::card_row()
                        .child(
                            ui::setting_lines(label, group_detail(group), palette)
                                .flex_1()
                                .opacity(0.7),
                        )
                        .children(CHANNELS.map(|(channel, _, _)| {
                            h_flex().w(cell_width).justify_center().child(ui::switch(
                                SharedString::from(format!("notify-{group}-{channel}")),
                                false,
                                true,
                                palette,
                            ))
                        })),
                );
            }
            return v_flex().child(head).child(card);
        }
        let column = v_flex()
            .child(head)
            .when_some(self.settings.notifications.note.clone(), |column, error| {
                column.child(error_line(error.into(), palette).mb(px(8.)))
            });
        let channels = match &self.settings.notifications.channels {
            // Read when the page opens: rows of grey bars hold the card's shape until Gateway answers.
            None => {
                return column.child(
                    ui::card_frame(palette)
                        .px(px(16.))
                        .py(px(4.))
                        .id("settings-email-phone-loading")
                        .debug_selector(|| "settings-email-phone-loading".into())
                        .child(ui::skeleton(4, palette)),
                );
            }
            Some(Err(error)) => {
                let retry = if self.settings.notifications.pending {
                    ui::disabled_button("settings-retry-channels", "Trying…", palette)
                } else {
                    ui::button("settings-retry-channels", "Try again", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_channels(cx)))
                };
                return column.child(
                    ui::card_row()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(palette.border)
                        .child(error_line(format!("Couldn't read your settings: {error}").into(), palette).flex_1())
                        .child(retry),
                );
            }
            Some(Ok(channels)) => channels,
        };
        let available = |group: &str, channel: &str| {
            channels
                .iter()
                .find(|setting| setting.group == group && setting.channel == channel)
                .filter(|setting| setting.available)
        };
        // A channel no group reaches yet (push, before a phone is set up) gets no column, rather than a column of
        // dashes.
        let shown: Vec<(&'static str, &'static str, Lucide)> = CHANNELS
            .into_iter()
            .filter(|(channel, ..)| GROUPS.iter().any(|(group, _)| available(group, channel).is_some()))
            .collect();
        let groups: Vec<(&'static str, &'static str)> = GROUPS
            .into_iter()
            .filter(|(group, _)| shown.iter().any(|(channel, ..)| available(group, channel).is_some()))
            .collect();
        let pending = self.settings.notifications.pending;
        let mut card = ui::card_frame(palette).child(
            h_flex()
                .h(px(36.))
                .px(px(16.))
                .border_b_1()
                .border_color(palette.border)
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(
                    h_flex()
                        .flex_1()
                        .gap(px(6.))
                        .child(glyph(Lucide::Inbox, palette.faint))
                        .child("The Inbox gets every topic")
                        .children(self.row_status_tag("channels", palette).map(|tag| tag.ml(px(8.)))),
                )
                .children(shown.iter().map(|(_, label, icon)| {
                    h_flex()
                        .w(px(CELL))
                        .flex_none()
                        .justify_center()
                        .gap(px(6.))
                        .child(glyph(*icon, palette.faint))
                        .child(*label)
                })),
        );
        for (ix, (group, label)) in groups.iter().enumerate() {
            let group: &'static str = group;
            let row = ui::card_row().child(ui::setting_lines(*label, group_detail(group), palette).flex_1());
            let row = row.children(shown.iter().map(|(channel, ..)| {
                let channel: &'static str = channel;
                let cell = h_flex().w(px(CELL)).flex_none().justify_center();
                match available(group, channel) {
                    // A required notice's channel: its state and a lock, not a greyed switch that reads as off.
                    Some(setting) if setting.locked => {
                        cell.child(locked_state(format!("locked-{group}-{channel}"), setting.on, palette))
                    }
                    Some(setting) => cell.child(
                        ui::switch(
                            SharedString::from(format!("notify-{group}-{channel}")),
                            setting.on,
                            pending,
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_channel(group, channel, cx))),
                    ),
                    None => cell.child(div().text_color(palette.faint).child("–")),
                }
            }));
            if ix > 0 {
                card = card.child(ui::card_rule(palette));
            }
            card = card.child(row);
        }
        column.when(!groups.is_empty(), |column| column.child(card))
    }

    pub(super) fn render_notifications(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .min_w(px(0.))
            .child(ui::lead("Choose what reaches you, and where.", palette).mb(px(0.)))
            .child(self.render_this_computer(palette, cx))
            .child(self.render_channels(palette, cx))
    }

    pub(super) fn render_account(
        &self,
        ctx: Option<&crate::pages::Ctx>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let account = self.account.as_ref().filter(|_| ctx.is_some());
        let or_dash = |value: Option<&String>| value.filter(|value| !value.is_empty()).cloned().unwrap_or("–".into());
        let name = or_dash(account.map(|account| &account.name));
        let email = or_dash(account.map(|account| &account.email));
        // The sidebar's initials, so the avatar here matches the one in the nav.
        let initials = account.map_or_else(|| "?".into(), |account| super::initials(&account.name));
        let avatar = div()
            .flex()
            .flex_none()
            .size(px(44.))
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(ui::thumb(palette))
            .text_size(px(16.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(palette.strong)
            .child(initials);
        let who = v_flex()
            .flex_1()
            .min_w(px(0.))
            .gap(px(2.))
            .child(
                div()
                    .truncate()
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child(name),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .child(email),
            );
        let sign_out = if ctx.is_none() {
            ui::disabled_button("settings-sign-out", "Sign out", palette)
        } else {
            ui::danger_button("settings-sign-out", "Sign out", palette).on_click(cx.listener(|this, _, _, cx| {
                this.request_sign_out();
                cx.notify();
            }))
        };
        // The organization and the role are the sidebar's group head, so the card names only the Gateway.
        let gateway = h_flex()
            .min_h(px(40.))
            .px(px(16.))
            .gap(px(16.))
            .border_t_1()
            .border_color(palette.border)
            .text_size(px(13.))
            .child(div().flex_none().text_color(palette.muted).child("Gateway"))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_right()
                    .font_family(ui::MONO)
                    .text_size(px(12.))
                    .text_color(palette.strong)
                    .child(ctx.map_or_else(|| "–".into(), |ctx| gateway_label(ctx.gateway.url()))),
            );
        v_flex()
            .child(
                v_flex()
                    .rounded(px(12.))
                    .bg(palette.raised)
                    .shadow(ui::lift(palette))
                    .child(
                        h_flex()
                            .gap(px(14.))
                            .p(px(16.))
                            .child(avatar)
                            .child(who)
                            .child(sign_out),
                    )
                    .child(gateway),
            )
            .child(
                ui::setting_detail("Signing out also signs out the Nexus app on this computer.", palette)
                    .mt(px(8.))
                    .px(px(16.)),
            )
    }
}

/// The Gateway's address as the Account card shows it: no `https://` and no trailing slash. Any other scheme stays,
/// so a plain `http://` Gateway still reads as one.
fn gateway_label(url: &str) -> String {
    let url = url.trim_end_matches('/');
    url.strip_prefix("https://").unwrap_or(url).to_string()
}

/// Saves the setting; refused while the Nexus app owns the store (`desktop_settings::update`).
fn save_session_notifications(on: bool) -> Result<(), String> {
    crate::desktop_settings::update(|settings| {
        settings.session_notifications = Some(on);
        Ok(())
    })
    .map(|_| ())
}

/// Saves the setting; refused while the Nexus app owns the store (`desktop_settings::update`).
fn save_path_notifications(on: bool) -> Result<(), String> {
    crate::desktop_settings::update(|settings| {
        settings.path_notifications = Some(on);
        Ok(())
    })
    .map(|_| ())
}

/// A setting's row: its name and what it does, wrapping in the column; the caller adds the control on the right.
pub(super) fn setting_row(title: &'static str, detail: &'static str, palette: Palette) -> Div {
    h_flex()
        .gap(px(24.))
        .py(px(8.))
        .child(ui::setting_lines(title, detail, palette).flex_1())
}

/// What a notification group carries, under its name (`nexus-base`'s `notify.rs` catalogue, by group).
fn group_detail(group: &str) -> &'static str {
    match group {
        "usage_and_billing" => "Spending limits, payments, plan changes, the month's report",
        "requests_and_approvals" => "Access requests, new members, skill promotions",
        "automations_and_integrations" => "A provider key is rejected, or a connector disconnects",
        "account_and_security" => "New keys, members and sign-ins",
        _ => "",
    }
}

/// A card's heading, with where its settings are kept on the right.
fn card_head(title: &'static str, icon: Lucide, place: &'static str, palette: Palette) -> Div {
    h_flex()
        .mt(px(24.))
        .mb(px(8.))
        .gap(px(12.))
        .child(
            div()
                .flex_1()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.muted)
                .child(title),
        )
        .child(
            h_flex()
                .flex_none()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(glyph(icon, palette.faint))
                .child(place),
        )
}

/// A channel nobody can change here: its state beside a lock, and why on hover.
fn locked_state(id: String, on: bool, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(SharedString::from(id))
        .gap(px(4.))
        .px(px(7.))
        .py(px(2.))
        .rounded(px(6.))
        .bg(palette.hover)
        .text_size(px(12.))
        .text_color(palette.faint)
        .child(glyph(Lucide::Lock, palette.faint).size(px(12.)))
        .child(if on { "On" } else { "Off" })
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "Nexus always sends this group's required notices, such as a security change",
            )
            .build(window, cx)
        })
}

/// The install ID under "Send usage data", with Copy: a person quotes it to have their data deleted (PRIVACY.md, "Your
/// choices").
fn install_id_line(
    text: String,
    id: String,
    button: &'static str,
    palette: Palette,
    cx: &mut Context<Workspace>,
) -> Div {
    h_flex()
        .gap(px(12.))
        .pb(px(8.))
        .child(note(text, palette).flex_1())
        .child(
            ui::button(button, "Copy", palette).on_click(cx.listener(move |_, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
            })),
        )
}

/// A setting's explanation. Takes a translated text (`i18n::tr`) as well as a literal.
pub(super) fn note(text: impl Into<SharedString>, palette: Palette) -> Div {
    let text: SharedString = text.into();
    div()
        .w_full()
        .min_w(px(0.))
        .whitespace_normal()
        .text_size(px(13.))
        .text_color(palette.faint)
        .child(text)
}

pub(super) fn error_line(text: SharedString, palette: Palette) -> Div {
    div().text_size(px(13.)).text_color(palette.destructive).child(text)
}

#[cfg(test)]
mod tests {
    use super::gateway_label;

    #[test]
    fn the_gateway_label_drops_only_https() {
        assert_eq!(gateway_label("https://api.nexus.inc/"), "api.nexus.inc");
        assert_eq!(gateway_label("http://localhost:8080"), "http://localhost:8080");
    }
}
