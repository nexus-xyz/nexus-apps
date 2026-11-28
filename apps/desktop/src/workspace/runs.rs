//! Where a Code session's commands go: this computer's runner (`code_run.rs`), or the server the session or project is
//! on (`machines.rs`). A server takes the same `wb_*` calls, with the same arguments,
//! relayed through `machine_call`. What a server's command reports comes back to the window as the runner's events do, so a new
//! session opens and a failure shows the same way.

use std::{path::PathBuf, rc::Rc};

use futures::channel::mpsc::UnboundedSender;
use futures::channel::oneshot;
use futures::future::{BoxFuture, FutureExt};
use nexus_workbench::api::{CreateArgs, UpdateArgs, WorktreeArgs};
use nexus_workbench::event::{Decision, Effort, PermissionMode, ThreadSummary};
use nexus_workbench::{GitState, draft};
use serde_json::{Map, Value, json};

use super::Workspace;
use crate::code_run::{CodeEvent, CodeRunner, NewSession};
use crate::machines::Machines;

/// How long `Runs::call` waits for a command's answer.
const CALL_LIMIT: std::time::Duration = std::time::Duration::from_secs(25);

/// How a command's answer came back.
enum Settled {
    Answered(Result<Value, String>),
    /// The other side dropped the answer.
    Lost,
    /// Past the wait's limit.
    TimedOut,
}

/// What `then` makes of `read`'s answer. An answer already in (an unknown machine, a refused call) goes to `then` here,
/// on the caller's thread. Only an answer still on its way waits on the `nexus-code` runtime, up to `limit`.
///
/// The hop matters in tests: fixtures answer at once, and their callers run on GPUI's test scheduler. A wake from a
/// runtime thread fails the test with "Your test is not deterministic" whenever the caller polled first.
fn settle<T: Send + 'static>(
    mut read: oneshot::Receiver<Result<Value, String>>,
    limit: Option<std::time::Duration>,
    then: impl FnOnce(Settled) -> T + Send + 'static,
) -> oneshot::Receiver<T> {
    let (send, receive) = oneshot::channel();
    match read.try_recv() {
        Ok(Some(answer)) => {
            let _ = send.send(then(Settled::Answered(answer)));
        }
        Err(oneshot::Canceled) => {
            let _ = send.send(then(Settled::Lost));
        }
        Ok(None) => {
            crate::code_run::runtime().spawn(async move {
                let answer = match limit {
                    Some(limit) => match tokio::time::timeout(limit, read).await {
                        Ok(Ok(answer)) => Settled::Answered(answer),
                        Ok(Err(_)) => Settled::Lost,
                        Err(_) => Settled::TimedOut,
                    },
                    None => match read.await {
                        Ok(answer) => Settled::Answered(answer),
                        Err(_) => Settled::Lost,
                    },
                };
                let _ = send.send(then(answer));
            });
        }
    }
    receive
}

/// What `then` makes of `read`'s answer, awaited by the caller with no runtime hop and no limit. A caller that awaits the
/// answer itself needs no runtime: in tests, a fixture that answers later wakes the caller from the test's own thread.
fn settle_here<T: Send + 'static>(
    read: oneshot::Receiver<Result<Value, String>>,
    then: impl FnOnce(Settled) -> T + Send + 'static,
) -> BoxFuture<'static, T> {
    async move {
        then(match read.await {
            Ok(answer) => Settled::Answered(answer),
            Err(_) => Settled::Lost,
        })
    }
    .boxed()
}

/// The original store or machine, independently of the runner/UI entity that is attached now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum Source {
    Local { data: PathBuf, scope: String },
    Machine(String),
}

/// A session's or a project's commands.
#[derive(Clone)]
pub(crate) enum Runs {
    Here(Rc<CodeRunner>),
    There(Relay),
}

