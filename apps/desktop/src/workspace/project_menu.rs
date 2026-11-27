//! The new session's project menu: one search over the person's projects and GitHub's
//! repositories, and the rows it finds in sections. With nothing typed, the current project comes first. With a
//! query, the best match comes first, the matched letters are marked, and chips narrow the rows to Projects or
//! GitHub. Up, Down, Enter, Tab and Esc work from the search (`project_menu_key`).

use crate::workspace::overlay::OverlaySlot;
use std::ops::Range;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Input;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::project_match::{self, Found};
use super::{View, Workspace, glyph, menu_item_hover, picker_navigation};
use crate::cloud::CloudRepo;
use crate::palette::Palette;

/// How many GitHub repositories show before "Show all"; the search finds the rest.
const GITHUB_SHOWN: usize = 8;

/// The scrolling list's height; New project, Open a folder and the hints stay in view under it.
const LIST_HEIGHT: f32 = 352.;

const MENU_WIDTH: f32 = 420.;

/// Which rows a query looks at. The chips show while something is typed; Tab cycles them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Scope {
    #[default]
    All,
    Projects,
    GitHub,
}

impl Scope {
    const ALL: [Scope; 3] = [Scope::All, Scope::Projects, Scope::GitHub];

    fn label(self) -> &'static str {
        match self {
            Scope::All => "All",
            Scope::Projects => "Projects",
            Scope::GitHub => "GitHub",
        }
    }

    fn step(self, back: bool) -> Scope {
        let at = Scope::ALL.iter().position(|scope| *scope == self).unwrap_or(0);
        let next = if back { at + Scope::ALL.len() - 1 } else { at + 1 };
        Scope::ALL[next % Scope::ALL.len()]
    }
}

/// A row of the project menu (`project_picks`).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ProjectPick {
    NoProject,
    /// The person's own project, by id.
    Own(String),
    /// A repository's project: its copies' ids, the lead's first.
    Repo(Vec<String>),
    /// A GitHub repository with no project yet.
    GitHub(CloudRepo),
    /// "Show all N repositories", under the first GitHub rows.
    MoreGitHub,
    NewProject,
}

/// A titled group of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    /// The project picked now, first while nothing is typed.
    Current,
    /// The highest-scoring row, first while something is typed.
    Best,
    Projects,
    GitHub,
}

/// A row with the byte ranges of its name and its detail that the query matched.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Row {
    pub pick: ProjectPick,
    pub name_marks: Vec<Range<usize>>,
    pub detail_marks: Vec<Range<usize>>,
    /// The project picked now: its row has a check.
    pub picked: bool,
}

/// The menu's rows, in the order it draws them.
#[derive(Debug, Default)]
pub(super) struct MenuRows {
    pub sections: Vec<(Section, Vec<Row>)>,
    /// The query's matches in each scope (All, Projects, GitHub), for the chips.
    pub counts: [usize; 3],
    /// GitHub's matching repositories, shown or not, for "8 of 46".
    pub github_total: usize,
    /// New project is a row; it is not while its name field is open.
    pub new_project: bool,
    pub searching: bool,
}

impl MenuRows {
    /// Every row Up, Down and Enter move through: the list's, then New project.
    pub fn picks(&self) -> Vec<ProjectPick> {
        let mut picks: Vec<ProjectPick> = self
            .sections
            .iter()
            .flat_map(|(_, rows)| rows.iter().map(|row| row.pick.clone()))
            .collect();
        if self.new_project {
            picks.push(ProjectPick::NewProject);
        }
        picks
    }

    /// The scrolling list's child that draws `picks()[at]`: each section draws its title, then its rows. None for
    /// New project, which sits under the list.
    pub fn child_of(&self, at: usize) -> Option<usize> {
        let mut child = 0;
        let mut seen = 0;
        for (_, rows) in &self.sections {
            child += 1;
            if at < seen + rows.len() {
                return Some(child + at - seen);
            }
            child += rows.len();
            seen += rows.len();
        }
        None
    }

