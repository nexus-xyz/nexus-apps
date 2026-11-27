//! A computer registers itself after a sign-in when Gateway vouches for the person's devices (`trust: gateway`). The
//! person does not need to open Settings › Devices: the key is made, registered and handed to session sync here. In
//! approvals mode nothing happens, and the keychain is not read.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::{App, Task};
use nexus_client::e2e::{self, DeviceKey};
use serde_json::json;

use super::gateway_trust::{DeviceTrust, read_device_trust};
use super::{Verb, load_or_create, off_thread, read_devices, this_device};
use crate::workspace::gateway_access::AccountGateway;

/// What a sign-in did with this computer's registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Registration {
    /// Gateway wants approvals: nothing changed.
    Approvals,
    /// The organization keeps encrypted session sync off on this computer: nothing changed.
    Managed,
    /// Gateway lists this computer's key already, in any state: nothing changed.
    Listed,
    /// This computer registered now.
    Registered,
}

/// Every window hears a sign-in. One registration runs at a time, so two windows never make two keys.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// After a sign-in: in gateway mode, make this computer's key, register it, and hand it to session sync.
/// `key_changed` is `CodeRunner::device_key_changed`; None without a runner.
#[cfg_attr(test, allow(dead_code))] // The workspace calls it outside tests; tests drive `register`.
pub(crate) fn register_this_computer(gateway: AccountGateway, key_changed: Option<Rc<dyn Fn()>>, cx: &mut App) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let url = gateway.url().to_string();
    let local_cache = crate::managed::local_cache(crate::managed::policy());
    let registration = register(gateway, local_cache, move || load_or_create(&url), key_changed, cx);
    cx.spawn(async move |_| {
        if let Err(error) = registration.await {
            eprintln!("nexus-desktop-gpui: this computer did not register for session sync: {error}");
        }
        RUNNING.store(false, Ordering::SeqCst);
    })
    .detach();
}

/// Read the directory, and register this computer only in gateway mode and only when Gateway does not list its key.
/// `load_key` gives the stored key or makes one, and says whether it is new; it runs only in gateway mode.
pub(super) fn register(
    gateway: AccountGateway,
    local_cache: bool,
    load_key: impl FnOnce() -> Result<(DeviceKey, bool), String> + Send + 'static,
    key_changed: Option<Rc<dyn Fn()>>,
    cx: &mut App,
) -> Task<Result<Registration, String>> {
    let directory = gateway.request(Verb::Get, "/v1/devices".into(), None);
    cx.spawn(async move |_| {
        let directory = directory.await?;
        if read_device_trust(&directory) != DeviceTrust::Gateway {
            return Ok(Registration::Approvals);
        }
        if !local_cache {
            return Ok(Registration::Managed);
        }
        let (key, made) = off_thread(load_key).await?;
        let encryption_key = e2e::b64u(&key.encryption_key());
        let signing_key = e2e::b64u(&key.signing_key());
        if this_device(&read_devices(&directory), &encryption_key, &signing_key).is_some() {
            return Ok(Registration::Listed);
        }
        let body = json!({
            "name": crate::computer_name::desktop_label(),
            "encryptionKey": encryption_key,
            "signingKey": signing_key,
        });
        let answer = gateway.request(Verb::Post, "/v1/devices".into(), Some(body)).await;
        // Session sync takes up a new key even when Gateway refused it: the keychain holds it now.
        if (made || answer.is_ok())
            && let Some(changed) = &key_changed
        {
            changed();
        }
        answer.map(|_| Registration::Registered)
    })
}

#[cfg(test)]
#[path = "sign_in_tests.rs"]
mod tests;
