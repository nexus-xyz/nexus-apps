//! Where a desktop app's Code commands go: the app's own workbench while it holds the store's lock, or the background
//! service that holds it ("Keep agents running when Nexus quits", [`crate::local`]). Nexus GPUI's runner sends all its
//! commands through it, and its outside access runs its tools through it.
//!
//! Each method has the name and the answer of the `nexus_workbench::api::Api` method the app called before, so the
//! commands read the same either way. Over the service, a method is the shared `wb_*` command, with the same arguments
//! (`nexus_workbench::api::Api::call`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::machine::{Remote, Status};
use nexus_workbench::api::{AgentInfo, Api, CreateArgs, UpdateArgs};
use nexus_workbench::attachments::{Chunk, FileInput, Progress};
use nexus_workbench::cleanup::{Report, Rules};
use nexus_workbench::event::{Decision, Image, Project, StoredEvent, ThreadSummary};
use nexus_workbench::project_defaults::ProjectDefaults;
use nexus_workbench::schedules::{ScheduleInput, ScheduleView, WebhookSecret};
use nexus_workbench::scripts::ProjectScript;
use nexus_workbench::{GitState, Workbench, draft};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

/// How long a command waits for the service to start or come back before it fails.
const CONNECT_WAIT: Duration = Duration::from_secs(20);

/// A store read or write off the async threads, as `nexus_workbench::api` runs its own.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Each clone's stage and percentage, by request.
type Clones = Arc<Mutex<HashMap<String, (String, Option<u8>)>>>;

/// An app's Code commands, on its own workbench or on the service.
#[derive(Clone)]
pub struct Engine {
    /// The app's workbench. Over the service it keeps no sessions (its store is in memory), and reads only what
    /// is this computer's: the agents installed and their models.
    api: Arc<Api>,
    /// The connection to the background service, when it owns the store.
    service: Option<Remote>,
    /// Each clone's progress as the service last said it, by request: the form polls it from the UI thread.
    clones: Clones,
}

