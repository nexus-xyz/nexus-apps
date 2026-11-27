//! The Assistant's errands in the window (brand v5 A12, A21·a, A23). The workspace keeps the list the agent
//! API gave last. Under the sidebar's Assistant row, one row per open errand, indented 16 px: its still mark, its
//! name, and "Needs you" while it waits on the person. A row opens that errand's thread. The chat opens with
//! "N errands need you" (A12·c) from the same list. ⇧⌘J, or ⌘K's New errand, opens Errands with its New errand box.
//!
//! The list is read when the account is known, when an answer in the chat ends (it may have opened one), and when the
//! Errands view reads or changes one (`Moves::errands`). With errands off at the agent API, or on a failed read, the
//! rows are not drawn, and the chat has no card.
//!
//! Errands in projects (A14·a): only the loose errands have a row under the Assistant, with "2 in projects"
//! under them. The Projects group lists each project the errands are filed in, with its errands; a project's row opens
//! its page on the Errands view, where ⇧⌘J asks Nexus about that project.

use futures::StreamExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace};
use crate::agent::Call;
use crate::pages::PageId;
use crate::pages::agent::AgentPage;
use crate::pages::assistant_views::{ErrandsPage, Gather, Noted, errand_mark};
use crate::palette::Palette;
use nexus_desktop_core::agent_errands::{self as words, Errand, Status};
use nexus_desktop_core::agent_projects as projects;

impl Workspace {
    /// Read the errands again, for the sidebar's rows and the chat's card. A failed read keeps the rows it had; with
    /// errands off, there are none.
    pub(super) fn refresh_errands(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if !self.assistant_available() {
            return;
        }
        let base = nexus_desktop_core::agent::base_url(crate::desktop_settings::load().agents_api_url.as_deref());
        let crate::agent::Stream { mut chunks, _stop } = account_gateway.agent(base, Call::Errands);
        let scope = self.assistant_scope();
        self.assistant._errands_read = Some(cx.spawn(async move |this, cx| {
            let _stop = _stop;
            let mut gather = Gather::default();
            let mut reply = None;
            while let Some(chunk) = chunks.next().await {
                if let Some(done) = gather.push(chunk) {
                    reply = Some(done);
                    break;
                }
            }
            let Some(reply) = reply else { return };
            this.update(cx, |workspace, cx| {
                // Another account since: its own read lists its errands.
                if workspace.assistant_scope() != scope {
                    return;
                }
                if reply.ok() {
                    if let Some(errands) = reply.body.as_ref().and_then(words::errand_list_of) {
                        let history = reply.body.as_ref().map(words::errand_history_of).unwrap_or_default();
                        let closed = workspace.assistant.errand_history != history;
                        workspace.assistant.errand_history = history;
                        let listed = workspace.assistant.errands != errands;
                        workspace.set_errands(errands, cx);
                        // Only the history changed: the chat's cards still follow it.
                        if closed
                            && !listed
                            && let Some(page) = workspace.assistant_kept_page()
                        {
                            workspace.give_errands(&page, cx);
                        }
                    }
                } else if words::errands_off(reply.status, reply.body.as_ref()) {
                    workspace.set_errands(Vec::new(), cx);
                }
                // "Not turned on" in the switcher: errands are off for this account.
                workspace.accounts.off = words::errands_off(reply.status, reply.body.as_ref());
                workspace.note_account_state();
            })
            .ok();
        }));
    }

    /// The list is known: the sidebar's rows and the chat's card follow it.
    pub(super) fn set_errands(&mut self, errands: Vec<Errand>, cx: &mut Context<Self>) {
        if self.assistant.errands == errands {
            return;
        }
        // Only the loose errands stay under the Assistant; a filed one is under its project (A14·a).
        self.assistant.errand_rows = words::sidebar_errands(&projects::loose_errands(&errands));
        self.assistant.in_projects = projects::in_projects_text(&errands);
        self.assistant.errand_projects = projects::errand_projects(&errands)
            .into_iter()
            .map(|name| {
                let rows = projects::project_errands(&errands, &name, projects::PROJECT_ERRANDS);
                (name, rows)
            })
            .collect();
        self.assistant.errands = errands;
        self.note_account_state();
        if let Some(page) = self.assistant_kept_page() {
            self.give_errands(&page, cx);
        }
        cx.notify();
    }

