use super::*;
use nexus_workbench::computer::{CallResult, Operation, Outcome, ProjectChoice};
use nexus_workbench::event::Decision;
use nexus_workbench::session_sync::control::Action;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

mod view;

const PEERS_POLL: Duration = Duration::from_secs(12);
const MAX_PROMPT_BYTES: usize = 48 * 1024;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Peer {
    computer_id: String,
    name: String,
    enabled: bool,
    expires_at: i64,
    capabilities: Vec<String>,
}

impl Peer {
    fn ready(&self) -> bool {
        self.enabled
            && self.expires_at > nexus_workbench::now_ms()
            && ["projects", "start"]
                .iter()
                .all(|cap| self.capabilities.iter().any(|c| c == cap))
    }
}

#[derive(Default)]
enum Screen {
    #[default]
    Computers,
    Projects {
        computer: Peer,
        choices: Vec<ProjectChoice>,
        selected: Option<String>,
    },
    Session {
        computer: Peer,
        project: String,
        thread_id: String,
        transcript: Result<Vec<Line>, String>,
    },
}

struct Line {
    label: &'static str,
    text: String,
    approval_id: Option<String>,
}

#[derive(Clone)]
enum OperationRequest {
    Computer { computer_id: String, operation: Operation },
    Command { thread_id: String, action: Action },
}

#[derive(Clone)]
struct Pending {
    id: String,
    request: OperationRequest,
}

enum Receipt {
    Pending,
    Computer(Outcome),
    Command,
    Failed(&'static str),
}

impl Pending {
    async fn run(&self, commands: &Projects, submit: bool) -> Result<Receipt, String> {
        let id = self.id.clone();
        match &self.request {
            OperationRequest::Computer { computer_id, operation } => {
                let computer_id = computer_id.clone();
                let call = if submit {
                    commands.computer_request(computer_id, id, operation.clone())
                } else {
                    commands.computer_request_get(computer_id, id)
                };
                let CallResult {
                    request_id,
                    state,
                    result,
                    ..
                } = answer(call).await?;
                if request_id != self.id {
                    return Err("The computer returned a receipt for a different request.".into());
                }
                match (state.as_str(), result) {
                    ("replied", Some(outcome)) => Ok(Receipt::Computer(outcome)),
                    ("expired", _) => Ok(Receipt::Failed(
                        "The request expired. Check that the other computer is online.",
                    )),
                    ("queued" | "claimed", _) => Ok(Receipt::Pending),
                    _ => Err("The other computer did not return a verified reply.".into()),
                }
            }
            OperationRequest::Command { thread_id, action } => {
                let thread_id = thread_id.clone();
                let call = if submit {
                    commands.synced_command(thread_id, id, action.clone())
                } else {
                    commands.synced_command_get(thread_id, id)
                };
                let row = answer(call).await?;
                match row["state"].as_str() {
                    Some("queued" | "claimed") => Ok(Receipt::Pending),
                    Some("completed") => Ok(Receipt::Command),
                    Some("expired") => Ok(Receipt::Failed(
                        "The command expired before the other computer finished it.",
                    )),
                    Some("cancelled") => Ok(Receipt::Failed("The command was cancelled.")),
                    Some("rejected") => Ok(Receipt::Failed("The other computer declined this command.")),
                    _ => Err("The session did not return a command receipt.".into()),
                }
            }
        }
    }
}

pub(super) struct Work {
    pub(super) prompt: Entity<InputState>,
    peers: Option<Result<Vec<Peer>, String>>,
    peers_task: Option<Task<()>>,
    screen: Screen,
    pending: Option<Pending>,
    request_running: bool,
    session_task: Option<Task<()>>,
    message: Option<String>,
}

impl Work {
    pub(super) fn new(window: &mut Window, cx: &mut Context<RemoteComputers>) -> Self {
        Self {
            prompt: cx.new(|cx| InputState::new(window, cx).placeholder("Tell the agent what to do")),
            peers: None,
            peers_task: None,
            screen: Screen::Computers,
            pending: None,
            request_running: false,
            session_task: None,
            message: None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.peers = None;
        self.peers_task = None;
        self.screen = Screen::Computers;
        self.pending = None;
        self.request_running = false;
        self.session_task = None;
        self.message = None;
    }

    fn project(&self) -> Option<&ProjectChoice> {
        let Screen::Projects {
            choices,
            selected: Some(selected),
            ..
        } = &self.screen
        else {
            return None;
        };
        choices.iter().find(|project| &project.id == selected)
    }
}

impl RemoteComputers {
    pub(super) fn load_peers(&mut self, cx: &mut Context<Self>) {
        let Some(commands) = self.access(cx) else { return };
        let identity = self.identity.clone();
        self.work.peers_task = Some(cx.spawn(async move |this, cx| {
            let mut commands = commands;
            loop {
                let response = async {
                    let local_request = commands.computer_info();
                    let peers_request = commands.computer_list();
                    let local = answer(local_request).await?;
                    let value = answer(peers_request).await?;
                    let Some(computers) = value["computers"].as_array() else {
                        return Err("Nexus could not read the list of computers.".into());
                    };
                    Ok(computers
                        .iter()
                        .filter_map(|value| Peer::deserialize(value).ok())
                        .filter(|computer| computer.computer_id != local.computer_id)
                        .collect())
                }
                .await;
                let Ok(true) = this.update(cx, |this, cx| {
                    if this.identity != identity || this.access(cx).is_none() {
                        return false;
                    }
                    this.work.peers = Some(response);
                    cx.notify();
                    true
                }) else {
                    return;
                };
                cx.background_executor().timer(PEERS_POLL).await;
                let Ok(Some(next)) = this.update(cx, |this, cx| {
                    (this.identity == identity).then(|| this.access(cx)).flatten()
                }) else {
                    return;
                };
                commands = next;
            }
        }));
        cx.notify();
    }

