//! Signed device-addressed transport. Only verified typed operations leave this crypto boundary.
use super::*;
use serde::{Deserialize, Serialize};
#[derive(Deserialize)]
struct DirectoryDevice {
    #[serde(flatten)]
    listed: Listed,
    #[serde(default)]
    name: String,
    #[serde(rename = "createdAt", default)]
    created_at: String,
}
impl std::ops::Deref for DirectoryDevice {
    type Target = Listed;
    fn deref(&self) -> &Listed {
        &self.listed
    }
}
use crate::computer::{self as c, Transport};
mod crypto;
mod join;
mod sender;
use crypto::{open_pairing, open_request};
use sha2::{Digest, Sha256};
const INVALID: &str = "Computer request could not be verified.";
/// Every capability a runner signs into its presence. A sender accepts a presence only when it lists a subset of these.
const CAPABILITIES: [&str; 4] = ["changes", "projects", "settings", "start"];
fn part(value: &str) -> String {
    format!("{}:{value}", value.len())
}
fn message(domain: &str, scope: &c::Scope, values: &[String]) -> String {
    let mut out = format!("{domain}\n");
    for s in [&scope.gateway, &scope.tenant_id, &scope.person_id]
        .into_iter()
        .chain(values.iter())
    {
        out.push_str(&part(s));
    }
    out
}
fn hash(bytes: &[u8]) -> String {
    e2e::b64u(&Sha256::digest(bytes))
}
fn raw(text: &str) -> Result<Vec<u8>> {
    let b = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| INVALID)?;
    if e2e::b64u(&b) != text {
        return Err(INVALID.into());
    }
    Ok(b)
}
fn val(v: &Value, key: &str) -> Result<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or(INVALID.into())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    version: u8,
    scope: c::Scope,
    computer_id: String,
    id: String,
    sender_device_id: String,
    runner_device_id: String,
    generation: i64,
    issued_at: i64,
    expires_at: i64,
    locked_key: String,
    sealed: String,
    signature: String,
}
impl Envelope {
    fn message(&self) -> String {
        message(
            "nexus-computer-request-v1",
            &self.scope,
            &[
                self.computer_id.clone(),
                self.id.clone(),
                self.sender_device_id.clone(),
                self.runner_device_id.clone(),
                self.generation.to_string(),
                self.issued_at.to_string(),
                self.expires_at.to_string(),
                self.locked_key.clone(),
                self.sealed.clone(),
            ],
        )
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Payload {
    version: u8,
    kind: String,
    scope: c::Scope,
    computer_id: String,
    id: String,
    sender_device_id: String,
    runner_device_id: String,
    generation: i64,
    issued_at: i64,
    expires_at: i64,
    operation: c::Operation,
}
#[derive(PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairEnvelope {
    version: u8,
    scope: c::Scope,
    id: String,
    invite_id: String,
    inviter_device_id: String,
    sender_device_id: String,
    expires_at: i64,
    locked_key: String,
    sealed: String,
    signature: String,
}
impl PairEnvelope {
    fn message(&self) -> String {
        message(
            "nexus-phone-pairing-v1",
            &self.scope,
            &[
                self.id.clone(),
                self.invite_id.clone(),
                self.inviter_device_id.clone(),
                self.sender_device_id.clone(),
                self.expires_at.to_string(),
                self.locked_key.clone(),
                self.sealed.clone(),
            ],
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairPayload {
    version: u8,
    kind: String,
    scope: c::Scope,
    id: String,
    invite_id: String,
    computer_id: String,
    inviter_device_id: String,
    sender_device_id: String,
    expires_at: i64,
    secret: String,
    encryption_key: String,
    signing_key: String,
}
impl GatewayBackend {
    async fn listed(&self) -> Result<Vec<DirectoryDevice>> {
        let answer = self.client.get("/v1/devices").await.map_err(|e| refusal(&e))?;
        self.note_trust(&answer);
        serde_json::from_value(answer["devices"].clone()).map_err(|_| INVALID.into())
    }
    async fn own_device(&self) -> Result<(Vec<DirectoryDevice>, String)> {
        let listed = self.listed().await?;
        let id = listed
            .iter()
            .filter(|d| {
                d.encryption_key == e2e::b64u(&self.me.encryption)
                    && d.signing_key.as_deref() == Some(&e2e::b64u(&self.device.signing_key()))
            })
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
            .map(|d| d.id.clone())
            .ok_or(INVALID)?;
        Ok((listed, id))
    }
    fn seal_for(&self, id: &str, recipient: &Holder, value: &Value) -> Result<(String, String)> {
        let plain = serde_json::to_vec(value).map_err(|_| INVALID)?;
        if plain.len() > 256 * 1024 {
            return Err(INVALID.into());
        }
        let key = e2e::ContentKey::generate(0);
        Ok((
            e2e::b64u(
                &key.lock_for(id, &recipient.id, &recipient.keys.encryption)
                    .map_err(|_| INVALID)?,
            ),
            e2e::b64u(&key.seal_event(id, 0, &plain).map_err(|_| INVALID)?),
        ))
    }

    fn pairing_approval(&self, recipient: &Holder, trusted: &Trusted) -> Result<Value> {
        let encryption = &recipient.keys.encryption;
        let signing = &recipient.keys.signing.ok_or(INVALID)?;
        let message = e2e::approval_message(&recipient.id, encryption, signing).map_err(|_| INVALID)?;
        let revoked = self.keys.memory(&self.owner)?.revoked;
        if revoked.contains(&recipient.id) {
            return Err(INVALID.into());
        }
        let mut body = json!({"approverDeviceId":trusted.me.id,"signature":e2e::b64u(&self.device.sign(&message))});
        // A handover that exceeds the size limit just leaves the recipient with its approver and descendants.
        if let Ok(handover) = self.device.seal_handover(
            &recipient.id,
            &trusted.me.id,
            encryption,
            &trust::handover_payload(trusted, &revoked),
        ) {
            body["handover"] = json!(e2e::b64u(&handover));
        }
        Ok(body)
    }
}
impl Transport for GatewayBackend {
    fn seal_join(&self, invitation: c::Invitation, name: String) -> BoxFuture<'_, Result<c::Outbound>> {
        Box::pin(self.seal_computer_join(invitation, name))
    }

    fn poll_join(&self, outbound: c::Outbound) -> BoxFuture<'_, Result<(c::JoinStatus, String)>> {
        Box::pin(self.poll_computer_join(outbound))
    }

    fn admission_sender(&self, sender: String) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move {
            let trusted = self.directory().await?;
            Ok(trusted
                .holders
                .iter()
                .any(|h| h.id == sender && h.keys.signing.is_some()))
        })
    }

