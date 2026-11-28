//! A Code session's row in the sidebar and its menu, as the Nexus app has them (`workbench/sidebar-menu.tsx`,
//! `snooze-menu.tsx`): a "…" on hover (or a right-click) opens Rename, Pin to the top, Archive, Snooze, Settle, Keep in
//! list, Copy branch name and Delete. Rename edits the title in place. Snoozed and settled sessions leave their project for
//! the folded "Snoozed" and "Settled" groups under the projects (`workbench/snooze.ts`).

use crate::workspace::MenuTrigger;
use crate::workspace::menu_item_hover;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::api::UpdateArgs;

use super::code_dock::menu_box;
use super::row_exit;
use super::sidebar::{ROW, ROW_TEXT, hint, small_button, thread_row_marked};
use super::{View, Workspace, glyph};
use crate::cloud::words::End;
use crate::code::{self, Aside, CodeSession};
use crate::motion;
use crate::palette::Palette;
use crate::snooze;

/// One retained handle keeps the original More button focusable after its dialog closes.
pub(super) struct SessionAnchor {
    id: String,
    in_title: bool,
    place: Option<super::Place>,
    handle: FocusHandle,
}

/// The session whose menu is open, and which of its pages shows.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SessionMenu {
    pub id: String,
    pub page: MenuPage,
    /// Opened from the open session's top bar, not its row: it holds that session's own actions too.
    pub in_title: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum MenuPage {
    Main,
    Export,
    Snooze,
    Fork,
    RestartOrchestration,
    MoveWorktree,
    SnoozeCustom,
    /// "Delete it and its messages?", with Delete and Keep.
    Delete,
}

/// The custom snooze form owns its input and its subscription while it is open.
pub(super) struct CustomSnooze {
    pub input: Entity<InputState>,
    _subscription: Subscription,
}

/// The session whose title is being edited in its row.
pub(super) struct Renaming {
    pub id: String,
    /// A Gateway chat's row, not a Code session's.
    pub chat: bool,
    /// Edited in the top bar's title, not in the row.
    pub in_title: bool,
    pub input: Entity<InputState>,
    _subscription: Subscription,
}

impl Workspace {
    fn return_session_to_list(&mut self, id: &str, aside: Aside, cx: &mut Context<Self>) {
        self.change_session(
            UpdateArgs {
                thread_id: id.to_string(),
                snoozed_until: (aside == Aside::Snoozed).then_some(0),
                keep_listed: (aside == Aside::Settled).then_some(true),
                ..UpdateArgs::default()
            },
            cx,
        );
    }

    pub(super) fn remember_session_anchor(&mut self, id: &str, in_title: bool, cx: &mut Context<Self>) {
        self.session_anchor = Some(SessionAnchor {
            id: id.into(),
            in_title,
            place: self.history.get(self.history_at).cloned(),
            handle: cx.focus_handle(),
        });
    }

    pub(super) fn session_anchor_handle(&self, id: &str, in_title: bool) -> Option<&FocusHandle> {
        self.session_anchor
            .as_ref()
            .filter(|anchor| anchor.id == id && anchor.in_title == in_title)
            .map(|anchor| &anchor.handle)
    }

