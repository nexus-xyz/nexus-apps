//! Chat uses a team for spend and follows the organization's web search setting.
//! The team chip appears in the panel and on a new chat in the app window.

use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::chat::{current_team, web_search_sync};
use serde_json::Value;

use super::code_dock::{menu_box, menu_label};
use super::{Workspace, glyph};
use crate::gateway::{ChatChoices, OpenedChat};
use crate::motion::{self, TweenExt as _};
use crate::palette::Palette;

/// The composer's team and web search, and what a new chat in the app window starts with.
#[derive(Debug, Default)]
pub(super) struct ChoicesState {
    /// The team chosen: `None` for the person's default, `Some("")` for none (an open chat that names no team).
    pub team: Option<String>,
    /// The organization's settings (`GET /v1/orgs/settings`); `None` until they are read.
    pub org_settings: Option<Value>,
    /// The composer's web search flag (`ComposerState.webSearch`), kept by [`web_search_sync`].
    pub web_search: Option<bool>,
    /// The open session's own `webSearch`, as Gateway has it; `None` for a chat not made yet.
    pub session_web_search: Option<bool>,
    /// The last team selected for a new chat in the app window.
    pub home_team: Option<String>,
}

/// Whether a team chip shows: in the panel, or on a new chat in the app window, and only with teams to choose.
pub(super) fn shows_team_chip(panel: bool, open_chat: bool, teams: usize) -> bool {
    (panel || !open_chat) && teams > 0
}

/// The team a question goes with: the chosen one; the default when none was chosen; none for an open chat that names
/// none (Nexus UI sends `team` only when it has one).
pub(super) fn send_team(chosen: Option<&str>, default: Option<&str>) -> Option<String> {
    match chosen {
        None => default.map(String::from),
        Some("") => None,
        Some(team) => Some(team.to_string()),
    }
}

/// What a question sends for web search, and the flag after it. The composers that sync the flag (`syncs`: the panel,
/// and a new chat in the app window) take [`web_search_sync`]'s value; an open chat in the app window keeps its
/// session's. Returns `(web_search, patch, flag)`: `patch` when an existing session's `webSearch` differs.
pub(super) fn web_search_for(
    syncs: bool,
    flag: Option<bool>,
    session: Option<bool>,
    settings: Option<&Value>,
) -> (bool, bool, Option<bool>) {
    let flag = if syncs {
        web_search_sync(flag, settings).map(Some).unwrap_or(flag)
    } else {
        flag
    };
    let on = if syncs {
        flag.unwrap_or(false)
    } else {
        session.unwrap_or(false)
    };
    let patch = session.is_some_and(|session| session != on);
    (on, patch, flag)
}

impl Workspace {
    /// The app window's new chat, where the team chip shows (Nexus UI's large composer).
    fn chat_home(&self) -> bool {
        !self.panel && self.current.is_none()
    }

    /// The choices the next question goes with. The open session takes the web search it sends.
    pub(super) fn pending_chat_choices(&self, session: Option<bool>) -> (ChatChoices, Option<bool>) {
        let default = self.account.as_ref().and_then(|account| account.default_team.clone());
        let team = send_team(self.choices.team.as_deref(), default.as_deref());
        let (web_search, patch_web_search, flag) = web_search_for(
            self.panel || self.current.is_none(),
            self.choices.web_search,
            session.filter(|_| self.current.is_some()),
            self.choices.org_settings.as_ref(),
        );
        (
            ChatChoices {
                team,
                web_search,
                patch_web_search,
            },
            flag,
        )
    }
    pub(super) fn chat_choices(&mut self) -> ChatChoices {
        let (choices, flag) = self.pending_chat_choices(self.choices.session_web_search);
        self.choices.web_search = flag;
        self.choices.session_web_search = Some(choices.web_search);
        choices
    }

