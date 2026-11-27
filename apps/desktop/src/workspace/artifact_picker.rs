//! Reference Library artifacts in a session (`artifact_refs.rs`). Two sheets share one shell:
//!
//! - "Add artifacts", from the composer's "+" menu: the Library's rows with
//!   checkboxes, searched as the Library searches. Add puts each as a chip in the composer.
//! - "Add to a session", from a Library row's menu: a new chat, a new Code session, or a recent session. Add opens it
//!   with the artifact's chip in its composer. Nothing is sent until the person sends.
//!
//! Motion follows the app's menus (`motion.rs`): the scrim fades in over 160 ms while the sheet rises 8 px into place
//! on the ease-out curve, and both fade out over 100 ms when it closes. Rows' fills follow the pointer over 150 ms
//! (`motion::hover_fill`). Less motion shows and hides it at once.

use crate::workspace::glass::FloatingGround;
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::overlay::OverlaySlot;
use super::{View, Workspace, glyph, picker_navigation};
use crate::artifact_refs::ArtifactRef;
use crate::attachments::{self, Attachment};
use crate::library::{self, Kind, Row, Tab};
use crate::motion::{self, Phase};
use crate::pages::ui;
use crate::palette::Palette;

const WIDTH: f32 = 520.;
/// The Library's tabs a picker offers: shared by you holds the same pages as your own.
const TABS: [Tab; 3] = [Tab::All, Tab::Pages, Tab::With];
/// Recent sessions listed under the two new ones.
const RECENT: usize = 40;

/// A Library kind's icon, as the Library's rows draw it (`ROW_ICON`).
pub(super) fn kind_icon(kind: Kind) -> Lucide {
    match kind {
        Kind::Chat => Lucide::MessageSquare,
        Kind::Doc => Lucide::FileText,
        Kind::Sheet => Lucide::Table,
        Kind::Board => Lucide::LayoutDashboard,
        Kind::Site | Kind::All => Lucide::AppWindow,
    }
}

enum Mode {
    /// Pick artifacts for the composer on screen.
    Compose { chosen: Vec<String> },
    /// Pick where `reference` goes.
    Session { reference: ArtifactRef },
}

#[derive(Clone, Debug, PartialEq)]
enum Dest {
    NewChat,
    NewCode,
    Chat(String),
    Code(String),
}

struct Choice {
    dest: Dest,
    title: SharedString,
    icon: Lucide,
    meta: SharedString,
}

pub(super) struct ArtifactPicker {
    /// False while it leaves: it is drawn once more, fading out (`motion::presence`).
    open: bool,
    mode: Mode,
    tab: Tab,
    search: Entity<InputState>,
    query: String,
    cursor: usize,
    scroll: ScrollHandle,
    _search: Subscription,
}

