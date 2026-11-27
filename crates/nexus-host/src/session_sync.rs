//! Session sync on a server. A server is a trusted device: it
//! gets its own device key, and the workbench syncs its sessions with the `GatewayBackend`, as the desktop app
//! does. Its Gateway sign-in (`nexus-host login`) bills and meters; its device key decrypts. Neither replaces the other.
//!
//! - The key is `device-key` in the host's folder, beside `token`, readable by the host's user only (0600).
//! - `nexus-host pair` registers it and prints its 16-character code. A trusted device approves it by that
//!   code in Settings › Devices. A server has no screen, so it never becomes the first device: it could approve no one.
//! - Then `pair` prints the approver's name and code, and the person confirms that the approver shows that code.
//!   Gateway names the approver, so the server locks no content key until then.
//! - Under "The person and the org", `pair --org-key-fingerprint <F>` pins the org's recovery key when F is the
//!   fingerprint of the key Gateway serves now. Until then the server locks nothing for that key,
//!   and `serve` logs how to confirm it.
//! - `serve` syncs only when `NEXUS_HOST_SESSION_SYNC=1` and the device is trusted. It checks its device every
//!   [`WATCH`] and before each upload. Once the device is revoked, it stops every sync call and logs why, with no
//!   content. Sessions stay on the server.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::future::BoxFuture;
use nexus_client::Client;
use nexus_client::e2e::{self, DEVICE_SECRET_LEN, DeviceKey};
use nexus_workbench::session_sync::approver::{self, Approval, Approver};
use nexus_workbench::session_sync::org_key::{self, OrgKeyState, copy};
use nexus_workbench::session_sync::{
    Backend, Batch, ContentKey, ContentPolicy, GatewayBackend, Identity, KeyStore, Policy,
};
use serde_json::{Value, json};
use tokio::sync::watch;
use zeroize::Zeroizing;

use crate::Result;
use crate::files::Paths;

/// The switch for a server. Any other value, or none, is off. The desktop app has no switch. A server
/// keeps this one until a separate change decides its default. The app's own service does not read it: it syncs with
/// the device key the app hands it ([`crate::local_sync`]).
pub const SWITCH: &str = "NEXUS_HOST_SESSION_SYNC";

/// How long Gateway lets a pending device be approved after it registers.
pub const PAIRING_WINDOW: Duration = Duration::from_secs(15 * 60);

/// How often a running host checks its own device.
pub const WATCH: Duration = Duration::from_secs(30);

pub fn enabled() -> bool {
    std::env::var(SWITCH).is_ok_and(|value| value == "1")
}

// What `serve` logs. Fixed texts: none carries a session's content, a key or a path.
pub const SYNC_ON: &str = "nexus-host: session sync is on. This server's sessions sync, encrypted, to your devices.";
pub const NOT_PAIRED: &str = "nexus-host: session sync is off. This server is not paired: run `nexus-host pair`.";
pub const PENDING: &str =
    "nexus-host: session sync is waiting for approval in Settings › Devices. `nexus-host pair` shows the code.";
pub const REVOKED: &str = "nexus-host: session sync stopped. This server's device was revoked in Settings › Devices. \
     Its sessions stay on this server. Run `nexus-host pair` to pair it again.";
pub const NOT_SIGNED_IN: &str =
    "nexus-host: session sync is off. This server is not signed in: run `nexus-host login`, then `nexus-host pair`.";
pub const UNREACHABLE: &str =
    "nexus-host: session sync is waiting. Nexus Gateway did not answer about this server's device.";
pub const KEY_UNREADABLE: &str =
    "nexus-host: session sync is off. The device key file cannot be read: run `nexus-host pair`.";
pub const UNCONFIRMED: &str = "nexus-host: session sync is waiting. Confirm the device that approved this server by \
     its code: run `nexus-host pair`.";
// The org's recovery key under "The person and the org". Sync for the person's devices goes on.
pub const ORG_KEY_UNCONFIRMED: &str = "nexus-host: your org's recovery key is not confirmed on this server, so nothing \
     is locked for it. Compare its fingerprint with the one computed from your vault on your admin's own machine, then run `nexus-host pair --org-key-fingerprint <fingerprint>`.";
pub const ORG_KEY_CHANGED: &str = "nexus-host: your org's recovery key changed since this server confirmed it, so \
     nothing is locked for the new key. Compare its fingerprint with the one computed from your vault on your admin's own machine, then run `nexus-host pair --org-key-fingerprint <fingerprint>`.";
pub const ORG_KEY_CONFIRMED: &str =
    "nexus-host: this server also locks its synced sessions for your org's recovery key, which you confirmed.";
/// The flag that confirms the org's recovery key on a server, which has no screen to press Confirm on.
pub const ORG_KEY_FLAG: &str = "--org-key-fingerprint";
/// What the guarded backend answers once it stopped. The engine reads it as unavailable.
const STOPPED: &str = "session sync: this server's device is not trusted";
/// The device list did not answer: nothing is uploaded, and the engine retries.
const UNCHECKED: &str = "session sync: this server's device could not be checked";

/// The key file: the device key's 64 secret bytes as base64url, one line.
pub fn load_key(path: &Path) -> Result<Option<DeviceKey>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => Zeroizing::new(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read {}: {error}", path.display())),
    };
    crate::files::make_private(path);
    // A file that is not a key reads as an error, never as no key: a new key would orphan the paired device.
    key_from_text(&text).map(Some).ok_or_else(|| {
        format!(
            "{} is not a device key Nexus can read. Move it away, then run `nexus-host pair` again.",
            path.display()
        )
    })
}

/// A device key from its text: its 64 secret bytes as base64url, as the key file and the desktop app's keychain hold
/// it. None when the text is not one.
pub fn key_from_text(text: &str) -> Option<DeviceKey> {
    let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(text.trim()).ok()?);
    let secret: &[u8; DEVICE_SECRET_LEN] = bytes.as_slice().try_into().ok()?;
    Some(DeviceKey::from_secret_bytes(secret))
}

