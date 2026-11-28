//! Scoped session choices and updates; native hosts own discovery and authentication.
use super::*;
use crate::event::{Pending, SessionAgent, ThreadStatus, Via};
use crate::session_sync::{Backend, Target};

mod discovery;
pub(crate) use discovery::check_spawn_payer;
pub use discovery::{check_launch_payer, native_choices, native_choices_by_account};

/// The extra profile folder selected for each agent in an execution-choices request.
pub type AccountProfiles = std::collections::HashMap<SessionAgent, std::path::PathBuf>;

/// Native-only evidence. Only `agents` is sent to another device.
pub struct NativeChoices {
    pub agents: Vec<AgentChoices>,
    pub logins: Vec<(SessionAgent, nexus_client::local_agents::Login)>,
}

pub(crate) fn accepts(choices: &[AgentChoices], agent: SessionAgent, selection: &Selection) -> bool {
    let Some(route) = choices.iter().find(|a| a.agent == agent).and_then(|a| {
        a.routes
            .iter()
            .find(|r| r.via == selection.via && r.payer == selection.payer && r.state == Availability::Usable)
    }) else {
        return false;
    };
    selection
        .model
        .as_ref()
        .is_none_or(|id| route.models.iter().any(|m| &m.id == id))
        && selection.effort.is_none_or(|effort| {
            selection
                .model
                .as_ref()
                .or(route.default_model.as_ref())
                .and_then(|id| route.models.iter().find(|m| &m.id == id))
                .is_some_and(|m| m.efforts.contains(&effort))
        })
}

/// Whether the phone may start `agent` on this computer.
pub(super) fn phone_offers(agent: SessionAgent) -> bool {
    match agent {
        SessionAgent::Claude | SessionAgent::Codex | SessionAgent::Nexus => true,
        SessionAgent::Antigravity | SessionAgent::Grok => false,
        #[cfg(feature = "cursor")]
        SessionAgent::Cursor => false,
        #[cfg(feature = "opencode")]
        SessionAgent::Opencode => false,
    }
}

impl Api {
    pub(super) async fn execution_choices(
        &self,
        session: Option<(SessionAgent, Option<String>)>,
    ) -> Result<NativeChoices> {
        let wb = self.workbench();
        let agents: Vec<_> = SessionAgent::ALL
            .into_iter()
            .filter(|agent| phone_offers(*agent))
            .filter(|agent| session.as_ref().is_none_or(|(selected, _)| selected == agent))
            .collect();
        let profiles = match session {
            Some((agent, account)) => wb
                .choice_profile(agent, account)?
                .into_iter()
                .map(|dir| (agent, dir))
                .collect(),
            None => wb.available_choice_profiles(&agents),
        };
        wb.inner.host.computer_choices_by_account(agents, profiles).await
    }
    pub(super) fn owned_session_target(&self, backend: &dyn Backend, thread: &str) -> Result<Target> {
        let target = self
            .workbench()
            .store()
            .control_by_sync(backend.identity(), thread)?
            .ok_or("This session does not belong to this computer and account.")?;
        if !self.workbench().sync_current(backend)
            || self.workbench().record(&target.local)?.scope != backend.identity().scope
        {
            return Err("Account changed.".into());
        }
        Ok(target)
    }
    fn settings_snapshot(&self, target: &Target, choices: &NativeChoices) -> Result<SessionSettings> {
        let wb = self.workbench();
        let record = wb.record(&target.local)?;
        let summary = &record.summary;
        let routes = choices
            .agents
            .iter()
            .find(|a| a.agent == summary.agent)
            .map(|a| a.routes.as_slice())
            .unwrap_or_default();
        let observed = wb
            .live(&target.local)
            .and_then(|l| l.state().connection.account.clone())
            .map(|a| a.kind);
        let required = wb.store().computer_required_payer(&target.local, &record.scope)?;
        let route = if let Some(required) = required {
            routes.iter().find(|r| r.payer == required)
        } else if summary.via == Via::Nexus {
            routes.iter().find(|r| r.payer == BillingPayer::Gateway)
        } else {
            routes.iter().find(|r| {
                r.via == Via::Subscription
                    && match observed.as_deref() {
                        Some("apiKey") => r.payer == BillingPayer::ApiKey,
                        Some("chatgpt") => r.payer == BillingPayer::Plan,
                        _ => match choices.logins.iter().find(|(a, _)| *a == summary.agent).map(|(_, l)| l) {
                            Some(nexus_client::local_agents::Login::Plan) => r.payer == BillingPayer::Plan,
                            Some(nexus_client::local_agents::Login::ApiKey) => r.payer == BillingPayer::ApiKey,
                            _ => false,
                        },
                    }
            })
        };
        let payer = route
            .map(|r| Payer {
                kind: match r.payer {
                    BillingPayer::Gateway => PayerKind::Gateway,
                    BillingPayer::Plan => PayerKind::Plan,
                    BillingPayer::ApiKey => PayerKind::ApiKey,
                },
                label: r.label.clone(),
                detail: r.detail.clone(),
            })
            .unwrap_or(Payer {
                kind: PayerKind::Unknown,
                label: "Payer could not be verified".into(),
                detail: Some("Check this agent's sign-in on the execution computer.".into()),
            });
        let pending = wb
            .pending(&target.local)
            .map(|p| Pending {
                model: p.model,
                effort: p.effort,
                ..Pending::default()
            })
            .filter(|p| *p != Pending::default());
        let editable = route.is_some_and(|r| r.state == Availability::Usable)
            && !matches!(summary.status, ThreadStatus::Starting)
            && pending.is_none();
        let reason = if pending.is_some() {
            Some("Waiting for the agent to accept the previous settings change.".into())
        } else if summary.status == ThreadStatus::Starting {
            Some("The agent is starting. Try again when it is ready.".into())
        } else if !editable {
            Some("Execution settings are unavailable. Check this computer and try again.".into())
        } else {
            None
        };
        let models = route.map(|r| r.models.clone()).unwrap_or_default();
        let revision = super::worker::hash(
            &serde_json::to_vec(&(
                &target.sync_id,
                summary.agent,
                summary.via,
                &summary.account,
                &summary.model,
                summary.effort,
                &pending,
                &payer,
                &models,
                editable,
            ))
            .map_err(|_| "Could not read session settings.")?,
        );
        Ok(SessionSettings {
            thread_id: target.sync_id.clone(),
            revision,
            agent: summary.agent,
            via: summary.via,
            model: summary.model.clone(),
            effort: summary.effort,
            payer,
            pending,
            models,
            editable,
            reason,
        })
    }
    pub(super) async fn compact_operation(
        &self,
        backend: &dyn Backend,
        target: &RequestTarget,
        thread: &str,
    ) -> Result<Outcome> {
        let local = self.owned_session_target(backend, thread)?;
        let guard = crate::CreationGuard {
            identity: backend.identity().clone(),
            expires_at: target.expires_at,
            generation: target.generation,
            sender: target.sender.clone(),
            selection: None,
        };
        self.workbench().check_creation_admission(Some(&guard)).await?;
        let current = self.owned_session_target(backend, thread)?;
        if current.local != local.local {
            return Err("Session ownership changed.".into());
        }
        self.workbench().check_creation_guard(Some(&guard))?;
        match self.compact(&local.local, None).await {
            Ok(()) => Ok(Outcome::Compacted {
                thread_id: thread.into(),
            }),
            Err(problem) => Ok(Outcome::Refused {
                code: "compaction_unavailable".into(),
                message: problem,
            }),
        }
    }

