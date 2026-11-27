//! The production `Backend`: Gateway's session store and key directory through `nexus-client`, and the
//! crypto core's keys (`nexus_client::e2e`). One value holds one immutable credential and one device key, so every
//! call is the same person's. Every error is one of this file's fixed texts: none carries content, a key or a path.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine as _;
use futures::future::BoxFuture;
use nexus_client::e2e::{self, DeviceKey};
use nexus_client::{Client, Error as ClientError};
use serde_json::{Value, json};

use super::keystore::{Audience, KeyStore, Recipient, StoredKey};
use super::org_key;
use super::trust::{self, Holder, Keys, Listed, Trusted};
use super::{Backend, Batch, ContentKey, ContentPolicy, DeviceTrust, Event, Identity, KEY_ROTATED, Policy, STORE_FULL};
use crate::Result;

mod computer;
mod control;
mod org;
mod team;
mod team_policy;
#[cfg(test)]
mod trust_mode_tests;

const UNREACHABLE: &str = "session sync: Nexus Gateway is unreachable";
const NOT_A_PERSON: &str = "session sync: the credential is not a person's";
const POLICY_UNKNOWN: &str = "session sync: the organization's content policy is unknown";
const NO_CONTENT_KEY: &str = "session sync: this policy takes no content key";
const INVALID_ANSWER: &str = "session sync: Gateway's answer is not in the expected form";
const INVALID_EVENT: &str = "session sync: an event is not in the form Gateway takes";
const INVALID_THREAD: &str = "session sync: a thread id is not a UUID";
const CRYPTO: &str = "session sync: a content key could not be locked, opened or used";
/// Gateway takes at most this many copies in one `PUT /v1/threads/{threadId}/keys`.
const MAX_COPIES: usize = 100;

pub struct GatewayBackend {
    client: Client,
    identity: Identity,
    computer_scope: crate::computer::Scope,
    allow_local_cache: bool,
    device: Arc<DeviceKey>,
    me: Keys,
    keys: Arc<KeyStore>,
    /// The key directory's memory belongs to the person on this Gateway, whatever the organization.
    owner: String,
    /// One key preparation at a time, so two threads never race the directory's memory.
    preparing: tokio::sync::Mutex<()>,
    /// The administrator file is local; team operations read it again so an opt-out takes effect.
    team_policy_path: std::path::PathBuf,
    members: team::Members,
    /// When this device last handed the team key over.
    handed_over: tokio::sync::Mutex<Option<std::time::Instant>>,
    /// Whether this computer follows Gateway's own trust (`DeviceTrust::Gateway`) since the device list's last read.
    gateway_trust: AtomicBool,
}

/// A content key from the crypto core. Its seal count starts at zero each time it is opened (`e2e::ContentKey`).
struct SealingKey(e2e::ContentKey);

impl ContentKey for SealingKey {
    fn epoch(&self) -> u64 {
        self.0.epoch()
    }
    fn seal(&self, thread: &str, seq: u64, plaintext: &[u8]) -> Result<Vec<u8>> {
        self.0
            .seal_event(thread, seq, plaintext)
            .map_err(|_| CRYPTO.to_string())
    }
}

/// Gateway's error as one of the texts the engine reads, or a content-free one. A Gateway error code is a fixed slug.
pub(crate) fn refusal(error: &ClientError) -> String {
    match error {
        ClientError::Core(core) => match (core.status, core.code.as_deref()) {
            (409, Some("key_version_behind")) => KEY_ROTATED.to_string(),
            (413, Some("session_store_full")) => STORE_FULL.to_string(),
            (status, code) => {
                let code = code
                    .filter(|code| code.len() <= 64 && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
                    .map(|code| format!(" {code}"))
                    .unwrap_or_default();
                format!("session sync: Gateway refused the call ({status}{code})")
            }
        },
        ClientError::Transport(_) => UNREACHABLE.to_string(),
    }
}

fn not_found(error: &ClientError) -> bool {
    matches!(error, ClientError::Core(core) if core.status == 404)
}

/// `sessionContent` of `GET /v1/orgs/settings`. Absent or unknown is an error, never a default.
pub(crate) fn content_policy(settings: &Value) -> std::result::Result<ContentPolicy, &'static str> {
    match settings.get("sessionContent").and_then(Value::as_str) {
        Some("never") => Ok(ContentPolicy::NeverLeaves),
        Some("user") => Ok(ContentPolicy::UserOnly),
        Some("user_and_org") => Ok(ContentPolicy::UserAndOrgAdmins),
        _ => Err(POLICY_UNKNOWN),
    }
}

