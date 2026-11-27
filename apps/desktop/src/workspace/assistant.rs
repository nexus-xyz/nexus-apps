//! The Assistant in the window (brand v5 A21): its page kept while the person is elsewhere, and what its sidebar row
//! says. The page (`pages/agent.rs`) lives as long as the account: leaving it does not stop an answer, and coming back
//! (the row, Cmd+J, Back) shows the same thread. Another account, a sign-out or another Gateway drops it.
//!
//! The row's states (A21·c), by shape: an unread count of Nexus's replies the person has not seen, the mark breathing
//! while an answer streams, and a filled dot with "Needs you" while the latest reply waits for an approval. "Paused"
//! wins over both while the person paused the Assistant (A11·b): the row reads `GET /agent/pause` once the account is
//! known, and the Privacy view tells it at once when the person pauses or resumes.
//!
//! The count, the conversation and the tapbacks are kept per account in a small file beside `seen.json`, so a restart
//! shows the same count, opens the same thread and keeps its reactions. Nothing of the conversation's words is kept.
//! With the policy's `AllowLocalCache` off, nothing is written.

use std::path::PathBuf;

use gpui_kit::*;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use super::{View, Workspace};
use crate::pages::PageId;
use crate::pages::agent::{AgentPage, Reactions, RowState};
use crate::pages::assistant_views::ErrandsPage;

/// The page while it lives, and the account it belongs to.
struct Kept {
    scope: String,
    page: Entity<AgentPage>,
    _changed: Subscription,
}

/// What the workspace holds for the Assistant.
#[derive(Default)]
pub(super) struct Assistant {
    kept: Option<Kept>,
    /// What the sidebar row shows.
    pub(super) row: RowState,
    /// The account whose marks `row` and `marks` hold.
    scope: Option<String>,
    marks: Marks,
    /// The Assistant's name for this account: the person's own agent profile's, Nexus until it is read or
    /// without one. The chat and the views beside it use it.
    pub(super) name: Option<SharedString>,
    _name_read: Option<Task<()>>,
    /// The errands as the agent API listed them last (A12), for the sidebar's rows and the chat's card
    /// (`assistant_errands.rs`).
    pub(super) errands: Vec<nexus_desktop_core::agent_errands::Errand>,
    /// The closed and archived errands of the same read: an errand card in the chat shows a failed one as failed.
    pub(super) errand_history: Vec<nexus_desktop_core::agent_errands::Errand>,
    /// The rows under the sidebar's Assistant row, computed when the list changed: the loose errands (A14).
    pub(super) errand_rows: Vec<nexus_desktop_core::agent_errands::Errand>,
    /// "2 in projects" under them, and the Projects group: each project with its errands (A14·a).
    pub(super) in_projects: Option<String>,
    pub(super) errand_projects: Vec<(String, Vec<nexus_desktop_core::agent_errands::Errand>)>,
    /// The errand open in Errands now: its row is the current one.
    pub(super) open_errand: Option<String>,
    /// The errand the title bar's crumb names while Errands shows it alone (A24·b), and the watch that keeps it.
    crumb: Option<String>,
    _crumb_watch: Option<Subscription>,
    /// The project whose page Errands shows now, and the one the next Errands view opens on.
    pub(super) open_project: Option<String>,
    pub(super) project_next: Option<String>,
    /// The New errand box's text for the Errands view that opens next (⇧⌘J).
    pub(super) new_errand: Option<String>,
    pub(super) _errands_read: Option<Task<()>>,
    /// The person paused the Assistant (A11·b): the row says Paused.
    pub(super) paused: bool,
    _pause: Option<Task<()>>,
}

/// The person's agent profiles, whose own one names the Assistant (`nexus_desktop_core::agent_cards::assistant_name_of`).
const PROFILES_PATH: &str = "/v1/agents/profiles";

