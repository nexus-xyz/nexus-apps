//! `nexus agy`'s usage report. Antigravity runs on the person's Google plan, so its model calls never pass
//! through Gateway. `agy` writes each call's token counts to its conversation file,
//! `<profile>/conversations/<uuid>.db`: one `gen_metadata` row per call. Its `data` is `agy`'s
//! `CortexStepGeneratorMetadata` protobuf, and field 1.4 is `chat_model.usage` (`ModelUsageStats`): 1.4.2 input (cache
//! reads not included), 1.4.3 output (thinking included), 1.4.4 cache writes, 1.4.5 cache reads, 1.4.9 thinking and
//! 1.4.10 response. Field 1.17 (`chat_model.retry_infos`, one entry per attempt) is never read. After `agy` exits, the
//! CLI reads the rows this run added and posts one `api_request` record per row to the session's usage endpoint,
//! before `/end`.
//!
//! The rules both CLIs follow, and the cases that check them, are in
//! `services/frontend/core/interfaces/antigravity-usage.json`. Any doubt makes the whole run's usage unknown: nothing
//! is posted, and the session still ends.
//!
//! Any `agy` version is read, as the Claude Code and Codex readers read any version of theirs: `agy` updates itself
//! between runs, so the reader never asks for its version. The checks on the table's columns and on each row's fields
//! are what stop a changed format from being reported.
//!
//! Only `gen_metadata.idx` and the varint fields under 1.4 are read. The walker skips every other field by its length
//! without looking inside it, so no text of the conversation is ever read, logged or sent.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nexus_client::agent_entries::Agent;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

use crate::agents::is_uuid;
use crate::client::{Client, enc};

/// The largest count a row may carry; a larger one is taken for a misread field.
pub const MAX_COUNT: u64 = 10_000_000;

/// Records per usage request. Gateway remembers a session's last 512 request ids, so a body stays well under that.
pub const BATCH: usize = 200;

/// How long the whole report may take, every batch and the client's own retries included: `/end` waits no longer.
/// Also each request's timeout.
pub const REPORT_DEADLINE: Duration = Duration::from_secs(10);

/// How long a read waits for a lock another process holds on the conversation file.
const BUSY_TIMEOUT: Duration = Duration::from_millis(500);

/// How many of the newest conversations `--continue` may have resumed, read before the run.
const NEWEST_RESUMABLE: usize = 3;

/// The reason when the run's baseline is not known (the fixture's "no baseline").
const NO_BASELINE: &str = "the conversation's rows before the run are not known";

/// Whether `nexus <agent>` counts a run's calls with this module, from the agent's conversation file after it exits.
/// The run reads this before it starts the agent, and `agent_capabilities` reads it for the agent's usage source.
pub fn counts_from_conversation(agent: Agent) -> bool {
    match agent {
        Agent::Antigravity => true,
        Agent::Claude | Agent::Codex | Agent::Cursor | Agent::Opencode => false,
    }
}

/// One model call this run made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub idx: i64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

/// What the run's conversation file says about its usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Usage {
    /// Every call this run made, in order; empty when it made none.
    Known(Vec<Record>),
    /// Why nothing is reported.
    Unknown(String),
}

impl Usage {
    fn unknown(reason: impl Into<String>) -> Usage {
        Usage::Unknown(reason.into())
    }
}

/// The conversations folder as it was before `agy` started: which files it held, and the highest `idx` of each
/// conversation the run may resume.
#[derive(Debug, Default)]
pub struct Before {
    conversations: Option<PathBuf>,
    /// The conversation ids with a file before the run; `None` when the folder could not be listed.
    files: Option<BTreeSet<String>>,
    baselines: HashMap<String, i64>,
}

impl Before {
    /// Read before `agy` starts. `named` is the conversation `--conversation <id>` resumes; `continued` says the run
    /// resumes the latest one (`--continue`), which is one of the [`NEWEST_RESUMABLE`] files changed last.
    pub fn take(conversations: Option<PathBuf>, named: Option<&str>, continued: bool) -> Before {
        let Some(dir) = conversations else {
            return Before::default();
        };
        // Every conversation's name counts as there before the run, whatever its metadata says: one missed here would
        // read as created by the run, and its whole history would be reported. The times only order `--continue`'s.
        let listed: Option<Vec<(Option<std::time::SystemTime>, String)>> = match std::fs::read_dir(&dir) {
            Ok(entries) => entries
                .map(|entry| {
                    let entry = entry.ok()?;
                    let name = entry.file_name().into_string().ok();
                    let Some(id) = name
                        .as_deref()
                        .and_then(|name| name.strip_suffix(".db"))
                        .filter(|id| is_uuid(id))
                    else {
                        return Some(None);
                    };
                    let modified = entry.metadata().and_then(|meta| meta.modified()).ok();
                    Some(Some((modified, id.to_string())))
                })
                // An entry the folder could not list makes the listing unknown.
                .collect::<Option<Vec<_>>>()
                .map(|entries| entries.into_iter().flatten().collect()),
            // `agy` has never run here: whatever conversation the run writes, it creates.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
            Err(_) => None,
        };
        let resumable: Vec<String> = match (named, continued, &listed) {
            (Some(id), _, _) => vec![id.to_string()],
            (None, true, Some(listed)) => {
                let mut newest = listed.clone();
                // Newest first; a file without a time sorts last.
                newest.sort_by(|a, b| b.0.cmp(&a.0));
                newest.into_iter().take(NEWEST_RESUMABLE).map(|(_, id)| id).collect()
            }
            _ => Vec::new(),
        };
        let baselines = resumable
            .into_iter()
            .filter_map(|id| {
                let max = max_idx(&file_of(&dir, &id))?;
                Some((id, max))
            })
            .collect();
        Before {
            files: listed.map(|listed| listed.into_iter().map(|(_, id)| id).collect()),
            conversations: Some(dir),
            baselines,
        }
    }

