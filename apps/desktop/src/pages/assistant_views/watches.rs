//! Watches, as Nexus UI's `agent-watches.tsx`: what Nexus watches for the person. A watch is a
//! standing instruction. It tells and asks; it never acts on its own.
//!
//! - The list: each watch with what it waits for and when it last fired. The selected one opens beside it: When,
//!   Checks, Until, Tells you, and its history, every time it fired and what it did.
//! - Edit: when it checks, until when, each change, where it tells you, quiet hours. Save keeps the change; Delete
//!   asks first. Pause and Resume are on the watch.
//!
//! The strict reads and the words are `nexus_desktop_core::agent_watches`'. An agent API without watches (404, or 503
//! `watches_off`) says they are not on yet.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::super::{Ctx, palette};
use super::{
    Moves, Reply, call, card, danger_pill, frame, modal, pill, primary_pill, problem, quiet, quiet_pill, sheet,
};
use crate::agent::Call;
use crate::palette::Palette;
use nexus_desktop_core::agent::Problem;
use nexus_desktop_core::agent_watches::{self as words, Cadence, Change, Channel, Detail, Read, Watch};

/// The days the edit sheet offers.
const DAYS: [u32; 5] = [1, 3, 7, 14, 30];

enum State {
    Loading,
    /// The agent API keeps no watches.
    Off,
    Ready {
        watches: Vec<Watch>,
        max: u64,
    },
    Failed(Problem),
}

/// What the edit sheet holds before Save.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub cadence: Cadence,
    pub days: u32,
    pub every: bool,
    pub thread: bool,
    pub quiet_hours: bool,
}

impl Draft {
    fn of(watch: &Watch) -> Draft {
        Draft {
            cadence: watch.cadence,
            days: words::days_between(&watch.since, &watch.until),
            every: watch.every,
            thread: watch.tells.contains(&Channel::Thread),
            quiet_hours: watch.quiet_hours,
        }
    }

    /// The change from `watch` to this draft: only what differs. None when nothing differs or no channel is chosen.
    pub fn change(&self, watch: &Watch) -> Option<Change> {
        // Push is how Nexus tells the person, so it stays on; the thread is added to it.
        let tells: Vec<Channel> = if self.thread {
            vec![Channel::Push, Channel::Thread]
        } else {
            vec![Channel::Push]
        };
        let change = Change {
            cadence: (self.cadence != watch.cadence).then_some(self.cadence),
            days: (self.days != words::days_between(&watch.since, &watch.until)).then_some(self.days),
            every: (self.every != watch.every).then_some(self.every),
            tells: (tells != watch.tells).then_some(tells),
            quiet_hours: (self.quiet_hours != watch.quiet_hours).then_some(self.quiet_hours),
            paused: None,
        };
        (change != Change::default()).then_some(change)
    }
}

pub struct WatchesPage {
    ctx: Ctx,
    state: State,
    /// The watch open beside the list, by id, and its history once read.
    selected: Option<String>,
    detail: Option<Detail>,
    editing: Option<Draft>,
    deleting: bool,
    refused: Option<Problem>,
    working: bool,
    focus: FocusHandle,
    _read: Task<()>,
    _detail: Option<Task<()>>,
    _change: Option<Task<()>>,
}

pub fn build(ctx: Ctx, _moves: Moves, window: &mut Window, cx: &mut App) -> Entity<WatchesPage> {
    cx.new(|cx| {
        let read = call(&ctx, Call::watches(), window, cx, WatchesPage::read);
        WatchesPage {
            ctx,
            state: State::Loading,
            selected: None,
            detail: None,
            editing: None,
            deleting: false,
            refused: None,
            working: false,
            focus: cx.focus_handle(),
            _read: read,
            _detail: None,
            _change: None,
        }
    })
}

impl WatchesPage {
    fn read(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        self.state = match words::list_read(reply.status, reply.body.as_ref()) {
            Read::Ok((watches, max)) => State::Ready { watches, max },
            Read::Off => State::Off,
            Read::Failed { status, .. } => {
                State::Failed(nexus_desktop_core::agent::problem_of(status, reply.body.as_ref()))
            }
        };
        let first = match &self.state {
            State::Ready { watches, .. } => watches.first().map(|watch| watch.id.clone()),
            _ => None,
        };
        if let Some(id) = first {
            self.select(&id, window, cx);
        }
    }

