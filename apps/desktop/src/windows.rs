//! The app's windows, and what they share while the app runs. Each window is a
//! [`Workspace`] with its own page or session and its own Back and Forward. They share one Gateway client and sign-in
//! (`gateway.rs`), and one Code sessions runner with the store's lock (`code_run.rs`), both held here.
//!
//! New Window (Ctrl+Shift+N, the tray, File on macOS, or `--new-window` on a second launch) opens another. With the tray
//! up, closing the last window keeps the app running; the tray's Quit Nexus, or Cmd+Q, ends it. With no tray, closing
//! the last window quits. The primary window parks its private PTYs, current route, live CEF session pages and
//! side-chat state while the app stays alive, then recreates fresh views when reopened. Additional windows release
//! their resources on close. Explicit quit and identity changes end all parked owners too.
//!
//! Routing: the tray's rows, a notification's click and a later launch's `nexus://` links go to the window focused
//! last ([`Registry::front`]), opened first when none is. The panel (`panel.rs`) is a window too: it shares the
//! Gateway and the runner, but it is never the front. The runner's events reach every window; a reply to one window's
//! request goes to the window that waits for it, else to the one focused last ([`Reach`]).

use std::rc::Rc;
use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui_kit::component::TitleBar;
use gpui_kit::*;

use crate::backend::Backend;
use crate::code_run::{CodeEvent, CodeRunner};
use crate::gateway::Event;
use crate::links::Arrival;
use crate::model::SessionSummary;
use crate::owner::Refused;
use crate::tray::Command;
use crate::workspace::{RetainedPrimary, Workspace};

pub(crate) mod retention;
#[cfg(test)]
pub(crate) mod retention_fixtures;
use retention::{Identity, IdentityEvent, Primary};

// New Window and, on macOS, Help › Send Feedback… (`main.rs`).
actions!(nexus, [NewWindow, SendFeedback, CheckForUpdates]);

/// The open windows, the one focused last at the end: the routing model, apart from GPUI so tests can drive it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry<W> {
    open: Vec<W>,
}

impl<W> Default for Registry<W> {
    fn default() -> Self {
        Registry { open: Vec::new() }
    }
}

/// Who a Code sessions event goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Every window: the store changed, or what every window shows (the agents, who pays).
    Every,
    /// The workspace that requested the operation, if it is still open.
    Origin,
    /// The window waiting for it (it started the session, or the clone), else the window focused last. The others
    /// read the store again.
    Waiting,
}

/// Who `event` goes to.
pub fn reach(event: &CodeEvent) -> Reach {
    match event {
        // Every window: only the one that shared the session with Keep updated holds it.
        CodeEvent::Changed
        | CodeEvent::Agents(_)
        | CodeEvent::Access { .. }
        | CodeEvent::Streamed
        | CodeEvent::TurnEnded(_) => Reach::Every,
        CodeEvent::ProjectAdded { .. } | CodeEvent::ProjectAddFailed { .. } => Reach::Origin,
        CodeEvent::Created { .. }
        | CodeEvent::StartFailed { .. }
        | CodeEvent::Cloned(_)
        | CodeEvent::Failed(_)
        | CodeEvent::Unsent { .. } => Reach::Waiting,
    }
}

impl<W: Copy + PartialEq> Registry<W> {
    /// A window opened. It has the focus.
    pub fn opened(&mut self, window: W) {
        self.open.retain(|open| *open != window);
        self.open.push(window);
    }

    /// `window` took the focus. A window not open is ignored.
    pub fn focused(&mut self, window: W) {
        if let Some(at) = self.open.iter().position(|open| *open == window) {
            let window = self.open.remove(at);
            self.open.push(window);
        }
    }

    /// `window` closed. Answers whether it was open.
    pub fn closed(&mut self, window: W) -> bool {
        let before = self.open.len();
        self.open.retain(|open| *open != window);
        self.open.len() != before
    }

