//! Discover, as Nexus UI's `agent-discover.tsx`: what Nexus can do, and how to let it do more. Five sections,
//! picked in the side as the board draws them: Try these (one tap sends the line to Nexus), Connect more (what each
//! account unlocks, from Gateway's connector catalog), Autonomy (the tiers as a ladder, with the person's spend line
//! and the kinds still asking first), Teach it about you (how it learns, a note to add, and What Nexus knows), and
//! What's new (a dot until this computer has shown it once).
//!
//! The static words are the kit's, kept in `nexus_desktop_core::agent_discover`. A part that cannot read says so in
//! one line, and the rest still works.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

use super::super::{Ctx, PageId, palette, ui};
use super::{Moves, Reply, call, card, pill, primary_pill, quiet, quiet_pill};
use crate::agent::Call;
use crate::palette::Palette;
use nexus_desktop_core::agent_approvals::{self as approvals, ApprovalsView};
use nexus_desktop_core::agent_discover::{self as words, ConnectRow, Offer, View};
use nexus_desktop_core::agent_memory::{self as memory, MemoryView};

/// A section of Discover, in the side's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Try,
    Connect,
    Autonomy,
    Teach,
    New,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Try,
        Section::Connect,
        Section::Autonomy,
        Section::Teach,
        Section::New,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Try => "Try these",
            Section::Connect => "Connect more",
            Section::Autonomy => "Autonomy",
            Section::Teach => "Teach it about you",
            Section::New => "What’s new",
        }
    }

    /// The section's lede, with the Assistant's `name` where it names it.
    pub fn lede(self, name: &str) -> String {
        match self {
            Section::Try => format!("One tap sends the words to {name}. Change them in the chat if you like."),
            Section::Connect => "Each account opens new things it can do.".into(),
            Section::Autonomy => {
                "A ladder: move a kind down one rung when you trust it. You can always move it back.".into()
            }
            Section::Teach => "It learns as you go. Correct anything in Memory; it forgets at once.".into(),
            Section::New => "A dot until you’ve seen it.".into(),
        }
    }
}

/// A part read once: loading, read, or failed.
enum Part<T> {
    Loading,
    Ready(T),
    Failed,
}

pub struct DiscoverPage {
    ctx: Ctx,
    moves: Moves,
    section: Section,
    memory: Part<MemoryView>,
    approvals: Part<ApprovalsView>,
    catalog: Part<Vec<Offer>>,
    mine: Vec<String>,
    rows: Vec<ConnectRow>,
    /// What's new this computer had not shown when Discover opened: its dots stay for this visit.
    fresh: HashSet<&'static str>,
    adding: bool,
    adder: Entity<TextareaState>,
    note_status: Option<String>,
    working: bool,
    _memory: Task<()>,
    _approvals: Task<()>,
    _add: Option<Task<()>>,
}

/// Where this computer keeps the What's new lines it has shown (the web's `nexus.discover.seen`), beside `seen.json`.
fn seen_file() -> Option<PathBuf> {
    if cfg!(test) || !crate::managed::local_cache(crate::managed::policy()) {
        return None;
    }
    crate::paths::seen_file().map(|file| file.with_file_name("discover-seen.json"))
}

/// The lines shown before, then every line kept as shown. Storage that fails shows every dot, and keeps nothing.
fn fresh_news() -> HashSet<&'static str> {
    let file = seen_file();
    let seen: Vec<String> = file
        .as_ref()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let fresh = words::unseen_news(&seen).into_iter().collect();
    if let Some(file) = file {
        let added: Vec<String> = words::NEWS
            .iter()
            .map(|item| item.id.to_string())
            .filter(|id| !seen.contains(id))
            .collect();
        let mut all = seen;
        all.extend(added);
        all.truncate(200);
        let written = serde_json::to_string(&all)
            .map_err(|error| error.to_string())
            .and_then(|text| std::fs::write(&file, text).map_err(|error| error.to_string()));
        if let Err(error) = written {
            eprintln!("nexus-desktop-gpui: Discover's seen lines could not be kept: {error}");
        }
    }
    fresh
}

