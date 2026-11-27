//! The adapter's durable state, in its own SQLite file beside the workbench's: each thread's content keys, the devices
//! they were locked for, the key directory's memory (revoked and verified devices, and the approver the person
//! confirmed for this device), the device trust this computer is pinned to, each teammate's pinned root, and the
//! organization recovery key the person confirmed for each organization. A content key is stored only as this device's own HPKE copy, so the
//! file holds nothing readable without the device key in the OS keychain.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::org_key::OrgKey;
use super::trust::{Holder, Keys, Memory};

mod provenance;
pub(crate) use provenance::{Audience, Recipient};
mod trust_pin;

const SCHEMA: &str = "
create table if not exists sync_keys (
  sync_id text not null,
  epoch integer not null check (epoch >= 0),
  holder text not null,
  locked blob not null,
  holders text not null default '[]',
  primary key (sync_id, epoch)
);
create table if not exists sync_revoked (
  owner text not null,
  device_id text not null,
  primary key (owner, device_id)
);
create table if not exists sync_verified_v2 (
  owner text not null,
  device_id text not null,
  encryption blob not null,
  signing blob not null,
  primary key (owner, device_id)
);

create table if not exists sync_confirmed_approvers (
  owner text not null,
  device_id text not null,
  approver_id text not null,
  encryption blob not null,
  signing blob not null,
  confirmed_at integer not null,
  primary key (owner, device_id)
);
create table if not exists sync_vault_org_keys (
  owner text not null,
  tenant_id text not null,
  key_id text not null,
  encryption blob not null,
  confirmed_at integer not null,
  primary key (owner, tenant_id)
);
";

const FAILED: &str = "session sync: the key store failed";

/// A content key as stored: this device's copy, and the devices the key was locked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredKey {
    pub epoch: u64,
    /// The holder id the copy is locked for (this device's id when it was stored).
    pub holder: String,
    pub locked: Vec<u8>,
    pub holders: BTreeSet<String>,
}

type Row = (i64, String, Vec<u8>, String);

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn stored_key((epoch, holder, locked, holders): Row) -> Result<StoredKey, &'static str> {
    Ok(StoredKey {
        epoch: u64::try_from(epoch).map_err(|_| FAILED)?,
        holder,
        locked,
        holders: serde_json::from_str(&holders).map_err(|_| FAILED)?,
    })
}

pub struct KeyStore {
    db: Mutex<Connection>,
}

fn check<T>(result: rusqlite::Result<T>) -> Result<T, &'static str> {
    result.map_err(|_| FAILED)
}

impl KeyStore {
    pub fn open(path: &Path) -> crate::Result<KeyStore> {
        // Only the person may read it: it holds the locked content keys and the trust anchors.
        if let Some(dir) = path.parent() {
            crate::private_fs::create_dir_all(dir).map_err(|_| FAILED.to_string())?;
        }
        crate::private_fs::restrict_file(path).map_err(|_| FAILED.to_string())?;
        let db = Connection::open(path).map_err(|_| FAILED.to_string())?;
        KeyStore::with(db)
    }

    pub fn in_memory() -> crate::Result<KeyStore> {
        KeyStore::with(Connection::open_in_memory().map_err(|_| FAILED.to_string())?)
    }

