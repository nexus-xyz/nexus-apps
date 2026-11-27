//! The Assistant's vault on this device: the Nexus vault's logins in the OS keychain, the
//! grants and last uses of every login on this device, the device's activity lines, and the index the device sends to
//! the agent API (`PUT /agent/vault/index`).
//!
//! The rule (threat model):
//! - A password lives in the keychain item and in a [`Secret`], and nowhere else. A [`Secret`] never prints, never
//!   serializes except into the keychain item, and is wiped when dropped.
//! - Everything that leaves this module toward a screen or a network is a [`LoginMeta`]. It has no password field, so
//!   no code path can copy one into the index, a log line or a view.
//! - The index has a name, a site, a masked hint of four characters at most, a source, a grant and a last use. Before
//!   it goes out, [`check_index_body`] applies the kit's `holdsSecret` rules and the index's own shape. A body it
//!   refuses is never sent.
//! - A secret is read only through [`LocalVault::secret`], which needs an [`Authorized`], which only a Touch ID
//!   answer of "approved" makes.
//! - Nothing in this module writes to a log. A test reads the source to keep it so.
//!
//! The keychain item is one JSON blob under the service [`SERVICE`]: the vault's logins, each login's grant, and the
//! activity lines. Only the keychain's own access control guards it: the OS asks the person nothing for an app that
//! made the item, and asks for another app.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use zeroize::Zeroize;

use crate::agent_vault::{self as words, Flag, Grant, ManagerKind, holds_secret};
use crate::own_keychain::Store;

/// The keychain service Nexus GPUI keeps the vault under, apart from the sign-in's.
pub const SERVICE: &str = "xyz.nexus.desktop-gpui.vault";
/// The vault's one keychain item.
pub const ACCOUNT: &str = "logins-v1";
/// The most logins one index holds (the kit's `VAULT_INDEX_MAX_LOGINS`).
pub const MAX_LOGINS: usize = 2000;
/// The most activity lines one index holds (the kit's `VAULT_INDEX_MAX_ACTIVITY`).
pub const MAX_ACTIVITY: usize = 200;
/// The longest masked hint, in characters.
pub const HINT_MAX: usize = 4;

/// One writer at a time, for every `LocalVault` in the process. Each change reads the keychain's one blob, edits it and
/// writes it back, and the page runs jobs on several threads: without this, two changes read the same blob and the
/// later write drops the earlier one.
static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn one_writer() -> std::sync::MutexGuard<'static, ()> {
    WRITER.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A password, in memory only while it is needed. It prints as `Secret(…)`, and is wiped on drop.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }

    /// The password, for the one place that shows or uses it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// How Touch ID answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    Approved,
    /// The person cancelled, or failed it.
    Declined,
    /// This device cannot ask (no Touch ID and no password, or not macOS), and why.
    Unavailable(String),
}

/// Whoever asks the person to prove it is them: Touch ID on macOS, a fake in tests.
pub trait Authenticator: Send + Sync {
    /// Ask, with the reason the person reads in the system sheet. Blocks until the person answers.
    fn ask(&self, reason: &str) -> Auth;
}

/// Proof that the person just approved with Touch ID. Only [`Authorized::from`] makes one, and only from "approved".
#[derive(Debug)]
pub struct Authorized(());

impl Authorized {
    pub fn from(auth: &Auth) -> Option<Authorized> {
        matches!(auth, Auth::Approved).then_some(Authorized(()))
    }
}

/// What can go wrong with the vault. No variant carries a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    /// The keychain refused or failed.
    Keychain(String),
    /// The keychain item is not a vault this version reads.
    Unreadable,
    /// No such login.
    NotFound,
    /// The login lacks a name, a site or a password.
    Incomplete(&'static str),
    /// The vault holds [`MAX_LOGINS`] already.
    Full,
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultError::Keychain(error) => write!(f, "The keychain did not answer: {error}"),
            VaultError::Unreadable => f.write_str("The Nexus vault in the keychain is not one this version reads."),
            VaultError::NotFound => f.write_str("There is no such login."),
            VaultError::Incomplete(what) => write!(f, "A login needs {what}."),
            VaultError::Full => write!(f, "The Nexus vault holds {MAX_LOGINS} logins at most."),
        }
    }
}

/// One login as the keychain holds it.
#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    id: String,
    name: String,
    site: String,
    username: String,
    password: Secret,
}

impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stored")
            .field("id", &self.id)
            .field("password", &self.password)
            .finish()
    }
}

/// What one login lets Nexus do, and when it last did. For logins of every source, keyed by login id.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct State {
    /// `each_use`, or an ISO 8601 date the grant runs until. None: Nexus may not use it.
    grant: Option<String>,
    last_used: Option<String>,
}

/// One line of the device's own activity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Line {
    pub at: String,
    pub what: String,
    pub with: Option<String>,
    pub approved: Option<String>,
    pub result: String,
    pub outcome: String,
}

/// One 1Password item's metadata, kept so the index does not need `op` (and 1Password's prompt) at each change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Cached {
    id: String,
    name: String,
    site: String,
    hint: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Blob {
    /// The 1Password items' metadata at the last Refresh, and their vault's name.
    #[serde(default)]
    op: Vec<Cached>,
    #[serde(default)]
    op_vault: Option<String>,
    #[serde(default)]
    v: u32,
    #[serde(default)]
    logins: Vec<Stored>,
    #[serde(default)]
    state: BTreeMap<String, State>,
    #[serde(default)]
    activity: Vec<Line>,
}

