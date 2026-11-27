//! The workbench's SQLite file (`workbench.sqlite3` in the app's data folder, or in memory when the organization keeps
//! nothing on disk): the projects, the sessions ("threads" here) and one append-only event log per session. In memory,
//! the Nexus sessions with a running agent are still written to a small file of their own (each one's id, its session's
//! id and its start time; no content), so a crash can end them in Gateway at the next launch.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::event::{
    Effort, LimitStop, PermissionMode, Project, SessionAgent, SetupStatus, StoredEvent, ThreadStatus, ThreadSummary,
    Via, WbEvent,
};
use crate::orchestra::grant::hash_token;
use crate::schedules::Schedule;

#[path = "session_sync/storage.rs"]
mod session_sync;

#[path = "session_sync/control_storage.rs"]
pub(crate) mod code_control;

#[path = "computer/storage.rs"]
mod computer;

#[path = "cli_log/storage.rs"]
mod cli_log;

#[path = "hooks/storage.rs"]
mod hooks;

#[path = "claude_trust/storage.rs"]
mod claude_trust;

#[path = "terminal/storage.rs"]
mod terminal_history;

pub use crate::page::EventsPage;

pub type Result<T> = std::result::Result<T, String>;

fn worktree_json(summary: &ThreadSummary) -> Option<String> {
    summary
        .worktree
        .as_ref()
        .and_then(|worktree| serde_json::to_string(worktree).ok())
}

/// A folder session's pull requests (`ThreadSummary::pull_requests`), as JSON; null when it has none.
fn pull_requests_json(summary: &ThreadSummary) -> Option<String> {
    (!summary.pull_requests.is_empty())
        .then(|| serde_json::to_string(&summary.pull_requests).ok())
        .flatten()
}

fn handoff_json(summary: &ThreadSummary) -> Option<String> {
    summary
        .handoff_from
        .as_ref()
        .and_then(|from| serde_json::to_string(from).ok())
}

fn limit_stop_json(summary: &ThreadSummary) -> Option<String> {
    summary
        .limited
        .as_ref()
        .and_then(|stop| serde_json::to_string(stop).ok())
}

pub(crate) fn sql<E: std::fmt::Display>(error: E) -> String {
    format!("workbench store: {error}")
}

/// A write the name index refused: two writers took the same name at once, after both checked it was free.
fn name_clash(error: rusqlite::Error) -> String {
    if error.to_string().contains("threads_name_lower") {
        return "That session name was just taken by another session.".into();
    }
    sql(error)
}

const SCHEMA: &str = "
create table if not exists projects (
  id text primary key,
  name text not null,
  path text not null unique,
  added_at integer not null,
  orchestration integer not null default 0,
  pinned integer not null default 0,
  archived integer not null default 0
);
create table if not exists threads (
  id text primary key,
  project_id text not null,
  scope text not null,
  title text not null,
  agent text not null,
  model text,
  permission_mode text not null,
  via text not null,
  status text not null,
  cost_usd real not null default 0,
  native_id text not null,
  native_started integer not null default 0,
  nexus_sessions text not null default '[]',
  context_tokens integer,
  context_window integer,
  created_at integer not null,
  updated_at integer not null,
  archived integer not null default 0,
  effort text,
  worktree text,
  account text,
  fork_from text,
  seed text,
  automatic_title integer not null default 0,
  parent_thread_id text,
  spawned_by text,
  group_id text,
  depth integer not null default 0,
  delivered_seq integer not null default 0,
  orchestration integer not null default 0,
  halt text,
  listed_at integer,
  pinned integer not null default 0,
  source_thread_id text,
  limit_stop text,
  snoozed_until integer,
  settled_at integer,
  settled_by integer,
  keep_listed integer not null default 0,
  side_chat integer not null default 0,
  name text,
  pull_requests text,
  fast integer not null default 0,
  acted_at integer,
  provider text,
  handoff_from text
);
create index if not exists threads_scope on threads (scope, updated_at);
create index if not exists threads_project on threads (project_id);
create table if not exists session_start_preferences (
  scope text not null, identity text not null, project_id text not null, preference text not null,
  primary key (scope, identity, project_id)
);
create table if not exists session_start_receipts (
  scope text not null, identity text not null, parent_id text not null, request_key text not null,
  primary key (scope, identity, parent_id, request_key)
);
create table if not exists project_setup (
  project_id text primary key,
  script text not null
);
create table if not exists project_scripts (
  project_id text primary key,
  scripts text not null
);
create table if not exists project_defaults (
  project_id text primary key,
  defaults_json text not null
);
create table if not exists schedules (
  id text primary key,
  scope text not null,
  project_id text not null,
  body text not null,
  token text
);
create table if not exists events (
  thread_id text not null,
  seq integer not null,
  at integer not null,
  body text not null,
  primary key (thread_id, seq)
) without rowid;
";

/// Nexus agent sessions whose agent process is running: what is left here after the app died is ended at its next
/// start. In the main file, or in a file of its own when the main one is in memory.
// The queries that run per event or per click, shared with `hot_queries_use_an_index` so it checks the real text.
/// After the columns are added: a file from before linked sessions has no `parent_thread_id` until then.
const LINK_INDEX: &str = "create index if not exists threads_parent on threads (parent_thread_id);";
/// A session's name is unique among the store's sessions regardless of case (session_name.rs); most sessions have
/// none. A file from before case mattered has the exact-case `threads_name` index: it goes, and this one replaces it.
const NAME_INDEX: &str = "drop index if exists threads_name;
create unique index if not exists threads_name_lower on threads (lower(name)) where name is not null;";
const PROJECT_BY_ID: &str =
    "select id, name, path, added_at, orchestration, pinned, archived from projects where id = ?1";
const PROJECT_BY_PATH: &str = "select id from projects where path = ?1";
const ARCHIVE_PROJECT_THREADS: &str = "update threads set archived = 1 where project_id = ?1";
/// After `select <columns> from threads`.
const THREADS_IN_SCOPE: &str = "where scope = ?1 order by updated_at desc";
/// After `select <columns> from threads`: a session's linked sessions, oldest first.
const CHILDREN: &str = "where parent_thread_id = ?1 order by created_at";
const SIDE_CHATS: &str = "where source_thread_id = ?1 and side_chat = 1 order by created_at";
const APPEND_EVENT: &str = "insert into events (thread_id, seq, at, body)
                 values (?1, coalesce((select max(seq) from events where thread_id = ?1), 0) + 1, ?2, ?3)
                 returning seq";
const EVENTS_AFTER: &str = "select seq, at, body from events where thread_id = ?1 and seq > ?2 order by seq";
/// A session's last two turn ends, newest first: a linked session's result is its log up to the last one.
const LAST_TURN_ENDS: &str = "select seq, at, body from events
                 where thread_id = ?1 and json_extract(body, '$.kind') = 'turnEnd' order by seq desc limit 2";
/// The session's own agent's last text between two events (`?2 < seq < ?3`): a linked session's last message.
const LAST_TEXT: &str = "select body from events
                 where thread_id = ?1 and seq > ?2 and seq < ?3 and json_extract(body, '$.kind') = 'text'
                   and json_extract(body, '$.parent') is null
                 order by seq desc limit 1";
/// The seqs of a session's last turn end, turn start and message logged at or after `?2` ([`Reader::last_seqs`]).
const LAST_SEQS: &str = "select max(case when json_extract(body, '$.kind') = 'turnEnd' then seq end),
                        max(case when json_extract(body, '$.kind') = 'turnStart' then seq end),
                        max(case when json_extract(body, '$.kind') = 'user' then seq end)
                 from events where thread_id = ?1 and at >= ?2";

/// The messages search reads (`Reader::search`): what the person typed and the agent's main answers, one row per event.
/// Trigrams, so any 3+ letters inside a word match, in any case, as the sidebar's title search does. Its rows go with
/// their session.
const SEARCH_INDEX: &str = "
create virtual table events_fts using fts5 (
  text,
  thread_id unindexed,
  seq unindexed,
  tokenize = 'trigram remove_diacritics 1'
);
";
const SEARCH_CLEANUP: &str = "
create trigger if not exists events_fts_thread_gone after delete on threads begin
  delete from events_fts where thread_id = old.id;
end;
";
/// Indexes the logs written before the index existed: the same events as `searchable` picks.
const SEARCH_BACKFILL: &str = "
insert into events_fts (text, thread_id, seq)
select json_extract(body, '$.text'), thread_id, seq from events
where ((json_extract(body, '$.kind') = 'user' and json_extract(body, '$.from') is null)
       or (json_extract(body, '$.kind') = 'text' and json_extract(body, '$.parent') is null))
  and coalesce(json_extract(body, '$.text'), '') <> '';
";
const SEARCH_EVENT: &str = "insert into events_fts (text, thread_id, seq) values (?1, ?2, ?3)";
/// One row per session: its best-matching message (by rank), the most recently active sessions first, at most `?3`.
/// The limit applies to sessions, so one long session with a match in every message cannot crowd the others out.
const SEARCH: &str = "
with best as (
  select f.rowid as hit, t.updated_at as updated_at,
         row_number() over (partition by f.thread_id order by f.rank) as place
  from events_fts f join threads t on t.id = f.thread_id
  where events_fts match ?2 and t.scope in (?1, ?4)
)
select f.thread_id, f.seq, snippet(events_fts, 0, char(2), char(3), '…', 48), t.updated_at
from events_fts f join threads t on t.id = f.thread_id
where events_fts match ?2 and f.rowid in (select hit from best where place = 1 order by updated_at desc limit ?3)
order by t.updated_at desc
";
/// Around the matched text in a `SearchHit`'s snippet.
pub const HIT_START: char = '\u{2}';
pub const HIT_END: char = '\u{3}';

/// The text of an event that search finds: a prompt, or an answer of the main agent (not a sub-agent's). Tool calls,
/// their output, thinking and images are left out, and so are a linked session's messages (`from`): not the person's.
fn searchable(event: &WbEvent) -> Option<&str> {
    let text = match event {
        WbEvent::User { text, from: None, .. } => text,
        WbEvent::Text { text, parent: None, .. } => text,
        _ => return None,
    };
    (!text.is_empty()).then_some(text.as_str())
}

/// What was typed as an FTS5 query: each word of 3+ letters as a literal (so quotes, brackets or AND are text), all of
/// them required. None when no word is long enough.
fn search_query(typed: &str) -> Option<String> {
    let words: Vec<String> = typed
        .split_whitespace()
        .filter(|word| word.chars().count() >= 3)
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// A session whose messages match a search, at its best-matching message.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub thread_id: String,
    pub seq: i64,
    /// Up to about 48 characters around the match, the matched text between `HIT_START` and `HIT_END`.
    pub snippet: String,
}

/// The seqs of a session's last turn end, turn start and message ([`Reader::last_seqs`]); None: none logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastSeqs {
    pub turn_end: Option<i64>,
    pub turn_start: Option<i64>,
    pub user: Option<i64>,
}

/// A bounded metadata-only page of newly appended turn completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnEndPage {
    pub through_seq: i64,
    pub turn_end: Option<i64>,
    pub more: bool,
}

const LEDGER: &str = "
create table if not exists open_sessions (
  session_id text primary key,
  thread_id text not null,
  started_at integer not null
);
";

/// What the store keeps about a session beyond its summary.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRecord {
    pub summary: ThreadSummary,
    /// Claude Code's session id: given with `--session-id` the first time, `--resume` after.
    pub native_id: String,
    /// Whether the agent has started this conversation (so the next run resumes it).
    pub native_started: bool,
    pub scope: String,
    /// A fork whose agent has not started yet: the conversation (the agent's own id) it goes on from, forked at its
    /// first start (Claude Code's `--fork-session`, Codex's `thread/fork`).
    pub fork_from: Option<String>,
    /// A fork whose agent has not started yet, from an earlier point than its agent can fork: what was said up to that
    /// point, given to the agent ahead of its first message.
    pub seed: Option<String>,
    /// False for legacy sessions, forks, or any explicit rename, even one that keeps the same text.
    pub automatic_title: bool,
    /// A linked session's last result its parent confirmed it read: a `TurnEnd`'s seq, 0 for none.
    pub delivered_seq: i64,
    /// Why the session may not run an automatic turn (the person's Stop or Interrupt, or the cascade's); None: free.
    pub halt: Option<String>,
}

/// Where a read-only connection finds the same data as the store's own.
#[derive(Debug)]
enum Target {
    File(PathBuf),
    /// The store's in-memory database, shared by name between its connections.
    Memory(String),
}

impl Target {
    fn open_reader(&self) -> Result<Connection> {
        let db = match self {
            Target::File(path) => {
                Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            }
            Target::Memory(uri) => Connection::open_with_flags(uri, memory_flags()),
        }
        .map_err(sql)?;
        db.busy_timeout(std::time::Duration::from_secs(5)).map_err(sql)?;
        // A reader never writes. In a shared in-memory database it also takes no table locks, so a write in progress
        // never makes it fail (a file's write-ahead log already lets readers and the writer pass each other).
        db.pragma_update(None, "query_only", true).map_err(sql)?;
        db.pragma_update(None, "read_uncommitted", true).map_err(sql)?;
        Ok(db)
    }
}

fn memory_flags() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
}

fn memory_uri() -> String {
    format!(
        "file:nexus-workbench-{}?mode=memory&cache=shared",
        uuid::Uuid::new_v4().simple()
    )
}

