//! The model panel's agents that can't run sessions where a new session starts. They fold into one quiet row
//! under the models; open, each agent is one row the height of a model's. The focused agent's setup shows in the
//! right pane, and Install runs its command in the side pane's Terminal, where the person sees it.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::driver::label;
use nexus_workbench::event::SessionAgent;

use super::harness::{install_command, readiness};
use super::model_menu::maker_mark;
use super::new_code::agent_maker;
use super::{Workspace, menu_item_hover};
use crate::palette::Palette;

/// How dim an agent that can't run is, beside the models that can.
const DIM: f32 = 0.55;

/// One row under the models, after the model rows in the cursor's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MissingRow {
    /// The fold over every agent that can't run here, with how many.
    Fold(usize),
    Agent(SessionAgent),
}

impl MissingRow {
    /// The row's key for `sync_model_rows`, which keeps the cursor on it while rows come and go.
    pub(super) fn key(self) -> String {
        match self {
            MissingRow::Fold(_) => "missing:fold".into(),
            MissingRow::Agent(agent) => format!("missing:{}", agent.as_str()),
        }
    }
}

/// The rows under the models: none when every agent runs. A search lists the agents whose name matches, with no
/// fold. Otherwise the fold comes first, and each agent follows it while it is open.
pub(super) fn missing_rows(missing: &[SessionAgent], open: bool, query: &str) -> Vec<MissingRow> {
    let query = query.trim().to_lowercase();
    if !query.is_empty() {
        return missing
            .iter()
            .filter(|agent| label(**agent).to_lowercase().contains(&query))
            .map(|agent| MissingRow::Agent(*agent))
            .collect();
    }
    if missing.is_empty() {
        return Vec::new();
    }
    let mut rows = vec![MissingRow::Fold(missing.len())];
    if open {
        rows.extend(missing.iter().map(|agent| MissingRow::Agent(*agent)));
    }
    rows
}

/// The fold's words. Closed, it counts the agents; open, it is their heading. `place` is a server's name.
pub(super) fn fold_label(count: usize, open: bool, place: Option<&str>) -> String {
    let place = place.map_or_else(|| "here".to_string(), |place| format!("on {place}"));
    match (open, count) {
        (true, _) => format!("Not set up {place}"),
        (false, 1) => format!("1 agent not set up {place}"),
        (false, count) => format!("{count} agents not set up {place}"),
    }
}

/// The right pane's main button: an installed agent that can't run sessions updates; a missing one installs.
pub(super) fn install_label(installed: bool, place: Option<&str>) -> String {
    let verb = if installed { "Update" } else { "Install" };
    match place {
        Some(place) => format!("{verb} on {place}"),
        None => format!("{verb} in Terminal"),
    }
}

/// The command the side pane's Terminal runs. `server` is where the session starts (its id and ssh target) and
/// `terminal_on` the server the Terminal's shells run on: a Terminal already on that server runs the command bare,
/// and one on this computer reaches the server over ssh.
pub(super) fn terminal_command(
    agent: SessionAgent,
    gateway: &str,
    server: Option<(&str, &str)>,
    terminal_on: Option<&str>,
) -> String {
    let ssh = server.filter(|(id, _)| terminal_on != Some(*id)).map(|(_, ssh)| ssh);
    install_command(agent, gateway, ssh)
}

impl Workspace {
    /// The server a new session starts on, when it is not this computer.
    fn missing_server(&self) -> Option<&crate::machines::ServerView> {
        self.harness_machine()
            .and_then(|id| self.servers.iter().find(|server| server.id == id))
    }

    /// The agents that can't run sessions where the new session starts. None on Cloud, and none in an open session's
    /// panel, which keeps its agent.
    pub(super) fn missing_agents(&self) -> Vec<SessionAgent> {
        if self.cloud_ui.on || self.session_pick().is_some() {
            return Vec::new();
        }
        SessionAgent::ALL
            .into_iter()
            .filter(|agent| self.offers_agent(*agent))
            .filter(|agent| self.launch_harness_info(*agent).is_some_and(|info| info.path.is_none()))
            .collect()
    }

    /// The rows under the models. The fold opens by itself when no model can be picked.
    pub(super) fn missing_panel_rows(&self) -> Vec<MissingRow> {
        let open = self.code_form.missing_open || self.launch_rows().is_empty();
        missing_rows(&self.missing_agents(), open, &self.model_query)
    }