impl Workspace {
    /// Open "Add artifacts" over the composer on screen.
    pub(super) fn open_artifact_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.composer_shown() {
            return;
        }
        self.show_artifact_picker(Mode::Compose { chosen: Vec::new() }, window, cx);
    }

    /// Open "Add to a session" for a Library row.
    pub(super) fn open_add_to_session(&mut self, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        let Some(reference) = ArtifactRef::from_row(row, &self.gateway.site_url()) else {
            self.library.note = Some("This artifact opens only in the browser, so a session cannot read it.".into());
            return;
        };
        self.show_artifact_picker(Mode::Session { reference }, window, cx);
    }

    fn show_artifact_picker(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = match mode {
            Mode::Compose { .. } => "Search titles and people",
            Mode::Session { .. } => "Search sessions",
        };
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscription = cx.subscribe_in(&search, window, |this, state, event: &InputEvent, window, cx| {
            let value = state.read(cx).value().to_string();
            let Some(picker) = this.artifact_picker.as_mut().filter(|picker| picker.open) else {
                return;
            };
            match event {
                InputEvent::Change => {
                    if picker_navigation::query_changed(&mut picker.query, &value) {
                        picker.cursor = 0;
                        picker.scroll.scroll_to_item(0);
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { secondary, .. } => this.artifact_picker_enter(*secondary, window, cx),
                _ => {}
            }
        });
        search.update(cx, |state, cx| state.focus(window, cx));
        // The rows come from the Library's lists: read them if this is the first look.
        if !matches!(self.library.lists, Some(Ok(_))) {
            self.reload_library();
        }
        self.overlay.close_attach_menu();
        self.artifact_picker = Some(ArtifactPicker {
            open: true,
            mode,
            tab: Tab::All,
            search,
            query: String::new(),
            cursor: 0,
            scroll: ScrollHandle::new(),
            _search: subscription,
        });
        cx.notify();
    }

    fn close_artifact_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = &mut self.artifact_picker {
            picker.open = false;
        }
        if self.composer_shown() {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    fn picker_open(&self) -> bool {
        self.artifact_picker.as_ref().is_some_and(|picker| picker.open)
    }

    /// Up, Down, Enter and Esc while the picker is open, before its search field sees them. Enter picks the row under
    /// the cursor (in "Add artifacts", it ticks or unticks it); Ctrl+Enter (Cmd+Enter) adds what is ticked.
    pub(super) fn artifact_picker_key(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.picker_open() {
            return false;
        }
        let mods = key.modifiers;
        let secondary = mods.control || mods.platform;
        if mods.alt || mods.shift || (secondary && key.key != "enter") {
            return false;
        }
        match key.key.as_str() {
            "escape" => self.close_artifact_picker(window, cx),
            "up" | "down" => {
                let count = self.picker_count();
                if let Some(picker) = &mut self.artifact_picker {
                    picker.cursor = picker_navigation::step(picker.cursor, count, key.key == "down");
                    picker.scroll.scroll_to_item(picker.cursor);
                }
                cx.notify();
            }
            "enter" => self.artifact_picker_enter(secondary, window, cx),
            _ => return false,
        }
        cx.stop_propagation();
        true
    }

    fn artifact_picker_enter(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.artifact_picker.as_ref().filter(|picker| picker.open) else {
            return;
        };
        let cursor = picker.cursor;
        match &picker.mode {
            Mode::Compose { chosen } => {
                if secondary {
                    if !chosen.is_empty() {
                        self.add_chosen_artifacts(window, cx);
                    }
                } else if let Some(key) = self.picker_rows().get(cursor).map(|row| row.key.clone()) {
                    self.toggle_artifact(&key, cx);
                }
            }
            Mode::Session { .. } => {
                if let Some(choice) = self.session_choices().into_iter().nth(cursor) {
                    self.add_to_session(choice.dest, window, cx);
                }
            }
        }
    }

    fn picker_count(&self) -> usize {
        match self.artifact_picker.as_ref().map(|picker| &picker.mode) {
            Some(Mode::Compose { .. }) => self.picker_rows().len(),
            Some(Mode::Session { .. }) => self.session_choices().len(),
            None => 0,
        }
    }

    /// The Library rows a session can read, in the picker's tab and search.
    fn picker_rows(&self) -> Vec<Row> {
        let (Some(picker), Some(Ok(lists))) = (&self.artifact_picker, &self.library.lists) else {
            return Vec::new();
        };
        let site = self.gateway.site_url();
        lists
            .visible(picker.tab, Kind::All, &picker.query)
            .into_iter()
            .filter(|row| ArtifactRef::from_row(row, &site).is_some())
            .cloned()
            .collect()
    }

    /// A new chat, a new Code session, then recent sessions, newest first, as the search finds them.
    fn session_choices(&self) -> Vec<Choice> {
        let query = self
            .artifact_picker
            .as_ref()
            .map(|picker| picker.query.trim().to_lowercase())
            .unwrap_or_default();
        let matches = |title: &str| query.is_empty() || title.to_lowercase().contains(&query);
        let mut recent: Vec<(i64, Choice)> = self
            .sessions
            .iter()
            .map(|session| {
                (
                    session.at_ms.unwrap_or(0),
                    Choice {
                        dest: Dest::Chat(session.id.clone()),
                        title: session.title.clone().into(),
                        icon: Lucide::MessageSquare,
                        meta: "Chat".into(),
                    },
                )
            })
            .chain(
                self.projects
                    .iter()
                    .flat_map(|project| &project.sessions)
                    .map(|session| {
                        (
                            session.updated_at,
                            Choice {
                                dest: Dest::Code(session.id.clone()),
                                title: session.title.clone().into(),
                                icon: Lucide::SquareTerminal,
                                meta: "Code".into(),
                            },
                        )
                    }),
            )
            .filter(|(_, choice)| matches(&choice.title))
            .collect();
        recent.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        let fresh = [
            (Dest::NewChat, "New chat", Lucide::MessageSquarePlus),
            (Dest::NewCode, "New Code session", Lucide::SquareTerminal),
        ];
        fresh
            .into_iter()
            .filter(|(_, title, _)| matches(title))
            .map(|(dest, title, icon)| Choice {
                dest,
                title: title.into(),
                icon,
                meta: "New".into(),
            })
            .chain(recent.into_iter().take(RECENT).map(|(_, choice)| choice))
            .collect()
    }

    fn toggle_artifact(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(ArtifactPicker {
            mode: Mode::Compose { chosen },
            ..
        }) = &mut self.artifact_picker
        {
            match chosen.iter().position(|known| known == key) {
                Some(at) => {
                    chosen.remove(at);
                }
                None if chosen.len() < crate::artifact_refs::MAX => chosen.push(key.to_string()),
                None => {}
            }
        }
        cx.notify();
    }

    fn add_chosen_artifacts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (
            Some(ArtifactPicker {
                mode: Mode::Compose { chosen },
                ..
            }),
            Some(Ok(lists)),
        ) = (&self.artifact_picker, &self.library.lists)
        else {
            return;
        };
        let site = self.gateway.site_url();
        let refs: Vec<_> = chosen
            .iter()
            .filter_map(|key| lists.find(key))
            .filter_map(|row| ArtifactRef::from_row(row, &site))
            .collect();
        self.close_artifact_picker(window, cx);
        self.add_artifacts(refs, window, cx);
    }

    fn add_to_session(&mut self, dest: Dest, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ArtifactPicker {
            mode: Mode::Session { reference },
            ..
        }) = &self.artifact_picker
        else {
            return;
        };
        let reference = reference.clone();
        self.close_artifact_picker(window, cx);
        match dest {
            Dest::NewChat => self.new_chat(window, cx),
            Dest::NewCode => self.open_new_code(window, cx),
            Dest::Chat(id) => {
                if let Some(session) = self.sessions.iter().find(|session| session.id == id).cloned() {
                    self.open(&session, window, cx);
                }
            }
            Dest::Code(id) => self.open_code(&id, window, cx),
        }
        self.add_artifacts(vec![reference], window, cx);
        self.focus_composer(window, cx);
    }

    /// Put `refs` in the composer on screen as chips. Each source is read now, with this app's sign-in: a chat sends it
    /// as a data file (`attachments::chat_body`), and a Code session as a file on its computer
    /// (`Draft::take_artifacts`), so its agent never depends on a `nexus` CLI that may reach another Gateway. A source
    /// it cannot read, or one larger than 4 MB, goes as its reference only.
    pub(super) fn add_artifacts(&mut self, refs: Vec<ArtifactRef>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if refs.is_empty() || !self.composer_shown() {
            return;
        }
        self.sync_draft(window, cx);
        // The draft they were added to, even if another session is open when the sources arrive.
        let owner = self.draft_owner(cx);
        let reads: Vec<_> = refs
            .iter()
            .map(|reference| account_gateway.text(reference.source_path()))
            .collect();
        self.attaching += 1;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let sources = futures::future::join_all(reads).await;
            let _ = this.update(cx, |this, cx| {
                this.attaching -= 1;
                let mut unread = Vec::new();
                let files = refs
                    .into_iter()
                    .zip(sources)
                    .map(|(reference, source)| {
                        let source = source
                            .ok()
                            .filter(|text| text.len() <= attachments::ARTIFACT_SOURCE_BYTES);
                        if source.is_none() {
                            unread.push(reference.title.clone());
                        }
                        Attachment::artifact(reference, source)
                    })
                    .collect();
                let problem = this.merge_attachments(
                    owner,
                    attachments::Prepared {
                        files,
                        ..Default::default()
                    },
                    cx,
                );
                let unread = (!unread.is_empty()).then(|| {
                    format!(
                        "Nexus could not attach the source of {}, so the agent gets its reference only.",
                        unread.join(", ")
                    )
                });
                this.composer_note = problem.or(unread).map(Into::into);
                cx.notify();
            });
        })
        .detach();
    }

    /// What a chip's click does: open the artifact beside the chat, at the version it was added at.
    pub(super) fn artifact_opener(&self, cx: &Context<Self>) -> Rc<dyn Fn(ArtifactRef, &mut Window, &mut App)> {
        let workspace = cx.weak_entity();
        Rc::new(move |reference, _, cx| {
            let _ = workspace.update(cx, |this, cx| this.open_artifact_ref(&reference, cx));
        })
    }

    /// Open a referenced artifact in the Workbench's Artifacts tab, by the path the Library and a page card take
    /// (`frame_page_version`, `frame_artifact`, `chat_text`, then `Event::Framed` → `open_page`). A click is the
    /// person's, so the Workbench shows even in a Code session's manual mode, as a page card's Open does.
    pub(super) fn open_artifact_ref(&mut self, reference: &ArtifactRef, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        use crate::artifact_refs::Target;
        let org = self.account.as_ref().map(|account| account.org.clone());
        let path = |tail: String| match &org {
            Some(org) => format!("/app/{org}{tail}"),
            None => format!("/app?next={tail}"),
        };
        crate::web::warm();
        self.reveal_manual_workbench();
        self.page_opening_auto = false;
        match &reference.target {
            Target::Page(id) => {
                let at = path(format!("/pages/{id}"));
                match reference.version {
                    Some(version) => {
                        self.page_opening = Some((id.clone(), version));
                        account_gateway.frame_page_version(id.clone(), version, at);
                    }
                    None => account_gateway.frame_page(id.clone(), at),
                }
            }
            Target::Share(id) => account_gateway.frame_artifact(id.clone(), path(format!("/artifacts/{id}"))),
            Target::Chat(id) => {
                account_gateway.chat_text(id.clone(), reference.title.clone(), path(format!("/artifacts/{id}")))
            }
        }
        cx.notify();
    }

    pub(super) fn render_artifact_picker(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let phase = motion::presence("artifact-picker", self.picker_open(), cx)?;
        let picker = self.artifact_picker.as_ref()?;
        let body = match &picker.mode {
            Mode::Compose { chosen } => self.render_compose(picker, chosen, palette, cx),
            Mode::Session { reference } => self.render_session(picker, reference, palette, cx),
        };
        let sheet = v_flex()
            .id("artifact-picker")
            .debug_selector(|| "artifact-picker".into())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(WIDTH))
            .max_w(relative(0.94))
            .max_h(relative(0.86))
            .rounded(px(12.))
            .floating_ground(12., palette.menu, true, &palette)
            .border_1()
            .border_color(super::share_ui::ring(palette))
            .shadow(palette.pop_shadow(palette.border))
            .text_size(px(13.))
            .text_color(palette.foreground)
            .child(body);
        let scrim = div().absolute().inset_0().bg(palette.scrim);
        let scrim = match phase {
            Phase::Open { epoch } => scrim
                .with_animation(
                    ("artifact-picker-scrim", epoch),
                    Animation::new(motion::MENU_IN).with_easing(motion::ease_out),
                    |scrim, t| scrim.opacity(t),
                )
                .into_any_element(),
            Phase::Leaving { since, .. } => {
                let t = (since.elapsed().as_secs_f32() / motion::MENU_OUT.as_secs_f32()).clamp(0., 1.);
                scrim.opacity(1. - motion::ease_out(t)).into_any_element()
            }
        };
        Some(
            deferred(
                div()
                    .id("artifact-picker-overlay")
                    .absolute()
                    .inset_0()
                    .size_full()
                    .occlude()
                    .child(scrim)
                    .child(
                        div()
                            .id("artifact-picker-frame")
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| this.close_artifact_picker(window, cx)),
                            )
                            .child(phase.menu("artifact-picker-sheet", 8., sheet)),
                    ),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }

    fn render_compose(
        &self,
        picker: &ArtifactPicker,
        chosen: &[String],
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tabs = h_flex().px(px(20.)).pb(px(8.)).gap(px(4.)).children(TABS.map(|tab| {
            pill(
                ("artifact-picker-tab", tab as usize),
                tab.label(),
                tab == picker.tab,
                palette,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(picker) = &mut this.artifact_picker {
                    picker.tab = tab;
                    picker.cursor = 0;
                    picker.scroll.scroll_to_item(0);
                }
                cx.notify();
            }))
        }));
        let list: AnyElement =
            match &self.library.lists {
                None => quiet("Loading artifacts…", palette).into_any_element(),
                Some(Err(error)) => {
                    v_flex()
                        .gap(px(8.))
                        .child(quiet(format!("Couldn't load artifacts. {error}"), palette))
                        .child(div().px(px(20.)).child(
                            ui::button("artifact-picker-retry", "Try again", palette).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.library.lists = None;
                                    this.reload_library();
                                    cx.notify();
                                },
                            )),
                        ))
                        .into_any_element()
                }
                Some(Ok(_)) => {
                    let rows = self.picker_rows();
                    if rows.is_empty() {
                        quiet(
                            if picker.query.is_empty() {
                                "Nothing here yet. Pages and shared chats show here."
                            } else {
                                "No artifact matches this search."
                            },
                            palette,
                        )
                        .into_any_element()
                    } else {
                        let now = library::chrono_lite::now();
                        let cursor = picker_navigation::clamp(picker.cursor, rows.len());
                        scroll_list(&picker.scroll, palette)
                            .children(rows.into_iter().enumerate().map(|(ix, row)| {
                                let ticked = chosen.contains(&row.key);
                                let key = row.key.clone();
                                let meta = [
                                    row.artifact_kind().name().to_string(),
                                    row.version.clone(),
                                    library::short_ago(&row.updated_at, now),
                                ]
                                .into_iter()
                                .filter(|part| !part.is_empty())
                                .collect::<Vec<_>>()
                                .join(" · ");
                                row_shell(("artifact-picker-row", ix), ix == cursor, palette)
                                    .child(checkbox(ix, ticked, palette))
                                    .child(glyph(kind_icon(row.artifact_kind()), palette.icon))
                                    .child(div().flex_1().min_w(px(0.)).truncate().child(row.title.clone()))
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(px(12.))
                                            .text_color(palette.faint)
                                            .child(meta),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(picker) = &mut this.artifact_picker {
                                            picker.cursor = ix;
                                        }
                                        this.toggle_artifact(&key, cx);
                                    }))
                            }))
                            .into_any_element()
                    }
                }
            };
        let count = chosen.len();
        let footer = footer(palette)
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(if count == 0 {
                        format!("Enter ticks a row · up to {}", crate::artifact_refs::MAX)
                    } else {
                        format!("{count} selected · {} adds", keys_label())
                    }),
            )
            .child(
                ui::button("artifact-picker-cancel", "Cancel", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.close_artifact_picker(window, cx))),
            )
            .child(if count == 0 {
                ui::disabled_button("artifact-picker-add", "Add", palette).into_any_element()
            } else {
                ui::primary_button("artifact-picker-add", format!("Add {count}"), palette)
                    .on_click(cx.listener(|this, _, window, cx| this.add_chosen_artifacts(window, cx)))
                    .into_any_element()
            });
        v_flex()
            .child(self.picker_header("Add artifacts", palette, cx))
            .child(sub("Choose pages and shared chats for the agent to read.", palette))
            .child(search_field(&picker.search, palette))
            .child(tabs)
            .child(list)
            .child(footer)
            .into_any_element()
    }

    fn render_session(
        &self,
        picker: &ArtifactPicker,
        reference: &ArtifactRef,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let artifact = h_flex()
            .mx(px(20.))
            .mb(px(12.))
            .p(px(10.))
            .gap(px(10.))
            .rounded(px(10.))
            .bg(palette.selected)
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .rounded(px(7.))
                    .bg(palette.menu)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(kind_icon(reference.kind())).size(px(15.))),
            )
            .child(
                v_flex()
                    .min_w(px(0.))
                    .child(div().truncate().child(reference.title.clone()))
                    .child(div().text_size(px(12.)).text_color(palette.faint).child(format!(
                        "{} · {}",
                        reference.kind,
                        reference.version_label()
                    ))),
            );
        let choices = self.session_choices();
        let cursor = picker_navigation::clamp(picker.cursor, choices.len());
        let list: AnyElement = if choices.is_empty() {
            quiet("No session matches this search.", palette).into_any_element()
        } else {
            scroll_list(&picker.scroll, palette)
                .children(choices.into_iter().enumerate().map(|(ix, choice)| {
                    let dest = choice.dest.clone();
                    row_shell(("artifact-session-row", ix), ix == cursor, palette)
                        .child(
                            div()
                                .size(px(16.))
                                .flex_none()
                                .rounded_full()
                                .border_1()
                                .border_color(if ix == cursor { palette.strong } else { palette.border })
                                .when(ix == cursor, |radio| radio.border_4()),
                        )
                        .child(glyph(choice.icon, palette.icon))
                        .child(div().flex_1().min_w(px(0.)).truncate().child(choice.title))
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(palette.faint)
                                .child(choice.meta),
                        )
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            if let Some(picker) = &mut this.artifact_picker {
                                picker.cursor = ix;
                            }
                            // A double click adds at once, as Enter does.
                            if event.click_count() > 1 {
                                this.add_to_session(dest.clone(), window, cx);
                            }
                            cx.notify();
                        }))
                }))
                .into_any_element()
        };
        let footer = footer(palette)
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child("Opens the session with it in the composer."),
            )
            .child(
                ui::button("artifact-session-cancel", "Cancel", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.close_artifact_picker(window, cx))),
            )
            .child(
                ui::primary_button("artifact-session-add", "Add", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.artifact_picker_enter(false, window, cx))),
            );
        v_flex()
            .child(self.picker_header("Add to a session", palette, cx))
            .child(sub(
                "The agent gets a reference it can read. The artifact itself does not change.",
                palette,
            ))
            .child(artifact)
            .child(search_field(&picker.search, palette))
            .child(list)
            .child(footer)
            .into_any_element()
    }

    fn picker_header(&self, title: &'static str, palette: Palette, cx: &mut Context<Self>) -> Div {
        h_flex()
            .h(px(44.))
            .pt(px(16.))
            .pr(px(12.))
            .pl(px(20.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(16.))
                    .line_height(px(24.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(
                super::share_ui::icon_button("artifact-picker-close", Lucide::X, palette)
                    .tooltip(|window, cx| Tooltip::new("Close  Esc").build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| this.close_artifact_picker(window, cx))),
            )
    }
}

fn keys_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘Enter"
    } else {
        "Ctrl+Enter"
    }
}

