//! Machines in the sidebar. A project's icon says where it runs (this computer, a server, the cloud). The servers' heads
//! (status, Retry or Update, and a + for a new session there) follow the projects and the sessions put aside, in a
//! section of their own under a hairline.
//!
//! A cloud session has one place, whatever its machine does: its repository's project. Its row shows a cloud in place
//! of the idle ring, and its machine's state as a small mark before its age: a quiet moon while asleep (a click only
//! opens it; the machine wakes when the person types or sends there, `cloud_view.rs`), a dot while it is not connected.
//! Before its machine is read, the session files by the repository this device started it on (`CloudRow`). One whose
//! repository has no project in the list gets a cloud project of its own, named for the repository; one with no known
//! repository goes in a project named "Cloud". Agent runs never wait among the chats. A machine that needs the person
//! says so once, in the cloud strip at the foot of the sidebar, with Retry.

use std::collections::HashMap;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::sidebar::{ROW, small_button, status_mark};
use super::{SettingsSection, Workspace, glyph};
use crate::cloud::words;
use crate::code::Status;
use crate::i18n::tr;
use crate::machine_view::{Action, Tone, status_view};
use crate::machines::{CloudView, ServerView};
use crate::pages::PageId;
use crate::palette::Palette;

const THIS_COMPUTER: &str = crate::code::THIS_COMPUTER;