    fn device(&self) -> BoxFuture<'_, Result<c::Device>> {
        Box::pin(async move {
            if self.computer_scope.tenant_id.is_empty() {
                return Err(INVALID.into());
            }
            let (listed, id) = self.own_device().await?;
            let trusted = self.directory().await.is_ok();
            let name = listed
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.name.clone())
                .unwrap_or_default();
            Ok(c::Device {
                id,
                name,
                scope: self.computer_scope.clone(),
                trusted,
            })
        })
    }
    fn presence(&self, info: c::Info, enabled: bool) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            thread_path(&info.computer_id)?;
            let trusted = self.directory().await?;
            let now = crate::now_ms();
            let expires = now + 45_000;
            let payload = |caps: &[&str]| {
                let msg = message(
                    "nexus-computer-presence-v1",
                    &self.computer_scope,
                    &[
                        info.computer_id.clone(),
                        trusted.me.id.clone(),
                        info.name.clone(),
                        info.generation.to_string(),
                        now.to_string(),
                        expires.to_string(),
                        enabled.to_string(),
                        caps.join(","),
                    ],
                );
                json!({"version":1,"scope":self.computer_scope,"computerId":info.computer_id,"runnerDeviceId":trusted.me.id,"name":info.name,"generation":info.generation,"issuedAt":now,"expiresAt":expires,"enabled":enabled,"capabilities":caps,"signature":e2e::b64u(&self.device.sign(&msg))})
            };
            let path = format!("/v1/computers/{}/presence", info.computer_id);
            let result = self.client.put(&path, payload(&CAPABILITIES)).await;
            // An older Gateway rejects the new enum value. Preserve the earlier signed capability set there.
            let result = match result {
                Err(ClientError::Core(ref error))
                    if error.status == 400 && error.code.as_deref() == Some("invalid_request") =>
                {
                    self.client
                        .put(&path, payload(&["projects", "settings", "start"]))
                        .await
                }
                result => result,
            };
            result.map(|_| ()).map_err(|error| refusal(&error))
        })
    }
    fn requests<'a>(&'a self, computer: &'a str) -> BoxFuture<'a, Result<Vec<c::Request>>> {
        Box::pin(async move {
            thread_path(computer)?;
            let trusted = self.directory().await?;
            let page = self
                .client
                .get(&format!("/v1/computers/{computer}/requests/inbox"))
                .await
                .map_err(|e| refusal(&e))?;
            let mut result = Vec::new();
            for row in page["requests"].as_array().ok_or(INVALID)?.iter().take(20) {
                if row["state"] != "queued" {
                    continue;
                }
                let Ok(e) = serde_json::from_value::<Envelope>(row["envelope"].clone()) else {
                    continue;
                };
                let Some(expected_hash) = row["requestHash"].as_str() else {
                    continue;
                };
                if let Ok(request) = open_request(
                    e,
                    expected_hash,
                    &self.computer_scope,
                    computer,
                    &trusted,
                    &self.device,
                    crate::now_ms(),
                ) {
                    result.push(request);
                }
            }
            Ok(result)
        })
    }
    fn claim(&self, r: c::RequestTarget) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move {
            let trusted = self.directory().await?;
            let m = message(
                "nexus-computer-claim-v1",
                &self.computer_scope,
                &[r.computer_id.clone(), r.id.clone(), r.hash, trusted.me.id.clone()],
            );
            let answer = self
                .client
                .post(
                    &format!("/v1/computers/{}/requests/{}/claim", r.computer_id, r.id),
                    json!({"runnerDeviceId":trusted.me.id,"signature":e2e::b64u(&self.device.sign(&m))}),
                )
                .await
                .map_err(|e| refusal(&e))?;
            answer["claimed"].as_bool().ok_or(INVALID.into())
        })
    }
    fn seal_reply(&self, r: c::RequestTarget, outcome: c::Outcome) -> BoxFuture<'_, Result<c::Reply>> {
        Box::pin(async move {
            let trusted = self.directory().await?;
            let recipient = trusted
                .holders
                .iter()
                .find(|d| d.id == r.sender && d.keys.signing.is_some())
                .ok_or(INVALID)?;
            let payload = json!({"version":1,"kind":"computerReply","scope":self.computer_scope,"computerId":r.computer_id,"requestId":r.id,"requestHash":r.hash,"runnerDeviceId":trusted.me.id,"recipientDeviceId":r.sender,"result":outcome});
            let (locked, sealed) = self.seal_for(&r.id, recipient, &payload)?;
            let m = message(
                "nexus-computer-reply-v1",
                &self.computer_scope,
                &[
                    r.computer_id.clone(),
                    r.id.clone(),
                    r.hash.clone(),
                    trusted.me.id.clone(),
                    r.sender.clone(),
                    locked.clone(),
                    sealed.clone(),
                ],
            );
            Ok(c::Reply {
                computer_id: r.computer_id.clone(),
                request_id: r.id.clone(),
                body: json!({"version":1,"scope":self.computer_scope,"computerId":r.computer_id,"requestId":r.id,"requestHash":r.hash,"runnerDeviceId":trusted.me.id,"recipientDeviceId":r.sender,"lockedKey":locked,"sealed":sealed,"signature":e2e::b64u(&self.device.sign(&m))}),
            })
        })
    }
    fn deliver(&self, r: c::Reply) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let trusted = self.directory().await?;
            let recipient = val(&r.body, "recipientDeviceId")?;
            if !trusted
                .holders
                .iter()
                .any(|h| h.id == recipient && h.keys.signing.is_some())
            {
                return Err(INVALID.into());
            }
            match self
                .client
                .post(
                    &format!("/v1/computers/{}/requests/{}/reply", r.computer_id, r.request_id),
                    r.body,
                )
                .await
            {
                Ok(_) => Ok(()),
                Err(ClientError::Core(error))
                    if error.status == 404 && error.code.as_deref() == Some("computer_request_not_found") =>
                {
                    Ok(())
                }
                Err(error) => Err(refusal(&error)),
            }
        })
    }
    fn invitation(&self, info: c::Info, signed_decisions: bool) -> BoxFuture<'_, Result<c::Invitation>> {
        Box::pin(async move {
            let encryption = e2e::b64u(&self.device.encryption_key());
            let signing = e2e::b64u(&self.device.signing_key());
            let (_, id) = self.pairing_device(&info.name, &encryption, &signing).await?;
            let secret = e2e::b64u(&DeviceKey::generate().secret_bytes()[..32]);
            let mut i = c::Invitation {
                version: if signed_decisions { 2 } else { 1 },
                scope: self.computer_scope.clone(),
                invite_id: uuid::Uuid::new_v4().to_string(),
                computer_id: info.computer_id,
                computer_name: info.name,
                inviter_device_id: id,
                encryption_key: encryption,
                signing_key: signing,
                expires_at: crate::now_ms() + 300_000,
                secret,
                signature: String::new(),
            };
            let m = join::invitation_message(&i)?;
            i.signature = e2e::b64u(&self.device.sign(&m));
            Ok(i)
        })
    }
    fn pairings(&self, i: c::InvitationRecord) -> BoxFuture<'_, Result<Vec<c::Pairing>>> {
        Box::pin(async move {
            let (listed, me) = self.own_device().await?;
            if i.scope != self.computer_scope || i.inviter != me || i.expires_at <= crate::now_ms() {
                return Err(INVALID.into());
            }
            let page = self
                .client
                .get(&format!("/v1/devices/{me}/pairing-requests"))
                .await
                .map_err(|e| refusal(&e))?;
            let mut results = Vec::new();
            for row in page["requests"].as_array().ok_or(INVALID)?.iter().take(20) {
                if row["state"] != "pending" {
                    continue;
                }
                let Ok(e) = serde_json::from_value::<PairEnvelope>(row["envelope"].clone()) else {
                    continue;
                };
                if let Ok(pairing) = open_pairing(e, &i, &me, &listed, &self.device, crate::now_ms()) {
                    results.push(pairing);
                }
            }
            Ok(results)
        })
    }
    fn approve_pairing(&self, p: c::Pairing, admission: c::PairingAdmission) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if !admission.current() {
                return Err("The connection invitation was cancelled or expired.".into());
            }
            // The desktop just approved the phone that proved possession of this invitation.
            // Pin its keys only if it also signed this computer's approval.
            let listed: Vec<Listed> = self.listed().await?.into_iter().map(|d| d.listed).collect();
            if !admission.current() {
                return Err("The connection invitation was cancelled or expired.".into());
            }
            if let Ok((mine, Some(approver))) = trust::listed_approver(&listed, &self.me)
                && approver.id == p.sender
                && e2e::b64u(&approver.keys.encryption) == p.encryption_key
                && approver.keys.signing.map(|key| e2e::b64u(&key)).as_deref() == Some(p.signing_key.as_str())
            {
                self.keys.confirm_approver(&self.owner, &mine.id, &approver)?;
            }
            let trusted = self.directory().await?;
            if !admission.current() {
                return Err("The connection invitation was cancelled or expired.".into());
            }
            let listed = self.listed().await?;
            if !admission.current() {
                return Err("The connection invitation was cancelled or expired.".into());
            }
            let device = listed
                .iter()
                .find(|d| {
                    d.id == p.sender
                        && d.encryption_key == p.encryption_key
                        && d.signing_key.as_deref() == Some(p.signing_key.as_str())
                })
                .ok_or(INVALID)?;
            if device.state == "trusted" {
                if trusted.holders.iter().any(|h| h.id == p.sender) {
                    return Ok(());
                }
                return Err(INVALID.into());
            }
            if device.state != "pending" {
                return Err(INVALID.into());
            }
            let encryption: [u8; 32] = raw(&p.encryption_key)?.try_into().map_err(|_| INVALID)?;
            let signing: [u8; 32] = raw(&p.signing_key)?.try_into().map_err(|_| INVALID)?;
            let recipient = Holder {
                id: p.sender.clone(),
                keys: Keys {
                    encryption,
                    signing: Some(signing),
                },
            };
            let body = self.pairing_approval(&recipient, &trusted)?;
            if !admission.current() {
                return Err("The connection invitation was cancelled or expired.".into());
            }
            self.client
                .post(&format!("/v1/devices/{}/approve", p.sender), body)
                .await
                .map_err(|e| refusal(&e))?;
            Ok(())
        })
    }
    fn publish_pairing_decision(&self, invitation: c::InvitationRecord) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            thread_path(&invitation.invite_id)?;
            if invitation.scope != self.computer_scope || self.own_device().await?.1 != invitation.inviter {
                return Err(INVALID.into());
            }
            if !invitation.signed_decisions {
                let request = invitation.accepted_request.ok_or(INVALID)?;
                thread_path(&request)?;
                self.client
                    .post(
                        &format!("/v1/devices/{}/pairing-requests/{request}/finish", invitation.inviter),
                        json!({}),
                    )
                    .await
                    .map_err(|error| refusal(&error))?;
                return Ok(());
            }
            let (state, request_id, sender_device_id) = match invitation.state.as_str() {
                "connected" => (
                    "approved",
                    Some(invitation.accepted_request.ok_or(INVALID)?),
                    Some(invitation.accepted.ok_or(INVALID)?),
                ),
                "cancelled" => ("cancelled", None, None),
                _ => return Err(INVALID.into()),
            };
            let mut decision = c::PairingDecision {
                scope: invitation.scope,
                computer_id: invitation.computer_id,
                invite_id: invitation.invite_id,
                inviter_device_id: invitation.inviter,
                expires_at: invitation.expires_at,
                state: state.into(),
                request_id,
                sender_device_id,
                signature: String::new(),
            };
            decision.signature = e2e::b64u(&self.device.sign(&decision.message()));
            self.client
                .post(
                    &format!(
                        "/v1/devices/{}/pairing-requests/{}/finish",
                        decision.inviter_device_id, decision.invite_id
                    ),
                    json!({"decision": decision}),
                )
                .await
                .map_err(|e| refusal(&e))?;
            Ok(())
        })
    }
}