    fn lit(&self, cursor: usize) -> Option<ProjectPick> {
        let picks = self.picks();
        picks.get(picker_navigation::clamp(cursor, picks.len())).cloned()
    }
}

/// A row's words and the byte ranges in them to mark.
pub(super) struct Label {
    pub text: SharedString,
    pub marks: Vec<Range<usize>>,
}

impl Label {
    fn marked(text: impl Into<SharedString>, marks: Vec<Range<usize>>) -> Label {
        Label {
            text: text.into(),
            marks,
        }
    }
}

impl From<&'static str> for Label {
    fn from(text: &'static str) -> Label {
        Label::marked(text, Vec::new())
    }
}

impl From<String> for Label {
    fn from(text: String) -> Label {
        Label::marked(text, Vec::new())
    }
}

impl From<SharedString> for Label {
    fn from(text: SharedString) -> Label {
        Label::marked(text, Vec::new())
    }
}

/// What a project menu row shows on the right, in a slot of its own so the rows' columns line up: where its project
/// lives, as icons (this computer, Cloud, a server), or a GitHub repository's lock and how long since its last push.
pub(super) enum Badge {
    None,
    Places(Vec<(Lucide, String)>),
    GitHub { private: bool, age: Option<String> },
}

/// The better of a match on a row's name and one on its detail; a tie keeps the name's.
type Hit = (i32, Vec<Range<usize>>, Vec<Range<usize>>);

fn better(name: Option<Hit>, detail: Option<Hit>) -> Option<Hit> {
    match (name, detail) {
        (Some(name), Some(detail)) if detail.0 > name.0 => Some(detail),
        (Some(name), _) => Some(name),
        (None, detail) => detail,
    }
}

/// `owner/name` of a repository key (`github.com/owner/name`).
fn repo_path(key: &str) -> &str {
    key.strip_prefix("github.com/").unwrap_or(key)
}

/// The best row of `rows` and its score; the first wins a tie.
fn top(rows: &[(i32, Row)]) -> Option<(usize, i32)> {
    rows.iter()
        .enumerate()
        .fold(None, |best: Option<(usize, i32)>, (ix, (score, _))| match best {
            Some((_, top)) if top >= *score => best,
            _ => Some((ix, *score)),
        })
}