/// A login without its password: all a screen or the index may hold.
#[derive(Debug, Clone, PartialEq)]
pub struct LoginMeta {
    pub id: String,
    pub name: String,
    pub site: String,
    /// The username, for the device's own screen. It never goes to the agent API: only [`hint`] does.
    pub username: String,
    /// The masked username: its first character and dots, four characters at most.
    pub hint: String,
    pub source: ManagerKind,
    pub grant: Option<Grant>,
    pub last_used: Option<String>,
    /// Weak or reused, noted quietly. The Nexus vault only: 1Password has its own watchtower.
    pub flag: Option<Flag>,
}

impl LoginMeta {
    pub fn granted(&self) -> bool {
        self.grant.is_some()
    }
}

/// A login to add.
#[derive(Clone)]
pub struct NewLogin {
    pub name: String,
    pub site: String,
    pub username: String,
    pub password: Secret,
}

impl std::fmt::Debug for NewLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewLogin")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// A login's edit: a field left None stays as it is.
#[derive(Clone, Default)]
pub struct Edit {
    pub name: Option<String>,
    pub site: Option<String>,
    pub username: Option<String>,
    pub password: Option<Secret>,
}

/// The masked hint of a username: its first character and dots, four characters at most. "maya@example.com" is "m•••".
pub fn hint(username: &str) -> String {
    let mut chars = username.trim().chars();
    match chars.next() {
        Some(first) if !chars.as_str().is_empty() => {
            let dots = (username.trim().chars().count() - 1).min(HINT_MAX - 1);
            format!("{first}{}", "•".repeat(dots))
        }
        Some(_) => "•".into(),
        None => String::new(),
    }
}