    /// The window focused last: where the tray, a notification and a link go.
    pub fn front(&self) -> Option<W> {
        self.open.last().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// The open windows, the one focused last at the end.
    #[cfg(test)]
    pub fn windows(&self) -> &[W] {
        &self.open
    }

    /// The windows an event with `reach` goes to: every window, or those `waiting` for it, else the front one.
    pub fn recipients(&self, reach: Reach, waiting: impl Fn(W) -> bool) -> Vec<W> {
        match reach {
            Reach::Every => self.open.clone(),
            Reach::Origin => self.open.iter().copied().filter(|window| waiting(*window)).collect(),
            Reach::Waiting => {
                let waiting: Vec<W> = self.open.iter().copied().filter(|window| waiting(*window)).collect();
                if waiting.is_empty() {
                    self.front().into_iter().collect()
                } else {
                    waiting
                }
            }
        }
    }
}

/// An open window.
struct Open {
    window: AnyWindowHandle,
    workspace: Entity<Workspace>,
    /// Where a later launch's links go (`links.rs`).
    arrive: UnboundedSender<Arrival>,
}

/// The app around its windows: what was one window's and is now shared.
struct Shell {
    windows: Vec<Open>,
    order: Registry<WindowId>,
    primary: Primary<WindowId, RetainedPrimary>,
    quitting: bool,
    /// Watches auth even while every window is closed.
    _identity_events: Option<Task<()>>,
    /// The Gateway a new window takes a side of. A Gateway switch in any window replaces it.
    gateway: Arc<dyn Backend>,
    /// The events of `Gateway::start`, for the first window.
    events: Option<UnboundedReceiver<Event>>,
    /// The Code sessions' runner once this build holds the store: one for every window, kept while no window is open.
    runner: Option<Rc<CodeRunner>>,
    /// Reads the runner's events and hands them to the windows.
    _code_events: Option<Task<()>>,
    /// The panel's window while it is open, and what it keeps between its windows (`panel.rs`).
    panel: Option<Open>,
    panel_state: crate::panel::PanelState,
}

impl Global for Shell {}

/// Take over the windows: open one now unless `open_now` is false, read a later launch's `arrivals`, and run the
/// tray's `commands` (`tray.rs`) when there is a tray.
pub fn start(
    gateway: Arc<dyn Backend>,
    events: UnboundedReceiver<Event>,
    arrivals: UnboundedReceiver<Arrival>,
    commands: Option<UnboundedReceiver<String>>,
    open_now: bool,
    cx: &mut App,
) {
    let recovery_file = crate::managed::local_cache(crate::managed::policy())
        .then(crate::paths::update_recovery_file)
        .flatten();
    cx.set_global(crate::update_recovery::Recovery::load(recovery_file));
    cx.set_global(Shell {
        windows: Vec::new(),
        order: Registry::default(),
        primary: Primary::default(),
        quitting: false,
        _identity_events: None,
        gateway,
        events: Some(events),
        runner: None,
        _code_events: None,
        panel: None,
        panel_state: Default::default(),
    });
    watch_identity(cx, true);
    // The agents' browser tools reach the windows from the app's socket (`side/browser/agent.rs`).
    crate::side::browser::agent::install(cx);
    // GPUI's default quits on Windows and Linux when the last window closes; with a tray, [`closed`] decides.
    install_close_handler(cx);
    cx.on_action(|_: &NewWindow, cx| {
        new_window(cx);
    });
    cx.on_action(|_: &SendFeedback, cx| run(Command::Feedback, cx));
    cx.on_action(|_: &CheckForUpdates, cx| run(Command::CheckForUpdates, cx));
    // This build's agents end their turns before the app goes, so none is left running without an owner. A server's
    // connection closes; its host and sessions keep running there (`machines.rs`).
    install_quit_handler(crate::machines::get(), cx);
    cx.spawn(async move |cx| {
        let mut arrivals = arrivals;
        while let Some(arrival) = arrivals.next().await {
            cx.update(|cx| arrived(arrival, cx));
        }
    })
    .detach();
    if let Some(commands) = commands {
        cx.spawn(async move |cx| {
            let mut commands = commands;
            while let Some(id) = commands.next().await {
                cx.update(|cx| {
                    if let Some(command) = crate::tray::command(cx, &id) {
                        run(command, cx);
                    }
                });
            }
        })
        .detach();
    }
    if !open_now {
        // No window to wait for: what the launch held runs now (`launch.rs`).
        crate::launch::first_frame_drawn(cx);
    }
    if open_now {
        show(cx);
        // For checking more windows without key presses: `NEXUS_GPUI_WINDOWS=2` opens two at start. Debug builds only.
        if cfg!(debug_assertions)
            && let Some(more) = std::env::var("NEXUS_GPUI_WINDOWS")
                .ok()
                .and_then(|count| count.parse::<usize>().ok())
        {
            for _ in 1..more.min(8) {
                new_window(cx);
            }
        }
    }
}

/// The installed app shutdown path also retires detached owners before native close callbacks.
fn install_quit_handler(machines: Arc<crate::machines::Machines>, cx: &mut App) {
    cx.on_app_quit(move |cx| {
        let shell = cx.global_mut::<Shell>();
        shell.quitting = true;
        shell.primary.saved = None;
        crate::workspace::browser_detach::quit(cx);
        crate::workspace::files_detach::quit(cx);
        crate::workspace::commands_detach::quit(cx);
        for (_, workspace) in windows(cx) {
            workspace.update(cx, |workspace, cx| workspace.discard_private_terminals(cx));
        }
        // Typing saved less than a second ago is not lost (`workspace/drafts.rs`).
        let drafts = crate::workspace::flush_composer_drafts(cx);
        if let Some(runner) = cx.global_mut::<Shell>().runner.take() {
            runner.shutdown();
        }
        machines.shutdown();
        async move {
            drafts.await;
        }
    })
    .detach();
}

/// A window's options: centred, its app_id named after the launcher entry.
fn options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(1280.), px(820.)), cx)),
        window_min_size: Some(size(px(720.), px(480.))),
        // The Wayland app_id (and the X11 WM_CLASS), named after the launcher entry
        // (`packaging/linux/install.sh`): custom installs must use their own entry, or KWin shows another copy's icon.
        app_id: Some(crate::app_id::get()),
        // macOS: the traffic lights center on the 44 head, in the slot the sidebar's head leaves for them.
        titlebar: Some(TitlebarOptions {
            traffic_light_position: Some(point(
                px(crate::tokens::TRAFFIC_LIGHT_ORIGIN.0),
                px(crate::tokens::TRAFFIC_LIGHT_ORIGIN.1),
            )),
            ..TitleBar::title_bar_options()
        }),
        ..TitleBar::window_options()
    }
}