    fn with(db: Connection) -> crate::Result<KeyStore> {
        db.execute_batch(&format!(
            "pragma journal_mode = wal; pragma synchronous = full; {SCHEMA} {} {}",
            provenance::SCHEMA,
            trust_pin::SCHEMA
        ))
        .map_err(|_| FAILED.to_string())?;
        Ok(KeyStore { db: Mutex::new(db) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn latest(&self, sync_id: &str) -> Result<Option<StoredKey>, &'static str> {
        let db = self.lock();
        let found = check(
            db.query_row(
                "select epoch, holder, locked, holders from sync_keys where sync_id = ?1 order by epoch desc limit 1",
                [sync_id],
                row,
            )
            .optional(),
        )?;
        found.map(stored_key).transpose()
    }

    /// Every stored epoch of the thread, oldest first.
    pub(crate) fn all(&self, sync_id: &str) -> Result<Vec<StoredKey>, &'static str> {
        let db = self.lock();
        let mut statement = check(
            db.prepare("select epoch, holder, locked, holders from sync_keys where sync_id = ?1 order by epoch"),
        )?;
        let rows = check(statement.query_map([sync_id], row))?;
        rows.map(|stored| stored_key(check(stored)?)).collect()
    }

    /// Stores a key the first time; a key already stored at this epoch is kept (a key never changes once stored).
    pub(crate) fn insert(&self, sync_id: &str, key: &StoredKey) -> Result<(), &'static str> {
        let holders = serde_json::to_string(&key.holders).map_err(|_| FAILED)?;
        let epoch = i64::try_from(key.epoch).map_err(|_| FAILED)?;
        check(self.lock().execute(
            "insert into sync_keys (sync_id, epoch, holder, locked, holders) values (?1, ?2, ?3, ?4, ?5)
             on conflict (sync_id, epoch) do nothing",
            params![sync_id, epoch, key.holder, key.locked, holders],
        ))?;
        Ok(())
    }

    /// Records that Gateway stored copies for `holders` too.
    pub(crate) fn add_holders(
        &self,
        sync_id: &str,
        epoch: u64,
        holders: &BTreeSet<String>,
    ) -> Result<(), &'static str> {
        let mut db = self.lock();
        // It reads, then writes. Another process (the app or the background service) may commit in between; a
        // deferred transaction would then fail at once with SQLITE_BUSY_SNAPSHOT, which the busy timeout does not
        // retry. Immediate takes the write lock first, so the other writer waits instead.
        let tx = check(db.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let epoch = i64::try_from(epoch).map_err(|_| FAILED)?;
        let current: String = check(tx.query_row(
            "select holders from sync_keys where sync_id = ?1 and epoch = ?2",
            params![sync_id, epoch],
            |row| row.get(0),
        ))?;
        #[cfg(test)]
        tests::between_read_and_write();
        let mut all: BTreeSet<String> = serde_json::from_str(&current).map_err(|_| FAILED)?;
        all.extend(holders.iter().cloned());
        check(tx.execute(
            "update sync_keys set holders = ?3 where sync_id = ?1 and epoch = ?2",
            params![sync_id, epoch, serde_json::to_string(&all).map_err(|_| FAILED)?],
        ))?;
        check(tx.commit())
    }

