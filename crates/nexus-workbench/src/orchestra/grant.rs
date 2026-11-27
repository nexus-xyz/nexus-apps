//! A run's grant: the agent id in the URL and the token in the agent's
//! environment. Every start of an agent process gets a new one, kept on the run; stop and exit clear it first, so a
//! call from an older run finds none and gets 401. Also the random tokens and their comparison, which `nexus-host`'s
//! socket token uses too.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

/// 244 random bits (two version 4 UUIDs, from the system's random source), as 64 hex digits.
pub fn new_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// Equal, in a time that does not depend on where they first differ.
pub fn same_token(given: &str, expected: &str) -> bool {
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A token's SHA-256, in hex. Unsalted on purpose: the tokens are 244 random bits, so there is nothing to guess.
pub fn sha256_hex(token: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Marks a kept token as a hash, not the token itself.
pub const HASH_PREFIX: &str = "sha256:";

/// What a token is kept as when its file tells hashes from older plain tokens: [`HASH_PREFIX`] and [`sha256_hex`].
pub fn hash_token(token: &str) -> String {
    format!("{HASH_PREFIX}{}", sha256_hex(token))
}

/// The app's toolsets a run may call. `browser` is the slot for the side pane's browser tools, always off
/// for now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Toolsets {
    /// The linked-session tools (`start_session` and the rest).
    pub crew: bool,
    pub browser: bool,
    pub terminal: bool,
}

/// One run's grant. Clones share its calls in flight and its open waits.
#[derive(Clone)]
pub struct Grant {
    /// 32 random hex digits: the last part of the server's URL for this run.
    pub agent_id: String,
    pub token: String,
    pub toolsets: Toolsets,
    /// Bind session choices to the sign-in that started this run. Never copied into agent arguments.
    pub(crate) choice_identity: Option<String>,
    in_flight: Arc<AtomicUsize>,
    waits: Arc<Mutex<Waits>>,
}

/// The token is the run's secret: debug output never shows it, as [`crate::process::Spawn`]'s never shows the
/// environment's values.
impl std::fmt::Debug for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grant")
            .field("agent_id", &self.agent_id)
            .field("token", &"<redacted>")
            .field("toolsets", &self.toolsets)
            .finish_non_exhaustive()
    }
}

/// A grant's open waits, each under a serial of its own, and the latest one under each JSON-RPC request id, so
/// `notifications/cancelled` can end it. A client may use an id again (a new MCP client after a reconnect starts
/// from 0): the newer wait takes the id, and the older one keeps its own place until it ends.
#[derive(Debug, Default)]
struct Waits {
    next: u64,
    open: HashMap<u64, oneshot::Sender<()>>,
    by_request: HashMap<String, u64>,
}

impl Default for Grant {
    fn default() -> Grant {
        Grant::new()
    }
}

impl Grant {
    /// A new grant with no toolset: [`Toolsets::crew`] is set when the run gets the tools.
    pub fn new() -> Grant {
        Grant {
            agent_id: uuid::Uuid::new_v4().simple().to_string(),
            token: new_token(),
            toolsets: Toolsets::default(),
            choice_identity: None,
            in_flight: Arc::default(),
            waits: Arc::default(),
        }
    }

    /// Whether `agent_id` and `token` are this grant's, both compared in constant time.
    pub fn matches(&self, agent_id: &str, token: &str) -> bool {
        // Both are compared, so the time does not say which one differed.
        same_token(agent_id, &self.agent_id) & same_token(token, &self.token)
    }

    /// A call in flight from now until the guard drops.
    pub(crate) fn flight(&self) -> Flight {
        let at = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        Flight {
            in_flight: self.in_flight.clone(),
            at,
        }
    }

    /// An open wait under `request`: the receiver resolves when a `notifications/cancelled` names it. A wait already
    /// open under the same id is not ended: the id names this one from now on.
    pub(crate) fn open_wait(&self, request: &str) -> (oneshot::Receiver<()>, OpenWait) {
        let (tx, rx) = oneshot::channel();
        let serial = {
            let mut waits = lock(&self.waits);
            waits.next += 1;
            let serial = waits.next;
            waits.open.insert(serial, tx);
            waits.by_request.insert(request.to_string(), serial);
            serial
        };
        let open = OpenWait {
            waits: self.waits.clone(),
            request: request.to_string(),
            serial,
        };
        (rx, open)
    }