    pub(super) fn restore_session_anchor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(anchor) = &self.session_anchor else { return };
        let same_place = anchor.place.as_ref() == self.history.get(self.history_at);
        let connected = self.session_by_id(&anchor.id).is_some()
            && if anchor.in_title {
                matches!(&self.view, View::Code(view) if view.id == anchor.id)
            } else {
                self.sidebar_open
            };
        if same_place && connected {
            window.focus(&anchor.handle, cx);
        } else {
            self.session_anchor = None;
        }
    }

    fn begin_custom_snooze(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(
                nexus_workbench::snooze::custom_default(chrono::Local::now()),
                window,
                cx,
            );
            state
        });
        let subscription = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                if let Some(at) = nexus_workbench::snooze::custom_time(&input.read(cx).value(), chrono::Local::now())
                    && let Some(menu) = this.overlay.session_menu()
                    && menu.page == MenuPage::SnoozeCustom
                {
                    let thread_id = menu.id.clone();
                    this.custom_snooze = None;
                    this.change_session(
                        UpdateArgs {
                            thread_id,
                            snoozed_until: Some(at),
                            ..UpdateArgs::default()
                        },
                        cx,
                    );
                }
            }
            cx.notify();
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        self.custom_snooze = Some(CustomSnooze {
            input,
            _subscription: subscription,
        });
        self.set_menu_page(MenuPage::SnoozeCustom, cx);
    }

    /// A Code session's row: its mark, title and age, with "…" opening beside the age on hover.
    pub(super) fn render_session_row(
        &self,
        session: &CodeSession,
        aside: Option<Aside>,
        now: i64,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_session_row_at(session, 0, false, aside, now, palette, window, cx)
    }

    /// A session's row at `depth` in its project's tree: a linked session's row sits 8px in from its line
    /// (`.wb-linked-threads .wb-thread`), a top one 24px. A row in a crew (`session_tree::in_crew`) or handed off to
    /// names its agent; a handed-off one says under its title which agent it came from.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_session_row_at(
        &self,
        session: &CodeSession,
        depth: usize,
        crew: bool,
        aside: Option<Aside>,
        now: i64,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let indent = if depth > 0 { 8. } else { 24. };
        if let Some(renaming) = self
            .renaming
            .as_ref()
            .filter(|renaming| !renaming.chat && !renaming.in_title && renaming.id == session.id)
        {
            return h_flex()
                .h(px(ROW))
                .pl(px(indent))
                .pr(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .bg(palette.selected)
                .text_size(px(ROW_TEXT))
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.renaming = None;
                    this.session_anchor = None;
                    cx.notify();
                }))
                .child(super::sidebar::status_mark(
                    &session.id,
                    self.session_mark(session, cx),
                    palette,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(Input::new(&renaming.input).appearance(false)),
                )
                .into_any_element();
        }
        let selected = matches!(&self.view, View::Code(open) if open.id == session.id);
        // Deleted: the row leaves, or comes back when the delete failed (`row_exit.rs`).
        let exit = self
            .row_exits
            .frame(&session.id, std::time::Instant::now(), cx.reduce_motion());
        let menu_open = self
            .overlay
            .session_menu()
            .as_ref()
            .is_some_and(|menu| menu.id == session.id && !menu.in_title);
        let clicked = session.id.clone();
        let more = {
            let id = session.id.clone();
            small_button(
                SharedString::from(format!("session-more-{}", session.id)),
                Lucide::Ellipsis,
                palette,
            )
            .menu_trigger(
                menu_open,
                cx,
                {
                    let id = id.clone();
                    move |this| {
                        this.overlay
                            .session_menu()
                            .as_ref()
                            .is_some_and(|menu| menu.id == id && !menu.in_title)
                    }
                },
                move |this, window, cx| this.toggle_session_menu(&id, false, window, cx),
            )
        };
        // The return button is a tab stop: kept across frames, so its focus opens the actions too.
        let return_focus = aside.map(|_| {
            window
                .use_keyed_state(
                    SharedString::from(format!("session-return-focus-{}", session.id)),
                    cx,
                    |_, cx| cx.focus_handle().tab_stop(true),
                )
                .read(cx)
                .clone()
        });
        let focused = self
            .session_anchor_handle(&session.id, false)
            .into_iter()
            .chain(&return_focus)
            .any(|handle| handle.is_focused(window));
        // The actions open beside the state on hover, with the menu open, or with the keyboard on "…": the state
        // stays in sight while they show.
        let actions_open = menu_open || focused || self.peek.hovers(&session.id);
        let mark = self.session_mark(session, cx);
        let actions = h_flex()
            .pl(px(4.))
            .gap(px(2.))
            .items_center()
            .when_some(aside, |cluster, aside| {
                let id = session.id.clone();
                let (icon, label) = match aside {
                    Aside::Snoozed => (Lucide::AlarmClockOff, "Unsnooze"),
                    Aside::Settled => (Lucide::ListPlus, "Keep in list"),
                };
                cluster.child(
                    small_button(SharedString::from(format!("session-return-{id}")), icon, palette)
                        .when_some(return_focus.as_ref(), |button, focus| {
                            button.track_focus(focus).tab_index(0)
                        })
                        .focus(|style| style.bg(palette.selected))
                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.return_session_to_list(&id, aside, cx);
                        })),
                )
            })
            .child(
                more.when_some(self.session_anchor_handle(&session.id, false), |button, focus| {
                    button.track_focus(focus).tab_index(0)
                }),
            );
        // A session on a cloud machine shows a cloud for its idle mark, and its machine's state before its age.
        let cloud = self.cloud_machine_running(&session.id);
        let machine_state = cloud.and_then(|machine| super::sidebar_machines::machine_state_mark(machine, palette));
        let tail = h_flex()
            .flex_none()
            .h_full()
            .items_center()
            .child(
                h_flex()
                    .gap(px(6.))
                    .when(crew || session.handoff_from.is_some(), |tail| {
                        tail.child(
                            div()
                                .text_size(px(11.))
                                .text_color(palette.faint)
                                .child(crate::agents::ui(session.agent).short_name()),
                        )
                    })
                    .when_some(pull_badge(&session.id, palette), |age, badge| age.child(badge))
                    .children(machine_state)
                    // While the session works or waits on the person, the row says so in place of its age. Work left
                    // in the background keeps the age: the status mark shows it, and the hover card counts it.
                    .child(
                        super::sidebar::row_state_word(mark, palette)
                            .unwrap_or_else(|| hint(code::age(session.updated_at, now), palette).into_any_element()),
                    ),
            )
            // Closed, the actions take no width, so the state rests at the row's right edge.
            .child(motion::collapse_width(
                SharedString::from(format!("session-actions-{}", session.id)),
                !actions_open,
                0.,
                ROW,
                actions,
            ));
        let picked = self.sidebar_filter.picked.has(&session.id);
        let mark_element = match cloud {
            Some(_) if mark == crate::code::Status::Idle => super::sidebar_machines::cloud_glyph(palette),
            _ => super::sidebar::status_mark(&session.id, mark, palette),
        };
        let row = thread_row_marked(
            SharedString::from(format!("code-{}", session.id)),
            session.title.clone().into(),
            session.name.as_deref().filter(|name| !name.is_empty()),
            None,
            Some(mark_element),
            selected,
            picked,
            // The card shows the whole title, so the title has no tooltip of its own.
            false,
            None,
            palette,
        )
        .on_hover({
            let id = session.id.clone();
            cx.listener(move |this, hovered: &bool, _, cx| this.hover_session_row(&id, *hovered, cx))
        })
        // A press opens the session or its menu: the card gets out of the way.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.peek.dismiss();
                cx.notify();
            }),
        )
        .relative()
        // Session rows sit under a project or group: the indent is inside the row, so its pill runs from x=8.
        .pl(px(indent))
        .when(session.pinned, |row| {
            row.child(glyph(Lucide::Pin, palette.faint).size(px(12.)))
        })
        .child(tail)
        // The card hangs from the row's right edge, beside the sidebar.
        .children(if menu_open || exit.is_some_and(|exit| exit.leaving) {
            None
        } else {
            self.render_session_peek(session, mark, now, palette, window, cx)
        })
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            if !this.pick_click(&clicked, event.modifiers(), cx) {
                this.open_code(&clicked, window, cx);
            }
        }))
        .on_mouse_down(MouseButton::Right, {
            let id = session.id.clone();
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.peek.dismiss();
                this.remember_session_anchor(&id, false, cx);
                this.open_overlay(Overlay::SessionMenu(SessionMenu {
                    id: id.clone(),
                    page: MenuPage::Main,
                    in_title: false,
                }));
                cx.notify();
            })
        })
        .when_some(
            // While it leaves, the page it was on (the state has let go of it already).
            motion::presence_of(
                format!("session-menu-row-{}", session.id),
                self.overlay
                    .session_menu()
                    .as_ref()
                    .filter(|menu| menu.id == session.id && !menu.in_title)
                    .map(|menu| menu.page),
                cx,
            ),
            |row, (phase, page)| row.child(self.render_session_menu(session, now, phase, page, false, palette, cx)),
        );
        let row: AnyElement = match super::session_tree::handoff_line(session) {
            Some(line) => v_flex()
                .child(row)
                .child(
                    div()
                        .pl(px(indent + 24.))
                        .h(px(super::session_tree::HANDOFF_CAPTION_HEIGHT))
                        .line_height(px(super::session_tree::HANDOFF_CAPTION_HEIGHT))
                        .text_size(px(11.))
                        .text_color(palette.faint)
                        .child(line),
                )
                .into_any_element(),
            None => row.into_any_element(),
        };
        match exit {
            Some(frame) => row_exit::draw(
                row,
                &session.id,
                super::session_tree::row_height(session),
                frame,
                palette,
                window,
            ),
            None => row,
        }
    }

    /// The "Snoozed" and "Settled" groups under the projects, folded until clicked, each with its count.
    pub(super) fn render_aside_groups(
        &self,
        now: i64,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open_id = match &self.view {
            View::Code(open) => Some(open.id.as_str()),
            _ => None,
        };
        let groups = [Aside::Snoozed, Aside::Settled].map(|group| {
            let sessions: Vec<&CodeSession> = self
                .projects
                .iter()
                .flat_map(|project| &project.sessions)
                .filter(|session| {
                    let open = Some(session.id.as_str()) == open_id;
                    session.shown_aside(now, open) == Some(group)
                })
                .collect();
            (group, sessions)
        });
        let mut drawn: Vec<AnyElement> = groups
            .into_iter()
            .map(|(group, sessions)| self.render_aside_slot(group, sessions, palette, window, cx))
            .collect();
        // During a drag that rests over "Snoozed", its presets open under it (`session_snooze_drops.rs`).
        drawn.insert(1, self.render_snooze_drops(palette, cx));
        v_flex().children(drawn)
    }

    /// One aside group, or, empty, the target it opens as while a dragged session could go in it.
    fn render_aside_slot(
        &self,
        group: Aside,
        sessions: Vec<&CodeSession>,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        {
            let place = super::session_drag::Place::Aside(group);
            let icon = match group {
                Aside::Snoozed => Lucide::AlarmClock,
                Aside::Settled => Lucide::CircleCheck,
            };
            // An empty group opens only while a dragged session could go in it: a place to drop it.
            if sessions.is_empty() {
                let open = self.drag_could_land(&place, cx);
                let head = self.history_head(group.label(), icon, None, false, palette, cx);
                let zone = self.drop_zone(
                    head.into_any_element(),
                    format!("{place:?}"),
                    super::session_drag::Target::Group(place),
                    palette,
                    cx,
                );
                return motion::reveal(
                    SharedString::from(format!("aside-zone-{}", group.label())),
                    open,
                    Some(zone),
                )
                .into_any_element();
            }
            self.render_aside_group(group, place, icon, sessions, palette, window, cx)
        }
    }

    /// One aside group with sessions: its head, and its rows while it is open.
    #[allow(clippy::too_many_arguments)]
    fn render_aside_group(
        &self,
        group: Aside,
        place: super::session_drag::Place,
        icon: Lucide,
        sessions: Vec<&CodeSession>,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = code::now_ms();
        let open = self.open_aside.contains(&group);
        let expansion = self
            .aside_fold_motion
            .get(&group)
            .copied()
            .unwrap_or_else(|| super::folder_motion::Transition::settled(if open { 1. } else { 0. }));
        let head = self
            .history_head(group.label(), icon, Some(sessions.len()), open, palette, cx)
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.open_aside.remove(&group) {
                    this.open_aside.insert(group);
                }
                let open = this.open_aside.contains(&group);
                // The same fold as a project's sessions (`render_projects`).
                this.aside_fold_motion
                    .entry(group)
                    .or_insert_with(|| super::folder_motion::Transition::settled(if open { 0. } else { 1. }))
                    .retarget(if open { 1. } else { 0. }, motion::UI_LONG);
                cx.notify();
            }));
        // While it folds shut, it keeps its rows until the motion ends. Each row is one ROW tall, less
        // what a deleted one has folded (`row_exit.rs`).
        let height = self.row_exits.height(
            sessions
                .iter()
                .map(|session| (session.id.as_str(), super::session_tree::row_height(session))),
            std::time::Instant::now(),
            cx.reduce_motion(),
        );
        // While the group folds, its rows' moving marks fold and fade with it (`sidebar_marks::in_place_while`).
        let rows: Vec<AnyElement> = super::sidebar_marks::in_place_while(expansion.moving(), || {
            if !expansion.visible() {
                return Vec::new();
            }
            sessions
                .into_iter()
                .map(|session| {
                    let row = self.render_session_row(session, Some(group), now, palette, window, cx);
                    self.drag_row(row, session, place.clone(), 0, None, palette, cx)
                })
                .collect()
        });
        let body = expansion.draw(
            div().flex_none().child(v_flex().flex_none().children(rows)),
            format!("aside-rows-{}", group.label()),
            move |frame, value| {
                frame
                    .h(px(height * value))
                    .opacity(value)
                    .when(value < 1., |frame| frame.overflow_hidden())
            },
        );
        // The whole group takes a dropped session, its rows as well as its head.
        self.drop_zone(
            v_flex().child(head).child(body).into_any_element(),
            format!("{place:?}"),
            super::session_drag::Target::Group(place),
            palette,
            cx,
        )
    }

    pub(super) fn toggle_session_menu(
        &mut self,
        id: &str,
        in_title: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.peek.dismiss();
        if self
            .overlay
            .session_menu()
            .is_some_and(|menu| menu.id == id && menu.in_title == in_title)
        {
            self.restore_session_anchor(window, cx);
            self.overlay.close_session_menu();
        } else {
            self.remember_session_anchor(id, in_title, cx);
            if let Some(anchor) = &self.session_anchor {
                window.focus(&anchor.handle, cx);
            }
            self.open_overlay(Overlay::SessionMenu(SessionMenu {
                id: id.to_string(),
                page: MenuPage::Main,
                in_title,
            }));
        }
        cx.notify();
    }

    pub(super) fn set_menu_page(&mut self, page: MenuPage, cx: &mut Context<Self>) {
        if let Some(menu) = self.overlay.session_menu_mut() {
            menu.page = page;
        }
        cx.notify();
    }

    /// The session's menu (`SessionMenu` in the Nexus app), on `page`, drawn as `phase` (`motion::presence_of`, so a
    /// menu closed from a sub-page leaves showing that page), under the title's button or beside its row.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_session_menu(
        &self,
        session: &CodeSession,
        now: i64,
        phase: motion::Phase,
        page: MenuPage,
        in_title: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let available = self
            .projects
            .iter()
            .flat_map(|project| &project.sessions)
            .chain(&self.archived_code)
            .filter(|candidate| candidate.machine == session.machine)
            .map(|candidate| candidate.id.as_str());
        let source = nexus_workbench::send_back::source_id(&session.id, session.source.as_deref(), available)
            .and_then(|id| self.session_by_id(id))
            .map(|source| (source.id.clone(), source.title.clone()));

        let supported = match &session.machine {
            Some(machine) => crate::machines::get().has_feature(machine, nexus_workbench::moving::FEATURE),
            None => self.runner.is_some(),
        };
        let repository = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .is_some_and(|project| project.branch.is_some());
        let move_offered = nexus_workbench::moving::menu_offered(
            supported,
            session.has_worktree,
            session.parent.is_some(),
            session.side_chat,
            repository,
        );
        let move_blocked = nexus_workbench::moving::menu_blocked(session.activity.status);

        let id = session.id.clone();
        let item = |key: &str, icon: Option<Lucide>, label: SharedString| {
            let selector = format!("session-{key}-{id}");
            h_flex()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector.clone())
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| menu_item_hover(style, palette))
                .children(icon.map(|icon| {
                    let tone = if key == "delete" {
                        palette.destructive
                    } else {
                        palette.icon
                    };
                    glyph(icon, tone).size(px(16.))
                }))
                .child(div().flex_1().truncate().child(label))
        };
        let update = |args: UpdateArgs| {
            cx.listener(move |this: &mut Workspace, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                // A cloud session archives by deleting its machine (`archive.ts`): no Undo.
                if args.archived == Some(true)
                    && let Some(machine) = this.cloud_machine_for(&args.thread_id)
                {
                    this.overlay.close_session_menu();
                    this.end_cloud(&args.thread_id, &machine, End::Archive, false, window, cx);
                    return;
                }
                this.change_session(args.clone(), cx);
            })
        };
        let thread = |id: &str| UpdateArgs {
            thread_id: id.to_string(),
            ..UpdateArgs::default()
        };
        let mut body = menu_box(
            SharedString::from(format!("session-menu-{id}")),
            if matches!(
                page,
                MenuPage::Fork | MenuPage::MoveWorktree | MenuPage::RestartOrchestration
            ) {
                340.
            } else if matches!(page, MenuPage::Snooze | MenuPage::SnoozeCustom) {
                // Its longest row, "Until tomorrow morning" with its wake time, and the custom time page in its place.
                crate::tokens::menu::SNOOZE_WIDTH
            } else {
                crate::tokens::menu::MIN_WIDTH
            },
            palette,
        )
        .text_size(px(ROW_TEXT))
        .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.overlay.close_session_menu();
            this.session_anchor = None;
            cx.notify();
        }));
        let local = chrono::Local::now();
        match page {
            MenuPage::Main => {
                let snoozed = session.snoozed_until.is_some_and(|until| until > now);
                body = body
                    .child(
                        item("open-editor", Some(Lucide::ExternalLink), "Open in editor".into()).on_click({
                            let id = id.clone();
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.overlay.close_session_menu();
                                this.open_session_editor(&id, None, None, cx);
                            })
                        }),
                    )
                    .child(item("rename", Some(Lucide::Pencil), "Rename".into()).on_click({
                        let id = id.clone();
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.start_rename(&id, in_title, window, cx);
                        })
                    }))
                    .child(
                        item("fork", Some(Lucide::GitFork), "Fork".into())
                            .child(glyph(Lucide::ChevronRight, palette.faint).size(px(14.)))
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.open_fork_points(id.clone(), window, cx);
                                    this.probe_plans(cx);
                                    if let Some(runner) = &this.runner {
                                        runner.load_access();
                                    }
                                }
                            })),
                    )
                    .when_some(source, |body, (source_id, title)| {
                        let fork = id.clone();
                        body.child(
                            item("send-back", Some(Lucide::ArrowLeft), "Send back to source".into())
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(nexus_workbench::send_back::hint(&title))
                                        .build(window, cx)
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.send_back_to_source(fork.clone(), source_id.clone(), window, cx);
                                })),
                        )
                    })
                    .when(move_offered, |body| {
                        body.child(
                            item(
                                "move-worktree",
                                Some(Lucide::GitBranch),
                                "Move to a new worktree".into(),
                            )
                            .opacity(if move_blocked.is_some() { 0.45 } else { 1.0 })
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    move_blocked.unwrap_or("Its own branch and folder, with the uncommitted changes"),
                                )
                                .build(window, cx)
                            })
                            .when(move_blocked.is_none(), |item| {
                                item.on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.set_menu_page(MenuPage::MoveWorktree, cx);
                                }))
                            }),
                        )
                    })
                    .child(
                        item("share-artifact", Some(Lucide::Share2), "Share as artifact".into()).on_click({
                            let id = id.clone();
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_session_share(&id, window, cx);
                            })
                        }),
                    )
                    .child(
                        item("export", Some(Lucide::Download), "Export".into())
                            .child(glyph(Lucide::ChevronRight, palette.faint).size(px(14.)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_menu_page(MenuPage::Export, cx);
                            })),
                    )
                    .when(!session.archived, |body| {
                        body.child(
                            item(
                                "pin",
                                Some(if session.pinned { Lucide::PinOff } else { Lucide::Pin }),
                                if session.pinned { "Unpin" } else { "Pin to the top" }.into(),
                            )
                            .on_click(update(UpdateArgs {
                                pinned: Some(!session.pinned),
                                ..thread(&id)
                            })),
                        )
                    })
                    .child(
                        item(
                            "archive",
                            Some(if session.archived {
                                Lucide::ArchiveRestore
                            } else {
                                Lucide::Archive
                            }),
                            if session.archived {
                                "Restore from the archive"
                            } else {
                                "Archive"
                            }
                            .into(),
                        )
                        .on_click(update(UpdateArgs {
                            archived: Some(!session.archived),
                            ..thread(&id)
                        })),
                    )
                    .when(!session.archived, |body| {
                        body.child(
                            item(
                                "snooze",
                                Some(Lucide::AlarmClock),
                                if snoozed { "Snooze until another time" } else { "Snooze" }.into(),
                            )
                            .child(glyph(Lucide::ChevronRight, palette.faint).size(px(14.)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_menu_page(MenuPage::Snooze, cx);
                            })),
                        )
                    })
                    .when(snoozed, |body| {
                        body.child(
                            item("wake", Some(Lucide::AlarmClockOff), "Unsnooze".into()).on_click(update(UpdateArgs {
                                snoozed_until: Some(0),
                                ..thread(&id)
                            })),
                        )
                    })
                    // Settled by hand, as a merge settles it: it leaves the list for the "Settled" group.
                    .when(!session.archived && !session.settled, |body| {
                        body.child(
                            item("settle", Some(Lucide::CircleCheck), "Settle".into()).on_click(update(UpdateArgs {
                                settled: Some(true),
                                ..thread(&id)
                            })),
                        )
                    })
                    .when(session.settled, |body| {
                        body.child(
                            item("keep", Some(Lucide::ListPlus), "Keep in list".into()).on_click(update(UpdateArgs {
                                keep_listed: Some(true),
                                ..thread(&id)
                            })),
                        )
                    })
                    .when_some(session.branch.clone(), |body, branch| {
                        body.child(
                            item("branch", Some(Lucide::GitBranch), "Copy branch name".into()).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    cx.write_to_clipboard(ClipboardItem::new_string(branch.clone()));
                                    this.overlay.close_session_menu();
                                    this.restore_session_anchor(window, cx);
                                    cx.notify();
                                },
                            )),
                        )
                    })
                    .when(in_title, |body| {
                        body.children(self.title_menu_items(session, palette, cx))
                    })
                    .child(div().h(px(8.)))
                    .child(
                        item("delete", Some(Lucide::Trash), "Delete".into())
                            .text_color(palette.destructive)
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_menu_page(MenuPage::Delete, cx);
                            })),
                    );
            }
            MenuPage::Fork => {
                let busy = self.fork_in_progress(&id);
                let points = self
                    .fork_points
                    .as_ref()
                    .filter(|open| open.id == id)
                    .and_then(|open| open.points.as_ref());
                let muted = |text: SharedString| div().px(px(8.)).py(px(6.)).text_color(palette.muted).child(text);
                body = body
                    .child(
                        item("back", Some(Lucide::ChevronLeft), "Fork from which point?".into()).on_click(
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.set_menu_page(MenuPage::Main, cx);
                            }),
                        ),
                    )
                    .child(muted(
                        "A new session in a worktree of its own, with the files as they were then and the conversation up to there."
                            .into(),
                    ));
                let forkable = match points {
                    None => {
                        body = body.child(muted("Reading the turns…".into()));
                        false
                    }
                    Some(Err(error)) => {
                        body = body.child(muted(format!("Couldn't read the turns: {error}").into()));
                        false
                    }
                    Some(Ok(points)) if points.is_empty() => {
                        body = body.child(
                            muted("Nothing to fork yet: no turn has finished with its files recorded.".into())
                                .debug_selector(|| "fork-nothing-yet".into()),
                        );
                        false
                    }
                    Some(Ok(points)) => {
                        for point in points {
                            let turn = point.turn;
                            let label = if busy { "Forking…".into() } else { point.label.clone() };
                            let row = item(&format!("fork-turn-{turn}"), Some(Lucide::GitFork), label.into())
                                .when(!point.text.is_empty(), |row| {
                                    row.child(hint(point.text.clone(), palette))
                                });
                            body = body.child(if busy {
                                row.opacity(0.45).cursor_default()
                            } else {
                                let id = id.clone();
                                row.on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.fork_session(id.clone(), Some(turn), None, window, cx);
                                }))
                            });
                        }
                        !busy
                    }
                };
                if forkable {
                    body = body.child(muted("The latest turn with another subscription:".into()));
                }
                // The probes describe this computer, never a remote server's accounts. A note that has a way on shows
                // it (`place_links.rs`).
                let mut way_on = None;
                let message = if !forkable {
                    None
                } else if session.machine.is_some() {
                    Some("Other subscriptions are available for local sessions.")
                } else if self.code_form.access.is_none() {
                    Some("Checking subscriptions…")
                } else if !self
                    .code_form
                    .access
                    .as_ref()
                    .is_some_and(|access| access.subscriptions)
                {
                    Some("Subscriptions are unavailable under your current organization settings.")
                } else {
                    match &self.plan_usage {
                        None => Some("Checking subscriptions…"),
                        Some((_, Err(_))) => {
                            way_on = Some(self.render_fork_plans_failed(palette, cx));
                            None
                        }
                        Some((_, Ok(plans))) => {
                            let signed: Vec<_> = plans.iter().filter(|plan| plan.signed_in).collect();
                            if signed.is_empty() && !self.plan_waiting.is_empty() {
                                Some("Checking subscriptions…")
                            } else if signed.is_empty() {
                                way_on = Some(self.render_fork_no_subscription(palette, cx));
                                None
                            } else {
                                for (index, plan) in signed.into_iter().enumerate() {
                                    let vendor = crate::pages::subscriptions::vendor(plan.agent);
                                    let account = plan
                                        .email
                                        .as_deref()
                                        .or(plan.account.as_deref())
                                        .unwrap_or("Default account");
                                    let label = format!("{vendor} · {account}");
                                    let target = nexus_workbench::ForkSubscription {
                                        agent: plan.agent,
                                        account: plan.account.clone(),
                                    };
                                    let id = id.clone();
                                    body =
                                        body.child(item(&format!("fork-plan-{index}"), None, label.into()).on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.fork_session(id.clone(), None, Some(target.clone()), window, cx);
                                            }),
                                        ));
                                }
                                None
                            }
                        }
                    }
                };
                if let Some(message) = message {
                    body = body.child(div().px(px(8.)).py(px(6.)).text_color(palette.muted).child(message));
                }
                body = body.children(way_on);
            }
            MenuPage::Snooze => {
                body = body
                    .children(
                        snooze::presets(local)
                            .into_iter()
                            .enumerate()
                            .map(|(index, (label, at))| {
                                item(&format!("snooze-{index}"), None, label.into())
                                    .child(hint(snooze::wake_text(at, local), palette))
                                    .on_click(update(UpdateArgs {
                                        snoozed_until: Some(at),
                                        ..thread(&id)
                                    }))
                            }),
                    )
                    .child(item("custom-time", None, "Custom time…".into()).on_click(cx.listener(
                        |this, _, window, cx| {
                            cx.stop_propagation();
                            this.begin_custom_snooze(window, cx);
                        },
                    )));
            }
            MenuPage::SnoozeCustom => {
                if let Some(custom) = &self.custom_snooze {
                    let value = custom.input.read(cx).value();
                    let at = nexus_workbench::snooze::custom_time(&value, local);
                    body = body
                        .child(div().px(px(8.)).py(px(6.)).child("Snooze until"))
                        .child(div().px(px(8.)).child(Input::new(&custom.input)))
                        .child(
                            div()
                                .px(px(8.))
                                .py(px(6.))
                                .text_size(px(12.))
                                .text_color(palette.muted)
                                .child(if !value.is_empty() && at.is_none() {
                                    "Pick a time later than now."
                                } else {
                                    "Local time · YYYY-MM-DDTHH:MM"
                                }),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap(px(6.))
                                .child(item("cancel-custom", None, "Cancel".into()).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.custom_snooze = None;
                                        this.set_menu_page(MenuPage::Snooze, cx);
                                    },
                                )))
                                .child(
                                    item("save-custom", None, "Snooze".into())
                                        .opacity(if at.is_some() { 1.0 } else { 0.4 })
                                        .when_some(at, |item, at| {
                                            item.on_click(update(UpdateArgs {
                                                snoozed_until: Some(at),
                                                ..thread(&id)
                                            }))
                                        }),
                                ),
                        );
                }
            }
            MenuPage::RestartOrchestration => {
                body = body
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(6.))
                            .child("Restart the agent to add the subsession tools?"),
                    )
                    .child(
                        div()
                            .px(px(8.))
                            .pb(px(6.))
                            .text_size(px(12.))
                            .text_color(palette.muted)
                            .child("Your next message resumes the conversation in the new process."),
                    )
                    .child(
                        item("restart-later", None, "Later".into()).on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.overlay.close_session_menu();
                            cx.notify();
                        })),
                    )
                    .child(item("restart-agent", None, "Restart agent".into()).on_click({
                        let id = id.clone();
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.overlay.close_session_menu();
                            if let Some(runs) = this.runs(&id) {
                                runs.stop(id.clone());
                            }
                            cx.notify();
                        })
                    }));
            }
            MenuPage::MoveWorktree => {
                let project = self
                    .projects
                    .iter()
                    .find(|project| project.id == session.project_id)
                    .map(|project| project.name.as_str())
                    .unwrap_or("the project folder");
                body = body.child(div().px(px(8.)).py(px(6.)).child("Move this session to a new worktree?"))
                    .child(div().px(px(8.)).py(px(6.)).text_size(px(12.)).text_color(palette.muted).child(format!(
                        "The uncommitted changes in {project} go with it, on a branch of its own. {project} is left clean. The agent resumes in the new folder at your next message.")))
                    .children(move_blocked.map(|note| div().px(px(8.)).py(px(6.)).text_color(palette.muted).child(note)))
                    .child(h_flex().justify_end().gap(px(6.))
                        .child(item("cancel-move", None, "Cancel".into()).on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation(); this.overlay.close_session_menu(); cx.notify();
                        })))
                        .child(item("confirm-move", None, "Move".into()).opacity(if move_blocked.is_some() { 0.45 } else { 1.0 })
                            .when(move_blocked.is_none(), |item| item.on_click({
                                let id = id.clone();
                                cx.listener(move |this, _, _, cx| { cx.stop_propagation(); this.move_session_worktree(id.clone(), cx); })
                            }))));
            }
            MenuPage::Export => {
                body = body.child(
                    item("back", Some(Lucide::ChevronLeft), "Export".into()).on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_menu_page(MenuPage::Main, cx);
                    })),
                );
                for (key, icon, label, download) in [
                    ("export-copy", Lucide::Copy, "Copy to clipboard", false),
                    ("export-file", Lucide::Download, "Save Markdown file…", true),
                ] {
                    let id = id.clone();
                    body = body.child(item(key, Some(icon), label.into()).on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.export_code_session(&id, download, cx);
                        },
                    )));
                }
            }
            MenuPage::Delete => {
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
                            .px(px(4.))
                            .pb(px(4.))
                            .gap(px(6.))
                            .justify_end()
                            .child(
                                item("keep-it", None, "Keep".into()).on_click(cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.overlay.close_session_menu();
                                    this.restore_session_anchor(window, cx);
                                    cx.notify();
                                })),
                            )
                            .child(
                                item("delete-it", None, "Delete".into())
                                    .text_color(palette.destructive)
                                    .on_click({
                                        let id = id.clone();
                                        cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.delete_session(id.clone(), window, cx);
                                        })
                                    }),
                            ),
                    );
            }
        }
        // From the top bar it hangs under the title, left edges aligned; from a row, under its "…".
        div()
            .absolute()
            .when(in_title, |menu| menu.left_0())
            .when(!in_title, |menu| menu.right_0())
            .top(px(if in_title { 30. } else { ROW }))
            .child(
                deferred(
                    anchored()
                        .anchor(if in_title { Anchor::TopLeft } else { Anchor::TopRight })
                        .offset(point(px(if in_title { 0. } else { -1. }), px(4.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(phase.menu("session-menu", -4., body)),
                )
                .with_priority(1),
            )
    }

    /// Change a session through this build's runner; read-only while the Nexus app holds the sessions. Archiving
    /// goes through `archive.rs`, so every caller (the menus, the palette, a key) offers Undo.
    pub(super) fn change_session(&mut self, args: UpdateArgs, cx: &mut Context<Self>) {
        self.overlay.close_session_menu();
        if args.archived == Some(true) {
            self.archive_with_undo(vec![args.thread_id], cx);
            return;
        }
        match self.runs(&args.thread_id) {
            Some(runs) => runs.update_session(args),
            None => self.refuse_read_only(),
        }
        cx.notify();
    }

    fn delete_session(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.close_session_menu();
        // A cloud session goes with its machine, so no machine stays behind asleep where nobody sees it.
        if let Some(machine) = self.cloud_machine_for(&id) {
            self.end_cloud(&id, &machine, End::Delete, false, window, cx);
            return;
        }
        self.delete_session_on_runner(id, false, window, cx);
    }

    pub(super) fn delete_session_on_runner(
        &mut self,
        id: String,
        discard_uncommitted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(runs) = self.runs(&id) else {
            self.refuse_read_only();
            cx.notify();
            return;
        };
        // The row starts to leave now, not when the call answers (`row_exit.rs`).
        let title = self
            .session_by_id(&id)
            .map(|session| session.title.clone())
            .unwrap_or_default();
        self.row_exits.start(&id, std::time::Instant::now());
        let answer = runs.call(
            "wb_thread_delete",
            serde_json::json!({
                "threadId": id,
                "discardUncommitted": discard_uncommitted,
                "offerDiscardUncommitted": !discard_uncommitted,
            }),
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer
                .await
                .unwrap_or_else(|_| Err("The command's answer was lost.".to_string()));
            let shown = this.update_in(cx, |this, window, cx| {
                match answer {
                    Ok(value) => {
                        if matches!(&this.view, View::Code(open) if open.id == id) {
                            this.open_new_code(window, cx);
                        }
                        let text = value
                            .as_str()
                            .map_or_else(|| row_exit::deleted_text(&title), str::to_owned);
                        this.show_notice(text, None, row_exit::DELETED_STAY, cx);
                    }
                    Err(error) => {
                        this.row_exits
                            .bring_back(&id, std::time::Instant::now(), cx.reduce_motion());
                        if !discard_uncommitted
                            && error == nexus_workbench::api::WORKTREE_DELETE_CONFIRMATION_REQUIRED
                            && this.session_by_id(&id).is_some()
                        {
                            this.show_delete_warning(id, title, cx);
                        } else {
                            this.show_notice(
                                row_exit::delete_failed_text(&title, &error),
                                None,
                                nexus_desktop_core::notice::NOTICE,
                                cx,
                            );
                        }
                    }
                }
                this.refresh_code(cx);
                cx.notify();
            });
            if let Err(error) = shown {
                eprintln!("nexus-desktop-gpui: the delete's answer had no window: {error}");
            }
        })
        .detach();
        cx.notify();
    }

    /// Edit the session's title in its row: Enter keeps it, leaving the field (or an empty title) drops it.
    pub(super) fn start_rename(&mut self, id: &str, in_title: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_anchor_handle(id, in_title).is_none() {
            self.remember_session_anchor(id, in_title, cx);
        }
        self.overlay.close_session_menu();
        if self.runner.is_none() {
            self.refuse_read_only();
            cx.notify();
            return;
        }
        let title = self
            .session_by_id(id)
            .map(|session| session.title.clone())
            .unwrap_or_default();
        self.begin_rename(id.to_string(), title, false, window, cx);
        if let Some(renaming) = &mut self.renaming {
            renaming.in_title = in_title;
        }
    }

    /// Edit a row's title in place: Enter keeps it, a click outside (or an empty title) drops it.
    pub(super) fn begin_rename(
        &mut self,
        id: String,
        title: String,
        chat: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlay.close_session_menu();
        self.overlay.close_row_menu();
        if chat {
            self.session_anchor = None;
        }
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(title, window, cx);
            state
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let title = state.read(cx).value().trim().to_string();
                    let renaming = this.renaming.take();
                    this.restore_session_anchor(window, cx);
                    if let Some(renaming) = renaming
                        && !title.is_empty()
                    {
                        if renaming.chat {
                            this.rename_chat(renaming.id, title, cx);
                            return;
                        }
                        this.change_session(
                            UpdateArgs {
                                thread_id: renaming.id,
                                title: Some(title),
                                ..UpdateArgs::default()
                            },
                            cx,
                        );
                    }
                    cx.notify();
                }
                InputEvent::Blur => {
                    if this.renaming.take().is_some() {
                        this.session_anchor = None;
                    }
                    cx.notify();
                }
                _ => {}
            },
        );
        input.update(cx, |state, cx| state.focus(window, cx));
        self.renaming = Some(Renaming {
            id,
            chat,
            in_title: false,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }
}

