//! The Code session's Terminal tab: the person's own shell in a pseudo-terminal, in the session's folder.
//! Alacritty's terminal core reads the output into a grid, which this view paints cell by cell. The shell ends when
//! its final owner goes. A primary-window close can move that owner to a window-free snapshot. A drag selects
//! (a double click a word, a triple click a line); Ctrl+Shift+C (Cmd+C on macOS) or the right-click menu copies.
//! A server session's shell runs on its server instead ([`server_shell`]), on the same screen.

mod run_once;
mod scrollbar;
mod server_shell;

pub use run_once::TerminalEvent;
pub use server_shell::{MachineShells, ServerTarget, ShellHost};

use crate::workspace::FloatingGround;
use crate::workspace::menu_item_hover;
use std::cell::Cell;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode, viewport_to_point};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::palette::Palette;
use crate::shortcut_label;

const FONT_SIZE: f32 = 12.5;
/// One line of the grid. A Commands well is sized in these (`side::well`).
pub(crate) const LINE_HEIGHT: f32 = 18.;
/// The grid's inset from the terminal's top and bottom edges.
pub(crate) const PADDING: f32 = 8.;
/// The text's inset from the pane's sides: the Workbench's 12px (`side::INSET`).
const PADDING_X: f32 = super::INSET;

/// Writes from the terminal core back to the shell: answers to its questions (the cursor's place, the colors).
#[derive(Clone)]
struct Listener(Arc<Mutex<Sender<Vec<u8>>>>);

impl Listener {
    fn new(sender: Sender<Vec<u8>>) -> Self {
        Self(Arc::new(Mutex::new(sender)))
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            if let Ok(sender) = self.0.lock() {
                let _ = sender.send(text.into_bytes());
            }
        }
    }
}

/// The running shell: what goes to it, its pseudo-terminal, and a way to end it.
struct Shell {
    input: Sender<Vec<u8>>,
    pty: Pty,
    wake: OutputWake,
}

/// Where the shell's pseudo-terminal is.
enum Pty {
    /// On this computer.
    Here {
        master: Box<dyn MasterPty + Send>,
        killer: Box<dyn ChildKiller + Send + Sync>,
    },
    /// On a server: dropping it closes the shell there.
    Server(server_shell::ServerShell),
}

impl Shell {
    fn resize(&self, cols: u16, rows: u16) {
        match &self.pty {
            Pty::Here { master, .. } => {
                let _ = master.resize(pty_size(cols, rows));
            }
            Pty::Server(server) => server.resize(cols, rows),
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        // Stop output notifications before killing the child or cancelling its view's local task.
        // The reader may finish later, after the foreground executor itself has been torn down.
        self.wake.bind(None);
        if let Pty::Here { killer, .. } = &mut self.pty {
            let _ = killer.kill();
        }
    }
}

/// The output thread has no view ownership. Missing or stale subscribers must never stop PTY reads.
#[derive(Clone, Default)]
struct OutputWake(Arc<Mutex<Option<UnboundedSender<()>>>>);

impl OutputWake {
    fn bind(&self, sender: Option<UnboundedSender<()>>) {
        if let Ok(mut current) = self.0.lock() {
            *current = sender;
        }
    }

    fn notify(&self) {
        if let Ok(mut current) = self.0.lock()
            && current
                .as_ref()
                .is_some_and(|sender| sender.unbounded_send(()).is_err())
        {
            *current = None;
        }
    }
}

/// A move-only private PTY owner, with no entities, focus handles, tasks or window subscriptions.
/// Dropping it ends the shell exactly as closing its terminal does.
pub struct RetainedTerminal {
    dir: PathBuf,
    server: Option<ServerTarget>,
    term: Arc<Mutex<Term<Listener>>>,
    shell: Option<Shell>,
    ended: Arc<AtomicBool>,
    error: Option<SharedString>,
    status: Status,
    size: (u16, u16),
    wake: OutputWake,
}

/// What a server shell's reader says beside the screen: the server can't be reached, or the command's exit code.
type Status = Arc<Mutex<Option<String>>>;

/// Attached PTYs belong to the session runner. Dropping their view never ends the command.
struct Attached {
    send: Rc<dyn Fn(&str, serde_json::Value)>,
    replies: Receiver<Vec<u8>>,
    processor: Processor,
    writable: bool,
    listener: Listener,
}

/// Display state only. The session runner remains the sole PTY owner. No old view,
/// input callback, reply receiver, focus or window subscription crosses this boundary.
pub(crate) struct RetainedSharedTerminal {
    term: Arc<Mutex<Term<Listener>>>,
    processor: Processor,
    listener: Listener,
    size: (u16, u16),
    writable: bool,
}

pub struct Terminal {
    retired: bool,
    dir: PathBuf,
    /// The server the shell runs on; None: this computer, in `dir`.
    server: Option<ServerTarget>,
    term: Arc<Mutex<Term<Listener>>>,
    shell: Option<Shell>,
    attached: Option<Attached>,
    /// A private shell drawn in a Commands row's output well (`terminals/commands.rs`), on the well's fill.
    well: bool,
    /// Set by the reader once the shell's output closes.
    ended: Arc<AtomicBool>,
    error: Option<SharedString>,
    /// A server shell's reader's note, as written ([`Status`]), and as drawn (copied when the output wakes the view).
    status: Status,
    notice: Option<SharedString>,
    size: (u16, u16),
    focus: FocusHandle,
    /// Wheel movement not yet a whole line.
    wheel: f32,
    scrollbar: scrollbar::Scrollbar,
    /// Where the grid was painted (its top left) and a cell's width: what a mouse position maps through.
    layout: Rc<Cell<Option<(Point<Pixels>, Pixels)>>>,
    /// Whether the left button is down over the grid, extending the selection as it moves.
    selecting: bool,
    /// The right-click menu, at this window position.
    menu: Option<Point<Pixels>>,
    _pump: Option<Task<()>>,
    wake: OutputWake,
    /// A command it runs once and watches, for a setup fix (`run_once.rs`).
    once: run_once::Once,
}

impl Terminal {
    pub fn new(dir: PathBuf, cx: &mut Context<Self>) -> Terminal {
        let mut terminal = Self::unstarted(dir, None, cx);
        terminal.start(cx);
        terminal
    }

    /// A shell on a server (a server session's), drawn as a shell here is.
    pub fn on_server(target: ServerTarget, cx: &mut Context<Self>) -> Terminal {
        let mut terminal = Self::unstarted(PathBuf::new(), Some(target), cx);
        terminal.start(cx);
        terminal
    }

    /// A server shell's view that opens nothing: GPUI's test scheduler takes no wakes from other threads.
    #[cfg(test)]
    pub(crate) fn server_for_view_test(target: ServerTarget, cx: &mut Context<Self>) -> Terminal {
        Self::unstarted(PathBuf::new(), Some(target), cx)
    }

    /// The command a server shell runs instead of an interactive shell, until it starts.
    #[cfg(test)]
    pub(crate) fn server_command(&self) -> Option<&str> {
        self.server.as_ref()?.command.as_deref()
    }