/// A site as the index names it: the host of a URL, without `www.`, lowercased. None when there is none to name.
pub fn site_of(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let rest = input.split_once("://").map_or(input, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    (!host.is_empty() && host.len() <= 253 && !host.contains(char::is_whitespace)).then_some(host)
}

const COMMON: [&str; 12] = [
    "password",
    "123456",
    "12345678",
    "123456789",
    "qwerty",
    "letmein",
    "welcome",
    "iloveyou",
    "admin",
    "abc123",
    "password1",
    "111111",
];

/// Whether a password is weak: short, common, one repeated character, or short with fewer than three kinds of
/// character.
pub fn weak(password: &str) -> bool {
    let count = password.chars().count();
    if count < 8 || COMMON.contains(&password.to_ascii_lowercase().as_str()) {
        return true;
    }
    if password.chars().all(|c| password.starts_with(c)) {
        return true;
    }
    let kinds = [
        password.chars().any(|c| c.is_lowercase()),
        password.chars().any(|c| c.is_uppercase()),
        password.chars().any(|c| c.is_ascii_digit()),
        password.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|kind| **kind)
    .count();
    count < 12 && kinds < 3
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn grant_of(state: &State) -> Option<Grant> {
    match state.grant.as_deref()? {
        "each_use" => Some(Grant::EachUse),
        until => Some(Grant::Until(until.to_string())),
    }
}

/// A new login id: letters only. The kit's `holdsSecret` refuses a run of twelve digits anywhere in the index, ids
/// included, and a hex id holds one now and then.
fn new_id(prefix: &str) -> String {
    let letters: String = uuid::Uuid::new_v4()
        .as_bytes()
        .iter()
        .map(|byte| (b'a' + byte % 26) as char)
        .collect();
    format!("{prefix}{letters}")
}

/// The id a 1Password item has here: `op_` and the item's own id, when it is a valid vault id.
pub fn op_login_id(item: &str) -> Option<String> {
    let id = format!("op_{item}");
    words::vault_id(&id).then_some(id)
}

/// The Nexus vault in the keychain, and the state of every login on this device.
pub struct LocalVault {
    store: Arc<dyn Store>,
}

impl LocalVault {
    pub fn new(store: Arc<dyn Store>) -> LocalVault {
        LocalVault { store }
    }

    /// The vault in the OS keychain.
    pub fn os() -> LocalVault {
        LocalVault::new(Arc::new(crate::own_keychain::OsStore))
    }

    fn read(&self) -> Result<Blob, VaultError> {
        match self.store.read(SERVICE, ACCOUNT) {
            Ok(None) => Ok(Blob::default()),
            Ok(Some(mut text)) => {
                let blob = serde_json::from_str::<Blob>(&text).map_err(|_| VaultError::Unreadable);
                text.zeroize();
                blob
            }
            Err(error) => Err(VaultError::Keychain(error.to_string())),
        }
    }

    fn write(&self, blob: &Blob) -> Result<(), VaultError> {
        let mut text = serde_json::to_string(&Blob {
            v: 1,
            op: blob.op.clone(),
            op_vault: blob.op_vault.clone(),
            logins: blob.logins.clone(),
            state: blob.state.clone(),
            activity: blob.activity.clone(),
        })
        .map_err(|_| VaultError::Unreadable)?;
        let done = self.store.write(SERVICE, ACCOUNT, &text).map_err(VaultError::Keychain);
        text.zeroize();
        done
    }

    fn metas(blob: &Blob) -> Vec<LoginMeta> {
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for login in &blob.logins {
            *seen.entry(login.password.expose()).or_default() += 1;
        }
        blob.logins
            .iter()
            .map(|login| {
                let state = blob.state.get(&login.id).cloned().unwrap_or_default();
                let reused = seen.get(login.password.expose()).copied().unwrap_or(0) > 1;
                LoginMeta {
                    id: login.id.clone(),
                    name: login.name.clone(),
                    site: login.site.clone(),
                    username: login.username.clone(),
                    hint: hint(&login.username),
                    source: ManagerKind::Nexus,
                    grant: grant_of(&state),
                    last_used: state.last_used,
                    flag: if reused {
                        Some(Flag::Reused)
                    } else if weak(login.password.expose()) {
                        Some(Flag::Weak)
                    } else {
                        None
                    },
                }
            })
            .collect()
    }

    /// The Nexus vault's logins, without a password.
    pub fn list(&self) -> Result<Vec<LoginMeta>, VaultError> {
        Ok(Self::metas(&self.read()?))
    }

    fn meta_of(blob: &Blob, id: &str) -> Result<LoginMeta, VaultError> {
        Self::metas(blob)
            .into_iter()
            .find(|login| login.id == id)
            .ok_or(VaultError::NotFound)
    }

    pub fn add(&self, new: NewLogin) -> Result<LoginMeta, VaultError> {
        let _writer = one_writer();
        let mut blob = self.read()?;
        if blob.logins.len() >= MAX_LOGINS {
            return Err(VaultError::Full);
        }
        let id = Self::push(&mut blob, new)?;
        self.write(&blob)?;
        Self::meta_of(&blob, &id)
    }

    fn push(blob: &mut Blob, new: NewLogin) -> Result<String, VaultError> {
        let site = site_of(&new.site).ok_or(VaultError::Incomplete("a site"))?;
        let name = new.name.trim().to_string();
        if name.is_empty() || name.encode_utf16().count() > 120 {
            return Err(VaultError::Incomplete("a name of 120 characters at most"));
        }
        if new.password.is_empty() {
            return Err(VaultError::Incomplete("a password"));
        }
        let id = new_id("nx_");
        blob.logins.push(Stored {
            id: id.clone(),
            name,
            site,
            username: new.username.trim().to_string(),
            password: new.password.clone(),
        });
        Ok(id)
    }

    pub fn edit(&self, id: &str, edit: Edit) -> Result<LoginMeta, VaultError> {
        let _writer = one_writer();
        let mut blob = self.read()?;
        let login = blob
            .logins
            .iter_mut()
            .find(|login| login.id == id)
            .ok_or(VaultError::NotFound)?;
        if let Some(name) = edit.name {
            let name = name.trim().to_string();
            if name.is_empty() || name.encode_utf16().count() > 120 {
                return Err(VaultError::Incomplete("a name of 120 characters at most"));
            }
            login.name = name;
        }
        if let Some(site) = edit.site {
            login.site = site_of(&site).ok_or(VaultError::Incomplete("a site"))?;
        }
        if let Some(username) = edit.username {
            login.username = username.trim().to_string();
        }
        if let Some(password) = edit.password {
            if password.is_empty() {
                return Err(VaultError::Incomplete("a password"));
            }
            login.password = password;
        }
        self.write(&blob)?;
        Self::meta_of(&blob, id)
    }

    pub fn delete(&self, id: &str) -> Result<(), VaultError> {
        let _writer = one_writer();
        let mut blob = self.read()?;
        let before = blob.logins.len();
        blob.logins.retain(|login| login.id != id);
        if blob.logins.len() == before {
            return Err(VaultError::NotFound);
        }
        blob.state.remove(id);
        self.write(&blob)
    }

    /// Delete every Nexus login, and the keychain item itself. Grants of other sources and the activity go too: after
    /// Delete all the device holds nothing.
    pub fn delete_all(&self) -> Result<(), VaultError> {
        let _writer = one_writer();
        self.store.delete(SERVICE, ACCOUNT).map_err(VaultError::Keychain)
    }

    /// Add the imported logins that the vault does not hold yet (same site and username). How many were added, and how
    /// many were already there or past the vault's size.
    pub fn import(&self, logins: Vec<NewLogin>) -> Result<ImportReport, VaultError> {
        let _writer = one_writer();
        let mut blob = self.read()?;
        let mut have: HashSet<(String, String)> = blob
            .logins
            .iter()
            .map(|login| (login.site.clone(), login.username.clone()))
            .collect();
        let mut report = ImportReport::default();
        for new in logins {
            let key = (site_of(&new.site).unwrap_or_default(), new.username.trim().to_string());
            if have.contains(&key) {
                report.duplicates += 1;
            } else if blob.logins.len() >= MAX_LOGINS {
                report.over += 1;
            } else if Self::push(&mut blob, new).is_ok() {
                have.insert(key);
                report.added += 1;
            } else {
                report.skipped += 1;
            }
        }
        if report.added > 0 {
            self.write(&blob)?;
        }
        Ok(report)
    }

    /// Set what Nexus may do with a login of any source. None: it may not.
    pub fn set_grant(&self, id: &str, grant: Option<&Grant>) -> Result<(), VaultError> {
        let _writer = one_writer();
        let mut blob = self.read()?;
        let state = blob.state.entry(id.to_string()).or_default();
        state.grant = grant.map(|grant| match grant {
            Grant::EachUse => "each_use".to_string(),
            Grant::Until(until) => until.clone(),
        });
        self.write(&blob)
    }

    /// The grant and last use of one login of any source.
    pub fn state_of(&self, id: &str) -> Result<(Option<Grant>, Option<String>), VaultError> {
        let blob = self.read()?;
        let state = blob.state.get(id).cloned().unwrap_or_default();
        Ok((grant_of(&state), state.last_used))
    }

    /// Note a use now.
    pub fn used(&self, id: &str) -> Result<(), VaultError> {
        let _writer = one_writer();
        let mut blob = self.read()?;
        blob.state.entry(id.to_string()).or_default().last_used = Some(now_iso());
        self.write(&blob)
    }

    /// The password of a Nexus login. It needs the person's Touch ID.
    pub fn secret(&self, _: &Authorized, id: &str) -> Result<Secret, VaultError> {
        let blob = self.read()?;
        blob.logins
            .iter()
            .find(|login| login.id == id)
            .map(|login| login.password.clone())
            .ok_or(VaultError::NotFound)
    }

    /// Add a line to the device's activity, newest first, at most [`MAX_ACTIVITY`].
    pub fn log(&self, line: Line) -> Result<(), VaultError> {
        let _writer = one_writer();
        let mut blob = self.read()?;
        blob.activity.insert(0, line);
        blob.activity.truncate(MAX_ACTIVITY);
        self.write(&blob)
    }

    pub fn activity(&self) -> Result<Vec<Line>, VaultError> {
        Ok(self.read()?.activity)
    }

    /// Whether any Nexus login or state is kept here.
    pub fn is_empty(&self) -> Result<bool, VaultError> {
        let blob = self.read()?;
        Ok(blob.logins.is_empty() && blob.state.is_empty())
    }

    /// Keep the 1Password items' metadata from the last Refresh (an empty list forgets them).
    pub fn set_op_cache(&self, logins: &[LoginMeta], vault: Option<&str>) -> Result<(), VaultError> {
        let _writer = one_writer();
        let mut blob = self.read()?;
        blob.op = logins
            .iter()
            .map(|login| Cached {
                id: login.id.clone(),
                name: login.name.clone(),
                site: login.site.clone(),
                hint: login.hint.clone(),
            })
            .collect();
        blob.op_vault = vault.map(String::from);
        self.write(&blob)
    }

    /// The 1Password items as of the last Refresh, with their grants and last uses, and their vault's name.
    pub fn op_cache(&self) -> Result<(Vec<LoginMeta>, Option<String>), VaultError> {
        let blob = self.read()?;
        let logins = blob
            .op
            .iter()
            .map(|item| {
                let state = blob.state.get(&item.id).cloned().unwrap_or_default();
                LoginMeta {
                    id: item.id.clone(),
                    name: item.name.clone(),
                    site: item.site.clone(),
                    username: String::new(),
                    hint: item.hint.clone(),
                    source: ManagerKind::OnePassword,
                    grant: grant_of(&state),
                    last_used: state.last_used,
                    flag: None,
                }
            })
            .collect();
        Ok((logins, blob.op_vault))
    }

    /// What this device sends: the Nexus vault's logins, and 1Password's when `op_ready` (the CLI is connected).
    pub fn snapshot(&self, device: &str, op_ready: bool) -> Result<Snapshot, VaultError> {
        let blob = self.read()?;
        let mut logins = Self::metas(&blob);
        let mut sources = Vec::new();
        if !logins.is_empty() {
            sources.push(SourceLine {
                kind: ManagerKind::Nexus,
                vault: format!("Nexus vault on {device}").chars().take(120).collect(),
                total: logins.len(),
            });
        }
        if op_ready && !blob.op.is_empty() {
            let (items, vault) = self.op_cache()?;
            let name = vault.map_or("1Password".to_string(), |vault| format!("{vault} vault"));
            sources.push(SourceLine {
                kind: ManagerKind::OnePassword,
                vault: name.chars().take(120).collect(),
                total: items.len(),
            });
            logins.extend(items);
        }
        Ok(Snapshot {
            device: device.chars().take(120).collect(),
            sources,
            logins,
            activity: blob.activity,
        })
    }

    /// Every password's text, for a check that no outgoing text holds one. Wiped when the caller drops them.
    pub fn secrets(&self) -> Result<Vec<Secret>, VaultError> {
        Ok(self.read()?.logins.into_iter().map(|login| login.password).collect())
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    pub duplicates: usize,
    pub skipped: usize,
    pub over: usize,
}

/// A line of the device's activity.
pub fn line(
    what: impl Into<String>,
    with: Option<&str>,
    approved: Option<&str>,
    result: &str,
    outcome: words::Outcome,
) -> Line {
    Line {
        at: now_iso(),
        what: what.into(),
        with: with.map(String::from),
        approved: approved.map(String::from),
        result: result.into(),
        outcome: match outcome {
            words::Outcome::Done => "done",
            words::Outcome::Approved => "approved",
            words::Outcome::Refused => "refused",
        }
        .into(),
    }
}

/// What the index says about each source linked on this device.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceLine {
    pub kind: ManagerKind,
    /// "Nexus vault on Maya’s MacBook Pro", "Personal 1Password": the vault's name, 120 characters at most.
    pub vault: String,
    pub total: usize,
}

/// What this device sends: the logins of every source, without a secret.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub device: String,
    pub sources: Vec<SourceLine>,
    pub logins: Vec<LoginMeta>,
    pub activity: Vec<Line>,
}

/// An index built, and how many logins it left out because their own words were shaped like a secret.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub body: String,
    pub left_out: usize,
}

