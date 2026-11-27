use super::worker::hash;
use super::*;
use crate::session_sync::{Backend, control::Settings};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

pub(crate) const PAIRING_KIND: &str = "pairing";
const INVALID_LINK: &str = "Paste a connection link copied from Nexus Settings → Devices.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JoinState {
    Pending,
    Approved,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinStatus {
    pub invite_id: String,
    pub computer_id: String,
    pub computer_name: String,
    pub state: JoinState,
}

fn invitation_link(link: &str) -> Result<(Invitation, String)> {
    if link.len() > 16 * 1024 {
        return Err(INVALID_LINK.into());
    }
    let url = url::Url::parse(link.trim()).map_err(|_| INVALID_LINK)?;
    let body = url
        .fragment()
        .and_then(|f| f.strip_prefix("invite="))
        .ok_or(INVALID_LINK)?;
    let bytes = URL_SAFE_NO_PAD.decode(body).map_err(|_| INVALID_LINK)?;
    let invitation: Invitation = serde_json::from_slice(&bytes).map_err(|_| INVALID_LINK)?;
    if invitation.version != 2 {
        return Err("Ask the other computer to update Nexus and copy a new connection link.".into());
    }
    Ok((invitation, hash(&bytes)))
}

impl Api {
    pub async fn computer_join_begin(&self, link: String) -> Result<JoinStatus> {
        let (invitation, digest) = invitation_link(&link)?;
        let backend = self.desktop_computer_sender().await?;
        let transport = backend.computer().ok_or("Computer transport unavailable.")?;
        let identity = backend.identity();
        let wb = self.workbench();
        let info = wb.store().computer_info(&identity.scope, &identity.user)?;
        if invitation.computer_id == info.computer_id {
            return Err("Open this link on another computer.".into());
        }
        let invite_id = invitation.invite_id.clone();
        let computer_id = invitation.computer_id.clone();
        let saved = || {
            wb.store()
                .computer_outgoing(&identity.scope, &identity.user, PAIRING_KIND, &invite_id)
        };
        let (old_digest, outbound) = if let Some(row) = saved()? {
            row
        } else {
            let outbound = self
                .computer_answer(backend.as_ref(), transport.seal_join(invitation, info.name))
                .await?;
            wb.store()
                .computer_outgoing_save(&identity.scope, &identity.user, &digest, &outbound)?;
            saved()?.ok_or("Computer connection was not saved.")?
        };
        if old_digest != digest || outbound.target != computer_id {
            return Err("This connection link has changed. Ask the other computer for a new link.".into());
        }
        let (status, _) = self
            .computer_answer(backend.as_ref(), transport.poll_join(outbound))
            .await?;
        Ok(status)
    }

    pub async fn computer_join_poll(&self, invite_id: String) -> Result<JoinStatus> {
        let (_, status, _) = self.computer_join_receipt(invite_id).await?;
        Ok(status)
    }

    pub async fn computer_join_allow(&self, invite_id: String) -> Result<Settings> {
        let (backend, status, inviter) = self.computer_join_receipt(invite_id).await?;
        if status.state != JoinState::Approved {
            return Err("Finish linking the computers before allowing control here.".into());
        }
        let wb = self.workbench();
        if !wb.sync_current(backend.as_ref()) {
            return Err("Account changed.".into());
        }
        wb.store().control_grant(backend.identity(), &inviter)?;
        wb.code_control()
    }

    async fn computer_join_receipt(&self, invite_id: String) -> Result<(Arc<dyn Backend>, JoinStatus, String)> {
        let backend = self.desktop_computer_sender().await?;
        let transport = backend.computer().ok_or("Computer transport unavailable.")?;
        let identity = backend.identity();
        let (_, outbound) = self
            .workbench()
            .store()
            .computer_outgoing(&identity.scope, &identity.user, PAIRING_KIND, &invite_id)?
            .ok_or("Open the connection link again.")?;
        let (status, inviter) = self
            .computer_answer(backend.as_ref(), transport.poll_join(outbound))
            .await?;
        Ok((backend, status, inviter))
    }
}
