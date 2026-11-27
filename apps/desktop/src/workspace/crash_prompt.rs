//! The prompt after a crash, as the Nexus app's (`crash-prompt.tsx`, Nexus UI's `CrashReportDialog`): once
//! per start and only signed in, the last run's crash (`crash.rs`) is offered to the Nexus team with an optional note.
//! "Do not send" drops it. "Always send" turns on "Send crash and error reports" (Settings, Appearance); with it on, the
//! next crash goes without asking and a short note says so. A send that fails keeps the prompt open to try again.

use crate::workspace::glass::FloatingGround;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

use super::{Workspace, glyph};
use crate::crash::{self, SeenCrash};
use crate::gateway::Verb;
use crate::palette::Palette;

/// How long the "report went" note stays.
const SENT_NOTE_FOR: Duration = Duration::from_secs(6);
const SENT_NOTE: &str = "Nexus quit unexpectedly last time. A crash report went to the Nexus team.";
const ENDPOINT: &str = "/v1/diagnostics/feedback";

/// A crash waiting for the person's answer.
pub struct Open {
    crash: SeenCrash,
    os_version: String,
    report: Value,
    note: Entity<TextareaState>,
    busy: bool,
    problem: Option<SharedString>,
    details: bool,
}

/// The prompt, or the note after a send without asking.
#[derive(Default)]
pub struct CrashPrompt {
    open: Option<Open>,
    sent_note: bool,
    _task: Option<Task<()>>,
}

/// The words for a failed send (`feedback::failure`), naming crash reports.
fn failure(error: &str) -> SharedString {
    crate::feedback::failure(error, "Crash reports").into()
}