    fn unstarted(dir: PathBuf, server: Option<ServerTarget>, cx: &mut Context<Self>) -> Terminal {
        let (input, _) = channel();
        let term = Term::new(Config::default(), &TermSize::new(80, 24), Listener::new(input));
        Terminal {
            retired: false,
            dir,
            server,
            term: Arc::new(Mutex::new(term)),
            shell: None,
            attached: None,
            well: false,
            ended: Arc::default(),
            error: None,
            status: Status::default(),
            notice: None,
            size: (80, 24),
            focus: cx.focus_handle(),
            wheel: 0.,
            scrollbar: Default::default(),
            layout: Rc::default(),
            selecting: false,
            menu: None,
            _pump: None,
            wake: OutputWake::default(),
            once: Default::default(),
        }
    }

    /// A private terminal view with no OS producer. GPUI's test scheduler requires all wakes
    /// to come from its deterministic executor; plain PTY tests exercise real process I/O.
    #[cfg(test)]
    pub(crate) fn for_view_test(dir: PathBuf, cx: &mut Context<Self>) -> Terminal {
        let mut terminal = Self::attached(Rc::new(|_, _| {}), cx);
        terminal.dir = dir;
        terminal.attached = None;
        terminal
    }

    /// Attach the same renderer and input handling to a session-owned PTY.
    pub fn attached(send: Rc<dyn Fn(&str, serde_json::Value)>, cx: &mut Context<Self>) -> Terminal {
        let (input, replies) = channel();
        let listener = Listener::new(input);
        Terminal {
            retired: false,
            dir: PathBuf::new(),
            server: None,
            term: Arc::new(Mutex::new(Term::new(
                Config::default(),
                &TermSize::new(80, 24),
                listener.clone(),
            ))),
            shell: None,
            attached: Some(Attached {
                send,
                replies,
                processor: Processor::new(),
                writable: false,
                listener,
            }),
            well: false,
            ended: Arc::default(),
            error: None,
            status: Status::default(),
            notice: None,
            size: (80, 24),
            focus: cx.focus_handle(),
            wheel: 0.,
            scrollbar: Default::default(),
            layout: Rc::default(),
            selecting: false,
            menu: None,
            _pump: None,
            wake: OutputWake::default(),
            once: Default::default(),
        }
    }

    pub(crate) fn retain_shared(&mut self) -> RetainedSharedTerminal {
        let attached = self.attached.take().expect("shared command display");
        self.retired = true;
        // Move the screen instead of sharing it with any surviving old entity clone.
        let (unused, _) = channel();
        let empty = Arc::new(Mutex::new(Term::new(
            Config::default(),
            &TermSize::new(80, 24),
            Listener::new(unused),
        )));
        RetainedSharedTerminal {
            term: std::mem::replace(&mut self.term, empty),
            processor: attached.processor,
            listener: attached.listener,
            size: self.size,
            writable: attached.writable,
        }
    }

    pub(crate) fn restore_shared(
        saved: RetainedSharedTerminal,
        send: Rc<dyn Fn(&str, serde_json::Value)>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::attached(send, cx);
        let (sender, replies) = channel();
        if let Ok(mut target) = saved.listener.0.lock() {
            *target = sender;
        }
        view.term = saved.term;
        view.size = saved.size;
        let attached = view.attached.as_mut().unwrap();
        attached.processor = saved.processor;
        attached.listener = saved.listener;
        attached.replies = replies;
        attached.writable = saved.writable;
        view
    }

    /// Feed exact PTY bytes; ownership controls stdin, without replacing the screen or its selection.
    pub fn feed(&mut self, bytes: &[u8], writable: bool, truncated: bool, cx: &mut Context<Self>) -> bool {
        let Some(attached) = &mut self.attached else {
            return false;
        };
        let gained_input = writable && !attached.writable;
        // An empty read of a quiet command changes nothing on screen: it does not redraw the window.
        let redraw = !bytes.is_empty() || truncated || writable != attached.writable;
        attached.writable = writable;
        if gained_input {
            (attached.send)("resize", serde_json::json!({"cols":self.size.0,"rows":self.size.1}));
        }
        if let Ok(mut term) = self.term.lock() {
            if truncated {
                attached
                    .processor
                    .advance(&mut *term, b"\r\n[Earlier output is no longer buffered.]\r\n");
            }
            attached.processor.advance(&mut *term, bytes);
        } else {
            return false;
        }
        while let Ok(reply) = attached.replies.try_recv() {
            if writable {
                (attached.send)("input", serde_json::json!({"data": String::from_utf8_lossy(&reply)}));
            }
        }
        if redraw {
            self.refresh_scrollbar();
            cx.notify();
        }
        true
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Draw the shell on a Commands row's output well, or on the pane's fill.
    pub fn set_well(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.well != on {
            self.well = on;
            cx.notify();
        }
    }

    /// Start the shell (again, after it ended), on a clean screen.
    fn start(&mut self, cx: &mut Context<Self>) {
        if self.retired || self.attached.is_some() {
            return;
        }
        // Enter after a watched command ended: a plain shell, watched by nobody.
        if self.once.reported {
            self.once = Default::default();
        }
        self.shell = None;
        let (cols, rows) = self.size;
        self._pump = None;
        self.wake = OutputWake::default();
        // A reader of the shell before writes to its own note, not this one's.
        self.status = Status::default();
        self.notice = None;
        let spawned = match &mut self.server {
            // A project command runs once; Enter after it starts a shell.
            Some(target) => {
                let spawned = spawn_server(
                    target,
                    cols,
                    rows,
                    self.term.clone(),
                    self.wake.clone(),
                    self.status.clone(),
                    self.once.exit.clone(),
                );
                target.command = None;
                Ok(spawned)
            }
            // A fix's command runs once; Enter after it starts a shell.
            None => {
                let once = self.once.command.take();
                let run = once.as_deref().map(|line| (line, self.once.exit.clone()));
                spawn(&self.dir, None, run, cols, rows, self.term.clone(), self.wake.clone())
            }
        };
        match spawned {
            Ok((shell, ended)) => {
                self.shell = Some(shell);
                self.ended = ended;
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("The terminal didn't start: {error}").into());
                return;
            }
        }
        self.bind_output(cx);
    }

