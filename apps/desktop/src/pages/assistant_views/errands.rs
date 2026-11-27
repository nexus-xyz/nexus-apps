//! Errands, as Nexus UI's `agent-errands.tsx`: one agent, many errands, and each errand is its own thread.
//! The list groups the open ones (what needs the person, what runs), each with a still mark, its name and
//! what it needs or how it goes. Closed and archived errands fold into the History, which a search narrows. New errand
//! (⇧⌘J) starts one from the person's words.
//!
//! An errand opens as a thread, as the chat reads: a crumb, `<Assistant> › <errand>`, with its state
//! and where it started; the person's goal as their first message, the agent's steps as quiet notes, then the replies
//! about it, with the approval card inside. A ⋯ menu renames, pins, marks done, archives or hands it off to a session
//! (a New session draft with its prompt, never sent). Under the thread, a
//! composer "Reply about <errand>": the agent answers there, in the errand's own context. A draft's Confirm and Cancel
//! run here too. Opened on one errand (the sidebar's row, the chat's card, a link), the thread fills the main column
//! alone; from the strip, the list stays beside it.
//!
//! The view tells the window what it read and changed (`Moves::errands`), so the sidebar's rows and the chat's card
//! follow it. The words and the strict reads are `nexus_desktop_core::agent_errands`'s. A goal, a step and a reply are
//! the person's and the agent's own text: they are drawn as plain text, and only an http(s) link opens.

use std::collections::HashMap;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::super::{Ctx, PageId, palette, ui};
use super::{Moves, Noted, Reply, call, confirm, day, pill, primary_pill, problem, quiet, quiet_pill};
use crate::agent::{Call, Message};
use crate::pages::agent::{self as thread, Approval, Decides, Decisions, EditBoxes};
use crate::palette::Palette;
use crate::workspace::{MenuTrigger as _, menu_box, menu_item_hover};
use nexus_desktop_core::agent::{self as rules, Problem};
use nexus_desktop_core::agent_cards::Decision;
use nexus_desktop_core::agent_errands::{
    self as words, Action, Edit, Entry, Errand, ErrandView, Evidence, MenuItem, Status,
};
use nexus_desktop_core::agent_projects as projects;

mod projects_view;

enum List {
    Loading,
    Off,
    Ready,
    Failed(Problem),
}

/// A text field and what it listens to.
struct Field {
    state: Entity<TextareaState>,
    _events: Subscription,
}

pub struct ErrandsPage {
    ctx: Ctx,
    moves: Moves,
    list: List,
    errands: Vec<Errand>,
    history: Vec<Errand>,
    /// The open errands in their groups, computed when the list changed.
    groups: Vec<(&'static str, Vec<Errand>)>,
    count: String,
    /// The History: folded or open, its search, and the rows the search holds.
    history_open: bool,
    search: Field,
    found: Vec<Errand>,
    view: Option<ErrandView>,
    /// The open errand's thread, computed when it changed, and each answer's Markdown by entry and message.
    entries: Vec<Entry>,
    answers: HashMap<(usize, usize), Entity<TextViewState>>,
    /// Opened on one errand: its thread fills the main column alone.
    alone: bool,
    /// The errand to open once the list is read.
    initial: Option<String>,
    menu_open: bool,
    /// The ⋯ menu shows Move to project in its place, with a New project field while one is named.
    moving: bool,
    naming: Option<Field>,
    /// This organization's projects, from the list and the history, computed when the list changed.
    projects: Vec<String>,
    /// The project whose page shows, and its Ask Nexus field.
    project: Option<String>,
    asking: Option<Field>,
    /// The project page's errands, open and closed, computed when the list changed.
    project_rows: Vec<Errand>,
    /// The project's page from the agent API: its repositories, sessions and notes. None while it loads,
    /// for a new name, or when the agent API predates it: the page is its errands alone.
    project_view: Option<projects::ProjectView>,
    /// "What Nexus knows here"'s Add field.
    noting: Option<Field>,
    _project: Option<Task<()>>,
    renaming: Option<Field>,
    composer: Option<Field>,
    composer_empty: bool,
    /// The reply on its way, drawn as the person's bubble with the typing dots under it.
    sending: Option<String>,
    /// The New errand box, while it is open.
    new_errand: Option<Field>,
    cancelling: bool,
    refused: Option<Problem>,
    status: Option<String>,
    working: bool,
    decisions: Decisions,
    /// The inputs of the cards whose Edit is open.
    edit_boxes: EditBoxes,
    typing: Entity<crate::motion::TypingDots>,
    _list: Task<()>,
    _open: Option<Task<()>>,
    _act: Option<Task<()>>,
    _reply: Option<Task<()>>,
    /// The hand-off's step on its way, apart from `_act`, whose reply starts it.
    _handed: Option<Task<()>>,
    _deciding: HashMap<String, Task<()>>,
}

/// A text field of the view: Enter does `enter`, and a change redraws.
fn field(
    window: &mut Window,
    cx: &mut Context<ErrandsPage>,
    placeholder: String,
    text: &str,
    enter: fn(&mut ErrandsPage, &mut Window, &mut Context<ErrandsPage>),
    change: fn(&mut ErrandsPage, &str, &mut Context<ErrandsPage>),
) -> Field {
    let state = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder(placeholder)
            .submit_on_enter(true)
            .auto_grow(1, 6)
    });
    if !text.is_empty() {
        state.update(cx, |state, cx| state.set_value(text.to_string(), window, cx));
    }
    let events = cx.subscribe_in(
        &state,
        window,
        move |page, state, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { shift: false, .. } => enter(page, window, cx),
            InputEvent::Change => {
                let text = state.read(cx).value().to_string();
                change(page, &text, cx);
            }
            _ => {}
        },
    );
    Field { state, _events: events }
}

