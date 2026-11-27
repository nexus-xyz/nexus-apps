//! The joining computer submits the same sealed scan proof as the phone.
use super::*;
use c::{Invitation, JoinState, JoinStatus, Outbound, PairingDecision};

const BAD_INVITATION: &str = "The connection link could not be verified. Ask the other computer for a new link.";
const EXPIRED: &str = "The connection link expired. Ask the other computer for a new link.";
const LOCAL_SETUP_EXPIRED: &str = "This computer’s setup expired. Revoke its old entry in Devices on a trusted device, then choose Make a new key here.";
const REMOTE_SETUP_EXPIRED: &str = "The other computer’s setup expired. Revoke its old entry in Devices on a trusted device, then choose Make a new key on that computer.";
// Gateway's deadline for approving a pending device in the account directory.
const DEVICE_SETUP_TTL_MS: i64 = 15 * 60 * 1000;

impl DirectoryDevice {
    fn setup_expired(&self) -> bool {
        self.state == "pending"
            && chrono::DateTime::parse_from_rfc3339(&self.created_at)
                .is_ok_and(|at| crate::now_ms().saturating_sub(at.timestamp_millis()) >= DEVICE_SETUP_TTL_MS)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JoinAttempt {
    computer_id: String,
    computer_name: String,
    encryption_key: String,
    signing_key: String,
    pending_inviter: bool,
    envelope: PairEnvelope,
}

#[derive(Deserialize)]
struct Receipt {
    envelope: PairEnvelope,
    state: String,
    decision: Option<PairingDecision>,
}

impl JoinAttempt {
    fn recipient(&self) -> Result<Holder> {
        Ok(Holder {
            id: self.envelope.inviter_device_id.clone(),
            keys: Keys {
                encryption: raw(&self.encryption_key)?.try_into().map_err(|_| INVALID)?,
                signing: Some(raw(&self.signing_key)?.try_into().map_err(|_| INVALID)?),
            },
        })
    }

    fn finish(&self, state: JoinState) -> JoinStatus {
        JoinStatus {
            invite_id: self.envelope.invite_id.clone(),
            computer_id: self.computer_id.clone(),
            computer_name: self.computer_name.clone(),
            state,
        }
    }

    fn decision(&self, receipt: Receipt) -> Result<JoinState> {
        let sent = &self.envelope;
        if receipt.envelope != *sent {
            return Err(INVALID.into());
        }
        let Some(d) = receipt.decision else {
            return match receipt.state.as_str() {
                "pending" if sent.expires_at <= crate::now_ms() => Ok(JoinState::Expired),
                "pending" => Ok(JoinState::Pending),
                _ => Err(INVALID.into()),
            };
        };
        let signing = self.recipient()?.keys.signing.ok_or(INVALID)?;
        if d.scope != sent.scope
            || d.computer_id != self.computer_id
            || d.invite_id != sent.invite_id
            || d.inviter_device_id != sent.inviter_device_id
            || d.expires_at != sent.expires_at
            || !e2e::verify(&signing, &d.message(), &raw(&d.signature)?)
        {
            return Err(INVALID.into());
        }
        match d.state.as_str() {
            "approved"
                if receipt.state == "finished"
                    && d.request_id.as_deref() == Some(&sent.id)
                    && d.sender_device_id.as_deref() == Some(&sent.sender_device_id) =>
            {
                Ok(JoinState::Approved)
            }
            "cancelled" if d.request_id.is_none() && d.sender_device_id.is_none() => Ok(JoinState::Cancelled),
            _ => Err(INVALID.into()),
        }
    }
}

impl GatewayBackend {
    pub(super) async fn seal_computer_join(&self, invite: Invitation, name: String) -> Result<Outbound> {
        if invite.version != 2 || invite.scope != self.computer_scope {
            return Err("Sign in to the same account and organization on both computers.".into());
        }
        if invite.expires_at <= crate::now_ms() {
            return Err(EXPIRED.into());
        }
        thread_path(&invite.invite_id).map_err(|_| BAD_INVITATION)?;
        thread_path(&invite.computer_id).map_err(|_| BAD_INVITATION)?;
        let secret = raw(&invite.secret)?;
        let encryption: [u8; 32] = raw(&invite.encryption_key)?.try_into().map_err(|_| BAD_INVITATION)?;
        let signing: [u8; 32] = raw(&invite.signing_key)?.try_into().map_err(|_| BAD_INVITATION)?;
        if secret.len() != 32 || !e2e::verify(&signing, &invitation_message(&invite)?, &raw(&invite.signature)?) {
            return Err(BAD_INVITATION.into());
        }
        let recipient = Holder {
            id: invite.inviter_device_id.clone(),
            keys: Keys {
                encryption,
                signing: Some(signing),
            },
        };
        let encryption_key = e2e::b64u(&self.me.encryption);
        let signing_key = e2e::b64u(&self.device.signing_key());
        let (listed, sender_id) = self.pairing_device(&name, &encryption_key, &signing_key).await?;
        if sender_id == recipient.id {
            return Err("Open this link on another computer.".into());
        }
        let inviter = listed
            .iter()
            .find(|d| {
                d.id == recipient.id
                    && d.kind == "device"
                    && d.encryption_key == invite.encryption_key
                    && d.signing_key.as_deref() == Some(invite.signing_key.as_str())
            })
            .ok_or(BAD_INVITATION)?;
        if self.keys.memory(&self.owner)?.revoked.contains(&recipient.id) {
            return Err(BAD_INVITATION.into());
        }
        if inviter.setup_expired() {
            return Err(REMOTE_SETUP_EXPIRED.into());
        }
        let pending_inviter = match inviter.state.as_str() {
            "pending" => {
                self.directory().await.map_err(|error| match error.as_str() {
                    trust::NOT_TRUSTED | trust::APPROVER_UNCONFIRMED => {
                        "Open this link on a trusted device, or create a link from a trusted computer.".into()
                    }
                    _ => error,
                })?;
                true
            }
            "trusted" => false,
            _ => return Err(BAD_INVITATION.into()),
        };
        let id = uuid::Uuid::new_v4().to_string();
        let payload = PairPayload {
            version: 1,
            kind: "phonePairing".into(),
            scope: invite.scope.clone(),
            id: id.clone(),
            invite_id: invite.invite_id.clone(),
            computer_id: invite.computer_id.clone(),
            inviter_device_id: inviter.id.clone(),
            sender_device_id: sender_id.clone(),
            expires_at: invite.expires_at,
            secret: invite.secret,
            encryption_key,
            signing_key,
        };
        let value = serde_json::to_value(payload).map_err(|_| INVALID)?;
        let (locked_key, sealed) = self.seal_for(&id, &recipient, &value)?;
        let mut envelope = PairEnvelope {
            version: 1,
            scope: invite.scope,
            id,
            invite_id: invite.invite_id.clone(),
            inviter_device_id: inviter.id.clone(),
            sender_device_id: sender_id,
            expires_at: invite.expires_at,
            locked_key,
            sealed,
            signature: String::new(),
        };
        envelope.signature = e2e::b64u(&self.device.sign(&envelope.message()));
        let attempt = JoinAttempt {
            computer_id: invite.computer_id.clone(),
            computer_name: invite.computer_name,
            encryption_key: invite.encryption_key,
            signing_key: invite.signing_key,
            pending_inviter,
            envelope,
        };
        Ok(Outbound {
            id: invite.invite_id,
            target: invite.computer_id,
            kind: c::PAIRING_KIND.into(),
            body: serde_json::to_value(attempt).map_err(|_| INVALID)?,
            expires_at: invite.expires_at,
        })
    }

    pub(super) async fn poll_computer_join(&self, outbound: Outbound) -> Result<(JoinStatus, String)> {
        let attempt: JoinAttempt = serde_json::from_value(outbound.body).map_err(|_| INVALID)?;
        let envelope = &attempt.envelope;
        let recipient = attempt.recipient()?;
        if envelope.scope != self.computer_scope
            || !recipient
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(INVALID.into());
        }
        let answer = self
            .client
            .post(
                &format!("/v1/devices/{}/pairing-requests", recipient.id),
                serde_json::to_value(envelope).map_err(|_| INVALID)?,
            )
            .await;
        let answer = match answer {
            Ok(answer) => answer,
            Err(ClientError::Core(error)) if error.code.as_deref() == Some("phone_invitation_expired") => {
                return Ok((attempt.finish(JoinState::Expired), recipient.id));
            }
            Err(ClientError::Core(error)) if error.code.as_deref() == Some("phone_pairing_conflict") => {
                return Err("This connection link was already used. Ask the other computer for a new link.".into());
            }
            Err(error) => return Err(refusal(&error)),
        };
        let state = attempt.decision(serde_json::from_value(answer).map_err(|_| INVALID)?)?;
        if state == JoinState::Pending && attempt.pending_inviter {
            self.approve_inviting_computer(&recipient, envelope).await?;
        }
        if state == JoinState::Approved {
            self.confirm_joined_computer(&recipient, &envelope.sender_device_id)
                .await?;
        }
        Ok((attempt.finish(state), recipient.id))
    }

    async fn confirm_joined_computer(&self, peer: &Holder, sender_id: &str) -> Result<()> {
        let listed: Vec<Listed> = self.listed().await?.into_iter().map(|d| d.listed).collect();
        let memory = self.keys.memory(&self.owner)?;
        if memory.revoked.contains(&peer.id) {
            return Err(INVALID.into());
        }
        if let Ok((mine, Some(approver))) = trust::listed_approver(&listed, &self.me)
            && &approver == peer
            && mine.id == sender_id
            && !memory.anchors.contains_key(&mine.id)
        {
            self.keys.confirm_approver(&self.owner, &mine.id, &approver)?;
        }
        let trusted = self.directory().await?;
        if trusted.me.id != sender_id || !trusted.holders.contains(peer) {
            return Err("The connected computer is no longer trusted.".into());
        }
        Ok(())
    }

    pub(super) async fn pairing_device(
        &self,
        name: &str,
        encryption: &str,
        signing: &str,
    ) -> Result<(Vec<DirectoryDevice>, String)> {
        let devices = self.listed().await?;
        if !devices
            .iter()
            .any(|d| d.encryption_key == encryption && d.signing_key.as_deref() == Some(signing))
        {
            self.client
                .post(
                    "/v1/devices",
                    json!({"name":name,"encryptionKey":encryption,"signingKey":signing}),
                )
                .await
                .map_err(|e| refusal(&e))?;
        }
        let (listed, device_id) = self.own_device().await?;
        if listed
            .iter()
            .any(|device| device.id == device_id && device.setup_expired())
        {
            return Err(LOCAL_SETUP_EXPIRED.into());
        }
        Ok((listed, device_id))
    }

    async fn approve_inviting_computer(&self, recipient: &Holder, envelope: &PairEnvelope) -> Result<()> {
        let (listed, sender_id) = self.own_device().await?;
        if sender_id != envelope.sender_device_id {
            return Err(INVALID.into());
        }
        let inviter = listed.iter().find(|device| device.id == recipient.id).ok_or(INVALID)?;
        if inviter.state == "trusted" {
            return Ok(());
        }
        if inviter.setup_expired() {
            return Err(REMOTE_SETUP_EXPIRED.into());
        }
        if inviter.state != "pending" || envelope.expires_at <= crate::now_ms() {
            return Err(BAD_INVITATION.into());
        }
        let trusted = self.directory().await?;
        if trusted.me.id != sender_id {
            return Err(INVALID.into());
        }
        let body = self.pairing_approval(recipient, &trusted)?;
        if envelope.expires_at <= crate::now_ms() {
            return Err(EXPIRED.into());
        }
        match self
            .client
            .post(&format!("/v1/devices/{}/approve", recipient.id), body)
            .await
        {
            Ok(_) => Ok(()),
            Err(ClientError::Core(error)) if error.code.as_deref() == Some("device_not_pending") => {
                if self.directory().await?.holders.contains(recipient) {
                    Ok(())
                } else {
                    Err(BAD_INVITATION.into())
                }
            }
            Err(ClientError::Core(error)) if error.code.as_deref() == Some("pairing_expired") => {
                Err(REMOTE_SETUP_EXPIRED.into())
            }
            Err(error) => Err(refusal(&error)),
        }
    }
}

pub(super) fn invitation_message(i: &Invitation) -> Result<String> {
    let domain = match i.version {
        1 => "nexus-phone-invitation-v1",
        2 => "nexus-phone-invitation-v2",
        _ => return Err(INVALID.into()),
    };
    Ok(message(
        domain,
        &i.scope,
        &[
            i.invite_id.clone(),
            i.computer_id.clone(),
            i.computer_name.clone(),
            i.inviter_device_id.clone(),
            i.encryption_key.clone(),
            i.signing_key.clone(),
            i.expires_at.to_string(),
            hash(&raw(&i.secret)?),
        ],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inviting_computer_must_sign_the_exact_join_approval() {
        let host = DeviceKey::generate();
        let other_device = DeviceKey::generate();
        let scope = c::Scope {
            gateway: "https://gateway.example".into(),
            tenant_id: "tenant".into(),
            person_id: "person".into(),
        };
        let attempt = JoinAttempt {
            computer_id: "computer".into(),
            computer_name: "Studio".into(),
            encryption_key: e2e::b64u(&host.encryption_key()),
            signing_key: e2e::b64u(&host.signing_key()),
            pending_inviter: false,
            envelope: PairEnvelope {
                version: 1,
                scope: scope.clone(),
                id: "this-request".into(),
                invite_id: "invite".into(),
                inviter_device_id: "inviter".into(),
                sender_device_id: "sender".into(),
                expires_at: 100_000,
                locked_key: "encrypted-key".into(),
                sealed: "encrypted-body".into(),
                signature: "signed-request".into(),
            },
        };
        let receipt = |signer: &DeviceKey, request: &str| {
            let mut decision = PairingDecision {
                scope: scope.clone(),
                computer_id: "computer".into(),
                invite_id: "invite".into(),
                inviter_device_id: "inviter".into(),
                expires_at: 100_000,
                state: "approved".into(),
                request_id: Some(request.into()),
                sender_device_id: Some("sender".into()),
                signature: String::new(),
            };
            decision.signature = e2e::b64u(&signer.sign(&decision.message()));
            serde_json::from_value(json!({
                "envelope": attempt.envelope,
                "state": "finished",
                "decision": decision,
            }))
            .expect("wire receipt")
        };

        assert_eq!(
            attempt.decision(receipt(&host, "this-request")),
            Ok(JoinState::Approved)
        );
        assert!(attempt.decision(receipt(&host, "another-request")).is_err());
        assert!(attempt.decision(receipt(&other_device, "this-request")).is_err());
    }
}
