//! Ctrl+P (Cmd+P on macOS), the file picker, and Ctrl+Shift+F, project search, over a Code session's folder, as the
//! Nexus app's (`file-picker.tsx`). Both look like the search palette (`search.rs`) and open what
//! is picked in the Workbench's Files tool, at its line. The picker lists the folder's files (nexus-workbench
//! `files::list`, `wb_files` on a server) and ranks them as the "@" menu does (`project_files.rs`); search runs in the
//! shared core (`project_files::search`, `wb_file_search` on a server). With no session on screen, Ctrl+Shift+F filters
//! the sidebar instead (`sidebar_filter.rs`), as in the Nexus app.

use crate::workspace::glass::FloatingGround;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::project_files::{self, SearchMatch, SearchResult};

use super::project_files::{
    PICKER_LIMIT, group_matches, in_folder, in_name, match_ranges, match_span, offered, pick_files, picker_query,
};
use super::slash::mention::FileIndex;
use super::{View, Workspace};
use crate::file_icons::FileIcon;
use crate::motion;
use crate::palette::Palette;
use crate::side::Place;

/// How long typing must pause before a search runs (`SEARCH_DELAY_MS`).
const SEARCH_DELAY: Duration = Duration::from_millis(250);
const WIDTH: f32 = 640.;
const TOP: f32 = 96.;

/// Which of the two the overlay is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// Ctrl+P: a file by name.
    Files,
    /// Ctrl+Shift+F: the lines that hold the text.
    Search,
}

/// What the overlay asks of the workspace.
pub(super) enum PickerEvent {
    /// Open `path` (relative to the session's folder) at `line` (from 1) in the Files tool.
    Open {
        path: String,
        line: Option<u32>,
    },
    Close,
    /// Project search's text and Match case, kept for its next open, as an editor's search keeps its last query.
    Searched {
        text: String,
        case_sensitive: bool,
    },
}

/// One row the cursor reaches.
#[derive(Debug, Clone, PartialEq)]
enum Pick {
    File(String),
    Line { path: String, line: u32 },
}

pub(super) struct FilePicker {
    mode: Mode,
    place: Place,
    input: Entity<InputState>,
    text: String,
    case_sensitive: bool,
    /// The folder's files, and whether the list was cut; None while they are read.
    files: Option<Result<(Arc<FileIndex>, bool), String>>,
    /// The picker's rows for `text`.
    shown: Vec<String>,
    /// The last search's answer and the key (case and text) it is for.
    found: Option<(String, Result<SearchResult, String>)>,
    cursor: usize,
    scroll: ScrollHandle,
    _input: Subscription,
    _work: Option<Task<()>>,
    _search: Option<Task<()>>,
}

impl EventEmitter<PickerEvent> for FilePicker {}

impl FilePicker {
    pub(super) fn new(
        mode: Mode,
        place: Place,
        (text, case_sensitive): (String, bool),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FilePicker {
        let placeholder = match mode {
            Mode::Files => "Find a file by name (add :42 for a line)…",
            Mode::Search => "Search the folder's files…",
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let text = if mode == Mode::Search { text } else { String::new() };
        input.update(cx, |state, cx| {
            state.set_value(text.clone(), window, cx);
            state.focus(window, cx);
        });
        let subscription = cx.subscribe_in(&input, window, |this, state, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                let text = state.read(cx).value().to_string();
                if text != this.text {
                    this.typed(text, cx);
                }
            }
        });
        let mut picker = FilePicker {
            mode,
            place,
            input,
            text,
            case_sensitive,
            files: None,
            shown: Vec::new(),
            found: None,
            cursor: 0,
            scroll: ScrollHandle::new(),
            _input: subscription,
            _work: None,
            _search: None,
        };
        match mode {
            Mode::Files => picker.list(cx),
            Mode::Search => picker.search(cx),
        }
        picker
    }

