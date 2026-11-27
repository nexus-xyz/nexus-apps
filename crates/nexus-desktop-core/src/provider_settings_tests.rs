use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Keys(Mutex<HashMap<KeyRef, String>>, AtomicBool);

impl KeySource for Keys {
    fn read(&self, key: &KeyRef) -> Result<Option<Secret>, ProviderError> {
        Ok(self.0.lock().expect("keys").get(key).cloned().map(Secret::new))
    }

    fn write(&self, key: &KeyRef, secret: Secret) -> Result<(), ProviderError> {
        self.0.lock().expect("keys").insert(key.clone(), secret.expose().into());
        Ok(())
    }

    fn delete(&self, key: &KeyRef) -> Result<(), ProviderError> {
        if self.1.load(Ordering::Relaxed) {
            return Err(ProviderError::KeychainUnread("test keyring locked".into()));
        }
        self.0.lock().expect("keys").remove(key);
        Ok(())
    }
}

fn folder() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nexus-provider-settings-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("test folder");
    dir
}

/// Pause A after it reads, then start B while A still holds the transaction open.
fn overlapping_edits(
    dir: &Path,
    first: impl FnOnce(ProviderSettingsStore) + Send,
    second: impl FnOnce(ProviderSettingsStore) + Send,
) {
    let (a_read, a_paused) = mpsc::channel();
    let (resume_a, a_resume) = mpsc::channel();
    let a_resume = Mutex::new(a_resume);
    let (b_started, b_running) = mpsc::channel();
    let (b_read, b_waiting) = mpsc::channel();
    let wait = Duration::from_secs(5);
    std::thread::scope(|scope| {
        let mut a = ProviderSettingsStore::in_config(dir);
        a.after_read = Some(Box::new(move || {
            a_read.send(()).expect("A paused");
            a_resume
                .lock()
                .expect("resume receiver")
                .recv_timeout(wait)
                .expect("resume A");
        }));
        scope.spawn(move || first(a));
        a_paused.recv_timeout(wait).expect("A reached the middle of its edit");

        // An independent file handle sees the lock held, even if the scheduler delays B.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join("providers.json.lock"));
        let held = match lock {
            Ok(file) => matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
            Err(_) => false,
        };

        let mut b = ProviderSettingsStore::in_config(dir);
        b.after_read = Some(Box::new(move || b_read.send(()).expect("B read")));
        scope.spawn(move || {
            b_started.send(()).expect("B started");
            second(b);
        });
        b_running.recv_timeout(wait).expect("B started");
        let blocked = b_waiting.recv_timeout(Duration::from_millis(100)) == Err(mpsc::RecvTimeoutError::Timeout);
        resume_a.send(()).expect("resume A");
        assert!(held, "A holds the lock across the read and write");
        assert!(blocked, "B entered its edit while A was paused");
        b_waiting
            .recv_timeout(wait)
            .expect("B enters after A releases the lock");
    });
}

#[test]
fn two_writers_keep_both_servers() {
    let dir = folder();
    overlapping_edits(
        &dir,
        |a| {
            a.add_server("first".into(), "http://localhost:1234/v1", vec![], None)
                .expect("first server");
        },
        |b| {
            b.add_server("second".into(), "http://localhost:1234/v1", vec![], None)
                .expect("second server");
        },
    );
    let settings = ProviderSettingsStore::in_config(&dir).read().expect("settings");
    let mut names: Vec<_> = settings.servers.iter().map(|server| server.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["first", "second"]);
    std::fs::remove_dir_all(dir).expect("remove fixture");
}

#[test]
fn rotation_and_an_edit_keep_both_changes() {
    let dir = folder();
    let store = ProviderSettingsStore::in_config(&dir);
    let keys = Keys::default();
    let id = store
        .add_server("before".into(), "http://localhost:1234/v1", vec![], None)
        .expect("server");
    store
        .rotate_server_key(&keys, &id, "http://localhost:1234/v1", Secret::new("before-key".into()))
        .expect("first key");
    overlapping_edits(
        &dir,
        |a| {
            a.rotate_server_key(&keys, &id, "http://localhost:1234/v1", Secret::new("after-key".into()))
                .expect("rotate")
        },
        |b| {
            b.edit_server(&id, "after".into(), "http://localhost:1234/v1", vec!["model".into()])
                .expect("edit")
        },
    );
    let settings = store.read().expect("settings");
    let server = &settings.servers[0];
    assert_eq!(server.name, "after");
    assert_eq!(server.models, ["model"]);
    assert_eq!(
        keys.read(server.key.as_ref().expect("key"))
            .expect("store")
            .expect("value")
            .expose(),
        "after-key"
    );
    assert_eq!(keys.0.lock().expect("keys").len(), 1);
    let bytes = std::fs::read_to_string(dir.join("providers.json")).expect("settings file");
    assert!(!bytes.contains("before-key") && !bytes.contains("after-key"));
    std::fs::remove_dir_all(dir).expect("remove fixture");
}