/// Whether some text holds a secret's shape, or one of `secrets`.
fn unsafe_text(text: &str, secrets: &[Secret]) -> bool {
    holds_secret(&Value::String(text.to_string()))
        || secrets
            .iter()
            .any(|secret| secret.expose().chars().count() >= 4 && text.contains(secret.expose()))
}

/// Build the index body from a snapshot. A login whose name, site or hint is shaped like a secret, or holds one of
/// `secrets`, is left out, so one odd login does not stop the rest. The whole body is then checked
/// ([`check_index_body`]): `Err` means nothing may be sent.
pub fn build_index(snapshot: &Snapshot, secrets: &[Secret]) -> Result<Built, Refusal> {
    let mut left_out = 0;
    let mut logins = Vec::new();
    for login in snapshot.logins.iter().take(MAX_LOGINS) {
        let words = [
            login.name.as_str(),
            login.site.as_str(),
            login.hint.as_str(),
            login.id.as_str(),
        ];
        if words.iter().any(|text| unsafe_text(text, secrets)) || !words::vault_id(&login.id) {
            left_out += 1;
            continue;
        }
        let mut each = Map::new();
        each.insert("id".into(), json!(login.id));
        each.insert("name".into(), json!(login.name));
        each.insert("site".into(), json!(login.site));
        if !login.hint.is_empty() {
            each.insert("account".into(), json!(login.hint));
        }
        each.insert("source".into(), json!(source_word(login.source)));
        each.insert(
            "grant".into(),
            match &login.grant {
                None => Value::Null,
                Some(Grant::EachUse) => json!({ "kind": "each_use" }),
                Some(Grant::Until(until)) => json!({ "kind": "until", "until": until }),
            },
        );
        each.insert(
            "lastUsed".into(),
            login.last_used.as_ref().map_or(Value::Null, |at| json!(at)),
        );
        match login.flag {
            Some(Flag::Weak) => each.insert("flag".into(), json!("weak")),
            Some(Flag::Reused) => each.insert("flag".into(), json!("reused")),
            None => None,
        };
        logins.push(Value::Object(each));
    }
    let sources: Vec<Value> = snapshot
        .sources
        .iter()
        .map(|source| json!({ "kind": source_word(source.kind), "vault": source.vault, "total": source.total }))
        .collect();
    let activity: Vec<Value> = snapshot
        .activity
        .iter()
        .take(MAX_ACTIVITY)
        .filter(|line| ![line.what.as_str(), line.result.as_str()].iter().any(|text| unsafe_text(text, secrets)))
        .map(|line| {
            json!({ "at": line.at, "what": line.what, "with": line.with, "approved": line.approved, "result": line.result, "outcome": line.outcome })
        })
        .collect();
    let body = json!({ "device": snapshot.device, "sources": sources, "logins": logins, "activity": activity });
    let body = body.to_string();
    check_index_body(&body)?;
    Ok(Built { body, left_out })
}

