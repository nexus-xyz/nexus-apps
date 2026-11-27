//! Settings › Servers (the Nexus app's `settings/servers.tsx` and `server-settings.tsx`): your own Linux servers that run
//! Code sessions over SSH, from the Nexus app's `settings.json` (`machines` of kind `ssh`). Add and Remove change that
//! list here, through `nexus-desktop-core`, while this build owns the store.
//!
//! Each server's connection is `machines.rs`'s: its status, Retry, and Update now (the host's running sessions stop,
//! as when the app quits). "Test connection" reaches a host before it is added, and "Add and install" saves it, after
//! which the connection installs the Nexus host there. A server's Sign-ins show its host's Nexus sign-in (signed in
//! from here with a device code) and Claude Code and Codex there, with the commands to install or sign them in.
//! Removing a server only forgets it here: its host and sessions keep running there.

use std::collections::{HashMap, HashSet};

use futures::StreamExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_client::local_agents::{LocalAgent, Login};
use nexus_desktop_core::servers::ServerCheck;
use nexus_desktop_core::settings::{MachineSetting, Settings};
use nexus_remote::machine::Status;

use super::desktop_app::{read_only_note, save_settings};
use super::{Ctx, DeviceCtx, Load, palette, ui};
use crate::machine_view::{Action, plain_error, status_view, suggested_name};
use crate::machines::ServerView;
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

fn host_source_requires_account(source: &nexus_remote::install::Source) -> bool {
    match source {
        nexus_remote::install::Source::Release { manifest, .. } => reqwest::Url::parse(manifest)
            .is_ok_and(|url| nexus_desktop_updates::access::host_download_requires_account(&url)),
        nexus_remote::install::Source::Local { .. } => false,
    }
}

pub fn build(ctx: DeviceCtx, account: Option<Ctx>, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Name, as Code shows it"));
        let ssh = cx.new(|cx| InputState::new(window, cx).placeholder("me@my-server.example.com"));
        let subscription = cx.subscribe_in(
            &ssh,
            window,
            |this: &mut ServersPage, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    if this.tested_ok(cx) {
                        this.add(window, cx);
                    } else {
                        this.test(cx);
                    }
                }
                InputEvent::Change => {
                    this.problem = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        // The servers' connections as they change.
        let changes = cx.spawn(async move |this, cx| {
            let mut changes = crate::machines::get().subscribe();
            while changes.next().await.is_some() {
                while changes.try_recv().is_ok() {}
                let read = this.update(cx, |this, cx| {
                    this.read_machines(cx);
                    this.take_sign_in_ask(cx);
                });
                if read.is_err() {
                    break;
                }
            }
        });
        let mut page = ServersPage {
            ctx,
            account,
            servers: Load::Loading,
            machines: crate::machines::get().servers(),
            hosts: Vec::new(),
            name,
            ssh,
            tested: None,
            busy: None,
            problem: None,
            added: None,
            confirm_remove: None,
            confirm_update: None,
            saving: false,
            error: None,
            open: HashSet::new(),
            checks: HashMap::new(),
            acting: HashSet::new(),
            problems: HashMap::new(),
            sign_in: None,
            _subscription: subscription,
            _changes: changes,
        };
        page.load(cx);
        page.take_sign_in_ask(cx);
        page
    });
    Some(view.into())
}

/// The servers reached over SSH; cloud machines are on the Cloud page.
fn ssh_servers(settings: &Settings) -> Vec<MachineSetting> {
    settings
        .machines
        .iter()
        .filter(|machine| machine.kind == "ssh" && !machine.ssh.trim().is_empty())
        .cloned()
        .collect()
}

/// Forget server `id`. A cloud machine is not removed here: it is deleted on the Cloud page.
fn remove_server(settings: &mut Settings, id: &str) -> Result<(), String> {
    let before = settings.machines.len();
    settings
        .machines
        .retain(|machine| !(machine.id == id && machine.kind == "ssh"));
    if settings.machines.len() == before {
        return Err("That server is not on the list.".into());
    }
    Ok(())
}

/// What a server's Sign-ins say about its host starting on boot.
fn boot_note(name: &str, starts: bool) -> String {
    if starts {
        format!("The Nexus host starts when {name} boots, and runs after you log out.")
    } else {
        format!(
            "The Nexus host does not start when {name} boots: after a restart, sessions there wait until Nexus \
             connects again. Turning it on leaves the host and its sessions running."
        )
    }
}