#[test]
fn a_keyed_address_does_not_change_without_explicitly_removing_the_key() {
    let dir = folder();
    let settings = ProviderSettingsStore::in_config(&dir);
    let keys = Keys::default();
    let id = settings
        .add_server("Local".into(), "https://one.invalid/v1", vec![], None)
        .expect("server");
    settings
        .rotate_server_key(&keys, &id, "https://one.invalid/v1", Secret::new("test-key".into()))
        .expect("key");
    assert!(
        settings
            .edit_server(&id, "Local".into(), "https://two.invalid/v1", vec![])
            .is_err()
    );
    assert_eq!(
        settings.read().expect("settings").servers[0].base_url,
        "https://one.invalid/v1"
    );
    settings.clear_server_key(&keys, &id).expect("remove key");
    settings
        .edit_server(&id, "Local".into(), "https://two.invalid/v1", vec![])
        .expect("change address");
    let stale = settings.rotate_server_key(&keys, &id, "https://one.invalid/v1", Secret::new("stale-key".into()));
    assert!(stale.is_err());
    assert!(settings.read().expect("settings").servers[0].key.is_none());
    assert!(keys.0.lock().expect("keys").is_empty());
    std::fs::remove_dir_all(dir).expect("remove fixture");
}

#[test]
fn failed_rotation_cleans_up_the_staged_key() {
    let dir = folder();
    let store = ProviderSettingsStore::in_config(&dir);
    let keys = Keys::default();
    let id = store
        .add_api_key(&keys, KeyVendor::Anthropic, Secret::new("original".into()))
        .expect("key");
    let original = store.read().expect("settings").api_keys.remove(0);
    // Inject failure at the transaction boundary. The old settings and their key still work.
    let result = store.stage_key(&keys, Secret::new("replacement".into()), |_, _| {
        Err::<(), _>(missing_provider())
    });
    assert!(result.is_err());
    assert_eq!(store.read().expect("settings").api_keys[0].id, id);
    assert_eq!(
        keys.read(&original.key).expect("store").expect("value").expose(),
        "original"
    );
    assert_eq!(keys.0.lock().expect("keys").len(), 1);
    std::fs::remove_dir_all(dir).expect("remove fixture");
}

const CHILD_DIR: &str = "NEXUS_TEST_PROVIDER_LOCK_HOLDER";

#[test]
fn lock_holder_process() {
    let Some(dir) = std::env::var_os(CHILD_DIR).map(PathBuf::from) else {
        return;
    };
    let _lock = lock_provider_settings(&dir.join("providers.json")).expect("child lock");
    std::fs::write(dir.join("ready"), b"").expect("child ready");
    loop {
        std::thread::park();
    }
}

