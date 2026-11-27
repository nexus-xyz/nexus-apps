//! Confirm an in-place Claude Code/Codex handoff on the session's original runner.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::agent_switch::AgentSwitch;
use nexus_workbench::driver::label;
use nexus_workbench::event::{Effort, Via};
use serde_json::json;

use super::new_code::LaunchRow;
use super::runs::{Runs, Source};
use super::{View, Workspace};
use crate::palette::Palette;

pub(super) struct PendingSwitch {
    pub target: AgentSwitch,
    source: Source,
    pub(super) runs: Runs,
    pub busy: bool,
    pub skip_notice: bool,
    error: Option<String>,
    context_tokens: Option<u64>,
}

impl Workspace {
    pub(super) fn request_agent_switch(&mut self, row: LaunchRow, effort: Option<Effort>, cx: &mut Context<Self>) {
        let Some(session) = self.code_session() else { return };
        if session.agent == row.agent || !self.launch_rows().contains(&row) {
            return;
        }
        let Some(runs) = self.runs(&session.id) else { return };
        let pending = PendingSwitch {
            target: AgentSwitch {
                expected_agent: session.agent,
                agent: row.agent,
                model: row.model,
                effort,
                via: session.via,
                account: None,
            },
            source: runs.source(),
            runs,
            busy: false,
            skip_notice: self.code_form.skip_agent_switch_notice(),
            error: None,
            context_tokens: session.context.map(|(tokens, _)| tokens),
        };
        let skip = pending.skip_notice;
        self.close_dock_menu(cx);
        if let View::Code(view) = &mut self.view {
            if view.agent_switch.is_some() {
                return;
            }
            view.agent_switch = Some(pending);
        }
        if skip {
            self.confirm_agent_switch(cx);
        }
        cx.notify();
    }

    pub(super) fn cancel_agent_switch(&mut self, cx: &mut Context<Self>) {
        if let View::Code(view) = &mut self.view
            && view.agent_switch.as_ref().is_some_and(|switch| !switch.busy)
        {
            view.agent_switch = None;
        }
        cx.notify();
    }

    pub(super) fn confirm_agent_switch(&mut self, cx: &mut Context<Self>) {
        let View::Code(view) = &self.view else { return };
        let Some(switch) = &view.agent_switch else { return };
        if switch.busy {
            return;
        }
        let id = view.id.clone();
        let source = switch.source.clone();
        let target = switch.target.clone();
        let command = switch.runs.clone();
        let skip = switch.skip_notice;
        if self.runs(&id).is_none_or(|runs| runs.source() != source) {
            return;
        }
        if self.code_session().is_none_or(|s| s.agent != target.expected_agent) {
            self.cancel_agent_switch(cx);
            return;
        }
        if let View::Code(view) = &mut self.view
            && let Some(switch) = &mut view.agent_switch
        {
            switch.busy = true;
            switch.error = None;
        }
        let receive = command.call(
            "wb_thread_switch_agent",
            json!({
                "threadId": id,
                "expectedAgent": target.expected_agent,
                "agent": target.agent,
                "model": target.model,
                "effort": target.effort,
                "via": target.via,
                "account": target.account,
            }),
        );
        cx.spawn(async move |this, cx| {
            let answer = receive.await;
            let lost = answer.is_err();
            let result = answer
                .unwrap_or_else(|_| Err("The switch's answer was lost. Refresh this session before retrying.".into()));
            let _ = this.update(cx, |this, cx| {
                if this.runs(&id).is_none_or(|runs| runs.source() != source) {
                    return;
                }
                let View::Code(view) = &mut this.view else { return };
                if view.id != id {
                    return;
                }
                let Some(switch) = &mut view.agent_switch else { return };
                match result {
                    Ok(_) => {
                        view.agent_switch = None;
                        view.commands = None;
                        this.code_form.remember_agent_switch_notice(skip);
                        this.refresh_code(cx);
                    }
                    Err(error) => {
                        switch.busy = false;
                        switch.error = Some(error);
                        // A lost reply can follow a committed switch. Reconcile before another choice.
                        if lost {
                            this.refresh_code(cx);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_agent_switch(
        &self,
        switch: &PendingSwitch,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = &switch.target;
        let heading = format!("Continue with {}?", label(target.agent));
        let payer = match target.via {
            Via::Subscription => crate::pages::subscriptions::plan_payer(target.agent),
            _ => "Your organization's budget".to_string(),
        };
        let context = switch.context_tokens.map_or_else(
            || "The new agent reads the handoff again. The old cache does not transfer, so its first turn uses extra input tokens.".to_string(),
            |tokens| format!("Current context: approximately {tokens} tokens. The new agent reads the handoff again; the old cache does not transfer."),
        );
        let mut card = v_flex().id("agent-switch-confirmation").debug_selector(|| "agent-switch-confirmation".into()).w_full().p(px(16.)).gap(px(10.))
            .rounded(px(16.)).bg(palette.raised).border_1().border_color(palette.border).text_sm()
            .child(div().font_weight(FontWeight::MEDIUM).child(heading))
            .child("Share this session's original request and recent text messages with the new provider. Your files and conversation history stay here.")
            .child(div().text_color(palette.muted).child(context))
            .child("The current agent and its sub-agents stop. Your next message continues with the new agent.");
        if !switch.busy {
            card = card.child(
                h_flex().gap(px(8.)).child("Who pays:").children(
                    [(Via::Subscription, "My plan"), (Via::Nexus, "Organization")]
                        .into_iter()
                        .map(|(via, text)| {
                            div()
                                .id(SharedString::from(format!("agent-switch-payer-{}", via.as_str())))
                                .px(px(8.))
                                .py(px(4.))
                                .rounded(px(6.))
                                .cursor_pointer()
                                .when(via == target.via, |button| button.bg(palette.selected))
                                .child(text)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let View::Code(view) = &mut this.view
                                        && let Some(switch) = &mut view.agent_switch
                                        && !switch.busy
                                    {
                                        switch.target.via = via;
                                    }
                                    cx.notify();
                                }))
                        }),
                ),
            );
            card = card.child(
                div()
                    .id("agent-switch-skip-notice")
                    .cursor_pointer()
                    .text_color(palette.muted)
                    .child(format!(
                        "{} Don't ask again in this organization",
                        if switch.skip_notice { "☑" } else { "☐" }
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let View::Code(view) = &mut this.view
                            && let Some(switch) = &mut view.agent_switch
                            && !switch.busy
                        {
                            switch.skip_notice = !switch.skip_notice;
                        }
                        cx.notify();
                    })),
            );
        }
        card = card.child(
            div()
                .text_color(palette.muted)
                .child(format!("Pays for the new agent: {payer}")),
        );
        if let Some(error) = &switch.error {
            card = card.child(div().text_color(palette.destructive).child(error.clone()));
        }
        if switch.busy {
            card.child("Stopping the old agent and preparing the handoff…")
                .into_any_element()
        } else {
            card.child(
                h_flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        crate::pages::ui::button("agent-switch-cancel", "Cancel", palette)
                            .debug_selector(|| "agent-switch-cancel".into())
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_agent_switch(cx))),
                    )
                    .child(
                        crate::pages::ui::primary_button("agent-switch-confirm", "Switch", palette)
                            .debug_selector(|| "agent-switch-confirm".into())
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_agent_switch(cx))),
                    ),
            )
            .into_any_element()
        }
    }
}