    /// The row under the models at the cursor, when the cursor is past the model rows.
    pub(super) fn missing_at_cursor(&self, model_rows: usize, missing: &[MissingRow]) -> Option<MissingRow> {
        self.model_cursor
            .checked_sub(model_rows)
            .and_then(|ix| missing.get(ix))
            .copied()
    }

    /// Enter or a click on a row under the models: the fold opens or closes, and an agent installs.
    pub(super) fn pick_missing_row(&mut self, row: MissingRow, window: &mut Window, cx: &mut Context<Self>) {
        match row {
            MissingRow::Fold(_) => self.code_form.missing_open = !self.code_form.missing_open,
            MissingRow::Agent(agent) => self.install_in_terminal(agent, window, cx),
        }
        cx.notify();
    }

    /// Install `agent` where the session starts: the panel closes, and the side pane's Terminal opens and runs the
    /// agent's install command, shown and focused.
    pub(super) fn install_in_terminal(&mut self, agent: SessionAgent, window: &mut Window, cx: &mut Context<Self>) {
        let server = self
            .missing_server()
            .map(|server| (server.id.clone(), server.ssh.clone()));
        self.close_launch_menu(cx);
        // The side pane follows the new session's project first, so its Terminal is on the right machine.
        self.sync_side(window, cx);
        let command = terminal_command(
            agent,
            self.gateway.url(),
            server.as_ref().map(|(id, ssh)| (id.as_str(), ssh.as_str())),
            self.side_server(),
        );
        let name = format!("Install {}", label(agent));
        self.run_in_side_terminal(&command, Some(&name), window, cx);
    }

