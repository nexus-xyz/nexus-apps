//! The sidebar's one grouping: Projects, then Recent. A project is a Code project (a folder or a
//! repository) or a plain one, a name with no folder. A chat sits in one project or in Recent, never in two places
//! (`project_groups.rs` keeps where). This file draws the plain projects, the Projects head's "+" menu, the name field
//! for a new project and the chat menu's "Move to" list.

use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::code_dock::menu_box;
use super::sidebar::{ROW, ROW_TEXT, hint, note, small_button};
use super::{Workspace, glyph};
use crate::code;
use crate::i18n::tr;
use crate::model::SessionSummary;
use crate::palette::Palette;
use crate::project_groups::{self, Groups, Home};
use crate::workspace::MenuTrigger as _;
use crate::workspace::menu_item_hover;

/// The groups of the organization on screen, and the sidebar's state around them.
#[derive(Default)]
pub(super) struct GroupsState {
    /// The organization these groups are for; None until an account arrives.
    org: Option<String>,
    pub groups: Groups,
    /// Where the next new chat goes: set by a plain project's "+".
    pub pending: Option<Home>,
    /// The name field for a new plain project, while it is open.
    pub adding: Option<(Entity<InputState>, Subscription)>,
    /// The name field is in the new session's project menu: Enter makes the project and starts a chat in it.
    pub adding_in_menu: bool,
    /// Plain projects the person folded.
    pub folded: std::collections::HashSet<String>,
    /// The organization's groups didn't read, so they show empty. No save runs until a read succeeds: a save would
    /// write the empty groups over the organization's projects once the file reads again.
    unread: bool,
}

impl GroupsState {
    /// Takes the organization's groups as read (`project_groups::load`). Answers the notice for a failed read.
    fn take_load(&mut self, org: &str, read: Result<Groups, String>) -> Option<String> {
        self.org = Some(org.to_string());
        self.pending = None;
        match read {
            Ok(groups) => {
                self.groups = groups;
                self.unread = false;
                None
            }
            Err(error) => {
                self.groups = Groups::default();
                self.unread = true;
                Some(tr("Couldn't read your projects: {error}").replace("{error}", &error.to_string()))
            }
        }
    }

    /// Saves the groups with `save`. After a failed read it reads again with `load` first and writes nothing: a read
    /// that works now takes the file's groups, and the change made over the empty ones is dropped. Answers a notice.
    fn save_with(
        &mut self,
        load: impl FnOnce(&str) -> Result<Groups, String>,
        save: impl FnOnce(&str, &Groups) -> Result<(), String>,
    ) -> Option<String> {
        let org = self.org.clone()?;
        if self.unread {
            return match load(&org) {
                Ok(groups) => {
                    self.groups = groups;
                    self.unread = false;
                    Some(tr("Your projects loaded just now, so that change wasn't saved. Make it again.").to_string())
                }
                Err(error) => {
                    Some(tr("Couldn't save your projects, as they didn't read: {error}").replace("{error}", &error))
                }
            };
        }
        save(&org, &self.groups)
            .err()
            .map(|error| tr("Couldn't save your projects: {error}").replace("{error}", &error))
    }
}

impl Workspace {
    /// Reads the organization's groups when the organization on screen changed.
    pub(super) fn load_groups(&mut self, org: &str) {
        if self.groups.org.as_deref() == Some(org) {
            return;
        }
        if let Some(notice) = self.groups.take_load(org, project_groups::load(org)) {
            self.notice = Some(notice.into());
        }
    }

    pub(super) fn save_groups(&mut self) {
        if let Some(notice) = self.groups.save_with(project_groups::load, project_groups::save) {
            self.notice = Some(notice.into());
        }
    }

    /// The chat's project, when the project is still there. A chat in an archived or removed Code project is in Recent.
    pub(super) fn chat_project(&self, chat: &str) -> Option<Home> {
        match self.groups.groups.home_of(chat)? {
            Home::Code(id) if !self.projects.iter().any(|project| project.id == id) => None,
            home => Some(home),
        }
    }

    /// The chats filed in `home`, newest first.
    pub(super) fn chats_in(&self, home: &Home) -> Vec<&SessionSummary> {
        self.sessions
            .iter()
            .filter(|session| self.chat_project(&session.id).as_ref() == Some(home))
            .collect()
    }

    /// Files a chat, or sends it back to Recent with None, and closes the row's menu.
    pub(super) fn file_chat(&mut self, chat: String, home: Option<Home>, cx: &mut Context<Self>) {
        self.groups.groups.file(&chat, home);
        self.overlay.close_row_menu();
        self.save_groups();
        cx.notify();
    }

