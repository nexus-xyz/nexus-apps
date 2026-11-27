//! An open Code session's top bar, as the Nexus app has it (`workbench/topbar.tsx`): its title (double-click to
//! rename) with "Archived", "Snoozed" or "Settled" beside it, then its cost and context meter ([`code_meter`]), its
//! project and branch (the git panel's button, [`git_panel`]), where it runs, its pull requests ([`pull_chip`], or the
//! form that opens one, [`pr_form`]), a cloud session's Cloud button ([`cloud_panel`]), "…" and the project commands' ▶
//! ([`commands_menu`]). "…" is the row's menu with the open session's own actions added: Remove worktree,
//! and End the agent process.

#[path = "cloud_panel.rs"]
mod cloud_panel;
#[path = "code_meter.rs"]
pub(super) mod code_meter;
#[path = "commands_menu.rs"]
mod commands_menu;
#[path = "editors.rs"]
mod editors;
#[path = "git_panel.rs"]
pub(in crate::workspace) mod git_panel;
#[path = "pr_form.rs"]
mod pr_form;
pub(super) use pr_form::PrForm;
#[path = "pull_chip.rs"]
mod pull_chip;
#[path = "title_fit.rs"]
pub(super) mod title_fit;

use crate::workspace::MenuTrigger;
use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::component::input::Input;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace, glyph};
use crate::code::{self, Aside, CodeSession, Status};
use crate::i18n::tr;
use crate::motion;
use crate::palette::Palette;

/// The top bar's one open panel: the meter's card, the git panel, the pull request chip's card, the Cloud panel or the
/// project commands' menu, each for one session. The pull request form is not one: it is `Workspace::pr_form`, which
/// any of these replaces (`pr_form.rs`).
// One per window, so the git panel's size is not worth a `Box`.
#[allow(clippy::large_enum_variant)]
pub(super) enum TopPanel {
    Meter(String),
    Commands(String),
    Git(git_panel::GitPanel),
    /// The pull request chip's card ([`pull_chip`]).
    Pulls(pull_chip::PullsCard),
    Cloud(cloud_panel::CloudPanel),
}

/// Where a session works ([`Workspace::code_place`]).
pub(super) struct CodePlace<'a> {
    pub(super) project: Option<&'a crate::code::CodeProject>,
    pub(super) branch: Option<String>,
    pub(super) machine: Option<(Lucide, String)>,
}

impl CodePlace<'_> {
    /// What the git button says: the branch, or "Git".
    pub(super) fn branch_label(&self) -> String {
        self.branch.clone().unwrap_or_else(|| "Git".into())
    }
}

/// "Archived", "Snoozed" or "Settled", beside the title.
fn session_tag(session: &CodeSession) -> Option<&'static str> {
    if session.archived {
        Some("Archived")
    } else {
        session.aside(code::now_ms()).map(Aside::label)
    }
}

impl Workspace {
    /// The session with this id: in its project, or in the archive.
    pub(super) fn session_by_id(&self, id: &str) -> Option<&CodeSession> {
        self.projects
            .iter()
            .flat_map(|project| &project.sessions)
            .chain(&self.archived_code)
            .find(|session| session.id == id)
    }

