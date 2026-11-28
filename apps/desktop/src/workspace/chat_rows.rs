//! A Gateway chat's row in the sidebar, with the menu Code rows have: "…" on hover or a right-click opens Rename,
//! Archive and Delete. Archived chats and Code sessions wait in the folded "Archived" group at the sidebar's foot,
//! where each one can come back ("Restore from the archive") or go for good ("Delete…").

use crate::workspace::MenuTrigger;
use crate::workspace::menu_item_hover;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

use super::code_dock::menu_box;
use super::sidebar::{ROW, ROW_TEXT, hint, small_button, state_word, thread_row};
use super::{View, Workspace, glyph};
use crate::code::{self, Status};
use crate::gateway::Verb;
use crate::model::SessionSummary;
use crate::motion;
use crate::pages::fetch;
use crate::palette::Palette;

/// The chat row whose menu is open.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RowMenu {
    pub id: String,
    pub archived: bool,
    /// "Delete it and its messages?", with Delete and Keep.
    pub confirm: bool,
    /// The "Move to" list of projects, in place of the menu (`sidebar_groups.rs`).
    pub moving: bool,
}

impl Workspace {
    /// A chat's row: its mark, title and age, with "…" opening beside the age on hover.
    pub(super) fn render_chat_row(
        &self,
        session: &SessionSummary,
        archived: bool,
        now: i64,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(renaming) = self
            .renaming
            .as_ref()
            .filter(|renaming| renaming.chat && renaming.id == session.id)
        {
            return h_flex()
                .h(px(ROW))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .bg(palette.selected)
                .text_size(px(ROW_TEXT))
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.renaming = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(gpui_kit::component::input::Input::new(&renaming.input).appearance(false)),
                )
                .into_any_element();
        }
        let selected = matches!(self.view, View::Chat) && self.current.as_deref() == Some(session.id.as_str());
        let menu_open = self.overlay.row_menu().is_some_and(|menu| menu.id == session.id);
        let open_menu = {
            let id = session.id.clone();
            move |this: &mut Workspace, cx: &mut Context<Workspace>| {
                match this.overlay.row_menu() {
                    Some(menu) if menu.id == id => {
                        this.overlay.close_row_menu();
                    }
                    _ => this.open_overlay(Overlay::RowMenu(RowMenu {
                        id: id.clone(),
                        archived,
                        confirm: false,
                        moving: false,
                    })),
                }
                cx.notify();
            }
        };
        let more = {
            let open_menu = open_menu.clone();
            small_button(
                SharedString::from(format!("chat-more-{}", session.id)),
                Lucide::Ellipsis,
                palette,
            )
            .menu_trigger(
                menu_open,
                cx,
                {
                    let id = session.id.clone();
                    move |this| this.overlay.row_menu().is_some_and(|menu| menu.id == id)
                },
                move |this, _, cx| open_menu(this, cx),
            )
        };
        let state = if self.chat_answering(&session.id) {
            Status::Running
        } else {
            Status::Idle
        };
        let actions_open = menu_open || self.hovered_chat.as_deref() == Some(session.id.as_str());
        let tail = h_flex()
            .flex_none()
            .h_full()
            .items_center()
            // While the chat answers, the row says so in place of its age. It stays while "…" shows beside it.
            .children(match state_word(state, palette) {
                Some(word) => Some(word),
                None => session
                    .at_ms
                    .map(|at| hint(code::age(at, now), palette).into_any_element()),
            })
            // Closed, "…" takes no width, so the state rests at the row's right edge.
            .child(motion::collapse_width(
                SharedString::from(format!("chat-actions-{}", session.id)),
                !actions_open,
                0.,
                ROW,
                div().pl(px(4.)).child(more),
            ));
        let clicked = session.clone();
        thread_row(
            SharedString::from(format!("chat-{}", session.id)),
            session.title.clone().into(),
            None,
            None,
            // A chat answering, open or in the background, spins as a working Code session. At rest it has no mark,
            // as the web's session rows have none: marks belong to agent runs.
            (state == Status::Running).then_some((session.id.as_str(), state)),
            selected,
            false,
            true,
            None,
            palette,
        )
        .on_hover({
            let id = session.id.clone();
            cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered_chat = Some(id.clone());
                    this.preload_chat(&id);
                } else if this.hovered_chat.as_ref() == Some(&id) {
                    this.hovered_chat = None;
                }
                cx.notify();
            })
        })
        .relative()
        // In the archive group the indent sits inside the row, so its pill runs the full width.
        .when(archived, |row| row.pl(px(24.)))
        .child(tail)
        .on_click(cx.listener(move |this, _, window, cx| this.open(&clicked, window, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                open_menu(this, cx);
            }),
        )
        .when(menu_open, |row| row.child(self.render_row_menu(session, palette, cx)))
        .into_any_element()
    }

    fn render_row_menu(&self, session: &SessionSummary, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = self.overlay.row_menu().cloned() else {
            return div().into_any_element();
        };
        let item = |key: &str, icon: Lucide, label: &'static str, danger: bool| {
            h_flex()
                .id(SharedString::from(format!("chat-row-{key}-{}", session.id)))
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(danger, |row| row.text_color(palette.destructive))
                .hover(|style| menu_item_hover(style, palette))
                .child(glyph(icon, if danger { palette.destructive } else { palette.icon }).size(px(16.)))
                .child(label)
        };
        let id = session.id.clone();
        let mut body = menu_box(SharedString::from(format!("chat-row-menu-{id}")), 220., palette)
            .text_size(px(ROW_TEXT))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.overlay.close_row_menu();
                cx.notify();
            }));
        if menu.moving {
            body = body.child(self.render_move_list(&id, palette, cx));
        } else if menu.confirm {
            body = body
                .child(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_color(palette.foreground)
                        .child("Delete it and its messages?"),
                )
                .child(
                    h_flex()
                        .justify_end()
                        .gap(px(4.))
                        .child(
                            item("keep", Lucide::X, "Keep", false).on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.overlay.close_row_menu();
                                cx.notify();
                            })),
                        )
                        .child(item("delete-it", Lucide::Trash, "Delete", true).on_click({
                            let id = id.clone();
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.delete_chat(id.clone(), window, cx);
                            })
                        })),
                );
        } else {
            if !menu.archived {
                let (id, title) = (id.clone(), session.title.clone());
                body = body.child(item("rename", Lucide::Pencil, "Rename", false).on_click(cx.listener(
                    move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.begin_rename(id.clone(), title.clone(), true, window, cx);
                    },
                )));
            }
            if !menu.archived {
                body = body.child(
                    item("move", Lucide::FolderOpen, "Move to project…", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            cx.stop_propagation();
                            if let Some(menu) = this.overlay.row_menu_mut() {
                                menu.moving = true;
                            }
                            cx.notify();
                        },
                    )),
                );
            }
            body = body
                .child(
                    item(
                        "archive",
                        if menu.archived {
                            Lucide::ArchiveRestore
                        } else {
                            Lucide::Archive
                        },
                        if menu.archived {
                            "Restore from the archive"
                        } else {
                            "Archive"
                        },
                        false,
                    )
                    .on_click({
                        let id = id.clone();
                        let archived = menu.archived;
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_chat_archived(id.clone(), !archived, cx);
                        })
                    }),
                )
                .child(div().h(px(8.)))
                .child(
                    item("delete", Lucide::Trash, "Delete…", true).on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(menu) = this.overlay.row_menu_mut() {
                            menu.confirm = true;
                        }
                        cx.notify();
                    })),
                );
        }
        div()
            .absolute()
            .right_0()
            .top(px(ROW))
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopRight)
                        .offset(point(px(0.), px(2.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(body),
                )
                .with_priority(1),
            )
            .into_any_element()
    }

    /// Read the archived chats (`archived=only`) and the archived Code sessions.
    pub(super) fn load_archived(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        self.refresh_code(cx);
        let request = account_gateway.request(Verb::Get, "/v1/chat/sessions?limit=50&archived=only".into(), None);
        fetch(cx, request, |this, answer, _| {
            this.archived_chats = Some(
                answer
                    .map(|page| {
                        page.get("sessions")
                            .and_then(Value::as_array)
                            .map(|sessions| sessions.iter().filter_map(SessionSummary::from_json).collect())
                            .unwrap_or_default()
                    })
                    .map_err(|error| format!("Couldn't load the archive: {error}")),
            );
        });
    }

    /// The folded "Archived" group at the sidebar's foot: archived chats, then archived Code sessions.
    pub(super) fn render_archived(
        &self,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let now = code::now_ms();
        let open = self.archived_open;
        let head = self
            .history_head("Archived", Lucide::Archive, None, open, palette, cx)
            .on_click(cx.listener(|this, _, _, cx| {
                this.archived_open = !this.archived_open;
                if this.archived_open {
                    this.load_archived(cx);
                }
                // The same fold as a project's sessions (`render_projects`).
                this.archived_fold_motion
                    .retarget(if this.archived_open { 1. } else { 0. }, motion::UI_LONG);
                cx.notify();
            }));
        let expansion = self.archived_fold_motion;
        let mut rows: Vec<AnyElement> = Vec::new();
        // While it folds shut, it keeps its rows until the motion ends. Their moving marks fold and fade with them
        // (`sidebar_marks::in_place_while`).
        if expansion.visible() {
            super::sidebar_marks::in_place_while(expansion.moving(), || {
                match &self.archived_chats {
                    None => rows.push(note("Loading…", palette)),
                    Some(Err(error)) => rows.push(note(error, palette)),
                    Some(Ok(chats)) => rows.extend(
                        chats
                            .iter()
                            .map(|session| self.render_chat_row(session, true, now, palette, cx)),
                    ),
                }
                rows.extend(self.archived_code.iter().map(|session| {
                    let row = self.render_session_row(session, None, now, palette, window, cx);
                    self.drag_row(row, session, super::session_drag::Place::Archived, 0, None, palette, cx)
                }));
                rows.extend(
                    self.archived_projects
                        .iter()
                        .map(|project| self.render_archived_project(project, palette, cx)),
                );
            });
            if matches!(&self.archived_chats, Some(Ok(chats)) if chats.is_empty())
                && self.archived_code.is_empty()
                && self.archived_projects.is_empty()
            {
                rows.push(note("Nothing archived", palette));
            }
        }
        let height = if expansion.visible() {
            let chat_height = rows.len().saturating_sub(self.archived_code.len()) as f32 * ROW;
            chat_height
                + self.row_exits.height(
                    self.archived_code
                        .iter()
                        .map(|session| (session.id.as_str(), super::session_tree::row_height(session))),
                    std::time::Instant::now(),
                    cx.reduce_motion(),
                )
        } else {
            0.
        };
        let body = expansion.draw(
            div().flex_none().child(v_flex().flex_none().children(rows)),
            "archived-rows".to_string(),
            move |frame, value| {
                frame
                    .h(px(height * value))
                    .opacity(value)
                    .when(value < 1., |frame| frame.overflow_hidden())
            },
        );
        // A session dropped on the group, its head or its rows, is archived, with the same Undo as the menu's Archive.
        let place = super::session_drag::Place::Archived;
        self.drop_zone(
            v_flex().child(head).child(body).into_any_element(),
            format!("{place:?}"),
            super::session_drag::Target::Group(place),
            palette,
            cx,
        )
    }

    /// An archived project's row in the "Archived" group: its folder and name, and Unarchive project, which brings it
    /// and its sessions back to the list, as the Nexus app's menu does.
    fn render_archived_project(
        &self,
        project: &code::ArchivedProject,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = project.id.clone();
        let unarchive = small_button(
            SharedString::from(format!("archived-project-unarchive-{}", project.id)),
            Lucide::ArchiveRestore,
            palette,
        )
        .debug_selector(|| "archived-project-unarchive".into())
        .tooltip(|window, cx| Tooltip::new("Unarchive project").build(window, cx))
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.update_project(id.clone(), None, Some(false), cx);
        }));
        h_flex()
            .id(SharedString::from(format!("archived-project-{}", project.id)))
            .debug_selector(|| "archived-project".into())
            .h(px(ROW))
            .pl(px(32.))
            .pr(px(4.))
            .gap(px(8.))
            .items_center()
            .rounded(px(8.))
            .text_size(px(ROW_TEXT))
            .text_color(palette.muted)
            .hover(|style| style.bg(palette.hover))
            .child(glyph(Lucide::Folder, palette.icon).flex_none().size(px(14.)))
            .child(div().flex_1().min_w(px(0.)).truncate().child(project.name.clone()))
            .child(unarchive)
            .into_any_element()
    }
}

fn note(text: &str, palette: Palette) -> AnyElement {
    // In line with the rows' titles.
    // One ROW tall, as the rows, so the fold knows its height.
    div()
        .h(px(ROW))
        .flex()
        .items_center()
        .pl(px(32.))
        .pr(px(8.))
        .text_xs()
        .text_color(palette.faint)
        .child(div().min_w(px(0.)).truncate().child(text.to_string()))
        .into_any_element()
}

#[cfg(test)]
#[path = "archived_projects_tests.rs"]
mod archived_projects_tests;
