//! `request_secret` in this build: the requests are
//! `nexus_desktop_mcp::secret_request`'s, and [`GpuiSecrets`] is their `Shell` over this build's runner.
//!
//! An agent's call reaches [`GpuiSecrets::answer`] over the app's socket (`app_socket.rs`), or from the background
//! service that runs the sessions (`code_run.rs`). The open request shows as a
//! private card in its session's dock (`workspace/secret_card.rs`), and a notification says so (`notify.rs`). The value
//! goes from the card's password field to [`GpuiSecrets::settle`], which writes it to a 0600 file in a 0700 folder
//! under this build's cache folder. The agent gets only the file's path. Decline ends the request with no file.
//!
//! The runner makes one when it takes the store, and its folder is emptied then: a file an earlier run left goes. A
//! run's files go when it ends, 15 minutes after they are written, or with the runner.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use nexus_desktop_core::notify::Notifier;
use nexus_desktop_mcp::secret_request::{self, SecretRequest, Secrets, Shell};
use nexus_workbench::Workbench;
use nexus_workbench::event::ThreadSummary;
use serde_json::Value;

use crate::notify::GpuiNotifier;

struct Held {
    secrets: Secrets,
    workbench: Workbench,
    runtime: tokio::runtime::Handle,
    /// The folder the files go in: the cache folder's `secret-files`.
    base: PathBuf,
    /// The open requests changed: the windows draw their cards again.
    changed: Arc<dyn Fn() + Send + Sync>,
    /// The sessions, while the background service runs them: read from the store. None: the workbench's.
    threads: Option<Threads>,
}

/// This computer's sessions, read from the store the background service holds.
pub type Threads = Arc<dyn Fn() -> Result<Vec<ThreadSummary>, String> + Send + Sync>;

/// The runner's requests: the `Shell` the shared requests run on.
#[derive(Clone)]
pub struct GpuiSecrets(Arc<Held>);

impl GpuiSecrets {
    /// The requests of the runner over `workbench`, their files in `base`. Deletes what an earlier run left there.
    pub fn new(
        workbench: Workbench,
        runtime: tokio::runtime::Handle,
        base: PathBuf,
        changed: Arc<dyn Fn() + Send + Sync>,
    ) -> GpuiSecrets {
        GpuiSecrets::reading(workbench, runtime, base, changed, None)
    }

    /// [`GpuiSecrets::new`], with the sessions read by `threads` when the background service runs them.
    pub fn reading(
        workbench: Workbench,
        runtime: tokio::runtime::Handle,
        base: PathBuf,
        changed: Arc<dyn Fn() + Send + Sync>,
        threads: Option<Threads>,
    ) -> GpuiSecrets {
        let _ = std::fs::remove_dir_all(&base);
        GpuiSecrets(Arc::new(Held {
            secrets: Secrets::default(),
            workbench,
            runtime,
            base,
            changed,
            threads,
        }))
    }

    /// The open request of the session `thread`: one at most.
    pub fn of(&self, thread: &str) -> Option<SecretRequest> {
        self.0
            .secrets
            .open()
            .into_iter()
            .find(|request| request.thread_id == thread)
    }

    /// The person's answer to the card `id`: the value to provide it, None to decline. The error is for the card, and
    /// never holds the value.
    pub fn settle(&self, id: &str, value: Option<&str>) -> Result<(), String> {
        let settled = self.0.secrets.settle(&self.0.base, id, value);
        secret_request::emit(self);
        settled
    }

    /// Answers a relayed `request_secret` call of the session `thread`'s run `run`: waits for the card's answer.
    pub async fn answer(&self, thread: &str, run: &str, message: &Value) -> Value {
        secret_request::answer(self, thread, run, message).await
    }

    /// The run `run` ended: its open card closes, and its files go.
    pub fn end_run(&self, run: &str) {
        secret_request::end_run(self, run);
    }
}

impl Notifier for GpuiSecrets {
    fn looking_at(&self, thread_id: &str) -> bool {
        GpuiNotifier.looking_at(thread_id)
    }

    fn show(&self, machine: Option<&str>, thread_id: &str, title: &str, body: &str) {
        GpuiNotifier.show(machine, thread_id, title, body);
    }
}

impl Shell for GpuiSecrets {
    fn secrets(&self) -> Option<&Secrets> {
        Some(&self.0.secrets)
    }

    fn emit(&self, _open: &[SecretRequest]) {
        (self.0.changed)();
    }

    fn workbench(&self) -> Option<Workbench> {
        Some(self.0.workbench.clone())
    }

    /// This computer's sessions, and the servers': a server's agent asks for a secret in this window too.
    fn threads(&self) -> Option<Vec<ThreadSummary>> {
        let mut threads = match &self.0.threads {
            Some(threads) => threads().ok(),
            None => self.0.workbench.threads().ok(),
        }?;
        threads.extend(crate::machines::get().server_threads());
        Some(threads)
    }

    fn spawn(&self, task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) {
        self.0.runtime.spawn(task);
    }
}