impl Workspace {
    /// Where a project runs, as its icon: a folder on this computer, a server, or the cloud. The sidebar has one list
    /// of projects, so the place is a mark on the row, not a group. A repository with copies in several
    /// places (`repo_groups.rs`) takes the repository icon, and its tooltip names them.
    pub(super) fn project_place_icon(&self, machines: &[Option<&str>], folded: bool, palette: Palette) -> AnyElement {
        let mut places: Vec<(Lucide, String)> = Vec::new();
        for machine in machines {
            let place = self.place_of(*machine, folded);
            if !places.iter().any(|(_, name)| *name == place.1) {
                places.push(place);
            }
        }
        let (icon, tip): (Lucide, SharedString) = match places.as_slice() {
            [] => (Lucide::Folder, THIS_COMPUTER.into()),
            [(icon, name)] => (*icon, name.clone().into()),
            several => (
                Lucide::FolderGit2,
                several
                    .iter()
                    .map(|(_, name)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into(),
            ),
        };
        div()
            .id("place")
            .flex()
            .child(glyph(icon, palette.icon))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .into_any_element()
    }

    /// One place's icon and name.
    pub(super) fn place_of(&self, machine: Option<&str>, folded: bool) -> (Lucide, String) {
        match machine {
            Some(id) if self.cloud_machines.iter().any(|machine| machine.id == id) => {
                (Lucide::Cloud, tr("Cloud").to_string())
            }
            Some(id) => {
                let name = self
                    .servers
                    .iter()
                    .find(|server| server.id == id)
                    .map(|server| server.name.to_string())
                    .unwrap_or_else(|| tr("a server").to_string());
                (Lucide::Server, tr("On {name}").replace("{name}", &name))
            }
            None if folded => (Lucide::Folder, THIS_COMPUTER.into()),
            None => (Lucide::FolderOpen, THIS_COMPUTER.into()),
        }
    }

    /// The cloud machine session `id` runs on, when its project was read from one.
    pub(super) fn cloud_machine_running(&self, id: &str) -> Option<&CloudView> {
        let machine = self
            .projects
            .iter()
            .find(|project| project.sessions.iter().any(|session| session.id == id))?
            .machine
            .as_deref()?;
        self.cloud_machines.iter().find(|cloud| cloud.id == machine)
    }

    /// Cloud machines whose projects were not read yet (the app just started, or it could not reach them), whose
    /// session clones the repository `key`. With no key, those whose repository has no project in the list: their
    /// rows wait in Recent.
    pub(super) fn unread_cloud_machines_in(&self, key: Option<&str>) -> Vec<&CloudView> {
        let listed: std::collections::HashSet<String> = self
            .projects
            .iter()
            .filter_map(|project| super::repo_groups::repo_key(&project.remotes))
            .collect();
        self.cloud_machines
            .iter()
            .filter(|machine| {
                !self
                    .projects
                    .iter()
                    .any(|project| project.machine.as_deref() == Some(machine.id.as_str()))
            })
            .filter(|machine| {
                let repo = machine_repo_key(machine);
                match key {
                    Some(key) => repo.as_deref() == Some(key),
                    None => repo.is_none_or(|repo| !listed.contains(&repo)),
                }
            })
            .collect()
    }

    /// Cloud projects: cloud sessions whose repository has no project in the list, one project per repository, named
    /// for it, with a cloud for its icon. Agent runs stay in projects, never among the chats. Sessions with no known
    /// repository share one project named "Cloud".
    pub(super) fn render_cloud_projects(&self, palette: Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        cloud_projects(self.unread_cloud_machines_in(None))
            .into_iter()
            .map(|project| self.render_cloud_project(project, palette, cx))
            .collect()
    }

    fn render_cloud_project(&self, project: CloudProject<'_>, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = format!("cloud-repo:{}", project.key);
        let folded = self.folded_projects.contains(&id);
        let head = h_flex()
            .id(SharedString::from(id.clone()))
            .h(px(ROW))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(8.))
            .text_size(px(super::sidebar::ROW_TEXT))
            .text_color(palette.secondary)
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.folded_projects.remove(&id) {
                    this.folded_projects.insert(id.clone());
                }
                cx.notify();
            }))
            .child(cloud_glyph(palette))
            .child(div().flex_1().min_w(px(0.)).truncate().child(project.name))
            .child(super::sidebar::hint(project.machines.len().to_string(), palette));
        let rows: Vec<AnyElement> = if folded {
            Vec::new()
        } else {
            project
                .machines
                .into_iter()
                .map(|machine| self.cloud_row(machine, 24., palette, cx).into_any_element())
                .collect()
        };
        v_flex().child(head).children(rows).into_any_element()
    }

    /// The cloud machines that need the person: not asleep, and offline, failed or reconnecting. Their state shows
    /// once, at the foot of the sidebar, not on every row.
    fn troubled_cloud_machines(&self) -> Vec<&CloudView> {
        let now = now_ms();
        self.cloud_machines
            .iter()
            .filter(|machine| !words::is_asleep(&machine.info))
            .filter(|machine| troubled(&words::cloud_view(&machine.info, &machine.status, now)))
            .collect()
    }

    /// The cloud strip: "Cloud · Reconnecting…" and Retry, while a cloud machine is not connected. Nothing otherwise.
    pub(super) fn render_cloud_strip(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let troubled = self.troubled_cloud_machines();
        let first = troubled.first()?;
        let view = words::cloud_view(&first.info, &first.status, now_ms());
        let label: SharedString = match troubled.len() {
            1 => tr("Cloud · {status}").replace("{status}", &view.label).into(),
            n => tr("Cloud · {count} machines not connected")
                .replace("{count}", &n.to_string())
                .into(),
        };
        let tip: SharedString = troubled
            .iter()
            .map(|machine| {
                let view = words::cloud_view(&machine.info, &machine.status, now_ms());
                format!("{}: {}", cloud_title(machine), view.label)
            })
            .collect::<Vec<_>>()
            .join("\n")
            .into();
        let ids: Vec<(String, bool)> = troubled
            .iter()
            .map(|machine| {
                (
                    machine.id.clone(),
                    machine.info.state == crate::cloud::CloudState::Failed,
                )
            })
            .collect();
        Some(
            h_flex()
                .id("cloud-strip")
                .mx(px(8.))
                .mt(px(4.))
                .h(px(32.))
                .px(px(8.))
                .gap(px(8.))
                .border_t_1()
                .border_color(palette.border)
                .text_xs()
                .text_color(palette.muted)
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .child(cloud_glyph(palette))
                .child(div().flex_1().min_w(px(0.)).truncate().child(label))
                .child(
                    side_link("cloud-strip-retry".into(), tr("Retry"), palette).on_click(cx.listener(
                        move |_, _, _, cx| {
                            for (id, failed) in &ids {
                                let _ = match crate::cloud::get() {
                                    Some(cloud) if *failed => cloud.retry(id),
                                    _ => crate::machines::get().reconnect(id),
                                };
                            }
                            cx.notify();
                        },
                    )),
                )
                .into_any_element(),
        )
    }

    /// The servers, each with its status and "+", after the projects and the sessions put aside. A hairline and a
    /// gap start them as a section of their own, so no row above reads as a server's. Their projects are in the one
    /// list above.
    pub(super) fn render_servers(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.servers.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .id("sidebar-servers")
                .mt(px(12.))
                .pt(px(8.))
                .border_t_1()
                .border_color(palette.border)
                .child(
                    h_flex()
                        .child(div().flex_1().child(machine_title(tr("Servers"), palette)))
                        .child(
                            small_button("servers-add", Lucide::Plus, palette)
                                .mr(px(2.))
                                .tooltip(|window, cx| Tooltip::new(tr("Add a server")).build(window, cx))
                                .on_click(cx.listener(|this, _, window, cx| this.open_servers(window, cx))),
                        ),
                )
                .children(
                    self.servers
                        .iter()
                        .map(|server| self.render_server_head(server, palette, cx).into_any_element()),
                )
                .into_any_element(),
        )
    }

    /// A server's row: its icon, its name, its status and, when connected, its folder and "+" buttons. A server
    /// that is not connected says why on a second line, with Retry or Update beside it.
    fn render_server_head(&self, server: &ServerView, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let view = status_view(&server.status);
        let tip: SharedString = [
            Some(server.ssh.clone()),
            Some(view.label.to_string()),
            view.detail.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
        .into();
        let mark = match view.tone {
            Tone::Ok => div()
                .size(px(16.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(7.)).rounded_full().bg(palette.muted))
                .into_any_element(),
            Tone::Busy => status_mark(&format!("machine-{}", server.id), Status::Running, palette),
            Tone::Warn => status_mark(&server.id, Status::NeedsYou, palette),
            Tone::Off => status_mark(&server.id, Status::Idle, palette),
        };
        let connected = matches!(server.status, nexus_remote::machine::Status::Connected { .. });
        let needs_update = matches!(server.status, nexus_remote::machine::Status::NeedsUpdate { .. });
        let (retry_id, new_id) = (server.id.clone(), server.id.clone());
        // While a pressed Retry's attempt runs, the line keeps the reason it was pressed for and its icon spins.
        let retrying = self.server_retries.get(&server.id);
        let retry = view.action == Some(Action::Retry) || retrying.is_some();
        let detail = match retrying {
            Some(retrying) => retrying.detail.clone(),
            None => view.detail.clone(),
        };
        let retry_detail = view.detail.clone();
        // The second line: why it is not connected, and the one thing to do about it.
        let fix = (retry || needs_update).then(|| {
            h_flex()
                .h(px(24.))
                .pl(px(32.))
                .pr(px(2.))
                .gap(px(8.))
                .text_xs()
                .text_color(palette.faint)
                .child(div().flex_1().min_w(px(0.)).truncate().children(detail))
                .when(retry, |line| {
                    let id = SharedString::from(format!("server-retry-{}", server.id));
                    line.child(match retrying {
                        Some(_) => retry_spinner(id, palette),
                        None => small_button(id, Lucide::RotateCw, palette)
                            .tooltip(|window, cx| Tooltip::new(tr("Retry")).build(window, cx))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if crate::machines::get().reconnect(&retry_id).is_ok() {
                                    this.server_retries
                                        .insert(retry_id.clone(), ServerRetry::new(retry_detail.clone()));
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                    })
                })
                .when(needs_update, |line| {
                    line.child(
                        side_button(
                            SharedString::from(format!("server-update-{}", server.id)),
                            tr("Update"),
                            palette,
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.open_servers(window, cx))),
                    )
                })
        });
        let head = h_flex()
            .h(px(ROW))
            .pr(px(2.))
            .gap(px(4.))
            .child(
                h_flex()
                    .id(SharedString::from(format!("server-{}", server.id)))
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(8.))
                    .text_size(px(super::sidebar::ROW_TEXT))
                    .text_color(palette.secondary)
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.open_servers(window, cx)))
                    .child(glyph(Lucide::Server, palette.icon))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(server.name.clone()))
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(px(4.))
                            .text_xs()
                            .text_color(palette.faint)
                            .child(mark)
                            .when(view.tone != Tone::Ok, |status| status.child(view.label)),
                    ),
            )
            .when(connected, |head| {
                head.child({
                    let tip: SharedString = tr("Add a folder on {server} as a project")
                        .replace("{server}", &server.name)
                        .into();
                    let id = server.id.clone();
                    small_button(
                        SharedString::from(format!("server-folder-{}", server.id)),
                        Lucide::FolderPlus,
                        palette,
                    )
                    .focusable()
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| this.toggle_server_folder(&id, window, cx)))
                })
            })
            .when(connected, |head| {
                head.child({
                    let tip: SharedString = tr("New session on {server}").replace("{server}", &server.name).into();
                    small_button(
                        SharedString::from(format!("server-new-{}", server.id)),
                        Lucide::Plus,
                        palette,
                    )
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| this.new_session_on(&new_id, window, cx)))
                })
            });
        v_flex().child(head).children(fix)
    }

    /// `CloudRow`: a cloud session whose machine was not read yet, by its title, a cloud, and its machine's state as a
    /// small mark (`machine_state_mark`). It sits under its repository's project (`indent` 24) or in Recent (8). A
    /// machine made before sessions had their own opens Settings › Cloud.
    pub(super) fn cloud_row(
        &self,
        machine: &CloudView,
        indent: f32,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = words::cloud_view(&machine.info, &machine.status, now_ms());
        let title: SharedString = cloud_title(machine).into();
        let tip: SharedString = [
            Some(title.to_string()),
            Some(view.label.to_string()),
            view.detail.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
        .into();
        let session = machine.info.session_id.clone();
        let asleep = words::is_asleep(&machine.info);
        let selected = session
            .as_deref()
            .is_some_and(|session| matches!(&self.view, super::View::Code(open) if open.id == session));
        h_flex()
            .id(SharedString::from(format!("cloud-{}", machine.id)))
            .h(px(ROW))
            .pl(px(indent))
            .pr(px(8.))
            .gap(px(8.))
            .when(selected, |row| row.bg(palette.selected))
            .rounded(px(8.))
            .text_size(px(super::sidebar::ROW_TEXT))
            .text_color(palette.secondary)
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, window, cx| match &session {
                Some(session) => this.open_cloud_session(session, window, cx),
                None => this.open_settings(SettingsSection::Page(PageId::Cloud), window, cx),
            }))
            // Created or woken, it spins as a working session does; otherwise it shows the cloud it runs in.
            .child(if view.tone == Tone::Busy {
                status_mark(&format!("cloud-{}", machine.id), Status::Running, palette)
            } else {
                cloud_glyph(palette)
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .when(asleep, |title| title.text_color(palette.faint))
                    .child(title),
            )
            .when(view.tone != Tone::Busy, |row| {
                row.children(machine_state_mark(machine, palette))
            })
    }

    fn open_servers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(SettingsSection::Page(PageId::Servers), window, cx);
    }

    /// The + of a server: a new session in its latest project, else the field for a folder there (it has no project
    /// yet).
    fn new_session_on(&mut self, machine: &str, window: &mut Window, cx: &mut Context<Self>) {
        let project = self
            .projects
            .iter()
            .find(|project| project.machine.as_deref() == Some(machine))
            .map(|project| project.id.clone());
        match project {
            Some(project) => self.open_new_code_in(project, window, cx),
            None => self.toggle_server_folder(machine, window, cx),
        }
    }
}

