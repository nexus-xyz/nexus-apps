//! Add a project's folder field: the path, the folders in it read from the chosen computer, and what the
//! path is. Its own view, so a keystroke redraws the field and its list, not the workspace.

use std::rc::Rc;

use futures::future::LocalBoxFuture;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::dirs::Dirs;

use super::browse::{self, Found};
use crate::palette::Palette;
use crate::workspace::glyph;

/// Reads the folders of a folder, as typed (`~/code/`), on the chosen computer.
pub(in crate::workspace) type Fetch = Rc<dyn Fn(String) -> LocalBoxFuture<'static, Result<Dirs, String>>>;

/// The rows the list shows before it scrolls.
const VISIBLE_ROWS: usize = 6;
const ROW: f32 = 32.;

pub(in crate::workspace) enum BrowserEvent {
    /// Enter in the field: add the path, or the highlighted folder.
    Submit,
}

/// What Add takes: the folder, and the project it already is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct Target {
    pub path: String,
    pub project: Option<String>,
}

pub(in crate::workspace) struct FolderBrowser {
    pub(in crate::workspace) input: Entity<InputState>,
    /// None: the computer cannot list its folders (an older host), and the field is a plain path.
    fetch: Option<Fetch>,
    /// The answer for a folder part, by that part as typed.
    listing: Option<(String, Result<Dirs, String>)>,
    /// The folder part being read now, and its read.
    reading: Option<String>,
    _read: Option<Task<()>>,
    /// The highlighted row among the matches; None: the path as typed.
    cursor: Option<usize>,
    pub(in crate::workspace) adding: bool,
    /// Why the last Add failed.
    pub(in crate::workspace) problem: Option<SharedString>,
    scroll: UniformListScrollHandle,
    _subscription: Subscription,
}

impl EventEmitter<BrowserEvent> for FolderBrowser {}

