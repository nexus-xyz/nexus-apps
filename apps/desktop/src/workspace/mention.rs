//! Mentions in a Code session's composer, as the Nexus app's (`file-mentions.ts`, `mentions.ts`,
//! `mention-menu.tsx`): "@" (at the start or after a space) lists other sessions, then the folder's files; "#" (or "!",
//! GitLab's sign) lists the repository's open pull requests. The words after the sign narrow the list. Up and Down
//! move, Enter or Tab or a click puts the pick in place of what was typed, Esc closes the menu for that text. The rows
//! are drawn by the slash menu's popup (`slash.rs` `render_menu`), so all three menus look and behave the same.
//!
//! A pick is plain text, as the Nexus app inserts it: `@src/app.ts` (`@"my notes.md"` with a space), a session as
//! `[Fix the login](session:t_1a2b)`, a pull request as `#123` (`!123` on GitLab). The shared core reads the session
//! links and numbers when the message is sent (nexus-workbench `mentions.rs`) and tells the agent what each one is.
//!
//! The files come from nexus-workbench `files::list` (what git knows of, less what .gitignore hides, capped at 25k),
//! read off the UI thread and kept per folder, with each path's lowercase made once, so typing ranks a big repository
//! in a few milliseconds. They are asked for again when the menu opens after [`FILES_FRESH`], as the Nexus app does,
//! and the old list shows meanwhile. Pull requests come from `gh` (`mentions::pull_requests`, which keeps them 30s).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::git::PullRequestInfo;
use nexus_workbench::source_host::Host;

use gpui_kit::component::input::TextareaState;

use super::super::{View, Workspace};
use super::{Glyph, MenuRow, render_menu};
use crate::code::CodeSession;
use crate::file_icons::FileIcon;
use crate::palette::Palette;

/// How many files the "@" menu lists (`MENU_LIMIT`).
pub(in crate::workspace) const FILE_LIMIT: usize = 50;
/// How many sessions it lists above the files, typed and with nothing typed yet (`SESSION_LIMIT`, `RECENT_SESSIONS`).
const SESSION_LIMIT: usize = 5;
const RECENT_SESSIONS: usize = 3;
/// How many pull requests the "#" menu lists (`PR_LIMIT`).
const PR_LIMIT: usize = 30;
/// A folder's files are asked for again when the menu opens this long after they were (the core keeps them 10s).
const FILES_FRESH: Duration = Duration::from_secs(10);
/// And its pull requests (the core keeps them 30s).
const PRS_FRESH: Duration = Duration::from_secs(30);

/// The mention word the caret is in, in bytes of the composer's text: from the sign to the caret, and what follows the
/// sign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::workspace) struct MentionQuery {
    pub query: String,
    pub start: usize,
    pub end: usize,
}

/// Where the word the caret ends starts: after the last whitespace before it.
pub(in crate::workspace) fn word_start(text: &str, end: usize) -> usize {
    text[..end]
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(ix, c)| ix + c.len_utf8())
}

