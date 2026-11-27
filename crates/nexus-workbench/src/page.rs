//! A page of a session's log, so a long session opens from its end (`wb_thread_page`) and older pages load as the
//! person scrolls up, instead of the whole log in one reply.
//!
//! A page starts where the window's folds (timeline.ts, queued.ts) need nothing from before it: at a message the person
//! sent while no turn ran. Approvals waiting, queued messages and the turn in progress all sit after that point. So a
//! page can be longer than asked for: it reaches back to the start of the turn in progress.
//!
//! A few things last across turns: the slash commands the agent last reported, its last plan (a main-agent TodoWrite)
//! and the last list of background tasks. The latest page carries the latest of each from before it (`carried`).

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::event::{StoredEvent, WbEvent};
use crate::store::{Result, read_events, sql};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsPage {
    /// Contiguous by seq, oldest first.
    pub events: Vec<StoredEvent>,
    /// The latest page only: events from before it whose state still holds, oldest first. Not part of the timeline.
    pub carried: Vec<StoredEvent>,
    /// Older events exist before `events`.
    pub more: bool,
}

/// The `limit`-th latest event's seq below `before`: where a page of `limit` would start.
const NTH_LATEST: &str = "select seq from events where thread_id = ?1 and seq < ?2 order by seq desc limit 1 offset ?3";
/// What kind each event is, newest first from `?2` down.
const KINDS_BACK: &str =
    "select seq, json_extract(body, '$.kind') from events where thread_id = ?1 and seq <= ?2 order by seq desc";
const RANGE: &str = "select seq, at, body from events where thread_id = ?1 and seq >= ?2 and seq < ?3 order by seq";
/// The events whose state lasts across turns: an index holds just them, so finding the latest of each does not read
/// the whole log (images and all) back to its start. The query repeats the index's condition word for word, so SQLite
/// can use it (and is told to: the primary key looks as good to it).
macro_rules! lasting_kinds {
    () => {
        "(json_extract(body, '$.kind') in ('commands', 'backgroundTasks', 'connection', 'rewind')
          or (json_extract(body, '$.kind') = 'tool' and json_extract(body, '$.name') = 'TodoWrite'))"
    };
}
/// Made at start-up (store.rs); on a file from before it, filled from the logs once.
pub(crate) const LASTING_INDEX: &str = concat!(
    "drop index if exists events_lasting; drop index if exists events_lasting_connection; create index if not exists events_lasting_rewind on events (thread_id, seq) where ",
    lasting_kinds!(),
    ";"
);
/// The lasting events below `?2`, newest first.
const LASTING_BACK: &str = concat!(
    "select seq, at, body from events indexed by events_lasting_rewind where thread_id = ?1 and seq < ?2 and ",
    lasting_kinds!(),
    " order by seq desc"
);

/// The page of at least `limit` events before `before` (None: the latest), reaching back to a quiet start; with no
/// `limit`, every event before `before`.
pub fn events_page(db: &Connection, thread_id: &str, before: Option<i64>, limit: Option<usize>) -> Result<EventsPage> {
    let below = before.unwrap_or(i64::MAX);
    let start = match limit {
        None | Some(0) => 1,
        Some(limit) => {
            let offset = i64::try_from(limit - 1).unwrap_or(i64::MAX);
            let nth: Option<i64> = db
                .prepare_cached(NTH_LATEST)
                .map_err(sql)?
                .query_row(params![thread_id, below, offset], |row| row.get(0))
                .optional()
                .map_err(sql)?;
            match nth {
                Some(seq) if seq > 1 => quiet_start(db, thread_id, seq)?,
                _ => 1,
            }
        }
    };
    let mut range = db.prepare_cached(RANGE).map_err(sql)?;
    let events = read_events(&mut range, params![thread_id, start, below], thread_id)?;
    let carried = if before.is_none() && start > 1 {
        lasting(db, thread_id, start)?
    } else {
        Vec::new()
    };
    Ok(EventsPage {
        events,
        carried,
        more: start > 1,
    })
}