/// What "Add a server" is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    Test,
    Add,
}

/// A server's Nexus sign-in on its way: the code to approve once its host prints it.
struct SignIn {
    machine: String,
    code: Option<(String, String)>,
    _task: Task<()>,
}

struct ServersPage {
    ctx: DeviceCtx,
    account: Option<Ctx>,
    servers: Load<Vec<MachineSetting>>,
    /// Their connections, as `machines.rs` has them now.
    machines: Vec<ServerView>,
    /// The hosts `~/.ssh/config` names, offered under the SSH field.
    hosts: Vec<String>,
    /// "Add a server": its name and its SSH destination.
    name: Entity<InputState>,
    ssh: Entity<InputState>,
    /// What "Test connection" found, for the destination it tested.
    tested: Option<(String, ServerCheck)>,
    busy: Option<Busy>,
    problem: Option<SharedString>,
    /// The server just added: the form shows its setup until it is connected.
    added: Option<String>,
    /// The server whose Remove, or Update now with sessions running, was pressed once.
    confirm_remove: Option<String>,
    confirm_update: Option<String>,
    saving: bool,
    error: Option<SharedString>,
    /// Servers whose Sign-ins are open, what each check read, and the servers with an action on its way.
    open: HashSet<String>,
    checks: HashMap<String, Load<ServerCheck>>,
    acting: HashSet<String>,
    problems: HashMap<String, SharedString>,
    sign_in: Option<SignIn>,
    _subscription: Subscription,
    _changes: Task<()>,
}

impl ServersPage {
    fn load(&mut self, cx: &mut Context<Self>) {
        self.servers = Load::Loading;
        super::desktop_app::off_thread(
            cx,
            || {
                (
                    ssh_servers(&crate::desktop_settings::load()),
                    crate::machines::ssh_hosts(),
                )
            },
            |this, (servers, hosts), _| {
                this.servers = Load::Ready(servers);
                this.hosts = hosts;
            },
        );
    }

    fn read_machines(&mut self, cx: &mut Context<Self>) {
        self.machines = crate::machines::get().servers();
        cx.notify();
    }

    fn machine(&self, id: &str) -> Option<&ServerView> {
        self.machines.iter().find(|machine| machine.id == id)
    }

    fn save(
        &mut self,
        change: impl FnOnce(&mut Settings) -> Result<Option<String>, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.saving {
            return;
        }
        self.saving = true;
        self.error = None;
        save_settings(&self.ctx, cx, change, |this, saved, cx| {
            this.saving = false;
            this.busy = None;
            this.confirm_remove = None;
            match saved {
                Ok((settings, added)) => {
                    this.servers = Load::Ready(ssh_servers(&settings));
                    if added.is_some() {
                        this.added = added;
                        this.tested = None;
                    }
                    this.read_machines(cx);
                }
                Err(error) => this.problem = Some(plain_error(&error).into()),
            }
        });
        cx.notify();
    }

    fn ssh_value(&self, cx: &App) -> String {
        self.ssh.read(cx).value().trim().to_string()
    }

    /// The destination typed was tested, and the Nexus host runs there.
    fn tested_ok(&self, cx: &App) -> bool {
        let ssh = self.ssh_value(cx);
        self.tested
            .as_ref()
            .is_some_and(|(tested, check)| *tested == ssh && check.supported)
    }