/// What is kept on disk for an account: the thread's conversation, the replies not seen yet, and the tapbacks on each
/// conversation's bubbles (`pages/agent/reactions.rs`), which name a bubble by its place, never by its words.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Marks {
    #[serde(default)]
    pub conversation: Option<String>,
    #[serde(default)]
    pub unread: u32,
    #[serde(default, skip_serializing_if = "Reactions::is_empty")]
    pub reactions: Reactions,
}

impl Marks {
    fn of(page: &AgentPage) -> Marks {
        Marks {
            conversation: page.conversation().map(String::from),
            unread: page.row().unread,
            reactions: page.reactions().clone(),
        }
    }
}

fn file(scope: &str) -> Option<PathBuf> {
    if cfg!(test) || !crate::managed::local_cache(crate::managed::policy()) {
        return None;
    }
    let digest: String = sha2::Sha256::digest(scope.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::paths::seen_file().map(|file| file.with_file_name(format!("assistant-{digest}.json")))
}

fn load(scope: &str) -> Marks {
    file(scope)
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(scope: &str, marks: &Marks) {
    let Some(file) = file(scope) else { return };
    let Ok(text) = serde_json::to_string(marks) else { return };
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&file, text));
    if let Err(error) = written {
        eprintln!("nexus-desktop-gpui: the Assistant's marks could not be saved: {error}");
    }
}

impl Workspace {
    /// The account the Assistant belongs to: this Gateway, the organization and the person.
    pub(super) fn assistant_scope(&self) -> Option<String> {
        self.assistant_scope_of(&self.account.as_ref()?.org)
    }

    /// The scope of the same person's Assistant at another organization (`org` is its `Account::org`).
    fn assistant_scope_of(&self, org: &str) -> Option<String> {
        let account = self.account.as_ref()?;
        let person = account
            .person_id
            .as_deref()
            .or(account.user_id.as_deref())
            .unwrap_or(&account.email);
        Some(format!("{}|{}|{}", self.gateway.url(), org, person))
    }

    /// The count of replies not seen that another account's chat kept, for the switcher.
    pub(super) fn unread_of(&self, org: &str) -> u32 {
        self.assistant_scope_of(org).map_or(0, |scope| load(&scope).unread)
    }

    /// The account is known or changed: the row reads that account's marks, and another account's page goes.
    pub(super) fn assistant_account(&mut self, cx: &mut Context<Self>) {
        let scope = self.assistant_scope();
        if self.assistant.scope == scope {
            return;
        }
        self.assistant = Assistant::default();
        if let Some(scope) = scope {
            self.assistant.marks = load(&scope);
            self.accounts.off = false;
            self.assistant.row = RowState {
                unread: self.assistant.marks.unread,
                ..RowState::default()
            };
            self.assistant.scope = Some(scope);
            self.read_assistant_name(cx);
            self.refresh_errands(cx);
            self.read_pause(cx);
        }
        cx.notify();
    }

    /// Read the person's agent profiles once per account: their own profile names the Assistant. A Gateway without
    /// profiles (404) or a failed read keeps Nexus (rule 8).
    fn read_assistant_name(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if !self.agent {
            return;
        }
        let read = account_gateway.request(crate::gateway::Verb::Get, PROFILES_PATH.into(), None);
        let scope = self.assistant.scope.clone();
        self.assistant._name_read = Some(cx.spawn(async move |this, cx| {
            let body = read.await.ok();
            let name: SharedString = nexus_desktop_core::agent_cards::assistant_name_of(body.as_ref()).into();
            this.update(cx, |workspace, cx| {
                // Another account since: its own read names its Assistant.
                if workspace.assistant.scope == scope {
                    workspace.set_assistant_name(name, cx);
                }
            })
            .ok();
        }));
    }

