//! A cloud machine's session history, sent to Gateway, so the apps read a session while the
//! machine sleeps. The workbench's log stays the source: this task reads it and sends what Gateway does not have yet.
//!
//! - It starts from Gateway's list of the machine's threads and their last numbers, at boot and after a wake, and
//!   after any answer that says the two disagree. It then sends each thread's events after that number, in order.
//! - An append is idempotent, so a batch sent twice (a lost answer, a restart) stores nothing twice.
//! - It never blocks a session: it reads the store on its own connection, and a failed call only waits and retries,
//!   from 2 seconds up to 5 minutes.
//! - A thread that is deleted or archived on the machine has its history deleted on Gateway.
//! - When the organization turns `cloudHistory` off, Gateway stores nothing: the task asks again every 10 minutes.
//!
//! The contract is the "Session history" routes of `/v1/cloud/machines/{id}`, with the machine's own token.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use futures::future::BoxFuture;
use nexus_client::{Client, Error};
use nexus_workbench::store::{EventText, LoggedThread, Reader};
use serde_json::{Value, json};

use crate::Result;
use crate::cloud::CloudFile;
use crate::files::Credentials;

/// How often a machine with nothing to send looks at its log again.
pub const TICK: Duration = Duration::from_secs(3);
/// The first wait after a failed call; it doubles up to [`MOST_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(2);
const MOST_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// How long the task waits while the organization's `cloudHistory` is off, or Gateway refuses the machine.
const RECHECK: Duration = Duration::from_secs(10 * 60);
/// How long it waits once the person's stored history is at Gateway's cap.
const FULL: Duration = Duration::from_secs(60 * 60);

/// Events in one call (Gateway takes 64).
const BATCH_EVENTS: usize = 32;
/// A batch's bodies as JSON text, under Gateway's 1 MB request limit with room for the rest.
const BATCH_BYTES: usize = 640 * 1024;
/// Gateway keeps a body up to this many bytes. A larger one is sent as its size only, as Gateway would store it.
const MAX_EVENT_BYTES: usize = 256 * 1024;

// What the task logs. Fixed texts: none carries a session's content.
const OFF: &str = "nexus-host: cloud session history is off for this organization. Nothing is sent.";
const RETRYING: &str = "nexus-host: cloud session history could not reach Nexus Gateway. It retries.";
const REFUSED: &str = "nexus-host: Nexus Gateway refused this machine's session history. It asks again later.";
const AT_CAP: &str = "nexus-host: this person's cloud session history is full. Sending waits.";
const LOCAL: &str = "nexus-host: cloud session history could not read the session log. It retries.";
const NO_KEY: &str = "nexus-host: a cloud machine without its Nexus key; no session history is sent.";

/// How often the host looks for `cloud.json` before it starts: the cloud provider may write it after the host started.
const LOOK: Duration = Duration::from_secs(2);

/// Starts once `cloud.json` is there, with the machine's own token, then runs until the host stops.
pub async fn begin(log: Reader, cloud: PathBuf, credentials: PathBuf) {
    let file = loop {
        if let Some(file) = CloudFile::load(&cloud) {
            break file;
        }
        tokio::time::sleep(LOOK).await;
    };
    let Some(credentials) = Credentials::load(&credentials) else {
        eprintln!("{NO_KEY}");
        return;
    };
    let gateway = file
        .gateway_url
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| credentials.gateway.clone());
    let client = Client::new(crate::gateway::client_options(
        &gateway,
        Some(credentials.token.clone()),
    ));
    run(
        log,
        Gateway {
            client,
            machine_id: file.machine_id,
        },
    )
    .await;
}

