//! Read-only session-sync reader. Keys and account checks stay in the shared core and store owner.
use super::{Answer, CodeRunner};
use serde_json::Value;

impl CodeRunner {
    pub fn synced_thread_page(&self, thread: String, before: Option<i64>, after: Option<i64>) -> Answer<Value> {
        self.ask(move |api| async move { api.synced_thread_page(thread, before, after, Some(100)).await })
    }
}
