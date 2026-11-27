//! Primary-window resource ownership, independent of GPUI handles and view lifetimes.
use crate::backend::Backend;
use crate::gateway::{Account, Event};
use std::sync::atomic::Ordering;

/// Credential fingerprints are opaque identifiers, never the credential itself.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Identity {
    gateway: String,
    credential: String,
    person: Option<String>,
    user: Option<String>,
    org: String,
}

impl Identity {
    pub(crate) fn current(gateway: &dyn Backend, account: &Account) -> Option<Self> {
        Self::at(gateway.url(), gateway.marker()?, account)
    }

    pub(crate) fn at(url: &str, marker: String, account: &Account) -> Option<Self> {
        Some(Self {
            gateway: url.to_string(),
            credential: marker,
            person: account.person_id.clone(),
            user: account.user_id.clone(),
            org: account.org.clone(),
        })
    }

    pub(crate) fn org(&self) -> &str {
        &self.org
    }

    pub(crate) fn matches_gateway(&self, gateway: &dyn Backend) -> bool {
        self.matches_credential(gateway.url(), gateway.marker().as_deref())
    }

    pub(crate) fn matches_credential(&self, url: &str, marker: Option<&str>) -> bool {
        self.gateway == url && marker == Some(self.credential.as_str())
    }
}

/// Account reads use the same epoch/revision envelope as Workspace::handle. A reply may
/// already be queued when a sign-out or local write invalidates it.
pub(super) enum IdentityEvent {
    SignedIn,
    SignedOut,
    Account(Account),
    ReloadAccount,
    Ignore,
}

pub(super) fn identity_event(mut event: Event) -> IdentityEvent {
    loop {
        match event {
            Event::ReadSnapshot {
                epoch,
                expected,
                revision,
                event: inner,
            } => {
                if epoch.load(Ordering::SeqCst) != expected {
                    return IdentityEvent::Ignore;
                }
                if revision
                    .as_ref()
                    .is_some_and(|(version, expected)| version.load(Ordering::SeqCst) != *expected)
                {
                    return if matches!(*inner, Event::Account(_)) {
                        IdentityEvent::ReloadAccount
                    } else {
                        IdentityEvent::Ignore
                    };
                }
                event = *inner;
            }
            Event::SignedIn => return IdentityEvent::SignedIn,
            Event::SignedOut | Event::SignInFailed(_) => return IdentityEvent::SignedOut,
            Event::Account(account) => return IdentityEvent::Account(account),
            _ => return IdentityEvent::Ignore,
        }
    }
}

/// The primary role survives its native window. Other windows never inherit its resources.
pub(super) struct Primary<W, R> {
    window: Option<W>,
    pub(super) saved: Option<R>,
}

impl<W, R> Default for Primary<W, R> {
    fn default() -> Self {
        Self {
            window: None,
            saved: None,
        }
    }
}

impl<W: Copy + PartialEq, R> Primary<W, R> {
    pub(super) fn opened(&mut self, window: W) {
        if self.window.is_none() {
            self.window = Some(window);
        }
    }

    pub(super) fn is_primary(&self, window: W) -> bool {
        self.window == Some(window)
    }

    pub(super) fn has_window(&self) -> bool {
        self.window.is_some()
    }

    pub(super) fn needs_window(&self) -> bool {
        self.window.is_none() && self.saved.is_some()
    }

    pub(super) fn closed(&mut self, window: W, keep: bool, saved: Option<R>) {
        if !self.is_primary(window) {
            return;
        }
        self.window = None;
        if !keep {
            self.saved = None;
        } else if saved.is_some() {
            self.saved = saved;
        }
        // If a replacement closed before its account arrived, its older parked resources remain.
    }

    pub(super) fn take(&mut self, window: W, valid: impl FnOnce(&R) -> bool) -> Option<R> {
        if !self.is_primary(window) {
            return None;
        }
        self.saved.take().filter(valid)
    }
}