    /// Read the folder's files, off the UI thread: each open lists them again, so files an agent just wrote show.
    fn list(&mut self, cx: &mut Context<Self>) {
        let read: Task<Result<(Arc<Vec<String>>, bool), String>> = match &self.place {
            Place::Here(Err(error)) => Task::ready(Err(error.clone())),
            Place::Here(Ok(folder)) => {
                let dir = folder.path.clone();
                cx.background_executor()
                    .spawn(async move { nexus_workbench::files::list(&dir).map(|list| (list.files, list.truncated)) })
            }
            Place::Server(server) => {
                let server = server.clone();
                cx.background_executor().spawn(async move {
                    server
                        .files()
                        .await
                        .map(|(files, truncated)| (Arc::new(files), truncated))
                })
            }
        };
        self._work = Some(cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |this, cx| {
                this.files = Some(read.map(|(files, truncated)| (Arc::new(FileIndex::new(files)), truncated)));
                this.rank();
                cx.notify();
            });
        }));
    }

    fn rank(&mut self) {
        self.shown = match &self.files {
            Some(Ok((index, _))) => pick_files(index, &self.text, PICKER_LIMIT),
            _ => Vec::new(),
        };
        self.cursor = 0;
        self.scroll.scroll_to_item(0);
    }

    fn key(&self) -> String {
        format!("{}\n{}", self.case_sensitive, self.text)
    }

    /// Search once typing pauses; nothing typed shows nothing.
    fn search(&mut self, cx: &mut Context<Self>) {
        self.cursor = 0;
        self.scroll.scroll_to_item(0);
        cx.emit(PickerEvent::Searched {
            text: self.text.clone(),
            case_sensitive: self.case_sensitive,
        });
        if self.text.trim().is_empty() {
            self._search = None;
            return;
        }
        let key = self.key();
        let (query, case_sensitive) = (self.text.clone(), self.case_sensitive);
        let place = self.place.clone();
        self._search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            let found = match place {
                Place::Here(Err(error)) => Err(error),
                Place::Here(Ok(folder)) => {
                    cx.background_executor()
                        .spawn(async move { project_files::search(&folder.path, &query, case_sensitive) })
                        .await
                }
                Place::Server(server) => server.search(&query, case_sensitive).await,
            };
            let _ = this.update(cx, |this, cx| {
                this.found = Some((key, found));
                this.cursor = 0;
                cx.notify();
            });
        }));
    }

    fn typed(&mut self, text: String, cx: &mut Context<Self>) {
        self.text = text;
        match self.mode {
            Mode::Files => self.rank(),
            Mode::Search => self.search(cx),
        }
        cx.notify();
    }

    fn toggle_case(&mut self, cx: &mut Context<Self>) {
        self.case_sensitive = !self.case_sensitive;
        self.search(cx);
        cx.notify();
    }

    /// The search answer for what is typed now, if it came.
    fn current(&self) -> Option<&Result<SearchResult, String>> {
        self.found
            .as_ref()
            .filter(|(key, _)| *key == self.key())
            .map(|(_, found)| found)
    }

    /// The rows the cursor reaches, in order.
    fn picks(&self) -> Vec<Pick> {
        match self.mode {
            Mode::Files => self.shown.iter().cloned().map(Pick::File).collect(),
            Mode::Search => match self.current() {
                Some(Ok(result)) => group_matches(&result.matches)
                    .into_iter()
                    .flat_map(|(path, found)| {
                        found.into_iter().map(move |hit| Pick::Line {
                            path: path.clone(),
                            line: hit.line,
                        })
                    })
                    .collect(),
                _ => Vec::new(),
            },
        }
    }

    fn open(&mut self, pick: Pick, cx: &mut Context<Self>) {
        let (path, line) = match pick {
            Pick::File(path) => (path, picker_query(&self.text).1),
            Pick::Line { path, line } => (path, Some(line)),
        };
        cx.emit(PickerEvent::Open { path, line });
    }

    /// Up, Down, Enter and Esc while the overlay is open; whether the key was its.
    pub(super) fn key_down(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let picks = self.picks();
        let count = picks.len();
        match key {
            "escape" => cx.emit(PickerEvent::Close),
            "up" if count > 0 => self.cursor = (self.cursor + count - 1) % count,
            "down" if count > 0 => self.cursor = (self.cursor + 1) % count,
            "enter" => {
                if let Some(pick) = picks.into_iter().nth(self.cursor) {
                    self.open(pick, cx);
                }
                return true;
            }
            "up" | "down" => return true,
            _ => return false,
        }
        self.scroll.scroll_to_item(self.child_of(self.cursor));
        cx.notify();
        true
    }

    pub(super) fn mode(&self) -> Mode {
        self.mode
    }
}

