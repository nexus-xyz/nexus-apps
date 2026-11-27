//! Privacy and trust, as Nexus UI's `agent-privacy.tsx`: the two switches that stop everything, and
//! what Nexus can see per account.
//!
//! - Pause: one confirm, then every errand, watch, brief and message stops. While paused, a bar says since when, with
//!   Resume, and the sidebar's Assistant row says Paused (`Moves::paused`). Nothing is deleted.
//! - Revoke all access: a dialog that names each agent link and each of the person's own accounts that ends, and says
//!   the organization's accounts stay. Then one call ends them, and the view reads again.
//! - What it can see: per connected account, what Nexus reads and what it never does there, and the one sentence about
//!   training.
//!
//! The strict reads and the words are `nexus_desktop_core::agent_privacy`'s. An agent API without the pause (404)
//! says Privacy is not here yet.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::super::{Ctx, palette, ui};
use super::{
    Moves, Reply, call, card, danger_pill, frame, head, modal, pill, primary_pill, problem, quiet, quiet_pill, sheet,
};
use crate::agent::Call;
use crate::palette::Palette;
use nexus_desktop_core::agent::Problem;
use nexus_desktop_core::agent_privacy::{self as words, PrivacyView, Request};

/// What the page shows: loading, the view, or why it did not open.
enum State {
    Loading,
    Ready(Box<Ready>),
    Failed(Problem),
}

/// One account's row, its words computed when the view was read.
struct Row {
    name: String,
    whose: &'static str,
    reads: String,
    writes: Option<String>,
    never: String,
    error: Option<String>,
}

/// The view once read, with its words.
struct Ready {
    view: PrivacyView,
    paused_line: Option<String>,
    revoke_text: String,
    rows: Vec<Row>,
}

impl Ready {
    fn of(view: PrivacyView) -> Ready {
        let rows = view
            .accounts
            .iter()
            .map(|account| {
                let (name, whose) = words::account_head(account);
                Row {
                    name,
                    whose,
                    reads: words::reads_line(account),
                    writes: words::writes_line(account),
                    never: words::never_line(account),
                    error: account.error.clone(),
                }
            })
            .collect();
        Ready {
            paused_line: view
                .pause
                .paused
                .then(|| words::paused_line(view.pause.paused_at.as_deref().and_then(clock).as_deref())),
            revoke_text: words::revoke_text(&view),
            rows,
            view,
        }
    }
}

/// An ISO time as the person's clock reads it: "14:02".
fn clock(iso: &str) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    Some(at.with_timezone(&chrono::Local).format("%H:%M").to_string())
}

/// The dialog that waits for the person's yes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Asking {
    Pause,
    Revoke,
}

pub struct PrivacyPage {
    ctx: Ctx,
    moves: Moves,
    state: State,
    /// A change that failed, over the page.
    refused: Option<Problem>,
    /// What the last change did.
    status: Option<String>,
    asking: Option<Asking>,
    working: bool,
    focus: FocusHandle,
    _read: Task<()>,
    _change: Option<Task<()>>,
}

pub fn build(ctx: Ctx, moves: Moves, window: &mut Window, cx: &mut App) -> Entity<PrivacyPage> {
    cx.new(|cx| {
        let read = call(&ctx, Call::Privacy(Request::Read), window, cx, PrivacyPage::read);
        PrivacyPage {
            ctx,
            moves,
            state: State::Loading,
            refused: None,
            status: None,
            asking: None,
            working: false,
            focus: cx.focus_handle(),
            _read: read,
            _change: None,
        }
    })
}

impl PrivacyPage {
    fn read(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        self.state = match (reply.ok(), reply.body.as_ref().and_then(words::privacy_view_of)) {
            (true, Some(view)) => {
                self.told(view.pause.paused, window, cx);
                State::Ready(Box::new(Ready::of(view)))
            }
            (true, None) => State::Failed(words::privacy_problem(502, None)),
            (false, _) => State::Failed(words::privacy_problem(reply.status, reply.body.as_ref())),
        };
    }

    /// Tell the sidebar's row whether the Assistant is paused.
    fn told(&self, paused: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(said) = self.moves.paused.clone() {
            said(paused, window, cx);
        }
    }