    /// The rows under the models in the panel's list, then the way to Settings › Coding agents. `first` is the
    /// cursor's index of the first row.
    pub(super) fn render_missing_rows(
        &self,
        rows: &[MissingRow],
        first: usize,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let place = self.missing_server().map(|server| server.name.to_string());
        let open = self.code_form.missing_open || self.launch_rows().is_empty();
        let mut out = Vec::with_capacity(rows.len() + 1);
        for (offset, row) in rows.iter().copied().enumerate() {
            let ix = first + offset;
            let frame = h_flex()
                .id(("missing-row", ix))
                .flex_none()
                .h(px(28.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(7.))
                .cursor_pointer()
                .text_size(px(13.))
                .when(ix == self.model_cursor, |item| item.bg(palette.menu_hover))
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if this.model_cursor != ix {
                        this.model_cursor = ix;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| this.pick_missing_row(row, window, cx)));
            let element =
                match row {
                    MissingRow::Fold(count) => {
                        let marks = (!open).then(|| {
                            h_flex().flex_none().gap(px(2.)).opacity(DIM).children(
                                self.missing_agents()
                                    .into_iter()
                                    .take(3)
                                    .map(|agent| maker_mark(agent_maker(agent), 12., palette)),
                            )
                        });
                        frame
                            .text_color(palette.muted)
                            .child(
                                Icon::new(if open {
                                    Lucide::ChevronDown
                                } else {
                                    Lucide::ChevronRight
                                })
                                .size(px(13.))
                                .text_color(palette.icon),
                            )
                            .child(div().flex_1().min_w(px(0.)).truncate().child(fold_label(
                                count,
                                open,
                                place.as_deref(),
                            )))
                            .children(marks)
                    }
                    MissingRow::Agent(agent) => {
                        let installed = self
                            .launch_harness_info(agent)
                            .is_some_and(|info| info.problem.is_some());
                        frame
                            .child(
                                div()
                                    .flex_none()
                                    .opacity(DIM)
                                    .child(maker_mark(agent_maker(agent), 13., palette)),
                            )
                            .child(div().flex_1().min_w(px(0.)).truncate().opacity(DIM).child(label(agent)))
                            .child(
                                h_flex()
                                    .flex_none()
                                    .gap(px(5.))
                                    .text_size(px(11.5))
                                    .text_color(palette.faint)
                                    .when(installed, |trail| trail.child(warning_dot(palette)))
                                    .child(if installed { "Fix" } else { "Install" }),
                            )
                    }
                };
            out.push(element.into_any_element());
        }
        if self.model_query.trim().is_empty() {
            out.push(
                div()
                    .id("missing-agents-settings")
                    .mx(px(2.))
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.foreground))
                    .child("Coding agents settings")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings(super::SettingsSection::Page(crate::pages::PageId::Agents), window, cx);
                    }))
                    .into_any_element(),
            );
        }
        out
    }

    /// The right pane for a row under the models: the fold explains itself; an agent says why it can't run, shows
    /// its command, and offers Install (or Update) and Check again.
    pub(super) fn render_missing_detail(&self, row: MissingRow, palette: Palette, cx: &mut Context<Self>) -> Div {
        let place = self.missing_server().map(|server| server.name.to_string());
        let where_ = place.as_deref().unwrap_or("this computer");
        let pane = v_flex().p(px(16.)).gap(px(12.)).text_size(px(12.5));
        let head = |mark: AnyElement, title: String| {
            h_flex()
                .gap(px(10.))
                .child(
                    div()
                        .flex_none()
                        .size(px(30.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(8.))
                        .bg(palette.raised)
                        .child(mark),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
        };
        let agent = match row {
            MissingRow::Fold(count) => {
                let text = format!(
                    "{} can't start sessions on {where_} yet. Open the list to install one in the Terminal.",
                    if count == 1 { "One agent" } else { "These agents" }
                );
                return pane
                    .child(head(
                        Icon::new(Lucide::SquareTerminal)
                            .size(px(15.))
                            .text_color(palette.icon)
                            .into_any_element(),
                        fold_label(count, true, place.as_deref()),
                    ))
                    .child(div().text_color(palette.muted).child(text));
            }
            MissingRow::Agent(agent) => agent,
        };
        let found = self.launch_harness_info(agent);
        let problem = found.and_then(|info| info.problem.clone());
        let installed = problem.is_some();
        let server = self
            .missing_server()
            .map(|server| (server.id.as_str(), server.ssh.as_str()));
        let shown = terminal_command(agent, self.gateway.url(), server, None);
        let copy = shown.clone();
        let button = |id: &'static str, text: String, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .h(px(28.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(px(8.))
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .map(|button| {
                    if primary {
                        button
                            .bg(palette.primary)
                            .text_color(palette.primary_foreground)
                            .hover(|style| style.opacity(0.86))
                    } else {
                        button
                            .bg(palette.hover)
                            .text_color(palette.foreground)
                            .hover(|style| menu_item_hover(style, palette))
                    }
                })
                .child(text)
        };
        pane.child(head(
            maker_mark(agent_maker(agent), 16., palette).into_any_element(),
            label(agent).to_string(),
        ))
        .child(
            h_flex()
                .gap(px(6.))
                .text_color(palette.muted)
                .when(installed, |line| line.child(warning_dot(palette)))
                .child(readiness(found, where_)),
        )
        .child(div().text_color(palette.foreground).child(problem.unwrap_or_else(|| {
            format!(
                "Install {} on {where_} to run its sessions there. Then check again.",
                label(agent)
            )
        })))
        .child(
            h_flex()
                .h(px(30.))
                .px(px(10.))
                .gap(px(8.))
                .rounded(px(7.))
                .bg(palette.hover)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .font_family("monospace")
                        .text_size(px(11.5))
                        .text_color(palette.muted)
                        .child(shown),
                )
                .child(
                    div()
                        .id("missing-copy-command")
                        .flex_none()
                        .p(px(3.))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .hover(|style| menu_item_hover(style, palette))
                        .tooltip(|window, cx| Tooltip::new("Copy the command").build(window, cx))
                        .child(Icon::new(Lucide::Copy).size(px(13.)).text_color(palette.icon))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        })),
                ),
        )
        .child(
            h_flex()
                .gap(px(8.))
                .child(
                    button("missing-install", install_label(installed, place.as_deref()), true)
                        .on_click(cx.listener(move |this, _, window, cx| this.install_in_terminal(agent, window, cx))),
                )
                .child(
                    button("missing-check-again", "Check again".into(), false)
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_harness(cx))),
                ),
        )
    }
}

/// A still mark for an agent that is installed but can't run sessions: someone must act, and nothing moves.
fn warning_dot(palette: Palette) -> Div {
    div().flex_none().size(px(7.)).rounded_full().bg(palette.status_waiting)
}

#[cfg(test)]
#[path = "missing_agents_tests.rs"]
mod tests;