/// The connectors a person may connect as themselves (Nexus UI's `connectRows` filter): offered, not kept off, and
/// taking a `user` connection; each with its catalog line.
pub fn offers_of(catalog: &Value) -> Vec<Offer> {
    catalog
        .get("connectors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("offered").and_then(Value::as_bool) != Some(false))
        .filter(|item| item.get("usable").and_then(Value::as_bool) != Some(false))
        .filter(|item| {
            item.get("connects")
                .and_then(Value::as_array)
                .is_some_and(|connects| connects.iter().any(|scope| scope.as_str() == Some("user")))
        })
        .filter_map(|item| {
            let connector = item.get("name")?.as_str()?.trim().to_string();
            let description = item.get("description").and_then(Value::as_str).unwrap_or_default();
            (!connector.is_empty()).then(|| Offer {
                connector,
                line: super::super::connectors::cards::short_line(description),
            })
        })
        .collect()
}

/// The connectors the person holds a connection of their own to.
pub fn mine_of(connections: &Value) -> Vec<String> {
    connections
        .get("connections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("scope").and_then(Value::as_str) != Some("org"))
        .filter_map(|item| item.get("connector")?.as_str().map(String::from))
        .collect()
}

pub fn build(ctx: Ctx, moves: Moves, window: &mut Window, cx: &mut App) -> Entity<DiscoverPage> {
    cx.new(|cx| {
        let adder = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Lily is in 1st grade at Rooftop.")
                .auto_grow(2, 6)
        });
        let memory_read = call(
            &ctx,
            Call::Memory,
            window,
            cx,
            |page: &mut DiscoverPage, reply: Reply, _, _| {
                page.memory = match reply
                    .ok()
                    .then_some(())
                    .and(reply.body.as_ref())
                    .and_then(memory::memory_view_of)
                {
                    Some(view) => Part::Ready(view),
                    None => Part::Failed,
                };
            },
        );
        let approvals_read = call(
            &ctx,
            Call::Approvals,
            window,
            cx,
            |page: &mut DiscoverPage, reply: Reply, _, _| {
                page.approvals = match reply
                    .ok()
                    .then_some(())
                    .and(reply.body.as_ref())
                    .and_then(approvals::approvals_view_of)
                {
                    Some(view) => Part::Ready(view),
                    None => Part::Failed,
                };
            },
        );
        super::super::read(
            cx,
            ctx.gateway.query("/v1/connectors".into()),
            |page: &mut DiscoverPage, answer, _| {
                page.catalog = match answer {
                    Ok(answer) => Part::Ready(offers_of(&answer)),
                    Err(_) => Part::Failed,
                };
                page.connect_rows();
            },
        );
        super::super::read(
            cx,
            ctx.gateway.query("/v1/connections".into()),
            |page: &mut DiscoverPage, answer, _| {
                if let Ok(answer) = answer {
                    page.mine = mine_of(&answer);
                    page.connect_rows();
                }
            },
        );
        DiscoverPage {
            ctx,
            moves,
            section: Section::Try,
            memory: Part::Loading,
            approvals: Part::Loading,
            catalog: Part::Loading,
            mine: Vec::new(),
            rows: Vec::new(),
            fresh: fresh_news(),
            adding: false,
            adder,
            note_status: None,
            working: false,
            _memory: memory_read,
            _approvals: approvals_read,
            _add: None,
        }
    })
}

impl DiscoverPage {
    fn connect_rows(&mut self) {
        if let Part::Ready(offers) = &self.catalog {
            self.rows = words::connect_rows(offers, &self.mine);
        }
    }

    #[cfg(test)]
    pub fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        cx.notify();
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[ConnectRow] {
        &self.rows
    }

    #[cfg(test)]
    pub fn try_line(&mut self, line: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.hand_off(line, window, cx);
    }

    /// "Try it": the line goes to Nexus as the person's message, after this view's update.
    fn hand_off(&mut self, line: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let (moves, ctx) = (self.moves.clone(), self.ctx.clone());
        window.defer(cx, move |window, cx| moves.hand_off(&ctx, line, window, cx));
    }

