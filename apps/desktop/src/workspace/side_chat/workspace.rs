use super::*;

impl Workspace {
    pub(in crate::workspace) fn open_side_chat(&self) -> Option<Entity<SideChat>> {
        let View::Code(view) = &self.view else {
            return None;
        };
        self.side
            .side_chat_for
            .get(&view.id)
            .and_then(|id| self.side.side_chats.get(id))
            .map(|(chat, _)| chat.clone())
    }

    pub(in crate::workspace) fn waiting_side_chat(&self) -> Option<&super::retention::RetainedChat> {
        let View::Code(view) = &self.view else { return None };
        self.side
            .side_chat_for
            .get(&view.id)
            .and_then(|id| self.side.side_chat_waiting.get(id))
    }
    pub(in crate::workspace) fn sync_side_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.adopt_side_chats(window, cx);
        // Opening it reads it: its entry stops saying "Finished, unread" now, not at the sessions' next read.
        self.note_side_chat_seen(cx);
        let current = match &self.view {
            View::Code(view) if self.side.open && self.side.side_chat_visible => {
                self.side.side_chat_for.get(&view.id).map(String::as_str)
            }
            _ => None,
        };
        for (id, (chat, _)) in &self.side.side_chats {
            chat.update(cx, |chat, cx| {
                chat.gateway_models(&self.models);
                chat.active(current == Some(id.as_str()), window, cx)
            });
        }
    }

    pub(in crate::workspace) fn show_side_chat(
        &mut self,
        side: ThreadSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let View::Code(view) = &self.view else {
            return;
        };
        if side.source_thread_id.as_deref() != Some(view.id.as_str()) || !side.side_chat {
            return;
        }
        self.side.connection = None;
        let parent = view.id.clone();
        let Some(runs) = self.side_chat_runs(&parent) else {
            return;
        };
        if let Some((old, _)) = self.side.side_chats.get(&side.id)
            && old.read(cx).summary.id == side.id
        {
            self.side.side_chat_for.insert(parent, side.id.clone());
            self.side.side_chat_visible = true;
            self.side.open = true;
            old.update(cx, |chat, cx| {
                chat.active(true, window, cx);
                chat.input.focus_handle(cx).focus(window, cx);
            });
            cx.notify();
            return;
        }
        let side_id = side.id.clone();
        let parent_title = self
            .code_session()
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "Parent session".into());
        let chat = cx.new(|cx| SideChat::new(side, parent_title, runs, window, cx));
        self.bind_side_chat(parent.clone(), side_id.clone(), chat.clone(), window, cx);
        self.side.side_chat_for.insert(parent, side_id.clone());
        self.side.side_chat_visible = true;
        self.side.open = true;
        chat.update(cx, |chat, cx| {
            chat.active(true, window, cx);
            chat.input.update(cx, |i, cx| i.focus(window, cx));
        });
        cx.notify();
    }
    pub(super) fn bind_side_chat(
        &mut self,
        parent: String,
        side_id: String,
        chat: Entity<SideChat>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let guard = self.side_chat_guard();
        chat.update(cx, |chat, _| chat.authority(guard));
        let closing = side_id.clone();
        let subscription = cx.subscribe_in(&chat, window, move |this, _, event: &SideEvent, window, cx| {
            let SideEvent::Closed(deleted) = event;
            if let View::Code(view) = &this.view
                && this.side.side_chat_for.get(&view.id) == Some(&closing)
            {
                this.side.side_chat_visible = false;
                this.side.side_chat_for.remove(&view.id);
                this.restore_agents_after_side_chat(window, cx);
            }
            this.side.side_chat_for.retain(|_, side| side != &closing);
            if *deleted {
                if let Some((chat, _)) = this.side.side_chats.remove(&closing) {
                    chat.update(cx, |chat, _| chat.retire(true));
                }
            }
            cx.notify();
        });

        self.side.side_chat_for.insert(parent, side_id.clone());
        self.side.side_chats.insert(side_id, (chat, subscription));
    }
}