/// Gateway's side, as the task uses it.
pub trait Remote: Send + Sync {
    /// The machine's threads with stored history: their last numbers. None: history is off.
    fn marks(&self) -> BoxFuture<'_, std::result::Result<Option<HashMap<String, i64>>, Error>>;
    /// Appends a batch. The thread's last stored number; None: history is off.
    fn append<'a>(
        &'a self,
        thread_id: &'a str,
        events: Vec<Value>,
    ) -> BoxFuture<'a, std::result::Result<Option<i64>, Error>>;
    fn delete<'a>(&'a self, thread_id: &'a str) -> BoxFuture<'a, std::result::Result<(), Error>>;
}

/// The machine's log, as the task reads it.
pub trait Log: Send + Sync {
    fn threads(&self) -> Result<Vec<LoggedThread>>;
    fn events_after(&self, thread_id: &str, after: i64, limit: usize) -> Result<Vec<EventText>>;
}

impl Log for Reader {
    fn threads(&self) -> Result<Vec<LoggedThread>> {
        self.logged_threads()
    }

    fn events_after(&self, thread_id: &str, after: i64, limit: usize) -> Result<Vec<EventText>> {
        self.event_texts_after(thread_id, after, limit)
    }
}

/// Gateway, with the machine's own token.
pub struct Gateway {
    pub client: Client,
    pub machine_id: String,
}

impl Remote for Gateway {
    fn marks(&self) -> BoxFuture<'_, std::result::Result<Option<HashMap<String, i64>>, Error>> {
        Box::pin(async move {
            // Inline, so scripts/api-requirement.mjs reads the operation.
            let list = self
                .client
                .get(&format!("/v1/cloud/machines/{}/history", self.machine_id))
                .await?;
            Ok(marks_of(&list))
        })
    }

    fn append<'a>(
        &'a self,
        thread_id: &'a str,
        events: Vec<Value>,
    ) -> BoxFuture<'a, std::result::Result<Option<i64>, Error>> {
        Box::pin(async move {
            let answer = self
                .client
                .post(
                    &format!("/v1/cloud/machines/{}/history/{}/events", self.machine_id, thread_id),
                    json!({ "events": events }),
                )
                .await?;
            Ok(appended_of(&answer))
        })
    }

    fn delete<'a>(&'a self, thread_id: &'a str) -> BoxFuture<'a, std::result::Result<(), Error>> {
        Box::pin(async move {
            self.client
                .delete(&format!("/v1/cloud/machines/{}/history/{}", self.machine_id, thread_id))
                .await?;
            Ok(())
        })
    }
}

/// `GET …/history`'s marks; None when history is off.
fn marks_of(list: &Value) -> Option<HashMap<String, i64>> {
    if list["enabled"] == Value::Bool(false) {
        return None;
    }
    let threads = list["threads"].as_array().map(Vec::as_slice).unwrap_or_default();
    Some(
        threads
            .iter()
            .filter_map(|thread| Some((thread["threadId"].as_str()?.to_string(), thread["lastSeq"].as_i64()?)))
            .collect(),
    )
}

/// `POST …/events`' last stored number; None when history is off.
fn appended_of(answer: &Value) -> Option<i64> {
    if answer["enabled"] == Value::Bool(false) {
        return None;
    }
    Some(answer["lastSeq"].as_i64().unwrap_or(0))
}

/// One event as Gateway takes it: its stored text, or only its size when Gateway would not keep the text.
fn wire(event: &EventText) -> (Value, usize) {
    let bytes = event.body.len();
    let value = if bytes > MAX_EVENT_BYTES {
        json!({ "seq": event.seq, "at": event.at, "omittedBytes": bytes })
    } else {
        json!({ "seq": event.seq, "at": event.at, "body": event.body })
    };
    let size = value.to_string().len();
    if size > BATCH_BYTES {
        // Escaping made it too large for one call: its size only, the same on every try.
        let only = json!({ "seq": event.seq, "at": event.at, "omittedBytes": bytes });
        let size = only.to_string().len();
        return (only, size);
    }
    (value, size)
}

