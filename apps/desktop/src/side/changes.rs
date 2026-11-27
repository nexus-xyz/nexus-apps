//! The Code session's Changes tab, as the Nexus app's (`changes-view.tsx`): what the session
//! changed (from its checkpoints) or one of its turns, the branch against its base branch, what is not committed yet,
//! or one of the branch's newest commits, as a diff; then a commit box. The scopes' rules and reads are
//! `nexus_workbench::changes_source`'s and `diff_source`'s, shared with the Nexus app. The diff is read from git
//! directly, so it shows while the Nexus app runs the session too. A server's session reads it from the server's
//! workbench (`server.rs`). The views of the folder as it is now reload shortly after the agent writes a file. Committing
//! needs this build to run the sessions, or the session to be on a server.
//!
//! In a session, selected diff lines open a comment box.
//! Comments join the composer draft through `nexus_workbench::diff_comment`.
//!
//! Find (Ctrl+F, Cmd+F on macOS, while the view has focus) is the Nexus app's (`changes-find.ts`,
//! `changes-find-view.ts`): a bar over the diff, "3 of 12", Enter and Shift+Enter step, Esc closes. It finds the text in
//! the parsed lines (`nexus_workbench::find::changes_matches`), so a folded file is found too: a step opens its file
//! and scrolls to its line. Unlike the Nexus app, which marks the text itself, the diff here marks each matching line,
//! and the line of the match shown more strongly.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::diff::{
    Diff, DiffAnnotation, DiffDecoration, DiffFile, DiffLinePosition, DiffLineRange, DiffParseOptions, DiffSide,
    DiffState,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::changes_source::{self, ChangesSource as Source};
use nexus_workbench::diff_comment::{self, DiffComment, DiffLine};
use nexus_workbench::diff_source::{self, COMMIT_LIMIT, DiffTargets};
use nexus_workbench::find::{ChangesMatch, changes_matches};
use nexus_workbench::{GitState, git};

use super::{INSET, Place, empty_state, error_note, field_fill};
use crate::code::SessionFolder;
use crate::palette::Palette;
use crate::workspace::find_bar::{Bar, find_bar, mark_colors, step};

actions!(changes, [FindInChanges]);

/// Quiet time after a file edit before the view reloads, as the Nexus app's (`EDIT_DEBOUNCE_MS`).
const EDIT_DEBOUNCE: Duration = Duration::from_millis(600);

/// The Changes tab asks the workspace to commit, draft a message, or put a diff comment in the composer.
pub enum ChangesEvent {
    Commit(String),
    WriteIt,
    Comment(DiffComment),
    /// The person scrolled, clicked or typed in the diff, or picked a view: following the agent's edits pauses
    /// (`workspace/side_pane/follow_edits.rs`).
    Touched,
    /// A setup line's fix to run in the window ("Run git init"): the workspace runs it (`setup::Fixer`).
    Fix(crate::setup::FixKey, crate::setup::Fix),
}

/// "Run git init"'s setup line, by its key.
const GIT_INIT: &str = "changes:git-init";

/// The comment box under picked lines: the lines, and what is typed.
struct CommentBox {
    range: DiffLineRange,
    /// Keep the quote stable when the agent edits and the diff reloads.
    lines: Vec<DiffLine>,
    place: Option<String>,
    input: Entity<InputState>,
    _submit: Subscription,
}

/// Find in the diff: the field, the text, every match and the one shown.
#[derive(Default)]
struct Find {
    open: bool,
    input: Option<(Entity<InputState>, Subscription)>,
    query: String,
    matches: Vec<ChangesMatch>,
    shown: Option<ChangesMatch>,
}

impl Find {
    /// "3 of 12", "No matches", or nothing with no text, as the Nexus app counts (`countLabel`).
    fn label(&self) -> String {
        count_label(&self.query, self.at(), self.matches.len())
    }

    fn at(&self) -> Option<usize> {
        let shown = self.shown?;
        self.matches.iter().position(|found| *found == shown)
    }
}

fn count_label(query: &str, at: Option<usize>, count: usize) -> String {
    if query.is_empty() {
        String::new()
    } else if count == 0 {
        "No matches".into()
    } else {
        format!("{} of {count}", at.map_or(0, |at| at + 1))
    }
}

/// Where a diff line is drawn: removed lines by the old file's number, the others by the new file's.
fn position_of(line: &DiffLine) -> Option<(DiffSide, usize)> {
    match line.sign {
        '-' => Some((DiffSide::Original, line.old?)),
        _ => Some((DiffSide::Modified, line.new?)),
    }
}

/// One read: the diff, what a commit would take, the turns with changes and the picker's base branch and commits.
struct Read {
    diff: Result<(String, bool), String>,
    uncommitted: Option<git::Stat>,
    turns: Vec<i64>,
    targets: Option<DiffTargets>,
}

pub struct Changes {
    /// The session, for its checkpoints; `None` for a project's folder outside a session, which has no session views.
    thread_id: Option<String>,
    /// Its folder here, or its server.
    place: Place,
    source: Source,
    /// The session's turns with changes, by their end events, oldest first.
    turns: Vec<i64>,
    /// The session's turn ends from its first event, to number the turns from its start; None while its transcript is
    /// not all read ([`turn_number`]).
    ends: Option<Vec<i64>>,
    /// The base branch and the newest commits, for the picker.
    targets: Option<DiffTargets>,
    /// The picker's menu, while open.
    picking: bool,
    diff: Entity<DiffState>,
    /// Each file's lines, by path, for a comment's quote and range.
    lines: Vec<(SharedString, Vec<DiffLine>)>,
    /// The comment box, while open.
    comment: Option<CommentBox>,
    files: usize,
    truncated: bool,
    loading: bool,
    error: Option<SharedString>,
    /// The folder is not in a git repository: no sources to pick, no commit box, just the empty state.
    not_repo: bool,
    /// Whether a read has come back: until then nothing shows, so the git bars don't flash outside a repository.
    read_once: bool,
    /// What a commit would take: files, lines added and removed.
    uncommitted: Option<git::Stat>,
    message: Entity<InputState>,
    /// Whether this build runs the sessions, so it can commit.
    can_commit: bool,
    write_refused: Option<&'static str>,
    committing: bool,
    /// "Write it" is drafting the message.
    drafting: bool,
    commit_note: Option<SharedString>,
    _draft: Option<Task<()>>,
    _load: Option<Task<()>>,
    /// The reload after the agent's last file edit, while it waits.
    _edited: Option<Task<()>>,
    /// The file to scroll to once the next read lands (`focus`).
    focus: Option<String>,
    /// The file the agent edited, as its tool named it, to scroll to once the diff has it (`follow_file`).
    follow: Option<String>,
    find: Find,
}

