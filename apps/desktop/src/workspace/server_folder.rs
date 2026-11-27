//! Remote folder and clone requests for Add a project and New session’s project menu.
//! Paths and Git credentials belong to the selected computer.

use futures::FutureExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::json;

use super::clone_form::{DEFAULT_PARENT, POLL, clone_field, clone_progress, picked_repo, ready};
use super::sidebar::small_button;
use super::{Workspace, glyph};
use crate::palette::Palette;

/// The open "A folder on <server>" field.
pub(super) struct ServerFolder {
    pub(super) machine: String,
    pub(super) input: Entity<InputState>,
    pub(super) adding: bool,
    pub(super) problem: Option<SharedString>,
    /// The clone form, once "Clone a repository" was picked.
    pub(super) clone: Option<ServerClone>,
    /// Open in New session's project menu, not under the server's head in the sidebar.
    pub(super) in_menu: bool,
    _subscription: Subscription,
}

/// "Clone a repository on <server>": its fields, and the clone while it runs.
pub(super) struct ServerClone {
    pub(super) source: Entity<InputState>,
    pub(super) parent: Entity<InputState>,
    /// The repository picked from GitHub, by name: Repository shows it, not a field to type in.
    pub(super) picked: Option<SharedString>,
    /// The clone running, by its request id.
    pub(super) running: Option<String>,
    /// Git's stage and percentage, while it runs.
    pub(super) progress: Option<(String, Option<u8>)>,
    pub(super) problem: Option<SharedString>,
    pub(super) uncertain: bool,
    _poll: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ServerFolder {
    /// A clone runs from this field: it stays open until the clone ends, to show how it went.
    pub(super) fn cloning(&self) -> bool {
        self.clone.as_ref().is_some_and(|clone| clone.running.is_some())
    }
}

impl Workspace {
    /// Open Add a project with this server selected. A running request keeps its existing dialog.
    pub(super) fn toggle_server_folder(&mut self, machine: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_project_dialog(Some(machine.to_string()), window, cx);
    }

    /// Opens server `machine`'s field in New session's project menu, with the clone form when `clone`.
    pub(super) fn open_server_folder_in_menu(
        &mut self,
        machine: &str,
        clone: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.server_folder.as_ref().is_some_and(ServerFolder::cloning) {
            return;
        }
        self.open_server_folder(machine, true, window, cx);
        if clone {
            self.open_server_clone(window, cx);
        }
    }

    /// The server whose field is open in New session's project menu.
    pub(super) fn server_folder_in_menu(&self) -> Option<&str> {
        self.server_folder
            .as_ref()
            .filter(|folder| folder.in_menu)
            .map(|folder| folder.machine.as_str())
    }

    /// The project menu opens afresh: a field it left open, with no clone running, goes (the picker's `setCloning`).
    pub(super) fn drop_idle_menu_server_folder(&mut self) {
        if self
            .server_folder
            .as_ref()
            .is_some_and(|folder| folder.in_menu && !folder.cloning())
        {
            self.server_folder = None;
        }
    }