/// Makes a new key and writes it, 0600, over any key there.
pub fn create_key(path: &Path) -> Result<DeviceKey> {
    let device = DeviceKey::generate();
    let text = Zeroizing::new(format!("{}\n", URL_SAFE_NO_PAD.encode(&device.secret_bytes()[..])));
    crate::files::save_private(path, &text)?;
    Ok(device)
}

/// The device's code as Settings › Devices shows it: four groups of four.
pub fn code(device: &Public) -> String {
    grouped(&device.code())
}

/// A 16-character code in four groups of four.
pub fn grouped(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    chars
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether the code a person typed is `code`: case, spaces and hyphens do not count.
pub fn same_code(typed: &str, code: &str) -> bool {
    let typed: String = typed.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    typed.to_uppercase() == code
}

/// A device's public keys as Gateway lists them (base64url).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Public {
    pub encryption: String,
    pub signing: String,
    fingerprint: [u8; 32],
}

impl Public {
    pub fn of(device: &DeviceKey) -> Public {
        Public {
            encryption: e2e::b64u(&device.encryption_key()),
            signing: e2e::b64u(&device.signing_key()),
            fingerprint: device.fingerprint(),
        }
    }

    /// The fingerprint's 16-character code.
    pub fn code(&self) -> String {
        e2e::short_code(&self.fingerprint)
    }
}

/// Where this server's device stands in the person's device list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// No device has this key.
    Unregistered,
    /// Registered, not yet approved. `expired`: past [`PAIRING_WINDOW`], so it can no longer be approved.
    Pending {
        id: String,
        expired: bool,
    },
    Trusted {
        id: String,
    },
    Revoked {
        id: String,
    },
}

/// The standing of `me` in `GET /v1/devices`'s `devices`. A revoked entry with these keys wins over every other.
pub fn standing_in(devices: &Value, me: &Public, now_ms: i64) -> Result<Standing> {
    let devices = devices
        .as_array()
        .ok_or("Gateway's device list is not in the expected form.")?;
    let mut best = Standing::Unregistered;
    for device in devices {
        let text = |name: &str| device.get(name).and_then(Value::as_str).unwrap_or_default();
        if text("encryptionKey") != me.encryption || text("signingKey") != me.signing {
            continue;
        }
        let id = text("id").to_string();
        let found = match text("state") {
            "revoked" => return Ok(Standing::Revoked { id }),
            "trusted" => Standing::Trusted { id },
            "pending" => {
                let created = chrono::DateTime::parse_from_rfc3339(text("createdAt"))
                    .ok()
                    .map(|at| at.timestamp_millis());
                // An unreadable time counts as expired: pairing again with a new key is always safe.
                let expired = created.is_none_or(|created| now_ms - created >= PAIRING_WINDOW.as_millis() as i64);
                Standing::Pending { id, expired }
            }
            _ => continue,
        };
        let rank = |standing: &Standing| match standing {
            Standing::Unregistered => 0,
            Standing::Pending { expired: true, .. } => 1,
            Standing::Pending { expired: false, .. } => 2,
            Standing::Trusted { .. } | Standing::Revoked { .. } => 3,
        };
        if rank(&found) > rank(&best) {
            best = found;
        }
    }
    Ok(best)
}

/// The device calls pairing makes, so its flow is tested without a Gateway.
pub trait Directory: Send + Sync {
    /// `GET /v1/devices`'s `devices`.
    fn list(&self) -> BoxFuture<'_, Result<Value>>;
    /// `POST /v1/devices`: the device as Gateway stored it.
    fn register(&self, name: &str, me: &Public) -> BoxFuture<'_, Result<Value>>;
    /// The key store's owner for this sign-in: the Gateway and the person (`GET /v1/auth/whoami`).
    fn owner(&self) -> BoxFuture<'_, Result<String>>;
    /// The org's recovery key and what pinning it needs.
    fn org_key(&self) -> BoxFuture<'_, Result<OrgKeyRead>>;
}

/// The org's recovery key as Gateway serves it: the organization of the sign-in (`GET /v1/auth/whoami`), whether its
/// content policy is "The person and the org" (`GET /v1/orgs/settings`), and `GET /v1/orgs/recovery-key`'s answer.
#[derive(Debug, Clone, PartialEq)]
pub struct OrgKeyRead {
    pub tenant: String,
    pub user_and_org: bool,
    pub answer: Value,
}

impl Directory for Client {
    fn org_key(&self) -> BoxFuture<'_, Result<OrgKeyRead>> {
        Box::pin(async move {
            let asked = |error: nexus_client::Error| {
                format!("Nexus Gateway did not answer about your org's recovery key: {error}")
            };
            let whoami = self.get("/v1/auth/whoami").await.map_err(asked)?;
            let tenant = whoami
                .get("tenantId")
                .and_then(Value::as_str)
                .filter(|tenant| !tenant.is_empty())
                .ok_or("Session sync needs a sign-in to an organization: run `nexus-host login`.")?
                .to_string();
            let settings = self.get("/v1/orgs/settings").await.map_err(asked)?;
            let answer = self.get("/v1/orgs/recovery-key").await.map_err(asked)?;
            Ok(OrgKeyRead {
                tenant,
                user_and_org: settings.get("sessionContent").and_then(Value::as_str) == Some("user_and_org"),
                answer,
            })
        })
    }

    fn owner(&self) -> BoxFuture<'_, Result<String>> {
        Box::pin(async move {
            let whoami = self
                .get("/v1/auth/whoami")
                .await
                .map_err(|error| format!("Nexus Gateway did not say who you are: {error}"))?;
            let person = whoami
                .get("userId")
                .and_then(Value::as_str)
                .filter(|person| !person.is_empty())
                .ok_or("Session sync needs a person's sign-in: run `nexus-host login`.")?;
            Ok(approver::owner(self.base(), person))
        })
    }

    fn list(&self) -> BoxFuture<'_, Result<Value>> {
        Box::pin(async move {
            let answer = self
                .get("/v1/devices")
                .await
                .map_err(|error| format!("Nexus Gateway did not list your devices: {error}"))?;
            Ok(answer.get("devices").cloned().unwrap_or(Value::Null))
        })
    }

    fn register(&self, name: &str, me: &Public) -> BoxFuture<'_, Result<Value>> {
        let body = json!({ "name": name, "encryptionKey": me.encryption, "signingKey": me.signing });
        Box::pin(async move {
            self.post("/v1/devices", body)
                .await
                .map_err(|error| format!("Nexus Gateway did not register this server: {error}"))
        })
    }
}

