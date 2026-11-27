//! The sidebar, as the Nexus app draws it (Nexus UI's `shell.tsx` with the desktop's `DesktopSidebar`, v5 tokens in
//! `ui/src/app/app/v5/shell.css`): the organization, the places (the Assistant first with an agent API, then New
//! session, Inbox and Artifacts), the Sessions and Projects groups (`sidebar_groups.rs`), and the person at the foot.
//! It is a card on the window's ground, 8 px in from the window's edges (Frost, `frost.rs`), with no border, and slides
//! shut and open (Cmd+B, or Ctrl+B). A hairline sets the groups off from the places only while the groups are
//! scrolled, as Finder and Notes do, so no row is cut against "Artifacts".

use crate::workspace::MenuTrigger;
use crate::workspace::menu_item_hover;
use crate::workspace::overlay::{Overlay, OverlaySlot};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::code_dock::menu_box;
use super::{SettingsSection, ThemeChoice, View, Workspace, glyph, initials};
use crate::code::{self, Status};
use crate::i18n::tr;
use crate::motion;
use crate::palette::Palette;

#[cfg(test)]
#[path = "session_name_tests.rs"]
mod session_name_tests;

/// Rows: 32px high, 13.5px text (`.nav-link`, `.wb-thread`).
pub(super) const ROW: f32 = 32.;
pub(super) const ROW_TEXT: f32 = 13.5;

/// The new-chat shortcut as this platform writes it.
const NEW_HINT: &str = if cfg!(target_os = "macos") { "⌘ N" } else { "Ctrl N" };
/// The Assistant's shortcut as this platform writes it (`main.rs` binds it to `OpenAssistant`).
pub(super) const ASSISTANT_HINT: &str = if cfg!(target_os = "macos") { "⌘ J" } else { "Ctrl J" };
/// A new errand's shortcut (`main.rs` binds it to `NewErrand`).
pub(super) const NEW_ERRAND_HINT: &str = if cfg!(target_os = "macos") {
    "⇧ ⌘ J"
} else {
    "Ctrl Shift J"
};

