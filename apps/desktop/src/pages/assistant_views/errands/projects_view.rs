//! Errands in projects on the Errands view, as Nexus UI's `errand-projects.tsx`:
//!
//! - Move to project, in the ⋯ menu's place: this organization's projects, the errand's own checked, Keep
//!   loose for a filed errand, then New project, which asks for a name, and "Acme projects only" under them.
//! - A project's page: "Acme · project", its name, Ask Nexus about it, which starts an errand there, its
//!   chats (its errands and the sessions they set up), its Artifacts and files, and what Nexus knows there
//!   from `GET /agent/projects/view`. Files come from the Gateway project's repositories: Gateway keeps
//!   no file per project. A name-only project with a Gateway project of the same name offers Link.
//!
//! A project is the name its errands share, or a Gateway project the person is in
//! (`nexus_desktop_core::agent_projects`). With Gateway's projects off, or an agent API without the view, the page is
//! its errands alone. Names, goals and notes are the person's text: they are drawn as plain text.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{ErrandsPage, Field, field, mark, pill, quiet, ui, words};
use crate::agent::Call;
use crate::pages::assistant_views::{Noted, call};
use crate::palette::Palette;
use crate::workspace::menu_item_hover;
use nexus_desktop_core::agent_errands::{Edit, ErrandView, Status};
use nexus_desktop_core::agent_projects as projects;

/// ⇧⌘J on a project's page asks Nexus about it (`main.rs` binds it to `NewErrand`).
const ASK_KEY: &str = if cfg!(target_os = "macos") {
    "⇧⌘J"
} else {
    "Ctrl+Shift+J"
};

/// A project page's Ask Nexus field: Enter starts an errand in the project.
pub(super) fn ask_field(assistant: &str, project: &str, window: &mut Window, cx: &mut Context<ErrandsPage>) -> Field {
    field(
        window,
        cx,
        projects::ask_about(assistant, project),
        "",
        |page, window, cx| page.ask_about(window, cx),
        |_, _, cx| cx.notify(),
    )
}

