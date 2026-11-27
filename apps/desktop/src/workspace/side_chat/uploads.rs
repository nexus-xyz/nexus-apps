//! A side chat's large files while they go up in pieces, as the session composer shows them (`workspace/uploads.rs`):
//! each such file's chip shows how much has gone, and its ✕ cancels the send. The pieces and the counting are
//! `nexus_workbench::attachment_upload`'s. A cancel sends nothing, and the text and every file stay in the composer.
use super::*;
use nexus_workbench::attachment_upload::UploadRun;
use std::sync::Arc;

/// How often the chips read the upload's progress while it runs.
const POLL: Duration = Duration::from_millis(100);

impl SideChat {
    /// Give the draft an upload its chips follow when a file goes in pieces, before the send takes its copy.
    pub(super) fn begin_upload(&mut self) {
        self.draft.upload = self.draft.sends_in_pieces().then(|| Arc::new(UploadRun::default()));
    }

    /// Redraw the chips while the draft's upload runs. A restored side chat calls it again for a send still running.
    pub(super) fn watch_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.draft.upload.clone().filter(|run| run.outcome().is_none()) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let running = this.update(cx, |this, cx| {
                    let ours = this.draft.upload.as_ref().is_some_and(|up| Arc::ptr_eq(up, &run));
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
        if let Some(run) = &self.draft.upload {
            run.cancel(id);
            cx.notify();
        }
    }
}
