//! The Code session's Files tab, as the Nexus app's (`pane-files.tsx`): the session folder's
//! files as a tree, each changed one with its git status letter; a click opens it in the viewer (`file_view.rs`), a
//! preview of an image or Markdown file and read-only highlighted text otherwise. "Filter files" over the tree keeps
//! the paths that hold its text, in any case (`nexus_workbench::files::matches_filter`), with every folder open. A server's
//! session or project lists and reads them through the server's workbench (`server.rs`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::files;

pub(crate) use super::file_lifetime::Guard as FilesGuard;
use super::file_lifetime::{Guard, Lifetime};
use super::file_view::{FileView, FileViewEvent, FileViewState};
use super::{INSET, Place, field_fill, note};
use crate::file_icons::FileIcon;
use crate::palette::Palette;

const ROW: f32 = 26.;
/// The most files "Show all" lists: past it the tree would cost more to build on each click than it is worth.
const MAX_ALL: usize = 250_000;

/// One row of the tree as it shows: a folder or a file, at its depth.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    path: String,
    name: String,
    depth: usize,
    folder: bool,
    /// The file's git status letter, or a folder's mark for changes inside it.
    status: Option<char>,
}

/// Plain navigation state; no UI entities, callbacks, tasks or native handles.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct FilesState {
    pub query: String,
    pub all: bool,
    pub expanded: HashSet<String>,
    pub scroll_top: usize,
    pub open: Option<FileViewState>,
}

type Read = (Result<(Arc<Vec<String>>, bool), String>, HashMap<String, char>);
#[cfg(test)]
type ListReply = futures::channel::oneshot::Receiver<Read>;

pub struct Files {
    lifetime: Lifetime,
    #[cfg(test)]
    pending_list: Option<ListReply>,
    place: Place,
    files: Arc<Vec<String>>,
    truncated: bool,
    /// The person asked for every file, past [`files::MAX_FILES`] (up to [`MAX_ALL`]).
    all: bool,
    status: HashMap<String, char>,
    expanded: HashSet<String>,
    rows: Vec<Row>,
    loading: bool,
    error: Option<SharedString>,
    /// The file open in the viewer (`file_view.rs`), and what it asks of the tool.
    open: Option<(Entity<FileView>, Subscription)>,
    /// "Filter files", and what it holds.
    filter: Option<Entity<InputState>>,
    query: String,
    _filter: Option<Subscription>,
    scroll: UniformListScrollHandle,
    _load: Option<Task<()>>,
}

impl Files {
    pub fn new(place: Place, window: &mut Window, cx: &mut Context<Self>) -> Files {
        Self::new_guarded(place, Arc::new(|| true), window, cx)
    }

    pub(crate) fn new_guarded(place: Place, guard: Guard, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::restore(place, FilesState::default(), guard, window, cx)
    }

