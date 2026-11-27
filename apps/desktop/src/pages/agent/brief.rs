//! The morning brief at the top of the Assistant's thread (brand v5 A1·A, "Writes first"; Nexus UI's `agent-brief.tsx`).
//! It is the brief Slack's DM got that morning (`GET /agent/brief/today`): the greeting in one of Nexus's bubbles, then
//! up to three lines in a second with the tail, the errands that need the person under a hairline beside a filled dot,
//! the clock under it, and "Why am I seeing this?" (A10·b, `assistant_views/proactivity.rs`).
//!
//! The read is quiet: while the agent API has the brief off, with none today, or on any failure, the thread shows what
//! it showed before. The page reads it when it opens, and again when the person comes back after an hour away.

use std::time::{Duration, Instant};

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{AgentPage, bubble, from_nexus, quiet_ink};
use crate::agent::Call;
use crate::pages::assistant_views::{self, WhySheet};
use crate::pages::ui;
use crate::palette::Palette;
use nexus_desktop_core::agent_brief::{self as words, Brief};

/// The brief is read again when the person comes back to the page after this long.
const READ_AGAIN: Duration = Duration::from_secs(60 * 60);
/// The brief's avatar keeps its own turn, apart from the thread's entries.
const AVATAR_ID: usize = 1_000_000;

/// The brief on show, with its "Why am I seeing this?".
pub struct Shown {
    pub brief: Brief,
    why: Option<Entity<WhySheet>>,
}

/// The brief's read: what shows, the call in flight, and when it was last asked.
#[derive(Default)]
pub struct BriefState {
    pub shown: Option<Shown>,
    read_at: Option<Instant>,
    _read: Option<Task<()>>,
}

impl AgentPage {
    /// Ask for today's brief. Its answer replaces the one shown: off, none today or a failure shows none, as the web.
    pub(super) fn read_brief(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.brief.read_at = Some(Instant::now());
        self.brief._read = Some(assistant_views::call(
            &self.ctx,
            Call::Brief,
            window,
            cx,
            |page, reply, _, cx| {
                let Some(brief) = words::brief_to_show(reply.status, reply.body.as_ref()) else {
                    page.brief.shown = None;
                    return;
                };
                if page.brief.shown.as_ref().is_some_and(|shown| shown.brief == brief) {
                    return;
                }
                let why = brief
                    .why
                    .clone()
                    .map(|why| assistant_views::why_sheet(page.ctx.clone(), why, cx));
                page.brief.shown = Some(Shown { brief, why });
            },
        ));
    }

    /// The person came back: read the brief again once it is an hour old, so a new morning shows its own.
    pub fn brief_on_return(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.brief.read_at.is_some_and(|at| at.elapsed() >= READ_AGAIN) {
            self.read_brief(window, cx);
        }
    }

    /// Read the brief now, as an hour away would.
    #[cfg(test)]
    pub fn read_brief_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read_brief(window, cx);
    }

    /// Today's brief, as the test reads it.
    #[cfg(test)]
    pub fn brief(&self) -> Option<&Brief> {
        self.brief.shown.as_ref().map(|shown| &shown.brief)
    }

    /// Its "Why am I seeing this?", as the test reads it.
    #[cfg(test)]
    pub fn brief_why(&self) -> Option<Entity<WhySheet>> {
        self.brief.shown.as_ref().and_then(|shown| shown.why.clone())
    }
}

/// An ISO 8601 time on this computer's clock: `07:58`.
fn clock(at: &str) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    Some(at.with_timezone(&chrono::Local).format("%H:%M").to_string())
}

/// The brief's row (A1·A): Nexus's avatar beside the greeting's bubble and the body's, the clock, and the Why.
pub(super) fn row(shown: &Shown, palette: Palette) -> Div {
    let brief = &shown.brief;
    let has_body = !brief.lines.is_empty() || brief.errands.is_some();
    let head = bubble(palette, !has_body).child(brief.head.clone());
    let body = has_body.then(|| {
        let errands = brief.errands.clone().map(|line| {
            h_flex()
                .gap(px(10.))
                .when(!brief.lines.is_empty(), |line| {
                    line.mt(px(8.)).pt(px(6.)).border_t_1().border_color(palette.border)
                })
                .text_color(palette.muted)
                .child(div().flex_none().size(px(7.)).rounded_full().bg(palette.foreground))
                .child(line)
        });
        bubble(palette, true)
            .child(v_flex().gap(px(6.)).children(brief.lines.iter().cloned()))
            .children(errands)
    });
    let time = clock(&brief.at).map(|at| {
        div()
            .pl(px(4.))
            .font_family(ui::MONO)
            .text_size(px(11.))
            .text_color(quiet_ink(palette))
            .child(at)
    });
    let column = v_flex()
        .flex_1()
        .min_w(px(0.))
        .items_start()
        .gap(px(4.))
        .child(head)
        .children(body)
        .children(time)
        .children(shown.why.clone());
    from_nexus(AVATAR_ID, false, column, palette)
}
