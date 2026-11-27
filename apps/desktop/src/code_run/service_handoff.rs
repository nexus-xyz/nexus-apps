//! A sign-out retires the old service before the shared runner can connect again.

use super::CodeRunner;
#[cfg(unix)]
use super::{CodeEvent, GpuiHost, SessionScope};
#[cfg(unix)]
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering::SeqCst},
};

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) enum State {
    #[default]
    Ready,
    Stopping,
    Failed(String),
    Released,
}

impl CodeRunner {
    pub(crate) fn service_handoff(&self) -> State {
        #[cfg(unix)]
        {
            self.service_handoff
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
        }
        #[cfg(not(unix))]
        State::Ready
    }

    pub(crate) fn stop_signed_in_service(&self) {
        #[cfg(unix)]
        {
            let (Some(remote), Some(local)) = (self.api.remote().cloned(), self.service.clone()) else {
                return;
            };
            {
                let mut state = self.service_handoff.lock().unwrap_or_else(|error| error.into_inner());
                if matches!(*state, State::Stopping | State::Released) {
                    return;
                }
                *state = State::Stopping;
                remote.hold();
            }
            let state = self.service_handoff.clone();
            let host = self.host.clone();
            self.runtime.spawn(async move {
                remote.wait_for_link().await;
                let result = local.stop_for_sign_out().await;
                {
                    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
                    *state = match result {
                        Ok(()) => State::Released,
                        Err(error) => State::Failed(error),
                    };
                }
                resume(&state, &host, &remote);
                if let Err(error) = host.events().unbounded_send(CodeEvent::Changed) {
                    eprintln!("nexus-desktop-gpui: could not report background service handoff: {error}");
                }
            });
        }
    }

    /// Refresh the service handshake when a credential arrives or leaves before its organization is known.
    pub(crate) fn refresh_local_service_account(&self) {
        #[cfg(unix)]
        if self.host.session_scope().is_local() {
            match self.service_handoff() {
                State::Released => self.resume_service_after_sign_out(),
                State::Ready => {
                    if let Some(remote) = self.api.remote().cloned() {
                        let generation = self.service_account_generation.clone();
                        let expected = generation.fetch_add(1, SeqCst).wrapping_add(1);
                        let state = self.service_handoff.clone();
                        remote.hold();
                        self.runtime.spawn(async move {
                            resume_after_account_change(&state, &generation, expected, &remote).await;
                        });
                    }
                }
                State::Stopping | State::Failed(_) => {}
            }
        }
    }

    #[cfg(unix)]
    pub(super) fn resume_service_after_sign_out(&self) {
        if let Some(remote) = self.api.remote() {
            resume(&self.service_handoff, &self.host, remote);
        }
    }
}

#[cfg(unix)]
async fn resume_after_account_change(
    state: &Mutex<State>,
    generation: &AtomicU64,
    expected: u64,
    remote: &nexus_remote::machine::Remote,
) {
    // An open may already have read the old hello. Let it discard that connection while held.
    remote.wait_for_link().await;
    let state = state.lock().unwrap_or_else(|error| error.into_inner());
    if generation.load(SeqCst) == expected && *state == State::Ready {
        remote.resume();
    }
}

#[cfg(unix)]
fn resume(state: &Mutex<State>, host: &GpuiHost, remote: &nexus_remote::machine::Remote) {
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    if *state != State::Released {
        return;
    }
    let (client, signed_out) = host.launch_client();
    let scope = host.session_scope();
    if (scope.is_local() && signed_out) || (!matches!(scope, SessionScope::Unscoped) && client.is_some()) {
        *state = State::Ready;
        remote.resume();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use futures::future::BoxFuture;
    use nexus_remote::machine::{Backoff, Link, Opened, Remote, Sink, Status};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Notify;

    #[derive(Default)]
    struct SlowOpen {
        attempts: AtomicU64,
        opening: Notify,
        release: Notify,
        reopened: Notify,
    }

    impl Link for SlowOpen {
        fn open(&self) -> BoxFuture<'_, Result<Opened, String>> {
            Box::pin(async move {
                if self.attempts.fetch_add(1, SeqCst) == 0 {
                    self.opening.notify_one();
                    self.release.notified().await;
                } else {
                    self.reopened.notify_one();
                }
                Err("test link has no socket".into())
            })
        }
        fn update(&self) -> BoxFuture<'_, Result<(), String>> {
            Box::pin(async { Err("the test link cannot be updated".into()) })
        }
    }

    impl Sink for SlowOpen {
        fn push(&self, _: &nexus_remote::client::Pushed) {}
        fn status(&self, _: &Status) {}
    }

    #[tokio::test]
    async fn pending_account_link_stays_held_until_the_old_open_finishes() {
        let link = Arc::new(SlowOpen::default());
        let remote = Remote::start(link.clone(), link.clone(), Backoff::default());
        tokio::time::timeout(Duration::from_secs(2), link.opening.notified())
            .await
            .unwrap();
        remote.hold();
        let state = Mutex::new(State::Ready);
        let generation = AtomicU64::new(2);
        let superseded = resume_after_account_change(&state, &generation, 1, &remote);
        tokio::pin!(superseded);
        assert!(futures::poll!(&mut superseded).is_pending());
        assert!(remote.is_held());
        link.release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), superseded).await.unwrap();
        assert!(remote.is_held(), "an earlier transition cannot resume the relay");
        resume_after_account_change(&state, &generation, 2, &remote).await;
        tokio::time::timeout(Duration::from_secs(2), link.reopened.notified())
            .await
            .unwrap();
        assert!(!remote.is_held());
        remote.close();
    }
}