pub(in crate::workspace) fn clamp(text: &str, caret: usize) -> usize {
    let mut end = caret.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// The "@" mention being typed at `caret` (`mentionQuery`), or None. The "@" must start the text or follow a space or
/// a line break, so an email address opens nothing; `@"` (a quoted path typed by hand) is left alone.
pub(in crate::workspace) fn mention_query(text: &str, caret: usize) -> Option<MentionQuery> {
    let end = clamp(text, caret);
    let start = word_start(text, end);
    let query = text[start..end].strip_prefix('@')?;
    (!query.starts_with('"')).then(|| MentionQuery {
        query: query.into(),
        start,
        end,
    })
}

/// The pull request mention being typed at `caret` (`pullRequestQuery`): "#" or "!" at the start or after a space, then
/// letters, digits, "-" or "_" up to the caret. "a#1" and "#!" open nothing.
pub(in crate::workspace) fn pull_request_query(text: &str, caret: usize) -> Option<MentionQuery> {
    let end = clamp(text, caret);
    let start = word_start(text, end);
    let query = text[start..end].strip_prefix(['#', '!'])?;
    query
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        .then(|| MentionQuery {
            query: query.into(),
            start,
            end,
        })
}

/// How a picked path reads in the message (`mentionText`).
pub(in crate::workspace) fn mention_text(path: &str) -> String {
    if path.chars().any(char::is_whitespace) {
        format!("@\"{path}\"")
    } else {
        format!("@{path}")
    }
}

/// A session's title as a link's text (`linkTitle`): no brackets, parentheses, backslashes or line breaks, at most 60
/// characters.
fn link_title(title: &str) -> String {
    let spaced: String = title
        .chars()
        .map(|c| {
            if matches!(c, '[' | ']' | '(' | ')' | '\\') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let spaced = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean = clip(&spaced, 60);
    let clean = clean.trim();
    if clean.is_empty() {
        "session".into()
    } else {
        clean.into()
    }
}

/// `clip.ts`: at most `max` characters, the last one an ellipsis when cut.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.into();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// How a picked session reads in the message (`sessionLink`).
pub(in crate::workspace) fn session_link(id: &str, title: &str) -> String {
    format!("[{}](session:{id})", link_title(title))
}

/// A pull request's number as its host writes it (`requestRef`): `#123`, or `!123` for a GitLab merge request.
pub(in crate::workspace) fn request_ref(host: Host, number: u64) -> String {
    match host {
        Host::GitLab => format!("!{number}"),
        Host::GitHub => format!("#{number}"),
    }
}

/// The text with the mention `at` replaced by `token` and a space (none added when one already follows), and where the
/// caret goes: after that space (`insertToken`).
pub(in crate::workspace) fn insert_token(text: &str, at: &MentionQuery, token: &str) -> (String, usize) {
    let rest = &text[at.end..];
    let spaced = rest.starts_with(char::is_whitespace);
    let inserted = if spaced { token.to_string() } else { format!("{token} ") };
    let caret = at.start
        + inserted.len()
        + if spaced {
            rest.chars().next().map_or(0, char::len_utf8)
        } else {
            0
        };
    (format!("{}{inserted}{rest}", &text[..at.start]), caret)
}

/// Put `token` in place of the word `at` in `input` (`insert_token`), the caret after it, and the focus there. Only the
/// typed word is replaced, so undo takes the pick back.
pub(in crate::workspace) fn put_token<T: 'static>(
    input: &Entity<TextareaState>,
    at: &MentionQuery,
    token: &str,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let text = input.read(cx).value();
    let (next, caret) = insert_token(&text, at, token);
    let inserted = next[at.start..next.len() - (text.len() - at.end)].to_string();
    input.update(cx, |state, cx| {
        state.set_selected_range(at.start..at.end, cx);
        state.replace(inserted, window, cx);
        state.set_selected_range(caret..caret, cx);
        state.focus(window, cx);
    });
}

/// A folder's files with each path's lowercase, made once when the list arrives.
#[derive(Default)]
pub(in crate::workspace) struct FileIndex {
    pub(in crate::workspace) paths: Arc<Vec<String>>,
    pub(in crate::workspace) lower: Vec<String>,
}

impl FileIndex {
    pub(in crate::workspace) fn new(paths: Arc<Vec<String>>) -> FileIndex {
        let lower = paths.iter().map(|path| path.to_lowercase()).collect();
        FileIndex { paths, lower }
    }
}

fn is_separator(c: char) -> bool {
    matches!(c, '/' | '-' | '_' | '.' | ' ')
}

/// How well `query` (lowercase) matches `path` as a subsequence (`fuzzyScore`), or None: each letter scores, more when
/// it starts a word (after a separator, or a capital after a small letter) or follows the one before, less the further
/// it skips ahead. Each place the first letter appears is tried as a start, and the best counts.
pub(in crate::workspace) fn fuzzy_score(path: &str, query: &str) -> Option<f64> {
    let chars: Vec<char> = path.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let query: Vec<char> = query.chars().collect();
    let first = *query.first()?;
    let mut best: Option<f64> = None;
    for start in (0..lower.len()).filter(|&ix| lower[ix] == first) {
        // A later start can't find letters an earlier one couldn't.
        let Some(score) = score_from(&chars, &lower, &query, start) else {
            break;
        };
        if best.is_none_or(|best| score > best) {
            best = Some(score);
        }
    }
    best
}

fn score_from(chars: &[char], lower: &[char], query: &[char], start: usize) -> Option<f64> {
    let mut score = 0.;
    let mut from = start;
    let mut last = start as isize - 1;
    for &c in query {
        let at = from + lower[from..].iter().position(|&l| l == c)?;
        let before = if at > 0 { chars[at - 1] } else { '/' };
        let boundary = is_separator(before) || (before.is_ascii_lowercase() && chars[at].is_ascii_uppercase());
        score += 1. + if boundary { 3. } else { 0. } + if at as isize == last + 1 { 2. } else { 0. }
            - ((at - from) as f64 / 8.).min(3.);
        last = at as isize;
        from = at + 1;
    }
    Some(score)
}

/// The files that match `query`, best first, at most `limit` (`rankFiles`). Case aside: a name that is the query, then
/// names that start with it, names that contain it, paths that contain it (earlier is better), and last paths that
/// hold its letters in order. Inside a tier, shallower and shorter paths win, then alphabetical order. Nothing typed:
/// the files nearest the top of the folder.
pub(in crate::workspace) fn rank_files(index: &FileIndex, query: &str, limit: usize) -> Vec<String> {
    let q = query.to_lowercase();
    let mut scored: Vec<(f64, usize)> = Vec::new();
    for (ix, lower) in index.lower.iter().enumerate() {
        let name = &lower[lower.rfind('/').map_or(0, |slash| slash + 1)..];
        let score = if q.is_empty() {
            Some(0.)
        } else if name == q {
            Some(5000.)
        } else if name.starts_with(&q) {
            Some(4000.)
        } else if name.contains(&q) {
            Some(3000.)
        } else if let Some(at) = lower.find(&q) {
            Some(2000. - lower[..at].chars().count() as f64 / 100.)
        } else if lower.contains(q.chars().next().unwrap_or_default()) {
            // Capped, so a long query never lifts a scattered match above one that contains it.
            fuzzy_score(&index.paths[ix], &q).map(|score| score.min(1999.))
        } else {
            None
        };
        if let Some(score) = score {
            scored.push((score, ix));
        }
    }
    let paths = &index.paths;
    let order = |a: &(f64, usize), b: &(f64, usize)| {
        let (pa, pb) = (&paths[a.1], &paths[b.1]);
        b.0.total_cmp(&a.0)
            .then_with(|| pa.matches('/').count().cmp(&pb.matches('/').count()))
            .then_with(|| pa.chars().count().cmp(&pb.chars().count()))
            .then_with(|| pa.cmp(pb))
    };
    // Only the best `limit` are put in order: nothing typed in a 25k-file folder sorts 50, not 25k.
    if scored.len() > limit {
        scored.select_nth_unstable_by(limit, order);
        scored.truncate(limit);
    }
    scored.sort_unstable_by(order);
    scored.into_iter().map(|(_, ix)| paths[ix].clone()).collect()
}

/// The sessions the "@" menu offers (`rankSessions`): not `self_id`, not archived. Typed: titles that start with the
/// query, then those with a word that does, then those that contain it, newest first inside each, at most
/// [`SESSION_LIMIT`]. Nothing typed: the [`RECENT_SESSIONS`] last worked in.
pub(in crate::workspace) fn rank_sessions<'a>(
    sessions: impl IntoIterator<Item = &'a CodeSession>,
    query: &str,
    self_id: Option<&str>,
) -> Vec<&'a CodeSession> {
    let q = query.to_lowercase();
    let mut scored: Vec<(u8, &CodeSession)> = Vec::new();
    for session in sessions {
        if Some(session.id.as_str()) == self_id || session.archived {
            continue;
        }
        let title = session.title.to_lowercase();
        let tier = if q.is_empty() || title.starts_with(&q) {
            0
        } else if title
            .split(|c: char| c.is_whitespace() || matches!(c, '/' | '.' | '_' | '-'))
            .any(|word| word.starts_with(&q))
        {
            1
        } else if title.contains(&q) {
            2
        } else {
            continue;
        };
        scored.push((tier, session));
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.updated_at.cmp(&a.1.updated_at)));
    let limit = if q.is_empty() { RECENT_SESSIONS } else { SESSION_LIMIT };
    scored.into_iter().take(limit).map(|(_, session)| session).collect()
}