impl FolderBrowser {
    pub(in crate::workspace) fn new(
        start: String,
        fetch: Option<Fetch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("~/code/app");
            state.set_value(start, window, cx);
            state
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::PressEnter { .. } => cx.emit(BrowserEvent::Submit),
            InputEvent::Change => {
                this.problem = None;
                this.cursor = None;
                this.read(cx);
                cx.notify();
            }
            _ => {}
        });
        let mut browser = Self {
            input,
            fetch,
            listing: None,
            reading: None,
            _read: None,
            cursor: None,
            adding: false,
            problem: None,
            scroll: UniformListScrollHandle::new(),
            _subscription: subscription,
        };
        browser.read(cx);
        browser
    }

    fn typed(&self, cx: &App) -> (String, String) {
        browse::split(self.input.read(cx).value().as_ref())
    }

    /// Reads the typed path's folder part, unless it is read or being read. Typing a name filters what came back.
    fn read(&mut self, cx: &mut Context<Self>) {
        let Some(fetch) = self.fetch.clone() else {
            return;
        };
        let (dir, _) = self.typed(cx);
        if self.listing.as_ref().is_some_and(|(read, _)| *read == dir) || self.reading.as_ref() == Some(&dir) {
            return;
        }
        self.reading = Some(dir.clone());
        let answer = fetch(dir.clone());
        self._read = Some(cx.spawn(async move |this, cx| {
            let answer = answer.await;
            // An error here means the dialog closed while the computer answered: nothing to show it in.
            this.update(cx, |this, cx| {
                if this.reading.as_ref() != Some(&dir) {
                    return;
                }
                this.reading = None;
                this.cursor = None;
                match answer {
                    // A host older than `wb_dirs` (its feature not yet in the hello it sent): a plain path field.
                    Err(error) if error.starts_with("Unknown command") => this.fetch = None,
                    answer => {
                        let answer = answer.map_err(|error| crate::machine_view::plain_error(&error));
                        this.listing = Some((dir, answer));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The answer for the folder part typed now, if it came.
    fn listing(&self, cx: &App) -> Option<&Result<Dirs, String>> {
        let (dir, _) = self.typed(cx);
        self.listing
            .as_ref()
            .filter(|(read, _)| *read == dir)
            .map(|(_, listing)| listing)
    }

    /// The folders that match the name being typed.
    fn rows(&self, cx: &App) -> Vec<usize> {
        let (_, leaf) = self.typed(cx);
        match self.listing(cx) {
            Some(Ok(listing)) => browse::matches(listing, &leaf),
            _ => Vec::new(),
        }
    }

    /// ↑ and ↓: move the highlight through the rows, from the path as typed. False when there are none.
    pub(in crate::workspace) fn move_cursor(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let count = self.rows(cx).len();
        if count == 0 {
            return false;
        }
        let next = match (self.cursor, down) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(at), true) => (at + 1) % count,
            (Some(at), false) => (at + count - 1) % count,
        };
        self.cursor = Some(next);
        self.scroll.scroll_to_item(next, ScrollStrategy::Top);
        cx.notify();
        true
    }

    /// Tab: the path takes the highlighted folder, else the first match, and lists its folders. False when nothing
    /// matches, so Tab moves the focus on.
    pub(in crate::workspace) fn complete(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rows = self.rows(cx);
        let (dir, leaf) = self.typed(cx);
        if rows.is_empty() || (leaf.is_empty() && self.cursor.is_none()) {
            return false;
        }
        let index = rows[self.cursor.unwrap_or(0).min(rows.len() - 1)];
        self.enter(&dir, index, window, cx);
        true
    }

    /// The path becomes the folder at `index` of the listing, with a `/`, so its own folders list next.
    fn enter(&mut self, dir: &str, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Ok(listing)) = self.listing(cx) else {
            return;
        };
        let Some(folder) = listing.dirs.get(index) else {
            return;
        };
        let path = browse::complete(dir, folder);
        self.set_path(path, window, cx);
    }

    /// Puts `path` in the field (a folder entered, or picked with the system's picker) and reads its folder.
    pub(in crate::workspace) fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| {
            state.set_value(path, window, cx);
            state.focus(window, cx);
        });
        // `set_value` may not report a change: read the new folder here (a second read of it is skipped).
        self.cursor = None;
        self.problem = None;
        self.read(cx);
        cx.notify();
    }

    /// What Add takes now: the highlighted folder, else the path typed (absolute once its folder was read).
    pub(in crate::workspace) fn target(&self, cx: &App) -> Option<Target> {
        let typed = self.input.read(cx).value().trim().to_string();
        if typed.is_empty() {
            return None;
        }
        let (_, leaf) = self.typed(cx);
        let Some(Ok(listing)) = self.listing(cx) else {
            return Some(Target {
                path: typed,
                project: None,
            });
        };
        if let Some(folder) = self
            .cursor
            .and_then(|at| self.rows(cx).get(at).copied())
            .and_then(|index| listing.dirs.get(index))
        {
            return Some(Target {
                path: browse::join(&listing.path, &folder.name),
                project: folder.project_id.clone(),
            });
        }
        let project = match browse::found(&Ok(listing.clone()), &leaf) {
            Found::Folder { project, .. } => project,
            _ => None,
        };
        Some(Target {
            path: browse::join(&listing.path, &leaf),
            project,
        })
    }

    /// Add cannot succeed: the computer said the path's folder is missing or unreadable, or nothing there has the
    /// name typed.
    pub(in crate::workspace) fn blocked(&self, cx: &App) -> bool {
        self.path_problem(cx).is_some()
    }

    pub(super) fn path_problem(&self, cx: &App) -> Option<String> {
        match self.found(cx) {
            Found::Refused(reason) => Some(reason),
            Found::NoMatch => Some("No folder by that name here.".into()),
            _ => None,
        }
    }

    /// What the line under the field says.
    fn found(&self, cx: &App) -> Found {
        if let Some(folder) = self.cursor.and_then(|at| {
            let index = *self.rows(cx).get(at)?;
            match self.listing(cx)? {
                Ok(listing) => listing.dirs.get(index).cloned(),
                Err(_) => None,
            }
        }) {
            return Found::Folder {
                branch: folder.branch,
                project: folder.project_id,
                named: true,
            };
        }
        let (_, leaf) = self.typed(cx);
        self.listing(cx)
            .map_or(Found::Quiet, |listing| browse::found(listing, &leaf))
    }

    fn render_row(&self, index: usize, at: usize, dir: &str, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(Ok(listing)) = self.listing(cx) else {
            return div().into_any_element();
        };
        let Some(folder) = listing.dirs.get(index).cloned() else {
            return div().into_any_element();
        };
        let dir = dir.to_string();
        h_flex()
            .id(("project-folder-row", at))
            .w_full()
            .h(px(ROW))
            .px(px(8.))
            .gap(px(12.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(self.cursor == Some(at), |row| row.bg(palette.selected))
            .hover(|style| style.bg(palette.hover))
            .on_click(cx.listener(move |this, _, window, cx| this.enter(&dir, index, window, cx)))
            .child(glyph(Lucide::Folder, palette.icon))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_family(crate::pages::ui::MONO)
                    .text_size(px(13.))
                    .text_color(if folder.project_id.is_some() {
                        palette.muted
                    } else {
                        palette.foreground
                    })
                    .child(folder.name.clone()),
            )
            .map(|row| match (&folder.project_id, &folder.branch) {
                (Some(_), _) => row.child(
                    div()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child("Already a project"),
                ),
                (None, Some(branch)) => row.child(branch_badge(branch, palette)),
                (None, None) => row,
            })
            .into_any_element()
    }
}

/// A repository's branch, as a small outlined pill beside its folder.
fn branch_badge(branch: &str, palette: Palette) -> Div {
    h_flex()
        .flex_none()
        .h(px(20.))
        .px(px(8.))
        .gap(px(4.))
        .rounded_full()
        .border_1()
        .border_color(palette.border)
        .font_family(crate::pages::ui::MONO)
        .text_size(px(12.))
        .text_color(palette.muted)
        .child(glyph(Lucide::GitBranch, palette.icon).size(px(12.)))
        .child(div().max_w(px(140.)).truncate().child(branch.to_string()))
}

/// The line under the field: what the path is, or why it cannot be added.
fn found_line(found: &Found, problem: Option<&SharedString>, palette: Palette) -> Option<Div> {
    let line = |icon: Lucide, color: Hsla, text: String| {
        h_flex()
            .px(px(4.))
            .gap(px(8.))
            .text_size(px(13.))
            .text_color(color)
            .child(glyph(icon, color))
            .child(div().flex_1().min_w(px(0.)).child(text))
    };
    if let Some(problem) = problem {
        return Some(line(Lucide::TriangleAlert, palette.destructive, problem.to_string()));
    }
    Some(match found {
        Found::Quiet => return None,
        Found::Refused(error) => line(Lucide::TriangleAlert, palette.destructive, error.clone()),
        Found::NoMatch => line(
            Lucide::TriangleAlert,
            palette.destructive,
            "No folder by that name here.".into(),
        ),
        Found::Folder { project: Some(_), .. } => line(
            Lucide::Check,
            palette.muted,
            "Already a project. Open it to start a new session there.".into(),
        ),
        Found::Folder {
            branch: Some(branch), ..
        } => line(
            Lucide::GitBranch,
            palette.muted,
            format!("Git repository on branch {branch}."),
        ),
        Found::Folder { named: true, .. } => line(
            Lucide::Folder,
            palette.muted,
            "A folder, not a Git repository. Sessions still work in it.".into(),
        ),
        Found::Folder { named: false, .. } => return None,
    })
}

impl Render for FolderBrowser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx
            .try_global::<crate::pages::Theme>()
            .map_or_else(Palette::expected_at_launch, |theme| theme.0);
        let rows = self.rows(cx);
        let (dir, leaf) = self.typed(cx);
        let found = self.found(cx);
        // Tab has something to do: not when the only match is the name already typed.
        let completes = match rows.as_slice() {
            [] => false,
            [only] => {
                self.cursor.is_some()
                    || !matches!(self.listing(cx), Some(Ok(listing)) if listing.dirs.get(*only).is_some_and(|dir| dir.name == leaf))
            }
            _ => true,
        };
        let field = h_flex()
            .id("project-field-Folder")
            .debug_selector(|| "project-field-Folder".into())
            .h(px(40.))
            .px(px(12.))
            .gap(px(8.))
            .rounded(px(12.))
            .border_1()
            .border_color(match found {
                Found::Refused(_) | Found::NoMatch => palette.destructive,
                _ => palette.border,
            })
            .bg(palette.background)
            .child(glyph(Lucide::Folder, palette.icon))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .font_family(crate::pages::ui::MONO)
                    .text_size(px(13.))
                    .child(Input::new(&self.input).appearance(false).disabled(self.adding)),
            )
            .when(completes, |field| {
                field.child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child("Tab completes"),
                )
            });
        let list = (!rows.is_empty()).then(|| {
            let shown = rows.len().min(VISIBLE_ROWS);
            let dir_for_rows = dir.clone();
            v_flex()
                .gap(px(4.))
                .child(
                    div()
                        .px(px(4.))
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(format!("Folders in {dir}")),
                )
                .child(
                    uniform_list(
                        "project-folders",
                        rows.len(),
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            let rows = this.rows(cx);
                            range
                                .filter_map(|at| rows.get(at).map(|&index| (at, index)))
                                .map(|(at, index)| this.render_row(index, at, &dir_for_rows, palette, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .h(px(ROW * shown as f32))
                    .mx(px(-8.))
                    .track_scroll(&self.scroll),
                )
        });
        v_flex()
            .gap(px(8.))
            .child(field)
            .children(found_line(&found, self.problem.as_ref(), palette))
            .children(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use nexus_workbench::dirs::Dir;
    use std::cell::RefCell;

    fn dirs(path: &str, names: &[&str]) -> Dirs {
        Dirs {
            path: path.into(),
            branch: None,
            project_id: None,
            dirs: names
                .iter()
                .map(|name| Dir {
                    name: (*name).into(),
                    branch: None,
                    project_id: None,
                })
                .collect(),
            truncated: false,
        }
    }

    #[gpui_kit::test]
    fn typing_reads_each_folder_once_and_tab_enters_the_match(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let asked: Rc<RefCell<Vec<String>>> = Rc::default();
        let fetch: Fetch = {
            let asked = asked.clone();
            Rc::new(move |dir: String| {
                use futures::FutureExt as _;
                asked.borrow_mut().push(dir.clone());
                let listing = match dir.as_str() {
                    "~/" => Ok(dirs("/home/ada", &["code", "notes"])),
                    "~/code/" => Ok(dirs("/home/ada/code", &["nexus", "api"])),
                    _ => Err("No folder at that path.".into()),
                };
                futures::future::ready(listing).boxed_local()
            })
        };
        let window = cx.add_window(|window, cx| FolderBrowser::new("~/".into(), Some(fetch), window, cx));
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        let browser = window.root(cx).unwrap();
        cx.run_until_parked();
        browser.update_in(cx, |browser, window, cx| {
            browser
                .input
                .update(cx, |input, cx| input.set_value("~/co", window, cx));
            assert_eq!(
                browser.rows(cx).len(),
                1,
                "the typed name filters the folder already read"
            );
            assert!(browser.complete(window, cx));
            assert_eq!(browser.input.read(cx).value().as_ref(), "~/code/");
        });
        cx.run_until_parked();
        browser.update_in(cx, |browser, window, cx| {
            assert_eq!(*asked.borrow(), ["~/", "~/code/"]);
            browser
                .input
                .update(cx, |input, cx| input.set_value("~/code/nexsu", window, cx));
            assert!(browser.blocked(cx), "a name nothing matches cannot be added");
            browser
                .input
                .update(cx, |input, cx| input.set_value("~/code/", window, cx));
            assert!(!browser.blocked(cx));
            assert!(browser.move_cursor(true, cx));
            assert_eq!(
                browser.target(cx),
                Some(Target {
                    path: "/home/ada/code/nexus".into(),
                    project: None
                })
            );
        });
    }

    #[gpui_kit::test]
    fn without_a_listing_the_field_adds_the_path_as_typed(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.add_window(|window, cx| FolderBrowser::new("~/code/app".into(), None, window, cx));
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        let browser = window.root(cx).unwrap();
        browser.update_in(cx, |browser, window, cx| {
            assert!(!browser.complete(window, cx), "Tab moves on when nothing is listed");
            assert_eq!(
                browser.target(cx),
                Some(Target {
                    path: "~/code/app".into(),
                    project: None
                })
            );
        });
    }
}