pub fn build(
    ctx: Ctx,
    moves: Moves,
    initial: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ErrandsPage> {
    cx.new(|cx| {
        let list = call(&ctx, Call::Errands, window, cx, ErrandsPage::listed);
        let search = field(
            window,
            cx,
            "Search errands".into(),
            "",
            |_, _, _| {},
            |page, query, cx| {
                page.found = words::search_errands(&page.history, query);
                cx.notify();
            },
        );
        let draft = moves.new_errand.clone();
        let project = moves.project.clone();
        let asking = project
            .as_ref()
            .map(|name| projects_view::ask_field(&moves.name(), name, window, cx));
        let mut page = ErrandsPage {
            ctx,
            moves,
            list: List::Loading,
            errands: Vec::new(),
            history: Vec::new(),
            groups: Vec::new(),
            count: String::new(),
            history_open: false,
            search,
            found: Vec::new(),
            view: None,
            entries: Vec::new(),
            answers: HashMap::new(),
            alone: initial.is_some() || project.is_some(),
            initial,
            menu_open: false,
            moving: false,
            naming: None,
            projects: Vec::new(),
            project,
            asking,
            project_rows: Vec::new(),
            project_view: None,
            noting: None,
            _project: None,
            renaming: None,
            composer: None,
            composer_empty: true,
            sending: None,
            new_errand: None,
            cancelling: false,
            refused: None,
            status: None,
            working: false,
            decisions: Decisions::default(),
            edit_boxes: EditBoxes::default(),
            typing: cx.new(|_| crate::motion::TypingDots),
            _list: list,
            _open: None,
            _act: None,
            _reply: None,
            _handed: None,
            _deciding: HashMap::new(),
        };
        if let Some(text) = draft {
            page.open_new(&text, window, cx);
        }
        page.load_project(window, cx);
        page
    })
}

impl ErrandsPage {
    fn name(&self) -> SharedString {
        self.moves.name()
    }

    fn note(&self, noted: Noted, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(note) = self.moves.errands.clone() {
            // After this update: the window updates the sidebar and the chat, never this view inside its own update.
            window.defer(cx, move |window, cx| note(noted, window, cx));
        }
    }

    fn listed(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        if !reply.ok() {
            self.list = if words::errands_off(reply.status, reply.body.as_ref()) {
                List::Off
            } else {
                List::Failed(words::errands_problem(reply.status, reply.body.as_ref()))
            };
            return;
        }
        match reply.body.as_ref().and_then(words::errand_list_of) {
            Some(errands) => {
                let history = reply.body.as_ref().map(words::errand_history_of).unwrap_or_default();
                self.set_list(errands, history, window, cx);
            }
            None => self.list = List::Failed(words::errands_problem(502, None)),
        }
        if let Some(id) = self.initial.take() {
            self.open(&id, window, cx);
        }
    }

    fn set_list(&mut self, errands: Vec<Errand>, history: Vec<Errand>, window: &mut Window, cx: &mut Context<Self>) {
        // With a history, a closed errand is in it, not in the groups.
        let open: Vec<Errand> = if history.is_empty() {
            errands.clone()
        } else {
            errands
                .iter()
                .filter(|errand| matches!(errand.status, Status::NeedsYou | Status::Running))
                .cloned()
                .collect()
        };
        self.groups = words::errand_groups(&open);
        self.count = words::count_text(&open);
        let query = self.search.state.read(cx).value().to_string();
        self.found = words::search_errands(&history, &query);
        let all: Vec<Errand> = errands.iter().chain(&history).cloned().collect();
        self.projects = projects::errand_projects(&all);
        // The list may hold a closed errand that the history has too: each shows once.
        let mut once: Vec<Errand> = Vec::new();
        for errand in all {
            if !once.iter().any(|seen| seen.id == errand.id) {
                once.push(errand);
            }
        }
        self.project_rows = self
            .project
            .as_deref()
            .map(|name| projects::project_errands(&once, name, usize::MAX))
            .unwrap_or_default();
        self.history = history;
        self.errands = errands.clone();
        self.list = List::Ready;
        self.note(Noted::Listed(errands), window, cx);
    }

    /// Read the list again: it moves with the errand. A failed read keeps the rows. A project's page reads its view too.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_project(window, cx);
        self._list = call(&self.ctx, Call::Errands, window, cx, |page, reply, window, cx| {
            if !reply.ok() {
                return;
            }
            if let Some(errands) = reply.body.as_ref().and_then(words::errand_list_of) {
                let history = reply.body.as_ref().map(words::errand_history_of).unwrap_or_default();
                page.set_list(errands, history, window, cx);
            }
        });
    }

    #[cfg(test)]
    pub fn errand(&self) -> Option<&ErrandView> {
        self.view.as_ref()
    }

    #[cfg(test)]
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    #[cfg(test)]
    pub fn decision(&self, card: &str) -> Option<&crate::pages::agent::Phase> {
        self.decisions.phase(card)
    }

    /// Opened on one errand: its thread fills the main column alone, also while it loads.
    pub fn alone(&self) -> bool {
        self.alone
    }

    /// The errand the title bar's crumb names while it is alone ("Nexus › Vendor contracts").
    pub fn crumb(&self) -> Option<String> {
        self.view
            .as_ref()
            .filter(|_| self.alone)
            .map(|view| words::errand_name(&view.errand).to_string())
    }

    #[cfg(test)]
    pub fn found(&self) -> Vec<&str> {
        self.found.iter().map(|errand| errand.id.as_str()).collect()
    }

    #[cfg(test)]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    #[cfg(test)]
    pub fn new_errand_text(&self, cx: &App) -> Option<String> {
        self.new_errand
            .as_ref()
            .map(|field| field.state.read(cx).value().to_string())
    }

    #[cfg(test)]
    pub fn act_on(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        self.act(action, window, cx);
    }

    #[cfg(test)]
    pub fn choose_item(&mut self, item: MenuItem, window: &mut Window, cx: &mut Context<Self>) {
        self.choose(item, window, cx);
    }

    #[cfg(test)]
    pub fn type_in(&mut self, what: &str, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let field = match what {
            "composer" => self.composer.as_ref(),
            "rename" => self.renaming.as_ref(),
            "new" => self.new_errand.as_ref(),
            "naming" => self.naming.as_ref(),
            "asking" => self.asking.as_ref(),
            "noting" => self.noting.as_ref(),
            _ => Some(&self.search),
        };
        if let Some(field) = field {
            field
                .state
                .update(cx, |state, cx| state.set_value(text.to_string(), window, cx));
        }
        // As the field's own Change does when the person types.
        if what == "search" {
            self.found = words::search_errands(&self.history, text);
        }
    }

    #[cfg(test)]
    pub fn press_enter(&mut self, what: &str, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            "composer" => self.send(window, cx),
            "rename" => self.save_name(window, cx),
            _ => self.start(window, cx),
        }
    }

    #[cfg(test)]
    pub fn show_history(&mut self, cx: &mut Context<Self>) {
        self.history_open = true;
        cx.notify();
    }

    /// The New errand box, holding `text` (⇧⌘J, or ⌘K's New errand with what was typed). On a project's page, its
    /// Ask Nexus field takes it instead: the errand starts in that project.
    pub fn open_new(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.view.is_none()
            && let Some(asking) = &self.asking
        {
            asking.state.update(cx, |state, cx| {
                if !text.is_empty() {
                    state.set_value(text.to_string(), window, cx);
                }
                state.focus(window, cx);
            });
            cx.notify();
            return;
        }
        let box_ = field(
            window,
            cx,
            "Meet five seed investors before the 20th".into(),
            text,
            |page, window, cx| page.start(window, cx),
            |_, _, cx| cx.notify(),
        );
        box_.state.update(cx, |state, cx| state.focus(window, cx));
        self.new_errand = Some(box_);
        cx.notify();
    }

    fn open(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(made) = Call::errand(id) else { return };
        if self.working {
            return;
        }
        self.refused = None;
        self._open = Some(call(&self.ctx, made, window, cx, |page, reply, window, cx| {
            match (reply.ok(), reply.body.as_ref().and_then(words::errand_view_of)) {
                (true, Some(view)) => {
                    page.status = Some(format!("Opened {}.", words::errand_name(&view.errand)));
                    let id = view.errand.id.clone();
                    page.set_view(Some(view), window, cx);
                    page.note(Noted::Opened(Some(id)), window, cx);
                }
                (true, None) => page.refused = Some(words::errands_problem(502, None)),
                (false, _) => page.refused = Some(words::errands_problem(reply.status, reply.body.as_ref())),
            }
        }));
        cx.notify();
    }

    /// Show `view`, or none: its thread is laid out now, not in `render`, and its composer names it.
    fn set_view(&mut self, view: Option<ErrandView>, window: &mut Window, cx: &mut Context<Self>) {
        let same = self.view.as_ref().map(|view| &view.errand.id) == view.as_ref().map(|view| &view.errand.id);
        self.menu_open = false;
        self.moving = false;
        self.naming = None;
        self.renaming = None;
        self.cancelling = false;
        self.entries = view.as_ref().map(words::entries).unwrap_or_default();
        self.answers.clear();
        for (ix, entry) in self.entries.iter().enumerate() {
            let Entry::Nexus { messages, .. } = entry else { continue };
            for (n, message) in messages.iter().enumerate() {
                if let Message::Answer { text, .. } = message {
                    let text = text.clone();
                    self.answers
                        .insert((ix, n), cx.new(|cx| TextViewState::markdown(&text, cx)));
                }
            }
        }
        if !same || self.composer.is_none() {
            self.composer = view.as_ref().map(|view| {
                field(
                    window,
                    cx,
                    words::reply_placeholder(&view.errand),
                    "",
                    |page, window, cx| page.send(window, cx),
                    |page, text, cx| {
                        let empty = text.trim().is_empty();
                        if empty != page.composer_empty {
                            page.composer_empty = empty;
                            cx.notify();
                        }
                    },
                )
            });
            self.composer_empty = true;
        }
        if view.is_none() && self.project.is_none() {
            self.alone = false;
        }
        self.view = view;
        cx.notify();
    }

    fn act(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = &self.view else { return };
        let Some(made) = Call::act(&view.errand.id, action) else {
            return;
        };
        if self.working {
            return;
        }
        self.working = true;
        self.refused = None;
        self.cancelling = false;
        self._act = Some(call(&self.ctx, made, window, cx, |page, reply, window, cx| {
            page.working = false;
            if !reply.ok() {
                page.refused = Some(words::errands_problem(reply.status, reply.body.as_ref()));
                return;
            }
            let Some(acted) = reply.body.as_ref().and_then(words::errand_acted_of) else {
                page.refused = Some(words::errands_problem(502, None));
                return;
            };
            page.status = Some(words::acted_text(&acted, &page.name()));
            let view = match acted {
                words::Acted::Confirmed(view) | words::Acted::Done(view) | words::Acted::Cancelled(view) => Some(*view),
                words::Acted::Discarded => None,
            };
            page.set_view(view, window, cx);
            page.note(Noted::Changed, window, cx);
            page.refresh(window, cx);
        }));
        cx.notify();
    }

    fn edit(&mut self, edit: Edit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = &self.view else { return };
        let Some(made) = Call::edit_errand(&view.errand.id, &edit) else {
            return;
        };
        if self.working {
            return;
        }
        self.working = true;
        self.refused = None;
        self.menu_open = false;
        self.moving = false;
        self.naming = None;
        self._act = Some(call(&self.ctx, made, window, cx, move |page, reply, window, cx| {
            page.working = false;
            if !reply.ok() {
                page.refused = Some(words::errands_problem(reply.status, reply.body.as_ref()));
                return;
            }
            let Some(view) = reply.body.as_ref().and_then(words::errand_view_of) else {
                page.refused = Some(words::errands_problem(502, None));
                return;
            };
            page.status = Some(words::edited_text(&edit));
            page.set_view(Some(view), window, cx);
            page.note(Noted::Changed, window, cx);
            page.refresh(window, cx);
        }));
        cx.notify();
    }

    /// A ⋯ menu item. Rename opens a field in the header.
    fn choose(&mut self, item: MenuItem, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = &self.view else { return };
        if item == MenuItem::Move {
            // The projects take the menu's place; the menu stays open.
            self.menu_open = true;
            self.moving = true;
            cx.notify();
            return;
        }
        self.menu_open = false;
        let edit = match item {
            MenuItem::Rename => {
                let name = words::errand_name(&view.errand).to_string();
                let rename = field(
                    window,
                    cx,
                    "Name this errand".into(),
                    &name,
                    |page, window, cx| page.save_name(window, cx),
                    |_, _, _| {},
                );
                rename.state.update(cx, |state, cx| state.focus(window, cx));
                self.renaming = Some(rename);
                cx.notify();
                return;
            }
            MenuItem::Pin => Edit {
                pinned: Some(!view.errand.pinned),
                ..Edit::default()
            },
            MenuItem::Done => Edit {
                done: true,
                ..Edit::default()
            },
            MenuItem::Archive => Edit {
                archived: Some(!view.errand.archived),
                ..Edit::default()
            },
            MenuItem::Move => return,
            MenuItem::HandOff => return self.hand_off(window, cx),
        };
        self.edit(edit, window, cx);
    }

    /// Hand off to a session: the agent API writes the errand as a session's prompt, with the kit's builder, and
    /// a New session draft opens with it and the repository, through the chat's session-card path. It never sends. The
    /// errand then records the step "Handed off to a session"; the desktop cannot know the session's link.
    fn hand_off(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = &self.view else { return };
        let (Some(read), Some(open)) = (Call::errand_handoff(&view.errand.id), self.moves.open_draft.clone()) else {
            return;
        };
        if self.working {
            return;
        }
        let id = view.errand.id.clone();
        self.working = true;
        self.refused = None;
        self._act = Some(call(&self.ctx, read, window, cx, move |page, reply, window, cx| {
            page.working = false;
            if !reply.ok() {
                page.refused = Some(words::errands_problem(reply.status, reply.body.as_ref()));
                return;
            }
            let Some(handoff) = reply.body.as_ref().and_then(words::handoff_of) else {
                page.refused = Some(words::errands_problem(502, None));
                return;
            };
            page.status = Some(words::HANDED_OFF_TEXT.to_string());
            let link = words::handoff_link(&handoff);
            // After this update: the window opens the draft, never this view inside its own update.
            window.defer(cx, move |window, cx| open(link, window, cx));
            page.handed_off(&id, window, cx);
        }));
        cx.notify();
    }

    /// The errand records the step. A failure keeps the draft open: the step is only a note.
    fn handed_off(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(made) = Call::handed_off(id) else { return };
        self._handed = Some(call(&self.ctx, made, window, cx, move |page, reply, window, cx| {
            let Some(view) = reply
                .body
                .as_ref()
                .filter(|_| reply.ok())
                .and_then(words::errand_view_of)
            else {
                return;
            };
            page.set_view(Some(view), window, cx);
            page.note(Noted::Changed, window, cx);
            page.refresh(window, cx);
        }));
    }

    fn save_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(view), Some(rename)) = (&self.view, &self.renaming) else {
            return;
        };
        let typed = rename.state.read(cx).value().to_string();
        let change = words::rename(&view.errand, &typed);
        self.renaming = None;
        match change {
            Some(edit) => self.edit(edit, window, cx),
            None => cx.notify(),
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(view), Some(composer)) = (&self.view, &self.composer) else {
            return;
        };
        if self.sending.is_some() {
            return;
        }
        let text = composer.state.read(cx).value().trim().to_string();
        let Some(made) = Call::reply_errand(&view.errand.id, &text) else {
            return;
        };
        let id = view.errand.id.clone();
        composer.state.update(cx, |state, cx| state.set_value("", window, cx));
        self.composer_empty = true;
        self.sending = Some(text);
        self.refused = None;
        self._reply = Some(call(&self.ctx, made, window, cx, move |page, reply, window, cx| {
            page.sending = None;
            if !reply.ok() {
                page.refused = Some(words::errands_problem(reply.status, reply.body.as_ref()));
                return;
            }
            let Some(answered) = reply.body.as_ref().and_then(words::errand_reply_of) else {
                page.refused = Some(words::errands_problem(502, None));
                return;
            };
            page.status = Some(format!("{} answered.", page.name()));
            match answered.errand {
                Some(view) => page.set_view(Some(view), window, cx),
                None => page.open(&id, window, cx),
            }
            page.note(Noted::Changed, window, cx);
            page.refresh(window, cx);
        }));
        cx.notify();
    }

    /// New errand's Start: the goal alone; the agent API opens it as the chat opens one, and the view shows it.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(box_) = &self.new_errand else { return };
        let goal = box_.state.read(cx).value().to_string();
        let Some(made) = Call::open_errand(&goal, None) else {
            return;
        };
        if self.working {
            return;
        }
        self.working = true;
        self.refused = None;
        self._act = Some(call(&self.ctx, made, window, cx, |page, reply, window, cx| {
            page.working = false;
            if !reply.ok() {
                page.refused = Some(words::open_problem(reply.status, reply.body.as_ref(), &page.name()));
                return;
            }
            let Some(view) = reply.body.as_ref().and_then(words::errand_opened_of) else {
                page.refused = Some(words::errands_problem(502, None));
                return;
            };
            page.new_errand = None;
            page.status = Some(format!("Started {}.", words::errand_name(&view.errand)));
            let id = view.errand.id.clone();
            page.set_view(Some(view), window, cx);
            page.note(Noted::Changed, window, cx);
            page.note(Noted::Opened(Some(id)), window, cx);
            page.refresh(window, cx);
        }));
        cx.notify();
    }
}