    pub(crate) fn restore(
        place: Place,
        state: FilesState,
        guard: Guard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut files = Self::unread(place, state, guard, window, cx);
        files.refresh(cx);
        files
    }
    fn unread(place: Place, state: FilesState, guard: Guard, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let lifetime = Lifetime::new(guard);
        let filter = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Filter files");
            input.set_value(state.query.clone(), window, cx);
            input
        });
        let events = cx.subscribe(&filter, |this: &mut Files, input, event: &InputEvent, cx| {
            if this.is_current() && matches!(event, InputEvent::Change) {
                this.query = input.read(cx).value().to_string();
                this.rebuild();
                cx.notify();
            }
        });
        let scroll = UniformListScrollHandle::new();
        scroll.scroll_to_item_strict(state.scroll_top, ScrollStrategy::Top);
        let mut files = Self {
            lifetime,
            #[cfg(test)]
            pending_list: None,
            place,
            files: Arc::default(),
            truncated: false,
            all: state.all,
            status: HashMap::new(),
            expanded: state.expanded,
            rows: Vec::new(),
            loading: false,
            error: None,
            open: None,
            filter: Some(filter),
            query: state.query,
            _filter: Some(events),
            scroll,
            _load: None,
        };
        if let Some(saved) = state.open {
            files.restore_file(saved, window, cx);
        }
        files
    }
    #[cfg(test)]
    pub(crate) fn fixture_list(
        place: Place,
        state: FilesState,
        guard: Guard,
        pending: ListReply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut files = Self::unread(place, state, guard, window, cx);
        files.pending_list = Some(pending);
        files.refresh(cx);
        files
    }

    #[cfg(test)]
    pub(crate) fn fixture_paths(&self) -> Option<&[String]> {
        if self.loading || self.error.is_some() || !self.is_current() {
            return None;
        }
        Some(self.files.as_slice())
    }
    /// The path of the file open in the viewer, as it was opened.
    #[cfg(test)]
    pub(crate) fn fixture_open_path(&self, cx: &App) -> Option<String> {
        Some(self.open.as_ref()?.0.read(cx).fixture_path().to_owned())
    }
    #[cfg(test)]
    pub(crate) fn fixture_preview_text(&self, cx: &App) -> Option<String> {
        if !self.is_current() {
            return None;
        }
        self.open.as_ref()?.0.read(cx).fixture_preview_text().map(str::to_owned)
    }

    #[cfg(test)]
    pub(crate) fn fixture_preview(&self) -> Option<Entity<FileView>> {
        self.open.as_ref().map(|(view, _)| view.clone())
    }

    pub(crate) fn is_current(&self) -> bool {
        self.lifetime.current()
    }
    pub(crate) fn is_retired(&self) -> bool {
        self.lifetime.retired()
    }
    pub(crate) fn snapshot(&self, cx: &App) -> FilesState {
        FilesState {
            query: self
                .filter
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_else(|| self.query.clone()),
            all: self.all,
            expanded: self.expanded.clone(),
            scroll_top: {
                let scroll = self.scroll.0.borrow();
                scroll
                    .deferred_scroll_to_item
                    .as_ref()
                    .map(|request| request.item_index)
                    .unwrap_or_else(|| scroll.base_handle.logical_scroll_top().0)
            },
            open: self.open.as_ref().map(|(view, _)| view.read(cx).snapshot()),
        }
    }
    pub(crate) fn retain(&mut self, cx: &mut Context<Self>) -> FilesState {
        let saved = self.snapshot(cx);
        self.retire(cx);
        saved
    }
    pub(crate) fn retire(&mut self, cx: &mut Context<Self>) {
        if self.is_retired() {
            return;
        }
        self.lifetime.retire();
        self._load = None;
        self._filter = None;
        self.filter = None;
        if let Some((view, _)) = self.open.take() {
            view.update(cx, |view, cx| view.retire(cx));
        }
        self.files = Arc::default();
        self.status.clear();
        self.rows.clear();
        self.error = None;
        self.loading = false;
        cx.notify();
    }

    /// The tool was shown: the tree's filter takes the keys, ready for typing. An open file keeps them.
    pub fn focus_filter(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_none()
            && let Some(filter) = &self.filter
        {
            filter.update(cx, |input, cx| input.focus(window, cx));
        }
    }

    /// Whether the filter has the keys, as the test reads it.
    #[cfg(test)]
    pub fn filter_focused(&self, window: &Window, cx: &App) -> bool {
        self.filter
            .as_ref()
            .is_some_and(|filter| filter.read(cx).focus_handle(cx).is_focused(window))
    }

    /// List the files and their git status again, off the UI thread.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        let permit = self.lifetime.next();
        let read = self.list_read(permit.guard(), cx);
        self.loading = true;
        self._load = Some(cx.spawn(async move |this, cx| {
            let (list, status) = read.await;
            let _ = this.update(cx, |this, cx| {
                if !permit.current() {
                    return;
                }
                this.loading = false;
                match list {
                    Ok((files, truncated)) => {
                        this.error = None;
                        this.files = files;
                        this.truncated = truncated;
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                this.status = status;
                this.rebuild();
                cx.notify();
            });
        }));
    }

    fn list_read(&mut self, valid: Guard, cx: &mut Context<Self>) -> Task<Read> {
        #[cfg(test)]
        if let Some(read) = self.pending_list.take() {
            return cx
                .background_executor()
                .spawn(async move { read.await.expect("fixture list reply must be completed") });
        }
        let all = self.all;
        match &self.place {
            Place::Here(Err(error)) => Task::ready((Err(error.clone()), HashMap::new())),
            Place::Here(Ok(folder)) => {
                let dir = folder.path.clone();
                cx.background_executor().spawn(async move {
                    if !valid() {
                        return (Err("This Files view is no longer active.".into()), HashMap::new());
                    }
                    let list = if all { list_all(&dir) } else { files::read(&dir) };
                    let list = list.map(|list| (list.files, list.truncated));
                    (list, git_status(&dir))
                })
            }
            Place::Server(server) => {
                let server = server.clone();
                cx.background_executor().spawn(async move {
                    if !valid() {
                        return (Err("This Files view is no longer active.".into()), HashMap::new());
                    }
                    let (list, status) = futures::join!(server.files(), server.status());
                    (list.map(|(files, truncated)| (Arc::new(files), truncated)), status)
                })
            }
        }
    }

    fn rebuild(&mut self) {
        self.rows = rows(&self.files, &self.expanded, &self.status, &self.query);
    }

    fn click(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        let Some(row) = self.rows.get(ix).cloned() else {
            return;
        };
        if row.folder {
            if !self.expanded.remove(&row.path) {
                self.expanded.insert(row.path);
            }
            self.rebuild();
        } else {
            self.open_file(row.path, window, cx);
        }
        cx.notify();
    }

    /// Open `path` in the viewer, read where the session runs.
    pub(crate) fn open_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_at(path, None, window, cx);
    }

    pub(crate) fn open_at(&mut self, path: String, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_file(
            FileViewState {
                path,
                line,
                source: line.is_some(),
                ..Default::default()
            },
            window,
            cx,
        );
    }

    fn restore_file(&mut self, state: FileViewState, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        if let Some((view, _)) = self.open.take() {
            view.update(cx, |view, cx| view.retire(cx));
        }
        let place = self.place.clone();
        let guard = self.lifetime.guard();
        let view = cx.new(|cx| FileView::restore(place, state, guard, window, cx));
        let events = cx.subscribe_in(&view, window, |this, view, event, window, cx| {
            if !this.is_current() || !view.read(cx).is_current() {
                return;
            }
            match event {
                FileViewEvent::Close => this.close_file(cx),
                FileViewEvent::Open(path) => {
                    this.open_file(path.clone(), window, cx);
                    cx.notify();
                }
            }
        });
        self.open = Some((view, events));
        cx.notify();
    }

    /// Whether the filter holds text: every folder shows open.
    fn filtering(&self) -> bool {
        !self.query.trim().is_empty()
    }

    fn close_file(&mut self, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        if let Some((view, _)) = self.open.take() {
            view.update(cx, |view, cx| view.retire(cx));
        }
        cx.notify();
    }

    fn show_all(&mut self, cx: &mut Context<Self>) {
        if !self.is_current() {
            return;
        }
        self.all = true;
        self.refresh(cx);
        cx.notify();
    }
}

