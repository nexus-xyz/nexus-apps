//! Session facts read from the same summary as the crew.
use super::*;
use crate::palette::Palette;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;

impl Workspace {
    pub(super) fn render_agent_facts(
        &self,
        mut body: Stateful<Div>,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        use crate::workspace::code_title::code_meter::{compact, permissions, usd};
        let state = self.side.agents.as_ref().unwrap();
        let Some(summary) = &state.summary else {
            return body;
        };
        if let Some((used, capacity)) = summary
            .context_tokens
            .zip(summary.context_window)
            .filter(|(_, size)| *size > 0)
        {
            let share = (used as f64 / capacity as f64).clamp(0., 1.) as f32;
            body = body.child(self.agent_fact_heading(
                "Context usage",
                format!("{}% full", (share * 100.).round() as u32),
                p,
                cx,
            ));
            if state.facts_open.contains("Context usage") {
                body = body
                    .child(format!("{} / {} tokens", compact(used), compact(capacity)))
                    .child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded_full()
                            .bg(p.border)
                            .child(div().w(relative(share)).h_full().rounded_full().bg(p.strong)),
                    );
                if let View::Code(view) = &self.view
                    && view.id == state.id
                    && view.loaded
                {
                    body = body.child(div().text_color(p.faint).child(if view.more {
                        "Estimated from loaded history. Load older messages for more detail."
                    } else if view.context_estimate.since_compaction {
                        "Estimated since the last compaction or clear."
                    } else {
                        "Estimated from this session's messages and tool calls."
                    }));
                    for part in view.context_estimate.parts(used) {
                        body = body.child(
                            h_flex()
                                .justify_between()
                                .gap_2()
                                .child(div().flex_1().child(part.label))
                                .child(compact(part.tokens)),
                        );
                    }
                }
            }
        }
        let project = self.projects.iter().find(|project| project.id == summary.project_id);
        let remote = crate::machines::get().machine_of(&state.id);
        let remote_project = remote.as_ref().and_then(|machine| {
            crate::machines::get()
                .projects_on(machine)
                .into_iter()
                .find(|project| project.id == summary.project_id)
        });
        let name = remote_project
            .as_ref()
            .map(|project| project.name.clone())
            .or_else(|| project.map(|project| project.name.clone()))
            .unwrap_or_default();
        body = body.child(self.agent_fact_heading("Environment", name, p, cx));
        if state.facts_open.contains("Environment") {
            let computer = remote
                .map(|id| {
                    crate::machines::get()
                        .servers()
                        .into_iter()
                        .find(|server| server.id == id)
                        .map(|server| server.name)
                        .unwrap_or_else(|| "Remote server".into())
                })
                .unwrap_or_else(|| "This computer".into());
            let folder = summary
                .worktree
                .as_ref()
                .map(|worktree| worktree.path.clone())
                .or_else(|| remote_project.as_ref().map(|project| project.path.clone()))
                .unwrap_or_else(|| self.agents_folder_label());
            body = body
                .child(format!("Computer · {computer}"))
                .child(format!("Folder · {folder}"));
            if let Some(branch) = summary
                .worktree
                .as_ref()
                .map(|worktree| worktree.branch.clone())
                .or_else(|| remote_project.as_ref().and_then(|project| project.branch.clone()))
                .or_else(|| project.and_then(|project| project.branch.clone()))
            {
                body = body.child(format!("Branch · {branch}"));
            }
            if summary.worktree.as_ref().is_some_and(|worktree| worktree.removed) {
                body = body.child("Worktree removed");
            }
            if let Some(url) = summary
                .worktree
                .as_ref()
                .and_then(|worktree| worktree.pull_request.clone())
            {
                body = body.child(
                    div()
                        .id("agents-pull-request")
                        .text_color(p.strong)
                        .cursor_pointer()
                        .child("Pull request ↗")
                        .on_click(move |_, _, cx| crate::external_link::open(&url, cx)),
                );
            }
        }
        body = body.child(self.agent_fact_heading(
            "Session",
            format!("{} · {}", crew_fold::agent_name(summary.agent), usd(summary.cost_usd)),
            p,
            cx,
        ));
        if state.facts_open.contains("Session") {
            let via = match summary.via {
                nexus_workbench::event::Via::Subscription => crate::pages::subscriptions::plan_payer(summary.agent),
                _ => "Nexus Gateway · API".to_string(),
            };
            body = body
                .child(format!(
                    "Model · {}",
                    summary.model.as_deref().unwrap_or("Organization default")
                ))
                .child(format!("Who pays · {via}"))
                .child(format!(
                    "Effort · {}",
                    summary.effort.map_or("Default", |effort| effort.as_str())
                ))
                .child(format!(
                    "Permissions · {}",
                    permissions(summary.agent, summary.permission_mode)
                ))
                .child(
                    div()
                        .text_color(p.faint)
                        .child(if summary.via == nexus_workbench::event::Via::Subscription {
                            if summary.agent.configured_provider().is_some() {
                                "Model calls use the agent's configured provider."
                            } else {
                                "Your plan pays for model calls; this is the agent's estimate."
                            }
                        } else {
                            "Reported by the agent. Gateway billing is in Usage."
                        }),
                )
                .child(
                    div()
                        .id("agents-connection")
                        .cursor_pointer()
                        .text_color(p.strong)
                        .child("Connection details ↗")
                        .on_click(cx.listener(|this, _, window, cx| this.show_connection(window, cx))),
                );
        }
        body
    }

    fn agent_fact_heading(
        &self,
        label: &'static str,
        value: String,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let open = self
            .side
            .agents
            .as_ref()
            .is_some_and(|state| state.facts_open.contains(label));
        div()
            .id(label)
            .mt_2()
            .py_1()
            .text_size(px(12.))
            .text_color(p.muted)
            .cursor_pointer()
            .child(
                h_flex()
                    .gap_2()
                    .child(if open { "⌄" } else { "›" })
                    .child(label)
                    .child(div().flex_1().min_w(px(0.)).truncate().text_color(p.faint).child(value)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(state) = &mut this.side.agents
                    && !state.facts_open.remove(label)
                {
                    state.facts_open.insert(label);
                }
                cx.notify();
            }))
    }
}
