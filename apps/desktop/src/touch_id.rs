//! Touch ID for the Assistant's vault: the grant sheet's Approve, and Show password. macOS asks through
//! LocalAuthentication (`LAContext`, policy `DeviceOwnerAuthentication`): Touch ID, with the Mac's password or an
//! Apple Watch when Touch ID is not there. Other systems answer "unavailable", and the vault stays not connected.
//!
//! The answer is `nexus_desktop_core::vault_local::Auth`. Only `Approved` makes an `Authorized`, which reading a
//! secret needs.

use nexus_desktop_core::vault_local::{Auth, Authenticator};

/// The system's own sheet.
pub struct TouchId;

impl Authenticator for TouchId {
    fn ask(&self, reason: &str) -> Auth {
        ask(reason)
    }
}

#[cfg(target_os = "macos")]
fn ask(reason: &str) -> Auth {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::Duration;

    // LAError codes that mean "this Mac cannot ask": passcode not set (-5), Touch ID not available (-6) or enrolled (-7).
    const UNAVAILABLE: [isize; 3] = [-5, -6, -7];
    const NO_WAY: &str = "This Mac has no Touch ID or password set, so Nexus cannot ask.";
    let (send, receive) = mpsc::channel::<Auth>();
    let send = Mutex::new(send);
    // Kept alive until the answer: the context owns the sheet.
    let context = unsafe { LAContext::new() };
    let policy = LAPolicy::DeviceOwnerAuthentication;
    if let Err(error) = unsafe { context.canEvaluatePolicy_error(policy) } {
        return if UNAVAILABLE.contains(&error.code()) {
            Auth::Unavailable(NO_WAY.into())
        } else {
            Auth::Declined
        };
    }
    let reply = RcBlock::new(move |ok: Bool, error: *mut NSError| {
        let auth = if ok.as_bool() {
            Auth::Approved
        } else if !error.is_null() && UNAVAILABLE.contains(&unsafe { (*error).code() }) {
            Auth::Unavailable(NO_WAY.into())
        } else {
            Auth::Declined
        };
        if let Ok(send) = send.lock() {
            send.send(auth).ok();
        }
    });
    let reason = NSString::from_str(reason);
    unsafe { context.evaluatePolicy_localizedReason_reply(policy, &reason, &reply) };
    // The person takes as long as they take; two minutes without an answer reads as no.
    let auth = receive.recv_timeout(Duration::from_secs(120)).unwrap_or(Auth::Declined);
    unsafe { context.invalidate() };
    auth
}

#[cfg(not(target_os = "macos"))]
fn ask(_reason: &str) -> Auth {
    Auth::Unavailable("Touch ID is on the Mac only.".into())
}