/// Reads from the store without waiting for whoever holds it: its own connections, which in a file database see every
/// committed write (the write-ahead log lets readers and the writer pass each other). Cheap to clone.
#[derive(Clone)]
pub struct Reader {
    target: Arc<Target>,
    idle: Arc<Mutex<Vec<Connection>>>,
    /// Whether a failed open was already logged: callers often drop read errors, so the first one is said out loud.
    warned: Arc<AtomicBool>,
}

impl Reader {
    /// Runs `read` on a connection of its own, so a long read does not hold up other reads.
    fn with<T>(&self, read: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let pooled = self.idle.lock().unwrap_or_else(|p| p.into_inner()).pop();
        let db = match pooled {
            Some(db) => db,
            None => self.target.open_reader().inspect_err(|error| {
                if !self.warned.swap(true, Ordering::Relaxed) {
                    eprintln!("nexus-workbench: could not open a read connection to the store: {error}");
                }
            })?,
        };
        let out = read(&db);
        let mut idle = self.idle.lock().unwrap_or_else(|p| p.into_inner());
        if idle.len() < 4 {
            idle.push(db);
        }
        out
    }

    /// Projects with their current Git metadata. Run this on a worker: it reads repository files.
    pub fn projects(&self) -> Result<Vec<Project>> {
        self.with(projects)
    }

    /// Indexed project locator, without reading Git metadata or scanning other projects.
    pub fn project_path(&self, id: &str) -> Result<Option<String>> {
        self.with(|db| {
            db.query_row("SELECT path FROM projects WHERE id = ?1", [id], |row| row.get(0))
                .optional()
                .map_err(sql)
        })
    }

    /// The scope's sessions, most recently active first.
    pub fn threads(&self, scope: &str) -> Result<Vec<ThreadRecord>> {
        self.with(|db| threads(db, scope))
    }

    pub fn scope_of(&self, thread_id: &str) -> Result<Option<String>> {
        self.with(|db| scope_of(db, thread_id))
    }

    pub fn thread(&self, id: &str) -> Result<Option<ThreadRecord>> {
        self.with(|db| thread(db, id))
    }

    pub fn events_after(&self, thread_id: &str, after: i64) -> Result<Vec<StoredEvent>> {
        self.with(|db| events_after(db, thread_id, after))
    }

    #[cfg(test)]
    pub(crate) fn with_db<T>(&self, read: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.with(read)
    }

    /// The side chats of session `session_id`, oldest first (side_chat.rs).
    pub fn side_chats(&self, session_id: &str) -> Result<Vec<ThreadRecord>> {
        self.with(|db| {
            let mut statement = db
                .prepare_cached(&format!("select {} from threads {SIDE_CHATS}", Store::THREAD_COLUMNS))
                .map_err(sql)?;
            let rows = statement.query_map([session_id], Store::read_thread).map_err(sql)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)
        })
    }

    /// A page of the session's log ending before `before` (page.rs).
    pub fn events_page(&self, thread_id: &str, before: Option<i64>, limit: Option<usize>) -> Result<EventsPage> {
        self.with(|db| crate::page::events_page(db, thread_id, before, limit))
    }

    /// Approvals asked and not yet answered, in order.
    pub fn open_approvals(&self, thread_id: &str) -> Result<Vec<String>> {
        self.with(|db| open_approvals(db, thread_id))
    }

    /// The sessions `parent_id` started, in any scope, oldest first ([`Store::children`]).
    pub fn children(&self, parent_id: &str) -> Result<Vec<ThreadRecord>> {
        self.with(|db| {
            let mut statement = db
                .prepare_cached(&format!("select {} from threads {CHILDREN}", Store::THREAD_COLUMNS))
                .map_err(sql)?;
            let rows = statement.query_map([parent_id], Store::read_thread).map_err(sql)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)
        })
    }

    /// Indexed latest sequence only; this never reads event bodies.
    pub fn latest_event_seq(&self, thread_id: &str) -> Result<i64> {
        self.with(|db| latest_event_seq(db, thread_id))
    }

    /// Inspect at most 256 new event kinds once, without returning or decoding their payloads.
    /// An unchanged thread takes only the primary-key MAX(seq) fast path.
    pub fn turn_end_page(&self, thread_id: &str, after: i64) -> Result<TurnEndPage> {
        self.with(|db| {
            let latest = latest_event_seq(db, thread_id)?;
            if latest <= after {
                return Ok(TurnEndPage {
                    through_seq: latest,
                    turn_end: None,
                    more: false,
                });
            }
            let (through_seq, turn_end) = db
                .prepare_cached(
                    "with page as materialized (
                    select seq, body from events
                    where thread_id=?1 and seq>?2 and seq<=?3 order by seq limit 256
                 )
                 select coalesce(max(seq),?2),
                        max(case when json_extract(body,'$.kind')='turnEnd' then seq end)
                 from page",
                )
                .map_err(sql)?
                .query_row(params![thread_id, after, latest], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
                })
                .map_err(sql)?;
            Ok(TurnEndPage {
                through_seq,
                turn_end,
                more: through_seq < latest,
            })
        })
    }

    /// The spawn path records a session event after the provider process starts.
    pub(crate) fn has_spawned_agent(&self, thread_id: &str) -> Result<bool> {
        self.with(|db| {
            db.prepare_cached(
                "select exists (
                    select 1 from events where thread_id=?1
                    and json_extract(body,'$.kind') = 'session'
                )",
            )
            .map_err(sql)?
            .query_row([thread_id], |row| row.get(0))
            .map_err(sql)
        })
    }

    /// The session's last two `TurnEnd` events, newest first.
    pub fn last_turn_ends(&self, thread_id: &str) -> Result<Vec<StoredEvent>> {
        self.with(|db| {
            let mut statement = db.prepare_cached(LAST_TURN_ENDS).map_err(sql)?;
            let rows = statement
                .query_map([thread_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
                })
                .map_err(sql)?;
            let mut ends = Vec::new();
            for row in rows {
                let (seq, at, body) = row.map_err(sql)?;
                if let Ok(event) = serde_json::from_str(&body) {
                    ends.push(StoredEvent {
                        thread_id: thread_id.into(),
                        seq,
                        at,
                        event,
                    });
                }
            }
            Ok(ends)
        })
    }

    /// The text of the session's own agent's last answer with `after < seq < before`, if any.
    pub fn last_text(&self, thread_id: &str, after: i64, before: i64) -> Result<Option<String>> {
        self.with(|db| {
            let body: Option<String> = db
                .prepare_cached(LAST_TEXT)
                .map_err(sql)?
                .query_row(params![thread_id, after, before], |row| row.get(0))
                .optional()
                .map_err(sql)?;
            Ok(body
                .and_then(|body| serde_json::from_str::<WbEvent>(&body).ok())
                .and_then(|event| match event {
                    WbEvent::Text { text, .. } => Some(text),
                    _ => None,
                }))
        })
    }

    /// The seqs of the session's last `TurnEnd`, `TurnStart` and `User` events logged at or after `since`, read in
    /// SQL: a check at startup that reads no whole log.
    pub fn last_seqs(&self, thread_id: &str, since: i64) -> Result<LastSeqs> {
        self.with(|db| {
            db.prepare_cached(LAST_SEQS)
                .map_err(sql)?
                .query_row(params![thread_id, since], |row| {
                    Ok(LastSeqs {
                        turn_end: row.get(0)?,
                        turn_start: row.get(1)?,
                        user: row.get(2)?,
                    })
                })
                .map_err(sql)
        })
    }

    /// The scope's sessions whose prompts or answers hold every word of `typed` (3+ letters each, shorter ones are
    /// skipped), most recently active first, at most `limit`.
    pub fn search(&self, scope: &str, typed: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.search_scopes(scope, None, typed, limit)
    }

    /// Search both visible scopes in recency order, with one shared limit.
    pub fn search_scopes(
        &self,
        scope: &str,
        additional_scope: Option<&str>,
        typed: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let Some(query) = search_query(typed) else {
            return Ok(Vec::new());
        };
        self.with(|db| {
            db.prepare_cached(SEARCH)
                .map_err(sql)?
                .query_map(
                    params![scope, query, i64::try_from(limit).unwrap_or(i64::MAX), additional_scope],
                    |row| {
                        Ok(SearchHit {
                            thread_id: row.get(0)?,
                            seq: row.get(1)?,
                            snippet: row.get(2)?,
                        })
                    },
                )
                .map_err(sql)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(sql)
        })
    }

    /// Every session with events: its last event's number, and whether it is archived. A cloud machine's history
    /// upload compares it with Gateway's.
    pub fn logged_threads(&self) -> Result<Vec<LoggedThread>> {
        self.with(logged_threads)
    }

    /// A session's events after `after`, at most `limit`, each with its body's text as stored: an upload sends the
    /// stored text, so a retry sends the same bytes.
    pub fn event_texts_after(&self, thread_id: &str, after: i64, limit: usize) -> Result<Vec<EventText>> {
        self.with(|db| event_texts_after(db, thread_id, after, limit))
    }
}