/// A thread id as it goes in a path: the engine's lowercase UUID, nothing else.
fn thread_path(thread: &str) -> std::result::Result<&str, &'static str> {
    match uuid::Uuid::try_parse(thread) {
        Ok(uuid) if uuid.hyphenated().to_string() == thread => Ok(thread),
        _ => Err(INVALID_THREAD),
    }
}

/// One contract event as Gateway's `SyncedEvent`: the epoch as `keyVersion` (epoch + 1), the sealed bytes from the
/// engine's standard base64 to base64url without padding.
pub(crate) fn wire_event(event: &Event) -> std::result::Result<Value, &'static str> {
    let (key_version, sealed) = match (event.epoch, event.sealed.as_deref()) {
        (Some(epoch), Some(sealed)) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(sealed)
                .map_err(|_| INVALID_EVENT)?;
            if bytes.is_empty() {
                return Err(INVALID_EVENT);
            }
            (
                Some(e2e::key_version(epoch).map_err(|_| INVALID_EVENT)?),
                Some(e2e::b64u(&bytes)),
            )
        }
        (None, None) => (None, None),
        _ => return Err(INVALID_EVENT),
    };
    let tokens = |count: u64| i64::try_from(count).map_err(|_| INVALID_EVENT);
    let usage = event
        .usage
        .as_ref()
        .map(|usage| {
            Ok::<_, &'static str>(json!({
                "model": usage.model,
                "inputTokens": tokens(usage.input_tokens)?,
                "outputTokens": tokens(usage.output_tokens)?,
                "cacheReadTokens": tokens(usage.cache_read_tokens)?,
                "cacheWriteTokens": tokens(usage.cache_write_tokens)?,
                "costUsd": usage.cost_usd,
            }))
        })
        .transpose()?;
    Ok(json!({
        "seq": tokens(event.seq)?,
        "at": event.at,
        "agent": event.agent.as_str(),
        "usage": usage,
        "status": event.status,
        "keyVersion": key_version,
        "sealed": sealed,
    }))
}

/// Gateway's `lastSeq`: the highest stored event number, or none for an empty thread.
fn last_seq(answer: &Value) -> std::result::Result<Option<u64>, &'static str> {
    match answer.get("lastSeq") {
        None | Some(Value::Null) => Ok(None),
        Some(seq) => seq.as_u64().map(Some).ok_or(INVALID_ANSWER),
    }
}

impl GatewayBackend {
    #[cfg(test)]
    pub(super) fn with_team_policy_file(mut self, path: std::path::PathBuf) -> Self {
        self.team_policy_path = path;
        self
    }

