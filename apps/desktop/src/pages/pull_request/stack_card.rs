//! The pull request page's stack card: when the page's pull request is in a stack, it
//! rebases the stack onto its trunk or merges it (`nexus_pull_requests::stack_actions`). A press reads the stack first,
//! then asks with what it read: which pull requests move, and which merge. The core refuses the action when the stack
//! moved after the person confirmed.

use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_pull_requests::pull_page::{PullError, PullPage};
use nexus_pull_requests::stack_actions::{Action, Stack};

use super::{PullRequestPage, card, confirm_box, model, ui};
use crate::palette::Palette;

/// The card's state. One action at a time: a read, then a confirmation, then the action.
#[derive(Default)]
pub(super) struct StackCard {
    /// The action whose read or write is running.
    busy: Option<Action>,
    confirming: Option<(Action, Stack)>,
    problem: Option<PullError>,
    done: Option<String>,
}

impl StackCard {
    /// Esc: drops the confirmation. Whether there was one.
    pub(super) fn cancel(&mut self) -> bool {
        self.confirming.take().is_some()
    }

    /// Drops the card's failure when `fixed` says a fix took it away.
    pub(super) fn forget(&mut self, fixed: impl Fn(&PullError) -> bool) {
        if self.problem.as_ref().is_some_and(fixed) {
            self.problem = None;
        }
    }
}

fn label(action: Action) -> &'static str {
    match action {
        Action::Rebase => "Rebase stack",
        Action::Merge => "Merge stack",
    }
}

impl PullRequestPage {
    /// The session's stack that holds the page's pull request, bottom first; None when it stands alone.
    fn stack_urls(&self) -> Option<Vec<String>> {
        let here = &self.page.as_ref()?.status.url;
        let pulls = self.session_pulls();
        nexus_pull_requests::stack::groups(&pulls)
            .into_iter()
            .find(|group| group.stacked() && group.rows.iter().any(|row| &row.pull.url == here))
            .map(|group| group.rows.iter().rev().map(|row| row.pull.url.clone()).collect())
    }

    /// Reads the stack as GitHub has it now, then asks.
    fn stack_ask(&mut self, action: Action, cx: &mut Context<Self>) {
        let Some(urls) = self.stack_urls() else { return };
        self.stack = StackCard {
            busy: Some(action),
            ..StackCard::default()
        };
        self.write(
            move |session| model::stack(&session, &urls),
            move |page, result, _| {
                page.stack.busy = None;
                match result {
                    Ok(stack) => page.stack.confirming = Some((action, stack)),
                    Err(problem) => page.stack.problem = Some(problem),
                }
            },
            cx,
        );
        cx.notify();
    }

    /// Runs what the person confirmed. A merge uses the merge card's method.
    fn stack_run(&mut self, cx: &mut Context<Self>) {
        let Some((action, stack)) = self.stack.confirming.clone() else {
            return;
        };
        let method = self.page.as_ref().and_then(|page| self.chosen_method(page));
        let Some(method) = method.or_else(|| (action == Action::Rebase).then(String::new)) else {
            return;
        };
        let (thread_id, trunk) = (self.thread_id.clone(), stack.trunk.clone());
        self.stack.busy = Some(action);
        self.stack.problem = None;
        self.write(
            move |session| model::stack_act(&session, &thread_id, &stack, action, &method),
            move |page, result, _| {
                page.stack.busy = None;
                page.stack.confirming = None;
                match result {
                    Ok(done) => page.stack.done = Some(model::stack_done_text(&done, &trunk)),
                    Err(problem) => page.stack.problem = Some(problem),
                }
            },
            cx,
        );
        cx.notify();
    }

    /// The card, for a pull request in a stack.
    pub(super) fn render_stack(&self, page: &PullPage, palette: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let count = self.stack_urls()?.len();
        let state = &self.stack;
        let method = self.chosen_method(page);
        let idle = state.busy.is_none() && state.confirming.is_none();
        let buttons = [
            ("pull-stack-rebase", Action::Rebase),
            ("pull-stack-merge", Action::Merge),
        ]
        .map(|(id, action)| {
            if idle && (action == Action::Rebase || method.is_some()) {
                ui::button(id, label(action), palette)
                    .on_click(cx.listener(move |page, _, _, cx| page.stack_ask(action, cx)))
            } else {
                ui::disabled_button(id, label(action), palette)
            }
        });
        let mut card = card("Stack", format!("{count} pull requests"), palette)
            .child(h_flex().mt(px(4.)).gap(px(8.)).children(buttons));
        if state.busy.is_some() && state.confirming.is_none() {
            card = card.child(ui::note("Reading the stack…", palette));
        }
        if let Some((action, stack)) = &state.confirming {
            let action = *action;
            let confirm = if state.busy.is_some() {
                ui::disabled_button("pull-stack-confirm", label(action), palette)
            } else {
                ui::primary_button("pull-stack-confirm", label(action), palette)
                    .on_click(cx.listener(|page, _, _, cx| page.stack_run(cx)))
            };
            // Not while it runs: the card would forget what it is waiting for.
            let cancel = if state.busy.is_some() {
                ui::disabled_button("pull-stack-cancel", "Cancel", palette)
            } else {
                ui::button("pull-stack-cancel", "Cancel", palette).on_click(cx.listener(|page, _, _, cx| {
                    page.stack.cancel();
                    cx.notify();
                }))
            };
            card = card.child(confirm_box(
                model::stack_confirm_text(stack, action, method.as_deref().unwrap_or("")),
                confirm,
                cancel,
                palette,
            ));
        }
        Some(
            card.children(state.done.clone().map(|done| ui::note(done, palette)))
                .children(
                    state
                        .problem
                        .as_ref()
                        .map(|problem| self.render_problem(problem, palette, cx)),
                ),
        )
    }
}