    /// Opening a chat in the app window keeps its team and web search. Move here brings them to the panel.
    pub(super) fn chat_opened(&mut self, chat: &OpenedChat) {
        self.choices.session_web_search = Some(chat.web_search);
        if !self.panel {
            self.choices.team = Some(chat.team.clone().unwrap_or_default());
            self.choices.web_search = Some(chat.web_search);
        } else if self.moving.as_deref() == Some(chat.id.as_str()) {
            // `panel.tsx`'s team is `state.team || default`: a chat that names none goes on with the default.
            self.choices.team = chat.team.clone();
            self.choices.web_search = Some(chat.web_search);
        }
    }

    /// A new chat: the app window's restores its last team.
    pub(super) fn reset_chat_choices(&mut self) {
        self.overlay.close_team_menu();
        self.choices.session_web_search = None;
        if !self.panel {
            self.choices.team = self.choices.home_team.clone();
            self.choices.web_search = None;
        }
    }

    /// The account was read: the organization's settings again, and a choice of team from another organization goes.
    pub(super) fn account_read(&mut self, before: Option<&str>, org: &str) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if before != Some(org) {
            self.choices.team = None;
            self.choices.home_team = None;
            self.choices.org_settings = None;
        }
        account_gateway.load_org_settings();
    }

    fn set_team(&mut self, slug: String, cx: &mut Context<Self>) {
        if self.chat_home() {
            self.choices.home_team = Some(slug.clone());
        }
        self.choices.team = Some(slug);
        self.overlay.close_team_menu();
        self.capture_gateway_choices(cx);
        cx.notify();
    }

    /// The team chip after the model, when teams are available.
    pub(super) fn render_chat_team_chip(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let teams = self.account.as_ref().map_or(0, |account| account.teams.len());
        shows_team_chip(self.panel, self.current.is_some(), teams)
            .then(|| self.render_team_chip(palette, cx).into_any_element())
    }

    /// The team (`.picker-team`): the team's name with a chevron, and its menu, "Spend goes to", the default marked.
    fn render_team_chip(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(account) = self.account.as_ref() else {
            return div().into_any_element();
        };
        let default = account.default_team.as_deref();
        let chosen = self.choices.team.as_deref().unwrap_or("");
        let current = current_team(&account.teams, chosen, default);
        let label: SharedString = current.map_or("Team".into(), |team| team.name.clone().into());
        let open = self.overlay.team_menu();
        let compact = self.composer_layout.chat_icons;
        let full_label: SharedString = format!("Team: {label}").into();
        let chip = h_flex()
            .id("team-chip")
            .role(Role::Button)
            .aria_label(full_label.clone())
            .h(px(26.))
            .px(px(4.))
            .gap(px(5.))
            .rounded(px(6.))
            .text_size(px(13.))
            .text_color(if open { palette.foreground } else { palette.muted })
            .cursor_pointer()
            .hover(|style| style.text_color(palette.foreground))
            .when(!open, |chip| {
                chip.tooltip(move |window, cx| Tooltip::new(full_label.clone()).build(window, cx))
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_overlay(Overlay::TeamMenu);
                this.overlay.close_model_menu();
                cx.notify();
            }))
            .child(Icon::new(Lucide::Users).size(px(14.)).text_color(palette.icon))
            .when(compact, |chip| chip.w(px(28.)).justify_center().px_0())
            .when(!compact, |chip| {
                chip.child(div().max_w(px(140.)).truncate().child(label))
            })
            .when(!compact, |chip| {
                chip.child(
                    Icon::new(Lucide::ChevronDown)
                        .size(px(13.))
                        .text_color(palette.faint)
                        .with_tween(
                            "team-chip-chevron",
                            if open { 1. } else { 0. },
                            motion::Tween::new(motion::COLLAPSE, motion::ease_out),
                            |icon, turn| icon.rotate(radians(std::f32::consts::PI * turn)),
                        ),
                )
            });
        let picked = current.map(|team| team.slug.clone());
        let mut menu = menu_box("team-menu", 220., palette)
            .text_size(px(13.))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.overlay.close_team_menu();
                cx.notify();
            }))
            .child(menu_label("Spend goes to", palette));
        for (ix, team) in account.teams.iter().enumerate() {
            let slug = team.slug.clone();
            let is_default = Some(team.slug.as_str()) == default;
            menu = menu.child(
                h_flex()
                    .id(("team-option", ix))
                    .h(px(30.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_team(slug.clone(), cx)))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(team.name.clone()))
                    .when(is_default, |row| {
                        row.child(div().text_size(px(11.)).text_color(palette.muted).child("default"))
                    })
                    .child(
                        div()
                            .w(px(14.))
                            .when(picked.as_deref() == Some(team.slug.as_str()), |cell| {
                                cell.child(Icon::new(Lucide::Check).size(px(14.)))
                            }),
                    ),
            );
        }
        div()
            .relative()
            .child(chip)
            .when_some(motion::presence("team-menu", open, cx), |frame, phase| {
                let menu = phase.menu("team-menu", 4., menu);
                frame.child(
                    div().absolute().top_0().right_0().child(
                        deferred(
                            anchored()
                                .anchor(Anchor::BottomRight)
                                .offset(point(px(0.), px(-8.)))
                                .snap_to_window_with_margin(px(12.))
                                .child(menu),
                        )
                        .with_priority(1),
                    ),
                )
            })
            .into_any_element()
    }
}

