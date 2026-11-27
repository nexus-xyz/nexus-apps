//! Owner-device commands for synced Code sessions. The computer opts in locally; Gateway cannot enable it. When
//! Gateway vouches for the person's devices (`DeviceTrust::Gateway`), it is on until the person turns it off here.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use futures::future::BoxFuture;

use super::control_loop::Performer;
use super::{ContentPolicy, remote};
use crate::event::{Decision, WbEvent};
use crate::{Result, Workbench};

mod answer;

/// The notices each phone command leaves in the session, in the same words on every runner (the CLI's too).
pub mod notices {
    pub use super::answer::{SENT, STOPPED, approval_notice};
}

pub const MAX_COMMAND_BYTES: usize = 64 * 1024;
/// The longest a phone's message may wait for this computer while it sleeps.
pub const WAIT_MS: i64 = 24 * 60 * 60 * 1000;
/// The normal life of a phone command: Stop and approval answers older than this never run, even when they waited.
const SHORT_MS: i64 = 60_000;
/// How far the phone's clock may differ from this computer's.
const SKEW_MS: i64 = 30_000;
const UNAVAILABLE: &str = "Phone control needs encrypted session sync and a paired, trusted computer.";

/// A command opened and authenticated by the backend, never an arbitrary workbench call.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Command {
    pub id: String,
    pub thread_id: String,
    #[serde(skip)]
    pub sender_device_id: String,
    pub expires_at: i64,
    /// When the phone sent a command that may wait for this computer, by the phone's clock. None for a
    /// command sent to a computer that was online.
    #[serde(default)]
    pub sent_at: Option<i64>,
    pub action: Action,
}

impl Command {
    /// Whether the command may still run at `now`. A waiting message runs within a day of its sending. A late Stop or
    /// approval answer is wrong, so a waiting one runs only within the normal minute.
    pub(crate) fn fresh(&self, now: i64) -> bool {
        if self.expires_at <= now {
            return false;
        }
        let Some(sent_at) = self.sent_at else {
            return true;
        };
        let longest = match self.action {
            Action::Send { .. } => WAIT_MS,
            Action::Stop | Action::Approve { .. } => SHORT_MS,
        };
        sent_at <= now.saturating_add(SKEW_MS) && now <= sent_at.saturating_add(longest + SKEW_MS)
    }

