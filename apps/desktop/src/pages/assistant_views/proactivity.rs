//! Proactivity, as Nexus UI's `agent-proactivity.tsx`: how often Nexus writes first.
//!
//! - [`ProactivityPanel`], on the Approvals view under its own heading: the dial (Quiet, Balanced and
//!   Hands-on, each with what it sends and about how often), quiet hours in this computer's time zone, the kinds turned
//!   off at every step with "Turn them back on", and the batching note. While the agent API has proactivity off, it
//!   draws nothing.
//! - [`WhySheet`], under each message Nexus starts (the morning brief today): "Why am I seeing this?" opens the
//!   reasons, then Fewer like this (the dial one step down; at Quiet, that kind off) and Turn off for that kind. It reads
//!   the setting only when opened, and the two actions show only while the agent API has proactivity on.
//!
//! The strict reads, the change bodies and the words are `nexus_desktop_core::agent_proactivity`'s.

use gpui_kit::component::input::InputState;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::super::{Ctx, palette, ui};
use super::{Reply, call, pill, quiet, quiet_pill};
use crate::agent::Call;
use crate::palette::Palette;
use nexus_desktop_core::agent_proactivity::{self as words, Change, DEFAULT_QUIET_HOURS, Kind, Level, Read, View, Why};

/// This computer's IANA time zone, for new quiet hours; UTC when it names none (Nexus UI's `browserZone`).
pub fn local_zone() -> String {
    iana_time_zone::get_timezone()
        .ok()
        .filter(|zone| !zone.is_empty())
        .unwrap_or_else(|| "UTC".into())
}

/// What the dial shows: loading, the setting, nothing (off), or why it did not open.
#[derive(Debug, Clone, PartialEq)]
enum State {
    Loading,
    Ready(View),
    Off,
    Failed,
}

/// The dial and quiet hours, for the Approvals view.
pub struct ProactivityPanel {
    ctx: Ctx,
    /// The Assistant's name, as the person named it: the lede says it.
    name: SharedString,
    state: State,
    /// What the last change did, or why it was not saved.
    status: Option<String>,
    /// Why the typed quiet hours cannot be saved.
    hours_problem: Option<&'static str>,
    /// This computer's time zone, read once when the panel opens: new quiet hours use it.
    zone: String,
    start: Entity<InputState>,
    end: Entity<InputState>,
    working: bool,
    _read: Task<()>,
    _save: Option<Task<()>>,
}

/// The panel; `name` is the Assistant's name, Nexus by default.
pub fn panel(ctx: Ctx, name: SharedString, window: &mut Window, cx: &mut App) -> Entity<ProactivityPanel> {
    cx.new(|cx| {
        let start = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(DEFAULT_QUIET_HOURS.0)
                .default_value(DEFAULT_QUIET_HOURS.0)
        });
        let end = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(DEFAULT_QUIET_HOURS.1)
                .default_value(DEFAULT_QUIET_HOURS.1)
        });
        let read = call(&ctx, Call::Proactivity, window, cx, ProactivityPanel::read);
        ProactivityPanel {
            ctx,
            name,
            state: State::Loading,
            status: None,
            hours_problem: None,
            zone: local_zone(),
            start,
            end,
            working: false,
            _read: read,
            _save: None,
        }
    })
}

impl ProactivityPanel {
    fn read(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        self.state = match words::read_of(reply.status, reply.body.as_ref()) {
            Read::View(view) => {
                self.show_hours(&view, window, cx);
                State::Ready(view)
            }
            Read::Off => State::Off,
            Read::Failed(_) => State::Failed,
        };
    }

    /// The fields show the saved quiet hours, or the defaults while they are off.
    fn show_hours(&mut self, view: &View, window: &mut Window, cx: &mut Context<Self>) {
        let (start, end) = match &view.quiet_hours {
            Some(hours) => (hours.start.clone(), hours.end.clone()),
            None => (DEFAULT_QUIET_HOURS.0.into(), DEFAULT_QUIET_HOURS.1.into()),
        };
        self.start.update(cx, |state, cx| state.set_value(start, window, cx));
        self.end.update(cx, |state, cx| state.set_value(end, window, cx));
    }