impl Workspace {
    /// The menu's rows for the search typed now (`MenuRows`).
    pub(super) fn project_rows(&self) -> MenuRows {
        use super::repo_groups::{cloud_repo_key, group_projects, repo_key};
        use crate::project_groups::Home;
        let query = project_match::normalize(self.repo_query());
        let searching = !query.is_empty();
        let found = |text: &str| project_match::find(&query, text);
        let plain = matches!(self.view, View::Chat);
        let cloud = self.cloud_ui.on && !plain;
        let picked_key = if cloud {
            self.cloud_ui.repo.as_ref()
        } else {
            self.cloud_ui.missing.as_ref()
        }
        .and_then(cloud_repo_key);
        let picked_plain = match (&self.groups.pending, plain) {
            (Some(Home::Plain(id)), true) => Some(id.as_str()),
            _ => None,
        };

        let mut projects: Vec<(i32, Row)> = Vec::new();
        for project in self.groups.groups.projects.iter().filter(|_| !self.is_local()) {
            if let Some(hit) = found(&project.name) {
                projects.push((
                    hit.score,
                    Row {
                        pick: ProjectPick::Own(project.id.clone()),
                        name_marks: hit.marks,
                        detail_marks: Vec::new(),
                        picked: picked_plain == Some(project.id.as_str()),
                    },
                ));
            }
        }
        let groups = group_projects(&self.projects);
        let keys: Vec<Option<String>> = groups.iter().map(|group| repo_key(&group.lead.remotes)).collect();
        for (group, key) in groups.iter().zip(&keys) {
            let by_name = found(&group.lead.name).map(|hit| (hit.score, hit.marks, Vec::new()));
            let by_repo = key
                .as_deref()
                .and_then(|key| found(repo_path(key)))
                .map(|hit| (hit.score, Vec::new(), hit.marks));
            let Some((score, name_marks, detail_marks)) = better(by_name, by_repo) else {
                continue;
            };
            let ids = group.ids();
            let picked = match (&picked_key, key) {
                (Some(picked), Some(key)) => picked == key,
                (None, _) if !cloud && !plain => self.code_form.project.as_ref().is_some_and(|id| ids.contains(id)),
                _ => false,
            };
            projects.push((
                score,
                Row {
                    pick: ProjectPick::Repo(ids),
                    name_marks,
                    detail_marks,
                    picked,
                },
            ));
        }
        // "chat" finds No project too, with nothing to mark.
        let chat = found("chat").map(|hit| Found {
            score: hit.score,
            marks: Vec::new(),
        });
        if !self.is_local()
            && let Some(hit) = project_match::best(found("No project"), chat)
        {
            projects.push((
                hit.score,
                Row {
                    pick: ProjectPick::NoProject,
                    name_marks: hit.marks,
                    detail_marks: Vec::new(),
                    picked: plain && picked_plain.is_none(),
                },
            ));
        }

        let mut github: Vec<(i32, Row)> = Vec::new();
        if !self.is_local()
            && let Some(Ok(repos)) = self.github_repos()
        {
            for repo in repos.into_iter().filter(|repo| {
                let key = cloud_repo_key(repo);
                key.is_none() || !keys.contains(&key)
            }) {
                let (owner, short_at) = match repo.name.find('/') {
                    Some(slash) => (&repo.name[..slash], slash + 1),
                    None => ("", 0),
                };
                let short = &repo.name[short_at..];
                let by_short = found(short).map(|hit| (hit.score, hit.marks, Vec::new()));
                let by_full = found(&repo.name).map(|hit| {
                    (
                        hit.score,
                        project_match::shift(&hit.marks, short_at, short.len()),
                        project_match::shift(&hit.marks, 0, owner.len()),
                    )
                });
                let Some((score, name_marks, detail_marks)) = better(by_short, by_full) else {
                    continue;
                };
                let picked = picked_key.is_some() && cloud_repo_key(&repo) == picked_key;
                github.push((
                    score,
                    Row {
                        pick: ProjectPick::GitHub(repo),
                        name_marks,
                        detail_marks,
                        picked,
                    },
                ));
            }
        }

        let counts = [projects.len() + github.len(), projects.len(), github.len()];
        let scope = if searching && !self.is_local() {
            self.cloud_ui.scope
        } else {
            Scope::All
        };
        match scope {
            Scope::All => {}
            Scope::Projects => github.clear(),
            Scope::GitHub => projects.clear(),
        }
        if searching {
            // Stable: equal scores keep the projects' and the repositories' own order (latest pushed first).
            projects.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            github.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        }
        let mut sections = Vec::new();
        // The best match first while searching, a project of the person's winning a tie with GitHub; the project
        // picked now first while not.
        let lead = if searching {
            match (top(&projects), top(&github)) {
                (Some((ix, mine)), Some((_, theirs))) if mine >= theirs => Some((Section::Best, false, ix)),
                (_, Some((ix, _))) => Some((Section::Best, true, ix)),
                (Some((ix, _)), None) => Some((Section::Best, false, ix)),
                (None, None) => None,
            }
        } else {
            projects
                .iter()
                .position(|(_, row)| row.picked)
                .map(|ix| (Section::Current, false, ix))
                .or_else(|| {
                    github
                        .iter()
                        .position(|(_, row)| row.picked)
                        .map(|ix| (Section::Current, true, ix))
                })
        };
        if let Some((section, in_github, ix)) = lead {
            let (_, row) = if in_github {
                github.remove(ix)
            } else {
                projects.remove(ix)
            };
            sections.push((section, vec![row]));
        }
        if !projects.is_empty() {
            sections.push((Section::Projects, projects.into_iter().map(|(_, row)| row).collect()));
        }
        let github_total = github.len();
        if !github.is_empty() {
            let mut rows: Vec<Row> = github.into_iter().map(|(_, row)| row).collect();
            let all = self.cloud_ui.github_all || scope == Scope::GitHub;
            if !all && rows.len() > GITHUB_SHOWN {
                rows.truncate(GITHUB_SHOWN);
                rows.push(Row {
                    pick: ProjectPick::MoreGitHub,
                    name_marks: Vec::new(),
                    detail_marks: Vec::new(),
                    picked: false,
                });
            }
            sections.push((Section::GitHub, rows));
        }
        MenuRows {
            sections,
            counts,
            github_total,
            // While its name field is open, New project is that field, not a row.
            new_project: !self.is_local() && !(self.groups.adding.is_some() && self.groups.adding_in_menu),
            searching,
        }
    }