/// Every file of `dir` up to [`MAX_ALL`], as [`files::read`] lists them (git's tracked and untracked-not-ignored, or a
/// walk that skips hidden folders and build output) but without its cap.
fn list_all(dir: &Path) -> Result<files::FileList, String> {
    const SKIPPED: &[&str] = &["node_modules", "target", "dist", "build", "__pycache__"];
    if !dir.is_dir() {
        return Err(format!("{} is not a folder.", dir.display()));
    }
    let git = Command::new("git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success());
    let mut files: Vec<String> = match git {
        Some(output) => String::from_utf8_lossy(&output.stdout)
            .split('\0')
            .filter(|path| !path.is_empty() && dir.join(path).is_file())
            .take(MAX_ALL + 1)
            .map(String::from)
            .collect(),
        None => {
            let mut files = Vec::new();
            let mut queue = std::collections::VecDeque::from([PathBuf::new()]);
            'walk: while let Some(relative) = queue.pop_front() {
                let Ok(entries) = std::fs::read_dir(dir.join(&relative)) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let Ok(kind) = entry.file_type() else { continue };
                    let path = relative.join(&name);
                    if kind.is_dir() {
                        if !name.starts_with('.') && !SKIPPED.contains(&name.as_str()) {
                            queue.push_back(path);
                        }
                    } else if kind.is_file() {
                        files.push(path.to_string_lossy().replace('\\', "/"));
                        if files.len() > MAX_ALL {
                            break 'walk;
                        }
                    }
                }
            }
            files
        }
    };
    let truncated = files.len() > MAX_ALL;
    files.truncate(MAX_ALL);
    files.sort();
    files.dedup();
    Ok(files::FileList {
        files: Arc::new(files),
        truncated,
    })
}