    pub(super) async fn session_operation(
        &self,
        backend: &dyn Backend,
        target: &RequestTarget,
        thread: &str,
        update: Option<(String, SessionUpdate)>,
    ) -> Result<Outcome> {
        let local = self.owned_session_target(backend, thread)?;
        let record = self.workbench().record(&local.local)?;
        let profile = (record.summary.agent, record.summary.account);
        let choices = self.execution_choices(Some(profile.clone())).await?;
        let guard = crate::CreationGuard {
            identity: backend.identity().clone(),
            expires_at: target.expires_at,
            generation: target.generation,
            sender: target.sender.clone(),
            selection: None,
        };
        self.workbench().check_creation_admission(Some(&guard)).await?;
        let current = self.owned_session_target(backend, thread)?;
        if current.local != local.local {
            return Err("Session ownership changed.".into());
        }
        let gate = self.workbench().settings_gate(&local.local);
        let _held = gate.lock().unwrap_or_else(|p| p.into_inner());
        self.workbench().check_creation_guard(Some(&guard))?;
        let current = self.workbench().record(&local.local)?;
        if (current.summary.agent, current.summary.account) != profile {
            return Err("The session's execution account changed. Refresh its settings.".into());
        }
        let session = self.settings_snapshot(&local, &choices)?;
        let Some((revision, settings)) = update else {
            return Ok(Outcome::SessionSettings { session });
        };
        if revision != session.revision {
            return Ok(Outcome::SessionChanged { session });
        }
        if !session.editable {
            return Err("The agent cannot change settings right now.".into());
        }
        let default_model = choices
            .agents
            .iter()
            .find(|a| a.agent == session.agent)
            .and_then(|a| {
                a.routes.iter().find(|r| {
                    r.via == session.via
                        && match session.payer.kind {
                            PayerKind::Gateway => r.payer == BillingPayer::Gateway,
                            PayerKind::Plan => r.payer == BillingPayer::Plan,
                            PayerKind::ApiKey => r.payer == BillingPayer::ApiKey,
                            PayerKind::Unknown => false,
                        }
                })
            })
            .and_then(|r| r.default_model.as_ref());
        if settings
            .model
            .as_ref()
            .is_some_and(|id| !session.models.iter().any(|m| &m.id == id))
            || settings.effort.is_some_and(|e| {
                !settings
                    .model
                    .as_ref()
                    .or(default_model)
                    .and_then(|id| session.models.iter().find(|m| &m.id == id))
                    .is_some_and(|m| m.efforts.contains(&e))
            })
        {
            return Err("The selected model or effort is no longer available.".into());
        }
        self.workbench().update_inner(
            &local.local,
            crate::ThreadPatch {
                model: Some(settings.model),
                effort: Some(settings.effort),
                ..crate::ThreadPatch::default()
            },
        )?;
        Ok(Outcome::SessionUpdated {
            session: self.settings_snapshot(&local, &choices)?,
        })
    }
}
