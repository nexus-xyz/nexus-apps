//! Hooks' rows in the store (`crate::hooks`): which hook ran on which event (claimed once), and each hook's last runs.

use rusqlite::params;

use super::{Result, Store, sql};
use crate::hooks::{HookRun, KEEP_RUNS};

pub(super) const SCHEMA: &str = "
create table if not exists hook_claims (
  hook text not null,
  thread_id text not null,
  event text not null,
  event_key text not null,
  claimed_at integer not null,
  primary key (hook, thread_id, event, event_key)
) without rowid;
create table if not exists hook_runs (
  id integer primary key autoincrement,
  hook text not null,
  event text not null,
  thread_id text not null,
  started_at integer not null,
  exit_code integer,
  timed_out integer not null default 0,
  output text not null default ''
);
create index if not exists hook_runs_hook on hook_runs (hook, id);
";

impl Store {
    /// Claims the hook's run on this event. False when it was claimed before: then it does not run again.
    pub fn claim_hook(&self, hook: &str, thread_id: &str, event: &str, key: &str) -> Result<bool> {
        let added = self
            .db
            .execute(
                "insert or ignore into hook_claims (hook, thread_id, event, event_key, claimed_at) \
                 values (?1, ?2, ?3, ?4, ?5)",
                params![hook, thread_id, event, key, crate::now_ms()],
            )
            .map_err(sql)?;
        Ok(added == 1)
    }

    /// Keeps a run, and only the hook's last [`KEEP_RUNS`].
    pub fn add_hook_run(&self, run: &HookRun) -> Result<()> {
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        tx.execute(
            "insert into hook_runs (hook, event, thread_id, started_at, exit_code, timed_out, output) \
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                run.hook,
                run.event,
                run.thread_id,
                run.started_at,
                run.exit_code,
                run.timed_out,
                run.output
            ],
        )
        .map_err(sql)?;
        tx.execute(
            "delete from hook_runs where hook = ?1 and id not in \
             (select id from hook_runs where hook = ?1 order by id desc limit ?2)",
            params![run.hook, KEEP_RUNS as i64],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }

    /// Every hook's kept runs, newest first.
    pub fn hook_runs(&self) -> Result<Vec<HookRun>> {
        let mut statement = self
            .db
            .prepare(
                "select hook, event, thread_id, started_at, exit_code, timed_out, output from hook_runs \
                 order by id desc",
            )
            .map_err(sql)?;
        let rows = statement
            .query_map([], |row| {
                Ok(HookRun {
                    hook: row.get(0)?,
                    event: row.get(1)?,
                    thread_id: row.get(2)?,
                    started_at: row.get(3)?,
                    exit_code: row.get(4)?,
                    timed_out: row.get(5)?,
                    output: row.get(6)?,
                })
            })
            .map_err(sql)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(sql)
    }
}