    /// Crypto-shredding on this computer: a deleted thread's copies go with it.
    pub(crate) fn forget(&self, sync_id: &str) -> Result<(), &'static str> {
        let mut db = self.lock();
        let tx = check(db.transaction_with_behavior(TransactionBehavior::Immediate))?;
        check(tx.execute("delete from sync_key_recipients_v2 where sync_id = ?1", [sync_id]))?;
        check(tx.execute("delete from sync_write_origins_v2 where sync_id = ?1", [sync_id]))?;
        check(tx.execute("delete from sync_keys where sync_id = ?1", [sync_id]))?;
        check(tx.commit())
    }

    /// Carries observations into a new scope without importing automatically verified keys or root pins.
    pub(crate) fn inherit_revocations(&self, owner: &str, legacy: &str) -> Result<(), &'static str> {
        check(self.lock().execute(
            "insert or ignore into sync_revoked (owner, device_id) select ?1, device_id from sync_revoked where owner = ?2",
            params![owner, legacy],
        ))?;
        Ok(())
    }

    /// Rebuild verified holders from corrected handovers; retain local approvers and observed revocations.
    pub(crate) fn memory(&self, owner: &str) -> Result<Memory, &'static str> {
        let db = self.lock();
        let mut revoked = HashSet::new();
        let mut statement = check(db.prepare("select device_id from sync_revoked where owner = ?1"))?;
        for id in check(statement.query_map([owner], |row| row.get::<_, String>(0)))? {
            revoked.insert(check(id)?);
        }
        let mut verified = HashMap::new();
        let mut statement =
            check(db.prepare("select device_id, encryption, signing from sync_verified_v2 where owner = ?1"))?;
        let rows = check(statement.query_map([owner], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        }))?;
        for row in rows {
            let (id, encryption, signing) = check(row)?;
            let keys = Keys {
                encryption: encryption.try_into().map_err(|_| FAILED)?,
                // A recovery key has no signing key: stored as no bytes.
                signing: match signing.len() {
                    0 => None,
                    _ => Some(signing.try_into().map_err(|_| FAILED)?),
                },
            };
            verified.insert(id, keys);
        }
        let mut anchors = HashMap::new();
        let mut statement = check(db.prepare(
            "select device_id, approver_id, encryption, signing from sync_confirmed_approvers where owner = ?1",
        ))?;
        let rows = check(statement.query_map([owner], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        }))?;
        for row in rows {
            let (device, approver, encryption, signing) = check(row)?;
            let keys = Keys {
                encryption: encryption.try_into().map_err(|_| FAILED)?,
                signing: Some(signing.try_into().map_err(|_| FAILED)?),
            };
            anchors.insert(device, Holder { id: approver, keys });
        }
        Ok(Memory {
            revoked,
            verified,
            anchors,
        })
    }

    /// Records `approver` as the approver the person confirmed by its code for this computer's device `device`. A
    /// later confirmation replaces it. [`KeyStore::remember`] never writes it: a refresh cannot anchor a device.
    pub(crate) fn confirm_approver(&self, owner: &str, device: &str, approver: &Holder) -> Result<(), &'static str> {
        let signing = approver.keys.signing.ok_or(FAILED)?;
        check(self.lock().execute(
            "insert into sync_confirmed_approvers (owner, device_id, approver_id, encryption, signing, confirmed_at)
             values (?1, ?2, ?3, ?4, ?5, ?6)
             on conflict (owner, device_id) do update set approver_id = excluded.approver_id,
               encryption = excluded.encryption, signing = excluded.signing, confirmed_at = excluded.confirmed_at",
            params![
                owner,
                device,
                approver.id,
                approver.keys.encryption.as_slice(),
                signing.as_slice(),
                crate::now_ms()
            ],
        ))?;
        Ok(())
    }

    /// Adds what `memory` learned. Nothing is ever removed: a revocation is final, and a verified device's keys never
    /// change under its id. The anchors are not written here ([`KeyStore::confirm_approver`]).
    pub(crate) fn remember(&self, owner: &str, memory: &Memory) -> Result<(), &'static str> {
        let db = self.lock();
        let tx = check(db.unchecked_transaction())?;
        for id in &memory.revoked {
            check(tx.execute(
                "insert into sync_revoked (owner, device_id) values (?1, ?2) on conflict do nothing",
                params![owner, id],
            ))?;
        }
        for (id, keys) in &memory.verified {
            check(tx.execute(
                "insert into sync_verified_v2 (owner, device_id, encryption, signing) values (?1, ?2, ?3, ?4)
                 on conflict do nothing",
                params![
                    owner,
                    id,
                    keys.encryption.as_slice(),
                    keys.signing.as_ref().map_or(&[][..], |key| key.as_slice())
                ],
            ))?;
        }
        check(tx.commit())
    }

    /// Only confirmations made against the vault source count. Legacy `sync_org_keys` pins grant no access.
    pub(crate) fn pinned_org_key(&self, owner: &str, tenant: &str) -> Result<Option<OrgKey>, &'static str> {
        let found = check(
            self.lock()
                .query_row(
                    "select key_id, encryption from sync_vault_org_keys where owner = ?1 and tenant_id = ?2",
                    params![owner, tenant],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
                )
                .optional(),
        )?;
        found
            .map(|(id, encryption)| {
                Ok(OrgKey {
                    id,
                    encryption: encryption.try_into().map_err(|_| FAILED)?,
                })
            })
            .transpose()
    }

    /// Records `key` as the organization recovery key the person confirmed for `tenant`. A later confirmation, after the
    /// organization replaces its key, replaces it. Only a confirmation calls this, never a sync pass.
    pub(crate) fn pin_org_key(&self, owner: &str, tenant: &str, key: &OrgKey) -> Result<(), &'static str> {
        check(self.lock().execute(
            "insert into sync_vault_org_keys (owner, tenant_id, key_id, encryption, confirmed_at) values (?1, ?2, ?3, ?4, ?5)
             on conflict (owner, tenant_id) do update set key_id = excluded.key_id, encryption = excluded.encryption,
               confirmed_at = excluded.confirmed_at",
            params![owner, tenant, key.id, key.encryption.as_slice(), crate::now_ms()],
        ))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        /// Runs once inside `add_holders`, after its read and before its write.
        static BETWEEN: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    }

    pub(super) fn between_read_and_write() {
        if let Some(step) = BETWEEN.with(|hook| hook.borrow_mut().take()) {
            step();
        }
    }

    /// The app and the background service both open the key store. When the other one commits between this one's
    /// read and its write, a deferred transaction fails at once with SQLITE_BUSY_SNAPSHOT, past the busy timeout.
    /// An immediate one holds the write lock from its start, so the other write waits until it commits.
    #[test]
    fn adding_holders_holds_the_write_lock_from_its_read() {
        let dir = std::env::temp_dir().join(format!("nexus-sync-keys-{}", uuid::Uuid::new_v4()));
        let path = dir.join("keys.sqlite3");
        let ours = KeyStore::open(&path).expect("open");
        let key = |holder: &str| StoredKey {
            epoch: 0,
            holder: holder.into(),
            locked: vec![1],
            holders: BTreeSet::new(),
        };
        ours.insert("t", &key("dev_a")).expect("insert");
        let theirs = Connection::open(&path).expect("open the other");
        // The other process's write, tried without waiting: inside our transaction it must not get in.
        theirs.busy_timeout(std::time::Duration::ZERO).expect("timeout");
        let landed = std::rc::Rc::new(RefCell::new(None));
        let seen = landed.clone();
        BETWEEN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let write = theirs.execute(
                    "insert into sync_keys (sync_id, epoch, holder, locked) values ('u', 0, 'dev_x', x'01')",
                    [],
                );
                *seen.borrow_mut() = Some((write, theirs));
            }))
        });
        ours.add_holders("t", 0, &BTreeSet::from(["dev_b".to_string()]))
            .expect("holders are added even when the other process writes");
        let (write, theirs) = landed.borrow_mut().take().expect("the hook ran");
        assert!(write.is_err(), "the other write waits for our commit");
        theirs.busy_timeout(std::time::Duration::from_secs(5)).expect("timeout");
        theirs
            .execute(
                "insert into sync_keys (sync_id, epoch, holder, locked) values ('u', 0, 'dev_x', x'01')",
                [],
            )
            .expect("it lands after our commit");
        assert_eq!(ours.latest("t").expect("read").expect("stored").holders.len(), 1);
        assert!(ours.latest("u").expect("read").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn handover_v2_retires_legacy_verified_keys_but_keeps_confirmations_and_revocations() {
        let store = KeyStore::in_memory().expect("keys");
        store
            .lock()
            .execute_batch(
                "create table if not exists sync_verified (owner text, device_id text, encryption blob, signing blob);",
            )
            .expect("legacy table");
        store
            .lock()
            .execute(
                "insert into sync_verified values (?1, ?2, ?3, ?4)",
                params!["me", "dev_f", &[1u8; 32][..], &[2u8; 32][..]],
            )
            .expect("legacy verified holder");
        let approver = Holder {
            id: "dev_b".into(),
            keys: Keys {
                encryption: [3; 32],
                signing: Some([4; 32]),
            },
        };
        store
            .confirm_approver("me", "dev_c", &approver)
            .expect("local confirmation");
        store
            .remember(
                "me",
                &Memory {
                    revoked: HashSet::from(["dev_revoked".into()]),
                    ..Default::default()
                },
            )
            .expect("revocation");
        let memory = store.memory("me").expect("upgraded memory");
        assert!(memory.verified.is_empty());
        assert_eq!(memory.anchors.get("dev_c"), Some(&approver));
        assert!(memory.revoked.contains("dev_revoked"));
    }

    #[test]
    fn keys_and_memory_survive_a_reopen() {
        let dir = std::env::temp_dir().join(format!("nexus-sync-keys-{}", uuid::Uuid::new_v4()));
        let path = dir.join("keys.sqlite3");
        let store = KeyStore::open(&path).expect("open");
        let key = StoredKey {
            epoch: 0,
            holder: "dev_a".into(),
            locked: vec![1, 2, 3],
            holders: BTreeSet::new(),
        };
        store.insert("t", &key).expect("insert");
        store
            .insert(
                "t",
                &StoredKey {
                    locked: vec![9],
                    ..key.clone()
                },
            )
            .expect("kept");
        store
            .add_holders("t", 0, &BTreeSet::from(["dev_a".to_string(), "dev_b".to_string()]))
            .expect("holders");
        let mut memory = Memory::default();
        memory.revoked.insert("dev_c".into());
        memory.verified.insert(
            "dev_a".into(),
            Keys {
                encryption: [1; 32],
                signing: Some([2; 32]),
            },
        );
        // A recovery key, with no signing key.
        memory.verified.insert(
            "dev_code".into(),
            Keys {
                encryption: [3; 32],
                signing: None,
            },
        );
        store.remember("me", &memory).expect("remember");
        drop(store);
        let store = KeyStore::open(&path).expect("reopen");
        let latest = store.latest("t").expect("read").expect("stored");
        assert_eq!(latest.locked, vec![1, 2, 3], "a stored key never changes");
        assert_eq!(latest.holders.len(), 2);
        assert_eq!(store.memory("me").expect("memory"), memory);
        assert_eq!(store.memory("other").expect("memory"), Memory::default());
        store.forget("t").expect("forget");
        assert!(store.latest("t").expect("read").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_confirmed_approver_survives_a_reopen_and_only_a_confirmation_writes_it() {
        let dir = std::env::temp_dir().join(format!("nexus-sync-keys-{}", uuid::Uuid::new_v4()));
        let path = dir.join("keys.sqlite3");
        let store = KeyStore::open(&path).expect("open");
        let approver = |id: &str, byte: u8| Holder {
            id: id.into(),
            keys: Keys {
                encryption: [byte; 32],
                signing: Some([byte + 1; 32]),
            },
        };
        // A refresh's memory never anchors a device.
        let mut learned = Memory::default();
        learned.anchors.insert("dev_me".into(), approver("dev_f", 9));
        store.remember("me", &learned).expect("remember");
        assert!(store.memory("me").expect("memory").anchors.is_empty());
        store
            .confirm_approver("me", "dev_me", &approver("dev_a", 1))
            .expect("confirm");
        drop(store);
        let store = KeyStore::open(&path).expect("reopen");
        let anchors = store.memory("me").expect("memory").anchors;
        assert_eq!(anchors.get("dev_me"), Some(&approver("dev_a", 1)));
        assert!(store.memory("other").expect("memory").anchors.is_empty());
        // The person confirms again: the new approver replaces the old.
        store
            .confirm_approver("me", "dev_me", &approver("dev_b", 3))
            .expect("confirm again");
        assert_eq!(
            store.memory("me").expect("memory").anchors.get("dev_me"),
            Some(&approver("dev_b", 3))
        );
        // A recovery key approves nothing, so it is never an anchor.
        let mut code = approver("dev_code", 5);
        code.keys.signing = None;
        assert_eq!(store.confirm_approver("me", "dev_me", &code), Err(FAILED));
        let _ = std::fs::remove_dir_all(dir);
    }
}
