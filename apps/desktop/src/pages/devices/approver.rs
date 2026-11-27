//! The approver this device's person confirms. Gateway names the device that approved this
//! one, and Gateway is not trusted for that. So the session engine locks no content key for it until the person
//! compares its code with the approver's own screen and presses Confirm. The confirmation goes in the app's session sync
//! key store, which the engine reads (`nexus_workbench::session_sync::approver`).

use nexus_workbench::session_sync::KeyStore;
use nexus_workbench::session_sync::approver::{self, Approval, Approver};

use super::*;

/// Where this device's approval stands, with the key store owner it was read for.
pub(super) type ApproverCheck = Load<(String, Approval)>;

/// The directory as the session engine reads it (`GET /v1/devices`'s `devices`).
fn listed(devices: &[Device]) -> Vec<Value> {
    devices
        .iter()
        .map(|device| {
            json!({
                "id": device.id,
                "name": device.name,
                "encryptionKey": device.encryption_key,
                "signingKey": device.signing_key,
                "state": device.state,
                "approvedBy": device.approved_by,
                "approvalSignature": device.approval_signature,
            })
        })
        .collect()
}

/// The app's session sync key store: the one the Code sessions' engine opens (`code_run.rs`).
pub(super) fn store() -> Result<KeyStore, String> {
    let data = crate::paths::data_dir().ok_or("This computer has no app data folder.")?;
    KeyStore::open(&data.join(nexus_desktop_core::session_sync::KEYS_FILE))
}

/// The person the sign-in is: the key store keeps one memory per person on each Gateway.
pub(super) fn owner(gateway: &str, whoami: &Value) -> Result<String, String> {
    let person = ui::text(whoami, "userId");
    if person.is_empty() {
        return Err("Session sync needs a person's sign-in.".into());
    }
    Ok(approver::owner(gateway, person))
}

/// The approver to confirm now, when there is one.
pub(super) fn unconfirmed(check: &ApproverCheck) -> Option<&Approver> {
    match check {
        Load::Ready((_, Approval::Unconfirmed(shown))) => Some(shown),
        _ => None,
    }
}

/// Whether this computer may lock content for its approver: true when confirmed, and for a root, which has none. None
/// while it is read, or when the read failed (the page reads it again, `watch_control`).
pub(super) fn approved(check: &ApproverCheck) -> Option<bool> {
    match check {
        Load::Ready((_, Approval::Root | Approval::Confirmed(_))) => Some(true),
        Load::Ready((_, Approval::Unconfirmed(_))) => Some(false),
        Load::Loading | Load::Failed(_) => None,
    }
}

/// What this computer hands `device` when it approves it: the holders its session engine trusts and the
/// revoked ids it remembers, signed and sealed to `device`. None when anything fails: the approval then carries none, and
/// the new device trusts only this computer and the devices below it.
pub(super) fn handover_for(
    gateway: &crate::workspace::gateway_access::AccountGateway,
    key: &std::sync::Arc<DeviceKey>,
    devices: &[Device],
    device: &Device,
) -> impl std::future::Future<Output = Option<String>> + use<> {
    let (key, devices) = (key.clone(), listed(devices));
    let (id, encryption_key) = (device.id.clone(), device.encryption_key.clone());
    let url = gateway.url().to_string();
    let whoami = gateway.request(Verb::Get, "/v1/auth/whoami".into(), None);
    async move {
        let owner = owner(&url, &whoami.await.ok()?).ok()?;
        off_thread(move || {
            approver::handover(&store()?, &owner, &devices, &key, &id, &encryption_key)
                .map_err(|error| error.message().to_string())
        })
        .await
        .ok()
        .flatten()
    }
}