    /// A chat Gateway just made: it goes where the "+" that started it pointed.
    pub(super) fn file_new_chat(&mut self, chat: &str) {
        if let Some(home) = self.groups.pending.take() {
            self.groups.groups.file(chat, Some(home));
            self.save_groups();
        }
    }

    /// A new chat that lands in a plain project.
    pub(super) fn new_chat_in(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.new_chat(window, cx);
        self.groups.pending = Some(Home::Plain(id));
    }

    /// Opens the name field for a new plain project: in the sidebar, or in the new session's project menu (`in_menu`),
    /// where Enter also starts a chat in it.
    pub(super) fn begin_new_project(&mut self, in_menu: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.account_gateway().is_none() {
            return;
        }
        self.overlay.close_add_group_menu();
        self.groups.adding_in_menu = in_menu;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("Project name")));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let name = state.read(cx).value().to_string();
                    this.groups.adding = None;
                    if let Some(id) = this.groups.groups.add(&name, code::now_ms()) {
                        this.save_groups();
                        if this.groups.adding_in_menu {
                            this.overlay.close_launch_menu();
                            this.new_chat_in(id, window, cx);
                        }
                    }
                    cx.notify();
                }
                InputEvent::Blur => {
                    this.groups.adding = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        input.update(cx, |state, cx| state.focus(window, cx));
        self.groups.adding = Some((input, subscription));
        cx.notify();
    }

    /// The project menu's "New project “name”": a plain project by that name, and a chat in it. A name already taken
    /// picks that project.
    pub(super) fn add_project_named(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.groups.groups.add(name, code::now_ms()) {
            self.save_groups();
            self.overlay.close_launch_menu();
            self.new_chat_in(id, window, cx);
        }
        cx.notify();
    }

    fn remove_plain_project(&mut self, id: String, cx: &mut Context<Self>) {
        self.groups.groups.remove(&id);
        self.save_groups();
        cx.notify();
    }

    /// The Projects head's "+": New project and Open a folder. A repository needs no adding: a new session picks it
    /// (the project menu's From GitHub), and its project shows with the session.
    pub(super) fn render_add_project(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.overlay.add_group_menu();
        let button = small_button("projects-add", Lucide::Plus, palette)
            .size(px(28.))
            .when(open, |button| button.bg(palette.selected))
            // No tooltip over its own open menu.
            .when(!open, |button| {
                button.tooltip(|window, cx| Tooltip::new(tr("Add a project")).build(window, cx))
            })
            .menu_trigger(
                open,
                cx,
                |this| this.overlay.add_group_menu(),
                |this, _, cx| {
                    this.toggle_overlay(Overlay::AddGroupMenu);
                    cx.notify();
                },
            );
        let item = |id: &'static str, icon: Lucide, label: SharedString, detail: SharedString| {
            h_flex()
                .id(id)
                .min_h(px(44.))
                .px(px(8.))
                .gap(px(10.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| menu_item_hover(style, palette))
                .child(glyph(icon, palette.icon).size(px(16.)))
                .child(
                    v_flex()
                        .min_w(px(0.))
                        .child(div().text_color(palette.foreground).child(label))
                        .child(div().text_xs().text_color(palette.faint).child(detail)),
                )
        };
        div().relative().child(button).when(open, |anchor| {
            anchor.child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopRight)
                        .offset(point(px(0.), px(30.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            menu_box("projects-add-menu", 260., palette)
                                .text_size(px(ROW_TEXT))
                                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                    this.overlay.close_add_group_menu();
                                    cx.notify();
                                }))
                                .children(self.account_gateway().is_some().then(|| {
                                    item(
                                        "project-new",
                                        Lucide::FolderPlus,
                                        tr("New project"),
                                        tr("A name for work with no code"),
                                    )
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.begin_new_project(false, window, cx);
                                        },
                                    ))
                                }))
                                .child(
                                    item(
                                        "project-folder",
                                        Lucide::FolderOpen,
                                        tr("Open a folder"),
                                        tr("A folder on this computer"),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.overlay.close_add_group_menu();
                                        this.add_project(cx);
                                    })),
                                ),
                        ),
                )
                .with_priority(1),
            )
        })
    }

    /// The name field for a new project, in the list where the project will appear.
    pub(super) fn render_new_project_field(&self, in_menu: bool, palette: Palette) -> Option<AnyElement> {
        let (input, _) = self
            .groups
            .adding
            .as_ref()
            .filter(|_| self.groups.adding_in_menu == in_menu)?;
        Some(
            h_flex()
                .h(px(ROW))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .bg(palette.selected)
                .text_size(px(ROW_TEXT))
                .child(glyph(Lucide::Folder, palette.icon))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(gpui_kit::component::input::Input::new(input).appearance(false)),
                )
                .into_any_element(),
        )
    }

    /// A plain project: its head (fold, "+" for a new chat in it, "…" to remove it) and its chats. `slot`, given
    /// outside a search, lets the person drag it to another place in the list (`project_drag.rs`).
    pub(super) fn render_plain_project(
        &self,
        project: &project_groups::PlainProject,
        slot: Option<super::project_drag::ProjectSlot>,
        now: i64,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let home = Home::Plain(project.id.clone());
        let chats = self.chats_in(&home);
        let folded = self.groups.folded.contains(&project.id);
        let hover = SharedString::from(format!("plain-hover-{}", project.id));
        let actions = h_flex()
            .absolute()
            .right_0()
            .top_0()
            .h_full()
            .items_center()
            .gap(px(2.))
            .opacity(0.)
            .group_hover(hover.clone(), |style| style.opacity(1.))
            .child({
                let id = project.id.clone();
                let tip: SharedString = tr("New session in {project}")
                    .replace("{project}", &project.name)
                    .into();
                small_button(
                    SharedString::from(format!("plain-new-{}", project.id)),
                    Lucide::Plus,
                    palette,
                )
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.new_chat_in(id.clone(), window, cx);
                }))
            })
            .child({
                let id = project.id.clone();
                small_button(
                    SharedString::from(format!("plain-remove-{}", project.id)),
                    Lucide::X,
                    palette,
                )
                .tooltip(|window, cx| Tooltip::new(tr("Remove project (its chats go to Recent)")).build(window, cx))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.remove_plain_project(id.clone(), cx);
                }))
            });
        let id = project.id.clone();
        let head = h_flex()
            .id(SharedString::from(format!("plain-{}", project.id)))
            .group(hover.clone())
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
                if !this.groups.folded.remove(&id) {
                    this.groups.folded.insert(id.clone());
                }
                cx.notify();
            }))
            .child(glyph(
                if folded { Lucide::Folder } else { Lucide::FolderOpen },
                palette.icon,
            ))
            .child(div().flex_1().min_w(px(0.)).truncate().child(project.name.clone()))
            .child(
                div()
                    .min_w(px(16.))
                    .flex()
                    .justify_end()
                    .group_hover(hover.clone(), |style| style.opacity(0.))
                    .when(!chats.is_empty(), |count| {
                        count.child(hint(chats.len().to_string(), palette))
                    }),
            )
            .child(actions);
        let rows: Vec<AnyElement> = if folded {
            Vec::new()
        } else {
            chats
                .iter()
                .map(|chat| self.render_filed_chat(chat, now, palette, cx))
                .collect()
        };
        let head = match &slot {
            Some(slot) => self.project_drag_source(head.into_any_element(), slot, &project.name, palette, cx),
            None => head.into_any_element(),
        };
        let block = v_flex()
            .child(head)
            .children(rows)
            .when(!folded && chats.is_empty(), |column| {
                column.child(div().pl(px(40.)).child(note(tr("No sessions yet"), palette.faint)))
            })
            .into_any_element();
        match slot {
            Some(slot) => self.project_drop_target(block, slot, palette, cx),
            None => block,
        }
    }

    /// A chat inside a project: the chat's row, set in under the project's name as a Code session's row is.
    pub(super) fn render_filed_chat(
        &self,
        chat: &SessionSummary,
        now: i64,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .pl(px(16.))
            .child(self.render_chat_row(chat, false, now, palette, cx))
            .into_any_element()
    }

    /// The chat menu's "Move to" list: Recent, the plain projects, then the Code projects. The chat's own place is
    /// checked and does nothing.
    pub(super) fn render_move_list(&self, chat: &str, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let here = self.chat_project(chat);
        let places = move_places(&self.groups.groups.projects, &self.projects);
        let rows = places.into_iter().enumerate().map(|(index, place)| {
            let current = place.holds(&here);
            let (home, name, icon) = (place.home, place.name, place.icon);
            let chat = chat.to_string();
            h_flex()
                .id(SharedString::from(format!("move-{index}")))
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| menu_item_hover(style, palette))
                .child(glyph(icon, palette.icon).size(px(16.)))
                .child(div().flex_1().min_w(px(0.)).truncate().child(name))
                .when(current, |row| {
                    row.child(glyph(Lucide::Check, palette.foreground).size(px(14.)))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if current {
                        this.overlay.close_row_menu();
                        cx.notify();
                    } else {
                        this.file_chat(chat.clone(), home.clone(), cx);
                    }
                }))
        });
        v_flex()
            .child(
                div()
                    .px(px(8.))
                    .pt(px(4.))
                    .pb(px(2.))
                    .text_xs()
                    .text_color(palette.faint)
                    .child(tr("Move to")),
            )
            .children(rows)
            .into_any_element()
    }
}

