//! Command permissions in the session's tray (`permission_queue.rs`): its header, request row, exact request,
//! reason field, footer, and actions.
use super::*;
use crate::pages::ui;
use crate::palette::Palette;
use crate::workspace::code_dock;
use crate::workspace::permission_queue::{self as queue, tool_request};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use nexus_workbench::event::Decision;

impl SideChat {
    fn decide_permission(
        &mut self,
        request: String,
        decision: Decision,
        suggestion: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_command() || self.request.as_deref() != Some(request.as_str()) {
            return;
        }
        let message = self.permission_reason.read(cx).value().trim().to_string();
        self.command(
            "wb_approve",
            json!({"threadId":self.summary.id,"requestId":request,"decision":decision,"suggestion":suggestion,
            "message":(!message.is_empty()).then_some(message)}),
            false,
            None,
            window,
            cx,
        );
    }

    pub(super) fn render_tool_permission(&self, block: &Block, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Block::Approval {
            request_id,
            tool,
            summary,
            input,
            description,
            suggestions,
            ..
        } = block
        else {
            return div().into_any_element();
        };
        let pending: Vec<&Block> = self
            .blocks
            .iter()
            .filter(|block| tool_request(block).is_some())
            .collect();
        let shown = !self.permissions_collapsed;
        let toggle = ui::focusable(h_flex().id("side-permission-toggle"), p).on_click(cx.listener(|this, _, _, cx| {
            this.permissions_collapsed = !this.permissions_collapsed;
            cx.notify();
        }));
        let header = queue::needs_header(toggle, shown, queue::needs_label(&pending), None, p);
        // "Sending…" while answered; the actions wait for a usable, ready side chat.
        let busy = self.sending;
        let enabled = self.can_command();
        let rules = code_dock::describe_all(suggestions);
        let mut row = v_flex()
            .w_full()
            .gap(px(6.))
            .child(queue::request_head(request_id, tool, summary, None, true, busy, p))
            .children(queue::directory(input).map(|cwd| queue::request_folder(request_id, cwd.to_string(), p)))
            .children(
                description
                    .clone()
                    .map(|reason| queue::request_reason(request_id, reason, p)),
            )
            .child(queue::exact_request(request_id, queue::payload(tool, input), p))
            .child(code_dock::reason_field(&self.permission_reason, p));
        if self.remember_open {
            for rule in &rules {
                let request = request_id.clone();
                let index = rule.index;
                let title = match &rule.code {
                    Some(code) => format!("{} {code}", rule.title),
                    None => rule.title.clone(),
                };
                row = row.child(queue::remember_rule(
                    title,
                    rule.scope,
                    code_dock::answer_button(
                        SharedString::from(format!("side-rule-{index}")),
                        "Allow and remember",
                        false,
                        enabled,
                        p,
                    )
                    .rounded_full()
                    .when(enabled, |button| {
                        button.on_click(cx.listener(move |this, _, window, cx| {
                            this.decide_permission(request.clone(), Decision::AllowSession, Some(index), window, cx)
                        }))
                    }),
                    p,
                ));
            }
        }
        let (allow, deny) = (request_id.clone(), request_id.clone());
        let actions = queue::request_actions()
            .child(
                ui::permission_button("side-deny", "Deny", false, enabled, p)
                    .debug_selector(|| "side-deny".into())
                    .when(enabled, |button| {
                        button.on_click(cx.listener(move |this, _, window, cx| {
                            this.decide_permission(deny.clone(), Decision::Deny, None, window, cx)
                        }))
                    }),
            )
            .when(!rules.is_empty(), |actions| {
                actions.child(
                    ui::permission_button("side-remember", "Remember…", false, enabled, p)
                        .debug_selector(|| "side-remember".into())
                        .when(enabled, |button| {
                            button.on_click(cx.listener(|this, _, _, cx| {
                                this.remember_open = !this.remember_open;
                                cx.notify();
                            }))
                        }),
                )
            })
            .child(
                ui::permission_button("side-allow", "Allow once", true, enabled, p)
                    .debug_selector(|| "side-allow".into())
                    .when(enabled, |button| {
                        button.on_click(cx.listener(move |this, _, window, cx| {
                            this.decide_permission(allow.clone(), Decision::Allow, None, window, cx)
                        }))
                    }),
            );
        let body = v_flex()
            .id("side-permission-body")
            .w_full()
            .min_h(px(0.))
            .max_h(px(PERMISSION_LIMIT - 112.))
            .overflow_y_scroll()
            .child(row);
        div()
            .w_full()
            .debug_selector(|| "side-permission-card".into())
            .child(crate::command_tray::frame(
                "side-permission-tray",
                PERMISSION_LIMIT,
                header.into_any_element(),
                shown.then(|| body.into_any_element()),
                shown.then(|| queue::request_footer(summary.clone(), actions, p).into_any_element()),
                p,
            ))
            .into_any_element()
    }
}

/// The tray's height in the side pane, which has no shared command budget to split.
const PERMISSION_LIMIT: f32 = 420.;