/// "Searching the web…" (`SearchingWeb`), while the answer's `web_search` runs.
pub(super) fn render_searching_web(palette: Palette) -> impl IntoElement {
    h_flex()
        .max_w(px(460.))
        .gap(px(12.))
        .mt(px(8.))
        .py(px(10.))
        .px(px(14.))
        .rounded(px(12.))
        .bg(palette.raised)
        .child(glyph(Lucide::Globe, palette.icon).size(px(18.)))
        .child(
            div()
                .text_size(px(13.))
                .text_color(palette.muted)
                .child("Searching the web…"),
        )
}

#[cfg(test)]
mod tests {
    use super::{send_team, shows_team_chip, web_search_for};
    use serde_json::json;

    #[test]
    fn the_team_chip_shows_in_the_panel_and_on_a_new_chat_only() {
        assert!(shows_team_chip(true, true, 2));
        assert!(shows_team_chip(false, false, 1));
        // An open chat in the app window keeps its team, as `plain-composer.tsx`: no chip.
        assert!(!shows_team_chip(false, true, 2));
        assert!(!shows_team_chip(true, false, 0));
    }

    #[test]
    fn a_question_goes_with_the_chosen_team_else_the_default() {
        assert_eq!(send_team(None, Some("growth")).as_deref(), Some("growth"));
        assert_eq!(send_team(Some("platform"), Some("growth")).as_deref(), Some("platform"));
        assert_eq!(send_team(Some(""), Some("growth")), None);
        assert_eq!(send_team(None, None), None);
    }

    #[test]
    fn web_search_follows_the_organization_in_a_composer_that_syncs() {
        let offered = json!({ "webSearch.engine": "borrowed" });
        let off = json!({ "webSearch.engine": "off" });
        // A new chat in Chat, search offered: the session is made with it on.
        assert_eq!(
            web_search_for(true, None, None, Some(&offered)),
            (true, false, Some(true))
        );
        // The panel's session had it on, and the organization turned it off: set it off first.
        assert_eq!(
            web_search_for(true, Some(true), Some(true), Some(&off)),
            (false, true, Some(false))
        );
        // Settings not read yet: the flag stays as it is.
        assert_eq!(
            web_search_for(true, Some(true), Some(true), None),
            (true, false, Some(true))
        );
    }

    #[test]
    fn an_open_chat_in_the_app_window_keeps_its_sessions_web_search() {
        let offered = json!({ "webSearch.engine": "borrowed" });
        assert_eq!(
            web_search_for(false, Some(false), Some(false), Some(&offered)),
            (false, false, Some(false))
        );
        assert_eq!(
            web_search_for(false, Some(true), Some(true), Some(&offered)),
            (true, false, Some(true))
        );
    }
}
