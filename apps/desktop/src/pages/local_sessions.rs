//! Settings › Local sessions, "History" in the nav (the Nexus app's `settings/connections.tsx` with Nexus UI's
//! `local-sessions.tsx`): the conversations Claude Code and Codex keep on this computer, newest first, from a terminal,
//! `nexus claude` / `nexus codex` or any other app, with where each ran and the command that resumes it through Nexus.
//! It reads them as the Nexus app does (`nexus_client::local_sessions::list`): only the newest transcripts' first and
//! last lines, and nothing leaves the computer.
//!
//! Resume in Terminal opens a terminal window on the conversation
//! (`nexus_desktop_core::terminal_window`): a script in this build's cache folder
//! goes into the folder and runs the resume command in the person's login shell, and the OS opens it in a terminal. The
//! row also copies the command. The agents' transcripts have no archive or delete in the Nexus app either.

mod changes;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_client::local_sessions::LocalSession;
use nexus_desktop_core::session_history;

use super::desktop_app::{off_thread, save_settings, tilde};
use super::{DeviceCtx, Load, palette, row, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

/// How many conversations it reads (the Nexus app's `LIST_LIMIT`), and how many show before Show all.
const LIMIT: usize = 50;
const FIRST: usize = 12;
const WATCH_UNAVAILABLE: &str =
    "History is paused because Nexus could not monitor privacy changes. Retry before reading local conversations.";

pub fn build(ctx: DeviceCtx, _window: &mut Window, cx: &mut App) -> Option<AnyView> {
    Some(page(ctx, false, cx).into())
}

/// Privacy uses the same control without discovering or displaying any conversations.
pub(super) fn controls(ctx: DeviceCtx, cx: &mut App) -> AnyView {
    page(ctx, true, cx).into()
}

type HistoryReader =
    Arc<dyn Fn(bool, ReadTicket) -> futures::future::BoxFuture<'static, (bool, Vec<LocalSession>)> + Send + Sync>;

/// Keep the live watcher until the last page closes. Tests inject their own change signal.
enum RevocationWatch {
    Live {
        _watcher: Rc<changes::SettingsWatch>,
    },
    #[cfg(test)]
    Synthetic,
}
type WatchSetup = Rc<dyn Fn(&mut App) -> Option<RevocationWatch>>;

fn page(ctx: DeviceCtx, controls_only: bool, cx: &mut App) -> Entity<LocalSessionsPage> {
    page_with_reader(
        ctx,
        controls_only,
        Arc::new(|controls_only, ticket| Box::pin(read_saved_history(controls_only, ticket))),
        Rc::new(|cx| changes::watch_settings(cx).map(|watcher| RevocationWatch::Live { _watcher: watcher })),
        cx,
    )
}

fn page_with_reader(
    ctx: DeviceCtx,
    controls_only: bool,
    reader: HistoryReader,
    watch_setup: WatchSetup,
    cx: &mut App,
) -> Entity<LocalSessionsPage> {
    let observed_revision = read_ticket(cx).value;
    cx.new(|cx| {
        let history_changed = cx.observe_global::<HistoryRevision>(|this: &mut LocalSessionsPage, cx| {
            let revision = read_ticket(cx).value;
            if revision != this.observed_revision {
                this.observed_revision = revision;
                this.generation += 1;
                this.history.clear();
                this.history.sessions = Load::Loading;
                this.copied = None;
                this.resuming.clear();
                this.all = false;
                this.load(cx);
                cx.notify();
            }
        });
        let mut page = LocalSessionsPage {
            ctx,
            controls_only,
            reader,
            watch_setup,
            settings_watch: None,
            history: HistorySnapshot {
                enabled: false,
                sessions: Load::Loading,
            },
            saving: false,
            generation: 0,
            observed_revision,
            _history_changed: history_changed,
            all: false,
            copied: None,
            resuming: HashMap::new(),
        };
        page.load(cx);
        page
    })
}

/// The agent's name as people say it (`AGENT_NAMES`): its entry's name, or the id when no agent has it.
fn agent_name(agent: &str) -> &str {
    nexus_client::agent_entries::Agent::parse(agent).map_or(agent, |agent| agent.entry().identity.name)
}

/// Where a conversation ran (`ENTRYPOINTS`).
fn where_ran(entrypoint: &str) -> &str {
    match entrypoint {
        "cli" | "codex_cli_rs" => "in a terminal",
        "claude-desktop" => "in Claude Desktop",
        "claude-vscode" | "codex_vscode" => "in VS Code",
        "sdk-ts" | "sdk-py" => "from the SDK",
        other => other,
    }
}

/// The row's project, else its folder.
fn project_of(session: &LocalSession) -> String {
    match session.project.as_deref().filter(|project| !project.is_empty()) {
        Some(project) => project.to_string(),
        None => session.cwd.as_deref().map(tilde).unwrap_or_else(|| "–".into()),
    }
}

/// Milliseconds since 1970 from an ISO time (`2026-11-05T14:03:00.123Z`); `None` when it does not read.
fn iso_ms(at: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| at.get(range)?.parse::<i64>().ok();
    let (y, m, d) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hh, mm, ss) = (number(11..13)?, number(14..16)?, number(17..19)?);
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + hh) * 60 + mm) * 60_000 + ss * 1000)
}

