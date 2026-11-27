//! A Code session's pull requests in the top bar, as the Nexus app has them (`workbench/pull-chip.tsx`): a chip with
//! the one that most needs the person (its state, number, what it needs, and a dot for each of the others), and a card
//! under it with its checks, review and merge state, Push changes, and "Checks, comments and merge". A session may open
//! several pull requests, from more than one branch, and some may sit on others in a stack: the card lists them as
//! stacks (`pull_stack.rs`), shows the one picked, and says what must merge first.
//!
//! The watch (`pr_watch.rs`) finds each one however it was opened (from the form, or by the agent with
//! `gh pr create`), so the chip replaces "Open PR" as soon as one exists. "Open PR" shows only once the watch has
//! looked and found none. The words are `nexus_pull_requests::chip`'s, shared with the Nexus app.

use crate::workspace::overlay::{Overlay, OverlaySlot};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_pull_requests::badge::Badge;
use nexus_pull_requests::chip;
use nexus_pull_requests::pull_request::PullRequestStatus;
use nexus_pull_requests::stack;
use nexus_workbench::source_host::Host;

use super::TopPanel;
use super::git_panel::{popover, submit_button};
use crate::code::CodeSession;
use crate::palette::Palette;
use crate::workspace::code_dock::{menu_box, menu_item_hover};
use crate::workspace::pull_stack::{Density, Pick, pull_list, pull_strip, stack_callout, word_color};
use crate::workspace::session_menu::{pull_color, pull_icon};
use crate::workspace::{MenuTrigger, Workspace, glyph};

/// How many failed checks the card names; the page lists the rest.
const FAILED_SHOWN: usize = 3;
/// The card's height cap when it lists several pull requests. The card does not scroll then, so the cap fits the
/// tallest details (a three-line title, a stack note and three failed checks) under a short list.
const CARD_WITH_LIST_MAX_H: f32 = 560.;

/// The card under the chip, for one session: the pull request picked from its list, else the chip's.
pub(in crate::workspace) struct PullsCard {
    pub(super) id: String,
    picked: Option<String>,
}