    /// Open another view of the Assistant, after this view's update.
    fn open(&mut self, id: PageId, window: &mut Window, cx: &mut Context<Self>) {
        let (moves, ctx) = (self.moves.clone(), self.ctx.clone());
        window.defer(cx, move |window, cx| moves.open(&ctx, id, None, window, cx));
    }

    /// Connect: the connector's card in Settings › Connectors, where its sign-in or key form starts.
    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let navigate = self.ctx.navigate_settings.clone();
        window.defer(cx, move |window, cx| {
            navigate("/settings/connectors".into(), window, cx)
        });
    }

    /// "Tell Nexus something about you": a note, as What Nexus knows adds one.
    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.adder.read(cx).value().to_string();
        let text = match memory::note_draft(&text) {
            Ok(text) => text,
            Err(error) => {
                self.note_status = Some(error);
                cx.notify();
                return;
            }
        };
        let Some(made) = Call::add_note(&text) else { return };
        if self.working {
            return;
        }
        self.working = true;
        self._add = Some(call(&self.ctx, made, window, cx, |page, reply, window, cx| {
            page.working = false;
            match reply
                .ok()
                .then_some(())
                .and(reply.body.as_ref())
                .and_then(|body| body.get("note"))
                .and_then(memory::note_of)
            {
                Some(note) => {
                    if let Part::Ready(view) = &mut page.memory {
                        view.notes.insert(0, note);
                    }
                    page.adding = false;
                    page.adder.update(cx, |state, cx| state.set_value("", window, cx));
                    page.note_status = Some(format!("{} keeps the note, from its next message.", page.moves.name()));
                }
                None => {
                    page.note_status = Some(format!(
                        "{} could not keep the note. Try again in Memory.",
                        page.moves.name()
                    ))
                }
            }
        }));
        cx.notify();
    }
}

/// An idea or an example: a title and its line in quotes, with Try it under it when it has one.
fn idea(title: &str, line: &str, palette: Palette) -> Div {
    card(palette)
        .flex_1()
        .child(
            div()
                .text_size(px(14.))
                .text_color(palette.foreground)
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(12.5))
                .line_height(px(18.))
                .text_color(palette.muted)
                .child(format!("“{line}”")),
        )
}

/// Cards four to a row, as the board's grid.
fn grid(cards: Vec<Div>) -> Div {
    let mut rows = v_flex().w_full().gap(px(12.));
    let mut cards = cards.into_iter().peekable();
    while cards.peek().is_some() {
        let mut row = h_flex().w_full().items_stretch().gap(px(12.));
        for _ in 0..4 {
            row = row.child(cards.next().unwrap_or_else(|| div().flex_1()));
        }
        rows = rows.child(row);
    }
    rows
}

/// A row of a list section: a lead, its title and line, and its action.
fn list_row(
    lead: Option<AnyElement>,
    title: impl Into<SharedString>,
    line: impl Into<SharedString>,
    action: Option<AnyElement>,
    palette: Palette,
) -> Div {
    h_flex()
        .w_full()
        .min_h(px(56.))
        .gap(px(12.))
        .py(px(8.))
        .border_t_1()
        .border_color(palette.border)
        .children(lead)
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(palette.foreground)
                        .child(title.into()),
                )
                .child(div().text_size(px(12.5)).text_color(palette.muted).child(line.into())),
        )
        .children(action)
}

impl DiscoverPage {
    fn try_these(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        grid(
            words::IDEAS
                .iter()
                .enumerate()
                .map(|(at, each)| {
                    let line = each.line;
                    idea(each.title, each.line, palette).child(
                        div().mt_auto().pt(px(4.)).child(
                            pill(("discover-try", at), "Try it", palette)
                                .on_click(cx.listener(move |page, _, window, cx| page.hand_off(line, window, cx))),
                        ),
                    )
                })
                .collect(),
        )
    }

