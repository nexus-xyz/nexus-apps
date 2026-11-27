//! Local opt-in, fair polling and command receipts. Receipts contain no message or approval content.

use rusqlite::{OptionalExtension, params};

use super::{Result, Store, sql};
use crate::session_sync::{DeviceTrust, Identity, Target};

pub(super) const SCHEMA: &str = "
create table if not exists code_control_settings (
  scope text not null, owner text not null, enabled integer not null check (enabled in (0,1)), heartbeat integer not null default 0,
  primary key(scope,owner)
);
create table if not exists code_control_pairing_mode (
  scope text not null, owner text not null, scoped integer not null check (scoped in (0,1)),
  primary key(scope,owner)
);
create table if not exists code_control_device_grants (
  scope text not null, owner text not null, device_id text not null,
  primary key(scope,owner,device_id)
);
create table if not exists code_control_poll (
  thread_id text primary key, last_poll integer not null, last_lease integer not null default 0
);
create table if not exists code_control_receipts (
  command_id text primary key, scope text not null, owner text not null, sync_id text not null,
  completed integer check (completed in (0,1)),
  acknowledged integer not null default 0 check (acknowledged in (0,1))
);
";

/// The person's own Remote control choice on this computer. `code_control_settings` cannot tell it apart from no
/// choice: a lease writes a row that is off. Under Gateway's device trust, no row here means on.
const CHOICES: &str = "
create table code_control_choices (
  scope text not null, owner text not null, enabled integer not null check (enabled in (0,1)),
  primary key(scope,owner)
);
";

/// Makes `code_control_choices` once, after [`SCHEMA`]. A file from before it keeps every stored switch as a choice, off
/// included. No column proves who wrote an off: a lease writes `enabled=0` and a heartbeat whether control is on or off,
/// and the next lease after the person's off overwrites its `heartbeat=0`. So an old off stays off, and only a person
/// with no row at all gets Gateway's default.
pub(super) fn migrate(db: &rusqlite::Connection) -> Result<()> {
    let made: bool = db
        .query_row(
            "select count(*) from sqlite_master where type='table' and name='code_control_choices'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sql)?
        > 0;
    if made {
        return Ok(());
    }
    db.execute_batch(&format!(
        "begin; {CHOICES}
         insert into code_control_choices(scope,owner,enabled)
           select scope,owner,enabled from code_control_settings;
         commit;"
    ))
    .map_err(|error| {
        if let Err(rollback) = db.execute_batch("rollback") {
            eprintln!("nexus-workbench: the Remote control choices were not rolled back: {rollback}");
        }
        sql(error)
    })
}

/// Records approval and enables control in the same transaction as the invitation.
pub(crate) fn control_grant_in(
    tx: &rusqlite::Transaction<'_>,
    scope: &str,
    owner: &str,
    device_id: &str,
) -> Result<()> {
    tx.execute(
        "insert into code_control_pairing_mode(scope,owner,scoped) values(?1,?2,
            not exists (select 1 from code_control_settings where scope=?1 and owner=?2 and enabled=1))
         on conflict(scope,owner) do nothing",
        params![scope, owner],
    )
    .map_err(sql)?;
    tx.execute(
        "insert into code_control_device_grants(scope,owner,device_id) values(?1,?2,?3)
         on conflict(scope,owner,device_id) do nothing",
        params![scope, owner, device_id],
    )
    .map_err(sql)?;
    set_enabled_in(tx, scope, owner, true)
}

fn set_enabled_in(tx: &rusqlite::Transaction<'_>, scope: &str, owner: &str, enabled: bool) -> Result<()> {
    let previous: bool = tx
        .query_row(
            "select enabled from code_control_settings where scope=?1 and owner=?2",
            params![scope, owner],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql)?
        .unwrap_or(false);
    if previous != enabled {
        tx.execute(
            "insert into computer_generations(scope,owner,generation,enabled) values(?1,?2,1,?3)
             on conflict(scope,owner) do update set generation=computer_generations.generation+1,enabled=excluded.enabled",
            params![scope, owner, enabled],
        ).map_err(sql)?;
    }
    tx.execute(
        "insert into code_control_settings(scope,owner,enabled) values (?1,?2,?3)
         on conflict(scope,owner) do update set enabled=excluded.enabled,heartbeat=0",
        params![scope, owner, enabled],
    )
    .map_err(sql)?;
    tx.execute(
        "insert into code_control_choices(scope,owner,enabled) values (?1,?2,?3)
         on conflict(scope,owner) do update set enabled=excluded.enabled",
        params![scope, owner, enabled],
    )
    .map_err(sql)?;
    tx.execute(
        "update code_control_poll set last_lease=0 where thread_id in
        (select thread_id from session_sync_threads where scope=?1 and owner=?2)",
        params![scope, owner],
    )
    .map_err(sql)?;
    Ok(())
}