    /// A message that waited longer than the normal minute: it arrived while this computer was asleep.
    pub(crate) fn waited(&self, now: i64) -> Option<i64> {
        self.sent_at.filter(|sent_at| now.saturating_sub(*sent_at) > SHORT_MS)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    Send {
        text: String,
    },
    Stop,
    Approve {
        #[serde(rename = "requestId")]
        request_id: String,
        decision: Decision,
        message: Option<String>,
        answers: Option<Map<String, Value>>,
    },
}

impl Action {
    pub(crate) fn valid(&self) -> bool {
        match self {
            Action::Send { text } => !text.trim().is_empty() && text.len() <= MAX_COMMAND_BYTES,
            Action::Stop => true,
            Action::Approve {
                request_id,
                decision,
                answers,
                ..
            } => {
                !request_id.is_empty()
                    && request_id.len() <= 1024
                    && *decision != Decision::Expired
                    && answers.as_ref().is_none_or(|answers| {
                        answers.values().all(|answer| {
                            answer.is_string()
                                || answer
                                    .as_array()
                                    .is_some_and(|items| items.iter().all(Value::is_string))
                        })
                    })
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub available: bool,
    pub enabled: bool,
}

impl Workbench {
    pub fn code_control(&self) -> Result<Settings> {
        let backend = self.inner.host.session_sync();
        let current = backend.filter(|backend| self.sync_current(backend.as_ref()) && backend.supports_control());
        Ok(Settings {
            available: self.persistent() && current.is_some(),
            enabled: current.as_ref().is_some_and(|backend| {
                self.store()
                    .control_enabled_in(backend.identity(), backend.device_trust())
                    .unwrap_or(false)
            }),
        })
    }

    /// Stored beside the sessions, so the app and the background service share one local choice.
    pub fn set_code_control(&self, enabled: bool) -> Result<Settings> {
        let backend = self.inner.host.session_sync().ok_or(UNAVAILABLE)?;
        if !self.persistent() || !backend.supports_control() || !self.sync_current(backend.as_ref()) {
            return Err(UNAVAILABLE.to_string());
        }
        self.store().set_control_enabled(backend.identity(), enabled)?;
        self.code_control()
    }

    /// Poll one device mailbox independently of uploads. Completed sessions can accept the next prompt too.
    pub async fn control_sessions_once(&self) {
        let Some(backend) = self.inner.host.session_sync() else {
            return;
        };
        if !self.persistent() || !backend.supports_control() || !self.sync_current(backend.as_ref()) {
            return;
        }
        let enabled = self
            .store()
            .control_enabled_in(backend.identity(), backend.device_trust())
            .unwrap_or(false)
            && matches!(
                self.sync_policy(backend.as_ref()).await,
                Ok(ContentPolicy::UserOnly | ContentPolicy::UserAndOrgAdmins)
            );
        if !self.sync_current(backend.as_ref()) {
            return;
        }
        let Ok(targets) = self.store().control_targets(backend.identity()) else {
            return;
        };
        // New bindings are registered a few at a time. Renewing any bound thread refreshes this device's shared
        // lease, so old sessions remain reachable without polling or renewing the entire history.
        let mut renew = self.store().control_heartbeat_due(backend.identity()).unwrap_or(true);
        for target in targets {
            if !self.sync_current(backend.as_ref()) {
                return;
            }
            let _ = self.store().control_touch(&target);
            if renew || self.store().control_unbound(&target).unwrap_or(true) {
                if remote(backend.control_lease(&target.sync_id, enabled)).await.is_err() {
                    continue;
                }
                let _ = self.store().control_leased(&target);
                renew = false;
            }
        }
        self.control_commands_once(backend.as_ref(), enabled, None, &Desktop(self))
            .await;
    }

    /// Each action leaves a notice that names it, then runs as the person's own on this computer would. `waited`: when
    /// the phone sent a message that waited for this computer to wake.
    async fn perform_control(&self, local: &str, waited: Option<i64>, action: Action) -> Result<()> {
        let notice = |text: String| self.append(local, WbEvent::Notice { text }).map(|_| ());
        match action {
            Action::Send { text } => {
                notice(waited.map_or_else(|| answer::SENT.into(), answer::waited_notice))?;
                self.send(local, &text).await
            }
            // The desktop's Stop: the turn ends, and the agent keeps the conversation. Nothing to stop is no error.
            Action::Stop => {
                notice(answer::STOPPED.into())?;
                self.interrupt(local)
            }
            Action::Approve {
                request_id,
                decision,
                message,
                answers,
            } => {
                let tool = self
                    .live(local)
                    .and_then(|live| live.state().pending.get(&request_id).map(|asked| asked.tool.clone()))
                    .ok_or("That request was already answered, or the agent withdrew it.")?;
                let remember = match decision {
                    Decision::AllowSession => answer::session_rule(&self.approval_suggestions(local, &request_id)?),
                    _ => None,
                };
                // With no rule to keep, the call is allowed once, and the session says so.
                let decision = match (decision, &remember) {
                    (Decision::AllowSession, None) => Decision::Allow,
                    (decision, _) => decision,
                };
                notice(answer::approval_notice(&tool, decision, remember.is_some()))?;
                self.queue_denial_reason(local, &request_id, decision, message.as_deref())
                    .await?;
                self.answer_approval(local, &request_id, decision, message, remember, answers)
            }
        }
    }
}

/// The desktop's runner: every command runs as the person's own on this computer would.
struct Desktop<'a>(&'a Workbench);

impl Performer for Desktop<'_> {
    fn ready(&self) -> bool {
        true
    }
    fn perform<'a>(&'a self, local: &'a str, waited: Option<i64>, action: Action) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.0.perform_control(local, waited, action))
    }
}