    /// The project menu's rows in the order it draws them, for Up, Down and Enter from its search.
    pub(super) fn project_picks(&self) -> Vec<ProjectPick> {
        self.project_rows().picks()
    }

    /// The name "New project" gives the project: what is typed, unless it is a GitHub URL. None opens the name field.
    pub(super) fn new_project_name(&self) -> Option<String> {
        let typed = self.repo_query();
        (!typed.is_empty() && project_match::normalize(typed) == typed).then(|| typed.to_string())
    }

    /// Runs a project menu row, as its click does.
    pub(super) fn pick_project(&mut self, pick: ProjectPick, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_local() && !matches!(pick, ProjectPick::Repo(_)) {
            return;
        }
        match pick {
            ProjectPick::NoProject => {
                self.overlay.close_launch_menu();
                self.code_form.plain = true;
                self.code_form.remember();
                self.new_chat(window, cx);
            }
            ProjectPick::Own(id) => self.pick_own_project(id, window, cx),
            ProjectPick::Repo(ids) => self.pick_repo_project(ids, window, cx),
            ProjectPick::GitHub(repo) => self.pick_github_repo(repo, window, cx),
            // The lit row stays where it was: on the first repository the list now shows.
            ProjectPick::MoreGitHub => {
                self.cloud_ui.github_all = true;
                cx.notify();
            }
            ProjectPick::NewProject => match self.new_project_name() {
                Some(name) => self.add_project_named(&name, window, cx),
                None => self.begin_new_project(true, window, cx),
            },
        }
        self.project_picked();
        self.check_claude_trust(cx);
    }

