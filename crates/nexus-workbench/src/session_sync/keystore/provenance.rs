//! Write eligibility for locally created epochs. Imported and legacy keys remain readable without this provenance.
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::{FAILED, KeyStore, StoredKey, check};
use crate::session_sync::trust::{Holder, Keys};

pub(super) const SCHEMA: &str = "
create table if not exists sync_write_origins_v2 (
  sync_id text not null, epoch integer not null, holder text not null, locked blob not null,
  primary key (sync_id, epoch)
);
create table if not exists sync_key_recipients_v2 (
  sync_id text not null, epoch integer not null, audience integer not null,
  recipient text not null, encryption blob not null, signing blob not null,
  primary key (sync_id, epoch, audience, recipient, encryption, signing)
);
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub(crate) enum Audience {
    Personal = 0,
    Team = 1,
    Organization = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recipient {
    pub audience: Audience,
    pub holder: Holder,
}

impl Recipient {
    pub fn personal(holder: &Holder) -> Self {
        Self {
            audience: Audience::Personal,
            holder: holder.clone(),
        }
    }

    pub fn shared(audience: Audience, id: &str, encryption: &[u8; 32]) -> Self {
        Self {
            audience,
            holder: Holder {
                id: id.into(),
                keys: Keys {
                    encryption: *encryption,
                    signing: None,
                },
            },
        }
    }
}

fn record(tx: &Transaction<'_>, sync_id: &str, epoch: i64, recipients: &[Recipient]) -> Result<(), &'static str> {
    for recipient in recipients {
        let holder = &recipient.holder;
        let signing = holder.keys.signing.map(|key| key.to_vec()).unwrap_or_default();
        check(tx.execute(
            "insert or ignore into sync_key_recipients_v2 (sync_id, epoch, audience, recipient, encryption, signing) values (?1, ?2, ?3, ?4, ?5, ?6)",
            params![sync_id, epoch, recipient.audience as i64, holder.id, holder.keys.encryption.as_slice(), signing],
        ))?;
    }
    Ok(())
}

impl KeyStore {
    /// Atomically claims a newly generated epoch. A lost insertion race never authorizes the existing key for writes.
    pub(crate) fn insert_for_writes(&self, sync_id: &str, key: &StoredKey, me: &Holder) -> Result<(), &'static str> {
        let mut db = self.lock();
        let tx = check(db.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let epoch = i64::try_from(key.epoch).map_err(|_| FAILED)?;
        let holders = serde_json::to_string(&key.holders).map_err(|_| FAILED)?;
        let inserted = check(tx.execute(
            "insert or ignore into sync_keys (sync_id, epoch, holder, locked, holders) values (?1, ?2, ?3, ?4, ?5)",
            params![sync_id, epoch, key.holder, key.locked, holders],
        ))?;
        if inserted != 1 {
            return Err("session sync: another process stored this key epoch; retry");
        }
        check(tx.execute(
            "insert into sync_write_origins_v2 (sync_id, epoch, holder, locked) values (?1, ?2, ?3, ?4)",
            params![sync_id, epoch, key.holder, key.locked],
        ))?;
        record(&tx, sync_id, epoch, &[Recipient::personal(me)])?;
        check(tx.commit())
    }

    /// Records every possible recipient before publication. Failure or cancellation never removes an exposure.
    pub(crate) fn record_recipients(
        &self,
        sync_id: &str,
        epoch: u64,
        recipients: &[Recipient],
    ) -> Result<(), &'static str> {
        let mut db = self.lock();
        let tx = check(db.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let epoch = i64::try_from(epoch).map_err(|_| FAILED)?;
        let exists: bool = check(tx.query_row(
            "select exists(select 1 from sync_keys where sync_id = ?1 and epoch = ?2)",
            params![sync_id, epoch],
            |row| row.get(0),
        ))?;
        if !exists {
            return Err(FAILED);
        }
        record(&tx, sync_id, epoch, recipients)?;
        check(tx.commit())
    }

    /// The complete local exposure history, only for the exact encrypted key row this runner generated.
    pub(crate) fn write_recipients(
        &self,
        sync_id: &str,
        key: &StoredKey,
    ) -> Result<Option<Vec<Recipient>>, &'static str> {
        let mut db = self.lock();
        let tx = check(db.transaction())?;
        let epoch = i64::try_from(key.epoch).map_err(|_| FAILED)?;
        let origin: Option<i64> = check(tx.query_row(
            "select 1 from sync_write_origins_v2 o join sync_keys k using (sync_id, epoch) where o.sync_id = ?1 and o.epoch = ?2 and o.holder = k.holder and o.locked = k.locked and o.holder = ?3 and o.locked = ?4",
            params![sync_id, epoch, key.holder, key.locked], |row| row.get(0),
        ).optional())?;
        if origin.is_none() {
            return Ok(None);
        }
        let mut statement = check(tx.prepare("select audience, recipient, encryption, signing from sync_key_recipients_v2 where sync_id = ?1 and epoch = ?2"))?;
        let rows = check(statement.query_map(params![sync_id, epoch], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        }))?;
        let mut recipients = Vec::new();
        for row in rows {
            let (audience, id, encryption, signing) = check(row)?;
            let audience = match audience {
                0 => Audience::Personal,
                1 => Audience::Team,
                2 => Audience::Organization,
                _ => return Err(FAILED),
            };
            let keys = Keys {
                encryption: encryption.try_into().map_err(|_| FAILED)?,
                signing: match signing.len() {
                    0 => None,
                    _ => Some(signing.try_into().map_err(|_| FAILED)?),
                },
            };
            recipients.push(Recipient {
                audience,
                holder: Holder { id, keys },
            });
        }
        Ok(Some(recipients))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn holder(id: &str, key: u8) -> Holder {
        Holder {
            id: id.into(),
            keys: Keys {
                encryption: [key; 32],
                signing: Some([key; 32]),
            },
        }
    }

    fn stored(locked: u8) -> StoredKey {
        StoredKey {
            epoch: 0,
            holder: "dev_me".into(),
            locked: vec![locked],
            holders: BTreeSet::new(),
        }
    }

    #[test]
    fn a_lost_insert_race_never_promotes_an_imported_key() {
        let dir = std::env::temp_dir().join(format!("nexus-key-provenance-{}", uuid::Uuid::new_v4()));
        let a = KeyStore::open(&dir.join("keys.sqlite3")).expect("first process");
        let b = KeyStore::open(&dir.join("keys.sqlite3")).expect("second process");
        a.insert("thread", &stored(1)).expect("adopted winner");
        assert!(b.insert_for_writes("thread", &stored(2), &holder("dev_me", 1)).is_err());
        assert_eq!(a.latest("thread").expect("read").expect("winner").locked, vec![1]);
        assert!(a.write_recipients("thread", &stored(1)).expect("history").is_none());
        drop((a, b));
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn exposure_history_retains_each_public_key_and_audience_for_the_same_id() {
        let keys = KeyStore::in_memory().expect("keys");
        keys.insert_for_writes("thread", &stored(1), &holder("dev_me", 1))
            .expect("fresh epoch");
        let old = Recipient::personal(&holder("same_id", 2));
        let replacement = Recipient::personal(&holder("same_id", 3));
        let team = Recipient::shared(Audience::Team, "same_id", &[2; 32]);
        keys.record_recipients(
            "thread",
            0,
            &[old.clone(), replacement.clone(), team.clone(), old.clone()],
        )
        .expect("exposures");
        let history = keys
            .write_recipients("thread", &stored(1))
            .expect("history")
            .expect("local origin");
        assert_eq!(history.len(), 4);
        assert!(history.contains(&old) && history.contains(&replacement) && history.contains(&team));
    }

    #[test]
    fn forgetting_a_thread_removes_its_write_eligibility_before_same_epoch_adoption() {
        let keys = KeyStore::in_memory().expect("keys");
        keys.insert_for_writes("thread", &stored(1), &holder("dev_me", 1))
            .expect("fresh epoch");
        assert!(keys.write_recipients("thread", &stored(1)).expect("history").is_some());
        keys.forget("thread").expect("forget");
        keys.insert("thread", &stored(1)).expect("adopt same row again");
        assert!(keys.write_recipients("thread", &stored(1)).expect("history").is_none());
    }
}