/// The events of one call: in order, at least one, at most [`BATCH_BYTES`] of JSON.
fn batch(events: &[EventText]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut total = 0;
    for event in events {
        let (value, size) = wire(event);
        if !out.is_empty() && total + size > BATCH_BYTES {
            break;
        }
        total += size;
        out.push(value);
    }
    out
}

/// What a round ended with, and how long the task waits before the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Gateway has the whole log.
    Done,
    /// The organization's `cloudHistory` is off.
    Off,
    /// The marks were wrong (a thread deleted on Gateway, say): read them again now.
    Resync,
    /// A call failed; wait and retry.
    Failed,
    /// Gateway refused the machine (its token, or the machine is gone).
    Refused,
    /// The person's history is at Gateway's cap.
    Full,
    /// The log could not be read.
    Local,
}

/// The task's state: Gateway's last number per thread, once read.
pub struct Pusher<L, R> {
    log: L,
    remote: R,
    marks: Option<HashMap<String, i64>>,
    /// Threads Gateway refused for good, already logged: the log says it once, not every round.
    skipped: HashSet<String>,
}

impl<L: Log, R: Remote> Pusher<L, R> {
    pub fn new(log: L, remote: R) -> Self {
        Pusher {
            log,
            remote,
            marks: None,
            skipped: HashSet::new(),
        }
    }

    /// Sends what Gateway lacks, and deletes what the machine no longer keeps.
    pub async fn round(&mut self) -> Outcome {
        if self.marks.is_none() {
            match self.remote.marks().await {
                Ok(Some(marks)) => self.marks = Some(marks),
                Ok(None) => return Outcome::Off,
                Err(error) => return failed(&error),
            }
        }
        let Ok(threads) = self.log.threads() else {
            return Outcome::Local;
        };
        let kept: HashSet<&str> = threads
            .iter()
            .filter(|thread| !thread.archived)
            .map(|thread| thread.id.as_str())
            .collect();
        let gone: Vec<String> = self
            .marks
            .iter()
            .flat_map(|marks| marks.keys())
            .filter(|id| !kept.contains(id.as_str()))
            .cloned()
            .collect();
        for id in gone {
            if let Err(error) = self.remote.delete(&id).await {
                return failed(&error);
            }
            self.forget(&id);
        }
        for thread in threads.iter().filter(|thread| !thread.archived) {
            let outcome = self.send(thread).await;
            if outcome != Outcome::Done {
                return outcome;
            }
        }
        Outcome::Done
    }

    /// Sends one thread's events after Gateway's last number.
    async fn send(&mut self, thread: &LoggedThread) -> Outcome {
        loop {
            let mark = self.mark(&thread.id);
            if mark >= thread.last_seq {
                return Outcome::Done;
            }
            let Ok(events) = self.log.events_after(&thread.id, mark, BATCH_EVENTS) else {
                return Outcome::Local;
            };
            if events.is_empty() {
                return Outcome::Done;
            }
            match self.remote.append(&thread.id, batch(&events)).await {
                Ok(Some(last)) => self.set_mark(&thread.id, last),
                Ok(None) => {
                    self.marks = None;
                    return Outcome::Off;
                }
                Err(error) => match code(&error) {
                    Some("seq_gap") => {
                        self.marks = None;
                        return Outcome::Resync;
                    }
                    // Gateway holds other events under these numbers: the machine's log wins, from 1 again.
                    Some("event_conflict") => {
                        if let Err(error) = self.remote.delete(&thread.id).await {
                            return failed(&error);
                        }
                        self.marks = None;
                        return Outcome::Resync;
                    }
                    // One thread Gateway will always refuse must not stop the others: skip it for this round.
                    _ if refuses_thread(&error) => {
                        if self.skipped.insert(thread.id.clone()) {
                            eprintln!("nexus-host: cloud history skips thread {}: {error}", thread.id);
                        }
                        return Outcome::Done;
                    }
                    _ => return failed(&error),
                },
            }
        }
    }

