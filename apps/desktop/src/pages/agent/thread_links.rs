//! The thread's quick replies, live step line and link cards (brand v5 A1), as Nexus UI's `agent-chat.tsx`,
//! `agent-session-card.tsx` and `agent-errand-card.tsx` draw them. The readers and the words are
//! `nexus_desktop_core::agent_thread`'s.
//!
//! - The step line (A1·C b): while Nexus thinks, "Thinking", then the tool that runs ("Searching Linear"), in the quiet
//!   ink with the board's shimmer. Under Reduce motion the line stays, still. [`StepLine`] is its own small view, so
//!   its frames redraw only the line (render rule 5).
//! - Quick replies (A1·D): at most three 28 px pills with a hairline under the thread's last answer. A tap sends one.
//! - The session card (A1·5): 400 px, a hairline, radius 14; the 30 px tile with the terminal mark, the title, the line
//!   with the state's shape, and a chevron. Open goes to the web page, or fills a New session draft from a desktop
//!   `nexus://new?` link. It never sends.
//! - The errand card (A1·5, A1·C f to h): the session card's parts at 380 px. The tile holds the state: a turning arc
//!   while it runs, a dot when it needs you, ✓ done, ✕ failed in red, the thread's only red. A failed card says why.
//!   A click opens the errand's own thread. The card shows the errand as the latest errands list has it.
//!
//! The cards are read when the thread changes ([`Links::of`]), never in `render` (render rule 2). A turning arc is a
//! small view per card ([`TurningArc`]), made then too, so only the arc redraws while it turns.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::AgentPage;
use crate::agent::Entry;
use crate::palette::Palette;
use nexus_desktop_core::agent_cards as cards;
use nexus_desktop_core::agent_errands::{Errand, Status};
use nexus_desktop_core::agent_thread::{self as words, Session, SessionState, SessionWhere};

/// The cards of one message of a reply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessageLinks {
    pub sessions: Vec<Session>,
    /// The errand an "I opened an errand" notice carries, as the notice had it.
    pub errand: Option<Errand>,
    /// The message ends in a card (Connect, a session or an errand): its bubble has no tail.
    pub carded: bool,
}

/// What the thread's replies carry, read once when the thread changes.
#[derive(Default)]
pub struct Links {
    pub messages: HashMap<(usize, usize), MessageLinks>,
    /// The quick replies of each reply's last answer, by entry.
    pub replies: HashMap<usize, Vec<String>>,
}

impl Links {
    pub fn of(entries: &[Entry]) -> Links {
        let mut links = Links::default();
        for (ix, entry) in entries.iter().enumerate() {
            let Entry::Agent(messages) = entry else { continue };
            let replies = words::last_suggestions(messages);
            if !replies.is_empty() {
                links.replies.insert(ix, replies);
            }
            for (n, message) in messages.iter().enumerate() {
                let found = MessageLinks {
                    sessions: words::answer_sessions(message),
                    errand: words::notice_errand(message),
                    carded: false,
                };
                let connects = !cards::answer_connects(message).is_empty();
                let carded = connects || !found.sessions.is_empty() || found.errand.is_some();
                if carded {
                    links.messages.insert((ix, n), MessageLinks { carded, ..found });
                }
            }
        }
        links
    }

    /// The cards that turn now: each running session, and each errand that runs as `now` has it.
    pub fn turning(&self, now: &HashMap<String, Errand>) -> Vec<String> {
        let mut keys = Vec::new();
        for links in self.messages.values() {
            if let Some(errand) = &links.errand {
                let shown = now.get(&errand.id).unwrap_or(errand);
                if shown.status == Status::Running {
                    keys.push(errand_key(&errand.id));
                }
            }
            keys.extend(
                links
                    .sessions
                    .iter()
                    .filter(|session| session.state == SessionState::Running)
                    .map(|session| session_key(&session.url)),
            );
        }
        keys
    }
}

pub fn errand_key(id: &str) -> String {
    format!("errand:{id}")
}

pub fn session_key(url: &str) -> String {
    format!("session:{url}")
}

/// The live step line, a view of its own: only it redraws while the shimmer moves.
pub struct StepLine {
    line: SharedString,
}

impl StepLine {
    pub fn new() -> StepLine {
        StepLine {
            line: "Thinking".into(),
        }
    }

