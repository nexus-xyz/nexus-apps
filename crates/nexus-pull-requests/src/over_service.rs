//! The watch's sessions while the desktop app's background service runs them (nexus-host's `local` mode): it reads
//! them from the store the service holds, as the windows do, and settles, wakes and tells a session through the
//! service's commands. The desktop app gives it to its [`crate::watch::Shell`], and `request_secret` reads the
//! sessions with it.

use std::path::PathBuf;
use std::sync::Arc;

use nexus_remote::machine::Remote;
use nexus_workbench::event::ThreadSummary;
use nexus_workbench::store::Store;
use serde_json::{Value, json};

use crate::watch::Changed;

/// The background service that runs the sessions, and the scope (`<gateway>|<org>`) they are read under.
#[derive(Clone)]
pub struct OverService {
    pub remote: Remote,
    /// The data folder that holds the store.
    pub data: PathBuf,
    pub scope: Arc<dyn Fn() -> String + Send + Sync>,
}

impl OverService {
    fn store(&self) -> Result<Store, String> {
        nexus_workbench::reader::open_in(&self.data)?.ok_or_else(|| "No Code sessions on this computer yet.".into())
    }

    /// The sessions as the windows read them from the store.
    pub fn threads(&self) -> Result<Vec<ThreadSummary>, String> {
        nexus_workbench::reader::summaries_in(&self.data, &(self.scope)())
    }

    pub fn folder(&self, thread_id: &str) -> Result<PathBuf, String> {
        nexus_workbench::reader::folder(&self.store()?, thread_id).map(|folder| folder.path)
    }

    pub fn settle_merged(&self, thread_id: &str, number: u64, archive: bool) -> Changed {
        self.relay(
            "wb_settle_merged",
            json!({ "threadId": thread_id, "number": number, "archive": archive }),
        )
    }

    /// Only a session snoozed now wakes: `wb_thread_update` with `snoozedUntil: 0`.
    pub fn wake(&self, thread_id: &str) -> Changed {
        let now = nexus_workbench::now_ms();
        let snoozed = self
            .threads()
            .ok()
            .and_then(|threads| threads.into_iter().find(|thread| thread.id == thread_id))
            .is_some_and(|thread| thread.aside.snoozed_until.is_some_and(|until| until > now));
        if !snoozed {
            return Box::pin(async { Ok(()) });
        }
        self.relay("wb_thread_update", json!({ "threadId": thread_id, "snoozedUntil": 0 }))
    }

    /// The service runs the hooks: its store claims them.
    pub fn check_failed(&self, thread_id: &str, name: &str, url: Option<&str>) -> Changed {
        self.relay(
            "wb_check_failed",
            json!({ "threadId": thread_id, "name": name, "url": url }),
        )
    }

    /// The service's store saves the link: its sessions are its own.
    pub fn link_pull_request(&self, thread_id: &str, url: &str) -> Changed {
        self.relay("wb_link_pull_request", json!({ "threadId": thread_id, "url": url }))
    }

    pub fn queue(&self, thread_id: &str, text: &str) -> Changed {
        self.relay("wb_send", json!({ "threadId": thread_id, "text": text, "images": [] }))
    }

    /// A command on the service that answers nothing the watch needs.
    fn relay(&self, call: &'static str, args: Value) -> Changed {
        let remote = self.remote.clone();
        Box::pin(async move { remote.call(call, args).await.map(drop) })
    }
}
