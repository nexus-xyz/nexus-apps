//! The organization's recovery key, as this computer pins it.
//! Under "The person and the org", this computer's sessions are locked for the org's key too. Gateway serves that key,
//! and Gateway is not trusted for it. So the session engine locks for it only after the person compares its
//! fingerprint with the one computed from your vault on the admin's machine, and presses Confirm. The fingerprint is computed on this
//! computer from the served key. The pin goes in the app's session sync key store, which the engine reads
//! (`nexus_workbench::session_sync::org_key`). A changed key waits for a new Confirm. `view.rs` draws the card, with
//! the copy every shell shares (`org_key::copy`).

use nexus_workbench::session_sync::org_key::{self, OrgKeyState, copy};

use super::*;

/// Where the org's key stands here, with what it was read for: the key store owner, the organization and Gateway's
/// answer, which a confirmation checks again.
pub(super) type OrgKeyCheck = Load<(String, String, Value, OrgKeyState)>;

/// The organization the sign-in is in.
fn tenant(whoami: &Value) -> Result<String, String> {
    let tenant = ui::text(whoami, "tenantId");
    if tenant.is_empty() {
        return Err("Session sync needs a sign-in to an organization.".into());
    }
    Ok(tenant.to_string())
}

impl DevicesPage {
    /// Reads the org's key and where it stands on this computer, once this device is trusted.
    pub(super) fn load_org_key(&mut self, cx: &mut Context<Self>) {
        if !self.me().is_some_and(|me| status(me, now_ms()) == Status::Trusted) {
            return;
        }
        let gateway = self.ctx.gateway.url().to_string();
        let whoami = self.ctx.gateway.request(Verb::Get, "/v1/auth/whoami".into(), None);
        let key = self
            .ctx
            .gateway
            .request(Verb::Get, "/v1/orgs/recovery-key".into(), None);
        cx.spawn(async move |this, cx| {
            let read = match (whoami.await, key.await) {
                (Ok(whoami), Ok(answer)) => match (approver::owner(&gateway, &whoami), tenant(&whoami)) {
                    (Ok(owner), Ok(tenant)) => {
                        off_thread(move || {
                            let state = org_key::state(&approver::store()?, &owner, &tenant, &answer)
                                .map_err(|error| error.message().to_string())?;
                            Ok((owner, tenant, answer, state))
                        })
                        .await
                    }
                    (Err(error), _) | (_, Err(error)) => Err(error),
                },
                (Err(error), _) | (_, Err(error)) => Err(error),
            };
            this.update(cx, |page, cx| {
                page.org_key = match read {
                    Ok(read) => Load::Ready(read),
                    Err(error) => Load::Failed(error.into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The person compared the fingerprint with their org admin's: record it, so this computer locks for the key.
    pub(super) fn confirm_org_key(&mut self, cx: &mut Context<Self>) {
        let Load::Ready((owner, tenant, answer, state)) = &self.org_key else {
            return;
        };
        let shown = match state {
            OrgKeyState::Unconfirmed(shown) | OrgKeyState::Changed { served: shown, .. } => shown.clone(),
            OrgKeyState::None | OrgKeyState::Confirmed(_) => return,
        };
        if self.busy {
            return;
        }
        let (owner, tenant, answer) = (owner.clone(), tenant.clone(), answer.clone());
        self.busy = true;
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let recorded = off_thread(move || {
                let shown = org_key::confirm(&approver::store()?, &owner, &tenant, &answer, &shown.fingerprint)
                    .map_err(|error| error.message().to_string())?;
                Ok((owner, tenant, answer, shown))
            })
            .await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match recorded {
                    Ok((owner, tenant, answer, shown)) => {
                        page.outcome = Some((Part::OrgKey, copy::CONFIRMED.into(), false));
                        page.org_key = Load::Ready((owner, tenant, answer, OrgKeyState::Confirmed(shown)));
                    }
                    Err(error) => page.outcome = Some((Part::OrgKey, error, true)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: the GPUI prelude's `test` macro would replace the standard one.
    use super::{OrgKeyState, e2e, json, org_key, tenant};
    use nexus_workbench::session_sync::KeyStore;

    #[test]
    fn the_engine_reads_the_page_s_confirmation_for_the_sign_in_s_organization() {
        assert!(tenant(&json!({ "userId": "user_ada" })).is_err());
        let tenant = tenant(&json!({ "userId": "user_ada", "tenantId": "ten_acme" })).expect("tenant");
        let keys = KeyStore::in_memory().expect("store");
        let answer = json!({ "key": { "id": "ork_one", "encryptionKey": e2e::b64u(&[4; 32]) } });
        let OrgKeyState::Unconfirmed(shown) = org_key::state(&keys, "o", &tenant, &answer).expect("state") else {
            panic!("waits for the person");
        };
        assert_eq!(shown.fingerprint, e2e::org_key_fingerprint(&[4; 32]));
        org_key::confirm(&keys, "o", &tenant, &answer, &shown.fingerprint).expect("recorded");
        assert!(matches!(
            org_key::state(&keys, "o", &tenant, &answer),
            Ok(OrgKeyState::Confirmed(_))
        ));
    }
}