/// A group's title in the sidebar's small, faint type.
fn machine_title(title: SharedString, palette: Palette) -> impl IntoElement {
    h_flex()
        .h(px(28.))
        .mt(px(4.))
        .pl(px(8.))
        .text_xs()
        .text_color(palette.faint)
        .child(title)
}

/// What a sleeping cloud session shows in place of its state: a small moon, "Asleep" on hover. Nothing to press.
fn asleep_mark(machine: &str, palette: Palette) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("cloud-asleep-{machine}")))
        .flex_none()
        .child(glyph(Lucide::Moon, palette.faint).size(px(12.)))
        .tooltip(|window, cx| Tooltip::new(tr("Asleep")).build(window, cx))
}

/// The sleeping mark's whole tooltip: no more to say.

/// A cloud session's mark in place of the idle ring: where it runs, at a glance.
pub(super) fn cloud_glyph(palette: Palette) -> AnyElement {
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(glyph(Lucide::Cloud, palette.faint).size(px(13.)))
        .into_any_element()
}

/// A cloud machine's state on its sessions' rows, before their age: a moon while asleep, a dot while it is not
/// connected (the waiting tone while it reconnects or wakes, the destructive one when offline or failed), nothing
/// while it runs. The tooltip names the state; the cloud strip says it in words (`render_cloud_strip`).
pub(super) fn machine_state_mark(machine: &CloudView, palette: Palette) -> Option<AnyElement> {
    if words::is_asleep(&machine.info) {
        return Some(asleep_mark(&machine.id, palette).into_any_element());
    }
    let view = words::cloud_view(&machine.info, &machine.status, now_ms());
    let color = match view.tone {
        Tone::Ok => return None,
        Tone::Busy => palette.waiting(),
        Tone::Warn | Tone::Off => palette.destructive,
    };
    let label = view.label;
    Some(
        div()
            .id(SharedString::from(format!("cloud-state-{}", machine.id)))
            .size(px(12.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(6.)).rounded_full().bg(color))
            .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
            .into_any_element(),
    )
}

