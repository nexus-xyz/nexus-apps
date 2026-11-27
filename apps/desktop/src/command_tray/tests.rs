use super::*;
use core::prelude::v1::test;
#[test]
fn tray_budget_tracks_pane_composer_and_competing_groups() {
    assert_eq!(budget(900., 104., 1), 300.);
    assert_eq!(budget(480., 104., 1), 168.);
    assert_eq!(budget(480., 240., 1), 92.);
    assert_eq!(budget(900., 104., 2) * 2. + 8., 300.);
    assert_eq!(budget(480., 104., 0), 0.);
}
struct Tray {
    limit: f32,
    width: f32,
    collapsed: bool,
}
impl Render for Tray {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::palette::Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
        let header = div()
            .debug_selector(|| "tray-header".into())
            .h(px(28.))
            .child("100 commands need permission");
        let body = (!self.collapsed).then(|| {
            gpui_kit::component::v_flex()
                .id("tray-content")
                .min_h(px(0.))
                .max_h(px((self.limit - 112.).max(0.)))
                .overflow_y_scroll()
                .children((0..100).map(|_| div().flex_none().h(px(200.)).child("long command")))
                .into_any_element()
        });
        let footer = (!self.collapsed).then(|| {
            div()
                .debug_selector(|| "tray-footer".into())
                .flex_none()
                .h(px(52.))
                .child("Run · Edit · Decline")
                .into_any_element()
        });
        div().w(px(self.width)).child(frame(
            "test-command-tray",
            self.limit,
            header.into_any_element(),
            body,
            footer,
            palette,
        ))
    }
}

struct ReviewFixture {
    width: f32,
    limit: f32,
}
impl Render for ReviewFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::palette::Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
        let header = gpui_kit::component::h_flex()
            .w_full()
            .flex_wrap()
            .gap(px(8.))
            .child(div().flex_1().min_w(px(100.)).child("Read the current branch"))
            .child(
                gpui_kit::component::h_flex()
                    .gap(px(8.))
                    .child(crate::pages::ui::ghost_button("test-decline", "Decline", palette))
                    .child(crate::pages::ui::ghost_button("test-edit", "Edit", palette))
                    .child(crate::pages::ui::primary_button("test-run", "Run", palette)),
            );
        let body = div()
            .debug_selector(|| "long-review-content".into())
            .h(px(1200.))
            .child("A long command");
        div().w(px(self.width)).child(review_card(
            self.limit,
            header.into_any_element(),
            body.into_any_element(),
            palette,
        ))
    }
}

