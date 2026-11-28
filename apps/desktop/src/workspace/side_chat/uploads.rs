//! A side chat's large files while they go up in pieces, as the session composer shows them (`workspace/uploads.rs`):
//! the sent message's chips stay over the composer, each such file's chip shows how much has gone, and its ✕ cancels
//! the send. The pieces and the counting are `nexus_workbench::attachment_upload`'s. A cancel sends nothing, and the
//! text and every file go back to the composer (`commands.rs`, `unsent`).
use super::*;
use std::sync::Arc;

/// How often the chips read the upload's progress while it runs.
const POLL: Duration = Duration::from_millis(100);

impl SideChat {
    /// Redraw the chips while the sent message's upload runs. A restored side chat calls it again for a send still
    /// running.
    pub(super) fn watch_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self
            .uploading
            .as_ref()
            .and_then(|draft| draft.upload.clone())
            .filter(|run| run.outcome().is_none())
        else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let running = this.update(cx, |this, cx| {
                    let ours = this
                        .uploading
                        .as_ref()
                        .and_then(|draft| draft.upload.as_ref())
                        .is_some_and(|up| Arc::ptr_eq(up, &run));
                    cx.notify();
                    !this.retired && ours && run.outcome().is_none()
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// The ✕ on a chip of a file going up in pieces.
    pub(super) fn cancel_upload(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(run) = self.uploading.as_ref().and_then(|draft| draft.upload.as_ref()) {
            run.cancel(id);
            cx.notify();
        }
    }
}