pub(crate) fn browser_window_options(cx: &App) -> WindowOptions {
    options(cx)
}
pub(crate) fn files_window_options(cx: &App) -> WindowOptions {
    options(cx)
}
pub(crate) fn files_other_copy(
    key: &crate::workspace::files_detach::Key,
    files: EntityId,
    source: WindowId,
    cx: &App,
) -> bool {
    windows(cx)
        .into_iter()
        .filter(|(window, _)| window.window_id() != source)
        .any(|(_, workspace)| workspace.read(cx).files_copy(key, files, cx))
}
pub(crate) fn files_return_window(
    key: &crate::workspace::files_detach::Key,
    preferred: WindowId,
    cx: &mut App,
) -> Option<(AnyWindowHandle, Entity<Workspace>, bool)> {
    let shell = cx.try_global::<Shell>()?;
    let target = shell
        .windows
        .iter()
        .find(|open| open.window.window_id() == preferred && open.workspace.read(cx).files_return_target(key))
        .or_else(|| {
            shell
                .windows
                .iter()
                .find(|open| open.workspace.read(cx).files_return_target(key))
        })
        .map(|open| (open.window, open.workspace.clone()));
    if let Some((window, workspace)) = target {
        return Some((window, workspace, false));
    }
    #[cfg(test)]
    if let Some(opened) = crate::workspace::files_detach::fixture_app_window(cx) {
        return opened.map(|(window, workspace)| (window, workspace, true));
    }
    let primary = cx.global::<Shell>().primary.needs_window();
    open_app_window(primary, cx).map(|(window, workspace)| (window, workspace, true))
}
pub(crate) fn commands_window_options(cx: &App) -> WindowOptions {
    options(cx)
}
pub(crate) fn commands_other_copy(
    key: &crate::workspace::commands_detach::Key,
    commands: EntityId,
    source: WindowId,
    cx: &App,
) -> bool {
    windows(cx)
        .into_iter()
        .filter(|(window, _)| window.window_id() != source)
        .any(|(_, workspace)| workspace.read(cx).commands_copy(key, commands, cx))
}
pub(crate) fn commands_return_window(
    key: &crate::workspace::commands_detach::Key,
    preferred: WindowId,
    cx: &mut App,
) -> Option<(AnyWindowHandle, Entity<Workspace>, bool)> {
    let shell = cx.try_global::<Shell>()?;
    let target = shell
        .windows
        .iter()
        .find(|open| open.window.window_id() == preferred && open.workspace.read(cx).commands_return_target(key))
        .or_else(|| {
            shell
                .windows
                .iter()
                .find(|open| open.workspace.read(cx).commands_return_target(key))
        })
        .map(|open| (open.window, open.workspace.clone()));
    if let Some((window, workspace)) = target {
        return Some((window, workspace, false));
    }
    #[cfg(test)]
    if let Some(opened) = crate::workspace::commands_detach::fixture_app_window(cx) {
        return opened.map(|(window, workspace)| (window, workspace, true));
    }
    let primary = cx.global::<Shell>().primary.needs_window();
    open_app_window(primary, cx).map(|(window, workspace)| (window, workspace, true))
}
pub(crate) fn browser_other_copy(
    key: &crate::workspace::browser_detach::Key,
    browser: EntityId,
    source_window: WindowId,
    cx: &App,
) -> bool {
    cx.try_global::<Shell>().is_some_and(|shell| {
        shell
            .windows
            .iter()
            .filter(|open| open.window.window_id() != source_window)
            .any(|open| open.workspace.read(cx).browser_copy(key, browser, cx))
            || shell
                .primary
                .saved
                .as_ref()
                .is_some_and(|saved| saved.browser_copy(key))
    })
}
pub(crate) fn browser_output_conflict(key: &crate::workspace::browser_detach::Key, cx: &App) -> bool {
    cx.try_global::<Shell>().is_some_and(|shell| {
        shell
            .windows
            .iter()
            .any(|open| open.workspace.read(cx).browser_output_conflict(key))
            || shell
                .primary
                .saved
                .as_ref()
                .is_some_and(|saved| saved.browser_output_conflict(key))
    })
}
pub(crate) fn browser_return_window(
    key: &crate::workspace::browser_detach::Key,
    preferred: WindowId,
    cx: &mut App,
) -> Option<(AnyWindowHandle, Entity<Workspace>, bool)> {
    let shell = cx.try_global::<Shell>()?;
    let existing = shell
        .windows
        .iter()
        .find(|open| open.window.window_id() == preferred && open.workspace.read(cx).browser_return_target(key))
        .or_else(|| {
            shell
                .windows
                .iter()
                .find(|open| open.workspace.read(cx).browser_return_target(key))
        })
        .map(|open| (open.window, open.workspace.clone()));
    if let Some((window, workspace)) = existing {
        return Some((window, workspace, false));
    }
    #[cfg(test)]
    if let Some(opened) = crate::workspace::browser_detach::fixture_app_window(cx) {
        return opened.map(|(window, workspace)| (window, workspace, true));
    }
    let primary = cx.global::<Shell>().primary.needs_window();
    open_app_window(primary, cx).map(|(window, workspace)| (window, workspace, true))
}
fn maybe_quit(cx: &mut App) {
    let shell = cx.global::<Shell>();
    if !shell.quitting
        && shell.order.is_empty()
        && shell.panel.is_none()
        && !crate::workspace::browser_detach::keeps_app_alive(cx)
        && !crate::workspace::files_detach::keeps_app_alive(cx)
        && !crate::workspace::commands_detach::keeps_app_alive(cx)
        && !crate::tray::is_up(cx)
    {
        cx.quit();
    }
}

