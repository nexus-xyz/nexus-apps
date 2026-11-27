//! The organization's recovery key on the runner.
//! Gateway's side is the key directory's organization key and copies. The pin
//! and its rules are [`super::super::org_key`]; this file reads the served key and stores the copies.
//!
//! - **Lock.** Under `user_and_org`, each epoch of a thread's content key that this computer holds is locked to the
//!   organization's key with its id as the holder id, signed by this device, and stored. A thread key's record names
//!   the copy by that id, so an epoch is locked once, and a key confirmed later gets the whole history.
//! - **Only the pinned key.** The runner locks only when Gateway serves the key the person confirmed here, with the
//!   same id and bytes. Otherwise it locks nothing for the organization, and a key locked for another organization key
//!   rotates.
//! - **No warning here.** The runner prints nothing: the CLI syncs beside an agent's terminal screen. Each shell that
//!   confirms the key shows a changed key as a warning (`org_key::state`).

use std::collections::BTreeSet;

use nexus_client::e2e;
use serde_json::json;

use super::super::ContentPolicy;
use super::super::keystore::{Audience, Recipient};
use super::super::org_key::{self, OrgKey, OrgLock};
use super::super::trust::Holder;
use super::{CRYPTO, GatewayBackend, refusal, team};

impl GatewayBackend {
    /// What a thread's key is locked for besides the person's holders, under `policy`.
    pub(super) async fn org_lock(&self, policy: ContentPolicy) -> Result<OrgLock, String> {
        if policy != ContentPolicy::UserAndOrgAdmins {
            return Ok(OrgLock::Off);
        }
        let answer = self
            .client
            .get("/v1/orgs/recovery-key")
            .await
            .map_err(|error| refusal(&error))?;
        let served = org_key::served(&answer)?;
        let pinned = self.keys.pinned_org_key(&self.owner, &self.computer_scope.tenant_id)?;
        Ok(OrgLock::of(pinned.as_ref(), served.as_ref()))
    }

    /// Locks every epoch of `thread` this computer holds, and has not locked for `org` yet, to `org`, signs each copy,
    /// and stores it (`PUT /v1/threads/{threadId}/org-key`). `current` is the epoch in use. A copy Gateway refuses
    /// because the organization replaced its key meanwhile is not recorded: the next pass reads the new key, which waits
    /// for the person.
    pub(super) async fn lock_to_org(
        &self,
        thread: &str,
        current: &e2e::ContentKey,
        me: &Holder,
        org: &OrgKey,
    ) -> Result<(), String> {
        for stored in self.keys.all(thread)? {
            if stored.holders.contains(&org.id) {
                continue;
            }
            let opened;
            let key = if stored.epoch == current.epoch() {
                current
            } else {
                opened = self.open(thread, &stored)?;
                &opened
            };
            let locked = key.lock_for(thread, &org.id, &org.encryption).map_err(|_| CRYPTO)?;
            let message = e2e::org_thread_key_message(thread, key.epoch(), &org.id, &locked).map_err(|_| CRYPTO)?;
            let body = json!({
                "keyVersion": e2e::key_version(key.epoch()).map_err(|_| CRYPTO)?,
                "orgKeyId": org.id,
                "lockedKey": e2e::b64u(&locked),
                "senderDeviceId": me.id,
                "signature": e2e::b64u(&self.device.sign(&message)),
            });
            self.keys.record_recipients(
                thread,
                key.epoch(),
                &[Recipient::shared(Audience::Organization, &org.id, &org.encryption)],
            )?;
            match self.client.put(&format!("/v1/threads/{thread}/org-key"), body).await {
                Ok(_) => {}
                Err(error) if team::conflict(&error, &["org_key_replaced", "org_recovery_key_required"]) => {
                    return Ok(());
                }
                Err(error) => return Err(refusal(&error)),
            }
            self.keys
                .add_holders(thread, stored.epoch, &BTreeSet::from([org.id.clone()]))?;
        }
        Ok(())
    }
}