/// A quiet button beside the card's main one.
fn second_button(id: &'static str, label: &'static str, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.))
        .px(px(12.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .bg(palette.selected)
        .text_size(px(13.))
        .cursor_pointer()
        .hover(|style| menu_item_hover(style, palette))
        .child(label)
}

impl Workspace {
    fn card_open(&self, id: &str) -> bool {
        matches!(self.overlay.code_top(), Some(TopPanel::Pulls(card)) if card.id == id)
    }

    fn toggle_pulls_card(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.card_open(id) {
            self.overlay.close_code_top();
        } else {
            self.overlay.close_session_menu();
            self.open_overlay(Overlay::CodeTop(TopPanel::Pulls(PullsCard {
                id: id.to_string(),
                picked: None,
            })));
        }
        cx.notify();
    }

    /// The top bar's pull request control: the chip and its card, a link while the watch has not read it, a quiet
    /// place while it first reads, or "Open PR". `word` is the project's `pull request` or `merge request`.
    pub(super) fn render_pull_chip(
        &self,
        session: &CodeSession,
        word: &'static str,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // This computer watches its own sessions and its SSH servers'; a cloud session has only its link.
        let watched = self.watches_pull(session);
        let watch = watched.then(|| crate::pr_watch::known(&session.id)).flatten();
        let shown = watch.as_ref().and_then(|watch| watch.status.clone());
        if let (false, Some(shown)) = (session.review, shown) {
            let pulls = watch.as_ref().map(chip::session_pulls).unwrap_or_default();
            return Some(self.render_chip(session, shown, pulls, palette, cx));
        }
        if let Some(url) = session.pull_request.clone() {
            return Some(self.render_pull_link(session, url, palette, cx));
        }
        if session.branch.is_none() || session.review {
            return None;
        }
        if watched && self.pull_pending(&session.id) {
            // Still looking: a quiet place of the chip's height, so the bar moves once.
            return Some(
                h_flex()
                    .id("code-pull-pending")
                    .flex_none()
                    .h(px(26.))
                    .px(px(8.))
                    .gap(px(6.))
                    .rounded(px(8.))
                    .bg(palette.selected)
                    .child(glyph(Lucide::GitPullRequest, palette.faint).size(px(13.)))
                    .child(div().w(px(28.)).h(px(8.)).rounded(px(4.)).bg(palette.hover))
                    .tooltip(move |window, cx| {
                        Tooltip::new(format!("Looking for this branch's {word}")).build(window, cx)
                    })
                    .into_any_element(),
            );
        }
        let id = session.id.clone();
        let short = if word == "merge request" { "Open MR" } else { "Open PR" };
        let open = matches!(&self.pr_form, Some(form) if form.id == id);
        let tip: SharedString = format!(
            "Commit, push {} and open a {word}",
            session.branch.as_deref().unwrap_or("the branch")
        )
        .into();
        Some(
            h_flex()
                .id("code-open-pull-request")
                .flex_none()
                .h(px(26.))
                .px(px(10.))
                .gap(px(6.))
                .rounded(px(8.))
                .bg(if open { palette.hover } else { palette.selected })
                .text_size(px(12.))
                .text_color(palette.foreground)
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .child(glyph(Lucide::GitPullRequest, palette.icon).size(px(13.)))
                .child(short)
                .when(!open, |button| {
                    button.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                })
                .menu_trigger(
                    open,
                    cx,
                    {
                        let id = id.clone();
                        move |this| matches!(&this.pr_form, Some(form) if form.id == id)
                    },
                    move |this, window, cx| {
                        if matches!(&this.pr_form, Some(form) if form.id == id) {
                            this.pr_form = None;
                            cx.notify();
                        } else {
                            this.open_pr_form(&id, window, cx);
                        }
                    },
                )
                .into_any_element(),
        )
    }

    /// A link to the saved pull request before the watch read it, and a review's: its page here when it has one.
    fn render_pull_link(
        &self,
        session: &CodeSession,
        url: String,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let number = chip::number_of_url(&url);
        let host = if url.contains("/-/merge_requests/") {
            Host::GitLab
        } else {
            Host::GitHub
        };
        let tip: SharedString = if session.review {
            format!("Reviewing {url}").into()
        } else {
            url.clone().into()
        };
        let id = session.id.clone();
        h_flex()
            .id("code-pull-request")
            .flex_none()
            .h(px(26.))
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(8.))
            .bg(palette.selected)
            .text_size(px(12.))
            .text_color(palette.foreground)
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .child(glyph(Lucide::GitPullRequest, palette.icon).size(px(13.)))
            .child(number.map_or_else(|| host.request_title().to_string(), |number| host.reference(number)))
            .child(glyph(Lucide::ArrowUpRight, palette.muted).size(px(12.)))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                if crate::pull_page::has_page(&url) {
                    this.open_pull(&id, window, cx);
                } else {
                    crate::external_link::open(&url, cx);
                }
            }))
            .into_any_element()
    }

    fn render_chip(
        &self,
        session: &CodeSession,
        shown: PullRequestStatus,
        pulls: Vec<PullRequestStatus>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = session.id.clone();
        let open = self.card_open(&id);
        let badge = Badge::of(&shown);
        let (word, tone) = chip::word(&shown);
        let others = pulls.iter().filter(|pull| pull.url != shown.url).count();
        let strip = pull_strip(&pulls, &shown.url, palette);
        let tip: SharedString = match stack::count_line(&pulls) {
            Some(count) => format!("{} ({count})", shown.title).into(),
            None => shown.title.clone().into(),
        };
        let chip = h_flex()
            .id("code-pull-chip")
            .flex_none()
            .h(px(26.))
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(8.))
            .bg(if open { palette.hover } else { palette.selected })
            .text_size(px(12.))
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .child(glyph(pull_icon(badge), pull_color(badge, palette)).size(px(13.)))
            .child(
                div()
                    .text_color(palette.foreground)
                    .child(shown.host.reference(shown.number)),
            )
            .child(
                div()
                    .max_w(px(160.))
                    .truncate()
                    .text_color(word_color(tone, palette))
                    .child(word),
            )
            .when(others > 0, |chip| {
                let more = div()
                    .h(px(14.))
                    .pl(px(6.))
                    .flex()
                    .items_center()
                    .border_l_1()
                    .border_color(palette.border)
                    .text_color(palette.muted);
                chip.child(match strip {
                    Some(strip) => more.child(strip),
                    None => more.child(format!("+{others}")),
                })
            })
            .when(!open, |chip| {
                chip.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            })
            .menu_trigger(
                open,
                cx,
                {
                    let id = id.clone();
                    move |this| this.card_open(&id)
                },
                {
                    let id = id.clone();
                    move |this, _, cx| this.toggle_pulls_card(&id, cx)
                },
            );
        let card = match self.overlay.code_top() {
            Some(TopPanel::Pulls(card)) if card.id == id => {
                let picked = card.picked.as_deref();
                let pull = pulls
                    .iter()
                    .find(|pull| Some(pull.url.as_str()) == picked)
                    .cloned()
                    .unwrap_or_else(|| shown.clone());
                Some(self.render_pulls_card(session, &shown, &pull, &pulls, palette, cx))
            }
            _ => None,
        };
        div().relative().child(chip).children(card).into_any_element()
    }

    fn render_pulls_card(
        &self,
        session: &CodeSession,
        shown: &PullRequestStatus,
        pull: &PullRequestStatus,
        pulls: &[PullRequestStatus],
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = session.id.clone();
        let host = pull.host;
        let badge = Badge::of(pull);
        let color = pull_color(badge, palette);
        let reference = host.reference(pull.number);
        let tally = chip::tally(pull);
        let review = chip::review_line(pull);
        let asked = review.starts_with("Changes");
        let warn = pull_color(Badge::Changes, palette);
        let fact = |label: &'static str, value: String, tint: Option<Hsla>| {
            h_flex()
                .gap(px(12.))
                .child(div().w(px(56.)).flex_none().text_color(palette.muted).child(label))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(tint.unwrap_or(palette.foreground))
                        .child(value),
                )
        };
        let rule = || div().h(px(1.)).bg(palette.border);
        let mut body = menu_box("pulls-card", 384., palette)
            .p(px(14.))
            .gap(px(12.))
            .text_size(px(13.))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                if matches!(this.overlay.code_top(), Some(TopPanel::Pulls(_))) {
                    this.overlay.close_code_top();
                    cx.notify();
                }
            }));
        // Several: as stacks, top first, the picked one on a fill.
        if pulls.len() > 1 {
            let this = cx.entity().downgrade();
            let card_id = id.clone();
            let pick: Pick = std::rc::Rc::new(move |url, _, cx| {
                this.update(cx, |this, cx| {
                    if let Some(TopPanel::Pulls(card)) = this.overlay.code_top_mut()
                        && card.id == card_id
                    {
                        card.picked = Some(url);
                        cx.notify();
                    }
                })
                .ok();
            });
            // The list is the card's one scroller: the card stops scrolling, and the list gives up height
            // first. Two nested scrollers both take a wheel over the list (GPUI never stops it at the inner one).
            let list = div()
                .id("pulls-card-list")
                .max_h(px(248.))
                .min_h(px(0.))
                .overflow_y_scroll()
                .mx(px(-6.))
                .mt(px(-6.))
                .child(pull_list(pulls, Some(&pull.url), Density::Card, palette, Some(pick)));
            body = body
                .max_h(px(CARD_WITH_LIST_MAX_H))
                .overflow_y_hidden()
                .child(list)
                .child(rule().flex_none());
        }
        // The rest never shrinks, so the list alone makes room.
        let mut details = v_flex().flex_none().gap(px(12.));
        // Its state, number and a link out.
        let out = pull.url.clone();
        details = details
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .h(px(22.))
                            .pl(px(6.))
                            .pr(px(8.))
                            .gap(px(5.))
                            .rounded_full()
                            .bg(color.opacity(0.14))
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(color)
                            .child(glyph(pull_icon(badge), color).size(px(13.)))
                            .child(if badge == Badge::Changes {
                                "Changes requested"
                            } else {
                                badge.label()
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.muted)
                            .child(reference.clone()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("pulls-card-out")
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|style| menu_item_hover(style, palette))
                            .child(glyph(Lucide::ArrowUpRight, palette.muted).size(px(14.)))
                            .tooltip(move |window, cx| {
                                Tooltip::new(format!("Open on {}", host.name())).build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.overlay.close_code_top();
                                crate::external_link::open(&out, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(relative(1.35))
                    .child(pull.title.clone()),
            )
            .child(rule());
        // Checks, review and merge, with the checks as one bar: failed, running, passed, skipped.
        let mut facts = v_flex()
            .gap(px(6.))
            .child(fact("Checks", chip::checks_line(tally), None));
        if !pull.checks.is_empty() {
            let mut bar = h_flex().ml(px(68.)).mb(px(4.)).h(px(4.)).gap(px(1.));
            let segments = [
                (tally.failed, pull_color(Badge::Closed, palette)),
                (tally.running, pull_color(Badge::Open, palette)),
                (tally.passed, pull_color(Badge::Approved, palette)),
                (tally.skipped, palette.border),
            ];
            for (count, hue) in segments {
                for _ in 0..count {
                    bar = bar.child(div().flex_1().h_full().min_w(px(2.)).rounded(px(1.)).bg(hue));
                }
            }
            facts = facts.child(bar);
        }
        facts = facts.child(fact("Review", review, asked.then_some(warn)));
        if let Some(merge) = chip::merge_line(pull) {
            let conflict = pull.mergeable == "CONFLICTING";
            facts = facts.child(fact("Merge", merge.to_string(), conflict.then_some(warn)));
        }
        if let Some(into) = stack::into_line(pull, pulls) {
            facts = facts.child(fact("Into", into, None));
        }
        details = details.child(facts);
        // A stacked one: what must merge first.
        if let Some(note) = stack::stack_note(pull, pulls) {
            details = details.child(stack_callout(note, palette));
        }
        // The failed checks, each to its run.
        let failed: Vec<_> = pull.checks.iter().filter(|check| check.state == "failure").collect();
        if !failed.is_empty() {
            let mut list = v_flex().gap(px(4.));
            for (ix, check) in failed.iter().take(FAILED_SHOWN).enumerate() {
                let row = h_flex()
                    .id(("pull-failed", ix))
                    .gap(px(8.))
                    .min_w(px(0.))
                    .child(div().size(px(6.)).flex_none().rounded_full().bg(palette.destructive))
                    .child(div().min_w(px(0.)).truncate().child(check.name.clone()));
                list = list.child(match check.url.clone() {
                    Some(url) => row
                        .cursor_pointer()
                        .hover(|style| style.text_color(palette.strong))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.overlay.close_code_top();
                            crate::external_link::open(&url, cx);
                            cx.notify();
                        })),
                    None => row,
                });
            }
            if failed.len() > FAILED_SHOWN {
                list = list.child(
                    div()
                        .pl(px(14.))
                        .text_color(palette.muted)
                        .child(format!("and {} more", failed.len() - FAILED_SHOWN)),
                );
            }
            details = details.child(list);
        }
        // Push changes to the branch, and the page with checks, comments and merge.
        let any_open = pulls.iter().any(|entry| entry.state == "OPEN");
        let pushes = any_open && session.branch.is_some() && !session.review && session.machine.is_none();
        let mut actions = h_flex().justify_end().gap(px(8.));
        if pushes {
            let push_id = id.clone();
            actions = actions.child(
                second_button("pulls-card-push", "Push changes", palette).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.overlay.close_code_top();
                        this.open_pr_form(&push_id, window, cx);
                    },
                )),
            );
        }
        let page_url = (pull.url != shown.url).then(|| pull.url.clone());
        let url = pull.url.clone();
        actions = actions.child(if crate::pull_page::has_page(&pull.url) {
            submit_button("pulls-card-page", "Checks, comments and merge", true, palette).on_click(cx.listener(
                move |this, _, window, cx| {
                    this.overlay.close_code_top();
                    this.open_pull_at(&id, page_url.clone(), window, cx);
                },
            ))
        } else {
            submit_button("pulls-card-page", format!("Open on {}", host.name()), true, palette).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.overlay.close_code_top();
                    crate::external_link::open(&url, cx);
                    cx.notify();
                },
            ))
        });
        details = details.child(actions);
        popover(body.child(details), palette)
    }
}
