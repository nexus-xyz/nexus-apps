//! Durable metadata only. Prompts, project titles and replies are never written here in plaintext.
use super::{Result, Store, sql};
use crate::computer::{ExecutionIdentity, Info, InvitationRecord, Origin, Reply, RequestTarget};
use rusqlite::{OptionalExtension, params};
pub(super) const SCHEMA:&str="
create table if not exists computer_outbox(id text not null,scope text not null,owner text not null,kind text not null,target text not null,digest text not null,body text not null,primary key(scope,owner,kind,id));
create table if not exists computer_origin(id integer primary key check(id=1),origin text not null);
create table if not exists computer_required_payers(thread_id text primary key,scope text not null,payer text not null);
create table if not exists computer_identity(id integer primary key check(id=1),computer_id text not null,name text not null);
create table if not exists computer_generations(scope text not null,owner text not null,generation integer not null,enabled integer not null,primary key(scope,owner));
create table if not exists computer_requests(id text primary key,scope text not null,owner text not null,request_hash text not null,target text not null,stage text not null,reply text,acknowledged integer not null default 0);
create table if not exists phone_invitations(id text primary key,scope text not null,owner text not null,record text not null);
create table if not exists phone_pairing_outbox(
    scope text not null,owner text not null,invite_id text not null,expires_at integer not null,record text not null,
    primary key(scope,owner,invite_id)
);
";
impl Store {
    pub(crate) fn computer_require_payer(
        &self,
        thread: &str,
        scope: &str,
        payer: crate::computer::BillingPayer,
    ) -> Result<()> {
        self.db
            .execute(
                "insert into computer_required_payers values(?1,?2,?3) on conflict(thread_id) do nothing",
                params![thread, scope, serde_json::to_string(&payer).map_err(sql)?],
            )
            .map(|_| ())
            .map_err(sql)
    }
    pub(crate) fn computer_required_payer(
        &self,
        thread: &str,
        scope: &str,
    ) -> Result<Option<crate::computer::BillingPayer>> {
        let payer: Option<String> = self
            .db
            .query_row(
                "select payer from computer_required_payers where thread_id=?1 and scope=?2",
                params![thread, scope],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql)?;
        payer.map(|p| serde_json::from_str(&p).map_err(sql)).transpose()
    }
    pub(crate) fn computer_info(&self, scope: &str, owner: &str) -> Result<Info> {
        self.db
            .execute(
                "insert or ignore into computer_identity(id,computer_id,name) values(1,?1,?2)",
                params![uuid::Uuid::new_v4().to_string(), default_name()],
            )
            .map_err(sql)?;
        let (computer_id, name) = self
            .db
            .query_row("select computer_id,name from computer_identity where id=1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(sql)?;
        let generation = self
            .db
            .query_row(
                "select generation from computer_generations where scope=?1 and owner=?2",
                params![scope, owner],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql)?
            .unwrap_or(0);
        Ok(Info {
            available: false,
            computer_id,
            name,
            enabled: false,
            scope: None,
            runner_device_id: None,
            generation,
        })
    }
    pub(crate) fn computer_outgoing(
        &self,
        scope: &str,
        owner: &str,
        kind: &str,
        id: &str,
    ) -> Result<Option<(String, crate::computer::Outbound)>> {
        let stored: Option<(String, String)> = self
            .db
            .query_row(
                "select digest,body from computer_outbox where scope=?1 and owner=?2 and kind=?3 and id=?4",
                params![scope, owner, kind, id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(sql)?;
        stored
            .map(|(d, b)| serde_json::from_str(&b).map(|b| (d, b)).map_err(sql))
            .transpose()
    }
    pub(crate) fn computer_outgoing_save(
        &self,
        scope: &str,
        owner: &str,
        digest: &str,
        outbound: &crate::computer::Outbound,
    ) -> Result<()> {
        self.db.execute("insert into computer_outbox(id,scope,owner,kind,target,digest,body) values(?1,?2,?3,?4,?5,?6,?7) on conflict(scope,owner,kind,id) do nothing",params![outbound.id,scope,owner,outbound.kind,outbound.target,digest,serde_json::to_string(outbound).map_err(sql)?]).map(|_|()).map_err(sql)
    }
    pub fn set_computer_origin(&self, origin: &Origin) -> Result<()> {
        self.db
            .execute(
                "insert into computer_origin values(1,?1) on conflict(id) do update set origin=excluded.origin",
                [serde_json::to_string(origin).map_err(sql)?],
            )
            .map(|_| ())
            .map_err(sql)
    }
    pub(crate) fn computer_origin(&self, scope: &str, owner: &str) -> Result<Origin> {
        let origin: Option<String> = self
            .db
            .query_row("select origin from computer_origin where id=1", [], |r| r.get(0))
            .optional()
            .map_err(sql)?;
        if let Some(origin) = origin {
            return serde_json::from_str(&origin).map_err(sql);
        }
        self.computer_info(scope, owner).map(|i| Origin::Computer {
            computer_id: i.computer_id,
            name: i.name,
        })
    }
    pub(crate) fn computer_creation(&self, scope: &str, owner: &str, target: &RequestTarget) -> Result<()> {
        let changed=self.db.execute("update computer_requests set target=?5 where id=?1 and scope=?2 and owner=?3 and request_hash=?4 and stage='executing'",params![target.id,scope,owner,target.hash,serde_json::to_string(target).map_err(sql)?]).map_err(sql)?;
        if changed != 1 {
            return Err("Computer request changed.".into());
        }
        Ok(())
    }
    pub(crate) fn computer_name(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.len() > 160 || name.chars().count() > 64 || name.chars().any(char::is_control) {
            return Err("Use a computer name of up to 64 characters.".into());
        }
        self.db
            .execute("update computer_identity set name=?1 where id=1", [name])
            .map(|_| ())
            .map_err(sql)
    }
    pub(crate) fn computer_generation(&self, scope: &str, owner: &str, enabled: bool, start: bool) -> Result<i64> {
        self.db.execute("insert into computer_generations(scope,owner,generation,enabled) values(?1,?2,1,?3)
   on conflict(scope,owner) do update set generation=computer_generations.generation+case when ?4 or computer_generations.enabled!=excluded.enabled then 1 else 0 end,enabled=excluded.enabled",params![scope,owner,enabled,start]).map_err(sql)?;
        self.computer_info(scope, owner).map(|i| i.generation)
    }
    pub(crate) fn computer_reserve(&self, scope: &str, owner: &str, target: &RequestTarget) -> Result<bool> {
        self.db.execute("insert into computer_requests(id,scope,owner,request_hash,target,stage) values(?1,?2,?3,?4,?5,'claimAttempted') on conflict(id) do nothing",params![target.id,scope,owner,target.hash,serde_json::to_string(target).map_err(sql)?]).map(|n|n==1).map_err(sql)
    }
    pub(crate) fn computer_stage(&self, scope: &str, owner: &str, target: &RequestTarget, stage: &str) -> Result<()> {
        let changed=self.db.execute("update computer_requests set stage=?6 where id=?1 and scope=?2 and owner=?3 and request_hash=?4 and stage=?5",params![target.id,scope,owner,target.hash,"claimAttempted",stage]).map_err(sql)?;
        if changed != 1 {
            return Err("Computer request changed.".into());
        }
        Ok(())
    }
    pub(crate) fn computer_reply(&self, scope: &str, owner: &str, target: &RequestTarget, reply: &Reply) -> Result<()> {
        let changed=self.db.execute("update computer_requests set stage='replied',reply=?5 where id=?1 and scope=?2 and owner=?3 and request_hash=?4 and reply is null",params![target.id,scope,owner,target.hash,serde_json::to_string(reply).map_err(sql)?]).map_err(sql)?;
        if changed != 1 {
            return Err("Computer request changed.".into());
        }
        Ok(())
    }
    pub(crate) fn computer_pending(&self, scope: &str, owner: &str) -> Result<Vec<(RequestTarget, Option<Reply>)>> {
        let mut stmt=self.db.prepare("select target,reply from computer_requests where scope=?1 and owner=?2 and acknowledged=0 order by rowid limit 20").map_err(sql)?;
        let rows = stmt
            .query_map(params![scope, owner], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .map_err(sql)?;
        rows.map(|r| {
            let (a, b) = r.map_err(sql)?;
            Ok((
                serde_json::from_str(&a).map_err(sql)?,
                b.map(|s| serde_json::from_str(&s).map_err(sql)).transpose()?,
            ))
        })
        .collect()
    }
    pub(crate) fn computer_ack(&self, scope: &str, owner: &str, id: &str) -> Result<()> {
        self.db
            .execute(
                "update computer_requests set acknowledged=1 where id=?1 and scope=?2 and owner=?3",
                params![id, scope, owner],
            )
            .map(|_| ())
            .map_err(sql)
    }
    pub(crate) fn computer_identities(
        &self,
        scope: &str,
        owner: &str,
        ids: &[String],
    ) -> Result<Vec<ExecutionIdentity>> {
        ids.iter()
            .map(|id| {
                let sync = self
                    .sync_target(id)?
                    .filter(|s| s.identity.scope == scope && s.identity.user == owner)
                    .map(|s| s.sync_id);
                Ok(ExecutionIdentity {
                    local_id: id.clone(),
                    sync_id: sync,
                    origin: self.computer_origin(scope, owner)?,
                })
            })
            .collect()
    }
    pub(crate) fn computer_prior(
        &self,
        scope: &str,
        owner: &str,
        project: &str,
    ) -> Result<Option<crate::store::ThreadRecord>> {
        let id:Option<String>=self.db.query_row("select t.id from threads t join session_sync_threads s on s.thread_id=t.id where s.scope=?1 and s.owner=?2 and t.scope=s.scope and t.project_id=?3 and t.native_started=1 order by t.updated_at desc,t.id desc limit 1",params![scope,owner,project],|r|r.get(0)).optional().map_err(sql)?;
        id.map(|id| self.thread(&id)).transpose().map(Option::flatten)
    }
    pub(crate) fn phone_invitation_save(&self, scope: &str, owner: &str, record: &InvitationRecord) -> Result<()> {
        self.db.execute("insert into phone_invitations(id,scope,owner,record) values(?1,?2,?3,?4) on conflict(id) do update set record=excluded.record where scope=excluded.scope and owner=excluded.owner",params![record.invite_id,scope,owner,serde_json::to_string(record).map_err(sql)?]).map(|_|()).map_err(sql)
    }
    pub(crate) fn phone_invitation(&self, scope: &str, owner: &str, id: &str) -> Result<Option<InvitationRecord>> {
        let value: Option<String> = self
            .db
            .query_row(
                "select record from phone_invitations where id=?1 and scope=?2 and owner=?3",
                params![id, scope, owner],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql)?;
        value.map(|s| serde_json::from_str(&s).map_err(sql)).transpose()
    }
    pub(crate) fn phone_invitation_accept(
        &self,
        scope: &str,
        owner: &str,
        before: &InvitationRecord,
        after: &InvitationRecord,
    ) -> Result<bool> {
        let tx = self.db.unchecked_transaction().map_err(sql)?;
        let changed = tx
            .execute(
                "update phone_invitations set record=?5 where id=?1 and scope=?2 and owner=?3 and record=?4",
                params![
                    before.invite_id,
                    scope,
                    owner,
                    serde_json::to_string(before).map_err(sql)?,
                    serde_json::to_string(after).map_err(sql)?
                ],
            )
            .map_err(sql)?
            == 1;
        if changed && after.signed_decisions && after.state == "connected" {
            let device = after.accepted.as_deref().ok_or("No device was approved.")?;
            crate::store::code_control::control_grant_in(&tx, scope, owner, device)?;
        }
        if changed && (after.state == "connected" || (after.signed_decisions && after.state == "cancelled")) {
            tx.execute(
                "insert into phone_pairing_outbox(scope,owner,invite_id,expires_at,record) values(?1,?2,?3,?4,?5)",
                params![
                    scope,
                    owner,
                    after.invite_id,
                    after.expires_at,
                    serde_json::to_string(after).map_err(sql)?
                ],
            )
            .map_err(sql)?;
        }
        tx.commit().map_err(sql)?;
        Ok(changed)
    }

    pub(crate) fn phone_pairing_outbox(&self, scope: &str, owner: &str) -> Result<Vec<InvitationRecord>> {
        let cutoff = crate::now_ms().saturating_sub(300_000);
        self.db
            .execute("delete from phone_pairing_outbox where expires_at <= ?1", [cutoff])
            .map_err(sql)?;
        let mut query = self
            .db
            .prepare("select record from phone_pairing_outbox where scope=?1 and owner=?2 order by expires_at limit 4")
            .map_err(sql)?;
        let rows = query
            .query_map(params![scope, owner], |row| row.get::<_, String>(0))
            .map_err(sql)?;
        rows.map(|row| serde_json::from_str(&row.map_err(sql)?).map_err(sql))
            .collect()
    }

    pub(crate) fn phone_pairing_sent(&self, scope: &str, owner: &str, invite: &str) -> Result<()> {
        self.db
            .execute(
                "delete from phone_pairing_outbox where scope=?1 and owner=?2 and invite_id=?3",
                params![scope, owner, invite],
            )
            .map(|_| ())
            .map_err(sql)
    }

    pub(crate) fn computer_history_ids(
        &self,
        scope: &str,
        before: Option<(i64, String)>,
        limit: usize,
    ) -> Result<Vec<String>> {
        let (at, id) = before.unwrap_or((i64::MAX, String::new()));
        let mut stmt=self.db.prepare("select id from threads where scope=?1 and (updated_at<?2 or (updated_at=?2 and id<?3)) order by updated_at desc,id desc limit ?4").map_err(sql)?;
        stmt.query_map(params![scope, at, id, limit as i64], |r| r.get(0))
            .map_err(sql)?
            .collect::<rusqlite::Result<_>>()
            .map_err(sql)
    }
}
fn default_name() -> String {
    #[cfg(unix)]
    let os_name = nix::unistd::gethostname()
        .ok()
        .map(|n| n.to_string_lossy().trim_end_matches(".local").to_string());
    #[cfg(not(unix))]
    let os_name = std::env::var("COMPUTERNAME").ok();
    let mut name = os_name
        .unwrap_or_else(|| "My computer".into())
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect::<String>();
    while name.len() > 160 {
        name.pop();
    }
    if name.trim().is_empty() {
        "My computer".into()
    } else {
        name
    }
}