pub async fn standing(directory: &dyn Directory, me: &Public) -> Result<Standing> {
    standing_in(&directory.list().await?, me, nexus_workbench::now_ms())
}

/// The device name Settings › Devices shows: Gateway takes at most 64 characters.
pub fn device_name(machine: &str) -> String {
    format!("nexus-host on {machine}").chars().take(64).collect()
}

/// The key store's files: the content keys this device holds, locked to its key.
fn key_store_files(paths: &Paths) -> [PathBuf; 3] {
    let store = paths.session_sync_keys();
    let with = |suffix: &str| PathBuf::from(format!("{}{suffix}", store.display()));
    [store.clone(), with("-wal"), with("-shm")]
}

/// How `pair` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub id: String,
    pub code: String,
}

/// Whether the person sees the approver's code on the approver's own screen: true records the approver.
pub type Confirm<'a> = &'a mut (dyn FnMut(&Approver) -> bool + Send);

/// After approval. Gateway names this server's approver, and Gateway is not trusted for
/// that. So the person compares the approver's code with the approver's own screen. Only a yes is recorded, and only
/// then does the server lock content keys for it. A no records nothing and is an error.
async fn confirm_approver(
    directory: &dyn Directory,
    paths: &Paths,
    device: &DeviceKey,
    say: &mut (dyn FnMut(&str) + Send),
    confirm: Confirm<'_>,
) -> Result<()> {
    let listed = directory.list().await?;
    let devices = listed
        .as_array()
        .ok_or("Gateway's device list is not in the expected form.")?;
    let failed = |error: approver::ConfirmError| error.message().to_string();
    // No approver, no store to open: a root has nothing to confirm.
    if approver::approver_of(devices, device).map_err(failed)?.is_none() {
        return Ok(());
    }
    let owner = directory.owner().await?;
    let keys = KeyStore::open(&paths.session_sync_keys())?;
    let shown = match approver::approval(&keys, &owner, devices, device).map_err(failed)? {
        Approval::Root | Approval::Confirmed(_) => return Ok(()),
        Approval::Unconfirmed(shown) => shown,
    };
    say(&format!(
        "Nexus says \"{}\" approved this server. On it, open Settings › Devices. Its fingerprint must be\n\n  {}\n",
        shown.name,
        grouped(&shown.code)
    ));
    if !confirm(&shown) {
        return Err(format!(
            "Nothing was recorded: this server syncs nothing until you confirm its approver. If \"{}\" shows another \
             code, revoke this server from a device you trust, in Settings › Devices.",
            shown.name
        ));
    }
    approver::confirm(&keys, &owner, devices, device, &shown.id).map_err(failed)?;
    say(&format!("Confirmed: \"{}\" approved this server.", shown.name));
    Ok(())
}

/// Whether this server is trusted with an approver the person has not confirmed here. False when that cannot be read:
/// the adapter refuses an unconfirmed approver itself.
async fn unconfirmed(directory: &dyn Directory, keys: &KeyStore, device: &DeviceKey) -> bool {
    let (Ok(listed), Ok(owner)) = (directory.list().await, directory.owner().await) else {
        return false;
    };
    let Some(devices) = listed.as_array() else {
        return false;
    };
    matches!(
        approver::approval(keys, &owner, devices, device),
        Ok(Approval::Unconfirmed(_))
    )
}

/// The org's recovery key on this server. A server has no screen, so the person confirms with
/// `--org-key-fingerprint`: the fingerprint computed from your vault. It is pinned only when it matches the
/// key Gateway serves now; otherwise nothing is recorded and this is an error. Without the flag, an unconfirmed or
/// changed key is shown with the next step, and pairing still succeeds: sync for the person's devices does not wait.
pub async fn confirm_org_key(
    directory: &dyn Directory,
    paths: &Paths,
    fingerprint: Option<&str>,
    say: &mut (dyn FnMut(&str) + Send),
) -> Result<()> {
    let owner = directory.owner().await?;
    let read = directory.org_key().await?;
    let keys = KeyStore::open(&paths.session_sync_keys())?;
    let failed = |error: org_key::ConfirmError| error.message().to_string();
    if let Some(typed) = fingerprint {
        let shown = org_key::confirm(&keys, &owner, &read.tenant, &read.answer, typed).map_err(failed)?;
        say(&format!(
            "{}\n\n  {}\n\n{}",
            copy::TITLE,
            org_key::grouped(&shown.fingerprint),
            copy::CONFIRMED
        ));
        return Ok(());
    }
    let (shown, standing) = match org_key::state(&keys, &owner, &read.tenant, &read.answer).map_err(failed)? {
        OrgKeyState::None | OrgKeyState::Confirmed(_) => return Ok(()),
        OrgKeyState::Unconfirmed(shown) => (shown, copy::UNCONFIRMED),
        OrgKeyState::Changed { served, .. } => (served, copy::CHANGED),
    };
    say(&format!(
        "{}\n\n  {}\n\n{standing}\n{}\nRun `nexus-host pair {ORG_KEY_FLAG} <fingerprint>` to confirm it.",
        copy::TITLE,
        org_key::grouped(&shown.fingerprint),
        copy::COMPARE
    ));
    Ok(())
}

/// What `serve` logs about the org's recovery key, under "The person and the org" only. None when that cannot be
/// read, or under another policy.
async fn org_key_notice(directory: &dyn Directory, keys: &KeyStore) -> Option<&'static str> {
    let (Ok(owner), Ok(read)) = (directory.owner().await, directory.org_key().await) else {
        return None;
    };
    if !read.user_and_org {
        return None;
    }
    match org_key::state(keys, &owner, &read.tenant, &read.answer) {
        Ok(OrgKeyState::Unconfirmed(_)) => Some(ORG_KEY_UNCONFIRMED),
        Ok(OrgKeyState::Changed { .. }) => Some(ORG_KEY_CHANGED),
        Ok(OrgKeyState::Confirmed(_)) => Some(ORG_KEY_CONFIRMED),
        Ok(OrgKeyState::None) | Err(_) => None,
    }
}