impl Store {
    pub(crate) fn control_grant(&self, identity: &Identity, device_id: &str) -> Result<()> {
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        control_grant_in(&tx, &identity.scope, &identity.user, device_id)?;
        tx.commit().map_err(sql)
    }

    pub(crate) fn control_allows(&self, identity: &Identity, device_id: &str) -> Result<bool> {
        self.db
            .query_row(
                "select exists (select 1 from code_control_settings where scope=?1 and owner=?2 and enabled=1)
                and (not exists (select 1 from code_control_pairing_mode where scope=?1 and owner=?2 and scoped=1)
                  or exists (select 1 from code_control_device_grants where scope=?1 and owner=?2 and device_id=?3))",
                params![identity.scope, identity.user, device_id],
                |r| r.get(0),
            )
            .map_err(sql)
    }

    pub(crate) fn control_enabled(&self, identity: &Identity) -> Result<bool> {
        self.db
            .query_row(
                "select enabled from code_control_settings where scope=?1 and owner=?2",
                params![identity.scope, identity.user],
                |row| row.get(0),
            )
            .optional()
            .map(|enabled| enabled.unwrap_or(false))
            .map_err(sql)
    }

    /// Whether Remote control is on here under `trust`. Under approvals, the stored switch: off until the person turns
    /// it on. Under Gateway's trust, on unless the person turned it off on this computer.
    pub(crate) fn control_enabled_in(&self, identity: &Identity, trust: DeviceTrust) -> Result<bool> {
        match trust {
            DeviceTrust::Approvals => self.control_enabled(identity),
            DeviceTrust::Gateway => Ok(self.control_choice(identity)?.unwrap_or(true)),
        }
    }

    /// Whether `device_id` may control this computer under `trust`. Under Gateway's trust, any of the person's trusted
    /// devices may while Remote control is on: the backend opens a command only from one of them.
    pub(crate) fn control_allows_in(&self, identity: &Identity, trust: DeviceTrust, device_id: &str) -> Result<bool> {
        match trust {
            DeviceTrust::Approvals => self.control_allows(identity, device_id),
            DeviceTrust::Gateway => self.control_enabled_in(identity, trust),
        }
    }

    /// The person's own choice on this computer, if they made one.
    fn control_choice(&self, identity: &Identity) -> Result<Option<bool>> {
        self.db
            .query_row(
                "select enabled from code_control_choices where scope=?1 and owner=?2",
                params![identity.scope, identity.user],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)
    }

    pub(crate) fn set_control_enabled(&self, identity: &Identity, enabled: bool) -> Result<()> {
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        if enabled {
            tx.execute(
                "update code_control_pairing_mode set scoped=0 where scope=?1 and owner=?2",
                params![identity.scope, identity.user],
            )
            .map_err(sql)?;
        }
        set_enabled_in(&tx, &identity.scope, &identity.user, enabled)?;
        tx.commit().map_err(sql)
    }