impl EventEmitter<ChangesEvent> for Changes {}

impl Changes {
    pub fn new(
        thread_id: Option<String>,
        place: Place,
        can_commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Changes {
        let mut changes = Changes {
            source: if thread_id.is_some() {
                Source::Session
            } else {
                Source::Uncommitted
            },
            thread_id,
            place,
            turns: Vec::new(),
            ends: None,
            targets: None,
            picking: false,
            diff: cx.new(|cx| DiffState::new(Vec::new(), cx)),
            lines: Vec::new(),
            comment: None,
            files: 0,
            truncated: false,
            loading: false,
            error: None,
            not_repo: false,
            read_once: false,
            uncommitted: None,
            message: cx.new(|cx| InputState::new(window, cx).placeholder("Commit message")),
            can_commit,
            write_refused: None,
            committing: false,
            drafting: false,
            commit_note: None,
            _draft: None,
            _load: None,
            _edited: None,
            focus: None,
            follow: None,
            find: Find::default(),
        };
        changes.refresh(cx);
        changes
    }

    pub fn set_can_commit(&mut self, can_commit: bool, cx: &mut Context<Self>) {
        if self.can_commit != can_commit {
            self.can_commit = can_commit;
            cx.notify();
        }
    }

    pub fn set_write_refusal(&mut self, reason: Option<&'static str>, cx: &mut Context<Self>) {
        if self.write_refused != reason {
            self.write_refused = reason;
            cx.notify();
        }
    }

    /// The session's turn ends, to number the turns ("Turn 3"): every one from its first event, or None while its
    /// transcript is not all read (a long session reads its newest events first).
    pub fn set_turn_ends(&mut self, ends: Option<Vec<i64>>, cx: &mut Context<Self>) {
        if self.ends != ends {
            self.ends = ends;
            cx.notify();
        }
    }

    /// The agent wrote a file: a view of the folder as it is now reloads once the agent pauses, as the Nexus app's.
    pub fn agent_edited(&mut self, cx: &mut Context<Self>) {
        if !self.source.follows_edits() {
            return;
        }
        self._edited = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EDIT_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.refresh(cx));
        }));
    }

    /// Show the file the agent edited (`path`, as its tool named it): the view moves to the whole session when it
    /// shows a fixed diff (a past turn, a commit), and scrolls to the file once a read has it.
    pub fn follow_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.follow = Some(path);
        if !self.source.follows_edits() && self.thread_id.is_some() {
            self.source = Source::Session;
            self.comment = None;
            self.refresh(cx);
        }
        self.reveal_followed(cx);
    }

    /// Whether a comment box is open: following the agent's edits waits while it is.
    pub fn commenting(&self) -> bool {
        self.comment.is_some()
    }

    /// The followed file, once the diff has it: its header scrolls to the top, opened.
    fn reveal_followed(&mut self, cx: &mut Context<Self>) {
        let Some(wanted) = self.follow.as_deref() else {
            return;
        };
        let Some(path) = diff_path_of(self.lines.iter().map(|(path, _)| path.as_ref()), wanted) else {
            return;
        };
        self.follow = None;
        self.diff.update(cx, |state, cx| {
            if state.is_file_collapsed(&path) {
                state.set_file_collapsed(&path, false, cx);
            }
            state.scroll_to_file(&path, cx);
        });
    }

    /// "Run git init" under the empty state of a folder on this computer that is not in a git repository: it runs in
    /// the Workbench terminal, where the person sees it, and the view reads the folder again once it exits 0. A failed
    /// run shows the red ✕ with its last line and Try again.
    fn render_git_init(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let Place::Here(Ok(folder)) = &self.place else {
            return None;
        };
        let step = crate::setup::TerminalStep {
            command: "git init".into(),
            cwd: Some(folder.path.clone()),
            name: Some("git init".into()),
        };
        let changes = cx.entity().downgrade();
        let fixer = crate::setup::Fixer::new(None, move |key, fix, _, cx| {
            // Once it exits 0, this view reads the folder again: Changes shows the new repository.
            if !crate::setup::state::waiting(&key, cx) {
                let refresh = changes.clone();
                crate::setup::on_done(key.clone(), cx, move |_, cx| {
                    if let Err(error) = refresh.update(cx, |changes, cx| changes.refresh(cx)) {
                        eprintln!("nexus-desktop-gpui: changes after git init: {error}");
                    }
                });
            }
            if let Err(error) = changes.update(cx, |_, cx| cx.emit(ChangesEvent::Fix(key, fix))) {
                eprintln!("nexus-desktop-gpui: git init: {error}");
            }
        });
        let line = crate::setup::SetupLine::new(GIT_INIT, Lucide::GitBranch, "This folder isn't a git repository.")
            .fix("Run git init", crate::setup::Fix::RunInTerminal(step));
        Some(div().w_full().max_w(px(380.)).child(line.render(&fixer, palette, cx)))
    }

    /// The person's own work in the view: a pending follow is dropped, and the workspace pauses following.
    fn touched(&mut self, cx: &mut Context<Self>) {
        self.follow = None;
        cx.emit(ChangesEvent::Touched);
    }

    fn pick(&mut self, source: Source, cx: &mut Context<Self>) {
        self.picking = false;
        if self.source != source {
            self.source = source;
            self.comment = None;
            self.diff.update(cx, |state, cx| state.set_selected_lines(None, cx));
            self.refresh(cx);
        }
        cx.notify();
    }

    /// Show `source`, scrolled to file `path` (relative to the folder) once it is read: a file chip at the top of an
    /// answer opens its turn on the file (`workspace/answer_links.rs`).
    pub fn focus(&mut self, source: Source, path: Option<String>, cx: &mut Context<Self>) {
        self.picking = false;
        self.source = source;
        self.follow = None;
        self.focus = path;
        self.refresh(cx);
    }

    /// Read the diff again, off the UI thread, with the turns and the picker's base branch and commits.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (thread_id, source) = (self.thread_id.clone(), self.source.clone());
        let read: Task<Option<Read>> = match &self.place {
            Place::Here(Err(error)) => {
                self.error = Some(error.clone().into());
                cx.notify();
                return;
            }
            Place::Here(Ok(folder)) => {
                let folder = folder.clone();
                cx.background_executor().spawn(async move {
                    if !git::in_repo(&folder.path) {
                        return None;
                    }
                    let dir = &folder.path;
                    Some(Read {
                        diff: read_diff(thread_id.as_deref(), &folder, &source),
                        uncommitted: git::uncommitted(dir).ok(),
                        turns: thread_id
                            .as_deref()
                            .map(|thread| changes_source::changed_turns(dir, thread))
                            .unwrap_or_default(),
                        targets: Some(DiffTargets {
                            base: diff_source::base_branch(dir, folder.base.as_deref()),
                            commits: diff_source::commits(dir, COMMIT_LIMIT).unwrap_or_default(),
                        }),
                    })
                })
            }
            Place::Server(server) => {
                let server = server.clone();
                cx.background_executor().spawn(async move {
                    let (diff, uncommitted, turns, targets) = futures::join!(
                        server.diff(&source),
                        server.uncommitted(),
                        server.turns(),
                        server.targets()
                    );
                    Some(Read {
                        diff,
                        uncommitted,
                        turns,
                        targets,
                    })
                })
            }
        };
        self.loading = true;
        cx.notify();
        self._load = Some(cx.spawn(async move |this, cx| {
            let read = read.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.read_once = true;
                this.not_repo = read.is_none();
                let Some(read) = read else {
                    this.error = None;
                    this.files = 0;
                    this.uncommitted = None;
                    this.diff.update(cx, |state, cx| state.set_files(Vec::new(), cx));
                    cx.notify();
                    return;
                };
                this.uncommitted = read.uncommitted;
                this.turns = read.turns;
                if read.targets.is_some() {
                    this.targets = read.targets;
                }
                match read.diff.and_then(|(patch, truncated)| {
                    parse_patch(&patch, truncated).map(|files| {
                        let lines = file_lines(&files, &patch);
                        (files, lines, truncated)
                    })
                }) {
                    Ok((files, lines, truncated)) => {
                        this.lines = lines;
                        this.error = None;
                        this.files = files.len();
                        this.truncated = truncated;
                        this.diff.update(cx, |state, cx| state.set_files(files, cx));
                        this.reveal_followed(cx);
                        if let Some(path) = this.focus.take() {
                            this.diff.update(cx, |state, cx| {
                                state.set_file_collapsed(&path, false, cx);
                                state.scroll_to_file(&path, cx);
                            });
                        }
                        this.refind(cx);
                    }
                    Err(error) => {
                        this.not_repo = error.contains("not a git repository");
                        this.error = Some(error.into());
                        this.files = 0;
                        this.diff.update(cx, |state, cx| state.set_files(Vec::new(), cx));
                    }
                }
                cx.notify();
            });
        }));
    }

    fn on_find(&mut self, _: &FindInChanges, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(window, cx);
    }

    /// Open find, or take the focus back to its field, with its text picked.
    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.input.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in changes"));
            let subscription = cx.subscribe_in(&input, window, |this, state, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    let query = state.read(cx).value().to_string();
                    if query != this.find.query {
                        this.find.query = query;
                        this.refind(cx);
                    }
                }
                InputEvent::PressEnter { shift, .. } => this.find_step(if *shift { -1 } else { 1 }, cx),
                _ => {}
            });
            self.find.input = Some((input, subscription));
        }
        self.find.open = true;
        if let Some((input, _)) = &self.find.input {
            input.update(cx, |state, cx| {
                state.select_all(window, cx);
                state.focus(window, cx);
            });
        }
        self.refind(cx);
    }

    /// Close find: its marks go, and the diff takes the focus back.
    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find.open {
            return;
        }
        self.find.open = false;
        self.find.matches.clear();
        self.find.shown = None;
        let focus = self.diff.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Find the text again in the lines read. The match shown stays when it is still there; else the first one shows.
    fn refind(&mut self, cx: &mut Context<Self>) {
        if !self.find.open {
            return;
        }
        self.find.matches = changes_matches(self.lines.iter().map(|(_, lines)| lines.as_slice()), &self.find.query);
        if self.find.at().is_none() {
            self.find.shown = self.find.matches.first().copied();
            self.reveal_found(cx);
        }
        cx.notify();
    }

    fn find_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.find.matches.is_empty() {
            return;
        }
        let at = step(self.find.at().unwrap_or(0), self.find.matches.len(), delta);
        self.find.shown = Some(self.find.matches[at]);
        self.reveal_found(cx);
        cx.notify();
    }

    /// The line of the match shown, with its file opened and its line drawn.
    fn place_of(&self, found: ChangesMatch) -> Option<DiffLinePosition> {
        let (path, lines) = self.lines.get(found.file)?;
        let (side, number) = position_of(lines.get(found.line)?)?;
        Some(DiffLinePosition::new(path.clone(), side, number))
    }

    fn reveal_found(&mut self, cx: &mut Context<Self>) {
        let Some(place) = self.find.shown.and_then(|found| self.place_of(found)) else {
            return;
        };
        self.diff.update(cx, |state, cx| state.scroll_to_line(place, cx));
    }

    /// Each matching line's mark, the line of the match shown in the stronger colour.
    fn find_marks(&self) -> Vec<DiffDecoration> {
        if !self.find.open {
            return Vec::new();
        }
        let (plain, strong) = mark_colors();
        let shown = self.find.shown.and_then(|found| self.place_of(found));
        let mut seen = std::collections::HashSet::new();
        self.find
            .matches
            .iter()
            .filter(|found| seen.insert((found.file, found.line)))
            .filter_map(|found| self.place_of(*found))
            .map(|place| {
                let color = if Some(&place) == shown.as_ref() { strong } else { plain };
                DiffDecoration::new(DiffLineRange::new(
                    place.path().clone(),
                    place.side(),
                    place.line(),
                    place.line(),
                ))
                .with_background(color)
            })
            .collect()
    }

    fn render_find(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        find_bar(
            Bar {
                input: self.find.input.as_ref().map(|(input, _)| input.clone()),
                label: self.find.label(),
                none: !self.find.query.is_empty() && self.find.matches.is_empty(),
                empty: self.find.matches.is_empty(),
                step: |this: &mut Changes, delta, cx| this.find_step(delta, cx),
                close: |this: &mut Changes, window, cx| this.close_find(window, cx),
                footer: None,
            },
            palette,
            cx,
        )
    }

    /// Open the comment box under `range`'s last line. Only a session's diff takes comments: they go to its agent.
    fn open_comment(&mut self, range: DiffLineRange, window: &mut Window, cx: &mut Context<Self>) {
        if self.thread_id.is_none() {
            return;
        }
        let lines = self.picked(&range).to_vec();
        if !lines.iter().any(diff_comment::pickable) {
            return;
        }
        let label = diff_comment::range_label(&lines);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(format!("Comment on {label}")));
        let submit = cx.subscribe_in(
            &input,
            window,
            |this: &mut Changes, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_comment(window, cx);
                }
            },
        );
        input.update(cx, |state, cx| state.focus(window, cx));
        self.comment = Some(CommentBox {
            range,
            lines,
            place: self.source.comment_place(),
            input,
            _submit: submit,
        });
        cx.notify();
    }

    /// The lines `range` picks, in its file's diff.
    fn picked(&self, range: &DiffLineRange) -> &[DiffLine] {
        let side = |side: DiffSide| match side {
            DiffSide::Original => diff_comment::Side::Old,
            DiffSide::Modified => diff_comment::Side::New,
        };
        self.lines
            .iter()
            .find(|(path, _)| path == range.path())
            .map(|(_, lines)| {
                diff_comment::lines_between(
                    lines,
                    side(range.side()),
                    range.start(),
                    side(range.end_side()),
                    range.end(),
                )
            })
            .unwrap_or_default()
    }

    /// "Add comment": the comment on the picked lines goes to the composer, and the box and the pick close.
    fn add_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &self.comment else {
            return;
        };
        let text = open.input.read(cx).value().to_string();
        let id = uuid::Uuid::new_v4().to_string();
        let Some(comment) =
            diff_comment::make_comment(open.range.path(), &open.lines, &text, &id, open.place.as_deref())
        else {
            return;
        };
        cx.emit(ChangesEvent::Comment(comment));
        self.cancel_comment(window, cx);
    }

    fn cancel_comment(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.comment = None;
        self.diff.update(cx, |state, cx| state.set_selected_lines(None, cx));
        cx.notify();
    }

    /// Where the comment box goes: under its last line.
    fn comment_at(&self) -> Option<DiffAnnotation> {
        let range = &self.comment.as_ref()?.range;
        Some(DiffAnnotation::line(
            "diff-comment",
            DiffLinePosition::new(range.path().clone(), range.end_side(), range.end()),
        ))
    }

    /// The comment box.
    fn comment_box(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(open) = self.comment.as_ref() else {
            return div().into_any_element();
        };
        let can = !open.input.read(cx).value().trim().is_empty();
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(26.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_size(px(12.5))
                .cursor_pointer()
                .when(primary, |button| {
                    button
                        .bg(palette.primary)
                        .text_color(palette.primary_foreground)
                        .when(!can, |button| button.opacity(0.4))
                })
                .when(!primary, |button| {
                    button
                        .border_1()
                        .border_color(palette.border)
                        .hover(|style| style.bg(palette.hover))
                })
                .child(label)
        };
        v_flex()
            .mx(px(12.))
            .mt(px(4.))
            .mb(px(6.))
            .p(px(8.))
            .gap(px(8.))
            .rounded(px(10.))
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border)
            .text_size(px(13.))
            .child(Input::new(&open.input).appearance(false))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child("Goes in your message as a chip"),
                    )
                    .child(
                        button("diff-comment-cancel", "Cancel", false)
                            .on_click(cx.listener(|this, _, window, cx| this.cancel_comment(window, cx))),
                    )
                    .child(button("diff-comment-add", "Add comment", true).when(can, |button| {
                        button.on_click(cx.listener(|this, _, window, cx| this.add_comment(window, cx)))
                    })),
            )
            .into_any_element()
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        if self.drafting || self.committing {
            return;
        }
        let message = self.message.read(cx).value().trim().to_string();
        if message.is_empty() {
            self.commit_note = Some("Write a commit message.".into());
            cx.notify();
            return;
        }
        self.committing = true;
        self.commit_note = None;
        cx.emit(ChangesEvent::Commit(message));
        cx.notify();
    }

    fn write_it(&mut self, cx: &mut Context<Self>) {
        if self.drafting || self.committing {
            return;
        }
        self.drafting = true;
        self.commit_note = None;
        cx.emit(ChangesEvent::WriteIt);
        cx.notify();
    }

    /// The workspace's draft of the message (None: it could not ask), into the message box; on an error the box stays
    /// as it was and the error shows under it.
    pub fn drafted(
        &mut self,
        work: Option<futures::future::LocalBoxFuture<'static, Result<nexus_workbench::draft::Draft, String>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(work) = work else {
            self.drafting = false;
            cx.notify();
            return;
        };
        let original = self.message.read(cx).value().to_string();
        self._draft = Some(cx.spawn_in(window, async move |this, cx| {
            let drafted = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.drafting = false;
                if this.message.read(cx).value().as_ref() != original {
                    cx.notify();
                    return;
                }
                match drafted {
                    Ok(draft) => this
                        .message
                        .update(cx, |input, cx| input.set_value(draft.title, window, cx)),
                    Err(error) => this.commit_note = Some(error.into()),
                }
                cx.notify();
            });
        }));
    }

    /// The workspace's answer to a commit: the folder's git state after it, or why it failed.
    pub fn committed(
        &mut self,
        answer: futures::channel::oneshot::Receiver<Result<GitState, String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._load = Some(cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.unwrap_or_else(|_| Err("The commit was cut off.".into()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.committing = false;
                match answer {
                    Ok(state) => {
                        this.commit_note = Some(match &state.branch {
                            Some(branch) => format!("Committed on {branch}.").into(),
                            None => "Committed.".into(),
                        });
                        this.message.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    Err(error) => this.commit_note = Some(error.into()),
                }
                this.refresh(cx);
            });
        }));
    }

    /// What the picker calls `source`, as the Nexus app's select does.
    fn label(&self, source: &Source) -> String {
        source_label(source, &self.turns, self.ends.as_deref(), self.targets.as_ref())
    }

    /// The picker's choices, in the Nexus app's order: the session's views (with a session), then the git ones, then
    /// the newest commits with the picked one kept when it is not among them. Group headings ride along as `None`.
    fn choices(&self) -> Vec<(Option<&'static str>, Source)> {
        let mut out = Vec::new();
        if self.thread_id.is_some() {
            out.push((None, Source::Session));
            out.extend(self.turns.iter().map(|seq| (None, Source::Turn(*seq))));
        }
        if self.targets.as_ref().is_some_and(|targets| targets.base.is_some()) {
            out.push((Some("Git"), Source::Branch));
            out.push((None, Source::Uncommitted));
        } else {
            out.push((Some("Git"), Source::Uncommitted));
        }
        let commits = self
            .targets
            .as_ref()
            .map(|targets| targets.commits.as_slice())
            .unwrap_or_default();
        let picked = match &self.source {
            Source::Commit(id) if !commits.iter().any(|commit| &commit.id == id) => Some(Source::Commit(id.clone())),
            _ => None,
        };
        let listed = picked
            .into_iter()
            .chain(commits.iter().map(|commit| Source::Commit(commit.id.clone())));
        for (n, source) in listed.enumerate() {
            out.push(((n == 0).then_some("Commits"), source));
        }
        out
    }

    fn render_picker(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let trigger = h_flex()
            .id("changes-source")
            .h(px(26.))
            .px(px(9.))
            .gap(px(4.))
            .rounded(px(6.))
            .text_size(px(12.5))
            .cursor_pointer()
            .text_color(palette.foreground)
            .when(self.picking, |trigger| trigger.bg(palette.selected))
            .when(!self.picking, |trigger| trigger.hover(|style| style.bg(palette.hover)))
            .child(div().max_w(px(240.)).truncate().child(self.label(&self.source)))
            .child(Icon::new(Lucide::ChevronDown).size(px(12.)).text_color(palette.muted))
            .on_click(cx.listener(|this, _, _, cx| {
                this.picking = !this.picking;
                this.touched(cx);
                cx.notify();
            }));
        let newest = self.targets.as_ref().map_or(0, |targets| targets.commits.len()).max(1);
        let menu = self.picking.then(|| {
            let rows = self
                .choices()
                .into_iter()
                .enumerate()
                .flat_map(|(n, (heading, source))| {
                    let heading = heading.map(|heading| match heading {
                        "Commits" => menu_heading(format!("Commits (newest {newest})"), palette).into_any_element(),
                        other => crate::workspace::menu_label(other, palette).into_any_element(),
                    });
                    let picked = source == self.source;
                    let row = crate::workspace::menu_row(
                        ("changes-pick", n),
                        self.label(&source).into(),
                        None,
                        picked,
                        palette,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(source.clone(), cx)))
                    .into_any_element();
                    heading.into_iter().chain([row])
                });
            div().absolute().left_0().top(px(28.)).child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopLeft)
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            crate::workspace::menu_box("changes-source-menu", 300., palette)
                                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                    this.picking = false;
                                    cx.notify();
                                }))
                                .children(rows),
                        ),
                )
                .with_priority(1),
            )
        });
        div().relative().child(trigger).children(menu)
    }
}