    /// The highest `idx` conversation `id` had before the run: read then for a conversation it resumed, -1 for one
    /// whose file the run created. `None` when neither is known.
    pub fn baseline(&self, id: &str) -> Option<i64> {
        if let Some(max) = self.baselines.get(id) {
            return Some(*max);
        }
        match &self.files {
            Some(files) if !files.contains(id) => Some(-1),
            _ => None,
        }
    }
}

fn file_of(conversations: &Path, id: &str) -> PathBuf {
    conversations.join(format!("{id}.db"))
}

/// After `agy` exits: reads the run's usage from `conversation`'s file and posts it to the session with a client from
/// `connect` (the CLI's client with the given timeout, on the key that opened the session). A failure to read or to
/// post never stops the caller: the answer only says what happened. Unknown when not every record landed.
pub fn report(
    connect: impl Fn(Duration) -> Client + Send + 'static,
    session_id: &str,
    before: &Before,
    conversation: Option<&str>,
) -> Usage {
    let (Some(dir), Some(id)) = (before.conversations.as_deref(), conversation) else {
        return Usage::unknown("the run's conversation is not known");
    };
    let Some(baseline) = before.baseline(id) else {
        return Usage::unknown(NO_BASELINE);
    };
    let usage = read(&file_of(dir, id), Some(baseline));
    // A report that did not fully land leaves Gateway with part of the run: its count would read as the whole.
    if let Usage::Known(records) = &usage
        && !post(connect, session_id, id, records)
    {
        return Usage::unknown("not every record reached Gateway");
    }
    usage
}

/// The calls above `baseline` in the conversation file at `path`. Rows at or below the baseline are never decoded.
/// One row in doubt makes the whole run unknown: a partial report would undercount.
pub fn read(path: &Path, baseline: Option<i64>) -> Usage {
    let Some(baseline) = baseline else {
        return Usage::unknown(NO_BASELINE);
    };
    let rows = match new_rows(path, baseline) {
        Ok(rows) => rows,
        Err(reason) => return Usage::Unknown(reason),
    };
    let mut records = Vec::with_capacity(rows.len());
    let mut last = baseline;
    for (idx, row) in rows {
        if idx <= last {
            return Usage::unknown("the conversation's rows are not in increasing idx order");
        }
        last = idx;
        let Ok(row) = row else {
            return Usage::unknown(format!("row {idx} could not be decoded"));
        };
        let usage = row.usage;
        let Some(input) = usage.input else {
            return Usage::unknown(format!("row {idx} has no input count (1.4.2)"));
        };
        let Some(output) = usage.output else {
            return Usage::unknown(format!("row {idx} has no output count (1.4.3)"));
        };
        let (cache_read, cache_write) = (usage.cache_read.unwrap_or(0), usage.cache_write.unwrap_or(0));
        let counts = [
            Some(input),
            Some(output),
            Some(cache_read),
            Some(cache_write),
            usage.thinking,
            usage.response,
        ];
        if counts.into_iter().flatten().any(|count| count > MAX_COUNT) {
            return Usage::unknown(format!("row {idx} has a count above 10,000,000"));
        }
        // `agy`'s output count is its thinking plus its response: a row where both are present and the sum differs
        // was not written the way this reader reads it.
        if let (Some(thinking), Some(response)) = (usage.thinking, usage.response)
            && thinking + response != output
        {
            return Usage::unknown(format!(
                "row {idx}'s output count is not its thinking plus response counts"
            ));
        }
        records.push(Record {
            idx,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_write,
        });
    }
    Usage::Known(records)
}

/// Opens `agy`'s file without writing to it or taking a write lock, and with its write-ahead log read too (no
/// `immutable=1`, which would skip the log's newest rows).
fn open(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    db.busy_timeout(BUSY_TIMEOUT)?;
    Ok(db)
}

