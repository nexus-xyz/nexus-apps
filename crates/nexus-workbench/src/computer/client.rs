//! The local renderer's typed client. Only ciphertext is journaled; the native device key stays in transport.
use super::worker::{bounded, hash};
use super::*;
use crate::session_sync::{Backend, ContentPolicy, DeviceTrust};

/// Whether this computer may control another of the person's computers: in the pairing preview, or when Gateway
/// vouches for the person's devices.
fn desktop_control(preview: bool, trust: DeviceTrust) -> bool {
    preview || trust == DeviceTrust::Gateway
}

impl Api {
    pub(super) async fn computer_answer<T: Send>(
        &self,
        backend: &dyn Backend,
        call: BoxFuture<'_, Result<T>>,
    ) -> Result<T> {
        if !self.workbench().sync_current(backend) {
            return Err("Account changed.".into());
        }
        let result = bounded(call).await?;
        if !self.workbench().sync_current(backend) {
            return Err("Account changed.".into());
        }
        Ok(result)
    }

    pub(super) async fn computer_sender(&self) -> Result<Arc<dyn Backend>> {
        let backend = self.computer_backend()?;
        if !matches!(
            self.workbench().sync_policy(backend.as_ref()).await,
            Ok(ContentPolicy::UserOnly | ContentPolicy::UserAndOrgAdmins)
        ) {
            return Err("Encrypted sessions are not available in this organization.".into());
        }
        Ok(backend)
    }
    pub(super) async fn desktop_computer_sender(&self) -> Result<Arc<dyn Backend>> {
        let trust = self
            .computer_backend()
            .map_or(DeviceTrust::Approvals, |backend| backend.device_trust());
        if !desktop_control(self.workbench().config().remote_pairing_preview, trust) {
            return Err("Desktop-to-desktop control is unavailable.".into());
        }
        self.computer_sender().await
    }

    pub async fn computer_list(&self) -> Result<serde_json::Value> {
        let backend = self.desktop_computer_sender().await?;
        self.computer_answer(
            backend.as_ref(),
            backend.computer().ok_or("Computer transport unavailable.")?.computers(),
        )
        .await
    }
    pub async fn computer_request(
        &self,
        computer_id: String,
        request_id: String,
        operation: Operation,
    ) -> Result<CallResult> {
        let backend = self.desktop_computer_sender().await?;
        let transport = backend.computer().ok_or("Computer transport unavailable.")?;
        let digest = hash(&serde_json::to_vec(&(&computer_id, &operation)).map_err(|_| "Invalid computer request.")?);
        let stored = self.workbench().store().computer_outgoing(
            &backend.identity().scope,
            &backend.identity().user,
            "computer",
            &request_id,
        )?;
        let outbound = if let Some((old, outbound)) = stored {
            if old != digest || outbound.target != computer_id {
                return Err("This request ID already belongs to another start.".into());
            }
            outbound
        } else {
            let outbound = bounded(transport.seal_request(computer_id.clone(), request_id.clone(), operation)).await?;
            if !self.workbench().sync_current(backend.as_ref()) {
                return Err("Account changed.".into());
            }
            self.workbench().store().computer_outgoing_save(
                &backend.identity().scope,
                &backend.identity().user,
                &digest,
                &outbound,
            )?;
            let (saved, stored) = self
                .workbench()
                .store()
                .computer_outgoing(
                    &backend.identity().scope,
                    &backend.identity().user,
                    "computer",
                    &request_id,
                )?
                .ok_or("Computer request was not saved.")?;
            if saved != digest || stored.target != computer_id {
                return Err("This request ID already belongs to another start.".into());
            }
            stored
        };
        self.computer_answer(backend.as_ref(), transport.send_request(outbound))
            .await
    }
    pub async fn computer_request_get(&self, computer_id: String, request_id: String) -> Result<CallResult> {
        let backend = self.desktop_computer_sender().await?;
        let (_, outbound) = self
            .workbench()
            .store()
            .computer_outgoing(
                &backend.identity().scope,
                &backend.identity().user,
                "computer",
                &request_id,
            )?
            .ok_or("This computer request is not in this account's journal.")?;
        if outbound.target != computer_id {
            return Err("This request belongs to another computer.".into());
        }
        self.computer_answer(
            backend.as_ref(),
            backend
                .computer()
                .ok_or("Computer transport unavailable.")?
                .get_request(outbound),
        )
        .await
    }
    pub async fn synced_thread_page(
        &self,
        thread_id: String,
        before: Option<i64>,
        after: Option<i64>,
        limit: Option<usize>,
    ) -> Result<serde_json::Value> {
        let backend = self.desktop_computer_sender().await?;
        self.computer_answer(
            backend.as_ref(),
            backend
                .computer()
                .ok_or("Computer transport unavailable.")?
                .synced_page(thread_id, before, after, limit.unwrap_or(100).clamp(1, 100)),
        )
        .await
    }
    pub async fn synced_command(
        &self,
        thread_id: String,
        request_id: String,
        action: crate::session_sync::control::Action,
    ) -> Result<serde_json::Value> {
        if !action.valid() {
            return Err("Invalid session command.".into());
        }
        let backend = self.desktop_computer_sender().await?;
        let transport = backend.computer().ok_or("Computer transport unavailable.")?;
        let digest = hash(&serde_json::to_vec(&(&thread_id, &action)).map_err(|_| "Invalid session command.")?);
        let stored = self.workbench().store().computer_outgoing(
            &backend.identity().scope,
            &backend.identity().user,
            "thread",
            &request_id,
        )?;
        let outbound = if let Some((old, outbound)) = stored {
            if old != digest || outbound.target != thread_id {
                return Err("This request ID belongs to another command.".into());
            }
            outbound
        } else {
            let outbound = bounded(transport.seal_command(thread_id.clone(), request_id.clone(), action)).await?;
            if !self.workbench().sync_current(backend.as_ref()) {
                return Err("Account changed.".into());
            }
            self.workbench().store().computer_outgoing_save(
                &backend.identity().scope,
                &backend.identity().user,
                &digest,
                &outbound,
            )?;
            let (saved, outbound) = self
                .workbench()
                .store()
                .computer_outgoing(
                    &backend.identity().scope,
                    &backend.identity().user,
                    "thread",
                    &request_id,
                )?
                .ok_or("Session command was not saved.")?;
            if saved != digest {
                return Err("This request ID belongs to another command.".into());
            }
            outbound
        };
        self.computer_answer(backend.as_ref(), transport.send_command(outbound))
            .await
    }
    pub async fn synced_command_get(&self, thread_id: String, request_id: String) -> Result<serde_json::Value> {
        let backend = self.desktop_computer_sender().await?;
        let (_, outbound) = self
            .workbench()
            .store()
            .computer_outgoing(
                &backend.identity().scope,
                &backend.identity().user,
                "thread",
                &request_id,
            )?
            .ok_or("This session command is not in this account's journal.")?;
        if outbound.target != thread_id {
            return Err("This command belongs to another session.".into());
        }
        self.computer_answer(
            backend.as_ref(),
            backend
                .computer()
                .ok_or("Computer transport unavailable.")?
                .get_command(thread_id, request_id),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_controls_another_in_the_preview_or_under_gateway_trust() {
        assert!(!desktop_control(false, DeviceTrust::Approvals));
        assert!(desktop_control(true, DeviceTrust::Approvals));
        assert!(desktop_control(false, DeviceTrust::Gateway));
        assert!(desktop_control(true, DeviceTrust::Gateway));
    }
}