    fn test(&mut self, cx: &mut Context<Self>) {
        let ssh = self.ssh_value(cx);
        if ssh.is_empty() || self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Test);
        self.problem = None;
        self.tested = None;
        let answer = crate::machines::get().test(&ssh);
        cx.spawn(async move |this, cx| {
            let answer = answer.await.unwrap_or_else(|_| Err("The test was cancelled.".into()));
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match answer {
                    Ok(check) => this.tested = Some((ssh, check)),
                    Err(error) => this.problem = Some(plain_error(&error).into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ssh = self.ssh_value(cx);
        if !self.tested_ok(cx) || self.busy.is_some() {
            return;
        }
        let name = self.name.read(cx).value().trim().to_string();
        let name = if name.is_empty() { suggested_name(&ssh) } else { name };
        self.busy = Some(Busy::Add);
        self.save(
            move |settings| settings.add_ssh_machine(&name, &ssh).map(|machine| Some(machine.id)),
            cx,
        );
        for input in [&self.name, &self.ssh] {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        if self.confirm_remove.as_deref() != Some(id.as_str()) {
            self.confirm_remove = Some(id);
            cx.notify();
            return;
        }
        if self.sign_in.as_ref().is_some_and(|sign_in| sign_in.machine == id) {
            self.sign_in = None;
        }
        self.save(move |settings| remove_server(settings, &id).map(|()| None), cx);
    }

    fn update_requires_sign_in(&self) -> bool {
        self.account.is_none() && host_source_requires_account(&crate::machines::host_source())
    }

    /// "Update now": asked once more while sessions run there, since they stop.
    fn update(&mut self, id: String, cx: &mut Context<Self>) {
        if self.update_requires_sign_in() {
            return;
        }
        let busy = self.machine(&id).map_or(0, |machine| machine.busy);
        if busy > 0 && self.confirm_update.as_deref() != Some(id.as_str()) {
            self.confirm_update = Some(id);
            cx.notify();
            return;
        }
        self.confirm_update = None;
        let answer = crate::machines::get().update(&id);
        self.act(id, answer, |_, _, _| {}, cx);
    }

    /// Runs a server's action: its button waits, and its error shows under the server.
    fn act<T: 'static>(
        &mut self,
        id: String,
        answer: futures::channel::oneshot::Receiver<Result<T, String>>,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.acting.insert(id.clone());
        self.problems.remove(&id);
        cx.spawn(async move |this, cx| {
            let answer = answer.await.unwrap_or_else(|_| Err("Cancelled.".into()));
            let _ = this.update(cx, |this, cx| {
                this.acting.remove(&id);
                match answer {
                    Ok(value) => done(this, value, cx),
                    Err(error) => {
                        this.problems.insert(id, plain_error(&error).into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn toggle_sign_ins(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.open.remove(&id) {
            self.open.insert(id.clone());
            self.check(id, cx);
        }
        cx.notify();
    }

    /// Reads what server `id` has: its host's sign-in, Claude Code and Codex.
    fn check(&mut self, id: String, cx: &mut Context<Self>) {
        let answer = crate::machines::get().probe(&id);
        self.checks.insert(id.clone(), Load::Loading);
        cx.spawn(async move |this, cx| {
            let answer = answer.await.unwrap_or_else(|_| Err("Cancelled.".into()));
            let _ = this.update(cx, |this, cx| {
                let read = match answer {
                    Ok(check) => Load::Ready(check),
                    Err(error) => Load::Failed(plain_error(&error).into()),
                };
                this.checks.insert(id, read);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Makes server `id`'s host start on boot, then reads the server again. The host running now goes on.
    fn start_on_boot(&mut self, id: String, cx: &mut Context<Self>) {
        let answer = crate::machines::get().start_on_boot(&id);
        let again = id.clone();
        self.act(id, answer, move |this, (), cx| this.check(again, cx), cx);
    }

    /// Signs server `id`'s host in to Nexus: its code shows here, and the approval page opens in the browser.
    fn start_sign_in(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(account) = &self.account else { return };
        let (codes, mut code_stream) = futures::channel::mpsc::unbounded::<(String, String)>();
        let answer = crate::machines::get().sign_in(&id, account.gateway.url(), codes);
        let machine = id.clone();
        let task = cx.spawn(async move |this, cx| {
            let mut answer = answer;
            loop {
                futures::select! {
                    code = code_stream.next() => {
                        let Some((link, code)) = code else { continue };
                        let _ = this.update(cx, |this, cx| {
                            if !link.is_empty() {
                                crate::external_link::open(&link, cx);
                            }
                            if let Some(sign_in) = &mut this.sign_in {
                                sign_in.code = Some((link, code));
                            }
                            cx.notify();
                        });
                    }
                    signed = answer => {
                        let signed = signed.unwrap_or_else(|_| Err("Cancelled.".into()));
                        let _ = this.update(cx, |this, cx| {
                            this.sign_in = None;
                            match signed {
                                Ok(_) => this.check(machine.clone(), cx),
                                Err(error) if error == "Cancelled." => {}
                                Err(error) => {
                                    this.problems.insert(machine.clone(), plain_error(&error).into());
                                }
                            }
                            cx.notify();
                        });
                        break;
                    }
                }
            }
        });
        self.problems.remove(&id);
        self.sign_in = Some(SignIn {
            machine: id,
            code: None,
            _task: task,
        });
        cx.notify();
    }

    /// A chat asked to sign a server in (`Machines::ask_sign_in`): its Sign-ins open and the sign-in starts, unless
    /// another is on its way.
    fn take_sign_in_ask(&mut self, cx: &mut Context<Self>) {
        if self.account.is_some()
            && let Some(id) = crate::machines::get().take_sign_in_ask()
        {
            self.take_sign_in_for(id, cx);
        }
    }

    /// Opens server `id`'s Sign-ins and starts its Nexus sign-in, unless another is on its way.
    fn take_sign_in_for(&mut self, id: String, cx: &mut Context<Self>) {
        if self.open.insert(id.clone()) {
            self.check(id.clone(), cx);
        }
        if self.sign_in.is_none() {
            self.start_sign_in(id, cx);
        }
    }

    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Some(sign_in) = self.sign_in.take() {
            crate::machines::get().cancel_sign_in(&sign_in.machine);
        }
        cx.notify();
    }

    fn render_server(&self, server: &MachineSetting, palette: Palette, cx: &mut Context<Self>) -> Div {
        let editable = crate::desktop_settings::writable() && !self.saving;
        let confirming = self.confirm_remove.as_deref() == Some(server.id.as_str());
        let acting = self.acting.contains(&server.id);
        let machine = self.machine(&server.id);
        let status = machine.map_or(Status::Closed, |machine| machine.status.clone());
        let view = status_view(&status);
        let id = server.id.clone();
        let remove_id = SharedString::from(format!("server-remove-{}", server.id));
        let remove = if editable && !acting {
            ui::danger_button(remove_id, if confirming { "Remove?" } else { "Remove" }, palette)
                .on_click(cx.listener(move |this, _, _, cx| this.remove(id.clone(), cx)))
                .into_any_element()
        } else {
            ui::disabled_button(remove_id, "Remove", palette).into_any_element()
        };
        let update_locked = matches!(view.action, Some(Action::Update)) && self.update_requires_sign_in();
        let action = match view.action {
            Some(Action::Update) => {
                let id = server.id.clone();
                let label = if self.confirm_update.as_deref() == Some(server.id.as_str()) {
                    "Update now?"
                } else {
                    "Update now"
                };
                Some(button_or_wait(
                    format!("server-update-{}", server.id),
                    label,
                    acting || update_locked,
                    palette,
                    cx.listener(move |this, _, _, cx| this.update(id.clone(), cx)),
                ))
            }
            Some(Action::Retry) => {
                let id = server.id.clone();
                Some(button_or_wait(
                    format!("server-retry-{}", server.id),
                    "Retry",
                    acting,
                    palette,
                    cx.listener(move |_, _, _, cx| {
                        let _ = crate::machines::get().reconnect(&id);
                        cx.notify();
                    }),
                ))
            }
            None => None,
        };
        let open = self.open.contains(&server.id);
        let toggle = {
            let id = server.id.clone();
            ui::button(
                SharedString::from(format!("server-sign-ins-{}", server.id)),
                if open { "Hide" } else { "Sign-ins" },
                palette,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_sign_ins(id.clone(), cx)))
        };
        let busy = machine.map_or(0, |machine| machine.busy);
        let update_warning = (self.confirm_update.as_deref() == Some(server.id.as_str()) && busy > 0).then(|| {
            let sessions = if busy == 1 { "1 session is" } else { "sessions are" };
            let them = if busy == 1 { "it" } else { "them" };
            let count = if busy == 1 { String::new() } else { format!("{busy} ") };
            ui::note(
                format!(
                    "{count}{sessions} running on {}. Updating stops {them}, as quitting does; each shows Continue \
                     after. Press Update now? again to go on.",
                    server.name
                ),
                palette,
            )
        });
        v_flex()
            .child(
                ui::row(palette)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(2.))
                            .child(
                                h_flex()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(palette.strong)
                                            .child(server.name.clone()),
                                    )
                                    .child(ui::badge(view.label, palette)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_family(ui::MONO)
                                    .text_color(palette.faint)
                                    .truncate()
                                    .child(server.ssh.clone()),
                            )
                            .children(view.detail.clone().map(|detail| ui::note(detail, palette))),
                    )
                    .children(action)
                    .child(toggle)
                    .child(remove),
            )
            .when(update_locked && self.ctx.local_workspace, |server| {
                server.child(ui::card_row().child(ui::requires_sign_in(palette)))
            })
            .children(update_warning)
            .children(
                self.problems
                    .get(&server.id)
                    .map(|problem| ui::error_line(problem.clone(), palette)),
            )
            .when(open, |column| {
                column.child(self.render_sign_ins(server, &status, palette, cx))
            })
    }

    /// A server's sign-ins: Nexus (from here), then Claude Code and Codex (commands to run there).
    fn render_sign_ins(
        &self,
        server: &MachineSetting,
        status: &Status,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let check = self.checks.get(&server.id);
        // The hello says which organization the host is signed in to; the check says so even while it is offline.
        let org = match status {
            Status::Connected { host, .. } | Status::NeedsUpdate { host, .. } => host.org.clone(),
            _ => None,
        };
        let signed_in = org.is_some() || matches!(check, Some(Load::Ready(check)) if check.nexus_signed_in);
        let connected = matches!(status, Status::Connected { .. });
        let mut column = v_flex()
            .pl(px(4.))
            .pb(px(12.))
            .gap(px(10.))
            .child(self.render_nexus_sign_in(server, signed_in, org, connected, palette, cx));
        column = match check {
            None | Some(Load::Loading) => column.child(ui::note(
                format!("Checking Claude Code and Codex on {}…", server.name),
                palette,
            )),
            Some(Load::Failed(error)) => column.child(ui::error_line(error.clone(), palette)),
            Some(Load::Ready(check)) => {
                let agents: Vec<Div> = check
                    .agents
                    .iter()
                    // A server check names Claude Code and Codex (`nexus_remote::probe::AGENTS`), each with its setup.
                    .filter_map(|agent| {
                        let setup = nexus_workbench::event::SessionAgent::parse(&agent.agent)
                            .and_then(|session| crate::agents::ui(session).server_setup(&server.ssh))?;
                        Some(agent_on_server(agent, &server.name, &setup, palette, cx))
                    })
                    .collect();
                let boot = check
                    .starts_on_boot
                    .map(|starts| self.render_boot(server, starts, palette, cx));
                column
                    .children(agents)
                    .children(boot)
                    .children(check.incomplete.clone().map(|incomplete| ui::note(incomplete, palette)))
            }
        };
        let id = server.id.clone();
        let checking = matches!(check, None | Some(Load::Loading));
        column.child(h_flex().child(if checking {
            ui::disabled_button(
                SharedString::from(format!("server-check-{}", server.id)),
                "Checking…",
                palette,
            )
        } else {
            ui::button(
                SharedString::from(format!("server-check-{}", server.id)),
                "Check again",
                palette,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.check(id.clone(), cx)))
        }))
    }

    /// Whether the server's host starts on boot, and the button that makes it.
    fn render_boot(&self, server: &MachineSetting, starts: bool, palette: Palette, cx: &mut Context<Self>) -> Div {
        let column = v_flex()
            .gap(px(6.))
            .child(ui::label("Start on boot", palette))
            .child(ui::note(boot_note(&server.name, starts), palette));
        if starts {
            return column;
        }
        let id = server.id.clone();
        column.child(h_flex().child(button_or_wait(
            format!("server-boot-{}", server.id),
            "Start on boot",
            self.acting.contains(&server.id),
            palette,
            cx.listener(move |this, _, _, cx| this.start_on_boot(id.clone(), cx)),
        )))
    }

    fn render_nexus_sign_in(
        &self,
        server: &MachineSetting,
        signed_in: bool,
        org: Option<String>,
        connected: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let column = v_flex().gap(px(6.)).child(ui::label("Nexus", palette));
        let waiting = self.sign_in.as_ref().filter(|sign_in| sign_in.machine == server.id);
        if let Some(sign_in) = waiting {
            return match &sign_in.code {
                None => column.child(ui::note(format!("Starting the sign-in on {}…", server.name), palette)),
                Some((link, code)) => {
                    let link = link.clone();
                    column
                        .child(ui::note(
                            format!(
                                "Approve this code in the browser window that opened. It signs {} in with its own \
                                 device, which you can revoke in Nexus like any other.",
                                server.name
                            ),
                            palette,
                        ))
                        .child(
                            div()
                                .font_family(ui::MONO)
                                .text_size(px(16.))
                                .text_color(palette.strong)
                                .child(code.clone()),
                        )
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(
                                    ui::button("server-approval-page", "Open the approval page", palette)
                                        .on_click(move |_, _, cx| crate::external_link::open(&link, cx)),
                                )
                                .child(
                                    ui::button("server-sign-in-cancel", "Cancel", palette)
                                        .on_click(cx.listener(|this, _, _, cx| this.cancel_sign_in(cx))),
                                ),
                        )
                }
            };
        }
        let line = if signed_in {
            format!(
                "Signed in{}. Sessions there go through Nexus and count toward your team's budget.",
                org.map(|org| format!(" to {org}")).unwrap_or_default()
            )
        } else {
            format!(
                "Not signed in yet: sessions can't start on {} until it is.",
                server.name
            )
        };
        let label = if signed_in {
            "Sign in again".to_string()
        } else {
            format!("Sign {} in to Nexus", server.name)
        };
        let id = server.id.clone();
        let button_id = SharedString::from(format!("server-sign-in-{}", server.id));
        let button = if self.account.is_some() && connected && self.sign_in.is_none() {
            ui::button(button_id, label, palette)
                .on_click(cx.listener(move |this, _, _, cx| this.start_sign_in(id.clone(), cx)))
        } else {
            ui::disabled_button(button_id, label, palette)
        };
        column
            .child(ui::note(line, palette))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(button)
                    .when(self.ctx.local_workspace, |row| row.child(ui::requires_sign_in(palette))),
            )
            .when(!connected, |column| {
                column.child(ui::note(
                    format!("Available once {} is connected.", server.name),
                    palette,
                ))
            })
    }

    fn render_add(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        // Added: the setup's progress is the connection's, until it is connected or says why not.
        if let Some(added) = &self.added
            && let Some(machine) = self.machine(added)
        {
            let view = status_view(&machine.status);
            let done = matches!(machine.status, Status::Connected { .. });
            let connecting = matches!(machine.status, Status::Connecting { .. });
            let title = if done {
                format!("{} is ready", machine.name)
            } else if connecting {
                format!("Setting up {}…", machine.name)
            } else {
                format!("{}: {}", machine.name, view.label)
            };
            // Sessions can't start there until its host is signed in to Nexus: the setup's last step.
            let signed_in = matches!(&machine.status, Status::Connected { host, .. } if host.org.is_some());
            let detail = if done && signed_in {
                Some("Start a session on it from its + in the sidebar.".to_string())
            } else if done {
                Some(format!(
                    "Sign {} in to Nexus next: sessions can't start there until it is. Then start one from its + in \
                     the sidebar.",
                    machine.name
                ))
            } else if connecting {
                Some("Installing and starting the Nexus host on it. The first time takes a minute.".to_string())
            } else {
                view.detail
            };
            return v_flex()
                .py(px(12.))
                .gap(px(8.))
                .child(ui::label(title, palette))
                .children(detail.map(|detail| ui::note(detail, palette)))
                .child(
                    h_flex()
                        .gap(px(8.))
                        .when(done && !signed_in, |row| {
                            let id = machine.id.clone();
                            row.child(
                                ui::primary_button(
                                    "server-added-sign-in",
                                    format!("Sign {} in to Nexus", machine.name),
                                    palette,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.added = None;
                                    this.take_sign_in_for(id.clone(), cx);
                                })),
                            )
                        })
                        .child(
                            ui::button("server-added-done", if done { "Done" } else { "Close" }, palette).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.added = None;
                                    cx.notify();
                                }),
                            ),
                        ),
                );
        }
        let editable = crate::desktop_settings::writable() && !self.saving;
        let typed = !self.ssh_value(cx).is_empty();
        let test = if typed && self.busy.is_none() {
            ui::button("server-test", "Test connection", palette)
                .on_click(cx.listener(|this, _, _, cx| this.test(cx)))
                .into_any_element()
        } else {
            let label = if self.busy == Some(Busy::Test) {
                "Testing…"
            } else {
                "Test connection"
            };
            ui::disabled_button("server-test", label, palette).into_any_element()
        };
        let add = if editable && self.busy.is_none() && self.tested_ok(cx) {
            ui::primary_button("server-add", "Add and install", palette)
                .on_click(cx.listener(|this, _, window, cx| this.add(window, cx)))
                .into_any_element()
        } else {
            let label = if self.busy == Some(Busy::Add) {
                "Adding…"
            } else {
                "Add and install"
            };
            ui::disabled_button("server-add", label, palette).into_any_element()
        };
        let ssh = self.ssh_value(cx);
        let tested = self
            .tested
            .as_ref()
            .filter(|(tested, _)| *tested == ssh)
            .map(|(_, check)| check);
        let hosts: Vec<AnyElement> = self
            .hosts
            .iter()
            .filter(|host| ssh.is_empty() || (host.contains(ssh.as_str()) && **host != ssh))
            .take(8)
            .map(|host| {
                let picked = host.clone();
                ui::chip(
                    SharedString::from(format!("ssh-host-{host}")),
                    host.clone(),
                    false,
                    palette,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.ssh
                        .update(cx, |state, cx| state.set_value(picked.clone(), window, cx));
                    this.problem = None;
                    cx.notify();
                }))
                .into_any_element()
            })
            .collect();
        v_flex()
            .py(px(12.))
            .gap(px(8.))
            .child(ui::label("Add a server", palette))
            .child(ui::field(
                "SSH host: a name from ~/.ssh/config, or user@server",
                ui::input_box(&self.ssh, palette).font_family(ui::MONO),
                palette,
            ))
            .when(!hosts.is_empty(), |column| {
                column.child(h_flex().flex_wrap().gap(px(6.)).children(hosts))
            })
            .child(ui::field(
                "Name, as Code shows it",
                ui::input_box(&self.name, palette),
                palette,
            ))
            .child(ui::note(
                "The app connects with your own ssh, keys and config, as your terminal does. It needs a key or \
                 ssh-agent, not a password. Linux on x86-64 or arm64.",
                palette,
            ))
            .children(tested.map(|check| check_summary(check, palette)))
            .children(self.problem.clone().map(|problem| ui::error_line(problem, palette)))
            .child(h_flex().gap(px(8.)).child(test).child(add))
    }
}

/// A button, or the same button waiting while its action runs.
fn button_or_wait(
    id: String,
    label: &'static str,
    waiting: bool,
    palette: Palette,
    click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let id = SharedString::from(id);
    if waiting {
        ui::disabled_button(id, label, palette).into_any_element()
    } else {
        ui::button(id, label, palette).on_click(click).into_any_element()
    }
}

/// What "Test connection" found, in a line.
fn check_summary(check: &ServerCheck, palette: Palette) -> Div {
    if !check.supported {
        return ui::error_line(
            format!(
                "It runs {} on {}. The Nexus host runs on Linux (x86-64 or arm64) for now.",
                check.os, check.arch
            ),
            palette,
        );
    }
    let found: Vec<&str> = check
        .agents
        .iter()
        .filter(|agent| agent.installed)
        .map(|agent| agent_name(&agent.agent))
        .collect();
    let agents = if found.is_empty() {
        "No Claude Code or Codex there yet: you can install them after adding it.".to_string()
    } else {
        format!("{} found there.", found.join(" and "))
    };
    v_flex()
        .gap(px(4.))
        .child(ui::note(
            format!("Connected: {} on {}. {agents}", check.os, check.arch),
            palette,
        ))
        .children(check.incomplete.clone().map(|incomplete| ui::note(incomplete, palette)))
}

fn agent_name(agent: &str) -> &str {
    nexus_client::agent_entries::Agent::parse(agent).map_or(agent, |agent| agent.entry().identity.name)
}

/// Claude Code or Codex on the server: installed and signed in, or the command to run there yourself.
fn agent_on_server(
    agent: &LocalAgent,
    server: &str,
    setup: &crate::agents::ServerSetup,
    palette: Palette,
    cx: &mut Context<ServersPage>,
) -> Div {
    let name = agent_name(&agent.agent);
    // A server check names Claude Code and Codex only (`nexus_remote::probe::AGENTS`).
    let plan =
        nexus_client::agent_entries::Agent::parse(&agent.agent).map_or("Claude", |agent| agent.entry().identity.plan);
    let column = v_flex().gap(px(4.)).child(ui::label(name, palette));
    if !agent.installed {
        return column
            .child(ui::note(
                format!("Not installed on {server}. Run this in your terminal to install it there:"),
                palette,
            ))
            .child(command(
                &format!("install-{}", agent.agent),
                &setup.install,
                palette,
                cx,
            ));
    }
    let status = match agent.login {
        Some(Login::Plan) => format!(
            "signed in to your {plan} plan{}",
            agent.plan.as_ref().map(|plan| format!(" ({plan})")).unwrap_or_default()
        ),
        Some(Login::ApiKey) => "signed in with an API key".into(),
        Some(Login::SignedOut) => format!("not signed in to a {plan} plan"),
        _ => "sign-in unknown".into(),
    };
    let version = agent
        .version
        .as_ref()
        .map(|version| format!("{version}, "))
        .unwrap_or_default();
    let column = column.child(ui::note(
        format!("{version}{status}. Through Nexus it needs no sign-in of its own; a session on your own plan does."),
        palette,
    ));
    if agent.login == Some(Login::Plan) {
        return column;
    }
    column.child(ui::note(setup.how, palette)).child(command(
        &format!("login-{}", agent.agent),
        &setup.login,
        palette,
        cx,
    ))
}

/// A command to run in a terminal, with Copy.
fn command(id: &str, text: &str, palette: Palette, _cx: &mut Context<ServersPage>) -> Div {
    let copied = text.to_string();
    h_flex()
        .gap(px(8.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(8.))
                .bg(palette.hover)
                .font_family(ui::MONO)
                .text_size(px(12.))
                .child(text.to_string()),
        )
        .child(
            ui::button(SharedString::from(format!("copy-{id}")), "Copy", palette)
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))),
        )
}