impl Workspace {
    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = self.sidebar_open;
        let sidebar_width = self.sidebar_width();
        // The panel keeps its width inside and is clipped by its frame, so the text does not rewrap while it moves.
        // It is a cached view (`pane.rs`): a page's frame does not lay it out again. It lies on the card
        // (`render_sidebar_card`), inset from the window's left and bottom, with a gap to the main pane.
        let frame = div()
            .id("sidebar-frame")
            .relative()
            .h_full()
            .flex_none()
            .overflow_hidden()
            .pl(px(crate::frost::INSET))
            .pr(px(crate::frost::INSET))
            .pb(px(crate::frost::INSET))
            .children(self.panes.as_ref().map(|(sidebar, _)| {
                let mut style = StyleRefinement::default();
                style.size.width = Some(px(sidebar_width).into());
                style.size.height = Some(relative(1.).into());
                sidebar.clone().cached(style)
            }))
            // A running or waiting session's mark moves over the pane, so the pane stays as it was drawn.
            .when(self.panes.is_some(), |frame| {
                // Over the marks: the cue to a waiting session out of the list's sight (`sidebar_edge_cue.rs`).
                frame
                    .child(self.sidebar_marks.overlay())
                    .child(self.sidebar_edge_cue.clone())
            })
            .when(open, |frame| frame.child(self.render_sidebar_resize(cx)));
        let width = move |delta: f32| (sidebar_width + crate::frost::GUTTER) * if open { delta } else { 1. - delta };
        if self.sidebar_toggles == 0 {
            frame.w(px(width(1.))).into_any_element()
        } else {
            frame
                .with_animation(
                    ("sidebar", self.sidebar_toggles),
                    Animation::new(motion::PANEL).with_easing(motion::ease_out),
                    move |frame, delta| frame.w(px(width(delta))),
                )
                .into_any_element()
        }
    }

    /// The sidebar's card (Frost): from under the window's head to the window's foot, `frost::INSET` in from its top,
    /// left and bottom. It grows and shrinks with the sidebar, and fades as it shuts. None while signed out.
    pub(super) fn render_sidebar_card(&self, look: crate::frost::Look) -> Option<AnyElement> {
        use crate::frost::INSET;
        if !(self.workspace_open() || matches!(self.auth, super::Auth::Restoring))
            || (!self.sidebar_open && self.sidebar_toggles == 0)
        {
            return None;
        }
        let open = self.sidebar_open;
        let sidebar_width = self.sidebar_width();
        let card = div()
            .absolute()
            .top(px(INSET))
            .bottom(px(INSET))
            .left(px(INSET))
            .child(super::backdrop::card_glass())
            .child(crate::frost::panel(look));
        if self.sidebar_toggles == 0 {
            return Some(card.w(px(sidebar_width)).into_any_element());
        }
        Some(
            card.with_animation(
                ("sidebar-card", self.sidebar_toggles),
                Animation::new(motion::PANEL).with_easing(motion::ease_out),
                move |card, delta| {
                    let shown = if open { delta } else { 1. - delta };
                    card.w(px(sidebar_width * shown)).opacity(shown)
                },
            )
            .into_any_element(),
        )
    }

    /// The window's ground around the sidebar card (`frost::Look::veil`), over the wash: everything but the card, so
    /// the gutters around the card match the pane and only the card shows the desktop more. It follows the card as the
    /// sidebar opens and shuts. None on Opaque, where the wash is the same solid color.
    pub(super) fn render_pane_ground(&self, look: crate::frost::Look) -> Option<AnyElement> {
        use crate::frost::INSET;
        if look.wash == look.veil {
            return None;
        }
        let veil = look.veil;
        if !matches!(self.auth, super::Auth::SignedIn | super::Auth::Restoring)
            || (!self.sidebar_open && self.sidebar_toggles == 0)
        {
            return Some(div().absolute().inset_0().bg(veil).into_any_element());
        }
        let open = self.sidebar_open;
        let sidebar_width = self.sidebar_width();
        let around = div()
            .absolute()
            .top(px(INSET))
            .bottom(px(INSET))
            .left(px(INSET))
            .child(crate::frost::around_panel(veil));
        if self.sidebar_toggles == 0 {
            return Some(around.w(px(sidebar_width)).into_any_element());
        }
        Some(
            around
                .with_animation(
                    ("pane-ground", self.sidebar_toggles),
                    Animation::new(motion::PANEL).with_easing(motion::ease_out),
                    move |around, delta| around.w(px(sidebar_width * if open { delta } else { 1. - delta })),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_sidebar_panel(
        &self,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Settings takes the sidebar's place with its own nav, as the Nexus app does, and gives the page the room.
        if let View::Settings(section) = &self.view {
            return self.render_settings_nav(*section, palette, window, cx);
        }
        let chat_selected =
            (matches!(self.view, View::Chat) && self.current.is_none()) || matches!(self.view, View::NewCode);
        let library_selected = matches!(self.view, View::Library);
        let scrolled = self.sidebar_filter.scroll.offset().y < px(0.);
        v_flex()
            .w(px(self.sidebar_width()))
            .h_full()
            .child(self.render_org(palette, cx))
            .child(
                v_flex()
                    .px(px(8.))
                    .gap(px(1.))
                    // The Assistant first, above New session, with the Nexus mark and its state (brand v5 A21). Only
                    // with an agent API, as the Nexus app's `sidebar-places.tsx` (`agent/gate.tsx`).
                    .when(self.assistant_available(), |places| {
                        let row = self.assistant.row;
                        places
                            .child(
                                place_row_with(
                                    "your-agent",
                                    assistant_mark(row.thinking, palette),
                                    tr("Assistant"),
                                    matches!(self.view, View::Page(crate::pages::PageId::Agent)),
                                    palette,
                                )
                                .child(assistant_state(row, self.assistant.paused, palette))
                                .on_click(cx.listener(|this, _, window, cx| this.open_agent(None, window, cx))),
                            )
                            // Its open errands, one row each (A21·a, A12).
                            .children(self.render_errand_rows(palette, cx))
                    })
                    .child(
                        place_row(
                            "new-session",
                            Lucide::SquarePen,
                            tr("New session"),
                            chat_selected,
                            palette,
                        )
                        // Online teammates take the hint's place, as the web's home row puts them in its `meta`.
                        .map(|row| match self.render_online_stack(palette, cx) {
                            Some(stack) => row.child(stack),
                            None => row.child(super::search::keycaps(NEW_HINT, false, palette)),
                        })
                        .on_click(cx.listener(|this, _, window, cx| this.start_new_session(window, cx))),
                    )
                    .children(self.account_gateway().is_some().then(|| {
                        place_row(
                            "inbox",
                            Lucide::Inbox,
                            tr("Inbox"),
                            matches!(self.view, View::Inbox),
                            palette,
                        )
                        .when(self.inbox > 0, |row| row.child(count(self.inbox, palette)))
                        .on_click(cx.listener(|this, _, window, cx| this.open_inbox(window, cx)))
                    }))
                    .children(self.account_gateway().is_some().then(|| {
                        place_row(
                            "artifacts",
                            Lucide::FileText,
                            tr("Artifacts"),
                            library_selected,
                            palette,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.open_library(window, cx)))
                    })),
            )
            .children(self.render_filter_field(palette))
            .child(
                v_flex()
                    .id("sidebar-groups")
                    .debug_selector(|| "sidebar-groups".into())
                    .track_scroll(&self.sidebar_filter.scroll)
                    // A dragged session near the list's edge scrolls it (`session_drag_edge.rs`).
                    .on_drag_move(cx.listener(
                        |this, event: &DragMoveEvent<super::session_drag::SessionDrag>, _, cx| {
                            this.drag_near_edge(event.event.position, event.bounds, cx);
                        },
                    ))
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .mt(px(6.))
                    // Always 1px, clear at rest, so the list does not jump when the line shows.
                    .border_t_1()
                    .border_color(if scrolled { palette.border } else { transparent_black() })
                    .px(px(8.))
                    .pb(px(8.))
                    // Its moving marks are drawn over the sidebar, not in this list (`sidebar_marks.rs`).
                    .children(self.sidebar_marks.clone().collect(|| {
                        let mut groups = Vec::new();
                        // The Assistant's projects first, as on the web (A14·a): each with its errands.
                        if self.assistant_available() && !self.filtering() {
                            groups.extend(
                                self.render_errand_projects(palette, cx)
                                    .map(IntoElement::into_any_element),
                            );
                        }
                        // Sessions next, as on the web: the chats sit right under the places, the projects' runs
                        // below.
                        if self.account_gateway().is_some() {
                            groups.push(self.render_recent(palette, cx).into_any_element());
                        }
                        groups.push(self.render_projects(palette, window, cx).into_any_element());
                        // The sessions put aside close the projects they come from; the servers start a section of
                        // their own under a hairline (`sidebar_history.rs`, `sidebar_machines.rs`).
                        if !self.filtering() {
                            groups.push(self.render_history(palette, window, cx).into_any_element());
                        }
                        if self.account_gateway().is_some() {
                            groups.extend(self.render_servers(palette, cx).map(IntoElement::into_any_element));
                        }
                        groups.extend(self.render_message_hits(palette, cx).map(IntoElement::into_any_element));
                        if self.account_gateway().is_some() {
                            groups.extend(self.render_following(palette, cx).map(IntoElement::into_any_element));
                            groups.extend(
                                self.render_team_presence(palette, cx)
                                    .map(IntoElement::into_any_element),
                            );
                        }
                        groups
                    })),
            )
            .children(self.render_bulk_bar(palette, cx))
            .children(
                self.account_gateway()
                    .is_some()
                    .then(|| self.render_cloud_strip(palette, cx))
                    .flatten(),
            )
            .children(
                self.account_gateway()
                    .is_some()
                    .then(|| self.render_sidebar_budget(palette, cx))
                    .flatten(),
            )
            .children(
                self.account_gateway()
                    .is_some()
                    .then(|| self.render_setup_checklist(palette, cx))
                    .flatten(),
            )
            .child(self.render_person(palette, cx))
            // The pointer on the card holds the session rows (`sidebar_hold.rs`).
            .relative()
            .child(self.sidebar_hold.watch(cx.entity().downgrade()))
            .into_any_element()
    }

    /// The organization's name as the switcher shows it ("Personal" for the person's own account) and its mark: the
    /// logo, the person's icon, or the initials. The sidebar's switcher and the panel's pill both show them.
    pub(super) fn org_mark(&self, palette: Palette) -> (SharedString, AnyElement) {
        let name: SharedString = self
            .account
            .as_ref()
            .map(|account| account.org_name.clone().into())
            .unwrap_or_else(|| "Nexus".into());
        let logo = self.account.as_ref().and_then(|account| account.org_logo.clone());
        let personal = self.account.as_ref().is_some_and(|account| account.personal);
        let name: SharedString = if personal { tr("Personal") } else { name };
        let mark = match logo {
            None if personal => glyph(Lucide::User, palette.icon).size(px(16.)).into_any_element(),
            Some(url) => super::logo::logo(url, 22.)
                .rounded(px(6.))
                .bg(white())
                // White on the light sidebar vanishes: a soft ring (a shadow, not a border) holds the tile's edge.
                .when(!palette.dark, |tile| {
                    tile.shadow(vec![BoxShadow {
                        color: gpui_kit::black().opacity(0.08),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(1.),
                        inset: false,
                    }])
                })
                .into_any_element(),
            None => div()
                .size(px(16.))
                .flex_none()
                .rounded(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .bg(palette.selected)
                .text_size(px(8.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(initials(&name))
                .into_any_element(),
        };
        (name, mark)
    }

    /// The organization's switcher (`OrgSwitcher` in Nexus UI's `shell.tsx`). With a logo it shows it, the name and
    /// "by NEXUS" under it (`OrgBrand`); without one, the initials.
    fn render_org(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let (name, mark) = self.org_mark(palette);
        let has_logo = self.account.as_ref().is_some_and(|account| account.org_logo.is_some());
        let label = if has_logo {
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .line_height(relative(1.2))
                .child(div().truncate().child(name))
                .child(
                    h_flex()
                        .gap(px(3.))
                        .text_size(px(10.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(palette.muted)
                        .child("by")
                        .child(
                            svg()
                                .path(crate::assets::WORDMARK)
                                .w(px(40.))
                                .h(px(7.))
                                .text_color(palette.muted),
                        ),
                )
                .into_any_element()
        } else {
            div().flex_1().min_w(px(0.)).truncate().child(name).into_any_element()
        };
        if self.account_gateway().is_none() {
            return h_flex()
                .h(px(46.))
                .px(px(16.))
                .gap(px(8.))
                .child(mark)
                .child(label)
                .into_any_element();
        }
        h_flex()
            .relative()
            .pt(px(4.))
            .px(px(8.))
            .pb(px(6.))
            .gap(px(4.))
            .child(
                h_flex()
                    .id("org")
                    .flex_1()
                    .min_w(px(0.))
                    .h(px(36.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(8.))
                    .text_size(px(ROW_TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when(self.overlay.org_menu().is_some(), |row| row.bg(palette.hover))
                    .hover(|style| style.bg(palette.hover))
                    .menu_trigger(
                        self.overlay.org_menu().is_some(),
                        cx,
                        |this| this.overlay.org_menu().is_some(),
                        |this, _, cx| this.toggle_org_menu(cx),
                    )
                    .child(mark)
                    .child(label)
                    .child(glyph(Lucide::ChevronsUpDown, palette.icon).size(px(14.))),
            )
            // Search is in the window head now, beside the sidebar toggle (`render_title_bar`).
            .children(self.render_org_menu(8., 42., palette, cx))
            .into_any_element()
    }

    /// Sessions: the chats in no project, newest first, drawn as the web draws them (`sessions_list.rs`). Agent runs,
    /// cloud ones too, are in their projects, not here. A search shows every matching chat here, filed or not.
    fn render_recent(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        use super::sessions_list::{self, Link, Step};
        let now = code::now_ms();
        let filtering = self.filtering() && !self.project_filter_open();
        let matches: Vec<_> = if filtering {
            self.filter_kept().chats
        } else {
            self.sessions
                .iter()
                .filter(|session| self.chat_project(&session.id).is_none())
                .collect()
        };
        let total = matches.len();
        let ready = self.sessions_loaded && (self.sessions_error.is_none() || !self.sessions.is_empty());
        // The head folds only when folding hides a session; with four or fewer it is a plain title.
        let foldable = ready && !filtering && sessions_list::foldable(total);
        let folded = self.recent_folded && foldable;
        let (shown, link) = sessions_list::shown(total, filtering, self.recent_folded, self.sessions_step);
        let open = |id: &str| matches!(self.view, View::Chat) && self.current.as_deref() == Some(id);
        // Folded, the open chat stays in sight even when it is older than the four.
        let kept_open = folded
            .then(|| matches.iter().skip(shown).find(|session| open(&session.id)).copied())
            .flatten();
        let hidden = total - shown - usize::from(kept_open.is_some());
        let head = group_head(tr("Sessions"), palette).id("recent-head").gap(px(4.));
        let head = if foldable {
            head.cursor_pointer()
                .hover(|style| style.text_color(palette.foreground))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.recent_folded = !this.recent_folded;
                    crate::paths::save_recent_folded(this.recent_folded);
                    cx.notify();
                }))
                .child(super::disclosure("recent-disclosure", !folded, palette.faint))
                // Folded, the head says how many sessions wait behind it.
                .when(folded && hidden > 0, |head| {
                    head.child(div().flex_1()).child(hint(format!("+{hidden}"), palette))
                })
                .into_any_element()
        } else {
            head.into_any_element()
        };
        let group = v_flex().child(head);
        if !self.sessions_loaded {
            return group.child(render_skeleton(palette, 4));
        }
        if let Some(error) = self.sessions_error.clone()
            && self.sessions.is_empty()
        {
            return group.child(note(error, palette.destructive));
        }
        if matches.is_empty() {
            let text = if filtering {
                tr("No matches")
            } else if self.sessions.is_empty() {
                tr("No sessions yet")
            } else {
                tr("Sessions in no project show here")
            };
            return group.child(note(text, palette.faint));
        }
        let listed = matches.iter().take(shown).copied().chain(kept_open);
        let headed = sessions_list::headed(total, filtering, self.recent_folded);
        let mut last_age = None;
        let mut rows: Vec<AnyElement> = Vec::new();
        for session in listed {
            if headed {
                let age = sessions_list::local_age(session.at_ms, now);
                if last_age != Some(age) {
                    rows.push(age_heading(age.label(), palette).into_any_element());
                    last_age = Some(age);
                }
            }
            let row = self.render_chat_row(session, false, now, palette, cx);
            rows.push(self.filter_cursor_mark(row, &super::sidebar_filter::Open::Chat(session.id.clone()), palette));
        }
        group.children(rows).when_some(link, |group, link| {
            group.child(
                h_flex()
                    .id("all-sessions")
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(8.))
                    .text_xs()
                    .text_color(palette.faint)
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.foreground))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sessions_step = match link {
                            Link::ShowMore => Step::More,
                            Link::AllSessions => Step::All,
                        };
                        cx.notify();
                    }))
                    .child(link.label()),
            )
        })
    }

    /// Code projects and their sessions (`WorkbenchSidebar`).
    /// A project row folds its sessions and shows session and menu actions on hover.
    fn render_projects(&self, palette: Palette, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now_ms = code::now_ms();
        let group = v_flex().child(
            h_flex()
                .h(px(28.))
                .mt(px(16.))
                .pl(px(8.))
                .pr(px(2.))
                .text_xs()
                .text_color(palette.faint)
                .gap(px(4.))
                .child(self.render_project_filter_input(palette, cx))
                .child(self.render_add_project(palette, cx)),
        );
        if let Some(error) = self.code_error.clone() {
            return group.child(note(error, palette.faint)).child(
                Button::new("sidebar-code-retry")
                    .ghost()
                    .label(tr("Retry"))
                    .on_click(cx.listener(|this, _, window, cx| this.retry_code_store(window, cx))),
            );
        }
        // One list: where a project runs is its icon, not a group (this computer, a server, or the cloud).
        let new_field = self.render_new_project_field(false, palette);
        // Cloud sessions on a repository with no project here: a cloud project each, after the others.
        let cloud: Vec<AnyElement> = if self.filtering() {
            Vec::new()
        } else {
            self.render_cloud_projects(palette, cx)
        };
        if self.projects.is_empty() && self.groups.groups.projects.is_empty() && new_field.is_none() {
            if !cloud.is_empty() {
                return group.children(cloud);
            }
            return group
                .children(self.account_gateway().is_some().then(|| {
                    place_row("new-project", Lucide::FolderPlus, tr("New project"), false, palette)
                        .on_click(cx.listener(|this, _, window, cx| this.begin_new_project(false, window, cx)))
                }))
                .child(
                    place_row("open-folder", Lucide::FolderOpen, tr("Open a folder"), false, palette)
                        .on_click(cx.listener(|this, _, _, cx| this.add_project(cx))),
                );
        }
        let filtering = self.filtering();
        let group = group.children(new_field);
        let listed: Vec<(&code::CodeProject, Vec<&code::CodeSession>)> = if filtering {
            // A search finds snoozed and settled sessions in place, and folds nothing.
            self.filter_kept().projects
        } else {
            // A cloud session stays in its repository's project whatever its machine does: asleep, it keeps the rows
            // last read from its machine, with a moon on each (`cloud_session_mark`).
            self.projects
                .iter()
                .map(|project| {
                    // The open session stays in its project while it is on screen, as the Nexus app keeps it, unless it
                    // is settled: that one stays under "Settled" (`CodeSession::shown_aside`).
                    let active = project
                        .sessions
                        .iter()
                        .filter(|session| {
                            let open = matches!(&self.view, View::Code(open) if open.id == session.id);
                            session.shown_aside(now_ms, open).is_none()
                        })
                        .collect();
                    (project, active)
                })
                .collect()
        };
        if filtering && listed.is_empty() {
            return group
                .child(note(tr("No projects or sessions found"), palette.muted))
                .child(note(tr("Try another name or branch."), palette.faint))
                .child(
                    Button::new("clear-project-search-empty")
                        .ghost()
                        .label(tr("Clear search"))
                        .on_click(cx.listener(|this, _, window, cx| this.clear_filter(window, cx))),
                );
        }
        // One repository is one project, wherever its copies run (`repo_groups.rs`). The person's own projects join
        // them in one list: pinned Code projects first, then each by when it was added (`project_list.rs`).
        let plain: &[crate::project_groups::PlainProject] = if filtering { &[] } else { &self.groups.groups.projects };
        let merged = super::project_list::merge(super::repo_groups::group(listed), plain);
        // The order the person dragged them in; a search lists them as found, and moves none (`project_drag.rs`).
        let merged = if filtering {
            merged
        } else {
            super::project_list::arrange(merged, self.groups.groups.project_order())
        };
        // Dragging stays on internal builds until the production switch (`channel.rs`); stable keeps the merged order.
        let keys: Option<std::rc::Rc<[String]>> =
            (!filtering && crate::channel::internal()).then(|| merged.iter().map(|entry| entry.key()).collect());
        let projects = merged.into_iter().map(|entry| {
            let slot = keys.clone().map(|listed| super::project_drag::ProjectSlot {
                key: entry.key(),
                pinned: entry.pinned(),
                listed,
            });
            let repo = match entry {
                super::project_list::Entry::Plain(project) => {
                    return self.render_plain_project(project, slot, now_ms, palette, cx);
                }
                super::project_list::Entry::Code(repo) => repo,
            };
            let ids = repo.ids();
            let places = repo.machines();
            let connected_on = |machine: Option<&str>| {
                machine.is_none_or(|id| {
                    self.servers.iter().any(|server| {
                        server.id == id && matches!(server.status, nexus_remote::machine::Status::Connected { .. })
                    }) || self.cloud_machines.iter().any(|machine| {
                        machine.id == id && matches!(machine.status, nexus_remote::machine::Status::Connected { .. })
                    })
                })
            };
            // Its "+" opens on a copy that answers: the lead's, else another's.
            let ready = repo
                .members
                .iter()
                .find(|member| connected_on(member.machine.as_deref()))
                .map(|member| member.id.clone());
            let project = repo.lead;
            // The order the person dragged them in; a search lists them as found.
            let active = if filtering {
                repo.sessions
            } else {
                super::session_order::arrange(repo.sessions, self.groups.groups.session_order(&project.id))
            };
            // Rows do not move under the pointer, and glide when they move (`sidebar_hold.rs`). The count stays live.
            let live = active.len();
            let moment = cx.background_executor().now();
            let active = if filtering {
                active
            } else {
                self.hold_sessions(&project.id, active, cx)
            };
            // The order on screen, for a drop that moves one of them; a search's partial list moves none.
            let listed: Option<std::rc::Rc<[String]>> =
                (!filtering).then(|| active.iter().map(|session| session.id.clone()).collect());
            let place = super::session_drag::Place::Project(project.id.clone());
            let members: std::rc::Rc<[String]> = ids.clone().into();
            let project_target = || super::session_drag::Target::Project {
                lead: project.id.clone(),
                members: members.clone(),
            };
            let folded = !filtering && self.folded_projects.contains(&project.id);
            let filed: Vec<_> = if filtering {
                Vec::new()
            } else {
                ids.iter()
                    .flat_map(|id| self.chats_in(&crate::project_groups::Home::Code(id.clone())))
                    .collect()
            };
            // Cloud sessions on this repository whose machine was not read yet: their rows file here too.
            let waiting: Vec<&crate::machines::CloudView> = if filtering {
                Vec::new()
            } else {
                super::repo_groups::repo_key(&project.remotes)
                    .map(|key| self.unread_cloud_machines_in(Some(&key)))
                    .unwrap_or_default()
            };
            let total = live + filed.len() + waiting.len();
            let menu_open = self.overlay.project_menu().map(String::as_str) == Some(project.id.as_str());
            let reveal = self.project_hover_motion.get(&project.id).copied().unwrap_or_default();
            let expansion = if filtering {
                super::folder_motion::Transition::settled(1.)
            } else {
                self.project_fold_motion
                    .get(&project.id)
                    .copied()
                    .unwrap_or_else(|| super::folder_motion::Transition::settled(if folded { 0. } else { 1. }))
            };
            let id = project.id.clone();
            // A folded project hides its sessions' marks: its head shows the loudest, so work and a wait stay in sight.
            let hidden_state = folded
                .then(|| loudest(active.iter().map(|session| self.session_mark(session, cx))))
                .flatten();
            let action_width = if ready.is_some() { 46. } else { 22. };
            let actions = h_flex()
                .w(px(action_width))
                .h(px(ROW))
                .flex_none()
                .items_center()
                .gap(px(2.))
                .when_some(ready, |actions, id| {
                    actions.child({
                        let tip: SharedString = tr("New session in {project}")
                            .replace("{project}", &project.name)
                            .into();
                        small_button(
                            SharedString::from(format!("project-new-{}", project.id)),
                            Lucide::Plus,
                            palette,
                        )
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_new_code_in(id.clone(), window, cx);
                        }))
                    })
                })
                .child({
                    let id = project.id.clone();
                    small_button(
                        SharedString::from(format!("project-more-{}", project.id)),
                        Lucide::Ellipsis,
                        palette,
                    )
                    .menu_trigger(
                        self.overlay.project_menu().map(String::as_str) == Some(id.as_str()),
                        cx,
                        {
                            let id = id.clone();
                            move |this| this.overlay.project_menu().map(String::as_str) == Some(id.as_str())
                        },
                        move |this, _, cx| this.toggle_project_menu(&id, cx),
                    )
                });
            let head = h_flex()
                .id(SharedString::from(format!("project-{}", project.id)))
                .debug_selector(|| format!("project-{}", project.id))
                .on_hover({
                    let id = project.id.clone();
                    cx.listener(move |this, hovered: &bool, _, cx| {
                        this.project_hover_motion
                            .entry(id.clone())
                            .or_default()
                            .retarget(if *hovered { 1. } else { 0. }, motion::UI);
                        cx.notify();
                    })
                })
                .relative()
                .h(px(ROW))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .text_size(px(ROW_TEXT))
                .text_color(palette.secondary)
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let folded = this.folded_projects.contains(&id);
                    this.fold_project(&id, !folded, cx);
                }))
                .on_mouse_down(MouseButton::Right, {
                    let id = project.id.clone();
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_overlay(Overlay::ProjectMenu(id.clone()));
                        cx.notify();
                    })
                })
                // The actions slide in on hover, so a press can land where "+" is going before its hitbox is there.
                // This fixed zone, painted under them, keeps that press from folding the project instead.
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(px(8.))
                        .w(px(action_width))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                )
                .child(self.project_place_icon(&places, folded, palette))
                .child(div().flex_1().min_w(px(0.)).truncate().child(project.name.clone()))
                // Status and count keep their place in the row while the actions make room on their right.
                .when_some(hidden_state, |row, state| {
                    row.child(status_mark(&format!("project-{}", project.id), state, palette))
                })
                .when(project.pinned, |row| row.child(glyph(Lucide::Pin, palette.faint)))
                .child(
                    div()
                        .min_w(px(16.))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .when(total > 0, |count| count.child(hint(total.to_string(), palette))),
                )
                .child({
                    let frame = div().flex_none().overflow_hidden().child(actions);
                    let reveal = if menu_open {
                        super::folder_motion::Transition::settled(1.)
                    } else {
                        reveal
                    };
                    reveal.draw(frame, format!("project-actions-{}", project.id), move |frame, value| {
                        // Cancel the row gap when closed, so the count rests at the original right edge.
                        frame
                            .w(px(action_width * value))
                            .ml(px(-8. * (1. - value)))
                            .opacity(value)
                    })
                })
                .when(menu_open, |head| {
                    head.child(self.render_project_actions(project, &ids, palette, cx))
                });
            // While the group folds, its rows' moving marks fold and fade with it (`sidebar_marks::in_place_while`).
            let gliding = self.sidebar_hold.moving(&project.id, moment);
            let (rows, waiting, chats) = super::sidebar_marks::in_place_while(expansion.moving() || gliding, || {
                // Linked sessions hang under the session that started them (`session-tree.ts`).
                let tree = super::session_tree::tree(&active);
                let at: Vec<(&str, usize)> = tree
                    .iter()
                    .map(|(session, depth)| (session.id.as_str(), *depth))
                    .collect();
                let families = super::session_tree::families(&at);
                let pinned: std::collections::HashMap<&str, bool> = active
                    .iter()
                    .map(|session| (session.id.as_str(), session.pinned))
                    .collect();
                let rows: Vec<(AnyElement, usize)> = tree
                    .iter()
                    .zip(&families)
                    .filter(|_| expansion.visible())
                    .map(|((session, depth), (top, last))| {
                        let row = self.render_session_row_at(session, *depth, None, now_ms, palette, window, cx);
                        let open = super::sidebar_filter::Open::Code(session.id.clone());
                        let row = self.filter_cursor_mark(row, &open, palette);
                        let slot = listed.clone().map(|listed| super::session_drag::RowSlot {
                            project: project.id.clone(),
                            members: members.clone(),
                            listed,
                            top: top.to_string(),
                            last: last.to_string(),
                            pinned: pinned.get(top).copied().unwrap_or(session.pinned),
                        });
                        let row = self.drag_row(row, session, place.clone(), *depth, slot, palette, cx);
                        let row = self.sidebar_hold.draw(&project.id, &session.id, row, moment, window);
                        (row, *depth)
                    })
                    .collect();
                let rows = super::session_tree::nest(rows, palette);
                let waiting: Vec<AnyElement> = if expansion.visible() {
                    waiting
                        .into_iter()
                        .map(|machine| self.cloud_row(machine, 24., palette, cx).into_any_element())
                        .collect()
                } else {
                    Vec::new()
                };
                // A project's chats file under its sessions, and fold with them.
                let chats: Vec<AnyElement> = if expansion.visible() {
                    filed
                        .iter()
                        .map(|chat| self.render_filed_chat(chat, now_ms, palette, cx))
                        .collect()
                } else {
                    Vec::new()
                };
                (rows, waiting, chats)
            });
            let empty = total == 0 && !filtering;
            // A deleted session's row folds as it leaves, and the group folds with it (`row_exit.rs`).
            let sessions_height = self.row_exits.height(
                active.iter().map(|session| session.id.as_str()),
                std::time::Instant::now(),
                cx.reduce_motion(),
            ) - self.sidebar_hold.shortfall(&project.id, moment);
            let height = if empty {
                ROW
            } else {
                (filed.len() + waiting.len()) as f32 * ROW + sessions_height
            };
            // Under its sessions, the project's foot takes a session dropped there, as its head does.
            let foot = v_flex()
                .flex_none()
                .children(waiting)
                .children(chats)
                .when(empty, |column| {
                    // In line with the session titles above it (56px from the sidebar's edge).
                    column.child(div().pl(px(40.)).child(note(tr("No sessions yet"), palette.faint)))
                });
            let foot = self.drop_zone(
                foot.into_any_element(),
                format!("project-foot-{}", project.id),
                project_target(),
                palette,
                cx,
            );
            let contents = v_flex().flex_none().children(rows).child(foot);
            let sessions = expansion.draw(
                div().flex_none().child(contents),
                format!("project-sessions-{}", project.id),
                move |frame, value| {
                    frame
                        .h(px(height * value))
                        .opacity(value)
                        .when(value < 1., |frame| frame.overflow_hidden())
                },
            );
            // A session put aside or archived comes back when dropped on its own project.
            let head = self.drop_zone(
                head.into_any_element(),
                format!("project-head-{}", project.id),
                project_target(),
                palette,
                cx,
            );
            // The head lifts the project, and the whole block takes a project dropped on it (`project_drag.rs`).
            let Some(slot) = slot else {
                return v_flex().child(head).child(sessions).into_any_element();
            };
            let head = self.project_drag_source(head, &slot, &project.name, palette, cx);
            let block = v_flex().child(head).child(sessions).into_any_element();
            self.project_drop_target(block, slot, palette, cx)
        });
        group.children(projects).children(cloud)
    }

    /// Fold project `id` (`folded`) or unfold it, its sessions folding over `motion::UI_LONG`. Its head's click and the
    /// edge cue (`sidebar_edge_cue.rs`) both fold it here.
    pub(super) fn fold_project(&mut self, id: &str, folded: bool, cx: &mut Context<Self>) {
        let changed = if folded {
            self.folded_projects.insert(id.to_string())
        } else {
            self.folded_projects.remove(id)
        };
        if !changed {
            return;
        }
        let open = !folded;
        self.project_fold_motion
            .entry(id.to_string())
            .or_insert_with(|| super::folder_motion::Transition::settled(if open { 0. } else { 1. }))
            .retarget(if open { 1. } else { 0. }, motion::UI_LONG);
        cx.notify();
    }

    fn toggle_project_menu(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.overlay.project_menu().map(String::as_str) == Some(id) {
            self.overlay.close_project_menu();
        } else {
            self.open_overlay(Overlay::ProjectMenu(id.to_string()));
        }
        cx.notify();
    }

    /// A project's menu (`ProjectMenu`): Pin or Unpin, and Archive. Both act on every copy of its repository (`ids`).
    fn render_project_actions(
        &self,
        project: &code::CodeProject,
        ids: &[String],
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (pin_icon, pin_label) = if project.pinned {
            (Lucide::PinOff, tr("Unpin project"))
        } else {
            (Lucide::Pin, tr("Pin project"))
        };
        let pin = {
            let (ids, pinned) = (ids.to_vec(), project.pinned);
            menu_row("project-pin", pin_icon, pin_label, palette).on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                for id in &ids {
                    this.update_project(id.clone(), Some(!pinned), None, cx);
                }
            }))
        };
        let archive = {
            let ids = ids.to_vec();
            menu_row("project-archive", Lucide::Archive, tr("Archive project"), palette).on_click(cx.listener(
                move |this, _, _, cx| {
                    cx.stop_propagation();
                    for id in &ids {
                        this.update_project(id.clone(), None, Some(true), cx);
                    }
                },
            ))
        };
        div().absolute().right_0().top(px(ROW)).child(
            deferred(
                anchored()
                    .anchor(Anchor::TopRight)
                    .offset(point(px(0.), px(2.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        menu_box(
                            SharedString::from(format!("project-menu-{}", project.id)),
                            200.,
                            palette,
                        )
                        .text_size(px(ROW_TEXT))
                        .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.overlay.close_project_menu();
                            cx.notify();
                        }))
                        .child(pin)
                        .child(archive),
                    ),
            )
            .with_priority(1),
        )
    }

    /// Pin or archive a project through this build's runner; read-only while the Nexus app holds the sessions.
    pub(super) fn update_project(
        &mut self,
        id: String,
        pinned: Option<bool>,
        archived: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        self.overlay.close_project_menu();
        match self.runs(&id) {
            Some(runs) => runs.update_project(id, pinned, archived),
            None => self.refuse_read_only(),
        }
        cx.notify();
    }

    /// The person at the foot (`PersonRow`): initials, name and the settings gear; the row opens the account menu.
    fn render_person(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let name: SharedString = self
            .account
            .as_ref()
            .map(|account| account.name.clone().into())
            .unwrap_or_else(|| {
                if self.is_local() {
                    "Not signed in".into()
                } else {
                    "You".into()
                }
            });
        div()
            .relative()
            .px(px(8.))
            .pb(px(4.))
            .child(
                h_flex()
                    .h(px(44.))
                    .gap(px(4.))
                    .child(
                        h_flex()
                            .id("person")
                            .debug_selector(|| "person".into())
                            .flex_1()
                            .min_w(px(0.))
                            .h(px(36.))
                            .px(px(6.))
                            .gap(px(8.))
                            .rounded(px(8.))
                            .cursor_pointer()
                            .when(self.overlay.person_menu(), |row| row.bg(palette.hover))
                            .hover(|style| style.bg(palette.hover))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_overlay(Overlay::PersonMenu);
                                this.overlay.close_model_menu();
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .size(px(24.))
                                    .flex_none()
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(palette.selected)
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(initials(&name)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_size(px(ROW_TEXT))
                                    .child(name),
                            ),
                    )
                    .child(icon_button("settings", Lucide::Settings, palette).on_click(
                        cx.listener(|this, _, window, cx| this.open_settings(SettingsSection::Appearance, window, cx)),
                    )),
            )
            .when_some(
                motion::presence("person-menu", self.overlay.person_menu(), cx),
                |foot, phase| {
                    let menu = phase.menu("person-menu", 4., self.render_person_menu(palette, cx));
                    foot.child(
                        div().absolute().top_0().left(px(8.)).child(
                            deferred(
                                anchored()
                                    .anchor(Anchor::BottomLeft)
                                    .offset(point(px(0.), px(-4.)))
                                    .snap_to_window_with_margin(px(8.))
                                    .child(menu),
                            )
                            .with_priority(1),
                        ),
                    )
                },
            )
    }

    fn render_person_menu(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        if self.is_local() {
            return v_flex()
                .id("local-account-menu")
                .occlude()
                .w(px(self.sidebar_width() - 16.))
                .p(px(4.))
                .rounded(px(14.))
                .bg(palette.pop)
                .border_1()
                .border_color(palette.border)
                .shadow_lg()
                .child(
                    menu_row("menu-sign-in", Lucide::LogIn, tr("Sign in"), palette)
                        .on_click(cx.listener(|this, _, window, cx| this.sign_in_from_local(window, cx))),
                )
                .into_any_element();
        }

        let email = self
            .account
            .as_ref()
            .map(|account| account.email.clone())
            .unwrap_or_default();
        let theme = |choice: ThemeChoice, id: &'static str, label: SharedString| {
            let picked = self.theme == choice;
            div()
                .id(id)
                .flex_1()
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .text_xs()
                .cursor_pointer()
                .when(picked, |segment| {
                    segment.bg(palette.raised).text_color(palette.foreground)
                })
                .when(!picked, |segment| {
                    segment
                        .text_color(palette.muted)
                        .hover(|style| style.text_color(palette.foreground))
                })
                .on_click(cx.listener(move |this, _, _, cx| this.set_theme(choice, cx)))
                .child(label)
        };
        // `occlude`: the backdrop under the menu closes it on mouse down, so a press on a row must not reach it, or the
        // menu goes before the row's click lands.
        v_flex()
            .id("person-menu")
            .occlude()
            .w(px(self.sidebar_width() - 16.))
            .p(px(4.))
            .gap(px(1.))
            .rounded(px(14.))
            .bg(palette.pop)
            .border_1()
            .border_color(palette.border)
            .shadow_lg()
            .text_size(px(ROW_TEXT))
            .text_color(palette.foreground)
            .when(!email.is_empty(), |menu| {
                menu.child(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_xs()
                        .text_color(palette.faint)
                        .truncate()
                        .child(email),
                )
            })
            .child(
                menu_row("menu-settings", Lucide::Settings, tr("Settings"), palette).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.overlay.close_person_menu();
                        this.open_settings(SettingsSection::Appearance, window, cx)
                    },
                )),
            )
            .child(
                menu_row("menu-docs", Lucide::BookOpen, tr("Docs"), palette)
                    .on_click(cx.listener(|this, _, _, cx| this.open_docs(String::new(), cx))),
            )
            .child(
                h_flex()
                    .m(px(4.))
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded(px(8.))
                    .bg(palette.selected)
                    .child(theme(ThemeChoice::System, "System", tr("System")))
                    .child(theme(ThemeChoice::Light, "Light", tr("Light")))
                    .child(theme(ThemeChoice::Dark, "Dark", tr("Dark"))),
            )
            .child(div().h(px(6.)))
            .child(
                menu_row("menu-onboarding", Lucide::Sparkles, tr("Set up Nexus"), palette).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.overlay.close_person_menu();
                        this.restart_onboarding(window, cx);
                    },
                )),
            )
            .child(
                menu_row("menu-sign-out", Lucide::LogOut, tr("Sign out"), palette).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.overlay.close_person_menu();
                        this.request_sign_out();
                        cx.notify();
                    },
                )),
            )
            .into_any_element()
    }
}