/// A row of the chat menu's "Move to" list.
pub(super) struct MovePlace {
    /// Where a pick files the chat; None is Sessions.
    pub home: Option<Home>,
    pub name: SharedString,
    pub icon: Lucide,
    /// Every place that counts as this row: a repository's row holds each of its copies.
    members: Vec<Option<Home>>,
}

impl MovePlace {
    fn holds(&self, here: &Option<Home>) -> bool {
        self.members.contains(here)
    }
}

/// Sessions, the plain projects, then one row per repository (`repo_groups.rs`): a cloud session's copy of a repository
/// is not a place of its own. A pick files the chat under the group's lead, and the sidebar gathers filed chats from
/// every member, so it shows in the right group either way.
pub(super) fn move_places(plain: &[project_groups::PlainProject], projects: &[code::CodeProject]) -> Vec<MovePlace> {
    let mut places = vec![MovePlace {
        home: None,
        name: tr("Sessions"),
        icon: Lucide::Clock,
        members: vec![None],
    }];
    places.extend(plain.iter().map(|project| {
        let home = Some(Home::Plain(project.id.clone()));
        MovePlace {
            home: home.clone(),
            name: project.name.clone().into(),
            icon: Lucide::Folder,
            members: vec![home],
        }
    }));
    places.extend(
        super::repo_groups::group_projects(projects)
            .into_iter()
            .map(|repo| MovePlace {
                home: Some(Home::Code(repo.lead.id.clone())),
                name: repo.lead.name.clone().into(),
                icon: Lucide::FolderGit2,
                members: repo.ids().into_iter().map(|id| Some(Home::Code(id))).collect(),
            }),
    );
    places
}

