//! Nexus GPUI's keychain: each item read at most once per launch and held in memory, under the app's own
//! service, with a one-time copy of each item the previous desktop app saved.
//!
//! macOS lets an app read a keychain item without asking only when that app made the item. GPUI is not the previous
//! app (`xyz.nexus.desktop-gpui`, not `xyz.nexus.desktop`), so each read of an old item asked for the person's
//! password. GPUI also read the same items again and again, so the dialog came back many times. Here:
//!
//! - **Once per launch.** The first read of an account goes to the store. Later reads get the held answer, and writes
//!   and deletes change it. A failed read is held too, so a denied dialog is not shown again in that run.
//! - **Own items.** With an old service (macOS), an account GPUI has no item for is read once from the old service: one
//!   dialog. The secret is copied under the own service, which GPUI made, so later launches never ask. The old item
//!   stays where it is.
//! - **Once ever.** A small record file lists the accounts already tried on the old service, and how it went. A tried
//!   account is never read there again. A denied one reads as [`Unread::Denied`], never as "no item", so a device key
//!   is never replaced in silence. [`OwnKeychain::retry`] asks once more, when the person asks for it.
//!
//! Linux and Windows have no per-app rule, so GPUI passes no old service there: it keeps the previous app's items,
//! held.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use serde::{Deserialize, Serialize};

/// The OS store, behind a trait so a test can count the reads and deny them.
pub trait Store: Send + Sync {
    /// The secret, or `Ok(None)` when there is no item. Preserve the platform error so callers can tell why a read failed.
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, keyring::Error>;
    fn write(&self, service: &str, account: &str, secret: &str) -> Result<(), String>;
    /// Delete the item. An item that is not there is fine.
    fn delete(&self, service: &str, account: &str) -> Result<(), String>;
}

/// The OS credential store, through `keyring`.
pub struct OsStore;

impl Store for OsStore {
    fn read(&self, service: &str, account: &str) -> Result<Option<String>, keyring::Error> {
        match keyring::Entry::new(service, account).and_then(|entry| entry.get_password()) {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn write(&self, service: &str, account: &str, secret: &str) -> Result<(), String> {
        keyring::Entry::new(service, account)
            .and_then(|entry| entry.set_password(secret))
            .map_err(|error| error.to_string())
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), String> {
        match keyring::Entry::new(service, account).and_then(|entry| entry.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[cfg(target_os = "macos")]
fn keychain_is_missing(error: &keyring::Error) -> bool {
    use std::error::Error as _;

    let root_error = match error {
        keyring::Error::NoDefaultStore => match keyring::Entry::store_status() {
            Err(cause) => cause,
            Ok(()) => return false,
        },
        other => other,
    };
    root_error
        .source()
        .and_then(|source| source.downcast_ref::<security_framework::base::Error>())
        .is_some_and(|status| {
            // Security framework status codes: unavailable, no such keychain, no default keychain.
            matches!(status.code(), -25291 | -25294 | -25307)
        })
}

#[cfg(not(target_os = "macos"))]
fn keychain_is_missing(_: &keyring::Error) -> bool {
    false
}

/// Why an account has no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unread {
    /// macOS has no keychain available. The item could not be looked up.
    NoKeychain,
    /// The old service's item could not be read: on macOS, the person denied the dialog. Nexus does not ask again.
    Denied,
    /// The own item could not be read.
    Failed(String),
}

impl std::fmt::Display for Unread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unread::NoKeychain => f.write_str("No system keychain is available."),
            Unread::Denied => f.write_str("The keychain did not let Nexus read what the old Nexus app saved."),
            Unread::Failed(error) => write!(f, "The keychain did not answer: {error}"),
        }
    }
}

pub type Answer = Result<Option<String>, Unread>;

/// What happened to an account on the old service. Kept in the record file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Tried {
    /// Read and copied under the own service.
    Copied,
    /// The old service had no item.
    Absent,
    /// The read failed: the person denied it.
    Denied,
    /// Nexus wrote or deleted the account itself first: the old item is out of date.
    Replaced,
}

type Slot = Arc<Mutex<Option<Answer>>>;

pub struct OwnKeychain {
    store: Box<dyn Store>,
    service: String,
    old: Option<String>,
    /// The record of tried accounts (JSON, no secrets). None: nothing is kept between launches.
    record: Option<PathBuf>,
    /// The record, read on first use.
    tried: OnceLock<Mutex<BTreeMap<String, Tried>>>,
    /// One slot per account. Its lock is held for the first read, so two callers never ask the store twice.
    held: Mutex<HashMap<String, Slot>>,
}

