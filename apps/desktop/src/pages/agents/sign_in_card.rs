//! The card of the sign-in Nexus runs (`subscriptions/sign_in.rs`), under its row or under the list. Three steps,
//! each with its mark: "Browser opened", "Waiting for sign-in", "Read your plan". Codex's
//! device code shows the code itself. A failure says what the login said, with Try again and a terminal instead.

use std::time::Instant;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::Input;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::event::SessionAgent;

use super::super::subscriptions::login::Mode;
use super::super::subscriptions::sign_in::{SignIn, Step};
use super::super::subscriptions::{sign_in_hint, vendor};
use super::super::{palette, ui};
use super::AgentsPage;
use crate::palette::Palette;

/// What the card draws, read from the sign-in once.
#[derive(Clone, Debug)]
pub(super) struct Shown {
    pub agent: SessionAgent,
    /// The login's key: the card sits under its row, unless the account is new.
    pub key: String,
    pub new: bool,
    command: String,
    step: Step,
    mode: Mode,
    link: Option<String>,
    code: Option<(String, Instant)>,
    last_line: Option<String>,
    code_sent: bool,
    terminal: bool,
    paste_open: bool,
    terminal_open: bool,
}

impl Shown {
    pub(super) fn of(sign_in: &SignIn) -> Shown {
        Shown {
            agent: sign_in.agent,
            key: sign_in.key(),
            new: sign_in.new,
            command: sign_in.command.clone(),
            step: sign_in.step.clone(),
            mode: sign_in.mode,
            link: sign_in.link.clone(),
            code: sign_in.code.clone(),
            last_line: sign_in.last_line.clone(),
            code_sent: sign_in.code_sent,
            terminal: sign_in.terminal,
            paste_open: sign_in.paste_open,
            terminal_open: sign_in.terminal_open,
        }
    }
}

/// The agent as its login names itself. Nexus signs in only Claude Code and Codex (`Plans::begin`).
fn said_by(agent: SessionAgent) -> &'static str {
    nexus_workbench::harness::harness(agent).label()
}

/// Why a login with no plan cannot pay.
pub(super) fn no_plan(agent: SessionAgent, name: &str) -> String {
    format!(
        "{name} has no {} plan, so {} cannot pay with it.",
        super::super::subscriptions::vendor(agent),
        said_by(agent)
    )
}

/// Why the sign-in failed.
pub(super) fn failure(agent: SessionAgent, step: &Step) -> Option<String> {
    match step {
        Step::NotStarted(reason) => Some(reason.clone()),
        Step::Failed(Some(line)) => Some(format!("The sign-in did not finish. {} said: {line}", said_by(agent))),
        Step::Failed(None) => Some("The sign-in did not finish.".into()),
        Step::TimedOut => Some("The sign-in took longer than 15 minutes.".into()),
        _ => None,
    }
}

/// How long a device code has left, in whole minutes rounded up; 0 once it expired.
pub(super) fn minutes_left(expires_at: Instant, now: Instant) -> u64 {
    expires_at.saturating_duration_since(now).as_secs().div_ceil(60)
}

/// A step's mark: done, the one that runs, or one to come.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Mark {
    Done,
    Current,
    Todo,
}

/// The three steps' marks at `step`.
pub(super) fn marks(step: &Step) -> [Mark; 3] {
    use Mark::*;
    match step {
        Step::Starting => [Current, Todo, Todo],
        Step::Waiting => [Done, Current, Todo],
        Step::Reading => [Done, Done, Current],
        _ => [Done, Done, Done],
    }
}

