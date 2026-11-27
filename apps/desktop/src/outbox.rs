//! One line of sends per session: a message goes to the core only after the one sent before it in the same session
//! was taken or refused, so a message with files to upload is never overtaken by a later one. Each send task gets
//! its place when the person presses Enter, on the UI thread, before it is spawned.

use futures::channel::oneshot;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// The last place taken in each session's line: it is done once its sender is dropped.
static LINES: LazyLock<Mutex<HashMap<String, oneshot::Receiver<()>>>> = LazyLock::new(Default::default);

/// A send's place in its session's line. The next send waits until this one is dropped.
pub struct Place {
    before: Option<oneshot::Receiver<()>>,
    _done: oneshot::Sender<()>,
}

/// Takes the next place in `thread_id`'s line. Call it in the order the person sent the messages.
pub fn join(thread_id: &str) -> Place {
    let (done, mine) = oneshot::channel();
    let before = LINES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(thread_id.to_string(), mine);
    Place { before, _done: done }
}

impl Place {
    /// Waits for the send before this one to finish, whether it went through or failed.
    pub async fn wait(&mut self) {
        if let Some(before) = self.before.take() {
            let _ = before.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn sends_leave_in_the_order_they_joined_even_when_the_first_is_slow() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .build()
            .expect("runtime");
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for (label, delay) in [("files", 60u64), ("text", 0), ("more", 0)] {
            let mut place = join("outbox-order-test");
            let order = order.clone();
            tasks.push(runtime.spawn(async move {
                place.wait().await;
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                order.lock().expect("order").push(label);
            }));
        }
        runtime.block_on(async {
            for task in tasks {
                task.await.expect("task");
            }
        });
        assert_eq!(*order.lock().expect("order"), ["files", "text", "more"]);
    }

    #[test]
    fn each_session_has_its_own_line() {
        let mut first = join("outbox-own-a");
        let _held = join("outbox-own-a");
        let mut other = join("outbox-own-b");
        // Nothing before either first place: neither waits.
        futures::executor::block_on(async {
            first.wait().await;
            other.wait().await;
        });
    }
}