fn sub(text: &'static str, palette: Palette) -> Div {
    div()
        .px(px(20.))
        .pt(px(2.))
        .pb(px(12.))
        .text_color(palette.muted)
        .child(text)
}

fn quiet(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .px(px(20.))
        .py(px(18.))
        .text_color(palette.muted)
        .child(text.into())
}

fn search_field(search: &Entity<InputState>, palette: Palette) -> Div {
    h_flex()
        .mx(px(20.))
        .mb(px(10.))
        .h(px(32.))
        .px(px(10.))
        .gap(px(8.))
        .rounded(px(8.))
        .bg(palette.hover)
        .child(glyph(Lucide::Search, palette.faint))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(13.))
                .child(Input::new(search).appearance(false)),
        )
}

fn pill(id: impl Into<ElementId>, label: &'static str, on: bool, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(10.))
        .py(px(4.))
        .rounded_full()
        .text_size(px(12.))
        .cursor_pointer()
        .when(on, |pill| pill.bg(palette.selected).text_color(palette.strong))
        .when(!on, |pill| {
            pill.text_color(palette.muted).hover(|style| style.bg(palette.hover))
        })
        .child(label)
}

fn scroll_list(scroll: &ScrollHandle, _palette: Palette) -> Stateful<Div> {
    v_flex()
        .id("artifact-picker-list")
        .max_h(px(300.))
        .px(px(12.))
        .overflow_y_scroll()
        .track_scroll(scroll)
}

