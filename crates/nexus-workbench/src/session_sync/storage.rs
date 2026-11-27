//! SQLite state beside the authoritative event log. Staging means the network outcome may be unknown.

use rusqlite::{OptionalExtension, params};

use super::{Result, Store, sql};
use crate::session_sync::{
    Batch, FailureKind, Identity, MAX_BATCH_BYTES, MAX_EVENTS, MAX_PLAINTEXT, PrivateThread, RawEvent, Target,
};

type Checked<T> = std::result::Result<T, FailureKind>;

pub(super) const SCHEMA: &str = "
create table if not exists session_sync_threads (
  thread_id text primary key,
  scope text not null,
  owner text not null,
  sync_id text not null unique,
  confirmed integer not null default 0 check (confirmed >= 0),
  epoch integer check (epoch >= 0),
  last_attempt integer not null default 0
);
create index if not exists session_sync_identity on session_sync_threads (scope, owner, last_attempt);
create table if not exists session_sync_outbox (
  thread_id text primary key,
  batch text not null
);
create table if not exists session_sync_deletions (
  sync_id text primary key,
  thread_id text not null,
  scope text not null,
  owner text not null,
  last_attempt integer not null default 0
);
create index if not exists session_sync_deletion_identity on session_sync_deletions (scope, owner, last_attempt);
";

fn db<T>(result: rusqlite::Result<T>) -> Checked<T> {
    result.map_err(|_| FailureKind::LocalStore)
}

fn value<T>(result: Result<T>) -> Checked<T> {
    result.map_err(|_| FailureKind::LocalStore)
}

fn read_target(row: &rusqlite::Row<'_>) -> rusqlite::Result<Target> {
    Ok(Target {
        local: row.get(0)?,
        identity: Identity {
            scope: row.get(1)?,
            user: row.get(2)?,
        },
        sync_id: row.get(3)?,
        confirmed: row.get(4)?,
        epoch: row.get::<_, Option<i64>>(5)?.map(|epoch| epoch as u64),
        deleted: row.get(6)?,
    })
}

const COLUMNS: &str = "s.thread_id, s.scope, s.owner, s.sync_id, s.confirmed, s.epoch, 0";

/// Unbinds the sessions that were bound to sync before [`Store::sync_register`] checked the agent. Gateway refuses
/// every upload of an agent that does not sync ([`crate::session_sync::syncs`]), so such a binding retries forever
/// and confirms nothing. Each one with nothing confirmed becomes a deletion, as in [`Store::delete_thread`], which also
/// removes any content key Gateway holds for it. The session itself stays, on this computer only.
pub(super) fn unbind_unsynced_agents(db: &rusqlite::Connection) -> Result<()> {
    let bound: Vec<(String, String)> = {
        let mut statement = db
            .prepare(
                "select s.thread_id, t.agent from session_sync_threads s join threads t on t.id = s.thread_id
                 where s.confirmed = 0",
            )
            .map_err(sql)?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(sql)?
            .collect::<std::result::Result<_, _>>()
            .map_err(sql)?
    };
    // An agent this version does not know may be one a newer version syncs: its binding stays.
    let unsynced: Vec<String> = bound
        .into_iter()
        .filter(|(_, agent)| {
            crate::event::SessionAgent::parse(agent).is_some_and(|agent| !crate::session_sync::syncs(agent))
        })
        .map(|(thread, _)| thread)
        .collect();
    if unsynced.is_empty() {
        return Ok(());
    }
    let tx = db.unchecked_transaction().map_err(sql)?;
    for thread in &unsynced {
        tx.execute(
            "insert into session_sync_deletions (thread_id, scope, owner, sync_id, last_attempt)
             select thread_id, scope, owner, sync_id, last_attempt from session_sync_threads
             where thread_id = ?1 and confirmed = 0",
            [thread],
        )
        .map_err(sql)?;
        tx.execute(
            "delete from session_sync_threads where thread_id = ?1 and confirmed = 0",
            [thread],
        )
        .map_err(sql)?;
        tx.execute("delete from session_sync_outbox where thread_id = ?1", [thread])
            .map_err(sql)?;
    }
    tx.commit().map_err(sql)
}