/// The latest seq at or before `from` where a message starts a turn while none runs: the earliest `user` event since a
/// turn end (or none), with no turn start between. Later messages before the next turn start were sent while it
/// launched, so they are queued behind it. 1 when there is none.
fn quiet_start(db: &Connection, thread_id: &str, from: i64) -> Result<i64> {
    let mut statement = db.prepare_cached(KINDS_BACK).map_err(sql)?;
    let mut rows = statement.query(params![thread_id, from]).map_err(sql)?;
    // The earliest message since the last turn marker passed (going back, so the last one seen).
    let mut message: Option<i64> = None;
    while let Some(row) = rows.next().map_err(sql)? {
        let seq: i64 = row.get(0).map_err(sql)?;
        let kind: Option<String> = row.get(1).map_err(sql)?;
        match kind.as_deref() {
            Some("user") => message = Some(seq),
            Some("turnEnd") if message.is_some() => return Ok(message.unwrap_or(1)),
            // Messages after a turn start were sent while it ran (queued): not a quiet start.
            Some("turnStart" | "turnEnd") => message = None,
            _ => {}
        }
    }
    Ok(1)
}

/// The latest commands, plan and background-task list from before `start`, oldest first.
fn lasting(db: &Connection, thread_id: &str, start: i64) -> Result<Vec<StoredEvent>> {
    let mut statement = db.prepare_cached(LASTING_BACK).map_err(sql)?;
    let mut rows = statement.query(params![thread_id, start]).map_err(sql)?;
    let mut picked: Vec<StoredEvent> = Vec::new();
    let mut seen = [false; 4];
    while let Some(row) = rows.next().map_err(sql)? {
        let (seq, at, body): (i64, i64, String) = (
            row.get(0).map_err(sql)?,
            row.get(1).map_err(sql)?,
            row.get(2).map_err(sql)?,
        );
        let Ok(event) = serde_json::from_str::<WbEvent>(&body) else {
            continue;
        };
        let slot = match &event {
            // The conversation's old plan stops at a rewind, even when the marker is on an older page.
            WbEvent::Rewind { .. } => {
                seen[2] = true;
                continue;
            }
            WbEvent::Connection { .. } => 3,
            WbEvent::Commands { .. } => 0,
            WbEvent::BackgroundTasks { .. } => 1,
            // A sub-agent's plan is its own (plan-steps.ts).
            WbEvent::Tool { parent: None, .. } => 2,
            _ => continue,
        };
        if !seen[slot] {
            seen[slot] = true;
            picked.push(StoredEvent {
                thread_id: thread_id.into(),
                seq,
                at,
                event,
            });
        }
        if seen.iter().all(|s| *s) {
            break;
        }
    }
    picked.reverse();
    Ok(picked)
}

#[cfg(test)]
mod tests {
    use rusqlite::params;
    use serde_json::{Value, json};

    use crate::event::StoredEvent;
    use crate::store::{Result, Store};

    fn user(text: &str) -> Value {
        json!({ "kind": "user", "text": text })
    }

    fn start(turn: usize) -> Value {
        json!({ "kind": "turnStart", "turnId": format!("t{turn}") })
    }

    fn end(turn: usize) -> Value {
        json!({ "kind": "turnEnd", "turnId": format!("t{turn}"), "status": "completed", "costUsd": null, "durationMs": 1, "error": null })
    }

    fn text(id: &str) -> Value {
        json!({ "kind": "text", "id": id, "text": "done", "parent": null })
    }

    fn todo(parent: Option<&str>) -> Value {
        json!({ "kind": "tool", "id": "todo", "name": "TodoWrite", "input": { "todos": [] }, "parent": parent })
    }

    /// Appends each event, in order, to session "a".
    fn put(store: &Store, events: impl IntoIterator<Item = Value>) -> Result<()> {
        for value in events {
            store.append("a", 0, &serde_json::from_value(value).map_err(|e| e.to_string())?)?;
        }
        Ok(())
    }