    /// The chat's errands: the overview card's (those that need the person), every errand's state for the thread's
    /// errand cards, how a row or a card opens its thread, and how a desktop session card opens its draft.
    pub(super) fn give_errands(&self, page: &Entity<AgentPage>, cx: &mut Context<Self>) {
        let needs = words::errands_needing_you(&self.assistant.errands);
        let states: Vec<Errand> = self
            .assistant
            .errands
            .iter()
            .chain(&self.assistant.errand_history)
            .cloned()
            .collect();
        let this = cx.entity().downgrade();
        let open = std::rc::Rc::new(move |id: String, window: &mut Window, cx: &mut App| {
            this.update(cx, |workspace, cx| {
                workspace.open_assistant(PageId::AgentErrands, Some(id), window, cx)
            })
            .ok();
        });
        let this = cx.entity().downgrade();
        let draft = std::rc::Rc::new(move |url: String, window: &mut Window, cx: &mut App| {
            this.update(cx, |workspace, cx| workspace.open_prompt_link(&url, window, cx))
                .ok();
        });
        let this = cx.entity().downgrade();
        let view: crate::pages::assistant_views::Open = std::rc::Rc::new(move |id, errand, window, cx| {
            this.update(cx, |workspace, cx| workspace.open_assistant(id, errand, window, cx))
                .ok();
        });
        page.update(cx, |page, cx| {
            page.set_open_view(view);
            page.set_errands(needs, open, cx);
            page.set_errand_states(states, cx);
            page.set_open_draft(draft);
        });
    }

    /// What the Errands view read or changed.
    pub(super) fn errands_noted(&mut self, noted: Noted, window: &mut Window, cx: &mut Context<Self>) {
        match noted {
            Noted::Listed(errands) => self.set_errands(errands, cx),
            Noted::Changed => self.refresh_errands(cx),
            Noted::Opened(id) => {
                // An errand opened from a project's page: its row is the current one now, not the project's.
                if id.is_some() {
                    self.assistant.open_project = None;
                }
                if self.assistant.open_errand != id {
                    self.assistant.open_errand = id;
                    cx.notify();
                }
            }
            Noted::Project(name) => self.open_errand_project(name, window, cx),
        }
    }

    /// A project's page (A14·a): Errands opens on it, with its errands and Ask Nexus about it.
    pub(super) fn open_errand_project(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.assistant_available() || !self.navigation_allowed() {
            return;
        }
        self.assistant.project_next = Some(name);
        self.open_assistant_view(PageId::AgentErrands, None, window, cx);
    }

    /// "2 in projects" under the loose errands (A14·a): it opens the Errands view. None while no open errand is filed.
    fn render_in_projects(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let text = self.assistant.in_projects.clone()?;
        Some(
            h_flex()
                .id("assistant-in-projects")
                .h(px(28.))
                .pl(px(26.))
                .pr(px(10.))
                .gap(px(8.))
                .rounded(px(8.))
                .text_size(px(13.))
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .child(Icon::new(Lucide::Folder).size(px(12.)).text_color(palette.faint))
                .child(div().flex_1().min_w(px(0.)).truncate().child(text))
                .child(Icon::new(Lucide::ChevronRight).size(px(12.)).text_color(palette.faint))
                .on_click(
                    cx.listener(|this, _, window, cx| this.open_assistant(PageId::AgentErrands, None, window, cx)),
                ),
        )
    }

