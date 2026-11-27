//! A command the person accepted stays for 5 seconds after it ends, under a bar that runs down the time left: as a
//! card in the tray above the composer, and as its row in the Commands tab's Active list. Each comes down from above,
//! and when the time is up swipes off to the right (the row into History). Hovering pauses the time; a card's click
//! opens the command in the Commands tab; × closes the card at once.
use super::list::{self, Tone};
use super::*;

/// How long a finished command stays.
pub(super) const SHOW: Duration = Duration::from_secs(5);
/// The card's entrance, from above (`--nx-dur-ui-long`).
const ENTER: Duration = crate::motion::UI_LONG;
/// The card's swipe to the right.
const LEAVE: Duration = Duration::from_millis(240);
/// The most finished cards the tray shows at once; an older one gives way to a newer one.
const MOST: usize = 3;
/// The card's corner radius; the countdown bar is inset by it.
const CARD_RADIUS: f32 = 12.;

pub(super) struct Finished {
    pub(super) entry: Entry,
    /// Whether the tray shows a card for it: only for a command the person accepted. Its row in Active counts down
    /// either way.
    pub(super) card: bool,
    shown: Instant,
    /// Time spent hovered, which does not count down.
    paused: Duration,
    hover: Option<Instant>,
    leaving: Option<Instant>,
}

impl Finished {
    pub(super) fn new(entry: Entry, now: Instant, card: bool) -> Self {
        Self {
            entry,
            card,
            shown: now,
            paused: Duration::ZERO,
            hover: None,
            leaving: None,
        }
    }

    fn held(&self, now: Instant) -> Duration {
        self.paused
            + self
                .hover
                .map_or(Duration::ZERO, |at| now.saturating_duration_since(at))
    }

    fn run(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.shown).saturating_sub(self.held(now))
    }

    /// The share of the 5 seconds still left, from 1 down to 0: the bar's width.
    pub(super) fn left(&self, now: Instant) -> f32 {
        1. - (self.run(now).as_secs_f32() / SHOW.as_secs_f32()).clamp(0., 1.)
    }

    pub(super) fn set_hover(&mut self, on: bool, now: Instant) {
        match (on, self.hover) {
            (true, None) if self.leaving.is_none() => self.hover = Some(now),
            (false, Some(at)) => {
                self.paused += now.saturating_duration_since(at);
                self.hover = None;
            }
            _ => {}
        }
    }

    pub(super) fn dismiss(&mut self, now: Instant) {
        self.leaving.get_or_insert(now);
    }

    /// Start the swipe when the time is up, dated from the moment it ran out, so a card nobody drew while it
    /// expired goes without a late swipe. True once the card is gone.
    pub(super) fn tick(&mut self, now: Instant, reduced: bool) -> bool {
        if self.leaving.is_none() && self.hover.is_none() {
            let run = self.run(now);
            if run >= SHOW {
                self.leaving = Some(now - (run - SHOW));
            }
        }
        let leave = if reduced { Duration::ZERO } else { LEAVE };
        self.leaving
            .is_some_and(|at| now.saturating_duration_since(at) >= leave)
    }

    /// The entrance, 0 to 1, eased out.
    pub(super) fn enter(&self, now: Instant, reduced: bool) -> f32 {
        if reduced {
            return 1.;
        }
        let t = now.saturating_duration_since(self.shown).as_secs_f32() / ENTER.as_secs_f32();
        crate::motion::ease_out(t.clamp(0., 1.))
    }

    /// The swipe, 0 to 1, accelerating as it leaves (`--nx-ease-in`).
    pub(super) fn leave(&self, now: Instant) -> f32 {
        self.leaving.map_or(0., |at| {
            let t = now.saturating_duration_since(at).as_secs_f32() / LEAVE.as_secs_f32();
            crate::motion::cubic_bezier(0.4, 0., 1., 1., t.clamp(0., 1.))
        })
    }
}

/// Whether an accepted command's state means it ended and has something to say: not one that never ran.
fn ended(entry: &Entry) -> bool {
    !entry.active() && !matches!(entry.status.as_str(), "declined" | "denied")
}

impl SharedTerminals {
    /// Start the countdown of each command that ended since the last read: it was active then and is not now. Each
    /// keeps its row in Active for the countdown; one the person accepted also gets a card in the tray. True when a
    /// card was added, so the workspace draws the tray.
    pub(super) fn note_finished(&mut self, entries: &[Entry]) -> bool {
        let now = Instant::now();
        let mut card_added = false;
        let was_active: HashSet<String> = self
            .entries
            .iter()
            .filter(|old| old.active())
            .map(|old| old.id.clone())
            .collect();
        for entry in entries {
            if !was_active.contains(&entry.id) || entry.active() {
                continue;
            }
            let accepted = self.accepted.remove(&entry.id);
            if ended(entry) {
                self.finished.retain(|card| card.entry.id != entry.id);
                self.finished.push(Finished::new(entry.clone(), now, accepted));
                card_added |= accepted;
            }
        }
        if !self.accepted.is_empty() {
            let active: HashSet<&str> = entries
                .iter()
                .filter(|entry| entry.active())
                .map(|entry| entry.id.as_str())
                .collect();
            self.accepted.retain(|id| active.contains(id.as_str()));
        }
        // Only the cards are capped: rows in Active take the room the list gives them.
        while self.finished.iter().filter(|card| card.card).count() > MOST {
            if let Some(oldest) = self.finished.iter().position(|card| card.card) {
                self.finished[oldest].card = false;
            }
        }
        card_added
    }