    /// A session of `turns` turns of four events each (message, start, answer, end), after a commands and two plans.
    fn session(store: &Store, turns: usize) -> Result<()> {
        put(
            store,
            [
                json!({ "kind": "commands", "commands": ["review"] }),
                todo(None),
                todo(Some("sub")),
            ],
        )?;
        for turn in 1..=turns {
            put(store, [user("go"), start(turn), text(&format!("x{turn}")), end(turn)])?;
        }
        Ok(())
    }

    fn seqs(events: &[StoredEvent]) -> Vec<i64> {
        events.iter().map(|e| e.seq).collect()
    }

    #[test]
    fn the_latest_page_starts_at_a_message_and_carries_what_lasts() -> Result<()> {
        let store = Store::in_memory()?;
        session(&store, 10)?;
        // 43 events. Six from the end is seq 38 (the answer of turn 9): the page reaches back to its message, 36.
        let page = store.reader().events_page("a", None, Some(6))?;
        assert_eq!(seqs(&page.events), (36..=43).collect::<Vec<_>>());
        assert!(page.more);
        // The commands and the main agent's plan, not a sub-agent's.
        assert_eq!(seqs(&page.carried), [1, 2]);
        // An older page ends before it, and carries nothing.
        let older = store.reader().events_page("a", Some(36), Some(4))?;
        assert_eq!(seqs(&older.events), (32..36).collect::<Vec<_>>());
        assert!(older.carried.is_empty());
        // No limit: everything before.
        let rest = store.reader().events_page("a", Some(32), None)?;
        assert_eq!(seqs(&rest.events), (1..32).collect::<Vec<_>>());
        assert!(!rest.more);
        Ok(())
    }