/// A place: an icon and a name, 32px high, filled when it is where the person is.
pub(super) fn place_row(
    id: &'static str,
    icon: Lucide,
    label: impl Into<SharedString>,
    selected: bool,
    palette: Palette,
) -> Stateful<Div> {
    place_row_with(id, glyph(icon, palette.icon).size(px(16.)), label, selected, palette)
}

/// The Assistant row's icon: the Nexus mark at the rows' 16 px, tinted as their Lucide icons are. It breathes and turns
/// slowly while an answer streams, and eases to rest when it ends (A21·c, A13). Under less motion it stands still.
fn assistant_mark(thinking: bool, palette: Palette) -> impl IntoElement {
    div()
        .size(px(16.))
        .flex_none()
        .child(motion::breathing_mark("assistant-row-mark", 16., palette.icon).breathing(thinking))
}

/// What ends the Assistant row (A21·c): "Paused" while the person paused it (A11·b), which wins because nothing runs;
/// else a filled dot and "Needs you" while a write waits for the person, else the count of Nexus's replies not seen
/// yet, else the ⌘J hint.
pub(super) fn assistant_state(row: crate::pages::agent::RowState, paused: bool, palette: Palette) -> AnyElement {
    if paused {
        return div()
            .flex_none()
            .text_size(px(11.5))
            .text_color(palette.muted)
            .child(tr("Paused"))
            .into_any_element();
    }
    match assistant_trailing(row) {
        Trailing::NeedsYou => h_flex()
            .flex_none()
            .gap(px(6.))
            .text_size(px(11.5))
            .text_color(palette.foreground)
            .child(div().size(px(6.)).flex_none().rounded_full().bg(palette.foreground))
            .child(tr("Needs you"))
            .into_any_element(),
        Trailing::Unread(unread) => count(unread as usize, palette).into_any_element(),
        Trailing::Hint => super::search::keycaps(ASSISTANT_HINT, false, palette).into_any_element(),
    }
}

