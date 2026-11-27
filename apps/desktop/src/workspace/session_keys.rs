//! Code's session keys, as the Nexus app's `ShortcutsHost` (`desktop/src/workbench/palette.tsx`): Mod+1 to Mod+9 jump
//! to a session in the sidebar's order, Mod+Shift+[ and ] step through them, Mod+. stops the turn, Mod+/ opens the
//! model picker, Mod+Shift+M, E and Mod+Alt+Shift+A step the model, effort and permission mode, Mod+Shift+S archives,
//! Mod+Shift+H snoozes, Mod+Shift+C copies the branch name, F2 renames, Mod+Backspace deletes (it asks first),
//! Mod+Shift+I and A open the Inbox and Artifacts, Mod+O adds a project, Mod+L focuses the message box and Mod+Shift+D
//! shows or hides Changes. The keys are `key_bindings.rs`'s, which a person may change.
//!
//! As in the Nexus app, they are Code's: they act while a Code screen (a session, or New session) is on screen, and
//! elsewhere the key goes on to whatever else has it. The ones that need a session do nothing on New session.

use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::*;
use nexus_workbench::api::UpdateArgs;
use nexus_workbench::cycle;
use nexus_workbench::event::Effort;

use super::code_dock::Menu;
use super::new_code::LaunchMenu;
use super::session_menu::{MenuPage, SessionMenu};
use super::side_pane::Tool;
use super::{View, Workspace};
use crate::cloud::words::End;
use crate::code::{CodeProject, CodeSession};

actions!(
    workspace,
    [
        AddProject,
        JumpSession1,
        JumpSession2,
        JumpSession3,
        JumpSession4,
        JumpSession5,
        JumpSession6,
        JumpSession7,
        JumpSession8,
        JumpSession9,
        PreviousSession,
        NextSession,
        FocusComposer,
        ToggleChanges,
        PickModel,
        OpenInbox,
        OpenArtifacts,
        StopTurn,
        CycleModel,
        CycleEffort,
        CycleMode,
        ArchiveSession,
        SnoozeSession,
        CopyBranch,
        RenameSession,
        DeleteSession
    ]
);