    /// Reads who the credential is (`GET /v1/auth/whoami`): only a person's own credential syncs. `client` must hold
    /// the credential and never change; `scope` is the workbench's scope for that credential's organization.
    pub async fn connect(
        client: Client,
        scope: String,
        device: DeviceKey,
        keys: Arc<KeyStore>,
        allow_local_cache: bool,
    ) -> Result<GatewayBackend> {
        let whoami = client.get("/v1/auth/whoami").await.map_err(|error| refusal(&error))?;
        let user = match (
            whoami.get("kind").and_then(Value::as_str),
            whoami.get("userId").and_then(Value::as_str),
        ) {
            (Some("person"), Some(user)) if !user.is_empty() => user.to_string(),
            _ => return Err(NOT_A_PERSON.to_string()),
        };
        let owner = super::approver::owner(client.base(), &user);
        let me = Keys {
            encryption: device.encryption_key(),
            signing: Some(device.signing_key()),
        };
        let backend = GatewayBackend {
            computer_scope: crate::computer::Scope {
                gateway: client.base().trim_end_matches('/').to_string(),
                tenant_id: whoami
                    .get("tenantId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                person_id: user.clone(),
            },
            client,
            identity: Identity { scope, user },
            allow_local_cache,
            device: Arc::new(device),
            me,
            keys,
            owner,
            preparing: tokio::sync::Mutex::new(()),
            team_policy_path: nexus_client::config::managed_config_path(&|name| std::env::var(name).ok()),
            members: tokio::sync::Mutex::new(None),
            handed_over: tokio::sync::Mutex::new(None),
            gateway_trust: AtomicBool::new(false),
        };
        // The device list's trust, read now so the Remote control default is right before the first sync. An
        // unanswered read leaves approvals, and the next read of the list sets it.
        if let Ok(answer) = backend.client.get("/v1/devices").await {
            backend.note_trust(&answer);
        }
        Ok(backend)
    }

    /// Keeps the trust this computer follows for a `GET /v1/devices` answer, for [`Backend::device_trust`]. Gateway's
    /// own trust is followed only as the pin allows ([`KeyStore::settle_trust`]): its answer alone never moves a computer
    /// off approvals. A key store that fails keeps approvals.
    fn note_trust(&self, answer: &Value) -> DeviceTrust {
        let trust = self
            .keys
            .settle_trust(&self.owner, DeviceTrust::of(answer))
            .unwrap_or(DeviceTrust::Approvals);
        self.gateway_trust
            .store(trust == DeviceTrust::Gateway, Ordering::SeqCst);
        trust
    }

    async fn directory(&self) -> std::result::Result<Trusted, String> {
        let answer = self.client.get("/v1/devices").await.map_err(|error| refusal(&error))?;
        let listed: Vec<Listed> = answer
            .get("devices")
            .cloned()
            .and_then(|devices| serde_json::from_value(devices).ok())
            .ok_or(INVALID_ANSWER)?;
        let mut memory = self.keys.memory(&self.owner)?;
        let verified = match self.note_trust(&answer) {
            // Gateway vouches for the person's devices: no approval chain, approver or handover.
            DeviceTrust::Gateway => trust::verify_gateway(&listed, &self.me, &mut memory),
            DeviceTrust::Approvals => {
                // The approver's handover, opened here; `verify_with` checks that the confirmed approver signed it.
                let handover = trust::open_handover(&listed, &self.me, &self.device);
                trust::verify_with(&listed, &self.me, &mut memory, handover.as_ref())
            }
        };
        // Revocations are kept even when this device is refused.
        self.keys.remember(&self.owner, &memory)?;
        Ok(verified?)
    }

    /// This device's newest copy in Gateway: its epoch, and the copy when a trusted device's signature verifies.
    async fn gateway_copy(
        &self,
        thread: &str,
        trusted: &Trusted,
    ) -> std::result::Result<Option<(u64, Option<Vec<u8>>)>, String> {
        let path = format!("/v1/threads/{thread}/keys?device={}", trusted.me.id);
        let copy = match self.client.get(&path).await {
            Ok(copy) => copy,
            Err(error) if not_found(&error) => return Ok(None),
            Err(error) => return Err(refusal(&error)),
        };
        let text = |name: &str| copy.get(name).and_then(Value::as_str).ok_or(INVALID_ANSWER);
        let epoch = copy
            .get("keyVersion")
            .and_then(Value::as_i64)
            .and_then(|version| i32::try_from(version).ok())
            .and_then(e2e::epoch_of)
            .ok_or(INVALID_ANSWER)?;
        if text("deviceId")? != trusted.me.id {
            return Err(INVALID_ANSWER.to_string());
        }
        let decode = |name: &str| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(text(name)?)
                .map_err(|_| INVALID_ANSWER)
        };
        let (locked, signature) = (decode("lockedKey")?, decode("signature")?);
        let sender = text("senderDeviceId")?;
        let signed = trusted
            .holders
            .iter()
            .find(|holder| holder.id == sender)
            .is_some_and(|holder| {
                e2e::thread_key_message(thread, epoch, &trusted.me.id, &locked).is_ok_and(|message| {
                    holder
                        .keys
                        .signing
                        .is_some_and(|signing| e2e::verify(&signing, &message, &signature))
                })
            });
        Ok(Some((epoch, signed.then_some(locked))))
    }