/// What ends the Assistant row, by its state: Needs you wins over the count, and the count over the hint.
#[derive(Debug, PartialEq)]
pub(super) enum Trailing {
    NeedsYou,
    Unread(u32),
    Hint,
}

pub(super) fn assistant_trailing(row: crate::pages::agent::RowState) -> Trailing {
    if row.needs_you {
        Trailing::NeedsYou
    } else if row.unread > 0 {
        Trailing::Unread(row.unread)
    } else {
        Trailing::Hint
    }
}

/// A place row with any 16px icon: the Assistant's is the Nexus mark, not a Lucide icon.
fn place_row_with(
    id: &'static str,
    icon: impl IntoElement,
    label: impl Into<SharedString>,
    selected: bool,
    palette: Palette,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    h_flex()
        .id(id)
        .debug_selector(|| id.into())
        .h(px(ROW))
        .px(px(8.))
        .gap(px(8.))
        .rounded(px(8.))
        .text_size(px(ROW_TEXT))
        .cursor_pointer()
        .relative()
        .when(selected, |row| row.bg(palette.selected).text_color(palette.foreground))
        // The fill fades under the pointer; a selected row is filled already.
        .when(!selected, |row| {
            row.child(crate::motion::hover_fill("hover", palette.hover, 8.))
        })
        // Hover is set on every row, selected or not: left off a selected row, it kept the last hover's bright text
        // once the row lost its selection.
        .hover(move |style| style.text_color(palette.foreground))
        .when(!selected, |row| row.text_color(palette.secondary))
        .active(|style| style.opacity(0.85))
        .child(icon)
        .child(div().flex_1().child(label))
}