/// Whether a cloud machine's state needs the person: failed, offline with a Retry, or reconnecting.
fn troubled(view: &crate::machine_view::StatusView) -> bool {
    view.tone == Tone::Warn || view.action == Some(Action::Retry) || view.label == "Reconnecting…"
}

/// A cloud session's title as Gateway keeps it, else its machine's name.
fn cloud_title(machine: &CloudView) -> String {
    machine
        .info
        .title
        .clone()
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| machine.name.clone())
}

/// A cloud project: the cloud sessions on one repository that has no project here.
pub(super) struct CloudProject<'a> {
    /// The repository's key (`repo_groups::repo_key`); empty for sessions with no known repository.
    key: String,
    name: String,
    machines: Vec<&'a CloudView>,
}

/// Groups cloud sessions by repository, in the order given; the ones with no known repository go last, as "Cloud".
fn cloud_projects<'a>(machines: impl IntoIterator<Item = &'a CloudView>) -> Vec<CloudProject<'a>> {
    let mut projects: Vec<CloudProject<'a>> = Vec::new();
    for machine in machines {
        let key = machine_repo_key(machine).unwrap_or_default();
        match projects.iter_mut().find(|project| project.key == key) {
            Some(project) => project.machines.push(machine),
            None => projects.push(CloudProject {
                name: repo_name(machine.info.repo.as_deref()),
                key,
                machines: vec![machine],
            }),
        }
    }
    projects.sort_by_key(|project| project.key.is_empty());
    projects
}

