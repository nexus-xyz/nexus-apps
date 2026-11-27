//! An update may stop only the active sessions the person confirmed. New work must ask again.

use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};

use crate::{Result, Workbench};

/// A conversation to offer after the app restarts. No prompts, credentials or native agent identifiers are kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub scope: String,
    pub identity: String,
    /// A later message means someone already continued or changed the conversation.
    pub last_user: i64,
    pub last_turn_end: i64,
}

impl Workbench {
    /// Active work across every organization, including approvals and background work. Idle processes are omitted.
    pub fn update_restart_sessions(&self) -> Result<Vec<Session>> {
        let lives = self.inner.live.lock().unwrap_or_else(|p| p.into_inner());
        let states: Vec<_> = lives.values().map(|live| live.state()).collect();
        lives
            .iter()
            .zip(states.iter())
            .filter(|(_, state)| !state.done && state.has_active_work())
            .map(|((id, live), _)| {
                Ok(Session {
                    id: id.clone(),
                    scope: live.scope.clone(),
                    identity: live.recovery_identity.clone(),
                    last_user: self.reader().last_seqs(id, 0)?.user.unwrap_or(0),
                    last_turn_end: self.reader().last_seqs(id, 0)?.turn_end.unwrap_or(0),
                })
            })
            .collect()
    }

    /// Freeze new work and return what will stop. Sessions started since the dialog opened require another confirmation.
    pub fn reserve_confirmed_update_restart(&self, confirmed: &[Session]) -> Result<Vec<Session>> {
        let lives = self.inner.live.lock().unwrap_or_else(|p| p.into_inner());
        let states: Vec<_> = lives.values().map(|live| live.state()).collect();
        if self.inner.update_restart.load(Ordering::SeqCst) {
            return Err("An update restart is already in progress.".into());
        }
        if self.inner.update_activities.load(Ordering::SeqCst) != 0 {
            return Err(
                "A session is starting or receiving a message. Wait a moment, then try restarting again.".into(),
            );
        }
        let mut sessions = Vec::new();
        for ((id, live), state) in lives.iter().zip(states.iter()) {
            if state.done || !state.has_active_work() {
                continue;
            }
            if !confirmed.iter().any(|session| {
                session.id == *id && session.scope == live.scope && session.identity == live.recovery_identity
            }) {
                return Err("More agents started working. Review the running agents before restarting.".into());
            }
            sessions.push(Session {
                id: id.clone(),
                scope: live.scope.clone(),
                identity: live.recovery_identity.clone(),
                last_user: self.reader().last_seqs(id, 0)?.user.unwrap_or(0),
                last_turn_end: self.reader().last_seqs(id, 0)?.turn_end.unwrap_or(0),
            });
        }
        self.inner.update_restart.store(true, Ordering::SeqCst);
        Ok(sessions)
    }

    /// Continue only an unchanged conversation in the current scope. A later message must never receive another Continue.
    pub async fn resume_after_update(&self, session: &Session) -> Result<()> {
        if self.inner.host.update_recovery_identity().as_deref() != Some(session.identity.as_str()) {
            return Err("The signed-in account changed. Switch back before continuing this agent.".into());
        }
        if !self.in_scope(&session.scope) {
            return Err("Switch to this session's organization before continuing it.".into());
        }
        let record = self.record(&session.id)?;
        if record.scope != session.scope {
            return Err("This session belongs to another organization.".into());
        }
        if record.summary.archived {
            return Err("This session is archived. Open it to continue.".into());
        }
        if self.is_live(&session.id) {
            return Ok(());
        }
        let ends = self.reader().last_turn_ends(&session.id)?;
        let last_user = self.reader().last_seqs(&session.id, 0)?.user.unwrap_or(0);
        if last_user > session.last_user {
            let failed_launch = ends.first().is_some_and(|event| {
                event.seq > last_user
                    && matches!(
                        event.event,
                        crate::event::WbEvent::TurnEnd {
                            status: crate::event::TurnStatus::Error,
                            ..
                        }
                    )
            });
            let page = self
                .reader()
                .events_page(&session.id, Some(last_user.saturating_add(1)), Some(1))?;
            let was_continue = page.events.iter().any(|event| {
                event.seq == last_user
                    && matches!(&event.event,
                crate::event::WbEvent::User { text, from: None, .. } if text == crate::CONTINUE)
            });
            // A launch can fail after the Continue is logged. Explicit Retry may try that failed launch again.
            if !failed_launch || !was_continue {
                return Ok(());
            }
        }
        if ends.iter().any(|event| {
            event.seq > session.last_turn_end
                && matches!(
                    event.event,
                    crate::event::WbEvent::TurnEnd {
                        status: crate::event::TurnStatus::Completed,
                        ..
                    }
                )
        }) {
            return Ok(());
        }
        self.resume(&session.id).await?;
        // The send API records launch failures in the transcript and can return Ok. Keep recovery retryable.
        if !self.is_live(&session.id)
            && self.reader().last_turn_ends(&session.id)?.first().is_some_and(|event| {
                event.seq > session.last_turn_end
                    && matches!(
                        event.event,
                        crate::event::WbEvent::TurnEnd {
                            status: crate::event::TurnStatus::Error,
                            ..
                        }
                    )
            })
        {
            return Err("The agent could not start. Open this session for details, then retry.".into());
        }
        Ok(())
    }
}