    fn bind_output(&mut self, cx: &mut Context<Self>) {
        let (wake, mut woken) = unbounded::<()>();
        self.wake.bind(Some(wake));
        self._pump = Some(cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                // Output comes in bursts: one frame for all that is waiting.
                while woken.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.woken(cx)).is_err() {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| this.woken(cx));
        }));
    }

    /// The shell wrote, ended, or its server's note changed: take the note, and draw again.
    fn woken(&mut self, cx: &mut Context<Self>) {
        let status = self.status.lock().ok().and_then(|status| status.clone());
        self.notice = status.map(SharedString::from);
        self.report_exit(cx);
        self.refresh_scrollbar();
        cx.notify();
    }

    /// Read the scrollback bar's place from the grid: after output, a wheel turn, or the pointer coming over it.
    fn refresh_scrollbar(&mut self) {
        if let Ok(term) = self.term.lock() {
            self.scrollbar.refresh(&term);
        }
    }

    /// The grid's columns and rows, as the shell sees them.
    pub(crate) fn size(&self) -> (u16, u16) {
        self.size
    }

    /// A column's width in pixels, once the grid has been painted.
    pub(crate) fn cell_width(&self) -> Option<f32> {
        self.layout.get().map(|(_, cell)| f32::from(cell))
    }

    /// Transfer a private shell before its window is destroyed. The old view becomes inert.
    pub fn retain(&mut self) -> RetainedTerminal {
        assert!(
            self.attached.is_none(),
            "session-owned PTYs must not be retained as private shells"
        );
        self.wake.bind(None);
        self._pump = None;
        RetainedTerminal {
            dir: self.dir.clone(),
            server: self.server.clone(),
            term: self.term.clone(),
            shell: self.shell.take(),
            ended: self.ended.clone(),
            error: self.error.take(),
            status: self.status.clone(),
            size: self.size,
            wake: self.wake.clone(),
        }
    }

    /// Rebind output to a fresh entity, preserving the PTY, parser grid, scrollback and dimensions.
    pub fn restore(saved: RetainedTerminal, cx: &mut Context<Self>) -> Self {
        let mut terminal = Terminal {
            retired: false,
            dir: saved.dir,
            server: saved.server,
            term: saved.term,
            shell: saved.shell,
            attached: None,
            well: false,
            ended: saved.ended,
            error: saved.error,
            status: saved.status,
            notice: None,
            size: saved.size,
            focus: cx.focus_handle(),
            wheel: 0.,
            scrollbar: Default::default(),
            layout: Rc::default(),
            selecting: false,
            menu: None,
            _pump: None,
            wake: saved.wake,
            once: Default::default(),
        };
        terminal.notice = terminal
            .status
            .lock()
            .ok()
            .and_then(|status| status.clone())
            .map(SharedString::from);
        terminal.bind_output(cx);
        terminal
    }

    fn ended(&self) -> bool {
        self.attached.as_ref().map_or_else(
            || self.shell.is_none() || self.ended.load(Ordering::Relaxed),
            |attached| !attached.writable,
        )
    }

    fn write(&mut self, bytes: Vec<u8>) {
        if self.retired {
            return;
        }
        if let Some(attached) = &self.attached {
            if attached.writable {
                (attached.send)("input", serde_json::json!({"data": String::from_utf8_lossy(&bytes)}));
            }
            return;
        }
        if let Some(shell) = &self.shell {
            let _ = shell.input.send(bytes);
        }
    }

    /// Make the grid and the shell `cols` by `rows`.
    fn resize(&mut self, cols: u16, rows: u16) {
        if self.retired {
            return;
        }
        if (cols, rows) == self.size {
            return;
        }
        self.size = (cols, rows);
        if let Ok(mut term) = self.term.lock() {
            term.resize(TermSize::new(cols.into(), rows.into()));
        }
        if let Some(shell) = &self.shell {
            shell.resize(cols, rows);
        }
        if let Some(attached) = &self.attached
            && attached.writable
        {
            (attached.send)("resize", serde_json::json!({"cols": cols, "rows": rows}));
        }
    }

    /// Type `command` and Enter, a line at a time, as if the person had: a shell that is still starting reads it
    /// once it is ready.
    pub fn type_command(&mut self, command: &str) {
        let lines: Vec<&str> = command.trim_end().lines().collect();
        self.to_bottom();
        self.write(format!("{}\r", lines.join("\r")).into_bytes());
    }

    pub fn paste(&mut self, text: &str) {
        let bracketed = self
            .term
            .lock()
            .map(|term| term.mode().contains(TermMode::BRACKETED_PASTE))
            .unwrap_or(false);
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if bracketed {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text
        };
        self.to_bottom();
        self.write(bytes.into_bytes());
    }

    fn to_bottom(&self) {
        if let Ok(mut term) = self.term.lock() {
            term.scroll_display(Scroll::Bottom);
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;
        // Paste: Ctrl+Shift+V, or Cmd+V on macOS.
        let paste = if cfg!(target_os = "macos") {
            modifiers.platform && keystroke.key == "v"
        } else {
            modifiers.control && modifiers.shift && keystroke.key == "v"
        };
        if paste {
            self.paste_clipboard(cx);
            cx.stop_propagation();
            return;
        }
        // Copy: Ctrl+Shift+C, or Cmd+C on macOS.
        let copy = if cfg!(target_os = "macos") {
            modifiers.platform && keystroke.key == "c"
        } else {
            modifiers.control && modifiers.shift && keystroke.key == "c"
        };
        if copy {
            self.copy(cx);
            cx.stop_propagation();
            return;
        }
        if self.ended() {
            if keystroke.key == "enter" && !modifiers.modified() {
                self.start(cx);
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }
        let app_cursor = self
            .term
            .lock()
            .map(|term| term.mode().contains(TermMode::APP_CURSOR))
            .unwrap_or(false);
        if let Some(bytes) = key_bytes(keystroke, app_cursor) {
            self.clear_selection();
            self.to_bottom();
            self.write(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.clear_selection();
            self.paste(&text);
            cx.notify();
        }
    }

    /// Put the selection on the clipboard. False when nothing is selected.
    pub fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.term.lock().ok().and_then(|term| term.selection_to_string());
        match text.filter(|text| !text.is_empty()) {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }

    fn has_selection(&self) -> bool {
        self.term
            .lock()
            .is_ok_and(|term| term.selection.as_ref().is_some_and(|selection| !selection.is_empty()))
    }

    fn clear_selection(&mut self) {
        if let Ok(mut term) = self.term.lock() {
            term.selection = None;
        }
    }

    /// The grid cell under a window position, and which half of it, as the selection reads it.
    fn grid_point(&self, position: Point<Pixels>) -> Option<(GridPoint, Side)> {
        let (origin, cell) = self.layout.get()?;
        let (cols, rows) = self.size;
        let x = f32::from(position.x - origin.x).max(0.);
        let y = f32::from(position.y - origin.y).max(0.);
        let cell = f32::from(cell).max(1.);
        let column = ((x / cell) as usize).min(usize::from(cols).saturating_sub(1));
        let row = ((y / LINE_HEIGHT) as usize).min(usize::from(rows).saturating_sub(1));
        let side = if x - column as f32 * cell > cell / 2. {
            Side::Right
        } else {
            Side::Left
        };
        let offset = self.term.lock().ok()?.grid().display_offset();
        Some((viewport_to_point(offset, GridPoint::new(row, Column(column))), side))
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.menu = None;
        let ty = match event.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        if let Some((point, side)) = self.grid_point(event.position)
            && let Ok(mut term) = self.term.lock()
        {
            term.selection = Some(Selection::new(ty, point, side));
        }
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some((point, side)) = self.grid_point(event.position)
            && let Ok(mut term) = self.term.lock()
            && let Some(selection) = term.selection.as_mut()
        {
            selection.update(point, side);
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        // A click that selected nothing leaves no selection behind.
        if !self.has_selection() {
            self.clear_selection();
        }
        cx.notify();
    }

    fn render_menu(&self, at: Point<Pixels>, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        // Cmd+C on macOS; elsewhere Ctrl+C is the shell's interrupt, so Ctrl+Shift+C, as GNOME Terminal.
        let (copy_keys, paste_keys) = if cfg!(target_os = "macos") {
            ("cmd-c", "cmd-v")
        } else {
            ("ctrl-shift-c", "ctrl-shift-v")
        };
        let (copy_keys, paste_keys) = (shortcut_label::label(copy_keys), shortcut_label::label(paste_keys));
        let can_copy = self.has_selection();
        let row = |id: &'static str, label: &'static str, keys: String, enabled: bool| {
            h_flex()
                .id(id)
                .h(px(28.))
                .px(px(8.))
                .gap(px(12.))
                .rounded(px(6.))
                .text_size(px(13.))
                .map(|row| {
                    if enabled {
                        row.text_color(palette.foreground)
                            .cursor_pointer()
                            .hover(|style| menu_item_hover(style, palette))
                    } else {
                        row.text_color(palette.faint)
                    }
                })
                .child(div().flex_1().child(label))
                .child(div().text_size(px(12.)).text_color(palette.faint).child(keys))
        };
        deferred(
            anchored().position(at).snap_to_window_with_margin(px(8.)).child(
                v_flex()
                    .id("terminal-menu")
                    .w(px(200.))
                    .p(px(4.))
                    .rounded(px(10.))
                    .floating_ground(10.0, palette.menu, false, &palette)
                    .shadow(palette.pop_shadow(palette.pop_ring()))
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    }))
                    .child(row("terminal-copy", "Copy", copy_keys, can_copy).when(can_copy, |row| {
                        row.on_click(cx.listener(|this, _, _, cx| {
                            this.copy(cx);
                            this.menu = None;
                            cx.notify();
                        }))
                    }))
                    .child(row("terminal-paste", "Paste", paste_keys, true).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.menu = None;
                            this.paste_clipboard(cx);
                            window.focus(&this.focus, cx);
                        },
                    ))),
            ),
        )
        .with_priority(2)
    }

    /// The viewport cell under a window position, 1-based (column, row), as a mouse report names it.
    fn report_cell(&self, position: Point<Pixels>) -> (usize, usize) {
        let Some((origin, cell)) = self.layout.get() else {
            return (1, 1);
        };
        let (cols, rows) = self.size;
        let x = f32::from(position.x - origin.x).max(0.) / f32::from(cell).max(1.);
        let y = f32::from(position.y - origin.y).max(0.) / LINE_HEIGHT;
        (
            (x as usize).min(usize::from(cols).saturating_sub(1)) + 1,
            (y as usize).min(usize::from(rows).saturating_sub(1)) + 1,
        )
    }

    fn on_wheel(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        // The terminal keeps the wheel: a Commands row's list around it does not scroll too.
        cx.stop_propagation();
        self.wheel += f32::from(event.delta.pixel_delta(px(LINE_HEIGHT)).y);
        let lines = (self.wheel / LINE_HEIGHT) as i32;
        if lines == 0 {
            return;
        }
        self.wheel -= lines as f32 * LINE_HEIGHT;
        let Ok(mode) = self.term.lock().map(|term| *term.mode()) else {
            return;
        };
        let cell = self.report_cell(event.position);
        match wheel_action(mode, lines, event.modifiers.shift, cell) {
            Wheel::Send(bytes) => self.write(bytes),
            Wheel::Scroll(lines) => {
                if let Ok(mut term) = self.term.lock() {
                    term.scroll_display(Scroll::Delta(lines));
                    self.scrollbar.refresh(&term);
                }
                self.scrollbar.scrolled(cx);
            }
        }
        cx.notify();
    }
}