    pub(super) fn render_code_title(&self, id: &str, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session_by_id(id);
        let renaming = self
            .renaming
            .as_ref()
            .filter(|renaming| renaming.in_title && renaming.id == id);
        let title = match renaming {
            Some(renaming) => div()
                .w(px(320.))
                // The field pads its text by 10px: the box reaches back so the title doesn't move.
                .ml(px(-10.))
                .rounded(px(6.))
                .bg(palette.selected)
                .text_color(palette.foreground)
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.renaming = None;
                    this.session_anchor = None;
                    cx.notify();
                }))
                .child(Input::new(&renaming.input).appearance(false))
                .into_any_element(),
            None => {
                let id = id.to_string();
                let menu_open = self
                    .overlay
                    .session_menu()
                    .as_ref()
                    .is_some_and(|menu| menu.in_title && menu.id == id);
                // The title is the session menu's button: 28 tall, its name and a
                // chevron; one click opens the menu, two rename in place.
                h_flex()
                    .id("code-title")
                    .debug_selector(|| "code-title".into())
                    .min_w(px(0.))
                    .h(px(28.))
                    .px(px(6.))
                    .ml(px(-6.))
                    .gap(px(4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_color(palette.foreground)
                    .font_weight(FontWeight::MEDIUM)
                    .when(menu_open, |title| title.bg(palette.selected))
                    .hover(|style| style.bg(palette.selected))
                    .when_some(self.session_anchor_handle(&id, true), |title, focus| {
                        title.track_focus(focus).tab_index(0)
                    })
                    .child(div().min_w(px(0.)).truncate().child(self.title.clone()))
                    .child(glyph(Lucide::ChevronDown, palette.faint))
                    .menu_trigger(
                        menu_open,
                        cx,
                        {
                            let id = id.clone();
                            move |this| {
                                this.overlay
                                    .session_menu()
                                    .as_ref()
                                    .is_some_and(|menu| menu.id == id && menu.in_title)
                            }
                        },
                        {
                            let id = id.clone();
                            move |this, window, cx| this.toggle_session_menu(&id, true, window, cx)
                        },
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, event: &MouseDownEvent, window, cx| {
                            // The title bar's own double-click zooms the window: here it renames instead.
                            if event.click_count == 2 {
                                cx.stop_propagation();
                                // After this press: during it, the field's focus would go back to the window.
                                let id = id.clone();
                                cx.defer_in(window, move |this, window, cx| this.start_rename(&id, true, window, cx));
                            }
                        }),
                    )
                    .on_click(|event, _, cx| {
                        if event.click_count() == 2 {
                            cx.stop_propagation();
                        }
                    })
                    .into_any_element()
            }
        };
        let tag = session.and_then(session_tag);
        let name = session
            .and_then(|session| session.name.as_deref())
            .filter(|name| !name.is_empty())
            .map(|name| {
                let selector = format!("title-session-name={name}");
                super::sidebar::session_name_tag(name, px(super::sidebar::TITLE_NAME_MAX).into(), palette)
                    .debug_selector(move || selector)
            });
        let tags = (name.is_some() || tag.is_some()).then(|| {
            // One measured group, so the bar knows what the tags take (`title_fit.rs`).
            h_flex()
                .relative()
                .flex_none()
                .gap(px(8.))
                .child(title_fit::measure(&self.title_fit.tags, cx))
                .children(name)
                .children(tag.map(|tag| {
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(palette.faint)
                        .child(tag)
                }))
        });
        h_flex()
            .relative()
            .flex_1()
            .min_w(px(0.))
            .pl(px(16.))
            .pr(px(4.))
            .gap(px(8.))
            .text_size(px(13.))
            .text_color(palette.muted)
            .child(title_fit::measure(&self.title_fit.row, cx))
            .child(div().relative().min_w(px(0.)).flex_shrink_1().child(title).when_some(
                session.and_then(|session| {
                    motion::presence_of(
                        format!("session-menu-title-{}", session.id),
                        self.overlay
                            .session_menu()
                            .as_ref()
                            .filter(|menu| menu.in_title && menu.id == session.id)
                            .map(|menu| menu.page),
                        cx,
                    )
                    .map(|presence| (session, presence))
                }),
                |wrap, (session, (phase, page))| {
                    wrap.child(self.render_session_menu(session, code::now_ms(), phase, page, true, palette, cx))
                },
            ))
            .children(tags)
            .child(div().flex_1())
            .children(session.map(|session| self.render_code_facts(session, palette, cx)))
    }

    /// Whether the title has tags beside it: the session's name, or "Archived", "Snoozed" or "Settled".
    pub(super) fn title_has_tags(&self, session: &CodeSession) -> bool {
        session.name.as_deref().is_some_and(|name| !name.is_empty()) || session_tag(session).is_some()
    }

    /// Where the session works: its project, its branch, and where it runs when that isn't this computer.
    pub(super) fn code_place<'a>(&'a self, session: &CodeSession) -> CodePlace<'a> {
        let project = self.projects.iter().find(|project| project.id == session.project_id);
        let branch = session
            .branch
            .clone()
            .or_else(|| project.and_then(|project| project.branch.clone()));
        let cloud = session
            .machine
            .as_deref()
            .is_some_and(|machine| self.cloud_machines.iter().any(|candidate| candidate.id == machine));
        let server = session
            .machine
            .as_deref()
            .and_then(|machine| self.servers.iter().find(|server| server.id == machine));
        // Where it runs shows only when it isn't here: a server's name or the cloud.
        let machine: Option<(Lucide, String)> = match server {
            _ if cloud => Some((Lucide::Cloud, tr("Cloud").to_string())),
            Some(server) => Some((Lucide::Server, server.name.clone())),
            None => None,
        };
        CodePlace {
            project,
            branch,
            machine,
        }
    }

    /// The bar's right side: the crew chip while it runs, the cost, one branch button
    /// (where it runs when that isn't this computer, then the branch in mono, then its pull request's number), the
    /// form that opens a pull request, a cloud session's Cloud button and the project commands. The session menu
    /// is the title's chevron. A tight bar sheds parts in the order `title_fit::Step` gives.
    fn render_code_facts(&self, session: &CodeSession, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let step = self.title_fit.step;
        let CodePlace {
            project,
            branch,
            machine,
        } = self.code_place(session);
        let label = branch.clone().unwrap_or_else(|| "Git".into());
        let runs_on = machine
            .as_ref()
            .map_or(crate::code::THIS_COMPUTER.to_string(), |(_, name)| name.clone());
        let tip: SharedString = [
            project.map(|project| project.name.clone()),
            branch.clone(),
            Some(runs_on),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
        .into();
        let git_open = matches!(self.overlay.code_top(), Some(TopPanel::Git(panel)) if panel.id == session.id);
        let git_id = session.id.clone();
        let git = h_flex()
            .id("code-git")
            .debug_selector(|| "code-git".into())
            .flex_none()
            .h(px(28.))
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(6.))
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(palette.muted)
            .cursor_pointer()
            .when(git_open, |fact| {
                fact.bg(palette.selected).text_color(palette.foreground)
            })
            .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
            .children(machine.map(|(icon, name)| {
                h_flex()
                    .gap(px(6.))
                    .child(glyph(icon, palette.icon))
                    .when(step.machine_name(), |machine| {
                        machine.child(div().max_w(px(title_fit::MACHINE_MAX)).truncate().child(name))
                    })
            }))
            .child(glyph(Lucide::GitBranch, palette.icon))
            .when(step.branch_name(), |git| {
                git.child(
                    div()
                        .max_w(px(step.branch_max()))
                        .truncate()
                        .font_family("Geist Mono")
                        .child(label),
                )
            })
            .when(!git_open, |fact| {
                fact.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            })
            .menu_trigger(
                git_open,
                cx,
                {
                    let id = git_id.clone();
                    move |this| matches!(this.overlay.code_top(), Some(TopPanel::Git(panel)) if panel.id == id)
                },
                move |this, _, cx| this.toggle_git_panel(&git_id, cx),
            );
        // The session's pull requests (the chip and its card), or "Open PR" for a worktree with none.
        let word = project.map_or("pull request", |project| git_panel::request_word(&project.remotes));
        let pulls = self.render_pull_chip(session, word, palette, cx);
        let pr_form = self.render_pr_form(session, palette, cx);
        h_flex()
            .relative()
            .flex_none()
            .gap(px(4.))
            .child(title_fit::measure(&self.title_fit.facts, cx))
            // While the session has sub-agents or background commands (`tasks-indicator.tsx`).
            .children(self.render_crew_pill(step.crew_words(), palette, cx))
            .when(step.cost(), |facts| {
                facts.child(self.render_meter(session, step.turn_cost(), palette, cx))
            })
            .child(
                div()
                    .relative()
                    .child(git)
                    .children(self.render_git_panel(session, palette, cx)),
            )
            .child(div().relative().children(pulls).children(pr_form))
            .children(self.render_cloud_button(&session.id, palette, cx))
            // Last, beside the Workbench's button, as the Nexus app's top bar has it.
            .children(self.render_commands_control(session, palette, cx))
    }

    /// The top bar menu's own items, between the row's items and Delete.
    pub(super) fn title_menu_items(
        &self,
        session: &CodeSession,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let at_rest = matches!(session.status, Status::Idle | Status::Failed);
        let item = |key: &'static str, icon: Lucide, label: SharedString, enabled: bool| {
            h_flex()
                .id(key)
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(8.))
                .when(enabled, |row| {
                    row.cursor_pointer().hover(|style| style.bg(palette.hover))
                })
                .when(!enabled, |row| row.opacity(0.45))
                .child(glyph(icon, palette.icon).size(px(16.)))
                .child(div().flex_1().truncate().child(label))
        };
        let mut items = Vec::new();
        if session.branch.is_some() {
            let id = session.id.clone();
            items.push(
                item(
                    "title-remove-worktree",
                    Lucide::FolderMinus,
                    tr("Remove worktree"),
                    at_rest,
                )
                .tooltip(|window, cx| {
                    Tooltip::new(tr(
                        "Only when nothing would be lost: no uncommitted changes, no unpushed commits",
                    ))
                    .build(window, cx)
                })
                .when(at_rest, |row| {
                    row.on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.overlay.close_session_menu();
                        match this.runs(&id) {
                            Some(runs) => runs.remove_worktree(id.clone()),
                            None => this.refuse_read_only(),
                        }
                        cx.notify();
                    }))
                })
                .into_any_element(),
            );
        }
        items.push(
            item("title-terminal", Lucide::SquareTerminal, tr("Terminal"), true)
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.overlay.close_session_menu();
                    this.pick_tool(super::side_pane::Tool::Terminal, window, cx);
                }))
                .into_any_element(),
        );
        let offered = match &session.machine {
            Some(machine) => crate::machines::get().has_feature(machine, nexus_workbench::orchestra::FEATURE),
            None => self.runner.is_some(),
        };
        if nexus_workbench::orchestra::switch_shown(offered, session.at_depth_limit) {
            let id = session.id.clone();
            let next = !session.orchestration;
            let restart = next && session.agent_started;
            items.push(
                item("title-orchestration", Lucide::Users, tr("Subsessions"), true)
                    .child(
                        div()
                            .text_color(palette.muted)
                            .child(if session.orchestration { "On" } else { "Off" }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_session_orchestration(id.clone(), next, restart, cx);
                    }))
                    .into_any_element(),
            );
        }
        let id = session.id.clone();
        items.push(
            item("title-stop", Lucide::CircleStop, tr("End the agent process"), !at_rest)
                .when(!at_rest, |row| {
                    row.on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.overlay.close_session_menu();
                        match this.runs(&id) {
                            Some(runs) => runs.stop(id.clone()),
                            None => this.refuse_read_only(),
                        }
                        cx.notify();
                    }))
                })
                .into_any_element(),
        );
        items
    }

    fn toggle_session_orchestration(&mut self, id: String, next: bool, restart: bool, cx: &mut Context<Self>) {
        let Some(runs) = self.runs(&id) else { return };
        let answer = runs.call(
            "wb_thread_update",
            serde_json::json!({ "threadId": id, "orchestration": next }),
        );
        self.overlay.close_session_menu();
        cx.spawn(async move |this, cx| {
            let answer = answer
                .await
                .unwrap_or_else(|_| Err(tr("The update's answer was lost.").to_string().into()));
            let _ = this.update(cx, |this, cx| {
                match answer {
                    Ok(_) => {
                        this.refresh_code(cx);
                        if restart {
                            this.open_overlay(Overlay::SessionMenu(super::session_menu::SessionMenu {
                                id,
                                page: super::session_menu::MenuPage::RestartOrchestration,
                                in_title: true,
                            }));
                        }
                    }
                    Err(error) => {
                        this.composer_note = Some(
                            tr("Couldn't turn subsessions on or off: {error}")
                                .replace("{error}", &error.to_string())
                                .into(),
                        )
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