    fn open_computer(&mut self, computer: Peer, cx: &mut Context<Self>) {
        if !computer.ready() || self.work.pending.is_some() {
            return;
        }
        let computer_id = computer.computer_id.clone();
        self.work.screen = Screen::Projects {
            computer,
            choices: Vec::new(),
            selected: None,
        };
        self.begin_request(
            OperationRequest::Computer {
                computer_id,
                operation: Operation::DescribeProjects,
            },
            cx,
        );
    }

    fn start_session(&mut self, cx: &mut Context<Self>) {
        let text = self.work.prompt.read(cx).value().trim().to_string();
        if text.is_empty() || text.len() > MAX_PROMPT_BYTES {
            return;
        }
        let Screen::Projects { computer, .. } = &self.work.screen else {
            return;
        };
        let Some(project) = self.work.project() else { return };
        let request = OperationRequest::Computer {
            computer_id: computer.computer_id.clone(),
            operation: Operation::StartSession {
                project_id: project.id.clone(),
                revision: project.revision.clone(),
                text,
                selection: None,
                base_branch: None,
                handoff: None,
            },
        };
        self.begin_request(request, cx);
    }

    fn session_command(&mut self, action: Action, cx: &mut Context<Self>) {
        let Screen::Session { thread_id, .. } = &self.work.screen else {
            return;
        };
        self.begin_request(
            OperationRequest::Command {
                thread_id: thread_id.clone(),
                action,
            },
            cx,
        );
    }

    fn begin_request(&mut self, request: OperationRequest, cx: &mut Context<Self>) {
        if self.work.pending.is_some() || self.access(cx).is_none() {
            return;
        }
        self.work.pending = Some(Pending {
            id: uuid::Uuid::new_v4().to_string(),
            request,
        });
        self.run_request(cx);
    }