impl DevicesPage {
    /// Reads where this device's approval stands, when it is trusted and has an approver. A root has none to confirm.
    pub(super) fn load_approver(&mut self, cx: &mut Context<Self>) {
        let (Load::Ready(key), Load::Ready(devices), Some(me)) = (&self.key, &self.devices, self.me()) else {
            return;
        };
        if status(me, now_ms()) != Status::Trusted || me.approved_by.is_none() {
            self.approver = Load::Ready((String::new(), Approval::Root));
            return;
        }
        let (key, devices) = (key.key.clone(), listed(devices));
        let gateway = self.ctx.gateway.url().to_string();
        let whoami = self.ctx.gateway.request(Verb::Get, "/v1/auth/whoami".into(), None);
        cx.spawn(async move |this, cx| {
            let checked = match whoami.await.and_then(|whoami| owner(&gateway, &whoami)) {
                Ok(owner) => {
                    off_thread(move || {
                        let approval = approver::approval(&store()?, &owner, &devices, &key)
                            .map_err(|error| error.message().to_string())?;
                        Ok((owner, approval))
                    })
                    .await
                }
                Err(error) => Err(error),
            };
            this.update(cx, |page, cx| {
                page.approver = match checked {
                    Ok(checked) => Load::Ready(checked),
                    Err(error) => Load::Failed(error.into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The person saw the approver's code on the approver's screen: record it, so this computer syncs.
    pub(super) fn confirm_approver(&mut self, cx: &mut Context<Self>) {
        let (Load::Ready(key), Load::Ready(devices), Load::Ready((owner, Approval::Unconfirmed(shown)))) =
            (&self.key, &self.devices, &self.approver)
        else {
            return;
        };
        if self.busy {
            return;
        }
        let (key, devices, owner, id) = (key.key.clone(), listed(devices), owner.clone(), shown.id.clone());
        self.busy = true;
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let recorded = off_thread(move || {
                let shown = approver::confirm(&store()?, &owner, &devices, &key, &id)
                    .map_err(|error| error.message().to_string())?;
                Ok((owner, shown))
            })
            .await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match recorded {
                    Ok((owner, shown)) => {
                        page.outcome = Some((
                            Part::ThisDevice,
                            format!("Confirmed. This computer's sessions can sync to {} now.", shown.name),
                            false,
                        ));
                        page.approver = Load::Ready((owner, Approval::Confirmed(shown)));
                    }
                    Err(error) => {
                        page.outcome = Some((
                            Part::ThisDevice,
                            format!("The approver wasn't confirmed: {error}"),
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

#[cfg(test)]
mod tests {
    // Named, not `super::*`: the GPUI prelude's `test` macro would replace the standard one.
    use super::{
        Approval, ApproverCheck, Device, DeviceKey, KeyStore, Load, approved, approver, device_code, e2e, json, listed,
        owner, sign_approval, unconfirmed,
    };

    #[test]
    fn a_root_and_a_confirmed_approver_are_ready_and_an_unconfirmed_one_is_not() {
        let shown = approver::Approver {
            id: "dev_a".into(),
            name: "Device dev_a".into(),
            code: "ABCD EFGH IJKL MNOP".into(),
        };
        let ready = |approval| approved(&Load::Ready((String::new(), approval)));
        assert_eq!(ready(Approval::Root), Some(true));
        assert_eq!(ready(Approval::Confirmed(shown.clone())), Some(true));
        assert_eq!(ready(Approval::Unconfirmed(shown)), Some(false));
        assert_eq!(approved(&Load::Loading), None);
        assert_eq!(approved(&Load::Failed("offline".into())), None);
    }

    #[test]
    fn the_engine_reads_the_page_s_directory_and_its_owner() {
        let a = DeviceKey::generate();
        let b = DeviceKey::generate();
        let row = |id: &str, key: &DeviceKey| Device {
            id: id.into(),
            name: format!("Device {id}"),
            encryption_key: e2e::b64u(&key.encryption_key()),
            signing_key: e2e::b64u(&key.signing_key()),
            state: "trusted".into(),
            approved_by: None,
            approval_signature: None,
            created_at: "2026-11-05T10:00:00Z".into(),
            approved_at: None,
            revoked_at: None,
        };
        let mut second = row("dev_b", &b);
        second.approval_signature = Some(sign_approval(&a, &second, &device_code(&second)).expect("signed"));
        second.approved_by = Some("dev_a".into());
        let devices = listed(&[row("dev_a", &a), second]);
        let keys = KeyStore::in_memory().expect("store");
        assert!(owner("https://gw.test", &json!({ "kind": "service_account" })).is_err());
        let owner = owner("https://gw.test/", &json!({ "userId": "user_ada" })).expect("owner");
        assert_eq!(owner, "https://gw.test|user_ada");
        let check: ApproverCheck = Load::Ready((
            owner.clone(),
            approver::approval(&keys, &owner, &devices, &b).expect("approval"),
        ));
        let shown = unconfirmed(&check).expect("waits for the person");
        assert_eq!(
            (shown.id.as_str(), shown.name.as_str(), shown.code.as_str()),
            ("dev_a", "Device dev_a", e2e::short_code(&a.fingerprint()).as_str())
        );
        approver::confirm(&keys, &owner, &devices, &b, "dev_a").expect("recorded");
        assert!(matches!(
            approver::approval(&keys, &owner, &devices, &b),
            Ok(Approval::Confirmed(_))
        ));
    }
}