    /// Whether the tray has a finished card to draw.
    pub(super) fn has_cards(&self) -> bool {
        self.finished.iter().any(|card| card.card)
    }

    /// Whether `id` is a finished command still counting down: the Commands tab keeps its row in Active.
    pub(super) fn lingering(&self, id: &str) -> bool {
        self.finished.iter().any(|card| card.entry.id == id)
    }

    /// Drop the finished commands whose time and swipe are over, and schedule the next frame while any remain. The
    /// tray and the Commands tab both draw them, so each calls this before it draws.
    pub(super) fn advance_finished(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let reduced = cx.reduce_motion();
        let before = self.finished.len();
        let had_cards = self.has_cards();
        self.finished.retain_mut(|card| !card.tick(now, reduced));
        let removed = self.finished.len() != before;
        if !self.finished.is_empty() || removed {
            // The bar runs down smoothly, and the frame after the last card goes tells the workspace, which drops
            // the tray when nothing else is in it. Emitting from the frame, not from this draw, keeps the
            // workspace from being updated while it draws.
            let last = had_cards && !self.has_cards();
            cx.spawn(async move |owner, cx| {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let _ = owner.update(cx, |_, cx| {
                    cx.notify();
                    if last {
                        cx.emit(SharedTerminalsEvent::Activity);
                    }
                });
            })
            .detach();
        }
    }

    /// The finished cards, newest on top. None when there are none.
    pub(super) fn render_finished(&mut self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.advance_finished(cx);
        if !self.has_cards() {
            return None;
        }
        let now = Instant::now();
        let reduced = cx.reduce_motion();
        let cards: Vec<AnyElement> = (0..self.finished.len())
            .rev()
            .filter(|ix| self.finished[*ix].card)
            .map(|ix| self.render_finished_card(ix, now, reduced, palette, cx))
            .collect();
        Some(
            v_flex()
                .debug_selector(|| "finished-commands".into())
                .w_full()
                .gap(px(6.))
                .children(cards)
                .into_any_element(),
        )
    }

