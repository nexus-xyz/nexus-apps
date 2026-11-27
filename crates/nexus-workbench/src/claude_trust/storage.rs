//! Each project's answer about its own Claude settings (`crate::claude_trust`). No row: never asked.

use rusqlite::{OptionalExtension, params};

use super::{Result, Store, sql};

pub(super) const SCHEMA: &str = "
create table if not exists project_claude_trust (
  project_id text primary key,
  trusted integer not null,
  answered_at integer not null
);
";

impl Store {
    /// The project's answer; None when nobody answered.
    pub fn claude_trust(&self, project_id: &str) -> Result<Option<bool>> {
        self.db
            .query_row(
                "select trusted from project_claude_trust where project_id = ?1",
                [project_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)
    }

    /// Keeps the person's answer.
    pub fn set_claude_trust(&self, project_id: &str, trusted: bool) -> Result<()> {
        self.db
            .execute(
                "insert into project_claude_trust (project_id, trusted, answered_at) values (?1, ?2, ?3)
                 on conflict (project_id) do update set trusted = excluded.trusted, answered_at = excluded.answered_at",
                params![project_id, trusted, crate::now_ms()],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// Forgets it, with the project.
    pub(super) fn forget_claude_trust(&self, project_id: &str) -> Result<()> {
        self.db
            .execute("delete from project_claude_trust where project_id = ?1", [project_id])
            .map_err(sql)?;
        Ok(())
    }
}
