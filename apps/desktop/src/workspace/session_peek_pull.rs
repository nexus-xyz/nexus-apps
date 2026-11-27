//! The hover card's pull request and formatted review previews.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::glyph;
use super::session_menu::{pull_color, pull_icon};
use crate::{code, palette::Palette};

const REVIEWS_SHOWN: usize = 6;

/// Cached status and review previews, followed by the session's other pull requests.
pub(super) fn body(
    watch: &nexus_pull_requests::watch::PullWatch,
    summary: &nexus_pull_requests::summary::Summary,
    palette: Palette,
) -> Option<AnyElement> {
    use nexus_pull_requests::summary::Tone;
    let pull = watch.status.as_ref()?;
    let rule = |section: Div| section.pt(px(8.)).border_t_1().border_color(palette.border);
    let reviews: Vec<_> = summary
        .reviews
        .iter()
        .take(REVIEWS_SHOWN)
        .enumerate()
        .map(|(ix, review)| {
            let dot = match review.tone {
                Tone::Changes => pull_color(nexus_pull_requests::badge::Badge::Changes, palette),
                Tone::Approved => pull_color(nexus_pull_requests::badge::Badge::Approved, palette),
                _ => palette.faint,
            };
            h_flex()
                .w_full()
                .min_w(px(0.))
                .items_start()
                .gap(px(8.))
                .child(div().mt(px(6.)).size(px(6.)).flex_none().rounded_full().bg(dot))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap(px(4.))
                        .child(
                            h_flex()
                                .min_w(px(0.))
                                .flex_wrap()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_color(palette.strong)
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(review.author.clone()),
                                )
                                .child(review.said.clone()),
                        )
                        .when(!review.excerpt.is_empty(), |text| {
                            text.child(
                                div()
                                    .w_full()
                                    .min_w(px(0.))
                                    .text_color(palette.muted)
                                    .line_height(px(18.))
                                    .line_clamp(4)
                                    .debug_selector(move || format!("session-peek-review-{ix}"))
                                    .child(review_excerpt(review, palette)),
                            )
                        }),
                )
        })
        .collect();
    let pulls = nexus_pull_requests::chip::session_pulls(watch);
    let all = (pulls.len() > 1).then(|| {
        crate::workspace::pull_stack::pull_list(
            &pulls,
            Some(&pull.url),
            crate::workspace::pull_stack::Density::Compact,
            palette,
            None,
        )
        .mx(px(-6.))
    });
    let read = match (&watch.problem, watch.read_at) {
        (Some(problem), _) => Some(format!("Last read failed: {problem}")),
        (None, Some(read_at)) => Some(checked(read_at, code::now_ms())),
        (None, None) => None,
    };
    let card = v_flex()
        .w_full()
        .min_w(px(0.))
        .gap(px(10.))
        .text_size(px(12.))
        .text_color(palette.muted)
        .child(
            h_flex()
                .justify_between()
                .gap(px(8.))
                .child(
                    h_flex()
                        .gap(px(4.))
                        .items_center()
                        .text_color(palette.strong)
                        .font_weight(FontWeight::MEDIUM)
                        .child(glyph(pull_icon(summary.badge), pull_color(summary.badge, palette)).size(px(12.)))
                        .child(summary.headline),
                )
                .child(
                    div()
                        .font_family("monospace")
                        .text_size(px(11.))
                        .text_color(palette.faint)
                        .child(summary.reference.clone()),
                ),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(palette.strong)
                .debug_selector(|| "session-peek-pull-title".into())
                .child(summary.title.clone()),
        )
        .when(!reviews.is_empty(), |card| {
            card.child(rule(v_flex().gap(px(12.))).children(reviews))
        })
        .when_some(summary.checks.as_ref(), |card, checks| {
            card.child(
                rule(v_flex().gap(px(2.)))
                    .child(format!("Checks: {}", checks.line))
                    .when(!checks.failed.is_empty(), |section| {
                        section.child(
                            div()
                                .text_color(palette.faint)
                                .truncate()
                                .child(format!("Failed: {}", checks.failed.join(", "))),
                        )
                    }),
            )
        })
        .children(
            summary
                .notes
                .iter()
                .map(|note| div().text_color(palette.strong).child(*note)),
        )
        .when_some(all, |card, all| card.child(rule(div()).child(all)))
        .when(summary.reviews.len() > REVIEWS_SHOWN, |card| {
            card.child(
                div()
                    .text_color(palette.faint)
                    .child(format!("+{} more reviews", summary.reviews.len() - REVIEWS_SHOWN)),
            )
        })
        .child(
            h_flex()
                .id("session-peek-open-pull")
                .gap(px(5.))
                .text_color(palette.strong)
                .cursor_pointer()
                .hover(|style| style.text_color(palette.primary))
                .child(if pull.host == nexus_workbench::source_host::Host::GitLab {
                    "Open merge request"
                } else {
                    "Open pull request"
                })
                .child(glyph(Lucide::ArrowUpRight, palette.faint).size(px(12.)))
                .on_click({
                    let url = pull.url.clone();
                    move |_, _, cx| {
                        cx.stop_propagation();
                        crate::external_link::open(&url, cx);
                    }
                }),
        )
        .when_some(read, |card, read| {
            card.child(div().text_size(px(11.)).text_color(palette.faint).child(read))
        })
        .into_any_element();
    Some(card)
}

fn review_excerpt(review: &nexus_pull_requests::summary::ReviewLine, palette: Palette) -> StyledText {
    let highlights = review.marks.iter().map(|mark| {
        (
            mark.range.clone(),
            HighlightStyle {
                font_weight: mark.style.strong.then_some(FontWeight::SEMIBOLD),
                font_style: mark.style.emphasis.then_some(FontStyle::Italic),
                color: (mark.style.code || mark.style.link).then_some(palette.strong),
                background_color: mark.style.code.then_some(palette.hover),
                strikethrough: mark.style.deleted.then_some(StrikethroughStyle {
                    thickness: px(1.),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
    });
    StyledText::new(review.excerpt.clone())
        .with_highlights(highlights)
        .with_font_family_overrides(
            review
                .marks
                .iter()
                .filter(|mark| mark.style.code)
                .map(|mark| (mark.range.clone(), SharedString::from("monospace"))),
        )
}

fn checked(read_at: i64, now: i64) -> String {
    let age = code::age(read_at, now);
    if age == "now" {
        "Checked just now".into()
    } else if age.ends_with(['m', 'h', 'd']) && age[..age.len() - 1].chars().all(|c| c.is_ascii_digit()) {
        format!("Checked {age} ago")
    } else {
        format!("Checked on {age}")
    }
}

#[cfg(test)]
#[path = "session_peek_pull_tests.rs"]
mod tests;
