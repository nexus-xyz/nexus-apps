//! Whether this computer follows Gateway's own device trust. With `trust: "gateway"`, Gateway makes every device it
//! lists trusted and turns Remote control on, so a computer that relied on approvals follows it only after the person
//! opts in here. Gateway's answer alone never moves it. The pin is in the app's session sync key store, which the
//! engine reads too (`KeyStore::settle_trust`). `view.rs` draws the card.

use nexus_workbench::session_sync::DeviceTrust as Followed;

use super::*;

impl DevicesPage {
    /// Gateway named its own trust: read whether this computer follows it. Until it does, the page stays in approvals.
    pub(super) fn settle_trust(&mut self, cx: &mut Context<Self>) {
        if self.offered != DeviceTrust::Gateway || self.settling {
            return;
        }
        self.settling = true;
        let gateway = self.ctx.gateway.url().to_string();
        let whoami = self.ctx.gateway.request(Verb::Get, "/v1/auth/whoami".into(), None);
        cx.spawn(async move |this, cx| {
            let settled = match whoami.await.and_then(|whoami| approver::owner(&gateway, &whoami)) {
                Ok(owner) => {
                    off_thread(move || {
                        let followed = approver::store()?
                            .settle_trust(&owner, Followed::Gateway)
                            .map_err(String::from)?;
                        Ok((owner, followed))
                    })
                    .await
                }
                Err(error) => Err(error),
            };
            this.update(cx, |page, cx| {
                page.settling = false;
                // A failed read keeps approvals: the next directory read tries again.
                if let (DeviceTrust::Gateway, Ok((owner, followed))) = (page.offered, settled) {
                    (page.trust, page.gateway_waits) = match followed {
                        Followed::Gateway => (DeviceTrust::Gateway, None),
                        Followed::Approvals => (DeviceTrust::Approvals, Some(owner)),
                    };
                }
                page.register_if_gateway(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The person chose, on this computer, to trust Gateway's device list.
    pub(super) fn trust_gateway(&mut self, cx: &mut Context<Self>) {
        let Some(owner) = self.gateway_waits.clone() else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let recorded = off_thread(move || approver::store()?.trust_gateway(&owner).map_err(String::from)).await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match recorded {
                    Ok(()) => {
                        page.gateway_waits = None;
                        page.trust = DeviceTrust::Gateway;
                        // Session sync reads the list again and follows Gateway's trust.
                        page.key_changed(cx);
                        page.register_if_gateway(cx);
                    }
                    Err(error) => {
                        page.outcome = Some((
                            Part::ThisDevice,
                            format!("Gateway's device list wasn't trusted: {error}"),
                            true,
                        ))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}