/// "5m ago", or the day when the time does not read.
fn last_active(updated_at: &str) -> String {
    match iso_ms(updated_at) {
        Some(ms) => match crate::code::age(ms, crate::code::now_ms()).as_str() {
            "now" => "now".into(),
            age => format!("{age} ago"),
        },
        None => ui::day(updated_at).to_string(),
    }
}

/// Consent stays in settings.json. This signal only invalidates reads across app windows.
#[derive(Default)]
struct HistoryRevision(Arc<AtomicU64>);
impl Global for HistoryRevision {}

#[derive(Clone)]
struct ReadTicket {
    revision: Arc<AtomicU64>,
    value: u64,
}
impl ReadTicket {
    fn current(&self) -> bool {
        self.revision.load(Ordering::SeqCst) == self.value
    }
}

fn read_ticket(cx: &mut App) -> ReadTicket {
    if !cx.has_global::<HistoryRevision>() {
        cx.set_global(HistoryRevision::default());
    }
    let revision = cx.global::<HistoryRevision>().0.clone();
    ReadTicket {
        value: revision.load(Ordering::SeqCst),
        revision,
    }
}

fn preference_changed(cx: &mut App) {
    let ticket = read_ticket(cx);
    ticket.revision.fetch_add(1, Ordering::SeqCst);
    cx.set_global(HistoryRevision(ticket.revision));
}

struct HistorySnapshot {
    enabled: bool,
    sessions: Load<Vec<LocalSession>>,
}
impl HistorySnapshot {
    fn clear(&mut self) {
        self.enabled = false;
        self.sessions = Load::Ready(Vec::new());
    }

    fn apply(&mut self, ticket: &ReadTicket, enabled: bool, sessions: Vec<LocalSession>) -> bool {
        if !ticket.current() {
            return false;
        }
        self.enabled = enabled;
        self.sessions = Load::Ready(if enabled { sessions } else { Vec::new() });
        true
    }
}

struct LocalSessionsPage {
    ctx: DeviceCtx,
    controls_only: bool,
    reader: HistoryReader,
    watch_setup: WatchSetup,
    settings_watch: Option<RevocationWatch>,
    history: HistorySnapshot,
    saving: bool,
    generation: u64,
    observed_revision: u64,
    _history_changed: Subscription,
    all: bool,
    /// The conversation whose command was just copied (agent and id).
    copied: Option<(String, String)>,
    /// Resume in Terminal by conversation (agent and id): opening while `None`, else why it failed.
    resuming: HashMap<(String, String), Option<String>>,
}

/// Where the resume scripts go: this build's cache folder.
fn scripts_dir() -> std::path::PathBuf {
    crate::paths::cache_dir().join("resume")
}

