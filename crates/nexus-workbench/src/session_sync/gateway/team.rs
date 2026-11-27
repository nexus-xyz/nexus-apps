//! The team key on the runner and the synced reader. Gateway's side is the team key service.
//!
//! - **Make.** A team-visible thread needs the current epoch. When there is none, or the newest is retired after a
//!   removal or a revoke, this device makes the next one and locks it for every verified holder of every member. Two
//!   devices may race: Gateway takes one (409), and the other reads it again.
//! - **Hand over.** At the directory refresh, at most once a minute, this device locks every epoch it holds for
//!   each member's verified holder that lacks the current one.
//! - **Trust.** Team operations need local managed opt-in. Only this person's chain or an administrator-admitted
//!   member root can send or receive a team key. Gateway's member list never grants trust.
//! - **Lock.** Each new epoch of a team-visible thread's content key is locked to the current epoch, signed. A
//!   thread key's record names its team copy by the team key's id. A key locked to another epoch, or to any epoch once
//!   the thread is private, is rotated.
//! - **Open.** A teammate's reader opens the team key with its device, then the thread key with the team key.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nexus_client::Error as ClientError;
use nexus_client::e2e::{self, HolderKey};
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::keystore::{Audience, Recipient};
use super::super::trust::{self, Holder, Listed, Trusted, key32};
use super::team_policy::TeamPolicy;
use super::{CRYPTO, GatewayBackend, INVALID_ANSWER, not_found, refusal};

/// The members' holders are read again at most this often: the directory refresh.
const MEMBERS_EVERY: Duration = Duration::from_secs(60);
/// Gateway's `MAX_TEAM_COPIES`: the most copies one request stores.
const MAX_TEAM_COPIES: usize = 500;
/// Gateway mints each team-key epoch's id with this prefix. A thread key's holders record its team copy by that id.
pub(super) const TEAM_ID_PREFIX: &str = "tmk_";

/// One member's verified holders, each with whether it holds the current epoch.
pub(super) struct Member {
    person: String,
    holders: Vec<(Holder, bool)>,
}

pub(super) type Members = tokio::sync::Mutex<Option<(Instant, TeamPolicy, Vec<Holder>, Arc<Vec<Member>>)>>;

/// The current epoch, which this device holds: its id, and its public half, checked against the private half.
pub(super) struct TeamEpoch {
    pub id: String,
    pub encryption: [u8; 32],
    policy: TeamPolicy,
}

/// Whether this thread uses a team key, including the current id when its private half has not arrived.
pub(super) enum TeamState {
    Off,
    Held(TeamEpoch),
    Waiting,
}