impl Decides for ErrandsPage {
    fn decisions(&self) -> &Decisions {
        &self.decisions
    }

    fn decisions_mut(&mut self) -> &mut Decisions {
        &mut self.decisions
    }

    /// Approve or Decline a write in the errand's thread: the same call as the chat's, then the errand read again.
    fn decide(&mut self, card: &str, decision: Decision, window: &mut Window, cx: &mut Context<Self>) {
        let Some(made) = self.decisions.start(card, decision) else {
            return;
        };
        let card_id = card.to_string();
        let task = call(&self.ctx, made, window, cx, move |page, reply, window, cx| {
            page.decisions.answered(&card_id, reply.status, reply.body.as_ref());
            if let Some(id) = page.view.as_ref().map(|view| view.errand.id.clone()) {
                page.open(&id, window, cx);
            }
            page.note(Noted::Changed, window, cx);
        });
        self._deciding.insert(card.to_string(), task);
        cx.notify();
    }

    fn edit_boxes(&self) -> &EditBoxes {
        &self.edit_boxes
    }

    fn edit_boxes_mut(&mut self) -> &mut EditBoxes {
        &mut self.edit_boxes
    }

    /// Save on a card's Edit in the errand's thread: the same call as the chat's. The edited card replaces the card in
    /// the thread as it stands, so the errand is not read again.
    fn save_edit(&mut self, card: &str, window: &mut Window, cx: &mut Context<Self>) {
        let values = self.edit_boxes.values(card, cx);
        let Some(made) = self.decisions.save_edit(card, &values) else {
            cx.notify();
            return;
        };
        let card_id = card.to_string();
        let task = call(&self.ctx, made, window, cx, move |page, reply, _window, cx| {
            page.decisions
                .edit_answered(&card_id, reply.status, reply.body.as_ref());
            if page.decisions.editing(&card_id).is_none() {
                page.edit_boxes.close(&card_id);
            }
            cx.notify();
        });
        self._deciding.insert(card.to_string(), task);
        cx.notify();
    }
}

