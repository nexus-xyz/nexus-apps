//! A server's Nexus sign-in from the app's server screens ([`crate::probe::login`] on its way), one per machine: a new
//! one, or "Cancel", stops the one before. Both desktop shells keep one [`Logins`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::task::{AbortHandle, JoinHandle};

use crate::Result;
use crate::install::Installer;
use crate::probe::LoginStep;

/// The sign-ins on their way, by machine.
#[derive(Default)]
pub struct Logins(Mutex<HashMap<String, AbortHandle>>);

impl Logins {
    /// Stops machine `id`'s sign-in on its way (the dialog closed).
    pub fn cancel(&self, id: &str) {
        if let Some(handle) = self.0.lock().ok().and_then(|mut logins| logins.remove(id)) {
            handle.abort();
        }
    }

    /// Removes `id`'s entry, but only if it is still the task `task` (a login that was cancelled and retried
    /// must not clear its replacement's handle).
    fn finish(&self, id: &str, task: tokio::task::Id) {
        if let Ok(mut logins) = self.0.lock()
            && logins.get(id).is_some_and(|handle| handle.id() == task)
        {
            logins.remove(id);
        }
    }

    /// Starts `id`'s login task and registers it as one step under the lock, so a cancel (which takes the same
    /// lock) either runs before the spawn or finds the registered task; a login already running for `id` is
    /// stopped first. It does not clear the entry when the task ends: the caller calls `finish` for that.
    fn start<T: Send + 'static>(&self, id: &str, work: impl Future<Output = T> + Send + 'static) -> JoinHandle<T> {
        let Ok(mut logins) = self.0.lock() else {
            return tokio::spawn(work);
        };
        if let Some(old) = logins.remove(id) {
            old.abort();
        }
        let task = tokio::spawn(work);
        logins.insert(id.to_string(), task.abort_handle());
        task
    }

    /// Signs machine `id`'s host (on `installer`'s server) in to Nexus at `gateway` with its own device token: the
    /// code to approve goes to `code` (`link`, `code`); answers once approved (the organization, when named) and the
    /// host restarted. Its running sessions stop as when the app quits. "Cancelled." when [`Logins::cancel`] or a
    /// new sign-in stopped it first.
    pub async fn sign_in(
        &self,
        id: &str,
        installer: Installer,
        gateway: String,
        code: impl Fn(&str, &str) + Send + Sync + 'static,
    ) -> Result<Option<String>> {
        // Set once the host reported "Signed in": a stop after that point has still signed the server in.
        let signed_in = Arc::new(AtomicBool::new(false));
        let seen = signed_in.clone();
        let task = self.start(id, async move {
            let org = Mutex::new(None);
            crate::probe::login(&installer, Some(&gateway), |step| match step {
                LoginStep::Code { link, code: shown } => code(&link, &shown),
                LoginStep::SignedIn { org: named } => {
                    seen.store(true, Ordering::SeqCst);
                    if let Ok(mut org) = org.lock() {
                        *org = named;
                    }
                }
            })
            .await?;
            Ok::<_, String>(org.into_inner().ok().flatten())
        });
        let task_id = task.id();
        let answer = task.await;
        self.finish(id, task_id);
        answer.map_err(|_| {
            if signed_in.load(Ordering::SeqCst) {
                // Stopped after the host signed in but before its restart was confirmed: the server is signed in.
                "Signed in, but the restart was not confirmed. Check the server again.".to_string()
            } else {
                "Cancelled.".to_string()
            }
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_cancel_after_start_stops_the_login_and_a_restart_replaces_it() {
        let logins = Logins::default();
        let first = logins.start("m", std::future::pending::<()>());
        let second = logins.start("m", std::future::pending::<()>());
        assert!(first.await.is_err());
        logins.cancel("m");
        assert!(second.await.is_err());
        assert!(logins.0.lock().is_ok_and(|l| l.is_empty()));
    }

    #[tokio::test]
    async fn a_finished_login_leaves_a_retry_registered() {
        let logins = Logins::default();
        let a = logins.start("m", std::future::pending::<()>());
        let a_id = a.id();
        // Cancel A, retry as B, then A's cleanup runs late.
        logins.cancel("m");
        let b = logins.start("m", std::future::pending::<()>());
        assert!(a.await.is_err());
        logins.finish("m", a_id);
        assert!(logins.0.lock().is_ok_and(|l| l.contains_key("m")));
        // B's own cleanup clears it, and a cancel then has nothing to stop.
        logins.finish("m", b.id());
        assert!(logins.0.lock().is_ok_and(|l| l.is_empty()));
        logins.cancel("m");
        b.abort();
    }
}
