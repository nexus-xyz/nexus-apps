use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const OWN: &str = "xyz.nexus.desktop-gpui";
const OLD: &str = "xyz.nexus.desktop";
const GATEWAY: &str = "https://api.nexus.inc";
const DEVICE: &str = "https://api.nexus.inc#e2e-device";

/// A keychain in memory that counts every read and can refuse the old service's reads, as a denied dialog does.
#[derive(Default)]
struct Fake {
    items: Mutex<HashMap<(String, String), String>>,
    reads: AtomicUsize,
    old_reads: AtomicUsize,
    deny_old: std::sync::atomic::AtomicBool,
    #[cfg(target_os = "macos")]
    service_without_keychain: Mutex<Option<&'static str>>,
    fail_writes: std::sync::atomic::AtomicBool,
}

impl Fake {
    fn with(items: &[(&str, &str, &str)]) -> Arc<Fake> {
        let fake = Arc::new(Fake::default());
        for (service, account, secret) in items {
            lock(&fake.items).insert((service.to_string(), account.to_string()), secret.to_string());
        }
        fake
    }

    fn item(&self, service: &str, account: &str) -> Option<String> {
        lock(&self.items)
            .get(&(service.to_string(), account.to_string()))
            .cloned()
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn old_reads(&self) -> usize {
        self.old_reads.load(Ordering::SeqCst)
    }
}

/// The store a keychain is given: a handle on the shared fake, so the test still sees it.
struct Handle(Arc<Fake>);

impl Store for Handle {
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, keyring::Error> {
        self.0.reads.fetch_add(1, Ordering::SeqCst);
        if service == OLD {
            self.0.old_reads.fetch_add(1, Ordering::SeqCst);
            if self.0.deny_old.load(Ordering::SeqCst) {
                return Err(keyring::Error::PlatformFailure(Box::new(std::io::Error::other(
                    "User canceled the operation.",
                ))));
            }
        }
        #[cfg(target_os = "macos")]
        if *lock(&self.0.service_without_keychain) == Some(service) {
            return Err(keyring::Error::PlatformFailure(Box::new(
                security_framework::base::Error::from_code(-25307),
            )));
        }
        Ok(self.0.item(service, account))
    }