/// The pull requests the "#" menu offers (`rankPullRequests`): digits match the number's start (`#12` finds 12 and
/// 123), words the title or the branch. The list's own order (newest first) is kept.
pub(in crate::workspace) fn rank_pull_requests<'a>(
    prs: &'a [PullRequestInfo],
    query: &str,
) -> Vec<&'a PullRequestInfo> {
    let q = query.to_lowercase();
    let digits = !q.is_empty() && q.chars().all(|c| c.is_ascii_digit());
    prs.iter()
        .filter(|pr| {
            q.is_empty()
                || if digits {
                    pr.number.to_string().starts_with(&q)
                } else {
                    pr.title.to_lowercase().contains(&q) || pr.head.to_lowercase().contains(&q)
                }
        })
        .take(PR_LIMIT)
        .collect()
}

/// What a row is: the menu groups rows by it, under a small label, each with its glyph (`KINDS`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::workspace) enum Kind {
    Session,
    File,
    PullRequest,
}

/// One row of the menu, and what a pick puts in the message.
#[derive(Clone, Debug)]
pub(in crate::workspace) struct Choice {
    pub kind: Kind,
    pub name: String,
    /// Dimmed after the name: a file's folder, a pull request's title. A session's shows none.
    pub hint: String,
    pub token: String,
}