impl Store {
    /// Called only at creation, with a host-verified identity. Never adopts a preexisting event log. Every way a session
    /// binds to sync comes here, so a session of an agent that does not sync ([`crate::session_sync::syncs`]) stays local.
    pub(crate) fn sync_register(&self, thread: &str, identity: &Identity) -> Result<()> {
        if !self.persistent() || identity.user.is_empty() || identity.scope.is_empty() {
            return Ok(());
        }
        let agent: Option<String> = self
            .db
            .query_row("select agent from threads where id = ?1", [thread], |row| row.get(0))
            .optional()
            .map_err(sql)?;
        if !agent
            .as_deref()
            .and_then(crate::event::SessionAgent::parse)
            .is_some_and(crate::session_sync::syncs)
        {
            return Ok(());
        }
        self.db
            .execute(
                "insert into session_sync_threads (thread_id, scope, owner, sync_id)
             select id, scope, ?2, ?3 from threads where id = ?1 and scope = ?4
             and not exists (select 1 from events where thread_id = ?1)
             on conflict(thread_id) do nothing",
                params![thread, identity.user, uuid::Uuid::now_v7().to_string(), identity.scope],
            )
            .map_err(sql)?;
        Ok(())
    }

    pub(crate) fn sync_targets(&self, identity: &Identity) -> Result<Vec<Target>> {
        let mut statement = self
            .db
            .prepare(&format!(
                "select * from (select {COLUMNS}, s.last_attempt from session_sync_threads s
             where s.scope = ?1 and s.owner = ?2 and (exists (select 1 from events e where e.thread_id = s.thread_id and e.seq > s.confirmed)
               or exists (select 1 from session_sync_outbox o where o.thread_id = s.thread_id))
             union all select thread_id, scope, owner, sync_id, 0, null, 1, last_attempt
               from session_sync_deletions where scope = ?1 and owner = ?2)
             order by last_attempt, sync_id"
            ))
            .map_err(sql)?;
        statement
            .query_map(params![identity.scope, identity.user], read_target)
            .map_err(sql)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql)
    }

    /// Only previously encrypted, fully confirmed history needs idle key distribution. Dirty work has its own queue.
    pub(crate) fn sync_idle_targets(&self, identity: &Identity) -> Result<Vec<Target>> {
        let mut statement = self
            .db
            .prepare(&format!(
                "select {COLUMNS} from session_sync_threads s
             where s.scope = ?1 and s.owner = ?2 and s.confirmed > 0 and s.epoch is not null
               and s.last_attempt <= ?3
               and not exists (select 1 from events e where e.thread_id = s.thread_id and e.seq > s.confirmed)
               and not exists (select 1 from session_sync_outbox o where o.thread_id = s.thread_id)
             order by s.last_attempt, s.sync_id limit ?4"
            ))
            .map_err(sql)?;
        statement
            .query_map(
                params![
                    identity.scope,
                    identity.user,
                    crate::now_ms().saturating_sub(30_000),
                    crate::session_sync::MAX_IDLE_THREADS as i64
                ],
                read_target,
            )
            .map_err(sql)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql)
    }

    /// An idle key rotation advances only the same owned incarnation and never rolls a persisted epoch back.
    pub(crate) fn sync_epoch(&self, target: &Target, epoch: u64) -> Checked<()> {
        let epoch = i64::try_from(epoch).map_err(|_| FailureKind::InvalidEpoch)?;
        if target.epoch.is_some_and(|previous| epoch < previous as i64) {
            return Err(FailureKind::InvalidEpoch);
        }
        if db(self.db.execute(
            "update session_sync_threads set epoch = ?7
             where thread_id = ?1 and scope = ?2 and owner = ?3 and sync_id = ?4
               and confirmed = ?5 and epoch is ?6",
            params![
                target.local,
                target.identity.scope,
                target.identity.user,
                target.sync_id,
                target.confirmed,
                target.epoch.map(|epoch| epoch as i64),
                epoch
            ],
        ))? != 1
        {
            return Err(FailureKind::LocalStore);
        }
        Ok(())
    }

    pub(crate) fn sync_target(&self, thread: &str) -> Result<Option<Target>> {
        self.db
            .query_row(
                &format!("select {COLUMNS} from session_sync_threads s where s.thread_id = ?1"),
                [thread],
                read_target,
            )
            .optional()
            .map_err(sql)
    }

    pub(crate) fn sync_touch(&self, target: &Target) -> Result<()> {
        if target.deleted {
            self.db
                .execute(
                    "update session_sync_deletions set last_attempt = ?2 where sync_id = ?1",
                    params![target.sync_id, crate::now_ms()],
                )
                .map_err(sql)?;
            return Ok(());
        }
        self.db
            .execute(
                "update session_sync_threads set last_attempt = ?2 where thread_id = ?1",
                params![target.local, crate::now_ms()],
            )
            .map_err(sql)?;
        Ok(())
    }

    pub(crate) fn sync_pending(&self, thread: &str) -> Result<Option<Batch>> {
        let json: Option<String> = self
            .db
            .query_row(
                "select batch from session_sync_outbox where thread_id = ?1",
                [thread],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?;
        json.map(|json| {
            if json.len() > MAX_BATCH_BYTES {
                return Err("session sync: oversized outbox".into());
            }
            serde_json::from_str(&json).map_err(|_| "session sync: invalid outbox".into())
        })
        .transpose()
    }

    /// Read raw bytes without the UI's unknown-event fallback. SQLite checks length before returning a large body.
    pub(crate) fn sync_raw(
        &self,
        target: &Target,
    ) -> Checked<(PrivateThread, crate::event::SessionAgent, Vec<RawEvent>)> {
        let (title, name, path, agent): (String, String, String, String) = db(self.db.query_row(
            "select t.title, coalesce(p.name, ''), coalesce(p.path, ''), t.agent
             from threads t left join projects p on p.id = t.project_id where t.id = ?1 and t.scope = ?2",
            params![target.local, target.identity.scope],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ))?;
        if title.len() + name.len() + path.len() + target.local.len() > 16 * 1024 {
            return Err(FailureKind::TooLarge);
        }
        let agent = crate::event::SessionAgent::parse(&agent).ok_or(FailureKind::InvalidMetadata)?;
        let mut statement = db(self.db.prepare(
            "select seq, at, case when length(cast(body as blob)) <= ?3 then body else null end
             from events where thread_id = ?1 and seq > ?2 order by seq limit ?4",
        ))?;
        let mut rows = db(statement.query(params![
            target.local,
            target.confirmed,
            MAX_PLAINTEXT as i64,
            MAX_EVENTS as i64
        ]))?;
        let mut events = Vec::new();
        let mut size = 0;
        while let Some(row) = db(rows.next())? {
            let seq = db(row.get::<_, i64>(0))?;
            if seq != target.confirmed + events.len() as i64 + 1 {
                return Err(FailureKind::LocalStore);
            }
            let body = db(row.get::<_, Option<String>>(2))?;
            let Some(body) = body else {
                if events.is_empty() {
                    return Err(FailureKind::TooLarge);
                }
                break;
            };
            if size + body.len() > MAX_PLAINTEXT {
                break;
            }
            size += body.len();
            events.push(RawEvent {
                seq,
                at: db(row.get(1))?,
                body,
            });
        }
        Ok((
            PrivateThread {
                origin: self.computer_origin(&target.identity.scope, &target.identity.user).ok(),
                local_id: target.local.clone(),
                title,
                project_name: name,
                project_path: path,
            },
            agent,
            events,
        ))
    }

    /// Persist ciphertext and its epoch together before the first possible network write.
    pub(crate) fn sync_stage(&self, target: &Target, batch: &Batch) -> Result<()> {
        if batch.events.is_empty()
            || batch.thread != target.sync_id
            || batch
                .events
                .iter()
                .enumerate()
                .any(|(index, event)| event.seq != target.confirmed as u64 + index as u64)
        {
            return Err("session sync: invalid staged range".into());
        }
        let json = serde_json::to_string(batch).map_err(sql)?;
        if json.len() > MAX_BATCH_BYTES {
            return Err("session sync: oversized batch".into());
        }
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        let changed = tx
            .execute(
                "update session_sync_threads set epoch = coalesce(?3, epoch)
             where thread_id = ?1 and confirmed = ?2 and scope = ?4 and owner = ?5 and sync_id = ?6",
                params![
                    target.local,
                    target.confirmed,
                    batch.events[0].epoch.map(|epoch| epoch as i64),
                    target.identity.scope,
                    target.identity.user,
                    target.sync_id
                ],
            )
            .map_err(sql)?;
        if changed != 1 {
            return Err("session sync: stale thread".into());
        }
        tx.execute(
            "insert into session_sync_outbox (thread_id, batch) values (?1, ?2)",
            params![target.local, json],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }

    /// A server cursor may advance only into the exact durable outbox. Never skip unseen local events.
    pub(crate) fn sync_reconcile(&self, target: &Target, through: Option<u64>) -> Checked<usize> {
        let confirmed = match through {
            None => 0,
            Some(seq) => i64::try_from(seq.checked_add(1).ok_or(FailureKind::InvalidAcknowledgement)?)
                .map_err(|_| FailureKind::InvalidAcknowledgement)?,
        };
        if confirmed < target.confirmed {
            return Err(FailureKind::InvalidAcknowledgement);
        }
        if confirmed == target.confirmed {
            return Ok(0);
        }
        self.sync_ack(target, through.ok_or(FailureKind::InvalidAcknowledgement)?)
    }

    pub(crate) fn sync_ack(&self, target: &Target, through: u64) -> Checked<usize> {
        let mut batch = value(self.sync_pending(&target.local))?.ok_or(FailureKind::InvalidAcknowledgement)?;
        let first = batch.events.first().ok_or(FailureKind::InvalidAcknowledgement)?.seq;
        let last = batch.events.last().ok_or(FailureKind::InvalidAcknowledgement)?.seq;
        if batch.thread != target.sync_id || first != target.confirmed as u64 || through < first || through > last {
            return Err(FailureKind::InvalidAcknowledgement);
        }
        let confirmed = i64::try_from(through.checked_add(1).ok_or(FailureKind::InvalidAcknowledgement)?)
            .map_err(|_| FailureKind::InvalidAcknowledgement)?;
        batch.events.retain(|event| event.seq > through);
        let tx = db(self.db.unchecked_transaction())?;
        if db(tx.execute(
            "update session_sync_threads set confirmed = ?2
             where thread_id = ?1 and confirmed = ?3 and scope = ?4 and owner = ?5 and sync_id = ?6",
            params![
                target.local,
                confirmed,
                target.confirmed,
                target.identity.scope,
                target.identity.user,
                target.sync_id
            ],
        ))? != 1
        {
            return Err(FailureKind::LocalStore);
        }
        if batch.events.is_empty() {
            db(tx.execute("delete from session_sync_outbox where thread_id = ?1", [&target.local]))?;
        } else {
            let json = serde_json::to_string(&batch).map_err(|_| FailureKind::LocalStore)?;
            db(tx.execute(
                "update session_sync_outbox set batch = ?2 where thread_id = ?1",
                params![target.local, json],
            ))?;
        }
        db(tx.commit())?;
        Ok((confirmed - target.confirmed) as usize)
    }

    /// Drops staged ciphertext that Gateway definitely refused (`KeyRotated`), so the next pass seals it again. Only
    /// while the cursor is where the caller read it.
    pub(crate) fn sync_unstage(&self, target: &Target) -> Result<()> {
        self.db
            .execute(
                "delete from session_sync_outbox where thread_id = ?1 and exists (select 1 from session_sync_threads
             where thread_id = ?1 and sync_id = ?2 and confirmed = ?3 and scope = ?4 and owner = ?5)",
                params![
                    target.local,
                    target.sync_id,
                    target.confirmed,
                    target.identity.scope,
                    target.identity.user
                ],
            )
            .map_err(sql)?;
        Ok(())
    }

    pub(crate) fn sync_forget(&self, target: &Target) -> Result<()> {
        self.db
            .execute(
                "delete from session_sync_deletions where sync_id = ?1 and scope = ?2 and owner = ?3",
                params![target.sync_id, target.identity.scope, target.identity.user],
            )
            .map_err(sql)?;
        Ok(())
    }
}