    fn write(&self, service: &str, account: &str, secret: &str) -> Result<(), String> {
        if self.0.fail_writes.load(Ordering::SeqCst) {
            return Err("The keychain is locked.".into());
        }
        lock(&self.0.items).insert((service.to_string(), account.to_string()), secret.to_string());
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        lock(&self.0.items).remove(&(service.to_string(), account.to_string()));
        Ok(())
    }
}

/// A record file in a folder of its own, removed when the test ends.
struct Record(PathBuf);

impl Record {
    fn new(name: &str) -> Record {
        let dir = std::env::temp_dir().join(format!("nexus-own-keychain-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Record(dir.join("keychain-moves.json"))
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        if let Some(dir) = self.0.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// One launch of the app on a Mac: a fresh keychain object over the same store and record.
fn launch(fake: &Arc<Fake>, record: &Record) -> OwnKeychain {
    OwnKeychain::new(Box::new(Handle(fake.clone())), OWN, Some(OLD), Some(record.0.clone()))
}

#[test]
fn a_launch_reads_the_token_once_however_many_callers_ask() {
    let fake = Fake::with(&[(OWN, GATEWAY, "nxc_own")]);
    let record = Record::new("once");
    let keychain = launch(&fake, &record);
    for _ in 0..20 {
        assert_eq!(keychain.read(GATEWAY), Ok(Some("nxc_own".into())));
    }
    assert_eq!(fake.reads(), 1);
    assert_eq!(fake.old_reads(), 0, "an own item never sends Nexus to the old service");
}

#[test]
fn callers_on_many_threads_still_read_once() {
    let fake = Fake::with(&[(OLD, GATEWAY, "nxc_old")]);
    let record = Record::new("threads");
    let keychain = Arc::new(launch(&fake, &record));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let keychain = keychain.clone();
            std::thread::spawn(move || keychain.read(GATEWAY))
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().expect("a reader"), Ok(Some("nxc_old".into())));
    }
    assert_eq!(fake.old_reads(), 1, "one dialog, not one per caller");
}

#[test]
fn an_old_item_is_asked_for_once_and_then_read_from_the_own_service() {
    let fake = Fake::with(&[(OLD, GATEWAY, "nxc_old"), (OLD, DEVICE, "device-secret")]);
    let record = Record::new("copy");
    let first = launch(&fake, &record);
    assert_eq!(first.read(GATEWAY), Ok(Some("nxc_old".into())));
    assert_eq!(first.read(DEVICE), Ok(Some("device-secret".into())));
    assert_eq!(fake.old_reads(), 2, "one dialog per old item");
    assert_eq!(fake.item(OWN, GATEWAY).as_deref(), Some("nxc_old"));
    assert_eq!(fake.item(OWN, DEVICE).as_deref(), Some("device-secret"));
    assert_eq!(
        fake.item(OLD, GATEWAY).as_deref(),
        Some("nxc_old"),
        "the old item stays"
    );

    for _ in 0..3 {
        let next = launch(&fake, &record);
        assert_eq!(next.read(GATEWAY), Ok(Some("nxc_old".into())));
        assert_eq!(next.read(DEVICE), Ok(Some("device-secret".into())));
    }
    assert_eq!(fake.old_reads(), 2, "later launches never ask again");
}

#[test]
fn a_person_who_never_had_the_old_app_is_never_asked() {
    let fake = Fake::with(&[]);
    let record = Record::new("fresh");
    let keychain = launch(&fake, &record);
    assert_eq!(keychain.read(GATEWAY), Ok(None));
    assert_eq!(keychain.read(GATEWAY), Ok(None));
    assert_eq!(fake.old_reads(), 1, "a missing old item shows no dialog on macOS");
    keychain.write(GATEWAY, "nxc_new").expect("saved");
    assert_eq!(keychain.read(GATEWAY), Ok(Some("nxc_new".into())));

    let next = launch(&fake, &record);
    assert_eq!(next.read(GATEWAY), Ok(Some("nxc_new".into())));
    assert_eq!(fake.old_reads(), 1);
}

#[cfg(target_os = "macos")]
#[test]
fn a_missing_keychain_does_not_record_a_denial() {
    for service in [OWN, OLD] {
        let fake = Fake::with(&[(OLD, GATEWAY, "nxc_tauri")]);
        *lock(&fake.service_without_keychain) = Some(service);
        let record = Record::new(service);
        let keychain = launch(&fake, &record);

        assert_eq!(keychain.read(GATEWAY), Err(Unread::NoKeychain));
        assert_eq!(keychain.held(GATEWAY), Some(Err(Unread::NoKeychain)));
        assert!(!record.0.exists());
        assert_eq!(keychain.read(DEVICE), Err(Unread::NoKeychain));

        *lock(&fake.service_without_keychain) = None;
        assert_eq!(launch(&fake, &record).read(GATEWAY), Ok(Some("nxc_tauri".into())));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_distinguishes_a_missing_keychain_from_denial_and_read_errors() {
    use security_framework::base::Error;

    for status in [-25291, -25294, -25307] {
        assert!(keychain_is_missing(&keyring::Error::NoStorageAccess(Box::new(
            Error::from_code(status),
        ))));
    }
    for status in [-128, -25293, -25295, -25308] {
        assert!(!keychain_is_missing(&keyring::Error::PlatformFailure(Box::new(
            Error::from_code(status),
        ))));
    }
    assert!(!keychain_is_missing(&keyring::Error::NoEntry));
}

#[test]
fn a_denied_dialog_is_not_shown_again_in_that_run_or_later() {
    let fake = Fake::with(&[(OLD, GATEWAY, "nxc_old")]);
    fake.deny_old.store(true, Ordering::SeqCst);
    let record = Record::new("denied");
    let keychain = launch(&fake, &record);
    for _ in 0..10 {
        assert_eq!(keychain.read(GATEWAY), Err(Unread::Denied));
    }
    assert_eq!(fake.old_reads(), 1);
    assert_eq!(keychain.held(GATEWAY), Some(Err(Unread::Denied)));

    fake.deny_old.store(false, Ordering::SeqCst);
    let next = launch(&fake, &record);
    assert_eq!(
        next.read(GATEWAY),
        Err(Unread::Denied),
        "still an error, never an empty sign-in"
    );
    assert_eq!(fake.old_reads(), 1, "the next launch does not ask either");

    // Signing in again settles it.
    next.write(GATEWAY, "nxc_new").expect("saved");
    let after = launch(&fake, &record);
    assert_eq!(after.read(GATEWAY), Ok(Some("nxc_new".into())));
    assert_eq!(fake.old_reads(), 1);
}

#[test]
fn a_denied_device_key_stays_an_error_until_the_person_asks_again() {
    let fake = Fake::with(&[(OLD, DEVICE, "device-secret")]);
    fake.deny_old.store(true, Ordering::SeqCst);
    let record = Record::new("device");
    let keychain = launch(&fake, &record);
    assert_eq!(keychain.read(DEVICE), Err(Unread::Denied));
    assert_eq!(launch(&fake, &record).read(DEVICE), Err(Unread::Denied));
    assert_eq!(fake.old_reads(), 1);

    // Settings › Devices' Try again: one more dialog, which the person allows this time.
    fake.deny_old.store(false, Ordering::SeqCst);
    keychain.retry(DEVICE);
    assert_eq!(keychain.read(DEVICE), Ok(Some("device-secret".into())));
    assert_eq!(fake.old_reads(), 2);
    assert_eq!(fake.item(OWN, DEVICE).as_deref(), Some("device-secret"));
}

#[test]
fn a_signed_out_account_does_not_come_back_from_the_old_item() {
    let fake = Fake::with(&[(OLD, GATEWAY, "nxc_old")]);
    let record = Record::new("signed-out");
    let keychain = launch(&fake, &record);
    assert_eq!(keychain.read(GATEWAY), Ok(Some("nxc_old".into())));
    keychain.delete(GATEWAY).expect("signed out");
    assert_eq!(keychain.read(GATEWAY), Ok(None));
    assert_eq!(launch(&fake, &record).read(GATEWAY), Ok(None));
    assert_eq!(fake.old_reads(), 1);
}

#[test]
fn a_failed_write_is_reported_and_holds_nothing_the_store_lacks() {
    let fake = Fake::with(&[(OWN, DEVICE, "device-secret")]);
    let record = Record::new("write");
    let keychain = launch(&fake, &record);
    assert_eq!(keychain.read(DEVICE), Ok(Some("device-secret".into())));
    fake.fail_writes.store(true, Ordering::SeqCst);
    assert!(keychain.write(DEVICE, "another-secret").is_err());
    assert_eq!(
        keychain.read(DEVICE),
        Ok(Some("device-secret".into())),
        "the run keeps the key the next launch will read"
    );
    assert!(keychain.write(GATEWAY, "nxc_new").is_err());
    assert_eq!(keychain.read(GATEWAY), Ok(None));
}

#[test]
fn a_copy_that_cannot_be_saved_is_asked_for_again_rather_than_lost() {
    let fake = Fake::with(&[(OLD, DEVICE, "device-secret")]);
    fake.fail_writes.store(true, Ordering::SeqCst);
    let record = Record::new("copy-fails");
    assert_eq!(launch(&fake, &record).read(DEVICE), Ok(Some("device-secret".into())));
    fake.fail_writes.store(false, Ordering::SeqCst);
    assert_eq!(launch(&fake, &record).read(DEVICE), Ok(Some("device-secret".into())));
    assert_eq!(fake.old_reads(), 2);
    assert_eq!(launch(&fake, &record).read(DEVICE), Ok(Some("device-secret".into())));
    assert_eq!(fake.old_reads(), 2);
}

#[test]
fn peek_never_reads_the_old_service() {
    let fake = Fake::with(&[(OLD, GATEWAY, "nxc_old"), (OWN, "https://other.test", "nxc_other")]);
    let record = Record::new("peek");
    let keychain = launch(&fake, &record);
    assert_eq!(keychain.peek(GATEWAY), None);
    assert_eq!(keychain.peek("https://other.test").as_deref(), Some("nxc_other"));
    assert_eq!(keychain.peek("https://other.test").as_deref(), Some("nxc_other"));
    assert_eq!(fake.old_reads(), 0);
    assert_eq!(fake.reads(), 2);
    // A later real read still copies the old item.
    assert_eq!(keychain.read(GATEWAY), Ok(Some("nxc_old".into())));
}

#[test]
fn the_notification_device_token_is_made_new_without_asking_for_the_old_one() {
    const NOTIFY: &str = "https://api.nexus.inc#notify-device";
    let fake = Fake::with(&[(OLD, NOTIFY, "old-token")]);
    let record = Record::new("notify");
    let vault = crate::secrets::Vault::Own(Arc::new(launch(&fake, &record)));
    let token = crate::path_notices::device_token(&vault, GATEWAY).expect("a token");
    assert_ne!(token, "old-token");
    assert_eq!(fake.item(OWN, NOTIFY), Some(token.clone()));
    // The next launch reads the own token, and no launch ever reads the old one.
    let vault = crate::secrets::Vault::Own(Arc::new(launch(&fake, &record)));
    assert_eq!(crate::path_notices::device_token(&vault, GATEWAY), Some(token));
    assert_eq!(fake.old_reads(), 0, "no keychain dialog");
    assert_eq!(
        fake.item(OLD, NOTIFY).as_deref(),
        Some("old-token"),
        "the old item stays"
    );
}

#[test]
fn without_an_old_service_it_is_the_same_store_held() {
    // Linux and Windows: the old app's service is the own one, and nothing is copied or recorded.
    let fake = Fake::with(&[(OLD, GATEWAY, "nxc_old")]);
    let keychain = OwnKeychain::new(Box::new(Handle(fake.clone())), OLD, None, None);
    for _ in 0..5 {
        assert_eq!(keychain.read(GATEWAY), Ok(Some("nxc_old".into())));
    }
    assert_eq!(fake.reads(), 1);
    keychain.write(GATEWAY, "nxc_new").expect("saved");
    assert_eq!(fake.item(OLD, GATEWAY).as_deref(), Some("nxc_new"));
}