    /// A finished command's row in the Commands tab's Active list, while its time runs: the countdown bar comes down
    /// on top of it, and when the time is up the row swipes right, into History. Hovering it pauses the time.
    pub(super) fn render_lingering(
        &self,
        ix: usize,
        row: AnyElement,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Instant::now();
        let reduced = cx.reduce_motion();
        let card = &self.finished[ix];
        let id = card.entry.id.clone();
        let failed = list::tone(&card.entry) == Tone::Failed;
        let enter = card.enter(now, reduced);
        let leave = card.leave(now);
        // As on the tray's card: the bar lies on the row's top edge, inset by the row's corner radius, so it reads as
        // the row's own edge and not as a line between rows.
        let bar = div()
            .absolute()
            .top_0()
            .left(px(super::rows::ROW_RADIUS))
            .right(px(super::rows::ROW_RADIUS))
            .h(px(2.))
            .rounded_full()
            .bg(palette.hover)
            .opacity(enter)
            .child(div().h_full().rounded_full().w(relative(card.left(now))).bg(if failed {
                palette.destructive
            } else {
                palette.muted
            }));
        v_flex()
            .id(SharedString::from(format!("lingering-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("lingering-{id}")
            })
            .relative()
            .w_full()
            .min_w(px(0.))
            // Comes down from above, then swipes off to the right.
            .top(px(-8. * (1. - enter)))
            .left(relative(leave))
            .opacity(1. - leave)
            .child(row)
            // After the row, so the row's hover and selection fill paint under it.
            .child(bar)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if let Some(card) = this.finished.iter_mut().find(|card| card.entry.id == id) {
                    card.set_hover(*hovered, Instant::now());
                }
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_finished_card(
        &self,
        ix: usize,
        now: Instant,
        reduced: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card = &self.finished[ix];
        let entry = &card.entry;
        let id = entry.id.clone();
        let tone = list::tone(entry);
        let enter = card.enter(now, reduced);
        let leave = card.leave(now);
        let (line, _) = list::first_line(&entry.command);
        let label = list::label(&entry.name, &entry.command);
        let failed = tone == Tone::Failed;
        let status = if tone == Tone::Done {
            "Done".to_string()
        } else {
            entry.status()
        };
        // The bar lies on the straight part of the card's top edge, inset by the corner radius: GPUI clips to a
        // rectangle, not to the rounded corners, so a full-width bar would run past the card's curve.
        let bar = div()
            .absolute()
            .top_0()
            .left(px(CARD_RADIUS))
            .right(px(CARD_RADIUS))
            .h(px(2.))
            .rounded_full()
            .bg(palette.hover)
            .child(div().h_full().rounded_full().w(relative(card.left(now))).bg(if failed {
                palette.destructive
            } else {
                palette.muted
            }));
        let row = h_flex()
            .h(px(36.))
            .pl(px(10.))
            .pr(px(6.))
            .gap(px(8.))
            .child(super::rows::mark(entry, tone, false, false, palette))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .when(label.is_none(), |title| title.font_family(ui::MONO).text_size(px(12.)))
                    .child(label.unwrap_or_else(|| line.to_string())),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(if failed { palette.destructive } else { palette.muted })
                    .child(status),
            )
            .child(
                div()
                    .id(SharedString::from(format!("finished-close-{id}")))
                    .size(px(22.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover))
                    .child(glyph(Lucide::X, palette.icon).size(px(12.)))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Close").build(window, cx))
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            if let Some(card) = this.finished.iter_mut().find(|card| card.entry.id == id) {
                                card.dismiss(Instant::now());
                            }
                            cx.notify();
                        }
                    })),
            );
        div()
            .id(SharedString::from(format!("finished-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("finished-{id}")
            })
            .relative()
            .w_full()
            // Comes down from above, then swipes off to the right.
            .top(px(-12. * (1. - enter)))
            .left(relative(leave))
            .opacity(enter * (1. - leave))
            .overflow_hidden()
            .rounded(px(CARD_RADIUS))
            .bg(palette.raised)
            .shadow(ui::lift(palette))
            .cursor_pointer()
            .child(bar)
            .child(row)
            .tooltip({
                let command = list::tooltip(&entry.command);
                move |window, cx| gpui_kit::component::tooltip::Tooltip::new(command.clone()).build(window, cx)
            })
            .on_hover(cx.listener({
                let id = id.clone();
                move |this, hovered: &bool, _, cx| {
                    if let Some(card) = this.finished.iter_mut().find(|card| card.entry.id == id) {
                        card.set_hover(*hovered, Instant::now());
                    }
                    cx.notify();
                }
            }))
            // The click keeps the countdown: the command's row carries it on in the Commands tab's Active list.
            .on_click(cx.listener(move |this, _, _, cx| {
                this.reveal = Some(id.clone());
                cx.emit(SharedTerminalsEvent::OpenCommands);
                cx.notify();
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn entry(status: &str, exit: Option<u32>) -> Entry {
        serde_json::from_value(json!({"id":"one","name":"echo hi","command":"echo hi","cwd":"/work",
            "status":status,"private":false,"exitCode":exit}))
        .unwrap()
    }

    #[test]
    fn the_card_counts_down_ten_seconds_then_swipes_away() {
        let now = Instant::now();
        let mut card = Finished::new(entry("completed", Some(0)), now, true);
        assert_eq!(card.left(now), 1.);
        assert!((card.left(now + SHOW / 2) - 0.5).abs() < 1e-3);
        assert!(!card.tick(now + SHOW - Duration::from_millis(1), false));
        assert!(!card.tick(now + SHOW, false), "the swipe starts");
        assert!(card.leave(now + SHOW + LEAVE / 2) > 0.);
        assert!(card.tick(now + SHOW + LEAVE, false));
    }

    #[test]
    fn hovering_pauses_the_time_and_reduced_motion_leaves_at_once() {
        let now = Instant::now();
        let mut card = Finished::new(entry("failed", Some(1)), now, true);
        card.set_hover(true, now + Duration::from_secs(2));
        assert!(!card.tick(now + Duration::from_secs(30), false), "a hovered card stays");
        card.set_hover(false, now + Duration::from_secs(30));
        // Two of the five seconds ran before the hover; the 28 hovered ones did not count.
        assert!((card.left(now + Duration::from_secs(30)) - 0.6).abs() < 1e-3);
        let mut quick = Finished::new(entry("completed", Some(0)), now, true);
        assert_eq!(quick.enter(now, true), 1.);
        assert!(quick.tick(now + SHOW, true));
    }

    #[test]
    fn a_card_nobody_drew_while_it_expired_goes_without_a_late_swipe() {
        let now = Instant::now();
        let mut card = Finished::new(entry("completed", Some(0)), now, true);
        assert!(card.tick(now + SHOW + LEAVE, false));
        let mut closed = Finished::new(entry("completed", Some(0)), now, true);
        closed.dismiss(now + Duration::from_secs(1));
        assert!(closed.tick(now + Duration::from_secs(1) + LEAVE, false));
    }

    #[test]
    fn only_accepted_commands_that_ran_get_a_card() {
        assert!(ended(&entry("completed", Some(0))));
        assert!(ended(&entry("failed", Some(2))));
        assert!(ended(&entry("cancelled", None)));
        assert!(!ended(&entry("declined", None)));
        assert!(!ended(&entry("running", None)));
    }
}