/// The files of `patch`. A patch the read cut (`truncated`) may end inside a hunk, so its last hunk is read up to the
/// cut; a whole one must match its hunk headers.
fn parse_patch(patch: &str, truncated: bool) -> Result<Vec<DiffFile>, String> {
    DiffFile::parse_with(patch, DiffParseOptions::new().truncated_hunks(truncated)).map_err(|error| error.to_string())
}

/// Each parsed file's lines, by its path, for comments: none when the two readings do not agree on the files.
fn file_lines(files: &[DiffFile], patch: &str) -> Vec<(SharedString, Vec<DiffLine>)> {
    let lines = diff_comment::parse_lines(patch);
    if lines.len() != files.len() {
        return Vec::new();
    }
    files.iter().map(|file| file.path().clone()).zip(lines).collect()
}

/// The diff's path for the followed file (relative to the folder Changes diffs, `follow_edits::in_folder`): the same
/// path, as the Nexus app's `diffPathOf`. None while the diff does not have it.
fn diff_path_of<'a>(mut paths: impl Iterator<Item = &'a str>, relative: &str) -> Option<String> {
    paths.find(|path| *path == relative).map(str::to_string)
}

/// A group heading with words made at run time ("Commits (newest 100)"), as `menu_label` draws one.
fn menu_heading(text: String, palette: Palette) -> Div {
    div()
        .px(px(10.))
        .pt(px(6.))
        .pb(px(2.))
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.faint)
        .child(text)
}