    /// Up, Down and Enter in the project menu's search; Tab (Shift+Tab: `back`) cycles the scope while something is
    /// typed, and Esc clears what is typed before it closes the menu. Answers whether the key was the menu's.
    pub(super) fn project_menu_key(
        &mut self,
        key: &str,
        back: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match key {
            "up" | "down" => {
                let rows = self.project_rows();
                let count = rows.picks().len();
                self.cloud_ui.cursor = picker_navigation::step(self.cloud_ui.cursor, count, key == "down");
                if let Some(child) = rows.child_of(self.cloud_ui.cursor) {
                    self.cloud_ui.list_scroll.scroll_to_item(child);
                }
            }
            "enter" => {
                if let Some(pick) = self.project_rows().lit(self.cloud_ui.cursor) {
                    self.pick_project(pick, window, cx);
                }
            }
            "tab" if !self.is_local() && !self.repo_query().is_empty() => {
                self.set_project_scope(self.cloud_ui.scope.step(back))
            }
            "escape" if !self.cloud_ui.query.is_empty() => self.clear_project_search(window, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    fn set_project_scope(&mut self, scope: Scope) {
        self.cloud_ui.scope = scope;
        self.cloud_ui.cursor = 0;
        self.cloud_ui.list_scroll.scroll_to_item(0);
    }

    fn clear_project_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cloud_ui.cursor = 0;
        let search = self.repo_search().clone();
        picker_navigation::clear_search(&mut self.cloud_ui.query, &search, window, cx);
        self.cloud_ui.list_scroll.scroll_to_item(0);
        cx.notify();
    }

    /// What the session works on: the search, its scope chips while something is typed, the list (or what was
    /// searched, when nothing matched), then New project, the machine's own end (Open a folder) and the key hints.
    pub(super) fn render_project_menu(&self, plain: bool, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        // A form picked at the end (a clone, a folder on a server) takes the whole menu.
        let adding = self.render_project_menu_add(plain, palette, cx);
        if adding.1 {
            return self
                .launch_box("launch-project-menu", 360., palette, cx)
                .child(adding.0)
                .into_any_element();
        }
        let rows = self.project_rows();
        let lit = rows.lit(self.cloud_ui.cursor);
        let groups = super::repo_groups::group_projects(&self.projects);
        let now = crate::code::now_ms();
        let mut list: Vec<AnyElement> = Vec::new();
        for (section, section_rows) in &rows.sections {
            list.push(
                self.section_title(*section, &rows, section_rows.len(), palette)
                    .into_any_element(),
            );
            for row in section_rows {
                let element = self.render_pick(row, &rows, &groups, now, palette, cx);
                list.push(
                    element
                        .when(lit.as_ref() == Some(&row.pick), |row| row.bg(palette.menu_hover))
                        .into_any_element(),
                );
            }
        }
        // GitHub is still read, is not connected, or did not answer: say so where its repositories would be.
        let github_note = (!self.is_local() && (self.cloud_ui.scope != Scope::Projects || !rows.searching))
            .then(|| {
                let read = self.github_repos().map(|read| read.map(|_| ()));
                super::project_menu_github::github_note(read, crate::cloud::get().is_some())
            })
            .flatten()
            .map(|note| self.render_github_note(note, palette, cx));
        if let Some(note) = github_note {
            list.push(section_label("GitHub".into(), None, palette).into_any_element());
            list.push(note);
        }
        let nothing = rows.searching && rows.sections.is_empty();
        let name_field = self.render_new_project_field(true, palette);
        // Only the list scrolls: the search, New project and the hints stay put, and the list shrinks when the
        // menu has less room. The box itself (`menu_box`) does not scroll.
        self.launch_box("launch-project-menu", MENU_WIDTH, palette, cx)
            .overflow_hidden()
            .child(self.render_project_search(palette, cx))
            .when(rows.searching && !self.is_local(), |menu| {
                menu.child(self.render_scope_chips(&rows, palette, cx))
            })
            .child(
                // Its rows are its direct children, so Up and Down can scroll the lit one into view
                // (`MenuRows::child_of`).
                v_flex()
                    .id("launch-project-list")
                    .max_h(px(LIST_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&self.cloud_ui.list_scroll)
                    .when(nothing, |list| list.child(self.render_no_match(&rows, palette)))
                    .children(list),
            )
            .child(div().flex_none().h(px(1.)).mx(px(4.)).my(px(6.)).bg(palette.border))
            .when(!self.is_local(), |menu| {
                menu.child(match name_field {
                    Some(field) => field,
                    None => {
                        let label = match self.new_project_name() {
                            Some(name) => format!("New project “{name}”"),
                            None => "New project…".into(),
                        };
                        pick_row(
                            "launch-new-project",
                            Lucide::FolderPlus,
                            label.into(),
                            Some("A name, no code".into()),
                            Badge::None,
                            false,
                            palette,
                        )
                        .when(lit == Some(ProjectPick::NewProject), |row| row.bg(palette.menu_hover))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.pick_project(ProjectPick::NewProject, window, cx)),
                        )
                        .into_any_element()
                    }
                })
            })
            .child(adding.0)
            .child(render_hints(lit.as_ref(), rows.searching && !self.is_local(), palette))
            .into_any_element()
    }

    /// The search: a field that reads as one in both themes, with a clear button once something is typed.
    fn render_project_search(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let typed = !self.cloud_ui.query.is_empty();
        h_flex()
            .flex_none()
            .h(px(34.))
            .mb(px(6.))
            .px(px(10.))
            .gap(px(8.))
            .rounded(px(8.))
            .bg(palette.background)
            .border_1()
            .border_color(palette.border)
            .child(glyph(Lucide::Search, palette.icon))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(13.))
                    .child(Input::new(self.repo_search()).appearance(false)),
            )
            .when(typed, |field| {
                field.child(
                    div()
                        .id("launch-project-clear")
                        .flex_none()
                        .size(px(20.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|style| menu_item_hover(style, palette))
                        .tooltip(|window, cx| Tooltip::new("Clear").build(window, cx))
                        .child(glyph(Lucide::X, palette.icon).size(px(12.)))
                        .on_click(cx.listener(|this, _, window, cx| this.clear_project_search(window, cx))),
                )
            })
    }