/// A privacy control never needs transcript access, even after the person opted in.
fn discover(
    settings: &nexus_desktop_core::settings::Settings,
    controls_only: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<LocalSession> {
    if controls_only {
        Vec::new()
    } else {
        session_history::list(settings, env, LIMIT)
    }
}

async fn read_saved_history(controls_only: bool, ticket: ReadTicket) -> (bool, Vec<LocalSession>) {
    let settings = crate::desktop_settings::load();
    let sessions = if ticket.current() {
        discover(&settings, controls_only, &|name| std::env::var(name).ok())
    } else {
        Vec::new()
    };
    // The saved preference may have changed while a scan was running.
    let enabled = session_history::enabled(&crate::desktop_settings::load());
    (enabled, sessions)
}

impl LocalSessionsPage {
    fn load(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        if self.settings_watch.is_none() {
            self.settings_watch = (self.watch_setup)(cx);
            if self.settings_watch.is_none() {
                self.history.clear();
                self.history.sessions = Load::Loading;
                self.copied = None;
                self.resuming.clear();
                self.all = false;
            }
        }
        // Consent can still load when monitoring fails. Transcript discovery must fail closed.
        let controls_only = self.controls_only || self.settings_watch.is_none();
        let reader = self.reader.clone();
        let ticket = read_ticket(cx);
        let work_ticket = ticket.clone();
        if !matches!(self.history.sessions, Load::Ready(_)) {
            self.history.sessions = Load::Loading;
        }
        let task = cx.background_executor().spawn(reader(controls_only, work_ticket));
        cx.spawn(async move |this, cx| {
            let (enabled, sessions) = task.await;
            this.update(cx, |this, cx| {
                this.apply_read(generation, &ticket, enabled, sessions);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply_read(&mut self, generation: u64, ticket: &ReadTicket, enabled: bool, sessions: Vec<LocalSession>) {
        if self.generation != generation || !ticket.current() {
            return;
        }
        if !self.controls_only && self.settings_watch.is_none() {
            self.history.enabled = enabled;
            self.history.sessions = Load::Failed(WATCH_UNAVAILABLE.into());
            return;
        }
        self.history
            .apply(ticket, enabled, if self.controls_only { Vec::new() } else { sessions });
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.saving || matches!(self.history.sessions, Load::Loading) || !crate::desktop_settings::writable() {
            return;
        }
        let on = !self.history.enabled;
        self.saving = true;
        self.generation += 1;
        // Clear titles before the asynchronous save, including an outstanding discovery response.
        self.history.sessions = Load::Ready(Vec::new());
        save_settings(
            &self.ctx,
            cx,
            move |settings| {
                settings.local_session_history = Some(on);
                Ok(())
            },
            |this, saved, cx| {
                this.saving = false;
                match saved {
                    Ok((_settings, ())) => preference_changed(cx),
                    Err(error) => this.history.sessions = Load::Failed(error.into()),
                }
            },
        );
        cx.notify();
    }

    fn copy(&mut self, session: &LocalSession, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(session.resume.clone()));
        self.copied = Some((session.agent.clone(), session.id.clone()));
        cx.notify();
    }

    /// Open a terminal on the conversation, off the UI thread: it reads the transcript again and, on macOS, waits for
    /// `open`.
    fn resume(&mut self, session: &LocalSession, cx: &mut Context<Self>) {
        let key = (session.agent.clone(), session.id.clone());
        if self.resuming.get(&key).is_some_and(Option::is_none) {
            return;
        }
        self.resuming.insert(key.clone(), None);
        cx.notify();
        let (agent, id) = key.clone();
        let ticket = read_ticket(cx);
        let work_ticket = ticket.clone();
        off_thread(
            cx,
            move || {
                if !work_ticket.current() {
                    return Err(session_history::DISABLED.into());
                }
                session_history::resume(
                    &crate::desktop_settings::load(),
                    &scripts_dir(),
                    &|name| std::env::var(name).ok(),
                    &agent,
                    &id,
                )
            },
            move |this, opened, _| {
                if !ticket.current() {
                    return;
                }
                match opened {
                    Ok(()) => {
                        this.resuming.remove(&key);
                    }
                    Err(error) => {
                        this.resuming.insert(key, Some(error));
                    }
                }
            },
        );
    }

    fn render_row(&self, ix: usize, session: &LocalSession, palette: Palette, cx: &mut Context<Self>) -> Div {
        let copied = self
            .copied
            .as_ref()
            .is_some_and(|(agent, id)| *agent == session.agent && *id == session.id);
        let agent = match session.entrypoint.as_deref() {
            Some(entrypoint) => format!("{} {}", agent_name(&session.agent), where_ran(entrypoint)),
            None => agent_name(&session.agent).to_string(),
        };
        let title = short_title(session.title.as_deref().unwrap_or("Untitled")).to_string();
        // Who and where, short: the title already names the work, and a branch or model only crowds the line.
        let detail = [agent, project_of(session), last_active(&session.updated_at)].join(" · ");
        let row_session = session.clone();
        let copy_session = session.clone();
        let resume_session = session.clone();
        let resuming = self.resuming.get(&(session.agent.clone(), session.id.clone()));
        let opening = resuming.is_some_and(Option::is_none);
        let error = resuming.cloned().flatten();
        let group = SharedString::from(format!("local-row-{ix}"));
        // The row click copies the resume command. Its other actions (and Copy again, by name) show on hover in one
        // cluster over the row's end, so the title keeps the full width; "Copied" stays pinned there after a click.
        let cluster = row::actions(group.clone(), copied, palette)
            .child(
                row::icon_button(
                    ("local-resume", ix),
                    Lucide::SquareTerminal,
                    if opening { "Opening…" } else { "Resume in Terminal" },
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.resume(&resume_session, cx)
                })),
            )
            .when(copied, |cluster| {
                cluster.child(row::status(Lucide::Check, "Copied", palette))
            })
            .when(!copied, |cluster| {
                cluster.child(
                    row::icon_button(("local-copy", ix), Lucide::Copy, "Copy resume command", palette).on_click(
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.copy(&copy_session, cx)
                        }),
                    ),
                )
            })
            .children(session.pr_url.clone().map(|url| {
                row::icon_button(("local-pr", ix), Lucide::GitPullRequest, "Open pull request", palette).on_click(
                    move |_, _, cx| {
                        cx.stop_propagation();
                        crate::external_link::open(&url, cx)
                    },
                )
            }));
        let row = ui::row(palette)
            .id(("local-row", ix))
            .relative()
            .group(group)
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.copy(&row_session, cx)))
            .child(ui::two_lines(title, detail, palette).flex_1().min_w_0())
            .child(cluster);
        match error {
            Some(error) => v_flex()
                .child(row)
                .child(ui::error_line(error, palette).px(px(8.)).py(px(4.))),
            None => div().child(row),
        }
    }
}