/// A repository's short name, in lower case ("nexus" for `github.com/acme/nexus`); "Cloud" with none.
fn repo_name(repo: Option<&str>) -> String {
    repo.and_then(crate::links::repo_ref)
        .and_then(|repo| repo.path.rsplit('/').next().map(str::to_string))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Cloud".into())
}

/// The repository a cloud machine's session clones, keyed as `repo_groups::repo_key`, when this device knows it.
fn machine_repo_key(machine: &CloudView) -> Option<String> {
    let repo = machine.info.repo.as_ref()?;
    super::repo_groups::repo_key(std::slice::from_ref(repo))
}

/// A server whose Retry was pressed: the reason shown then, and whether its new attempt has begun.
pub(super) struct ServerRetry {
    detail: Option<String>,
    started: bool,
    pressed: std::time::Instant,
}

impl ServerRetry {
    fn new(detail: Option<String>) -> Self {
        Self {
            detail,
            started: false,
            pressed: std::time::Instant::now(),
        }
    }
}

/// How long a pressed Retry waits for its attempt to begin before it stops spinning.
const RETRY_START: std::time::Duration = std::time::Duration::from_secs(3);

/// Ends the retries whose attempt is over: the server left "Connecting" after it began, the server is gone, or the
/// attempt never began.
pub(super) fn settle_retries(retries: &mut HashMap<String, ServerRetry>, servers: &[ServerView]) {
    let now = std::time::Instant::now();
    retries.retain(|id, retry| {
        let Some(server) = servers.iter().find(|server| &server.id == id) else {
            return false;
        };
        if matches!(server.status, nexus_remote::machine::Status::Connecting { .. }) {
            retry.started = true;
            return true;
        }
        !retry.started && now.duration_since(retry.pressed) < RETRY_START
    });
}