pub struct Store {
    db: Connection,
    target: Arc<Target>,
    /// Where `open_sessions` lives when it is not in `db`.
    ledger: Option<Connection>,
    persistent: bool,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        // Only the person may read it: it holds whole conversations, project paths and webhook tokens.
        if let Some(dir) = path.parent() {
            crate::private_fs::create_dir_all(dir).map_err(sql)?;
        }
        crate::private_fs::restrict_file(path).map_err(sql)?;
        let db = Connection::open(path).map_err(sql)?;
        db.pragma_update(None, "journal_mode", "wal").map_err(sql)?;
        db.pragma_update(None, "synchronous", "normal").map_err(sql)?;
        Store::init(db, None, true, Target::File(path.to_path_buf()))
    }

    /// The store's file for reads only: no schema work and no write, so it never waits for the write lock that a
    /// running session holds. The file must have been made by [`Store::open`].
    pub fn open_reading(path: &Path) -> Result<Store> {
        let target = Target::File(path.to_path_buf());
        Ok(Store {
            db: target.open_reader()?,
            target: Arc::new(target),
            ledger: None,
            persistent: true,
        })
    }

    /// Everything in memory, nothing written down (tests, or when even the ledger cannot be opened).
    pub fn in_memory() -> Result<Store> {
        let (db, target) = Store::memory()?;
        Store::init(db, None, false, target)
    }

    fn memory() -> Result<(Connection, Target)> {
        let uri = memory_uri();
        let db = Connection::open_with_flags(&uri, memory_flags()).map_err(sql)?;
        Ok((db, Target::Memory(uri)))
    }

    /// Everything in memory but the open Nexus sessions, kept in `ledger` so a crash can still end them.
    pub fn in_memory_with_ledger(ledger: &Path) -> Result<Store> {
        if let Some(dir) = ledger.parent() {
            crate::private_fs::create_dir_all(dir).map_err(sql)?;
        }
        crate::private_fs::restrict_file(ledger).map_err(sql)?;
        let file = Connection::open(ledger).map_err(sql)?;
        file.execute_batch(LEDGER).map_err(sql)?;
        let (db, target) = Store::memory()?;
        Store::init(db, Some(file), false, target)
    }

    fn init(db: Connection, ledger: Option<Connection>, persistent: bool, target: Target) -> Result<Store> {
        // Wait a moment for a lock instead of failing at once.
        db.busy_timeout(std::time::Duration::from_secs(5)).map_err(sql)?;
        db.execute_batch(SCHEMA).map_err(sql)?;
        db.execute_batch(session_sync::SCHEMA).map_err(sql)?;
        db.execute_batch(code_control::SCHEMA).map_err(sql)?;
        code_control::migrate(&db)?;
        db.execute_batch(computer::SCHEMA).map_err(sql)?;
        db.execute_batch(cli_log::SCHEMA).map_err(sql)?;
        db.execute_batch(hooks::SCHEMA).map_err(sql)?;
        db.execute_batch(claude_trust::SCHEMA).map_err(sql)?;
        db.execute_batch(terminal_history::SCHEMA).map_err(sql)?;
        // Files from before a column existed: it joins them empty (effort: the model's default; worktree: none;
        // account: Claude Code's own login; fork_from, seed: not a fork; the links: a session the person started, with
        // orchestration off and no halt; pinned: not pinned; source_thread_id: not a fork; limit_stop: no limit stop;
        // snoozed_until, settled_at, settled_by, keep_listed: in the active list, settled by no merge, not opted out;
        // a project's pinned and archived: neither; side_chat: not a side chat; pull_requests: none opened in the
        // project's folder; fast: standard speed; handoff_from: no handoff).
        for (table, column, declaration) in [
            ("threads", "effort", "text"),
            ("threads", "worktree", "text"),
            ("threads", "account", "text"),
            ("threads", "fork_from", "text"),
            ("threads", "seed", "text"),
            ("threads", "provider", "text"),
            ("threads", "automatic_title", "integer not null default 0"),
            ("threads", "parent_thread_id", "text"),
            ("threads", "spawned_by", "text"),
            ("threads", "group_id", "text"),
            ("threads", "depth", "integer not null default 0"),
            ("threads", "delivered_seq", "integer not null default 0"),
            ("threads", "orchestration", "integer not null default 0"),
            ("threads", "halt", "text"),
            ("threads", "listed_at", "integer"),
            ("threads", "pinned", "integer not null default 0"),
            ("threads", "source_thread_id", "text"),
            ("threads", "limit_stop", "text"),
            ("threads", "snoozed_until", "integer"),
            ("threads", "settled_at", "integer"),
            ("threads", "settled_by", "integer"),
            ("threads", "keep_listed", "integer not null default 0"),
            ("threads", "side_chat", "integer not null default 0"),
            ("threads", "name", "text"),
            ("threads", "pull_requests", "text"),
            ("threads", "fast", "integer not null default 0"),
            ("threads", "handoff_from", "text"),
            ("threads", "acted_at", "integer"),
            ("projects", "orchestration", "integer not null default 0"),
            ("projects", "pinned", "integer not null default 0"),
            ("projects", "archived", "integer not null default 0"),
        ] {
            let has: bool = db
                .query_row(
                    "select count(*) from pragma_table_info(?1) where name = ?2",
                    [table, column],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(sql)?
                > 0;
            if !has {
                db.execute_batch(&format!("alter table {table} add column {column} {declaration}"))
                    .map_err(sql)?;
            }
        }
        // Keep the existing order on upgrade, then preserve it through ordinary session updates.
        db.execute("update threads set listed_at = updated_at where listed_at is null", [])
            .map_err(sql)?;
        // A session saved before acted_at keeps its place: it reads as made when the person last acted on it.
        db.execute("update threads set acted_at = created_at where acted_at is null", [])
            .map_err(sql)?;
        session_sync::unbind_unsynced_agents(&db)?;
        hash_plain_schedule_tokens(&db)?;
        db.execute_batch(LINK_INDEX).map_err(sql)?;
        db.execute_batch(NAME_INDEX).map_err(sql)?;
        // Files from before search: the index is made and filled from their logs once, in one go, so a crash midway
        // leaves no half-filled index behind.
        let indexed: bool = db
            .query_row(
                "select count(*) from sqlite_master where name = 'events_fts'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(sql)?
            > 0;
        if !indexed {
            db.execute_batch(&format!("begin; {SEARCH_INDEX} {SEARCH_BACKFILL} commit;"))
                .map_err(|error| {
                    let _ = db.execute_batch("rollback");
                    sql(error)
                })?;
        }
        db.execute_batch(SEARCH_CLEANUP).map_err(sql)?;
        db.execute_batch(crate::page::LASTING_INDEX).map_err(sql)?;
        db.execute_batch(LEDGER).map_err(sql)?;
        Ok(Store {
            db,
            target: Arc::new(target),
            ledger,
            persistent,
        })
    }

    /// A handle for reading without taking the store: hand it to whoever reads often or for long.
    pub fn reader(&self) -> Reader {
        Reader {
            target: self.target.clone(),
            idle: Arc::new(Mutex::new(Vec::new())),
            warned: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Another store on the same data, for reads only: what a reader in this process opens when the store is in
    /// memory and has no file to open. It writes nothing; the data lives while this store does.
    pub fn reading(&self) -> Result<Store> {
        Ok(Store {
            db: self.target.open_reader()?,
            target: self.target.clone(),
            ledger: None,
            persistent: self.persistent,
        })
    }

    fn ledger(&self) -> &Connection {
        self.ledger.as_ref().unwrap_or(&self.db)
    }

    /// The scope a session belongs to.
    pub fn scope_of(&self, thread_id: &str) -> Result<Option<String>> {
        scope_of(&self.db, thread_id)
    }

    /// False when nothing outlives the app.
    pub fn persistent(&self) -> bool {
        self.persistent
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        projects(&self.db)
    }

    pub fn project(&self, id: &str) -> Result<Option<Project>> {
        let found = self
            .db
            .query_row(PROJECT_BY_ID, [id], read_project)
            .optional()
            .map_err(sql)?;
        // Only this project's branch and remotes are read, not every project's.
        Ok(found.map(|mut project| {
            read_git(&mut project);
            project
        }))
    }

    /// Adds a folder, or returns the project already open on it.
    pub fn add_project(&self, path: &Path, now: i64) -> Result<Project> {
        let path_text = path.to_string_lossy().to_string();
        let existing: Option<String> = self
            .db
            .query_row(PROJECT_BY_PATH, [&path_text], |row| row.get(0))
            .optional()
            .map_err(sql)?;
        let id = match existing {
            // Opened again: an archived project comes back.
            Some(id) => {
                self.db
                    .execute("update projects set archived = 0 where id = ?1", [&id])
                    .map_err(sql)?;
                id
            }
            None => {
                let id = format!("p_{}", uuid::Uuid::new_v4().simple());
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path_text.clone());
                self.db
                    .execute(
                        "insert into projects (id, name, path, added_at) values (?1, ?2, ?3, ?4)",
                        params![id, name, path_text, now],
                    )
                    .map_err(sql)?;
                id
            }
        };
        self.project(&id)?.ok_or_else(|| sql("project vanished"))
    }

    /// Removes the project from the list. Its sessions are archived, not deleted.
    pub fn remove_project(&self, id: &str) -> Result<()> {
        self.db
            .execute("delete from projects where id = ?1", [id])
            .map_err(sql)?;
        self.db.execute(ARCHIVE_PROJECT_THREADS, [id]).map_err(sql)?;
        self.db
            .execute("delete from project_setup where project_id = ?1", [id])
            .map_err(sql)?;
        self.db
            .execute("delete from project_scripts where project_id = ?1", [id])
            .map_err(sql)?;
        self.db
            .execute("delete from session_start_preferences where project_id = ?1", [id])
            .map_err(sql)?;
        self.db
            .execute("delete from project_defaults where project_id = ?1", [id])
            .map_err(sql)?;
        self.forget_claude_trust(id)?;
        self.db
            .execute("delete from schedules where project_id = ?1", [id])
            .map_err(sql)?;
        Ok(())
    }

    /// Whether the project's new sessions may start linked sessions.
    pub fn set_project_orchestration(&self, id: &str, on: bool) -> Result<()> {
        self.db
            .execute("update projects set orchestration = ?2 where id = ?1", params![id, on])
            .map_err(sql)?;
        Ok(())
    }

    /// Pins or unpins the project, and archives it or brings it back; None leaves that one as it is. Its sessions,
    /// settings and worktrees stay either way.
    pub fn set_project_state(&self, id: &str, pinned: Option<bool>, archived: Option<bool>) -> Result<()> {
        self.db
            .execute(
                "update projects set pinned = coalesce(?2, pinned), archived = coalesce(?3, archived) where id = ?1",
                params![id, pinned, archived],
            )
            .map_err(sql)?;
        Ok(())
    }

    pub(crate) fn start_preference(&self, scope: &str, identity: &str, project: &str) -> Result<Option<String>> {
        self.db
            .query_row(
                "select preference from session_start_preferences where scope=?1 and identity=?2 and project_id=?3",
                params![scope, identity, project],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)
    }

    pub(crate) fn start_preference_overrides(&self, scope: &str, identity: &str, clear: bool) -> Result<u32> {
        if clear {
            self.db
                .execute(
                    "delete from session_start_preferences where scope=?1 and identity=?2",
                    params![scope, identity],
                )
                .map_err(sql)?;
        }
        self.db
            .query_row(
                "select count(*) from session_start_preferences pref
                 join projects on projects.id = pref.project_id where pref.scope=?1 and pref.identity=?2",
                params![scope, identity],
                |row| row.get(0),
            )
            .map_err(sql)
    }

    pub(crate) fn set_start_preference(
        &self,
        scope: &str,
        identity: &str,
        project: &str,
        preference: &str,
    ) -> Result<()> {
        self.db.execute("insert into session_start_preferences values (?1,?2,?3,?4) on conflict(scope,identity,project_id) do update set preference=excluded.preference", params![scope,identity,project,preference]).map(|_| ()).map_err(sql)
    }
    pub(crate) fn chose_here(&self, scope: &str, identity: &str, parent: &str, key: &str) -> Result<bool> {
        self.db.query_row("select exists(select 1 from session_start_receipts where scope=?1 and identity=?2 and parent_id=?3 and request_key=?4)", params![scope,identity,parent,key], |row| row.get(0)).map_err(sql)
    }
    pub(crate) fn remember_here(&self, scope: &str, identity: &str, parent: &str, key: &str) -> Result<()> {
        self.db
            .execute(
                "insert or ignore into session_start_receipts values (?1,?2,?3,?4)",
                params![scope, identity, parent, key],
            )
            .map(|_| ())
            .map_err(sql)
    }

    pub fn project_setup(&self, id: &str) -> Result<Option<String>> {
        self.db
            .query_row("select script from project_setup where project_id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(sql)
    }

    /// Sets the project's setup script; None takes it away.
    pub fn set_project_setup(&self, id: &str, script: Option<&str>) -> Result<()> {
        match script {
            Some(script) => self.db.execute(
                "insert into project_setup (project_id, script) values (?1, ?2)
                 on conflict (project_id) do update set script = excluded.script",
                params![id, script],
            ),
            None => self.db.execute("delete from project_setup where project_id = ?1", [id]),
        }
        .map_err(sql)?;
        Ok(())
    }

    /// The project's commands ([`crate::scripts`]) as the JSON list they are kept in, if it has any.
    pub fn project_scripts(&self, id: &str) -> Result<Option<String>> {
        self.db
            .query_row(
                "select scripts from project_scripts where project_id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)
    }

    /// Sets the project's commands (a JSON list); None takes them away.
    pub fn set_project_scripts(&self, id: &str, scripts: Option<&str>) -> Result<()> {
        match scripts {
            Some(scripts) => self.db.execute(
                "insert into project_scripts (project_id, scripts) values (?1, ?2)
                 on conflict (project_id) do update set scripts = excluded.scripts",
                params![id, scripts],
            ),
            None => self
                .db
                .execute("delete from project_scripts where project_id = ?1", [id]),
        }
        .map_err(sql)?;
        Ok(())
    }

    /// The project's new-session defaults ([`crate::project_defaults`]) as the JSON they are kept in, if any.
    pub fn project_defaults(&self, id: &str) -> Result<Option<String>> {
        self.db
            .query_row(
                "select defaults_json from project_defaults where project_id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)
    }

    /// Sets them (JSON); None takes them away.
    pub fn set_project_defaults(&self, id: &str, defaults: Option<&str>) -> Result<()> {
        match defaults {
            Some(defaults) => self.db.execute(
                "insert into project_defaults (project_id, defaults_json) values (?1, ?2)
                 on conflict (project_id) do update set defaults_json = excluded.defaults_json",
                params![id, defaults],
            ),
            None => self
                .db
                .execute("delete from project_defaults where project_id = ?1", [id]),
        }
        .map_err(sql)?;
        Ok(())
    }

    /// The scope's schedules ([`crate::schedules`]), oldest first. A row that no longer reads (written by a later
    /// app) is left out, not a broken list.
    pub fn schedules(&self, scope: &str) -> Result<Vec<Schedule>> {
        let mut statement = self
            .db
            .prepare_cached("select body from schedules where scope = ?1")
            .map_err(sql)?;
        let rows = statement
            .query_map([scope], |row| row.get::<_, String>(0))
            .map_err(sql)?;
        let mut schedules: Vec<Schedule> = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql)?
            .iter()
            .filter_map(|body| serde_json::from_str(body).ok())
            .collect();
        schedules.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(schedules)
    }

    /// Schedule `id` and its scope.
    pub fn schedule(&self, id: &str) -> Result<Option<(String, Schedule)>> {
        let row: Option<(String, String)> = self
            .db
            .query_row("select scope, body from schedules where id = ?1", [id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(sql)?;
        Ok(row.and_then(|(scope, body)| Some((scope, serde_json::from_str(&body).ok()?))))
    }

    /// Saves the schedule; its webhook token stays as it was.
    pub fn save_schedule(&self, scope: &str, schedule: &Schedule) -> Result<()> {
        let body = serde_json::to_string(schedule).map_err(sql)?;
        self.db
            .execute(
                "insert into schedules (id, scope, project_id, body) values (?1, ?2, ?3, ?4)
                 on conflict (id) do update set scope = excluded.scope, project_id = excluded.project_id,
                   body = excluded.body",
                params![schedule.id, scope, schedule.project_id, body],
            )
            .map_err(sql)?;
        Ok(())
    }

    pub fn delete_schedule(&self, id: &str) -> Result<()> {
        self.db
            .execute("delete from schedules where id = ?1", [id])
            .map_err(sql)?;
        Ok(())
    }

    /// The hash of the schedule's webhook token ([`hash_token`]), if its webhook was ever turned on and not off
    /// since. The token itself is never kept.
    pub fn schedule_token_hash(&self, id: &str) -> Result<Option<String>> {
        self.db
            .query_row("select token from schedules where id = ?1", [id], |row| row.get(0))
            .optional()
            .map(Option::flatten)
            .map_err(sql)
    }

    /// Sets the schedule's webhook token, kept as its hash; None takes it away.
    pub fn set_schedule_token(&self, id: &str, token: Option<&str>) -> Result<()> {
        let hash = token.map(hash_token);
        self.db
            .execute("update schedules set token = ?2 where id = ?1", params![id, hash])
            .map_err(sql)?;
        Ok(())
    }

    pub fn insert_thread(&self, record: &ThreadRecord) -> Result<()> {
        let s = &record.summary;
        self.db
            .execute(
                "insert into threads (id, project_id, scope, title, agent, model, permission_mode, via, status, cost_usd,
                   native_id, native_started, nexus_sessions, context_tokens, context_window, created_at, updated_at,
                   archived, effort, worktree, account, fork_from, seed, automatic_title, parent_thread_id, spawned_by,
                   group_id, depth, delivered_seq, orchestration, halt, listed_at, pinned, source_thread_id, limit_stop,
                   snoozed_until, settled_at, settled_by, keep_listed, side_chat, name, pull_requests, fast, acted_at, provider, handoff_from)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21,
                   ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, ?41,
                   ?42, ?43, ?44, ?45, ?46)",
                params![
                    s.id,
                    s.project_id,
                    record.scope,
                    s.title,
                    s.agent.as_str(),
                    s.model,
                    s.permission_mode.as_str(),
                    s.via.as_str(),
                    s.status.as_str(),
                    s.cost_usd,
                    record.native_id,
                    record.native_started,
                    serde_json::to_string(&s.nexus_sessions).unwrap_or_else(|_| "[]".into()),
                    s.context_tokens.map(|t| t as i64),
                    s.context_window.map(|t| t as i64),
                    s.created_at,
                    s.updated_at,
                    s.archived,
                    s.effort.map(Effort::as_str),
                    worktree_json(s),
                    s.account,
                    record.fork_from,
                    record.seed,
                    record.automatic_title,
                    s.parent_thread_id,
                    s.spawned_by,
                    s.group_id,
                    s.depth,
                    record.delivered_seq,
                    s.orchestration,
                    record.halt,
                    s.listed_at,
                    s.pinned,
                    s.source_thread_id,
                    limit_stop_json(s),
                    s.aside.snoozed_until,
                    s.aside.settled_at,
                    s.aside.settled_by.map(|number| number as i64),
                    s.aside.keep_listed,
                    s.side_chat,
                    s.name,
                    pull_requests_json(s),
                    s.fast,
                    // A new session: the person acted on it when they made it.
                    s.acted_at.unwrap_or(s.created_at),
                    s.provider.as_ref().map(nexus_providers::InstanceId::as_str),
                    handoff_json(s),
                ],
            )
            .map_err(name_clash)?;
        Ok(())
    }

    /// Transfer the checkout, name and child links atomically. A second handoff cannot replace the same owner.
    pub(crate) fn replace_for_handoff(
        &self,
        old: &ThreadRecord,
        next: &ThreadRecord,
        identity: Option<&crate::session_sync::Identity>,
    ) -> Result<Vec<ThreadRecord>> {
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        let changed = self
            .db
            .execute(
                "update threads set archived=1, worktree=null, name=null, limit_stop=null, updated_at=?3
             where id=?1 and native_id=?2 and archived=0 and orchestration=1 and halt is null",
                params![old.summary.id, old.native_id, next.summary.created_at],
            )
            .map_err(sql)?;
        if changed != 1 {
            return Err("The session changed before the handoff.".into());
        }
        self.insert_thread(next)?;
        if let Some(identity) = identity {
            self.sync_register(&next.summary.id, identity)?;
        }
        let children = self.children(&old.summary.id)?;
        self.db
            .execute(
                "update threads set parent_thread_id=?2 where parent_thread_id=?1",
                params![old.summary.id, next.summary.id],
            )
            .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(children)
    }

    /// Inserts a session with its log so far (`(at, event)`, in order), all or nothing: a fork that fails part way
    /// leaves no half-copied session behind.
    pub fn insert_thread_with<'a>(
        &self,
        record: &ThreadRecord,
        events: impl IntoIterator<Item = (i64, &'a WbEvent)>,
    ) -> Result<()> {
        self.insert_thread_with_identity(record, events, None)
    }

    pub(crate) fn insert_thread_with_identity<'a>(
        &self,
        record: &ThreadRecord,
        events: impl IntoIterator<Item = (i64, &'a WbEvent)>,
        identity: Option<&crate::session_sync::Identity>,
    ) -> Result<()> {
        // Dropped without a commit, it rolls back.
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        self.insert_thread(record)?;
        if let Some(identity) = identity {
            self.sync_register(&record.summary.id, identity)?;
        }
        for (at, event) in events {
            self.append(&record.summary.id, at, event)?;
        }
        tx.commit().map_err(sql)
    }

    /// Saves the native conversation and its history boundary together, so a restart cannot resume discarded history.
    pub(crate) fn save_thread_with_event(&self, record: &ThreadRecord, event: &WbEvent) -> Result<StoredEvent> {
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        self.save_thread(record)?;
        let stored = self.append(&record.summary.id, record.summary.updated_at, event)?;
        tx.commit().map_err(sql)?;
        Ok(stored)
    }

    /// Writes back everything but the id, the project, the scope and the creation time. `acted_at` only moves forward:
    /// a record read before the person's message cannot move it back.
    pub fn save_thread(&self, record: &ThreadRecord) -> Result<()> {
        let s = &record.summary;
        self.db
            .prepare_cached(
                "update threads set title = ?2, model = ?3, permission_mode = ?4, via = ?5, status = ?6, cost_usd = ?7,
                   native_started = ?8, nexus_sessions = ?9, context_tokens = ?10, context_window = ?11,
                   updated_at = ?12, archived = ?13, effort = ?14, native_id = ?15, worktree = ?16, account = ?17,
                   fork_from = ?18, seed = ?19, automatic_title = ?20, parent_thread_id = ?21, spawned_by = ?22,
                   group_id = ?23, depth = ?24, delivered_seq = ?25, orchestration = ?26, halt = ?27,
                   listed_at = ?28, pinned = ?29, source_thread_id = ?30, limit_stop = ?31, snoozed_until = ?32,
                   settled_at = ?33, settled_by = ?34, keep_listed = ?35, side_chat = ?36, name = ?37, agent = ?38,
                   pull_requests = ?39, fast = ?40,
                   acted_at = nullif(max(coalesce(?41, 0), coalesce(acted_at, 0)), 0), provider = ?42, handoff_from = ?43
                 where id = ?1",
            )
            .map_err(sql)?
            .execute(params![
                s.id,
                s.title,
                s.model,
                s.permission_mode.as_str(),
                s.via.as_str(),
                s.status.as_str(),
                s.cost_usd,
                record.native_started,
                serde_json::to_string(&s.nexus_sessions).unwrap_or_else(|_| "[]".into()),
                s.context_tokens.map(|t| t as i64),
                s.context_window.map(|t| t as i64),
                s.updated_at,
                s.archived,
                s.effort.map(Effort::as_str),
                record.native_id,
                worktree_json(s),
                s.account,
                record.fork_from,
                record.seed,
                record.automatic_title,
                s.parent_thread_id,
                s.spawned_by,
                s.group_id,
                s.depth,
                record.delivered_seq,
                s.orchestration,
                record.halt,
                s.listed_at,
                s.pinned,
                s.source_thread_id,
                limit_stop_json(s),
                s.aside.snoozed_until,
                s.aside.settled_at,
                s.aside.settled_by.map(|number| number as i64),
                s.aside.keep_listed,
                s.side_chat,
                s.name,
                s.agent.as_str(),
                pull_requests_json(s),
                s.fast,
                s.acted_at,
                s.provider.as_ref().map(nexus_providers::InstanceId::as_str),
                handoff_json(s),
            ])
            .map_err(name_clash)?;
        Ok(())
    }

    /// Deletes a session and its event log for good. The open Nexus sessions ledger is left alone: an agent run that
    /// is still open there gets ended in Gateway like any other.
    pub fn delete_thread(&self, id: &str) -> Result<()> {
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        // Keep deletion intent until Gateway removes the encrypted copy and its locked keys.
        tx.execute("insert into session_sync_deletions (thread_id, scope, owner, sync_id, last_attempt)
                    select thread_id, scope, owner, sync_id, last_attempt from session_sync_threads where thread_id = ?1", [id])
            .map_err(sql)?;
        tx.execute("delete from session_sync_threads where thread_id = ?1", [id])
            .map_err(sql)?;
        tx.execute("delete from session_sync_outbox where thread_id = ?1", [id])
            .map_err(sql)?;
        tx.execute("delete from cli_transcripts where thread_id = ?1", [id])
            .map_err(sql)?;
        tx.execute("delete from terminal_command_events where command_id in (select id from terminal_commands where thread_id = ?1)", [id]).map_err(sql)?;
        tx.execute("delete from terminal_commands where thread_id = ?1", [id])
            .map_err(sql)?;
        tx.execute("delete from events where thread_id = ?1", [id])
            .map_err(sql)?;
        tx.execute("delete from threads where id = ?1", [id]).map_err(sql)?;
        tx.commit().map_err(sql)
    }

    const THREAD_COLUMNS: &'static str = "id, project_id, scope, title, agent, model, permission_mode, via, status,
        cost_usd, native_id, native_started, nexus_sessions, context_tokens, context_window, created_at, updated_at,
        archived, effort, worktree, account, fork_from, seed, automatic_title, parent_thread_id, spawned_by, group_id,
        depth, delivered_seq, orchestration, halt, listed_at, pinned, source_thread_id, limit_stop, snoozed_until,
        settled_at, settled_by, keep_listed, side_chat, name, pull_requests, fast, acted_at, provider, handoff_from";

    fn read_thread(row: &rusqlite::Row) -> rusqlite::Result<ThreadRecord> {
        let sessions: String = row.get(12)?;
        let agent: String = row.get(4)?;
        let mode: String = row.get(6)?;
        let via: String = row.get(7)?;
        let status: String = row.get(8)?;
        Ok(ThreadRecord {
            summary: ThreadSummary {
                id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(3)?,
                name: row.get(40)?,
                agent: SessionAgent::parse(&agent).unwrap_or(SessionAgent::Claude),
                model: row.get(5)?,
                permission_mode: PermissionMode::parse(&mode).unwrap_or(PermissionMode::Default),
                effort: row.get::<_, Option<String>>(18)?.as_deref().and_then(Effort::parse),
                fast: row.get(42)?,
                via: Via::parse(&via).unwrap_or(Via::Nexus),
                provider: row.get::<_, Option<String>>(44)?.map(nexus_providers::InstanceId::new),
                status: ThreadStatus::parse(&status),
                cost_usd: row.get(9)?,
                nexus_sessions: serde_json::from_str(&sessions).unwrap_or_default(),
                context_tokens: row.get::<_, Option<i64>>(13)?.map(|t| t as u64),
                context_window: row.get::<_, Option<i64>>(14)?.map(|t| t as u64),
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
                listed_at: row.get(31)?,
                acted_at: row.get(43)?,
                archived: row.get(17)?,
                pinned: row.get(32)?,
                worktree: row
                    .get::<_, Option<String>>(19)?
                    .and_then(|json| serde_json::from_str(&json).ok()),
                pull_requests: row
                    .get::<_, Option<String>>(41)?
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .unwrap_or_default(),
                account: row.get(20)?,
                pending: None,
                capabilities: None,
                parent_thread_id: row.get(24)?,
                source_thread_id: row.get(33)?,
                side_chat: row.get(39)?,
                spawned_by: row.get(25)?,
                handoff_from: row
                    .get::<_, Option<String>>(45)?
                    .and_then(|json| serde_json::from_str(&json).ok()),
                group_id: row.get(26)?,
                depth: row.get(27)?,
                orchestration: row.get(29)?,
                waits_on: vec![],
                pending_results: 0,
                start_choices: vec![],
                // The halt the chips read, from the row (an unknown value reads as "fault").
                halt: row
                    .get::<_, Option<String>>(30)?
                    .map(|halt| crate::linked::Halt::parse(&halt).as_str().to_string()),
                crew_paused: false,
                crew_tripped: false,
                crew_spend_trip: None,
                in_background: false,
                background: vec![],
                background_total: 0,
                at_depth_limit: false,
                limited: row
                    .get::<_, Option<String>>(34)?
                    .and_then(|json| serde_json::from_str(&json).ok()),
                aside: crate::snooze::Aside {
                    snoozed_until: row.get(35)?,
                    settled_at: row.get(36)?,
                    settled_by: row.get::<_, Option<i64>>(37)?.map(|number| number as u64),
                    keep_listed: row.get(38)?,
                },
            },
            native_id: row.get(10)?,
            native_started: row.get(11)?,
            scope: row.get(2)?,
            fork_from: row.get(21)?,
            seed: row.get(22)?,
            automatic_title: row.get(23)?,
            delivered_seq: row.get(28)?,
            halt: row.get(30)?,
        })
    }

    pub fn thread(&self, id: &str) -> Result<Option<ThreadRecord>> {
        thread(&self.db, id)
    }

    /// The id of the session whose name is `name` in any case, in any scope: names are unique in the whole store
    /// regardless of case (`NAME_INDEX`).
    pub fn thread_named(&self, name: &str) -> Result<Option<String>> {
        self.db
            .prepare_cached("select id from threads where lower(name) = lower(?1)")
            .map_err(sql)?
            .query_row([name], |row| row.get(0))
            .optional()
            .map_err(sql)
    }

    /// The scope's sessions, most recently active first.
    pub fn threads(&self, scope: &str) -> Result<Vec<ThreadRecord>> {
        threads(&self.db, scope)
    }

    /// The folders of the worktrees sessions still have, in every scope: a folder in the worktrees folder that none
    /// of them names is no session's (`cleanup::Workbench::worktrees`).
    pub fn worktree_paths(&self) -> Result<Vec<String>> {
        let mut statement = self
            .db
            .prepare("select worktree from threads where worktree is not null")
            .map_err(sql)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(sql)?;
        Ok(rows
            .filter_map(std::result::Result::ok)
            .filter_map(|text| serde_json::from_str::<crate::event::Worktree>(&text).ok())
            .filter(|worktree| !worktree.removed)
            .map(|worktree| worktree.path)
            .collect())
    }

    /// The scope's sessions set to continue by themselves when their plan's limit resets, soonest first: their ids and
    /// reset times. An archived session never continues.
    pub fn scheduled_continues(&self, scope: &str) -> Result<Vec<(String, i64)>> {
        let mut statement = self
            .db
            .prepare_cached(
                "select id, limit_stop from threads where scope = ?1 and archived = 0 and limit_stop is not null",
            )
            .map_err(sql)?;
        let rows = statement
            .query_map([scope], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(sql)?;
        let mut due: Vec<(String, i64)> = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql)?
            .into_iter()
            .filter_map(|(id, json)| {
                let stop: LimitStop = serde_json::from_str(&json).ok()?;
                Some((id, stop.resets_at.filter(|_| stop.scheduled)?))
            })
            .collect();
        due.sort_by_key(|(_, at)| *at);
        Ok(due)
    }

    /// The sessions `parent_id` started, in any scope, oldest first.
    pub fn children(&self, parent_id: &str) -> Result<Vec<ThreadRecord>> {
        let mut statement = self
            .db
            .prepare_cached(&format!("select {} from threads {CHILDREN}", Store::THREAD_COLUMNS))
            .map_err(sql)?;
        let rows = statement.query_map([parent_id], Store::read_thread).map_err(sql)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)
    }

    /// A linked session's last result its parent confirmed it read (`ThreadRecord::delivered_seq`): raised to `seq`,
    /// or with `lower`, lowered to it (a revert, which never raises it). Only that column: the session's other fields
    /// and `updated_at` stay.
    pub fn set_delivered(&self, id: &str, seq: i64, lower: bool) -> Result<()> {
        let query = if lower {
            "update threads set delivered_seq = min(delivered_seq, ?2) where id = ?1"
        } else {
            "update threads set delivered_seq = max(delivered_seq, ?2) where id = ?1"
        };
        self.db.execute(query, params![id, seq]).map_err(sql)?;
        Ok(())
    }

    /// The session's halt (`ThreadRecord::halt`), None to clear it. Only that column: `updated_at` stays.
    pub fn set_halt(&self, id: &str, halt: Option<&str>) -> Result<()> {
        self.db
            .execute("update threads set halt = ?2 where id = ?1", params![id, halt])
            .map_err(sql)?;
        Ok(())
    }

    /// Persist the whole cascade atomically before any process receives a stop signal.
    pub fn halt_crew(&self, id: &str) -> Result<()> {
        self.db
            .execute(
                "with recursive crew(id) as (select id from threads where id = ?1
             union select t.id from threads t join crew c on t.parent_thread_id = c.id)
             update threads set halt = case when id = ?1 then 'person' else coalesce(halt, 'parent') end
             where id in (select id from crew)",
                [id],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// A person's message clears its own halt and inherited halts below it in one write.
    pub fn clear_halts(&self, id: &str) -> Result<()> {
        self.db
            .execute(
                "with recursive crew(id) as (select id from threads where id = ?1
             union select t.id from threads t join crew c on t.parent_thread_id = c.id)
             update threads set halt = null where id = ?1
             or (id in (select id from crew) and halt = 'parent')",
                [id],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// Delete promotes every descendant to a root, with its halt cleared and its worktree kept.
    pub fn free_children(&self, parent_id: &str) -> Result<()> {
        self.db
            .execute(
                "with recursive crew(id) as (select id from threads where parent_thread_id = ?1
             union select t.id from threads t join crew c on t.parent_thread_id = c.id)
             update threads set parent_thread_id = null, depth = 0, halt = null,
             spawned_by = null, group_id = null, delivered_seq = 0 where id in (select id from crew)",
                [parent_id],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// Every linked session, in any scope: the app's or host's limit on live ones counts them all.
    pub fn linked_threads(&self) -> Result<Vec<ThreadRecord>> {
        let mut statement = self
            .db
            .prepare(&format!(
                "select {} from threads where parent_thread_id is not null",
                Store::THREAD_COLUMNS
            ))
            .map_err(sql)?;
        let rows = statement.query_map([], Store::read_thread).map_err(sql)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)
    }

    /// Every session that was busy when the app last stopped, in any scope.
    pub fn busy_threads(&self) -> Result<Vec<ThreadRecord>> {
        let mut statement = self
            .db
            .prepare(&format!(
                "select {} from threads where status in ('starting', 'running', 'approval')",
                Store::THREAD_COLUMNS
            ))
            .map_err(sql)?;
        let rows = statement.query_map([], Store::read_thread).map_err(sql)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)
    }

    /// Appends an event under the session's next number.
    pub fn append(&self, thread_id: &str, at: i64, event: &WbEvent) -> Result<StoredEvent> {
        let body = serde_json::to_string(event).map_err(sql)?;
        let seq: i64 = self
            .db
            .prepare_cached(APPEND_EVENT)
            .map_err(sql)?
            .query_row(params![thread_id, at, body], |row| row.get(0))
            .map_err(sql)?;
        if let Some(text) = searchable(event) {
            // The event is stored either way: a message missing from search is not worth failing the session over.
            let indexed = self
                .db
                .prepare_cached(SEARCH_EVENT)
                .and_then(|mut statement| statement.execute(params![text, thread_id, seq]));
            if let Err(error) = indexed {
                eprintln!("nexus-workbench: could not index a message for search: {error}");
            }
        }
        Ok(StoredEvent {
            thread_id: thread_id.into(),
            seq,
            at,
            event: event.clone(),
        })
    }

    pub(crate) fn stream_events_after(&self, thread_id: &str, after: i64) -> Result<Vec<StoredEvent>> {
        let mut statement = self
            .db
            .prepare_cached("select seq, at, body from events where thread_id=?1 and seq>?2 order by seq limit 257")
            .map_err(sql)?;
        read_events(&mut statement, params![thread_id, after], thread_id)
    }

    pub(crate) fn last_event_seq(&self, thread_id: &str) -> Result<i64> {
        self.db
            .query_row(
                "select coalesce(max(seq),0) from events where thread_id=?1",
                [thread_id],
                |row| row.get(0),
            )
            .map_err(sql)
    }

    pub fn events_after(&self, thread_id: &str, after: i64) -> Result<Vec<StoredEvent>> {
        events_after(&self.db, thread_id, after)
    }

    /// Approvals asked and not yet answered, in order.
    pub fn open_approvals(&self, thread_id: &str) -> Result<Vec<String>> {
        open_approvals(&self.db, thread_id)
    }

    /// Setup scripts whose last event says they still run: `(thread id, the last event)`. Called at start-up, when
    /// none can be running any more.
    pub fn unfinished_setups(&self) -> Result<Vec<(String, WbEvent)>> {
        let mut statement = self
            .db
            .prepare(
                "select thread_id, body from events where body like '{\"kind\":\"setup\"%' order by thread_id, seq",
            )
            .map_err(sql)?;
        let rows = statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(sql)?;
        let mut last: Vec<(String, WbEvent)> = Vec::new();
        for row in rows {
            let (thread_id, body) = row.map_err(sql)?;
            let Ok(event) = serde_json::from_str::<WbEvent>(&body) else {
                continue;
            };
            let WbEvent::Setup { id, .. } = &event else {
                continue;
            };
            match last.iter_mut().find(|(thread, seen)| {
                *thread == thread_id && matches!(seen, WbEvent::Setup { id: seen, .. } if seen == id)
            }) {
                Some(slot) => slot.1 = event,
                None => last.push((thread_id, event)),
            }
        }
        last.retain(|(_, event)| {
            matches!(
                event,
                WbEvent::Setup {
                    status: SetupStatus::Running,
                    ..
                }
            )
        });
        Ok(last)
    }

    pub fn open_session(&self, session_id: &str, thread_id: &str, started_at: i64) -> Result<()> {
        self.ledger()
            .execute(
                "insert or replace into open_sessions (session_id, thread_id, started_at) values (?1, ?2, ?3)",
                params![session_id, thread_id, started_at],
            )
            .map_err(sql)?;
        Ok(())
    }

    pub fn close_session(&self, session_id: &str) -> Result<()> {
        self.ledger()
            .execute("delete from open_sessions where session_id = ?1", params![session_id])
            .map_err(sql)?;
        Ok(())
    }

    /// The sessions left open (each with when it started). Each stays until `close_session`, once Gateway took its end.
    pub fn open_sessions(&self) -> Result<Vec<(String, i64)>> {
        let open = self
            .ledger()
            .prepare("select session_id, started_at from open_sessions")
            .map_err(sql)?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(sql)?
            .collect::<std::result::Result<Vec<(String, i64)>, _>>()
            .map_err(sql)?;
        Ok(open)
    }
}

fn read_project(row: &rusqlite::Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        branch: None,
        remotes: vec![],
        host: Default::default(),
        added_at: row.get(3)?,
        orchestration: row.get(4)?,
        pinned: row.get(5)?,
        archived: row.get(6)?,
    })
}

fn scope_of(db: &Connection, thread_id: &str) -> Result<Option<String>> {
    db.prepare_cached("select scope from threads where id = ?1")
        .map_err(sql)?
        .query_row([thread_id], |row| row.get(0))
        .optional()
        .map_err(sql)
}

fn thread(db: &Connection, id: &str) -> Result<Option<ThreadRecord>> {
    db.prepare_cached(&format!("select {} from threads where id = ?1", Store::THREAD_COLUMNS))
        .map_err(sql)?
        .query_row([id], Store::read_thread)
        .optional()
        .map_err(sql)
}

fn projects(db: &Connection) -> Result<Vec<Project>> {
    let mut statement = db
        .prepare("select id, name, path, added_at, orchestration, pinned, archived from projects order by added_at")
        .map_err(sql)?;
    let rows = statement.query_map([], read_project).map_err(sql)?;
    let mut projects = rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)?;
    for project in &mut projects {
        read_git(project);
    }
    Ok(projects)
}

/// A project's branch, remotes and host, read from the repository its folder is in: the folder itself, or one above
/// it (`git::project_top`). A folder in no repository has none of them.
fn read_git(project: &mut Project) {
    let path = Path::new(&project.path);
    let top = crate::git::project_top(path);
    let repo = top.as_deref().unwrap_or(path);
    project.branch = crate::git::project_branch(path);
    project.remotes = crate::git::remotes(repo);
    project.host = crate::source_host::Host::of_repo(repo);
}

fn threads(db: &Connection, scope: &str) -> Result<Vec<ThreadRecord>> {
    let mut statement = db
        .prepare(&format!(
            "select {} from threads {THREADS_IN_SCOPE}",
            Store::THREAD_COLUMNS
        ))
        .map_err(sql)?;
    let rows = statement.query_map([scope], Store::read_thread).map_err(sql)?;
    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)
}

fn latest_event_seq(db: &Connection, thread_id: &str) -> Result<i64> {
    db.query_row(
        "select coalesce(max(seq),0) from events where thread_id=?1",
        [thread_id],
        |row| row.get(0),
    )
    .map_err(sql)
}

#[cfg(test)]
#[path = "store/completion_tests.rs"]
mod completion_tests;

fn events_after(db: &Connection, thread_id: &str, after: i64) -> Result<Vec<StoredEvent>> {
    let mut statement = db.prepare_cached(EVENTS_AFTER).map_err(sql)?;
    read_events(&mut statement, params![thread_id, after], thread_id)
}

/// The events a `select seq, at, body` statement answers, in its order.
pub(crate) fn read_events(
    statement: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
    thread_id: &str,
) -> Result<Vec<StoredEvent>> {
    let rows = statement
        .query_map(params, |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
        })
        .map_err(sql)?;
    let mut events = Vec::new();
    for row in rows {
        let (seq, at, body) = row.map_err(sql)?;
        // An event this version cannot read (written by a newer one) becomes a notice, not a gap.
        let event = serde_json::from_str(&body).unwrap_or_else(|_| WbEvent::Notice {
            text: "An event this version of Nexus cannot show.".into(),
        });
        events.push(StoredEvent {
            thread_id: thread_id.into(),
            seq,
            at,
            event,
        });
    }
    Ok(events)
}