    fn mark(&self, id: &str) -> i64 {
        self.marks
            .as_ref()
            .and_then(|marks| marks.get(id).copied())
            .unwrap_or(0)
    }

    fn set_mark(&mut self, id: &str, last: i64) {
        if let Some(marks) = self.marks.as_mut() {
            marks.insert(id.to_string(), last);
        }
    }

    fn forget(&mut self, id: &str) {
        if let Some(marks) = self.marks.as_mut() {
            marks.remove(id);
        }
    }
}

fn code(error: &Error) -> Option<&str> {
    match error {
        Error::Core(error) => error.code.as_deref(),
        Error::Transport(_) => None,
    }
}

/// A permanent 4xx about this thread alone: not auth, not full, not a rate limit or timeout.
fn refuses_thread(error: &Error) -> bool {
    match error {
        Error::Core(error) => {
            error.code.as_deref() != Some("cloud_history_full")
                && (400..500).contains(&error.status)
                && !matches!(error.status, 401 | 403 | 404 | 408 | 413 | 429)
        }
        Error::Transport(_) => false,
    }
}

fn failed(error: &Error) -> Outcome {
    match error {
        Error::Core(error) if error.code.as_deref() == Some("cloud_history_full") => Outcome::Full,
        Error::Core(error) if matches!(error.status, 401 | 403 | 404) => Outcome::Refused,
        _ => Outcome::Failed,
    }
}

/// The wait after a round, and the backoff after it: doubles on each failure in a row, back to the first on success.
pub fn wait(outcome: Outcome, backoff: &mut Duration) -> Duration {
    match outcome {
        Outcome::Done => {
            *backoff = FIRST_BACKOFF;
            TICK
        }
        Outcome::Resync => Duration::ZERO,
        Outcome::Off | Outcome::Refused => RECHECK,
        Outcome::Full => FULL,
        Outcome::Failed | Outcome::Local => {
            let now = *backoff;
            *backoff = (now * 2).min(MOST_BACKOFF);
            now
        }
    }
}

fn said(outcome: Outcome) -> Option<&'static str> {
    match outcome {
        Outcome::Off => Some(OFF),
        Outcome::Failed => Some(RETRYING),
        Outcome::Refused => Some(REFUSED),
        Outcome::Full => Some(AT_CAP),
        Outcome::Local => Some(LOCAL),
        Outcome::Done | Outcome::Resync => None,
    }
}

