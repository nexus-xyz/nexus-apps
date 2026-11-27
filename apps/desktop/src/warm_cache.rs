//! Scoped Gateway query answers. No credential or native account state goes to disk.
use nexus_client::Client;
use nexus_desktop_core::cache::{Cache, Entry};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

pub struct WarmCache {
    db: Mutex<Option<Cache>>,
}

impl WarmCache {
    pub fn new() -> Self {
        #[cfg(test)]
        {
            Self::at(None, false)
        }
        #[cfg(not(test))]
        {
            Self::at(
                Some(&crate::paths::cache_dir().join("queries.sqlite3")),
                crate::managed::policy().allow_local_cache,
            )
        }
    }
    fn at(path: Option<&std::path::Path>, allowed: bool) -> Self {
        let db = match path {
            Some(path) if allowed => Cache::open(path).ok().or_else(|| Cache::in_memory().ok()),
            Some(path) => {
                for suffix in ["", "-wal", "-shm"] {
                    let mut sidecar = path.as_os_str().to_os_string();
                    sidecar.push(suffix);
                    let _ = std::fs::remove_file(std::path::PathBuf::from(sidecar));
                }
                Cache::in_memory().ok()
            }
            None => Cache::in_memory().ok(),
        };
        Self { db: Mutex::new(db) }
    }
    pub fn get(&self, key: &str) -> Option<Value> {
        Some(self.db.lock().ok()?.as_ref()?.get(key).ok()??.data)
    }
    pub fn put(&self, key: &str, value: &Value, epoch: &AtomicU64, expected: u64) {
        self.put_guarded(key, value, || epoch.load(Ordering::SeqCst) == expected);
    }
    pub fn put_guarded(&self, key: &str, value: &Value, current: impl Fn() -> bool) {
        let Ok(mut guard) = self.db.lock() else { return };
        if !current() {
            return;
        }
        if let Some(db) = guard.as_mut() {
            let at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            let _ = db.put(&[Entry {
                hash: key.into(),
                key: json!(["gpui-query", key]),
                data: value.clone(),
                updated_at: at,
            }]);
        }
    }
    pub fn invalidate(&self, client: &Client, revision: &AtomicU64) {
        self.invalidate_versioned(client, revision, None);
    }
    pub fn invalidate_versioned(&self, client: &Client, revision: &AtomicU64, family: Option<&AtomicU64>) {
        let Ok(mut guard) = self.db.lock() else { return };
        if let Some(family) = family {
            family.fetch_add(1, Ordering::SeqCst);
        }
        revision.fetch_add(1, Ordering::SeqCst);
        if let (Some(db), Some((gateway, fingerprint))) = (guard.as_mut(), scope(client)) {
            let mut prefix = json!([gateway, fingerprint]).to_string();
            prefix.pop();
            prefix.push(',');
            let _ = db.remove_prefix(&prefix);
        }
    }
    pub fn snapshot(&self, keys: &[Option<String>], revision: &AtomicU64) -> (Vec<Option<Value>>, u64) {
        let Ok(guard) = self.db.lock() else {
            return (vec![None; keys.len()], revision.load(Ordering::SeqCst));
        };
        let stamp = revision.load(Ordering::SeqCst);
        let values = keys
            .iter()
            .map(|key| key.as_ref().and_then(|key| Some(guard.as_ref()?.get(key).ok()??.data)))
            .collect();
        (values, stamp)
    }
    pub fn wipe(&self) {
        if let Ok(mut guard) = self.db.lock()
            && let Some(db) = guard.as_mut()
        {
            if let Err(error) = db.wipe() {
                eprintln!("nexus-desktop-gpui: cache sign-out cleanup: {error}");
            }
        }
    }
}