/// The columns of `gen_metadata` this reader knows, in order.
const COLUMNS: [&str; 3] = ["idx", "data", "size"];

/// Whether the file has `gen_metadata` with the known columns; the reason when not.
fn check_table(db: &Connection) -> Result<(), String> {
    let columns: Vec<String> = db
        .prepare("PRAGMA table_info(gen_metadata)")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<String>>>()
        })
        .map_err(|_| "the conversation file could not be read".to_string())?;
    if columns.is_empty() {
        return Err("the conversation file has no gen_metadata table".into());
    }
    if columns != COLUMNS {
        return Err("gen_metadata's columns are not the known ones".into());
    }
    Ok(())
}

/// The highest `idx` in the file at `path`, -1 when it has no rows; `None` when it cannot be read.
fn max_idx(path: &Path) -> Option<i64> {
    let db = open(path).ok()?;
    check_table(&db).ok()?;
    let max: Option<i64> = db
        .query_row("SELECT MAX(idx) FROM gen_metadata", [], |row| row.get(0))
        .ok()?;
    Some(max.unwrap_or(-1))
}

/// Each row above `baseline`: its `idx` and its decoded `data`, in the order the rows were written (`rowid`, so
/// `idx` order is checked, not imposed). A row whose `idx` is NULL is selected too, and voids the read: it cannot be
/// placed against the baseline. `data` is decoded as it is read, so no row's bytes are kept. One read transaction,
/// so the highest `idx` and the rows come from the same state of the file.
fn new_rows(path: &Path, baseline: i64) -> Result<Vec<NewRow>, String> {
    let unreadable = |_| "the conversation file could not be read".to_string();
    let mut db = open(path).map_err(|_| "the conversation file could not be opened".to_string())?;
    let read = db.transaction().map_err(unreadable)?;
    check_table(&read)?;
    let max: Option<i64> = read
        .query_row("SELECT MAX(idx) FROM gen_metadata", [], |row| row.get(0))
        .map_err(unreadable)?;
    if max.unwrap_or(-1) < baseline {
        return Err("the conversation's rows end below the baseline".into());
    }
    let mut statement = read
        .prepare("SELECT idx, data FROM gen_metadata WHERE idx > ?1 OR idx IS NULL ORDER BY rowid")
        .map_err(unreadable)?;
    let rows = statement
        .query_map([baseline], |row| {
            let idx = row.get::<_, i64>(0)?;
            // `data` that is not a blob does not decode either.
            let data = row.get_ref(1)?.as_blob().map_err(|_| Malformed);
            Ok((idx, data.and_then(decode)))
        })
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
        .map_err(unreadable)?;
    Ok(rows)
}

/// A row above the baseline: its `idx` and what its `data` decoded to.
type NewRow = (i64, Result<Row, Malformed>);

/// The counts of a row's usage message, 1.4 (`ModelUsageStats`), by its field numbers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Counts {
    /// 2, `input_tokens`: cache reads not included.
    input: Option<u64>,
    /// 3, `output_tokens`: thinking included.
    output: Option<u64>,
    /// 4, `cache_write_tokens`.
    cache_write: Option<u64>,
    /// 5, `cache_read_tokens`.
    cache_read: Option<u64>,
    /// 9, `thinking_output_tokens`.
    thinking: Option<u64>,
    /// 10, `response_output_tokens`.
    response: Option<u64>,
}

/// What a row's `data` holds of its usage.
#[derive(Debug, Default, PartialEq, Eq)]
struct Row {
    usage: Counts,
}

/// `data` that is not a well-formed protobuf on the path the reader walks.
#[derive(Debug, PartialEq, Eq)]
struct Malformed;

/// One field's value as the walker sees it: a varint, the bytes of a length-delimited field (never looked into
/// unless it is on the usage path), or a fixed-size value it skipped.
enum Wire<'a> {
    Varint(u64),
    Delimited(&'a [u8]),
    Fixed,
}

/// A protobuf message's fields, one at a time.
struct Fields<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Fields<'a> {
    fn of(bytes: &'a [u8]) -> Fields<'a> {
        Fields { bytes, at: 0 }
    }

    fn varint(&mut self) -> Result<u64, Malformed> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.bytes.get(self.at).ok_or(Malformed)?;
            self.at += 1;
            // The tenth byte holds only the 64th bit.
            if shift == 63 && byte > 1 {
                return Err(Malformed);
            }
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Malformed)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], Malformed> {
        let end = self.at.checked_add(len).ok_or(Malformed)?;
        let taken = self.bytes.get(self.at..end).ok_or(Malformed)?;
        self.at = end;
        Ok(taken)
    }

    /// The next field's number and value; `None` at the end. Groups (wire types 3 and 4) are malformed here: no
    /// field on the usage path is one, and a reader cannot skip one without parsing it.
    fn next(&mut self) -> Result<Option<(u64, Wire<'a>)>, Malformed> {
        if self.at == self.bytes.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        let field = key >> 3;
        if field == 0 {
            return Err(Malformed);
        }
        let value = match key & 7 {
            0 => Wire::Varint(self.varint()?),
            1 => {
                self.take(8)?;
                Wire::Fixed
            }
            2 => {
                let len = usize::try_from(self.varint()?).map_err(|_| Malformed)?;
                Wire::Delimited(self.take(len)?)
            }
            5 => {
                self.take(4)?;
                Wire::Fixed
            }
            _ => return Err(Malformed),
        };
        Ok(Some((field, value)))
    }
}