/// What the picker calls `source`: "This session", "Turn 3" ([`turn_number`]), "Against origin/main", "Uncommitted
/// changes" or a commit's `a1b2c3d Subject`.
pub fn source_label(source: &Source, turns: &[i64], ends: Option<&[i64]>, targets: Option<&DiffTargets>) -> String {
    match source {
        Source::Session => "This session".into(),
        Source::Turn(seq) => match turn_number(*seq, turns, ends) {
            Some(n) => format!("Turn {n}"),
            None => "A turn".into(),
        },
        Source::Branch => match targets.and_then(|targets| targets.base.as_deref()) {
            Some(base) => format!("Against {base}"),
            None => "Against the base branch".into(),
        },
        Source::Uncommitted => "Uncommitted changes".into(),
        Source::Commit(_) => {
            let commits = targets.map(|targets| targets.commits.as_slice()).unwrap_or_default();
            changes_source::picked_commit(source, commits)
                .map(|(_, label)| label)
                .unwrap_or_default()
        }
    }
}

/// The picker's number for the changed turn that ended at event `seq`; None when `turns` does not have it. It counts
/// from the session's start (`changes_source::turn_numbers`) when `ends` holds every turn end from the session's first
/// event and each changed turn is among them. Otherwise each turn takes its place in `turns`, as the Nexus app's
/// `changes-view.tsx` does while its log is not all read. One count serves every turn, so no two share a number.
fn turn_number(seq: i64, turns: &[i64], ends: Option<&[i64]>) -> Option<usize> {
    let place = turns.iter().position(|turn| *turn == seq)? + 1;
    if let Some(ends) = ends {
        let numbers: HashMap<i64, usize> = changes_source::turn_numbers(turns, ends.iter().copied());
        if numbers.len() == turns.len() {
            return numbers.get(&seq).copied();
        }
    }
    Some(place)
}