/// Mod+1 to Mod+9's actions on their keys, in order (`key_bindings.rs` binds them).
pub const JUMP_BINDS: [fn(&'static str) -> KeyBinding; 9] = [
    |keys| KeyBinding::new(keys, JumpSession1, Some("Workspace")),
    |keys| KeyBinding::new(keys, JumpSession2, Some("Workspace")),
    |keys| KeyBinding::new(keys, JumpSession3, Some("Workspace")),
    |keys| KeyBinding::new(keys, JumpSession4, Some("Workspace")),
    |keys| KeyBinding::new(keys, JumpSession5, Some("Workspace")),
    |keys| KeyBinding::new(keys, JumpSession6, Some("Workspace")),
    |keys| KeyBinding::new(keys, JumpSession7, Some("Workspace")),
    |keys| KeyBinding::new(keys, JumpSession8, Some("Workspace")),
    |keys| KeyBinding::new(keys, JumpSession9, Some("Workspace")),
];

/// The Code sessions in the order the sidebar draws them (`sidebarOrder`): this computer's projects, then each
/// server's, then each cloud machine's, each project's sessions as listed. A folded project still counts, so Mod+3
/// is the same session whether or not its folder is open. `listed` is the projects as the sidebar keeps them.
pub(super) fn sidebar_order(listed: &[(&CodeProject, Vec<&CodeSession>)], machines: &[&str]) -> Vec<String> {
    let place = |project: &CodeProject| {
        project
            .machine
            .as_deref()
            .and_then(|machine| machines.iter().position(|id| *id == machine))
            .map_or(0, |at| at + 1)
    };
    let mut ordered: Vec<&(&CodeProject, Vec<&CodeSession>)> = listed.iter().collect();
    // Stable: projects keep their order within a machine.
    ordered.sort_by_key(|(project, _)| place(project));
    ordered
        .into_iter()
        .flat_map(|(_, sessions)| {
            super::session_tree::tree(sessions)
                .into_iter()
                .map(|(session, _)| session.id.clone())
        })
        .collect()
}

/// The effort levels to step through: the default (None), then the ones the model takes.
fn with_default(efforts: Vec<(Effort, &'static str)>) -> Vec<Option<Effort>> {
    std::iter::once(None)
        .chain(efforts.into_iter().map(|(effort, _)| Some(effort)))
        .collect()
}

impl Workspace {
    /// A Code screen is on screen: Code's keys are its, as when the Nexus app's Code page claims the keyboard.
    pub(super) fn on_code_screen(&self) -> bool {
        self.workspace_open() && matches!(self.view, View::Code(_) | View::NewCode)
    }

    /// The open session's id, on its Code screen.
    fn open_session(&self) -> Option<String> {
        match &self.view {
            View::Code(view) => Some(view.id.clone()),
            _ => None,
        }
    }

    /// Before a key acts: Code's keys belong to a Code screen (elsewhere the key goes on), and the palette closes
    /// first, so it never sits over what the key opened. False when the key is not this screen's.
    fn take_code_key(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.on_code_screen() {
            cx.propagate();
            return false;
        }
        if self.overlay.palette().is_some() {
            self.close_palette(window, cx);
        }
        true
    }

    /// The sessions in the sidebar's order, as it lists them now: the filter's matches while it filters, else each
    /// project's sessions but the snoozed and settled ones (the one on screen stays).
    pub(super) fn jump_order(&self) -> Vec<String> {
        let now = crate::code::now_ms();
        let open = self.open_session();
        let listed: Vec<(&CodeProject, Vec<&CodeSession>)> = if self.filtering() {
            self.filter_kept().projects
        } else {
            self.projects
                .iter()
                .map(|project| {
                    let sessions = project
                        .sessions
                        .iter()
                        .filter(|session| {
                            let shown = open.as_deref() == Some(session.id.as_str());
                            session.shown_aside(now, shown).is_none()
                        })
                        .collect();
                    (project, sessions)
                })
                .collect()
        };
        let machines: Vec<&str> = self
            .servers
            .iter()
            .map(|server| server.id.as_str())
            .chain(self.cloud_machines.iter().map(|machine| machine.id.as_str()))
            .collect();
        sidebar_order(&listed, &machines)
    }

    fn jump(&mut self, place: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.take_code_key(window, cx) {
            return;
        }
        if let Some(id) = self.jump_order().get(place) {
            let id = id.clone();
            self.open_code(&id, window, cx);
        }
    }

    fn step_session(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.take_code_key(window, cx) {
            return;
        }
        let order = self.jump_order();
        let current = self.open_session();
        if let Some(id) = nexus_desktop_core::key_bindings::neighbour(&order, current.as_deref(), forward)
            && Some(id) != current.as_ref()
        {
            let id = id.clone();
            self.open_code(&id, window, cx);
        }
    }

    /// The open session, for the keys that need one.
    fn key_session(&self) -> Option<CodeSession> {
        let id = self.open_session()?;
        self.session_by_id(&id).cloned()
    }

    fn on_add_project(&mut self, _: &AddProject, window: &mut Window, cx: &mut Context<Self>) {
        if self.take_code_key(window, cx) {
            self.add_project(cx);
        }
    }

    fn on_previous_session(&mut self, _: &PreviousSession, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(false, window, cx);
    }

    fn on_next_session(&mut self, _: &NextSession, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(true, window, cx);
    }

    fn on_focus_composer(&mut self, _: &FocusComposer, window: &mut Window, cx: &mut Context<Self>) {
        if self.take_code_key(window, cx) {
            self.focus_composer(window, cx);
        }
    }

    /// Mod+Shift+D: Changes, and back. In a session whose folder has something to diff.
    fn on_toggle_changes(&mut self, _: &ToggleChanges, window: &mut Window, cx: &mut Context<Self>) {
        if !self.take_code_key(window, cx) || self.open_session().is_none() {
            return;
        }
        if self.side.open && self.side_tool() == Tool::Changes {
            self.toggle_side_pane(window, cx);
        } else {
            self.pick_tool(Tool::Changes, window, cx);
        }
    }

    /// Mod+/: the model menu, as `/model` opens it: the session's, or the new session's.
    fn on_pick_model(&mut self, _: &PickModel, window: &mut Window, cx: &mut Context<Self>) {
        if !self.take_code_key(window, cx) {
            return;
        }
        if self.open_session().is_some() {
            self.open_dock_menu(Menu::Model, window, cx);
        } else if self.overlay.launch_menu() != Some(LaunchMenu::Model) {
            self.toggle_launch_menu(LaunchMenu::Model, window, cx);
        }
    }

    fn on_open_inbox(&mut self, _: &OpenInbox, window: &mut Window, cx: &mut Context<Self>) {
        if self.take_code_key(window, cx) {
            self.open_inbox(window, cx);
        }
    }

    fn on_open_artifacts(&mut self, _: &OpenArtifacts, window: &mut Window, cx: &mut Context<Self>) {
        if self.take_code_key(window, cx) {
            self.open_library(window, cx);
        }
    }

    /// Mod+.: the agent's turn stops, when it runs.
    fn on_stop_turn(&mut self, _: &StopTurn, window: &mut Window, cx: &mut Context<Self>) {
        if self.take_code_key(window, cx) && self.open_session().is_some() && self.code_running() {
            self.stop(window, cx);
        }
    }

    /// Mod+Shift+M: the next of the default and the newest model of each family, for the session or the new one.
    fn on_cycle_model(&mut self, _: &CycleModel, window: &mut Window, cx: &mut Context<Self>) {
        if !self.take_code_key(window, cx) {
            return;
        }
        if let Some(session) = self.key_session() {
            let ids: Vec<String> = self.session_models(&session).into_iter().map(|(id, _)| id).collect();
            let next = cycle::next_model(&cycle::latest_models(&ids), session.model.as_deref());
            self.update_session(Some(next), None, None, cx);
        } else {
            let ids: Vec<String> = self
                .agent_models(self.code_form.agent)
                .into_iter()
                .map(|model| model.id)
                .collect();
            let next = cycle::next_model(&cycle::latest_models(&ids), self.code_form.model.as_deref());
            // An effort stays only with the model it was picked for, as a pick in the menu.
            if self.code_form.model != next {
                self.code_form.effort = None;
            }
            self.code_form.model = next;
            self.code_form.remember();
            cx.notify();
        }
    }

    /// Mod+Shift+E: the next effort the model takes, the default among them.
    fn on_cycle_effort(&mut self, _: &CycleEffort, window: &mut Window, cx: &mut Context<Self>) {
        if !self.take_code_key(window, cx) {
            return;
        }
        if let Some(session) = self.key_session() {
            let next = cycle::next_effort(&with_default(self.session_efforts(&session)), session.effort);
            self.update_session(None, Some(next), None, cx);
        } else {
            self.code_form.effort = cycle::next_effort(&with_default(self.form_efforts()), self.code_form.effort);
            self.code_form.remember();
            cx.notify();
        }
    }

    /// Mod+Alt+Shift+A: the next permission mode; Full access only from its menu.
    fn on_cycle_mode(&mut self, _: &CycleMode, window: &mut Window, cx: &mut Context<Self>) {
        if !self.take_code_key(window, cx) {
            return;
        }
        if let Some(session) = self.key_session() {
            let modes = nexus_workbench::harness::harness(session.agent).offered_modes();
            if let Some(next) = cycle::next_mode(session.mode, modes) {
                self.update_session(None, None, Some(next), cx);
            }
        } else {
            let modes = nexus_workbench::harness::harness(self.code_form.agent).offered_modes();
            if let Some(next) = cycle::next_mode(self.code_form.mode, modes) {
                self.code_form.mode = next;
                self.code_form.remember();
                cx.notify();
            }
        }
    }

    /// Mod+Shift+S: archived, or back from the archive, as the row's menu does (a cloud session goes with its machine).
    fn on_archive_session(&mut self, _: &ArchiveSession, window: &mut Window, cx: &mut Context<Self>) {
        if !self.take_code_key(window, cx) {
            return;
        }
        let Some(session) = self.key_session() else {
            return;
        };
        if !session.archived
            && let Some(machine) = self.cloud_machine_for(&session.id)
        {
            self.end_cloud(&session.id, &machine, End::Archive, false, window, cx);
            return;
        }
        self.change_session(
            UpdateArgs {
                thread_id: session.id,
                archived: Some(!session.archived),
                ..UpdateArgs::default()
            },
            cx,
        );
    }

    /// Mod+Shift+H: a snoozed session wakes; another shows its snooze times, in the top bar's menu. An archived
    /// session is the archive's.
    fn on_snooze_session(&mut self, _: &SnoozeSession, window: &mut Window, cx: &mut Context<Self>) {
        if !self.take_code_key(window, cx) {
            return;
        }
        let Some(session) = self.key_session() else {
            return;
        };
        if session.archived {
            return;
        }
        if session.snoozed_until.is_some_and(|until| until > crate::code::now_ms()) {
            self.change_session(
                UpdateArgs {
                    thread_id: session.id,
                    snoozed_until: Some(0),
                    ..UpdateArgs::default()
                },
                cx,
            );
        } else {
            self.open_overlay(Overlay::SessionMenu(SessionMenu {
                id: session.id,
                page: MenuPage::Snooze,
                in_title: true,
            }));
            cx.notify();
        }
    }

    fn on_copy_branch(&mut self, _: &CopyBranch, window: &mut Window, cx: &mut Context<Self>) {
        if !self.take_code_key(window, cx) {
            return;
        }
        if let Some(branch) = self.key_session().and_then(|session| session.branch) {
            cx.write_to_clipboard(ClipboardItem::new_string(branch));
        }
    }

    /// F2: the session's title, in the top bar.
    fn on_rename_session(&mut self, _: &RenameSession, window: &mut Window, cx: &mut Context<Self>) {
        // Not over the palette: its own Rename is there.
        if self.overlay.palette().is_some() || !self.take_code_key(window, cx) {
            return;
        }
        if let Some(id) = self.open_session() {
            self.start_rename(&id, true, window, cx);
        }
    }

    /// Mod+Backspace: "Delete it and its messages?", in the top bar's menu.
    pub(super) fn on_delete_session(&mut self, _: &DeleteSession, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.palette().is_some() || !self.take_code_key(window, cx) {
            return;
        }
        if let Some(id) = self.open_session() {
            self.open_overlay(Overlay::SessionMenu(SessionMenu {
                id,
                page: MenuPage::Delete,
                in_title: true,
            }));
            cx.notify();
        }
    }

    /// The workspace's handlers for these keys.
    pub(super) fn session_key_actions(&self, root: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        root.on_action(cx.listener(|this, _: &JumpSession1, window, cx| this.jump(0, window, cx)))
            .on_action(cx.listener(|this, _: &JumpSession2, window, cx| this.jump(1, window, cx)))
            .on_action(cx.listener(|this, _: &JumpSession3, window, cx| this.jump(2, window, cx)))
            .on_action(cx.listener(|this, _: &JumpSession4, window, cx| this.jump(3, window, cx)))
            .on_action(cx.listener(|this, _: &JumpSession5, window, cx| this.jump(4, window, cx)))
            .on_action(cx.listener(|this, _: &JumpSession6, window, cx| this.jump(5, window, cx)))
            .on_action(cx.listener(|this, _: &JumpSession7, window, cx| this.jump(6, window, cx)))
            .on_action(cx.listener(|this, _: &JumpSession8, window, cx| this.jump(7, window, cx)))
            .on_action(cx.listener(|this, _: &JumpSession9, window, cx| this.jump(8, window, cx)))
            .on_action(cx.listener(Self::on_add_project))
            .on_action(cx.listener(Self::on_previous_session))
            .on_action(cx.listener(Self::on_next_session))
            .on_action(cx.listener(Self::on_focus_composer))
            .on_action(cx.listener(Self::on_toggle_changes))
            .on_action(cx.listener(Self::on_pick_model))
            .on_action(cx.listener(Self::on_open_inbox))
            .on_action(cx.listener(Self::on_open_artifacts))
            .on_action(cx.listener(Self::on_stop_turn))
            .on_action(cx.listener(Self::on_cycle_model))
            .on_action(cx.listener(Self::on_cycle_effort))
            .on_action(cx.listener(Self::on_cycle_mode))
            .on_action(cx.listener(Self::on_archive_session))
            .on_action(cx.listener(Self::on_snooze_session))
            .on_action(cx.listener(Self::on_copy_branch))
            .on_action(cx.listener(Self::on_rename_session))
            .on_action(cx.listener(Self::on_delete_session))
    }
}

#[cfg(test)]
pub(super) mod tests {
    use core::prelude::v1::test;

    use super::*;

    /// A Code session at rest in "project", titled by its id: also the session name tag tests' fixture.
    pub(in crate::workspace) fn session(id: &str) -> CodeSession {
        CodeSession {
            saved_provider: false,
            linked: None,
            name: None,
            created_at: 0,
            acted_at: 0,
            account: None,
            connection: None,
            interrupted: false,
            live: false,
            parent: None,
            background: false,
            background_busy: false,
            background_work: vec![],
            background_total: 0,
            waits_on: vec![],
            has_worktree: false,
            side_chat: false,
            compact: false,
            orchestration: false,
            at_depth_limit: false,
            agent_started: false,
            pending: None,
            source: None,
            crew: Default::default(),
            start_choices: Vec::new(),
            id: id.into(),
            title: id.into(),
            status: crate::code::Status::Idle,
            activity: nexus_workbench::attention::Activity {
                status: nexus_workbench::event::ThreadStatus::Idle,
                updated_at: 0,
                listed_at: None,
            },
            limited: None,
            updated_at: 0,
            agent: nexus_workbench::event::SessionAgent::Claude,
            model: None,
            mode: nexus_workbench::event::PermissionMode::Default,
            effort: None,
            fast: false,
            via: nexus_workbench::event::Via::Subscription,
            pinned: false,
            branch: None,
            snoozed_until: None,
            settled: false,
            archived: false,
            machine: None,
            project_id: "project".into(),
            cost_usd: 0.0,
            context: None,
            pull_request: None,
            review: false,
        }
    }

    fn project(id: &str, machine: Option<&str>, sessions: &[&str]) -> CodeProject {
        CodeProject {
            id: id.into(),
            name: id.into(),
            branch: None,
            pinned: false,
            remotes: Vec::new(),
            sessions: sessions.iter().map(|id| session(id)).collect(),
            machine: machine.map(Into::into),
            added_at: 0,
        }
    }

    #[test]
    fn the_order_is_the_sidebars_this_computer_then_servers_then_cloud() {
        let projects = [
            project("cloud", Some("c1"), &["c-a"]),
            project("local", None, &["l-a", "l-b"]),
            project("server", Some("s1"), &["s-a"]),
            project("other-local", None, &["o-a"]),
        ];
        let listed: Vec<(&CodeProject, Vec<&CodeSession>)> = projects
            .iter()
            .map(|project| (project, project.sessions.iter().collect()))
            .collect();
        assert_eq!(
            sidebar_order(&listed, &["s1", "c1"]),
            ["l-a", "l-b", "o-a", "s-a", "c-a"]
        );
        // With no servers or cloud machines, the projects keep their order.
        assert_eq!(sidebar_order(&listed[1..2], &[]), ["l-a", "l-b"]);
    }

    #[test]
    fn shortcuts_follow_the_drawn_tree_when_a_child_is_stored_before_its_parent() {
        let mut project = project("project", None, &["child", "other", "lead"]);
        project.sessions[0].parent = Some("lead".into());
        let listed = [(&project, project.sessions.iter().collect())];
        let order = sidebar_order(&listed, &[]);
        assert_eq!(order, ["other", "lead", "child"]);
        assert_eq!(
            nexus_desktop_core::key_bindings::neighbour(&order, Some("lead"), true),
            Some(&"child".into())
        );
        assert_eq!(
            nexus_desktop_core::key_bindings::neighbour(&order, Some("lead"), false),
            Some(&"other".into())
        );
    }

    #[test]
    fn effort_steps_start_at_the_default() {
        assert_eq!(
            with_default(vec![(Effort::Low, "Low"), (Effort::High, "High")]),
            [None, Some(Effort::Low), Some(Effort::High)]
        );
    }
}