/// The Retry icon while its attempt runs: it turns and takes no click.
fn retry_spinner(id: SharedString, palette: Palette) -> AnyElement {
    div()
        .id(id.clone())
        .size(px(22.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .tooltip(|window, cx| Tooltip::new(tr("Retrying…")).build(window, cx))
        .child(
            glyph(Lucide::RotateCw, palette.icon).with_animation(
                id,
                Animation::new(std::time::Duration::from_millis(900))
                    .repeat()
                    .with_max_fps(crate::motion::LOOP_FPS),
                |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
            ),
        )
        .into_any_element()
}

/// A server's fix ("Update"): a small outlined button, so the one action on the line reads as pressable.
fn side_button(id: SharedString, label: SharedString, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(8.))
        .py(px(1.))
        .rounded(px(6.))
        .border_1()
        .border_color(palette.border)
        .text_xs()
        .text_color(palette.foreground)
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .child(label)
}

/// A small text action in a group's head ("Retry", "Update").
fn side_link(id: SharedString, label: SharedString, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(4.))
        .rounded(px(4.))
        .text_xs()
        .text_color(palette.muted)
        .cursor_pointer()
        .hover(|style| style.text_color(palette.foreground).bg(palette.hover))
        .child(label)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use nexus_remote::machine::Status;

    use super::{RETRY_START, ServerRetry, repo_name, settle_retries};
    use crate::machines::ServerView;

    fn server(status: Status) -> ServerView {
        ServerView {
            id: "box".into(),
            name: "box".into(),
            ssh: "box.invalid".into(),
            busy: 0,
            status,
        }
    }

    fn offline() -> Status {
        Status::Unreachable {
            error: "timed out".into(),
            retry_in_secs: 30,
        }
    }

    #[test]
    fn a_retry_spins_until_its_attempt_ends() {
        let mut retries = HashMap::from([("box".to_string(), ServerRetry::new(Some("why".into())))]);
        // Pressed, not yet begun: it keeps spinning.
        settle_retries(&mut retries, &[server(offline())]);
        assert!(retries.contains_key("box"));
        // The attempt runs.
        settle_retries(&mut retries, &[server(Status::Connecting { attempt: 2 })]);
        assert!(retries.contains_key("box"));
        // It failed again: the icon stops and can be pressed.
        settle_retries(&mut retries, &[server(offline())]);
        assert!(retries.is_empty());
    }

    #[test]
    fn a_retry_that_never_begins_or_whose_server_is_gone_ends() {
        let mut stale = ServerRetry::new(None);
        stale.pressed -= RETRY_START;
        let mut retries = HashMap::from([("box".to_string(), stale)]);
        settle_retries(&mut retries, &[server(offline())]);
        assert!(retries.is_empty());

        let mut retries = HashMap::from([("box".to_string(), ServerRetry::new(None))]);
        settle_retries(&mut retries, &[]);
        assert!(retries.is_empty());
    }

    #[test]
    fn a_cloud_project_is_named_for_its_repository() {
        // `repo_ref` keys a repository in lower case, as the projects are matched.
        assert_eq!(repo_name(Some("https://github.com/acme/Web-App")), "web-app");
        assert_eq!(repo_name(Some("git@github.com:acme/nexus.git")), "nexus");
        assert_eq!(repo_name(None), "Cloud");
    }
}