    #[cfg(test)]
    pub fn view(&self) -> Option<&View> {
        match &self.state {
            State::Ready(view) => Some(view),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn off(&self) -> bool {
        self.state == State::Off
    }

    #[cfg(test)]
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Save one change; `done` says what it did once saved.
    pub fn change(&mut self, change: Change, done: fn(&View) -> String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(made) = Call::save_proactivity(&change) else {
            return;
        };
        if self.working {
            return;
        }
        self.working = true;
        self._save = Some(call(&self.ctx, made, window, cx, move |panel, reply, window, cx| {
            panel.working = false;
            match words::read_of(reply.status, reply.body.as_ref()) {
                Read::View(view) => {
                    panel.status = Some(done(&view));
                    panel.show_hours(&view, window, cx);
                    panel.state = State::Ready(view);
                }
                failed => panel.status = Some(words::unsaved_text(&failed).into()),
            }
        }));
        cx.notify();
    }

    /// Quiet hours on with the typed times, or off.
    pub fn set_quiet_hours(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !on {
            self.hours_problem = None;
            return self.change(Change::quiet_hours(None), |_| "Quiet hours are off.".into(), window, cx);
        }
        let State::Ready(view) = &self.state else { return };
        let zone = view
            .quiet_hours
            .as_ref()
            .map_or_else(|| self.zone.clone(), |hours| hours.timezone.clone());
        let (start, end) = (
            self.start.read(cx).value().to_string(),
            self.end.read(cx).value().to_string(),
        );
        match words::quiet_hours_typed(&start, &end, &zone) {
            Ok(hours) => {
                self.hours_problem = None;
                self.change(
                    Change::quiet_hours(Some(hours)),
                    |saved| match &saved.quiet_hours {
                        Some(hours) => format!("Quiet from {} to {}.", hours.start, hours.end),
                        None => "Quiet hours are off.".into(),
                    },
                    window,
                    cx,
                );
            }
            Err(problem) => {
                self.hours_problem = Some(problem);
                cx.notify();
            }
        }
    }
}

/// One step of the dial: its name, what it sends and about how often, in a 12 px card; the chosen one has the
/// strong ring and the control fill.
fn step(level: Level, on: bool, palette: Palette) -> Stateful<Div> {
    ui::focusable(v_flex().id(("proactivity-step", level as usize)), palette)
        .flex_1()
        .min_w(px(160.))
        .gap(px(4.))
        .px(px(14.))
        .py(px(12.))
        .rounded(px(12.))
        .border_1()
        .border_color(if on { palette.strong } else { palette.border })
        .when(on, |step| step.bg(palette.selected))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.strong)
                .child(level.label()),
        )
        .child(div().text_size(px(13.)).text_color(palette.muted).child(level.does()))
        .child(
            div()
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(level.per_day()),
        )
}

/// A quiet link: 13 px, underlined on hover.
fn link(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .text_color(palette.foreground)
        .underline()
        .cursor_pointer()
        .child(text.into())
}

/// The panel's lede, with the Assistant's name, as the web's (`agent-proactivity.tsx`).
pub fn lede(name: &str) -> String {
    format!("How often {name} writes first. It writes first only with a reason it can show.")
}

#[cfg(test)]
impl ProactivityPanel {
    /// The lede drawn now, for the view tests.
    pub fn lede(&self) -> String {
        lede(&self.name)
    }
}