    /// The contract's `key`: refresh and verify the directory, take the thread's newest key, rotate it when
    /// a device that holds it was revoked, store it on this computer, lock it for every trusted device, then return it.
    /// A team-visible thread's new key is locked to the team key's current epoch too, and a key locked to
    /// another epoch, or to any once the thread is private, rotates (`team`).
    /// Under `user_and_org`, every epoch is locked to the organization's recovery key too, once the person confirmed
    /// it on this computer. A key locked for another organization key, or for any under `user`, rotates (`org`).
    async fn prepare(
        &self,
        thread: &str,
        previous: Option<u64>,
        policy: ContentPolicy,
    ) -> std::result::Result<SealingKey, String> {
        let _one = self.preparing.lock().await;
        let trusted = self.directory().await?;
        let me = &trusted.me;
        let org = self.org_lock(policy).await?;
        let team = if self.multiplayer().await? {
            // A failed handover must not stop this person's own sync: the next refresh tries again.
            let _ = self.hand_over(&trusted).await;
            match self.team_visible(thread).await? {
                true => self.team_epoch(&trusted).await?,
                false => team::TeamState::Off,
            }
        } else {
            team::TeamState::Off
        };
        let revoked = self.keys.memory(&self.owner)?.revoked;
        let gateway = self.gateway_copy(thread, &trusted).await?;
        let local = self.keys.latest(thread)?;
        // A newer verified copy in Gateway (rotated elsewhere) is adopted; an unverified one is rotated past.
        let mut newest_known = previous.max(local.as_ref().map(|key| key.epoch));
        if let Some((epoch, copy)) = gateway {
            newest_known = newest_known.max(Some(epoch));
            if let Some(locked) = copy
                && local.as_ref().is_none_or(|key| key.epoch < epoch)
            {
                self.keys.insert(
                    thread,
                    &StoredKey {
                        epoch,
                        holder: me.id.clone(),
                        locked,
                        holders: BTreeSet::from([me.id.clone()]),
                    },
                )?;
            }
        }
        let current = match self.keys.latest(thread)? {
            Some(stored) => Some((self.open(thread, &stored)?, stored)),
            None => None,
        };
        let mut allowed: Vec<Recipient> = trusted.holders.iter().map(Recipient::personal).collect();
        if let team::TeamState::Held(epoch) = &team {
            allowed.push(Recipient::shared(Audience::Team, &epoch.id, &epoch.encryption));
        }
        if let org_key::OrgLock::Pinned(key) = &org {
            allowed.push(Recipient::shared(Audience::Organization, &key.id, &key.encryption));
        }
        let history = match current.as_ref() {
            Some((_, stored)) => self.keys.write_recipients(thread, stored)?,
            None => None,
        };
        let verified_history = history.is_some_and(|recipients| {
            recipients.contains(&Recipient::personal(me))
                && recipients.iter().all(|recipient| allowed.contains(recipient))
                && current.as_ref().is_some_and(|(_, stored)| {
                    stored
                        .holders
                        .iter()
                        .all(|id| recipients.iter().any(|recipient| &recipient.holder.id == id))
                })
        });
        let usable = current.filter(|(key, stored)| {
            let holders = &stored.holders;
            Some(key.epoch()) == newest_known
                && !holders.iter().any(|holder| revoked.contains(holder))
                && verified_history
                && team.keeps(holders)
                && org.keeps(holders)
        });
        let fresh = usable.is_none();
        let key = match usable {
            Some((key, _)) => key,
            None => {
                let epoch = newest_known.map_or(Ok(0), |epoch| epoch.checked_add(1).ok_or(CRYPTO))?;
                let key = e2e::ContentKey::generate(epoch);
                let locked = key.lock_for(thread, &me.id, &me.keys.encryption).map_err(|_| CRYPTO)?;
                // Stored before any copy is published, so a crash never leaves Gateway with a key this computer lost.
                self.keys.insert_for_writes(
                    thread,
                    &StoredKey {
                        epoch,
                        holder: me.id.clone(),
                        locked,
                        holders: BTreeSet::new(),
                    },
                    me,
                )?;
                key
            }
        };
        // Every trusted device and recovery key gets every epoch this computer holds: a device paired later, or one that
        // unlocks with a recovery key, reads the whole thread. A revoked device is not in `trusted`, so it gets nothing
        // new.
        for stored in self.keys.all(thread)? {
            let missing: Vec<_> = trusted
                .holders
                .iter()
                .filter(|holder| !stored.holders.contains(&holder.id))
                .collect();
            if missing.is_empty() {
                continue;
            }
            if stored.epoch == key.epoch() {
                self.publish(thread, &key, me, &missing).await?;
            } else {
                self.publish(thread, &self.open(thread, &stored)?, me, &missing).await?;
            }
        }
        // After this person's own copies: Gateway takes a team copy only of a version they hold.
        if fresh && let team::TeamState::Held(team) = &team {
            self.lock_to_team(thread, &key, me, team).await?;
        }
        // Also after the person's own copies: Gateway takes an org copy only of a version they hold.
        if let org_key::OrgLock::Pinned(org) = &org {
            self.lock_to_org(thread, &key, me, org).await?;
        }
        Ok(SealingKey(key))
    }