    /// Show `line`; a notify only when it changed.
    pub fn set(&mut self, line: &str, cx: &mut Context<Self>) {
        if self.line != line {
            self.line = SharedString::from(line.to_string());
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn line(&self) -> &str {
        &self.line
    }
}

impl Render for StepLine {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = super::palette(cx);
        let rest = super::quiet_ink(palette);
        let line = div().id("agent-step-line").text_size(px(13.)).text_color(rest);
        if cx.reduce_motion() {
            return line.child(self.line.clone()).into_any_element();
        }
        line.child(crate::motion::shimmer(
            "agent-step",
            self.line.clone(),
            rest,
            palette.strong,
        ))
        .into_any_element()
    }
}

/// The running state's arc: 12 px, turning, a view of its own so only it redraws (render rule 5). Under Reduce motion
/// it stands still.
pub struct TurningArc;

/// One turn of the arc (the web's `agent-session-turn`, 0.9 s).
const TURN: Duration = Duration::from_millis(900);

impl Render for TurningArc {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = super::palette(cx);
        let arc = Icon::new(Lucide::LoaderCircle)
            .size(px(12.))
            .text_color(palette.foreground);
        if cx.reduce_motion() {
            return arc.into_any_element();
        }
        arc.with_animation(
            "agent-card-arc",
            Animation::new(TURN).repeat().with_max_fps(crate::motion::LOOP_FPS),
            |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
        )
        .into_any_element()
    }
}