/// Registers this server's device and waits until a trusted device approves it, then has the person confirm the
/// approver (`confirm`). A revoked key, or a pending one past its window, is replaced by a new key: the old one's content
/// keys go with it. `say` hears what the person reads.
pub async fn pair(
    directory: &dyn Directory,
    paths: &Paths,
    name: &str,
    poll: Duration,
    say: &mut (dyn FnMut(&str) + Send),
    confirm: Confirm<'_>,
) -> Result<Paired> {
    let path = paths.device_key();
    let mut device = match load_key(&path)? {
        Some(device) => device,
        None => create_key(&path)?,
    };
    let mut me = Public::of(&device);
    let listed = directory.list().await?;
    let mut standing = standing_in(&listed, &me, nexus_workbench::now_ms())?;
    if let Standing::Trusted { id } = standing {
        say(&format!("This server is paired already. Its code is {}.", code(&me)));
        confirm_approver(directory, paths, &device, say, confirm).await?;
        return Ok(Paired { id, code: me.code() });
    }
    if matches!(
        standing,
        Standing::Revoked { .. } | Standing::Pending { expired: true, .. }
    ) {
        // The old key can never be trusted again. Its content keys open with it only, so they go too.
        for file in key_store_files(paths) {
            let _ = std::fs::remove_file(file);
        }
        device = create_key(&path)?;
        me = Public::of(&device);
        standing = Standing::Unregistered;
    }
    if standing == Standing::Unregistered {
        // A server cannot approve another device, so it must not become the person's first one.
        let any_trusted = listed
            .as_array()
            .into_iter()
            .flatten()
            .any(|device| matches!(device["state"].as_str(), Some("trusted" | "revoked")));
        if !any_trusted {
            return Err(
                "Set up Settings › Devices on your laptop or in Nexus UI first. A server cannot approve your \
                        other devices, so it cannot be your first one."
                    .into(),
            );
        }
        let stored = directory.register(name, &me).await?;
        let id = stored["id"].as_str().unwrap_or_default().to_string();
        standing = match stored["state"].as_str() {
            Some("trusted") => Standing::Trusted { id },
            _ => Standing::Pending { id, expired: false },
        };
    }
    if let Standing::Trusted { id } = standing {
        say("This server is paired: it was trusted when it registered.");
        confirm_approver(directory, paths, &device, say, confirm).await?;
        return Ok(Paired { id, code: me.code() });
    }
    say(&format!(
        "To pair this server, open Nexus › Settings › Devices on a trusted device. Approve \"{name}\" only if its \
         code is\n\n  {}\n\nWaiting up to 15 minutes. If you stop now, run `nexus-host pair` again within that time: \
         it shows the same code.",
        code(&me)
    ));
    let deadline = Instant::now() + PAIRING_WINDOW + poll;
    loop {
        tokio::time::sleep(poll).await;
        // A failed check is retried: the person may be approving right now.
        match standing_in(
            &directory.list().await.unwrap_or(Value::Null),
            &me,
            nexus_workbench::now_ms(),
        ) {
            Ok(Standing::Trusted { id }) => {
                say("Approved. This server is paired.");
                confirm_approver(directory, paths, &device, say, confirm).await?;
                return Ok(Paired { id, code: me.code() });
            }
            Ok(Standing::Revoked { .. }) => {
                return Err("This server was revoked instead of approved. Run `nexus-host pair` to try again.".into());
            }
            Ok(Standing::Pending { expired: true, .. }) => break,
            Ok(Standing::Unregistered) => {
                return Err("This server's device is no longer listed. Run `nexus-host pair` again.".into());
            }
            Ok(Standing::Pending { expired: false, .. }) | Err(_) => {}
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    Err("The pairing expired: no trusted device approved it in 15 minutes. Run `nexus-host pair` again.".into())
}

/// Where `serve` logs: standard error, each text once until another replaces it.
pub type Sink = Arc<dyn Fn(&str) + Send + Sync>;

struct Said {
    sink: Sink,
    last: Mutex<Option<String>>,
}

impl Said {
    fn say(&self, line: &str) {
        let mut last = self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.as_deref() != Some(line) {
            (self.sink)(line);
            *last = Some(line.to_string());
        }
    }
}

/// `GatewayBackend` for a trusted device, stopped for good once the host sees the device is no longer trusted. Then
/// `allow_local_cache` is false, so the engine makes no call and binds no new thread.
struct Guarded {
    inner: GatewayBackend,
    client: Client,
    me: Public,
    stopped: AtomicBool,
    said: Arc<Said>,
}

impl Guarded {
    fn stop(&self, why: &str) {
        if !self.stopped.swap(true, Ordering::SeqCst) {
            self.said.say(why);
        }
    }

    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Whether the device is still trusted. An unanswered check keeps it as it was; the engine retries.
    async fn check(&self) -> std::result::Result<(), String> {
        if self.stopped() {
            return Err(STOPPED.into());
        }
        match standing(&self.client, &self.me).await {
            Ok(Standing::Trusted { .. }) => Ok(()),
            Ok(_) => {
                self.stop(REVOKED);
                Err(STOPPED.into())
            }
            Err(_) => Err(UNCHECKED.into()),
        }
    }

    fn refuse<'a, T: Send + 'a>(&'a self) -> Option<BoxFuture<'a, Result<T>>> {
        self.stopped()
            .then(|| Box::pin(async { Err(STOPPED.to_string()) }) as BoxFuture<'a, Result<T>>)
    }
}

impl Backend for Guarded {
    fn computer(&self) -> Option<&dyn nexus_workbench::computer::Transport> {
        if self.stopped() { None } else { self.inner.computer() }
    }
    fn identity(&self) -> &Identity {
        self.inner.identity()
    }

    fn allow_local_cache(&self) -> bool {
        !self.stopped() && self.inner.allow_local_cache()
    }

    fn policy(&self) -> BoxFuture<'_, Result<Policy>> {
        self.refuse().unwrap_or_else(|| self.inner.policy())
    }

    fn head(&self, thread: &str) -> BoxFuture<'_, Result<Option<u64>>> {
        self.refuse().unwrap_or_else(|| self.inner.head(thread))
    }

    fn key(
        &self,
        thread: &str,
        previous_epoch: Option<u64>,
        policy: ContentPolicy,
    ) -> BoxFuture<'_, Result<Arc<dyn ContentKey>>> {
        // The adapter verifies the directory first and refuses a revoked device itself.
        self.refuse()
            .unwrap_or_else(|| self.inner.key(thread, previous_epoch, policy))
    }

    fn append(&self, batch: Batch) -> BoxFuture<'_, Result<u64>> {
        Box::pin(async move {
            // Checked before every upload: a revoked server sends nothing, staged ciphertext included.
            self.check().await?;
            self.inner.append(batch).await
        })
    }

    fn delete(&self, thread: &str) -> BoxFuture<'_, Result<()>> {
        self.refuse().unwrap_or_else(|| self.inner.delete(thread))
    }
}

