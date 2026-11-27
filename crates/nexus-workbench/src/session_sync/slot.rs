//! The session sync backend of the sign-in now, resolved in the background ([`Slot`]): the desktop apps' when they
//! run the Code sessions, and their background service's (nexus-host's `local` mode) when it runs them. The switch,
//! the managed policy and the device key are the apps'.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nexus_client::Client;
use nexus_client::e2e::DeviceKey;

use super::{Backend, GatewayBackend, KeyStore};

/// How long a failed resolution waits before the next try.
pub const RETRY: Duration = Duration::from_secs(60);
/// The key store's file in the app's data folder.
pub const KEYS_FILE: &str = "session-sync-keys.sqlite3";

/// The sign-in a backend is for: the workbench's scope (`<gateway>|<org>`) and the token's marker, never the token.
pub fn sign_in(scope: &str, marker: &str) -> String {
    format!("{scope}\n{marker}")
}

/// A resolution that runs in the background.
pub type Task = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
/// A resolution's answer: the backend, or `None` when it was refused.
pub type Resolving<T> = Pin<Box<dyn Future<Output = Option<T>> + Send + 'static>>;

/// The backend of the current sign-in, resolved in the background. `T` is the backend (`Arc<dyn Backend>`; a fake in
/// tests).
pub struct Slot<T = Arc<dyn Backend>> {
    state: Arc<Mutex<SlotState<T>>>,
    keys: Mutex<Option<Arc<KeyStore>>>,
    retry: Duration,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Slot {
            state: Arc::default(),
            keys: Mutex::new(None),
            retry: RETRY,
        }
    }
}

struct SlotState<T> {
    /// The sign-in the backend is for ([`sign_in`]).
    session: Option<String>,
    backend: Option<T>,
    /// When the current resolution started; None when none runs or ran.
    tried: Option<Instant>,
    running: bool,
}

impl<T> Default for SlotState<T> {
    fn default() -> Self {
        SlotState {
            session: None,
            backend: None,
            tried: None,
            running: false,
        }
    }
}

impl<T: Clone + Send + 'static> Slot<T> {
    /// A slot that tries again `retry` after a failed resolution.
    pub fn with_retry(retry: Duration) -> Self {
        Slot {
            retry,
            ..Slot::default()
        }
    }

    /// The backend for `sign_in`, or None while it resolves or after it was refused. Another sign-in than the last
    /// drops the old backend at once. With none, and no resolution running or tried within the retry, `resolve` starts
    /// one through `spawn`.
    pub fn current(
        &self,
        sign_in: &str,
        spawn: impl FnOnce(Task),
        resolve: impl FnOnce() -> Resolving<T>,
    ) -> Option<T> {
        let (backend, start) = {
            let mut slot = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.session.as_deref() != Some(sign_in) {
                // Another account: the old backend is never used again, not even during the switch.
                *slot = SlotState {
                    session: Some(sign_in.to_string()),
                    ..SlotState::default()
                };
            }
            let start =
                slot.backend.is_none() && !slot.running && slot.tried.is_none_or(|tried| tried.elapsed() >= self.retry);
            if start {
                slot.running = true;
                slot.tried = Some(Instant::now());
            }
            (slot.backend.clone(), start)
        };
        // Started with the lock released: `spawn` may run the task at once.
        if start {
            let (state, key, resolving) = (self.state.clone(), sign_in.to_string(), resolve());
            spawn(Box::pin(async move {
                let backend = resolving.await;
                let mut slot = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if slot.session.as_deref() == Some(key.as_str()) {
                    slot.running = false;
                    slot.backend = backend;
                }
            }));
        }
        backend
    }

    /// Retry after the device setup UI made the missing key available. A ready or in-flight backend is preserved.
    pub fn retry_unavailable(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.backend.is_none() && !state.running {
            state.tried = None;
        }
    }

    /// The content keys' store, opened at `path` on first use and kept.
    pub fn keys(&self, path: impl FnOnce() -> Option<PathBuf>) -> Option<Arc<KeyStore>> {
        let mut keys = self.keys.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if keys.is_none() {
            *keys = KeyStore::open(&path()?).ok().map(Arc::new);
        }
        keys.clone()
    }
}