    fn connect_more(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let open_integrations = |id: &'static str, cx: &mut Context<Self>| {
            pill(id, "Open Integrations", palette).on_click(cx.listener(|page, _, window, cx| page.connect(window, cx)))
        };
        match &self.catalog {
            Part::Loading => v_flex().child(quiet("Loading your accounts…", palette)),
            Part::Failed => v_flex()
                .gap(px(8.))
                .child(quiet("Your accounts did not load.", palette))
                .child(h_flex().child(open_integrations("discover-integrations", cx))),
            Part::Ready(_) if self.rows.is_empty() => v_flex()
                .gap(px(8.))
                .child(quiet("Your organization offers no accounts to connect yet.", palette))
                .child(h_flex().child(open_integrations("discover-integrations", cx))),
            Part::Ready(_) => v_flex()
                .w_full()
                .children(self.rows.iter().enumerate().map(|(at, row)| {
                    let lead = match crate::assets::connector_named(&row.connector)
                        .or_else(|| crate::assets::connector_named(&row.name))
                    {
                        Some((path, color)) => ui::mark_tile(path, color, palette),
                        None => ui::letter_tile(&row.name, palette),
                    };
                    let action = (!row.connected).then(|| {
                        pill(("discover-connect", at), "Connect", palette)
                            .on_click(cx.listener(|page, _, window, cx| page.connect(window, cx)))
                            .into_any_element()
                    });
                    list_row(
                        Some(lead.into_any_element()),
                        row.name.clone(),
                        row.line(),
                        action,
                        palette,
                    )
                })),
        }
    }

    fn autonomy(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let view = match &self.approvals {
            Part::Ready(view) => Some(view),
            _ => None,
        };
        let mut out = v_flex()
            .w_full()
            .gap(px(10.))
            .children(words::autonomy_ladder(view).into_iter().map(|rung| {
                card(palette)
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .text_color(palette.foreground)
                                    .child(rung.title),
                            )
                            .child(div().text_size(px(12.)).text_color(palette.faint).child(rung.tag)),
                    )
                    .child(div().text_size(px(12.5)).text_color(palette.muted).child(rung.line))
            }));
        if matches!(self.approvals, Part::Failed) {
            out = out.child(quiet(
                "Your approval settings did not load, so the line shows no amount.",
                palette,
            ));
        }
        if view.is_some_and(|view| !view.tiers_on) {
            out = out.child(quiet(
                "Here, every write asks first for now: the tiers are not on yet.",
                palette,
            ));
        }
        let steps = view
            .filter(|view| view.tiers_on)
            .map(words::safe_next_steps)
            .unwrap_or_default();
        if !steps.is_empty() {
            out = out
                .child(
                    div()
                        .pt(px(6.))
                        .text_size(px(13.))
                        .text_color(palette.foreground)
                        .child("Safe next steps"),
                )
                .child(v_flex().children(steps.into_iter().map(|what| {
                    list_row(
                        None,
                        format!("{what}, without asking"),
                        "Choose Always allow on its next card.",
                        None,
                        palette,
                    )
                })));
        }
        out.child(
            h_flex().pt(px(4.)).child(
                pill("discover-approvals", "Open Approvals", palette)
                    .on_click(cx.listener(|page, _, window, cx| page.open(PageId::AgentApprovals, window, cx))),
            ),
        )
    }

    fn teach(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let name = self.moves.name();
        let mut out = v_flex().w_full().gap(px(12.)).child(grid(
            words::TEACH
                .iter()
                .map(|(title, line)| idea(title, line, palette))
                .collect(),
        ));
        match &self.memory {
            Part::Failed => out = out.child(quiet("Memory did not load here.", palette)),
            Part::Ready(view) => {
                let count = view.notes.len();
                out = out.child(quiet(
                    format!(
                        "{name} keeps {count} {} about you.",
                        if count == 1 { "note" } else { "notes" }
                    ),
                    palette,
                ));
            }
            Part::Loading => {}
        }
        let controls = matches!(&self.memory, Part::Ready(view) if view.controls);
        if controls && self.adding {
            out = out.child(
                v_flex()
                    .w_full()
                    .gap(px(6.))
                    .child(
                        div()
                            .w_full()
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(8.))
                            .bg(palette.hover)
                            .text_size(px(13.))
                            .child(Textarea::new(&self.adder).appearance(false)),
                    )
                    .child(div().text_size(px(12.)).text_color(palette.faint).child(format!(
                        "{name} uses it in your direct messages, from its next message."
                    )))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(if self.working {
                                quiet_pill("discover-add", "Saving…", palette)
                            } else {
                                primary_pill("discover-add", "Add", palette)
                                    .on_click(cx.listener(|page, _, window, cx| page.add(window, cx)))
                            })
                            .child(pill("discover-add-cancel", "Cancel", palette).on_click(cx.listener(
                                |page, _, _, cx| {
                                    page.adding = false;
                                    cx.notify();
                                },
                            ))),
                    ),
            );
        }
        if let Some(status) = &self.note_status {
            out = out.child(quiet(status.clone(), palette));
        }
        out.child(
            h_flex()
                .gap(px(8.))
                .when(controls && !self.adding, |row| {
                    row.child(
                        pill("discover-tell", format!("Tell {name} something about you"), palette).on_click(
                            cx.listener(|page, _, window, cx| {
                                page.adding = true;
                                page.note_status = None;
                                page.adder.update(cx, |state, cx| state.focus(window, cx));
                                cx.notify();
                            }),
                        ),
                    )
                })
                .child(
                    pill("discover-memory", "Open memory", palette)
                        .on_click(cx.listener(|page, _, window, cx| page.open(PageId::AgentMemory, window, cx))),
                ),
        )
    }

    fn whats_new(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        v_flex()
            .w_full()
            .children(words::NEWS.iter().enumerate().map(|(at, item)| {
                let fresh = self.fresh.contains(item.id);
                let lead = div()
                    .flex_none()
                    .size(px(6.))
                    .rounded_full()
                    .when(fresh, |dot| dot.bg(palette.foreground))
                    .into_any_element();
                let opens = match item.opens {
                    View::Chat => PageId::Agent,
                    View::Errands => PageId::AgentErrands,
                    View::Memory => PageId::AgentMemory,
                    View::Approvals => PageId::AgentApprovals,
                };
                let action = pill(("discover-new", at), "Try it", palette)
                    .on_click(cx.listener(move |page, _, window, cx| page.open(opens, window, cx)))
                    .into_any_element();
                list_row(Some(lead), item.title, item.line, Some(action), palette)
            }))
    }
}