/// The host's side of `Host::session_sync`: the guarded backend while this server is a trusted device, else none.
pub struct HostSync {
    slot: RwLock<Option<Arc<Guarded>>>,
    said: Arc<Said>,
}

impl Default for HostSync {
    fn default() -> HostSync {
        HostSync::new(Arc::new(|line: &str| eprintln!("{line}")))
    }
}

impl HostSync {
    pub fn new(sink: Sink) -> HostSync {
        HostSync {
            slot: RwLock::new(None),
            said: Arc::new(Said {
                sink,
                last: Mutex::new(None),
            }),
        }
    }

    pub fn current(&self) -> Option<Arc<dyn Backend>> {
        let slot = self.slot.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        slot.as_ref()
            .filter(|guarded| !guarded.stopped())
            .map(|guarded| guarded.clone() as Arc<dyn Backend>)
    }

    fn set(&self, next: Option<Arc<Guarded>>) {
        *self.slot.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
    }

    /// One check. A live backend is dropped once its device is not trusted. Without one, the key file is read
    /// again, so a server paired again while it runs syncs without a restart.
    pub async fn refresh(&self, client: &Client, paths: &Paths, scope: &str) {
        let live = self
            .slot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(live) = live {
            let checked = live.check().await;
            if checked.is_ok() {
                if let Ok(keys) = KeyStore::open(&paths.session_sync_keys()) {
                    self.say_org_key(client, &keys).await;
                }
                return;
            }
            if !live.stopped() {
                return;
            }
            self.set(None);
        }
        let device = match load_key(&paths.device_key()) {
            Ok(Some(device)) => device,
            Ok(None) => return self.said.say(NOT_PAIRED),
            Err(_) => return self.said.say(KEY_UNREADABLE),
        };
        let me = Public::of(&device);
        match standing(client, &me).await {
            Ok(Standing::Trusted { .. }) => {}
            Ok(Standing::Pending { .. }) => return self.said.say(PENDING),
            Ok(Standing::Revoked { .. }) => return self.said.say(REVOKED),
            Ok(Standing::Unregistered) => return self.said.say(NOT_PAIRED),
            Err(_) => return self.said.say(UNREACHABLE),
        }
        let keys = match KeyStore::open(&paths.session_sync_keys()) {
            Ok(keys) => Arc::new(keys),
            // The adapter's errors carry no content.
            Err(error) => return self.said.say(&format!("nexus-host: {error}")),
        };
        if unconfirmed(client, &keys, &device).await {
            return self.said.say(UNCONFIRMED);
        }
        // A server has no managed policy that forbids a local cache: its store is the session log itself.
        match GatewayBackend::connect(client.clone(), scope.to_string(), device, keys.clone(), true).await {
            Ok(backend) => {
                self.set(Some(Arc::new(Guarded {
                    inner: backend,
                    client: client.clone(),
                    me,
                    stopped: AtomicBool::new(false),
                    said: self.said.clone(),
                })));
                self.said.say(SYNC_ON);
                self.say_org_key(client, &keys).await;
            }
            Err(error) => self.said.say(&format!("nexus-host: {error}")),
        }
    }

    /// Logs where the org's recovery key stands, under "The person and the org" only, once until it changes.
    async fn say_org_key(&self, directory: &dyn Directory, keys: &KeyStore) {
        if let Some(notice) = org_key_notice(directory, keys).await {
            self.said.say(notice);
        }
    }