/// `text` with `ranges` (bytes) marked, as the Nexus app's `<mark>` in the picker (`file-view.css`).
fn marked(text: String, ranges: &[Range<usize>], palette: Palette) -> StyledText {
    let mark = HighlightStyle {
        background_color: Some(palette.selected),
        font_weight: Some(FontWeight::MEDIUM),
        ..Default::default()
    };
    let highlights: Vec<(Range<usize>, HighlightStyle)> = ranges
        .iter()
        .filter(|range| range.start < range.end && range.end <= text.len())
        .filter(|range| text.is_char_boundary(range.start) && text.is_char_boundary(range.end))
        .map(|range| (range.clone(), mark))
        .collect();
    StyledText::new(text).with_highlights(highlights)
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

impl FilePicker {
    fn row(&self, ix: usize, selected: bool, palette: Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        h_flex()
            .id(("file-pick", ix))
            .min_h(px(36.))
            .px(px(12.))
            .py(px(6.))
            .gap(px(10.))
            .rounded(px(6.))
            .text_size(px(13.))
            .cursor_pointer()
            .when(selected, |row| row.bg(palette.selected))
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.cursor != ix {
                    this.cursor = ix;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(pick) = this.picks().into_iter().nth(ix) {
                    this.open(pick, cx);
                }
            }))
    }

    fn empty(text: impl Into<SharedString>, palette: Palette) -> Div {
        v_flex()
            .py(px(28.))
            .px(px(12.))
            .items_center()
            .text_size(px(13.))
            .text_color(palette.muted)
            .child(text.into())
    }

    fn file_rows(&self, palette: Palette, cx: &mut Context<Self>) -> (Vec<AnyElement>, Option<String>) {
        let mut rows = Vec::new();
        let foot = match &self.files {
            None => {
                rows.push(Self::empty("Listing the folder's files…", palette).into_any_element());
                return (rows, None);
            }
            Some(Err(error)) => {
                rows.push(Self::empty(error.clone(), palette).into_any_element());
                return (rows, None);
            }
            Some(Ok((_, truncated))) => truncated.then(|| {
                format!(
                    "The folder has more files than the picker lists: only the {} nearest its top.",
                    crate::side::files::group_thousands(nexus_workbench::files::MAX_FILES)
                )
            }),
        };
        if self.shown.is_empty() {
            rows.push(Self::empty("No file matches.", palette).into_any_element());
        }
        for (ix, path) in self.shown.iter().enumerate() {
            let selected = ix == self.cursor;
            let ranges = match_ranges(path, &self.text);
            let cut = path.rfind('/').map_or(0, |slash| slash + 1);
            let (folder, name) = path.split_at(cut);
            rows.push(
                self.row(ix, selected, palette, cx)
                    .child(img(FileIcon::file(path).asset(palette.dark)).flex_none().size(px(16.)))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(8.))
                            .items_baseline()
                            .child(div().flex_none().max_w(relative(0.7)).truncate().child(marked(
                                name.to_string(),
                                &in_name(&ranges, path),
                                palette,
                            )))
                            .when(!folder.is_empty(), |text| {
                                text.child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .truncate()
                                        .text_size(px(12.))
                                        .text_color(palette.faint)
                                        .child(marked(folder.to_string(), &in_folder(&ranges, path), palette)),
                                )
                            }),
                    )
                    .into_any_element(),
            );
        }
        (rows, foot)
    }

    fn search_rows(&self, palette: Palette, cx: &mut Context<Self>) -> (Vec<AnyElement>, Option<String>) {
        let mut rows = Vec::new();
        if self.text.trim().is_empty() {
            return (rows, None);
        }
        let result = match self.current() {
            None => {
                rows.push(Self::empty("Searching…", palette).into_any_element());
                return (rows, None);
            }
            Some(Err(error)) => {
                rows.push(Self::empty(error.clone(), palette).into_any_element());
                return (rows, None);
            }
            Some(Ok(result)) => result,
        };
        if result.matches.is_empty() {
            rows.push(Self::empty("Nothing in the folder's files matches.", palette).into_any_element());
            return (rows, None);
        }
        let groups = group_matches(&result.matches);
        let mut foot = format!(
            "{} in {}",
            plural(result.matches.len(), "line"),
            plural(groups.len(), "file")
        );
        if result.truncated {
            foot.push_str(". The search stopped early: make it narrower to see every match.");
        }
        let mut ix = 0;
        for (path, found) in groups {
            rows.push(
                div()
                    .pt(px(10.))
                    .px(px(12.))
                    .pb(px(4.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.faint)
                    .truncate()
                    .child(path)
                    .into_any_element(),
            );
            for hit in found {
                rows.push(self.hit_row(ix, &hit, palette, cx).into_any_element());
                ix += 1;
            }
        }
        (rows, Some(foot))
    }

    fn hit_row(&self, ix: usize, hit: &SearchMatch, palette: Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let text = hit.text.trim_end().to_string();
        let span = match_span(hit, &self.text, self.case_sensitive);
        self.row(ix, ix == self.cursor, palette, cx)
            .child(
                div()
                    .flex_none()
                    .min_w(px(24.))
                    .text_right()
                    .text_size(px(11.))
                    .text_color(palette.faint)
                    .child(hit.line.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_family("Geist Mono")
                    .text_size(px(12.))
                    .child(marked(text, span.as_slice(), palette)),
            )
    }

    /// The child each row the cursor reaches is, in the list (search's file headings are children too).
    fn child_of(&self, cursor: usize) -> usize {
        if self.mode == Mode::Files {
            return cursor;
        }
        let Some(Ok(result)) = self.current() else {
            return cursor;
        };
        let mut child = 0;
        let mut seen = 0;
        for (_, found) in group_matches(&result.matches) {
            child += 1;
            if cursor < seen + found.len() {
                return child + cursor - seen;
            }
            seen += found.len();
            child += found.len();
        }
        child
    }
}

impl Render for FilePicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<crate::pages::Theme>().0;
        let (rows, foot) = match self.mode {
            Mode::Files => self.file_rows(palette, cx),
            Mode::Search => self.search_rows(palette, cx),
        };
        let case = self.case_sensitive;
        let head = h_flex()
            .h(px(52.))
            .pl(px(16.))
            .pr(px(14.))
            .gap(px(10.))
            .child(
                Icon::new(match self.mode {
                    Mode::Files => Lucide::File,
                    Mode::Search => Lucide::Search,
                })
                .size(px(16.))
                .text_color(palette.icon),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(15.))
                    .child(Input::new(&self.input).appearance(false)),
            )
            .when(self.mode == Mode::Search, |head| {
                head.child(
                    div()
                        .id("file-search-case")
                        .size(px(26.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(case, |button| button.bg(palette.selected))
                        .hover(|button| button.bg(palette.hover))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Match case").build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_case(cx)))
                        .child(Icon::new(Lucide::CaseSensitive).size(px(16.)).text_color(if case {
                            palette.foreground
                        } else {
                            palette.icon
                        })),
                )
            })
            .child(
                div()
                    .id("file-picker-esc")
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(6.))
                    .bg(palette.hover)
                    .text_size(px(11.))
                    .text_color(palette.faint)
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.selected).text_color(palette.muted))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PickerEvent::Close)))
                    .child("esc"),
            );
        let dialog = v_flex()
            .id("file-picker")
            .occlude()
            .w(px(WIDTH))
            .max_w(relative(0.94))
            .max_h((window.viewport_size().height - px(TOP + 16.)).max(px(120.)))
            .rounded(px(14.))
            .floating_ground(14., palette.pop, false, &palette)
            .shadow(palette.pop_shadow(palette.border))
            .overflow_hidden()
            .text_color(palette.foreground)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(head)
            .child(
                div()
                    .id("file-picker-list")
                    .flex_1()
                    .min_h(px(0.))
                    .max_h(px(520.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(v_flex().px(px(6.)).pt(px(2.)).pb(px(8.)).children(rows)),
            )
            .when_some(foot, |dialog, foot| {
                dialog.child(
                    div()
                        .px(px(16.))
                        .py(px(10.))
                        .bg(palette.hover)
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(foot),
                )
            });
        div()
            .id("file-picker-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(palette.scrim)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(PickerEvent::Close)),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_center()
                    .pt(px(TOP))
                    .child(dialog.with_animation(
                        "file-picker-in",
                        Animation::new(motion::UI).with_easing(motion::ease_out),
                        |dialog, delta| dialog.opacity(delta).mt(px(-6. * (1. - delta))),
                    )),
            )
    }
}