impl ErrandsPage {
    /// The project page shown now, for the view tests.
    #[cfg(test)]
    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }

    /// The project page's rows, by id, for the view tests.
    #[cfg(test)]
    pub fn project_rows(&self) -> Vec<&str> {
        self.project_rows.iter().map(|errand| errand.id.as_str()).collect()
    }

    /// The project's view as the page read it, for the view tests.
    #[cfg(test)]
    pub fn project_view(&self) -> Option<&projects::ProjectView> {
        self.project_view.as_ref()
    }

    /// Read the project's page, when one shows. A failed read, or an agent API without the view, leaves
    /// the page its errands alone.
    pub(super) fn load_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.project.clone() else { return };
        let id = self.project_view.as_ref().and_then(|view| view.id.clone());
        let Some(read) = Call::project_view(id.as_deref(), &name) else {
            return;
        };
        self._project = Some(call(&self.ctx, read, window, cx, |page, reply, window, cx| {
            let view = reply
                .ok()
                .then(|| reply.body.as_ref().and_then(projects::project_view_of))
                .flatten();
            if view.is_some() && page.noting.is_none() {
                page.noting = Some(field(
                    window,
                    cx,
                    "Add what Nexus should know here".into(),
                    "",
                    |page, window, cx| page.add_project_note(window, cx),
                    |_, _, cx| cx.notify(),
                ));
            }
            page.project_view = view;
        }));
    }

    /// Link: the name-only project binds to its Gateway project, and its errands and notes move there.
    pub fn link_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = &self.project_view else { return };
        let Some((id, name)) = view.link.clone() else { return };
        let Some(made) = Call::link_project(&view.name, &id) else {
            return;
        };
        if self.working {
            return;
        }
        self.working = true;
        self.refused = None;
        self._act = Some(call(&self.ctx, made, window, cx, move |page, reply, window, cx| {
            page.working = false;
            if !reply.ok() {
                page.refused = Some(words::errands_problem(reply.status, reply.body.as_ref()));
                return;
            }
            page.status = Some(format!("Linked to {name}."));
            page.note(Noted::Changed, window, cx);
            // The project is the Gateway project's now: its page goes by its name.
            page.open_project(&name, window, cx);
        }));
        cx.notify();
    }

    /// "What Nexus knows here"'s Add: a note scoped to this project.
    pub fn add_project_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(view), Some(noting)) = (&self.project_view, &self.noting) else {
            return;
        };
        let text = noting.state.read(cx).value().to_string();
        let Some(made) = Call::add_project_note(&text, view) else {
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
                page.refused = Some(words::errands_problem(reply.status, reply.body.as_ref()));
                return;
            }
            if let Some(noting) = &page.noting {
                noting.state.update(cx, |state, cx| state.set_value("", window, cx));
            }
            page.status = Some("Nexus knows it here now.".into());
            page.load_project(window, cx);
        }));
        cx.notify();
    }

    /// Forget one of the project's notes.
    pub fn forget_project_note(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(made) = Call::forget(Some(id)) else { return };
        if self.working {
            return;
        }
        self.working = true;
        self.refused = None;
        self._act = Some(call(&self.ctx, made, window, cx, |page, reply, window, cx| {
            page.working = false;
            if !reply.ok() {
                page.refused = Some(words::errands_problem(reply.status, reply.body.as_ref()));
                return;
            }
            page.status = Some("Forgotten.".into());
            page.load_project(window, cx);
        }));
        cx.notify();
    }

    /// The project page's Ask Nexus text, for the view tests.
    #[cfg(test)]
    pub fn asking_text(&self, cx: &App) -> Option<String> {
        self.asking
            .as_ref()
            .map(|field| field.state.read(cx).value().to_string())
    }

    /// Whether the ⋯ menu shows Move to project, and its projects, for the view tests.
    #[cfg(test)]
    pub fn move_menu_projects(&self) -> Option<&[String]> {
        (self.menu_open && self.moving).then_some(self.projects.as_slice())
    }

    /// Move to project's pick: file the open errand in `project`, or make it loose with None. The same place closes
    /// the menu and changes nothing.
    pub fn move_to(&mut self, project: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = &self.view else { return };
        if view.errand.project == project {
            self.menu_open = false;
            self.moving = false;
            cx.notify();
            return;
        }
        self.edit(
            Edit {
                project: Some(project),
                ..Edit::default()
            },
            window,
            cx,
        );
    }

    /// New project: a field for its name opens in the menu.
    pub fn name_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let naming = field(
            window,
            cx,
            "Project name".into(),
            "",
            |page, window, cx| page.move_to_named(window, cx),
            |_, _, cx| cx.notify(),
        );
        naming.state.update(cx, |state, cx| state.focus(window, cx));
        self.naming = Some(naming);
        cx.notify();
    }

    /// The New project field's Move: its name as the agent API reads it, or nothing when it is not one.
    pub fn move_to_named(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(naming) = &self.naming else { return };
        let typed = naming.state.read(cx).value().to_string();
        if let Some(name) = nexus_desktop_core::agent::project_name(&typed) {
            self.move_to(Some(name), window, cx);
        }
    }

    /// Open a project's page: the window opens it, so the sidebar's row is the current one.
    pub(super) fn open_project(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.note(Noted::Project(name.to_string()), window, cx);
    }

    /// Ask Nexus about the project: an errand starts there, and its thread opens.
    pub fn ask_about(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(project), Some(asking)) = (&self.project, &self.asking) else {
            return;
        };
        let goal = asking.state.read(cx).value().to_string();
        // A Gateway project starts it there by id, so the errand reads its repositories.
        let made = match self.project_view.as_ref().and_then(|view| view.id.as_deref()) {
            Some(id) => Call::open_errand_in(&goal, id),
            None => Call::open_errand(&goal, Some(project)),
        };
        let Some(made) = made else {
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
            if let Some(asking) = &page.asking {
                asking.state.update(cx, |state, cx| state.set_value("", window, cx));
            }
            page.status = Some(format!("Started {}.", words::errand_name(&view.errand)));
            let id = view.errand.id.clone();
            page.set_view(Some(view), window, cx);
            page.note(Noted::Changed, window, cx);
            page.note(Noted::Opened(Some(id)), window, cx);
            page.refresh(window, cx);
        }));
        cx.notify();
    }

    /// Move to project in the ⋯ menu's place.
    pub(super) fn move_menu(
        &self,
        view: &ErrandView,
        menu: Stateful<Div>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let current = view.errand.project.clone();
        let item = |id: (&'static str, usize)| {
            h_flex()
                .id(id)
                .gap(px(8.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .text_size(px(13.5))
                .cursor_pointer()
                .hover(|style| menu_item_hover(style, palette))
        };
        let rows = self.projects.iter().enumerate().map(|(at, project)| {
            let picked = current.as_deref() == Some(project.as_str());
            let name = project.clone();
            item(("errand-move-project", at))
                .child(ui::icon(Lucide::Folder, palette.muted))
                .child(div().flex_1().min_w(px(0.)).truncate().child(project.clone()))
                .when(picked, |row| row.child(ui::icon(Lucide::Check, palette.foreground)))
                .on_click(cx.listener(move |page, _, window, cx| page.move_to(Some(name.clone()), window, cx)))
        });
        let new_project: AnyElement = match &self.naming {
            Some(naming) => h_flex()
                .gap(px(6.))
                .px(px(6.))
                .py(px(4.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(palette.border)
                        .text_size(px(13.))
                        .child(Textarea::new(&naming.state).appearance(false)),
                )
                .child(
                    pill("errand-move-new-go", "Move", palette)
                        .on_click(cx.listener(|page, _, window, cx| page.move_to_named(window, cx))),
                )
                .into_any_element(),
            None => item(("errand-move-new", 0))
                .child(ui::icon(Lucide::FolderPlus, palette.muted))
                .child("New project")
                .on_click(cx.listener(|page, _, window, cx| page.name_new_project(window, cx)))
                .into_any_element(),
        };
        menu.child(
            div()
                .px(px(10.))
                .pt(px(4.))
                .pb(px(2.))
                .text_size(px(12.))
                .text_color(palette.faint)
                .child("Move to project"),
        )
        .children(rows)
        .when(current.is_some(), |menu| {
            menu.child(
                item(("errand-move-loose", 0))
                    .child(div().size(px(14.)))
                    .child("Keep loose")
                    .on_click(cx.listener(|page, _, window, cx| page.move_to(None, window, cx))),
            )
        })
        .child(new_project)
        .child(div().mx(px(10.)).my(px(4.)).h(px(1.)).bg(palette.border))
        .child(
            div()
                .px(px(10.))
                .pb(px(4.))
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(projects::scope_note(self.moves.organization.as_deref())),
        )
    }

    /// A project's page: its crumb, name and lede, Ask Nexus about it, then its errands.
    pub(super) fn project_page(&self, name: &str, palette: Palette, cx: &mut Context<Self>) -> Div {
        let assistant = self.name();
        let mut page = v_flex()
            .w_full()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(projects::project_crumb(self.moves.organization.as_deref())),
            )
            .child(
                div()
                    .text_size(px(22.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child(name.to_string()),
            )
            .child(quiet(
                self.project_view
                    .as_ref()
                    .map_or_else(|| projects::PROJECT_LEDE.to_string(), projects::project_lede),
                palette,
            ));
        if let Some(offer) = self
            .project_view
            .as_ref()
            .and_then(|view| projects::link_offer(view, &assistant))
        {
            let name = self
                .project_view
                .as_ref()
                .and_then(|view| view.link.as_ref())
                .map(|(_, name)| name.clone())
                .unwrap_or_default();
            page = page.child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(palette.border)
                    .child(div().flex_1().min_w(px(200.)).child(quiet(offer, palette)))
                    .child(
                        pill("errand-project-link", format!("Link to {name}"), palette)
                            .on_click(cx.listener(|page, _, window, cx| page.link_project(window, cx))),
                    ),
            );
        }
        if let Some(asking) = &self.asking {
            page = page.child(
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(10.))
                    .rounded(px(14.))
                    .bg(palette.raised)
                    .border_1()
                    .border_color(palette.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(14.))
                            .child(Textarea::new(&asking.state).appearance(false)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(palette.faint)
                            .child(ASK_KEY),
                    ),
            );
        }
        let sessions = self
            .project_view
            .as_ref()
            .map(|view| view.sessions.clone())
            .unwrap_or_default();
        if self.project_rows.is_empty() && sessions.is_empty() {
            page = page.child(ui::empty(
                format!("No errands in {name}"),
                format!("Ask {assistant} about {name} above, or move an errand here from its ⋯ menu."),
                palette,
            ));
            return self.project_sections(page, palette, cx);
        }
        page = page.child(
            div()
                .pt(px(8.))
                .text_size(px(12.))
                .text_color(palette.faint)
                .child("Chats"),
        );
        let page = page.children(self.project_rows.iter().enumerate().map(|(at, errand)| {
            let id = errand.id.clone();
            let line = if errand.status == Status::NeedsYou {
                "Needs you".to_string()
            } else {
                words::errand_line(errand)
            };
            h_flex()
                .id(("errand-project-row", at))
                .w_full()
                .gap(px(10.))
                .px(px(8.))
                .py(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .child(crate::pages::agent::avatar(
                    ("errand-project-mark", at),
                    18.,
                    false,
                    palette,
                ))
                .child(
                    div()
                        .flex_none()
                        .max_w(px(260.))
                        .truncate()
                        .text_size(px(13.5))
                        .text_color(palette.foreground)
                        .child(words::errand_name(errand).to_string()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child("Errand"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(palette.muted)
                        .child(line),
                )
                .child(mark(errand.status, false, palette))
                .on_click(cx.listener(move |page, _, window, cx| page.open(&id, window, cx)))
        }));
        let page = page.children(sessions.into_iter().enumerate().map(|(at, session)| {
            let url = session.url.clone();
            h_flex()
                .id(("errand-project-session", at))
                .w_full()
                .gap(px(10.))
                .px(px(8.))
                .py(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .child(ui::icon(Lucide::SquareTerminal, palette.muted))
                .child(
                    div()
                        .flex_none()
                        .max_w(px(260.))
                        .truncate()
                        .text_size(px(13.5))
                        .text_color(palette.foreground)
                        .child(session.title.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child("Session"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(palette.muted)
                        .child(projects::session_line(&session)),
                )
                .on_click(move |_, _, cx| open_session(&url, cx))
        }));
        self.project_sections(page, palette, cx)
    }

    /// The page's sections from its view: Artifacts and files, then What Nexus knows here.
    fn project_sections(&self, page: Div, palette: Palette, cx: &mut Context<Self>) -> Div {
        let Some(view) = &self.project_view else { return page };
        let assistant = self.name();
        let heading = |text: String| {
            div()
                .pt(px(12.))
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(text)
        };
        let files = view.repos.iter().enumerate().map(|(at, (full, url))| {
            let url = url.clone();
            h_flex()
                .id(("errand-project-repo", at))
                .w_full()
                .gap(px(10.))
                .px(px(8.))
                .py(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .child(ui::icon(Lucide::Folder, palette.muted))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(13.5))
                        .text_color(palette.foreground)
                        .child(full.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child("Repository"),
                )
                .on_click(move |_, _, cx| crate::external_link::open(&url, cx))
        });
        let notes = view.notes.iter().enumerate().map(|(at, note)| {
            let id = note.id.clone();
            h_flex()
                .id(("errand-project-note", at))
                .w_full()
                .gap(px(10.))
                .px(px(8.))
                .py(px(6.))
                .rounded(px(8.))
                .hover(|style| style.bg(palette.hover))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(13.5))
                        .text_color(palette.foreground)
                        .child(note.text.clone()),
                )
                .child(
                    pill(("errand-project-forget", at), "Forget", palette)
                        .on_click(cx.listener(move |page, _, window, cx| page.forget_project_note(&id, window, cx))),
                )
        });
        let mut page = page
            .child(heading("Artifacts and files".into()))
            .children(files)
            .child(quiet(projects::files_note(view, &assistant), palette))
            .child(heading(format!("What {assistant} knows here")));
        page = if view.notes.is_empty() {
            page.child(quiet(projects::knows_empty(view), palette))
        } else {
            page.children(notes)
        };
        if let Some(noting) = &self.noting {
            page = page.child(
                h_flex()
                    .w_full()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(palette.border)
                            .text_size(px(13.))
                            .child(Textarea::new(&noting.state).appearance(false)),
                    )
                    .child(
                        pill("errand-project-add-note", "Add", palette)
                            .on_click(cx.listener(|page, _, window, cx| page.add_project_note(window, cx))),
                    ),
            );
        }
        page
    }
}

/// A session row opens its link: a synced session's page in the browser, or a desktop draft (`nexus://new?`) in this
/// app, which never sends it.
fn open_session(url: &str, cx: &mut App) {
    if url.starts_with("nexus:") {
        crate::external_link::open_app(url, cx);
    } else {
        crate::external_link::open(url, cx);
    }
}
