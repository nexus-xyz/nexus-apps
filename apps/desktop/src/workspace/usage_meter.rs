//! The usage popover over the composer's usage gauge (`render_usage_donut` in `new_code.rs`). A click on the gauge
//! opens it in place, instead of opening Settings. It shows each limit of the session's plan (the 5-hour, the weekly
//! and each model's week): how much is left, a bar and when it resets. A Nexus-paid session shows the person's own
//! budget, as the sidebar's card does (`sidebar_budget.rs`). Then the session's cost and context, when one is open.
//!
//! It opens and closes with the design system's menu motion (`motion::presence` and `Phase::menu`): it fades in as it
//! rises from the gauge over `MENU_IN`, and fades back toward it over `MENU_OUT`.

use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::allowance::{self, Allowance, LOW_ALLOWANCE, Source};
use nexus_workbench::plans::{Limit, PlanUsage};

use super::code_dock::menu_box;
use super::code_title::code_meter::{compact, share, usd};
use super::new_code::PlanRead;
use super::sidebar_budget;
use super::{View, Workspace};
use crate::palette::Palette;

/// The popover's width: a limit's reset line fits on one row.
const WIDTH: f32 = 300.;

/// How far the popover travels as it opens: up, rising from a gauge under it; down, dropping from one over it.
const RISE: f32 = 4.;

impl Workspace {
    pub(super) fn toggle_usage_meter(&mut self, cx: &mut Context<Self>) {
        self.toggle_overlay(Overlay::UsageMeter);
        if self.overlay.usage_meter() {
            // A read older than a minute is read again; the popover shows the new figures when they come.
            self.probe_plans(cx);
        }
        cx.notify();
    }

    /// The popover, while open or leaving; anchored to the gauge's frame, which must be `relative`.
    pub(super) fn render_usage_meter(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let phase = crate::motion::presence("usage-meter", self.overlay.usage_meter(), cx)?;
        let (source, read, _) = self.composer_plan()?;
        let now = crate::code::now_ms();
        let mut card = menu_box("usage-meter", WIDTH, palette)
            .p(px(12.))
            .gap(px(12.))
            .text_size(px(13.))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                if this.overlay.usage_meter() {
                    this.overlay.close_usage_meter();
                    cx.notify();
                }
            }));
        card = match source {
            Source::Nexus => card.child(self.nexus_budget(palette)),
            Source::Plan(_) => card.child(plan_body(source, read, now, palette)),
        };
        card = card.children(self.session_facts(source, palette));
        let page = if source == Source::Nexus {
            (crate::pages::PageId::Usage, "Usage in Settings")
        } else {
            (crate::pages::PageId::Subscriptions, "Plans in Settings")
        };
        card = card.child(
            h_flex().justify_end().child(
                div()
                    .id("usage-meter-settings")
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.foreground))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.overlay.close_usage_meter();
                        this.open_native(page.0, window, cx);
                    }))
                    .child(page.1),
            ),
        );
        // Under Home's large composer it drops from the gauge; over a session's it rises from it (as `stash.rs`).
        let below = matches!(self.view, View::NewCode);
        let (anchor, offset, travel) = if below {
            (Anchor::TopRight, point(px(0.), px(34.)), -RISE)
        } else {
            (Anchor::BottomRight, point(px(0.), px(-8.)), RISE)
        };
        let card = phase.menu("usage-meter", travel, card);
        Some(
            div()
                .absolute()
                .top_0()
                .right_0()
                .child(
                    deferred(
                        anchored()
                            .anchor(anchor)
                            .offset(offset)
                            .snap_to_window_with_margin(px(12.))
                            .child(card),
                    )
                    .with_priority(1),
                )
                .into_any_element(),
        )
    }

    /// A Nexus-paid session: the person's own budget, never the organization's.
    fn nexus_budget(&self, palette: Palette) -> AnyElement {
        let head = header("Nexus Gateway", None, None, palette);
        let card = self
            .spend
            .as_ref()
            .and_then(|spend| sidebar_budget::card(spend, self.own_budget.as_ref()));
        let Some(card) = card else {
            return v_flex()
                .gap(px(8.))
                .child(head)
                .child(quiet("Reading your budget…", palette))
                .into_any_element();
        };
        let words = sidebar_budget::words(&card, chrono::Utc::now());
        v_flex()
            .gap(px(8.))
            .child(head)
            .child(
                h_flex()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(palette.foreground)
                            .child(words.figure),
                    )
                    .child(words.rest),
            )
            .children(words.left_share.map(|left| bar(left, false, palette)))
            .child(quiet(words.caption, palette))
            .child(quiet("Provider plan allowances are separate.", palette))
            .into_any_element()
    }

    /// The open session's cost and how full its context is.
    fn session_facts(&self, source: Source, palette: Palette) -> Option<AnyElement> {
        let session = self.code_session()?;
        let cost = if source == Source::Nexus {
            usd(session.cost_usd)
        } else {
            format!("{} est.", usd(session.cost_usd))
        };
        let context = match (session.context, share(session.context)) {
            (Some((tokens, size)), Some((_, percent))) => {
                Some(format!("{} of {} ({percent}%)", compact(tokens), compact(size)))
            }
            _ => None,
        };
        Some(
            v_flex()
                .gap(px(6.))
                .pt(px(12.))
                .border_t_1()
                .border_color(palette.border)
                .text_size(px(12.))
                .child(fact("This session", cost, palette))
                .children(context.map(|context| fact("Context", context, palette)))
                .into_any_element(),
        )
    }
}