#[cfg(test)]
type FixtureCall =
    std::sync::Arc<dyn Fn(&'static str, Value) -> oneshot::Receiver<Result<Value, String>> + Send + Sync>;

/// A server's commands, and where their results go.
#[derive(Clone)]
pub(crate) struct Relay {
    #[cfg(test)]
    calls: Option<FixtureCall>,
    #[cfg(test)]
    source: Option<Source>,
    machine: String,
    machines: std::sync::Arc<Machines>,
    events: UnboundedSender<CodeEvent>,
}

impl Workspace {
    /// The commands for session or project `id`: its server's, else this computer's runner (None while the Nexus app
    /// holds this computer's sessions).
    pub(super) fn runs(&self, id: &str) -> Option<Runs> {
        #[cfg(test)]
        if let Some(runs) = self.run_fixtures.get(id) {
            return Some(runs.clone());
        }
        if self.is_local() {
            return self.runner.clone().map(Runs::Here);
        }
        let machines = crate::machines::get();
        match machines.machine_of(id) {
            Some(machine) => Some(Runs::There(Relay {
                #[cfg(test)]
                calls: None,
                #[cfg(test)]
                source: None,
                machine,
                machines,
                events: self.relay.clone(),
            })),
            None => self.runner.clone().map(Runs::Here),
        }
    }
}

impl Relay {
    /// `call` with `args` on the server; its answer goes to `then` on the runtime, and a failure to the window.
    fn run(&self, call: &'static str, args: Value, then: impl FnOnce(Value) -> Option<CodeEvent> + Send + 'static) {
        let answered = self.request(call, args);
        let events = self.events.clone();
        let _ = settle(answered, None, move |answer| {
            let event = match answer {
                Settled::Answered(Ok(value)) => then(value).unwrap_or(CodeEvent::Changed),
                Settled::Answered(Err(error)) => CodeEvent::Failed(crate::machine_view::plain_error(&error)),
                Settled::Lost | Settled::TimedOut => CodeEvent::Failed("The server's answer was lost.".into()),
            };
            let _ = events.unbounded_send(event);
        });
    }

    /// `call` with `args` on the server, or on the test's fixture transport when it has one.
    fn request(&self, call: &'static str, args: Value) -> oneshot::Receiver<Result<Value, String>> {
        #[cfg(test)]
        if let Some(calls) = &self.calls {
            return calls(call, args);
        }
        self.machines.call(&self.machine, call, args)
    }

    fn on_thread(&self, call: &'static str, thread_id: String) {
        self.run(call, json!({ "threadId": thread_id }), |_| None);
    }
}

impl Runs {
    pub(in crate::workspace) fn source_guard(&self) -> crate::side::browser::agent::Guard {
        match self {
            Self::Here(runner) => runner.source_guard(),
            Self::There(relay) => {
                #[cfg(test)]
                if matches!(relay.source, Some(Source::Local { .. })) {
                    return std::sync::Arc::new(|| true); // Immutable, per-instance deferred local fixture.
                }
                let _ = relay;
                std::sync::Arc::new(|| false) // Remote sources cannot authorize local CEF ownership.
            }
        }
    }
    /// The authoritative source locator; never substitute a local runner for a retained remote session.
    pub(in crate::workspace) fn source(&self) -> Source {
        match self {
            Self::Here(runner) => {
                let (data, scope) = runner.store_source();
                Source::Local { data, scope }
            }
            Self::There(relay) => {
                #[cfg(test)]
                if let Some(source) = &relay.source {
                    return source.clone();
                }
                Source::Machine(relay.machine.clone())
            }
        }
    }

    #[cfg(test)]
    pub(in crate::workspace) fn isolated() -> Self {
        let (events, _) = futures::channel::mpsc::unbounded();
        Self::There(Relay {
            calls: None,
            source: None,
            machine: "side-chat-fixture".into(),
            machines: super::machine_source::MachineSource::isolated().machines,
            events,
        })
    }

    #[cfg(test)]
    pub(in crate::workspace) fn deferred(machine: &str, calls: FixtureCall) -> Self {
        Self::deferred_source(Source::Machine(machine.into()), calls)
    }

    /// Instance-local deferred transport: never replaces process-global machine/runner behavior.
    #[cfg(test)]
    pub(in crate::workspace) fn deferred_source(source: Source, calls: FixtureCall) -> Self {
        let (events, _) = futures::channel::mpsc::unbounded();
        let machine = match &source {
            Source::Machine(id) => id.clone(),
            Source::Local { .. } => "fixture-local".into(),
        };
        Self::There(Relay {
            machine,
            machines: super::machine_source::MachineSource::isolated().machines,
            events,
            calls: Some(calls),
            source: Some(source),
        })
    }

    /// The same validated workbench command on a local, service, SSH or cloud session.
    pub fn call(&self, name: &'static str, args: Value) -> oneshot::Receiver<Result<Value, String>> {
        #[cfg(test)]
        if let Runs::There(relay) = self
            && let Some(calls) = &relay.calls
        {
            return calls(name, args);
        }
        let read = match self {
            Runs::Here(runner) => runner.git(move |api| async move { api.call(name, args).await }),
            Runs::There(relay) => relay.machines.call(&relay.machine, name, args),
        };
        settle(read, Some(CALL_LIMIT), |answer| match answer {
            Settled::Answered(result) => result,
            Settled::Lost => Err("The command's answer was lost. Read the current state before retrying.".into()),
            Settled::TimedOut => Err("The command timed out. Read the current state before retrying.".into()),
        })
    }

    /// Send a message (`CodeRunner::send`): `queued` waits for the running turn to end rather than joining it.
    pub fn send(&self, thread_id: String, text: String, attachments: crate::attachments::Draft, queued: bool) {
        match self {
            Runs::Here(runner) => runner.send(thread_id, text, attachments, queued),
            Runs::There(relay) => {
                let (machines, machine, events) = (relay.machines.clone(), relay.machine.clone(), relay.events.clone());
                let mut place = crate::outbox::join(&thread_id);
                crate::code_run::runtime().spawn(async move {
                    place.wait().await;
                    let sent = async {
                        let files = machines.upload(&machine, &thread_id, &attachments).await?;
                        let images = crate::attachments::session_images(&attachments);
                        // An older host must refuse files rather than take the text and drop them (`relayedCall`).
                        let call = if files.is_empty() { "wb_send" } else { "wb_send_files" };
                        let mut args = json!({ "threadId": thread_id, "text": text, "images": images, "files": files });
                        // Only when set: an older host ignores it, and the message joins the turn.
                        if queued {
                            args["queued"] = Value::Bool(true);
                        }
                        machines
                            .call(&machine, call, args)
                            .await
                            .map_err(|_| "The server's answer was lost.".to_string())?
                    };
                    let event = match sent.await {
                        Ok(_) => CodeEvent::Changed,
                        Err(error) => CodeEvent::Unsent {
                            thread_id,
                            text,
                            attachments,
                            error: crate::machine_view::plain_error(&error),
                        },
                    };
                    let _ = events.unbounded_send(event);
                });
            }
        }
    }

    /// Commit everything that changed in the session's folder as `message`: here, or on its server (`wb_commit`, the
    /// server's own git identity).
    pub fn commit(&self, thread_id: String, message: String) -> oneshot::Receiver<Result<GitState, String>> {
        let relay = match self {
            Runs::Here(runner) => return runner.commit(thread_id, message),
            Runs::There(relay) => relay,
        };
        let args = json!({ "threadId": thread_id, "repo": null, "message": message, "push": false });
        let committed = relay.machines.call(&relay.machine, "wb_commit", args);
        settle(committed, None, |answer| match answer {
            Settled::Answered(Ok(value)) => {
                serde_json::from_value::<GitState>(value).map_err(|error| error.to_string())
            }
            Settled::Answered(Err(error)) => Err(crate::machine_view::plain_error(&error)),
            Settled::Lost | Settled::TimedOut => Err("The server's answer was lost.".into()),
        })
    }

    /// What "Write it" drafts from (`wb_draft_context`): the change in the session's folder, or `repo` inside it, read
    /// here or on its server. The Gateway call is this computer's (`write_it.rs`): a server has no token.
    pub fn draft_context(
        &self,
        thread_id: String,
        repo: Option<String>,
        kind: draft::Kind,
    ) -> oneshot::Receiver<Result<draft::Context, String>> {
        let relay = match self {
            Runs::Here(runner) => {
                return runner
                    .git(move |api| async move { api.draft_context(&thread_id, repo.as_deref(), kind).await });
            }
            Runs::There(relay) => relay,
        };
        let args = json!({ "threadId": thread_id, "repo": repo, "kind": kind });
        let read = relay.machines.call(&relay.machine, "wb_draft_context", args);
        settle(read, None, |answer| match answer {
            Settled::Answered(Ok(value)) => {
                serde_json::from_value::<draft::Context>(value).map_err(|error| error.to_string())
            }
            Settled::Answered(Err(error)) => Err(crate::machine_view::plain_error(&error)),
            Settled::Lost | Settled::TimedOut => Err("The server's answer was lost.".into()),
        })
    }

    pub fn interrupt(&self, thread_id: String) {
        match self {
            Runs::Here(runner) => runner.interrupt(thread_id),
            Runs::There(relay) => relay.on_thread("wb_interrupt", thread_id),
        }
    }

    /// Only a frozen local browser owner can use this retained Stop path.
    pub(in crate::workspace) fn interrupt_guarded(&self, thread_id: String, valid: crate::side::browser::agent::Guard) {
        if !matches!(self.source(), Source::Local { .. }) {
            return;
        }
        match self {
            Runs::Here(runner) => runner.interrupt_guarded(thread_id, valid),
            // The only Local relay is the per-instance deferred fixture. Production remote sources are refused above.
            Runs::There(relay) => {
                if valid() {
                    relay.on_thread("wb_interrupt", thread_id);
                }
            }
        }
    }

    pub fn stop(&self, thread_id: String) {
        match self {
            Runs::Here(runner) => runner.stop(thread_id),
            Runs::There(relay) => relay.on_thread("wb_stop", thread_id),
        }
    }

    pub fn approve(
        &self,
        thread_id: String,
        request_id: String,
        decision: Decision,
        message: Option<String>,
        suggestion: Option<usize>,
        answers: Option<Map<String, Value>>,
    ) {
        match self {
            Runs::Here(runner) => runner.approve(thread_id, request_id, decision, message, suggestion, answers),
            Runs::There(relay) => relay.run(
                "wb_approve",
                json!({
                    "threadId": thread_id, "requestId": request_id, "decision": decision, "message": message,
                    "suggestion": suggestion, "answers": answers,
                }),
                |_| None,
            ),
        }
    }

    /// Approve the plan, then switch the session to `mode`, as on this computer.
    pub fn implement_plan(&self, thread_id: String, request_id: String, mode: PermissionMode, mode_name: &'static str) {
        let relay = match self {
            Runs::Here(runner) => return runner.implement_plan(thread_id, request_id, mode, mode_name),
            Runs::There(relay) => relay,
        };
        let (machines, machine, events) = (relay.machines.clone(), relay.machine.clone(), relay.events.clone());
        crate::code_run::runtime().spawn(async move {
            let lost = |_| "The server's answer was lost.".to_string();
            let approve = json!({ "threadId": thread_id, "requestId": request_id, "decision": Decision::Allow });
            let approved = machines
                .call(&machine, "wb_approve", approve)
                .await
                .map_err(lost)
                .and_then(|a| a);
            let event = match approved {
                Err(error) => CodeEvent::Failed(crate::machine_view::plain_error(&error)),
                Ok(_) => {
                    let mode = json!({ "threadId": thread_id, "permissionMode": mode.as_str() });
                    match machines
                        .call(&machine, "wb_thread_update", mode)
                        .await
                        .map_err(lost)
                        .and_then(|a| a)
                    {
                        Ok(_) => CodeEvent::Changed,
                        Err(_) => CodeEvent::Failed(crate::code_run::plan_mode_error(mode_name)),
                    }
                }
            };
            let _ = events.unbounded_send(event);
        });
    }

    pub fn queue_cancel(&self, thread_id: String, seq: i64) {
        match self {
            Runs::Here(runner) => runner.queue_cancel(thread_id, seq),
            Runs::There(relay) => relay.run("wb_queue_cancel", json!({ "threadId": thread_id, "seq": seq }), |_| {
                None
            }),
        }
    }

    pub fn queue_edit(&self, thread_id: String, seq: i64, text: String) {
        match self {
            Runs::Here(runner) => runner.queue_edit(thread_id, seq, text),
            Runs::There(relay) => relay.run(
                "wb_queue_edit",
                json!({ "threadId": thread_id, "seq": seq, "text": text }),
                |_| None,
            ),
        }
    }

    pub fn queue_move(&self, thread_id: String, seq: i64, before: Option<i64>) {
        match self {
            Runs::Here(runner) => runner.queue_move(thread_id, seq, before),
            Runs::There(relay) => relay.run(
                "wb_queue_move",
                json!({ "threadId": thread_id, "seq": seq, "before": before }),
                |_| None,
            ),
        }
    }

    pub fn queue_steer(&self, thread_id: String, seq: i64) {
        match self {
            Runs::Here(runner) => runner.queue_steer(thread_id, seq),
            Runs::There(relay) => relay.run("wb_queue_steer", json!({ "threadId": thread_id, "seq": seq }), |_| None),
        }
    }

    /// Change the session's model, effort or permission mode, as [`CodeRunner::update`].
    pub fn update(
        &self,
        thread_id: String,
        model: Option<Option<String>>,
        effort: Option<Option<Effort>>,
        mode: Option<PermissionMode>,
    ) {
        match self {
            Runs::Here(runner) => runner.update(thread_id, model, effort, mode),
            Runs::There(_) => self.update_session(UpdateArgs {
                thread_id,
                model: model.map(Option::unwrap_or_default),
                effort: effort.map(|effort| effort.map(|effort| effort.as_str().to_string()).unwrap_or_default()),
                permission_mode: mode.map(|mode| mode.as_str().to_string()),
                ..UpdateArgs::default()
            }),
        }
    }

    pub fn update_session(&self, args: UpdateArgs) {
        match self {
            Runs::Here(runner) => runner.update_session(args),
            Runs::There(relay) => relay.run("wb_thread_update", json!(args), |_| None),
        }
    }

    pub fn remove_worktree(&self, thread_id: String) {
        match self {
            Runs::Here(runner) => runner.remove_worktree(thread_id),
            Runs::There(relay) => relay.on_thread("wb_worktree_remove", thread_id),
        }
    }

    /// A new session that goes on from turn `turn` (the latest when None): its id.
    pub fn fork(
        &self,
        thread_id: String,
        turn: Option<i64>,
        subscription: Option<nexus_workbench::ForkSubscription>,
    ) -> BoxFuture<'static, Result<String, String>> {
        let relay = match self {
            Runs::Here(runner) => {
                let forked = runner.fork(thread_id, turn, subscription);
                return async move { forked.await.unwrap_or_else(|_| Err("The fork was cancelled.".into())) }.boxed();
            }
            Runs::There(relay) => relay,
        };
        let forked = relay.request(
            "wb_thread_fork",
            json!({ "threadId": thread_id, "turn": turn, "subscription": subscription }),
        );
        settle_here(forked, |answer| match answer {
            Settled::Answered(Ok(value)) => serde_json::from_value::<ThreadSummary>(value)
                .map(|thread| thread.id)
                .map_err(|error| error.to_string()),
            Settled::Answered(Err(error)) => Err(crate::machine_view::plain_error(&error)),
            Settled::Lost | Settled::TimedOut => Err("The server's answer was lost.".into()),
        })
    }

    /// Read the complete log, including events outside the transcript's visible window.
    pub fn session_events(
        &self,
        thread_id: String,
    ) -> BoxFuture<'static, Result<Vec<nexus_workbench::event::StoredEvent>, String>> {
        let relay = match self {
            Runs::Here(runner) => {
                let read = runner.session_events(thread_id);
                return async move { read.await.unwrap_or_else(|_| Err("The read was cancelled.".into())) }.boxed();
            }
            Runs::There(relay) => relay,
        };
        let request = relay.request("wb_thread_events", json!({ "threadId": thread_id, "afterSeq": 0 }));
        settle_here(request, |answer| match answer {
            Settled::Answered(Ok(value)) => serde_json::from_value(value).map_err(|error| error.to_string()),
            Settled::Answered(Err(error)) => Err(crate::machine_view::plain_error(&error)),
            Settled::Lost | Settled::TimedOut => Err("The server's answer was lost.".into()),
        })
    }

    pub fn update_project(&self, project_id: String, pinned: Option<bool>, archived: Option<bool>) {
        match self {
            Runs::Here(runner) => runner.update_project(project_id, pinned, archived),
            Runs::There(relay) => relay.run(
                "wb_project_update",
                json!({ "projectId": project_id, "pinned": pinned, "archived": archived }),
                |_| None,
            ),
        }
    }

    /// Make a session and send its first message. On a server, a new worktree's branch takes this computer's branch
    /// prefix, which a host does not keep.
    pub fn create(&self, draft: String, mut new: NewSession) {
        let relay = match self {
            Runs::Here(runner) => return runner.create(draft, new),
            Runs::There(relay) => relay,
        };
        if new.provider.is_some() {
            let _ = relay.events.unbounded_send(CodeEvent::StartFailed {
                draft,
                error: "Choose a project on this computer to use a local provider.".into(),
            });
            return;
        }
        let (machines, machine, events) = (relay.machines.clone(), relay.machine.clone(), relay.events.clone());
        let prefix = crate::desktop_settings::load().branch_prefix;
        crate::code_run::runtime().spawn(async move {
            let created = async {
                if new.worktree && new.review.is_none() && new.base.is_none() {
                    let value = machines
                        .call(&machine, "wb_worktree_bases", json!({ "projectId": new.project_id }))
                        .await
                        .map_err(|_| "The server's answer was lost.".to_string())??;
                    let bases: Vec<String> = serde_json::from_value(value).map_err(|error| error.to_string())?;
                    new.base = crate::code_run::default_worktree_base(&bases).cloned();
                }
                // Larger files go up for the session this request makes.
                let request = uuid::Uuid::new_v4().to_string();
                let thread = nexus_workbench::creation_thread_id(&request)?;
                let files = machines.upload(&machine, &thread, &new.attachments).await?;
                let args = CreateArgs {
                    request_id: (!files.is_empty()).then_some(request),
                    project_id: new.project_id,
                    agent: new.agent.as_str().to_string(),
                    model: new.model,
                    permission_mode: new.permission_mode,
                    effort: new.effort.map(|effort| effort.as_str().to_string()),
                    via: new.via.as_str().to_string(),
                    provider: None,
                    images: crate::attachments::session_images(&new.attachments),
                    text: new.text,
                    worktree: new.worktree.then(|| WorktreeArgs {
                        review: new.review,
                        base: new.base,
                        branch_prefix: prefix,
                        ..WorktreeArgs::default()
                    }),
                    orchestration: None,
                    name: None,
                    files,
                };
                let call = if args.files.is_empty() {
                    "wb_thread_create"
                } else {
                    "wb_thread_create_files"
                };
                let value = machines
                    .call(&machine, call, json!(args))
                    .await
                    .map_err(|_| "The server's answer was lost.".to_string())??;
                serde_json::from_value::<ThreadSummary>(value).map_err(|error| error.to_string())
            };
            let event = match created.await {
                Ok(thread) => CodeEvent::Created { draft, id: thread.id },
                Err(error) => CodeEvent::StartFailed {
                    draft,
                    error: crate::machine_view::plain_error(&error),
                },
            };
            let _ = events.unbounded_send(event);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::Runs;

    #[test]
    fn an_answer_already_in_comes_back_without_waiting_on_the_runtime() {
        // The fixture's machine is unknown, so the refusal is in at once: the caller has it before it waits.
        let mut read = Runs::isolated().call("wb_overview", serde_json::json!({}));
        assert!(matches!(read.try_recv(), Ok(Some(Err(_)))));
    }
}