/// Walks `data` into field 1 -> 4 (`chat_model.usage`) only; every other field, `retry_infos` (1.17) included, is
/// skipped by its length. A message on the path met twice merges as protobuf merges it, but a count may appear only
/// once ([`counts`]). A field on the path with another wire type is malformed.
fn decode(data: &[u8]) -> Result<Row, Malformed> {
    let mut row = Row::default();
    let mut top = Fields::of(data);
    while let Some((field, value)) = top.next()? {
        match (field, value) {
            (1, Wire::Delimited(step)) => {
                let mut step = Fields::of(step);
                while let Some((field, value)) = step.next()? {
                    match (field, value) {
                        (4, Wire::Delimited(usage)) => counts(usage, &mut row.usage)?,
                        (4, _) => return Err(Malformed),
                        _ => {}
                    }
                }
            }
            (1, _) => return Err(Malformed),
            _ => {}
        }
    }
    Ok(row)
}

/// The counts of one usage message into `into`, by [`Counts`]' field numbers. A count met twice, in one message or in
/// two copies of 1.4 that protobuf would merge, is malformed: which one holds is not known.
fn counts(message: &[u8], into: &mut Counts) -> Result<(), Malformed> {
    let mut fields = Fields::of(message);
    while let Some((field, value)) = fields.next()? {
        let slot = match field {
            2 => &mut into.input,
            3 => &mut into.output,
            4 => &mut into.cache_write,
            5 => &mut into.cache_read,
            9 => &mut into.thinking,
            10 => &mut into.response,
            _ => continue,
        };
        match value {
            Wire::Varint(count) if slot.is_none() => *slot = Some(count),
            _ => return Err(Malformed),
        }
    }
    Ok(())
}

/// The OTLP logs body for `records`: one `api_request` per call, with no model, no cost and no time. Gateway counts
/// each unpriced, on the `unknown` model, on the day it arrives, once per `request_id`.
pub fn body(conversation: &str, records: &[Record]) -> Value {
    let attribute = |key: &str, value: Value| json!({ "key": key, "value": value });
    let records: Vec<Value> = records
        .iter()
        .map(|record| {
            json!({
                "body": { "stringValue": "api_request" },
                "attributes": [
                    attribute("event.name", json!({ "stringValue": "api_request" })),
                    attribute(
                        "request_id",
                        json!({ "stringValue": format!("antigravity:{conversation}:{}", record.idx) }),
                    ),
                    attribute("input_tokens", json!({ "intValue": record.input_tokens })),
                    attribute("output_tokens", json!({ "intValue": record.output_tokens })),
                    attribute("cache_read_tokens", json!({ "intValue": record.cache_read_tokens })),
                    attribute("cache_creation_tokens", json!({ "intValue": record.cache_creation_tokens })),
                ],
            })
        })
        .collect();
    json!({ "resourceLogs": [{ "scopeLogs": [{ "logRecords": records }] }] })
}

/// Posts `records` to the session, [`BATCH`] per request, within [`REPORT_DEADLINE`] in all. The client sends a batch
/// up to three times under one idempotency key when Gateway answers 502, 503 or 504 or the connection drops after
/// sending (nexus-client's `attempts` and `retryable`); any other failure, or the third, ends the report, quietly.
/// Gateway counts a `request_id` once, so a batch sent again counts nothing twice. The requests run on their own
/// thread, so `/end` waits for them no longer than the deadline. No batch starts after it, and each batch's client
/// times out at it; only a retry the client starts just before it can still reach Gateway after `/end`. Answers
/// whether all landed in time.
pub fn post(
    connect: impl Fn(Duration) -> Client + Send + 'static,
    session_id: &str,
    conversation: &str,
    records: &[Record],
) -> bool {
    post_within(connect, session_id, conversation, records, REPORT_DEADLINE)
}