/// What a wheel turn of whole lines does.
#[derive(Debug, PartialEq)]
enum Wheel {
    /// Bytes for the program: mouse wheel reports, or arrow keys.
    Send(Vec<u8>),
    /// Move the view through the scrollback; positive goes up.
    Scroll(i32),
}

/// The wheel as xterm and Alacritty treat it. `lines` > 0 is up; `cell` is the 1-based (column, row) under the pointer.
/// - A program that asked for mouse reports (Claude Code, htop, vim with `mouse=a`) gets wheel reports.
/// - A full-screen program without them (less, man) gets arrow keys, while it keeps alternate scroll on.
/// - Otherwise, and always with Shift held, the view scrolls through the scrollback.
fn wheel_action(mode: TermMode, lines: i32, shift: bool, (column, row): (usize, usize)) -> Wheel {
    let count = lines.unsigned_abs() as usize;
    if !shift && mode.intersects(TermMode::MOUSE_MODE) {
        // Buttons 64 and 65: wheel up and down.
        let button = if lines > 0 { 64 } else { 65 };
        let report = if mode.contains(TermMode::SGR_MOUSE) {
            format!("\x1b[<{button};{column};{row}M").into_bytes()
        } else {
            // The legacy encoding adds 32 to each value in a byte, so it stops at column and row 223.
            let byte = |value: usize| (32 + value.min(223)) as u8;
            vec![0x1b, b'[', b'M', 32 + button, byte(column), byte(row)]
        };
        return Wheel::Send(report.repeat(count));
    }
    if !shift && mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
        let key = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
            (true, true) => "\x1bOA",
            (true, false) => "\x1b[A",
            (false, true) => "\x1bOB",
            (false, false) => "\x1b[B",
        };
        return Wheel::Send(key.repeat(count).into_bytes());
    }
    Wheel::Scroll(lines)
}

/// The recessed fill a command's output sits in: a step darker than the pane's `background` in both themes.
pub(crate) fn output_well(palette: Palette) -> Hsla {
    // See-through: the same recess as ink, so it darkens whatever tier it lies on (`frost::Tiers`).
    if palette.frosted {
        return gpui_kit::black().opacity(if palette.dark { 0.22 } else { 0.07 });
    }
    if palette.dark {
        rgb(0x161617).into()
    } else {
        rgb(0xE4E4E7).into()
    }
}