/// A list row: its fill fades with the pointer, and the keyboard's row keeps the fill.
fn row_shell(id: (&'static str, usize), current: bool, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .relative()
        .flex_none()
        .h(px(36.))
        .px(px(8.))
        .gap(px(10.))
        .rounded(px(8.))
        .cursor_pointer()
        .when(current, |row| row.bg(palette.menu_hover))
        .when(!current, |row| {
            row.child(motion::hover_fill(
                ElementId::Name(format!("{}-{}-fill", id.0, id.1).into()),
                palette.menu_hover,
                8.,
            ))
        })
}

/// A 16 px box; ticked, the strong fill and a check that fades in (`motion::fade_in`).
fn checkbox(ix: usize, ticked: bool, palette: Palette) -> Div {
    div()
        .size(px(16.))
        .flex_none()
        .rounded(px(4.))
        .border_1()
        .flex()
        .items_center()
        .justify_center()
        .border_color(if ticked { palette.strong } else { palette.border })
        .when(ticked, |box_| {
            box_.bg(palette.strong).child(motion::fade_in(
                Icon::new(Lucide::Check)
                    .size(px(11.))
                    .text_color(palette.primary_foreground),
                ElementId::NamedInteger("artifact-tick".into(), ix as u64),
            ))
        })
}

fn footer(palette: Palette) -> Div {
    h_flex()
        .mt(px(10.))
        .px(px(20.))
        .py(px(14.))
        .gap(px(8.))
        .border_t_1()
        .border_color(palette.border)
}