fn mark(mark: Mark, id: SharedString, reduce_motion: bool, palette: Palette) -> AnyElement {
    let slot = div().flex_none().size(px(16.)).flex().items_center().justify_center();
    match mark {
        Mark::Done => slot
            .child(ui::icon(Lucide::Check, palette.muted).size(px(14.)))
            .into_any_element(),
        // The shared loader, as a removal turns it; with less motion, a still dot.
        Mark::Current if reduce_motion => slot
            .child(
                div()
                    .size(px(6.))
                    .rounded(px(3.))
                    .bg(palette.muted)
                    .debug_selector(|| "sign-in-dot".into()),
            )
            .into_any_element(),
        Mark::Current => slot
            .debug_selector(|| "sign-in-spinner".into())
            .child(
                ui::icon(Lucide::LoaderCircle, palette.muted)
                    .size(px(14.))
                    .with_animation(
                        id,
                        // As the removal's loader: a loop is capped (AGENTS.md, render rule 4).
                        Animation::new(std::time::Duration::from_millis(1000))
                            .repeat()
                            .with_max_fps(30.),
                        |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
                    ),
            )
            .into_any_element(),
        Mark::Todo => slot
            .child(
                div()
                    .size(px(10.))
                    .rounded(px(5.))
                    .border(px(1.5))
                    .border_color(palette.faint),
            )
            .into_any_element(),
    }
}