    /// All, Projects and GitHub, each with how many rows the query matches there.
    fn render_scope_chips(&self, rows: &MenuRows, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.cloud_ui.scope;
        h_flex()
            .flex_none()
            .px(px(2.))
            .pb(px(4.))
            .gap(px(4.))
            .children(Scope::ALL.into_iter().zip(rows.counts).map(|(scope, count)| {
                let on = scope == current;
                h_flex()
                    .id(SharedString::from(format!("launch-scope-{}", scope.label())))
                    .h(px(22.))
                    .px(px(9.))
                    .gap(px(5.))
                    .rounded_full()
                    .text_size(px(11.5))
                    .cursor_pointer()
                    .when(on, |chip| chip.bg(palette.menu_hover).text_color(palette.foreground))
                    .when(!on, |chip| {
                        chip.border_1()
                            .border_color(palette.border)
                            .text_color(palette.muted)
                            .hover(|style| menu_item_hover(style, palette))
                    })
                    .child(scope.label())
                    .child(div().text_color(palette.faint).child(count.to_string()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_project_scope(scope);
                        let search = this.repo_search().clone();
                        search.update(cx, |state, cx| state.focus(window, cx));
                        cx.notify();
                    }))
            }))
    }

    /// Nothing matched: what was searched, so a miss reads as real and not as a slow load.
    fn render_no_match(&self, rows: &MenuRows, palette: Palette) -> impl IntoElement {
        let searched = match self.github_repos() {
            Some(Ok(repos)) => format!(
                "Searched your projects and {} GitHub {}.",
                repos.len(),
                if repos.len() == 1 { "repository" } else { "repositories" }
            ),
            _ => "Searched your projects.".into(),
        };
        let elsewhere = (self.cloud_ui.scope != Scope::All && rows.counts[0] > 0)
            .then(|| "Other scopes have matches: press Tab.".to_string());
        v_flex()
            .px(px(12.))
            .py(px(14.))
            .gap(px(4.))
            .items_center()
            .text_center()
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .child(format!("No project or repository matches “{}”", self.repo_query())),
            )
            .child(div().text_size(px(12.)).text_color(palette.faint).child(searched))
            .children(elsewhere.map(|line| div().text_size(px(12.)).text_color(palette.faint).child(line)))
    }

    fn section_title(&self, section: Section, rows: &MenuRows, shown: usize, palette: Palette) -> impl IntoElement {
        let (title, count): (&str, Option<String>) = match section {
            Section::Current => ("Current", None),
            Section::Best => ("Best match", None),
            Section::Projects => ("Projects", Some(shown.to_string())),
            Section::GitHub if rows.searching => ("GitHub", Some(rows.github_total.to_string())),
            Section::GitHub => ("GitHub · recently pushed", {
                let listed = shown.min(GITHUB_SHOWN);
                Some(if rows.github_total > listed {
                    format!("{listed} of {}", rows.github_total)
                } else {
                    rows.github_total.to_string()
                })
            }),
        };
        section_label(title.into(), count.map(SharedString::from), palette)
    }

    /// One row of the list, by its pick; a click runs it as Enter does.
    fn render_pick(
        &self,
        row: &Row,
        rows: &MenuRows,
        groups: &[super::repo_groups::RepoGroup<'_, ()>],
        now: i64,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        use super::repo_groups::repo_key;
        let name = |text: String| Label::marked(text, row.name_marks.clone());
        let detail = |text: String| Some(Label::marked(text, row.detail_marks.clone()));
        let element = match &row.pick {
            ProjectPick::NoProject => pick_row(
                "launch-no-project",
                Lucide::MessageSquare,
                name("No project".into()),
                Some("A chat, in Recent".into()),
                Badge::None,
                row.picked,
                palette,
            ),
            ProjectPick::Own(id) => {
                let project_name = self
                    .groups
                    .groups
                    .projects
                    .iter()
                    .find(|project| &project.id == id)
                    .map(|project| project.name.clone())
                    .unwrap_or_default();
                pick_row(
                    SharedString::from(format!("launch-own-{id}")),
                    Lucide::Folder,
                    name(project_name),
                    None,
                    Badge::None,
                    row.picked,
                    palette,
                )
            }
            ProjectPick::Repo(ids) => {
                let Some(group) = groups.iter().find(|group| &group.ids() == ids) else {
                    return div().id("launch-project-gone");
                };
                let key = repo_key(&group.lead.remotes);
                let mut places: Vec<(Lucide, String)> = Vec::new();
                for machine in group.machines() {
                    let place = self.place_of(machine, true);
                    if !places.iter().any(|(_, name)| *name == place.1) {
                        places.push(place);
                    }
                }
                // This computer reads as a laptop next to Cloud and a server, not as a folder.
                let places = places
                    .into_iter()
                    .map(|(icon, name)| match icon {
                        Lucide::Cloud | Lucide::Server => (icon, name),
                        _ => (Lucide::Laptop, name),
                    })
                    .collect();
                // owner/name, as the row's detail; a folder with no remote shows its branch.
                let shown = match key.as_deref() {
                    Some(key) => detail(repo_path(key).to_string()),
                    None => group.lead.branch.clone().map(Label::from),
                };
                pick_row(
                    SharedString::from(format!("launch-project-{}", group.lead.id)),
                    if key.is_some() {
                        Lucide::FolderGit2
                    } else {
                        Lucide::Folder
                    },
                    name(group.lead.name.clone()),
                    shown,
                    Badge::Places(places),
                    row.picked,
                    palette,
                )
            }
            ProjectPick::GitHub(repo) => {
                let (owner, short) = repo.name.split_once('/').unwrap_or(("", &repo.name));
                pick_row(
                    SharedString::from(format!("launch-github-{}", repo.name)),
                    Lucide::Github,
                    name(short.to_string()),
                    (!owner.is_empty()).then(|| detail(owner.to_string())).flatten(),
                    Badge::GitHub {
                        private: repo.private,
                        age: repo.pushed_at.map(|at| crate::code::age(at, now)),
                    },
                    row.picked,
                    palette,
                )
            }
            ProjectPick::MoreGitHub => pick_row(
                "launch-github-more",
                Lucide::ChevronDown,
                format!("Show all {} repositories", rows.github_total).into(),
                None,
                Badge::None,
                false,
                palette,
            )
            .text_color(palette.muted),
            ProjectPick::NewProject => return div().id("launch-new-project-row"),
        };
        let pick = row.pick.clone();
        element.on_click(cx.listener(move |this, _, window, cx| this.pick_project(pick.clone(), window, cx)))
    }
}