/// The window and browser that take an agent's call for the session `thread`: the window that holds its browser, else
/// the window focused last, which makes one in the background.
pub(crate) type AdmittedBrowser = (
    AnyWindowHandle,
    Entity<crate::side::browser::Browser>,
    crate::side::browser::agent::Guard,
);
pub fn agent_browser(
    origin: &crate::side::browser::agent::LocalOrigin,
    thread: &str,
    valid: &Arc<dyn Fn() -> bool + Send + Sync>,
    cx: &mut App,
) -> Result<AdmittedBrowser, String> {
    // Home is a window-owned UI browser, never an agent session. In particular a
    // retired native-window key must never fall back to another window.
    if thread.starts_with("home:") {
        return Err("Home browser tools are available only through its window controls.".into());
    }
    if !valid() {
        return Err("This browser tool credential is invalid or expired.".into());
    }
    crate::workspace::browser_detach::revoke_stale(cx);
    if let Some(admitted) = crate::workspace::browser_detach::admit(origin, thread, valid, cx) {
        return admitted;
    }
    let shell = cx.global::<Shell>();
    let existing = shell
        .windows
        .iter()
        .filter(|open| open.workspace.read(cx).owns_browser_origin(origin, thread))
        .find(|open| open.workspace.read(cx).has_browser(thread));
    // A parked owner blocks duplicate creation, but never hides an already-live exact-source browser.
    if existing.is_none()
        && (shell.primary.saved.as_ref().is_some_and(|saved| {
            saved.identity.matches_gateway(shell.gateway.as_ref()) && saved.has_browser_origin(thread, origin)
        }) || shell
            .windows
            .iter()
            .any(|open| open.workspace.read(cx).has_parked_browser_origin(thread, origin)))
    {
        return Err("This session browser is parked. Reopen the primary Nexus window to use its browser tools.".into());
    }
    let holder = existing
        .or_else(|| {
            let front = shell.order.front()?;
            shell.windows.iter().find(|open| {
                open.window.window_id() == front && open.workspace.read(cx).owns_browser_origin(origin, thread)
            })
        })
        .or_else(|| {
            shell
                .windows
                .iter()
                .find(|open| open.workspace.read(cx).owns_browser_origin(origin, thread))
        })
        .map(|open| (open.window, open.workspace.clone()))
        .ok_or_else(|| unadmitted(origin, thread, cx))?;
    let (window, workspace) = holder;
    let proof = workspace
        .read(cx)
        .browser_authority(origin, thread)
        .ok_or("This browser source authority expired.")?;
    let producer = valid.clone();
    let valid: crate::side::browser::agent::Guard = Arc::new(move || proof.current() && producer());
    let browser = window
        .update(cx, |_, window, cx| {
            if !valid() {
                return Err("This browser tool credential is invalid or expired.".to_string());
            }
            workspace.update(cx, |workspace, cx| {
                if !workspace.owns_browser_origin(origin, thread) {
                    return Err("This session's browser source is no longer available.".into());
                }
                workspace.agent_browser(thread, window, cx)
            })
        })
        .map_err(|_| "The app window is closed.".to_string())??;
    Ok((window, browser, valid))
}

/// Why no window takes a browser call: the front window's reason, else the first window's, else that none is open.
fn unadmitted(origin: &crate::side::browser::agent::LocalOrigin, thread: &str, cx: &App) -> String {
    let shell = cx.global::<Shell>();
    let front = shell
        .order
        .front()
        .and_then(|front| shell.windows.iter().find(|open| open.window.window_id() == front));
    front
        .or_else(|| shell.windows.first())
        .and_then(|open| open.workspace.read(cx).browser_refusal(origin, thread))
        .unwrap_or("Open a Nexus window to use the browser.")
        .to_string()
}

/// Open another window, on a new page, and bring it forward.
pub fn new_window(cx: &mut App) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    open_app_window(false, cx)
}

fn open_app_window(primary: bool, cx: &mut App) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    let shell = cx.global_mut::<Shell>();
    let (gateway, events) = match shell.events.take() {
        Some(events) => (shell.gateway.clone(), events),
        None => shell.gateway.window(),
    };
    let (arrive, arrivals) = unbounded();
    // Frost: a main window lets the desktop show through, blurred, at the chosen transparency (`frost.rs`).
    let options = crate::frost::main_window(options(cx));
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Workspace::new(gateway, events, arrivals, false, window, cx))
    });
    let (window, workspace) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            eprintln!("nexus-desktop-gpui: a window did not open: {error}");
            return None;
        }
    };
    crate::frost::clear_root(window, cx);
    let shell = cx.global_mut::<Shell>();
    shell.order.opened(window.window_id());
    if primary {
        shell.primary.opened(window.window_id());
    }
    shell.windows.push(Open {
        window,
        workspace: workspace.clone(),
        arrive,
    });
    // An open panel watches the new window for Move here now, not the next time it opens. Deferred: the panel may be
    // the one asking for the window, in the middle of its own update.
    cx.defer(|cx| {
        if let Some((_, panel)) = panel(cx) {
            panel.update(cx, |panel, cx| panel.watch_main_chat(cx));
        }
    });
    measure_first_frame(window, cx);
    // Initialize auxiliary UI once the window's first frame has reached the screen (`launch.rs`).
    let _ = window.update(cx, |_, window, _| {
        window.on_next_frame(|window, _| window.on_next_frame(|_, cx| crate::launch::first_frame_drawn(cx)));
    });
    cx.activate(true);
    Some((window, workspace))
}

/// `npm run measure`'s window and first-frame marks (`measure.rs`). `open_window` lays the window out once; GPUI's
/// next frame presents it, and the frame after that runs once it is on screen.
fn measure_first_frame(window: AnyWindowHandle, cx: &mut App) {
    if !crate::measure::on() {
        return;
    }
    crate::measure::mark_once(nexus_desktop_core::metrics::WINDOW);
    let _ = window.update(cx, |_, window, _| {
        window.on_next_frame(|window, _| {
            window.on_next_frame(|_, _| crate::measure::mark_once(nexus_desktop_core::metrics::FIRST_FRAME));
        });
    });
}

/// The window focused last, with its workspace.
fn front(cx: &App) -> Option<&Open> {
    let shell = cx.global::<Shell>();
    let id = shell.order.front()?;
    shell.windows.iter().find(|open| open.window.window_id() == id)
}

/// Bring the window focused last forward, opening one when none is open. Answers it and its workspace.
pub fn show(cx: &mut App) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    if !cx.global::<Shell>().primary.has_window() {
        return open_app_window(true, cx);
    }
    if let Some(open) = front(cx) {
        let (window, workspace) = (open.window, open.workspace.clone());
        if window.update(cx, |_, window, _| window.activate_window()).is_ok() {
            cx.activate(true);
            return Some((window, workspace));
        }
    }
    open_app_window(true, cx)
}

/// Run `f` on the workspace of the window focused last, opening one first when none is open.
pub fn with_workspace(cx: &mut App, f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>)) {
    let Some((window, workspace)) = show(cx) else {
        return;
    };
    let _ = window.update(cx, |_, window, cx| {
        workspace.update(cx, |workspace, cx| f(workspace, window, cx))
    });
}

/// The chat the app window focused last shows, for the panel's Move here; none on any other page.
pub fn app_chat(cx: &App) -> Option<SessionSummary> {
    front(cx)?.workspace.read(cx).shown_chat()
}