    /// Read the view again: after Revoke all access, what is still connected and the links that are left.
    fn reread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._read = call(&self.ctx, Call::Privacy(Request::Read), window, cx, PrivacyPage::read);
    }

    /// Open a dialog, with the focus in it so Esc closes it.
    pub fn ask(&mut self, asking: Asking, window: &mut Window, cx: &mut Context<Self>) {
        if self.working {
            return;
        }
        self.asking = Some(asking);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.asking = None;
        cx.notify();
    }

    /// Send one change; `done` gets its body once it succeeded.
    fn change(
        &mut self,
        made: Option<Request>,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, serde_json::Value, &mut Window, &mut Context<Self>) + 'static,
    ) {
        self.asking = None;
        let Some(made) = made else { return };
        if self.working {
            return;
        }
        self.working = true;
        self.refused = None;
        self._change = Some(call(
            &self.ctx,
            Call::Privacy(made),
            window,
            cx,
            move |page, reply, window, cx| {
                page.working = false;
                if reply.ok() {
                    done(page, reply.body.unwrap_or_default(), window, cx);
                } else {
                    page.refused = Some(words::privacy_problem(reply.status, reply.body.as_ref()));
                }
            },
        ));
    }

    /// Pause or resume: Pause after the dialog's yes, Resume from the bar.
    pub fn set_paused(&mut self, paused: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.change(Request::set_pause(paused), window, cx, |page, body, window, cx| {
            let Some(pause) = words::pause_view_of(&body) else {
                page.refused = Some(words::privacy_problem(502, None));
                return;
            };
            page.told(pause.paused, window, cx);
            page.status = Some(
                if pause.paused {
                    words::PAUSED_STATUS
                } else {
                    words::RESUMED_STATUS
                }
                .into(),
            );
            if let State::Ready(ready) = &mut page.state {
                let mut view = ready.view.clone();
                view.pause = pause;
                **ready = Ready::of(view);
            }
        });
    }

    /// Revoke all access, after the dialog's yes: every agent link and the person's own accounts.
    pub fn revoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.change(
            Request::revoke(true),
            window,
            cx,
            |page, body, window, cx| match words::revoke_result_of(&body) {
                Some(result) => {
                    page.status = Some(words::revoked_text(&result));
                    page.reread(window, cx);
                }
                None => page.refused = Some(words::privacy_problem(502, None)),
            },
        );
    }

    #[cfg(test)]
    pub fn view(&self) -> Option<&PrivacyView> {
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
    pub fn asking(&self) -> Option<Asking> {
        self.asking
    }

    #[cfg(test)]
    pub fn dialog_text(&self) -> Option<&str> {
        match &self.state {
            State::Ready(ready) => Some(&ready.revoke_text),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn paused_line(&self) -> Option<&str> {
        match &self.state {
            State::Ready(ready) => ready.paused_line.as_deref(),
            _ => None,
        }
    }
}

/// A switch's row: its name and line, and its pill.
fn switch_row(title: &'static str, line: &'static str, action: Option<AnyElement>, palette: Palette) -> Div {
    h_flex()
        .w_full()
        .gap(px(16.))
        .py(px(12.))
        .border_t_1()
        .border_color(palette.border)
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(px(2.))
                .child(div().text_size(px(14.)).text_color(palette.foreground).child(title))
                .child(quiet(line, palette)),
        )
        .children(action)
}

/// The table's three columns: the account, what it reads, and what it never does.
fn columns(account: impl IntoElement, reads: impl IntoElement, never: impl IntoElement, palette: Palette) -> Div {
    h_flex()
        .w_full()
        .items_start()
        .gap(px(16.))
        .py(px(10.))
        .border_t_1()
        .border_color(palette.border)
        .child(div().w(px(240.)).flex_none().min_w(px(0.)).child(account))
        .child(div().flex_1().min_w(px(0.)).child(reads))
        .child(div().flex_1().min_w(px(0.)).child(never))
}

fn account_row(row: &Row, palette: Palette) -> Div {
    let account = v_flex()
        .gap(px(1.))
        .child(
            div()
                .text_size(px(13.))
                .text_color(palette.foreground)
                .child(row.name.clone()),
        )
        .child(div().text_size(px(12.)).text_color(palette.faint).child(row.whose));
    if let Some(error) = &row.error {
        return columns(account, quiet(error.clone(), palette), div(), palette);
    }
    let reads = v_flex()
        .gap(px(2.))
        .child(
            div()
                .text_size(px(13.))
                .text_color(palette.foreground)
                .child(row.reads.clone()),
        )
        .when_some(row.writes.clone(), |reads, writes| {
            reads.child(div().text_size(px(12.)).text_color(palette.faint).child(writes))
        });
    let never = div()
        .text_size(px(13.))
        .text_color(palette.muted)
        .child(row.never.clone());
    columns(account, reads, never, palette)
}

impl Render for PrivacyPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let (outer, mut page) = frame(
            "assistant-privacy",
            "Privacy",
            "Two switches that stop everything, and what Nexus can see per account.",
            880.,
            palette,
        );
        if let Some(status) = &self.status {
            page = page.child(quiet(status.clone(), palette));
        }
        if let Some(refused) = &self.refused {
            page = page.child(problem(refused, palette));
        }
        let ready = match &self.state {
            State::Loading => {
                return div()
                    .size_full()
                    .child(outer.child(page.child(quiet("Loading your privacy settings…", palette))));
            }
            State::Failed(failed) => {
                return div()
                    .size_full()
                    .child(outer.child(page.child(problem(failed, palette))));
            }
            State::Ready(ready) => ready,
        };
        let working = self.working;

        // The paused bar, with Resume.
        if let Some(line) = &ready.paused_line {
            let resume = if working {
                quiet_pill("privacy-resume", "Resume", palette).into_any_element()
            } else {
                primary_pill("privacy-resume", "Resume", palette)
                    .on_click(cx.listener(|page, _, window, cx| page.set_paused(false, window, cx)))
                    .into_any_element()
            };
            page = page.child(
                h_flex()
                    .w_full()
                    .gap(px(12.))
                    .px(px(14.))
                    .py(px(10.))
                    .rounded(px(12.))
                    .bg(palette.hover)
                    .child(ui::icon(Lucide::Pause, palette.foreground))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(13.))
                            .text_color(palette.foreground)
                            .child(line.clone()),
                    )
                    .child(resume),
            );
        }

        // The two switches.
        let pause = (!ready.view.pause.paused).then(|| {
            if working {
                quiet_pill("privacy-pause", "Pause", palette).into_any_element()
            } else {
                pill("privacy-pause", "Pause", palette)
                    .on_click(cx.listener(|page, _, window, cx| page.ask(Asking::Pause, window, cx)))
                    .into_any_element()
            }
        });
        let revoke = if working {
            quiet_pill("privacy-revoke", "Revoke all", palette).into_any_element()
        } else {
            danger_pill("privacy-revoke", "Revoke all", palette)
                .on_click(cx.listener(|page, _, window, cx| page.ask(Asking::Revoke, window, cx)))
                .into_any_element()
        };
        page = page.child(
            card(palette)
                .gap(px(0.))
                .pt(px(2.))
                .child(switch_row(
                    "Pause the Assistant",
                    "Every errand, watch, brief and message stops at once. Nothing is deleted.",
                    pause,
                    palette,
                ))
                .child(switch_row(
                    "Revoke all access",
                    "Ends every agent link and disconnects your own accounts. You can reconnect one at a time.",
                    Some(revoke),
                    palette,
                )),
        );

        // What it can see, per account.
        let mut see = v_flex()
            .w_full()
            .gap(px(4.))
            .child(head("What it can see", palette))
            .child(quiet(
                "Per account: what it reads, and what it never does there.",
                palette,
            ));
        if let Some(error) = &ready.view.accounts_error {
            see = see.child(ui::settings_notice(
                "Some accounts could not be read",
                error.clone(),
                palette,
            ));
        }
        if ready.rows.is_empty() && ready.view.accounts_error.is_none() {
            see = see.child(ui::empty(
                "No accounts are connected",
                "Nexus reads nothing outside Nexus until you connect an account.",
                palette,
            ));
        } else {
            let label = |text: &'static str| div().text_size(px(12.)).text_color(palette.faint).child(text);
            see = see
                .child(
                    columns(label("Account"), label("Reads"), label("Never"), palette)
                        .mt(px(8.))
                        .border_t_0(),
                )
                .children(ready.rows.iter().map(|row| account_row(row, palette)));
        }
        see = see.child(quiet(words::NEVER_TRAINS, palette).pt(px(10.)));
        page = page.child(see);
        if let Some(error) = &ready.view.links_error {
            page = page.child(ui::settings_notice(
                "Your agent links could not be read",
                error.clone(),
                palette,
            ));
        }

        let dialog = self.asking.map(|asking| {
            let (title, body, action, id) = match asking {
                Asking::Pause => (
                    words::PAUSE_TITLE,
                    words::PAUSE_BODY.to_string(),
                    "Pause",
                    "privacy-dialog-pause",
                ),
                Asking::Revoke => (
                    words::REVOKE_TITLE,
                    ready.revoke_text.clone(),
                    "Revoke all",
                    "privacy-dialog-revoke",
                ),
            };
            let confirm = danger_pill((id, 1usize), action, palette).on_click(cx.listener(
                move |page, _, window, cx| match asking {
                    Asking::Pause => page.set_paused(true, window, cx),
                    Asking::Revoke => page.revoke(window, cx),
                },
            ));
            modal(
                "privacy-scrim",
                sheet(id, &self.focus, 420., palette)
                    .on_key_down(cx.listener(|page, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            page.cancel(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(div().text_size(px(16.)).text_color(palette.strong).child(title))
                    .child(div().text_size(px(13.)).text_color(palette.muted).child(body))
                    .child(
                        h_flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                pill((id, 0usize), "Cancel", palette)
                                    .on_click(cx.listener(|page, _, _, cx| page.cancel(cx))),
                            )
                            .child(confirm),
                    ),
                palette,
            )
        });
        div().relative().size_full().child(outer.child(page)).children(dialog)
    }
}
