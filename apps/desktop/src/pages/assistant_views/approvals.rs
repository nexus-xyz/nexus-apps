//! Approvals, as Nexus UI's `agent-approvals.tsx`: the three tiers as columns, with what falls in each;
//! the tiers' rows, where the person sets the line over which a spend always asks and the rest are the rules the core
//! applies (If nobody answers says how long a card waits); then the learned rules by recipient, with Reset for one and
//! Reset all. The words and the strict read are `nexus_desktop_core::agent_approvals`'s. Under them, Proactivity
//! is its own view (`proactivity.rs`), as the web draws it on this view.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::InputState;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::super::{Ctx, palette, ui};
use super::{Moves, Reply, call, card, confirm, day, frame, pill, primary_pill, problem, quiet, quiet_pill};
use crate::agent::Call;
use crate::palette::Palette;
use nexus_desktop_core::agent::Problem;
use nexus_desktop_core::agent_approvals::{self as words, ApprovalsView, FixedRow, Rule, Tier, TierColumn};
use nexus_desktop_core::agent_cards::{self as cards, Voice};

/// What the page shows: loading, the settings, or why they did not open.
enum State {
    Loading,
    Ready(Box<Ready>),
    Failed(Problem),
}

/// The settings once read, with their words computed when they changed.
struct Ready {
    view: ApprovalsView,
    columns: Vec<TierColumn>,
    rows: Vec<FixedRow>,
    groups: Vec<(String, Vec<Rule>)>,
    hint: String,
}

impl Ready {
    fn of(view: ApprovalsView) -> Ready {
        Ready {
            columns: words::tier_columns(&view.spend_limit_usd),
            rows: words::fixed_rows(&view),
            groups: words::learned_groups(&view.rules),
            hint: words::line_hint(&view),
            view,
        }
    }
}

/// A reset that waits for the person's yes.
#[derive(Clone, Debug, PartialEq)]
enum Asking {
    All,
    Rule(String),
}

pub struct ApprovalsPage {
    ctx: Ctx,
    state: State,
    /// The two voices a write goes in, with the person's names in their samples.
    voices: [Voice; 2],
    /// A change that failed, over the page.
    refused: Option<Problem>,
    /// What the last change did.
    status: Option<String>,
    line: Entity<InputState>,
    line_error: Option<String>,
    asking: Option<Asking>,
    working: bool,
    _read: Task<()>,
    _change: Option<Task<()>>,
    /// The dial and quiet hours, under the learned rules.
    proactivity: Entity<super::proactivity::ProactivityPanel>,
}

pub fn build(ctx: Ctx, moves: Moves, window: &mut Window, cx: &mut App) -> Entity<ApprovalsPage> {
    cx.new(|cx| {
        let line = cx.new(|cx| InputState::new(window, cx).placeholder("50"));
        let read = call(&ctx, Call::Approvals, window, cx, ApprovalsPage::read);
        let full = moves.person.as_deref();
        let first = full.and_then(cards::first_name_of);
        let proactivity = super::proactivity::panel(ctx.clone(), moves.name(), window, cx);
        ApprovalsPage {
            proactivity,
            ctx,
            state: State::Loading,
            voices: cards::write_voices(first.as_deref(), full),
            refused: None,
            status: None,
            line,
            line_error: None,
            asking: None,
            working: false,
            _read: read,
            _change: None,
        }
    })
}

impl ApprovalsPage {
    #[cfg(test)]
    pub fn voices(&self) -> &[Voice; 2] {
        &self.voices
    }

    fn read(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        self.state = match (reply.ok(), reply.body.as_ref().and_then(words::approvals_view_of)) {
            (true, Some(view)) => {
                self.show_line(&view.spend_limit_usd, window, cx);
                State::Ready(Box::new(Ready::of(view)))
            }
            (true, None) => State::Failed(words::approvals_problem(502, None)),
            (false, _) => State::Failed(words::approvals_problem(reply.status, reply.body.as_ref())),
        };
    }