#[gpui_kit::test]
fn review_actions_stay_above_scrolling_command_text_at_small_sizes(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, cx) = cx.add_window_view(|_, _| ReviewFixture {
        width: 680.,
        limit: 240.,
    });
    for (width, limit) in [(680., 240.), (320., 180.), (480., 200.)] {
        view.update(cx, |view, cx| {
            view.width = width;
            view.limit = limit;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let card = cx.debug_bounds("command-review-card").unwrap();
        let header = cx.debug_bounds("command-review-header").unwrap();
        let body = cx.debug_bounds("command-review-text").unwrap();
        let text_before = cx.debug_bounds("long-review-content").unwrap();
        assert!(card.size.height <= px(limit));
        assert!(header.top() >= card.top() && header.bottom() <= card.bottom());
        assert!(body.top() >= header.bottom() && body.size.height > px(0.));
        assert!(body.bottom() <= card.bottom(), "command text must fit the visible card");
        cx.simulate_event(ScrollEvent {
            position: body.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(cx.debug_bounds("command-review-header").unwrap(), header);
        assert!(cx.debug_bounds("long-review-content").unwrap().top() < text_before.top());
    }
}

struct RequestListFixture {
    scroll: ScrollHandle,
}
impl Render for RequestListFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::palette::Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
        let expanded = review_card(
            160.,
            div()
                .h(px(32.))
                .child("Expanded request · Run · Edit · Decline")
                .into_any_element(),
            div().h(px(600.)).child("Command text").into_any_element(),
            palette,
        );
        let mut cards = vec![
            div()
                .id("expanded-first-request")
                .w_full()
                .flex_none()
                .child(expanded)
                .into_any_element(),
        ];
        for ix in 0usize..8 {
            cards.push(
                div()
                    .id(("collapsed-request", ix))
                    .debug_selector(move || format!("collapsed-request-{ix}").into())
                    .h(px(40.))
                    .flex_none()
                    .child("Collapsed request")
                    .into_any_element(),
            );
        }
        div().w(px(600.)).child(request_list(220., &self.scroll, cards))
    }
}

#[gpui_kit::test]
fn the_expanded_request_and_collapsed_requests_scroll_together(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (_, cx) = cx.add_window_view(|_, _| RequestListFixture {
        scroll: ScrollHandle::new(),
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let header = cx.debug_bounds("command-review-header").unwrap();
    let row = cx.debug_bounds("collapsed-request-0").unwrap();
    cx.simulate_event(ScrollEvent {
        position: header.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-60.))),
        ..Default::default()
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let moved_header = cx.debug_bounds("command-review-header").unwrap();
    let moved_row = cx.debug_bounds("collapsed-request-0").unwrap();
    assert!(moved_header.top() < header.top(), "expanded request must not be pinned");
    assert_eq!(moved_header.top() - header.top(), moved_row.top() - row.top());
}

struct BulkHeaderFixture {
    width: f32,
    limit: f32,
    scroll: ScrollHandle,
}
impl Render for BulkHeaderFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::palette::Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
        let actions = gpui_kit::component::h_flex()
            .min_h(px(32.))
            .gap(px(8.))
            .child(crate::pages::ui::ghost_button(
                "test-decline-all",
                "Decline all",
                palette,
            ))
            .child(crate::pages::ui::primary_button("test-run-all", "Run all", palette));
        let header = decision_header(
            div()
                .min_w(px(0.))
                .truncate()
                .debug_selector(|| "permission-summary".into())
                .child("100 commands need permission")
                .into_any_element(),
            actions.into_any_element(),
            crate::pages::ui::ghost_button("test-commands", "Commands", palette).into_any_element(),
        );
        let rows = (0..100)
            .map(|_| {
                div()
                    .flex_none()
                    .h(px(40.))
                    .child("Collapsed command · Decline · Run")
                    .into_any_element()
            })
            .collect();
        let body = request_list(self.limit - HEADER, &self.scroll, rows);
        div().w(px(self.width)).child(frame(
            "bulk-test-tray",
            self.limit,
            header.into_any_element(),
            Some(body.into_any_element()),
            None,
            palette,
        ))
    }
}
#[gpui_kit::test]
fn bulk_actions_remain_above_the_list_at_narrow_sizes(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, cx) = cx.add_window_view(|_, _| BulkHeaderFixture {
        width: 320.,
        limit: 180.,
        scroll: ScrollHandle::new(),
    });
    for (width, limit) in [(320., 180.), (680., 300.)] {
        view.update(cx, |view, cx| {
            view.width = width;
            view.limit = limit;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let tray = cx.debug_bounds("bulk-test-tray").unwrap();
        let actions = cx.debug_bounds("command-bulk-actions").unwrap();
        let list = cx.debug_bounds("inline-command-list").unwrap();
        let commands = cx.debug_bounds("command-pane-action").unwrap();
        let summary = cx.debug_bounds("permission-summary").unwrap();
        assert_eq!(actions.center().y, commands.center().y);
        assert!(actions.right() <= commands.left());
        assert!(summary.right() <= actions.left());
        assert!(tray.size.height <= px(limit));
        assert!(actions.top() >= tray.top() && actions.bottom() <= list.top());
        assert!(actions.right() <= tray.right() && actions.left() >= tray.left());
        cx.simulate_event(ScrollEvent {
            position: list.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(cx.debug_bounds("command-bulk-actions").unwrap(), actions);
        view.read_with(cx, |view, _| assert!(view.scroll.offset().y < px(0.)));
    }
}

struct ScrollFixture {
    outer: ScrollHandle,
    inner: ScrollHandle,
}
impl Render for ScrollFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::palette::Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
        let body = gpui_kit::component::v_flex()
            .id("scroll-body")
            .debug_selector(|| "scroll-body".into())
            .h(px(120.))
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.inner)
            .child(div().flex_none().h(px(1000.)).child("Commands"));
        div()
            .id("transcript-scroll")
            .w(px(600.))
            .h(px(400.))
            .overflow_y_scroll()
            .track_scroll(&self.outer)
            .child(frame(
                "scroll-tray",
                220.,
                div().h(px(28.)).into_any_element(),
                Some(body.into_any_element()),
                None,
                palette,
            ))
            .child(div().h(px(1000.)).flex_none().child("Messages"))
    }
}

#[gpui_kit::test]
fn command_scroll_never_moves_the_transcript_even_at_boundaries(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, cx) = cx.add_window_view(|_, _| ScrollFixture {
        outer: ScrollHandle::new(),
        inner: ScrollHandle::new(),
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let bounds = cx.debug_bounds("scroll-body").unwrap();
    for delta in [-40., -10000., -40., 10000., 40.] {
        cx.simulate_event(ScrollEvent {
            position: bounds.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        view.read_with(cx, |view, _| assert_eq!(view.outer.offset().y, px(0.)));
    }
    cx.simulate_event(ScrollEvent {
        position: bounds.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
        ..Default::default()
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    view.read_with(cx, |view, _| {
        assert!(view.inner.offset().y < px(0.), "commands must still scroll")
    });
}

#[gpui_kit::test]
fn long_command_lists_keep_header_and_decision_footer_inside_the_bound(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (tray, cx) = cx.add_window_view(|_, _| Tray {
        limit: 300.,
        width: 680.,
        collapsed: false,
    });
    for (limit, width) in [(300., 680.), (180., 320.), (240., 480.)] {
        tray.update(cx, |tray, cx| {
            tray.limit = limit;
            tray.width = width;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let bounds = cx.debug_bounds("test-command-tray").unwrap();
        let header = cx.debug_bounds("tray-header").unwrap();
        let footer = cx.debug_bounds("tray-footer").unwrap();
        assert!(bounds.size.height <= px(limit), "{bounds:?}");
        assert!(header.top() >= bounds.top());
        assert!(
            footer.bottom() <= bounds.bottom(),
            "footer {footer:?} must fit {bounds:?}"
        );
        assert!(footer.top() >= header.bottom());
    }
    tray.update(cx, |tray, cx| {
        tray.collapsed = true;
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("test-command-tray").unwrap().size.height <= px(HEADER));
    assert!(cx.debug_bounds("tray-header").is_some());
    assert!(cx.debug_bounds("tray-footer").is_none());
}