/// Runs until the host stops (the caller aborts it). Logs each new state once, not each retry.
pub async fn run<L: Log, R: Remote>(log: L, remote: R) {
    let mut pusher = Pusher::new(log, remote);
    let mut backoff = FIRST_BACKOFF;
    let mut last_said = None;
    let mut resyncs = 0;
    loop {
        let outcome = pusher.round().await;
        let text = said(outcome);
        if text.is_some() && text != last_said {
            eprintln!("{}", text.unwrap_or_default());
        }
        if outcome != Outcome::Resync {
            last_said = text;
        }
        // Two disagreements in a row wait as a failure does, so a Gateway that keeps disagreeing is not called in a loop.
        resyncs = if outcome == Outcome::Resync { resyncs + 1 } else { 0 };
        let pause = if resyncs > 1 {
            wait(Outcome::Failed, &mut backoff)
        } else {
            wait(outcome, &mut backoff)
        };
        tokio::time::sleep(pause).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_client::CoreError;
    use std::sync::Mutex;

    /// A log in memory: each thread's event bodies, numbered from 1, and the archived ones.
    #[derive(Default)]
    struct Memory {
        threads: Mutex<Vec<(String, Vec<String>, bool)>>,
    }

    impl Memory {
        fn with(threads: &[(&str, usize, bool)]) -> Memory {
            Memory {
                threads: Mutex::new(
                    threads
                        .iter()
                        .map(|(id, count, archived)| {
                            let bodies = (1..=*count)
                                .map(|seq| format!(r#"{{"kind":"user","n":{seq}}}"#))
                                .collect();
                            (id.to_string(), bodies, *archived)
                        })
                        .collect(),
                ),
            }
        }
    }

    impl Log for Memory {
        fn threads(&self) -> Result<Vec<LoggedThread>> {
            Ok(self
                .threads
                .lock()
                .map_err(|_| "poisoned")?
                .iter()
                .filter(|(_, bodies, _)| !bodies.is_empty())
                .map(|(id, bodies, archived)| LoggedThread {
                    id: id.clone(),
                    last_seq: bodies.len() as i64,
                    archived: *archived,
                })
                .collect())
        }

        fn events_after(&self, thread_id: &str, after: i64, limit: usize) -> Result<Vec<EventText>> {
            let threads = self.threads.lock().map_err(|_| "poisoned")?;
            let bodies = threads
                .iter()
                .find(|(id, _, _)| id == thread_id)
                .map(|(_, bodies, _)| bodies.clone())
                .unwrap_or_default();
            Ok(bodies
                .into_iter()
                .enumerate()
                .map(|(index, body)| EventText {
                    seq: index as i64 + 1,
                    at: 1_000 + index as i64,
                    body,
                })
                .skip(after as usize)
                .take(limit)
                .collect())
        }
    }

    /// Gateway in memory, with the routes' rules: contiguous, idempotent, and off when told.
    #[derive(Default)]
    struct Fake {
        stored: Mutex<HashMap<String, Vec<Value>>>,
        off: Mutex<bool>,
        down: Mutex<bool>,
        calls: Mutex<Vec<String>>,
        bad: Mutex<Option<String>>,
    }

    fn refused(status: u16, code: &str) -> Error {
        Error::Core(Box::new(CoreError {
            status,
            message: code.into(),
            code: Some(code.into()),
            interval: None,
            details: None,
        }))
    }

    impl Remote for Fake {
        fn marks(&self) -> BoxFuture<'_, std::result::Result<Option<HashMap<String, i64>>, Error>> {
            Box::pin(async move {
                self.calls.lock().expect("calls").push("list".into());
                if *self.down.lock().expect("down") {
                    return Err(Error::Transport(nexus_client::Transport::Timeout));
                }
                if *self.off.lock().expect("off") {
                    return Ok(None);
                }
                let stored = self.stored.lock().expect("stored");
                Ok(Some(
                    stored
                        .iter()
                        .map(|(id, events)| (id.clone(), events.len() as i64))
                        .collect(),
                ))
            })
        }

        fn append<'a>(
            &'a self,
            thread_id: &'a str,
            events: Vec<Value>,
        ) -> BoxFuture<'a, std::result::Result<Option<i64>, Error>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("calls")
                    .push(format!("append {thread_id} {}", events.len()));
                if *self.down.lock().expect("down") {
                    return Err(Error::Transport(nexus_client::Transport::Timeout));
                }
                if *self.off.lock().expect("off") {
                    return Ok(None);
                }
                if self.bad.lock().expect("bad").as_deref() == Some(thread_id) {
                    return Err(refused(400, "invalid_request"));
                }
                let mut stored = self.stored.lock().expect("stored");
                let have = stored.entry(thread_id.to_string()).or_default();
                for event in events {
                    let seq = event["seq"].as_i64().expect("seq");
                    let at = usize::try_from(seq - 1).expect("from 1");
                    match have.get(at) {
                        Some(kept) if *kept == event => {}
                        Some(_) => return Err(refused(409, "event_conflict")),
                        None if at == have.len() => have.push(event),
                        None => return Err(refused(409, "seq_gap")),
                    }
                }
                Ok(Some(have.len() as i64))
            })
        }

        fn delete<'a>(&'a self, thread_id: &'a str) -> BoxFuture<'a, std::result::Result<(), Error>> {
            Box::pin(async move {
                self.calls.lock().expect("calls").push(format!("delete {thread_id}"));
                self.stored.lock().expect("stored").remove(thread_id);
                Ok(())
            })
        }
    }

    fn stored(pusher: &Pusher<Memory, Fake>, id: &str) -> usize {
        pusher.remote.stored.lock().expect("stored").get(id).map_or(0, Vec::len)
    }

    #[tokio::test]
    async fn backfills_from_gateways_last_number_in_batches() {
        let fake = Fake::default();
        // Gateway already has the first 10 of `a`, from before the machine slept.
        let earlier = Memory::with(&[("a", 10, false)]);
        let mut first = Pusher::new(earlier, fake);
        assert_eq!(first.round().await, Outcome::Done);
        let fake = first.remote;
        fake.calls.lock().expect("calls").clear();

        let mut pusher = Pusher::new(
            Memory::with(&[("a", 50, false), ("b", 3, false), ("off", 2, true)]),
            fake,
        );
        assert_eq!(pusher.round().await, Outcome::Done);
        assert_eq!(
            (stored(&pusher, "a"), stored(&pusher, "b"), stored(&pusher, "off")),
            (50, 3, 0)
        );
        let calls = pusher.remote.calls.lock().expect("calls").clone();
        assert_eq!(calls[0], "list");
        // From 11, 32 at a time; nothing for the archived thread.
        assert!(calls.contains(&"append a 32".to_string()) && calls.contains(&"append a 8".to_string()));
        assert!(!calls.iter().any(|call| call.contains("off")));

        // Nothing new: no call at all.
        pusher.remote.calls.lock().expect("calls").clear();
        assert_eq!(pusher.round().await, Outcome::Done);
        assert!(pusher.remote.calls.lock().expect("calls").is_empty());
    }

    #[tokio::test]
    async fn a_refused_thread_does_not_block_the_next_one() {
        let fake = Fake::default();
        *fake.bad.lock().expect("bad") = Some("a".into());
        let mut pusher = Pusher::new(Memory::with(&[("a", 3, false), ("b", 2, false)]), fake);
        assert_eq!(pusher.round().await, Outcome::Done);
        assert_eq!(stored(&pusher, "a"), 0);
        assert_eq!(stored(&pusher, "b"), 2);
        assert!(pusher.skipped.contains("a"));
        // Next round retries it, and still sends the rest.
        assert_eq!(pusher.round().await, Outcome::Done);
        assert_eq!(pusher.skipped.len(), 1);
    }

    #[tokio::test]
    async fn a_failure_waits_and_the_retry_sends_nothing_twice() {
        let fake = Fake::default();
        *fake.down.lock().expect("down") = true;
        let mut pusher = Pusher::new(Memory::with(&[("a", 4, false)]), fake);
        assert_eq!(pusher.round().await, Outcome::Failed);
        let mut backoff = FIRST_BACKOFF;
        assert_eq!(wait(Outcome::Failed, &mut backoff), Duration::from_secs(2));
        assert_eq!(wait(Outcome::Failed, &mut backoff), Duration::from_secs(4));
        for _ in 0..20 {
            wait(Outcome::Failed, &mut backoff);
        }
        assert_eq!(backoff, MOST_BACKOFF);
        assert_eq!(wait(Outcome::Done, &mut backoff), TICK);
        assert_eq!(backoff, FIRST_BACKOFF);

        *pusher.remote.down.lock().expect("down") = false;
        assert_eq!(pusher.round().await, Outcome::Done);
        // Marks lost (a restart): the batch goes again and Gateway stores it once.
        pusher.marks = Some(HashMap::new());
        assert_eq!(pusher.round().await, Outcome::Done);
        assert_eq!(stored(&pusher, "a"), 4);
    }

    #[tokio::test]
    async fn disagreements_read_the_marks_again() {
        let fake = Fake::default();
        let mut pusher = Pusher::new(Memory::with(&[("a", 3, false)]), fake);
        assert_eq!(pusher.round().await, Outcome::Done);
        // The owner deleted the thread's history in an app: Gateway now has none, the pusher thinks 3.
        pusher.remote.stored.lock().expect("stored").clear();
        pusher.log.threads.lock().expect("threads")[0].1.push("{}".into());
        assert_eq!(pusher.round().await, Outcome::Resync);
        assert_eq!(pusher.round().await, Outcome::Done);
        assert_eq!(stored(&pusher, "a"), 4);

        // Gateway holds other events under the same numbers: they go, and the machine's log is sent again.
        pusher
            .remote
            .stored
            .lock()
            .expect("stored")
            .insert("a".into(), vec![json!({ "seq": 1, "body": "other" })]);
        pusher.marks = Some(HashMap::new());
        assert_eq!(pusher.round().await, Outcome::Resync);
        assert_eq!(pusher.round().await, Outcome::Done);
        assert_eq!(stored(&pusher, "a"), 4);
        assert_eq!(
            pusher.remote.stored.lock().expect("stored")["a"][0]["body"],
            r#"{"kind":"user","n":1}"#
        );
    }

    #[tokio::test]
    async fn deleted_and_archived_threads_go_and_off_sends_nothing() {
        let fake = Fake::default();
        let mut pusher = Pusher::new(Memory::with(&[("a", 2, false), ("b", 2, false)]), fake);
        assert_eq!(pusher.round().await, Outcome::Done);
        {
            let mut threads = pusher.log.threads.lock().expect("threads");
            threads[0].2 = true;
            threads.remove(1);
        }
        assert_eq!(pusher.round().await, Outcome::Done);
        assert!(pusher.remote.stored.lock().expect("stored").is_empty());
        assert!(
            pusher
                .remote
                .calls
                .lock()
                .expect("calls")
                .contains(&"delete b".to_string())
        );

        *pusher.remote.off.lock().expect("off") = true;
        pusher.log.threads.lock().expect("threads")[0].2 = false;
        assert_eq!(pusher.round().await, Outcome::Off);
        assert_eq!(pusher.round().await, Outcome::Off);
        assert!(pusher.remote.stored.lock().expect("stored").is_empty());
        let mut backoff = FIRST_BACKOFF;
        assert_eq!(wait(Outcome::Off, &mut backoff), RECHECK);
    }

    #[test]
    fn large_bodies_go_as_their_size_and_batches_stay_under_the_limit() {
        let event = |seq: i64, bytes: usize| EventText {
            seq,
            at: 1,
            body: format!(r#"{{"t":"{}"}}"#, "a".repeat(bytes)),
        };
        let (big, _) = wire(&event(1, 300 * 1024));
        assert_eq!(big["body"], Value::Null);
        assert_eq!(big["omittedBytes"], 300 * 1024 + 8);
        // A control character escapes to six bytes: a body under Gateway's cap can still be too large for one call.
        let escaped = EventText {
            seq: 2,
            at: 1,
            body: "\u{1}".repeat(MAX_EVENT_BYTES),
        };
        assert_eq!(wire(&escaped).0["omittedBytes"], MAX_EVENT_BYTES);
        let events: Vec<EventText> = (1..=10).map(|seq| event(seq, 200 * 1024)).collect();
        let sent = batch(&events);
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().map(|value| value.to_string().len()).sum::<usize>() <= BATCH_BYTES);
        assert_eq!(batch(&events[..1]).len(), 1);
    }

    #[test]
    fn reads_gateways_answers() {
        let list = json!({ "enabled": true, "threads": [{ "threadId": "t_a", "lastSeq": 7 }] });
        assert_eq!(marks_of(&list), Some(HashMap::from([("t_a".to_string(), 7)])));
        assert_eq!(marks_of(&json!({ "enabled": false, "threads": [] })), None);
        assert_eq!(appended_of(&json!({ "enabled": true, "lastSeq": 9 })), Some(9));
        assert_eq!(appended_of(&json!({ "enabled": false, "lastSeq": null })), None);
    }
}