/// Quick replies (A1·D "Inline"): the board's 28 px pills with a hairline. A tap sends the reply's words.
pub fn quick_replies(ix: usize, replies: &[String], palette: Palette, cx: &mut Context<AgentPage>) -> Div {
    h_flex()
        .flex_wrap()
        .gap(px(8.))
        .max_w(px(440.))
        .pt(px(2.))
        .children(replies.iter().enumerate().map(|(n, reply)| {
            let words = reply.clone();
            div()
                .id(("agent-quick-reply", ix * 10 + n))
                .h(px(28.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(px(14.))
                .border_1()
                .border_color(palette.border)
                .text_size(px(12.5))
                .text_color(palette.foreground)
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .on_click(cx.listener(move |page, _, window, cx| page.start_with(&words, window, cx)))
                .child(reply.clone())
        }))
}

/// The card's frame: a hairline, radius 14, the tile, the text and the chevron.
fn frame(id: ElementId, width: f32, tile: AnyElement, text: Div, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .w_full()
        .max_w(px(width))
        .gap(px(12.))
        .px(px(12.))
        .py(px(10.))
        .rounded(px(14.))
        .border_1()
        .border_color(palette.border)
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .child(
            div()
                .flex_none()
                .size(px(30.))
                .rounded(px(8.))
                .bg(palette.hover)
                .flex()
                .items_center()
                .justify_center()
                .child(tile),
        )
        .child(text.flex_1().min_w(px(0.)))
        .child(
            Icon::new(Lucide::ChevronRight)
                .size(px(14.))
                .flex_none()
                .text_color(palette.faint),
        )
}

fn title(text: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .truncate()
        .text_size(px(13.5))
        .text_color(palette.strong)
        .child(text.into())
}

/// A state's shape on a session's line: a still ring while it waits to open, the arc while it runs, ✓, ✕ or a square.
fn session_mark(session: &Session, arc: Option<&Entity<TurningArc>>, palette: Palette) -> AnyElement {
    let icon = |name: Lucide| {
        Icon::new(name)
            .size(px(13.))
            .flex_none()
            .text_color(palette.faint)
            .into_any_element()
    };
    match (session.state, arc) {
        (SessionState::Running, Some(arc)) => arc.clone().into_any_element(),
        (SessionState::Done, _) => icon(Lucide::Check),
        (SessionState::Failed, _) => icon(Lucide::X),
        (SessionState::Stopped, _) => icon(Lucide::Square),
        // Ready, or running before its arc is made: the 8 px ring, still.
        _ => div()
            .flex_none()
            .size(px(8.))
            .mx(px(2.5))
            .rounded_full()
            .border(px(1.5))
            .border_color(palette.faint)
            .into_any_element(),
    }
}

/// The session link card (A1·5). `open` takes its link: the browser for a web page, a New session draft for a
/// desktop link.
pub fn session_card(
    id: usize,
    session: &Session,
    arc: Option<&Entity<TurningArc>>,
    palette: Palette,
    cx: &mut Context<AgentPage>,
) -> Stateful<Div> {
    let line = words::session_card_line(session);
    let text = v_flex().gap(px(2.)).child(title(session.title.clone(), palette)).child(
        h_flex()
            .gap(px(6.))
            .min_w(px(0.))
            .text_size(px(12.))
            .text_color(palette.muted)
            .child(session_mark(session, arc, palette))
            .child(
                h_flex()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .child(div().flex_none().child(line.lead()))
                    .children(
                        line.repo
                            .map(|repo| div().truncate().text_color(palette.faint).child(format!(" · {repo}"))),
                    ),
            ),
    );
    let tile = Icon::new(Lucide::SquareTerminal)
        .size(px(15.))
        .text_color(palette.muted)
        .into_any_element();
    let url = session.url.clone();
    let place = session.place;
    frame(("agent-session-card", id).into(), 400., tile, text, palette).on_click(cx.listener(
        move |page, _, window, cx| match place {
            SessionWhere::Desktop => page.open_draft(&url, window, cx),
            SessionWhere::Synced => super::open_link(&url, cx),
        },
    ))
}

/// The errand link card (A1·5): its state now, its name and its line. A click opens its thread.
pub fn errand_card(
    id: usize,
    errand: &Errand,
    arc: Option<&Entity<TurningArc>>,
    palette: Palette,
    cx: &mut Context<AgentPage>,
) -> Stateful<Div> {
    let mark = match (errand.status, arc) {
        (Status::Running, Some(arc)) => arc.clone().into_any_element(),
        (Status::Running, None) => div()
            .size(px(8.))
            .rounded_full()
            .border(px(1.5))
            .border_color(palette.foreground)
            .into_any_element(),
        (Status::NeedsYou, _) => div()
            .size(px(7.))
            .rounded_full()
            .bg(palette.foreground)
            .into_any_element(),
        (Status::Done, _) => Icon::new(Lucide::Check)
            .size(px(12.))
            .text_color(palette.faint)
            .into_any_element(),
        (Status::Failed, _) => Icon::new(Lucide::X)
            .size(px(12.))
            .text_color(palette.destructive)
            .into_any_element(),
    };
    let text = v_flex()
        .gap(px(2.))
        .child(title(
            nexus_desktop_core::agent_errands::errand_name(errand).to_string(),
            palette,
        ))
        .child(
            div()
                .truncate()
                .text_size(px(12.))
                .text_color(palette.muted)
                .child(words::errand_card_line(errand)),
        );
    let errand_id = errand.id.clone();
    frame(("agent-errand-card", id).into(), 380., mark, text, palette).on_click(cx.listener(
        move |page, _, window, cx| {
            if let Some(open) = page.open_errand.clone() {
                open(errand_id.clone(), window, cx);
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::{Entry, HashMap, Links, errand_key, session_key};
    use crate::agent::Message;
    use serde_json::json;

    fn message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).expect("a message")
    }

    /// The thread reads its cards once: quick replies of each reply's last answer, a session card and an opened
    /// errand, and which messages end in a card. Only running ones turn, as the errands list has them now.
    #[test]
    fn the_links_are_read_once_and_only_running_cards_turn() {
        let errand =
            json!({ "id": "i3", "goal": "Book the dentist", "status": "running", "at": "2026-11-18T09:00:00Z" });
        let entries = vec![
            Entry::User("hi".into()),
            Entry::Agent(vec![
                message(
                    json!({ "kind": "notice", "text": "x", "topic": "errands", "state": "opened", "errand": errand }),
                ),
                message(
                    json!({ "kind": "answer", "text": "Shall I?", "suggestions": ["Yes", "No"],
                    "sessions": [{ "where": "synced", "state": "running", "title": "Codex session", "url": "https://nexus.test/s/1" }] }),
                ),
            ]),
        ];
        let links = Links::of(&entries);
        assert_eq!(links.replies.get(&1), Some(&vec!["Yes".to_string(), "No".to_string()]));
        assert!(links.messages[&(1, 0)].errand.is_some() && links.messages[&(1, 0)].carded);
        assert_eq!(links.messages[&(1, 1)].sessions.len(), 1);
        let mut turning = links.turning(&HashMap::new());
        turning.sort();
        assert_eq!(turning, vec![errand_key("i3"), session_key("https://nexus.test/s/1")]);

        // The list says it failed since: its card stops turning.
        let failed = nexus_desktop_core::agent_errands::errand_of(
            &json!({ "id": "i3", "goal": "Book the dentist", "status": "failed", "at": "t" }),
        )
        .expect("an errand");
        let now = HashMap::from([("i3".to_string(), failed)]);
        assert_eq!(links.turning(&now), vec![session_key("https://nexus.test/s/1")]);

        // A plain answer carries nothing.
        let plain = Links::of(&[Entry::Agent(vec![message(
            json!({ "kind": "answer", "text": "Done." }),
        )])]);
        assert!(plain.messages.is_empty() && plain.replies.is_empty());
    }
}