impl Render for ServersPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let mut page = v_flex()
            .w_full()
            .child(ui::lead(
                "Run Code sessions on your own Linux server over SSH. They keep going when this computer sleeps.",
                palette,
            ))
            .children(read_only_note(palette))
            .child(ui::section_head("Your servers", palette));
        page = match &self.servers {
            Load::Loading => page.child(ui::skeleton(2, palette)),
            Load::Failed(error) => page.child(ui::failed(error.clone(), palette)),
            Load::Ready(servers) if servers.is_empty() => page.child(ui::empty_state(
                gpui_kit::assets::IconName::Server,
                "No servers yet",
                "Add one by its SSH host, as in ~/.ssh/config.",
                palette,
            )),
            Load::Ready(servers) => {
                let rows: Vec<Div> = servers
                    .iter()
                    .map(|server| self.render_server(server, palette, cx))
                    .collect();
                page.children(rows)
            }
        };
        page.child(self.render_add(palette, cx))
            .children(self.error.clone().map(|error| ui::error_line(error, palette)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn server_settings_only_private_host_downloads_need_an_account() {
        use nexus_remote::install::{DOWNLOADS, Source};
        assert!(super::host_source_requires_account(&Source::release(
            DOWNLOADS,
            "1.0.0-internal.1",
            None
        )));
        assert!(!super::host_source_requires_account(&Source::release(
            DOWNLOADS, "1.0.0", None
        )));
        assert!(!super::host_source_requires_account(&Source::Local {
            path: "/tmp/host".into()
        }));
    }

    #[test]
    fn says_whether_the_host_starts_on_boot() {
        assert!(boot_note("vps", true).starts_with("The Nexus host starts when vps boots"));
        let off = boot_note("vps", false);
        assert!(off.starts_with("The Nexus host does not start when vps boots"));
        assert!(off.contains("leaves the host and its sessions running"));
    }

    #[test]
    fn adds_and_removes_ssh_servers_only() {
        let file = crate::desktop_settings::TempFile::new("servers");
        nexus_desktop_core::settings::save(
            file.path(),
            &Settings {
                machines: vec![MachineSetting {
                    id: "cloud-1".into(),
                    name: "Cloud".into(),
                    kind: "cloud".into(),
                    ssh: String::new(),
                    cloud: None,
                }],
                ..Settings::default()
            },
        )
        .expect("save");
        let (_, added) = file
            .update(|settings| settings.add_ssh_machine("Build box", " dev@build.example.com "))
            .expect("added");
        assert!(
            file.update(|settings| settings.add_ssh_machine("", "-oProxyCommand=x"))
                .is_err()
        );
        let saved = nexus_desktop_core::settings::load(file.path());
        assert_eq!(
            ssh_servers(&saved)
                .iter()
                .map(|server| server.ssh.as_str())
                .collect::<Vec<_>>(),
            vec!["dev@build.example.com"]
        );
        assert!(
            file.update(|settings| remove_server(settings, "cloud-1")).is_err(),
            "cloud machines stay"
        );
        file.update(|settings| remove_server(settings, &added.id))
            .expect("removed");
        let saved = nexus_desktop_core::settings::load(file.path());
        assert!(ssh_servers(&saved).is_empty());
        assert_eq!(saved.machines.len(), 1);
    }
}