    pub(super) fn open_server_folder(
        &mut self,
        machine: &str,
        in_menu: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.server_name(machine);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(format!("A folder on {name}: ~/code/app")));
        input.update(cx, |state, cx| state.focus(window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.add_server_folder(window, cx),
            InputEvent::Change => {
                if let Some(folder) = &mut this.server_folder {
                    folder.problem = None;
                }
                cx.notify();
            }
            _ => {}
        });
        self.server_folder = Some(ServerFolder {
            machine: machine.to_string(),
            input,
            adding: false,
            problem: None,
            clone: None,
            in_menu,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn server_name(&self, machine: &str) -> String {
        self.servers
            .iter()
            .find(|server| server.id == machine)
            .map_or_else(|| "the server".to_string(), |server| server.name.clone())
    }

    /// Adds the folder typed as a project on its server, then opens a new session in it.
    pub(super) fn add_server_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .server_folder
            .as_ref()
            .is_some_and(|folder| !self.project_target_ready(Some(&folder.machine), false))
        {
            return;
        }
        let Some(folder) = &mut self.server_folder else {
            return;
        };
        let path = folder.input.read(cx).value().trim().to_string();
        if path.is_empty() || folder.adding {
            return;
        }
        folder.adding = true;
        folder.problem = None;
        let machine = folder.machine.clone();
        let answer = crate::machines::get().call(&machine, "wb_project_add", json!({ "path": path }));
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer
                .map(|answer| answer.unwrap_or_else(|_| Err("The server's answer was lost.".into())))
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if !this
                    .server_folder
                    .as_ref()
                    .is_some_and(|folder| folder.machine == machine && folder.adding)
                {
                    return;
                }
                match answer.and_then(|project| {
                    project["id"]
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| "The server answered something else.".to_string())
                }) {
                    Ok(project) => {
                        if this.server_folder.as_ref().is_some_and(|folder| !folder.in_menu) {
                            this.project_dialog = None;
                        }
                        this.server_folder = None;
                        this.refresh_code(cx);
                        this.open_new_code_in(project, window, cx);
                    }
                    Err(error) => {
                        if let Some(folder) = &mut this.server_folder {
                            folder.adding = false;
                            folder.problem = Some(crate::machine_view::plain_error(&error).into());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Shows the clone form in the open field: Repository, and Into (the server's home folder to start).
    pub(super) fn open_server_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(machine) = self.server_folder.as_ref().map(|folder| folder.machine.clone()) else {
            return;
        };
        let name = self.server_name(&machine);
        let saved = crate::paths::clone_parent(Some(&machine)).unwrap_or_else(|| DEFAULT_PARENT.to_string());
        let source = cx.new(|cx| InputState::new(window, cx).placeholder("https://… or owner/name"));
        let parent = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(format!("A folder on {name}: ~/code"));
            state.set_value(saved, window, cx);
            state
        });
        let submit = |this: &mut Workspace,
                      _: &Entity<InputState>,
                      event: &InputEvent,
                      window: &mut Window,
                      cx: &mut Context<Workspace>| {
            if let InputEvent::PressEnter { .. } = event {
                this.start_server_clone(window, cx);
            } else if let InputEvent::Change = event {
                if let Some(form) = &mut this.server_folder {
                    if let Some(clone) = &mut form.clone {
                        if !clone.uncertain {
                            clone.problem = None;
                        }
                    }
                }
                cx.notify();
            }
        };
        let subscriptions = vec![
            cx.subscribe_in(&source, window, submit),
            cx.subscribe_in(&parent, window, submit),
        ];
        source.update(cx, |state, cx| state.focus(window, cx));
        if let Some(folder) = &mut self.server_folder {
            folder.problem = None;
            folder.clone = Some(ServerClone {
                source,
                picked: None,
                parent,
                running: None,
                progress: None,
                problem: None,
                uncertain: false,
                _poll: None,
                _subscriptions: subscriptions,
            });
        }
        cx.notify();
    }

    /// Clones on the field's server, reads git's progress there every `POLL`, and opens the clone when it ends.
    pub(super) fn start_server_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .server_folder
            .as_ref()
            .is_some_and(|folder| !self.project_target_ready(Some(&folder.machine), true))
        {
            return;
        }
        let Some(folder) = &mut self.server_folder else {
            return;
        };
        let machine = folder.machine.clone();
        let Some(clone) = &mut folder.clone else {
            return;
        };
        let source = clone.source.read(cx).value().trim().to_string();
        let parent = clone.parent.read(cx).value().trim().to_string();
        if clone.running.is_some() || clone.uncertain || !ready(&source, &parent) {
            return;
        }
        crate::paths::save_clone_parent(Some(&machine), &parent);
        let request = uuid::Uuid::new_v4().to_string();
        clone.running = Some(request.clone());
        clone.progress = None;
        clone.problem = None;
        let poll_machine = machine.clone();
        let poll_request = request.clone();
        clone._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let progress = crate::machines::get()
                    .clone_progress(&poll_machine, &poll_request)
                    .await;
                let alive = this.update(cx, |this, cx| {
                    let Some(clone) = this.server_clone_running(&poll_request) else {
                        return false;
                    };
                    if progress.is_some() {
                        clone.progress = progress;
                        cx.notify();
                    }
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }));
        cx.spawn_in(window, async move |this, cx| {
            let cloned = crate::machines::get()
                .clone_project(&machine, &source, &parent, &request)
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.server_clone_running(&request).is_none() {
                    return;
                }
                match cloned {
                    Ok(project) => {
                        if this.server_folder.as_ref().is_some_and(|folder| !folder.in_menu) {
                            this.project_dialog = None;
                        }
                        this.server_folder = None;
                        this.refresh_code(cx);
                        this.open_new_code_in(project, window, cx);
                    }
                    Err(error) => {
                        if let Some(clone) = this.server_clone_running(&request) {
                            clone.running = None;
                            clone.progress = None;
                            clone._poll = None;
                            // git's own words, as the Nexus app's clone form shows them: an SSH failure here is
                            // the server's git reaching the repository, not this computer reaching the server.
                            clone.uncertain = uncertain_clone(&error);
                            clone.problem = Some(if clone.uncertain {
                                format!("{error} Clone status is unknown. Check the destination on this computer before cloning again.").into()
                            } else { error.into() });
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The open clone form while clone `request` runs from it.
    fn server_clone_running(&mut self, request: &str) -> Option<&mut ServerClone> {
        self.server_folder
            .as_mut()?
            .clone
            .as_mut()
            .filter(|clone| clone.running.as_deref() == Some(request))
    }

    /// The field for server `machine`, while it is open there: under its head in the sidebar, or in New session's
    /// project menu when `in_menu`.
    pub(super) fn render_server_folder(
        &self,
        machine: &str,
        in_menu: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let folder = self
            .server_folder
            .as_ref()
            .filter(|folder| folder.machine == machine && folder.in_menu == in_menu)?;
        let clones = self
            .servers
            .iter()
            .any(|server| server.id == machine && crate::machines::clones(&server.status));
        let close = small_button("server-folder-close", Lucide::X, palette).on_click(cx.listener(|this, _, _, cx| {
            if this.server_folder.as_ref().is_some_and(|folder| !folder.cloning()) {
                this.server_folder = None;
                cx.notify();
            }
        }));
        if let Some(clone) = &folder.clone {
            return Some(self.render_server_clone(clone, &self.server_name(machine), close, palette, cx));
        }
        let add = div()
            .id("server-folder-add")
            .px(px(8.))
            .h(px(24.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .text_xs()
            .text_color(palette.foreground)
            .bg(palette.selected)
            .when(!folder.adding, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover))
                    .on_click(cx.listener(|this, _, window, cx| this.add_server_folder(window, cx)))
            })
            .child(if folder.adding { "Adding…" } else { "Add" });
        let clone_link = h_flex()
            .id("server-clone-open")
            .h(px(24.))
            .px(px(4.))
            .gap(px(6.))
            .rounded(px(6.))
            .text_xs()
            .text_color(palette.muted)
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
            .child(glyph(Lucide::GitBranch, palette.icon).size(px(13.)))
            .child("Clone a repository")
            .on_click(cx.listener(|this, _, window, cx| this.open_server_clone(window, cx)));
        Some(
            v_flex()
                .px(px(8.))
                .py(px(4.))
                .gap(px(4.))
                .child(
                    h_flex()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .h(px(28.))
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .bg(palette.hover)
                                .font_family("monospace")
                                .text_size(px(12.))
                                .child(Input::new(&folder.input).appearance(false)),
                        )
                        .child(add)
                        .child(close),
                )
                .children(
                    folder
                        .problem
                        .clone()
                        .map(|problem| div().text_xs().text_color(palette.destructive).child(problem)),
                )
                .when(clones, |field| field.child(clone_link))
                .into_any_element(),
        )
    }

    /// "Clone a repository on <server>": Repository, Into, git's progress while it runs, why it failed, and Clone.
    fn render_server_clone(
        &self,
        clone: &ServerClone,
        name: &str,
        close: Stateful<Div>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let running = clone.running.is_some();
        let can_clone = !running
            && !clone.uncertain
            && ready(
                clone.source.read(cx).value().as_ref(),
                clone.parent.read(cx).value().as_ref(),
            );
        let clone_button = div()
            .id("server-clone-go")
            .flex_none()
            .px(px(12.))
            .h(px(28.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(palette.border)
            .text_color(if can_clone { palette.foreground } else { palette.faint })
            .when(can_clone, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover))
                    .on_click(cx.listener(|this, _, window, cx| this.start_server_clone(window, cx)))
            })
            .child(if running { "Cloning…" } else { "Clone" });
        v_flex()
            .mx(px(8.))
            .my(px(4.))
            .p(px(8.))
            .gap(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(palette.border)
            .text_size(px(13.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .text_color(palette.foreground)
                    .child(glyph(Lucide::GitBranch, palette.icon).size(px(15.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .child(format!("Clone a repository on {name}")),
                    )
                    .when(!running, |head| head.child(close)),
            )
            .child(match &clone.picked {
                Some(name) => picked_repo(name.clone(), palette),
                None => clone_field("Repository", &clone.source, running, palette),
            })
            .child(clone_field("Into", &clone.parent, running, palette))
            .when(running, |body| {
                body.child(clone_progress(clone.progress.as_ref(), palette))
            })
            .when_some(clone.problem.clone(), |body, problem| {
                body.child(div().text_xs().text_color(palette.destructive).child(problem))
            })
            .child(h_flex().justify_end().child(clone_button))
            .into_any_element()
    }
}

/// A lost transport reply does not prove that Git stopped on the remote computer.
fn uncertain_clone(error: &str) -> bool {
    let error = error.to_lowercase();
    ["lost", "closed", "disconnected", "timed out", "not connected"]
        .iter()
        .any(|part| error.contains(part))
}

#[cfg(test)]
mod tests {
    use super::uncertain_clone;
    #[test]
    fn a_lost_reply_blocks_retry_but_a_git_refusal_can_be_retried() {
        assert!(uncertain_clone("The server's answer was lost."));
        assert!(uncertain_clone("Connection closed"));
        assert!(!uncertain_clone("Permission denied (publickey)."));
        assert!(!uncertain_clone("destination path already exists"));
    }
}