/// A session row: its state mark (Code sessions), its title and how long ago it moved. With `title_tooltip`, hovering
/// a cut title shows the whole of it; a Code session's row shows its card instead (`session_peek.rs`). `retitled`:
/// when the title last changed on screen, so the new one rises into place (`motion::retitle`).
#[allow(clippy::too_many_arguments)]
pub(super) fn thread_row(
    id: SharedString,
    title: SharedString,
    name: Option<&str>,
    age: Option<String>,
    status: Option<(&str, Status)>,
    selected: bool,
    picked: bool,
    title_tooltip: bool,
    retitled: Option<std::time::Instant>,
    palette: Palette,
) -> Stateful<Div> {
    let mark = status.map(|(id, status)| status_mark(id, status, palette));
    thread_row_marked(
        id,
        title,
        name,
        age,
        mark,
        selected,
        picked,
        title_tooltip,
        retitled,
        palette,
    )
}

/// [`thread_row`] with its mark drawn by the caller: a cloud session's row shows a cloud in place of the idle ring.
#[allow(clippy::too_many_arguments)]
pub(super) fn thread_row_marked(
    id: SharedString,
    title: SharedString,
    name: Option<&str>,
    age: Option<String>,
    mark: Option<AnyElement>,
    selected: bool,
    picked: bool,
    title_tooltip: bool,
    retitled: Option<std::time::Instant>,
    palette: Palette,
) -> Stateful<Div> {
    // A picked row (bulk selection) is filled in the primary tone, hovered or not.
    let fill = if picked {
        Some(super::sidebar_select::picked_fill(palette))
    } else if selected {
        Some(palette.selected)
    } else {
        None
    };
    let selected = selected || picked;
    // Tests find the title by its row's id.
    let selector = format!("{id}-title");
    h_flex()
        .id(id.clone())
        .h(px(ROW))
        .px(px(8.))
        .gap(px(8.))
        .rounded(px(8.))
        .text_size(px(ROW_TEXT))
        .cursor_pointer()
        .relative()
        .when_some(fill, |row, fill| row.bg(fill).text_color(palette.foreground))
        // The fill fades under the pointer; a selected or picked row is filled already.
        .when(fill.is_none(), |row| {
            row.child(crate::motion::hover_fill("hover", palette.hover, 8.))
        })
        // Hover is set on every row, selected or not: left off a selected row, it kept the last hover's bright text
        // once the row lost its selection.
        .hover(move |style| style.text_color(palette.foreground))
        .when(!selected, |row| row.text_color(palette.secondary))
        .active(|style| style.opacity(0.85))
        .when_some(mark, |row, mark| {
            row.child(if picked { picked_mark(palette) } else { mark })
        })
        // The row's text: its title, and a named session's name. A person picks a session by its name, so the name
        // shows whole up to `ROW_NAME_SHARE` of the text and the title takes the rest. A long title fades out at its
        // right edge (`title_fade.rs`), the name's tag ends in an ellipsis, and with `title_tooltip` hovering the
        // title shows both whole (a Code row shows its card instead).
        .child({
            let tip = row_tip(&title, name);
            let tag = format!("{id}-name");
            let shown = title.clone();
            h_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(px(8.))
                .child(crate::motion::retitle(
                    div()
                        .id("title")
                        .debug_selector(move || selector)
                        .relative()
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .when(title_tooltip, |text| {
                            text.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        })
                        .child(super::title_fade::fading_title(title)),
                    &shown,
                    retitled,
                ))
                .when_some(name, |text, name| {
                    let selector = format!("{tag}={name}");
                    text.child(
                        session_name_tag(name, relative(ROW_NAME_SHARE).into(), palette)
                            .debug_selector(move || selector),
                    )
                })
        })
        .when_some(age, |row, age| row.child(hint(age, palette)))
}