/// The app windows' workspaces: the panel watches them for Move here.
pub fn app_workspaces(cx: &App) -> Vec<Entity<Workspace>> {
    cx.global::<Shell>()
        .windows
        .iter()
        .map(|open| open.workspace.clone())
        .collect()
}

/// Whether the windows are up: the panel needs them.
pub fn running(cx: &App) -> bool {
    cx.has_global::<Shell>()
}

/// The shared Gateway: the Mac chat bar asks the agent API through it (`chat_bar.rs`).
pub fn gateway(cx: &App) -> Arc<dyn Backend> {
    cx.global::<Shell>().gateway.clone()
}

/// The conversation of the Assistant's thread in the app window focused last, when it has one.
pub fn assistant_conversation(cx: &App) -> Option<String> {
    front(cx)?.workspace.read(cx).assistant_conversation(cx)
}

/// The panel's window and workspace, while it is open.
pub fn panel(cx: &App) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    let open = cx.global::<Shell>().panel.as_ref()?;
    Some((open.window, open.workspace.clone()))
}

/// What the panel keeps between its windows.
pub fn panel_state(cx: &mut App) -> &mut crate::panel::PanelState {
    &mut cx.global_mut::<Shell>().panel_state
}

/// Open the panel's window with `options`, on a side of the shared Gateway. It stays out of the routing order.
pub fn open_panel(options: WindowOptions, cx: &mut App) -> Option<(AnyWindowHandle, Entity<Workspace>)> {
    let shell = cx.global_mut::<Shell>();
    let (gateway, events) = match shell.events.take() {
        Some(events) => (shell.gateway.clone(), events),
        None => shell.gateway.window(),
    };
    let (arrive, arrivals) = unbounded();
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Workspace::new(gateway, events, arrivals, true, window, cx))
    });
    let (window, workspace) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            eprintln!("nexus-desktop-gpui: the panel did not open: {error}");
            return None;
        }
    };
    cx.global_mut::<Shell>().panel = Some(Open {
        window,
        workspace: workspace.clone(),
        arrive,
    });
    Some((window, workspace))
}

/// A tray row, a click on the tray item, or what the panel asks of the app window.
pub fn run(command: Command, cx: &mut App) {
    match command {
        Command::Quit => cx.quit(),
        Command::Open => {
            show(cx);
        }
        Command::NewWindow => {
            new_window(cx);
        }
        Command::Panel => crate::panel::show(cx),
        // While recording, the item (showing the time) is the stop button.
        Command::TogglePanel if crate::record::is_recording(cx) => crate::record::toggle(cx),
        Command::TogglePanel => crate::panel::toggle(cx),
        // The result shows in the app window's Settings, at Desktop updates (`workspace/shell.rs`).
        Command::CheckForUpdates => {
            crate::updates::check();
            on_tray(Command::CheckForUpdates, cx);
        }
        // A refusal (Code sessions still running) shows at the same place, which says why.
        Command::RestartToUpdate => {
            if restart_for_update(cx).is_err() {
                on_tray(Command::RestartToUpdate, cx);
            }
        }
        Command::Record => crate::record::toggle(cx),
        Command::RecordCancel => crate::record::cancel(cx),
        // New Chat, Settings…, a recent chat and Send Feedback… act on the app window, never on the panel: only Ask
        // Nexus and a click on the item open the panel.
        command => on_tray(command, cx),
    }
}

/// A tray row the app window focused last does, opening one first when none is open.
fn on_tray(command: Command, cx: &mut App) {
    with_workspace(cx, move |workspace, window, cx| workspace.on_tray(command, window, cx));
}

/// The tray asks through the same confirmation as Settings and the update toast.
pub fn restart_for_update(cx: &mut App) -> Result<(), String> {
    let mut result = Err("Open a Nexus window before restarting for an update.".into());
    with_workspace(cx, |workspace, _, cx| result = workspace.restart_for_update(cx));
    result
}

pub fn update_restart_sessions(cx: &App) -> Result<Vec<nexus_workbench::update_restart::Session>, String> {
    cx.try_global::<Shell>()
        .and_then(|shell| shell.runner.as_ref())
        .map_or_else(|| Ok(Vec::new()), |runner| runner.update_restart_sessions())
}

/// Notify all windows after a shared recovery change, once the caller's workspace update has ended.
pub fn recovery_changed(cx: &mut App) {
    cx.defer(|cx| {
        if !cx.has_global::<Shell>() {
            return;
        }
        for (_, workspace) in windows(cx) {
            workspace.update(cx, |_, cx| cx.notify());
        }
    });
}

pub fn install_update(confirmed: &[nexus_workbench::update_restart::Session], cx: &mut App) -> Result<(), String> {
    let runner = cx.try_global::<Shell>().and_then(|shell| shell.runner.clone());
    let sessions = match &runner {
        Some(runner) => runner.reserve_confirmed_update_restart(confirmed).map_err(|message| {
            crate::updates::refused(message.clone());
            message
        })?,
        None => Vec::new(),
    };
    let staged = match crate::updates::take_ready() {
        Ok(staged) => staged,
        Err(message) => {
            if let Some(runner) = &runner {
                runner.cancel_update_restart();
            }
            return Err(message);
        }
    };
    let before = match cx.global_mut::<crate::update_recovery::Recovery>().prepare(&sessions) {
        Ok(before) => before,
        Err(message) => {
            if let Some(runner) = &runner {
                runner.cancel_update_restart();
            }
            crate::updates::install_failed(staged, message.clone());
            return Err(message);
        }
    };
    let task = cx.background_executor().spawn(async move {
        let installed = crate::updates::install(&staged);
        (staged, installed)
    });
    cx.spawn(async move |cx| {
        let (staged, installed) = task.await;
        cx.update(|cx| match installed {
            Ok(relaunch) => {
                // The agents stop before the installer or the new copy starts: the Windows installer replaces this
                // copy's files.
                if let Some(runner) = &runner {
                    runner.shutdown();
                }
                match relaunch.start() {
                    Ok(()) => cx.quit(),
                    Err(error) => {
                        if let Some(runner) = &runner {
                            runner.cancel_update_restart();
                        }
                        let recovery = cx.global_mut::<crate::update_recovery::Recovery>();
                        recovery.installing = false;
                        recovery.hidden = false;
                        crate::updates::install_failed(staged, format!("Couldn't start the new version: {error}."));
                        recovery_changed(cx);
                    }
                }
            }
            Err(message) => {
                if let Some(runner) = &runner {
                    runner.cancel_update_restart();
                }
                cx.global_mut::<crate::update_recovery::Recovery>().cancel(before);
                crate::updates::install_failed(staged, message);
            }
        });
    })
    .detach();
    Ok(())
}