impl Render for Terminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<crate::pages::Theme>().0;
        let mut colors = TermColors::new(palette);
        // A command's output in the Commands tab sits in a recessed well, the window's darkest fill, so it reads
        // apart from the pane and the command text above it. A Workbench shell keeps the pane's fill, unless it is
        // drawn in a Commands row too.
        if self.attached.is_some() || self.well {
            colors.background = output_well(palette);
        }
        let focused = self.focus.is_focused(window);
        let note: Option<SharedString> = self.error.clone().or_else(|| self.notice.clone()).or_else(|| {
            (self.attached.is_none() && self.ended()).then(|| "The shell ended. Press Enter to start a new one.".into())
        });
        let this = cx.entity().downgrade();
        let term = self.term.clone();
        let layout = self.layout.clone();
        // The components' code face (DejaVu Sans Mono on Linux, Menlo on macOS), as the Files viewer's.
        let family = gpui_kit::component::Theme::global(cx).mono_font_family.clone();
        let grid = canvas(
            move |bounds, window, cx| {
                let font = font(family);
                let font_id = window.text_system().resolve_font(&font);
                let cell = window
                    .text_system()
                    .advance(font_id, px(FONT_SIZE), 'm')
                    .map(|size| size.width)
                    .unwrap_or(px(FONT_SIZE * 0.6));
                let cols = (f32::from(bounds.size.width) / f32::from(cell)).floor().max(2.) as u16;
                let rows = (f32::from(bounds.size.height) / LINE_HEIGHT).floor().max(1.) as u16;
                layout.set(Some((bounds.origin, cell)));
                let _ = this.update(cx, |this, _| this.resize(cols, rows));
                let frame = term.lock().map(|term| Frame::read(&term, &colors, &font)).ok();
                (cell, frame)
            },
            move |bounds, (cell, frame), window, cx| {
                let Some(frame) = frame else {
                    return;
                };
                let line_height = px(LINE_HEIGHT);
                for (row, line) in frame.lines.into_iter().enumerate() {
                    let y = bounds.origin.y + line_height * row as f32;
                    for (column, width, color) in line.backgrounds {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.origin.x + cell * column as f32, y),
                                size(cell * width as f32, line_height),
                            ),
                            color,
                        ));
                    }
                    if line.text.trim().is_empty() {
                        continue;
                    }
                    let shaped =
                        window
                            .text_system()
                            .shape_line(line.text.into(), px(FONT_SIZE), &line.runs, Some(cell));
                    let _ = shaped.paint(
                        point(bounds.origin.x, y),
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
                if let Some((column, row)) = frame.cursor {
                    let cursor = Bounds::new(
                        point(
                            bounds.origin.x + cell * column as f32,
                            bounds.origin.y + line_height * row as f32,
                        ),
                        size(cell, line_height),
                    );
                    if focused {
                        window.paint_quad(fill(cursor, colors.cursor.opacity(0.55)));
                    } else {
                        window.paint_quad(outline(cursor, colors.cursor.opacity(0.7), BorderStyle::Solid));
                    }
                }
            },
        )
        .size_full();
        let scrollbar = self.scrollbar.render(self.size.1, palette);
        v_flex()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(colors.background)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.refresh_scrollbar();
                }
                if this.scrollbar.hover(*hovered) {
                    cx.notify();
                }
            }))
            .children(scrollbar)
            .on_key_down(cx.listener(Self::on_key))
            .on_scroll(cx.listener(Self::on_wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            // Files dropped on a running shell type their quoted paths, as Terminal.app (and the Nexus app) do.
            // On an ended one they go on to the composer.
            .when(!self.ended(), |view| {
                view.on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                    window.focus(&this.focus, cx);
                    this.paste(&crate::attachments::paths_for_shell(paths.paths(), cfg!(windows)));
                    cx.notify();
                }))
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.menu = Some(event.position);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .debug_selector(|| "terminal-grid".into())
                    .flex_1()
                    .min_h(px(0.))
                    .px(px(PADDING_X))
                    .py(px(PADDING))
                    .child(grid),
            )
            .when_some(self.menu, |view, at| view.child(self.render_menu(at, palette, cx)))
            .when_some(note, |view, note| {
                view.child(
                    div()
                        .flex_none()
                        .px(px(PADDING_X))
                        .py(px(8.))
                        .bg(palette.hover)
                        .text_size(px(12.5))
                        .text_color(palette.muted)
                        .child(note),
                )
            })
    }
}

/// The shell in `dir` in a `cols` by `rows` pseudo-terminal, as the Nexus app starts it: the person's login shell (or
/// `program`), with the agents' environment (their `PATH`), `TERM=xterm-256color` and true color. A thread reads its
/// output into `term` and wakes the view; the flag says when the output closed. `run`: the login shell runs this one
/// command instead, and its exit code goes to the cell (`run_once.rs`).
fn spawn(
    dir: &Path,
    program: Option<&str>,
    run: Option<(&str, run_once::Exit)>,
    cols: u16,
    rows: u16,
    term: Arc<Mutex<Term<Listener>>>,
    wake: OutputWake,
) -> Result<(Shell, Arc<AtomicBool>), String> {
    let pair = native_pty_system()
        .openpty(pty_size(cols, rows))
        .map_err(|error| error.to_string())?;
    // Without it, Backspace in a program that reads a line erases part of a letter and walks into the prompt.
    if let Err(error) = nexus_workbench::pty::utf8_line_editing(&*pair.master) {
        eprintln!("nexus-desktop-gpui: terminal: UTF-8 line editing: {error}");
    }
    // None: the person's login shell.
    let mut command = match &run {
        Some((line, _)) => run_once::shell_command(line),
        None => program.map_or_else(CommandBuilder::new_default_prog, CommandBuilder::new),
    };
    let exit = run.map(|(_, exit)| exit);
    command.env_clear();
    for (name, value) in crate::code_run::agent_env() {
        command.env(name, value);
    }
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.cwd(dir);
    let mut child = pair.slave.spawn_command(command).map_err(|error| error.to_string())?;
    // The shell holds its own end now; ours would keep the output open after the shell ends.
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|error| error.to_string())?;
    let mut writer = pair.master.take_writer().map_err(|error| error.to_string())?;
    let (input, typed) = channel::<Vec<u8>>();
    // Ends when the shell does (the write fails) or the view goes (the sender drops).
    std::thread::spawn(move || {
        for bytes in typed {
            if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                break;
            }
        }
    });
    // The terminal core answers the shell's questions through the same writer.
    if let Ok(mut term) = term.lock() {
        *term = Term::new(
            Config::default(),
            &TermSize::new(cols.into(), rows.into()),
            Listener::new(input.clone()),
        );
    }
    let ended = Arc::new(AtomicBool::new(false));
    let output_ended = ended.clone();
    let shell_wake = wake.clone();
    let exit_wake = wake.clone();
    std::thread::spawn(move || {
        let mut parser: Processor = Processor::new();
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let read = match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            match term.lock() {
                Ok(mut term) => parser.advance(&mut *term, &buffer[..read]),
                Err(_) => break,
            }
            wake.notify();
        }
        output_ended.store(true, Ordering::Relaxed);
        wake.notify();
    });
    let killer = child.clone_killer();
    // Reaped, so no zombie stays behind. A watched command's code goes to its cell, and the view hears of it.
    std::thread::spawn(move || {
        let status = child.wait();
        if let Some(exit) = exit {
            if let Ok(mut exit) = exit.lock() {
                *exit = Some(status.ok().map(|status| status.exit_code()));
            }
            exit_wake.notify();
        }
    });
    Ok((
        Shell {
            input,
            pty: Pty::Here {
                master: pair.master,
                killer,
            },
            wake: shell_wake,
        },
        ended,
    ))
}

/// The note a server shell's end leaves: its exit code when it failed, and that a command (a project command) finished.
/// None: the usual "the shell ended".
fn exit_note(code: Option<u32>, command: bool) -> Option<String> {
    match code {
        Some(code) if code != 0 => Some(format!("It exited with code {code}. Press Enter to start a shell.")),
        Some(_) if command => Some("It finished. Press Enter to start a shell.".into()),
        _ => None,
    }
}