    /// Tests skip the minute between two reads of the members and two handovers.
    #[cfg(test)]
    pub(super) async fn refresh_team_now(&self) {
        *self.members.lock().await = None;
        *self.handed_over.lock().await = None;
    }

    fn open(&self, thread: &str, stored: &StoredKey) -> std::result::Result<e2e::ContentKey, &'static str> {
        self.device
            .unlock_content_key(thread, stored.epoch, &stored.holder, &stored.locked)
            .map_err(|_| CRYPTO)
    }

    /// Locks `key` for `holders`, signs each copy, and stores them in Gateway (`PUT /v1/threads/{threadId}/keys`).
    async fn publish(
        &self,
        thread: &str,
        key: &e2e::ContentKey,
        me: &Holder,
        holders: &[&Holder],
    ) -> std::result::Result<(), String> {
        let epoch = key.epoch();
        for chunk in holders.chunks(MAX_COPIES) {
            let mut copies = Vec::with_capacity(chunk.len());
            for holder in chunk {
                let locked = key
                    .lock_for(thread, &holder.id, &holder.keys.encryption)
                    .map_err(|_| CRYPTO)?;
                let message = e2e::thread_key_message(thread, epoch, &holder.id, &locked).map_err(|_| CRYPTO)?;
                copies.push(json!({
                    "deviceId": holder.id,
                    "lockedKey": e2e::b64u(&locked),
                    "signature": e2e::b64u(&self.device.sign(&message)),
                }));
            }
            let body = json!({
                "keyVersion": e2e::key_version(epoch).map_err(|_| CRYPTO)?,
                "senderDeviceId": me.id,
                "keys": copies,
            });
            let recipients: Vec<_> = chunk.iter().map(|holder| Recipient::personal(holder)).collect();
            self.keys.record_recipients(thread, epoch, &recipients)?;
            self.client
                .put(&format!("/v1/threads/{thread}/keys"), body)
                .await
                .map_err(|error| refusal(&error))?;
            let stored = chunk.iter().map(|holder| holder.id.clone()).collect();
            self.keys.add_holders(thread, epoch, &stored)?;
        }
        Ok(())
    }
}

impl Backend for GatewayBackend {
    fn computer(&self) -> Option<&dyn crate::computer::Transport> {
        Some(self)
    }
    fn device_trust(&self) -> DeviceTrust {
        if self.gateway_trust.load(Ordering::SeqCst) {
            DeviceTrust::Gateway
        } else {
            DeviceTrust::Approvals
        }
    }
    fn supports_control(&self) -> bool {
        true
    }