/// A session with events ([`Reader::logged_threads`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedThread {
    pub id: String,
    pub last_seq: i64,
    pub archived: bool,
}

/// A stored event with its body's text ([`Reader::event_texts_after`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventText {
    pub seq: i64,
    pub at: i64,
    pub body: String,
}

fn logged_threads(db: &Connection) -> Result<Vec<LoggedThread>> {
    let mut statement = db
        .prepare("select id, archived, (select max(seq) from events where events.thread_id = threads.id) from threads")
        .map_err(sql)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })
        .map_err(sql)?;
    let mut threads = Vec::new();
    for row in rows {
        let (id, archived, last_seq) = row.map_err(sql)?;
        if let Some(last_seq) = last_seq {
            threads.push(LoggedThread { id, last_seq, archived });
        }
    }
    Ok(threads)
}

fn event_texts_after(db: &Connection, thread_id: &str, after: i64, limit: usize) -> Result<Vec<EventText>> {
    let mut statement = db
        .prepare_cached("select seq, at, body from events where thread_id = ?1 and seq > ?2 order by seq limit ?3")
        .map_err(sql)?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = statement
        .query_map(params![thread_id, after, limit], |row| {
            Ok(EventText {
                seq: row.get(0)?,
                at: row.get(1)?,
                body: row.get(2)?,
            })
        })
        .map_err(sql)?;
    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)
}