    /// Open one watch beside the list, and read its history.
    pub fn select(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(id.to_string());
        self.detail = None;
        self.editing = None;
        self.deleting = false;
        self.refused = None;
        let Some(made) = Call::watch(id) else { return };
        let wanted = id.to_string();
        self._detail = Some(call(&self.ctx, made, window, cx, move |page, reply, _, _| {
            if page.selected.as_deref() == Some(wanted.as_str())
                && let Read::Ok(detail) = words::detail_read(reply.status, reply.body.as_ref())
            {
                page.detail = Some(detail);
            }
        }));
    }

    fn watches(&self) -> &[Watch] {
        match &self.state {
            State::Ready { watches, .. } => watches,
            _ => &[],
        }
    }

    fn current(&self) -> Option<&Watch> {
        let id = self.selected.as_deref()?;
        self.watches().iter().find(|watch| watch.id == id)
    }

    /// Replace the watch in the list with its new state.
    fn replace(&mut self, saved: Watch) {
        if let State::Ready { watches, .. } = &mut self.state
            && let Some(slot) = watches.iter_mut().find(|watch| watch.id == saved.id)
        {
            *slot = saved.clone();
        }
        if let Some(detail) = self.detail.as_mut()
            && detail.watch.id == saved.id
        {
            detail.watch = saved;
        }
    }

    /// Send one change to the selected watch.
    pub fn change(&mut self, change: Change, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(id), false) = (self.selected.clone(), self.working) else {
            return;
        };
        let Some(made) = Call::change_watch(&id, &change) else {
            return;
        };
        self.working = true;
        self.refused = None;
        self._change = Some(call(&self.ctx, made, window, cx, |page, reply, _, _| {
            page.working = false;
            match words::watch_read(reply.status, reply.body.as_ref()) {
                Read::Ok(saved) => {
                    page.replace(saved);
                    page.editing = None;
                }
                Read::Off => page.state = State::Off,
                Read::Failed { error, .. } => page.refused = Some(refusal(error.as_deref())),
            }
        }));
    }

    pub fn toggle_pause(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(paused) = self.current().map(|watch| watch.paused) else {
            return;
        };
        self.change(
            Change {
                paused: Some(!paused),
                ..Change::default()
            },
            window,
            cx,
        );
    }

    pub fn edit(&mut self, cx: &mut Context<Self>) {
        self.editing = self.current().map(Draft::of);
        cx.notify();
    }

    pub fn edit_with(&mut self, change: impl FnOnce(&mut Draft), cx: &mut Context<Self>) {
        if let Some(draft) = self.editing.as_mut() {
            change(draft);
        }
        cx.notify();
    }

    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(draft), Some(watch)) = (self.editing.clone(), self.current().cloned()) else {
            return;
        };
        match draft.change(&watch) {
            Some(change) => self.change(change, window, cx),
            None => self.editing = None,
        }
    }

    /// Delete the selected watch, after the dialog's yes.
    pub fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.deleting = false;
        let (Some(id), false) = (self.selected.clone(), self.working) else {
            return;
        };
        let Some(made) = Call::delete_watch(&id) else { return };
        self.working = true;
        self._change = Some(call(&self.ctx, made, window, cx, move |page, reply, window, cx| {
            page.working = false;
            match words::deleted_read(reply.status, reply.body.as_ref()) {
                Read::Ok(()) => {
                    let next = if let State::Ready { watches, .. } = &mut page.state {
                        watches.retain(|watch| watch.id != id);
                        watches.first().map(|watch| watch.id.clone())
                    } else {
                        None
                    };
                    page.selected = None;
                    page.detail = None;
                    page.editing = None;
                    if let Some(next) = next {
                        page.select(&next, window, cx);
                    }
                }
                Read::Off => page.state = State::Off,
                Read::Failed { error, .. } => page.refused = Some(refusal(error.as_deref())),
            }
        }));
        cx.notify();
    }

    #[cfg(test)]
    pub fn ids(&self) -> Vec<String> {
        self.watches().iter().map(|watch| watch.id.clone()).collect()
    }

    #[cfg(test)]
    pub fn is_off(&self) -> bool {
        matches!(self.state, State::Off)
    }

    #[cfg(test)]
    pub fn history_len(&self) -> Option<usize> {
        self.detail.as_ref().map(|detail| detail.history.len())
    }
}

