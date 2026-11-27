//! A withdrawn build. Gateway refuses every `/v1` call from a withdrawn desktop version with
//! status 426, the header `Upgrade: nexus-desktop` and the code `desktop_revoked`, in the shape of the path's route
//! family (Gateway's `desktop_revoked` module). Every Gateway client of this app carries an observer
//! ([`Latch::observer`]) that reads each answer. The first refusal is kept in the [`Latch`], starts an update check and
//! goes to every window, where `workspace/revoked_notice.rs` shows it until the app restarts. Later refusals change
//! nothing, so the notice shows once.
//!
//! A refusal locks nothing: what needs no Gateway (local Code sessions
//! with the person's own agent sign-in, the files, the terminal) keeps working.

use std::sync::{Arc, OnceLock};

use serde_json::Value;

/// The refusal's code, as Gateway's `desktop_revoked::CODE`.
pub const CODE: &str = "desktop_revoked";

/// The `Upgrade` header's value on the refusal.
const UPGRADE: &str = "nexus-desktop";

/// What Gateway said about this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revoked {
    /// The version Gateway named, else the one this app runs.
    pub version: String,
    /// Gateway's message, or ours when the answer had no body.
    pub message: String,
}

/// The refusal in an answer, if it is one: status 426, and the `Upgrade: nexus-desktop` header or the code
/// `desktop_revoked`. Gateway's own shape and OpenAI's carry the code at `error.code`; Anthropic's carries it at
/// `error.nexus.code`, and the version at `error.version` or `error.nexus.version`. `body` is `None` for a stream.
pub fn recognize(status: u16, headers: &[(String, String)], body: Option<&[u8]>) -> Option<Revoked> {
    if status != 426 {
        return None;
    }
    let upgrade = headers
        .iter()
        .any(|(name, value)| name.eq_ignore_ascii_case("upgrade") && value.trim().eq_ignore_ascii_case(UPGRADE));
    let error = body
        .and_then(|body| serde_json::from_slice::<Value>(body).ok())
        .and_then(|body| body.get("error").cloned());
    let field = |key: &str| {
        let error = error.as_ref()?;
        error
            .get(key)
            .or_else(|| error.get("nexus").and_then(|nexus| nexus.get(key)))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(String::from)
    };
    let coded = error.as_ref().is_some_and(|error| {
        [
            error.get("code"),
            error.get("nexus").and_then(|nexus| nexus.get("code")),
        ]
        .into_iter()
        .flatten()
        .any(|code| code.as_str() == Some(CODE))
    });
    if !upgrade && !coded {
        return None;
    }
    let version = field("version").unwrap_or_else(|| crate::channel::VERSION.to_string());
    let message = field("message").unwrap_or_else(|| {
        format!("This version of the Nexus app ({version}) was withdrawn. Update Nexus to continue.")
    });
    Some(Revoked { version, message })
}

/// The first refusal, for the whole process: one owner, set once.
pub struct Latch {
    current: tokio::sync::watch::Sender<Option<Revoked>>,
    /// Runs once, on the first refusal: the app starts an update check.
    on_first: Box<dyn Fn(&Revoked) + Send + Sync>,
}

impl Latch {
    pub fn new(on_first: impl Fn(&Revoked) + Send + Sync + 'static) -> Arc<Latch> {
        Arc::new(Latch {
            current: tokio::sync::watch::Sender::new(None),
            on_first: Box::new(on_first),
        })
    }

    /// Keep `revoked` if it is the first refusal; true then. A later one changes nothing.
    pub fn note(&self, revoked: Revoked) -> bool {
        let kept = self.current.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(revoked);
            true
        });
        if kept && let Some(revoked) = self.current.borrow().clone() {
            eprintln!(
                "nexus-desktop-gpui: Gateway withdrew this version ({})",
                revoked.version
            );
            (self.on_first)(&revoked);
        }
        kept
    }

    /// The refusal kept, or None while this build works.
    pub fn current(&self) -> Option<Revoked> {
        self.current.borrow().clone()
    }

    /// The refusal as it lands, for a window that shows it.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<Option<Revoked>> {
        self.current.subscribe()
    }

    /// What every Gateway client of this app carries: each answer goes through [`recognize`].
    pub fn observer(self: &Arc<Self>) -> nexus_client::Observer {
        let latch = Arc::downgrade(self);
        nexus_client::Observer::new(move |answer| {
            let Some(latch) = latch.upgrade() else {
                return;
            };
            if latch.current.borrow().is_some() {
                return;
            }
            if let Some(revoked) = recognize(answer.status, answer.headers, answer.body) {
                latch.note(revoked);
            }
        })
    }
}

/// This app's latch: the first refusal starts a check for updates, whatever the automatic-updates setting says.
pub fn app() -> &'static Arc<Latch> {
    static APP: OnceLock<Arc<Latch>> = OnceLock::new();
    APP.get_or_init(|| Latch::new(|_| crate::updates::check()))
}

/// A Gateway client of this app, watched for the refusal.
pub fn client(options: nexus_client::ClientOptions) -> nexus_client::Client {
    nexus_client::Client::new(options).observed(app().observer())
}

#[cfg(test)]
#[path = "revoked_tests.rs"]
mod tests;
