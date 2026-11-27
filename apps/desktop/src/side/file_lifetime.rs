//! Frozen authority plus one replaceable native presentation's lifetime.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

pub(crate) type Guard = Arc<dyn Fn() -> bool + Send + Sync>;

pub(super) struct Lifetime {
    authority: Guard,
    live: Arc<AtomicBool>,
    revision: Arc<AtomicU64>,
}
impl Lifetime {
    pub fn new(authority: Guard) -> Self {
        Self {
            authority,
            live: Arc::new(AtomicBool::new(true)),
            revision: Arc::new(AtomicU64::new(0)),
        }
    }
    pub fn current(&self) -> bool {
        !self.retired() && (self.authority)() && !self.retired()
    }
    pub fn retired(&self) -> bool {
        !self.live.load(Ordering::SeqCst)
    }
    pub fn guard(&self) -> Guard {
        let authority = self.authority.clone();
        let live = self.live.clone();
        Arc::new(move || live.load(Ordering::SeqCst) && authority() && live.load(Ordering::SeqCst))
    }
    pub fn next(&self) -> Permit {
        self.revision.fetch_add(1, Ordering::SeqCst);
        self.capture()
    }
    pub fn capture(&self) -> Permit {
        Permit {
            guard: self.guard(),
            revision: self.revision.clone(),
            expected: self.revision.load(Ordering::SeqCst),
        }
    }
    pub fn retire(&self) {
        self.live.store(false, Ordering::SeqCst);
    }
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.retire();
    }
}
#[derive(Clone)]
pub(super) struct Permit {
    guard: Guard,
    revision: Arc<AtomicU64>,
    expected: u64,
}
impl Permit {
    pub fn current(&self) -> bool {
        self.revision.load(Ordering::SeqCst) == self.expected
            && (self.guard)()
            && self.revision.load(Ordering::SeqCst) == self.expected
    }
    pub fn guard(&self) -> Guard {
        let permit = self.clone();
        Arc::new(move || permit.current())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_receipts_do_not_recapture_authority_or_survive_retirement() {
        let epoch = Arc::new(AtomicU64::new(1));
        let captured = epoch.clone();
        let lifetime = Lifetime::new(Arc::new(move || captured.load(Ordering::SeqCst) == 1));
        let old = lifetime.next();
        assert!(old.current());
        let fresh = lifetime.next();
        assert!(!old.current());
        assert!(fresh.current());
        epoch.store(3, Ordering::SeqCst);
        assert!(!fresh.current());
        let next = Lifetime::new(Arc::new(|| true));
        let queued = next.capture();
        drop(next);
        assert!(!queued.current());
    }

    fn held_authority() -> (Lifetime, std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (entered, entry) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let released = std::sync::Mutex::new(released);
        let first = AtomicBool::new(true);
        let authority: Guard = Arc::new(move || {
            if first.swap(false, Ordering::SeqCst) {
                entered.send(()).expect("authority entered");
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(3))
                    .expect("held authority released");
            }
            true
        });
        (Lifetime::new(authority), entry, release)
    }
    #[test]
    fn retirement_during_a_held_authority_check_rejects_current_and_queued_dispatch() {
        for queued in [false, true] {
            let (lifetime, entered, release) = held_authority();
            let lifetime = Arc::new(lifetime);
            let worker_lifetime = lifetime.clone();
            let dispatch = lifetime.capture().guard();
            let worker = std::thread::spawn(move || if queued { dispatch() } else { worker_lifetime.current() });
            entered
                .recv_timeout(std::time::Duration::from_secs(3))
                .expect("real authority callback is held");
            lifetime.retire();
            release.send(()).unwrap();
            assert!(
                !worker.join().unwrap(),
                "a late true authority result cannot revive a retired presentation"
            );
        }
        let fresh = Lifetime::new(Arc::new(|| true));
        assert!(
            fresh.current() && fresh.capture().guard()(),
            "a fresh presentation must remain usable"
        );
    }
    #[test]
    fn superseding_a_request_during_its_held_guard_rejects_the_old_receipt() {
        let (lifetime, entered, release) = held_authority();
        let old = lifetime.next();
        let worker = std::thread::spawn(move || old.current());
        entered
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("actual old receipt authority callback is held");
        let fresh = lifetime.next();
        release.send(()).unwrap();
        assert!(
            !worker.join().unwrap(),
            "a late true guard cannot approve a superseded request"
        );
        assert!(fresh.current(), "the current request must still accept its result");
    }
}