/// The Gateway backend for `scope` on `client`'s sign-in, with this computer's device key. None, and an error line
/// without content on stderr, when Gateway refuses it.
pub async fn connect(
    client: Client,
    scope: String,
    device: DeviceKey,
    keys: Arc<KeyStore>,
    allow_local_cache: bool,
) -> Option<Arc<dyn Backend>> {
    match GatewayBackend::connect(client, scope, device, keys, allow_local_cache).await {
        Ok(backend) => Some(Arc::new(backend)),
        Err(error) => {
            // The adapter's errors carry no content.
            eprintln!("nexus-desktop: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    /// Runs a task that never waits, as the fake resolutions here.
    fn run(mut task: Task) {
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(()));
    }

    /// A fake resolution: counts its calls and answers `answer`.
    fn fake(calls: &Arc<AtomicUsize>, answer: Option<&'static str>) -> impl FnOnce() -> Resolving<&'static str> {
        let calls = calls.clone();
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { answer })
        }
    }

    #[test]
    fn a_backend_is_for_a_scope_and_a_sign_in() {
        assert_eq!(sign_in("https://gw|org", "abc"), "https://gw|org\nabc");
    }

    #[test]
    fn resolves_once_in_the_background_and_then_serves_the_backend() {
        let slot = Slot::<&str>::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut pending = Vec::new();
        assert_eq!(
            slot.current("a", |task| pending.push(task), fake(&calls, Some("backend a"))),
            None
        );
        // While it runs, no second resolution starts.
        assert_eq!(
            slot.current("a", |task| pending.push(task), fake(&calls, Some("other"))),
            None
        );
        assert_eq!((calls.load(Ordering::SeqCst), pending.len()), (1, 1));
        pending.drain(..).for_each(run);
        assert_eq!(
            slot.current("a", |_| panic!("no new resolution"), fake(&calls, None)),
            Some("backend a")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn another_sign_in_drops_the_backend_at_once_and_a_late_answer_is_ignored() {
        let slot = Slot::<&str>::default();
        let calls = Arc::new(AtomicUsize::new(0));
        slot.current("a", run, fake(&calls, Some("backend a")));
        assert_eq!(slot.current("a", run, fake(&calls, None)), Some("backend a"));
        // The other sign-in's resolution is held: until it ends, nothing syncs.
        let mut pending = Vec::new();
        assert_eq!(
            slot.current("b", |task| pending.push(task), fake(&calls, Some("backend b"))),
            None
        );
        // Back to `a` before `b` answered: `b`'s answer is dropped.
        let mut again = Vec::new();
        assert_eq!(
            slot.current("a", |task| again.push(task), fake(&calls, Some("backend a2"))),
            None
        );
        pending.drain(..).for_each(run);
        assert_eq!(slot.current("a", |task| again.push(task), fake(&calls, None)), None);
        again.drain(..).for_each(run);
        assert_eq!(
            slot.current("a", |_| panic!("resolved"), fake(&calls, None)),
            Some("backend a2")
        );
    }

    #[test]
    fn a_refused_resolution_waits_for_the_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let slot = Slot::<&str>::default();
        assert_eq!(slot.current("a", run, fake(&calls, None)), None);
        assert_eq!(slot.current("a", |_| panic!("too soon"), fake(&calls, Some("x"))), None);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let eager = Slot::<&str>::with_retry(Duration::ZERO);
        assert_eq!(eager.current("a", run, fake(&calls, None)), None);
        assert_eq!(eager.current("a", run, fake(&calls, Some("x"))), None);
        assert_eq!(
            eager.current("a", |_| panic!("resolved"), fake(&calls, None)),
            Some("x")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn opens_the_key_store_once() {
        let dir = std::env::temp_dir().join(format!("nexus-sync-keys-{}", std::process::id()));
        let slot = Slot::<&str>::default();
        let first = slot.keys(|| Some(dir.join(KEYS_FILE))).expect("store");
        let again = slot.keys(|| panic!("opened again")).expect("store");
        assert!(Arc::ptr_eq(&first, &again));
        assert!(dir.join(KEYS_FILE).exists());
        assert!(Slot::<&str>::default().keys(|| None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