/// What the Changes tab reads from a session's transcript: its turn ends, to number the turns, and how many file
/// writes it shows that did not fail, which grows as the agent edits (the Nexus app's `fileEdits`, by tool rows).
/// The turn ends are None when `more` says older events exist before `blocks`: counted from the transcript's first
/// read event, they would number its turns from there, not from the session's start.
pub fn transcript_marks(blocks: &[crate::code::Block], more: bool) -> (Option<Vec<i64>>, usize) {
    let mut ends = Vec::new();
    let mut edits = 0;
    for block in blocks {
        match block.inner() {
            crate::code::Block::Turn { seq, .. } => ends.push(*seq),
            crate::code::Block::Tool { name, error: None, .. }
                if changes_source::FILE_TOOLS.contains(&name.as_str()) =>
            {
                edits += 1
            }
            _ => {}
        }
    }
    ((!more).then_some(ends), edits)
}

/// The patch for `source` in the session's folder, and whether it was cut.
fn read_diff(thread_id: Option<&str>, folder: &SessionFolder, source: &Source) -> Result<(String, bool), String> {
    let (dir, base) = (&folder.path, folder.base.as_deref());
    match (source, thread_id) {
        (Source::Session, Some(thread)) => changes_source::session_diff(dir, thread, base, None),
        (Source::Turn(seq), Some(thread)) => changes_source::session_diff(dir, thread, base, Some(*seq)),
        (Source::Session | Source::Turn(_), None) => {
            diff_source::diff(dir, &diff_source::DiffSource::Uncommitted, None)
        }
        (other, _) => match other.diff_source() {
            Some(read) => diff_source::diff(dir, &read, base),
            None => Err("Nothing to compare.".into()),
        },
    }
}