/// A `cols` by `rows` shell on `target`'s server, read into `term` as [`spawn`] reads one here. Typing goes to the
/// server in order; the server's notes (unreachable, an exit code) go to `status`.
fn spawn_server(
    target: &ServerTarget,
    cols: u16,
    rows: u16,
    term: Arc<Mutex<Term<Listener>>>,
    wake: OutputWake,
    status: Status,
    exit: run_once::Exit,
) -> (Shell, Arc<AtomicBool>) {
    let (input, typed) = channel::<Vec<u8>>();
    // The terminal core answers the shell's questions through the same input.
    if let Ok(mut term) = term.lock() {
        *term = Term::new(
            Config::default(),
            &TermSize::new(cols.into(), rows.into()),
            Listener::new(input.clone()),
        );
    }
    let ended = Arc::new(AtomicBool::new(false));
    let (output_ended, shell_wake) = (ended.clone(), wake.clone());
    let command = target.command.is_some();
    let mut parser: Processor = Processor::new();
    let server = server_shell::ServerShell::start(target, cols, rows, move |event| {
        match event {
            server_shell::Event::Output { bytes, truncated } => {
                if let Ok(mut term) = term.lock() {
                    if truncated {
                        parser.advance(&mut *term, b"\r\n[Earlier output is no longer buffered.]\r\n");
                    }
                    parser.advance(&mut *term, &bytes);
                }
            }
            server_shell::Event::Exited(code) => {
                if let Some(note) = exit_note(code, command)
                    && let Ok(mut status) = status.lock()
                {
                    *status = Some(note);
                }
                if command && let Ok(mut exit) = exit.lock() {
                    *exit = Some(code);
                }
                output_ended.store(true, Ordering::Relaxed);
            }
            server_shell::Event::Problem(problem) => {
                if let Ok(mut status) = status.lock() {
                    *status = problem;
                }
            }
        }
        wake.notify();
    });
    // Typing (and the terminal core's answers) reach the server through the shell's own queue, in order. Ends when
    // the screen and the shell let go of `input`, or the shell's queue is gone.
    let typing = server.typing();
    std::thread::spawn(move || {
        for bytes in typed {
            if !typing(bytes) {
                break;
            }
        }
    });
    let shell = Shell {
        input,
        pty: Pty::Server(server),
        wake: shell_wake,
    };
    (shell, ended)
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols: cols.max(2),
        rows: rows.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// What a key sends to the shell, as xterm sends it. None: not the terminal's key (an app shortcut).
fn key_bytes(keystroke: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let modifiers = &keystroke.modifiers;
    if modifiers.platform || modifiers.function && keystroke.key.len() == 1 {
        return None;
    }
    // xterm's modifier parameter: 1, plus 1 for Shift, 2 for Alt, 4 for Ctrl.
    let parameter = 1 + u8::from(modifiers.shift) + 2 * u8::from(modifiers.alt) + 4 * u8::from(modifiers.control);
    let cursor = |end: char| -> Vec<u8> {
        if parameter > 1 {
            format!("\x1b[1;{parameter}{end}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{end}").into_bytes()
        } else {
            format!("\x1b[{end}").into_bytes()
        }
    };
    let tilde = |code: u8| -> Vec<u8> {
        if parameter > 1 {
            format!("\x1b[{code};{parameter}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        }
    };
    let alt = |bytes: Vec<u8>| -> Vec<u8> {
        if modifiers.alt {
            [vec![0x1b], bytes].concat()
        } else {
            bytes
        }
    };
    let bytes = match keystroke.key.as_str() {
        "enter" => alt(b"\r".to_vec()),
        "backspace" if modifiers.control => alt(vec![0x08]),
        "backspace" => alt(vec![0x7f]),
        "tab" if modifiers.shift => b"\x1b[Z".to_vec(),
        "tab" => alt(b"\t".to_vec()),
        "escape" => alt(vec![0x1b]),
        "up" => cursor('A'),
        "down" => cursor('B'),
        "right" => cursor('C'),
        "left" => cursor('D'),
        "home" => cursor('H'),
        "end" => cursor('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if modifiers.control => vec![0],
        key if modifiers.control => {
            let mut chars = key.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            let code = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 - b'a' + 1,
                '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' | '/' | '7' => 0x1f,
                '8' | '?' => 0x7f,
                _ => return None,
            };
            alt(vec![code])
        }
        _ => {
            let text = keystroke
                .key_char
                .clone()
                .or_else(|| (keystroke.key == "space").then(|| " ".to_string()))?;
            alt(text.into_bytes())
        }
    };
    Some(bytes)
}

/// The terminal's colors for the app's theme: the 16 ANSI colors tuned for its background, then xterm's 256.
#[derive(Clone, Copy)]
struct TermColors {
    foreground: Hsla,
    background: Hsla,
    cursor: Hsla,
    selection: Hsla,
    ansi: [Hsla; 16],
}

impl TermColors {
    fn new(palette: Palette) -> TermColors {
        let ansi: [u32; 16] = if palette.dark {
            [
                0x1d1f21, 0xf0726c, 0x7fc27f, 0xe5c07b, 0x6aa9f2, 0xc493e5, 0x5fc4c9, 0xc8c8c8, 0x6b6f76, 0xff8f8a,
                0x9ad69a, 0xf2d391, 0x8fbfff, 0xd7aef0, 0x82dade, 0xffffff,
            ]
        } else {
            [
                0x1f2328, 0xc4322b, 0x2f7d32, 0x8a6100, 0x1f5fbf, 0x8340b0, 0x167c86, 0x6e7781, 0x57606a, 0xd8453e,
                0x3a8f3e, 0x9e7100, 0x2f6fd6, 0x9554c4, 0x1c8f9a, 0x8c959f,
            ]
        };
        TermColors {
            foreground: palette.foreground,
            // The pane's own fill, so a shell sits flush in the Workbench.
            background: palette.sidebar,
            cursor: palette.foreground,
            selection: palette.foreground.opacity(if palette.dark { 0.24 } else { 0.18 }),
            ansi: ansi.map(|hex| rgb(hex).into()),
        }
    }

    fn indexed(&self, index: u8) -> Hsla {
        match index {
            0..16 => self.ansi[index as usize],
            16..232 => {
                let index = index - 16;
                let level = |value: u8| if value == 0 { 0 } else { 55 + 40 * u32::from(value) };
                let (r, g, b) = (level(index / 36), level(index / 6 % 6), level(index % 6));
                rgb((r << 16) | (g << 8) | b).into()
            }
            _ => {
                let gray = 8 + 10 * u32::from(index - 232);
                rgb((gray << 16) | (gray << 8) | gray).into()
            }
        }
    }

    fn of(&self, color: Color, set: &alacritty_terminal::term::color::Colors) -> Hsla {
        let custom = |index: usize| {
            set[index].map(|Rgb { r, g, b }| -> Hsla {
                rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into()
            })
        };
        match color {
            Color::Spec(Rgb { r, g, b }) => rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into(),
            Color::Indexed(index) => custom(index as usize).unwrap_or_else(|| self.indexed(index)),
            Color::Named(named) => custom(named as usize).unwrap_or_else(|| match named {
                NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => self.foreground,
                NamedColor::Background => self.background,
                NamedColor::DimForeground => self.foreground.opacity(0.66),
                named if (named as usize) < 16 => self.ansi[named as usize],
                // DimBlack to DimWhite.
                named => self.ansi[(named as usize).saturating_sub(NamedColor::DimBlack as usize) % 8].opacity(0.66),
            }),
        }
    }
}

/// One painted line: its text, styled in runs, and the cells with their own background.
struct Line {
    text: String,
    runs: Vec<TextRun>,
    backgrounds: Vec<(usize, usize, Hsla)>,
}

/// What the grid shows now, read under the lock and painted after it.
struct Frame {
    lines: Vec<Line>,
    /// Column and row, while the cursor shows.
    cursor: Option<(usize, usize)>,
}

impl Frame {
    fn read(term: &Term<Listener>, colors: &TermColors, font: &Font) -> Frame {
        let content = term.renderable_content();
        let rows = term.screen_lines();
        let offset = content.display_offset as i32;
        let mut lines: Vec<Line> = (0..rows)
            .map(|_| Line {
                text: String::new(),
                runs: Vec::new(),
                backgrounds: Vec::new(),
            })
            .collect();
        let cursor = content.cursor;
        let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR);
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset;
            let Some(line) = usize::try_from(row).ok().and_then(|row| lines.get_mut(row)) else {
                continue;
            };
            let cell = &indexed.cell;
            let column = indexed.point.column.0;
            let (mut fg, mut bg) = (colors.of(cell.fg, content.colors), colors.of(cell.bg, content.colors));
            let mut default_bg = matches!(cell.bg, Color::Named(NamedColor::Background));
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
                default_bg = false;
            }
            if cell.flags.contains(Flags::DIM) {
                fg = fg.opacity(0.66);
            }
            if content.selection.is_some_and(|range| range.contains(indexed.point)) {
                bg = colors.selection;
                default_bg = false;
            }
            if !default_bg {
                match line.backgrounds.last_mut() {
                    Some((start, width, color)) if *start + *width == column && *color == bg => *width += 1,
                    _ => line.backgrounds.push((column, 1, bg)),
                }
            }
            let c = if cell.flags.intersects(Flags::HIDDEN | Flags::WIDE_CHAR_SPACER) || cell.c == '\0' {
                ' '
            } else {
                cell.c
            };
            let mut font = font.clone();
            if cell.flags.contains(Flags::BOLD) {
                font.weight = FontWeight::BOLD;
            }
            if cell.flags.contains(Flags::ITALIC) {
                font.style = FontStyle::Italic;
            }
            let underline = cell.flags.intersects(Flags::ALL_UNDERLINES).then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(fg),
                wavy: cell.flags.contains(Flags::UNDERCURL),
            });
            let strikethrough = cell.flags.contains(Flags::STRIKEOUT).then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(fg),
            });
            let len = c.len_utf8();
            line.text.push(c);
            match line.runs.last_mut() {
                Some(run)
                    if run.color == fg
                        && run.font == font
                        && run.underline == underline
                        && run.strikethrough == strikethrough =>
                {
                    run.len += len
                }
                _ => line.runs.push(TextRun {
                    len,
                    font,
                    color: fg,
                    background_color: None,
                    underline,
                    strikethrough,
                }),
            }
        }
        let row = cursor.point.line.0 + offset;
        let cursor =
            (show_cursor && row >= 0 && (row as usize) < rows).then_some((cursor.point.column.0, row as usize));
        Frame { lines, cursor }
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    fn stroke(text: &'static str) -> Keystroke {
        Keystroke::parse(text).unwrap_or_else(|_| panic!("{text} is a keystroke"))
    }

    #[gpui_kit::test]
    fn an_attached_pty_draws_exact_bytes_and_only_its_owner_can_write(cx: &mut TestAppContext) {
        let writes = Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = writes.clone();
        let terminal = cx.new(|cx| {
            Terminal::attached(
                Rc::new(move |op, args| seen.borrow_mut().push((op.to_string(), args))),
                cx,
            )
        });
        terminal.update(cx, |terminal, cx| {
            terminal.feed(b"\x1b[31mred\x1b[0m", false, false, cx);
            terminal.write(b"refused".to_vec());
            terminal.resize(90, 25);
            assert!(terminal.shell.is_none());
            assert!(writes.borrow().is_empty());
            terminal.feed(b"", true, false, cx);
            terminal.write(b"yes".to_vec());
            terminal.write("Néxus\r".as_bytes().to_vec());
            terminal.write(vec![3]);
            let writes = writes.borrow();
            let inputs: Vec<_> = writes
                .iter()
                .filter(|(op, _)| op == "input")
                .map(|(_, args)| args["data"].clone())
                .collect();
            assert_eq!(
                inputs,
                vec![
                    serde_json::json!("yes"),
                    serde_json::json!("Néxus\r"),
                    serde_json::json!("\u{3}")
                ]
            );
            assert!(
                writes
                    .iter()
                    .any(|(op, args)| op == "resize" && args["cols"] == 90 && args["rows"] == 25)
            );
            let term = terminal.term.lock().expect("grid");
            let text: String = term.renderable_content().display_iter.map(|cell| cell.cell.c).collect();
            assert!(text.contains("red"));
        });
    }

    #[gpui_kit::test]
    fn shared_display_moves_partial_parser_and_rebinds_input_without_a_pty(cx: &mut TestAppContext) {
        let old_calls = Rc::new(std::cell::RefCell::new(Vec::new()));
        let calls = old_calls.clone();
        let old = cx.new(|cx| Terminal::attached(Rc::new(move |op, _| calls.borrow_mut().push(op.to_string())), cx));
        let saved = old.update(cx, |view, cx| {
            view.feed(b"prefix\x1b[3", true, false, cx);
            view.retain_shared()
        });
        old_calls.borrow_mut().clear();
        old.update(cx, |view, cx| {
            view.write(b"old input".to_vec());
            view.resize(100, 40);
            view.start(cx);
            assert!(view.shell.is_none());
        });
        assert!(old_calls.borrow().is_empty());
        let new_calls = Rc::new(std::cell::RefCell::new(Vec::new()));
        let calls = new_calls.clone();
        let fresh = cx.new(|cx| {
            Terminal::restore_shared(
                saved,
                Rc::new(move |op, args| calls.borrow_mut().push((op.to_string(), args))),
                cx,
            )
        });
        fresh.update(cx, |view, cx| {
            assert!(view.feed(b"1mred\x1b[0m", true, false, cx));
            view.write(b"new input".to_vec());
            let term = view.term.lock().unwrap();
            let text: String = term.renderable_content().display_iter.map(|cell| cell.cell.c).collect();
            assert!(text.contains("prefixred"));
            assert!(!text.contains("1m"));
            let red = term
                .renderable_content()
                .display_iter
                .filter(|cell| cell.cell.c == 'r')
                .nth(1)
                .unwrap();
            assert_eq!(red.cell.fg, Color::Named(NamedColor::Red));
        });
        assert!(
            new_calls
                .borrow()
                .iter()
                .any(|(op, args)| op == "input" && args["data"] == "new input")
        );
        assert!(old_calls.borrow().is_empty());
    }

    #[test]
    fn a_server_shells_end_names_a_failure_and_a_finished_command() {
        assert_eq!(
            exit_note(Some(3), true).as_deref(),
            Some("It exited with code 3. Press Enter to start a shell.")
        );
        assert_eq!(
            exit_note(Some(0), true).as_deref(),
            Some("It finished. Press Enter to start a shell.")
        );
        // An interactive shell that ended well, or one closed on the server, keeps the usual note.
        assert_eq!(exit_note(Some(0), false), None);
        assert_eq!(exit_note(None, true), None);
    }

    #[test]
    fn keys_send_what_xterm_sends() {
        assert_eq!(key_bytes(&stroke("enter"), false), Some(b"\r".to_vec()));
        assert_eq!(key_bytes(&stroke("backspace"), false), Some(vec![0x7f]));
        assert_eq!(key_bytes(&stroke("ctrl-c"), false), Some(vec![3]));
        assert_eq!(key_bytes(&stroke("up"), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes(&stroke("up"), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_bytes(&stroke("ctrl-left"), false), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(key_bytes(&stroke("shift-tab"), false), Some(b"\x1b[Z".to_vec()));
        assert_eq!(key_bytes(&stroke("cmd-k"), false), None);
    }

    #[test]
    fn the_wheel_scrolls_the_scrollback_unless_a_program_takes_it() {
        let shell = TermMode::default();
        assert_eq!(wheel_action(shell, 3, false, (1, 1)), Wheel::Scroll(3));
        assert_eq!(wheel_action(shell, -2, false, (1, 1)), Wheel::Scroll(-2));
        // A full-screen program with no mouse reports (less) gets arrows, in its cursor mode.
        let less = shell | TermMode::ALT_SCREEN;
        assert_eq!(
            wheel_action(less, 2, false, (1, 1)),
            Wheel::Send(b"\x1b[A\x1b[A".to_vec())
        );
        assert_eq!(
            wheel_action(less | TermMode::APP_CURSOR, -1, false, (1, 1)),
            Wheel::Send(b"\x1bOB".to_vec())
        );
        // Alternate scroll turned off: the wheel does not type.
        assert_eq!(
            wheel_action(less - TermMode::ALTERNATE_SCROLL, 1, false, (1, 1)),
            Wheel::Scroll(1)
        );
        // A program that asked for mouse reports (Claude Code, htop) gets wheel reports, not arrows.
        let tui = less | TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(
            wheel_action(tui, 1, false, (5, 7)),
            Wheel::Send(b"\x1b[<64;5;7M".to_vec())
        );
        assert_eq!(
            wheel_action(tui, -2, false, (5, 7)),
            Wheel::Send(b"\x1b[<65;5;7M\x1b[<65;5;7M".to_vec())
        );
        assert_eq!(
            wheel_action(less | TermMode::MOUSE_DRAG, -1, false, (5, 300)),
            Wheel::Send(vec![0x1b, b'[', b'M', 32 + 65, 32 + 5, 32 + 223])
        );
        // Shift always scrolls the scrollback, as in xterm and Alacritty.
        assert_eq!(wheel_action(tui, 1, true, (5, 7)), Wheel::Scroll(1));
    }

    #[gpui_kit::test]
    fn the_wheel_scrolls_a_shells_scrollback(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|_, cx| {
            cx.set_global(crate::pages::Theme(Palette::for_choice(
                crate::palette::ThemeChoice::Dark,
                WindowAppearance::Dark,
            )));
            TerminalHost(cx.new(|cx| Terminal::attached(Rc::new(|_, _| {}), cx)))
        });
        let terminal = host.read_with(cx, |host, _| host.0.clone());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        terminal.update(cx, |terminal, cx| {
            let lines: String = (0..100).map(|n| format!("line {n}\r\n")).collect();
            terminal.feed(lines.as_bytes(), true, false, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let center = cx.debug_bounds("terminal-grid").expect("the grid drew").center();
        let offset = |cx: &mut VisualTestContext| {
            terminal.read_with(cx, |terminal, _| terminal.term.lock().unwrap().grid().display_offset())
        };
        assert_eq!(offset(cx), 0);
        cx.simulate_event(ScrollEvent {
            position: center,
            delta: ScrollDelta::Pixels(point(px(0.), px(LINE_HEIGHT * 5.))),
            ..Default::default()
        });
        assert_eq!(offset(cx), 5, "the wheel moved up through the scrollback");
    }

    /// A window that holds only a terminal.
    struct TerminalHost(Entity<Terminal>);

    impl Render for TerminalHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(600.)).h(px(300.)).child(self.0.clone())
        }
    }

    #[test]
    fn the_256_colors_follow_xterm() {
        let colors = TermColors::new(Palette::for_choice(
            crate::palette::ThemeChoice::Dark,
            WindowAppearance::Dark,
        ));
        assert_eq!(colors.indexed(16), rgb(0x000000).into());
        assert_eq!(colors.indexed(231), rgb(0xffffff).into());
        assert_eq!(colors.indexed(232), rgb(0x080808).into());
    }

    #[test]
    fn output_wakes_rebind_after_the_old_view_is_gone() {
        let wake = OutputWake::default();
        let (first, receiver) = unbounded();
        wake.bind(Some(first));
        drop(receiver);
        wake.notify();
        assert!(wake.0.lock().unwrap().is_none());
        let (second, mut receiver) = unbounded();
        wake.bind(Some(second));
        wake.notify();
        assert_eq!(receiver.try_recv(), Ok(()));
        wake.bind(None);
        wake.notify();
        assert!(receiver.try_recv().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn retained_shell_reads_without_a_view_and_ends_when_its_owner_is_dropped() {
        let dir = std::env::temp_dir();
        let (input, _) = channel();
        let term = Arc::new(Mutex::new(Term::new(
            Config::default(),
            &TermSize::new(80, 24),
            Listener::new(input),
        )));
        let wake = OutputWake::default();
        let (old_view, receiver) = unbounded();
        wake.bind(Some(old_view));
        drop(receiver);
        let (shell, ended) = spawn(&dir, Some("/bin/sh"), None, 80, 24, term.clone(), wake.clone()).unwrap();
        let saved = RetainedTerminal {
            dir,
            server: None,
            term: term.clone(),
            shell: Some(shell),
            ended: ended.clone(),
            error: None,
            status: Status::default(),
            size: (80, 24),
            wake: wake.clone(),
        };
        let text = || {
            term.lock()
                .unwrap()
                .renderable_content()
                .display_iter
                .map(|cell| cell.cell.c)
                .collect::<String>()
        };
        let wait_for = |marker: &str| {
            for _ in 0..100 {
                if text().contains(marker) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            panic!("missing {marker}: {}", text());
        };
        saved
            .shell
            .as_ref()
            .unwrap()
            .input
            .send(b"value=retained; printf 'first:%s\\n' $value\n".to_vec())
            .unwrap();
        wait_for("first:retained");
        assert!(!ended.load(Ordering::Relaxed));
        let (new_view, mut receiver) = unbounded();
        wake.bind(Some(new_view));
        saved
            .shell
            .as_ref()
            .unwrap()
            .input
            .send(b"printf 'second:%s\\n' $value\n".to_vec())
            .unwrap();
        wait_for("second:retained");
        let mut notified = false;
        for _ in 0..100 {
            if receiver.try_recv().is_ok() {
                notified = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(notified, "output must wake the replacement view");
        drop(saved);
        assert!(
            wake.0.lock().unwrap().is_none(),
            "the final owner must detach foreground notifications before teardown"
        );
        for _ in 0..100 {
            if ended.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            ended.load(Ordering::Relaxed),
            "dropping the final owner must end the PTY"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_shell_runs_in_the_folder_and_its_output_reaches_the_grid() {
        let dir = std::env::temp_dir().join(format!("nexus-gpui-term-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
        let dir = dir.canonicalize().expect("the test folder exists");
        let (input, _) = channel();
        let term = Arc::new(Mutex::new(Term::new(
            Config::default(),
            &TermSize::new(80, 24),
            Listener::new(input),
        )));
        let wake = OutputWake::default();
        let (shell, ended) =
            spawn(&dir, Some("/bin/sh"), None, 80, 24, term.clone(), wake).unwrap_or_else(|error| panic!("{error}"));
        let _ = shell.input.send(b"echo \"at:$(pwd)\"; exit\n".to_vec());
        for _ in 0..100 {
            if ended.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let term = term.lock().unwrap_or_else(|error| panic!("{error}"));
        let text: String = term.renderable_content().display_iter.map(|cell| cell.cell.c).collect();
        assert!(text.contains(&format!("at:{}", dir.display())), "{text}");
    }
}