/// A section's title, with its count on the right.
fn section_label(title: SharedString, count: Option<SharedString>, palette: Palette) -> impl IntoElement {
    h_flex()
        .px(px(10.))
        .pt(px(8.))
        .pb(px(2.))
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.faint)
        .child(div().flex_1().min_w(px(0.)).truncate().child(title))
        .children(count)
}

/// The keys under the menu: Up and Down, what Enter does to the lit row, Tab while something is typed, and Esc.
fn render_hints(lit: Option<&ProjectPick>, searching: bool, palette: Palette) -> impl IntoElement {
    let enter = match lit {
        Some(ProjectPick::NoProject) => Some("chat"),
        Some(ProjectPick::Own(_) | ProjectPick::Repo(_)) => Some("open"),
        Some(ProjectPick::GitHub(_)) => Some("open in Cloud"),
        Some(ProjectPick::MoreGitHub) => Some("show all"),
        Some(ProjectPick::NewProject) => Some("create"),
        None => None,
    };
    let hint = |keys: &'static str, what: &'static str| {
        h_flex()
            .gap(px(4.))
            .child(div().font_family("Geist Mono").text_color(palette.muted).child(keys))
            .child(what)
    };
    h_flex()
        .flex_none()
        .mt(px(6.))
        .pt(px(6.))
        .px(px(10.))
        .gap(px(14.))
        .border_t_1()
        .border_color(palette.border)
        .text_size(px(11.5))
        .text_color(palette.faint)
        .child(hint("↑↓", "move"))
        .children(enter.map(|what| hint("↵", what)))
        .when(searching, |hints| hints.child(hint("Tab", "scope")))
        .child(hint("Esc", if searching { "clear" } else { "close" }))
}

