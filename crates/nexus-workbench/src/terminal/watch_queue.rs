//! The order in which watched exits are delivered.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use super::super::Execution;

/// Delivers one watched exit: the workbench's `deliver_terminal_watch`.
pub(crate) type Deliver = Arc<dyn Fn(Arc<Execution>) + Send + Sync>;

/// Watched exits wait here and are delivered one at a time, oldest first. A command joins the queue in the same hold
/// of its state lock that publishes its end. So a command whose end is seen later is never delivered before it, and a
/// notice that arrived proves that every earlier delivery attempt has finished.
pub(crate) struct ExitQueue {
    deliver: Deliver,
    pending: Mutex<VecDeque<Arc<Execution>>>,
    /// Held while a task delivers, so two tasks never deliver at once or out of order.
    turn: Mutex<()>,
}

impl ExitQueue {
    pub(crate) fn new(deliver: Deliver) -> Arc<Self> {
        Arc::new(Self {
            deliver,
            pending: Mutex::default(),
            turn: Mutex::default(),
        })
    }

    /// Queues `entry` and starts a task on `runtime` that delivers what is queued. Without a runtime, a plain thread
    /// delivers it, and the delivery writes a row instead of steering. Takes no execution's state lock, so a caller may
    /// hold one.
    pub(super) fn push(self: &Arc<Self>, entry: Arc<Execution>, runtime: Option<&tokio::runtime::Handle>) {
        self.pending.lock().unwrap_or_else(|p| p.into_inner()).push_back(entry);
        let queue = self.clone();
        match runtime {
            Some(runtime) => {
                runtime.spawn(async move { queue.drain() });
            }
            None => {
                std::thread::spawn(move || queue.drain());
            }
        }
    }

    /// Delivery takes no await, so the turn is never held across one. A delivery that panics is logged and skipped:
    /// the panic stops at `catch_unwind`, so it never poisons the turn and the later exits are still delivered.
    fn drain(&self) {
        let _turn = self.turn.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            let next = self.pending.lock().unwrap_or_else(|p| p.into_inner()).pop_front();
            let Some(entry) = next else {
                break;
            };
            let thread_id = entry.thread.clone();
            if catch_unwind(AssertUnwindSafe(|| (self.deliver)(entry))).is_err() {
                eprintln!("nexus-workbench: {thread_id}: a watched command's exit notice panicked and was skipped");
            }
        }
    }
}