impl Render for DiscoverPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let mut side = v_flex()
            .w(px(210.))
            .flex_none()
            .h_full()
            .gap(px(2.))
            .px(px(10.))
            .py(px(20.))
            .bg(palette.sidebar);
        for section in Section::ALL {
            let picked = section == self.section;
            let dot = section == Section::New && !self.fresh.is_empty();
            side = side.child(
                h_flex()
                    .id(("discover-section", section as usize))
                    .h(px(32.))
                    .gap(px(8.))
                    .px(px(10.))
                    .rounded(px(8.))
                    .text_size(px(13.5))
                    .cursor_pointer()
                    .when(picked, |row| row.bg(palette.selected).text_color(palette.foreground))
                    .when(!picked, |row| {
                        row.text_color(palette.muted).hover(|style| style.bg(palette.hover))
                    })
                    .child(div().flex_1().child(section.label()))
                    .when(dot, |row| {
                        row.child(div().size(px(6.)).rounded_full().bg(palette.foreground))
                    })
                    .on_click(cx.listener(move |page, _, _, cx| {
                        page.section = section;
                        cx.notify();
                    })),
            );
        }
        let body = match self.section {
            Section::Try => self.try_these(palette, cx),
            Section::Connect => self.connect_more(palette, cx),
            Section::Autonomy => self.autonomy(palette, cx),
            Section::Teach => self.teach(palette, cx),
            Section::New => self.whats_new(palette, cx),
        };
        let main = v_flex()
            .id("discover-main")
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .px(px(28.))
                    .pt(px(20.))
                    .pb(px(8.))
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(20.))
                            .text_color(palette.foreground)
                            .child("Discover"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .child(format!("What {} can do, and how to let it do more.", self.moves.name())),
                    ),
            )
            .child(
                v_flex()
                    .px(px(28.))
                    .pt(px(8.))
                    .pb(px(24.))
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .child(self.section.lede(&self.moves.name())),
                    )
                    .child(body),
            );
        div()
            .id("assistant-discover")
            .size_full()
            .flex()
            .child(side)
            .child(main)
    }
}