    /// [`HostSync::refresh`] now and every [`WATCH`], until `stop`.
    pub async fn watch(self: Arc<Self>, client: Client, paths: Paths, scope: String, mut stop: watch::Receiver<bool>) {
        loop {
            self.refresh(&client, &paths, &scope).await;
            tokio::select! {
                _ = stop.wait_for(|stopped| *stopped) => return,
                _ = tokio::time::sleep(WATCH) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    fn folder(name: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!("nh-sync-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = Paths::at(dir);
        paths.create().expect("folder");
        paths
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[test]
    fn the_key_file_is_private_and_reads_back() {
        let paths = folder("key");
        let path = paths.device_key();
        assert_eq!(path, paths.token().with_file_name("device-key"), "beside the token");
        assert!(load_key(&path).expect("no file").is_none());
        let made = Public::of(&create_key(&path).expect("made"));
        assert_eq!(mode(&path), 0o600);
        assert_eq!(Public::of(&load_key(&path).expect("read").expect("a key")), made);
        assert_eq!(made.code().len(), 16);

        // A file others could read is made private again.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert_eq!(Public::of(&load_key(&path).expect("read").expect("a key")), made);
        assert_eq!(mode(&path), 0o600);

        // A file that is not a key is an error, and stays as it was.
        std::fs::write(&path, "not a key\n").expect("write");
        assert!(load_key(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "not a key\n");
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn the_code_reads_in_groups_of_four() {
        let me = Public::of(&DeviceKey::generate());
        let shown = code(&me);
        assert_eq!(shown.len(), 19);
        assert_eq!(shown.replace(' ', ""), me.code());
        assert!(same_code(&shown.to_lowercase(), &me.code()));
        assert!(same_code(&shown.replace(' ', "-"), &me.code()));
        assert!(!same_code(&shown[..18], &me.code()));
    }

    fn entry(me: &Public, id: &str, state: &str, created_ms: i64) -> Value {
        let created = chrono::DateTime::from_timestamp_millis(created_ms)
            .expect("time")
            .to_rfc3339();
        json!({ "id": id, "encryptionKey": me.encryption, "signingKey": me.signing, "state": state,
                "createdAt": created })
    }

    #[test]
    fn standing_reads_the_device_list() {
        let me = Public::of(&DeviceKey::generate());
        let other = Public::of(&DeviceKey::generate());
        let now = 1_762_000_000_000;
        let window = PAIRING_WINDOW.as_millis() as i64;
        let standing = |devices: Value| standing_in(&devices, &me, now).expect("standing");
        assert_eq!(
            standing(json!([entry(&other, "dev_o", "trusted", now)])),
            Standing::Unregistered
        );
        assert_eq!(
            standing(json!([entry(&me, "dev_m", "pending", now - 1000)])),
            Standing::Pending {
                id: "dev_m".into(),
                expired: false
            }
        );
        assert_eq!(
            standing(json!([entry(&me, "dev_m", "pending", now - window)])),
            Standing::Pending {
                id: "dev_m".into(),
                expired: true
            }
        );
        assert_eq!(
            standing(json!([entry(&me, "dev_m", "trusted", now)])),
            Standing::Trusted { id: "dev_m".into() }
        );
        // A revoked entry with these keys wins, wherever it is listed.
        assert_eq!(
            standing(json!([
                entry(&me, "dev_a", "trusted", now),
                entry(&me, "dev_b", "revoked", now)
            ])),
            Standing::Revoked { id: "dev_b".into() }
        );
        // Only both keys match: a listed device with this encryption key and another signing key is not this one.
        let mut half = entry(&me, "dev_h", "trusted", now);
        half["signingKey"] = json!(other.signing);
        assert_eq!(standing(json!([half])), Standing::Unregistered);
        assert!(standing_in(&json!({ "devices": [] }), &me, now).is_err());
    }

    /// Gateway's device list, in memory. `approve_after` lists: the pending device turns trusted after that many,
    /// approved by `approver` when there is one, else with no approver.
    struct Fake {
        devices: Mutex<Vec<Value>>,
        approve_after: Mutex<Option<usize>>,
        registered: Mutex<Vec<String>>,
        approver: Option<(String, Arc<DeviceKey>)>,
        org: Mutex<OrgKeyRead>,
    }

    impl Fake {
        fn new(devices: Vec<Value>, approve_after: Option<usize>) -> Fake {
            Fake {
                devices: Mutex::new(devices),
                approve_after: Mutex::new(approve_after),
                registered: Mutex::new(Vec::new()),
                approver: None,
                org: Mutex::new(OrgKeyRead {
                    tenant: "ten_acme".into(),
                    user_and_org: false,
                    answer: json!({ "key": null }),
                }),
            }
        }

        /// The org's policy and its recovery key: `key` is the id and the key's bytes, or none.
        fn set_org(&self, user_and_org: bool, key: Option<(&str, u8)>) {
            let answer = match key {
                Some((id, byte)) => json!({ "key": { "id": id, "encryptionKey": e2e::b64u(&[byte; 32]) } }),
                None => json!({ "key": null }),
            };
            let mut org = self.org.lock().expect("lock");
            org.user_and_org = user_and_org;
            org.answer = answer;
        }

        /// The approval `approver` signs for `device`, as Settings › Devices does.
        fn approve(device: &mut Value, approver: &(String, Arc<DeviceKey>)) {
            let key = |name: &str| -> [u8; 32] {
                URL_SAFE_NO_PAD
                    .decode(device[name].as_str().unwrap_or_default())
                    .ok()
                    .and_then(|bytes| bytes.try_into().ok())
                    .unwrap_or([0; 32])
            };
            let id = device["id"].as_str().unwrap_or_default().to_string();
            let message = e2e::approval_message(&id, &key("encryptionKey"), &key("signingKey")).expect("message");
            device["approvedBy"] = json!(approver.0);
            device["approvalSignature"] = json!(e2e::b64u(&approver.1.sign(&message)));
        }
    }

    impl Directory for Fake {
        fn owner(&self) -> BoxFuture<'_, Result<String>> {
            Box::pin(async { Ok(approver::owner("https://gw.test", "user_ada")) })
        }

        fn org_key(&self) -> BoxFuture<'_, Result<OrgKeyRead>> {
            let read = self.org.lock().expect("lock").clone();
            Box::pin(async move { Ok(read) })
        }

        fn list(&self) -> BoxFuture<'_, Result<Value>> {
            let mut after = self.approve_after.lock().expect("lock");
            let mut devices = self.devices.lock().expect("lock");
            if let Some(left) = after.as_mut() {
                if *left == 0 {
                    // As Gateway: a pending device past its window cannot be approved.
                    let now = nexus_workbench::now_ms();
                    for device in devices.iter_mut() {
                        let me = Public {
                            encryption: device["encryptionKey"].as_str().unwrap_or_default().into(),
                            signing: device["signingKey"].as_str().unwrap_or_default().into(),
                            fingerprint: [0; 32],
                        };
                        if standing_in(&json!([device.clone()]), &me, now)
                            == Ok(Standing::Pending {
                                id: device["id"].as_str().unwrap_or_default().into(),
                                expired: false,
                            })
                        {
                            device["state"] = json!("trusted");
                            if let Some(approver) = &self.approver {
                                Fake::approve(device, approver);
                            }
                        }
                    }
                    *after = None;
                } else {
                    *left -= 1;
                }
            }
            let listed = Value::Array(devices.clone());
            Box::pin(async move { Ok(listed) })
        }

        fn register(&self, name: &str, me: &Public) -> BoxFuture<'_, Result<Value>> {
            let mut devices = self.devices.lock().expect("lock");
            let id = format!("dev_{}", devices.len());
            let device = entry(me, &id, "pending", nexus_workbench::now_ms());
            devices.push(device.clone());
            self.registered.lock().expect("lock").push(name.to_string());
            Box::pin(async move { Ok(device) })
        }
    }

    fn laptop() -> Value {
        entry(&Public::of(&DeviceKey::generate()), "dev_laptop", "trusted", 0)
    }

    const POLL: Duration = Duration::from_millis(1);

    /// For a pairing whose approval names no approver: nothing to confirm.
    fn never(_: &Approver) -> bool {
        panic!("an approval with no approver has nothing to confirm")
    }

    #[tokio::test]
    async fn pairing_records_the_approver_only_when_the_person_confirms_its_code() {
        let paths = folder("confirm");
        let laptop_key = Arc::new(DeviceKey::generate());
        let laptop = entry(&Public::of(&laptop_key), "dev_laptop", "trusted", 0);
        let mut fake = Fake::new(vec![laptop], Some(1));
        fake.approver = Some(("dev_laptop".into(), laptop_key.clone()));
        async fn approval(fake: &Fake, paths: &Paths) -> std::result::Result<Approval, approver::ConfirmError> {
            let device = load_key(&paths.device_key()).expect("read").expect("a key");
            let devices = fake.list().await.expect("list");
            approver::approval(
                &KeyStore::open(&paths.session_sync_keys()).expect("key store"),
                &fake.owner().await.expect("owner"),
                devices.as_array().expect("devices"),
                &device,
            )
        }
        // The person says no: nothing is recorded, and pairing fails with how to revoke.
        let mut said = Vec::new();
        let mut shown = Vec::new();
        let refused = pair(
            &fake,
            &paths,
            "box",
            POLL,
            &mut |line: &str| said.push(line.to_string()),
            &mut |approver: &Approver| {
                shown.push(approver.clone());
                false
            },
        )
        .await
        .expect_err("not confirmed");
        assert!(
            refused.contains("Nothing was recorded") && refused.contains("revoke"),
            "{refused}"
        );
        let laptop_code = Public::of(&laptop_key).code();
        assert_eq!(
            shown,
            [Approver {
                id: "dev_laptop".into(),
                name: String::new(),
                code: laptop_code.clone(),
            }]
        );
        assert!(
            said.iter().any(|line| line.contains(&grouped(&laptop_code))),
            "{said:?}"
        );
        assert!(matches!(approval(&fake, &paths).await, Ok(Approval::Unconfirmed(_))));
        // Run again: the server is trusted, and this time the person sees that code on the laptop.
        let paired = pair(
            &fake,
            &paths,
            "box",
            POLL,
            &mut |_: &str| {},
            &mut |approver: &Approver| approver.code == laptop_code,
        )
        .await
        .expect("confirmed");
        assert_eq!(paired.id, "dev_1");
        assert!(matches!(approval(&fake, &paths).await, Ok(Approval::Confirmed(_))));
        // Confirmed once: pairing again asks nothing.
        pair(&fake, &paths, "box", POLL, &mut |_: &str| {}, &mut never)
            .await
            .expect("again");
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[tokio::test]
    async fn pairing_prints_the_code_and_waits_for_the_approval() {
        let paths = folder("pair");
        let fake = Fake::new(vec![laptop()], Some(3));
        let mut said = Vec::new();
        let paired = pair(
            &fake,
            &paths,
            "nexus-host on box",
            POLL,
            &mut |line: &str| said.push(line.to_string()),
            &mut never,
        )
        .await
        .expect("paired");
        let me = Public::of(&load_key(&paths.device_key()).expect("read").expect("a key"));
        assert_eq!(paired.code, me.code());
        assert_eq!(paired.id, "dev_1");
        assert_eq!(*fake.registered.lock().expect("lock"), ["nexus-host on box"]);
        assert!(said[0].contains(&code(&me)), "{said:?}");
        assert_eq!(
            said.last().map(String::as_str),
            Some("Approved. This server is paired.")
        );

        // Again: already paired, nothing registered.
        let again = pair(&fake, &paths, "nexus-host on box", POLL, &mut |_: &str| {}, &mut never)
            .await
            .expect("again");
        assert_eq!(again, paired);
        assert_eq!(fake.registered.lock().expect("lock").len(), 1);
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[tokio::test]
    async fn a_pairing_run_again_in_its_window_keeps_its_code() {
        let paths = folder("again");
        let me = Public::of(&create_key(&paths.device_key()).expect("key"));
        let fake = Fake::new(
            vec![laptop(), entry(&me, "dev_m", "pending", nexus_workbench::now_ms())],
            Some(1),
        );
        let paired = pair(&fake, &paths, "box", POLL, &mut |_: &str| {}, &mut never)
            .await
            .expect("paired");
        assert_eq!((paired.id.as_str(), paired.code), ("dev_m", me.code()));
        assert!(fake.registered.lock().expect("lock").is_empty());
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[tokio::test]
    async fn a_revoked_or_expired_key_is_replaced_with_its_content_keys() {
        for state in ["revoked", "pending"] {
            let paths = folder(state);
            let old = Public::of(&create_key(&paths.device_key()).expect("key"));
            for file in key_store_files(&paths) {
                std::fs::write(file, "locked copies").expect("store");
            }
            let fake = Fake::new(vec![laptop(), entry(&old, "dev_old", state, 0)], Some(1));
            let paired = pair(&fake, &paths, "box", POLL, &mut |_: &str| {}, &mut never)
                .await
                .expect("paired");
            let new = Public::of(&load_key(&paths.device_key()).expect("read").expect("a key"));
            assert_ne!(new, old, "{state}");
            assert_eq!(paired.code, new.code());
            assert_eq!(mode(&paths.device_key()), 0o600);
            assert!(key_store_files(&paths).iter().all(|file| !file.exists()), "{state}");
            let _ = std::fs::remove_dir_all(&paths.dir);
        }
    }

    #[tokio::test]
    async fn a_server_is_never_the_first_device() {
        let paths = folder("first");
        let fake = Fake::new(vec![], None);
        let refused = pair(&fake, &paths, "box", POLL, &mut |_: &str| {}, &mut never).await;
        assert!(refused.expect_err("refused").contains("first one"));
        assert!(fake.registered.lock().expect("lock").is_empty());
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[tokio::test]
    async fn a_pairing_revoked_while_waiting_says_so() {
        let paths = folder("refused");
        let fake = Fake::new(vec![laptop()], None);
        let mut quiet = |_: &str| {};
        let mut unasked = never;
        let waiting = pair(
            &fake,
            &paths,
            "box",
            Duration::from_millis(20),
            &mut quiet,
            &mut unasked,
        );
        let revoke = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            for device in fake.devices.lock().expect("lock").iter_mut() {
                if device["state"] == "pending" {
                    device["state"] = json!("revoked");
                }
            }
        };
        let (paired, ()) = tokio::join!(waiting, revoke);
        assert!(paired.expect_err("refused").contains("revoked"));
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[tokio::test]
    async fn the_org_key_is_pinned_only_with_the_fingerprint_of_the_key_gateway_serves() {
        let paths = folder("org-key");
        let fake = Fake::new(vec![laptop()], None);
        let print = |byte: u8| e2e::org_key_fingerprint(&[byte; 32]);
        let state = |fake: &Fake| {
            let read = fake.org.lock().expect("lock").clone();
            org_key::state(
                &KeyStore::open(&paths.session_sync_keys()).expect("key store"),
                &approver::owner("https://gw.test", "user_ada"),
                &read.tenant,
                &read.answer,
            )
        };
        // No key: a fingerprint has nothing to match, and no flag says nothing.
        let refused = confirm_org_key(&fake, &paths, Some(&print(1)), &mut |_: &str| {}).await;
        assert_eq!(refused.expect_err("no key"), copy::NO_KEY);
        let mut said = Vec::new();
        confirm_org_key(&fake, &paths, None, &mut |line: &str| said.push(line.to_string()))
            .await
            .expect("nothing to do");
        assert!(said.is_empty(), "{said:?}");

        // A key, and no flag: the fingerprint and the next step, and pairing does not fail.
        fake.set_org(true, Some(("ork_one", 1)));
        confirm_org_key(&fake, &paths, None, &mut |line: &str| said.push(line.to_string()))
            .await
            .expect("shown");
        let shown = said.join("\n");
        assert!(shown.contains(&org_key::grouped(&print(1))), "{shown}");
        assert!(shown.contains(ORG_KEY_FLAG) && shown.contains(copy::COMPARE), "{shown}");
        assert!(matches!(state(&fake), Ok(OrgKeyState::Unconfirmed(_))));

        // Another key's fingerprint, as a swapped key would show: refused, nothing recorded.
        let refused = confirm_org_key(&fake, &paths, Some(&print(2)), &mut |_: &str| {}).await;
        assert_eq!(refused.expect_err("mismatch"), copy::MISMATCH);
        assert!(matches!(state(&fake), Ok(OrgKeyState::Unconfirmed(_))));

        // The served key's, as the admin reads it: grouped and in upper case.
        let typed = org_key::grouped(&print(1)).to_uppercase();
        confirm_org_key(&fake, &paths, Some(&typed), &mut |_: &str| {})
            .await
            .expect("confirmed");
        assert!(matches!(state(&fake), Ok(OrgKeyState::Confirmed(_))));

        // Gateway serves another key: changed, and the flag pins it only with its own fingerprint.
        fake.set_org(true, Some(("ork_two", 2)));
        assert!(matches!(state(&fake), Ok(OrgKeyState::Changed { .. })));
        let refused = confirm_org_key(&fake, &paths, Some(&print(1)), &mut |_: &str| {}).await;
        assert_eq!(refused.expect_err("old fingerprint"), copy::MISMATCH);
        confirm_org_key(&fake, &paths, Some(&print(2)), &mut |_: &str| {})
            .await
            .expect("confirmed again");
        assert!(matches!(state(&fake), Ok(OrgKeyState::Confirmed(_))));
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[tokio::test]
    async fn serve_logs_the_org_key_only_under_the_person_and_the_org() {
        let paths = folder("org-notice");
        let keys = KeyStore::open(&paths.session_sync_keys()).expect("key store");
        let fake = Fake::new(vec![laptop()], None);
        fake.set_org(false, Some(("ork_one", 1)));
        assert_eq!(org_key_notice(&fake, &keys).await, None, "under `user`");
        fake.set_org(true, Some(("ork_one", 1)));
        assert_eq!(org_key_notice(&fake, &keys).await, Some(ORG_KEY_UNCONFIRMED));
        confirm_org_key(
            &fake,
            &paths,
            Some(&e2e::org_key_fingerprint(&[1; 32])),
            &mut |_: &str| {},
        )
        .await
        .expect("confirmed");
        assert_eq!(org_key_notice(&fake, &keys).await, Some(ORG_KEY_CONFIRMED));
        fake.set_org(true, Some(("ork_one", 9)));
        assert_eq!(
            org_key_notice(&fake, &keys).await,
            Some(ORG_KEY_CHANGED),
            "a swapped key"
        );
        for line in [ORG_KEY_UNCONFIRMED, ORG_KEY_CHANGED] {
            assert!(
                line.contains(ORG_KEY_FLAG)
                    && line.contains("computed from your vault")
                    && !line.contains("Synced sessions"),
                "{line}"
            );
        }
        let _ = std::fs::remove_dir_all(&paths.dir);
    }

    #[test]
    fn the_device_name_fits_gateways_limit() {
        assert_eq!(device_name("box"), "nexus-host on box");
        assert_eq!(device_name(&"x".repeat(100)).chars().count(), 64);
    }

    #[test]
    fn the_logs_carry_no_content_and_repeat_nothing() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = lines.clone();
        let sync = HostSync::new(Arc::new(move |line: &str| sink.lock().expect("lock").push(line.into())));
        sync.said.say(PENDING);
        sync.said.say(PENDING);
        sync.said.say(REVOKED);
        assert_eq!(*lines.lock().expect("lock"), [PENDING, REVOKED]);
        assert!(sync.current().is_none());
    }
}