/// The card. `framed`: under the list, in its own frame; else under its row, inside the accounts card.
pub(super) fn card(
    page: &AgentsPage,
    shown: &Shown,
    copied: Option<&str>,
    framed: bool,
    reduce_motion: bool,
    cx: &mut Context<AgentsPage>,
) -> AnyElement {
    let palette = palette(cx);
    let agent = shown.agent;
    let id = |what: &str| SharedString::from(format!("plans-sign-in-{what}-{}", agent.as_str()));
    let plans = page.plans.clone();
    let action = move |label: &'static str,
                       what: &str,
                       primary: bool,
                       run: fn(&mut super::Plans, &mut Context<super::Plans>)| {
        let plans = plans.clone();
        let button = if primary {
            ui::button(id(what), label, palette)
        } else {
            ui::ghost_button(id(what), label, palette)
        };
        button
            .on_click(move |_, _, cx| plans.update(cx, run))
            .into_any_element()
    };
    let copy_button = |text: String, label: &'static str, what: &str| {
        let plans = page.plans.clone();
        let done = copied == Some(text.as_str());
        ui::ghost_button(id(what), if done { "Copied" } else { label }, palette)
            .on_click(move |_, _, cx| plans.update(cx, |plans, cx| plans.copy(text.clone(), cx)))
            .into_any_element()
    };
    let open_button = |link: String, label: &'static str, what: &str| {
        ui::ghost_button(id(what), label, palette)
            .on_click(move |_, _, cx| crate::external_link::open(&link, cx))
            .into_any_element()
    };
    let cancel = || action("Cancel", "cancel", false, |plans, cx| plans.close_sign_in(cx));
    let close = || action("Close", "close", false, |plans, cx| plans.close_sign_in(cx));
    let another = || {
        action("Sign in another account", "another", true, |plans, cx| {
            plans.sign_in_another(cx)
        })
    };
    let terminal_toggle = || {
        action("Having trouble?", "terminal-toggle", false, |plans, cx| {
            plans.toggle_sign_in_part(false, cx)
        })
    };

    let mut body = v_flex().gap(px(12.));
    if shown.terminal {
        body = body
            .child(ui::setting_detail(
                "Finish the sign-in in the terminal. Nexus checks for it every few seconds.",
                palette,
            ))
            .child(terminal_part(page, shown, copied, palette))
            .child(h_flex().justify_end().child(close()));
    } else if let Some(why) = failure(agent, &shown.step) {
        body = body
            .child(ui::error_line(why, palette))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(action("Try again", "again", true, |plans, cx| {
                        plans.try_sign_in_again(cx)
                    }))
                    .child(terminal_toggle())
                    .child(div().flex_1())
                    .child(close()),
            )
            .when(shown.terminal_open, |body| {
                body.child(terminal_part(page, shown, copied, palette))
            });
    } else if let Step::Duplicate(email) = &shown.step {
        body = body
            .child(ui::setting_detail(
                format!("{email} is already on this computer. Nexus kept the existing account."),
                palette,
            ))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(another())
                    .child(div().flex_1())
                    .child(close()),
            );
    } else if let Step::NoPlan(name) = &shown.step {
        body = body.child(ui::setting_detail(no_plan(agent, name), palette)).child(
            h_flex()
                .gap(px(8.))
                .child(another())
                .child(action("Remove it", "remove", false, |plans, cx| {
                    plans.remove_signed_in(cx)
                }))
                .child(div().flex_1())
                .child(close()),
        );
    } else if let (Mode::DeviceCode, Some((code, expires_at)), Step::Waiting) = (shown.mode, &shown.code, &shown.step) {
        let left = minutes_left(*expires_at, Instant::now());
        body = body
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .child("Open auth.openai.com/codex/device and enter this code"),
            )
            .child(
                div()
                    .px(px(14.))
                    .py(px(10.))
                    .rounded(px(8.))
                    .bg(palette.hover)
                    .font_family(ui::MONO)
                    .text_size(px(22.))
                    .text_color(palette.strong)
                    .when(left == 0, |code| code.opacity(0.4))
                    .child(code.clone()),
            )
            .child(ui::setting_detail(
                if left == 0 {
                    "The code expired.".to_string()
                } else {
                    format!(
                        "The code expires in {left} {}. Enter it only if you started this sign-in in Nexus.",
                        if left == 1 { "minute" } else { "minutes" }
                    )
                },
                palette,
            ))
            .child(
                h_flex()
                    .gap(px(8.))
                    .map(|row| {
                        if left == 0 {
                            row.child(action("Get a new code", "new-code", true, |plans, cx| {
                                plans.use_device_code(cx)
                            }))
                        } else {
                            row.child(copy_button(code.clone(), "Copy code", "copy-code")).children(
                                shown
                                    .link
                                    .clone()
                                    .map(|link| open_button(link, "Open the page", "open")),
                            )
                        }
                    })
                    .child(div().flex_1())
                    .child(cancel()),
            );
    } else {
        let labels = [
            "Browser opened".to_string(),
            if shown.step == Step::Waiting {
                format!("Waiting for sign-in with {}", vendor(agent))
            } else {
                format!("Sign in with {}", vendor(agent))
            },
            "Read your plan".to_string(),
        ];
        let steps = marks(&shown.step);
        body = body.child(
            v_flex()
                .gap(px(8.))
                .children(labels.into_iter().zip(steps).enumerate().map(|(index, (label, step))| {
                    h_flex()
                        .gap(px(10.))
                        .child(mark(step, id(&format!("spinner-{index}")), reduce_motion, palette))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(if step == Mark::Todo {
                                    palette.faint
                                } else if step == Mark::Done {
                                    palette.muted
                                } else {
                                    palette.foreground
                                })
                                .child(label),
                        )
                })),
        );
        if shown.step == Step::Waiting {
            body = body.child(
                h_flex()
                    .gap(px(8.))
                    .children(
                        shown
                            .link
                            .clone()
                            .map(|link| open_button(link, "Open the page again", "open")),
                    )
                    .children(
                        shown
                            .link
                            .clone()
                            .map(|link| copy_button(link, "Copy link", "copy-link")),
                    )
                    .when(crate::agents::ui(agent).device_code(), |row| {
                        row.child(action("Use a code instead", "device", false, |plans, cx| {
                            plans.use_device_code(cx)
                        }))
                    })
                    .child(div().flex_1())
                    .child(cancel()),
            );
            if crate::agents::ui(agent).pastes_code() {
                body = body.child(paste_part(page, shown, palette, cx));
            }
            body = body.child(
                h_flex().child(action("Having trouble?", "terminal-toggle", false, |plans, cx| {
                    plans.toggle_sign_in_part(false, cx)
                })),
            );
            if shown.terminal_open {
                body = body.child(terminal_part(page, shown, copied, palette));
            }
        }
    }
    if framed {
        ui::card_frame(palette)
            .mt(px(10.))
            .p(px(14.))
            .child(body)
            .into_any_element()
    } else {
        div().px(px(16.)).pb(px(14.)).child(body).into_any_element()
    }
}

