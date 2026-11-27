//! The CLI's transcripts beside its threads (`crate::cli_log`): which conversation each thread logs, and how far its
//! transcript was read.

use rusqlite::{OptionalExtension, params};

use super::{Result, Store, ThreadRecord, sql};
use crate::cli_log::CliThread;
use crate::event::{SessionAgent, WbEvent};
use crate::session_sync::Identity;

pub(super) const SCHEMA: &str = "
create table if not exists cli_transcripts (
  thread_id text primary key,
  scope text not null,
  agent text not null,
  native_id text not null,
  read_to integer not null default 0 check (read_to >= 0),
  unique (scope, agent, native_id)
);
";

impl Store {
    pub(crate) fn cli_transcript(
        &self,
        scope: &str,
        agent: SessionAgent,
        native_id: &str,
    ) -> Result<Option<CliThread>> {
        self.db
            .query_row(
                "select thread_id, read_to from cli_transcripts where scope = ?1 and agent = ?2 and native_id = ?3",
                params![scope, agent.as_str(), native_id],
                |row| {
                    Ok(CliThread {
                        id: row.get(0)?,
                        read_to: row.get::<_, i64>(1)? as u64,
                    })
                },
            )
            .optional()
            .map_err(sql)
    }

    /// Inserts the thread, binds it to `identity` and records its conversation, all or nothing. False when sync did not
    /// bind it (an in-memory store): then nothing is kept.
    pub(crate) fn insert_cli_thread(&self, record: &ThreadRecord, identity: &Identity) -> Result<bool> {
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        self.insert_thread(record)?;
        self.sync_register(&record.summary.id, identity)?;
        if self.sync_target(&record.summary.id)?.is_none() {
            return Ok(false);
        }
        tx.execute(
            "insert into cli_transcripts (thread_id, scope, agent, native_id) values (?1, ?2, ?3, ?4)",
            params![
                record.summary.id,
                record.scope,
                record.summary.agent.as_str(),
                record.native_id
            ],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)?;
        Ok(true)
    }

    /// Appends `events` to a CLI thread, all or nothing, and keeps its transcript mark.
    pub(crate) fn cli_events(&self, thread: &str, events: &[(i64, WbEvent)]) -> Result<()> {
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        let known: Option<i64> = tx
            .query_row(
                "select 1 from cli_transcripts where thread_id = ?1",
                params![thread],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?;
        if known.is_none() {
            return Err("nexus-workbench: that CLI thread is gone".into());
        }
        for (at, event) in events {
            self.append(thread, *at, event)?;
        }
        tx.commit().map_err(sql)
    }

    pub(crate) fn cli_append(
        &self,
        thread: &str,
        events: &[(i64, WbEvent)],
        read_to: u64,
        title: Option<&str>,
    ) -> Result<()> {
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        for (at, event) in events {
            self.append(thread, *at, event)?;
        }
        let read_to = i64::try_from(read_to).map_err(sql)?;
        let moved = tx
            .execute(
                "update cli_transcripts set read_to = ?2 where thread_id = ?1 and read_to <= ?2",
                params![thread, read_to],
            )
            .map_err(sql)?;
        if moved != 1 {
            return Err("nexus-workbench: that CLI thread is gone, or its transcript was read further".into());
        }
        if let Some(title) = title {
            tx.execute(
                "update threads set title = ?2, automatic_title = 0 where id = ?1 and automatic_title = 1",
                params![thread, title],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)
    }

    pub(crate) fn cli_unsynced(&self, identity: &Identity) -> Result<u64> {
        let count: i64 = self
            .db
            .query_row(
                "select count(*) from session_sync_threads s join events e on e.thread_id = s.thread_id
                 where s.scope = ?1 and s.owner = ?2 and e.seq > s.confirmed",
                params![identity.scope, identity.user],
                |row| row.get(0),
            )
            .map_err(sql)?;
        Ok(count as u64)
    }
}