impl Workspace {
    /// Once per start, after the first sign-in: take the last run's crash, then send it or ask.
    pub(super) fn take_crash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            return;
        }
        self.crash_prompt._task = Some(cx.spawn_in(window, async move |this, cx| {
            // The file and the OS version are read off the UI thread.
            let found = cx
                .background_spawn(async { crash::take().map(|seen| (seen, crash::os_version())) })
                .await;
            let Some((seen, os_version)) = found else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if crate::paths::error_reports() {
                    this.send_crash_unasked(seen, os_version, window, cx);
                } else {
                    this.ask_crash(seen, os_version, None, window, cx);
                }
            });
        }));
    }

    fn ask_crash(
        &mut self,
        crash: SeenCrash,
        os_version: String,
        problem: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let note = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("I had just opened a session and…")
                .rows(3)
        });
        let report = crash::report(&crash, &os_version);
        self.crash_prompt.open = Some(Open {
            crash,
            os_version,
            report,
            note,
            busy: false,
            problem,
            details: false,
        });
        cx.notify();
    }

    /// "Send crash and error reports" is on: it goes, and a note says so; a failure asks instead.
    fn send_crash_unasked(
        &mut self,
        crash: SeenCrash,
        os_version: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let body = crash::feedback(&crash, &os_version, "");
        let request = account_gateway.request(Verb::Post, ENDPOINT.into(), Some(body));
        self.crash_prompt._task = Some(cx.spawn_in(window, async move |this, cx| {
            let sent = request.await;
            let _ = this.update_in(cx, |this, window, cx| match sent {
                Ok(_) => this.show_sent_note(window, cx),
                Err(error) => this.ask_crash(crash, os_version, Some(failure(&error)), window, cx),
            });
        }));
    }

    fn show_sent_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.crash_prompt.sent_note = true;
        cx.notify();
        self.crash_prompt._task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SENT_NOTE_FOR).await;
            let _ = this.update(cx, |this, cx| {
                this.crash_prompt.sent_note = false;
                cx.notify();
            });
        }));
    }

    /// "Send report": the crash and the note; the prompt closes once it went.
    fn send_crash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(open) = self.crash_prompt.open.as_mut() else {
            return;
        };
        if open.busy {
            return;
        }
        let note = open.note.read(cx).value().to_string();
        let body = crash::feedback(&open.crash, &open.os_version, &note);
        open.busy = true;
        open.problem = None;
        let request = account_gateway.request(Verb::Post, ENDPOINT.into(), Some(body));
        cx.notify();
        self.crash_prompt._task = Some(cx.spawn_in(window, async move |this, cx| {
            let sent = request.await;
            let _ = this.update(cx, |this, cx| {
                match sent {
                    Ok(_) => this.crash_prompt.open = None,
                    Err(error) => {
                        if let Some(open) = this.crash_prompt.open.as_mut() {
                            open.busy = false;
                            open.problem = Some(failure(&error));
                        }
                    }
                }
                cx.notify();
            });
        }));
    }

    fn close_crash(&mut self, cx: &mut Context<Self>) {
        if self.crash_prompt.open.as_ref().is_some_and(|open| !open.busy) {
            self.crash_prompt.open = None;
            cx.notify();
        }
    }

    /// The prompt over the window, or the note after a send without asking.
    pub(super) fn render_crash_prompt(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.crash_prompt.sent_note && self.crash_prompt.open.is_none() {
            return Some(
                deferred(
                    div().absolute().bottom(px(24.)).left_0().w_full().child(
                        h_flex().w_full().justify_center().child(
                            div()
                                .px(px(14.))
                                .py(px(10.))
                                .rounded(px(10.))
                                .floating_ground(10., palette.pop, true, &palette)
                                .border_1()
                                .border_color(palette.border)
                                .shadow(palette.pop_shadow(palette.border))
                                .text_size(px(13.))
                                .child(SENT_NOTE),
                        ),
                    ),
                )
                .with_priority(3)
                .into_any_element(),
            );
        }
        let open = self.crash_prompt.open.as_ref()?;
        let always = crate::paths::error_reports();
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(6.))
                .rounded(px(8.))
                .text_size(px(13.))
                .when(primary, |button| {
                    button.bg(palette.primary).text_color(palette.primary_foreground)
                })
                .when(!primary, |button| {
                    button
                        .border_1()
                        .border_color(palette.border)
                        .text_color(palette.foreground)
                        .hover(|style| style.bg(palette.hover))
                })
                .when(open.busy, |button| button.opacity(0.6))
                .when(!open.busy, |button| button.cursor_pointer())
                .child(label)
        };
        let dialog = v_flex()
            .id("crash-prompt")
            .occlude()
            .w(px(460.))
            .max_w(relative(0.94))
            .p(px(20.))
            .gap(px(12.))
            .rounded(px(14.))
            .floating_ground(14., palette.pop, false, &palette)
            .shadow(palette.pop_shadow(palette.border))
            .text_color(palette.foreground)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.strong)
                            .child("Nexus quit unexpectedly"),
                    )
                    .child(
                        div()
                            .id("crash-close")
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|style| style.bg(palette.hover))
                            .child(glyph(Lucide::X, palette.icon))
                            .on_click(cx.listener(|this, _, _, cx| this.close_crash(cx))),
                    ),
            )
            .child(div().text_size(px(13.)).text_color(palette.muted).child(
                "Send a crash report so the Nexus team can fix it. It says what failed and where in the Nexus code, \
                 never your chats, code or keys.",
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child("What were you doing? (optional)"),
            )
            .child(
                div()
                    .p(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(palette.border)
                    .text_size(px(13.))
                    .child(Textarea::new(&open.note).appearance(false)),
            )
            .child(
                h_flex()
                    .id("crash-details")
                    .gap(px(4.))
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .child(glyph(
                        if open.details {
                            Lucide::ChevronDown
                        } else {
                            Lucide::ChevronRight
                        },
                        palette.icon,
                    ))
                    .child("What the report holds")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(open) = this.crash_prompt.open.as_mut() {
                            open.details = !open.details;
                            cx.notify();
                        }
                    })),
            )
            .when(open.details, |dialog| {
                dialog.child(
                    div()
                        .p(px(8.))
                        .rounded(px(8.))
                        .bg(palette.background)
                        .font_family("Geist Mono")
                        .text_size(px(11.5))
                        .text_color(palette.muted)
                        .child(crash::summary(&open.report)),
                )
            })
            .child(
                h_flex()
                    .id("crash-always")
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .child(
                        div()
                            .size(px(14.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(if always { palette.primary } else { palette.border })
                            .when(always, |box_| box_.bg(palette.primary))
                            .when(always, |box_| {
                                box_.child(glyph(Lucide::Check, palette.primary_foreground).size(px(10.)))
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child("Always send crash reports to the team, with my name and email, without asking"),
                    )
                    .on_click(cx.listener(|_, _, _, cx| {
                        // A failed save is logged; the checkbox shows the saved value.
                        let _ = crate::paths::save_error_reports(!crate::paths::error_reports());
                        cx.notify();
                    })),
            )
            .when_some(open.problem.clone(), |dialog, problem| {
                dialog.child(super::feedback_dialog::problem_line("crash:send", problem, palette, cx))
            })
            .child(
                h_flex()
                    .gap(px(8.))
                    .justify_end()
                    .child(
                        button("crash-dont-send", "Do not send", false)
                            .on_click(cx.listener(|this, _, _, cx| this.close_crash(cx))),
                    )
                    .child(
                        button("crash-send", if open.busy { "Sending…" } else { "Send report" }, true)
                            .on_click(cx.listener(|this, _, window, cx| this.send_crash(window, cx))),
                    ),
            );
        Some(
            deferred(
                div()
                    .id("crash-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .bg(palette.scrim)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(dialog),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::failure;

    #[test]
    fn a_server_without_the_channel_is_named_and_anything_else_is_retried() {
        assert_eq!(
            failure("feedback is not configured on this Gateway (NEXUS_FEEDBACK_SLACK_WEBHOOK_URL)").as_ref(),
            "Crash reports go nowhere on this Gateway yet."
        );
        assert!(failure("Nexus Gateway did not answer in time").contains("try again"));
    }
}