/// The workspace's picker: the open one, and project search's last text and Match case.
#[derive(Default)]
pub(super) struct PickerState {
    /// The session it is for, the picker, and what it asks.
    open: Option<(String, Entity<FilePicker>, Subscription)>,
    search: (String, bool),
}

impl Workspace {
    /// The Code session on screen, if one is (not one being made).
    fn picker_session(&self) -> Option<String> {
        match &self.view {
            View::Code(view) if !view.is_draft() => Some(view.id.clone()),
            _ => None,
        }
    }

    /// Ctrl+P and Ctrl+Shift+F (Cmd on macOS), and the open overlay's own keys; whether the key was theirs.
    pub(super) fn file_picker_keystroke(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = &keystroke.modifiers;
        let chord = if cfg!(target_os = "macos") {
            modifiers.platform && !modifiers.control
        } else {
            modifiers.control && !modifiers.platform
        } && !modifiers.alt;
        let mode = match keystroke.key.as_str() {
            "p" if chord && !modifiers.shift => Some(Mode::Files),
            "f" if chord && modifiers.shift => Some(Mode::Search),
            _ => None,
        };
        if let Some(mode) = mode {
            // With no session on screen, Ctrl+Shift+F is the sidebar's filter; Ctrl+P is nothing.
            if self.picker_session().is_none() {
                return false;
            }
            self.open_file_picker(mode, window, cx);
            return true;
        }
        let Some((_, picker, _)) = &self.file_picker.open else {
            return false;
        };
        if modifiers.modified() {
            return false;
        }
        let picker = picker.clone();
        picker.update(cx, |picker, cx| picker.key_down(&keystroke.key, cx))
    }