    fn run_request(&mut self, cx: &mut Context<Self>) {
        if self.work.request_running {
            return;
        }
        let Some(pending) = self.work.pending.clone() else {
            return;
        };
        let Some(commands) = self.access(cx) else { return };
        self.work.request_running = true;
        self.work.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut reply = pending.run(&commands, true).await;
            for _ in 0..16 {
                if !matches!(reply, Ok(Receipt::Pending)) {
                    break;
                }
                cx.background_executor().timer(POLL).await;
                let Ok(Some(commands)) = this.update(cx, |this, cx| {
                    this.work.pending.as_ref().filter(|job| job.id == pending.id)?;
                    this.access(cx)
                }) else {
                    return;
                };
                reply = pending.run(&commands, false).await;
            }
            let _ = this.update(cx, |this, cx| {
                if this.work.pending.as_ref().is_none_or(|job| job.id != pending.id) || this.access(cx).is_none() {
                    return;
                }
                this.work.request_running = false;
                match reply {
                    Ok(Receipt::Pending) => this.work.message = Some("Still waiting for the other computer.".into()),
                    Err(error) => this.work.message = Some(error),
                    Ok(receipt) => {
                        this.work.pending = None;
                        this.show_receipt(pending.request, receipt, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show_receipt(&mut self, request: OperationRequest, receipt: Receipt, cx: &mut Context<Self>) {
        match receipt {
            Receipt::Pending => {}
            Receipt::Failed(reason) => self.work.message = Some(reason.into()),
            Receipt::Command => {
                self.work.message = Some(
                    match request {
                        OperationRequest::Command {
                            action: Action::Stop, ..
                        } => "Stop request acknowledged.",
                        OperationRequest::Command {
                            action: Action::Approve { .. },
                            ..
                        } => "Approval response acknowledged.",
                        _ => "Message acknowledged.",
                    }
                    .into(),
                )
            }
            Receipt::Computer(outcome) => match outcome {
                Outcome::Projects { projects, .. } | Outcome::ChoicesChanged { projects, .. } => {
                    if let Screen::Projects { choices, selected, .. } = &mut self.work.screen {
                        *choices = projects;
                        *selected = None;
                    }
                    if matches!(
                        request,
                        OperationRequest::Computer {
                            operation: Operation::StartSession { .. },
                            ..
                        }
                    ) {
                        self.work.message = Some("Saved projects changed. Select a project and try again.".into());
                    }
                }
                Outcome::Created { thread_id, .. }
                | Outcome::Uncertain {
                    thread_id: Some(thread_id),
                } => {
                    if let Screen::Projects { computer, .. } = &self.work.screen {
                        let project = self
                            .work
                            .project()
                            .map(|project| project.name.clone())
                            .unwrap_or("Remote session".into());
                        self.work.screen = Screen::Session {
                            computer: computer.clone(),
                            project,
                            thread_id: thread_id.clone(),
                            transcript: Ok(Vec::new()),
                        };
                        self.follow(thread_id, cx);
                    }
                }
                Outcome::Uncertain { thread_id: None } => {
                    self.work.message = Some("Nexus could not confirm the start. Check the other computer for a new session before trying again.".into());
                }
                Outcome::Refused { message, .. } => self.work.message = Some(message),
                _ => self.work.message = Some("The other computer returned an unexpected result.".into()),
            },
        }
    }

    fn follow(&mut self, thread_id: String, cx: &mut Context<Self>) {
        self.work.session_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(Some(commands)) = this.update(cx, |this, cx| this.access(cx)) else {
                    return;
                };
                let page = answer(commands.synced_thread_page(thread_id.clone()))
                    .await
                    .map(|value| lines(&value));
                let Ok(true) = this.update(cx, |this, cx| {
                    if this.access(cx).is_none() {
                        return false;
                    }
                    let Screen::Session {
                        thread_id: active_id,
                        transcript,
                        ..
                    } = &mut this.work.screen
                    else {
                        return false;
                    };
                    if active_id != &thread_id {
                        return false;
                    }
                    *transcript = page;
                    cx.notify();
                    true
                }) else {
                    return;
                };
                cx.background_executor().timer(POLL).await;
            }
        }));
    }

    fn back_to_computers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.work.request_running {
            self.work.pending = None;
            self.work.session_task = None;
            self.work.message = None;
            self.work.screen = Screen::Computers;
            self.work.prompt.update(cx, |state, cx| state.set_value("", window, cx));
            cx.notify();
        }
    }
}

fn lines(value: &Value) -> Vec<Line> {
    let Some(events) = value["events"].as_array() else {
        return Vec::new();
    };
    let mut resolved = HashSet::new();
    let mut lines = events
        .iter()
        .rev()
        .filter_map(|row| {
            if row["state"] != "open" {
                return None;
            }
            let event = &row["event"];
            let (label, text, approval_id) = match event["kind"].as_str()? {
                "user" => ("You", event["text"].as_str()?.to_owned(), None),
                "text" => ("Agent", event["text"].as_str()?.to_owned(), None),
                "approvalResolved" => {
                    resolved.insert(event["requestId"].as_str()?);
                    return None;
                }
                "approval" => {
                    let id = event["requestId"].as_str()?;
                    if resolved.contains(id) {
                        return None;
                    }
                    let tool = event["tool"].as_str()?;
                    let input = event["input"].to_string();
                    let review_here = tool != "AskUserQuestion" && input.len() <= 4000;
                    if review_here {
                        ("Approval needed", format!("{tool}\n{input}"), Some(id.to_owned()))
                    } else {
                        (
                            "Approval needed",
                            format!("Review {tool} in the session on the other computer."),
                            None,
                        )
                    }
                }
                "turnEnd" if event["status"] == "error" => (
                    "Error",
                    event["error"]
                        .as_str()
                        .unwrap_or("The agent stopped with an error.")
                        .to_owned(),
                    None,
                ),
                _ => return None,
            };
            Some(Line {
                label,
                text: text.chars().take(4096).collect(),
                approval_id,
            })
        })
        .take(12)
        .collect::<Vec<_>>();
    lines.reverse();
    lines
}