    #[test]
    fn the_latest_page_carries_only_the_latest_safe_connection_snapshot() -> Result<()> {
        let store = Store::in_memory()?;
        let snapshot = |generation: &str| {
            json!({ "kind": "connection", "connection": crate::event::SessionConnection {
                generation: generation.into(), ..Default::default()
            }})
        };
        put(&store, [snapshot("old"), snapshot("current")])?;
        session(&store, 10)?;
        let page = store.reader().events_page("a", None, Some(4))?;
        let connections: Vec<_> = page
            .carried
            .iter()
            .filter_map(|stored| match &stored.event {
                crate::event::WbEvent::Connection { connection } => Some(connection.generation.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(connections, ["current"]);
        assert!(
            store
                .reader()
                .events_page("a", Some(page.events[0].seq), Some(4))?
                .carried
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn the_scan_for_what_lasts_uses_its_index() -> Result<()> {
        let store = Store::in_memory()?;
        let plan: Vec<String> = store.reader().with_db(|db| {
            let mut statement = db
                .prepare(&format!("explain query plan {}", super::LASTING_BACK))
                .map_err(|e| e.to_string())?;
            let rows = statement
                .query_map(params!["a", 1], |row| row.get::<_, String>(3))
                .map_err(|e| e.to_string())?;
            rows.map(|row| row.map_err(|e| e.to_string())).collect()
        })?;
        assert!(plan.join(" ").contains("events_lasting"), "{plan:?}");
        Ok(())
    }

    #[test]
    fn a_rewind_on_an_older_page_clears_the_carried_plan() -> Result<()> {
        let store = Store::in_memory()?;
        session(&store, 2)?;
        put(
            &store,
            [
                json!({ "kind": "rewind", "before": 8 }),
                user("replacement"),
                start(3),
                text("new"),
                end(3),
            ],
        )?;
        let page = store.reader().events_page("a", None, Some(2))?;
        assert_eq!(seqs(&page.carried), [1]);
        // A new plan after the rewind is still carried into later turns.
        put(&store, [todo(None), user("continue"), start(4), text("next"), end(4)])?;
        let page = store.reader().events_page("a", None, Some(2))?;
        assert_eq!(seqs(&page.carried), [1, 17]);
        Ok(())
    }

    #[test]
    fn a_page_reaches_back_past_queued_messages_to_the_turn_in_progress() -> Result<()> {
        let store = Store::in_memory()?;
        session(&store, 2)?;
        // Turn 3 runs; two messages are queued behind it, then handed a turn of their own with no end yet.
        put(
            &store,
            [
                user("go"),
                start(3),
                text("a"),
                user("queued 1"),
                user("queued 2"),
                end(3),
                start(4),
                text("b"),
            ],
        )?;
        // 19 events; the 2 latest are in turn 4, whose messages were queued in turn 3: the page starts at turn 3's message.
        let page = store.reader().events_page("a", None, Some(2))?;
        assert_eq!(seqs(&page.events), (12..=19).collect::<Vec<_>>());
        // A session shorter than the page is read whole.
        let whole = store.reader().events_page("a", None, Some(400))?;
        assert_eq!(seqs(&whole.events), (1..=19).collect::<Vec<_>>());
        assert!(!whole.more && whole.carried.is_empty());
        Ok(())
    }

    #[test]
    fn a_page_starts_at_the_first_of_two_messages_sent_while_none_runs() -> Result<()> {
        let store = Store::in_memory()?;
        session(&store, 2)?;
        // The second message is sent while turn 3 launches, so it is queued behind the first.
        put(&store, [user("A"), user("B"), start(3), text("a"), text("b")])?;
        // 16 events; the 2 latest are in turn 3, which the first message (12) starts.
        let page = store.reader().events_page("a", None, Some(2))?;
        assert_eq!(seqs(&page.events), (12..=16).collect::<Vec<_>>());
        Ok(())
    }

    /// Before and after on a long session with images, on disk: `cargo test -p nexus-workbench --release --lib
    /// page::tests::measure -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn measure() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("nexus-page-measure-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let store = Store::open(&dir.join("wb.db"))?;
        // About 5,000 events in turns of 51: a message (with a 300 KB image every fifth turn), then 12 steps of a tool
        // call, its 1 KB result, a usage and a text, then the turn's end.
        let image = "A".repeat(300_000);
        let filler = "lorem ipsum dolor sit amet ".repeat(40);
        put(&store, [json!({ "kind": "commands", "commands": ["review"] })])?;
        for turn in 1..=99 {
            let images = if turn % 5 == 0 {
                json!([{ "mediaType": "image/png", "data": image, "name": null }])
            } else {
                json!([])
            };
            put(
                &store,
                [
                    json!({ "kind": "user", "text": format!("turn {turn}"), "images": images }),
                    start(turn),
                ],
            )?;
            for step in 0..12 {
                let id = format!("t{turn}s{step}");
                put(
                    &store,
                    [
                        json!({ "kind": "tool", "id": id, "name": "Bash", "input": { "command": "ls" }, "parent": null }),
                        json!({ "kind": "toolResult", "id": id, "output": filler, "isError": false, "truncated": false }),
                        json!({ "kind": "usage", "model": "m", "inputTokens": 1, "outputTokens": 1, "cacheReadTokens": 0, "cacheWriteTokens": 0, "costUsd": 0.001 }),
                        text(&format!("{id}x")),
                    ],
                )?;
            }
            put(&store, [end(turn)])?;
        }
        let reader = store.reader();
        for _ in 0..3 {
            let at = std::time::Instant::now();
            let whole = reader.events_after("a", 0)?;
            let whole_ms = at.elapsed().as_secs_f64() * 1e3;
            let whole_bytes = serde_json::to_vec(&whole).map_err(|e| e.to_string())?.len();
            let at = std::time::Instant::now();
            let page = reader.events_page("a", None, Some(400))?;
            let page_ms = at.elapsed().as_secs_f64() * 1e3;
            let page_bytes = serde_json::to_vec(&page).map_err(|e| e.to_string())?.len();
            println!(
                "[measure] whole: {} events, {:.1} MB, read {whole_ms:.1} ms | latest page: {} events (+{} carried), {:.2} MB, read {page_ms:.1} ms",
                whole.len(),
                whole_bytes as f64 / 1e6,
                page.events.len(),
                page.carried.len(),
                page_bytes as f64 / 1e6
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
