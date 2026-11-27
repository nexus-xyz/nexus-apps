//! Queued first tokens reach the projection before a draw already requested by other work.
use super::*;

impl Workspace {
    pub(in crate::workspace) fn await_first_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.first_token_armed || !self.waits_first_token() {
            return;
        }
        self.first_token_armed = true;
        let this = cx.weak_entity();
        window.before_next_native_draw(move |window, cx| {
            if let Err(error) = this.update(cx, |this, cx| {
                this.first_token_armed = false;
                if !this.waits_first_token() {
                    return;
                }
                if !this.chat_binding_current(cx) {
                    this.sync_gateway_chat(window, cx);
                    return;
                }
                if let Some((binding, key)) = this.chat_execution.clone().zip(this.chat_execution_at) {
                    binding.owner.update(cx, |owner, cx| owner.drain_first_token(key, cx));
                    this.sync_gateway_chat(window, cx);
                }
            }) {
                eprintln!("first-token projection is unavailable: {error:#}");
            }
        });
    }

    fn waits_first_token(&self) -> bool {
        matches!(self.view, super::super::View::Chat)
            && self.asking.is_some()
            && self
                .entries
                .last()
                .is_some_and(|entry| entry.streaming && entry.streamed.is_empty())
    }
}