/// The session's pull request before its age (`pull-badge.tsx` in the Nexus app): an icon and one grey word with no
/// fill, from the watch's last read (nexus-pull-requests' `badge`). Only the icon keeps a dim tint of the state's hue.
/// Hover hides it for "…", so the row's card ([`pull_card`]) says it in full then. A session with several adds "+N" for
/// the others, fainter still. None before the watch read one.
fn pull_badge(thread_id: &str, palette: Palette) -> Option<AnyElement> {
    use nexus_pull_requests::badge::Badge;
    let watch = crate::pr_watch::known(thread_id)?;
    let pull = watch.status.clone()?;
    let others = nexus_pull_requests::chip::session_pulls(&watch)
        .iter()
        .filter(|other| other.url != pull.url)
        .count();
    let badge = Badge::of(&pull);
    Some(
        h_flex()
            .flex_none()
            .gap(px(3.))
            .items_center()
            .font_family("monospace")
            .text_size(px(11.))
            .text_color(palette.muted)
            .child(glyph(pull_icon(badge), pull_color(badge, palette).opacity(0.75)).size(px(11.)))
            .child(badge.label())
            .when(others > 0, |row| {
                row.child(div().text_color(palette.faint).child(format!("+{others}")))
            })
            .into_any_element(),
    )
}