impl Render for ProactivityPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        if self.state == State::Off {
            return div().id("assistant-proactivity");
        }
        let mut panel = v_flex()
            .id("assistant-proactivity")
            .w_full()
            .gap(px(12.))
            .pt(px(12.))
            .child(
                div()
                    .text_size(px(17.))
                    .text_color(palette.foreground)
                    .child("Proactivity"),
            )
            .child(quiet(lede(&self.name), palette));
        let view = match &self.state {
            State::Loading => return panel.child(quiet("Loading…", palette)),
            State::Failed => {
                return panel.child(ui::settings_notice(
                    "Your proactivity did not open",
                    "Try again in a moment.",
                    palette,
                ));
            }
            State::Ready(view) => view.clone(),
            State::Off => unreachable!("drawn above"),
        };
        let working = self.working;
        panel = panel.child(
            h_flex()
                .w_full()
                .flex_wrap()
                .gap(px(8.))
                .children(Level::ALL.map(|level| {
                    let on = view.level == level;
                    step(level, on, palette).when(!on && !working, |step| {
                        step.cursor_pointer()
                            .hover(|style| style.bg(palette.hover))
                            .on_click(cx.listener(move |panel, _, window, cx| {
                                panel.change(Change::level(level), words::level_text, window, cx)
                            }))
                    })
                })),
        );

        // Quiet hours: off, or from one time to another in this computer's time zone.
        let hours_on = view.quiet_hours.is_some();
        let zone = view
            .quiet_hours
            .as_ref()
            .map_or_else(|| self.zone.clone(), |hours| hours.timezone.clone());
        let save_label = if hours_on { "Save" } else { "Turn on" };
        let save = if working {
            quiet_pill("proactivity-hours-save", save_label, palette)
        } else {
            pill("proactivity-hours-save", save_label, palette)
                .on_click(cx.listener(|panel, _, window, cx| panel.set_quiet_hours(true, window, cx)))
        };
        panel = panel.child(
            v_flex()
                .w_full()
                .gap(px(8.))
                .pt(px(4.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(palette.strong)
                        .child("Quiet hours"),
                )
                .child(
                    h_flex()
                        .gap(px(8.))
                        .child(
                            ui::switch("proactivity-hours", hours_on, working, palette).when(!working, |switch| {
                                switch.on_click(
                                    cx.listener(move |panel, _, window, cx| {
                                        panel.set_quiet_hours(!hours_on, window, cx)
                                    }),
                                )
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(13.5))
                                .text_color(palette.foreground)
                                .child("Hold everything but the urgent"),
                        ),
                )
                .child(
                    h_flex()
                        .gap(px(8.))
                        .text_size(px(13.))
                        .text_color(palette.muted)
                        .child("From")
                        .child(div().w(px(88.)).child(ui::input_box(&self.start, palette)))
                        .child("to")
                        .child(div().w(px(88.)).child(ui::input_box(&self.end, palette)))
                        .child(save),
                )
                .child(quiet(zone, palette))
                .children(self.hours_problem.map(|problem| ui::error_line(problem, palette))),
        );
        if let Some(muted) = words::muted_text(&view.muted) {
            panel = panel.child(
                h_flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .child(muted)
                    .when(!working, |line| {
                        line.child(
                            link("proactivity-unmute", "Turn them back on", palette).on_click(cx.listener(
                                |panel, _, window, cx| {
                                    panel.change(
                                        Change::muted(Vec::new()),
                                        |_| "Every kind is back on.".into(),
                                        window,
                                        cx,
                                    )
                                },
                            )),
                        )
                    }),
            );
        }
        panel = panel.child(quiet(words::batch_text(&view), palette));
        if let Some(status) = &self.status {
            panel = panel.child(quiet(status.clone(), palette));
        }
        panel
    }
}

/// "Why am I seeing this?" under a message Nexus started: the reasons on a click, then Fewer like this and Turn
/// off.
pub struct WhySheet {
    ctx: Ctx,
    why: Why,
    open: bool,
    /// The setting, read once the sheet opens: the actions need it.
    view: Option<Read>,
    done: Option<String>,
    working: bool,
    _read: Option<Task<()>>,
    _save: Option<Task<()>>,
}