/// The tree's rows: folders first, then files, each sorted by name, inside the expanded folders only. A filter keeps
/// the files whose path holds it, and shows every folder open, as the Nexus app does.
fn rows(files: &[String], expanded: &HashSet<String>, status: &HashMap<String, char>, filter: &str) -> Vec<Row> {
    let filtering = !filter.trim().is_empty();
    #[derive(Default)]
    struct Node {
        folders: BTreeMap<String, Node>,
        files: Vec<String>,
    }
    let mut root = Node::default();
    for file in files.iter().filter(|file| files::matches_filter(file, filter)) {
        let mut node = &mut root;
        let mut parts = file.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                node.files.push(part.to_string());
            } else {
                node = node.folders.entry(part.to_string()).or_default();
            }
        }
    }
    let changed_inside = |folder: &str| {
        let prefix = format!("{folder}/");
        status.keys().any(|path| path.starts_with(&prefix))
    };
    fn walk(
        node: &Node,
        prefix: &str,
        depth: usize,
        expanded: &HashSet<String>,
        status: &HashMap<String, char>,
        changed_inside: &dyn Fn(&str) -> bool,
        filtering: bool,
        out: &mut Vec<Row>,
    ) {
        for (name, child) in &node.folders {
            let path = format!("{prefix}{name}");
            let open = filtering || expanded.contains(&path);
            out.push(Row {
                status: changed_inside(&path).then_some('•'),
                path: path.clone(),
                name: name.clone(),
                depth,
                folder: true,
            });
            if open {
                walk(
                    child,
                    &format!("{path}/"),
                    depth + 1,
                    expanded,
                    status,
                    changed_inside,
                    filtering,
                    out,
                );
            }
        }
        let mut names = node.files.clone();
        names.sort();
        for name in names {
            let path = format!("{prefix}{name}");
            out.push(Row {
                status: status.get(&path).copied(),
                path,
                name,
                depth,
                folder: false,
            });
        }
    }
    let mut out = Vec::new();
    walk(&root, "", 0, expanded, status, &changed_inside, filtering, &mut out);
    out
}

/// Each changed file's status letter, as the Nexus app shows it: M modified, A added, D deleted, R renamed, U
/// untracked. Empty outside a repository.
fn git_status(dir: &Path) -> HashMap<String, char> {
    let git = |args: &[&str]| {
        let mut command = Command::new("git");
        command.args(args).current_dir(dir).env("GIT_OPTIONAL_LOCKS", "0");
        // A shipped Windows build has no console, and git would open one of its own on each call.
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
        command
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    // Git names files from the repository's root; the tree, from the folder, which can be inside it.
    let Some(prefix) = git(&["rev-parse", "--show-prefix"]) else {
        return HashMap::new();
    };
    let porcelain = git(&["status", "--porcelain=v1", "-z", "--untracked-files=all", "--", "."]).unwrap_or_default();
    parse_status(&porcelain, prefix.trim())
}

fn parse_status(porcelain: &str, prefix: &str) -> HashMap<String, char> {
    let mut status = HashMap::new();
    let mut entries = porcelain.split('\0');
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        let mut letters = code.chars();
        let (x, y) = (letters.next().unwrap_or(' '), letters.next().unwrap_or(' '));
        let letter = match (x, y) {
            ('?', '?') => 'U',
            ('R', _) | (_, 'R') => {
                // A rename's old path follows as its own entry.
                entries.next();
                'R'
            }
            ('A', _) => 'A',
            ('D', _) | (_, 'D') => 'D',
            _ => 'M',
        };
        if let Some(path) = path.strip_prefix(prefix) {
            status.insert(path.to_string(), letter);
        }
    }
    status
}

