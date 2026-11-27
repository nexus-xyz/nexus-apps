//! The local cache (SQLite, compiled in): Nexus UI's query answers as the webview last saw them, keyed by React
//! Query's hash (its key carries the organization and a fingerprint of the credential), so the next launch shows
//! sessions, messages, artifacts, the model catalog and spend before the network answers. One file in the app's data
//! folder, WAL mode; trimmed to the most recent entries; wiped on sign-out.

use std::path::Path;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How many answers the cache keeps (the most recently updated).
pub const MAX_ENTRIES: i64 = 600;
/// Answers above this size stay in memory only.
pub const MAX_ENTRY_BYTES: usize = 2_000_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub hash: String,
    pub key: Value,
    pub data: Value,
    /// When the answer arrived (React Query's `dataUpdatedAt`, milliseconds since the epoch).
    pub updated_at: i64,
}

pub struct Cache {
    db: Connection,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cache")
    }
}

impl Cache {
    pub fn open(path: &Path) -> rusqlite::Result<Cache> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Cache::with(Connection::open(path)?)
    }

    pub fn in_memory() -> rusqlite::Result<Cache> {
        Cache::with(Connection::open_in_memory()?)
    }

    fn with(db: Connection) -> rusqlite::Result<Cache> {
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS queries (
                hash TEXT PRIMARY KEY,
                key TEXT NOT NULL,
                data TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS queries_updated ON queries (updated_at DESC);",
        )?;
        Ok(Cache { db })
    }

    /// Every saved answer, newest first.
    pub fn load(&self) -> rusqlite::Result<Vec<Entry>> {
        let mut statement = self
            .db
            .prepare_cached("SELECT hash, key, data, updated_at FROM queries ORDER BY updated_at DESC LIMIT ?1")?;
        let rows = statement.query_map(params![MAX_ENTRIES], |row| {
            let key: String = row.get(1)?;
            let data: String = row.get(2)?;
            Ok(Entry {
                hash: row.get(0)?,
                key: serde_json::from_str(&key).unwrap_or(Value::Null),
                data: serde_json::from_str(&data).unwrap_or(Value::Null),
                updated_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// One scoped answer, without loading other accounts' data into memory.
    pub fn get(&self, hash: &str) -> rusqlite::Result<Option<Entry>> {
        use rusqlite::OptionalExtension;
        let mut statement = self
            .db
            .prepare_cached("SELECT key,data,updated_at FROM queries WHERE hash=?1")?;
        let raw: Option<(String, String, i64)> = statement
            .query_row(params![hash], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?;
        Ok(raw.and_then(|(key, data, updated_at)| {
            Some(Entry {
                hash: hash.into(),
                key: serde_json::from_str(&key).ok()?,
                data: serde_json::from_str(&data).ok()?,
                updated_at,
            })
        }))
    }

    /// Save answers (an older answer never replaces a newer one), then trim to `MAX_ENTRIES`.
    pub fn put(&mut self, entries: &[Entry]) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        {
            let mut statement = tx.prepare_cached(
                "INSERT INTO queries (hash, key, data, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (hash) DO UPDATE SET key = excluded.key, data = excluded.data, updated_at = excluded.updated_at
                 WHERE excluded.updated_at >= queries.updated_at",
            )?;
            for entry in entries {
                if entry.data.to_string().len() > MAX_ENTRY_BYTES {
                    continue;
                }
                statement.execute(params![
                    entry.hash,
                    entry.key.to_string(),
                    entry.data.to_string(),
                    entry.updated_at
                ])?;
            }
        }
        tx.execute(
            "DELETE FROM queries WHERE hash NOT IN (SELECT hash FROM queries ORDER BY updated_at DESC LIMIT ?1)",
            params![MAX_ENTRIES],
        )?;
        tx.commit()
    }

    /// Invalidate one credential's reads after a write. The prefix is data, not a LIKE pattern.
    pub fn remove_prefix(&mut self, prefix: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "DELETE FROM queries WHERE substr(hash,1,length(?1))=?1",
            params![prefix],
        )?;
        Ok(())
    }

    /// Forget everything (sign-out).
    pub fn wipe(&mut self) -> rusqlite::Result<()> {
        self.db.execute("DELETE FROM queries", [])?;
        self.db.execute_batch("VACUUM")?;
        // VACUUM in WAL mode still leaves old frames until an explicit checkpoint.
        let busy: i64 = self
            .db
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        if busy != 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("query-cache checkpoint is busy".into()),
            ));
        }
        Ok(())
    }

    /// The newest saved sessions list (the sidebar's), for the tray: `(id, title)`, most recent first.
    pub fn recent_sessions(&self, limit: usize) -> Vec<(String, String)> {
        let Ok(mut statement) = self.db.prepare_cached(
            "SELECT data FROM queries WHERE key LIKE '%\"chat-sessions\"%' AND key NOT LIKE '%\"q\":%'
             ORDER BY updated_at DESC LIMIT 1",
        ) else {
            return Vec::new();
        };
        let data: Option<String> = statement.query_row([], |row| row.get(0)).ok();
        let Some(Value::Object(list)) = data.and_then(|text| serde_json::from_str(&text).ok()) else {
            return Vec::new();
        };
        list.get("sessions")
            .and_then(Value::as_array)
            .map(|sessions| {
                sessions
                    .iter()
                    .filter(|session| session.get("archived").and_then(Value::as_bool) != Some(true))
                    .filter_map(|session| {
                        Some((
                            session.get("id")?.as_str()?.to_string(),
                            session
                                .get("title")
                                .and_then(Value::as_str)
                                .unwrap_or("Untitled")
                                .to_string(),
                        ))
                    })
                    .take(limit)
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(hash: &str, data: Value, updated_at: i64) -> Entry {
        Entry {
            hash: hash.into(),
            key: json!(["nexus", "acme", "fp", hash]),
            data,
            updated_at,
        }
    }

    #[test]
    fn keeps_the_newest_answer_and_lists_sessions() {
        let mut cache = Cache::in_memory().expect("open");
        cache.put(&[entry("me", json!({ "v": 2 }), 20)]).expect("put");
        cache.put(&[entry("me", json!({ "v": 1 }), 10)]).expect("older");
        assert_eq!(cache.load().expect("load")[0].data, json!({ "v": 2 }));

        let sessions = json!({ "sessions": [
            { "id": "chs_1", "title": "First" },
            { "id": "chs_2", "title": "Old", "archived": true },
            { "id": "chs_3" }
        ] });
        cache
            .put(&[Entry {
                hash: "list".into(),
                key: json!(["nexus", "acme", "fp", "chat-sessions", { "limit": 20 }]),
                data: sessions,
                updated_at: 30,
            }])
            .expect("put list");
        assert_eq!(
            cache.recent_sessions(5),
            vec![
                ("chs_1".to_string(), "First".to_string()),
                ("chs_3".to_string(), "Untitled".to_string())
            ]
        );
        cache.wipe().expect("wipe");
        assert!(cache.load().expect("load").is_empty());
        assert!(cache.recent_sessions(5).is_empty());
    }

    #[test]
    fn trims_to_the_newest() {
        let mut cache = Cache::in_memory().expect("open");
        let many: Vec<Entry> = (0..(MAX_ENTRIES + 5))
            .map(|i| entry(&format!("q{i}"), json!(i), i))
            .collect();
        cache.put(&many).expect("put");
        let loaded = cache.load().expect("load");
        assert_eq!(loaded.len() as i64, MAX_ENTRIES);
        assert_eq!(loaded[0].hash, format!("q{}", MAX_ENTRIES + 4));
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn lookup_respects_entry_limits_and_keeps_a_newer_value() {
        let mut cache = Cache::in_memory().expect("memory cache");
        let entry = Entry {
            hash: "scope".into(),
            key: json!(["one"]),
            data: json!({"v":2}),
            updated_at: 20,
        };
        cache.put(std::slice::from_ref(&entry)).expect("new answer");
        cache
            .put(&[Entry {
                data: json!({"v":1}),
                updated_at: 10,
                ..entry.clone()
            }])
            .expect("late old answer");
        assert_eq!(cache.get("scope").expect("lookup"), Some(entry.clone()));
        cache
            .put(&[Entry {
                hash: "large".into(),
                data: json!("x".repeat(MAX_ENTRY_BYTES)),
                ..entry
            }])
            .expect("oversized answer skipped");
        assert_eq!(cache.get("large").expect("large lookup"), None);
    }
}

#[cfg(test)]
mod wipe_tests {
    use super::*;
    #[test]
    fn logout_removes_payload_bytes_even_while_the_connection_stays_open() {
        let directory = std::env::temp_dir().join(format!("nexus-cache-wipe-{}", uuid::Uuid::new_v4()));
        let path = directory.join("queries.sqlite3");
        let mut wal = path.as_os_str().to_os_string();
        wal.push("-wal");
        let wal = std::path::PathBuf::from(wal);
        let mut cache = Cache::open(&path).expect("cache");
        let marker = "private-payload-that-logout-must-erase";
        cache
            .put(&[Entry {
                hash: "scope".into(),
                key: serde_json::json!(["one"]),
                data: serde_json::json!({"message":marker}),
                updated_at: 1,
            }])
            .expect("put");
        let contains = |bytes: Vec<u8>| bytes.windows(marker.len()).any(|window| window == marker.as_bytes());
        assert!(contains(std::fs::read(&wal).expect("WAL fixture")));
        cache.wipe().expect("wipe and checkpoint");
        assert!(!contains(std::fs::read(&path).expect("database")));
        assert!(!contains(std::fs::read(&wal).unwrap_or_default()));
        assert!(cache.load().expect("logical rows").is_empty());
        drop(cache);
        let _ = std::fs::remove_dir_all(directory);
    }
}