fn source_word(kind: ManagerKind) -> &'static str {
    match kind {
        ManagerKind::OnePassword => "1password",
        ManagerKind::Bitwarden => "bitwarden",
        ManagerKind::Nexus => "nexus",
    }
}

/// Why an index was refused. It never quotes the body: a refused body may hold the very secret it was refused for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not JSON, or not an index.
    NotAnIndex,
    /// A key the index does not know.
    UnknownField,
    /// A secret's shape: a key named like one, or a run of twelve digits.
    HoldsSecret,
    /// Too many logins or lines, or a duplicate id.
    TooMany,
}

fn only(fields: &Map<String, Value>, allowed: &[&str]) -> bool {
    fields.keys().all(|key| allowed.contains(&key.as_str()))
}

/// Whether `body` is exactly an index the agent API takes (the kit's `vaultIndexOf`): known fields only, no secret's
/// shape anywhere, a masked hint of four characters at most, unique ids, and the caps.
pub fn check_index_body(body: &str) -> Result<(), Refusal> {
    let value: Value = serde_json::from_str(body).map_err(|_| Refusal::NotAnIndex)?;
    let fields = value.as_object().ok_or(Refusal::NotAnIndex)?;
    if holds_secret(&value) {
        return Err(Refusal::HoldsSecret);
    }
    if !only(fields, &["device", "sources", "logins", "activity"]) {
        return Err(Refusal::UnknownField);
    }
    words::text(fields.get("device"), 120).ok_or(Refusal::NotAnIndex)?;
    let list = |key: &str, max: usize| -> Result<&Vec<Value>, Refusal> {
        let items = fields.get(key).and_then(Value::as_array).ok_or(Refusal::NotAnIndex)?;
        if items.len() > max {
            Err(Refusal::TooMany)
        } else {
            Ok(items)
        }
    };
    for source in list("sources", 4)? {
        let each = source.as_object().ok_or(Refusal::NotAnIndex)?;
        if !only(each, &["kind", "vault", "total"]) {
            return Err(Refusal::UnknownField);
        }
        let kind = each.get("kind").and_then(Value::as_str).ok_or(Refusal::NotAnIndex)?;
        if !["1password", "bitwarden", "nexus"].contains(&kind) || words::text(each.get("vault"), 120).is_none() {
            return Err(Refusal::NotAnIndex);
        }
        if each.get("total").is_some_and(|total| total.as_u64().is_none()) {
            return Err(Refusal::NotAnIndex);
        }
    }
    let mut ids = HashSet::new();
    for login in list("logins", MAX_LOGINS)? {
        let each = login.as_object().ok_or(Refusal::NotAnIndex)?;
        if !only(
            each,
            &["id", "name", "site", "account", "source", "grant", "lastUsed", "flag"],
        ) {
            return Err(Refusal::UnknownField);
        }
        let id = words::id(each.get("id")).ok_or(Refusal::NotAnIndex)?;
        if !ids.insert(id) {
            return Err(Refusal::TooMany);
        }
        if words::text(each.get("name"), 120).is_none() || words::text(each.get("site"), 253).is_none() {
            return Err(Refusal::NotAnIndex);
        }
        if let Some(account) = each.get("account") {
            let account = words::text(Some(account), 80).ok_or(Refusal::NotAnIndex)?;
            if account.chars().count() > HINT_MAX || account.chars().filter(char::is_ascii_digit).count() > 4 {
                return Err(Refusal::HoldsSecret);
            }
        }
        let source = each.get("source").and_then(Value::as_str).ok_or(Refusal::NotAnIndex)?;
        if !["1password", "bitwarden", "nexus"].contains(&source) {
            return Err(Refusal::NotAnIndex);
        }
        match each.get("grant") {
            Some(Value::Null) => {}
            Some(grant) if grant.get("kind").and_then(Value::as_str) == Some("each_use") => {}
            Some(grant)
                if grant.get("kind").and_then(Value::as_str) == Some("until")
                    && words::time(grant.get("until")).is_some() => {}
            _ => return Err(Refusal::NotAnIndex),
        }
        match each.get("lastUsed") {
            Some(Value::Null) => {}
            Some(at) if words::time(Some(at)).is_some() => {}
            _ => return Err(Refusal::NotAnIndex),
        }
        if each
            .get("flag")
            .is_some_and(|flag| !matches!(flag.as_str(), Some("weak" | "reused")))
        {
            return Err(Refusal::NotAnIndex);
        }
    }
    for line in list("activity", MAX_ACTIVITY)? {
        let each = line.as_object().ok_or(Refusal::NotAnIndex)?;
        if !only(each, &["at", "what", "with", "approved", "result", "outcome"]) {
            return Err(Refusal::UnknownField);
        }
        let outcome = each.get("outcome").and_then(Value::as_str).ok_or(Refusal::NotAnIndex)?;
        if !["done", "approved", "refused"].contains(&outcome)
            || words::time(each.get("at")).is_none()
            || words::text(each.get("what"), 200).is_none()
            || words::text(each.get("result"), 120).is_none()
        {
            return Err(Refusal::NotAnIndex);
        }
    }
    Ok(())
}

