//! The organization's notifications on this computer: this computer is a `desktop` notification device at Gateway,
//! and its deliveries show as system notifications. The loop, the device token, the calls and their rules are
//! `nexus_desktop_core::path_notices`, shared with the Nexus app; its header is the spec. Here: GPUI's [`Host`], and
//! the moments the workspace reports (a store it owns, a new sign-in, a sign-out).
//!
//! - **One device for both apps.** The device token is the Nexus app's keychain entry (`<gateway>#notify-device`), so
//!   either app registers the same device. Only one collects: this build runs the loop only while it owns the Code
//!   sessions store ([`start`], `owner.rs`), as the cloud machines do. While the Nexus app runs, it collects.
//! - **The sign-in** is the keychain's token for the Gateway, read again at each change ([`session_changed`]). An
//!   organization switch answers a new token, so the key (the Gateway and the token's marker) changes with it.
//! - **Sign-out** revokes the device before the token leaves the keychain ([`sign_out`], within the core's limit).
//! - **The switch** is "Your organization" in Settings › Notifications (`pathNotifications` in `settings.json`): it mutes
//!   this computer only. A click opens the notice's page: natively when this build draws it, else in the browser.

use std::sync::{Arc, Mutex, OnceLock};

use nexus_client::Client;
use nexus_desktop_core::path_notices::{Host, Notice, PathNotices};
use nexus_desktop_core::secrets::Vault;

/// The sign-in the loop belongs to, as last read from the keychain.
/// GPUI's side of the loop, with the active Workspace credential.
#[derive(Default)]
struct GpuiHost {
    client: Mutex<Option<Client>>,
}

impl GpuiHost {
    fn set_client(&self, next: Option<Client>) -> bool {
        let Ok(mut client) = self.client.lock() else {
            return false;
        };
        let identity = |client: &Option<Client>| {
            client.as_ref().map(|client| {
                let options = client.options();
                (options.base_url.clone(), options.api_key.clone())
            })
        };
        if identity(&client) == identity(&next) {
            return false;
        }
        *client = next;
        true
    }
}

/// The loop's key for a sign-in: the Gateway and the token's marker, and only while this build owns the store.
fn key_of(owns_store: bool, gateway: &str, token: Option<&str>) -> Option<String> {
    if !owns_store {
        return None;
    }
    Some(format!("{gateway}\n{}", nexus_desktop_core::secrets::marker(token?)))
}

impl Host for GpuiHost {
    fn key(&self) -> Option<String> {
        let client = self.client.lock().ok()?;
        let options = client.as_ref()?.options();
        key_of(
            crate::owner::owns_store(),
            &options.base_url,
            options.api_key.as_deref(),
        )
    }

    fn session(&self) -> Option<(String, Client)> {
        let client = self.client.lock().ok()?.clone()?;
        Some((client.options().base_url.clone(), client))
    }

    fn vault(&self) -> Vault {
        crate::keychain::vault()
    }

    fn showing(&self) -> bool {
        setting()
    }

    fn show(&self, notice: Notice) {
        crate::notify::nexus(notice);
    }
}

static RUNNING: OnceLock<(Arc<PathNotices>, Arc<GpuiHost>)> = OnceLock::new();

/// This build owns the store: run the loop here for `gateway`, once for the app's life. Later calls wake it.
pub fn start(client: Client) {
    if let Some((shared, host)) = RUNNING.get() {
        host.set_client(Some(client));
        shared.nudge();
        return;
    }
    let host = Arc::new(GpuiHost::default());
    host.set_client(Some(client));
    let shared = Arc::new(PathNotices::default());
    if RUNNING.set((shared.clone(), host.clone())).is_ok() {
        crate::code_run::runtime().spawn(async move { shared.run(&*host).await });
    }
}

/// The sign-in may have changed (signed in or out, another organization or Gateway): when it did, register or stop at
/// once. Each window reports each change, so an unchanged sign-in makes no call.
pub fn session_changed(client: Option<Client>) {
    if let Some((shared, host)) = RUNNING.get()
        && host.set_client(client)
    {
        shared.nudge();
    }
}

/// Before sign-out drops `client`'s token for `gateway`: revoke this computer's device, when this build registered
/// it. Returns at once when the loop does not run here.
pub async fn sign_out(gateway: &str, client: &Client) {
    let Some((shared, _)) = RUNNING.get() else {
        return;
    };
    let key = key_of(crate::owner::owns_store(), gateway, client.options().api_key.as_deref());
    shared.sign_out(key.as_deref(), client).await;
}

/// Whether this computer shows the organization's notifications: the Nexus app's setting. On by default.
pub fn setting() -> bool {
    crate::desktop_settings::load().path_notifications.unwrap_or(true)
}