/// Retain only when ordinary closure leaves the app running.
pub(super) fn keep_on_close(quitting: bool, other_window: bool, panel: bool, tray: bool) -> bool {
    !quitting && (other_window || panel || tray)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct Resource(Rc<Cell<usize>>);
    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn wrapped_account(
        epoch: &std::sync::Arc<std::sync::atomic::AtomicU64>,
        revision: &std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) -> Event {
        let account = Account::from_json(&serde_json::json!({
            "tenant": {"id":"organization-b"}, "user": {"id":"member-b"}
        }))
        .unwrap();
        Event::ReadSnapshot {
            epoch: epoch.clone(),
            expected: 7,
            revision: Some((revision.clone(), 11)),
            event: Box::new(Event::Account(account)),
        }
    }

    #[test]
    fn queued_account_delivery_unwraps_the_real_gateway_snapshot_envelope() {
        use futures::StreamExt;
        use std::sync::{Arc, atomic::AtomicU64};
        let epoch = Arc::new(AtomicU64::new(7));
        let revision = Arc::new(AtomicU64::new(11));
        let (send, mut events) = futures::channel::mpsc::unbounded();
        send.unbounded_send(wrapped_account(&epoch, &revision)).unwrap();
        let event = futures::executor::block_on(events.next()).unwrap();
        assert!(matches!(identity_event(event), IdentityEvent::Account(account)
            if account.org == "organization-b" && account.user_id.as_deref() == Some("member-b")));
    }

    #[test]
    fn queued_account_snapshots_reject_old_auth_and_reload_after_a_local_write() {
        use futures::StreamExt;
        use std::sync::{Arc, atomic::AtomicU64};
        let epoch = Arc::new(AtomicU64::new(7));
        let revision = Arc::new(AtomicU64::new(11));
        let (send, mut events) = futures::channel::mpsc::unbounded();
        send.unbounded_send(wrapped_account(&epoch, &revision)).unwrap();
        epoch.store(8, Ordering::SeqCst);
        let event = futures::executor::block_on(events.next()).unwrap();
        assert!(matches!(identity_event(event), IdentityEvent::Ignore));
        epoch.store(7, Ordering::SeqCst);
        send.unbounded_send(wrapped_account(&epoch, &revision)).unwrap();
        revision.store(12, Ordering::SeqCst);
        let event = futures::executor::block_on(events.next()).unwrap();
        assert!(matches!(identity_event(event), IdentityEvent::ReloadAccount));
        send.unbounded_send(Event::SignedOut).unwrap();
        let event = futures::executor::block_on(events.next()).unwrap();
        assert!(matches!(identity_event(event), IdentityEvent::SignedOut));
    }

    #[test]
    fn last_window_without_tray_and_explicit_quit_never_retain() {
        assert!(!keep_on_close(false, false, false, false));
        assert!(keep_on_close(false, true, false, false));
        assert!(keep_on_close(false, false, true, false));
        assert!(keep_on_close(false, false, false, true));
        assert!(!keep_on_close(true, true, true, true));
    }

    #[test]
    fn only_the_reopened_primary_can_adopt_and_secondary_close_does_not_destroy_it() {
        let dropped = Rc::new(Cell::new(0));
        let mut primary = Primary::default();
        primary.opened(1);
        primary.opened(2);
        primary.closed(1, true, Some(Resource(dropped.clone())));
        assert!(primary.needs_window());
        primary.closed(2, true, None);
        assert!(primary.saved.is_some());
        primary.opened(3);
        assert!(primary.take(2, |_| true).is_none());
        let saved = primary.take(3, |_| true).unwrap();
        assert_eq!(dropped.get(), 0);
        drop(saved);
        assert_eq!(dropped.get(), 1);
    }

    #[test]
    fn identity_mismatch_and_quitting_drop_parked_resources() {
        let dropped = Rc::new(Cell::new(0));
        let mut primary = Primary::default();
        primary.opened(1);
        primary.closed(1, true, Some(Resource(dropped.clone())));
        primary.opened(2);
        assert!(primary.take(2, |_| false).is_none());
        assert_eq!(dropped.get(), 1);
        primary.closed(2, true, Some(Resource(dropped.clone())));
        primary.saved = None;
        assert_eq!(dropped.get(), 2);
        primary.opened(3);
        primary.closed(3, false, Some(Resource(dropped.clone())));
        assert!(!primary.needs_window());
        assert_eq!(dropped.get(), 3);
    }

    #[test]
    fn cef_owners_survive_secondary_close_and_die_on_identity_rejection_or_quit() {
        let (page, probe) = crate::web::fixture_page(101);
        let mut primary = Primary::default();
        primary.opened(1);
        primary.closed(1, true, Some(page));
        primary.closed(2, true, None);
        primary.opened(3);
        assert!(primary.take(2, |_| true).is_none());
        assert!(probe.alive());
        assert!(primary.take(3, |_| false).is_none());
        assert!(!probe.alive());
        let (page, probe) = crate::web::fixture_page(102);
        primary.closed(3, true, Some(page));
        primary.saved = None; // Same disposal used by SignedOut/Gateway switch/explicit quit.
        assert!(!probe.alive());
        let (page, probe) = crate::web::fixture_page(103);
        primary.opened(4);
        primary.closed(4, keep_on_close(false, false, false, false), Some(page));
        assert!(!probe.alive());
    }

    #[test]
    fn closing_a_replacement_before_sign_in_keeps_the_original_owner() {
        let mut primary = Primary::default();
        primary.opened(1);
        primary.closed(1, true, Some("original"));
        primary.opened(2);
        primary.closed(2, true, None);
        primary.opened(3);
        assert_eq!(primary.take(3, |_| true), Some("original"));
    }
}

#[cfg(test)]
mod browser_routing_tests {
    #[gpui_kit::test]
    fn retired_home_browser_keys_are_refused_before_any_window_fallback(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            for key in ["home:old-primary:org", "home:replacement:org"] {
                // No Shell is installed: reaching fallback would panic, not merely refuse.
                let origin = crate::side::browser::agent::LocalOrigin {
                    data: std::path::PathBuf::from("/fixture/local-store"),
                    scope: "fixture".into(),
                    machine: None,
                };
                let valid: std::sync::Arc<dyn Fn() -> bool + Send + Sync> = std::sync::Arc::new(|| true);
                assert!(crate::windows::agent_browser(&origin, key, &valid, cx).is_err());
            }
        });
    }
}