/// A row's hover text: its whole title, and a named session's whole name, which its tag may cut.
pub(super) fn row_tip(title: &SharedString, name: Option<&str>) -> SharedString {
    match name {
        Some(name) => format!("{title} · name: {name}").into(),
        None => title.clone(),
    }
}

/// A picked row's mark: a filled primary dot with a check, in the state mark's 16px box.
fn picked_mark(palette: Palette) -> AnyElement {
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .size(px(14.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(palette.primary)
                .child(glyph(Lucide::Check, palette.primary_foreground).size(px(10.))),
        )
        .into_any_element()
}

/// The box a session's mark is drawn in.
const MARK: f32 = 16.;

/// A Code session's state as a shape in a 16px box (`sidebar-mark.tsx`): a spinning ring while it runs, a filled
/// dot when it needs the person, a red cross when it failed, a hollow ring at rest. Running and needing the person
/// wear their own colors (`Palette::working`, `Palette::waiting`), so they stand out from the grey rows at rest: the
/// spinner turns on a faint track, and the waiting dot breathes a soft ring around it.
/// The moving shapes are `sidebar_marks::moving`.
pub(super) fn status_mark(id: &str, status: Status, palette: Palette) -> AnyElement {
    let frame = div()
        .relative()
        .size(px(MARK))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    match status {
        // In the sidebar's list, a moving shape is drawn over the list rather than in it (`sidebar_marks.rs`).
        Status::Running => frame
            .child(
                div()
                    .absolute()
                    .size(px(12.))
                    .rounded_full()
                    .border(px(1.5))
                    .border_color(palette.working().opacity(0.25)),
            )
            .child(
                super::sidebar_marks::still(id, status, palette)
                    .unwrap_or_else(|| super::sidebar_marks::moving(id, status, palette)),
            )
            .into_any_element(),
        // The breathing ring and its dot, one shape (`sidebar_marks::moving`), centred in the frame.
        Status::NeedsYou => frame
            .child(
                super::sidebar_marks::still(id, status, palette)
                    .unwrap_or_else(|| super::sidebar_marks::moving(id, status, palette)),
            )
            .into_any_element(),
        // A faint ring in the working colour with a dot going round it; an unread reply adds a dot in its middle. The
        // ring, both dots and the turn are one shape (`sidebar_marks::moving`), drawn from one centre. Drawn apart, the
        // ring and the unread dot snapped to different device pixels at a fractional scale, and sat off each other.
        Status::Background | Status::BackgroundUnread => {
            let at = px((MARK - super::sidebar_marks::ORBIT) / 2.);
            frame
                .child(
                    div()
                        .absolute()
                        .top(at)
                        .left(at)
                        .size(px(super::sidebar_marks::ORBIT))
                        .child(
                            super::sidebar_marks::still(id, status, palette)
                                .unwrap_or_else(|| super::sidebar_marks::moving(id, status, palette)),
                        ),
                )
                .into_any_element()
        }
        Status::Failed => frame
            .child(glyph(Lucide::X, palette.destructive).size(px(12.)))
            .into_any_element(),
        Status::Idle => frame
            .child(
                div()
                    .size(px(8.))
                    .rounded_full()
                    .border(px(1.5))
                    .border_color(palette.faint),
            )
            .into_any_element(),
    }
}

/// An age heading in the Sessions list (Today, This week, Older): the web's `.wb-age`, smaller than a group's title.
fn age_heading(label: &'static str, palette: Palette) -> impl IntoElement {
    div()
        .h(px(24.))
        .pt(px(8.))
        .px(px(8.))
        .text_size(px(11.))
        .text_color(palette.faint)
        .child(label)
}

pub(super) fn group_head(title: impl Into<SharedString>, palette: Palette) -> Div {
    let title: SharedString = title.into();
    h_flex()
        .h(px(28.))
        .mt(px(16.))
        .pl(px(8.))
        .pr(px(4.))
        .text_xs()
        .text_color(palette.faint)
        .child(title)
}

/// What a row says in place of its age while its session works or waits on the person: the age tells nothing then,
/// and the word reads at a glance down a long list. None at rest or failed, where the age stays.
pub(super) fn state_word(status: Status, palette: Palette) -> Option<AnyElement> {
    session_state_word(status, 0, palette)
}

/// [`state_word`] for a Code session whose agent left `background` things running: "In background · 2". The hover
/// card says it; the session's row in the list does not ([`row_state_word`]).
pub(super) fn session_state_word(status: Status, background: usize, palette: Palette) -> Option<AnyElement> {
    Some(
        div()
            .flex_none()
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(palette.muted)
            .child(state_text(status, background)?)
            .into_any_element(),
    )
}

/// What a session's row in the sidebar says in place of its age. Work left in the background says nothing there: the
/// row keeps its age, its status mark shows the work, and the hover card (`session_peek.rs`) counts and lists it.
pub(super) fn row_state_word(status: Status, palette: Palette) -> Option<AnyElement> {
    row_state_text(status)?;
    state_word(status, palette)
}

/// The words of [`row_state_word`].
pub(super) fn row_state_text(status: Status) -> Option<SharedString> {
    match status {
        Status::Background | Status::BackgroundUnread => None,
        status => state_text(status, 0),
    }
}

/// The words of [`session_state_word`].
pub(super) fn state_text(status: Status, background: usize) -> Option<SharedString> {
    Some(match status {
        Status::Running => tr("Working"),
        Status::NeedsYou => tr("Needs you"),
        Status::Background | Status::BackgroundUnread if background > 0 => tr("In background · {count}")
            .replace("{count}", &background.to_string())
            .into(),
        Status::Background | Status::BackgroundUnread => tr("In background"),
        Status::Failed | Status::Idle => return None,
    })
}

/// The strongest state among sessions, for a group that hides them (a folded project): needing the person outranks
/// working, and working outranks work left in the background. None when none of them works or waits.
pub(super) fn loudest(statuses: impl IntoIterator<Item = Status>) -> Option<Status> {
    let rank = |status: Status| match status {
        Status::NeedsYou => 3,
        Status::Running => 2,
        Status::Background | Status::BackgroundUnread => 1,
        Status::Failed | Status::Idle => 0,
    };
    statuses
        .into_iter()
        .filter(|status| rank(*status) > 0)
        .max_by_key(|status| rank(*status))
        // A folded group says only that something runs behind it, not that a reply waits in one of its sessions.
        .map(|status| match status {
            Status::BackgroundUnread => Status::Background,
            status => status,
        })
}

/// Ages and shortcut hints: mono, 11px, faint.
pub(super) fn hint(text: impl Into<SharedString>, palette: Palette) -> impl IntoElement {
    div()
        .flex_none()
        .font_family("monospace")
        .text_size(px(11.))
        .text_color(palette.faint)
        .child(text.into())
}

/// A sidebar row's name tag takes at most this share of the row's text (its title and its tag). A person picks a
/// session by its name, so the name comes first and the title takes what is left; past the share the name is cut too.
/// The row's hover text carries the whole title and name.
pub(super) const ROW_NAME_SHARE: f32 = 0.6;
/// The top bar's name tag is at most this wide: a 40-character name is cut there.
pub(super) const TITLE_NAME_MAX: f32 = 120.;

/// A named session's name (`nexus_workbench::session_name`), as the app's small tag (`ui::badge`), at most `max` wide
/// and cut with an ellipsis.
pub(super) fn session_name_tag(name: &str, max: Length, palette: Palette) -> Div {
    crate::pages::ui::badge(name.to_string(), palette).max_w(max).truncate()
}

fn count(count: usize, palette: Palette) -> impl IntoElement {
    div()
        .px(px(7.))
        .rounded_full()
        // The Frost board's count chip: ink at 7% under the primary text, a quiet pill rather than a black slab.
        .bg(if palette.dark {
            gpui_kit::white().opacity(0.07)
        } else {
            gpui_kit::black().opacity(0.07)
        })
        .font_family("monospace")
        .text_size(px(11.5))
        .text_color(palette.foreground)
        .child(if count >= 50 {
            "50+".to_string()
        } else {
            count.to_string()
        })
}

pub(super) fn note(text: SharedString, color: Hsla) -> impl IntoElement {
    div().px(px(8.)).py(px(6.)).text_xs().text_color(color).child(text)
}

/// A 22px icon button for a row's own actions.
pub(super) fn small_button(id: impl Into<ElementId>, icon: Lucide, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(22.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .hover(|style| style.bg(palette.selected))
        .child(glyph(icon, palette.icon).size(px(14.)))
}

fn icon_button(id: &'static str, icon: Lucide, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .active(|style| style.opacity(0.8))
        .child(glyph(icon, palette.icon).size(px(16.)))
}

fn menu_row(id: &'static str, icon: Lucide, label: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    let label: SharedString = label.into();
    h_flex()
        .id(id)
        .h(px(30.))
        .px(px(8.))
        .gap(px(8.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|style| menu_item_hover(style, palette))
        .child(glyph(icon, palette.icon).size(px(16.)))
        .child(label)
}

/// Rows of grey bars, breathing, until the list arrives.
pub(super) fn render_skeleton(palette: Palette, rows: usize) -> impl IntoElement {
    let widths = [0.7, 0.85, 0.6, 0.75, 0.5, 0.8];
    v_flex()
        .pt(px(8.))
        .gap(px(14.))
        .px(px(8.))
        .children(
            widths
                .into_iter()
                .take(rows)
                .map(|width| div().h(px(10.)).w(relative(width)).rounded(px(4.)).bg(palette.selected)),
        )
        .with_animation(
            "sessions-loading",
            Animation::new(motion::PULSE)
                .repeat()
                .with_max_fps(crate::motion::LOOP_FPS)
                .with_easing(pulsating_between(0.4, 0.9)),
            |column, delta| column.opacity(delta),
        )
}

#[cfg(test)]
mod tests {
    use super::{loudest, row_state_text, state_text};
    use crate::code::Status;

    #[test]
    fn a_folded_project_shows_its_loudest_session() {
        use Status::*;
        assert_eq!(loudest([Idle, Running, Failed]), Some(Running));
        assert_eq!(loudest([Running, NeedsYou, Running]), Some(NeedsYou));
        assert_eq!(loudest([NeedsYou, Running]), Some(NeedsYou));
        assert_eq!(loudest([Idle, Failed]), None);
        assert_eq!(loudest([]), None);
        assert_eq!(loudest([Idle, BackgroundUnread]), Some(Background));
        assert_eq!(loudest([Background, Running]), Some(Running));
        assert_eq!(loudest([Background, NeedsYou]), Some(NeedsYou));
    }

    #[test]
    fn a_session_row_keeps_its_age_while_work_runs_in_the_background() {
        use Status::*;
        // The status mark shows the work and the hover card counts it: the row says nothing about it.
        assert_eq!(row_state_text(Background), None);
        assert_eq!(row_state_text(BackgroundUnread), None);
        assert_eq!(row_state_text(Running).as_deref(), Some("Working"));
        assert_eq!(row_state_text(NeedsYou).as_deref(), Some("Needs you"));
        assert_eq!(row_state_text(Idle), None);
    }

    #[test]
    fn the_hover_card_counts_what_runs_in_the_background() {
        use Status::*;
        assert_eq!(state_text(Background, 2).as_deref(), Some("In background · 2"));
        assert_eq!(state_text(BackgroundUnread, 1).as_deref(), Some("In background · 1"));
        // An older host sends no list: no count.
        assert_eq!(state_text(Background, 0).as_deref(), Some("In background"));
        assert_eq!(state_text(Running, 3).as_deref(), Some("Working"));
        assert_eq!(state_text(Idle, 3), None);
    }
}
