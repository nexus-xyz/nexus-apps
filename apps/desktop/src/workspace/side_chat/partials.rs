//! Restored chats read current full partials from their original owner, never the global bare-delta cache.
use super::{Context, Duration, SideChat, Window, json};
use nexus_workbench::stream_snapshot::Snapshot;

impl SideChat {
    pub(super) fn suspend_partials(&mut self) {
        self.partial_epoch = self.partial_epoch.wrapping_add(1);
        self.partial_poll = None;
        self.partial_revision = None;
        if !self.allow_partial_stream {
            self.streamed = None;
        }
    }
    pub(super) fn start_partials(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !window.is_visible() || !self.summary.status.busy() {
            self.suspend_partials();
            return;
        }

        if self.allow_partial_stream
            || !self.ready
            || self.retired
            || self.sending
            || self.pending.occupied()
            || !self.pending.valid()
            || self.poll.is_none()
            || self.partial_poll.is_some()
        {
            return;
        }
        let epoch = self.partial_epoch;
        let source = self.runs.source();
        self.partial_poll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let read = this.update_in(cx, |this, window, _| {
                    if !window.is_visible()
                        || this.retired
                        || !this.pending.valid()
                        || this.partial_epoch != epoch
                        || this.runs.source() != source
                        || !this.ready
                        || this.sending
                        || this.pending.occupied()
                        || !this.summary.status.busy()
                    {
                        return None;
                    }
                    Some(this.runs.call(
                        "wb_thread_stream",
                        json!({"threadId":this.summary.id,"afterSeq":this.events.last().map_or(0,|event|event.seq)}),
                    ))
                });
                let Ok(Some(read)) = read else { break };
                let response = read
                    .await
                    .unwrap_or_else(|_| Err("The partial answer read was lost.".into()));
                let mut delay = Duration::from_millis(500);
                if this
                    .update_in(cx, |this, window, cx| {
                        if !window.is_visible() {
                            this.streamed = None;
                            return false;
                        }
                        if this.retired
                            || !this.pending.valid()
                            || this.partial_epoch != epoch
                            || this.runs.source() != source
                            || !this.ready
                            || this.sending
                            || this.pending.occupied()
                            || !this.summary.status.busy()
                        {
                            return false;
                        }
                        if !this.summary.status.busy() {
                            delay = Duration::from_secs(2);
                        }
                        match response {
                            Ok(value) => match serde_json::from_value::<Snapshot>(value) {
                                Ok(snapshot) => this.apply_partial(snapshot, window, cx),
                                Err(_) => {
                                    this.streamed = None;
                                    delay = Duration::from_secs(2);
                                    cx.notify();
                                }
                            },
                            Err(error) => {
                                this.streamed = None;
                                // Exact shared legacy API refusal; auth/source/transport errors are not capabilities.
                                delay = if error == "Unknown command wb_thread_stream." {
                                    Duration::from_secs(30)
                                } else {
                                    Duration::from_secs(2)
                                };
                                cx.notify();
                            }
                        }
                        true
                    })
                    .unwrap_or(false)
                    == false
                {
                    break;
                }
                cx.background_executor().timer(delay).await;
            }
            let _ = this.update(cx, |this, _| {
                if this.partial_epoch == epoch {
                    this.partial_poll = None;
                }
            });
        }));
    }
    pub(super) fn apply_partial(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        if self.retired || !self.pending.valid() || !self.ready || !self.summary.status.busy() {
            return;
        }
        let last = self.events.last().map_or(0, |event| event.seq);
        if snapshot.thread_id != self.summary.id
            || snapshot.summary.id != self.summary.id
            || !snapshot.summary.side_chat
            || snapshot.summary.source_thread_id != self.summary.source_thread_id
            || snapshot.through_seq < last
            || snapshot
                .events
                .iter()
                .any(|event| event.thread_id != self.summary.id || event.seq > snapshot.through_seq)
        {
            return;
        }
        if let Some(incarnation) = &snapshot.incarnation {
            if self
                .partial_revision
                .as_ref()
                .is_some_and(|(old, revision)| old == incarnation && *revision > snapshot.revision)
            {
                return;
            }
            self.partial_revision = Some((incarnation.clone(), snapshot.revision));
        } else if !snapshot.messages.is_empty() {
            return;
        }
        // The same response carries completion rows and logical text from one owner cut.
        // Retain fresher overview/status; the snapshot's summary proves the parent binding only.
        let summary = self.summary.clone();
        self.apply_read(snapshot.events, vec![summary], window, cx);
        let text = snapshot
            .messages
            .into_iter()
            .filter(|message| message.parent.is_none())
            .map(|message| message.text)
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        self.set_stream((!text.is_empty()).then_some(text), cx);
        cx.notify();
    }
}