/// A conversation's title as a list shows it: its first line, and its first sentence when that line runs on (titles
/// are often the whole first prompt).
fn short_title(title: &str) -> &str {
    let line = title
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Untitled");
    match line.match_indices(". ").map(|(end, _)| end).find(|end| *end >= 24) {
        Some(end) => &line[..end],
        None => line,
    }
}

impl Render for LocalSessionsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let switch = ui::switch(
            "local-session-history",
            self.history.enabled,
            self.saving || matches!(self.history.sessions, Load::Loading) || !crate::desktop_settings::writable(),
            palette,
        )
        .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)));
        // Privacy draws the switch as its card's one row; the card's head and footnote are Privacy's own.
        if self.controls_only {
            let failed = match &self.history.sessions {
                Load::Failed(error) => Some(ui::failed(error.clone(), palette).mt(px(8.))),
                _ => None,
            };
            return v_flex()
                .w_full()
                .child(ui::group(
                    [Some(
                        ui::setting_row(
                            "Show conversations from other apps",
                            "Off by default. Lists Claude Code and Codex conversations in Computer > History.",
                            switch,
                            palette,
                        )
                        .into_any_element(),
                    )],
                    palette,
                ))
                .children(failed);
        }
        let mut page = v_flex().w_full().child(ui::setting(
            "Show conversations from other apps",
            "Off by default. Read local Claude Code and Codex titles and folders for Computer > History. Nothing leaves this computer.",
            switch,
            palette,
        ));
        page = page
            .child(ui::lead("Conversations from coding agents on this computer.", palette))
            .child(ui::section_head("On this computer", palette))
            .child(ui::note(
                "Claude Code and Codex conversations here, newest first. Resume one in a new terminal window, or copy \
                 its command: it runs through Nexus, in the conversation's folder.",
                palette,
            ));
        page = match &self.history.sessions {
            Load::Loading => page.child(ui::skeleton(3, palette)),
            Load::Failed(error) => page.child(ui::failed(error.clone(), palette)),
            Load::Ready(_) if !self.history.enabled => page.child(ui::note(
                "Local conversation history is off. Nexus does not discover conversations from other apps.",
                palette,
            )),
            Load::Ready(sessions) if sessions.is_empty() => page.child(ui::empty(
                "No conversations on this computer yet",
                "Claude Code and Codex conversations you start here show up in this list. Try nexus claude in a \
                 terminal.",
                palette,
            )),
            Load::Ready(sessions) => {
                let shown = if self.all {
                    sessions.len()
                } else {
                    FIRST.min(sessions.len())
                };
                let rows: Vec<Div> = sessions[..shown]
                    .iter()
                    .enumerate()
                    .map(|(ix, session)| self.render_row(ix, session, palette, cx))
                    .collect();
                let more = (sessions.len() > FIRST).then(|| {
                    let label = if self.all {
                        "Show fewer".to_string()
                    } else {
                        format!("Show all {}", sessions.len())
                    };
                    ui::button("local-more", label, palette).on_click(cx.listener(|this, _, _, cx| {
                        this.all = !this.all;
                        cx.notify();
                    }))
                });
                page.child(v_flex().mt(px(8.)).children(rows))
                    .child(h_flex().mt(px(10.)).gap(px(8.)).children(more))
            }
        };
        page.child(
            h_flex().mt(px(10.)).child(
                ui::ghost_button(
                    "local-refresh",
                    if self.settings_watch.is_some() {
                        "Refresh"
                    } else {
                        "Retry"
                    },
                    palette,
                )
                .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    struct ContextWindow;

    impl Render for ContextWindow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn context(_runtime: tokio::runtime::Handle, cx: &mut TestAppContext) -> DeviceCtx {
        let mut settings_fields = None;
        cx.add_window(|window, _| {
            settings_fields = Some(crate::settings_fields::SettingsFields::new(window, ScrollHandle::new()));
            ContextWindow
        });
        DeviceCtx {
            local_workspace: false,
            settings_fields: settings_fields.unwrap(),
            gateway_url: "http://gateway.invalid".into(),
            navigate: Rc::new(|_, _, _| {}),
            navigate_settings: Rc::new(|_, _, _| {}),
            switch_gateway: Rc::new(|_, _, _| Ok(())),
            settings_saved: Rc::new(|_, _| panic!("read-only view test must not save settings")),
            restart_for_update: Rc::new(|_| Ok(())),
            restart_for_service: Rc::new(|_| Ok(())),
            code: Rc::new(|_| super::super::CodeAccess::default()),
            new_code_session: Rc::new(|_, _| {}),
        }
    }

    #[gpui_kit::test]
    fn external_revocation_clears_real_pages_and_reenable_rejects_a_pending_scan(cx: &mut TestAppContext) {
        use futures::channel::oneshot;
        use std::{collections::VecDeque, sync::Mutex};

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ctx = context(runtime.handle().clone(), cx);
        // Scans use deferred synthetic responses. No settings, transcript, home or keychain is read.
        let scans = Arc::new(Mutex::new(VecDeque::<oneshot::Sender<(bool, Vec<LocalSession>)>>::new()));
        let reader: HistoryReader = Arc::new({
            let scans = scans.clone();
            move |controls_only, _| {
                assert!(!controls_only);
                let (send, receive) = oneshot::channel();
                scans.lock().unwrap().push_back(send);
                Box::pin(async move { receive.await.unwrap() })
            }
        });
        let (first, second, signal, _listener) = cx.update(|cx| {
            let (signal, listener) = changes::listen(cx);
            (
                page_with_reader(
                    ctx.clone(),
                    false,
                    reader.clone(),
                    Rc::new(|_| Some(RevocationWatch::Synthetic)),
                    cx,
                ),
                page_with_reader(ctx, false, reader, Rc::new(|_| Some(RevocationWatch::Synthetic)), cx),
                signal,
                listener,
            )
        });
        for _ in 0..2 {
            scans
                .lock()
                .unwrap()
                .pop_front()
                .unwrap()
                .send((true, vec![session()]))
                .unwrap();
        }
        cx.run_until_parked();
        for page in [&first, &second] {
            page.update(cx, |page, _| {
                assert!(page.history.enabled);
                assert!(matches!(&page.history.sessions, Load::Ready(rows) if rows.len() == 1));
                page.copied = Some(("claude".into(), "old".into()));
                page.resuming.insert(("claude".into(), "old".into()), None);
                page.all = true;
            });
        }
        let in_flight = cx.update(read_ticket);
        first.update(cx, |page, cx| page.load(cx));
        let old_scan = scans.lock().unwrap().pop_front().unwrap();
        let generations = [&first, &second].map(|page| page.read_with(cx, |page, _| page.generation));

        // This is the production file-watcher signal, delivered independently of the GPUI toggle.
        signal.notify();
        assert!(!in_flight.current(), "a watcher invalidates before the UI wakes");
        cx.run_until_parked();
        assert_eq!(scans.lock().unwrap().len(), 2, "both real page handlers must reload");
        for (page, generation) in [&first, &second].into_iter().zip(generations) {
            page.read_with(cx, |page, _| {
                assert!(!page.history.enabled);
                assert!(
                    matches!(&page.history.sessions, Load::Loading),
                    "cached titles are cleared while consent reloads"
                );
                assert!(page.generation > generation);
                assert!(page.copied.is_none() && page.resuming.is_empty() && !page.all);
            });
        }
        for _ in 0..2 {
            scans
                .lock()
                .unwrap()
                .pop_front()
                .unwrap()
                .send((false, vec![session()]))
                .unwrap();
        }
        cx.run_until_parked();
        for page in [&first, &second] {
            assert!(page.read_with(cx, |page, _| !page.history.enabled
                && matches!(&page.history.sessions, Load::Ready(rows) if rows.is_empty())));
        }

        // A later opt-in uses the same observer. Old work must stay revoked after it completes.
        cx.update(preference_changed);
        cx.run_until_parked();
        let mut fresh = session();
        fresh.title = Some("Fresh scan after opt-in".into());
        for _ in 0..2 {
            scans
                .lock()
                .unwrap()
                .pop_front()
                .unwrap()
                .send((true, vec![fresh.clone()]))
                .unwrap();
        }
        cx.run_until_parked();
        old_scan.send((true, vec![session()])).unwrap();
        cx.run_until_parked();
        for page in [&first, &second] {
            assert!(page.read_with(cx, |page, _| page.history.enabled
                && matches!(&page.history.sessions, Load::Ready(rows) if rows.len() == 1 && rows[0].title == fresh.title)));
        }
        let before_coalesced_changes = cx.update(read_ticket);
        signal.notify();
        signal.notify();
        assert!(
            !before_coalesced_changes.current(),
            "off -> on between UI wakeups still revokes old work"
        );
        // Drop pages before their final synthetic reload, as when the settings window closes.
        drop((first, second));
    }

    #[gpui_kit::test]
    fn unavailable_monitor_never_discovers_or_displays_history_until_retry_succeeds(cx: &mut TestAppContext) {
        use std::{cell::Cell, sync::atomic::AtomicUsize};

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let consent_reads = Arc::new(AtomicUsize::new(0));
        let transcript_reads = Arc::new(AtomicUsize::new(0));
        let reader: HistoryReader = Arc::new({
            let (consent_reads, transcript_reads) = (consent_reads.clone(), transcript_reads.clone());
            move |controls_only, _| {
                if controls_only {
                    consent_reads.fetch_add(1, Ordering::SeqCst);
                } else {
                    transcript_reads.fetch_add(1, Ordering::SeqCst);
                }
                // Even an erroneous consent-only response must not display conversation titles.
                Box::pin(async { (true, vec![session()]) })
            }
        });
        let available = Rc::new(Cell::new(false));
        let attempts = Rc::new(Cell::new(0));
        let setup: WatchSetup = Rc::new({
            let (available, attempts) = (available.clone(), attempts.clone());
            move |_| {
                attempts.set(attempts.get() + 1);
                available.get().then_some(RevocationWatch::Synthetic)
            }
        });
        let ctx = context(runtime.handle().clone(), cx);
        let page = cx.update(|cx| page_with_reader(ctx.clone(), false, reader.clone(), setup.clone(), cx));
        cx.run_until_parked();
        let assert_paused = |page: &LocalSessionsPage| {
            assert!(
                page.history.enabled,
                "saved opt-in remains distinct from monitor availability"
            );
            assert!(matches!(&page.history.sessions, Load::Failed(error) if error.as_ref() == WATCH_UNAVAILABLE));
            assert!(page.settings_watch.is_none());
        };
        page.read_with(cx, |page, _| assert_paused(page));
        assert_eq!(transcript_reads.load(Ordering::SeqCst), 0);
        assert_eq!(consent_reads.load(Ordering::SeqCst), 1);

        page.update(cx, |page, cx| {
            page.history.sessions = Load::Ready(vec![session()]);
            page.load(cx); // The visible Retry action uses this handler and the injected setup.
            assert!(
                matches!(&page.history.sessions, Load::Loading),
                "retry clears any cached title first"
            );
        });
        cx.run_until_parked();
        page.read_with(cx, |page, _| assert_paused(page));
        assert_eq!(attempts.get(), 2);
        assert_eq!(transcript_reads.load(Ordering::SeqCst), 0);

        // Privacy still loads consent without monitoring or transcript discovery.
        let controls = cx.update(|cx| page_with_reader(ctx, true, reader, setup, cx));
        cx.run_until_parked();
        assert!(controls.read_with(cx, |page, _| page.history.enabled
            && matches!(&page.history.sessions, Load::Ready(rows) if rows.is_empty())));
        assert_eq!(transcript_reads.load(Ordering::SeqCst), 0);

        available.set(true);
        page.update(cx, |page, cx| page.load(cx));
        cx.run_until_parked();
        assert_eq!(transcript_reads.load(Ordering::SeqCst), 1);
        assert!(page.read_with(cx, |page, _| page.settings_watch.is_some()
            && matches!(&page.history.sessions, Load::Ready(rows) if rows.len() == 1)));
    }

    #[test]
    fn a_disabled_saved_preference_discards_results_even_with_a_current_ticket() {
        let revision = Arc::new(AtomicU64::new(1));
        let ticket = ReadTicket { revision, value: 1 };
        let mut history = HistorySnapshot {
            enabled: true,
            sessions: Load::Ready(vec![session()]),
        };
        assert!(history.apply(&ticket, false, vec![session()]));
        assert!(!history.enabled);
        assert!(matches!(history.sessions, Load::Ready(rows) if rows.is_empty()));
    }

    #[test]
    fn privacy_control_never_resolves_agent_homes_even_after_opt_in() {
        for on in [None, Some(false), Some(true)] {
            let settings = nexus_desktop_core::settings::Settings {
                local_session_history: on,
                ..Default::default()
            };
            assert!(discover(&settings, true, &|_| panic!("privacy must not read transcripts")).is_empty());
        }
    }

    #[test]
    fn history_stays_unread_after_opt_in_is_revoked() {
        let mut settings = nexus_desktop_core::settings::Settings {
            local_session_history: Some(true),
            ..Default::default()
        };
        settings.local_session_history = Some(false);
        assert!(
            discover(&settings, false, &|_| panic!(
                "disabled history must not inspect the computer"
            ))
            .is_empty()
        );
    }

    fn session() -> LocalSession {
        LocalSession {
            agent: "claude".into(),
            id: "0b5c6f1e-2c1d-4f5a-9b7e-1a2b3c4d5e6f".into(),
            title: Some("Fix the login bug".into()),
            cwd: Some("/tmp/nexus".into()),
            project: Some("nexus".into()),
            git_branch: Some("main".into()),
            model: Some("claude-opus-4-5".into()),
            entrypoint: Some("cli".into()),
            started_at: Some("2026-11-05T10:00:00.000Z".into()),
            updated_at: "2026-11-05T14:03:00.123Z".into(),
            pr_url: None,
            resume: "cd /tmp/nexus && nexus claude --resume 0b5c6f1e-2c1d-4f5a-9b7e-1a2b3c4d5e6f".into(),
        }
    }

    #[test]
    fn a_row_says_who_where_and_when() {
        let mut session = session();
        assert_eq!(agent_name(&session.agent), "Claude Code");
        assert_eq!(where_ran("codex_vscode"), "in VS Code");
        assert_eq!(where_ran("something-new"), "something-new");
        assert_eq!(project_of(&session), "nexus");
        session.project = None;
        session.git_branch = None;
        assert_eq!(project_of(&session), "/tmp/nexus");
    }

    #[test]
    fn titles_keep_their_first_sentence() {
        assert_eq!(
            short_title("Task: code-edit. Issue ABC-123. Worktree: ~/work"),
            "Task: code-edit. Issue ABC-123"
        );
        assert_eq!(short_title("\nFix the login bug\nmore"), "Fix the login bug");
        assert_eq!(short_title("Hi. There"), "Hi. There");
    }

    #[test]
    fn resume_scripts_go_to_this_builds_cache_folder() {
        assert_eq!(scripts_dir(), crate::paths::cache_dir().join("resume"));
        assert!(scripts_dir().ends_with("nexus-desktop-gpui/resume"));
    }

    #[test]
    fn reads_iso_times() {
        assert_eq!(iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_ms("2026-11-05T14:03:00.123Z"), Some(1_793_887_380_000));
        assert_eq!(iso_ms("yesterday"), None);
        assert_eq!(last_active("not a time"), "not a time");
    }
}