/// Claude: the code the browser may show, pasted into a private field.
fn paste_part(page: &AgentsPage, shown: &Shown, palette: Palette, cx: &mut Context<AgentsPage>) -> AnyElement {
    let agent = shown.agent;
    let id = |what: &str| SharedString::from(format!("plans-sign-in-{what}-{}", agent.as_str()));
    let toggle = {
        let plans = page.plans.clone();
        ui::ghost_button(id("paste-toggle"), "The browser shows a code? Paste it here", palette)
            .on_click(move |_, _, cx| plans.update(cx, |plans, cx| plans.toggle_sign_in_part(true, cx)))
    };
    let refused = shown.last_line.clone().filter(|_| shown.code_sent);
    v_flex()
        .gap(px(8.))
        .child(h_flex().child(toggle))
        .when(shown.paste_open, |part| {
            let plans = page.plans.clone();
            let input = page.code_input.clone();
            part.child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .px(px(8.))
                            .rounded(px(6.))
                            .bg(palette.hover)
                            .child(Input::new(&page.code_input).appearance(false)),
                    )
                    .child(
                        ui::button(id("paste-continue"), "Continue", palette).on_click(cx.listener(
                            move |_, _, window, cx| {
                                // The code leaves the field at once: Nexus keeps no copy.
                                let code = input.read(cx).value().to_string();
                                input.update(cx, |state, cx| state.set_value("", window, cx));
                                plans.update(cx, |plans, cx| plans.submit_code(code, cx));
                            },
                        )),
                    ),
            )
            .child(ui::setting_detail(
                "Paste it only if you started this sign-in. Nexus sends it to Claude's login and keeps no copy.",
                palette,
            ))
            .when_some(refused, |part, line| part.child(ui::error_line(line, palette)))
        })
        .into_any_element()
}

/// The command for a terminal. Copy command or Open terminal stops Nexus's own login first.
fn terminal_part(page: &AgentsPage, shown: &Shown, copied: Option<&str>, palette: Palette) -> AnyElement {
    let agent = shown.agent;
    let id = |what: &str| SharedString::from(format!("plans-sign-in-{what}-{}", agent.as_str()));
    let (copy, open) = (page.plans.clone(), page.plans.clone());
    let done = copied == Some(shown.command.as_str());
    v_flex()
        .gap(px(8.))
        .when(crate::agents::ui(agent).hint_in_terminal(), |part| {
            part.child(ui::setting_detail(sign_in_hint(agent), palette))
        })
        .child(
            h_flex()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(6.))
                        .bg(palette.hover)
                        .font_family(ui::MONO)
                        .text_size(px(12.))
                        .text_color(palette.foreground)
                        .child(shown.command.clone()),
                )
                .child(
                    ui::ghost_button(
                        id("copy-command"),
                        if done { "Copied" } else { "Copy command" },
                        palette,
                    )
                    .on_click(move |_, _, cx| copy.update(cx, |plans, cx| plans.copy_sign_in_command(cx))),
                )
                .child(
                    ui::button(id("terminal"), "Open terminal", palette)
                        .on_click(move |_, _, cx| open.update(cx, |plans, cx| plans.open_sign_in_terminal(cx))),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use std::time::Duration;

    #[test]
    fn marks_follow_the_step() {
        assert_eq!(marks(&Step::Starting), [Mark::Current, Mark::Todo, Mark::Todo]);
        assert_eq!(marks(&Step::Waiting), [Mark::Done, Mark::Current, Mark::Todo]);
        assert_eq!(marks(&Step::Reading), [Mark::Done, Mark::Done, Mark::Current]);
    }

    #[test]
    fn says_why_a_sign_in_ended() {
        assert_eq!(
            failure(
                SessionAgent::Claude,
                &Step::Failed(Some("OAuth error: invalid_grant".into()))
            )
            .as_deref(),
            Some("The sign-in did not finish. Claude Code said: OAuth error: invalid_grant")
        );
        assert_eq!(
            failure(SessionAgent::Codex, &Step::TimedOut).as_deref(),
            Some("The sign-in took longer than 15 minutes.")
        );
        assert_eq!(failure(SessionAgent::Codex, &Step::Waiting), None);
        assert_eq!(
            no_plan(SessionAgent::Codex, "ada@acme.test"),
            "ada@acme.test has no ChatGPT plan, so Codex cannot pay with it."
        );
    }

    #[test]
    fn a_device_code_counts_down_in_whole_minutes() {
        let now = Instant::now();
        assert_eq!(minutes_left(now + Duration::from_secs(15 * 60), now), 15);
        assert_eq!(minutes_left(now + Duration::from_secs(61), now), 2);
        assert_eq!(minutes_left(now + Duration::from_secs(1), now), 1);
        assert_eq!(minutes_left(now, now), 0);
    }
}
