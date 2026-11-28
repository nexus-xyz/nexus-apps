//! Rewriting a queued message holds the session's queue: while the person edits one, a turn that ends starts no
//! queued message, so the old text never goes out mid-edit (`queue_hold.rs` in `nexus-workbench`). The hold lives as
//! long as the rewrite: Save sends the new text, which lets the queue go on; Cancel, leaving the session or closing
//! the window drops the hold, and the queue goes on as it was.

use super::runs::Runs;

/// The engine holds the queue while this lives, for the rewrite of queued message `seq`.
pub(super) struct QueueHold {
    runs: Runs,
    thread: String,
    seq: i64,
    /// The rewrite was saved: `queue_edit` let the queue go already.
    saved: bool,
}

impl QueueHold {
    pub(super) fn new(runs: Runs, thread: String, seq: i64) -> QueueHold {
        runs.queue_hold(thread.clone(), seq, true);
        QueueHold {
            runs,
            thread,
            seq,
            saved: false,
        }
    }

    pub(super) fn seq(&self) -> i64 {
        self.seq
    }

    /// Save the rewrite: the engine takes the new text and lets the queue go in one call. A second call to let go
    /// could race it and start the old text first.
    pub(super) fn save(mut self, text: String) {
        self.saved = true;
        self.runs.queue_edit(self.thread.clone(), self.seq, text);
    }

    /// The rewrite was saved another way (a side chat's own `wb_queue_edit`), which let the queue go already.
    pub(super) fn edited(mut self) {
        self.saved = true;
    }
}

impl Drop for QueueHold {
    fn drop(&mut self) {
        if !self.saved {
            self.runs.queue_hold(self.thread.clone(), self.seq, false);
        }
    }
}