/// An errand's still mark (rule 6: nothing moves while it waits): a dot needs you, a ring runs, a check is done,
/// a red dot failed, and a faint ring stopped.
pub fn mark(status: Status, cancelled: bool, palette: Palette) -> AnyElement {
    let dot = || div().flex_none().size(px(8.)).rounded_full();
    if cancelled {
        return dot().border(px(1.5)).border_color(palette.faint).into_any_element();
    }
    match status {
        Status::NeedsYou => dot().bg(palette.foreground).into_any_element(),
        Status::Running => dot()
            .border(px(1.5))
            .border_color(palette.foreground)
            .into_any_element(),
        Status::Done => ui::icon(Lucide::Check, palette.faint).size(px(10.)).into_any_element(),
        Status::Failed => dot().bg(palette.destructive).into_any_element(),
    }
}

/// UTC's today and tomorrow, for a deadline.
fn days() -> (String, String) {
    let now = chrono::Utc::now();
    let tomorrow = now + chrono::Duration::days(1);
    (
        now.format("%Y-%m-%d").to_string(),
        tomorrow.format("%Y-%m-%d").to_string(),
    )
}

fn local(at: &str) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.with_timezone(&chrono::Local))
}

fn link(id: impl Into<ElementId>, url: String, label: String, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .text_color(palette.foreground)
        .underline()
        .cursor_pointer()
        .child(label)
        .on_click(move |_, _, cx| crate::external_link::open(&url, cx))
}