impl std::fmt::Debug for OwnKeychain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnKeychain")
            .field("service", &self.service)
            .field("old", &self.old)
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl OwnKeychain {
    /// Items under `service`. With `old`, an account missing there is copied once from `old`, and `record` keeps which
    /// accounts were tried.
    pub fn new(store: Box<dyn Store>, service: &str, old: Option<&str>, record: Option<PathBuf>) -> Self {
        Self {
            store,
            service: service.to_string(),
            old: old.filter(|old| *old != service).map(str::to_string),
            record,
            tried: OnceLock::new(),
            held: Mutex::new(HashMap::new()),
        }
    }

    fn slot(&self, account: &str) -> Slot {
        lock(&self.held).entry(account.to_string()).or_default().clone()
    }

    /// The account's secret: the held answer, else one read (and at most one read of the old service, ever).
    pub fn read(&self, account: &str) -> Answer {
        let slot = self.slot(account);
        let mut slot = lock(&slot);
        if let Some(answer) = slot.as_ref() {
            return answer.clone();
        }
        let answer = self.first_read(account);
        *slot = Some(answer.clone());
        answer
    }

    /// The held answer only, without waiting and without a read: for a screen that must not block on the keychain.
    pub fn held(&self, account: &str) -> Option<Answer> {
        let slot = lock(&self.held).get(account).cloned()?;
        slot.try_lock().ok()?.clone()
    }

    /// The secret if GPUI has it already, never from the old service: a read that can never show a dialog. For a list
    /// of sign-ins (Settings › Gateways), where asking for each listed Gateway would be wrong.
    pub fn peek(&self, account: &str) -> Option<String> {
        let slot = self.slot(account);
        let mut slot = lock(&slot);
        if let Some(answer) = slot.as_ref() {
            return answer.clone().ok().flatten();
        }
        let own = self.store.read(&self.service, account).ok().flatten()?;
        *slot = Some(Ok(Some(own.clone())));
        Some(own)
    }

    /// Save `secret`. A failed write changes nothing, not even the held answer: a held device key the store does not
    /// have would work for one run and be gone at the next launch, orphaning the device it paired.
    pub fn write(&self, account: &str, secret: &str) -> Result<(), String> {
        let slot = self.slot(account);
        let mut slot = lock(&slot);
        self.store.write(&self.service, account, secret)?;
        *slot = Some(Ok(Some(secret.to_string())));
        self.mark(account, Tried::Replaced);
        Ok(())
    }

    /// Delete the account. The old item stays, and is never read again.
    pub fn delete(&self, account: &str) -> Result<(), String> {
        let slot = self.slot(account);
        let mut slot = lock(&slot);
        *slot = Some(Ok(None));
        self.mark(account, Tried::Replaced);
        self.store.delete(&self.service, account)
    }

    /// The person asked to try again: a denied account is read from the old service once more, at the next read.
    pub fn retry(&self, account: &str) {
        let slot = self.slot(account);
        let mut slot = lock(&slot);
        if matches!(slot.as_ref(), Some(Err(_))) {
            *slot = None;
        }
        let mut tried = self.tried();
        if tried.get(account) == Some(&Tried::Denied) {
            tried.remove(account);
            self.save(&tried);
        }
    }

    fn first_read(&self, account: &str) -> Answer {
        match self.store.read(&self.service, account) {
            Ok(None) => {}
            Ok(Some(secret)) => return Ok(Some(secret)),
            Err(error) if keychain_is_missing(&error) => return Err(Unread::NoKeychain),
            Err(error) => {
                eprintln!(
                    "nexus-desktop: keychain: could not read {account} under {}: {error}",
                    self.service
                );
                return Err(Unread::Failed(error.to_string()));
            }
        }
        let Some(old) = &self.old else {
            return Ok(None);
        };
        match self.tried().get(account) {
            Some(Tried::Denied) => return Err(Unread::Denied),
            Some(_) => return Ok(None),
            None => {}
        }
        match self.store.read(old, account) {
            Ok(None) => {
                self.mark(account, Tried::Absent);
                Ok(None)
            }
            Ok(Some(secret)) => {
                match self.store.write(&self.service, account, &secret) {
                    Ok(()) => self.mark(account, Tried::Copied),
                    // Not recorded: the next launch asks once more rather than lose it.
                    Err(error) => eprintln!(
                        "nexus-desktop: keychain: could not copy {account} to {}, so the next start asks again: {error}",
                        self.service
                    ),
                }
                Ok(Some(secret))
            }
            Err(error) if keychain_is_missing(&error) => Err(Unread::NoKeychain),
            Err(error) => {
                eprintln!(
                    "nexus-desktop: keychain: the old app's {account} was not read, and Nexus will not ask again: {error}"
                );
                self.mark(account, Tried::Denied);
                Err(Unread::Denied)
            }
        }
    }

    /// The record, read from its file on first use. A file that is missing or not valid reads as empty.
    fn tried(&self) -> MutexGuard<'_, BTreeMap<String, Tried>> {
        lock(self.tried.get_or_init(|| {
            Mutex::new(
                self.record
                    .as_ref()
                    .and_then(|path| std::fs::read_to_string(path).ok())
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or_default(),
            )
        }))
    }

    fn mark(&self, account: &str, state: Tried) {
        if self.old.is_none() {
            return;
        }
        let mut tried = self.tried();
        if tried.get(account) == Some(&state) {
            return;
        }
        tried.insert(account.to_string(), state);
        self.save(&tried);
    }

    /// Write the record whole, through a temporary file, so a crash never leaves half of it.
    fn save(&self, tried: &BTreeMap<String, Tried>) {
        let Some(path) = &self.record else {
            return;
        };
        let saved = (|| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            crate::atomic_file::write_atomic(path, &serde_json::to_vec_pretty(tried).unwrap_or_default())
        })();
        if let Err(error) = saved {
            eprintln!("nexus-desktop: keychain: could not save {}: {error}", path.display());
        }
    }
}

#[cfg(test)]
#[path = "own_keychain_tests.rs"]
mod tests;