/// The page of the organization a notice's path names (`/app?next=%2Fusage%3Fmonth%3D1`, `/app/acme/usage`), as
/// Nexus UI's path below the organization (`usage`). None for `/app` alone.
pub fn page_of(path: &str) -> Option<String> {
    let parsed = url::Url::parse(&format!("http://app.invalid{path}")).ok()?;
    let inner = match parsed.path().trim_end_matches('/') {
        "/app" => {
            let next = parsed
                .query_pairs()
                .find(|(name, _)| name == "next")
                .map(|(_, next)| next.into_owned())?;
            url::Url::parse(&format!("http://app.invalid{next}"))
                .ok()?
                .path()
                .to_string()
        }
        page => {
            // `/app/<org>/<page>`: the page below the organization.
            let below = page.strip_prefix("/app/")?;
            below.split_once('/').map(|(_, page)| page.to_string())?
        }
    };
    let inner = inner.trim_matches('/');
    (!inner.is_empty()).then(|| inner.to_string())
}

/// Skills promotion intent keeps its query, unlike the legacy PageId-only classifier below.
pub(crate) fn skills_target(path: &str) -> Option<crate::pages::skills::Target> {
    let parsed = url::Url::parse(&format!("http://app.invalid{path}")).ok()?;
    let route = if parsed.path() == "/app" {
        parsed.query_pairs().find(|(name, _)| name == "next")?.1.into_owned()
    } else {
        let below = parsed.path().strip_prefix("/app/")?;
        let (_, page) = below.split_once('/')?;
        format!("/{page}{}", parsed.query().map(|q| format!("?{q}")).unwrap_or_default())
    };
    crate::pages::skills::Target::parse(&route)
}

/// The native page a notice's path opens: Nexus UI's path, or the same page under Settings (`billing`,
/// `settings/billing`).
pub fn native_page(path: &str) -> Option<crate::pages::PageId> {
    let page = page_of(path)?;
    crate::pages::PageId::from_web_path(&page)
        .or_else(|| crate::pages::PageId::from_web_path(&format!("settings/{page}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::PageId;

    #[test]
    fn the_key_is_the_gateway_and_the_tokens_marker_while_this_build_owns_the_store() {
        let key = key_of(true, "https://gw.example", Some("tok-1")).expect("a key");
        assert!(key.starts_with("https://gw.example\ndesktop:"));
        assert!(!key.contains("tok-1"), "never the token");
        assert_ne!(
            key_of(true, "https://gw.example", Some("tok-2")),
            Some(key.clone()),
            "an organization switch is another token, so another sign-in"
        );
        assert_eq!(key_of(true, "https://gw.example", None), None, "signed out");
        assert_eq!(
            key_of(false, "https://gw.example", Some("tok-1")),
            None,
            "the Nexus app owns the store: it collects"
        );
    }

    #[test]
    fn a_notices_path_names_its_page_below_the_organization() {
        assert_eq!(
            page_of("/app?next=%2Fusage%3Fmonth%3D2026-10").as_deref(),
            Some("usage")
        );
        assert_eq!(page_of("/app/acme/usage?month=2026-10").as_deref(), Some("usage"));
        assert_eq!(
            page_of("/app/acme/settings/members").as_deref(),
            Some("settings/members")
        );
        assert_eq!(page_of("/app?next=%2Fnotifications").as_deref(), Some("notifications"));
        for none in ["/app", "/app?next=%2F", "/app/acme"] {
            assert_eq!(page_of(none), None, "{none}");
        }
    }

    #[test]
    fn skills_promotion_notice_intent_preserves_ids_and_grouped_filters() {
        use crate::pages::skills::Target;
        assert_eq!(
            skills_target("/app/acme/skills/promotions/spr%2Fa%3Fb"),
            Some(Target::Detail("spr/a?b".into()))
        );
        assert_eq!(
            skills_target("/app?next=%2Fskills%2Fpromotions%2Fspr%252Fa%253Fb"),
            Some(Target::Detail("spr/a?b".into()))
        );
        assert_eq!(
            skills_target("/app/acme/skills?view=requests&of=reach&state=held"),
            Some(Target::Requests {
                of: Some("reach".into()),
                state: Some("held".into())
            })
        );
        assert_eq!(
            skills_target("/app?next=%2Fskills%3Fview%3Drequests%26of%3Dmine"),
            Some(Target::Requests {
                of: Some("mine".into()),
                state: None
            })
        );
        for path in [
            "/app/acme/usage?month=1",
            "/app/acme/skills",
            "/app",
            "/app?next=%2Fskills%2Fpromotions%2F",
            "/app/acme/skills/promotions/a/b",
        ] {
            assert_eq!(skills_target(path), None, "{path}");
        }
        assert_eq!(native_page("/app/acme/usage?month=1"), Some(PageId::Usage));
    }

    #[test]
    fn a_page_this_build_draws_opens_natively() {
        assert_eq!(native_page("/app?next=%2Fusage%3Fmonth%3D2026-10"), Some(PageId::Usage));
        assert_eq!(native_page("/app?next=%2Fbilling"), Some(PageId::Billing));
        assert_eq!(native_page("/app/acme/settings/members"), Some(PageId::Members));
        assert_eq!(native_page("/app?next=%2Fnotifications"), None, "the browser");
    }
}