    /// Read whether the person paused the Assistant, for the row. An agent API without the pause (404) leaves it off.
    fn read_pause(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if !self.assistant_available() {
            return;
        }
        let base = nexus_desktop_core::agent::base_url(crate::desktop_settings::load().agents_api_url.as_deref());
        let call = crate::agent::Call::Privacy(nexus_desktop_core::agent_privacy::Request::Pause);
        let crate::agent::Stream { mut chunks, _stop } = account_gateway.agent(base, call);
        let scope = self.assistant.scope.clone();
        self.assistant._pause = Some(cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            let _stop = _stop;
            let mut gather = crate::pages::assistant_views::Gather::default();
            let mut reply = None;
            while let Some(chunk) = chunks.next().await {
                if let Some(done) = gather.push(chunk) {
                    reply = Some(done);
                    break;
                }
            }
            let paused = reply
                .filter(crate::pages::assistant_views::Reply::ok)
                .and_then(|reply| reply.body)
                .and_then(|body| nexus_desktop_core::agent_privacy::pause_view_of(&body))
                .map(|view| view.paused);
            this.update(cx, |workspace, cx| {
                if workspace.assistant.scope == scope
                    && let Some(paused) = paused
                {
                    workspace.assistant_paused(paused, cx);
                }
            })
            .ok();
        }));
    }

    /// The Assistant's name is known: the kept chat names it so, and the views opened from now on.
    pub(super) fn set_assistant_name(&mut self, name: SharedString, cx: &mut Context<Self>) {
        if self.assistant.name.as_ref() == Some(&name) {
            return;
        }
        self.assistant.name = Some(name.clone());
        if let Some(kept) = &self.assistant.kept {
            kept.page.update(cx, |page, cx| page.set_name(name, cx));
        }
    }

    /// The Assistant thread's conversation: the page's while it lives, else the one kept for the account.
    pub fn assistant_conversation(&self, cx: &App) -> Option<String> {
        match &self.assistant.kept {
            Some(kept) => kept.page.read(cx).conversation().map(String::from),
            None => self.assistant.marks.conversation.clone(),
        }
    }

    /// The Mac chat bar's Open in Nexus (A24·h): the thread opens on `conversation`, read again so the bar's question and
    /// answer are in it, and scrolled to the answer.
    pub fn open_chat_bar_answer(&mut self, conversation: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_agent(Some(conversation.clone()), window, cx);
        if let Some(kept) = &self.assistant.kept {
            kept.page
                .update(cx, |page, cx| page.read_again(&conversation, window, cx));
        }
    }

    /// The chat while it lives.
    pub(super) fn assistant_kept_page(&self) -> Option<Entity<AgentPage>> {
        self.assistant.kept.as_ref().map(|kept| kept.page.clone())
    }

    /// The person paused or resumed the Assistant: the row follows.
    pub(super) fn assistant_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        if self.assistant.paused != paused {
            self.assistant.paused = paused;
            self.note_account_state();
            cx.notify();
        }
    }

    /// Signed out, or on another Gateway: nothing of the Assistant stays.
    pub(super) fn forget_assistant(&mut self) {
        self.assistant = Assistant::default();
    }

    /// The page to show, kept or built now. A notification's click on another conversation builds it anew.
    pub(super) fn assistant_page(
        &mut self,
        conversation: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<AgentPage>> {
        let scope = self.assistant_scope()?;
        if self.assistant.scope.as_deref() != Some(scope.as_str()) {
            self.assistant_account(cx);
        }
        if let Some(kept) = &self.assistant.kept
            && kept.scope == scope
            && (conversation.is_none() || conversation.as_deref() == kept.page.read(cx).conversation())
        {
            return Some(kept.page.clone());
        }
        let ctx = self.page_ctx(cx)?;
        let label: SharedString = match &self.account {
            Some(account) if account.personal => "Personal".into(),
            Some(account) => account.org_name.clone().into(),
            None => "Personal".into(),
        };
        // The thread kept for this account opens again, unless a notification names another one.
        let conversation = conversation.or_else(|| self.assistant.marks.conversation.clone());
        let person = crate::pages::agent::Person {
            first_name: self
                .account
                .as_ref()
                .and_then(|account| nexus_desktop_core::agent_cards::first_name_of(&account.name)),
            personal: self.account.as_ref().is_none_or(|account| account.personal),
            assistant: self.assistant.name.clone(),
        };
        let reactions = self.assistant.marks.reactions.clone();
        let page = crate::pages::agent::build(ctx, conversation, label, person, reactions, window, cx);
        self.give_errands(&page, cx);
        let changed = cx.observe(&page, |this, page, cx| this.assistant_changed(&page, cx));
        self.assistant.kept = Some(Kept {
            scope,
            page: page.clone(),
            _changed: changed,
        });
        self.assistant_changed(&page, cx);
        Some(page)
    }

    /// The page changed: the row follows it, and the marks are kept when they changed.
    fn assistant_changed(&mut self, page: &Entity<AgentPage>, cx: &mut Context<Self>) {
        let (row, marks) = {
            let page = page.read(cx);
            (page.row(), Marks::of(page))
        };
        if marks != self.assistant.marks {
            if let Some(scope) = &self.assistant.scope {
                save(scope, &marks);
            }
            self.assistant.marks = marks;
        }
        if row != self.assistant.row {
            // An answer ended: it may have opened or changed an errand ("On it. New errand: …").
            if self.assistant.row.thinking && !row.thinking {
                self.refresh_errands(cx);
            }
            self.assistant.row = row;
            self.note_account_state();
            cx.notify();
        }
    }

    /// Tell the kept page whether the person looks at it, at each frame of the window: looking reads its replies.
    pub(super) fn sync_assistant(&mut self, cx: &mut Context<Self>) {
        let viewing = matches!(self.view, View::Page(PageId::Agent));
        if let Some(kept) = &self.assistant.kept {
            kept.page.update(cx, |page, cx| page.set_viewing(viewing, cx));
        }
    }

    /// The palette's Hand off (A21·e), or Cmd+J over typed text: the Assistant opens and sends `text`.
    pub(super) fn hand_off(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_agent(None, window, cx);
        if !matches!(self.view, View::Page(PageId::Agent)) {
            return;
        }
        if let Some(kept) = &self.assistant.kept {
            kept.page.update(cx, |page, cx| page.hand_off(&text, window, cx));
        }
    }

    /// Whether the Assistant can open here: an agent API, signed in, and the main window, not the panel.
    pub(super) fn assistant_available(&self) -> bool {
        !self.panel && self.agent && self.account_gateway().is_some()
    }

    /// One of the Assistant's views beside its chat (`pages/assistant_views.rs`), on `errand` for Errands. Each opens
    /// fresh, so it reads the agent API again. Without an agent API it is not there, as the chat.
    pub(super) fn open_assistant_view(
        &mut self,
        id: PageId,
        errand: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ctx = match self.page_ctx(cx) {
            Some(ctx) if self.agent => ctx,
            _ => {
                self.start_new_session(window, cx);
                return;
            }
        };
        let this = cx.entity().downgrade();
        let moves = crate::pages::assistant_views::Moves {
            personal: self.account.as_ref().is_none_or(|account| account.personal),
            name: self.assistant.name.clone(),
            person: self
                .account
                .as_ref()
                .map(|account| account.name.trim().to_string())
                .filter(|name| !name.is_empty()),
            hand_off: Some(std::rc::Rc::new({
                let this = this.clone();
                move |line, window, cx| {
                    this.update(cx, |workspace, cx| workspace.hand_off(line, window, cx))
                        .ok();
                }
            })),
            open: Some(std::rc::Rc::new({
                let this = this.clone();
                move |id, errand, window, cx| {
                    this.update(cx, |workspace, cx| workspace.open_assistant(id, errand, window, cx))
                        .ok();
                }
            })),
            errands: Some(std::rc::Rc::new({
                let this = this.clone();
                move |noted, window, cx| {
                    this.update(cx, |workspace, cx| workspace.errands_noted(noted, window, cx))
                        .ok();
                }
            })),
            open_draft: Some(std::rc::Rc::new({
                let this = this.clone();
                move |url, window, cx| {
                    this.update(cx, |workspace, cx| workspace.open_prompt_link(&url, window, cx))
                        .ok();
                }
            })),
            paused: Some(std::rc::Rc::new(move |paused, _, cx| {
                this.update(cx, |workspace, cx| workspace.assistant_paused(paused, cx))
                    .ok();
            })),
            new_errand: (id == PageId::AgentErrands)
                .then(|| self.assistant.new_errand.take())
                .flatten(),
            project: (id == PageId::AgentErrands)
                .then(|| self.assistant.project_next.take())
                .flatten(),
            organization: self.account.as_ref().map(|account| {
                if account.personal {
                    "Personal".to_string()
                } else {
                    account.org_name.clone()
                }
            }),
        };
        self.assistant.open_errand = errand.clone();
        self.assistant.open_project = moves.project.clone();
        let view = crate::pages::assistant_views::build_on(id, ctx, moves, errand, window, cx);
        // The title bar names an errand opened alone once it is read (A24·b): it redraws when that name changes only.
        self.assistant.crumb = None;
        self.assistant._crumb_watch = view.clone().downcast::<ErrandsPage>().ok().map(|page| {
            cx.observe(&page, |this, page, cx| {
                let crumb = page.read(cx).crumb();
                if this.assistant.crumb != crumb {
                    this.assistant.crumb = crumb;
                    cx.notify();
                }
            })
        });
        self.show_page(id, view, cx);
    }

    /// The strip's pick, or a view's link: the chat, or another view.
    pub(super) fn open_assistant(
        &mut self,
        id: PageId,
        errand: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigation_allowed() {
            return;
        }
        if id == PageId::Agent {
            self.open_agent(None, window, cx);
        } else if id.is_assistant_view() {
            self.open_assistant_view(id, errand, window, cx);
        }
    }

    /// Whether the main area draws the strip over `id`: over the Assistant's views, while the Assistant is here. A24
    /// draws no strip over the thread (c), whose header, ⌘I, ⌘K and ⋯ reach the views, nor over an errand opened alone
    /// (b), as the web has it.
    pub(super) fn assistant_strip(&self, id: PageId, cx: &App) -> bool {
        id.is_assistant_view() && self.assistant_available() && !self.assistant_alone(cx)
    }

    /// The Errands view on screen, if it is.
    pub(super) fn assistant_errands_page(&self) -> Option<Entity<ErrandsPage>> {
        if !matches!(self.view, View::Page(PageId::AgentErrands)) {
            return None;
        }
        let (_, view) = self.native_page.as_ref()?;
        view.clone().downcast::<ErrandsPage>().ok()
    }

    /// The errand shown alone in the main column (A24·b), by its name, for the title bar's crumb.
    pub(super) fn assistant_crumb(&self) -> Option<&str> {
        self.assistant_errands_page()?;
        self.assistant.crumb.as_deref()
    }

    /// A view under the strip that moves between the views.
    pub(super) fn render_assistant(
        &self,
        id: PageId,
        view: AnyView,
        palette: crate::palette::Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity().downgrade();
        let pick = std::rc::Rc::new(move |id: PageId, window: &mut Window, cx: &mut App| {
            this.update(cx, |workspace, cx| workspace.open_assistant(id, None, window, cx))
                .ok();
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(crate::pages::assistant_views::strip(id, pick, palette))
            .child(div().flex_1().min_h(px(0.)).child(view))
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "assistant_errands_tests.rs"]
mod errands_tests;

#[cfg(test)]
#[path = "assistant_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "assistant_views_tests.rs"]
mod views_tests;

#[cfg(test)]
#[path = "assistant_proactivity_tests.rs"]
mod proactivity_tests;

#[cfg(test)]
#[path = "assistant_thread_tests.rs"]
mod thread_tests;