#[cfg(test)]
mod tests {
    use super::{GroupsState, Home, move_places};
    use crate::project_groups::{Groups, load_at, save_at};

    #[test]
    fn a_read_that_failed_never_lets_a_save_drop_the_projects() {
        let dir = std::env::temp_dir().join(format!("nexus-groups-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("project-groups.json");
        let mut saved = Groups::default();
        saved.add("Launch", 1);
        save_at(&file, "org", &saved).unwrap();
        let good = std::fs::read(&file).unwrap();
        // The read fails once (a lock from a backup tool, say): the groups show empty.
        let mut state = GroupsState::default();
        assert!(state.take_load("org", Err("locked".into())).is_some());
        assert_eq!(state.groups, Groups::default());
        // A change over the empty groups, once the file reads again: nothing is written, the file's groups come back.
        state.groups.add("Other", 2);
        let notice = state.save_with(|org| load_at(&file, org), |org, groups| save_at(&file, org, groups));
        assert!(notice.is_some_and(|notice| notice.contains("didn't save") || notice.contains("wasn't saved")));
        assert_eq!(std::fs::read(&file).unwrap(), good);
        assert_eq!(state.groups, saved);
        // From then on saves run, and keep the projects.
        state.groups.add("Other", 2);
        assert_eq!(
            state.save_with(|org| load_at(&file, org), |org, groups| save_at(&file, org, groups)),
            None
        );
        let after = load_at(&file, "org").unwrap();
        assert_eq!(after.projects.len(), 2);
        assert_eq!(after.projects[0].name, "Launch");
        let _ = std::fs::remove_dir_all(&dir);
    }
    use crate::code::CodeProject;
    use crate::project_groups::PlainProject;

    fn project(id: &str, remote: &str, machine: Option<&str>) -> CodeProject {
        CodeProject {
            id: id.into(),
            name: remote.rsplit('/').next().unwrap_or(remote).into(),
            branch: Some("main".into()),
            pinned: false,
            remotes: vec![remote.into()],
            sessions: Vec::new(),
            machine: machine.map(String::from),
            added_at: 0,
        }
    }

    #[test]
    fn move_to_lists_a_repository_once_however_many_cloud_copies_it_has() {
        let plain = [PlainProject {
            id: "notes".into(),
            name: "Notes".into(),
            created_ms: 0,
        }];
        let nexus = "https://github.com/acme/nexus";
        let projects = [
            project("cloud-1", nexus, Some("m1")),
            project("local", nexus, None),
            project("cloud-2", nexus, Some("m2")),
            project("other", "https://github.com/acme/other", None),
        ];
        let places = move_places(&plain, &projects);
        let names: Vec<&str> = places.iter().map(|place| place.name.as_ref()).collect();
        assert_eq!(names, ["Sessions", "Notes", "nexus", "other"]);
        // A pick files under the lead (the local copy); a chat filed under any copy shows as already there.
        assert_eq!(places[2].home, Some(Home::Code("local".into())));
        assert!(places[2].holds(&Some(Home::Code("cloud-2".into()))));
        assert!(!places[3].holds(&Some(Home::Code("cloud-2".into()))));
        assert!(places[0].holds(&None));
    }
}