#[test]
fn dead_lock_holder_does_not_block_the_next_writer() {
    let dir = folder();
    let mut child = nexus_workbench::process::command(std::env::current_exe().expect("test binary"))
        .args(["--exact", "provider_settings::tests::lock_holder_process"])
        .env(CHILD_DIR, &dir)
        .spawn()
        .expect("holder process");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !dir.join("ready").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let was_ready = dir.join("ready").exists();
    let lockfile = dir.join("providers.json.lock");
    let probe = OpenOptions::new().read(true).write(true).open(&lockfile);
    let is_locked = probe
        .as_ref()
        .is_ok_and(|file| matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    child.kill().expect("kill holder");
    child.wait().expect("reap holder");
    assert!(was_ready && is_locked);
    drop(probe);
    let probe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(lockfile)
        .expect("lock file still exists");
    probe.try_lock().expect("OS released the killed process's lock");
    probe.unlock().expect("release probe");
    ProviderSettingsStore::in_config(&dir)
        .add_server("next".into(), "http://localhost:1234/v1", vec![], None)
        .expect("new writer");
    std::fs::remove_dir_all(dir).expect("remove fixture");
}

#[test]
fn cleartext_non_loopback_servers_cannot_save_keys() {
    let dir = folder();
    let settings = ProviderSettingsStore::in_config(&dir);
    let keys = Keys::default();
    let base = "http://server.invalid/v1";
    let denied = settings.add_server(
        "Keyed".into(),
        base,
        vec![],
        Some((&keys, Secret::new("fake-key".into()))),
    );
    assert!(matches!(denied, Err(ProviderError::Refused(_))));
    let id = settings
        .add_server("Keyless".into(), base, vec![], None)
        .expect("save keyless server");
    let denied = settings.rotate_server_key(&keys, &id, base, Secret::new("fake-key".into()));
    assert!(matches!(denied, Err(ProviderError::Refused(_))));
    let stored = settings.read().expect("settings");
    assert_eq!(stored.servers.len(), 1);
    assert_eq!(stored.servers[0].base_url, base);
    assert!(stored.servers[0].key.is_none());
    assert!(keys.0.lock().expect("keys").is_empty());
    std::fs::remove_dir_all(dir).expect("remove fixture");
}

#[test]
fn a_removed_key_stays_queued_until_the_store_unlocks() {
    let dir = folder();
    let store = ProviderSettingsStore::in_config(&dir);
    let keys = Keys::default();
    let id = store
        .add_api_key(&keys, KeyVendor::Anthropic, Secret::new("fake-original".into()))
        .expect("key");
    let key = store.read().expect("settings").api_keys.remove(0).key;
    keys.1.store(true, Ordering::Relaxed);
    assert!(store.remove(&keys, &id).is_err());
    assert!(store.read().expect("settings").api_keys.is_empty());
    assert_eq!(
        store.read_file().expect("journal").pending_key_deletes,
        std::slice::from_ref(&key)
    );
    assert!(keys.read(&key).expect("keychain").is_some());

    keys.1.store(false, Ordering::Relaxed);
    let reopened = ProviderSettingsStore::in_config(&dir);
    reopened.retry_key_cleanup(&keys).expect("retry old deletion");
    assert!(reopened.read_file().expect("journal").pending_key_deletes.is_empty());
    assert!(keys.read(&key).expect("keychain").is_none());
    std::fs::remove_dir_all(dir).expect("remove fixture");
}

#[test]
fn a_failed_save_keeps_its_error_and_the_key_ready_for_cleanup() {
    let dir = folder();
    let store = ProviderSettingsStore::in_config(&dir);
    let keys = Keys::default();
    keys.1.store(true, Ordering::Relaxed);
    let result: Result<(), _> =
        store.stage_key(&keys, Secret::new("fake-stage".into()), |_, _| Err(missing_provider()));
    assert_eq!(result, Err(missing_provider()));
    assert_eq!(store.read_file().expect("journal").pending_key_deletes.len(), 1);
    assert_eq!(keys.0.lock().expect("keys").len(), 1);
    keys.1.store(false, Ordering::Relaxed);
    store.retry_key_cleanup(&keys).expect("delete failed stage");
    assert!(keys.0.lock().expect("keys").is_empty());
    std::fs::remove_dir_all(dir).expect("remove fixture");
}

#[test]
fn cleanup_leaves_an_active_stage_alone_and_reclaims_it_after_the_guard_closes() {
    let dir = folder();
    let store = ProviderSettingsStore::in_config(&dir);
    let keys = Keys::default();
    let id = uuid::Uuid::new_v4();
    let key = KeyRef::new(format!("provider-key/{id}"));
    let guard = open_private_lock(&store.key_operation_path(id).expect("lock path")).expect("stage file");
    guard.lock().expect("active writer");
    store
        .edit_file(|settings| {
            settings.pending_key_deletes.push(key.clone());
            Ok(())
        })
        .expect("persist stage before secret write");
    keys.write(&key, Secret::new("fake-stage".into())).expect("staged key");
    store.retry_key_cleanup(&keys).expect("skip active writer");
    assert!(keys.read(&key).expect("credential").is_some());
    assert_eq!(
        store.read_file().expect("journal").pending_key_deletes,
        std::slice::from_ref(&key)
    );

    // Process exit releases the same OS lock as closing the guard here.
    drop(guard);
    store.retry_key_cleanup(&keys).expect("recover unfinished write");
    assert!(keys.read(&key).expect("credential").is_none());
    assert!(store.read_file().expect("journal").pending_key_deletes.is_empty());
    std::fs::remove_dir_all(dir).expect("remove fixture");
}