/// A refusal in the page's words.
fn refusal(error: Option<&str>) -> Problem {
    Problem {
        title: "Nexus could not do that".into(),
        detail: words::error_text(error).into(),
    }
}

/// A choice's pill: filled when it is the one chosen.
fn choice(id: impl Into<ElementId>, text: &'static str, on: bool, palette: Palette) -> Stateful<Div> {
    if on {
        primary_pill(id, text, palette)
    } else {
        pill(id, text, palette)
    }
}

fn fact(label: &'static str, value: String, palette: Palette) -> Div {
    h_flex()
        .w_full()
        .gap(px(16.))
        .child(
            div()
                .w(px(84.))
                .flex_none()
                .text_size(px(13.))
                .text_color(palette.faint)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(13.))
                .text_color(palette.foreground)
                .child(value),
        )
}

impl Render for WatchesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let (outer, mut page) = frame(
            "assistant-watches",
            "Watches",
            "A watch checks something for you and tells you when it changes. It tells and asks. It never acts on its own.",
            960.,
            palette,
        );
        match &self.state {
            State::Loading => {
                return div()
                    .size_full()
                    .child(outer.child(page.child(quiet("Loading your watches…", palette))));
            }
            State::Off => {
                return div()
                    .size_full()
                    .child(outer.child(page.child(quiet("Watches are not on for this Nexus yet.", palette))));
            }
            State::Failed(failed) => {
                return div()
                    .size_full()
                    .child(outer.child(page.child(problem(failed, palette))));
            }
            State::Ready { .. } => {}
        }
        if let Some(refused) = &self.refused {
            page = page.child(problem(refused, palette));
        }
        let (count, max) = match &self.state {
            State::Ready { watches, max } => (watches.len(), *max),
            _ => (0, 0),
        };
        if count == 0 {
            page = page.child(quiet(
                "No watches yet. Ask Nexus to tell you when something changes, such as “tell me when this PR merges”. It proposes a watch, and nothing runs until you press Keep.",
                palette,
            ));
            return div().size_full().child(outer.child(page));
        }
        let mut list = v_flex().w(px(300.)).flex_none().gap(px(6.));
        for (ix, watch) in self.watches().iter().enumerate() {
            let on = self.selected.as_deref() == Some(watch.id.as_str());
            let id = watch.id.clone();
            list = list.child(
                v_flex()
                    .id(("watch-row", ix))
                    .gap(px(2.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(if on { palette.strong } else { palette.border })
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover))
                    .on_click(cx.listener(move |page, _, window, cx| page.select(&id, window, cx)))
                    .child(div().text_size(px(14.)).text_color(palette.strong).child(
                        match words::place(&watch.target) {
                            Some(place) => format!("{} · {place}", words::name(&watch.target)),
                            None => words::name(&watch.target),
                        },
                    ))
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .child(words::list_line(watch)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(match &watch.last_fired {
                                Some(firing) => format!("Fired {}", words::day_text(&firing.at)),
                                None => "Has not fired".into(),
                            }),
                    ),
            );
        }
        let mut body = h_flex().w_full().items_start().gap(px(16.)).child(list);
        if let Some(watch) = self.current() {
            body = body.child(self.detail_card(watch, palette, cx));
        }
        page = page
            .child(body)
            .child(quiet(format!("{count} of {max} watches."), palette));
        let dialog = self.deleting.then(|| {
            modal(
                "watch-scrim",
                sheet("watch-delete", &self.focus, 400., palette)
                    .on_key_down(cx.listener(|page, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            page.deleting = false;
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        div()
                            .text_size(px(16.))
                            .text_color(palette.strong)
                            .child("Delete this watch?"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .child("It stops checking and its history goes with it."),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                pill("watch-keep-it", "Keep it", palette).on_click(cx.listener(|page, _, _, cx| {
                                    page.deleting = false;
                                    cx.notify();
                                })),
                            )
                            .child(
                                danger_pill("watch-delete-yes", "Delete", palette)
                                    .on_click(cx.listener(|page, _, window, cx| page.delete(window, cx))),
                            ),
                    ),
                palette,
            )
        });
        div().relative().size_full().child(outer.child(page)).children(dialog)
    }
}