/// The composer's box, as the chat's: the field over a bar, the 28 px round button at its end.
fn composer_box(field: &Field, enabled: bool, palette: Palette, cx: &mut Context<ErrandsPage>) -> Div {
    v_flex()
        .w_full()
        .gap(px(10.))
        .pt(px(12.))
        .pr(px(12.))
        .pb(px(10.))
        .pl(px(16.))
        .rounded(px(16.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
        .child(
            div()
                .min_h(px(20.))
                .text_size(px(14.5))
                .line_height(relative(1.45))
                .child(Textarea::new(&field.state).appearance(false)),
        )
        .child(
            h_flex().h(px(28.)).child(div().flex_1()).child(
                thread::round_button("errand-send", palette)
                    .bg(palette.primary)
                    .when(!enabled, |button| button.opacity(0.4))
                    .when(enabled, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.opacity(0.88))
                            .on_click(cx.listener(|page, _, window, cx| page.send(window, cx)))
                    })
                    .child(
                        Icon::new(Lucide::ArrowUp)
                            .size(px(16.))
                            .text_color(palette.primary_foreground),
                    ),
            ),
        )
}

impl ErrandsPage {
    /// The thread's header: the crumb `<Assistant> › <errand>`, Pin and the ⋯ menu, then its state, where it
    /// started, when, its deadline and budget, and the goal under a name the person gave it.
    fn head(&self, view: &ErrandView, palette: Palette, cx: &mut Context<Self>) -> Div {
        let errand = &view.errand;
        let cancelled = view
            .outcome
            .as_ref()
            .is_some_and(|outcome| outcome.status == "cancelled");
        let mut meta = vec![
            words::state_text(view).to_string(),
            format!("Started in {}", words::surface_name(&view.surface)),
            day(&view.created_at),
        ];
        if let Some(deadline) = &errand.deadline {
            let (today, tomorrow) = days();
            meta.push(words::deadline_text(
                deadline,
                &today,
                &tomorrow,
                &day(&format!("{deadline}T00:00:00Z")),
            ));
        }
        if let Some(budget) = &view.budget_usd {
            meta.push(format!("Budget ${budget}"));
        }
        let title: AnyElement = match &self.renaming {
            Some(rename) => div()
                .flex_1()
                .min_w(px(160.))
                .px(px(8.))
                .py(px(2.))
                .rounded(px(6.))
                .border_1()
                .border_color(palette.border)
                .text_size(px(15.))
                .child(Textarea::new(&rename.state).appearance(false))
                .into_any_element(),
            None => div()
                .min_w(px(0.))
                .truncate()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.foreground)
                .child(words::errand_name(errand).to_string())
                .into_any_element(),
        };
        let menu_open = self.menu_open;
        let more = div()
            .id("errand-more")
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .when(menu_open, |button| button.bg(palette.hover))
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("More").build(window, cx))
            .child(Icon::new(Lucide::Ellipsis).size(px(16.)).text_color(palette.muted))
            .menu_trigger(
                menu_open,
                cx,
                |page: &ErrandsPage| page.menu_open,
                |page, _, cx| {
                    page.menu_open = !page.menu_open;
                    page.moving = false;
                    page.naming = None;
                    cx.notify();
                },
            );
        let menu = menu_open.then(|| {
            let menu = menu_box("errand-menu", if self.moving { 240. } else { 200. }, palette)
                .absolute()
                .top(px(34.))
                .right_0()
                .on_mouse_down_out(cx.listener(|page, _, _, cx| {
                    page.menu_open = false;
                    page.moving = false;
                    page.naming = None;
                    cx.notify();
                }));
            if self.moving {
                return self.move_menu(view, menu, palette, cx);
            }
            // Hand off to a session needs a way to open the draft; a view without one leaves it out.
            let drafts = self.moves.open_draft.is_some();
            let items = words::menu(view)
                .into_iter()
                .filter(move |(item, _)| drafts || *item != MenuItem::HandOff);
            menu.children(items.enumerate().map(|(at, (item, label))| {
                div()
                    .id(("errand-menu-item", at))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .text_size(px(13.5))
                    .cursor_pointer()
                    .hover(|style| menu_item_hover(style, palette))
                    .on_click(cx.listener(move |page, _, window, cx| page.choose(item, window, cx)))
                    .child(label)
            }))
        });
        let pinned = errand.pinned;
        let pin_label = if pinned { "Pinned" } else { "Pin" };
        let pin = if self.working {
            quiet_pill("errand-pin", pin_label, palette)
        } else {
            pill("errand-pin", pin_label, palette).on_click(cx.listener(move |page, _, window, cx| {
                page.edit(
                    Edit {
                        pinned: Some(!pinned),
                        ..Edit::default()
                    },
                    window,
                    cx,
                )
            }))
        };
        let name = self.name();
        v_flex()
            .w_full()
            .gap(px(4.))
            .pb(px(10.))
            .border_b_1()
            .border_color(palette.border)
            .child(
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .child(mark(errand.status, cancelled, palette))
                    // Alone, the title bar's crumb names the errand, so the head does not repeat it. Rename
                    // still edits the name here.
                    .when(!self.alone, |row| {
                        row.child(
                            div()
                                .id("errand-crumb")
                                .flex_none()
                                .text_size(px(15.))
                                .text_color(palette.muted)
                                .cursor_pointer()
                                .hover(|style| style.text_color(palette.strong))
                                .on_click(cx.listener(|page, _, window, cx| {
                                    page.moves.open(&page.ctx, PageId::Agent, None, window, cx)
                                }))
                                .child(name),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(15.))
                                .text_color(palette.faint)
                                .child("›"),
                        )
                    })
                    .when(!self.alone || self.renaming.is_some(), |row| row.child(title))
                    .child(div().flex_1())
                    .child(pin)
                    .child(div().relative().child(more).children(menu)),
            )
            .child(
                h_flex()
                    .pl(px(16.))
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .text_color(palette.faint)
                    .child(meta.join(" · "))
                    // A filed errand says where it is, and opens its project's page.
                    .children(projects::in_project(errand).map(|line| {
                        let project = errand.project.clone().unwrap_or_default();
                        div()
                            .id("errand-in-project")
                            .text_color(palette.muted)
                            .cursor_pointer()
                            .hover(|style| style.text_color(palette.strong))
                            .on_click(cx.listener(move |page, _, window, cx| page.open_project(&project, window, cx)))
                            .child(format!("· {line}"))
                    })),
            )
            .when(errand.title.is_some(), |head| {
                head.child(
                    div()
                        .pl(px(16.))
                        .text_size(px(12.5))
                        .text_color(palette.muted)
                        .child(errand.goal.clone()),
                )
            })
    }

    /// One message of an agent's reply in the thread, as the chat draws it: an answer or a note is a bubble, a write
    /// that waits is the approval card.
    fn message(&self, key: (usize, usize), message: &Message, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = key.0 * 100 + key.1;
        match message {
            Message::Answer { .. } => match self.answers.get(&key) {
                Some(view) => thread::bubble(palette, true)
                    .child(crate::workspace::prose(
                        crate::external_link::text_view(view)
                            .selectable(true)
                            .on_link_click(|href, _, _, cx| thread::open_link(href, cx)),
                        palette,
                    ))
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Message::PendingAction { .. } => match Approval::of(self.decisions.resolve(message)) {
                Some(approval) => thread::approval_card(self, id, approval, cx).into_any_element(),
                None => div().into_any_element(),
            },
            Message::Refused { detail, .. } => thread::card(palette)
                .child(thread::card_title("Refused", palette))
                .child(thread::body(detail.clone(), palette))
                .into_any_element(),
            Message::Notice { text, .. } => thread::bubble(palette, true).child(text.clone()).into_any_element(),
            Message::Insight { text, url } => match rules::safe_url(url) {
                Some(url) => thread::card(palette)
                    .child(thread::body(text.clone(), palette))
                    .child(thread::link(("errand-insight", id), "Open it", url, palette))
                    .into_any_element(),
                None => thread::bubble(palette, true).child(text.clone()).into_any_element(),
            },
            Message::Forgotten { count } => thread::bubble(palette, true)
                .child(format!(
                    "{} forgot {count} {}.",
                    self.name(),
                    if *count == 1 { "note" } else { "notes" }
                ))
                .into_any_element(),
            Message::Memories { notes, .. } => thread::bubble(palette, true)
                .child(v_flex().children(notes.iter().map(|note| format!("• {}", note.text))))
                .into_any_element(),
            Message::Error { cut, .. } => thread::notice_error(
                if *cut {
                    "The answer was cut off before it finished.".into()
                } else {
                    format!("{} could not finish this answer.", self.name())
                },
                palette,
            )
            .into_any_element(),
            Message::Unknown => div().into_any_element(),
        }
    }

    /// The thread itself: the time lines, the person's bubbles, the steps as quiet notes and the agent's replies.
    fn thread(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let now = chrono::Local::now();
        let mut out = v_flex().w_full().gap(px(14.));
        for (ix, entry) in self.entries.iter().enumerate() {
            let row = match entry {
                Entry::Day(at) => match local(at) {
                    Some(at) => thread::time_line(thread::time_label(at, now), palette).into_any_element(),
                    None => continue,
                },
                Entry::You { text, .. } => h_flex()
                    .w_full()
                    .justify_end()
                    .child(thread::person_bubble(text.clone(), palette))
                    .into_any_element(),
                Entry::Step { text, urls, at } => {
                    let links: Vec<(String, String)> = urls.iter().filter_map(|url| words::link_of(url)).collect();
                    v_flex()
                        .w_full()
                        .pl(px(thread::AVATAR + 10.))
                        .gap(px(2.))
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .text_size(px(13.))
                                .text_color(palette.muted)
                                .child(div().size(px(4.)).flex_none().rounded_full().bg(palette.faint))
                                .child(text.clone()),
                        )
                        .child(
                            h_flex()
                                .pl(px(12.))
                                .gap(px(8.))
                                .text_size(px(11.5))
                                .text_color(palette.faint)
                                .children(local(at).map(|at| at.format("%H:%M").to_string()))
                                .children(links.into_iter().enumerate().map(|(n, (url, host))| {
                                    link(("errand-step-link", ix * 100 + n), url, host, palette)
                                })),
                        )
                        .into_any_element()
                }
                Entry::Nexus { messages, text, at } => {
                    let column = v_flex().flex_1().min_w(px(0.)).items_start().gap(px(4.));
                    let column = if messages.is_empty() {
                        column.child(thread::bubble(palette, true).child(text.clone()))
                    } else {
                        column.children(
                            messages
                                .iter()
                                .enumerate()
                                .map(|(n, message)| self.message((ix, n), message, palette, cx))
                                .collect::<Vec<_>>(),
                        )
                    };
                    let column = column.children(thread::reply_time(ix, local(at), String::new(), palette));
                    thread::from_nexus(ix, false, column, palette).into_any_element()
                }
            };
            out = out.child(row);
        }
        if let Some(sending) = &self.sending {
            out = out
                .child(
                    v_flex()
                        .w_full()
                        .items_end()
                        .gap(px(4.))
                        .child(thread::person_bubble(sending.clone(), palette))
                        .child(
                            div()
                                .pr(px(4.))
                                .text_size(px(11.))
                                .text_color(thread::quiet_ink(palette))
                                .child("Delivered"),
                        ),
                )
                .child(thread::from_nexus(
                    self.entries.len(),
                    true,
                    h_flex()
                        .h(px(34.))
                        .px(px(14.))
                        .rounded_tl(px(18.))
                        .rounded_tr(px(18.))
                        .rounded_br(px(18.))
                        .rounded_bl(px(6.))
                        .bg(palette.selected)
                        .child(self.typing.clone()),
                    palette,
                ));
        }
        out
    }

    /// One errand as its own thread: the header, what it needs, the thread with its cards, a draft's
    /// Confirm and Cancel, how it ended, and the composer while it is open.
    fn errand_thread(&self, view: &ErrandView, palette: Palette, cx: &mut Context<Self>) -> Div {
        let errand = &view.errand;
        let name = self.name();
        let draft = view.actions.contains(&Action::Confirm);
        let open = !view.actions.is_empty();
        let mut out = v_flex().w_full().gap(px(14.)).child(self.head(view, palette, cx));
        // The approval's own card is in the thread: it asks there, so no note above repeats it.
        let card_in_thread = errand.need == Some(words::Need::Approval)
            && view.thread.iter().any(|turn| {
                turn.messages
                    .iter()
                    .any(|message| matches!(message, Message::PendingAction { .. }))
            });
        if let Some(need) = errand.need.filter(|_| !card_in_thread) {
            let mut lines = v_flex()
                .gap(px(4.))
                .p(px(12.))
                .rounded(px(12.))
                .bg(palette.hover)
                .text_size(px(13.));
            match (need, &errand.detail) {
                (words::Need::Approval, Some(summary)) => {
                    lines = lines.child(
                        div()
                            .text_color(palette.foreground)
                            .child(format!("Asks first: {summary}")),
                    );
                }
                (words::Need::Confirm, _) => {
                    lines = lines.child(
                        div()
                            .text_color(palette.foreground)
                            .child(format!("This errand is a draft. Confirm it, and {name} starts on it.")),
                    );
                }
                _ => {}
            }
            if let Some(place) = words::where_to_answer(view) {
                lines = lines.child(div().text_color(palette.muted).child(place));
            }
            out = out.child(lines);
        }
        if !view.constraints.is_empty() {
            out = out.child(v_flex().gap(px(4.)).children(view.constraints.iter().map(|constraint| {
                h_flex()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .child(div().size(px(4.)).rounded_full().bg(palette.faint))
                    .child(constraint.clone())
            })));
        }
        out = out.child(self.thread(palette, cx));
        if let Some(outcome) = &view.outcome
            && !outcome.evidence.is_empty()
        {
            out = out.child(
                v_flex()
                    .gap(px(4.))
                    .text_size(px(13.))
                    .children(outcome.evidence.iter().enumerate().map(|(at, item)| {
                        match item {
                            Evidence::Citation(text) => div()
                                .text_color(palette.foreground)
                                .child(text.clone())
                                .into_any_element(),
                            Evidence::Link(url) => match words::link_of(url) {
                                Some((safe, _)) => {
                                    link(("errand-evidence", at), safe.clone(), safe, palette).into_any_element()
                                }
                                None => div().text_color(palette.muted).child(url.clone()).into_any_element(),
                            },
                        }
                    })),
            );
        }
        // With the menu, Mark done is in it; Confirm a draft and Cancel errand stay here.
        let confirms = draft || view.actions.contains(&Action::Cancel);
        if confirms {
            let working = self.working;
            let mut actions = h_flex().gap(px(8.)).flex_wrap();
            if draft {
                actions = actions.child(if working {
                    quiet_pill("errand-confirm", Action::Confirm.label(), palette)
                } else {
                    primary_pill("errand-confirm", Action::Confirm.label(), palette)
                        .on_click(cx.listener(|page, _, window, cx| page.act(Action::Confirm, window, cx)))
                });
            }
            if view.actions.contains(&Action::Cancel) && !self.cancelling {
                actions = actions.child(
                    pill("errand-cancel", Action::Cancel.label(), palette).on_click(cx.listener(|page, _, _, cx| {
                        page.cancelling = true;
                        cx.notify();
                    })),
                );
            }
            out = out.child(actions);
            if self.cancelling {
                let hint = if draft {
                    "The draft is deleted.".to_string()
                } else {
                    format!("{name} stops working on it.")
                };
                out = out.child(confirm(
                    "errand-confirm-cancel",
                    "Cancel this errand?",
                    &hint,
                    cx.listener(|page, _, window, cx| page.act(Action::Cancel, window, cx)),
                    cx.listener(|page, _, _, cx| {
                        page.cancelling = false;
                        cx.notify();
                    }),
                    palette,
                ));
            }
        }
        match (&self.composer, open && !draft) {
            (Some(composer), true) => {
                let enabled = !self.composer_empty && self.sending.is_none();
                out.child(composer_box(composer, enabled, palette, cx))
            }
            (_, false) if !open => out.child(quiet(
                format!("This errand is closed. Ask {name} in the chat to start a new one."),
                palette,
            )),
            _ => out,
        }
    }

    /// New errand: the goal, one line on what the errand shares, and Cancel and Start.
    fn new_box(&self, field: &Field, palette: Palette, cx: &mut Context<Self>) -> Div {
        let empty = field.state.read(cx).value().trim().is_empty();
        super::card(palette)
            .w_full()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.foreground)
                    .child("New errand"),
            )
            .child(
                div()
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(palette.border)
                    .text_size(px(14.))
                    .child(Textarea::new(&field.state).appearance(false)),
            )
            .child(quiet(
                "It shares the Assistant’s memory, accounts and grants. It gets its own thread and its own approvals.",
                palette,
            ))
            .child(
                h_flex()
                    .gap(px(8.))
                    .justify_end()
                    .child(
                        pill("errand-new-cancel", "Cancel", palette).on_click(cx.listener(|page, _, _, cx| {
                            page.new_errand = None;
                            cx.notify();
                        })),
                    )
                    .child(if self.working || empty {
                        quiet_pill("errand-new-start", "Start", palette)
                    } else {
                        primary_pill("errand-new-start", "Start", palette)
                            .on_click(cx.listener(|page, _, window, cx| page.start(window, cx)))
                    }),
            )
    }

    /// One row of the list: the still mark, the name, and what it needs or how it goes.
    fn row(
        &self,
        id: usize,
        errand: &Errand,
        current: bool,
        line: String,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let errand_id = errand.id.clone();
        h_flex()
            .id(("errand-row", id))
            .min_h(px(38.))
            .gap(px(10.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(current, |row| row.bg(palette.selected))
            .when(!current, |row| row.hover(|style| style.bg(palette.hover)))
            .child(mark(errand.status, false, palette))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .text_size(px(13.))
                                    .text_color(palette.foreground)
                                    .truncate()
                                    .child(words::errand_name(errand).to_string()),
                            )
                            .when(errand.pinned, |name| {
                                name.child(ui::icon(Lucide::Pin, palette.faint).size(px(11.)))
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.muted)
                            .truncate()
                            .child(line),
                    ),
            )
            .on_click(cx.listener(move |page, _, window, cx| page.open(&errand_id, window, cx)))
    }

    /// The list beside the thread: the count, New errand, the groups, then the History with its search.
    fn list(&self, palette: Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let open = self.view.as_ref().map(|view| view.errand.id.clone());
        let mut list = v_flex()
            .id("errands-list")
            .w(px(300.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .px(px(10.))
            .py(px(16.))
            .gap(px(2.))
            .bg(palette.sidebar)
            .child(
                h_flex()
                    .px(px(8.))
                    .pb(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(self.count.clone()),
                    )
                    .child(
                        pill("errand-new", "New errand", palette)
                            .on_click(cx.listener(|page, _, window, cx| page.open_new("", window, cx))),
                    ),
            );
        for (group, (title, errands)) in self.groups.iter().enumerate() {
            list = list.child(
                div()
                    .px(px(8.))
                    .pt(px(10.))
                    .pb(px(4.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(*title),
            );
            for (at, errand) in errands.iter().enumerate() {
                let current = open.as_deref() == Some(errand.id.as_str());
                list = list.child(self.row(
                    group * 1000 + at,
                    errand,
                    current,
                    words::errand_line(errand),
                    palette,
                    cx,
                ));
            }
        }
        if !self.history.is_empty() {
            let folded = !self.history_open;
            list = list.child(
                h_flex()
                    .id("errands-history")
                    .px(px(8.))
                    .pt(px(14.))
                    .pb(px(4.))
                    .gap(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .hover(|style| style.text_color(palette.muted))
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.history_open = !page.history_open;
                        cx.notify();
                    }))
                    .child(
                        ui::icon(
                            if folded {
                                Lucide::ChevronRight
                            } else {
                                Lucide::ChevronDown
                            },
                            palette.faint,
                        )
                        .size(px(12.)),
                    )
                    .child(format!("History · {}", self.history.len())),
            );
            if self.history_open {
                list = list.child(
                    div()
                        .mx(px(8.))
                        .my(px(4.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(palette.border)
                        .text_size(px(13.))
                        .child(Textarea::new(&self.search.state).appearance(false)),
                );
                if self.found.is_empty() {
                    list = list.child(div().px(px(8.)).child(quiet("No errand matches.", palette)));
                }
                for (at, errand) in self.found.iter().enumerate() {
                    let current = open.as_deref() == Some(errand.id.as_str());
                    let when = day(&errand.at);
                    let line = if errand.archived {
                        format!("Archived · {when}")
                    } else {
                        when
                    };
                    list = list.child(self.row(100_000 + at, errand, current, line, palette, cx));
                }
            }
        }
        list
    }
}

impl Render for ErrandsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let name = self.name();
        let outer = div().id("assistant-errands").size_full().flex();
        let lone = |child: AnyElement| v_flex().p(px(28.)).w_full().gap(px(12.)).child(child);
        match &self.list {
            List::Loading => return outer.child(lone(quiet("Loading your errands…", palette).into_any_element())),
            List::Failed(failed) => return outer.child(lone(problem(failed, palette).into_any_element())),
            List::Off => {
                return outer.child(lone(
                    ui::empty(
                        format!("Errands are not on for {name} yet"),
                        format!("When they are, each job you hand {name} shows here, with what it needs from you."),
                        palette,
                    )
                    .into_any_element(),
                ));
            }
            List::Ready
                if self.errands.is_empty()
                    && self.history.is_empty()
                    && self.view.is_none()
                    && self.new_errand.is_none()
                    && self.project.is_none() =>
            {
                return outer.child(
                    lone(
                        ui::empty(
                            "No errands yet",
                            format!(
                                "Ask {name} for something that takes several steps, and it opens an errand for it. You can also say errand: and the goal."
                            ),
                            palette,
                        )
                        .into_any_element(),
                    )
                    .child(
                        h_flex().child(
                            pill("errand-new", "New errand", palette)
                                .on_click(cx.listener(|page, _, window, cx| page.open_new("", window, cx))),
                        ),
                    ),
                );
            }
            List::Ready => {}
        }
        let widths = crate::chat_width::of(window.window_handle().window_id());
        let mut column = v_flex().w_full().max_w(px(widths.column)).gap(px(12.)).pb(px(24.));
        if let Some(status) = &self.status {
            column = column.child(quiet(status.clone(), palette));
        }
        if let Some(refused) = &self.refused {
            column = column.child(problem(refused, palette));
        }
        if let Some(field) = &self.new_errand {
            column = column.child(self.new_box(field, palette, cx));
        }
        column = match (&self.view, &self.project) {
            (Some(view), _) => column.child(self.errand_thread(view, palette, cx)),
            (None, Some(project)) => column.child(self.project_page(project, palette, cx)),
            (None, None) if self.new_errand.is_none() => column.child(quiet(
                "Open an errand to see its thread and what it needs from you.",
                palette,
            )),
            (None, None) => column,
        };
        let pane = div()
            .id("errand-pane")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .overflow_y_scroll()
            .px(px(32.))
            .py(px(20.))
            .flex()
            .flex_col()
            .items_center()
            .child(column);
        if self.alone && (self.view.is_some() || self.project.is_some()) {
            outer.child(pane)
        } else {
            outer.child(self.list(palette, cx)).child(pane)
        }
    }
}
