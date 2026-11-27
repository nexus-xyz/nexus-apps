use super::*;
use core::prelude::v1::test;
use nexus_pull_requests::{pull_request, summary::Summary, watch::PullWatch};

#[test]
fn says_when_the_watch_last_read_the_pull_request() {
    let now = 1_800_000_000_000;
    assert_eq!(checked(now - 10_000, now), "Checked just now");
    assert_eq!(checked(now - 5 * 60_000, now), "Checked 5m ago");
    assert_eq!(checked(now - 3 * 3_600_000, now), "Checked 3h ago");
    assert!(checked(now - 30 * 86_400_000, now).starts_with("Checked on "));
}

struct Preview {
    watch: PullWatch,
    summary: Summary,
    palette: Palette,
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // A sidebar row truncates its text. The card must explicitly restore wrapping.
        div().whitespace_nowrap().child(super::super::session_peek::frame(
            v_flex()
                .w_full()
                .p(px(12.))
                .children(body(&self.watch, &self.summary, self.palette)),
            true,
            self.palette,
            window,
        ))
    }
}

#[gpui_kit::test]
fn pull_previews_wrap_inside_the_card_and_scroll_without_clipping(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let reviews: Vec<_> = (0..8).map(|ix| serde_json::json!({
        "id": format!("review-{ix}"), "author": { "login": format!("reviewer-{ix}") },
        "state": "APPROVED",
        "body": "**Verdict: APPROVE.** This adds `GET /v1/invites` and [audited routes](https://example.com/routes). The preview must wrap onto several readable lines inside the card instead of clipping at its right edge."
    })).collect();
    let pull = pull_request::parse(serde_json::json!({
        "number": 42, "title": "feat(api): create, list and revoke invite codes over HTTP, with an audit trail",
        "url": "https://github.com/example/repo/pull/42", "state": "MERGED", "isDraft": false,
        "mergeable": "MERGEABLE", "latestReviews": reviews
    }))
    .unwrap();
    let summary = Summary::of(&pull);
    let (_, cx) = cx.add_window_view(|_, _| Preview {
        watch: PullWatch {
            status: Some(pull),
            ..PullWatch::default()
        },
        summary,
        palette: Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark),
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let card = cx.debug_bounds("session-peek-card").unwrap();
    assert_eq!(card.size.width, px(384.));
    assert_eq!(card.size.height, px(560.));
    for selector in ["session-peek-pull-title", "session-peek-review-0"] {
        let text = cx.debug_bounds(selector).expect(selector);
        assert!(text.size.height > px(18.), "{selector} wraps: {text:?}");
        assert!(
            text.left() >= card.left() && text.right() <= card.right(),
            "{selector} stays inside the card"
        );
    }
    let before = cx.debug_bounds("session-peek-review-0").unwrap();
    cx.simulate_mouse_move(card.center(), None, Modifiers::none());
    cx.simulate_event(ScrollEvent {
        position: card.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-180.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        source: ScrollSource::Pointer,
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("session-peek-review-0")
            .is_none_or(|after| after.top() < before.top())
    );
}