/// A grant that the person approved here, with Touch ID: one login, one site, one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedUse {
    pub login: String,
    pub source: ManagerKind,
    pub site: String,
    pub task: String,
}

/// How an approved use ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filled {
    /// Approved, and ready on this device. Nexus has no browser of its own to type the login into yet, so nothing was
    /// typed and no secret was read.
    ReadyOnDevice,
}

/// The seam where a browser filler goes. It runs for an approved use only, after Touch ID. Today it fills nothing:
/// `secret` is never called, so the password stays where it is. A filler later calls `secret()` here, types the
/// result into `use_.site` and drops it.
pub fn fill_login(_use: &ApprovedUse, _secret: &dyn Fn() -> Option<Secret>) -> Filled {
    Filled::ReadyOnDevice
}

/// The ISO date `until` as a `DateTime`, when it is one.
pub fn until_of(grant: &Grant) -> Option<DateTime<Utc>> {
    match grant {
        Grant::Until(until) => DateTime::parse_from_rfc3339(until)
            .ok()
            .map(|at| at.with_timezone(&Utc)),
        Grant::EachUse => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A keychain in memory that remembers every write, to look for a password anywhere but the blob.
    #[derive(Default)]
    pub struct Fake(Mutex<HashMap<(String, String), String>>);

    impl Store for Fake {
        fn read(&self, service: &str, account: &str) -> Result<Option<String>, keyring::Error> {
            Ok(self.0.lock().unwrap().get(&(service.into(), account.into())).cloned())
        }
        fn write(&self, service: &str, account: &str, secret: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert((service.into(), account.into()), secret.into());
            Ok(())
        }
        fn delete(&self, service: &str, account: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(&(service.into(), account.into()));
            Ok(())
        }
    }

    fn vault() -> (LocalVault, Arc<Fake>) {
        let store = Arc::new(Fake::default());
        (LocalVault::new(store.clone()), store)
    }

    fn login(name: &str, site: &str, user: &str, password: &str) -> NewLogin {
        NewLogin {
            name: name.into(),
            site: site.into(),
            username: user.into(),
            password: Secret::new(password),
        }
    }

    fn snapshot(vault: &LocalVault) -> Snapshot {
        let logins = vault.list().unwrap();
        Snapshot {
            device: "Maya’s MacBook Pro".into(),
            sources: vec![SourceLine {
                kind: ManagerKind::Nexus,
                vault: "Nexus vault".into(),
                total: logins.len(),
            }],
            logins,
            activity: vec![line(
                "Approved Delta",
                Some("Nexus vault"),
                Some("Touch ID"),
                "Ready on this device",
                words::Outcome::Approved,
            )],
        }
    }

    #[test]
    fn hint_masks_to_four_characters() {
        assert_eq!(hint("maya@example.com"), "m•••");
        assert_eq!(hint("ab"), "a•");
        assert_eq!(hint("a"), "•");
        assert_eq!(hint(""), "");
        assert!(hint("1234567890123456").chars().count() <= HINT_MAX);
    }

    #[test]
    fn sites_are_hosts() {
        assert_eq!(site_of("https://www.Delta.com/login?x=1").as_deref(), Some("delta.com"));
        assert_eq!(site_of("user:pw@bank.example:8443/x").as_deref(), Some("bank.example"));
        assert_eq!(site_of("  "), None);
    }

    #[test]
    fn weak_and_reused_are_noted_on_this_device() {
        let (vault, _) = vault();
        vault
            .add(login("A", "a.com", "u", "Tr0ub4dor&3-horse-battery"))
            .unwrap();
        vault.add(login("B", "b.com", "u", "abc123")).unwrap();
        vault.add(login("C", "c.com", "u", "Sh4red-Pass-phrase!")).unwrap();
        vault.add(login("D", "d.com", "u", "Sh4red-Pass-phrase!")).unwrap();
        let flags: Vec<_> = vault.list().unwrap().into_iter().map(|login| login.flag).collect();
        assert_eq!(flags, [None, Some(Flag::Weak), Some(Flag::Reused), Some(Flag::Reused)]);
    }

    #[test]
    fn add_edit_delete_and_delete_all() {
        let (vault, store) = vault();
        let added = vault
            .add(login("Delta", "https://www.delta.com", "maya", "pw-one-123456"))
            .unwrap();
        assert_eq!((added.site.as_str(), added.hint.as_str()), ("delta.com", "m•••"));
        assert!(vault.add(login("", "x.com", "", "p")).is_err());
        assert!(vault.add(login("X", "x.com", "", "")).is_err());
        let edited = vault
            .edit(
                &added.id,
                Edit {
                    name: Some("Delta Air".into()),
                    ..Edit::default()
                },
            )
            .unwrap();
        assert_eq!(edited.name, "Delta Air");
        let auth = Authorized::from(&Auth::Approved).unwrap();
        assert_eq!(vault.secret(&auth, &added.id).unwrap().expose(), "pw-one-123456");
        vault
            .edit(
                &added.id,
                Edit {
                    password: Some(Secret::new("pw-two-654321")),
                    ..Edit::default()
                },
            )
            .unwrap();
        assert_eq!(vault.secret(&auth, &added.id).unwrap().expose(), "pw-two-654321");
        vault.delete(&added.id).unwrap();
        assert_eq!(vault.delete(&added.id), Err(VaultError::NotFound));
        vault.add(login("Other", "o.com", "", "pw-three-1234")).unwrap();
        vault.delete_all().unwrap();
        assert!(vault.list().unwrap().is_empty());
        assert!(
            store.0.lock().unwrap().is_empty(),
            "Delete all removes the keychain item itself"
        );
    }

    #[test]
    fn only_an_approval_makes_an_authorization() {
        assert!(Authorized::from(&Auth::Approved).is_some());
        assert!(Authorized::from(&Auth::Declined).is_none());
        assert!(Authorized::from(&Auth::Unavailable("none".into())).is_none());
    }

    #[test]
    fn import_skips_what_the_vault_holds() {
        let (vault, _) = vault();
        vault.add(login("A", "a.com", "u", "pw-aaaa-1111")).unwrap();
        let report = vault
            .import(vec![
                login("A again", "a.com", "u", "pw-x"),
                login("B", "b.com", "u", "pw-bbbb-2222"),
                login("", "", "", ""),
            ])
            .unwrap();
        assert_eq!(
            report,
            ImportReport {
                added: 1,
                duplicates: 1,
                skipped: 1,
                over: 0
            }
        );
    }

    /// Jobs run on several threads: no change may be lost to another that read the same blob.
    #[test]
    fn concurrent_writes_are_not_lost() {
        let (vault, _) = vault();
        let vault = Arc::new(vault);
        let threads: Vec<_> = (0..8)
            .map(|at| {
                let vault = vault.clone();
                std::thread::spawn(move || {
                    for each in 0..5 {
                        vault
                            .add(login(
                                &format!("L{at}-{each}"),
                                "a.com",
                                "u",
                                &format!("pw-{at}-{each}-xxxx"),
                            ))
                            .unwrap();
                        vault
                            .set_grant(&format!("op_{at}_{each}"), Some(&Grant::EachUse))
                            .unwrap();
                        vault
                            .log(line(
                                format!("line {at}-{each}"),
                                None,
                                None,
                                "ok",
                                words::Outcome::Done,
                            ))
                            .unwrap();
                        vault.used(&format!("op_{at}_{each}")).unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(vault.list().unwrap().len(), 40);
        assert_eq!(vault.activity().unwrap().len(), 40);
        for at in 0..8 {
            for each in 0..5 {
                let (grant, used) = vault.state_of(&format!("op_{at}_{each}")).unwrap();
                assert_eq!(grant, Some(Grant::EachUse));
                assert!(used.is_some());
            }
        }
    }

    #[test]
    fn grants_and_last_use_are_kept_for_any_source() {
        let (vault, _) = vault();
        vault.set_grant("op_abc", Some(&Grant::EachUse)).unwrap();
        vault.used("op_abc").unwrap();
        let (grant, used) = vault.state_of("op_abc").unwrap();
        assert_eq!(grant, Some(Grant::EachUse));
        assert!(used.is_some());
        vault.set_grant("op_abc", None).unwrap();
        assert_eq!(vault.state_of("op_abc").unwrap().0, None);
    }

    /// The core of the threat model: whatever passwords the vault holds, the index body holds none of them, nor any
    /// username, and it passes the kit's own checks.
    #[test]
    fn the_index_never_holds_a_password_or_a_username() {
        let (vault, _) = vault();
        let passwords = [
            "Zq9!kLm2#vXw7-unique-one",
            "Hunter2-unique-two-abcdef",
            "4111111111111111",
        ];
        for (at, password) in passwords.iter().enumerate() {
            let added = vault
                .add(login(
                    &format!("Site {at}"),
                    &format!("s{at}.com"),
                    &format!("person{at}@mail.com"),
                    password,
                ))
                .unwrap();
            vault.set_grant(&added.id, Some(&Grant::EachUse)).unwrap();
        }
        let secrets = vault.secrets().unwrap();
        let built = build_index(&snapshot(&vault), &secrets).unwrap();
        for password in passwords {
            assert!(!built.body.contains(password), "a password is in the index");
        }
        assert!(
            !built.body.contains("person") && !built.body.contains("mail.com"),
            "a username is in the index"
        );
        assert!(!built.body.contains("password"));
        assert!(check_index_body(&built.body).is_ok());
        assert!(crate::agent_vault::Request::index(built.body.clone()).is_some());
        assert_eq!(built.left_out, 0);
        // The hint is the only thing of the username.
        assert!(built.body.contains("\"account\":\"p•••\""));
    }

    #[test]
    fn the_snapshot_joins_both_sources_and_drops_1password_when_not_connected() {
        let (vault, _) = vault();
        vault
            .add(login("Delta", "delta.com", "maya", "pw-delta-123456"))
            .unwrap();
        let item = LoginMeta {
            id: "op_abc".into(),
            name: "GitHub".into(),
            site: "github.com".into(),
            username: "maya-secret-user".into(),
            hint: hint("maya-secret-user"),
            source: ManagerKind::OnePassword,
            grant: None,
            last_used: None,
            flag: None,
        };
        vault.set_op_cache(&[item], Some("Personal")).unwrap();
        vault.set_grant("op_abc", Some(&Grant::EachUse)).unwrap();
        let both = vault.snapshot("Mac", true).unwrap();
        assert_eq!(both.sources.len(), 2);
        assert_eq!(both.logins.len(), 2);
        assert_eq!(both.logins[1].grant, Some(Grant::EachUse));
        let built = build_index(&both, &[]).unwrap();
        assert!(!built.body.contains("maya-secret-user") && built.body.contains("\"source\":\"1password\""));
        assert_eq!(vault.snapshot("Mac", false).unwrap().sources.len(), 1);
        vault.delete_all().unwrap();
        assert!(vault.snapshot("Mac", true).unwrap().sources.is_empty());
    }

    #[test]
    fn a_login_whose_words_hold_a_secret_is_left_out_not_sent() {
        let (vault, _) = vault();
        vault.add(login("Fine", "fine.com", "u", "pw-fine-123456")).unwrap();
        // A person typed the card number or the password into the name.
        vault
            .add(login("Visa 4111 1111 1111 1111", "pay.com", "u", "pw-card-123456"))
            .unwrap();
        vault
            .add(login("my pw-leak-123456 login", "leak.com", "u", "pw-leak-123456"))
            .unwrap();
        let secrets = vault.secrets().unwrap();
        let built = build_index(&snapshot(&vault), &secrets).unwrap();
        assert_eq!(built.left_out, 2);
        assert!(built.body.contains("Fine") && !built.body.contains("4111") && !built.body.contains("pw-leak"));
    }

    #[test]
    fn a_body_with_a_secret_shape_is_refused_before_a_request_exists() {
        let ok = json!({ "device": "Mac", "sources": [], "logins": [], "activity": [] });
        assert!(check_index_body(&ok.to_string()).is_ok());
        let keyed = json!({ "device": "Mac", "sources": [], "logins": [{ "id": "a", "name": "A", "site": "a.com", "source": "nexus", "grant": null, "lastUsed": null, "password": "x" }], "activity": [] });
        assert_eq!(check_index_body(&keyed.to_string()), Err(Refusal::HoldsSecret));
        let digits = json!({ "device": "Mac", "sources": [], "logins": [{ "id": "a", "name": "4111-1111-1111-1111", "site": "a.com", "source": "nexus", "grant": null, "lastUsed": null }], "activity": [] });
        assert_eq!(check_index_body(&digits.to_string()), Err(Refusal::HoldsSecret));
        let long_hint = json!({ "device": "Mac", "sources": [], "logins": [{ "id": "a", "name": "A", "site": "a.com", "account": "maya@example.com", "source": "nexus", "grant": null, "lastUsed": null }], "activity": [] });
        assert!(check_index_body(&long_hint.to_string()).is_err());
        let extra = json!({ "device": "Mac", "sources": [], "logins": [], "activity": [], "username": "maya" });
        assert_eq!(check_index_body(&extra.to_string()), Err(Refusal::UnknownField));
        let twice = json!({ "device": "Mac", "sources": [], "activity": [], "logins": [
            { "id": "a", "name": "A", "site": "a.com", "source": "nexus", "grant": null, "lastUsed": null },
            { "id": "a", "name": "B", "site": "b.com", "source": "nexus", "grant": null, "lastUsed": null }] });
        assert_eq!(check_index_body(&twice.to_string()), Err(Refusal::TooMany));
        assert!(crate::agent_vault::Request::index(keyed.to_string()).is_none());
        assert!(crate::agent_vault::Request::index("not json".into()).is_none());
    }

    #[test]
    fn a_secret_never_prints() {
        let (vault, _) = vault();
        let added = vault.add(login("A", "a.com", "u", "Unique-Print-Secret-9")).unwrap();
        let auth = Authorized::from(&Auth::Approved).unwrap();
        let secret = vault.secret(&auth, &added.id).unwrap();
        for printed in [
            format!("{secret:?}"),
            format!("{added:?}"),
            format!("{:?}", login("A", "a.com", "u", "Unique-Print-Secret-9")),
        ] {
            assert!(!printed.contains("Unique-Print-Secret-9"), "{printed}");
        }
        let blob = vault.read().unwrap();
        assert!(!format!("{blob:?}").contains("Unique-Print-Secret-9"));
    }

    #[test]
    fn an_approved_use_is_ready_on_this_device_and_reads_no_secret() {
        let used = ApprovedUse {
            login: "nx_1".into(),
            source: ManagerKind::Nexus,
            site: "delta.com".into(),
            task: "Check in".into(),
        };
        let read = std::cell::Cell::new(false);
        let filled = fill_login(&used, &|| {
            read.set(true);
            None
        });
        assert_eq!(filled, Filled::ReadyOnDevice);
        assert!(!read.get(), "nothing reads a password until a filler exists");
    }

    /// No vault module writes to a log or to the terminal: a secret has nowhere to go but the keychain.
    #[test]
    fn the_vault_modules_never_log() {
        for source in [
            include_str!("vault_local.rs"),
            include_str!("vault_import.rs"),
            include_str!("vault_op.rs"),
        ] {
            let code = source.split("#[cfg(test)]").next().unwrap();
            for banned in [
                "println!",
                "eprintln!",
                "dbg!",
                "tracing::",
                "log::",
                "print!(",
                "eprint!(",
            ] {
                assert!(!code.contains(banned), "a vault module uses {banned}");
            }
        }
    }
}