impl Engine {
    pub async fn computer_list(&self) -> Result<serde_json::Value, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_computer_list", json!({})).await,
            None => self.api.computer_list().await,
        }
    }
    pub async fn computer_request(
        &self,
        computer_id: String,
        request_id: String,
        operation: nexus_workbench::computer::Operation,
    ) -> Result<nexus_workbench::computer::CallResult, String> {
        match &self.service {
            Some(remote) => {
                Self::relay(
                    remote,
                    "wb_computer_request",
                    json!({"computerId":computer_id,"requestId":request_id,"operation":operation}),
                )
                .await
            }
            None => self.api.computer_request(computer_id, request_id, operation).await,
        }
    }
    pub async fn computer_request_get(
        &self,
        computer_id: String,
        request_id: String,
    ) -> Result<nexus_workbench::computer::CallResult, String> {
        match &self.service {
            Some(remote) => {
                Self::relay(
                    remote,
                    "wb_computer_request_get",
                    json!({"computerId":computer_id,"requestId":request_id}),
                )
                .await
            }
            None => self.api.computer_request_get(computer_id, request_id).await,
        }
    }
    pub async fn synced_thread_page(
        &self,
        thread_id: String,
        before: Option<i64>,
        after: Option<i64>,
        limit: Option<usize>,
    ) -> Result<serde_json::Value, String> {
        match &self.service {
            Some(remote) => {
                Self::relay(
                    remote,
                    "wb_synced_thread_page",
                    json!({"threadId":thread_id,"before":before,"after":after,"limit":limit}),
                )
                .await
            }
            None => self.api.synced_thread_page(thread_id, before, after, limit).await,
        }
    }
    pub async fn synced_command(
        &self,
        thread_id: String,
        request_id: String,
        action: nexus_workbench::session_sync::control::Action,
    ) -> Result<serde_json::Value, String> {
        match &self.service {
            Some(remote) => {
                Self::relay(
                    remote,
                    "wb_synced_command",
                    json!({"threadId":thread_id,"requestId":request_id,"action":action}),
                )
                .await
            }
            None => self.api.synced_command(thread_id, request_id, action).await,
        }
    }
    pub async fn synced_command_get(&self, thread_id: String, request_id: String) -> Result<serde_json::Value, String> {
        match &self.service {
            Some(remote) => {
                Self::relay(
                    remote,
                    "wb_synced_command_get",
                    json!({"threadId":thread_id,"requestId":request_id}),
                )
                .await
            }
            None => self.api.synced_command_get(thread_id, request_id).await,
        }
    }
    pub async fn computer_info(&self) -> Result<nexus_workbench::computer::Info, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_computer_info", json!({})).await,
            None => self.api.computer_info().await,
        }
    }
    pub async fn computer_name_set(&self, name: String) -> Result<nexus_workbench::computer::Info, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_computer_name_set", json!({"name":name})).await,
            None => self.api.computer_name_set(name).await,
        }
    }
    pub async fn computer_join_begin(&self, link: String) -> Result<nexus_workbench::computer::JoinStatus, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_computer_join_begin", json!({"link":link})).await,
            None => self.api.computer_join_begin(link).await,
        }
    }

    pub async fn computer_join_poll(&self, invite_id: String) -> Result<nexus_workbench::computer::JoinStatus, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_computer_join_poll", json!({"inviteId":invite_id})).await,
            None => self.api.computer_join_poll(invite_id).await,
        }
    }

    pub async fn computer_join_allow(
        &self,
        invite_id: String,
    ) -> Result<nexus_workbench::session_sync::control::Settings, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_computer_join_allow", json!({"inviteId":invite_id})).await,
            None => self.api.computer_join_allow(invite_id).await,
        }
    }

    pub async fn phone_invitation_begin(&self) -> Result<nexus_workbench::computer::Invitation, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_phone_invitation_begin", json!({})).await,
            None => self.api.phone_invitation_begin().await,
        }
    }
    pub async fn phone_invitation_poll(
        &self,
        invite_id: String,
    ) -> Result<nexus_workbench::computer::LinkStatus, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_phone_invitation_poll", json!({"inviteId":invite_id})).await,
            None => self.api.phone_invitation_poll(invite_id).await,
        }
    }
    pub async fn phone_invitation_approve(
        &self,
        invite_id: String,
        device_id: String,
        code: String,
    ) -> Result<nexus_workbench::computer::LinkStatus, String> {
        match &self.service {
            Some(remote) => {
                Self::relay(
                    remote,
                    "wb_phone_invitation_approve",
                    json!({"inviteId":invite_id,"deviceId":device_id,"code":code}),
                )
                .await
            }
            None => self.api.phone_invitation_approve(invite_id, device_id, code).await,
        }
    }
    pub async fn phone_invitation_cancel(&self, invite_id: String) -> Result<(), String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_phone_invitation_cancel", json!({"inviteId":invite_id})).await,
            None => self.api.phone_invitation_cancel(invite_id).await,
        }
    }
    pub async fn local_history(
        &self,
        cursor: Option<String>,
        limit: Option<usize>,
    ) -> Result<nexus_workbench::computer::HistoryPage, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_local_history", json!({"cursor":cursor,"limit":limit})).await,
            None => self.api.local_history(cursor, limit).await,
        }
    }
    pub async fn code_control(&self) -> Result<nexus_workbench::session_sync::control::Settings, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_code_control", json!({})).await,
            None => self.api.code_control().await,
        }
    }

    pub async fn set_code_control(
        &self,
        enabled: bool,
    ) -> Result<nexus_workbench::session_sync::control::Settings, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_code_control_set", json!({"enabled":enabled})).await,
            None => self.api.set_code_control(enabled).await,
        }
    }

    /// The commands on the app's own workbench.
    pub fn here(api: Arc<Api>) -> Engine {
        Engine {
            api,
            service: None,
            clones: Arc::default(),
        }
    }

    /// The commands on the service, over `remote`. `api` answers what is this computer's.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn service(api: Arc<Api>, remote: Remote) -> Engine {
        Engine {
            api,
            service: Some(remote),
            clones: Arc::default(),
        }
    }

    /// The connection to the service, when the commands go there.
    pub fn remote(&self) -> Option<&Remote> {
        self.service.as_ref()
    }

    /// The app's workbench: the store's when the app holds it, else one in memory.
    pub fn workbench(&self) -> &Workbench {
        self.api.workbench()
    }

    /// `call` on the service with `args`, its answer read as `T`. Waits for the service to start (or come back) before
    /// it fails.
    async fn relay<T: DeserializeOwned>(remote: &Remote, call: &str, args: Value) -> Result<T, String> {
        Self::relay_guarded(remote, call, args, None).await
    }

    async fn relay_guarded<T: DeserializeOwned>(
        remote: &Remote,
        call: &str,
        args: Value,
        valid: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) -> Result<T, String> {
        let revoked = || valid.is_some_and(|valid| !valid());
        let waited = std::time::Instant::now();
        while !matches!(remote.status(), Status::Connected { .. }) && waited.elapsed() < CONNECT_WAIT {
            if revoked() {
                return Err("This command's original source authority expired.".into());
            }
            if matches!(remote.status(), Status::NeedsUpdate { .. } | Status::Closed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if revoked() {
            return Err("This command's original source authority expired.".into());
        }
        let answer = remote
            .call_if_current(call, args, || !revoked())
            .await
            .map_err(|error| error.replace("this machine", "the background service"))?;
        serde_json::from_value(answer).map_err(|error| format!("The background service's answer to {call}: {error}"))
    }

    /// Arguments as JSON. The workbench's argument types all serialize.
    fn args(value: impl serde::Serialize) -> Value {
        serde_json::to_value(value).unwrap_or(Value::Null)
    }

    /// The agents on this computer and their models. The service runs on this computer too, so the app reads them.
    pub async fn agent_models(&self) -> Result<Vec<AgentInfo>, String> {
        self.api.agent_models().await
    }

    /// [`Engine::agent_models`], with the whole list handed to `landed` as each agent's discovery ends.
    pub async fn agent_models_each(&self, landed: impl FnMut(Vec<AgentInfo>) + Send) -> Result<Vec<AgentInfo>, String> {
        self.api.agent_models_each(landed).await
    }

    pub async fn send_files(
        &self,
        thread_id: &str,
        text: &str,
        images: Vec<Image>,
        files: Vec<FileInput>,
    ) -> Result<(), String> {
        self.send_files_as(thread_id, text, images, files, false).await
    }

    /// [`Engine::send_files`], `queued`: it waits for the running turn to end rather than joining it
    /// (`Api::send_files_as`). Sent as `queued` only when set, so a plain message reads as before on an older service.
    pub async fn send_files_as(
        &self,
        thread_id: &str,
        text: &str,
        images: Vec<Image>,
        files: Vec<FileInput>,
        queued: bool,
    ) -> Result<(), String> {
        match &self.service {
            Some(remote) => {
                let mut args = json!({ "threadId": thread_id, "text": text, "images": images, "files": files });
                if queued {
                    args["queued"] = Value::Bool(true);
                }
                Self::relay::<Value>(remote, "wb_send_files", args).await.map(drop)
            }
            None => self.api.send_files_as(thread_id, text, images, files, queued).await,
        }
    }

    pub async fn attachment_chunk(&self, chunk: Chunk) -> Result<Progress, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_attachment_chunk", Self::args(chunk)).await,
            None => self.api.attachment_chunk(chunk).await,
        }
    }

    /// Deletes upload `upload_id` made for session `thread_id`.
    pub async fn attachment_cancel(&self, upload_id: String, thread_id: String) -> Result<(), String> {
        match &self.service {
            Some(remote) => Self::relay::<Value>(
                remote,
                "wb_attachment_cancel",
                json!({ "uploadId": upload_id, "threadId": thread_id }),
            )
            .await
            .map(drop),
            None => self.api.attachment_cancel(upload_id, Some(thread_id)).await,
        }
    }

    pub async fn commit(
        &self,
        thread_id: &str,
        repo: Option<&str>,
        message: &str,
        push: bool,
    ) -> Result<GitState, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "threadId": thread_id, "repo": repo, "message": message, "push": push });
                Self::relay(remote, "wb_commit", args).await
            }
            None => self.api.commit(thread_id, repo, message, push).await,
        }
    }

    /// What "Write it" drafts from: the change in the session's folder (or `repo` inside it), read where it is.
    pub async fn draft_context(
        &self,
        thread_id: &str,
        repo: Option<&str>,
        kind: draft::Kind,
    ) -> Result<draft::Context, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "threadId": thread_id, "repo": repo, "kind": kind });
                Self::relay(remote, "wb_draft_context", args).await
            }
            None => self.api.draft_context(thread_id, repo, kind).await,
        }
    }

    /// The session's folder (or `repo` inside it) as git sees it: the top bar's git panel.
    pub async fn git_state(&self, thread_id: &str, repo: Option<&str>) -> Result<GitState, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_git_state", json!({ "threadId": thread_id, "repo": repo })).await,
            None => self.api.git_state(thread_id, repo).await,
        }
    }

    /// Fetches the branch's remote, then reads the state again.
    pub async fn git_fetch(&self, thread_id: &str, repo: Option<&str>) -> Result<GitState, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_git_fetch", json!({ "threadId": thread_id, "repo": repo })).await,
            None => self.api.git_fetch(thread_id, repo).await,
        }
    }

    /// Checks out another local branch of the session's folder (or `repo` inside it).
    pub async fn switch_branch(&self, thread_id: &str, repo: Option<&str>, branch: &str) -> Result<GitState, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "threadId": thread_id, "repo": repo, "branch": branch });
                Self::relay(remote, "wb_switch_branch", args).await
            }
            None => self.api.switch_branch(thread_id, repo, branch).await,
        }
    }

    /// Makes a branch at the current commit and checks it out.
    pub async fn branch_create(&self, thread_id: &str, repo: Option<&str>, branch: &str) -> Result<GitState, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "threadId": thread_id, "repo": repo, "branch": branch });
                Self::relay(remote, "wb_branch_create", args).await
            }
            None => self.api.branch_create(thread_id, repo, branch).await,
        }
    }

    /// Brings the branch up to date with its upstream; `rebase` replays its own commits on top.
    pub async fn git_pull(
        &self,
        thread_id: &str,
        repo: Option<&str>,
        rebase: bool,
    ) -> Result<nexus_workbench::sync::Pull, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "threadId": thread_id, "repo": repo, "rebase": rebase });
                Self::relay(remote, "wb_git_pull", args).await
            }
            None => self.api.git_pull(thread_id, repo, rebase).await,
        }
    }

    /// Opens a pull request from the session's branch with `title` and `body`.
    pub async fn pull_request(&self, thread_id: &str, title: &str, body: &str) -> Result<ThreadSummary, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "threadId": thread_id, "title": title, "body": body });
                Self::relay(remote, "wb_pull_request", args).await
            }
            None => self.api.pull_request(thread_id, title, body).await,
        }
    }

    /// The commands that answer nothing and take only the session: `wb_interrupt` and `wb_stop`.
    async fn on_thread(&self, call: &str, thread_id: &str) -> Result<(), String> {
        match &self.service {
            Some(remote) => Self::relay::<Value>(remote, call, json!({ "threadId": thread_id }))
                .await
                .map(drop),
            None if call == "wb_interrupt" => self.api.interrupt(thread_id),
            None => self.api.stop(thread_id),
        }
    }

    pub async fn interrupt(&self, thread_id: &str) -> Result<(), String> {
        self.on_thread("wb_interrupt", thread_id).await
    }

    /// A retained owner's interrupt keeps its original authority through service reconnect waits.
    pub async fn interrupt_guarded(
        &self,
        thread_id: &str,
        valid: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<(), String> {
        if !valid() {
            return Err("This command's original source authority expired.".into());
        }
        match &self.service {
            Some(remote) => {
                Self::relay_guarded::<Value>(remote, "wb_interrupt", json!({ "threadId": thread_id }), Some(valid))
                    .await
                    .map(drop)
            }
            None => self.api.interrupt(thread_id),
        }
    }

    /// The person's action on a session's shared terminal, from the app window that checked it. Over the background
    /// service it goes as [`nexus_workbench::terminal::SERVICE_CALL`], which only the app's own service answers; here,
    /// to this build's workbench. Either way with the person's authority (`human`).
    pub async fn terminal_guarded(
        &self,
        thread_id: &str,
        op: &str,
        args: Value,
        valid: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Value, String> {
        if !valid() {
            return Err("terminal_not_found".into());
        }
        match &self.service {
            Some(remote) => {
                let call = json!({ "threadId": thread_id, "op": op, "args": args });
                Self::relay_guarded::<Value>(remote, nexus_workbench::terminal::SERVICE_CALL, call, Some(valid)).await
            }
            None => {
                self.api
                    .workbench()
                    .terminal_guarded(thread_id, op, args, true, valid)
                    .await
            }
        }
    }

    pub async fn stop(&self, thread_id: &str) -> Result<(), String> {
        self.on_thread("wb_stop", thread_id).await
    }

    pub async fn approve(
        &self,
        thread_id: String,
        request_id: String,
        decision: Decision,
        message: Option<String>,
        suggestion: Option<usize>,
        answers: Option<Map<String, Value>>,
    ) -> Result<(), String> {
        match &self.service {
            Some(remote) => {
                let args = json!({
                    "threadId": thread_id, "requestId": request_id, "decision": decision, "message": message,
                    "suggestion": suggestion, "answers": answers,
                });
                Self::relay::<Value>(remote, "wb_approve", args).await.map(drop)
            }
            None => {
                self.api
                    .approve(thread_id, request_id, decision, message, suggestion, answers)
                    .await
            }
        }
    }

    pub async fn thread_update(&self, args: UpdateArgs) -> Result<ThreadSummary, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_thread_update", Self::args(args)).await,
            None => self.api.thread_update(args).await,
        }
    }

    pub async fn queue_cancel(&self, thread_id: String, seq: i64) -> Result<(), String> {
        match &self.service {
            Some(remote) => {
                Self::relay::<Value>(remote, "wb_queue_cancel", json!({ "threadId": thread_id, "seq": seq }))
                    .await
                    .map(drop)
            }
            None => self.api.queue_cancel(thread_id, seq).await,
        }
    }

    pub async fn queue_edit(&self, thread_id: String, seq: i64, text: String) -> Result<(), String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "threadId": thread_id, "seq": seq, "text": text });
                Self::relay::<Value>(remote, "wb_queue_edit", args).await.map(drop)
            }
            None => self.api.queue_edit(thread_id, seq, text).await,
        }
    }

    pub async fn queue_move(&self, thread_id: String, seq: i64, before: Option<i64>) -> Result<(), String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "threadId": thread_id, "seq": seq, "before": before });
                Self::relay::<Value>(remote, "wb_queue_move", args).await.map(drop)
            }
            None => self.api.queue_move(thread_id, seq, before).await,
        }
    }

    pub async fn queue_steer(&self, thread_id: String, seq: i64) -> Result<(), String> {
        match &self.service {
            Some(remote) => {
                Self::relay::<Value>(remote, "wb_queue_steer", json!({ "threadId": thread_id, "seq": seq }))
                    .await
                    .map(drop)
            }
            None => self.api.queue_steer(thread_id, seq).await,
        }
    }

    /// None, or what stayed behind (`Api::thread_delete`).
    pub async fn thread_delete(&self, thread_id: &str) -> Result<Option<String>, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_thread_delete", json!({ "threadId": thread_id })).await,
            None => self.api.thread_delete(thread_id).await,
        }
    }

    pub async fn worktree_remove(&self, thread_id: &str) -> Result<ThreadSummary, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_worktree_remove", json!({ "threadId": thread_id })).await,
            None => self.api.worktree_remove(thread_id).await,
        }
    }

    /// Every worktree Settings › Worktrees lists: the sessions' and the ones no session names.
    pub async fn worktrees(&self) -> Result<Vec<nexus_workbench::cleanup::Listed>, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_worktrees", json!({})).await,
            None => self.api.worktrees().await,
        }
    }

    /// Removes a worktree no session names, unless that would lose work.
    pub async fn worktree_remove_orphan(&self, path: &str) -> Result<(), String> {
        match &self.service {
            Some(remote) => Self::relay::<Value>(remote, "wb_worktree_remove_orphan", json!({ "path": path }))
                .await
                .map(drop),
            None => self.api.worktree_remove_orphan(path).await,
        }
    }

    /// The worktree sweep by `rules`. Over the service, it sweeps its own sessions, as a server's host does.
    pub async fn worktree_cleanup(&self, rules: &Rules) -> Result<Report, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_worktree_cleanup", json!({ "rules": rules })).await,
            None => self.api.worktree_cleanup(rules, None).await,
        }
    }

    pub async fn thread_events(&self, thread_id: String, after_seq: i64) -> Result<Vec<StoredEvent>, String> {
        match &self.service {
            Some(remote) => {
                Self::relay(
                    remote,
                    "wb_thread_events",
                    json!({ "threadId": thread_id, "afterSeq": after_seq }),
                )
                .await
            }
            None => self.api.thread_events(thread_id, after_seq).await,
        }
    }

    pub async fn thread_fork(&self, thread_id: &str, turn: Option<i64>) -> Result<ThreadSummary, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_thread_fork", json!({ "threadId": thread_id, "turn": turn })).await,
            None => self.api.thread_fork(thread_id, turn).await,
        }
    }

    pub async fn thread_fork_with_subscription(
        &self,
        thread_id: &str,
        turn: Option<i64>,
        subscription: Option<nexus_workbench::ForkSubscription>,
    ) -> Result<ThreadSummary, String> {
        match &self.service {
            Some(remote) => {
                Self::relay(
                    remote,
                    "wb_thread_fork",
                    json!({ "threadId": thread_id, "turn": turn, "subscription": subscription }),
                )
                .await
            }
            None => {
                self.api
                    .thread_fork_with_subscription(thread_id, turn, subscription)
                    .await
            }
        }
    }

    /// How many files going back to the turn ending at event `turn` would change (`wb_revert_preview`).
    pub async fn revert_preview(&self, thread_id: &str, turn: i64) -> Result<u32, String> {
        match &self.service {
            Some(remote) => {
                Self::relay(
                    remote,
                    "wb_revert_preview",
                    json!({ "threadId": thread_id, "turn": turn }),
                )
                .await
            }
            None => self.api.revert_preview(thread_id, turn).await,
        }
    }

    /// Puts the session's files back as they were when the turn ending at event `turn` ended (`wb_revert_to_turn`).
    pub async fn revert_to_turn(&self, thread_id: &str, turn: i64) -> Result<u32, String> {
        match &self.service {
            Some(remote) => {
                Self::relay(
                    remote,
                    "wb_revert_to_turn",
                    json!({ "threadId": thread_id, "turn": turn }),
                )
                .await
            }
            None => self.api.revert_to_turn(thread_id, turn).await,
        }
    }

    /// Takes a message and what followed it back (`wb_rewind_message`).
    pub async fn rewind_message(&self, thread_id: &str, seq: i64) -> Result<(), String> {
        match &self.service {
            Some(remote) => Self::relay::<Value>(
                remote,
                "wb_rewind_message",
                json!({ "threadId": thread_id, "seq": seq }),
            )
            .await
            .map(drop),
            None => self.api.rewind_message(thread_id, seq).await,
        }
    }

    pub async fn project_add(&self, path: String) -> Result<Project, String> {
        self.project_add_guarded(path, None).await
    }

    pub async fn project_add_guarded(
        &self,
        path: String,
        valid: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) -> Result<Project, String> {
        if valid.is_some_and(|valid| !valid()) {
            return Err(
                "The active workspace changed before the folder could be added. Choose the folder again.".into(),
            );
        }
        match &self.service {
            Some(remote) => Self::relay_guarded(remote, "wb_project_add", json!({ "path": path }), valid).await,
            None => self.api.project_add(path).await,
        }
    }

    /// The folders in `path`, for Add a project's folder field (`wb_dirs`).
    pub async fn dirs(&self, path: String) -> Result<nexus_workbench::dirs::Dirs, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_dirs", json!({ "path": path })).await,
            None => self.api.dirs(path).await,
        }
    }

    pub async fn project_clone(&self, source: String, parent: String, request_id: String) -> Result<Project, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "source": source, "parent": parent, "requestId": request_id });
                Self::relay(remote, "wb_project_clone", args).await
            }
            None => self.api.project_clone(source, parent, request_id).await,
        }
    }

    /// Git's progress on a clone: its stage, and how far as a percentage. Over the service the clone runs there: the
    /// answer is what it said at the last ask, and a new ask (`wb_project_clone_progress`) goes
    /// out on `runtime` for the next one.
    pub fn clone_progress(&self, request_id: &str, runtime: &tokio::runtime::Handle) -> Option<(String, Option<u8>)> {
        if let Some(remote) = self.service.clone() {
            let (clones, id) = (self.clones.clone(), request_id.to_string());
            runtime.spawn(async move {
                let asked = remote
                    .call("wb_project_clone_progress", json!({ "requestId": id }))
                    .await;
                if let Ok(progress) = asked
                    && let Some(stage) = progress.get("stage").and_then(Value::as_str)
                {
                    let percent = progress
                        .get("percent")
                        .and_then(Value::as_u64)
                        .and_then(|percent| u8::try_from(percent).ok());
                    lock(&clones).insert(id, (stage.to_string(), percent));
                }
            });
            return lock(&self.clones).get(request_id).cloned();
        }
        self.api
            .clone_progress(request_id)
            .map(|progress| (progress.stage, progress.percent))
    }

    /// The organization's sessions and the projects, for outside access. Over the service, its `wb_overview`.
    pub async fn sessions(&self) -> Result<(Vec<ThreadSummary>, Vec<Project>), String> {
        match &self.service {
            Some(remote) => {
                let overview: Value = Self::relay(remote, "wb_overview", Value::Null).await?;
                fn read<T: DeserializeOwned>(overview: &Value, key: &str) -> Result<T, String> {
                    serde_json::from_value(overview.get(key).cloned().unwrap_or_default())
                        .map_err(|error| format!("The background service's {key}: {error}"))
                }
                Ok((read(&overview, "threads")?, read(&overview, "projects")?))
            }
            None => {
                let wb = self.api.workbench();
                Ok((wb.threads()?, wb.projects()?))
            }
        }
    }

    /// A workbench command, on the store owner. The API validates its name and arguments.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, name, args).await,
            None => self.api.call(name, args).await,
        }
    }

    /// A session's latest `limit` events, oldest first (`wb_thread_page` with no `beforeSeq`).
    pub async fn latest_events(&self, thread_id: String, limit: usize) -> Result<Vec<StoredEvent>, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "threadId": thread_id, "limit": limit });
                let page: Value = Self::relay(remote, "wb_thread_page", args).await?;
                serde_json::from_value(page.get("events").cloned().unwrap_or_default())
                    .map_err(|error| format!("The background service's events: {error}"))
            }
            None => Ok(self.api.thread_page(thread_id, None, Some(limit)).await?.events),
        }
    }

    pub async fn project_update(
        &self,
        project_id: String,
        pinned: Option<bool>,
        archived: Option<bool>,
    ) -> Result<(), String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "projectId": project_id, "pinned": pinned, "archived": archived });
                Self::relay::<Value>(remote, "wb_project_update", args).await.map(drop)
            }
            None => self.api.project_update(project_id, pinned, archived).await.map(drop),
        }
    }

    /// Set the project's setup script (blank or None takes it away); answers with what was saved.
    pub async fn project_setup_set(
        &self,
        project_id: String,
        script: Option<String>,
    ) -> Result<Option<String>, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "projectId": project_id, "script": script });
                Self::relay(remote, "wb_project_setup_set", args).await
            }
            None => self.api.project_setup_set(project_id, script).await,
        }
    }

    /// Replace the project's commands (an empty list takes them away); answers with what was saved.
    pub async fn project_scripts_set(
        &self,
        project_id: String,
        scripts: Vec<ProjectScript>,
    ) -> Result<Vec<ProjectScript>, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "projectId": project_id, "scripts": scripts });
                Self::relay(remote, "wb_project_scripts_set", args).await
            }
            None => self.api.project_scripts_set(project_id, scripts).await,
        }
    }

    /// The project's new-session defaults (`nexus_workbench::project_defaults`).
    pub async fn project_defaults(&self, project_id: String) -> Result<ProjectDefaults, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_project_defaults", json!({ "projectId": project_id })).await,
            None => self.api.project_defaults(project_id).await,
        }
    }

    /// Set them (all unset takes them away); answers with what was saved.
    pub async fn project_defaults_set(
        &self,
        project_id: String,
        defaults: ProjectDefaults,
    ) -> Result<ProjectDefaults, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "projectId": project_id, "defaults": defaults });
                Self::relay(remote, "wb_project_defaults_set", args).await
            }
            None => self.api.project_defaults_set(project_id, defaults).await,
        }
    }

    pub async fn session_start_overrides(&self, clear: bool) -> Result<u32, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_session_start_overrides", json!({ "clear": clear })).await,
            None => {
                let wb = self.api.workbench().clone();
                blocking(move || wb.session_start_overrides(clear)).await
            }
        }
    }

    /// A new session. Over the service, a new worktree's branch takes `branch_prefix`, which a host does not keep.
    pub async fn thread_create(
        &self,
        args: CreateArgs,
        branch_prefix: Option<String>,
    ) -> Result<ThreadSummary, String> {
        self.thread_create_at_scope(args, branch_prefix, None, None).await
    }

    pub async fn thread_create_at_scope(
        &self,
        mut args: CreateArgs,
        branch_prefix: Option<String>,
        source: Option<&nexus_workbench::api::StartAuthority>,
        current: Option<&(dyn Fn() -> bool + Send + Sync)>,
    ) -> Result<ThreadSummary, String> {
        if current.is_some_and(|guard| !guard()) {
            return Err("This command's original source authority expired.".into());
        }
        match &self.service {
            Some(remote) => {
                if let Some(worktree) = args.worktree.as_mut()
                    && worktree.review.is_none()
                    && worktree.branch_prefix.is_none()
                {
                    worktree.branch_prefix = branch_prefix;
                }
                let mut args = Self::args(args);
                let call = if let Some(source) = source {
                    args["source"] = json!(source);
                    "wb_thread_create_bound"
                } else {
                    "wb_thread_create"
                };
                Self::relay_guarded(remote, call, args, current).await
            }
            None => self.api.thread_create_at_scope(args, source).await,
        }
    }

    pub async fn schedules(&self) -> Result<Vec<ScheduleView>, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_schedules", Value::Null).await,
            None => self.api.schedules().await,
        }
    }

    pub async fn schedule_save(
        &self,
        schedule_id: Option<String>,
        schedule: ScheduleInput,
    ) -> Result<ScheduleView, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "scheduleId": schedule_id, "schedule": schedule });
                Self::relay(remote, "wb_schedule_save", args).await
            }
            None => self.api.schedule_save(schedule_id, schedule).await,
        }
    }

    pub async fn schedule_delete(&self, schedule_id: String) -> Result<(), String> {
        match &self.service {
            Some(remote) => Self::relay::<Value>(remote, "wb_schedule_delete", json!({ "scheduleId": schedule_id }))
                .await
                .map(drop),
            None => self.api.schedule_delete(schedule_id).await,
        }
    }

    pub async fn schedule_run(&self, schedule_id: &str) -> Result<ThreadSummary, String> {
        match &self.service {
            Some(remote) => Self::relay(remote, "wb_schedule_run", json!({ "scheduleId": schedule_id })).await,
            None => self.api.schedule_run(schedule_id).await,
        }
    }

    pub async fn schedule_webhook(&self, schedule_id: &str, on: bool) -> Result<Option<WebhookSecret>, String> {
        match &self.service {
            Some(remote) => {
                let args = json!({ "scheduleId": schedule_id, "on": on });
                Self::relay(remote, "wb_schedule_webhook", args).await
            }
            None => self.api.schedule_webhook(schedule_id, on).await,
        }
    }
}

#[cfg(test)]
mod interrupt_tests;