    /// The Projects group (A14·a): each project the errands are filed in, in name order, with its errands under it,
    /// each under the Nexus mark. A project's row opens its page. None while no errand is filed.
    pub(super) fn render_errand_projects(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Div> {
        if !self.assistant_available() || self.assistant.errand_projects.is_empty() {
            return None;
        }
        let here = matches!(self.view, View::Page(PageId::AgentErrands));
        let current_errand = here.then_some(self.assistant.open_errand.as_deref()).flatten();
        let current_project = here.then_some(self.assistant.open_project.as_deref()).flatten();
        let row = |id: ElementId, selected: bool| {
            h_flex()
                .id(id)
                .h(px(28.))
                .pr(px(10.))
                .gap(px(8.))
                .rounded(px(8.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(selected, |row| row.bg(palette.selected).text_color(palette.foreground))
                .when(!selected, |row| {
                    row.text_color(palette.muted).hover(|style| style.bg(palette.hover))
                })
        };
        let mut group = v_flex()
            .gap(px(1.))
            .child(super::sidebar::group_head("Projects", palette));
        for (at, (name, errands)) in self.assistant.errand_projects.iter().enumerate() {
            let project = name.clone();
            group = group.child(
                row(("assistant-project", at).into(), current_project == Some(name.as_str()))
                    .pl(px(8.))
                    .child(Icon::new(Lucide::Folder).size(px(14.)).text_color(palette.icon))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(name.clone()))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.open_errand_project(project.clone(), window, cx)),
                    ),
            );
            for (n, errand) in errands.iter().enumerate() {
                let id = errand.id.clone();
                let trailing: AnyElement = match errand.status {
                    Status::NeedsYou => div()
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(palette.faint)
                        .child("Needs you")
                        .into_any_element(),
                    Status::Done => Icon::new(Lucide::Check)
                        .size(px(12.))
                        .text_color(palette.faint)
                        .into_any_element(),
                    status => errand_mark(status, false, palette),
                };
                group = group.child(
                    row(
                        ("assistant-project-errand", at * 100 + n).into(),
                        current_errand == Some(errand.id.as_str()),
                    )
                    .pl(px(26.))
                    .child(crate::pages::agent::avatar(
                        ("assistant-project-mark", at * 100 + n),
                        14.,
                        false,
                        palette,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .child(words::errand_name(errand).to_string()),
                    )
                    .child(trailing)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_assistant(PageId::AgentErrands, Some(id.clone()), window, cx)
                    })),
                );
            }
        }
        Some(group)
    }

    /// ⇧⌘J, or ⌘K's New errand (A21·e, A23·e): Errands opens with its New errand box holding `text`. Over an Errands
    /// view already open, its box opens there.
    pub(super) fn new_errand(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.assistant_available() || !self.navigation_allowed() {
            return;
        }
        if matches!(self.view, View::Page(PageId::AgentErrands))
            && let Some((_, view)) = &self.native_page
            && let Ok(page) = view.clone().downcast::<ErrandsPage>()
        {
            page.update(cx, |page, cx| page.open_new(&text, window, cx));
            return;
        }
        self.assistant.new_errand = Some(text);
        self.open_assistant_view(PageId::AgentErrands, None, window, cx);
    }

    /// The open errands under the sidebar's Assistant row (A21·a, A12): indented 16 px, 28 px high, the still mark,
    /// the name and "Needs you". None while no errand is open.
    pub(super) fn render_errand_rows(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Div> {
        if self.assistant.errand_rows.is_empty() && self.assistant.in_projects.is_none() {
            return None;
        }
        let here = matches!(self.view, View::Page(PageId::AgentErrands));
        let current = here.then_some(self.assistant.open_errand.as_deref()).flatten();
        Some(
            v_flex()
                .gap(px(1.))
                .children(self.assistant.errand_rows.iter().enumerate().map(|(at, errand)| {
                    let selected = current == Some(errand.id.as_str());
                    let id = errand.id.clone();
                    h_flex()
                        .id(("assistant-errand", at))
                        .h(px(28.))
                        .pl(px(26.))
                        .pr(px(10.))
                        .gap(px(8.))
                        .rounded(px(8.))
                        .text_size(px(13.))
                        .cursor_pointer()
                        .when(selected, |row| row.bg(palette.selected).text_color(palette.foreground))
                        .when(!selected, |row| {
                            row.text_color(palette.muted).hover(|style| style.bg(palette.hover))
                        })
                        .child(errand_mark(errand.status, false, palette))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .child(words::errand_name(errand).to_string()),
                        )
                        .when(errand.status == Status::NeedsYou, |row| {
                            row.child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.5))
                                    .text_color(palette.faint)
                                    .child("Needs you"),
                            )
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_assistant(PageId::AgentErrands, Some(id.clone()), window, cx)
                        }))
                }))
                .children(self.render_in_projects(palette, cx)),
        )
    }
}