/// A later launch: a new window for `--new-window`; otherwise the window focused last comes forward, opened if none
/// is, and takes its links.
fn arrived(arrival: Arrival, cx: &mut App) {
    if arrival == Arrival::NewWindow {
        new_window(cx);
        return;
    }
    if show(cx).is_none() {
        return;
    }
    if let Some(open) = front(cx) {
        let _ = open.arrive.unbounded_send(arrival);
    }
}

/// A window took the focus (`Workspace`'s activation observer): the tray, notifications and links go to it now.
pub fn focused(cx: &mut App, id: WindowId) {
    if cx.has_global::<Shell>() {
        cx.global_mut::<Shell>().order.focused(id);
    }
}

fn install_close_handler(cx: &mut App) {
    cx.set_quit_mode(QuitMode::Explicit);
    cx.on_window_closed(closed).detach();
}

/// A window closed: forget it, and quit when it was the last and there is no tray. The panel keeps the chat it showed
/// for its next window.
fn closed(cx: &mut App, id: WindowId) {
    crate::rich_text::window_closed(cx, id);
    crate::notify::closed(id.as_u64());
    if crate::workspace::browser_detach::closed(id, cx)
        || crate::workspace::files_detach::closed(id, cx)
        || crate::workspace::commands_detach::closed(id, cx)
    {
        maybe_quit(cx);
        return;
    }
    let detached_alive = crate::workspace::browser_detach::keeps_app_alive(cx)
        || crate::workspace::files_detach::keeps_app_alive(cx)
        || crate::workspace::commands_detach::keeps_app_alive(cx);
    let shell = cx.global_mut::<Shell>();
    if let Some(panel) = shell.panel.take_if(|panel| panel.window.window_id() == id) {
        let chat = panel.workspace.read(cx).panel_chat();
        panel
            .workspace
            .update(cx, |workspace, _| workspace.retire_gateway_chat_view());
        panel
            .workspace
            .update(cx, |workspace, cx| workspace.retire_files_view(cx));
        panel
            .workspace
            .update(cx, |workspace, cx| workspace.retire_commands_view(cx));
        crate::workspace::files_detach::source_closed(id, cx);
        crate::workspace::commands_detach::source_closed(id, cx);
        cx.global_mut::<Shell>()
            .panel_state
            .closed(chat, std::time::Instant::now());
    } else {
        if !shell.order.closed(id) {
            return;
        }
        let Some(at) = shell.windows.iter().position(|open| open.window.window_id() == id) else {
            return;
        };
        let open = shell.windows.remove(at);
        let primary = shell.primary.is_primary(id);
        let (quitting, other_window, panel) = (
            shell.quitting,
            !shell.order.is_empty() || detached_alive,
            shell.panel.is_some(),
        );
        let keep = retention::keep_on_close(quitting, other_window, panel, crate::tray::is_up(cx));
        let saved = if primary && keep {
            open.workspace
                .update(cx, |workspace, cx| workspace.retain_primary_terminals(cx))
        } else {
            None
        };
        cx.global_mut::<Shell>().primary.closed(id, keep, saved);
        open.workspace
            .update(cx, |workspace, _| workspace.retire_gateway_chat_view());
        open.workspace
            .update(cx, |workspace, cx| workspace.retire_files_view(cx));
        open.workspace
            .update(cx, |workspace, cx| workspace.retire_commands_view(cx));
        crate::workspace::browser_detach::source_closed(id, cx);
        crate::workspace::files_detach::source_closed(id, cx);
        crate::workspace::commands_detach::source_closed(id, cx);
        // No strong Workspace reference is retained; only the move-only private resources survive.
        drop(open);
    }
    maybe_quit(cx);
}

/// The Code sessions' runner on `scope` (`<gateway>|<org>`): the one this build holds, or a new one when the store's
/// lock is free. A new one is handed to every open window, and its events reach them all ([`dispatch`]). Refused while
/// another app holds the lock.
pub fn runner(
    cx: &mut App,
    gateway: &str,
    scope: &str,
    client: crate::backend::SessionClient,
) -> Result<Rc<CodeRunner>, Refused> {
    let local_providers_opt_in = crate::local_providers_opt_in::get(cx).clone();
    if !cx.has_global::<Shell>() {
        return CodeRunner::start(gateway, scope, client, local_providers_opt_in).map(|(runner, _)| Rc::new(runner));
    }
    if let Some(runner) = &cx.global::<Shell>().runner {
        runner.set_gateway(gateway, client);
        runner.set_scope(scope);
        return Ok(runner.clone());
    }
    let (runner, events) = CodeRunner::start(gateway, scope, client, local_providers_opt_in)?;
    let runner = Rc::new(runner);
    crate::keep_awake::runner(runner.awake_engine());
    let reader = cx.spawn(async move |cx| {
        let mut events = events;
        while let Some(first) = events.next().await {
            // A busy agent writes many events at once: each window reads the store once for all that came in.
            let mut batch = vec![first];
            while let Ok(more) = events.try_recv() {
                batch.push(more);
            }
            cx.update(|cx| dispatch(batch, cx));
        }
    });
    let shell = cx.global_mut::<Shell>();
    shell.runner = Some(runner.clone());
    shell._code_events = Some(reader);
    // The asking window is being updated now: the others take the runner once it is done.
    let shared = runner.clone();
    cx.defer(move |cx| {
        for (window, workspace) in windows(cx) {
            let runner = shared.clone();
            let _ = window.update(cx, |_, window, cx| {
                workspace.update(cx, |workspace, cx| workspace.adopt_runner(runner, window, cx))
            });
        }
    });
    Ok(runner)
}