/// `label` with its marks drawn strong.
fn marked(label: Label, palette: Palette) -> StyledText {
    let mark = HighlightStyle {
        color: Some(palette.strong),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    let text = label.text;
    let highlights: Vec<(Range<usize>, HighlightStyle)> = label
        .marks
        .into_iter()
        .filter(|range| range.start < range.end && range.end <= text.len())
        .filter(|range| text.is_char_boundary(range.start) && text.is_char_boundary(range.end))
        .map(|range| (range, mark))
        .collect();
    StyledText::new(text).with_highlights(highlights)
}

/// A row of the project menu: the icon, the name, the repository it works on or its owner (muted), its badge on the
/// right, and a check on the picked one.
pub(super) fn pick_row(
    id: impl Into<ElementId>,
    icon: Lucide,
    name: Label,
    detail: Option<Label>,
    badge: Badge,
    picked: bool,
    palette: Palette,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let badge_id = ElementId::NamedChild(std::sync::Arc::new(id.clone()), "badge".into());
    let badge = match badge {
        Badge::None => div(),
        Badge::Places(places) => {
            let names = places
                .iter()
                .map(|(_, name)| name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            div()
                .children(
                    places
                        .into_iter()
                        .map(|(icon, _)| glyph(icon, palette.faint).size(px(13.))),
                )
                .child(
                    div()
                        .id(badge_id)
                        .absolute()
                        .inset_0()
                        .tooltip(move |window, cx| Tooltip::new(names.clone()).build(window, cx)),
                )
        }
        Badge::GitHub { private, age } => div()
            .gap(px(4.))
            .text_size(px(11.5))
            .text_color(palette.faint)
            .when(private, |badge| {
                badge.child(glyph(Lucide::Lock, palette.faint).size(px(11.)))
            })
            .children(age),
    };
    h_flex()
        .id(id)
        .w_full()
        .h(px(30.))
        .px(px(10.))
        .gap(px(10.))
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|style| menu_item_hover(style, palette))
        .child(glyph(icon, palette.icon).size(px(15.)).flex_none())
        .child(
            div()
                .flex_none()
                .max_w(px(200.))
                .truncate()
                .text_size(px(13.5))
                .child(marked(name, palette)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_size(px(12.))
                .text_color(palette.faint)
                .children(detail.map(|detail| marked(detail, palette))),
        )
        // The badge's slot keeps its width on every row, so the icons and the checks line up.
        .child(
            badge
                .relative()
                .flex()
                .flex_none()
                .w(px(64.))
                .gap(px(6.))
                .items_center()
                .justify_end(),
        )
        .child(div().w(px(14.)).flex_none().when(picked, |slot| {
            slot.child(Icon::new(Lucide::Check).size(px(14.)).text_color(palette.foreground))
        }))
}
