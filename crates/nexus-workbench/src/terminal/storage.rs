//! Durable receipts and metadata. Raw PTY output and private input never enter this journal.
use super::*;
use crate::terminal::Info;

pub(super) const SCHEMA: &str = "
create table if not exists terminal_commands (
  id text primary key,
  scope text not null,
  identity text not null,
  thread_id text not null,
  request_id text not null,
  digest text not null,
  body text not null,
  unique(scope, identity, thread_id, request_id)
);
create index if not exists terminal_commands_thread on terminal_commands(scope, identity, thread_id);
create table if not exists terminal_command_events (
  command_id text not null,
  seq integer not null,
  at integer not null,
  kind text not null,
  body text not null,
  primary key(command_id, seq)
);
";

impl Store {
    pub(crate) fn terminal_insert(
        &self,
        scope: &str,
        identity: &str,
        thread: &str,
        key: &str,
        digest: &str,
        info: &Info,
    ) -> Result<()> {
        // A receipt and its first event commit together, before a process can start.
        let body = serde_json::to_string(&info.for_storage()).map_err(sql)?;
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        tx.execute("insert into terminal_commands (id, scope, identity, thread_id, request_id, digest, body) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![info.id, scope, identity, thread, key, digest, body]).map_err(sql)?;
        tx.execute(
            "insert into terminal_command_events (command_id, seq, at, kind, body) values (?1, 1, ?2, 'recorded', ?3)",
            params![info.id, crate::now_ms(), body],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }

    pub(crate) fn terminal_save(&self, info: &Info, kind: &str) -> Result<()> {
        let body = serde_json::to_string(&info.for_storage()).map_err(sql)?;
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        let changed = tx
            .execute(
                "update terminal_commands set body = ?2 where id = ?1",
                params![info.id, body],
            )
            .map_err(sql)?;
        if changed != 1 {
            return Err("terminal_history_missing: The session or command was deleted.".into());
        }
        tx.execute("insert into terminal_command_events (command_id, seq, at, kind, body) select ?1, coalesce(max(seq), 0) + 1, ?2, ?3, ?4 from terminal_command_events where command_id = ?1",
            params![info.id, crate::now_ms(), kind, body]).map_err(sql)?;
        tx.commit().map_err(sql)
    }

    pub(crate) fn terminal_receipt(
        &self,
        scope: &str,
        identity: &str,
        thread: &str,
        key: &str,
    ) -> Result<Option<(String, Info)>> {
        self.db.query_row("select digest, body from terminal_commands where scope = ?1 and identity = ?2 and thread_id = ?3 and request_id = ?4",
            params![scope, identity, thread, key], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .optional().map_err(sql)?.map(|(digest, body)| Ok((digest, serde_json::from_str(&body).map_err(sql)?))).transpose()
    }

    pub(crate) fn terminal_history(&self, scope: &str, identity: &str, thread: &str) -> Result<Vec<Info>> {
        let mut query = self.db.prepare_cached("select body from terminal_commands where scope = ?1 and identity = ?2 and thread_id = ?3 order by rowid").map_err(sql)?;
        let rows = query
            .query_map(params![scope, identity, thread], |row| row.get::<_, String>(0))
            .map_err(sql)?;
        rows.map(|row| serde_json::from_str(&row.map_err(sql)?).map_err(sql))
            .collect()
    }

    /// Shared agent shell input is an intent, not proof of a command boundary or exit.
    pub(crate) fn terminal_submission(&self, info: &Info, input: &str) -> Result<()> {
        let snapshot = info.for_storage();
        let body = serde_json::to_string(&snapshot).map_err(sql)?;
        let submitted =
            serde_json::to_string(&serde_json::json!({"terminal":snapshot,"submittedInput":input})).map_err(sql)?;
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        if tx
            .execute(
                "update terminal_commands set body = ?2 where id = ?1",
                params![info.id, body],
            )
            .map_err(sql)?
            != 1
        {
            return Err("terminal_history_missing".into());
        }
        tx.execute("insert into terminal_command_events (command_id, seq, at, kind, body) select ?1, coalesce(max(seq), 0) + 1, ?2, 'shellInputSubmitted', ?3 from terminal_command_events where command_id = ?1", params![info.id, crate::now_ms(), submitted]).map_err(sql)?;
        tx.commit().map_err(sql)
    }

    pub(crate) fn terminal_submissions(&self, id: &str) -> Result<Vec<String>> {
        let mut query = self.db.prepare_cached("select json_extract(body, '$.submittedInput') from terminal_command_events where command_id = ?1 and kind = 'shellInputSubmitted' order by seq desc limit 64").map_err(sql)?;
        let mut rows = query
            .query_map([id], |row| row.get::<_, String>(0))
            .map_err(sql)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql)?;
        rows.reverse();
        Ok(rows)
    }

    pub(crate) fn terminal_recover(&self) -> Result<()> {
        let bodies = self
            .db
            .prepare("select body from terminal_commands")
            .map_err(sql)?
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(sql)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql)?;
        for body in bodies {
            let mut info: Info = serde_json::from_str(&body).map_err(sql)?;
            match info.status.as_str() {
                "running" | "starting" | "stopping" | "providerPending" => info.status = "unknown".into(),
                "awaitingUser" => info.status = "cancelled".into(),
                _ => continue,
            }
            info.owner = "user".into();
            info.lease += 1;
            info.result_shared = false;
            self.terminal_save(&info, "recovered")?;
        }
        Ok(())
    }
}