    #[cfg(test)]
    pub fn view(&self) -> Option<&ApprovalsView> {
        match &self.state {
            State::Ready(ready) => Some(&ready.view),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    #[cfg(test)]
    pub fn proactivity(&self) -> Entity<super::proactivity::ProactivityPanel> {
        self.proactivity.clone()
    }

    #[cfg(test)]
    pub fn reset(&mut self, rule: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.revoke(rule, window, cx);
    }

    /// Send one change; `done` gets its body once it succeeded.
    fn change(
        &mut self,
        call_made: Option<Call>,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, serde_json::Value, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(made) = call_made else { return };
        if self.working {
            return;
        }
        self.working = true;
        self.refused = None;
        self.asking = None;
        self._change = Some(call(&self.ctx, made, window, cx, move |page, reply, window, cx| {
            page.working = false;
            if reply.ok() {
                done(page, reply.body.unwrap_or_default(), window, cx);
            } else {
                page.refused = Some(words::approvals_problem(reply.status, reply.body.as_ref()));
            }
        }));
    }

    fn save_line(&mut self, usd: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let made = Call::save_line(usd.as_deref());
        self.change(
            made,
            window,
            cx,
            |page, body, window, cx| match words::approvals_view_of(&body) {
                Some(view) => {
                    page.status = Some(format!(
                        "Over {}, a spend always asks.",
                        words::dollars(&view.spend_limit_usd)
                    ));
                    page.show_line(&view.spend_limit_usd, window, cx);
                    page.state = State::Ready(Box::new(Ready::of(view)));
                }
                None => page.refused = Some(words::approvals_problem(502, None)),
            },
        );
    }

    fn submit_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let State::Ready(ready) = &self.state else { return };
        let text = self.line.read(cx).value().to_string();
        match words::line_of(&text, &ready.view.max_spend_limit_usd) {
            Ok(usd) => {
                self.line_error = None;
                self.save_line(Some(usd), window, cx);
            }
            Err(error) => self.line_error = Some(error),
        }
        cx.notify();
    }

    fn revoke(&mut self, rule: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let one = rule.clone();
        self.change(Call::revoke(rule.as_deref()), window, cx, move |page, body, _, _| {
            let count = body.get("revoked").and_then(serde_json::Value::as_u64).unwrap_or(0);
            if let State::Ready(ready) = &mut page.state {
                let mut view = ready.view.clone();
                match &one {
                    Some(id) => view.rules.retain(|rule| rule.id != *id),
                    None => view.rules.clear(),
                }
                **ready = Ready::of(view);
            }
            page.status = Some(words::revoked_text(one.is_some(), count));
        });
    }
}

impl ApprovalsPage {
    /// The field shows the line as the person types one: `50`, or `49.99`.
    fn show_line(&mut self, usd: &str, window: &mut Window, cx: &mut Context<Self>) {
        let shown = usd.strip_suffix(".00").unwrap_or(usd).to_string();
        self.line.update(cx, |state, cx| state.set_value(shown, window, cx));
    }
}

/// A tier's shape: a ring does freely, a dot asks first, a lock always asks.
fn shape(tier: Tier, palette: Palette) -> AnyElement {
    match tier {
        Tier::Freely => div()
            .flex_none()
            .size(px(10.))
            .rounded_full()
            .border(px(1.5))
            .border_color(palette.faint)
            .into_any_element(),
        Tier::AsksFirst => div()
            .flex_none()
            .size(px(10.))
            .rounded_full()
            .bg(palette.foreground)
            .into_any_element(),
        Tier::AlwaysAsks => ui::icon(Lucide::Lock, palette.foreground)
            .size(px(12.))
            .into_any_element(),
    }
}

fn column(column: &TierColumn, palette: Palette) -> Div {
    v_flex()
        .flex_1()
        .min_w(px(0.))
        .gap(px(10.))
        .p(px(18.))
        .rounded(px(14.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
        .child(
            h_flex().gap(px(10.)).child(shape(column.tier, palette)).child(
                div()
                    .text_size(px(16.))
                    .text_color(palette.foreground)
                    .child(column.title),
            ),
        )
        .child(div().text_size(px(12.5)).text_color(palette.muted).child(column.note))
        .child(v_flex().children(column.lines.iter().map(|line| {
            div()
                .py(px(8.))
                .border_t_1()
                .border_color(palette.border)
                .text_size(px(13.))
                .text_color(palette.foreground)
                .child(line.clone())
        })))
}

/// A row of the tiers: its name, its line, and its value or control.
fn tune_row(
    title: impl Into<SharedString>,
    hint: impl Into<SharedString>,
    value: impl IntoElement,
    palette: Palette,
) -> Div {
    h_flex()
        .w_full()
        .min_h(px(48.))
        .gap(px(16.))
        .py(px(8.))
        .border_t_1()
        .border_color(palette.border)
        .text_size(px(13.5))
        .child(
            div()
                .w(px(200.))
                .flex_none()
                .text_color(palette.foreground)
                .child(title.into()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_color(palette.muted)
                .child(hint.into()),
        )
        .child(value)
}

/// How it writes for the person, under the tiers: as their assistant, signed, or as them, with only words they
/// approved. Each voice is a card with its title, its line and how the message ends, in mono.
fn voices(voices: &[Voice; 2], palette: Palette) -> Div {
    v_flex()
        .w_full()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(17.))
                .text_color(palette.foreground)
                .child("How it writes for you"),
        )
        .child(quiet(
            "It acts in your name only through an approval. By default it writes as itself, and replies come back to you.",
            palette,
        ))
        .child(
            h_flex()
                .w_full()
                .pt(px(8.))
                .gap(px(12.))
                .items_start()
                .children(voices.iter().map(|voice| {
                    card(palette)
                        .flex_1()
                        .px(px(16.))
                        .child(div().text_size(px(13.5)).text_color(palette.foreground).child(voice.title))
                        .child(div().text_size(px(12.5)).text_color(palette.muted).child(voice.line))
                        .child(
                            div()
                                .font_family(ui::MONO)
                                .text_size(px(11.))
                                .text_color(palette.faint)
                                .child(voice.sample.clone()),
                        )
                })),
        )
}

/// A fixed row's value: read-only, in a quiet box.
fn value_box(text: &'static str, palette: Palette) -> Div {
    h_flex()
        .flex_none()
        .h(px(28.))
        .px(px(10.))
        .rounded(px(8.))
        .border_1()
        .border_color(palette.border)
        .text_size(px(12.5))
        .text_color(palette.foreground)
        .child(text)
}

impl Render for ApprovalsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let (outer, mut page) = frame(
            "assistant-approvals",
            "Approvals",
            "Three tiers. What it does freely, what asks first, and what always asks.",
            1040.,
            palette,
        );
        if let Some(status) = &self.status {
            page = page.child(quiet(status.clone(), palette));
        }
        if let Some(refused) = &self.refused {
            page = page.child(problem(refused, palette));
        }
        let ready = match &self.state {
            State::Loading => return outer.child(page.child(quiet("Loading your approval settings…", palette))),
            // The dial is its own read: it still shows when the tiers did not open.
            State::Failed(failed) => {
                return outer.child(page.child(problem(failed, palette)).child(self.proactivity.clone()));
            }
            State::Ready(ready) => ready,
        };
        let view = &ready.view;
        if !view.tiers_on {
            page = page.child(ui::settings_notice(
                "Approval tiers are off for @nexus here",
                "Every write gets a card. These settings apply once they are on.",
                palette,
            ));
        }
        page = page.child(
            h_flex()
                .w_full()
                .items_start()
                .gap(px(14.))
                .children(ready.columns.iter().map(|each| column(each, palette))),
        );

        // Tune the tiers: the first fixed row, the spend line, then the others.
        let working = self.working;
        let save = if working {
            quiet_pill("approvals-save", "Saving…", palette)
        } else {
            primary_pill("approvals-save", "Save", palette)
                .on_click(cx.listener(|page, _, window, cx| page.submit_line(window, cx)))
        };
        let mut spend = Some(
            v_flex()
                .w_full()
                .gap(px(6.))
                .py(px(8.))
                .border_t_1()
                .border_color(palette.border)
                .child(
                    h_flex()
                        .gap(px(16.))
                        .text_size(px(13.5))
                        .child(
                            div()
                                .w(px(200.))
                                .flex_none()
                                .text_color(palette.foreground)
                                .child("Always asks above (USD)"),
                        )
                        .child(div().w(px(140.)).child(ui::input_box(&self.line, palette)))
                        .child(save)
                        .when(view.own_line, |row| {
                            let fallback = view.default_spend_limit_usd.clone();
                            row.child(
                                pill("approvals-default", "Use the default", palette).on_click(cx.listener(
                                    move |page, _, window, cx| {
                                        page.show_line(&fallback, window, cx);
                                        page.save_line(None, window, cx);
                                    },
                                )),
                            )
                        }),
                )
                .child(
                    div()
                        .pl(px(216.))
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(ready.hint.clone()),
                )
                .when_some(self.line_error.clone(), |row, error| {
                    row.child(div().pl(px(216.)).child(ui::error_line(error, palette)))
                }),
        );
        page = page.child(voices(&self.voices, palette));
        let mut tune = v_flex()
            .w_full()
            .child(div().text_size(px(17.)).text_color(palette.foreground).child("Tune the tiers"))
            .child(quiet(
                "Set the amount where a spend stops asking first and always asks. The other rows are the rules Nexus applies today.",
                palette,
            ).pb(px(8.)));
        for (at, row) in ready.rows.iter().enumerate() {
            tune = tune.child(tune_row(
                row.title,
                row.hint.clone(),
                value_box(row.value, palette),
                palette,
            ));
            if at == 0 {
                tune = tune.children(spend.take());
            }
        }
        page = page.child(tune);

        // Learned.
        let mut learned = v_flex()
            .w_full()
            .gap(px(4.))
            .child(
                h_flex()
                    .w_full()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(17.))
                            .text_color(palette.foreground)
                            .child(format!("Learned · {}", view.rules.len())),
                    )
                    .when(!view.rules.is_empty() && self.asking != Some(Asking::All), |head| {
                        head.child(pill("approvals-reset-all", "Reset all", palette).on_click(cx.listener(
                            |page, _, _, cx| {
                                page.asking = Some(Asking::All);
                                cx.notify();
                            },
                        )))
                    }),
            )
            .child(quiet(
                "Things it no longer asks about. A write that always asks is never learned.",
                palette,
            ));
        if self.asking == Some(Asking::All) {
            learned = learned.child(confirm(
                "approvals-confirm-all",
                "Reset every learned rule?",
                "Nexus asks first again for each of them.",
                cx.listener(|page, _, window, cx| page.revoke(None, window, cx)),
                cx.listener(|page, _, _, cx| {
                    page.asking = None;
                    cx.notify();
                }),
                palette,
            ));
        }
        if view.rules.is_empty() {
            learned = learned.child(
                card(palette).mt(px(8.)).child(div().text_size(px(13.)).text_color(palette.foreground).child("Nexus has learned nothing yet")).child(quiet(
                    "On a card that asks first, choose Always allow. Nexus then stops asking for that kind of write to that recipient.",
                    palette,
                )),
            );
        }
        for (group, (to, rules)) in ready.groups.iter().enumerate() {
            learned = learned.child(
                div()
                    .pt(px(10.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(to.clone()),
            );
            for (at, rule) in rules.iter().enumerate() {
                let id = rule.id.clone();
                let asking = self.asking == Some(Asking::Rule(id.clone()));
                let key = group * 1000 + at;
                let action = if asking {
                    let yes = id.clone();
                    confirm(
                        "approvals-confirm-rule",
                        "Reset this rule?",
                        "Nexus asks first again.",
                        cx.listener(move |page, _, window, cx| page.revoke(Some(yes.clone()), window, cx)),
                        cx.listener(|page, _, _, cx| {
                            page.asking = None;
                            cx.notify();
                        }),
                        palette,
                    )
                    .into_any_element()
                } else {
                    pill(("approvals-reset", key), "Reset", palette)
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.asking = Some(Asking::Rule(id.clone()));
                            cx.notify();
                        }))
                        .into_any_element()
                };
                learned = learned.child(
                    h_flex()
                        .w_full()
                        .min_h(px(48.))
                        .gap(px(12.))
                        .border_t_1()
                        .border_color(palette.border)
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w(px(0.))
                                .gap(px(1.))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(palette.foreground)
                                        .child(words::rule_text(&rule.kind, rule.to.as_deref()).0),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(palette.faint)
                                        .child(format!("Since {}", day(&rule.since))),
                                ),
                        )
                        .child(action),
                );
            }
        }
        outer.child(page.child(learned).child(self.proactivity.clone()))
    }
}
