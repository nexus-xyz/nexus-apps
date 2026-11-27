//! Nexus GPUI's update channel: the employee check of `access.rs`, held for a while, with a public build
//! that never fails because of it.
//!
//! - **A public build falls back to stable on any failure.** The stable feed needs no credential, so a Gateway or sign-in provider
//!   outage, a timeout or a malformed answer only means "no internal updates". Before, anything but 401 and 403 failed
//!   the whole check with "Cannot verify update access".
//! - **An internal build still reports the failure.** Moving it to stable would hide why its internal updates stopped.
//! - **The answer is held.** Allowed or refused, it holds for [`HOLD`]; an outage for [`HOLD_OUTAGE`]. A check on
//!   launch, focus, wake or reconnect within that time does not call Gateway. A manual check asks again unless access
//!   was allowed. A new sign-in is a new key, so it always asks.

use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use reqwest::header::HeaderValue;

use crate::access::{self, Check};
use crate::{Channel, Failure};

/// How long an allowed or refused answer holds.
pub const HOLD: Duration = Duration::from_secs(60 * 60);
/// How long an outage holds before the next automatic check asks again.
pub const HOLD_OUTAGE: Duration = Duration::from_secs(5 * 60);

/// The last answer: whose (the Gateway and the token, hashed), when, and what.
type Last = (u64, Instant, Result<HeaderValue, Failure>);

/// The held answer, one per app.
#[derive(Default)]
pub struct Held {
    last: Mutex<Option<Last>>,
}

impl Held {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Last>> {
        self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn key(gateway: &str, token: Option<&str>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    gateway.trim_end_matches('/').hash(&mut hasher);
    token.hash(&mut hasher);
    hasher.finish()
}

/// How long `answer` holds.
fn hold(answer: &Result<HeaderValue, Failure>) -> Duration {
    match answer {
        Err(failure) if failure.code == "employee_verification_unavailable" => HOLD_OUTAGE,
        _ => HOLD,
    }
}

/// The channel to check for the session on `gateway` with `token`, and the download credential for an internal one.
pub async fn channel(
    held: &Held,
    gateway: &str,
    token: Option<&str>,
    installed: &str,
    manual: bool,
) -> Result<(Channel, Option<HeaderValue>), Failure> {
    channel_with(held, &access::LIVE, gateway, token, installed, manual, Instant::now()).await
}

pub(crate) async fn channel_with(
    held: &Held,
    check: &Check<'_>,
    gateway: &str,
    token: Option<&str>,
    installed: &str,
    manual: bool,
    now: Instant,
) -> Result<(Channel, Option<HeaderValue>), Failure> {
    let key = key(gateway, token);
    let kept = held.lock().as_ref().and_then(|(whose, at, answer)| {
        let fresh = *whose == key && now.saturating_duration_since(*at) < hold(answer);
        // A manual check asks again, unless access was allowed: the person may have just fixed the cause.
        (fresh && (answer.is_ok() || !manual)).then(|| answer.clone())
    });
    let answer = match kept {
        Some(answer) => answer,
        None => {
            let answer = access::authorize_with(check, gateway, token).await;
            *held.lock() = Some((key, now, answer.clone()));
            answer
        }
    };
    match answer {
        Ok(header) => Ok((Channel::Internal, Some(header))),
        Err(failure) if Channel::of(installed) == Some(Channel::Internal) => Err(failure),
        Err(_) => Ok((Channel::Stable, None)),
    }
}

#[cfg(test)]
#[path = "held_access_tests.rs"]
mod tests;