/// Every open window with its workspace, the panel's included.
pub(crate) fn windows(cx: &App) -> Vec<(AnyWindowHandle, Entity<Workspace>)> {
    let shell = cx.global::<Shell>();
    shell
        .windows
        .iter()
        .chain(shell.panel.as_ref())
        .map(|open| (open.window, open.workspace.clone()))
        .collect()
}

/// Hand the runner's `batch` to the windows: each event to those [`reach`] names, [`CodeEvent::Changed`] to the rest.
fn dispatch(batch: Vec<CodeEvent>, cx: &mut App) {
    crate::workspace::files_detach::revoke_stale(cx);
    crate::workspace::commands_detach::revoke_stale(cx);
    let shell = cx.global::<Shell>();
    let order = shell.order.clone();
    let open: Vec<(WindowId, AnyWindowHandle, Entity<Workspace>)> = shell
        .windows
        .iter()
        .map(|open| (open.window.window_id(), open.window, open.workspace.clone()))
        .collect();
    let mut each: Vec<Vec<CodeEvent>> = vec![Vec::new(); open.len()];
    for event in batch {
        let to = order.recipients(reach(&event), |id| {
            open.iter()
                .find(|(each, _, _)| *each == id)
                .is_some_and(|(_, _, workspace)| match &event {
                    CodeEvent::ProjectAdded { origin, .. } | CodeEvent::ProjectAddFailed { origin, .. } => {
                        *origin == Some(workspace.entity_id())
                    }
                    _ => workspace.read(cx).waits_for(&event),
                })
        });
        for ((id, _, _), events) in open.iter().zip(each.iter_mut()) {
            events.push(if to.contains(id) {
                event.clone()
            } else {
                CodeEvent::Changed
            });
        }
    }
    for ((_, window, workspace), events) in open.into_iter().zip(each) {
        let _ = window.update(cx, |_, window, cx| {
            workspace.update(cx, |workspace, cx| workspace.on_code_events(events, window, cx))
        });
    }
}

/// Eager revocation also runs while every primary Workspace is closed.
pub(crate) fn prune_parked_browsers(cx: &mut App) {
    if !cx.has_global::<Shell>() {
        return;
    }
    if let Some(saved) = cx.global_mut::<Shell>().primary.saved.as_mut() {
        saved.discard_stale_browsers();
    }
}

/// Adopt only into a fresh primary whose account has been read successfully.
pub(crate) fn take_primary_terminals(id: WindowId, identity: &Identity, cx: &mut App) -> Option<RetainedPrimary> {
    if !cx.has_global::<Shell>() {
        return None;
    }
    prune_parked_browsers(cx);
    let shell = cx.global_mut::<Shell>();
    if shell.quitting || !identity.matches_gateway(shell.gateway.as_ref()) {
        shell.primary.saved = None;
        return None;
    }
    shell.primary.take(id, |saved| &saved.identity == identity)
}

/// A dedicated backend side keeps auth invalidation alive after the final Workspace is gone.
fn watch_identity(cx: &mut App, restore: bool) {
    let (gateway, events) = cx.global::<Shell>().gateway.window();
    if restore {
        gateway.restore();
    }
    let reader = identity_reader(gateway, events, cx);
    cx.global_mut::<Shell>()._identity_events = Some(reader);
}
/// The app-owned event pump is shared with an instance-local channel fixture.
fn identity_reader(gateway: Arc<dyn Backend>, mut events: UnboundedReceiver<Event>, cx: &mut App) -> Task<()> {
    let mut signed_in = gateway.marker().is_some();
    let local_providers_opt_in = crate::local_providers_opt_in::get(cx).clone();
    let mut access = crate::workspace::gateway_access::GatewayAccess::new(gateway.clone());
    cx.spawn(async move |cx| {
        while let Some(event) = events.next().await {
            match retention::identity_event(event) {
                IdentityEvent::SignedIn => {
                    access.accept_sign_in();
                    let Some(account) = access.account() else { continue };
                    signed_in = true;
                    cx.update(prune_parked_browsers);
                    cx.update(crate::workspace::browser_detach::revoke_stale);
                    cx.update(crate::workspace::files_detach::revoke_stale);
                    cx.update(crate::workspace::commands_detach::revoke_stale);
                    cx.update(|cx| {
                        prune_parked_browsers(cx);
                        let shell = cx.global_mut::<Shell>();
                        if local_providers_opt_in.enabled()
                            && let Some(runner) = &shell.runner
                        {
                            runner.refresh_local_service_account();
                        }
                        if shell
                            .primary
                            .saved
                            .as_ref()
                            .is_some_and(|saved| !saved.identity.matches_gateway(gateway.as_ref()))
                        {
                            shell.primary.saved = None;
                        }
                    });
                    account.load_account();
                }
                IdentityEvent::SignedOut => {
                    let was_signed_in = std::mem::take(&mut signed_in);
                    if gateway.marker().is_none() {
                        crate::workspace::gateway_access::AccountGateway::clear_workspace_services();
                        let machines = crate::machines::get();
                        machines.set_account_client(None);
                        if local_providers_opt_in.enabled() {
                            machines.sync(&Default::default());
                        }
                        crate::updates::session(gateway.url(), None);
                    }
                    cx.update(|cx| {
                        crate::workspace::browser_detach::identity_changed(None, cx);
                        crate::workspace::files_detach::identity_changed(None, cx);
                        crate::workspace::commands_detach::identity_changed(None, cx);
                        let shell = cx.global_mut::<Shell>();
                        shell.primary.saved = None;
                        if local_providers_opt_in.enabled()
                            && let Some(runner) = &shell.runner
                        {
                            if was_signed_in {
                                runner.retire_service_after_sign_out();
                                runner.refresh_local_service_account();
                            } else if gateway.marker().is_none() {
                                runner.signed_out();
                            }
                        }
                    });
                }
                IdentityEvent::Account(account) => {
                    let identity = Identity::current(gateway.as_ref(), &account);
                    cx.update(prune_parked_browsers);
                    cx.update(|cx| crate::workspace::browser_detach::identity_changed(identity.as_ref(), cx));
                    cx.update(|cx| crate::workspace::files_detach::identity_changed(identity.as_ref(), cx));
                    cx.update(|cx| crate::workspace::commands_detach::identity_changed(identity.as_ref(), cx));
                    cx.update(|cx| {
                        prune_parked_browsers(cx);
                        let shell = cx.global_mut::<Shell>();
                        if shell
                            .primary
                            .saved
                            .as_ref()
                            .is_some_and(|saved| Some(&saved.identity) != identity.as_ref())
                        {
                            shell.primary.saved = None;
                        }
                    });
                }
                IdentityEvent::ReloadAccount => {
                    if let Some(account) = access.account() {
                        account.load_account();
                    }
                }
                IdentityEvent::Ignore => {}
            }
        }
    })
}

