use super::worker::{bounded, hash};
use super::*;
use base64::Engine as _;
impl Api {
    pub(super) async fn publish_phone_pairing(
        &self,
        backend: &dyn crate::session_sync::Backend,
        record: InvitationRecord,
    ) -> Result<()> {
        let wb = self.workbench();
        if !wb.sync_current(backend) {
            return Err("Account changed.".into());
        }
        let transport = backend.computer().ok_or("Computer transport unavailable.")?;
        let id = record.invite_id.clone();
        bounded(transport.publish_pairing_decision(record)).await?;
        let identity = backend.identity();
        wb.store().phone_pairing_sent(&identity.scope, &identity.user, &id)
    }

    pub async fn phone_invitation_begin(&self) -> Result<Invitation> {
        let mut backend = self.computer_backend();
        for _ in 0..20 {
            if backend.is_ok() {
                break;
            }
            self.workbench().inner.host.refresh_session_sync();
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            backend = self.computer_backend();
        }
        backend?;
        let backend = self.computer_sender().await?;
        let transport = backend.computer().ok_or("Computer transport unavailable.")?;
        let signed_decisions = self.workbench().config().remote_pairing_preview;
        let invitation = bounded(transport.invitation(self.computer_info().await?, signed_decisions)).await?;
        if !self.workbench().sync_current(backend.as_ref()) {
            return Err("Account changed.".into());
        }
        let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&invitation.secret)
            .map_err(|_| "Invitation could not be made.")?;
        let record = InvitationRecord {
            signed_decisions,
            scope: invitation.scope.clone(),
            invite_id: invitation.invite_id.clone(),
            computer_id: invitation.computer_id.clone(),
            inviter: invitation.inviter_device_id.clone(),
            expires_at: invitation.expires_at,
            secret_hash: hash(&secret),
            state: "waiting".into(),
            accepted: None,
            accepted_encryption: None,
            accepted_signing: None,
            accepted_request: None,
            accepted_name: None,
        };
        self.workbench()
            .store()
            .phone_invitation_save(&backend.identity().scope, &backend.identity().user, &record)?;
        Ok(invitation)
    }
    pub async fn phone_invitation_cancel(&self, invite_id: String) -> Result<()> {
        let backend = self.computer_backend()?;
        let wb = self.workbench();
        let Some(mut i) =
            wb.store()
                .phone_invitation(&backend.identity().scope, &backend.identity().user, &invite_id)?
        else {
            return Ok(());
        };
        if matches!(i.state.as_str(), "connected" | "expired") {
            return Ok(());
        }
        if i.state != "cancelled" {
            let previous = i.clone();
            i.state = "cancelled".into();
            if !wb.store().phone_invitation_accept(
                &backend.identity().scope,
                &backend.identity().user,
                &previous,
                &i,
            )? {
                return Err("The connection changed. Try again.".into());
            }
        }
        if i.signed_decisions {
            self.publish_phone_pairing(backend.as_ref(), i).await?;
        }
        Ok(())
    }
    pub async fn phone_invitation_poll(&self, invite_id: String) -> Result<LinkStatus> {
        let backend = self.computer_backend()?;
        let transport = backend.computer().ok_or("Computer transport unavailable.")?;
        let wb = self.workbench();
        let identity = backend.identity();
        let mut i = wb
            .store()
            .phone_invitation(&identity.scope, &identity.user, &invite_id)?
            .ok_or("Invitation is no longer available.")?;
        if i.state == "connected" || (i.signed_decisions && i.state == "cancelled" && i.expires_at > crate::now_ms()) {
            self.publish_phone_pairing(backend.as_ref(), i.clone()).await?;
            return Ok(status(&i));
        }
        if matches!(i.state.as_str(), "cancelled" | "expired") {
            return Ok(status(&i));
        }
        if i.expires_at > crate::now_ms() && i.accepted.is_none() {
            let pairings = bounded(transport.pairings(i.clone())).await?;
            if !wb.sync_current(backend.as_ref()) {
                return Err("Account changed.".into());
            }
            for p in pairings {
                if p.secret_hash != i.secret_hash {
                    continue;
                }
                let mut chosen = i.clone();
                chosen.state = "approving".into();
                chosen.accepted = Some(p.sender);
                chosen.accepted_encryption = Some(p.encryption_key);
                chosen.accepted_signing = Some(p.signing_key);
                chosen.accepted_request = Some(p.id);
                chosen.accepted_name = Some(p.name);
                if wb
                    .store()
                    .phone_invitation_accept(&identity.scope, &identity.user, &i, &chosen)?
                {
                    i = chosen;
                    break;
                }
                i = wb
                    .store()
                    .phone_invitation(&identity.scope, &identity.user, &invite_id)?
                    .ok_or("Invitation is no longer available.")?;
                break;
            }
        }
        if matches!(i.state.as_str(), "connected" | "cancelled" | "expired") {
            return Ok(status(&i));
        }
        if i.expires_at <= crate::now_ms() {
            let previous = i.clone();
            i.state = "expired".into();
            if !wb
                .store()
                .phone_invitation_accept(&identity.scope, &identity.user, &previous, &i)?
            {
                i = wb
                    .store()
                    .phone_invitation(&identity.scope, &identity.user, &invite_id)?
                    .ok_or("Invitation is no longer available.")?;
                if i.state == "connected" {
                    self.publish_phone_pairing(backend.as_ref(), i.clone()).await?;
                } else if i.state != "cancelled" {
                    i.state = "expired".into();
                }
            }
        }
        Ok(status(&i))
    }
    pub async fn phone_invitation_approve(
        &self,
        invite_id: String,
        device_id: String,
        code: String,
    ) -> Result<LinkStatus> {
        let backend = self.computer_sender().await?;
        let transport = backend.computer().ok_or("Computer transport unavailable.")?;
        let wb = self.workbench();
        let identity = backend.identity();
        let mut i = wb
            .store()
            .phone_invitation(&identity.scope, &identity.user, &invite_id)?
            .ok_or("Invitation is no longer available.")?;
        if i.accepted.as_deref() != Some(device_id.as_str()) || match_code(&i).as_deref() != Some(code.as_str()) {
            return Err("The device changed. Check the request again before approving.".into());
        }
        if i.state == "connected" {
            self.publish_phone_pairing(backend.as_ref(), i.clone()).await?;
            return Ok(status(&i));
        }
        if i.signed_decisions && !wb.config().remote_pairing_preview {
            return Err("Open a new connection code on this computer.".into());
        }
        if i.state != "approving" || i.expires_at <= crate::now_ms() || !wb.sync_current(backend.as_ref()) {
            return Err("The connection invitation was cancelled or expired.".into());
        }
        if let (Some(sender), Some(encryption), Some(signing), Some(id)) = (
            &i.accepted,
            &i.accepted_encryption,
            &i.accepted_signing,
            &i.accepted_request,
        ) {
            let id = id.clone();
            if !wb.sync_current(backend.as_ref()) || i.expires_at <= crate::now_ms() {
                return Err("Invitation expired or account changed.".into());
            }
            let p = Pairing {
                id: id.clone(),
                sender: sender.clone(),
                name: i.accepted_name.clone().unwrap_or_default(),
                encryption_key: encryption.clone(),
                signing_key: signing.clone(),
                secret_hash: i.secret_hash.clone(),
            };
            let admission = PairingAdmission {
                workbench: wb.clone(),
                identity: identity.clone(),
                invitation: i.clone(),
            };
            bounded(transport.approve_pairing(p, admission)).await?;
            if !wb.sync_current(backend.as_ref()) {
                return Err("Account changed.".into());
            }
            let before = i.clone();
            i.state = "connected".into();
            if !wb
                .store()
                .phone_invitation_accept(&identity.scope, &identity.user, &before, &i)?
            {
                return Err("The connection changed while approval was in progress.".into());
            }
            self.publish_phone_pairing(backend.as_ref(), i.clone()).await?;
        }
        Ok(status(&i))
    }
}
fn status(i: &InvitationRecord) -> LinkStatus {
    LinkStatus {
        state: i.state.clone(),
        code: match_code(i),
        device_id: i.accepted.clone(),
        name: i.accepted_name.clone(),
    }
}

fn match_code(i: &InvitationRecord) -> Option<String> {
    let encryption: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(i.accepted_encryption.as_deref()?)
        .ok()?
        .try_into()
        .ok()?;
    let signing: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(i.accepted_signing.as_deref()?)
        .ok()?
        .try_into()
        .ok()?;
    Some(nexus_client::e2e::short_code(&nexus_client::e2e::fingerprint(
        &encryption,
        Some(&signing),
    )))
}