/// A folder's list, and when it was read.
struct Loaded<T> {
    value: T,
    at: Instant,
}

/// The menu's state between frames.
#[derive(Default)]
pub(in crate::workspace) struct MentionState {
    active: usize,
    /// The query the highlight is for (sign's place and the words): another starts again at the top.
    key: Option<(usize, String)>,
    /// The composer's text when Esc closed the menu, or a pick filled it.
    dismissed: RefCell<Option<String>>,
    files: RefCell<HashMap<PathBuf, Loaded<Arc<FileIndex>>>>,
    prs: RefCell<HashMap<PathBuf, Loaded<Arc<Vec<PullRequestInfo>>>>>,
    /// Folders being read now, files and pull requests apart.
    loading: RefCell<HashSet<(PathBuf, bool)>>,
    /// Folders that a frame found missing or stale: render reads them (it has the context to spawn on).
    wanted: RefCell<Vec<(PathBuf, bool)>>,
    /// The open view's folder: (session or project id, its folder).
    folder: RefCell<Option<(String, Option<PathBuf>)>>,
    /// The last rows made: (folder, query start, query, pull requests?, files list's age) and the choices.
    rows: RefCell<Option<(RowsKey, Arc<Vec<Choice>>)>>,
    scroll: ScrollHandle,
}

type RowsKey = (PathBuf, usize, String, bool, Option<Instant>, Option<Instant>, usize);

impl MentionState {
    /// The folder for `key` ("t:" and a session id, or "p:" and a project's), read by `resolve` only when the key
    /// changed since the last ask.
    pub(in crate::workspace) fn folder_for(
        &self,
        key: String,
        resolve: impl FnOnce(&str) -> Option<PathBuf>,
    ) -> Option<PathBuf> {
        if let Some((cached, folder)) = &*self.folder.borrow()
            && *cached == key
        {
            return folder.clone();
        }
        let folder = resolve(&key);
        *self.folder.borrow_mut() = Some((key, folder.clone()));
        folder
    }