impl WatchesPage {
    fn detail_card(&self, watch: &Watch, palette: Palette, cx: &mut Context<Self>) -> Div {
        let now = chrono::Utc::now().timestamp_millis();
        let working = self.working;
        let pause = if working {
            quiet_pill("watch-pause", if watch.paused { "Resume" } else { "Pause" }, palette).into_any_element()
        } else {
            pill("watch-pause", if watch.paused { "Resume" } else { "Pause" }, palette)
                .on_click(cx.listener(|page, _, window, cx| page.toggle_pause(window, cx)))
                .into_any_element()
        };
        let edit = pill("watch-edit", "Edit", palette).on_click(cx.listener(|page, _, _, cx| {
            if page.editing.is_some() {
                page.editing = None;
                cx.notify()
            } else {
                page.edit(cx)
            }
        }));
        let title = match &watch.title {
            Some(title) => format!("{} {title}", words::name(&watch.target)),
            None => words::name(&watch.target),
        };
        let mut card = card(palette).flex_1().gap(px(12.)).child(
            h_flex()
                .w_full()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(15.))
                        .text_color(palette.strong)
                        .child(title),
                )
                .children((watch.ended.is_none()).then_some(pause))
                .children((watch.ended.is_none()).then_some(edit)),
        );
        if let Some(ended) = &watch.ended {
            card = card.child(quiet(
                format!(
                    "It fired for the last time on {} and no longer checks. Its history stays here.",
                    words::day_text(ended)
                ),
                palette,
            ));
        }
        card = match self.editing.as_ref().filter(|_| watch.ended.is_none()) {
            Some(draft) => card.child(self.edit_sheet(draft, palette, cx)),
            None => card
                .child(fact("When", words::when(&watch.target, watch.every), palette))
                .child(fact("Checks", words::checks(&watch.target, watch.cadence), palette))
                .child(fact(
                    "Until",
                    words::until_text(&watch.target, watch.every, &watch.until, now),
                    palette,
                ))
                .child(fact(
                    "Tells you",
                    format!(
                        "{}{}",
                        words::tells_text(&watch.tells),
                        if watch.quiet_hours {
                            ", except in quiet hours"
                        } else {
                            ""
                        }
                    ),
                    palette,
                ))
                .child(fact(
                    "Why",
                    format!("You said “{}” on {}.", watch.said, words::day_text(&watch.since)),
                    palette,
                )),
        };
        card = card.child(
            div()
                .text_size(px(13.))
                .text_color(palette.muted)
                .child("Every time it fired"),
        );
        match &self.detail {
            Some(detail) if !detail.history.is_empty() => {
                for firing in &detail.history {
                    card = card.child(
                        v_flex()
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .text_color(palette.strong)
                                    .child(words::firing_what(&watch.target, firing)),
                            )
                            .child(quiet(
                                format!("{} · {}", words::when_text(&firing.at), words::firing_told(firing)),
                                palette,
                            )),
                    );
                }
            }
            Some(_) => {
                card = card.child(quiet(
                    "It has not fired yet. It checks quietly and speaks up only when what it waits for happens.",
                    palette,
                ));
            }
            None => card = card.child(quiet("Reading its history…", palette)),
        }
        if watch.ended.is_some() {
            card = card.child(
                pill("watch-delete-ended", "Delete its history", palette)
                    .on_click(cx.listener(|page, _, window, cx| page.delete(window, cx))),
            );
        }
        card
    }

    fn edit_sheet(&self, draft: &Draft, palette: Palette, cx: &mut Context<Self>) -> Div {
        let row = |label: &'static str| {
            h_flex().w_full().gap(px(8.)).flex_wrap().items_center().child(
                div()
                    .w(px(84.))
                    .flex_none()
                    .text_size(px(13.))
                    .text_color(palette.faint)
                    .child(label),
            )
        };
        let cadences = Cadence::ALL
            .into_iter()
            .enumerate()
            .fold(row("Checks"), |line, (ix, cadence)| {
                line.child(
                    choice(
                        ("watch-cadence", ix),
                        cadence.label(),
                        draft.cadence == cadence,
                        palette,
                    )
                    .on_click(cx.listener(move |page, _, _, cx| page.edit_with(|draft| draft.cadence = cadence, cx))),
                )
            });
        let days = DAYS.into_iter().enumerate().fold(row("Until"), |line, (ix, days)| {
            let label: &'static str = match days {
                1 => "1 day",
                3 => "3 days",
                7 => "7 days",
                14 => "14 days",
                _ => "30 days",
            };
            line.child(
                choice(("watch-days", ix), label, draft.days == days, palette)
                    .on_click(cx.listener(move |page, _, _, cx| page.edit_with(|draft| draft.days = days, cx))),
            )
        });
        let tells = row("Tells you")
            .child(choice("watch-push", "Push (always on)", true, palette))
            .child(
                choice("watch-thread", "The thread", draft.thread, palette)
                    .on_click(cx.listener(|page, _, _, cx| page.edit_with(|draft| draft.thread = !draft.thread, cx))),
            );
        let more = row("More")
            .child(
                choice("watch-every", "Every change", draft.every, palette)
                    .on_click(cx.listener(|page, _, _, cx| page.edit_with(|draft| draft.every = !draft.every, cx))),
            )
            .child(
                choice("watch-quiet", "Wait for quiet hours", draft.quiet_hours, palette).on_click(
                    cx.listener(|page, _, _, cx| page.edit_with(|draft| draft.quiet_hours = !draft.quiet_hours, cx)),
                ),
            );
        let actions = h_flex()
            .gap(px(8.))
            .child(
                primary_pill("watch-save", "Save", palette)
                    .on_click(cx.listener(|page, _, window, cx| page.save(window, cx))),
            )
            .child(
                pill("watch-cancel", "Cancel", palette).on_click(cx.listener(|page, _, _, cx| {
                    page.editing = None;
                    cx.notify();
                })),
            )
            .child(
                danger_pill("watch-delete", "Delete", palette).on_click(cx.listener(|page, _, window, cx| {
                    page.deleting = true;
                    window.focus(&page.focus, cx);
                    cx.notify();
                })),
            );
        v_flex()
            .gap(px(10.))
            .child(cadences)
            .child(days)
            .child(tells)
            .child(more)
            .child(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::{Cadence, Draft, Watch, words};

    fn watch() -> Watch {
        let text = r#"{"id":"w1","target":{"kind":"pull","repo":"a/b","number":1},"every":false,"cadence":"often","until":"2026-11-24T12:00:00.000Z","tells":["push"],"quietHours":true,"paused":false,"since":"2026-11-10T12:00:00.000Z","said":"x","link":"https://github.com/a/b/pull/1","lastFired":null}"#;
        words::watch_of(&serde_json::from_str(text).unwrap()).unwrap()
    }

    #[test]
    fn the_sheet_sends_only_what_differs_and_needs_a_channel() {
        let watch = watch();
        let mut draft = Draft::of(&watch);
        assert_eq!(draft.change(&watch), None, "nothing differs");
        draft.cadence = Cadence::Daily;
        draft.thread = true;
        draft.days = 3;
        let change = draft.change(&watch).unwrap();
        assert_eq!(
            change.body().unwrap(),
            r#"{"cadence":"daily","days":3,"tells":["push","thread"]}"#
        );
        draft.thread = false;
        draft.cadence = watch.cadence;
        draft.days = words::days_between(&watch.since, &watch.until);
        assert_eq!(draft.change(&watch), None, "back to what it was");
    }
}