/// A provider plan: its name and account, then each limit; or why it cannot be read.
fn plan_body(source: Source, read: PlanRead<'_>, now: i64, palette: Palette) -> AnyElement {
    let name = source.name();
    let title = format!("{name} plan");
    let plan = match read {
        PlanRead::Checking => return note(title, format!("Checking your {name} plan usage…"), palette),
        PlanRead::Failed(error) => {
            return note(
                title,
                format!("Could not read your {name} plan usage: {error}"),
                palette,
            );
        }
        PlanRead::Read(None) => {
            return note(
                title,
                format!("No signed-in {name} plan for this session's account."),
                palette,
            );
        }
        PlanRead::Read(Some(plan)) => plan,
    };
    // A Google Cloud sign-in is no Google AI plan: its badge names the project.
    let title = if plan.google_cloud {
        "Antigravity".to_string()
    } else {
        title
    };
    let head = header(&title, plan.email.as_deref(), plan.plan.as_deref(), palette);
    if let Some(error) = &plan.error {
        return note_with(head, format!("Could not read your {name} plan usage: {error}"), palette);
    }
    let rows: Vec<AnyElement> = plan
        .limits
        .iter()
        .filter_map(|limit| limit_row(limit, name, now, palette))
        .collect();
    if rows.is_empty() {
        let note = if plan.google_cloud {
            nexus_workbench::plans::CLOUD_USAGE_NOTE.to_string()
        } else {
            format!("Your {name} plan reported no usage limits.")
        };
        return note_with(head, note, palette);
    }
    v_flex()
        .gap(px(12.))
        .child(head)
        .children(rows)
        .children(banked(plan, palette))
        .into_any_element()
}

/// One limit: its name and share left, a bar of what is left, and when it resets.
fn limit_row(limit: &Limit, name: &str, now: i64, palette: Palette) -> Option<AnyElement> {
    let left = allowance::left(limit)?;
    let low = left <= LOW_ALLOWANCE;
    let reset = if limit.resets_at.is_some_and(|at| at <= now) {
        format!("A {name} limit just reset. Reading it again…")
    } else {
        allowance::reset_text(&Allowance { limit, left }, now, &chrono::Local)
    };
    Some(
        v_flex()
            .gap(px(6.))
            .child(
                h_flex()
                    .justify_between()
                    .gap(px(12.))
                    .child(allowance::limit_label(&limit.label))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(if low { palette.warning } else { palette.muted })
                            .child(format!("{left}% left")),
                    ),
            )
            .child(bar(left as f32 / 100., low, palette))
            .child(quiet(reset, palette))
            .into_any_element(),
    )
}

/// The free limit resets banked on the plan (Codex).
fn banked(plan: &PlanUsage, palette: Palette) -> Option<AnyElement> {
    let resets = plan.resets.as_ref().filter(|resets| resets.available > 0)?;
    let count = match resets.available {
        1 => "1 free limit reset banked".to_string(),
        n => format!("{n} free limit resets banked"),
    };
    Some(quiet(count, palette).into_any_element())
}

fn header(title: &str, account: Option<&str>, plan: Option<&str>, palette: Palette) -> AnyElement {
    h_flex()
        .justify_between()
        .gap(px(8.))
        .child(
            v_flex()
                .min_w(px(0.))
                .child(div().font_weight(FontWeight::MEDIUM).child(title.to_string()))
                .children(account.map(|account| {
                    div()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(account.to_string())
                })),
        )
        .children(plan.map(|plan| {
            div()
                .flex_none()
                .px(px(6.))
                .rounded(px(4.))
                .border_1()
                .border_color(palette.border)
                .text_size(px(11.))
                .text_color(palette.muted)
                .child(plan.to_string())
        }))
        .into_any_element()
}

fn note(title: String, text: String, palette: Palette) -> AnyElement {
    note_with(header(&title, None, None, palette), text, palette)
}

fn note_with(head: AnyElement, text: String, palette: Palette) -> AnyElement {
    v_flex()
        .gap(px(8.))
        .child(head)
        .child(quiet(text, palette))
        .into_any_element()
}

fn quiet(text: impl Into<SharedString>, palette: Palette) -> Div {
    div().text_size(px(12.)).text_color(palette.faint).child(text.into())
}

fn fact(term: &'static str, value: String, palette: Palette) -> Div {
    h_flex()
        .justify_between()
        .gap(px(12.))
        .child(div().text_color(palette.muted).child(term))
        .child(div().text_color(palette.foreground).child(value))
}

/// A track with what is left filled; amber when low. At least a sliver, so an empty limit still reads as a bar.
fn bar(left: f32, low: bool, palette: Palette) -> Div {
    div().w_full().h(px(4.)).rounded(px(2.)).bg(palette.selected).child(
        div()
            .h_full()
            .rounded(px(2.))
            .bg(if low { palette.warning } else { palette.muted })
            .w(relative(left.clamp(0.02, 1.))),
    )
}
