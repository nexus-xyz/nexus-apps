//! Internal builds are for Nexus's own people. A packaged internal build ([`crate::channel`]) keeps a
//! sign-in to Nexus's Gateway only when Gateway verifies the account as an employee. It uses the updater's check,
//! `GET /v1/desktop/download-access` (`nexus_desktop_updates::access`): a provider-verified `@nexus.xyz` email.
//!
//! - A new sign-in (device or API key) is kept only when the check allows it. A refusal or an unknown answer fails the
//!   sign-in, and a device sign-in's new token is revoked.
//! - A saved sign-in is checked after launch. A refusal signs out and forgets the saved token. An unknown answer keeps
//!   the session and asks again later, so an employee is not locked out while the check is down.
//!
//! A development build and a stable build do not check. Another Gateway (staging, a developer's own) does not
//! either: it holds no Nexus accounts, and the check runs only on Nexus's own Gateways.

use std::time::Duration;

use nexus_desktop_updates::Failure;
use nexus_desktop_updates::access::{authorize, nexus_gateway};

/// What a refused account reads.
pub const REFUSED: &str = "This internal build of Nexus is only for Nexus employees. Sign in with your @nexus.xyz \
     account, or install Nexus from nexus.inc/download.";

/// What a new sign-in reads when the check gives no answer.
pub const UNKNOWN: &str = "Nexus couldn't confirm your @nexus.xyz account. Try again in a moment.";

/// The check's answer for one account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    Refused,
    /// No answer: the check was down or did not reply in time.
    Unknown,
}

impl Verdict {
    /// What a failed sign-in shows.
    pub fn message(self) -> &'static str {
        match self {
            Verdict::Unknown => UNKNOWN,
            Verdict::Allowed | Verdict::Refused => REFUSED,
        }
    }
}

/// Whether this build checks its sign-ins: a packaged internal build.
pub fn gated() -> bool {
    !cfg!(debug_assertions) && crate::channel::is_internal(crate::channel::VERSION)
}

/// Whether a sign-in on `gateway` is checked, in a build where [`gated`] says `gated`.
pub fn applies(gated: bool, gateway: &str) -> bool {
    gated && nexus_gateway(gateway)
}

/// The verdict of the check's answer. Only an unavailable check is unknown; every other failure refuses.
pub fn verdict(answer: Result<(), Failure>) -> Verdict {
    match answer {
        Ok(()) => Verdict::Allowed,
        Err(failure) if failure.code == "employee_verification_unavailable" => Verdict::Unknown,
        Err(_) => Verdict::Refused,
    }
}

/// Check the account behind `token` on `gateway`.
pub async fn check(gateway: &str, token: &str) -> Verdict {
    if !applies(gated(), gateway) {
        return Verdict::Allowed;
    }
    verdict(authorize(gateway, Some(token)).await.map(|_| ()))
}

/// How long a saved sign-in waits before it asks again after the `attempt`-th unknown answer: 30 seconds, doubled
/// each time, at most 5 minutes.
pub fn retry_after(attempt: u32) -> Duration {
    Duration::from_secs(30u64.saturating_mul(1 << attempt.min(4)).min(300))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_nexus_gateways_in_internal_builds_are_checked() {
        assert!(applies(true, "https://api.nexus.inc"));
        assert!(applies(true, "https://api.nexus.inc/"));
        assert!(applies(true, "https://api.nexus.xyz"));
        assert!(!applies(true, "http://localhost:8080"));
        assert!(!applies(true, "https://staging.example.com"));
        assert!(!applies(false, "https://api.nexus.inc"));
    }

    #[test]
    fn only_an_unavailable_check_is_unknown() {
        assert_eq!(verdict(Ok(())), Verdict::Allowed);
        assert_eq!(
            verdict(Err(Failure::new("employee_verification_unavailable", "down", true))),
            Verdict::Unknown
        );
        for code in ["employee_access_required", "employee_sign_in_required", "anything_else"] {
            assert_eq!(
                verdict(Err(Failure::new(code, "no", false))),
                Verdict::Refused,
                "{code}"
            );
        }
    }

    #[test]
    fn a_refusal_names_the_way_out() {
        assert!(Verdict::Refused.message().contains("@nexus.xyz"));
        assert!(Verdict::Refused.message().contains("nexus.inc/download"));
        assert_eq!(Verdict::Unknown.message(), UNKNOWN);
    }

    #[test]
    fn retries_back_off_to_five_minutes() {
        assert_eq!(retry_after(0), Duration::from_secs(30));
        assert_eq!(retry_after(1), Duration::from_secs(60));
        assert_eq!(retry_after(3), Duration::from_secs(240));
        assert_eq!(retry_after(4), Duration::from_secs(300));
        assert_eq!(retry_after(40), Duration::from_secs(300));
    }
}