impl TeamState {
    /// Retired team copies never keep a content key alive. An unknown current epoch permits only a private copy.
    pub(super) fn keeps(&self, holders: &BTreeSet<String>) -> bool {
        let mut team_ids = holders.iter().filter(|holder| holder.starts_with(TEAM_ID_PREFIX));
        match self {
            Self::Off | Self::Waiting => team_ids.next().is_none(),
            Self::Held(epoch) => holders.contains(&epoch.id) && team_ids.all(|id| *id == epoch.id),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamKeyView {
    id: String,
    epoch: i32,
    encryption_key: String,
    retired_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyView {
    team_key_id: String,
    epoch: i32,
    holder_id: String,
    locked_key: String,
    sender_device_id: String,
    signature: String,
}

#[derive(Deserialize)]
struct TeamKeyState {
    key: Option<TeamKeyView>,
    copies: Vec<CopyView>,
}

/// One entry of `GET /v1/orgs/members/{userId}/holders`. Gateway includes revoked holders so clients remember them.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HolderView {
    id: String,
    kind: String,
    encryption_key: String,
    signing_key: Option<String>,
    approved_by: Option<String>,
    approval_signature: Option<String>,
    has_current_team_key: bool,
    #[serde(default = "trusted_state")]
    state: String,
}

fn trusted_state() -> String {
    "trusted".into()
}

impl HolderView {
    fn listed(self) -> Listed {
        Listed {
            id: self.id,
            kind: self.kind,
            encryption_key: self.encryption_key,
            signing_key: self.signing_key,
            state: self.state,
            approved_by: self.approved_by,
            approval_signature: self.approval_signature,
            handover: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadCopyView {
    key_version: i32,
    team_key_id: String,
    locked_key: String,
    sender_device_id: String,
    signature: String,
}

/// Gateway refused with 409 and one of `codes`.
pub(super) fn conflict(error: &ClientError, codes: &[&str]) -> bool {
    matches!(error, ClientError::Core(core) if core.status == 409 && core.code.as_deref().is_some_and(|code| codes.contains(&code)))
}

fn decode(text: &str) -> Result<Vec<u8>, &'static str> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| INVALID_ANSWER)
}

/// A verified holder's signing key: this person's, or a member's.
fn signer(trusted: &Trusted, members: &[Member], id: &str) -> Option<[u8; 32]> {
    trusted
        .holders
        .iter()
        .chain(
            members
                .iter()
                .flat_map(|member| member.holders.iter().map(|(holder, _)| holder)),
        )
        .find(|holder| holder.id == id)
        .and_then(|holder| holder.keys.signing)
}

impl GatewayBackend {
    fn team_policy(&self) -> Option<TeamPolicy> {
        TeamPolicy::load(
            &self.team_policy_path,
            &self.computer_scope.gateway,
            &self.computer_scope.tenant_id,
        )
    }

    fn require_team_policy(&self, policy: &TeamPolicy) -> Result<(), String> {
        if self.team_policy().as_ref() != Some(policy) {
            return Err("session sync: team sharing authorization changed on this computer".into());
        }
        Ok(())
    }

    /// Whether the organization's members read each other's sessions (`multiplayerSessions`).
    pub(super) async fn multiplayer(&self) -> Result<bool, String> {
        if self.team_policy().is_none() {
            return Ok(false);
        }
        let settings = self
            .client
            .get("/v1/orgs/settings")
            .await
            .map_err(|error| refusal(&error))?;
        Ok(settings.get("multiplayerSessions").and_then(Value::as_bool) == Some(true))
    }

    /// Whether Gateway explicitly lists this thread as team-visible. An unknown thread remains private.
    pub(super) async fn team_visible(&self, thread: &str) -> Result<bool, String> {
        match self.client.get(&format!("/v1/threads/{thread}")).await {
            Ok(view) => Ok(view.get("private").and_then(Value::as_bool) == Some(false)),
            Err(error) if not_found(&error) => Ok(false),
            Err(error) => Err(refusal(&error)),
        }
    }

    async fn team_state(&self, me: &str) -> Result<TeamKeyState, String> {
        let answer = self
            .client
            .get(&format!("/v1/orgs/team-key?holder={me}"))
            .await
            .map_err(|error| refusal(&error))?;
        Ok(serde_json::from_value(answer).map_err(|_| INVALID_ANSWER)?)
    }

    /// Members verified against the current local admissions and this person's confirmed chain.
    /// Read again when admissions or personal trust change, or after one minute, unless `fresh`.
    async fn members(&self, trusted: &Trusted, policy: &TeamPolicy, fresh: bool) -> Result<Arc<Vec<Member>>, String> {
        let mut cache = self.members.lock().await;
        if !fresh
            && let Some((at, admitted, own, members)) = cache.as_ref()
            && at.elapsed() < MEMBERS_EVERY
            && admitted == policy
            && *own == trusted.holders
        {
            return Ok(members.clone());
        }
        let answer = self
            .client
            .get("/v1/orgs/members")
            .await
            .map_err(|error| refusal(&error))?;
        let people: Vec<String> = answer
            .get("members")
            .and_then(Value::as_array)
            .ok_or(INVALID_ANSWER)?
            .iter()
            .map(|member| member.get("userId").and_then(Value::as_str).map(str::to_string))
            .collect::<Option<_>>()
            .ok_or(INVALID_ANSWER)?;
        let mut members = Vec::with_capacity(people.len());
        for person in people {
            if person != self.identity.user && !policy.roots.contains_key(&person) {
                continue;
            }
            let answer = match self.client.get(&format!("/v1/orgs/members/{person}/holders")).await {
                Ok(answer) => answer,
                // Removed since the list was read.
                Err(error) if not_found(&error) => continue,
                Err(error) => return Err(refusal(&error)),
            };
            let views: Vec<HolderView> = answer
                .get("holders")
                .cloned()
                .and_then(|holders| serde_json::from_value(holders).ok())
                .ok_or(INVALID_ANSWER)?;
            let has: HashMap<String, bool> = views
                .iter()
                .map(|view| (view.id.clone(), view.has_current_team_key))
                .collect();
            let holders: Vec<Holder> = if person == self.identity.user {
                trusted
                    .holders
                    .iter()
                    .filter(|holder| has.contains_key(&holder.id))
                    .cloned()
                    .collect()
            } else {
                let listed: Vec<Listed> = views.into_iter().map(HolderView::listed).collect();
                let Some(root) = policy.roots.get(&person) else {
                    continue;
                };
                let scope = format!("{}/team/{}/{person}", self.owner, self.computer_scope.tenant_id);
                self.keys
                    .inherit_revocations(&scope, &format!("{}/team/{person}", self.owner))?;
                let mut memory = self.keys.memory(&scope)?;
                let verified = trust::member(&listed, root, &mut memory);
                self.keys.remember(&scope, &memory)?;
                match verified {
                    Ok((_, holders)) => holders,
                    // `ROOT_CHANGED`: nothing for this teammate until a member confirms the new root.
                    Err(_) => continue,
                }
            };
            let holders = holders
                .into_iter()
                .map(|holder| {
                    let held = has.get(&holder.id).copied().unwrap_or(false);
                    (holder, held)
                })
                .collect();
            members.push(Member { person, holders });
        }
        let members = Arc::new(members);
        *cache = Some((Instant::now(), policy.clone(), trusted.holders.clone(), members.clone()));
        Ok(members)
    }

    /// The epochs this device holds: its copies whose sender is a verified holder, opened.
    fn held(
        &self,
        state: &TeamKeyState,
        trusted: &Trusted,
        members: &[Member],
    ) -> Result<Vec<(String, i32, HolderKey)>, String> {
        let (tenant, me) = (&self.computer_scope.tenant_id, &trusted.me.id);
        let mut held = Vec::new();
        for copy in state.copies.iter().filter(|copy| &copy.holder_id == me) {
            let Some(sender) = signer(trusted, members, &copy.sender_device_id) else {
                continue;
            };
            let (locked, signature) = (decode(&copy.locked_key)?, decode(&copy.signature)?);
            let message = e2e::team_key_message(tenant, copy.epoch, me, &locked).map_err(|_| CRYPTO)?;
            if !e2e::verify(&sender, &message, &signature) {
                continue;
            }
            if let Ok(team) = self.device.unlock_team_key(tenant, copy.epoch, me, &locked) {
                held.push((copy.team_key_id.clone(), copy.epoch, team));
            }
        }
        Ok(held)
    }

    /// The team key's current epoch, for a team-visible thread. Made when there is none or the newest is retired.
    /// A pending handover still names the current epoch, so retired thread copies cannot keep a key alive.
    pub(super) async fn team_epoch(&self, trusted: &Trusted) -> Result<TeamState, String> {
        let Some(policy) = self.team_policy() else {
            return Ok(TeamState::Off);
        };
        for _ in 0..2 {
            let state = self.team_state(&trusted.me.id).await?;
            let current = match &state.key {
                Some(key) if key.retired_at.is_none() => key,
                newest => {
                    let next = newest.as_ref().map_or(Some(1), |key| key.epoch.checked_add(1));
                    self.make_team_epoch(next.ok_or(INVALID_ANSWER)?, trusted, &policy)
                        .await?;
                    continue;
                }
            };
            if !current.id.starts_with(TEAM_ID_PREFIX) || current.epoch < 1 {
                return Err(INVALID_ANSWER.into());
            }
            let public = key32(&current.encryption_key).ok_or(INVALID_ANSWER)?;
            let members = self.members(trusted, &policy, false).await?;
            let held = self.held(&state, trusted, &members)?;
            let holds = held.iter().any(|(id, epoch, team)| {
                *id == current.id && *epoch == current.epoch && team.encryption_key() == public
            });
            return Ok(if holds {
                TeamState::Held(TeamEpoch {
                    id: current.id.clone(),
                    encryption: public,
                    policy: policy.clone(),
                })
            } else {
                TeamState::Waiting
            });
        }
        // The epoch changed during both creation attempts. Preserve only a key with no team copy.
        Ok(TeamState::Waiting)
    }

    /// A copy of `team`, epoch `epoch`, for `holder`, signed by this device.
    fn team_copy(&self, team: &HolderKey, epoch: i32, holder: &Holder) -> Result<Value, String> {
        let tenant = &self.computer_scope.tenant_id;
        let locked = team
            .lock_team_key_for(tenant, epoch, &holder.id, &holder.keys.encryption)
            .map_err(|_| CRYPTO)?;
        let message = e2e::team_key_message(tenant, epoch, &holder.id, &locked).map_err(|_| CRYPTO)?;
        Ok(json!({
            "epoch": epoch,
            "holderId": holder.id,
            "lockedKey": e2e::b64u(&locked),
            "signature": e2e::b64u(&self.device.sign(&message)),
        }))
    }

    async fn put_team_copies(&self, sender: &str, copies: &[Value], policy: &TeamPolicy) -> Result<(), String> {
        for chunk in copies.chunks(MAX_TEAM_COPIES) {
            self.require_team_policy(policy)?;
            self.client
                .put(
                    "/v1/orgs/team-key/copies",
                    json!({ "senderDeviceId": sender, "copies": chunk }),
                )
                .await
                .map_err(|error| refusal(&error))?;
        }
        Ok(())
    }

    /// Makes epoch `epoch`: a new pair, locked for this device and every verified holder of every member.
    /// When another device made it first (409), the caller reads it again.
    async fn make_team_epoch(&self, epoch: i32, trusted: &Trusted, policy: &TeamPolicy) -> Result<(), String> {
        let members = self.members(trusted, policy, true).await?;
        let team = HolderKey::generate();
        let mut copies = vec![self.team_copy(&team, epoch, &trusted.me)?];
        for (holder, _) in members.iter().flat_map(|member| member.holders.iter()) {
            if holder.id != trusted.me.id {
                copies.push(self.team_copy(&team, epoch, holder)?);
            }
        }
        let (first, rest) = copies.split_at(copies.len().min(MAX_TEAM_COPIES));
        let body = json!({
            "epoch": epoch,
            "encryptionKey": e2e::b64u(&team.encryption_key()),
            "senderDeviceId": trusted.me.id,
            "copies": first,
        });
        self.require_team_policy(policy)?;
        match self.client.post("/v1/orgs/team-key", body).await {
            Ok(_) => {}
            Err(error) if conflict(&error, &["team_key_epoch_taken", "team_key_current"]) => return Ok(()),
            Err(error) => return Err(refusal(&error)),
        }
        self.put_team_copies(&trusted.me.id, rest, policy).await
    }

    /// Hands every epoch this device holds to each member's verified holder that lacks the current one. At most
    /// once a minute, and only while an epoch is current: after a removal, the next epoch's maker locks it for all.
    pub(super) async fn hand_over(&self, trusted: &Trusted) -> Result<(), String> {
        let Some(policy) = self.team_policy() else {
            return Ok(());
        };
        {
            let mut last = self.handed_over.lock().await;
            if last.is_some_and(|at| at.elapsed() < MEMBERS_EVERY) {
                return Ok(());
            }
            *last = Some(Instant::now());
        }
        let state = self.team_state(&trusted.me.id).await?;
        if state.key.as_ref().is_none_or(|key| key.retired_at.is_some()) {
            return Ok(());
        }
        let members = self.members(trusted, &policy, true).await?;
        let held = self.held(&state, trusted, &members)?;
        let mut copies = Vec::new();
        for (holder, has) in members.iter().flat_map(|member| member.holders.iter()) {
            if *has || holder.id == trusted.me.id {
                continue;
            }
            for (_, epoch, team) in &held {
                copies.push(self.team_copy(team, *epoch, holder)?);
            }
        }
        self.put_team_copies(&trusted.me.id, &copies, &policy).await
    }

    /// Locks the thread's content key to the current epoch, signs it, stores it, and records it. A thread made
    /// private meanwhile is refused (409 `thread_private`) and gets none.
    pub(super) async fn lock_to_team(
        &self,
        thread: &str,
        key: &e2e::ContentKey,
        me: &Holder,
        team: &TeamEpoch,
    ) -> Result<(), String> {
        self.require_team_policy(&team.policy)?;
        let locked = key.lock_for(thread, &team.id, &team.encryption).map_err(|_| CRYPTO)?;
        let message = e2e::team_thread_key_message(thread, key.epoch(), &team.id, &locked).map_err(|_| CRYPTO)?;
        let body = json!({
            "keyVersion": e2e::key_version(key.epoch()).map_err(|_| CRYPTO)?,
            "teamKeyId": team.id,
            "lockedKey": e2e::b64u(&locked),
            "senderDeviceId": me.id,
            "signature": e2e::b64u(&self.device.sign(&message)),
        });
        self.keys.record_recipients(
            thread,
            key.epoch(),
            &[Recipient::shared(Audience::Team, &team.id, &team.encryption)],
        )?;
        match self.client.put(&format!("/v1/threads/{thread}/team-key"), body).await {
            Ok(_) => {}
            Err(error) if conflict(&error, &["thread_private"]) => return Ok(()),
            Err(error) => return Err(refusal(&error)),
        }
        self.keys
            .add_holders(thread, key.epoch(), &BTreeSet::from([team.id.clone()]))?;
        Ok(())
    }

    /// The synced reader's way through the team key: the thread's team copy at `version`, signed by a verified
    /// holder of the thread's person, opened with an epoch this device holds. `None` when there is no such copy.
    pub(super) async fn team_content_key(
        &self,
        thread: &str,
        version: i32,
        person: &str,
        trusted: &Trusted,
    ) -> Result<Option<e2e::ContentKey>, String> {
        let Some(policy) = self.team_policy() else {
            return Ok(None);
        };
        if !self.multiplayer().await? || !self.team_visible(thread).await? {
            return Ok(None);
        }
        let list = match self.client.get(&format!("/v1/threads/{thread}/team-keys")).await {
            Ok(list) => list,
            Err(error) if not_found(&error) => return Ok(None),
            Err(error) => return Err(refusal(&error)),
        };
        let copies: Vec<ThreadCopyView> = list
            .get("keys")
            .cloned()
            .and_then(|keys| serde_json::from_value(keys).ok())
            .ok_or(INVALID_ANSWER)?;
        let Some(copy) = copies.iter().find(|copy| copy.key_version == version) else {
            return Ok(None);
        };
        let epoch = e2e::epoch_of(version).ok_or(INVALID_ANSWER)?;
        let members = self.members(trusted, &policy, false).await?;
        let owner: Vec<&Holder> = if person == self.identity.user {
            trusted.holders.iter().collect()
        } else {
            members
                .iter()
                .filter(|member| member.person == person)
                .flat_map(|member| member.holders.iter().map(|(holder, _)| holder))
                .collect()
        };
        let Some(sender) = owner
            .iter()
            .find(|holder| holder.id == copy.sender_device_id)
            .and_then(|holder| holder.keys.signing)
        else {
            return Ok(None);
        };
        let locked = decode(&copy.locked_key)?;
        let message = e2e::team_thread_key_message(thread, epoch, &copy.team_key_id, &locked).map_err(|_| CRYPTO)?;
        if !e2e::verify(&sender, &message, &decode(&copy.signature)?) {
            return Ok(None);
        }
        let state = self.team_state(&trusted.me.id).await?;
        if self.team_policy().as_ref() != Some(&policy) {
            return Ok(None);
        }
        let held = self.held(&state, trusted, &members)?;
        Ok(held
            .iter()
            .find(|(id, _, _)| *id == copy.team_key_id)
            .and_then(|(_, _, team)| team.unlock_content_key(thread, epoch, &copy.team_key_id, &locked).ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_keeps_only_unshared_keys() {
        let waiting = TeamState::Waiting;
        let holders = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect();
        assert!(waiting.keeps(&holders(&["dev_owner"])));
        assert!(!waiting.keeps(&holders(&["dev_owner", "tmk_current"])));
        assert!(!waiting.keeps(&holders(&["dev_owner", "tmk_retired"])));
        assert!(!waiting.keeps(&holders(&["dev_owner", "tmk_current", "tmk_retired"])));
        assert!(!TeamState::Waiting.keeps(&holders(&["dev_owner", "tmk_retired"])));
    }

    #[test]
    fn held_and_private_states_still_reject_retired_team_copies() {
        let held = TeamState::Held(TeamEpoch {
            id: "tmk_current".into(),
            encryption: [1; 32],
            policy: TeamPolicy { roots: HashMap::new() },
        });
        let old = BTreeSet::from(["dev_owner".into(), "tmk_retired".into()]);
        assert!(!held.keeps(&old));
        assert!(!TeamState::Off.keeps(&old));
        assert!(held.keeps(&BTreeSet::from(["tmk_current".into()])));
        assert!(TeamState::Off.keeps(&BTreeSet::from(["dev_owner".into()])));
    }
}

#[cfg(test)]
#[path = "team_security_tests.rs"]
mod security_tests;