    /// Open the file picker or project search over the session on screen, when its machine offers them.
    pub(super) fn open_file_picker(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.picker_session() else {
            return;
        };
        if self
            .file_picker
            .open
            .as_ref()
            .is_some_and(|(open, picker, _)| *open == id && picker.read(cx).mode() == mode)
        {
            return;
        }
        let machine = crate::machines::get().machine_of(&id);
        if !offered(machine.as_deref()) {
            let keys = if cfg!(target_os = "macos") {
                "⌘P and ⇧⌘F"
            } else {
                "Ctrl+P and Ctrl+Shift+F"
            };
            self.show_notice(
                format!("This machine's Nexus host cannot open or search files yet. Update it to use {keys}."),
                None,
                Duration::from_secs(6),
                cx,
            );
            return;
        }
        self.close_palette(window, cx);
        let place = self.session_place(&id);
        let last = self.file_picker.search.clone();
        let picker = cx.new(|cx| FilePicker::new(mode, place, last, window, cx));
        let events = cx.subscribe_in(
            &picker,
            window,
            |this, _, event: &PickerEvent, window, cx| match event {
                PickerEvent::Close => this.close_file_picker(window, cx),
                PickerEvent::Searched { text, case_sensitive } => {
                    this.file_picker.search = (text.clone(), *case_sensitive);
                }
                PickerEvent::Open { path, line } => {
                    let (path, line) = (path.clone(), *line);
                    this.close_file_picker(window, cx);
                    this.open_in_files(path, line, window, cx);
                }
            },
        );
        self.file_picker.open = Some((id, picker, events));
        cx.notify();
    }

    fn close_file_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_picker.open.take().is_some() {
            self.focus_page(window, cx);
            cx.notify();
        }
    }

    pub(super) fn file_picker_open(&self) -> bool {
        self.file_picker.open.is_some()
    }

    /// The overlay over the window, while open.
    pub(super) fn render_file_picker(&mut self) -> Option<AnyElement> {
        // A picker for a session no longer on screen closes.
        let session = self.picker_session();
        if self
            .file_picker
            .open
            .as_ref()
            .is_some_and(|(id, _, _)| session.as_ref() != Some(id))
        {
            self.file_picker.open = None;
        }
        let (_, picker, _) = self.file_picker.open.as_ref()?;
        Some(deferred(picker.clone()).with_priority(2).into_any_element())
    }
}