impl Render for Changes {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<crate::pages::Theme>().0;
        // Outside a repository there is nothing git here to show: no sources, no commit box, one centred line.
        if !self.read_once && self.error.is_none() {
            return div().size_full().into_any_element();
        }
        if self.not_repo {
            let state = match self.render_git_init(palette, cx) {
                Some(line) => v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .px(px(24.))
                    .pb(px(40.))
                    .child(Icon::new(Lucide::GitBranch).size(px(28.)).text_color(palette.faint))
                    .child(line),
                // A server's folder: the step would run on this computer, so there is none to offer.
                None => empty_state(
                    Some(Lucide::GitBranch),
                    Some("Not a git repository"),
                    "Changes show once this folder is in a git repository.",
                    palette,
                ),
            };
            return v_flex().size_full().child(state).into_any_element();
        }
        // The chips' text lines up with the pane's first tab: 12px in, less the chip's own 9px.
        let picker = h_flex()
            .flex_none()
            .h(px(40.))
            .pl(px(INSET - 9.))
            .pr(px(INSET - 6.))
            .gap(px(4.))
            .child(self.render_picker(palette, cx))
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(if self.loading {
                        "Reading…".to_string()
                    } else if self.error.is_some() {
                        String::new()
                    } else {
                        match self.files {
                            1 => "1 file".to_string(),
                            files => format!("{files} files"),
                        }
                    }),
            )
            .child(
                super::icon_button("changes-refresh", Lucide::RefreshCw, palette)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );
        let body = if let Some(error) = self.error.clone() {
            error_note("changes-error", error, palette).into_any_element()
        } else if self.files == 0 && !self.loading {
            match &self.source {
                // The pane's first view, as the Nexus app's (`PaneEmpty`).
                Source::Session => empty_state(
                    Some(Lucide::FileDiff),
                    Some("No changes yet"),
                    "Edits the agent makes show here, file by file.",
                    palette,
                ),
                source => {
                    let base = self.targets.as_ref().and_then(|targets| targets.base.as_deref());
                    let note: SharedString = source.empty_note(base).into();
                    empty_state(Some(Lucide::FileDiff), None, note, palette)
                }
            }
            .into_any_element()
        } else {
            v_flex()
                .relative()
                .flex_1()
                .min_h(px(0.))
                // The person's own scrolling, clicks and keys here pause following the agent's edits.
                .on_scroll(cx.listener(|this, _, _, cx| this.touched(cx)))
                .capture_any_mouse_down(cx.listener(|this, _, _, cx| this.touched(cx)))
                .capture_key_down(cx.listener(|this, _, _, cx| this.touched(cx)))
                .when(self.truncated, |body| {
                    body.child(
                        div()
                            .px(px(12.))
                            .py(px(6.))
                            .text_size(px(12.))
                            .text_color(palette.muted)
                            .child("The diff is too large to show whole; it is cut short."),
                    )
                })
                .child({
                    let diff = Diff::new(&self.diff)
                        .size_full()
                        .flex_1()
                        .text_size(px(12.5))
                        .decorations(self.find_marks());
                    let diff = match self.thread_id.is_some() {
                        true => {
                            let this = cx.entity().downgrade();
                            diff.on_add_annotation(move |range, window, cx| {
                                let _ = this.update(cx, |this, cx| this.open_comment(range.clone(), window, cx));
                            })
                        }
                        false => diff,
                    };
                    match self.comment_at() {
                        Some(at) => {
                            let this = cx.entity().downgrade();
                            diff.annotations([at]).render_annotation(move |_, _, cx| {
                                this.update(cx, |this, cx| this.comment_box(palette, cx))
                                    .unwrap_or_else(|_| div().into_any_element())
                            })
                        }
                        None => diff,
                    }
                })
                .when(self.find.open, |body| body.child(self.render_find(palette, cx)))
                .into_any_element()
        };
        let stat = self.uncommitted.as_ref().filter(|stat| stat.files > 0);
        let footer = v_flex()
            .flex_none()
            .gap(px(8.))
            .px(px(INSET))
            .pt(px(8.))
            .pb(px(INSET))
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child(Icon::new(Lucide::GitCommitHorizontal).size(px(14.)))
                    .child(match stat {
                        Some(stat) => format!(
                            "{} {} to commit",
                            stat.files,
                            if stat.files == 1 { "file" } else { "files" }
                        ),
                        None => "Nothing to commit".to_string(),
                    })
                    .when_some(stat, |row, stat| {
                        row.child(
                            div()
                                .text_color(gpui_kit::rgb(0x3a8f3e))
                                .child(format!("+{}", stat.insertions)),
                        )
                        .child(
                            div()
                                .text_color(palette.destructive)
                                .child(format!("−{}", stat.deletions)),
                        )
                    }),
            )
            .when(self.can_commit, |footer| {
                footer.child(
                    h_flex()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .h(px(30.))
                                .px(px(10.))
                                .flex()
                                .items_center()
                                .rounded(px(8.))
                                .bg(field_fill(palette))
                                .text_size(px(13.))
                                .child(Input::new(&self.message).appearance(false)),
                        )
                        .child({
                            let write = self.write_refused.is_none() && !self.drafting && !self.committing;
                            crate::workspace::write_button(
                                "changes-write",
                                self.drafting,
                                write,
                                self.write_refused,
                                palette,
                            )
                            .when(write, |button| {
                                button.on_click(cx.listener(|this, _, _, cx| this.write_it(cx)))
                            })
                        })
                        .child(
                            div()
                                .id("commit")
                                .h(px(30.))
                                .px(px(12.))
                                .flex()
                                .items_center()
                                .rounded(px(8.))
                                .bg(palette.primary)
                                .text_color(palette.primary_foreground)
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .when(stat.is_none() || self.committing, |button| button.opacity(0.4))
                                .when(stat.is_some() && !self.committing, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.opacity(0.88))
                                        .on_click(cx.listener(|this, _, _, cx| this.commit(cx)))
                                })
                                .child(if self.committing { "Committing…" } else { "Commit" }),
                        ),
                )
            })
            .when(!self.can_commit, |footer| {
                footer.child(
                    div()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child("The Nexus app runs this session: commit from there."),
                )
            })
            .when_some(self.commit_note.clone(), |footer, text| {
                footer.child(div().text_size(px(12.)).text_color(palette.muted).child(text))
            });
        v_flex()
            .size_full()
            .key_context("Changes")
            .on_action(cx.listener(Self::on_find))
            .child(picker)
            .child(body)
            .child(footer)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Source, count_label, diff_path_of, source_label, transcript_marks};
    use nexus_workbench::diff_source::{Commit, DiffTargets};

    #[test]
    fn find_counts_as_the_session_find_does() {
        assert_eq!(count_label("retry", Some(2), 5), "3 of 5");
        assert_eq!(count_label("nope", None, 0), "No matches");
        assert_eq!(count_label("", None, 0), "");
    }

    /// git's diff of `notes.md` whose last line is a blank context line: one space, then the newline.
    const BLANK_LAST: &str = "diff --git a/notes.md b/notes.md\nindex 1111111..2222222 100644\n--- a/notes.md\n\
        +++ b/notes.md\n@@ -1,4 +1,4 @@\n-# Old\n+# New\n one\n two\n \n";

    #[test]
    fn patch_ending_on_a_blank_context_line_parses_whole() {
        let files = super::parse_patch(BLANK_LAST, false).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(files.len(), 1);
        assert!(!files[0].is_truncated());
        // Trimmed, as the read once returned it, the hunk is a line short of its header.
        let trimmed = super::parse_patch(BLANK_LAST.trim_end(), false)
            .map(|_| ())
            .unwrap_err();
        assert!(
            trimmed.contains("hunk ended before its declared line counts"),
            "{trimmed}"
        );
    }

    #[test]
    fn cut_patch_parses_up_to_the_cut() {
        let second = "diff --git a/b.rs b/b.rs\nindex 3333333..4444444 100644\n--- a/b.rs\n+++ b/b.rs\n\
            @@ -1,3 +1,3 @@\n-fn old() {}\n+fn new() {}\n";
        let patch = format!("{BLANK_LAST}{second}");
        assert!(
            super::parse_patch(&patch, false).is_err(),
            "a cut hunk is short of its header"
        );
        let files = super::parse_patch(&patch, true).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(files.len(), 2);
        assert!(!files[0].is_truncated() && files[1].is_truncated());
        assert_eq!((files[1].additions(), files[1].deletions()), (1, 1));
    }

    #[test]
    fn follow_edits_finds_the_edited_file_in_the_diff() {
        let paths = ["a.rs", "src/a.rs", "src/b.rs"];
        let find = |edited| diff_path_of(paths.iter().copied(), edited);
        assert_eq!(find("src/a.rs").as_deref(), Some("src/a.rs"));
        assert_eq!(find("src/b.rs").as_deref(), Some("src/b.rs"));
        assert_eq!(find("src/c.rs"), None);
        assert_eq!(find("/other-worktree/src/a.rs"), None, "only the exact relative path");
    }

    #[test]
    fn the_picker_names_each_source_as_the_nexus_app_does() {
        let targets = DiffTargets {
            base: Some("origin/main".into()),
            commits: vec![Commit {
                id: "a1b2c3d4e5".into(),
                short: "a1b2c3d".into(),
                subject: "Fix the retry".into(),
                author: "You".into(),
                time: 1,
            }],
        };
        let turns = [4, 15];
        let label = |source: Source, ends: Option<&[i64]>| source_label(&source, &turns, ends, Some(&targets));
        assert_eq!(label(Source::Session, None), "This session");
        // Numbered from the session's start once its turn ends are known, by place until then.
        assert_eq!(label(Source::Turn(15), Some(&[4, 9, 15])), "Turn 3");
        assert_eq!(label(Source::Turn(15), None), "Turn 2");
        assert_eq!(label(Source::Turn(16), None), "A turn");
        assert_eq!(label(Source::Branch, None), "Against origin/main");
        assert_eq!(
            source_label(&Source::Branch, &[], None, None),
            "Against the base branch"
        );
        assert_eq!(label(Source::Uncommitted, None), "Uncommitted changes");
        assert_eq!(
            label(Source::Commit("a1b2c3d4e5".into()), None),
            "a1b2c3d Fix the retry"
        );
        assert_eq!(label(Source::Commit("ffffeeee11".into()), None), "ffffeee");
    }

    /// A session of `turns` turns, each a message (odd event) and its end (even event), read from event `start` on.
    fn transcript(turns: i64, start: i64) -> crate::code::Transcript {
        use nexus_workbench::event::{TurnStatus, WbEvent};
        let events = (start..=turns * 2)
            .map(|seq| {
                let event = if seq % 2 == 1 {
                    WbEvent::User {
                        text: format!("Message {}", (seq + 1) / 2),
                        from: None,
                        images: Vec::new(),
                    }
                } else {
                    WbEvent::TurnEnd {
                        turn_id: format!("turn {}", seq / 2),
                        status: TurnStatus::Completed,
                        cost_usd: None,
                        duration_ms: 10,
                        error: None,
                        cursor: None,
                    }
                };
                (seq, 0, event)
            })
            .collect();
        crate::code::Transcript::of(events)
    }

    /// The picker's turn labels, in its order.
    fn turn_labels(turns: &[i64], read: &crate::code::Transcript) -> Vec<String> {
        let (ends, _) = transcript_marks(&read.blocks, read.more);
        turns
            .iter()
            .map(|seq| source_label(&Source::Turn(*seq), turns, ends.as_deref(), None))
            .collect()
    }

    #[test]
    fn partly_read_session_numbers_its_turns_once_and_in_order() {
        // 200 turns that each changed files; only the newest 19 are read, as a long session opens.
        let turns: Vec<i64> = (1..=200).map(|turn| turn * 2).collect();
        let read = transcript(200, 363);
        assert!(read.more);
        let labels = turn_labels(&turns, &read);
        let expected: Vec<String> = (1..=200).map(|n| format!("Turn {n}")).collect();
        assert_eq!(labels, expected);
    }

    #[test]
    fn all_read_session_counts_turns_that_changed_nothing() {
        // Turns 2 and 4 changed nothing, so they have no checkpoint; the rest keep their numbers from the start.
        let turns = [2, 6, 10];
        let read = transcript(5, 1);
        assert!(!read.more);
        assert_eq!(turn_labels(&turns, &read), ["Turn 1", "Turn 3", "Turn 5"]);
    }

    #[test]
    fn changed_turn_missing_from_the_transcript_numbers_every_turn_by_place() {
        // The transcript is all read but lacks the turn ending at event 12: mixing counts would make two "Turn 3"s.
        let turns = [2, 6, 12];
        let read = transcript(5, 1);
        assert_eq!(turn_labels(&turns, &read), ["Turn 1", "Turn 2", "Turn 3"]);
    }

    #[test]
    fn empty_session_has_no_turns_to_number() {
        let read = transcript(0, 1);
        assert_eq!(read.start, None);
        assert!(turn_labels(&[], &read).is_empty());
        assert_eq!(source_label(&Source::Turn(2), &[], Some(&[]), None), "A turn");
    }
}