/// 25000 as "25,000".
pub(crate) fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The highlighter's language for a file, from its extension (or its name, for a Makefile or a Dockerfile).
pub(super) fn language_of(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let extension = name.rsplit_once('.').map(|(_, extension)| extension).unwrap_or(name);
    match extension.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "json" | "jsonc" => "json",
        "md" | "mdx" => "markdown",
        "py" => "python",
        "go" => "go",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "css" | "scss" => "css",
        "html" | "htm" => "html",
        "sh" | "bash" | "zsh" => "bash",
        "sql" => "sql",
        "c" | "h" => "c",
        "cpp" | "cc" | "hpp" => "cpp",
        "java" => "java",
        "rb" => "ruby",
        "swift" => "swift",
        "kt" => "kotlin",
        "makefile" => "make",
        _ => "text",
    }
}

fn status_color(letter: char, palette: Palette) -> Hsla {
    match letter {
        'A' | 'U' => rgb(0x3a8f3e).into(),
        'D' => palette.destructive,
        'M' | 'R' => rgb(0xb07d10).into(),
        _ => palette.faint,
    }
}

impl Render for Files {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<crate::pages::Theme>().0;
        if !self.is_current() {
            self.retire(cx);
            return note("This Files view is no longer active.", palette).into_any_element();
        }
        if let Some((view, _)) = &self.open {
            return view.clone().into_any_element();
        }
        if let Some(error) = self.error.clone() {
            return note(error, palette).into_any_element();
        }
        if self.files.is_empty() {
            return note(
                if self.loading {
                    "Reading the folder…"
                } else {
                    "This folder is empty."
                },
                palette,
            )
            .into_any_element();
        }
        let list = uniform_list(
            "files",
            self.rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|ix| this.rows.get(ix).cloned().map(|row| (ix, row)))
                    .map(|(ix, row)| {
                        let open = row.folder && (this.filtering() || this.expanded.contains(&row.path));
                        h_flex()
                            .id(("file-row", ix))
                            .w_full()
                            .h(px(ROW))
                            .pl(px(INSET - 4. + 14. * row.depth as f32))
                            .pr(px(INSET))
                            .gap(px(4.))
                            .text_size(px(13.))
                            .text_color(palette.foreground)
                            .cursor_pointer()
                            .hover(|style| style.bg(palette.hover))
                            .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
                            // A chevron says a folder opens; a file keeps its place so the names line up.
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(14.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(row.folder, |slot| {
                                        slot.child(
                                            Icon::new(if open {
                                                Lucide::ChevronDown
                                            } else {
                                                Lucide::ChevronRight
                                            })
                                            .size(px(12.))
                                            .text_color(palette.faint),
                                        )
                                    }),
                            )
                            // The file type's icon: a few lookups for each row on screen (`uniform_list` builds
                            // only those), cheaper than resolving every row of a large tree on each rebuild.
                            .child(
                                img(if row.folder {
                                    FileIcon::folder(&row.path, open)
                                } else {
                                    FileIcon::file(&row.path)
                                }
                                .asset(palette.dark))
                                .flex_none()
                                .size(px(14.))
                                .mr(px(2.)),
                            )
                            .child(div().flex_1().min_w(px(0.)).truncate().child(row.name.clone()))
                            .when_some(row.status, |row_el, letter| {
                                row_el.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(status_color(letter, palette))
                                        .child(letter.to_string()),
                                )
                            })
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h(px(0.))
        .py(px(4.));
        // The filter over the tree, as the Nexus app's `wb-files-filter`.
        let filter = h_flex()
            .flex_none()
            .mx(px(INSET))
            .mt(px(8.))
            .mb(px(4.))
            .h(px(30.))
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(8.))
            .bg(field_fill(palette))
            .text_size(px(13.))
            .child(Icon::new(Lucide::Search).size(px(14.)).text_color(palette.faint))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(Input::new(self.filter.as_ref().expect("current filter")).appearance(false)),
            );
        v_flex()
            .size_full()
            .child(filter)
            .child(list)
            .when(self.truncated, |view| {
                let all = self.all;
                view.child(
                    h_flex()
                        .flex_none()
                        .h(px(36.))
                        .pl(px(INSET))
                        .pr(px(INSET - 8.))
                        .gap(px(8.))
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(div().flex_1().min_w(px(0.)).truncate().child(if all {
                            format!(
                                "Showing the first {} files: the folder has more",
                                group_thousands(MAX_ALL)
                            )
                        } else {
                            format!("Showing the first {} files", group_thousands(files::MAX_FILES))
                        }))
                        .when(!all, |footer| {
                            footer.child(
                                div()
                                    .id("files-show-all")
                                    .flex_none()
                                    .h(px(24.))
                                    .px(px(8.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(6.))
                                    .text_color(palette.foreground)
                                    .font_weight(FontWeight::MEDIUM)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(palette.hover))
                                    .on_click(cx.listener(|this, _, _, cx| this.show_all(cx)))
                                    .child(if self.loading { "Reading…" } else { "Show all" }),
                            )
                        }),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn the_tree_shows_folders_first_and_only_open_ones_inside() {
        let files = vec!["src/main.rs".to_string(), "README.md".into(), "src/lib/a.rs".into()];
        let status = HashMap::from([("src/main.rs".to_string(), 'M')]);
        let closed = rows(&files, &HashSet::new(), &status, "");
        let names: Vec<&str> = closed.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["src", "README.md"]);
        assert_eq!(closed[0].status, Some('•'));
        let open = rows(&files, &HashSet::from(["src".to_string()]), &status, "");
        let names: Vec<(&str, usize)> = open.iter().map(|row| (row.name.as_str(), row.depth)).collect();
        assert_eq!(names, [("src", 0), ("lib", 1), ("main.rs", 1), ("README.md", 0)]);
        assert_eq!(open[2].status, Some('M'));
    }

    #[test]
    fn the_filter_keeps_matching_paths_with_their_folders_open() {
        let files = vec!["src/Store.ts".to_string(), "src/view.ts".into(), "README.md".into()];
        let kept = rows(&files, &HashSet::new(), &HashMap::new(), "store");
        let names: Vec<(&str, usize)> = kept.iter().map(|row| (row.name.as_str(), row.depth)).collect();
        assert_eq!(names, [("src", 0), ("Store.ts", 1)]);
        assert!(rows(&files, &HashSet::new(), &HashMap::new(), "nothing").is_empty());
    }

    #[test]
    fn git_status_letters() {
        let status = parse_status(
            " M app/a.rs\0?? app/b.rs\0A  app/c.rs\0 D app/d.rs\0R  app/new.rs\0app/old.rs\0",
            "app/",
        );
        assert_eq!(status.get("a.rs"), Some(&'M'));
        assert_eq!(status.get("b.rs"), Some(&'U'));
        assert_eq!(status.get("c.rs"), Some(&'A'));
        assert_eq!(status.get("d.rs"), Some(&'D'));
        assert_eq!(status.get("new.rs"), Some(&'R'));
        assert_eq!(status.get("old.rs"), None);
    }
}

#[cfg(test)]
mod lifecycle_tests;