fn post_within(
    connect: impl Fn(Duration) -> Client + Send + 'static,
    session_id: &str,
    conversation: &str,
    records: &[Record],
    deadline: Duration,
) -> bool {
    let started = Instant::now();
    let path = format!("/v1/agents/sessions/{}/usage", enc(session_id));
    let bodies: Vec<Value> = records.chunks(BATCH).map(|batch| body(conversation, batch)).collect();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let landed = bodies.into_iter().all(|body| {
            let left = deadline.saturating_sub(started.elapsed());
            !left.is_zero() && connect(left).post(&path, body).is_ok()
        });
        let _ = sender.send(landed);
    });
    receiver
        .recv_timeout(deadline.saturating_sub(started.elapsed()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// The cases both CLIs read.
    const FIXTURE: &str = include_str!("../tests/fixtures/interfaces/antigravity-usage.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).expect("antigravity-usage.json")
    }

    /// A folder of its own per test (tests run in parallel), removed when the value drops.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!("nexus-agy-usage-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("dir");
            TempDir(dir)
        }

        fn join(&self, name: impl AsRef<Path>) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn varint(mut value: u64, out: &mut Vec<u8>) {
        while value >= 0x80 {
            out.push((value as u8 & 0x7f) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    /// A protobuf message to encode: varints, nested messages, and the text and fixed-size fields the reader must
    /// skip without reading.
    enum Node {
        Varint(u64),
        Message(BTreeMap<u64, Node>),
        Text(&'static str),
        Fixed32(u32),
    }

    fn encode(message: &BTreeMap<u64, Node>) -> Vec<u8> {
        let mut out = Vec::new();
        for (field, node) in message {
            match node {
                Node::Varint(value) => {
                    varint(field << 3, &mut out);
                    varint(*value, &mut out);
                }
                Node::Message(inner) => {
                    let bytes = encode(inner);
                    varint((field << 3) | 2, &mut out);
                    varint(bytes.len() as u64, &mut out);
                    out.extend(bytes);
                }
                Node::Text(text) => {
                    varint((field << 3) | 2, &mut out);
                    varint(text.len() as u64, &mut out);
                    out.extend(text.as_bytes());
                }
                Node::Fixed32(value) => {
                    varint((field << 3) | 5, &mut out);
                    out.extend(value.to_le_bytes());
                }
            }
        }
        out
    }

    fn message(node: &mut Node) -> &mut BTreeMap<u64, Node> {
        match node {
            Node::Message(inner) => inner,
            _ => panic!("not a message"),
        }
    }

    /// A row's `fields` (paths such as `"1.4.3"`) as protobuf bytes, with text at 2 and 1.3, and, when the row has a
    /// `retry_infos` (1.17), text at 1.17.1.5 and a fixed32 at 1.17.3: all of them the reader must skip.
    fn row_bytes(fields: &serde_json::Map<String, Value>) -> Vec<u8> {
        let mut root = Node::Message(BTreeMap::new());
        for (path, value) in fields {
            let numbers: Vec<u64> = path.split('.').map(|n| n.parse().expect("field number")).collect();
            let (leaf, parents) = numbers.split_last().expect("a path");
            let mut at = &mut root;
            for parent in parents {
                at = message(at)
                    .entry(*parent)
                    .or_insert_with(|| Node::Message(BTreeMap::new()));
            }
            message(at).insert(*leaf, Node::Varint(value.as_u64().expect("a count")));
        }
        let top = message(&mut root);
        top.insert(2, Node::Text("the person's prompt"));
        let step = message(top.entry(1).or_insert_with(|| Node::Message(BTreeMap::new())));
        step.insert(3, Node::Text("the model's answer"));
        if let Some(copies) = step.get_mut(&17) {
            let copies = message(copies);
            copies.insert(3, Node::Fixed32(7));
            copies.insert(
                1,
                Node::Message(BTreeMap::from([(5, Node::Text("gemini-3.8-flash-high"))])),
            );
        }
        encode(message(&mut root))
    }

    /// A conversation file with `rows` (idx, data) in insertion order, as `agy` lays it out.
    fn conversation(path: &Path, rows: &[(i64, Vec<u8>)]) {
        let db = Connection::open(path).expect("open");
        db.execute_batch("CREATE TABLE gen_metadata (idx INTEGER, data BLOB, size INTEGER)")
            .expect("table");
        for (idx, data) in rows {
            db.execute(
                "INSERT INTO gen_metadata (idx, data, size) VALUES (?1, ?2, ?3)",
                rusqlite::params![idx, data, data.len() as i64],
            )
            .expect("row");
        }
    }

    fn usage_row(input: u64, output: u64) -> Vec<u8> {
        let fields = json!({ "1.4.2": input, "1.4.3": output });
        row_bytes(fields.as_object().expect("fields"))
    }

    fn record(idx: i64, input: u64, output: u64) -> Record {
        Record {
            idx,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        }
    }

    #[test]
    fn every_fixture_case_reads_as_it_expects() {
        let dir = TempDir::new();
        let fixture = fixture();
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(!cases.is_empty());
        for (n, case) in cases.iter().enumerate() {
            let name = case["name"].as_str().expect("name");
            let rows: Vec<(i64, Vec<u8>)> = case["rows"]
                .as_array()
                .expect("rows")
                .iter()
                .map(|row| {
                    let data = match row.get("hex").and_then(Value::as_str) {
                        Some(hex) => hex::decode(hex).expect("hex"),
                        None => row_bytes(row["fields"].as_object().expect("fields")),
                    };
                    (row["idx"].as_i64().expect("idx"), data)
                })
                .collect();
            let path = dir.join(format!("{n}.db"));
            conversation(&path, &rows);
            let read = read(&path, case["baseline"].as_i64());
            let expect = &case["expect"];
            let wanted = match expect["usage"].as_str() {
                Some("known") => Usage::Known(
                    expect["records"]
                        .as_array()
                        .expect("records")
                        .iter()
                        .map(|record| Record {
                            idx: record["idx"].as_i64().expect("idx"),
                            input_tokens: record["input_tokens"].as_u64().expect("input"),
                            output_tokens: record["output_tokens"].as_u64().expect("output"),
                            cache_read_tokens: record["cache_read_tokens"].as_u64().expect("cache read"),
                            cache_creation_tokens: record["cache_creation_tokens"].as_u64().expect("cache write"),
                        })
                        .collect(),
                ),
                Some("unknown") => Usage::Unknown(expect["reason"].as_str().expect("reason").to_string()),
                other => panic!("{name}: usage {other:?}"),
            };
            assert_eq!(read, wanted, "{name}");
        }
    }

    /// The body is the one nexus-gateway's tests/subscription_usage_days.rs posts for "two calls".
    #[test]
    fn body_is_the_record_gateway_counts() {
        let records = [
            record(0, 8882, 96),
            Record {
                cache_read_tokens: 6838,
                cache_creation_tokens: 120,
                ..record(1, 9185, 104)
            },
        ];
        let id = "7c1e2f4a-93b5-4d06-8a1f-2b3c4d5e6f70";
        let sent = |idx: i64, input: u64, output: u64, cache_read: u64, cache_write: u64| {
            json!({
                "body": { "stringValue": "api_request" },
                "attributes": [
                    { "key": "event.name", "value": { "stringValue": "api_request" } },
                    { "key": "request_id", "value": { "stringValue": format!("antigravity:{id}:{idx}") } },
                    { "key": "input_tokens", "value": { "intValue": input } },
                    { "key": "output_tokens", "value": { "intValue": output } },
                    { "key": "cache_read_tokens", "value": { "intValue": cache_read } },
                    { "key": "cache_creation_tokens", "value": { "intValue": cache_write } },
                ],
            })
        };
        assert_eq!(
            body(id, &records),
            json!({ "resourceLogs": [{ "scopeLogs": [{ "logRecords": [
                sent(0, 8882, 96, 0, 0),
                sent(1, 9185, 104, 6838, 120),
            ] }] }] })
        );
    }

    #[test]
    fn walker_skips_every_other_field_and_refuses_malformed_bytes() {
        let fields = json!({
            "1.4.1": 312, "1.4.2": 5, "1.4.3": 6, "1.4.4": 7, "1.4.5": 8, "1.4.9": 4, "1.4.10": 2,
            "1.17.2.2": 99, "1.17.2.3": 98,
        });
        let row = decode(&row_bytes(fields.as_object().expect("fields"))).expect("decodes");
        assert_eq!(
            row,
            Row {
                usage: Counts {
                    input: Some(5),
                    output: Some(6),
                    cache_write: Some(7),
                    cache_read: Some(8),
                    thinking: Some(4),
                    response: Some(2),
                },
            }
        );
        // Empty data: no counts, and nothing malformed.
        assert_eq!(decode(&[]), Ok(Row::default()));
        for bad in [
            // A length past the end.
            &[0x12, 0x05, b'a'][..],
            // A varint that never ends.
            &[0x08, 0xff, 0xff][..],
            // A varint past 64 bits.
            &[0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01][..],
            // Field number 0.
            &[0x00, 0x01][..],
            // A group (wire type 3).
            &[0x1b][..],
            // Field 1 as a varint, not a message.
            &[0x08, 0x01][..],
            // 1.4 as a varint.
            &[0x0a, 0x02, 0x20, 0x01][..],
            // 1.4.2 as a length-delimited field.
            &[0x0a, 0x04, 0x22, 0x02, 0x12, 0x00][..],
            // The fixture's malformed row.
            &[0xff, 0xff, 0xff, 0xff][..],
            // 1.4.2 twice in one message.
            &[0x0a, 0x06, 0x22, 0x04, 0x10, 0x07, 0x10, 0x08][..],
            // 1.4 twice, each with a 1.4.2.
            &[0x0a, 0x08, 0x22, 0x02, 0x10, 0x07, 0x22, 0x02, 0x10, 0x08][..],
            // 1.4.5 (cache reads) twice in one message.
            &[0x0a, 0x06, 0x22, 0x04, 0x28, 0x07, 0x28, 0x08][..],
        ] {
            assert_eq!(decode(bad), Err(Malformed), "{bad:?}");
        }
        // The same counts once each decode: above, only the repetition was malformed.
        assert_eq!(
            decode(&[0x0a, 0x04, 0x22, 0x02, 0x10, 0x07]).map(|row| row.usage.input),
            Ok(Some(7))
        );
        // retry_infos (1.17) is skipped whole: two entries, each with its own usage, decode to nothing.
        assert_eq!(
            decode(&[
                0x0a, 0x0b, 0x8a, 0x01, 0x08, 0x12, 0x02, 0x18, 0x01, 0x12, 0x02, 0x18, 0x01,
            ]),
            Ok(Row::default())
        );
        // So does a 1.17 that is not even a message.
        assert_eq!(decode(&[0x0a, 0x03, 0x88, 0x01, 0x05]), Ok(Row::default()));
    }

    #[test]
    fn output_is_checked_against_thinking_plus_response_only_when_both_are_there() {
        let dir = TempDir::new();
        for (n, (fields, wanted)) in [
            (
                json!({ "1.4.2": 5, "1.4.3": 6, "1.4.9": 4, "1.4.10": 2 }),
                Usage::Known(vec![record(0, 5, 6)]),
            ),
            (
                json!({ "1.4.2": 5, "1.4.3": 6, "1.4.9": 4, "1.4.10": 3 }),
                Usage::unknown("row 0's output count is not its thinking plus response counts"),
            ),
            (
                json!({ "1.4.2": 5, "1.4.3": 6, "1.4.9": 9 }),
                Usage::Known(vec![record(0, 5, 6)]),
            ),
            (
                json!({ "1.4.2": 5, "1.4.3": 6, "1.4.10": 9 }),
                Usage::Known(vec![record(0, 5, 6)]),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let path = dir.join(format!("{n}.db"));
            conversation(&path, &[(0, row_bytes(fields.as_object().expect("fields")))]);
            assert_eq!(read(&path, Some(-1)), wanted, "{fields}");
        }
    }

    #[test]
    fn report_gives_up_at_its_deadline() {
        // A Gateway that takes the request and never answers.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let timeouts: std::sync::Arc<std::sync::Mutex<Vec<Duration>>> = Default::default();
        let seen = timeouts.clone();
        let connect = move |timeout| {
            seen.lock().expect("timeouts").push(timeout);
            crate::cli::cli_client_with_timeout(&url, Some("nx_test"), timeout)
        };
        // Two batches: the second never starts.
        let records: Vec<Record> = (0..=BATCH as i64).map(|idx| record(idx, 1, 2)).collect();
        let started = Instant::now();
        let deadline = Duration::from_millis(300);
        assert!(!post_within(connect, "ags_1", "id", &records, deadline));
        let took = started.elapsed();
        assert!(took >= deadline && took < Duration::from_secs(5), "{took:?}");
        // The first batch's request times out at the deadline too, and is not sent again.
        std::thread::sleep(Duration::from_millis(500));
        let timeouts = timeouts.lock().expect("timeouts").clone();
        assert_eq!(timeouts.len(), 1, "{timeouts:?}");
        assert!(timeouts[0] <= deadline, "{timeouts:?}");
        drop(listener);
    }

    #[test]
    fn files_that_cannot_be_read_as_agys_make_usage_unknown() {
        let dir = TempDir::new();
        let unknown = |path: &Path| read(path, Some(-1));
        assert_eq!(
            unknown(&dir.join("missing.db")),
            Usage::unknown("the conversation file could not be opened")
        );

        let empty = dir.join("empty.db");
        std::fs::write(&empty, b"").expect("empty");
        assert_eq!(
            unknown(&empty),
            Usage::unknown("the conversation file has no gen_metadata table")
        );

        let other = dir.join("other.db");
        Connection::open(&other)
            .expect("open")
            .execute_batch("CREATE TABLE gen_metadata (idx INTEGER, data BLOB)")
            .expect("table");
        assert_eq!(
            unknown(&other),
            Usage::unknown("gen_metadata's columns are not the known ones")
        );

        let text = dir.join("text.db");
        std::fs::write(&text, b"not a database at all, just text").expect("text");
        assert!(matches!(unknown(&text), Usage::Unknown(_)));

        let garbled = dir.join("garbled.db");
        conversation(&garbled, &[(0, usage_row(1, 2)), (1, vec![0xff, 0xff])]);
        assert_eq!(unknown(&garbled), Usage::unknown("row 1 could not be decoded"));

        let repeated = dir.join("repeated.db");
        conversation(&repeated, &[(0, usage_row(1, 2)), (0, usage_row(3, 4))]);
        assert_eq!(
            unknown(&repeated),
            Usage::unknown("the conversation's rows are not in increasing idx order")
        );
        // Written out of order: caught even when an index on idx would hand the rows back sorted.
        let backwards = dir.join("backwards.db");
        conversation(&backwards, &[(1, usage_row(1, 2)), (0, usage_row(3, 4))]);
        Connection::open(&backwards)
            .expect("open")
            .execute_batch("CREATE INDEX by_idx ON gen_metadata (idx)")
            .expect("index");
        assert_eq!(
            unknown(&backwards),
            Usage::unknown("the conversation's rows are not in increasing idx order")
        );

        let no_idx = dir.join("no-idx.db");
        conversation(&no_idx, &[(0, usage_row(1, 2))]);
        Connection::open(&no_idx)
            .expect("open")
            .execute(
                "INSERT INTO gen_metadata (idx, data, size) VALUES (NULL, ?1, 1)",
                [usage_row(3, 4)],
            )
            .expect("row");
        assert_eq!(
            unknown(&no_idx),
            Usage::unknown("the conversation file could not be read")
        );

        let not_a_blob = dir.join("not-a-blob.db");
        conversation(&not_a_blob, &[]);
        Connection::open(&not_a_blob)
            .expect("open")
            .execute_batch("INSERT INTO gen_metadata (idx, data, size) VALUES (0, 'text', 4)")
            .expect("row");
        assert_eq!(unknown(&not_a_blob), Usage::unknown("row 0 could not be decoded"));
    }

    #[test]
    fn busy_file_makes_usage_unknown_after_the_busy_timeout() {
        let dir = TempDir::new();
        let path = dir.join("busy.db");
        conversation(&path, &[(0, usage_row(1, 2))]);
        let writer = Connection::open(&path).expect("open");
        writer.execute_batch("BEGIN EXCLUSIVE").expect("lock");
        let started = Instant::now();
        assert_eq!(
            read(&path, Some(-1)),
            Usage::unknown("the conversation file could not be read")
        );
        assert!(started.elapsed() >= BUSY_TIMEOUT, "{:?}", started.elapsed());
        writer.execute_batch("ROLLBACK").expect("unlock");
        assert_eq!(read(&path, Some(-1)), Usage::Known(vec![record(0, 1, 2)]));
    }

    #[test]
    fn baseline_is_minus_one_for_a_file_the_run_created_and_the_highest_idx_for_one_it_resumed() {
        let temp = TempDir::new();
        let dir = temp.join("conversations");
        let ids = [
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
            "33333333-3333-4333-8333-333333333333",
            "44444444-4444-4444-8444-444444444444",
        ];
        // No folder yet: whatever the run writes is new.
        let first = Before::take(Some(dir.clone()), None, false);
        assert_eq!(first.baseline(ids[0]), Some(-1));

        std::fs::create_dir_all(&dir).expect("dir");
        // Written oldest first, each with its index + 1 rows; the times step so the order is clear.
        for (n, id) in ids.iter().enumerate() {
            let rows: Vec<(i64, Vec<u8>)> = (0..=n as i64).map(|idx| (idx, usage_row(1, 2))).collect();
            conversation(&file_of(&dir, id), &rows);
            let at = std::time::SystemTime::now() - Duration::from_secs(100 - n as u64);
            std::fs::File::options()
                .write(true)
                .open(file_of(&dir, id))
                .and_then(|file| file.set_modified(at))
                .expect("time");
        }
        let new = Before::take(Some(dir.clone()), None, false);
        assert_eq!(new.baseline("55555555-5555-4555-8555-555555555555"), Some(-1));
        // A file there before a new run: not created by it, and its rows before the run were not read.
        assert_eq!(new.baseline(ids[3]), None);

        let named = Before::take(Some(dir.clone()), Some(ids[1]), false);
        assert_eq!(named.baseline(ids[1]), Some(1));
        assert_eq!(named.baseline(ids[2]), None);

        // `--continue`: the three changed last, never the oldest.
        let continued = Before::take(Some(dir.clone()), None, true);
        assert_eq!(continued.baseline(ids[0]), None);
        assert_eq!(continued.baseline(ids[1]), Some(1));
        assert_eq!(continued.baseline(ids[2]), Some(2));
        assert_eq!(continued.baseline(ids[3]), Some(3));

        // A name there before the run counts as there, even when it is not a file: never taken for one the run made.
        let odd = "66666666-6666-4666-8666-666666666666";
        std::fs::create_dir_all(file_of(&dir, odd)).expect("dir");
        assert_eq!(Before::take(Some(dir.clone()), None, false).baseline(odd), None);

        // No profile folder: nothing is known.
        assert_eq!(Before::take(None, None, false).baseline(ids[0]), None);
    }
}
