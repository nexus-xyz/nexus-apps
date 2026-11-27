//! The computer addressed by a phone. Native state owns identity, admission and execution; transport owns crypto.
use crate::{Result, api::Api};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

mod changes;
mod client;
mod join;
mod link;
pub mod settings;
mod worker;
pub(crate) use join::PAIRING_KIND;
pub use join::{JoinState, JoinStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BillingPayer {
    Plan,
    ApiKey,
    Gateway,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PayerKind {
    Plan,
    ApiKey,
    Gateway,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Payer {
    pub kind: PayerKind,
    pub label: String,
    pub detail: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelChoice {
    pub id: String,
    pub name: String,
    pub efforts: Vec<crate::event::Effort>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Availability {
    Usable,
    Unavailable,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteChoices {
    pub via: crate::event::Via,
    pub payer: BillingPayer,
    pub label: String,
    pub detail: Option<String>,
    pub state: Availability,
    pub models: Vec<ModelChoice>,
    pub default_model: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentChoices {
    pub agent: crate::event::SessionAgent,
    pub name: String,
    pub routes: Vec<RouteChoices>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub via: crate::event::Via,
    pub payer: BillingPayer,
    pub model: Option<String>,
    pub effort: Option<crate::event::Effort>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionUpdate {
    pub model: Option<String>,
    pub effort: Option<crate::event::Effort>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSettings {
    pub thread_id: String,
    pub revision: String,
    pub agent: crate::event::SessionAgent,
    pub via: crate::event::Via,
    pub model: Option<String>,
    pub effort: Option<crate::event::Effort>,
    pub payer: Payer,
    pub pending: Option<crate::event::Pending>,
    pub models: Vec<ModelChoice>,
    pub editable: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub gateway: String,
    pub tenant_id: String,
    pub person_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Origin {
    Computer {
        #[serde(rename = "computerId")]
        computer_id: String,
        name: String,
    },
    Cloud {
        #[serde(rename = "machineId")]
        machine_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartSettings {
    pub agent: String,
    pub model: Option<String>,
    pub via: String,
    pub permission_mode: String,
    pub effort: Option<String>,
    pub workspace: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectChoice {
    pub id: String,
    pub name: String,
    pub location_hint: String,
    pub revision: String,
    pub settings: StartSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payer: Option<Payer>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HandoffStart {
    pub source_thread_id: String,
    pub agent: crate::event::SessionAgent,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Operation {
    DescribeProjects,
    DescribeBranches {
        #[serde(rename = "projectId")]
        project_id: String,
        revision: String,
    },
    DescribeHandoff {
        #[serde(rename = "threadId")]
        thread_id: String,
    },
    StartSession {
        #[serde(rename = "projectId")]
        project_id: String,
        revision: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selection: Option<Selection>,
        #[serde(default, rename = "baseBranch", skip_serializing_if = "Option::is_none")]
        base_branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handoff: Option<HandoffStart>,
    },
    DescribeSession {
        #[serde(rename = "threadId")]
        thread_id: String,
    },
    ReadSessionChanges {
        #[serde(rename = "threadId")]
        thread_id: String,
    },
    CompactSession {
        #[serde(rename = "threadId")]
        thread_id: String,
    },
    UpdateSession {
        #[serde(rename = "threadId")]
        thread_id: String,
        revision: String,
        settings: SessionUpdate,
    },
}
impl Operation {
    pub(crate) fn requires_settings(&self) -> bool {
        matches!(
            self,
            Self::DescribeSession { .. }
                | Self::DescribeHandoff { .. }
                | Self::CompactSession { .. }
                | Self::UpdateSession { .. }
                | Self::StartSession { selection: Some(_), .. }
        )
    }
    pub(crate) fn admission_ms(&self) -> i64 {
        if matches!(self, Self::StartSession { .. }) {
            15_000
        } else {
            30_000
        }
    }
    pub(crate) fn valid(&self) -> bool {
        let text = |v: &str, max: usize| !v.is_empty() && v.chars().count() <= max && !v.chars().any(char::is_control);
        let model = |v: &Option<String>| v.as_ref().is_none_or(|s| text(s, 256));
        let uuid = |v: &str| uuid::Uuid::parse_str(v).is_ok_and(|id| id.to_string() == v);
        match self {
            Self::DescribeProjects => true,
            Self::DescribeBranches { project_id, revision } => text(project_id, 128) && text(revision, 128),
            Self::StartSession {
                project_id,
                revision,
                text: prompt,
                selection,
                base_branch,
                handoff,
            } => {
                text(project_id, 128)
                    && text(revision, 128)
                    && !prompt.trim().is_empty()
                    && prompt.len() <= 48 * 1024
                    && base_branch.as_deref().is_none_or(valid_base_branch)
                    && handoff.as_ref().is_none_or(|choice| {
                        uuid(&choice.source_thread_id) && selection.is_some() && base_branch.is_none()
                    })
                    && selection.as_ref().is_none_or(|s| {
                        model(&s.model)
                            && matches!(
                                (s.via, s.payer),
                                (crate::event::Via::Nexus, BillingPayer::Gateway)
                                    | (
                                        crate::event::Via::Subscription,
                                        BillingPayer::Plan | BillingPayer::ApiKey
                                    )
                            )
                    })
            }
            Self::DescribeSession { thread_id }
            | Self::ReadSessionChanges { thread_id }
            | Self::DescribeHandoff { thread_id }
            | Self::CompactSession { thread_id } => uuid(thread_id),
            Self::UpdateSession {
                thread_id,
                revision,
                settings,
            } => uuid(thread_id) && text(revision, 128) && model(&settings.model),
        }
    }
}
pub(crate) fn valid_base_branch(name: &str) -> bool {
    !name.is_empty()
        && name.encode_utf16().count() <= 256
        && !name.starts_with('-')
        && !name.chars().any(|ch| ch.is_control() || ch.is_whitespace())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Outcome {
    Projects {
        projects: Vec<ProjectChoice>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agents: Option<Vec<AgentChoices>>,
    },
    Branches {
        #[serde(rename = "projectId")]
        project_id: String,
        revision: String,
        branches: Vec<String>,
    },
    HandoffChoices {
        #[serde(rename = "threadId")]
        thread_id: String,
        project: Box<ProjectChoice>,
        agents: Vec<AgentChoices>,
    },
    Created {
        #[serde(rename = "threadId")]
        thread_id: String,
        origin: Origin,
        launch: String,
    },
    ChoicesChanged {
        projects: Vec<ProjectChoice>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agents: Option<Vec<AgentChoices>>,
    },
    SessionSettings {
        session: SessionSettings,
    },
    SessionChanges {
        #[serde(rename = "threadId")]
        thread_id: String,
        files: Vec<String>,
        patch: String,
        truncated: bool,
    },
    SessionUpdated {
        session: SessionSettings,
    },
    Compacted {
        #[serde(rename = "threadId")]
        thread_id: String,
    },
    SessionChanged {
        session: SessionSettings,
    },
    Refused {
        code: String,
        message: String,
    },
    Uncertain {
        #[serde(rename = "threadId")]
        thread_id: Option<String>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Invitation {
    pub version: u8,
    pub scope: Scope,
    pub invite_id: String,
    pub computer_id: String,
    pub computer_name: String,
    pub inviter_device_id: String,
    pub encryption_key: String,
    pub signing_key: String,
    pub expires_at: i64,
    pub secret: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingDecision {
    pub scope: Scope,
    pub computer_id: String,
    pub invite_id: String,
    pub inviter_device_id: String,
    pub expires_at: i64,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_device_id: Option<String>,
    pub signature: String,
}

impl PairingDecision {
    pub fn message(&self) -> String {
        let mut message = String::from("nexus-phone-pairing-decision-v1\n");
        for field in [
            self.scope.gateway.as_str(),
            self.scope.tenant_id.as_str(),
            self.scope.person_id.as_str(),
            self.computer_id.as_str(),
            self.invite_id.as_str(),
            self.inviter_device_id.as_str(),
            &self.expires_at.to_string(),
            self.state.as_str(),
            self.request_id.as_deref().unwrap_or_default(),
            self.sender_device_id.as_deref().unwrap_or_default(),
        ] {
            use std::fmt::Write as _;
            let _ = write!(message, "{}:{field}", field.len());
        }
        message
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub available: bool,
    pub computer_id: String,
    pub name: String,
    pub enabled: bool,
    pub scope: Option<Scope>,
    pub runner_device_id: Option<String>,
    pub generation: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkStatus {
    pub state: String,
    pub code: Option<String>,
    pub device_id: Option<String>,
    pub name: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionIdentity {
    pub local_id: String,
    pub sync_id: Option<String>,
    pub origin: Origin,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub threads: Vec<crate::event::ThreadSummary>,
    pub identities: Vec<ExecutionIdentity>,
    pub next_cursor: Option<String>,
}

#[derive(Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    pub trusted: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestTarget {
    pub id: String,
    pub hash: String,
    pub sender: String,
    pub computer_id: String,
    pub generation: i64,
    pub expires_at: i64,
    pub creation_hash: Option<String>,
}
#[derive(Clone)]
pub struct Request {
    pub target: RequestTarget,
    pub operation: Operation,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    pub computer_id: String,
    pub request_id: String,
    pub body: serde_json::Value,
}
#[derive(Clone)]
pub struct Pairing {
    pub id: String,
    pub sender: String,
    pub name: String,
    pub encryption_key: String,
    pub signing_key: String,
    pub secret_hash: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outbound {
    pub id: String,
    pub target: String,
    pub kind: String,
    pub body: serde_json::Value,
    pub expires_at: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallResult {
    pub request_id: String,
    pub state: String,
    pub result: Option<Outcome>,
    pub expires_at: i64,
}
/// Purpose-built operations. No generic workbench or arbitrary signing command crosses this boundary.
pub trait ClientTransport: Send + Sync {
    fn computers(&self) -> BoxFuture<'_, Result<serde_json::Value>>;
    fn seal_request(&self, computer: String, id: String, operation: Operation) -> BoxFuture<'_, Result<Outbound>>;
    fn send_request(&self, outbound: Outbound) -> BoxFuture<'_, Result<CallResult>>;
    fn get_request(&self, outbound: Outbound) -> BoxFuture<'_, Result<CallResult>>;
    fn synced_page(
        &self,
        thread: String,
        before: Option<i64>,
        after: Option<i64>,
        limit: usize,
    ) -> BoxFuture<'_, Result<serde_json::Value>>;
    fn seal_command(
        &self,
        thread: String,
        id: String,
        action: crate::session_sync::control::Action,
    ) -> BoxFuture<'_, Result<Outbound>>;
    fn send_command(&self, outbound: Outbound) -> BoxFuture<'_, Result<serde_json::Value>>;
    fn get_command(&self, thread: String, id: String) -> BoxFuture<'_, Result<serde_json::Value>>;
}
/// Enables the desktop pairing preview for a development or internal launch.
pub fn remote_pairing_preview() -> bool {
    std::env::var_os("NEXUS_REMOTE_PAIRING_PREVIEW").is_some_and(|value| value == "1")
}

pub trait Transport: ClientTransport + Send + Sync {
    fn seal_join(&self, invitation: Invitation, local_name: String) -> BoxFuture<'_, Result<Outbound>>;
    fn poll_join(&self, outbound: Outbound) -> BoxFuture<'_, Result<(JoinStatus, String)>>;
    fn admission_sender(&self, sender: String) -> BoxFuture<'_, Result<bool>>;
    fn device(&self) -> BoxFuture<'_, Result<Device>>;
    fn presence(&self, info: Info, enabled: bool) -> BoxFuture<'_, Result<()>>;
    fn requests<'a>(&'a self, computer: &'a str) -> BoxFuture<'a, Result<Vec<Request>>>;
    fn claim(&self, request: RequestTarget) -> BoxFuture<'_, Result<bool>>;
    fn seal_reply(&self, request: RequestTarget, outcome: Outcome) -> BoxFuture<'_, Result<Reply>>;
    fn deliver(&self, reply: Reply) -> BoxFuture<'_, Result<()>>;
    fn invitation(&self, info: Info, signed_decisions: bool) -> BoxFuture<'_, Result<Invitation>>;
    fn pairings(&self, invitation: InvitationRecord) -> BoxFuture<'_, Result<Vec<Pairing>>>;
    fn approve_pairing(&self, pairing: Pairing, admission: PairingAdmission) -> BoxFuture<'_, Result<()>>;
    fn publish_pairing_decision(&self, invitation: InvitationRecord) -> BoxFuture<'_, Result<()>>;
}
#[derive(Clone, Serialize, Deserialize)]
pub struct InvitationRecord {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub signed_decisions: bool,
    pub scope: Scope,
    pub invite_id: String,
    pub computer_id: String,
    pub inviter: String,
    pub expires_at: i64,
    pub secret_hash: String,
    pub state: String,
    pub accepted: Option<String>,
    pub accepted_encryption: Option<String>,
    pub accepted_signing: Option<String>,
    pub accepted_request: Option<String>,
    pub accepted_name: Option<String>,
}

/// Re-read the local invitation after network awaits, before authorizing its exact selected phone.
#[derive(Clone)]
pub struct PairingAdmission {
    pub(crate) workbench: crate::Workbench,
    pub(crate) identity: crate::session_sync::Identity,
    pub(crate) invitation: InvitationRecord,
}
impl PairingAdmission {
    pub fn current(&self) -> bool {
        let wb = &self.workbench;
        let current = wb
            .inner
            .host
            .session_sync()
            .is_some_and(|b| b.identity() == &self.identity && wb.sync_current(b.as_ref()));
        if !current || self.invitation.expires_at <= crate::now_ms() {
            return false;
        }
        let stored = wb
            .store()
            .phone_invitation(&self.identity.scope, &self.identity.user, &self.invitation.invite_id)
            .ok()
            .flatten();
        stored.is_some_and(|i| {
            i.state == "approving"
                && i.scope == self.invitation.scope
                && i.computer_id == self.invitation.computer_id
                && i.inviter == self.invitation.inviter
                && i.expires_at == self.invitation.expires_at
                && i.secret_hash == self.invitation.secret_hash
                && i.accepted == self.invitation.accepted
                && i.accepted_encryption == self.invitation.accepted_encryption
                && i.accepted_signing == self.invitation.accepted_signing
                && i.accepted_request == self.invitation.accepted_request
        })
    }
}

impl Api {
    fn computer_backend(&self) -> Result<Arc<dyn crate::session_sync::Backend>> {
        let wb = self.workbench();
        let backend = wb
            .inner
            .host
            .session_sync()
            .ok_or("Encrypted session sync is not available.")?;
        if !wb.persistent() || !wb.sync_current(backend.as_ref()) || backend.computer().is_none() {
            return Err("Encrypted computer control is not available.".into());
        }
        Ok(backend)
    }
}