    fn control_lease(&self, thread: &str, enabled: bool) -> BoxFuture<'_, Result<()>> {
        let thread = thread.to_string();
        Box::pin(async move { self.publish_control(&thread, enabled).await })
    }

    fn control_commands(&self) -> BoxFuture<'_, Result<Vec<super::control::Command>>> {
        Box::pin(async move { self.read_control().await })
    }

    fn control_claim(&self, thread: &str, id: &str) -> BoxFuture<'_, Result<bool>> {
        let (thread, id) = (thread.to_string(), id.to_string());
        Box::pin(async move { self.claim_control(&thread, &id).await })
    }

    fn control_finish(&self, thread: &str, id: &str, completed: bool) -> BoxFuture<'_, Result<()>> {
        let (thread, id) = (thread.to_string(), id.to_string());
        Box::pin(async move { self.finish_control(&thread, &id, completed).await })
    }

    fn control_watch(&self, wake: Arc<tokio::sync::Notify>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move { self.watch_control(&wake).await })
    }

    fn identity(&self) -> &Identity {
        &self.identity
    }

    fn allow_local_cache(&self) -> bool {
        self.allow_local_cache
    }

    fn policy(&self) -> BoxFuture<'_, Result<Policy>> {
        Box::pin(async move {
            let settings = self
                .client
                .get("/v1/orgs/settings")
                .await
                .map_err(|error| refusal(&error))?;
            Ok(Policy {
                content: content_policy(&settings)?,
            })
        })
    }

    fn head(&self, thread: &str) -> BoxFuture<'_, Result<Option<u64>>> {
        let path = thread_path(thread).map(|thread| format!("/v1/threads/{thread}"));
        Box::pin(async move {
            match self.client.get(&path?).await {
                Ok(answer) => Ok(last_seq(&answer)?),
                Err(error) if not_found(&error) => Ok(None),
                Err(error) => Err(refusal(&error)),
            }
        })
    }

    fn key(
        &self,
        thread: &str,
        previous_epoch: Option<u64>,
        policy: ContentPolicy,
    ) -> BoxFuture<'_, Result<Arc<dyn ContentKey>>> {
        let thread = thread_path(thread).map(str::to_string);
        Box::pin(async move {
            let thread = thread?;
            if policy == ContentPolicy::NeverLeaves {
                return Err(NO_CONTENT_KEY.to_string());
            }
            let key = self.prepare(&thread, previous_epoch, policy).await?;
            Ok(Arc::new(key) as Arc<dyn ContentKey>)
        })
    }

    fn append(&self, batch: Batch) -> BoxFuture<'_, Result<u64>> {
        Box::pin(async move {
            let thread = thread_path(&batch.thread)?;
            let events = batch
                .events
                .iter()
                .map(wire_event)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let answer = self
                .client
                .post(&format!("/v1/threads/{thread}/events"), json!({ "events": events }))
                .await
                .map_err(|error| refusal(&error))?;
            Ok(last_seq(&answer)?.ok_or(INVALID_ANSWER)?)
        })
    }

    fn delete(&self, thread: &str) -> BoxFuture<'_, Result<()>> {
        let thread = thread_path(thread).map(str::to_string);
        Box::pin(async move {
            let thread = thread?;
            match self.client.delete(&format!("/v1/threads/{thread}")).await {
                Ok(_) => {}
                Err(error) if not_found(&error) => {}
                Err(error) => return Err(refusal(&error)),
            }
            self.keys.forget(&thread)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::SessionAgent;
    use crate::session_sync::{Status, Usage};
    use nexus_client::{CoreError, Transport};

    fn event(epoch: Option<u64>, sealed: Option<&[u8]>) -> Event {
        Event {
            seq: 4,
            at: 1_762_000_000_000,
            agent: SessionAgent::Claude,
            usage: Some(Usage {
                model: "claude-opus".into(),
                input_tokens: 10,
                output_tokens: 2,
                cache_read_tokens: 3,
                cache_write_tokens: 0,
                cost_usd: "0.0123".into(),
            }),
            status: Some(Status::Completed),
            epoch,
            sealed: sealed.map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)),
        }
    }

    fn core(status: u16, code: &str) -> ClientError {
        ClientError::Core(Box::new(CoreError {
            status,
            message: "a message that could name a secret title".into(),
            code: Some(code.into()),
            interval: None,
            details: None,
        }))
    }

    #[test]
    fn an_event_becomes_gateways_synced_event() {
        // Bytes whose standard base64 has `+`, `/` and padding, so the conversion shows.
        let bytes = [0xfb, 0xff, 0xbf, 0x00];
        let wire = wire_event(&event(Some(0), Some(&bytes))).expect("wire");
        assert_eq!(wire["keyVersion"], 1, "epoch 0 is key version 1");
        assert_eq!(wire["sealed"], "-_-_AA");
        assert_eq!(wire["seq"], 4);
        assert_eq!(wire["agent"], "claude");
        assert_eq!(wire["status"], "completed");
        assert_eq!(wire["usage"]["inputTokens"], 10);
        assert_eq!(wire["usage"]["costUsd"], "0.0123");
        assert_eq!(
            wire_event(&event(Some(6), Some(&bytes))).expect("wire")["keyVersion"],
            7
        );
        let open = wire_event(&event(None, None)).expect("usage only");
        assert!(open["keyVersion"].is_null() && open["sealed"].is_null());
        for half in [event(Some(0), None), event(None, Some(&bytes))] {
            assert_eq!(wire_event(&half), Err(INVALID_EVENT));
        }
        assert_eq!(wire_event(&event(Some(u64::MAX), Some(&bytes))), Err(INVALID_EVENT));
        let mut not_base64 = event(Some(0), None);
        not_base64.sealed = Some("not base64!".into());
        assert_eq!(wire_event(&not_base64), Err(INVALID_EVENT));
    }

    #[test]
    fn gateways_errors_map_to_the_engines_without_content() {
        assert_eq!(refusal(&core(409, "key_version_behind")), KEY_ROTATED);
        assert_eq!(refusal(&core(413, "session_store_full")), STORE_FULL);
        let other = refusal(&core(409, "seq_gap"));
        assert_eq!(other, "session sync: Gateway refused the call (409 seq_gap)");
        assert!(!refusal(&core(400, "Secret Title")).contains("Secret"));
        assert_eq!(
            refusal(&ClientError::Transport(Transport::Unreachable(
                "connection refused".into()
            ))),
            UNREACHABLE
        );
        assert!(not_found(&core(404, "thread_not_found")));
        assert!(!not_found(&core(410, "thread_deleted")));
    }

    #[test]
    fn the_content_policy_maps_one_to_one_and_fails_closed() {
        let policy = |value: Value| content_policy(&json!({ "sessionContent": value }));
        assert_eq!(policy(json!("never")), Ok(ContentPolicy::NeverLeaves));
        assert_eq!(policy(json!("user")), Ok(ContentPolicy::UserOnly));
        assert_eq!(policy(json!("user_and_org")), Ok(ContentPolicy::UserAndOrgAdmins));
        for unknown in [json!("everyone"), json!(null), json!(1)] {
            assert_eq!(policy(unknown), Err(POLICY_UNKNOWN));
        }
        assert_eq!(
            content_policy(&json!({ "agentSubscriptions": true })),
            Err(POLICY_UNKNOWN)
        );
    }

    #[test]
    fn heads_and_thread_paths() {
        assert_eq!(last_seq(&json!({ "lastSeq": 7 })), Ok(Some(7)));
        assert_eq!(last_seq(&json!({ "lastSeq": null })), Ok(None));
        assert_eq!(last_seq(&json!({ "lastSeq": -1 })), Err(INVALID_ANSWER));
        let id = uuid::Uuid::now_v7().to_string();
        assert_eq!(thread_path(&id), Ok(id.as_str()));
        for bad in ["../devices", "t_local", &id.to_uppercase()] {
            assert_eq!(thread_path(bad), Err(INVALID_THREAD));
        }
    }
}