pub fn key(client: &Client, path: &str) -> Option<String> {
    // Native account state and image payloads are memory-only in persist.ts too.
    if !path.starts_with("/v1/")
        || path.contains("/attachments/")
        || path.contains("/attachment/")
        || path.starts_with("/v1/cloud/account")
    {
        return None;
    }
    let (gateway, fingerprint) = scope(client)?;
    Some(json!([gateway, fingerprint, path]).to_string())
}
fn scope(client: &Client) -> Option<(String, String)> {
    let options = client.options();
    let token = options.api_key.as_ref()?;
    let mut hash = Sha256::new();
    hash.update(token.as_bytes());
    hash.update([0]);
    hash.update(serde_json::to_vec(&options.headers).ok()?);
    let fingerprint = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Some((options.base_url.trim_end_matches('/').to_string(), fingerprint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn client(url: &str, token: &str) -> Client {
        Client::new(crate::gateway::client_options(url, Some(token.into())))
    }
    fn temp() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("nexus-warm-{}", uuid::Uuid::new_v4()))
            .join("queries.sqlite3")
    }

    #[test]
    fn scopes_answers_and_never_stores_the_credential() {
        let cache = WarmCache::new();
        let epoch = AtomicU64::new(0);
        let one = key(&client("http://one.invalid", "secret-one"), "/v1/usage").expect("signed-in key");
        let two = key(&client("http://one.invalid", "secret-two"), "/v1/usage").expect("second key");
        let other = key(&client("http://two.invalid", "secret-one"), "/v1/usage").expect("other gateway");
        assert!(!one.contains("secret-one"));
        cache.put(&one, &json!({"spend":"12.34"}), &epoch, 0);
        assert_eq!(cache.get(&one), Some(json!({"spend":"12.34"})));
        assert_eq!(cache.get(&two), None);
        assert_eq!(cache.get(&other), None);
        assert!(
            key(
                &client("http://one.invalid", "secret-one"),
                "/v1/chat/sessions/x/attachments/a"
            )
            .is_none()
        );
        assert!(key(&client("http://one.invalid", "secret-one"), "/v1/cloud/account").is_none());
        assert!(
            key(
                &Client::new(crate::gateway::client_options("http://one.invalid", None)),
                "/v1/usage"
            )
            .is_none()
        );
    }
    #[test]
    fn a_late_answer_cannot_repopulate_a_signed_out_cache() {
        let cache = Arc::new(WarmCache::new());
        let epoch = AtomicU64::new(0);
        cache.put("one", &json!(1), &epoch, 0);
        epoch.store(1, Ordering::SeqCst);
        cache.wipe();
        cache.put("one", &json!(2), &epoch, 0);
        assert_eq!(cache.get("one"), None);
        cache.put("two", &json!(3), &epoch, 1);
        assert_eq!(cache.get("two"), Some(json!(3)));
    }
    #[test]
    fn reopens_saved_answers_and_policy_off_removes_disk_state() {
        let path = temp();
        let epoch = AtomicU64::new(0);
        {
            let cache = WarmCache::at(Some(&path), true);
            cache.put("scoped", &json!({"data":"saved"}), &epoch, 0);
        }
        {
            let cache = WarmCache::at(Some(&path), true);
            assert_eq!(cache.get("scoped"), Some(json!({"data":"saved"})));
        }
        {
            let cache = WarmCache::at(Some(&path), false);
            assert_eq!(cache.get("scoped"), None);
            cache.put("memory", &json!(true), &epoch, 0);
            assert_eq!(cache.get("memory"), Some(json!(true)));
            assert!(!path.exists());
        }
        let _ = std::fs::remove_dir_all(path.parent().expect("temporary folder"));
    }
    #[test]
    fn a_damaged_cache_falls_back_to_memory() {
        let path = temp();
        std::fs::create_dir_all(path.parent().expect("folder")).expect("create folder");
        std::fs::write(&path, b"damaged SQLite data").expect("damage cache fixture");
        let cache = WarmCache::at(Some(&path), true);
        let epoch = AtomicU64::new(0);
        cache.put("fresh", &json!([1, 2]), &epoch, 0);
        assert_eq!(cache.get("fresh"), Some(json!([1, 2])));
        drop(cache);
        let _ = std::fs::remove_dir_all(path.parent().expect("folder"));
    }
    #[test]
    fn a_write_invalidates_only_its_account_and_blocks_late_cache_puts() {
        let cache = WarmCache::new();
        let epoch = AtomicU64::new(0);
        let revision = AtomicU64::new(0);
        let one = client("http://one.invalid", "one");
        let two = client("http://one.invalid", "two");
        let a = key(&one, "/v1/usage").expect("key");
        let b = key(&two, "/v1/usage").expect("key");
        cache.put(&a, &json!(1), &epoch, 0);
        cache.put(&b, &json!(2), &epoch, 0);
        cache.invalidate(&one, &revision);
        cache.put_guarded(&a, &json!("old read"), || revision.load(Ordering::SeqCst) == 0);
        assert_eq!(cache.get(&a), None);
        assert_eq!(cache.get(&b), Some(json!(2)));
        cache.put_guarded(&a, &json!("fresh"), || revision.load(Ordering::SeqCst) == 1);
        assert_eq!(cache.get(&a), Some(json!("fresh")));
    }
}
