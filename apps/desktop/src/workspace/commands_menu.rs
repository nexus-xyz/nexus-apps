//! The Code top bar's project commands (the Nexus app's `project-scripts-control.tsx`): a ▶ button, shown when the
//! session's project has commands, whose menu runs one in a new terminal of the session's folder, named after it.
//! In a session on a server, one runs in a new shell there when the server's host opens shells
//! (`nexus_workbench::shell_pty`). With an older host the menu says why none can run. "Edit commands…" opens Settings ›
//! Worktrees.

use crate::workspace::MenuTrigger;
use crate::workspace::menu_item_hover;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::scripts::{self, ProjectScript};

use super::TopPanel;
use super::git_panel::popover;
use crate::code::CodeSession;
use crate::pages::PageId;
use crate::palette::Palette;
use crate::workspace::code_dock::{menu_box, menu_label};
use crate::workspace::settings_view::SettingsSection;
use crate::workspace::{Workspace, bar_button, glyph};

/// The trigger's tooltip while the session can run them.
const RUN_HERE: &str = "Run a project command in a new terminal";

impl Workspace {
    /// Why session `id` cannot run project commands, or None when it can: a server's or cloud machine's session runs
    /// them there, when its host opens shells.
    pub(in crate::workspace) fn commands_unavailable(id: &str) -> Option<&'static str> {
        let machines = crate::machines::get();
        let machine = machines.machine_of(id);
        let shells = machine
            .as_deref()
            .is_some_and(|machine| machines.has_feature(machine, nexus_workbench::shell_pty::FEATURE));
        scripts::unavailable_on(machine.is_some(), shells)
    }

    /// Run `script` in a new terminal of session `id`'s folder, named after it, and show it (the Nexus app's
    /// `runScript`). Nothing for a session on another machine.
    pub(in crate::workspace) fn run_project_command(
        &mut self,
        id: &str,
        script: &ProjectScript,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.overlay.code_top(), Some(TopPanel::Commands(_))) {
            self.overlay.close_code_top();
        }
        if Self::commands_unavailable(id).is_some() {
            cx.notify();
            return;
        }
        self.run_in_side_terminal(&script.command, Some(&script.name), window, cx);
    }

    /// A project command's own key (the Nexus app's `runScriptKey`), on a Code session's screen. It reaches here only
    /// when no binding and no element took the key, so Code's own keys win. A held key runs the command once, and its
    /// repeats stop here. Off a Mac, a Ctrl+Alt key stays with a text box, where it is AltGr.
    pub(in crate::workspace) fn on_command_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.code_session() else {
            return;
        };
        let (id, project) = (session.id.clone(), session.project_id.clone());
        let list = crate::project_commands::of(&project, cx);
        let mac = cfg!(target_os = "macos");
        let Some((script, alt)) = crate::project_commands::command_for(&list, &event.keystroke, mac) else {
            return;
        };
        let typing = window.context_stack().iter().any(|context| context.contains("Input"));
        if typing && !crate::project_commands::runs_while_typing(alt, mac) {
            return;
        }
        cx.stop_propagation();
        if !event.is_held {
            let script = script.clone();
            self.run_project_command(&id, &script, window, cx);
        }
    }

    /// Settings › Worktrees, where each project's commands are edited.
    pub(in crate::workspace) fn edit_project_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.close_code_top();
        self.open_settings(SettingsSection::Page(PageId::Worktrees), window, cx);
    }

    fn toggle_commands(&mut self, id: &str, project: &str, cx: &mut Context<Self>) {
        match self.overlay.code_top() {
            Some(TopPanel::Commands(open)) if open == id => {
                self.overlay.close_code_top();
            }
            _ => {
                // The Nexus app may have changed them since: the menu shows what was read, then what is saved.
                crate::project_commands::reload(project, cx);
                self.open_overlay(Overlay::CodeTop(TopPanel::Commands(id.to_string())));
            }
        }
        cx.notify();
    }

    /// The ▶ button with its menu under it while open; None while the project has no commands.
    pub(super) fn render_commands_control(
        &self,
        session: &CodeSession,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        crate::project_commands::load(&session.project_id, cx);
        let list = crate::project_commands::of(&session.project_id, cx);
        if list.is_empty() {
            return None;
        }
        let reason = Self::commands_unavailable(&session.id);
        let open = matches!(self.overlay.code_top(), Some(TopPanel::Commands(id)) if *id == session.id);
        let (id, project) = (session.id.clone(), session.project_id.clone());
        let button = bar_button("code-commands", Lucide::Play, palette)
            .when(open, |button| button.bg(palette.selected))
            .when(!open, |button| {
                button.tooltip(move |window, cx| Tooltip::new(reason.unwrap_or(RUN_HERE)).build(window, cx))
            })
            .menu_trigger(
                open,
                cx,
                {
                    let id = id.clone();
                    move |this| matches!(this.overlay.code_top(), Some(TopPanel::Commands(open)) if *open == id)
                },
                move |this, _, cx| this.toggle_commands(&id, &project, cx),
            );
        Some(
            div()
                .relative()
                .child(button)
                .when(open, |control| {
                    control.child(self.render_commands_menu(&session.id, list, reason, palette, cx))
                })
                .into_any_element(),
        )
    }

    fn render_commands_menu(
        &self,
        id: &str,
        list: Vec<ProjectScript>,
        reason: Option<&'static str>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mac = cfg!(target_os = "macos");
        let item = |key: SharedString, icon: Lucide, label: SharedString, enabled: bool| {
            h_flex()
                .id(key)
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .text_size(px(13.))
                .when(enabled, |row| {
                    row.cursor_pointer().hover(|style| menu_item_hover(style, palette))
                })
                .when(!enabled, |row| row.opacity(0.45))
                .child(glyph(icon, palette.icon).size(px(14.)))
                .child(div().flex_1().min_w(px(0.)).truncate().child(label))
        };
        let rows = list.into_iter().map(|script| {
            let key = script
                .shortcut
                .as_deref()
                .map(|key| crate::project_commands::key_label(key, mac))
                .filter(|label| !label.is_empty());
            let tip: SharedString = script.command.clone().into();
            let id = id.to_string();
            item(
                format!("code-command-{}", script.id).into(),
                Lucide::Play,
                script.name.clone().into(),
                reason.is_none(),
            )
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .children(key.map(|key| {
                div()
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(palette.faint)
                    .child(key)
            }))
            .when(reason.is_none(), |row| {
                row.on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.run_project_command(&id, &script, window, cx);
                }))
            })
        });
        let body = menu_box("code-commands-menu", 256., palette)
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                if matches!(this.overlay.code_top(), Some(TopPanel::Commands(_))) {
                    this.overlay.close_code_top();
                    cx.notify();
                }
            }))
            .child(menu_label(reason.unwrap_or("Run in a new terminal"), palette))
            .children(rows)
            .child(div().mx(px(8.)).my(px(4.)).h(px(1.)).bg(palette.border))
            .child(
                item(
                    "code-commands-edit".into(),
                    Lucide::Settings,
                    "Edit commands…".into(),
                    true,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.edit_project_commands(window, cx);
                })),
            );
        popover(body, palette)
    }
}