/// Files from before tokens were hashed kept each webhook token in clear: hash them in place. The token was shown once
/// when it was made, so callers keep working.
fn hash_plain_schedule_tokens(db: &Connection) -> Result<()> {
    let plain: Vec<(String, String)> = {
        let mut statement = db
            .prepare("select id, token from schedules where token is not null and token not like 'sha256:%'")
            .map_err(sql)?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(sql)?
            .collect::<std::result::Result<_, _>>()
            .map_err(sql)?
    };
    for (id, token) in plain {
        db.execute(
            "update schedules set token = ?2 where id = ?1",
            params![id, hash_token(&token)],
        )
        .map_err(sql)?;
    }
    Ok(())
}

fn open_approvals(db: &Connection, thread_id: &str) -> Result<Vec<String>> {
    let mut open: Vec<String> = Vec::new();
    for stored in events_after(db, thread_id, 0)? {
        match stored.event {
            WbEvent::Approval { request_id, .. } => open.push(request_id),
            WbEvent::ApprovalResolved { request_id, .. } => open.retain(|id| *id != request_id),
            _ => {}
        }
    }
    Ok(open)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Worktree;

    #[test]
    fn opens_a_file_from_before_effort_worktrees_accounts_and_links() {
        let db = Connection::open_in_memory().unwrap_or_else(|e| panic!("{e}"));
        let old_schema = SCHEMA
            .replace(
                "  archived integer not null default 0,\n  effort text,\n  worktree text,\n  account text,\n  fork_from text,\n  seed text,\n  automatic_title integer not null default 0,\n  parent_thread_id text,\n  spawned_by text,\n  group_id text,\n  depth integer not null default 0,\n  delivered_seq integer not null default 0,\n  orchestration integer not null default 0,\n  halt text,\n  listed_at integer,\n  pinned integer not null default 0,\n  source_thread_id text,\n  limit_stop text,\n  snoozed_until integer,\n  settled_at integer,\n  settled_by integer,\n  keep_listed integer not null default 0,\n  side_chat integer not null default 0,\n  name text,\n  pull_requests text,\n  fast integer not null default 0,\n  acted_at integer,\n  provider text,\n  handoff_from text\n",
                "  archived integer not null default 0\n",
            )
            .replace(
                "  added_at integer not null,\n  orchestration integer not null default 0,\n  pinned integer not null default 0,\n  archived integer not null default 0\n",
                "  added_at integer not null\n",
            );
        assert!(
            !old_schema.contains("effort")
                && !old_schema.contains("orchestration")
                && !old_schema.contains("pinned")
                && !old_schema.contains("snoozed_until")
                && !old_schema.contains("side_chat")
                && !old_schema.contains("pull_requests")
                && !old_schema.contains("fast")
                && !old_schema.contains("acted_at")
                && !old_schema.contains("provider")
                && !old_schema.contains("handoff_from"),
            "the test strips the newer columns"
        );
        db.execute_batch(&old_schema).unwrap_or_else(|e| panic!("{e}"));
        db.execute(
            "insert into threads (id, project_id, scope, title, agent, model, permission_mode, via, status, native_id,
               created_at, updated_at) values ('t', 'p', 's', 'x', 'claude', null, 'default', 'nexus', 'idle', 'n', 1, 2)",
            [],
        )
        .unwrap_or_else(|e| panic!("{e}"));
        db.execute(
            "insert into projects (id, name, path, added_at) values ('p', 'repo', '/nowhere/repo', 1)",
            [],
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let store = Store::init(db, None, false, Target::Memory(String::new())).unwrap_or_else(|e| panic!("{e}"));
        let mut old = store
            .thread("t")
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("the old session reads"));
        assert_eq!(old.summary.effort, None);
        assert_eq!(old.summary.worktree, None);
        assert_eq!(old.summary.account, None);
        assert_eq!(old.summary.provider, None);
        assert_eq!(old.summary.listed_at, Some(old.summary.updated_at));
        // It keeps its place in its project: the person last acted on it when they made it.
        assert_eq!(old.summary.acted_at, Some(old.summary.created_at));
        assert!(!old.automatic_title, "an old title may already be a manual rename");
        // A session from before linked sessions is one the person started, with orchestration off and no halt.
        assert_eq!(old.summary.parent_thread_id, None);
        // Nor is it a fork that can send back.
        assert_eq!(old.summary.source_thread_id, None);
        assert_eq!(old.summary.handoff_from, None);
        assert!(!old.summary.side_chat);
        assert_eq!((old.summary.depth, old.summary.orchestration), (0, false));
        assert_eq!((old.delivered_seq, old.halt.clone()), (0, None));
        // Nor did its plan stop it at a limit, nor has it a name.
        assert_eq!(old.summary.limited, None);
        assert_eq!(old.summary.name, None);
        // Nor did its agent open a pull request in the project's folder.
        assert!(old.summary.pull_requests.is_empty());
        let project = store
            .project("p")
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("the old project reads"));
        assert!(!project.orchestration);
        // Nor was it pinned or archived.
        assert!(!project.pinned && !project.archived);
        store
            .set_project_orchestration("p", true)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(store.project("p").ok().flatten().is_some_and(|p| p.orchestration));
        store
            .set_project_state("p", Some(true), Some(true))
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            store
                .project("p")
                .ok()
                .flatten()
                .is_some_and(|p| p.pinned && p.archived && p.orchestration)
        );
        assert!(store.children("t").unwrap_or_else(|e| panic!("{e}")).is_empty());
        assert!(!old.summary.pinned);
        old.summary.pinned = true;
        old.summary.effort = Some(Effort::High);
        old.summary.fast = true;
        old.summary.worktree = Some(Worktree {
            path: "/wt/repo/nexus-x".into(),
            branch: "nexus/x".into(),
            base: "origin/main".into(),
            removed: false,
            pull_request: Some("https://github.com/o/r/pull/1".into()),
            pull_requests: Vec::new(),
            review: None,
        });
        old.summary.parent_thread_id = Some("t_parent".into());
        old.summary.source_thread_id = Some("t_source".into());
        old.summary.side_chat = true;
        old.summary.pull_requests = vec!["https://github.com/o/r/pull/2".into()];
        old.summary.limited = Some(LimitStop {
            resets_at: Some(1_792_605_600_000),
            scheduled: true,
        });
        old.summary.spawned_by = Some("agent".into());
        old.summary.group_id = Some("g".into());
        old.summary.depth = 1;
        old.summary.orchestration = true;
        old.delivered_seq = 7;
        old.halt = Some("person".into());
        // The summary reads the row's halt, for the app's chips.
        old.summary.halt = Some("person".into());
        assert_eq!(
            old.summary.aside,
            crate::snooze::Aside::default(),
            "older files: in the active list"
        );
        old.summary.aside = crate::snooze::Aside {
            snoozed_until: Some(9_000),
            settled_at: Some(8_000),
            settled_by: Some(42),
            keep_listed: true,
        };
        store.save_thread(&old).unwrap_or_else(|e| panic!("{e}"));
        let saved = store
            .thread("t")
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("the session reads"));
        assert_eq!(saved.summary.effort, Some(Effort::High));
        assert!(saved.summary.fast);
        assert_eq!(saved.summary.worktree, old.summary.worktree);
        assert!(saved.summary.pinned);
        assert_eq!(saved, old, "every new column round-trips");
        assert_eq!(
            store.scheduled_continues("s").unwrap_or_else(|e| panic!("{e}")),
            [("t".to_string(), 1_792_605_600_000)]
        );
        assert_eq!(
            store.children("t_parent").unwrap_or_else(|e| panic!("{e}")),
            vec![saved]
        );
        assert_eq!(store.linked_threads().unwrap_or_else(|e| panic!("{e}")).len(), 1);
    }

    fn record(id: &str, scope: &str) -> ThreadRecord {
        ThreadRecord {
            summary: ThreadSummary {
                name: None,
                id: id.into(),
                project_id: "p".into(),
                title: "Fix the build".into(),
                agent: SessionAgent::Claude,
                model: None,
                permission_mode: PermissionMode::Default,
                effort: None,
                fast: false,
                via: Via::Nexus,
                provider: None,
                account: None,
                status: ThreadStatus::Idle,
                cost_usd: 0.0,
                nexus_sessions: vec![],
                context_tokens: None,
                context_window: None,
                created_at: 1,
                updated_at: 1,
                listed_at: Some(1),
                acted_at: Some(1),
                archived: false,
                pinned: false,
                worktree: None,
                pull_requests: Vec::new(),
                pending: None,
                capabilities: None,
                parent_thread_id: None,
                source_thread_id: None,
                side_chat: false,
                spawned_by: None,
                group_id: None,
                depth: 0,
                orchestration: false,
                waits_on: vec![],
                pending_results: 0,
                start_choices: vec![],
                halt: None,
                crew_paused: false,
                crew_tripped: false,
                crew_spend_trip: None,
                in_background: false,
                background: vec![],
                background_total: 0,
                at_depth_limit: false,
                handoff_from: None,
                limited: None,
                aside: Default::default(),
            },
            native_id: "n".into(),
            native_started: false,
            scope: scope.into(),
            fork_from: None,
            seed: None,
            automatic_title: false,
            delivered_seq: 0,
            halt: None,
        }
    }

    #[test]
    fn a_saved_provider_survives_a_store_reopen_and_thread_update() -> Result<()> {
        let root = std::env::temp_dir().join(format!("nexus-provider-thread-{}", uuid::Uuid::new_v4()));
        let store = Store::open(&root)?;
        let mut thread = record("chosen", "|");
        let choice = nexus_providers::InstanceId::new("server:chosen".into());
        thread.summary.provider = Some(choice.clone());
        store.insert_thread(&thread)?;
        thread.summary.title = "renamed".into();
        store.save_thread(&thread)?;
        drop(store);
        let reopened = Store::open(&root)?;
        let summary = reopened.thread("chosen")?.expect("saved thread").summary;
        assert_eq!(summary.provider, Some(choice));
        assert_eq!(summary.title, "renamed");
        assert_eq!(
            serde_json::to_value(&summary).expect("serializes")["provider"],
            "server:chosen"
        );
        drop(reopened);
        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn replays_tool_result_images_from_the_event_log() -> Result<()> {
        let store = Store::in_memory()?;
        store.insert_thread(&record("a", "org1"))?;
        let event = WbEvent::ToolResult {
            id: "shot".into(),
            output: "Captured".into(),
            is_error: false,
            truncated: false,
            parent: None,
            images: vec![crate::event::Image {
                media_type: "image/png".into(),
                data: "aGk=".into(),
                name: None,
            }],
        };
        store.append("a", 1, &event)?;
        assert_eq!(store.reader().events_after("a", 0)?[0].event, event);
        Ok(())
    }

    /// What a cloud machine's history upload reads: sessions with events, and their stored text in number order.
    #[test]
    fn logged_threads_and_their_event_texts() -> Result<()> {
        let store = Store::in_memory()?;
        for id in ["a", "b", "empty"] {
            store.insert_thread(&record(id, "org1"))?;
        }
        for (thread, text) in [("a", "one"), ("a", "two"), ("a", "three"), ("b", "x")] {
            store.append(thread, 7, &user(text))?;
        }
        let mut archived = record("b", "org1");
        archived.summary.archived = true;
        store.save_thread(&archived)?;
        let reader = store.reader();
        let mut threads = reader.logged_threads()?;
        threads.sort_by(|left, right| left.id.cmp(&right.id));
        let found: Vec<(&str, i64, bool)> = threads
            .iter()
            .map(|thread| (thread.id.as_str(), thread.last_seq, thread.archived))
            .collect();
        assert_eq!(found, [("a", 3, false), ("b", 1, true)]);
        let texts = reader.event_texts_after("a", 1, 1)?;
        assert_eq!(texts.len(), 1);
        assert_eq!((texts[0].seq, texts[0].at), (2, 7));
        assert_eq!(serde_json::from_str::<WbEvent>(&texts[0].body).ok(), Some(user("two")));
        assert!(reader.event_texts_after("a", 3, 10)?.is_empty());
        Ok(())
    }

    /// A name round-trips on insert and on save, and the store's index refuses a second session with the same name.
    #[test]
    fn a_name_is_stored_and_unique_in_the_store() -> Result<()> {
        let store = Store::in_memory()?;
        let mut first = record("a", "org1");
        first.summary.name = Some("orchestrator".into());
        store.insert_thread(&first)?;
        assert_eq!(store.thread("a")?, Some(first.clone()), "insert");
        assert_eq!(store.thread_named("orchestrator")?, Some("a".to_string()));
        assert_eq!(store.thread_named("lead")?, None);
        assert_eq!(store.thread_named("Orchestrator")?, Some("a".to_string()), "any case");
        // Any scope and any case: a name is unique in the whole store.
        let mut second = record("b", "org2");
        second.summary.name = Some("ORCHESTRATOR".into());
        assert_eq!(
            store.insert_thread(&second),
            Err("That session name was just taken by another session.".to_string())
        );
        second.summary.name = None;
        store.insert_thread(&second)?;
        second.summary.name = Some("orchestrator".into());
        assert!(store.save_thread(&second).is_err(), "save");
        // Cleared, it is free again; many sessions have no name.
        first.summary.name = None;
        store.save_thread(&first)?;
        store.save_thread(&second)?;
        assert_eq!(store.thread_named("orchestrator")?, Some("b".to_string()));
        assert_eq!(
            store.thread("b")?.and_then(|thread| thread.summary.name),
            Some("orchestrator".into())
        );
        Ok(())
    }

    /// A file from before names ignored case has the exact-case index; opening it replaces that with `lower(name)`.
    #[test]
    fn a_file_with_the_exact_case_name_index_gets_the_case_free_one() -> Result<()> {
        let db = Connection::open_in_memory().map_err(sql)?;
        db.execute_batch(SCHEMA).map_err(sql)?;
        db.execute_batch("create unique index threads_name on threads (name) where name is not null;")
            .map_err(sql)?;
        let store = Store::init(db, None, false, Target::Memory(String::new()))?;
        let indexes: Vec<String> = store
            .db
            .prepare("select name from sqlite_master where type = 'index' and name like 'threads_name%' order by name")
            .map_err(sql)?
            .query_map([], |row| row.get(0))
            .map_err(sql)?
            .collect::<std::result::Result<_, _>>()
            .map_err(sql)?;
        assert_eq!(indexes, ["threads_name_lower"]);
        let mut first = record("a", "org1");
        first.summary.name = Some("lead".into());
        store.insert_thread(&first)?;
        let mut second = record("b", "org1");
        second.summary.name = Some("Lead".into());
        assert!(store.insert_thread(&second).is_err(), "Lead and lead cannot both exist");
        Ok(())
    }

    /// The limit stop and the snooze marks each bind to their own column, on insert and on save: every value is
    /// distinct, so a swapped placeholder reads back a different summary.
    #[test]
    fn limit_stop_and_aside_round_trip_through_insert_and_save() -> Result<()> {
        let store = Store::in_memory()?;
        let mut session = record("a", "org1");
        session.summary.limited = Some(LimitStop {
            resets_at: Some(1_792_605_600_000),
            scheduled: true,
        });
        session.summary.aside = crate::snooze::Aside {
            snoozed_until: Some(9_000),
            settled_at: Some(8_000),
            settled_by: Some(42),
            keep_listed: true,
        };
        store.insert_thread(&session)?;
        assert_eq!(store.thread("a")?, Some(session.clone()), "insert");
        session.summary.limited = Some(LimitStop {
            resets_at: Some(1_792_700_000_000),
            scheduled: false,
        });
        session.summary.aside = crate::snooze::Aside {
            snoozed_until: Some(7_000),
            settled_at: Some(6_000),
            settled_by: Some(43),
            keep_listed: false,
        };
        store.save_thread(&session)?;
        assert_eq!(store.thread("a")?, Some(session), "save");
        Ok(())
    }

    #[test]
    fn sidebar_activity_survives_metadata_updates_and_reopening() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("nexus-sidebar-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).map_err(sql)?;
        let path = dir.join("workbench.sqlite3");
        {
            let store = Store::open(&path)?;
            let mut session = record("a", "org1");
            session.summary.listed_at = Some(5);
            store.insert_thread(&session)?;
            session.summary.updated_at = 100;
            session.summary.title = "Renamed".into();
            store.save_thread(&session)?;
        }
        {
            let store = Store::open(&path)?;
            let session = store.reader().thread("a")?.ok_or("no a")?;
            assert_eq!(session.summary.updated_at, 100);
            assert_eq!(session.summary.listed_at, Some(5));
        }
        std::fs::remove_dir_all(&dir).map_err(sql)?;
        Ok(())
    }

    #[test]
    fn acted_at_starts_at_creation_and_only_moves_forward() -> Result<()> {
        let store = Store::in_memory()?;
        let mut session = record("a", "org1");
        (session.summary.created_at, session.summary.acted_at) = (7, None);
        store.insert_thread(&session)?;
        assert_eq!(store.reader().thread("a")?.ok_or("no a")?.summary.acted_at, Some(7));
        session.summary.acted_at = Some(50);
        store.save_thread(&session)?;
        assert_eq!(store.reader().thread("a")?.ok_or("no a")?.summary.acted_at, Some(50));
        // A record read before the person's message writes back its older time: the newer one stays.
        session.summary.acted_at = Some(7);
        session.summary.updated_at = 90;
        store.save_thread(&session)?;
        let saved = store.reader().thread("a")?.ok_or("no a")?.summary;
        assert_eq!((saved.acted_at, saved.updated_at), (Some(50), 90));
        Ok(())
    }

    #[test]
    fn keeps_threads_per_scope_and_numbers_events() -> Result<()> {
        let store = Store::in_memory()?;
        store.insert_thread(&record("a", "org1"))?;
        store.insert_thread(&record("b", "org2"))?;
        let mut a = store.thread("a")?.ok_or("no a")?;
        a.summary.status = ThreadStatus::Running;
        a.summary.nexus_sessions.push("s1".into());
        a.native_started = true;
        a.fork_from = Some("n0".into());
        a.seed = Some("What was said".into());
        store.save_thread(&a)?;
        assert_eq!(store.thread("a")?, Some(a.clone()));
        assert_eq!(store.threads("org1")?.len(), 1);
        assert_eq!(store.busy_threads()?.len(), 1);

        let user = |text: &str| WbEvent::User {
            text: text.into(),
            images: vec![],
            from: None,
        };
        let first = store.append("a", 5, &user("hi"))?;
        let second = store.append(
            "a",
            6,
            &WbEvent::Approval {
                request_id: "r1".into(),
                tool_use_id: None,
                tool: "Bash".into(),
                input: serde_json::json!({"command": "ls"}),
                description: None,
                suggestions: vec![],
                parent: None,
            },
        )?;
        assert_eq!((first.seq, second.seq), (1, 2));
        assert_eq!(store.append("b", 7, &user("x"))?.seq, 1);
        assert_eq!(store.events_after("a", 1)?.len(), 1);
        assert_eq!(store.open_approvals("a")?, vec!["r1".to_string()]);
        store.append(
            "a",
            8,
            &WbEvent::ApprovalResolved {
                request_id: "r1".into(),
                decision: crate::event::Decision::Allow,
                message: None,
            },
        )?;
        assert!(store.open_approvals("a")?.is_empty());

        store.delete_thread("a")?;
        assert_eq!(store.thread("a")?, None);
        assert!(store.events_after("a", 0)?.is_empty());
        assert_eq!(store.events_after("b", 0)?.len(), 1, "another session's log stays");
        Ok(())
    }

    #[test]
    fn keeps_open_sessions_on_disk_when_the_rest_is_in_memory() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("nexus-workbench-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ledger = dir.join("workbench-sessions.sqlite3");
        let store = Store::in_memory_with_ledger(&ledger)?;
        assert!(!store.persistent());
        store.insert_thread(&record("t1", "gw|org"))?;
        store.open_session("ns_1", "t1", 10)?;
        store.open_session("ns_2", "t1", 20)?;
        store.close_session("ns_2")?;
        // The app dies: the sessions are gone, the open Nexus session is not.
        drop(store);
        let store = Store::in_memory_with_ledger(&ledger)?;
        assert_eq!(store.threads("gw|org")?, vec![]);
        assert_eq!(store.open_sessions()?, vec![("ns_1".to_string(), 10)]);
        // Read, not taken: it stays until Gateway took its end, even if the app dies again first.
        drop(store);
        let store = Store::in_memory_with_ledger(&ledger)?;
        assert_eq!(store.open_sessions()?, vec![("ns_1".to_string(), 10)]);
        store.close_session("ns_1")?;
        assert_eq!(store.open_sessions()?, vec![]);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// A webhook token is kept only as its hash, and a file from before that keeps one in clear has it hashed when
    /// it opens.
    #[test]
    fn webhook_tokens_are_kept_hashed_and_old_clear_ones_are_hashed_on_open() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("nexus-workbench-tokens-{}", uuid::Uuid::new_v4().simple()));
        let file = dir.join("workbench.sqlite3");
        {
            let store = Store::open(&file)?;
            store
                .db
                .execute(
                    "insert into schedules (id, scope, project_id, body, token) values
                       ('old', 's', 'p', '{}', 'tok_plain'), ('new', 's', 'p', '{}', null), ('off', 's', 'p', '{}', null)",
                    [],
                )
                .map_err(sql)?;
            store.set_schedule_token("new", Some("tok_fresh"))?;
            assert_eq!(store.schedule_token_hash("new")?, Some(hash_token("tok_fresh")));
        }
        let store = Store::open(&file)?;
        assert_eq!(store.schedule_token_hash("old")?, Some(hash_token("tok_plain")));
        assert_eq!(
            store.schedule_token_hash("new")?,
            Some(hash_token("tok_fresh")),
            "hashed once, not twice"
        );
        assert_eq!(store.schedule_token_hash("off")?, None);
        let kept: Vec<String> = store
            .db
            .prepare("select token from schedules where token is not null")
            .map_err(sql)?
            .query_map([], |row| row.get(0))
            .map_err(sql)?
            .collect::<std::result::Result<_, _>>()
            .map_err(sql)?;
        assert!(
            kept.iter()
                .all(|token| !token.contains("tok_") && token.starts_with(crate::orchestra::grant::HASH_PREFIX)),
            "{kept:?}"
        );
        std::fs::remove_dir_all(&dir).map_err(sql)?;
        Ok(())
    }

    /// A project's pin and archive survive a restart (the file opened again), change one at a time, and leave its
    /// sessions as they were. Opening an archived project's folder again brings it back, still pinned.
    #[test]
    fn a_projects_pin_and_archive_are_kept_across_restarts() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("nexus-workbench-pins-{}", uuid::Uuid::new_v4().simple()));
        let folder = dir.join("repo");
        std::fs::create_dir_all(&folder).map_err(sql)?;
        let file = dir.join("workbench.sqlite3");
        let id = {
            let store = Store::open(&file)?;
            let project = store.add_project(&folder, 1)?;
            assert!(!project.pinned && !project.archived);
            let mut session = record("t", "s");
            session.summary.project_id = project.id.clone();
            store.insert_thread(&session)?;
            store.set_project_state(&project.id, Some(true), None)?;
            store.set_project_state(&project.id, None, Some(true))?;
            project.id
        };
        let store = Store::open(&file)?;
        let project = store.project(&id)?.ok_or("the project reads")?;
        assert!(project.pinned && project.archived, "both kept");
        assert_eq!(store.projects()?.len(), 1, "an archived project is still listed");
        let session = store.thread("t")?.ok_or("the session reads")?;
        assert!(!session.summary.archived, "its sessions stay as they were");
        store.set_project_state(&id, Some(false), None)?;
        let project = store.project(&id)?.ok_or("the project reads")?;
        assert!(!project.pinned && project.archived, "None leaves the archive alone");
        store.set_project_state(&id, Some(true), None)?;
        let again = store.add_project(&folder, 2)?;
        assert_eq!(again.id, id);
        assert!(again.pinned && !again.archived, "opened again: back, and still pinned");
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn finds_one_project_by_id() -> Result<()> {
        let store = Store::in_memory()?;
        let base = std::env::temp_dir().join(format!("nexus-workbench-project-{}", uuid::Uuid::new_v4().simple()));
        let (a, b) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(a.join(".git")).map_err(sql)?;
        std::fs::write(a.join(".git/HEAD"), "ref: refs/heads/feature\n").map_err(sql)?;
        std::fs::create_dir_all(&b).map_err(sql)?;
        let first = store.add_project(&a, 1)?;
        let second = store.add_project(&b, 2)?;
        let found = store.project(&first.id)?.ok_or("no first")?;
        assert_eq!(found.id, first.id);
        assert_eq!(found.name, "a");
        assert_eq!(found.branch.as_deref(), Some("feature"));
        assert_eq!(store.project(&second.id)?.ok_or("no second")?.branch, None);
        assert_eq!(store.project("p_missing")?, None);
        let _ = std::fs::remove_dir_all(&base);
        Ok(())
    }

    /// The queries that run per event or per click (the shipped text, unbound parameters read as null). Not every
    /// query: `busy_threads` scans `threads`, which is fine because it runs once at launch.
    #[test]
    fn per_event_and_per_click_queries_use_an_index() -> Result<()> {
        let store = Store::in_memory()?;
        let plan = |query: &str| -> Result<String> {
            let mut statement = store.db.prepare(&format!("explain query plan {query}")).map_err(sql)?;
            let mut rows = statement.raw_query();
            let mut details = Vec::new();
            while let Some(row) = rows.next().map_err(sql)? {
                details.push(row.get::<_, String>(3).map_err(sql)?);
            }
            Ok(details.join(" | "))
        };
        for query in [
            EVENTS_AFTER,
            APPEND_EVENT,
            PROJECT_BY_ID,
            PROJECT_BY_PATH,
            ARCHIVE_PROJECT_THREADS,
            &format!("select id from threads {THREADS_IN_SCOPE}"),
            &format!("select id from threads {CHILDREN}"),
        ] {
            let plan = plan(query)?;
            assert!(
                plan.contains("SEARCH") && !plan.contains("SCAN"),
                "{query} scans a table: {plan}"
            );
        }
        Ok(())
    }

    fn user(text: &str) -> WbEvent {
        WbEvent::User {
            text: text.into(),
            images: vec![],
            from: None,
        }
    }

    #[test]
    fn a_reader_sees_the_stores_writes_in_a_file_and_in_memory() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("nexus-workbench-reader-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for store in [Store::open(&dir.join("w.sqlite3"))?, Store::in_memory()?] {
            let reader = store.reader();
            store.insert_thread(&record("a", "org1"))?;
            store.append("a", 1, &user("one"))?;
            store.append("a", 2, &user("two"))?;
            assert_eq!(reader.events_after("a", 0)?.len(), 2);
            assert_eq!(reader.events_after("a", 1)?.len(), 1);
            assert_eq!(reader.scope_of("a")?.as_deref(), Some("org1"));
            assert_eq!(reader.thread("a")?, store.thread("a")?);
            assert_eq!(reader.threads("org1")?, store.threads("org1")?);
            assert!(reader.threads("another-org")?.is_empty());
            let repo = dir.join("repo");
            std::fs::create_dir_all(repo.join(".git")).map_err(sql)?;
            std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/reader-test\n").map_err(sql)?;
            std::fs::write(
                repo.join(".git/config"),
                "[remote \"origin\"]\nurl = https://github.com/example/repo.git\n",
            )
            .map_err(sql)?;
            store.add_project(&repo, 1)?;
            let worker = reader.clone();
            let projects = std::thread::spawn(move || worker.projects())
                .join()
                .map_err(|_| "project reader thread panicked".to_string())??;
            assert_eq!(projects, store.projects()?);
            assert_eq!(projects[0].branch.as_deref(), Some("reader-test"));
            assert_eq!(projects[0].remotes, ["https://github.com/example/repo.git"]);
            assert!(reader.open_approvals("a")?.is_empty());
            assert_eq!(reader.search("org1", "two", 20)?.len(), 1);
            // A reader has no way to write.
            assert!(
                reader
                    .with(|db| db.execute("delete from events", []).map_err(sql))
                    .is_err()
            );
            assert_eq!(reader.events_after("a", 0)?.len(), 2);
        }
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// A reader on the UI thread opens the store while a running session writes: it may not wait for the write lock.
    /// `Store::open` migrates and writes, so it waits the full busy timeout, then fails.
    #[test]
    fn a_reading_open_does_not_wait_for_a_held_write_lock() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("nexus-workbench-open-reading-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("w.sqlite3");
        let store = Store::open(&path)?;
        store.insert_thread(&record("a", "org1"))?;
        store.db.execute_batch("begin immediate").map_err(sql)?;
        store.append("a", 1, &user("one"))?;

        let started = std::time::Instant::now();
        let reading = Store::open_reading(&path)?;
        let found = reading.thread("a")?.map(|record| record.summary.id);
        let took = started.elapsed();
        store.db.execute_batch("commit").map_err(sql)?;

        assert_eq!(found.as_deref(), Some("a"));
        assert!(
            took < std::time::Duration::from_secs(1),
            "the reading open waited {took:?}"
        );
        assert!(
            reading.db.execute_batch("update threads set title = 'x'").is_err(),
            "a reading store wrote"
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn reads_go_on_while_the_store_is_held_or_writing() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("nexus-workbench-readers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for store in [Store::open(&dir.join("w.sqlite3"))?, Store::in_memory()?] {
            let reader = store.reader();
            store.insert_thread(&record("a", "org1"))?;
            store.append("a", 1, &user("one"))?;
            // A write in progress (not yet committed) does not stop a read.
            store.db.execute_batch("begin immediate").map_err(sql)?;
            store.append("a", 2, &user("two"))?;
            let during = reader.events_after("a", 0)?.len();
            // A file shows only what is committed; the shared in-memory database may show the row in flight. Either
            // way the committed row is there, so a reader on the wrong database fails here.
            assert!(if store.persistent() {
                during == 1
            } else {
                (1..=2).contains(&during)
            });
            store.db.execute_batch("commit").map_err(sql)?;
            assert_eq!(reader.events_after("a", 0)?.len(), 2);

            // The workbench keeps the store behind a lock; a reader does not wait for it.
            let shared = Mutex::new(store);
            let guard = shared.lock().map_err(sql)?;
            let from_another_thread = std::thread::scope(|scope| {
                scope
                    .spawn(|| reader.events_after("a", 0).map(|events| events.len()))
                    .join()
                    .map_err(|_| sql("reader thread panicked"))
            })??;
            assert_eq!(from_another_thread, 2);
            drop(guard);
        }
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn a_file_store_keeps_its_write_ahead_log() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("nexus-workbench-wal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir.join("w.sqlite3"))?;
        let mode: String = store
            .db
            .query_row("pragma journal_mode", [], |row| row.get(0))
            .map_err(sql)?;
        assert_eq!(mode, "wal");
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    fn text(text: &str, parent: Option<&str>) -> WbEvent {
        WbEvent::Text {
            id: "m".into(),
            text: text.into(),
            parent: parent.map(Into::into),
        }
    }

    /// One of each kind that mentions "migration": only the prompt and the main answer are searchable.
    fn mentions() -> Vec<WbEvent> {
        vec![
            user("Fix the flaky Migration in staging"),
            text("The migration now waits for the lock.", None),
            text("A sub-agent read the migration file.", Some("tool_1")),
            WbEvent::Thinking {
                id: "k".into(),
                text: "the migration again".into(),
                parent: None,
            },
            WbEvent::Tool {
                id: "tool_2".into(),
                name: "Bash".into(),
                input: serde_json::json!({"command": "cat migration.sql"}),
                parent: None,
            },
            WbEvent::ToolResult {
                images: vec![],
                id: "tool_2".into(),
                output: "create table migration".into(),
                is_error: false,
                truncated: false,
                parent: None,
            },
            WbEvent::User {
                text: String::new(),
                images: vec![crate::event::Image {
                    media_type: "image/png".into(),
                    data: "migration".into(),
                    name: Some("migration.png".into()),
                }],
                from: None,
            },
            // A linked session's message is not the person's: search leaves it out.
            WbEvent::User {
                text: "the migration a parent session asked for".into(),
                images: vec![],
                from: Some(crate::event::Sender::Session {
                    thread_id: "t_lead".into(),
                    title: "Lead".into(),
                }),
            },
            WbEvent::Notice {
                text: "migration notice".into(),
            },
        ]
    }

    fn indexed(store: &Store) -> Result<Vec<(String, i64)>> {
        store
            .db
            .prepare("select thread_id, seq from events_fts order by thread_id, seq")
            .map_err(sql)?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(sql)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql)
    }

    #[test]
    fn indexes_prompts_and_answers_as_they_are_appended() -> Result<()> {
        let store = Store::in_memory()?;
        store.insert_thread(&record("a", "org1"))?;
        for (at, event) in mentions().iter().enumerate() {
            store.append("a", at as i64, event)?;
        }
        assert_eq!(indexed(&store)?, vec![("a".to_string(), 1), ("a".to_string(), 2)]);
        // Any case, part of a word; one hit per session, with the match marked in its snippet.
        let hits = store.reader().search("org1", "MIGRAT", 20)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].thread_id, "a");
        assert!([1, 2].contains(&hits[0].seq));
        assert!(
            hits[0]
                .snippet
                .to_lowercase()
                .contains(&format!("{HIT_START}migrat{HIT_END}")),
            "{}",
            hits[0].snippet
        );
        // Every word has to be in the same message, in any order.
        assert_eq!(store.reader().search("org1", "staging flaky", 20)?[0].seq, 1);
        assert_eq!(store.reader().search("org1", "lock", 20)?[0].seq, 2);
        assert!(store.reader().search("org1", "staging lock", 20)?.is_empty());
        // Tool output, thinking, sub-agents, notices and image bytes are not searched.
        for missing in [
            "create table",
            "again",
            "sub-agent",
            "notice",
            "png",
            "cat ",
            "parent session",
        ] {
            assert!(
                store.reader().search("org1", missing, 20)?.is_empty(),
                "{missing} was found"
            );
        }
        Ok(())
    }

    #[test]
    fn searches_only_the_scope_and_newest_session_first() -> Result<()> {
        let store = Store::in_memory()?;
        let mut old = record("old", "org1");
        old.summary.updated_at = 1;
        let mut new = record("new", "org1");
        new.summary.updated_at = 5;
        store.insert_thread(&old)?;
        store.insert_thread(&new)?;
        store.insert_thread(&record("other", "org2"))?;
        for id in ["old", "new", "other"] {
            store.append(id, 1, &user("deploy the gateway"))?;
            store.append(id, 2, &text("the gateway is deployed", None))?;
        }
        let hits = store.reader().search("org1", "gateway", 20)?;
        let ids: Vec<&str> = hits.iter().map(|hit| hit.thread_id.as_str()).collect();
        assert_eq!(ids, ["new", "old"]);
        assert_eq!(store.reader().search("org1", "gateway", 1)?.len(), 1);
        let mut local = record("local", crate::session_scope::SessionScope::local_text());
        local.summary.updated_at = 3;
        store.insert_thread(&local)?;
        store.append("local", 1, &user("deploy the gateway"))?;
        let hits = store.reader().search_scopes(
            "org1",
            Some(crate::session_scope::SessionScope::local_text()),
            "gateway",
            2,
        )?;
        let ids: Vec<_> = hits.iter().map(|hit| hit.thread_id.as_str()).collect();
        assert_eq!(ids, ["new", "local"]);
        Ok(())
    }

    #[test]
    fn a_session_with_many_matches_does_not_crowd_the_others_out() -> Result<()> {
        let store = Store::in_memory()?;
        let mut long = record("long", "org1");
        long.summary.updated_at = 9;
        let mut short = record("short", "org1");
        short.summary.updated_at = 1;
        store.insert_thread(&long)?;
        store.insert_thread(&short)?;
        // Far more matching messages than the old row cap, all better matches than the other session's.
        for at in 0..700 {
            store.append("long", at, &user("fix fix fix the fix"))?;
        }
        store.append(
            "short",
            1,
            &user("a long message that mentions one fix somewhere inside of it"),
        )?;
        let ids: Vec<String> = store
            .reader()
            .search("org1", "fix", 20)?
            .into_iter()
            .map(|hit| hit.thread_id)
            .collect();
        assert_eq!(ids, ["long", "short"]);
        assert_eq!(store.reader().search("org1", "fix", 1)?.len(), 1);
        Ok(())
    }

    #[test]
    fn short_words_and_query_syntax_are_taken_as_text() -> Result<()> {
        let store = Store::in_memory()?;
        store.insert_thread(&record("a", "org1"))?;
        store.append("a", 1, &user("run \"npm test\" AND fix(core): it's NEAR done"))?;
        let reader = store.reader();
        assert!(reader.search("org1", "", 20)?.is_empty());
        assert!(reader.search("org1", "fi", 20)?.is_empty());
        for query in ["\"npm", "fix(core)", "AND", "NEAR", "it's", "npm test\"", "*", "core):"] {
            reader.search("org1", query, 20)?;
        }
        assert_eq!(reader.search("org1", "fix(core)", 20)?.len(), 1);
        assert_eq!(reader.search("org1", "it's", 20)?.len(), 1);
        // A word under three letters is skipped, the rest still has to match.
        assert_eq!(reader.search("org1", "it npm", 20)?.len(), 1);
        Ok(())
    }

    #[test]
    fn indexes_the_logs_of_a_file_from_before_search_once() -> Result<()> {
        let db = Connection::open_in_memory().map_err(sql)?;
        db.execute_batch(SCHEMA).map_err(sql)?;
        let old = Store {
            db,
            target: Arc::new(Target::Memory(String::new())),
            ledger: None,
            persistent: false,
        };
        old.insert_thread(&record("a", "org1"))?;
        // Written straight to the log, the way a version without the index did.
        for (seq, event) in mentions().iter().enumerate() {
            old.db
                .execute(
                    "insert into events (thread_id, seq, at, body) values ('a', ?1, 0, ?2)",
                    params![seq as i64 + 1, serde_json::to_string(event).map_err(sql)?],
                )
                .map_err(sql)?;
        }
        let store = Store::init(old.db, None, false, Target::Memory(String::new()))?;
        // The same messages as indexing on append picks.
        assert_eq!(indexed(&store)?, vec![("a".to_string(), 1), ("a".to_string(), 2)]);
        let store = Store::init(store.db, None, false, Target::Memory(String::new()))?;
        assert_eq!(indexed(&store)?.len(), 2, "a second open indexes nothing again");
        store.append("a", 9, &user("and the migration after"))?;
        assert_eq!(indexed(&store)?.len(), 3);
        Ok(())
    }

    #[test]
    fn a_deleted_session_leaves_the_index() -> Result<()> {
        let store = Store::in_memory()?;
        store.insert_thread(&record("a", "org1"))?;
        store.insert_thread(&record("b", "org1"))?;
        store.append("a", 1, &user("rotate the signing key"))?;
        store.append("b", 1, &user("rotate the signing key"))?;
        store
            .db
            .execute("delete from threads where id = 'a'", [])
            .map_err(sql)?;
        assert_eq!(indexed(&store)?, vec![("b".to_string(), 1)]);
        Ok(())
    }

    #[test]
    fn removed_projects_do_not_keep_or_count_start_choices() -> Result<()> {
        let store = Store::in_memory()?;
        let root = std::env::temp_dir();
        let kept = store.add_project(&root.join("kept"), 1)?;
        let removed = store.add_project(&root.join("removed"), 2)?;
        store.set_start_preference("scope", "person", &kept.id, "ask")?;
        store.set_start_preference("scope", "person", &removed.id, "here")?;
        assert_eq!(store.start_preference_overrides("scope", "person", false)?, 2);

        store.remove_project(&removed.id)?;
        assert_eq!(store.start_preference("scope", "person", &removed.id)?, None);
        // An older app could have removed a project without cleaning up its choice.
        store.set_start_preference("scope", "person", &removed.id, "new")?;
        assert_eq!(store.start_preference_overrides("scope", "person", false)?, 1);
        assert_eq!(store.start_preference_overrides("scope", "person", true)?, 0);
        assert_eq!(store.start_preference("scope", "person", &kept.id)?, None);
        assert_eq!(store.start_preference("scope", "person", &removed.id)?, None);
        Ok(())
    }

    #[test]
    fn adds_a_folder_once() -> Result<()> {
        let store = Store::in_memory()?;
        let dir = std::env::temp_dir();
        let first = store.add_project(&dir, 1)?;
        let again = store.add_project(&dir, 2)?;
        assert_eq!(first.id, again.id);
        store.remove_project(&first.id)?;
        assert!(store.projects()?.is_empty());
        Ok(())
    }
}