    /// Cancels the open wait under `request`, if there is one: the latest one opened under it, and only that one.
    pub(crate) fn cancel(&self, request: &str) {
        let tx = {
            let mut waits = lock(&self.waits);
            let serial = waits.by_request.remove(request);
            serial.and_then(|serial| waits.open.remove(&serial))
        };
        if let Some(tx) = tx {
            let _ = tx.send(());
        }
    }
}

fn lock(waits: &Mutex<Waits>) -> std::sync::MutexGuard<'_, Waits> {
    waits.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A call in flight on a grant ([`Grant::flight`]).
#[derive(Debug)]
pub(crate) struct Flight {
    in_flight: Arc<AtomicUsize>,
    at: usize,
}

impl Flight {
    /// The grant's calls in flight when this one was admitted, this one included. Two calls admitted at once each
    /// see their own count, so a burst admits exactly up to a cap.
    pub(crate) fn at(&self) -> usize {
        self.at
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An open wait's place in its grant ([`Grant::open_wait`]): it leaves when the wait ends, and takes only its own
/// place, never a newer wait's under the same request id.
#[derive(Debug)]
pub(crate) struct OpenWait {
    waits: Arc<Mutex<Waits>>,
    request: String,
    serial: u64,
}

impl Drop for OpenWait {
    fn drop(&mut self) {
        let mut waits = lock(&self.waits);
        waits.open.remove(&self.serial);
        if waits.by_request.get(&self.request) == Some(&self.serial) {
            waits.by_request.remove(&self.request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_matches_its_own_id_and_token_only() {
        let grant = Grant::new();
        assert_eq!(grant.agent_id.len(), 32);
        assert_eq!(grant.token.len(), 64);
        assert!(grant.matches(&grant.agent_id, &grant.token));
        assert!(!grant.matches(&grant.agent_id, &new_token()));
        assert!(!grant.matches(&Grant::new().agent_id, &grant.token));
        assert!(!grant.matches("", ""));
        assert!(same_token("abc", "abc"));
        assert!(!same_token("abc", "abd"));
        assert!(!same_token("short", "longer"));
    }

    #[test]
    fn calls_in_flight_count_until_they_drop() {
        let grant = Grant::new();
        let one = grant.flight();
        let two = grant.clone().flight();
        assert_eq!((one.at(), two.at()), (1, 2));
        drop(one);
        assert_eq!(grant.flight().at(), 2);
        assert_eq!(grant.flight().at(), 2);
    }

    #[test]
    fn a_cancel_reaches_the_open_wait_it_names() {
        let grant = Grant::new();
        let (mut rx, open) = grant.open_wait("7");
        grant.cancel("8");
        assert!(rx.try_recv().is_err());
        grant.cancel("7");
        assert_eq!(rx.try_recv(), Ok(()));
        drop(open);
        let waits = lock(&grant.waits);
        assert!(waits.open.is_empty() && waits.by_request.is_empty());
    }

    #[test]
    fn a_second_wait_under_the_same_id_takes_it_and_leaves_the_first_alone() {
        use tokio::sync::oneshot::error::TryRecvError;
        let grant = Grant::new();
        let (mut first, first_open) = grant.open_wait("7");
        let (mut second, second_open) = grant.open_wait("7");
        // Not cancelled, and not closed either: the first wait goes on.
        assert_eq!(first.try_recv(), Err(TryRecvError::Empty));
        // The first one ending takes only its own place.
        drop(first_open);
        assert_eq!(second.try_recv(), Err(TryRecvError::Empty));
        // The id names the second one.
        grant.cancel("7");
        assert_eq!(second.try_recv(), Ok(()));
        drop(second_open);
        let waits = lock(&grant.waits);
        assert!(waits.open.is_empty() && waits.by_request.is_empty());
        drop(waits);
        // A cancel under an id reaches the latest wait only.
        let (mut older, _older_open) = grant.open_wait("9");
        let (mut newer, _newer_open) = grant.open_wait("9");
        grant.cancel("9");
        assert_eq!((older.try_recv(), newer.try_recv()), (Err(TryRecvError::Empty), Ok(())));
    }

    #[test]
    fn debug_output_never_shows_the_token() {
        let grant = Grant::new();
        let shown = format!("{grant:?} {grant:#?}");
        assert!(!shown.contains(&grant.token), "{shown}");
        assert!(shown.contains(&grant.agent_id), "{shown}");
    }
}
