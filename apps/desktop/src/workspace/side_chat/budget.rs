//! A used-up budget holds a side chat's send, as it holds the session's (`limit_line.rs`): the composer keeps the
//! message, the budget's line shows over it, and the message goes once the budget is raised or the payer changes. The
//! window reads the budget; it hands each side chat the line for that side chat's own payer.
use super::*;
use nexus_workbench::event::Via;

impl SideChat {
    /// The used-up budget's line for this side chat's payer, or None: a send held for it goes now.
    pub(in crate::workspace) fn set_budget(
        &mut self,
        line: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.budget == line {
            return;
        }
        self.budget = line;
        if self.budget.is_none()
            && let Some(queued) = self.held.take()
        {
            self.send(queued, window, cx);
        }
        cx.notify();
    }

    /// A send pressed while the budget is used up: the composer keeps it until the budget lifts. False when nothing
    /// blocks it.
    pub(super) fn hold_for_budget(&mut self, queued: bool, cx: &mut Context<Self>) -> bool {
        if self.budget.is_none() {
            return false;
        }
        self.held = Some(queued);
        cx.notify();
        true
    }

    /// The line over the composer: the budget's, and that a send waits for it.
    pub(super) fn budget_note(&self) -> Option<String> {
        let line = self.budget.as_ref()?;
        Some(if self.held.is_some() {
            format!("{line} Your message sends once it is raised.")
        } else {
            line.clone()
        })
    }
}

impl Workspace {
    /// Hand each side chat the budget's line for its payer. Deferred, because a lifted budget sends a held message.
    pub(in crate::workspace) fn push_side_budget(&mut self, cx: &mut Context<Self>) {
        let lines: Vec<_> = self
            .side
            .side_chats
            .values()
            .map(|(chat, _)| {
                let gateway = chat.read(cx).summary.via == Via::Nexus;
                (chat.clone(), self.budget_line_for(gateway))
            })
            .collect();
        if lines.is_empty() {
            return;
        }
        let window = self.window_handle;
        cx.defer(move |cx| {
            let pushed = window.update(cx, |_, window, cx| {
                for (chat, line) in lines {
                    chat.update(cx, |chat, cx| chat.set_budget(line, window, cx));
                }
            });
            if let Err(error) = pushed {
                eprintln!("nexus-desktop-gpui: side chat: budget: {error}");
            }
        });
    }
}