pub fn why_sheet(ctx: Ctx, why: Why, cx: &mut App) -> Entity<WhySheet> {
    cx.new(|_| WhySheet {
        ctx,
        why,
        open: false,
        view: None,
        done: None,
        working: false,
        _read: None,
        _save: None,
    })
}

impl WhySheet {
    /// Open or close the sheet. The first open reads the setting.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        if self.open && self.view.is_none() && self._read.is_none() {
            self._read = Some(call(&self.ctx, Call::Proactivity, window, cx, |sheet, reply, _, _| {
                sheet.view = Some(words::read_of(reply.status, reply.body.as_ref()));
            }));
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn done(&self) -> Option<&str> {
        self.done.as_deref()
    }

    /// Fewer like this (`fewer` true) or Turn off, for this message's kind.
    pub fn act(&mut self, fewer: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Read::View(view)) = &self.view else { return };
        if self.working {
            return;
        }
        let kind = self.why.kind;
        let change = if fewer {
            words::fewer_like_this(view, kind)
        } else {
            words::turn_off(view, kind)
        };
        let Some(made) = Call::save_proactivity(&change) else {
            return;
        };
        self.working = true;
        self._save = Some(call(&self.ctx, made, window, cx, move |sheet, reply, _, _| {
            sheet.working = false;
            let read = words::read_of(reply.status, reply.body.as_ref());
            sheet.done = Some(match &read {
                Read::View(saved) if fewer => words::fewer_text(saved, kind),
                Read::View(_) => format!("Done: {} off.", kind.name()),
                failed => words::unsaved_text(failed).into(),
            });
            if matches!(read, Read::View(_)) {
                sheet.view = Some(read);
            }
        }));
        cx.notify();
    }
}

fn turn_off_label(kind: Kind) -> String {
    format!("Turn off {}", kind.name())
}

impl Render for WhySheet {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let opener = div()
            .id("agent-why-open")
            .pl(px(4.))
            .text_size(px(11.))
            .text_color(palette.faint)
            .cursor_pointer()
            .hover(|style| style.underline())
            .on_click(cx.listener(|sheet, _, window, cx| sheet.toggle(window, cx)))
            .child("Why am I seeing this?");
        let sheet = v_flex().items_start().child(opener);
        if !self.open {
            return sheet;
        }
        let mut body = v_flex()
            .w_full()
            .max_w(px(420.))
            .mt(px(6.))
            .gap(px(8.))
            .px(px(14.))
            .py(px(12.))
            .rounded(px(12.))
            .border_1()
            .border_color(palette.border)
            .text_size(px(13.))
            .text_color(palette.foreground)
            .child(v_flex().gap(px(2.)).children(self.why.lines.iter().map(|line| {
                h_flex()
                    .items_start()
                    .gap(px(8.))
                    .child(
                        div()
                            .pt(px(7.))
                            .child(div().size(px(4.)).rounded_full().bg(palette.faint)),
                    )
                    .child(div().flex_1().min_w(px(0.)).child(line.clone()))
            })));
        match (&self.view, &self.done) {
            (_, Some(done)) => {
                body = body.child(quiet(format!("{done} Change it in Approvals."), palette));
            }
            (Some(Read::View(_)), None) => {
                let working = self.working;
                let button = |id: &'static str, text: String, fewer: bool, cx: &mut Context<Self>| {
                    if working {
                        quiet_pill(id, text, palette)
                    } else {
                        pill(id, text, palette)
                            .on_click(cx.listener(move |sheet, _, window, cx| sheet.act(fewer, window, cx)))
                    }
                };
                body = body.child(
                    h_flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .child(button("agent-why-fewer", "Fewer like this".into(), true, cx))
                        .child(button("agent-why-off", turn_off_label(self.why.kind), false, cx)),
                );
            }
            _ => {}
        }
        sheet.child(body)
    }
}
