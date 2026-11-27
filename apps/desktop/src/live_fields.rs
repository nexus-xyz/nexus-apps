//! Each local session's live fields (`nexus_workbench::event::LiveFields`): what only the running workbench knows,
//! such as the work its agent left running past its turn. The store never holds them, and the windows read their rows
//! from the store (`code.rs`). So this keeps them from the workbench's last push of each session (`wb:thread`) and puts
//! them back on every summary read ([`apply`]).
//!
//! The runner notes each push ([`noted`]). It reads every session once when it attaches to the workbench or reconnects
//! to the background service ([`seed`]), so a session pushed before then is known too. An organization switch drops
//! them all ([`forget`], from `leave_org`). Remote machines apply `wb:thread` themselves (`machines.rs`) and do not
//! use this.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use nexus_workbench::event::{LiveFields, ThreadSummary};

/// The live fields of each session, with the push that set them: a [`seed`] read before a later push never
/// overwrites it.
#[derive(Default)]
pub(crate) struct Live {
    entries: Mutex<Entries>,
}

#[derive(Default)]
struct Entries {
    /// Counts the pushes and the seeds, in order.
    clock: u64,
    by_thread: HashMap<String, (u64, LiveFields)>,
}

impl Live {
    fn lock(&self) -> std::sync::MutexGuard<'_, Entries> {
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The workbench pushed `thread`: its live fields replace the session's last ones, so an empty push clears them.
    pub(crate) fn noted(&self, thread: &ThreadSummary) {
        let mut entries = self.lock();
        entries.clock += 1;
        let at = entries.clock;
        entries.by_thread.insert(thread.id.clone(), (at, thread.take_live()));
    }

    /// The point a [`Live::seed`] read starts from: pushes after it win over what it read.
    pub(crate) fn mark(&self) -> u64 {
        let mut entries = self.lock();
        entries.clock += 1;
        entries.clock
    }

    /// Every session as the workbench read it at `mark`, each kept unless a push came after.
    pub(crate) fn seed(&self, mark: u64, threads: impl IntoIterator<Item = ThreadSummary>) {
        let mut entries = self.lock();
        for thread in threads {
            let pushed_since = entries.by_thread.get(&thread.id).is_some_and(|(at, _)| *at > mark);
            if !pushed_since {
                entries.by_thread.insert(thread.id.clone(), (mark, thread.take_live()));
            }
        }
    }

    /// Puts the session's live fields back on a summary read from the store. None known: it stays as stored.
    pub(crate) fn apply(&self, thread: &mut ThreadSummary) {
        if let Some((_, live)) = self.lock().by_thread.get(&thread.id) {
            thread.with_live(live);
        }
    }

    /// Drops every session's live fields.
    pub(crate) fn forget(&self) {
        self.lock().by_thread.clear();
    }
}

fn held() -> &'static Live {
    static HELD: OnceLock<Live> = OnceLock::new();
    HELD.get_or_init(Live::default)
}

/// [`Live::noted`] on this app's sessions: the runner got a `wb:thread` push.
pub(crate) fn noted(thread: &ThreadSummary) {
    held().noted(thread);
}

/// [`Live::mark`]: call it before the read whose result goes to [`seed`].
pub(crate) fn mark() -> u64 {
    held().mark()
}

/// [`Live::seed`] on this app's sessions.
pub(crate) fn seed(mark: u64, threads: impl IntoIterator<Item = ThreadSummary>) {
    held().seed(mark, threads);
}

/// [`Live::apply`] on this app's sessions: every summary read from the store goes through it.
pub(crate) fn apply(thread: &mut ThreadSummary) {
    held().apply(thread);
}

/// [`Live::forget`] on this app's sessions: the window leaves its organization (`leave_org`).
pub(crate) fn forget() {
    held().forget();
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_workbench::event::{BackgroundKind, BackgroundWork, ThreadStatus};

    fn summary(id: &str) -> ThreadSummary {
        serde_json::from_value(serde_json::json!({
            "id": id, "projectId": "p", "title": "t", "agent": "claude", "model": null, "permissionMode": "default",
            "via": "nexus", "status": "idle", "costUsd": 0, "nexusSessions": [], "contextTokens": null,
            "contextWindow": null, "createdAt": 1, "updatedAt": 1, "archived": false,
        }))
        .expect("a summary")
    }

    fn busy(id: &str) -> ThreadSummary {
        let mut thread = summary(id);
        thread.background = vec![BackgroundWork {
            kind: BackgroundKind::Shell,
            title: "sleep 60".into(),
            started_at: Some(1),
        }];
        thread
    }

    #[test]
    fn a_push_replaces_the_last_and_an_empty_one_clears_it() {
        let live = Live::default();
        let mut read = summary("a");
        live.apply(&mut read);
        assert_eq!(read, summary("a"), "nothing known: as stored");
        live.noted(&busy("a"));
        live.apply(&mut read);
        assert_eq!(read.background.len(), 1);
        live.noted(&summary("a"));
        let mut read = summary("a");
        live.apply(&mut read);
        assert!(read.background.is_empty());
        assert_eq!(read.status, ThreadStatus::Idle);
    }

    #[test]
    fn a_seed_read_before_a_push_does_not_undo_it() {
        let live = Live::default();
        let mark = live.mark();
        // The session's work ended after the seed's read began.
        live.noted(&summary("a"));
        live.seed(mark, [busy("a"), busy("b")]);
        let (mut a, mut b) = (summary("a"), summary("b"));
        live.apply(&mut a);
        live.apply(&mut b);
        assert!(a.background.is_empty(), "the later push wins");
        assert_eq!(
            b.background.len(),
            1,
            "a session pushed before then is known from the seed"
        );
        live.forget();
        let mut b = summary("b");
        live.apply(&mut b);
        assert!(b.background.is_empty());
    }
}