/// Settings' Gateway switch in window `from`, now on `gateway`: new windows take sides of it, and every other open
/// window moves to it and signs in there again.
pub fn gateway_switched(cx: &mut App, from: WindowId, gateway: Arc<dyn Backend>, restore: bool) {
    crate::workspace::browser_detach::identity_changed(None, cx);
    crate::workspace::files_detach::identity_changed(None, cx);
    crate::workspace::commands_detach::identity_changed(None, cx);
    if !cx.has_global::<Shell>() {
        return;
    }
    {
        let shell = cx.global_mut::<Shell>();
        shell.primary.saved = None;
        shell.gateway = gateway.clone();
    }
    watch_identity(cx, restore);
    // `from` is being updated now: the others follow once it is done.
    cx.defer(move |cx| {
        for (window, workspace) in windows(cx) {
            if window.window_id() == from {
                continue;
            }
            let (side, events) = gateway.window();
            let _ = window.update(cx, |_, window, cx| {
                workspace.update(cx, |workspace, cx| {
                    workspace.follow_gateway(side, events, window, cx, restore)
                })
            });
        }
    });
}

#[cfg(test)]
mod tests {
    // `gpui_kit::*` brings GPUI's own `test` attribute; these are plain tests.
    use core::prelude::v1::test;

    use super::*;

    fn windows(registry: &Registry<u32>) -> Vec<u32> {
        registry.windows().to_vec()
    }

    #[test]
    fn the_last_focused_window_is_the_front() {
        let mut registry = Registry::default();
        assert_eq!(registry.front(), None);
        registry.opened(1);
        registry.opened(2);
        // A new window has the focus.
        assert_eq!(registry.front(), Some(2));
        registry.focused(1);
        assert_eq!(registry.front(), Some(1));
        assert_eq!(windows(&registry), [2, 1]);
        // A window not open does not take the front.
        registry.focused(9);
        assert_eq!(registry.front(), Some(1));
    }

    #[test]
    fn closing_a_window_keeps_the_others_and_the_front_moves_back() {
        let mut registry = Registry::default();
        registry.opened(1);
        registry.opened(2);
        registry.opened(3);
        registry.focused(2);
        assert!(registry.closed(2));
        assert_eq!(registry.front(), Some(3));
        assert_eq!(windows(&registry), [1, 3]);
        // A window closed twice, or never opened, changes nothing.
        assert!(!registry.closed(2));
        assert!(!registry.closed(7));
        assert!(registry.closed(1));
        assert!(registry.closed(3));
        assert!(registry.is_empty());
        assert_eq!(registry.front(), None);
    }

    #[test]
    fn a_notification_and_a_link_go_to_the_last_focused_window() {
        let mut registry = Registry::default();
        registry.opened(1);
        registry.opened(2);
        registry.focused(1);
        // `show` and `arrived` route to the front; with every window closed they open one.
        assert_eq!(registry.front(), Some(1));
        registry.closed(1);
        assert_eq!(registry.front(), Some(2));
    }

    #[test]
    fn the_runners_events_reach_every_window_and_replies_the_waiting_one() {
        let mut registry = Registry::default();
        registry.opened(1);
        registry.opened(2);
        registry.opened(3);
        registry.focused(1);
        assert_eq!(reach(&CodeEvent::Changed), Reach::Every);
        assert_eq!(reach(&CodeEvent::Agents(Vec::new())), Reach::Every);
        // A turn's end goes to every window: only the one that shared the session with Keep updated acts on it.
        assert_eq!(reach(&CodeEvent::TurnEnded("t1".into())), Reach::Every);
        assert_eq!(registry.recipients(Reach::Every, |_| false), [2, 3, 1]);
        // Window 3 started a session: its reply goes there, even though window 1 has the focus now.
        let created = CodeEvent::Created {
            draft: "draft-3".into(),
            id: "t-1".into(),
        };
        assert_eq!(reach(&created), Reach::Waiting);
        assert_eq!(registry.recipients(Reach::Waiting, |window| window == 3), [3]);
        // A folder picker result goes to its original window and is never handed to a different window.
        assert_eq!(
            reach(&CodeEvent::ProjectAdded {
                id: "p".into(),
                scope: "scope".into(),
                generation: 0,
                origin: None,
            }),
            Reach::Origin
        );
        assert_eq!(registry.recipients(Reach::Origin, |window| window == 3), [3]);
        assert!(registry.recipients(Reach::Origin, |_| false).is_empty());
        assert_eq!(registry.recipients(Reach::Waiting, |_| false), [1]);
        assert!(
            Registry::<u32>::default()
                .recipients(Reach::Waiting, |_| false)
                .is_empty()
        );
    }
}
