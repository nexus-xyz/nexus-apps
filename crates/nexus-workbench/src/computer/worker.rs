use super::*;
use crate::api::{CreateArgs, WorktreeArgs};
use crate::event::{PermissionMode, SessionAgent, ThreadStatus, Via};
use crate::session_sync::{Backend, ContentPolicy};
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::time::Duration;

pub(super) async fn bounded<T>(f: BoxFuture<'_, Result<T>>) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .map_err(|_| "Computer request timed out.".to_string())?
}
pub(super) fn hash(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))
}
impl Api {
    pub async fn computer_info(&self) -> Result<Info> {
        let wb = self.workbench();
        let owner = wb
            .inner
            .host
            .session_sync()
            .map(|b| b.identity().user.clone())
            .unwrap_or_default();
        let mut info = wb.store().computer_info(&wb.config().scope, &owner)?;
        if let Ok(backend) = self.computer_backend() {
            let Ok(device) = bounded(backend.computer().ok_or("Computer transport unavailable.")?.device()).await
            else {
                return Ok(info);
            };
            if !wb.sync_current(backend.as_ref()) {
                return Err("Account changed.".into());
            }
            info.available = device.trusted;
            info.scope = Some(device.scope);
            info.runner_device_id = Some(device.id);
            info.enabled = wb
                .store()
                .control_enabled_in(backend.identity(), backend.device_trust())?;
        }
        Ok(info)
    }
    pub async fn computer_name_set(&self, name: String) -> Result<Info> {
        let _ = self.computer_info().await?;
        self.workbench().store().computer_name(&name)?;
        self.computer_info().await
    }
    pub async fn local_history(&self, cursor: Option<String>, limit: Option<usize>) -> Result<HistoryPage> {
        let wb = self.workbench();
        let scope = wb.config().scope;
        let before = cursor
            .map(|c| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(c)
                    .map_err(|_| "Invalid history cursor.".to_string())
                    .and_then(|b| {
                        serde_json::from_slice::<(String, i64, String)>(&b)
                            .map_err(|_| "Invalid history cursor.".into())
                    })
            })
            .transpose()?;
        if before.as_ref().is_some_and(|(s, _, _)| s != &scope) {
            return Err("History cursor belongs to another organization.".into());
        }
        let limit = limit.unwrap_or(50).clamp(1, 100);
        let ids = wb
            .store()
            .computer_history_ids(&scope, before.map(|(_, at, id)| (at, id)), limit + 1)?;
        let more = ids.len() > limit;
        let mut records = ids
            .into_iter()
            .take(limit)
            .map(|id| wb.record(&id))
            .collect::<Result<Vec<_>>>()?;
        let next_cursor = if more {
            records
                .last()
                .map(|r| {
                    serde_json::to_vec(&(scope.clone(), r.summary.updated_at, r.summary.id.clone()))
                        .map(|v| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v))
                })
                .transpose()
                .map_err(|_| "Could not encode history cursor.".to_string())?
        } else {
            None
        };
        let owner = wb
            .inner
            .host
            .session_sync()
            .map(|b| b.identity().user.clone())
            .unwrap_or_default();
        let identities = wb.store().computer_identities(
            &scope,
            &owner,
            &records.iter().map(|r| r.summary.id.clone()).collect::<Vec<_>>(),
        )?;
        for record in &mut records {
            wb.fill_live_fields(&mut record.summary);
        }
        Ok(HistoryPage {
            threads: records.into_iter().map(|r| r.summary).collect(),
            identities,
            next_cursor,
        })
    }
    pub async fn execution_identities(&self, ids: &[String]) -> Result<Vec<ExecutionIdentity>> {
        let wb = self.workbench();
        let owner = wb
            .inner
            .host
            .session_sync()
            .map(|b| b.identity().user.clone())
            .unwrap_or_default();
        wb.store().computer_identities(&wb.config().scope, &owner, ids)
    }
    pub async fn run_computer_control_until_shutdown(&self) {
        let mut stop = self.workbench().inner.session_sync_stop.subscribe();
        if *stop.borrow() {
            return;
        }
        let heartbeats = async {
            let mut marker = None;
            loop {
                if let Ok(backend) = self.computer_backend() {
                    let identity = backend.identity().clone();
                    let enabled = self.computer_enabled(backend.as_ref()).await;
                    let next = (identity.scope.clone(), identity.user.clone(), enabled);
                    let start = marker
                        .as_ref()
                        .is_none_or(|(scope, user, _)| scope != &identity.scope || user != &identity.user);
                    let generation =
                        self.workbench()
                            .store()
                            .computer_generation(&identity.scope, &identity.user, enabled, start);
                    if let Ok(generation) = generation
                        && let Ok(mut info) = self.computer_info().await
                    {
                        info.generation = generation;
                        if self.workbench().sync_current(backend.as_ref())
                            && let Some(transport) = backend.computer()
                        {
                            let _ = bounded(transport.presence(info, enabled)).await;
                            marker = Some(next);
                        }
                    }
                } else {
                    marker = None;
                }
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        };
        let requests = async {
            loop {
                self.computer_pass().await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        };
        tokio::select! {_=stop.changed()=>{},_=heartbeats=>{},_=requests=>{}}
    }
    async fn computer_enabled(&self, backend: &dyn Backend) -> bool {
        let wb = self.workbench();
        wb.sync_current(backend)
            && wb
                .store()
                .control_enabled_in(backend.identity(), backend.device_trust())
                .unwrap_or(false)
            && matches!(
                wb.sync_policy(backend).await,
                Ok(ContentPolicy::UserOnly | ContentPolicy::UserAndOrgAdmins)
            )
    }
    pub async fn computer_pass(&self) {
        let Ok(backend) = self.computer_backend() else { return };
        let Some(transport) = backend.computer() else { return };
        let wb = self.workbench();
        let identity = backend.identity().clone();
        let decisions = wb.store().phone_pairing_outbox(&identity.scope, &identity.user);
        if let Ok(decisions) = decisions {
            for decision in decisions {
                let _ = self.publish_phone_pairing(backend.as_ref(), decision).await;
            }
        }
        // A prior claim attempt is never executed again. Only a provable row or stored reply can recover it.
        let pending = wb.store().computer_pending(&identity.scope, &identity.user);
        if let Ok(pending) = pending {
            for (target, reply) in pending {
                if !wb.sync_current(backend.as_ref()) {
                    return;
                }
                let reply = match reply {
                    Some(reply) => reply,
                    None => {
                        let outcome = self.recovered_computer_result(&target);
                        let Ok(reply) = bounded(transport.seal_reply(target.clone(), outcome)).await else {
                            continue;
                        };
                        if wb
                            .store()
                            .computer_reply(&identity.scope, &identity.user, &target, &reply)
                            .is_err()
                        {
                            continue;
                        }
                        reply
                    }
                };
                if bounded(transport.deliver(reply.clone())).await.is_ok() {
                    let _ = wb
                        .store()
                        .computer_ack(&identity.scope, &identity.user, &reply.request_id);
                }
            }
        }
        if !self.computer_enabled(backend.as_ref()).await {
            return;
        }
        let Ok(info) = self.computer_info().await else { return };
        let Ok(requests) = bounded(transport.requests(&info.computer_id)).await else {
            return;
        };
        for request in requests {
            let target = &request.target;
            if target.computer_id != info.computer_id
                || target.generation != info.generation
                || target.expires_at <= crate::now_ms()
                || !self.computer_enabled(backend.as_ref()).await
            {
                continue;
            }
            if !matches!(
                wb.store().computer_reserve(&identity.scope, &identity.user, target),
                Ok(true)
            ) {
                continue;
            }
            // The durable claim-attempt marker above precedes even the network claim. Lost acknowledgement is uncertain.
            let Ok(true) = bounded(transport.claim(target.clone())).await else {
                continue;
            };
            if wb
                .store()
                .computer_stage(&identity.scope, &identity.user, target, "executing")
                .is_err()
            {
                continue;
            }
            let outcome = if target.expires_at <= crate::now_ms() || !self.computer_enabled(backend.as_ref()).await {
                Outcome::Refused {
                    code: "unavailable".into(),
                    message: "This computer is no longer accepting new sessions.".into(),
                }
            } else if !wb
                .store()
                .control_allows_in(&identity, backend.device_trust(), &target.sender)
                .unwrap_or(false)
            {
                Outcome::Refused {
                    code: "device_not_connected".into(),
                    message: "Connect this device in the other computer's Nexus settings first.".into(),
                }
            } else {
                self.execute_computer(&backend, &info, request.clone()).await
            };
            let Ok(reply) = bounded(transport.seal_reply(target.clone(), outcome)).await else {
                continue;
            };
            if wb
                .store()
                .computer_reply(&identity.scope, &identity.user, target, &reply)
                .is_ok()
                && bounded(transport.deliver(reply)).await.is_ok()
            {
                let _ = wb.store().computer_ack(&identity.scope, &identity.user, &target.id);
            }
        }
    }
    fn recovered_computer_result(&self, target: &RequestTarget) -> Outcome {
        let wb = self.workbench();
        if target.creation_hash.is_none() {
            return Outcome::Uncertain { thread_id: None };
        }
        let Ok(id) = crate::creation_thread_id(&target.id) else {
            return Outcome::Uncertain { thread_id: None };
        };
        let Some(backend) = wb.inner.host.session_sync() else {
            return Outcome::Uncertain { thread_id: None };
        };
        let stored = wb
            .store()
            .sync_target(&id)
            .ok()
            .flatten()
            .filter(|t| t.identity == *backend.identity());
        match (stored, wb.record(&id)) {
            (Some(stored), Ok(record)) if target.creation_hash.as_ref() == observed_creation(wb, &record).as_ref() => {
                Outcome::Created {
                    thread_id: stored.sync_id,
                    origin: wb
                        .store()
                        .computer_origin(&record.scope, &backend.identity().user)
                        .unwrap_or(Origin::Unknown),
                    launch: launch(record.summary.status),
                }
            }
            _ => Outcome::Uncertain { thread_id: None },
        }
    }
    async fn execute_computer(&self, backend: &Arc<dyn Backend>, _info: &Info, mut request: Request) -> Outcome {
        match request.operation {
            Operation::DescribeProjects => match self.phone_projects(backend.as_ref()).await {
                Ok((projects, agents)) => {
                    let guard = crate::CreationGuard {
                        identity: backend.identity().clone(),
                        expires_at: request.target.expires_at,
                        generation: request.target.generation,
                        sender: request.target.sender.clone(),
                        selection: None,
                    };
                    if self.workbench().check_creation_admission(Some(&guard)).await.is_err() {
                        return refused();
                    }
                    Outcome::Projects {
                        projects,
                        agents: Some(agents),
                    }
                }
                Err(_) => refused(),
            },
            Operation::DescribeBranches { project_id, revision } => {
                let Ok((projects, agents)) = self.phone_projects(backend.as_ref()).await else {
                    return refused();
                };
                if !projects
                    .iter()
                    .any(|project| project.id == project_id && project.revision == revision)
                {
                    return Outcome::ChoicesChanged {
                        projects,
                        agents: Some(agents),
                    };
                }
                let branches = match self.phone_branch_bases(&project_id).await {
                    Ok(branches) => branches,
                    Err(outcome) => return *outcome,
                };
                let guard = crate::CreationGuard {
                    identity: backend.identity().clone(),
                    expires_at: request.target.expires_at,
                    generation: request.target.generation,
                    sender: request.target.sender.clone(),
                    selection: None,
                };
                if self.workbench().check_creation_admission(Some(&guard)).await.is_err() {
                    return refused();
                }
                Outcome::Branches {
                    project_id,
                    revision,
                    branches,
                }
            }
            Operation::DescribeHandoff { thread_id } => self
                .phone_handoff_choices(backend.as_ref(), &request.target, &thread_id)
                .await
                .unwrap_or_else(|_| refused()),
            Operation::StartSession {
                project_id,
                revision,
                text,
                selection,
                base_branch,
                handoff,
            } => {
                if text.trim().is_empty() || text.len() > 48 * 1024 {
                    return refused();
                }
                let Ok((projects, agents)) = self.phone_projects(backend.as_ref()).await else {
                    return refused();
                };
                let Some(choice) = projects.iter().find(|p| p.id == project_id) else {
                    return Outcome::ChoicesChanged {
                        projects,
                        agents: Some(agents),
                    };
                };
                if choice.revision != revision {
                    return Outcome::ChoicesChanged {
                        projects,
                        agents: Some(agents),
                    };
                }
                let agent = if let Some(handoff) = &handoff {
                    let Ok(source) = self.owned_session_target(backend.as_ref(), &handoff.source_thread_id) else {
                        return refused();
                    };
                    let Ok(record) = self.workbench().record(&source.local) else {
                        return refused();
                    };
                    if record.summary.project_id != project_id
                        || record.summary.agent == handoff.agent
                        || !super::settings::phone_offers(handoff.agent)
                        || selection.is_none()
                    {
                        return Outcome::Refused {
                            code: "handoff_unavailable".into(),
                            message: "Choose another available agent in the source project.".into(),
                        };
                    }
                    handoff.agent
                } else {
                    let Some(agent) =
                        SessionAgent::parse(&choice.settings.agent).filter(|a| super::settings::phone_offers(*a))
                    else {
                        return refused();
                    };
                    agent
                };
                if selection
                    .as_ref()
                    .is_some_and(|s| !super::settings::accepts(&agents, agent, s))
                {
                    return Outcome::ChoicesChanged {
                        projects,
                        agents: Some(agents),
                    };
                }
                if request.target.expires_at <= crate::now_ms() || !self.computer_enabled(backend.as_ref()).await {
                    return refused();
                }
                let s = &choice.settings;
                let worktree = if handoff.is_some() {
                    match self.phone_branch_bases(&project_id).await {
                        Ok(branches) if !branches.is_empty() => Some(WorktreeArgs::default()),
                        Err(outcome) => return *outcome,
                        Ok(_) => return refused(),
                    }
                } else if let Some(base) = base_branch {
                    match self.phone_branch_bases(&project_id).await {
                        Ok(branches) => match branch_worktree(&branches, base) {
                            Some(worktree) => Some(worktree),
                            None => {
                                return Outcome::Refused {
                                    code: "branch_changed".into(),
                                    message: "This branch changed on the computer. Refresh branches before starting."
                                        .into(),
                                };
                            }
                        },
                        Err(outcome) => return *outcome,
                    }
                } else {
                    (s.workspace == "worktree").then(WorktreeArgs::default)
                };
                let args = CreateArgs {
                    provider: None,
                    request_id: Some(request.target.id.clone()),
                    project_id,
                    agent: agent.as_str().into(),
                    model: selection
                        .as_ref()
                        .map(|v| v.model.clone())
                        .unwrap_or_else(|| s.model.clone()),
                    permission_mode: s.permission_mode.clone(),
                    effort: selection
                        .as_ref()
                        .map(|v| v.effort.map(|e| e.as_str().to_string()))
                        .unwrap_or_else(|| s.effort.clone()),
                    via: selection
                        .as_ref()
                        .map(|v| v.via.as_str().to_string())
                        .unwrap_or_else(|| s.via.clone()),
                    text,
                    images: vec![],
                    files: vec![],
                    worktree,
                    orchestration: None,
                    name: None,
                };
                let local = match crate::creation_thread_id(&request.target.id) {
                    Ok(id) => id,
                    Err(_) => return refused(),
                };
                if self.workbench().has_thread(&local) {
                    return Outcome::Refused {
                        code: "creation_conflict".into(),
                        message: "This creation ID already belongs to a session. Refresh and try a new start.".into(),
                    };
                }
                request.target.creation_hash = Some(creation_hash(&args));
                if self
                    .workbench()
                    .store()
                    .computer_creation(&backend.identity().scope, &backend.identity().user, &request.target)
                    .is_err()
                {
                    return Outcome::Uncertain { thread_id: None };
                }
                match self
                    .thread_create_scoped(
                        args,
                        backend.identity(),
                        request.target.expires_at,
                        request.target.generation,
                        request.target.sender.clone(),
                        selection.map(|s| (agent, s)),
                    )
                    .await
                {
                    Ok(record) => {
                        if !self.workbench().sync_current(backend.as_ref()) {
                            return Outcome::Uncertain { thread_id: None };
                        }
                        let sync = self
                            .workbench()
                            .store()
                            .sync_target(&record.id)
                            .ok()
                            .flatten()
                            .filter(|t| t.identity == *backend.identity());
                        match sync {
                            Some(t) => Outcome::Created {
                                thread_id: t.sync_id,
                                origin: self
                                    .workbench()
                                    .store()
                                    .computer_origin(&backend.identity().scope, &backend.identity().user)
                                    .unwrap_or(Origin::Unknown),
                                launch: launch(record.status),
                            },
                            None => Outcome::Uncertain { thread_id: None },
                        }
                    }
                    Err(_) => self.recovered_computer_result(&request.target),
                }
            }
            Operation::DescribeSession { thread_id } => self
                .session_operation(backend.as_ref(), &request.target, &thread_id, None)
                .await
                .unwrap_or_else(|_| refused()),
            Operation::ReadSessionChanges { thread_id } => self
                .session_changes(backend.as_ref(), &request.target, &thread_id)
                .await
                .unwrap_or_else(|_| refused()),
            Operation::CompactSession { thread_id } => self
                .compact_operation(backend.as_ref(), &request.target, &thread_id)
                .await
                .unwrap_or_else(|_| refused()),
            Operation::UpdateSession {
                thread_id,
                revision,
                settings,
            } => self
                .session_operation(
                    backend.as_ref(),
                    &request.target,
                    &thread_id,
                    Some((revision, settings)),
                )
                .await
                .unwrap_or_else(|_| refused()),
        }
    }
    async fn phone_handoff_choices(
        &self,
        backend: &dyn Backend,
        request: &RequestTarget,
        thread_id: &str,
    ) -> Result<Outcome> {
        let source = self.owned_session_target(backend, thread_id)?;
        let record = self.workbench().record(&source.local)?;
        let (projects, agents) = self.phone_projects(backend).await?;
        let project = projects
            .into_iter()
            .find(|project| project.id == record.summary.project_id)
            .ok_or("The source project is not available for phone sessions.")?;
        if !matches!(self.phone_branch_bases(&project.id).await, Ok(branches) if !branches.is_empty()) {
            return Ok(Outcome::Refused {
                code: "handoff_worktree_unavailable".into(),
                message: "This project needs an available Git worktree before you can continue in another agent."
                    .into(),
            });
        }
        let guard = crate::CreationGuard {
            identity: backend.identity().clone(),
            expires_at: request.expires_at,
            generation: request.generation,
            sender: request.sender.clone(),
            selection: None,
        };
        self.workbench().check_creation_admission(Some(&guard)).await?;
        self.owned_session_target(backend, thread_id)?;
        let agents = agents
            .into_iter()
            .filter(|entry| entry.agent != record.summary.agent)
            .filter_map(|mut entry| {
                entry.routes.retain(|route| route.state == Availability::Usable);
                (!entry.routes.is_empty()).then_some(entry)
            })
            .collect();
        Ok(Outcome::HandoffChoices {
            thread_id: thread_id.into(),
            project: Box::new(project),
            agents,
        })
    }

    async fn phone_branch_bases(&self, project_id: &str) -> std::result::Result<Vec<String>, Box<Outcome>> {
        if self.workbench().config().worktrees.is_none() {
            return Err(Box::new(Outcome::Refused {
                code: "branches_unavailable".into(),
                message: "Worktrees are unavailable on this computer.".into(),
            }));
        }
        self.worktree_bases(project_id)
            .await
            .map(|branches| {
                branches
                    .into_iter()
                    .filter(|branch| valid_base_branch(branch))
                    .collect()
            })
            .map_err(|_| {
                Box::new(Outcome::Refused {
                    code: "branches_unavailable".into(),
                    message: "Choose a Git project on this computer to start from a branch.".into(),
                })
            })
    }

    async fn phone_projects(&self, backend: &dyn Backend) -> Result<(Vec<ProjectChoice>, Vec<AgentChoices>)> {
        let overview = self.overview().await?;
        let native = self.execution_choices(None).await?;
        let agents = &native.agents;
        if !self.workbench().sync_current(backend) {
            return Err("Account changed.".into());
        }
        let wb = self.workbench();
        let mut choices = Vec::new();
        for project in overview.projects.into_iter().filter(|p| !p.archived).take(100) {
            let defaults = wb.project_defaults(&project.id)?;
            let prior = wb
                .store()
                .computer_prior(&backend.identity().scope, &backend.identity().user, &project.id)?;
            // An agent the phone can't start is no project's choice there: the next, else Claude Code.
            let offered = |agent: &SessionAgent| super::settings::phone_offers(*agent);
            let agent = defaults
                .agent
                .filter(offered)
                .or_else(|| prior.as_ref().map(|p| p.summary.agent).filter(offered))
                .unwrap_or(SessionAgent::Claude);
            if !overview.agents.iter().any(|a| a.agent == agent && a.path.is_some()) {
                continue;
            }
            let previous = prior.as_ref().filter(|p| p.summary.agent == agent);
            let model = if defaults.agent == Some(agent) {
                defaults.model.clone()
            } else {
                previous.and_then(|p| p.summary.model.clone())
            };
            let via = previous
                .map(|p| p.summary.via)
                .filter(|v| *v != Via::Subscription || overview.agent_subscriptions)
                .unwrap_or(Via::Nexus);
            let settings = StartSettings {
                agent: agent.as_str().into(),
                model: model.clone(),
                via: via.as_str().into(),
                permission_mode: defaults.mode.unwrap_or(PermissionMode::Default).as_str().into(),
                effort: previous
                    .filter(|p| p.summary.model == model)
                    .and_then(|p| p.summary.effort)
                    .map(|e| e.as_str().into()),
                workspace: match defaults.workspace {
                    Some(crate::project_defaults::Workspace::Worktree) if project.branch.is_some() => "worktree",
                    _ => "folder",
                }
                .into(),
            };
            let route = agents.iter().find(|a| a.agent == agent).and_then(|a| {
                a.routes.iter().find(|r| {
                    r.via == via
                        && if via == Via::Nexus {
                            r.payer == BillingPayer::Gateway
                        } else {
                            match native.logins.iter().find(|(a, _)| *a == agent).map(|(_, l)| l) {
                                Some(nexus_client::local_agents::Login::Plan) => r.payer == BillingPayer::Plan,
                                Some(nexus_client::local_agents::Login::ApiKey) => r.payer == BillingPayer::ApiKey,
                                _ => false,
                            }
                        }
                })
            });
            let payer = Some(
                route
                    .map(|r| Payer {
                        kind: match r.payer {
                            BillingPayer::Plan => PayerKind::Plan,
                            BillingPayer::ApiKey => PayerKind::ApiKey,
                            BillingPayer::Gateway => PayerKind::Gateway,
                        },
                        label: r.label.clone(),
                        detail: r.detail.clone(),
                    })
                    .unwrap_or(Payer {
                        kind: PayerKind::Unknown,
                        label: "Payer could not be verified".into(),
                        detail: Some("Check this agent's billing on the computer.".into()),
                    }),
            );
            let revision = hash(
                &serde_json::to_vec(&(
                    &project.id,
                    &project.path,
                    &settings,
                    // What a start from here gets: `CreateArgs::orchestration` is None, so the computer's switch.
                    wb.config().subsessions,
                    &agents,
                    &payer,
                ))
                .map_err(|_| "Could not read project choices.")?,
            );
            choices.push(ProjectChoice {
                id: project.id,
                name: project.name,
                location_hint: std::path::Path::new(&project.path)
                    .file_name()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                revision,
                settings,
                payer,
            });
        }
        Ok((choices, native.agents))
    }
}
fn refused() -> Outcome {
    Outcome::Refused {
        code: "unavailable".into(),
        message: "This computer cannot complete that request. Refresh the choices or check Nexus on the computer."
            .into(),
    }
}
fn launch(status: ThreadStatus) -> String {
    match status {
        ThreadStatus::Running => "running",
        ThreadStatus::Error => "error",
        ThreadStatus::Interrupted => "interrupted",
        ThreadStatus::Idle => "idle",
        _ => "starting",
    }
    .into()
}

fn branch_worktree(branches: &[String], base: String) -> Option<WorktreeArgs> {
    if !valid_base_branch(&base) || !branches.contains(&base) {
        return None;
    }
    // An explicit base always creates a new worktree. Never switch or clean the project's current checkout.
    Some(WorktreeArgs {
        base: Some(base),
        ..WorktreeArgs::default()
    })
}

fn creation_hash(args: &CreateArgs) -> String {
    hash(
        &serde_json::to_vec(&(
            &args.project_id,
            &args.agent,
            &args.model,
            &args.via,
            &args.permission_mode,
            &args.effort,
            args.worktree.is_some(),
            args.text.trim_end(),
        ))
        .unwrap_or_default(),
    )
}
fn observed_creation(wb: &crate::Workbench, r: &crate::store::ThreadRecord) -> Option<String> {
    let text = wb
        .reader()
        .events_after(&r.summary.id, 0)
        .ok()?
        .into_iter()
        .find_map(|e| match e.event {
            crate::event::WbEvent::User { text, .. } => Some(text),
            _ => None,
        })?;
    Some(hash(
        &serde_json::to_vec(&(
            &r.summary.project_id,
            r.summary.agent.as_str(),
            &r.summary.model,
            r.summary.via.as_str(),
            r.summary.permission_mode.as_str(),
            r.summary.effort.map(|e| e.as_str()),
            r.summary.worktree.is_some(),
            text.trim_end(),
        ))
        .ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn git(repo: &Path, args: &[&str]) {
        let result = crate::process::command("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            result.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn phone_branch_starts_a_new_worktree_with_the_selected_base_and_keeps_dirty_files() {
        let temp = std::env::temp_dir().join(format!("nexus-mobile-branch-{}", uuid::Uuid::new_v4()));
        let repo = temp.join("project");
        let worktree = temp.join("session");
        std::fs::create_dir_all(&repo).expect("repository directory");
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.name", "Branch Test"]);
        git(&repo, &["config", "user.email", "branch@example.invalid"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        let source = repo.join("notes.txt");
        std::fs::write(&source, "Main\n").expect("main file");
        git(&repo, &["add", "notes.txt"]);
        git(&repo, &["commit", "-m", "main"]);
        git(&repo, &["switch", "-c", "topic"]);
        std::fs::write(&source, "Topic\n").expect("topic file");
        git(&repo, &["commit", "-am", "topic"]);
        git(&repo, &["switch", "main"]);
        std::fs::write(&source, "Unsaved edits\n").expect("dirty file");
        std::fs::write(repo.join("draft.txt"), "Untracked\n").expect("untracked file");

        let branches = crate::git::worktree_bases(&repo).expect("worktree bases");
        let settings = branch_worktree(&branches, "topic".into()).expect("selected branch");
        crate::git::add_worktree(
            &repo,
            &worktree,
            "mobile-session",
            settings.base.as_deref().expect("worktree base"),
        )
        .expect("new worktree");

        assert_eq!(crate::git::branch(&repo).as_deref(), Some("main"));
        assert_eq!(
            std::fs::read_to_string(source).expect("current checkout"),
            "Unsaved edits\n"
        );
        assert!(repo.join("draft.txt").exists());
        assert!(crate::git::is_worktree(&worktree));
        assert_eq!(
            std::fs::read_to_string(worktree.join("notes.txt")).expect("session checkout"),
            "Topic\n"
        );
        std::fs::remove_dir_all(temp).expect("remove test repository");
    }

    #[test]
    fn phone_branch_refuses_a_missing_base_without_a_folder_fallback() {
        assert!(branch_worktree(&["main".into()], "gone".into()).is_none());
    }
}