    /// Poll idle and completed threads too. Bound each pass and rotate the oldest poll first.
    pub(crate) fn control_targets(&self, identity: &Identity) -> Result<Vec<Target>> {
        let mut statement = self
            .db
            .prepare(
                "select s.thread_id from session_sync_threads s join threads t on t.id=s.thread_id
             left join code_control_poll p on p.thread_id=s.thread_id
             where s.scope=?1 and s.owner=?2 and t.scope=s.scope and s.confirmed>0
             order by (coalesce(p.last_lease,0)>0),coalesce(p.last_poll,0),s.sync_id limit 4",
            )
            .map_err(sql)?;
        let ids = statement
            .query_map(params![identity.scope, identity.user], |row| row.get::<_, String>(0))
            .map_err(sql)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql)?;
        ids.into_iter()
            .filter_map(|id| self.sync_target(&id).transpose())
            .collect()
    }

    pub(crate) fn control_touch(&self, target: &Target) -> Result<()> {
        self.db.execute(
            "insert into code_control_poll(thread_id,last_poll) values (?1,?2) on conflict(thread_id) do update set last_poll=excluded.last_poll",
            params![target.local,crate::now_ms()],
        ).map(|_|()).map_err(sql)
    }

    pub(crate) fn control_unbound(&self, target: &Target) -> Result<bool> {
        self.db
            .query_row(
                "select last_lease=0 from code_control_poll where thread_id=?1",
                [&target.local],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)
            .map(|value| value.unwrap_or(true))
    }

    pub(crate) fn control_heartbeat_due(&self, identity: &Identity) -> Result<bool> {
        let last: i64 = self
            .db
            .query_row(
                "select heartbeat from code_control_settings where scope=?1 and owner=?2",
                params![identity.scope, identity.user],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?
            .unwrap_or(0);
        Ok(crate::now_ms().saturating_sub(last) >= 15_000)
    }

    pub(crate) fn control_leased(&self, target: &Target) -> Result<()> {
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        tx.execute(
            "update code_control_poll set last_lease=?2 where thread_id=?1",
            params![target.local, crate::now_ms()],
        )
        .map_err(sql)?;
        tx.execute(
            "insert into code_control_settings(scope,owner,enabled,heartbeat) values (?1,?2,0,?3)
            on conflict(scope,owner) do update set heartbeat=excluded.heartbeat",
            params![target.identity.scope, target.identity.user, crate::now_ms()],
        )
        .map_err(sql)?;
        tx.commit().map_err(sql)
    }

    pub(crate) fn control_by_sync(&self, identity: &Identity, sync_id: &str) -> Result<Option<Target>> {
        let local: Option<String> = self
            .db
            .query_row(
                "select s.thread_id from session_sync_threads s
            join threads t on t.id=s.thread_id and t.scope=s.scope where s.scope=?1 and s.owner=?2 and s.sync_id=?3",
                params![identity.scope, identity.user, sync_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?;
        match local {
            Some(local) => self.sync_target(&local),
            None => Ok(None),
        }
    }

    /// This write precedes every side effect. A restarted process cannot replay an uncertain command.
    /// By the synced thread's id, so a command for a thread this computer no longer has is reserved, then rejected.
    pub(crate) fn control_reserve(&self, identity: &Identity, sync_id: &str, id: &str) -> Result<bool> {
        self.db
            .execute(
                "insert into code_control_receipts(command_id,scope,owner,sync_id)
             values (?1,?2,?3,?4) on conflict(command_id) do nothing",
                params![id, identity.scope, identity.user, sync_id],
            )
            .map(|changed| changed == 1)
            .map_err(sql)
    }

    pub(crate) fn control_complete(&self, identity: &Identity, sync_id: &str, id: &str, completed: bool) -> Result<()> {
        self.db.execute(
            "update code_control_receipts set completed=?5 where command_id=?1 and scope=?2 and owner=?3 and sync_id=?4",
            params![id,identity.scope,identity.user,sync_id,completed],
        ).map(|_|()).map_err(sql)
    }

    /// Results not delivered yet: every thread's, or with `only` that synced thread's (a CLI run's own).
    pub(crate) fn control_results(
        &self,
        identity: &Identity,
        only: Option<&str>,
    ) -> Result<Vec<(String, String, bool)>> {
        let mut statement = self
            .db
            .prepare(
                "select sync_id,command_id,completed from code_control_receipts
            where scope=?1 and owner=?2 and acknowledged=0 and completed is not null
            and (?3 is null or sync_id=?3) limit 20",
            )
            .map_err(sql)?;
        statement
            .query_map(params![identity.scope, identity.user, only], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(sql)?
            .collect::<rusqlite::Result<_>>()
            .map_err(sql)
    }

    pub(crate) fn control_acknowledge(&self, identity: &Identity, sync_id: &str, id: &str) -> Result<()> {
        self.db.execute("update code_control_receipts set acknowledged=1 where command_id=?1 and scope=?2 and owner=?3 and sync_id=?4",
            params![id,identity.scope,identity.user,sync_id]).map(|_|()).map_err(sql)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crash_uncertain_receipt_never_replays_or_reports_a_false_failure() {
        let root = std::env::temp_dir().join(format!("nexus-control-receipts-{}", uuid::Uuid::new_v4()));
        let path = root.join("workbench.sqlite3");
        let target = Target {
            local: "local".into(),
            identity: Identity {
                scope: "gw|org".into(),
                user: "person".into(),
            },
            sync_id: uuid::Uuid::new_v4().to_string(),
            confirmed: 1,
            epoch: Some(0),
            deleted: false,
        };
        let (identity, sync_id) = (&target.identity, target.sync_id.as_str());
        let store = Store::open(&path).expect("open");
        assert!(
            store
                .control_reserve(identity, sync_id, "command")
                .expect("reserve before side effect")
        );
        drop(store);
        let store = Store::open(&path).expect("reopen after uncertain side effect");
        assert!(
            !store
                .control_reserve(identity, sync_id, "command")
                .expect("must never replay")
        );
        assert!(
            store
                .control_results(&target.identity, None)
                .expect("results")
                .is_empty(),
            "unknown completion cannot be reported as rejected"
        );
        store
            .control_complete(identity, sync_id, "command", true)
            .expect("actual result");
        assert_eq!(
            store.control_results(&target.identity, None).expect("known result"),
            [(target.sync_id.clone(), "command".into(), true)]
        );
        store
            .control_acknowledge(&target.identity, &target.sync_id, "command")
            .expect("acknowledge");
        assert!(
            store
                .control_results(&target.identity, None)
                .expect("acknowledged")
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn identity(scope: &str) -> Identity {
        Identity {
            scope: scope.into(),
            user: "person".into(),
        }
    }

    #[test]
    fn under_gateway_trust_a_lease_is_no_choice_and_only_the_person_turns_it_off() {
        let store = Store::in_memory().expect("store");
        let who = identity("gw|org");
        let target = Target {
            local: "local".into(),
            identity: who.clone(),
            sync_id: uuid::Uuid::new_v4().to_string(),
            confirmed: 1,
            epoch: Some(0),
            deleted: false,
        };
        let enabled = |trust| store.control_enabled_in(&who, trust).expect("enabled");
        assert!(!enabled(DeviceTrust::Approvals));
        assert!(enabled(DeviceTrust::Gateway));
        // A lease writes the switch off: that is no choice of the person's.
        store.control_leased(&target).expect("leased");
        assert!(!enabled(DeviceTrust::Approvals));
        assert!(enabled(DeviceTrust::Gateway));
        store.set_control_enabled(&who, false).expect("off");
        assert!(!enabled(DeviceTrust::Approvals));
        assert!(!enabled(DeviceTrust::Gateway));
        store.set_control_enabled(&who, true).expect("on");
        assert!(enabled(DeviceTrust::Approvals));
        assert!(enabled(DeviceTrust::Gateway));
    }

    #[test]
    fn a_file_from_before_the_choices_keeps_every_off_and_only_an_untouched_computer_reads_on() {
        let root = std::env::temp_dir().join(format!("nexus-control-choices-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("folder");
        let path = root.join("workbench.sqlite3");
        {
            let old = rusqlite::Connection::open(&path).expect("old file");
            old.execute_batch(SCHEMA).expect("old schema");
            // On; the person's off (heartbeat 0); an off a later lease overwrote (heartbeat set); off after pairing.
            old.execute_batch(
                "insert into code_control_settings(scope,owner,enabled,heartbeat) values
                   ('on','person',1,0),('off','person',0,0),('leased','person',0,1767000000000),
                   ('paired','person',0,0);
                 insert into code_control_pairing_mode(scope,owner,scoped) values ('paired','person',1);",
            )
            .expect("old rows");
        }
        let store = Store::open(&path).expect("migrated");
        let enabled = |scope: &str, trust| store.control_enabled_in(&identity(scope), trust).expect("enabled");
        assert!(enabled("on", DeviceTrust::Gateway) && enabled("on", DeviceTrust::Approvals));
        for off in ["off", "leased", "paired"] {
            assert!(
                !enabled(off, DeviceTrust::Gateway),
                "{off} stays off under Gateway trust"
            );
            assert!(!enabled(off, DeviceTrust::Approvals), "{off}");
        }
        // A computer with no row at all was never touched: Gateway's default.
        assert!(enabled("never", DeviceTrust::Gateway) && !enabled("never", DeviceTrust::Approvals));
        drop(store);
        // Opening it again changes no choice, and a new one sticks.
        let store = Store::open(&path).expect("reopened");
        store.set_control_enabled(&identity("leased"), true).expect("on");
        drop(store);
        let store = Store::open(&path).expect("reopened");
        let enabled = |scope: &str| {
            store
                .control_enabled_in(&identity(scope), DeviceTrust::Gateway)
                .expect("enabled")
        };
        assert!(enabled("leased") && !enabled("off") && enabled("never"));
        let _ = std::fs::remove_dir_all(root);
    }
}