    /// The menu's query and rows for `text` with the caret at `caret`: an "@" or "#" word there, not dismissed, not
    /// under a selection, in a composer with a folder (`folder`), with something to list. `sessions` are the ones "@"
    /// may name; `self_id` is the composer's own session, which it never lists.
    pub(in crate::workspace) fn matches<'a>(
        &self,
        text: &str,
        caret: usize,
        selected: bool,
        folder: impl FnOnce() -> Option<PathBuf>,
        sessions: impl FnOnce() -> Vec<&'a CodeSession>,
        self_id: Option<&str>,
    ) -> Option<(MentionQuery, Arc<Vec<Choice>>)> {
        let state = self;
        {
            let mut dismissed = state.dismissed.borrow_mut();
            match dismissed.as_deref() {
                Some(closed) if closed == text => return None,
                Some(_) => *dismissed = None,
                None => {}
            }
        }
        // A selection is not a mention being typed.
        if selected {
            return None;
        }
        let (at, prs) = match mention_query(text, caret) {
            Some(at) => (at, false),
            None => (pull_request_query(text, caret)?, true),
        };
        let folder = folder()?;
        let files_at = self.want(&folder, false);
        let prs_at = prs.then(|| self.want(&folder, true)).flatten();
        let sessions = sessions();
        let key: RowsKey = (
            folder.clone(),
            at.start,
            at.query.clone(),
            prs,
            files_at,
            prs_at,
            sessions.len(),
        );
        if let Some((cached, rows)) = &*state.rows.borrow()
            && *cached == key
        {
            return (!rows.is_empty()).then(|| (at, rows.clone()));
        }
        let mut rows = Vec::new();
        if prs {
            if let Some(list) = state.prs.borrow().get(&folder) {
                for pr in rank_pull_requests(&list.value, &at.query) {
                    let number = request_ref(pr.host, pr.number);
                    rows.push(Choice {
                        kind: Kind::PullRequest,
                        name: number.clone(),
                        hint: pr.title.clone(),
                        token: number,
                    });
                }
            }
        } else {
            for session in rank_sessions(sessions, &at.query, self_id) {
                rows.push(Choice {
                    kind: Kind::Session,
                    name: session.title.clone(),
                    hint: String::new(),
                    token: session_link(&session.id, &session.title),
                });
            }
            if let Some(index) = state.files.borrow().get(&folder) {
                for path in rank_files(&index.value, &at.query, FILE_LIMIT) {
                    let (folder, name) = path.rsplit_once('/').unwrap_or(("", &path));
                    rows.push(Choice {
                        kind: Kind::File,
                        name: name.into(),
                        hint: folder.into(),
                        token: mention_text(&path),
                    });
                }
            }
        }
        let rows = Arc::new(rows);
        *state.rows.borrow_mut() = Some((key, rows.clone()));
        (!rows.is_empty()).then_some((at, rows))
    }

    /// When `folder`'s files (or pull requests) were read; asks for them again when they never were, or are stale.
    fn want(&self, folder: &PathBuf, prs: bool) -> Option<Instant> {
        let state = self;
        let at = if prs {
            state.prs.borrow().get(folder).map(|loaded| loaded.at)
        } else {
            state.files.borrow().get(folder).map(|loaded| loaded.at)
        };
        let fresh = if prs { PRS_FRESH } else { FILES_FRESH };
        let key = (folder.clone(), prs);
        if at.is_none_or(|at| at.elapsed() >= fresh) && !state.loading.borrow().contains(&key) {
            let mut wanted = state.wanted.borrow_mut();
            if !wanted.contains(&key) {
                wanted.push(key);
            }
        }
        at
    }

    /// Read what the last frames asked for, off the UI thread, into the state `state` finds on `T`.
    pub(in crate::workspace) fn load_wanted<T: 'static>(&self, cx: &mut Context<T>, state: fn(&T) -> &MentionState) {
        let wanted = std::mem::take(&mut *self.wanted.borrow_mut());
        for (folder, prs) in wanted {
            if !self.loading.borrow_mut().insert((folder.clone(), prs)) {
                continue;
            }
            let dir = folder.clone();
            if prs {
                let read = cx.background_executor().spawn(async move {
                    let path = crate::code_run::agent_env().remove("PATH").unwrap_or_default();
                    nexus_workbench::mentions::pull_requests(&dir, &path).unwrap_or_default()
                });
                cx.spawn(async move |this, cx| {
                    let list = read.await;
                    let _ = this.update(cx, |this, cx| {
                        let state = state(this);
                        state.loading.borrow_mut().remove(&(folder.clone(), true));
                        state.prs.borrow_mut().insert(
                            folder,
                            Loaded {
                                value: Arc::new(list),
                                at: Instant::now(),
                            },
                        );
                        cx.notify();
                    });
                })
                .detach();
            } else {
                let read = cx.background_executor().spawn(async move {
                    let files = nexus_workbench::files::list(&dir)
                        .map(|list| list.files)
                        .unwrap_or_default();
                    FileIndex::new(files)
                });
                cx.spawn(async move |this, cx| {
                    let index = read.await;
                    let _ = this.update(cx, |this, cx| {
                        let state = state(this);
                        state.loading.borrow_mut().remove(&(folder.clone(), false));
                        state.files.borrow_mut().insert(
                            folder,
                            Loaded {
                                value: Arc::new(index),
                                at: Instant::now(),
                            },
                        );
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    /// The highlighted row for the mention `at`: another starts again at the top.
    pub(in crate::workspace) fn active_for(&self, at: &MentionQuery, rows: usize) -> usize {
        if self.key.as_ref() == Some(&(at.start, at.query.clone())) {
            self.active.min(rows.saturating_sub(1))
        } else {
            0
        }
    }

    /// Up or Down: the next row, wrapping, scrolled into view.
    pub(in crate::workspace) fn step(&mut self, at: MentionQuery, rows: &[Choice], down: bool) {
        let active = self.active_for(&at, rows.len());
        self.active = if down {
            (active + 1) % rows.len()
        } else {
            (active + rows.len() - 1) % rows.len()
        };
        self.key = Some((at.start, at.query));
        self.scroll.scroll_to_item(row_child(rows, self.active));
    }

    /// Esc: the menu stays closed while the text is `text`.
    pub(in crate::workspace) fn dismiss(&self, text: String) {
        *self.dismissed.borrow_mut() = Some(text);
    }

    /// A pick filled the word: the next menu starts at the top.
    pub(in crate::workspace) fn picked(&mut self) {
        self.active = 0;
        self.key = None;
    }

    pub(in crate::workspace) fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }
}

/// The mention menu's rows as the popup draws them: a group header over each kind, then a glyph, the name and the
/// dimmed hint.
pub(in crate::workspace) fn menu_rows(rows: &[Choice]) -> Vec<MenuRow> {
    rows.iter()
        .map(|choice| {
            let (group, icon) = match choice.kind {
                Kind::Session => ("Sessions", Lucide::MessageSquare.into()),
                // The file's type icon. Its hint is its folder, so the folder can pick it (`.config/…`).
                Kind::File => (
                    "Files",
                    Glyph::File(FileIcon::file(&format!("{}/{}", choice.hint, choice.name))),
                ),
                Kind::PullRequest => ("Pull requests", Lucide::GitFork.into()),
            };
            MenuRow {
                group,
                icon,
                name: choice.name.clone().into(),
                hint: choice.hint.clone().into(),
                dim: false,
            }
        })
        .collect()
}

impl Workspace {
    /// The folder of the session `id`, read from the store.
    pub(in crate::workspace) fn session_folder(&self, id: &str) -> Option<PathBuf> {
        self.code.as_ref()?.folder(id).ok().map(|folder| folder.path)
    }

    /// The sessions "@" may name: every project's.
    pub(in crate::workspace) fn mention_sessions(&self) -> Vec<&CodeSession> {
        self.projects.iter().flat_map(|project| &project.sessions).collect()
    }

    /// The folder whose files and pull requests the composer on screen mentions: the open session's, or the new
    /// session's project. None for a session still being made, or no project.
    fn mention_folder(&self) -> Option<PathBuf> {
        let key = match &self.view {
            View::Code(view) if !view.is_draft() => format!("t:{}", view.id),
            View::NewCode => format!("p:{}", self.code_form.project.as_deref()?),
            _ => return None,
        };
        self.slash.mention.folder_for(key, |key| match key.split_at(2) {
            ("t:", id) => self.session_folder(id),
            (_, id) => self.code.as_ref()?.project_folder(id).ok(),
        })
    }

    /// The menu's query and rows while it is open: a Code composer, focused, the slash menu closed, an "@" or "#" word
    /// at the caret, not dismissed, with something to list.
    pub(in crate::workspace) fn mention_matches(&self, cx: &App) -> Option<(MentionQuery, Arc<Vec<Choice>>)> {
        if !self.composer_focused
            || !matches!(self.view, View::Code(_) | View::NewCode)
            || self.slash_matches(cx).is_some()
        {
            return None;
        }
        let composer = self.composer.read(cx);
        let self_id = match &self.view {
            View::Code(view) => Some(view.id.as_str()),
            _ => None,
        };
        self.slash.mention.matches(
            &composer.value(),
            composer.cursor(),
            !composer.selected_range().is_empty(),
            || self.mention_folder(),
            || self.mention_sessions(),
            self_id,
        )
    }

    fn load_wanted(&self, cx: &mut Context<Self>) {
        self.slash
            .mention
            .load_wanted(cx, |this: &Workspace| &this.slash.mention);
    }

    fn mention_active(&self, at: &MentionQuery, rows: usize) -> usize {
        self.slash.mention.active_for(at, rows)
    }

    /// Up or Down while the menu is open: the next row, wrapping. False when it is closed.
    pub(in crate::workspace) fn mention_move(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some((at, rows)) = self.mention_matches(cx) else {
            return false;
        };
        self.slash.mention.step(at, &rows, down);
        cx.notify();
        true
    }

    /// Esc while the menu is open: it closes for this text.
    pub(in crate::workspace) fn mention_dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        if self.mention_matches(cx).is_none() {
            return false;
        }
        self.slash.mention.dismiss(self.composer.read(cx).value().to_string());
        cx.notify();
        true
    }

    /// Enter or Tab while the menu is open: the highlighted row goes in the message.
    pub(in crate::workspace) fn mention_pick_active(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((at, rows)) = self.mention_matches(cx) else {
            return false;
        };
        let token = rows[self.mention_active(&at, rows.len())].token.clone();
        self.mention_pick(&at, &token, window, cx);
        true
    }

    /// Put `token` in place of the mention `at`, a space after it, and the caret after that.
    fn mention_pick(&mut self, at: &MentionQuery, token: &str, window: &mut Window, cx: &mut Context<Self>) {
        put_token(&self.composer, at, token, window, cx);
        self.composer_empty = false;
        self.slash.mention.picked();
        cx.notify();
    }

    /// The menu, as the slash menu's: a group header over each kind, then a glyph, the name and the dimmed hint.
    pub(in crate::workspace) fn render_mention_menu(
        &self,
        below: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.mention_matches(cx);
        self.load_wanted(cx);
        let shown = open
            .as_ref()
            .map(|(at, rows)| (menu_rows(rows), self.mention_active(at, rows.len())));
        // A closed menu leaves with the rows it showed last (`motion::presence_of`); it takes no picks then.
        let (phase, (menu_rows, active)) = crate::motion::presence_of("mention-menu", shown, cx)?;
        Some(render_menu(
            "mention-menu",
            &self.slash.mention.scroll,
            menu_rows,
            active,
            below,
            phase,
            palette,
            move |this: &mut Workspace, ix, window, cx| {
                if let Some((at, picks)) = &open
                    && let Some(choice) = picks.get(ix)
                {
                    this.mention_pick(at, &choice.token, window, cx);
                }
            },
            cx,
        ))
    }
}

/// The child index of row `active` in the menu's list, its group headers counted.
fn row_child(rows: &[Choice], active: usize) -> usize {
    super::menu_row_child(rows.iter().map(|row| row.kind), active)
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's prelude has a `test` attribute of its own.
    use super::{
        CodeSession, FILE_LIMIT, FileIndex, Host, MentionQuery, PullRequestInfo, fuzzy_score, insert_token,
        mention_query, mention_text, pull_request_query, rank_files, rank_pull_requests, rank_sessions, request_ref,
        session_link,
    };
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn index(paths: &[&str]) -> FileIndex {
        FileIndex::new(Arc::new(paths.iter().map(|path| path.to_string()).collect()))
    }

    #[test]
    fn at_opens_at_a_word_start_only() {
        assert_eq!(
            mention_query("see @src/a", 10),
            Some(MentionQuery {
                query: "src/a".into(),
                start: 4,
                end: 10
            })
        );
        assert_eq!(mention_query("@", 1).map(|at| at.start), Some(0));
        assert_eq!(mention_query("me@acme.test", 12), None);
        assert_eq!(mention_query("@\"my no", 7), None);
        assert_eq!(mention_query("@a b", 4), None);
        // The caret mid-word ends the query there.
        assert_eq!(mention_query("@compo", 3).map(|at| at.query), Some("co".into()));
    }

    #[test]
    fn hash_takes_numbers_and_words() {
        assert_eq!(pull_request_query("fix #12", 7).map(|at| at.query), Some("12".into()));
        assert_eq!(pull_request_query("!login-fix", 10).map(|at| at.start), Some(0));
        assert_eq!(pull_request_query("a#1", 3), None);
        assert_eq!(pull_request_query("#!", 2), None);
        assert_eq!(pull_request_query("# ", 2), None);
    }

    #[test]
    fn picks_read_as_the_nexus_app_writes_them() {
        assert_eq!(mention_text("src/app.ts"), "@src/app.ts");
        assert_eq!(mention_text("docs/my notes.md"), "@\"docs/my notes.md\"");
        assert_eq!(
            session_link("t_1", "Fix [the] login\n(now)"),
            "[Fix the login now](session:t_1)"
        );
        assert_eq!(session_link("t_2", " [] "), "[session](session:t_2)");
        assert_eq!(request_ref(Host::GitHub, 12), "#12");
        assert_eq!(request_ref(Host::GitLab, 12), "!12");
        let at = mention_query("look @co now", 8).unwrap();
        assert_eq!(
            insert_token("look @co now", &at, "@composer.rs"),
            ("look @composer.rs now".into(), 18)
        );
        let at = mention_query("@co", 3).unwrap();
        assert_eq!(insert_token("@co", &at, "#1"), ("#1 ".into(), 3));
    }

    #[test]
    fn names_beat_paths_and_paths_beat_scattered_letters() {
        let files = index(&[
            "src/workspace/composer.rs",
            "src/composer/mod.rs",
            "composer.rs",
            "docs/my-composer-notes.md",
            "src/code/mention_picker.rs",
            "src/main.rs",
        ]);
        assert_eq!(
            rank_files(&files, "composer.rs", 10),
            ["composer.rs", "src/workspace/composer.rs", "src/composer/mod.rs"]
        );
        assert_eq!(
            rank_files(&files, "comp", 10),
            [
                "composer.rs",
                "src/workspace/composer.rs",
                "docs/my-composer-notes.md",
                "src/composer/mod.rs",
                "src/code/mention_picker.rs"
            ]
        );
        // No path holds "mepi" as typed; the fuzzy tier finds "mention_picker".
        assert_eq!(rank_files(&files, "mepi", 10), ["src/code/mention_picker.rs"]);
        // Case aside.
        assert_eq!(rank_files(&files, "MAIN", 10), ["src/main.rs"]);
    }

    #[test]
    fn nothing_typed_lists_the_top_of_the_folder_and_caps() {
        let files = index(&["b/deep/x.rs", "z.rs", "a/y.rs", "a.rs"]);
        assert_eq!(rank_files(&files, "", 3), ["a.rs", "z.rs", "a/y.rs"]);
    }

    #[test]
    fn fuzzy_prefers_word_starts() {
        let starts = fuzzy_score("src/new_code.rs", "nc").unwrap();
        let middle = fuzzy_score("src/financial.rs", "nc").unwrap();
        assert!(starts > middle, "{starts} vs {middle}");
        assert_eq!(fuzzy_score("abc", "x"), None);
        // A capital after a small letter starts a word too.
        assert!(fuzzy_score("MentionMenu.tsx", "mm").unwrap() > fuzzy_score("mammoth.tsx", "mm").unwrap());
    }

    #[test]
    fn a_big_folder_ranks_fast() {
        let paths: Vec<String> = (0..25_000)
            .map(|ix| format!("crates/part{}/src/module_{ix}/file_{}.rs", ix % 97, ix % 1013))
            .collect();
        let files = FileIndex::new(Arc::new(paths));
        let started = Instant::now();
        for query in ["", "f", "file_1", "mod5fil", "zzz"] {
            assert!(rank_files(&files, query, FILE_LIMIT).len() <= FILE_LIMIT);
        }
        // Generous for debug builds on a busy CI machine; a release build takes a few milliseconds each.
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    fn session(id: &str, title: &str, updated_at: i64, archived: bool) -> CodeSession {
        CodeSession {
            saved_provider: false,
            id: id.into(),
            linked: None,
            title: title.into(),
            name: None,
            status: crate::code::Status::Idle,
            updated_at,
            created_at: 0,
            acted_at: 0,
            agent: nexus_workbench::event::SessionAgent::Claude,
            model: None,
            mode: nexus_workbench::event::PermissionMode::Auto,
            effort: None,
            fast: false,
            pinned: false,
            branch: None,
            snoozed_until: None,
            settled: false,
            archived,
            machine: None,
            limited: None,
            interrupted: false,
            live: false,
            parent: None,
            background: false,
            background_busy: false,
            background_work: vec![],
            background_total: 0,
            waits_on: vec![],
            source: None,
            has_worktree: false,
            side_chat: false,
            compact: false,
            orchestration: false,
            at_depth_limit: false,
            handoff_from: None,
            agent_started: false,
            crew: Default::default(),
            start_choices: Vec::new(),
            project_id: "p".into(),
            pull_request: None,
            review: false,
            cost_usd: 0.0,
            context: None,
            pending: None,
            account: None,
            connection: None,
            via: nexus_workbench::event::Via::Nexus,
            activity: nexus_workbench::attention::Activity {
                status: nexus_workbench::event::ThreadStatus::Idle,
                updated_at,
                listed_at: None,
            },
        }
    }

    #[test]
    fn sessions_rank_by_title_then_recency() {
        let sessions = [
            session("a", "Fix the login", 1, false),
            session("b", "Login page polish", 2, false),
            session("c", "Relogin flow", 3, false),
            session("d", "Login old", 9, true),
            session("e", "Me", 10, false),
        ];
        let ids = |list: Vec<&CodeSession>| list.into_iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(rank_sessions(&sessions, "login", Some("e"))), ["b", "a", "c"]);
        // Nothing typed: the three last worked in, never this one or an archived one.
        assert_eq!(ids(rank_sessions(&sessions, "", Some("e"))), ["c", "b", "a"]);
    }

    #[test]
    fn pull_requests_match_numbers_by_start_and_words_anywhere() {
        let pr = |number, title: &str, head: &str| PullRequestInfo {
            number,
            title: title.into(),
            url: String::new(),
            author: String::new(),
            head: head.into(),
            base: "main".into(),
            host: Host::GitHub,
        };
        let prs = [
            pr(123, "Fix login", "fix/login"),
            pr(12, "Docs", "docs"),
            pr(45, "Speed", "perf/12"),
        ];
        let numbers = |q| {
            rank_pull_requests(&prs, q)
                .into_iter()
                .map(|pr| pr.number)
                .collect::<Vec<_>>()
        };
        assert_eq!(numbers("12"), [123, 12]);
        assert_eq!(numbers("perf"), [45]);
        assert_eq!(numbers(""), [123, 12, 45]);
    }
}