/// Each state's icon, as the Nexus app's `pull-badge.tsx` has them; the top bar's chip uses them too.
pub(super) fn pull_icon(badge: nexus_pull_requests::badge::Badge) -> Lucide {
    use nexus_pull_requests::badge::Badge;
    match badge {
        Badge::Draft => Lucide::GitPullRequestDraft,
        Badge::Open | Badge::Changes => Lucide::GitPullRequest,
        Badge::Approved => Lucide::Check,
        Badge::Merged => Lucide::GitMerge,
        Badge::Closed => Lucide::GitPullRequestClosed,
    }
}

/// Each state's hue, as on GitHub and in the Nexus app's `sidebar.css` (`.wb-thread-pr`): draft grey, open blue,
/// approved green, changes amber, merged purple, closed red. The word still says it.
pub(crate) fn pull_color(badge: nexus_pull_requests::badge::Badge, palette: Palette) -> Hsla {
    use nexus_pull_requests::badge::Badge;
    let (light, dark) = match badge {
        Badge::Draft => return palette.muted,
        Badge::Open => (0x0969DA, 0x4493F8),
        Badge::Approved => (0x1A7F37, 0x3FB950),
        Badge::Changes => (0x9A6700, 0xD29922),
        Badge::Merged => (0x8250DF, 0xA371F7),
        Badge::Closed => (0xCF222E, 0xF85149),
    };
    rgb(if palette.dark { dark } else { light }).into()
}
