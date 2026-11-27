//! Literal message selection and the shared selection context menu.

use std::ops::Range;

use gpui_kit::{
    App, BorderStyle, Bounds, Corners, CursorStyle, Edges, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior,
    Hsla, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point, SharedString, StyledText, Subscription,
    Window, transparent_black,
};

use crate::i18n::tr;
use gpui_kit::HighlightStyle;
use gpui_kit::base::{TextSelectionHandle, TextSelectionRegistration, TextSelectionRun};

/// Selectable literal message text with search highlights.
/// Adapted from GPUI Base 0.9's SelectableText, which does not expose range highlights. GPUI Base is `gpui-base`
/// 0.9.0 (<https://github.com/longbridge/gpui-kit>, `crates/base`), Apache-2.0, Copyright 2024 - 2026 Longbridge.
pub struct MessageText {
    id: ElementId,
    text: SharedString,
    styled_text: StyledText,
}

impl MessageText {
    /// Creates a run that owns its own selection.
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        Self {
            id: id.into(),
            styled_text: StyledText::new(text.clone()),
            text,
        }
    }

    pub fn with_highlights(mut self, highlights: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>) -> Self {
        self.styled_text = StyledText::new(self.text.clone()).with_highlights(highlights);
        self
    }

    fn paint_selection(layout: &gpui_kit::TextLayout, range: Range<usize>, color: Hsla, window: &mut Window) {
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return;
        };
        for bounds in selection_quad_bounds(start, end, layout.bounds(), layout.line_height()) {
            window.paint_quad(PaintQuad {
                bounds,
                background: color.into(),
                corner_radii: Corners::default(),
                border_widths: Edges::default(),
                border_color: transparent_black(),
                border_style: BorderStyle::default(),
            });
        }
    }
}

fn selection_quad_bounds(
    start: Point<Pixels>,
    end: Point<Pixels>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
) -> Vec<Bounds<Pixels>> {
    if start.y == end.y {
        return vec![Bounds::from_corners(start, Point::new(end.x, end.y + line_height))];
    }

    let mut quads = vec![Bounds::from_corners(
        start,
        Point::new(bounds.right(), start.y + line_height),
    )];
    if end.y > start.y + line_height {
        quads.push(Bounds::from_corners(
            Point::new(bounds.left(), start.y + line_height),
            Point::new(bounds.right(), end.y),
        ));
    }
    quads.push(Bounds::from_corners(
        Point::new(bounds.left(), end.y),
        Point::new(end.x, end.y + line_height),
    ));
    quads
}

impl IntoElement for MessageText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for MessageText {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let handle = window.with_element_state(
            global_id.expect("message text needs a stable id"),
            |retained: Option<(SharedString, TextSelectionHandle, Subscription)>, window| {
                // Keep selection across frames, but retire it when a virtualized slot changes text.
                let (_, handle, refresh) = retained.filter(|(text, _, _)| text == &self.text).unwrap_or_else(|| {
                    let handle = TextSelectionHandle::new(self.text.clone(), cx);
                    // Selection changes must repaint even while the pointer stays inside this text.
                    let refresh = handle.refresh_window_on_change(window, cx);
                    (self.text.clone(), handle, refresh)
                });
                (handle.clone(), (self.text.clone(), handle, refresh))
            },
        );
        let (layout_id, ()) = self.styled_text.request_layout(global_id, inspector_id, window, cx);
        (layout_id, handle)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled_text
            .prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(handle, window, cx);
        handle.register(registration, window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.set_cursor_style(CursorStyle::IBeam, hitbox);
        let layout = self.styled_text.layout().clone();
        let projection = handle.update_runs(&[TextSelectionRun::new(self.text.clone(), layout.clone(), bounds)], cx);
        let color = gpui_kit::component::Theme::global(cx).selection;
        for range in projection.ranges().iter().flatten().cloned() {
            Self::paint_selection(&layout, range, color, window);
        }
        self.styled_text
            .paint(global_id, inspector_id, bounds, &mut (), &mut (), window, cx);
    }
}

/// Capture before the menu takes focus; its click must copy exactly the original selection.
pub(super) fn selection_menu(
    menu: gpui_kit::component::menu::PopupMenu,
    window: &mut Window,
    cx: &mut gpui_kit::Context<gpui_kit::component::menu::PopupMenu>,
) -> gpui_kit::component::menu::PopupMenu {
    use gpui_kit::base::TextSelection;
    use gpui_kit::component::menu::PopupMenuItem;
    let text = TextSelection::selected_text(window, cx);
    // This menu opens on mouse-down; links open their menu on mouse-up.
    // An empty menu stays hidden, so it cannot overlap the link's menu.
    if text.is_empty() {
        return menu;
    }
    menu.item(PopupMenuItem::new(tr("Copy")).on_click(move |_, _, cx| {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.clone()));
    }))
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::{MessageText, selection_menu};
    use gpui_kit::base::{TextSelection, TextSelectionLayer};
    use gpui_kit::component::menu::ContextMenuExt;
    use gpui_kit::component::text::{TextView, TextViewState};
    use gpui_kit::{
        AppContext, Context, Entity, HighlightStyle, InteractiveElement, IntoElement, Modifiers, MouseButton,
        ParentElement, Render, Styled, TestAppContext, Window, div, point, px,
    };

    struct Probe {
        markdown: Option<Entity<TextViewState>>,
        renders: Rc<Cell<usize>>,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            let text = match &self.markdown {
                Some(state) => TextView::new(state).selectable(true).into_any_element(),
                None => MessageText::new("sent", "héllo **literal** <tag>\nsecond line")
                    .with_highlights([(
                        0..6,
                        HighlightStyle {
                            background_color: Some(gpui_kit::rgb(0xffff00).into()),
                            ..Default::default()
                        },
                    )])
                    .into_any_element(),
            };
            div().size_full().child(TextSelectionLayer).child(
                div()
                    .id("text-menu")
                    .w(px(240.))
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .child(text)
                    .context_menu(selection_menu),
            )
        }
    }

    fn select_and_copy(markdown: bool, word_only: bool, expected: &str, cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(move |_, cx| Probe {
            markdown: markdown.then(|| cx.new(|cx| TextViewState::markdown("**héllo** world", cx))),
            renders: Rc::default(),
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let start = point(px(0.), px(10.));
        let end = if markdown {
            point(px(220.), px(10.))
        } else {
            point(px(220.), px(32.))
        };
        if word_only {
            cx.simulate_event(gpui_kit::MouseDownEvent {
                position: point(px(12.), px(10.)),
                button: MouseButton::Left,
                click_count: 2,
                modifiers: Modifiers::none(),
                first_mouse: false,
            });
            cx.simulate_mouse_up(point(px(12.), px(10.)), MouseButton::Left, Modifiers::none());
        } else {
            cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::none());
            cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        }
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert_eq!(TextSelection::selected_text(window, cx), expected);
        });
        // The menu takes focus. Its item must retain the selected text until clicked.
        let press = point(px(20.), px(10.));
        cx.simulate_mouse_down(press, MouseButton::Right, Modifiers::none());
        cx.simulate_mouse_up(press, MouseButton::Right, Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let item = point(press.x + px(30.), press.y + px(17.));
        cx.simulate_mouse_move(item, None, Modifiers::none());
        cx.simulate_click(item, Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(expected.to_string())
            )
        });
    }

    #[gpui_kit::test]
    fn repaints_selection_before_mouse_release(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let renders = Rc::new(Cell::new(0));
        let counter = renders.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Probe {
            markdown: None,
            renders: counter,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_down(point(px(2.), px(10.)), MouseButton::Left, Modifiers::none());
        // Settle the press and hover so neither can cause the drag's repaint.
        cx.simulate_mouse_move(point(px(20.), px(10.)), Some(MouseButton::Left), Modifiers::none());
        let before = renders.get();
        cx.simulate_mouse_move(point(px(80.), px(10.)), Some(MouseButton::Left), Modifiers::none());
        assert!(renders.get() > before, "dragging must repaint before mouse-up");
        cx.update(|window, cx| assert!(!TextSelection::selected_text(window, cx).is_empty()));
        let before = renders.get();
        cx.simulate_mouse_move(point(px(30.), px(10.)), Some(MouseButton::Left), Modifiers::none());
        assert!(renders.get() > before, "reversing the drag must repaint");
        cx.simulate_mouse_up(point(px(30.), px(10.)), MouseButton::Left, Modifiers::none());
    }

    #[gpui_kit::test]
    fn copies_literal_multiline_user_text_with_find_highlights(cx: &mut TestAppContext) {
        select_and_copy(false, false, "héllo **literal** <tag>\nsecond line", cx);
    }

    #[gpui_kit::test]
    fn copies_only_the_selected_word(cx: &mut TestAppContext) {
        select_and_copy(false, true, "héllo", cx);
    }

    #[gpui_kit::test]
    fn copies_selected_response_text_from_context_menu(cx: &mut TestAppContext) {
        // GPUI includes the paragraph separator when the drag covers the whole response.
        select_and_copy(true, false, "héllo world\n", cx);
    }

    #[gpui_kit::test]
    fn right_click_on_unselected_link_has_no_selection_menu(cx: &mut TestAppContext) {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        struct LinkProbe {
            text: Entity<TextViewState>,
            selection_menu_open: Rc<Cell<bool>>,
            link_clicks: Arc<AtomicUsize>,
        }

        impl Render for LinkProbe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let selection_menu_open = self.selection_menu_open.clone();
                let link_clicks = self.link_clicks.clone();
                div().size_full().child(TextSelectionLayer).child(
                    div()
                        .id("link-text-menu")
                        .w(px(240.))
                        .child(
                            TextView::new(&self.text)
                                .selectable(true)
                                .on_link_click(move |_, event, _, _| {
                                    if event.is_right_click() {
                                        link_clicks.fetch_add(1, Ordering::SeqCst);
                                    }
                                }),
                        )
                        .context_menu(move |menu, window, cx| {
                            let menu = selection_menu(menu, window, cx);
                            selection_menu_open.set(!menu.is_empty());
                            menu
                        }),
                )
            }
        }

        cx.update(gpui_kit::init);
        let selection_menu_open = Rc::new(Cell::new(false));
        let link_clicks = Arc::new(AtomicUsize::new(0));
        let (_, cx) = cx.add_window_view({
            let selection_menu_open = selection_menu_open.clone();
            let link_clicks = link_clicks.clone();
            move |_, cx| LinkProbe {
                text: cx.new(|cx| TextViewState::markdown("[example](https://example.com)", cx)),
                selection_menu_open,
                link_clicks,
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let press = point(px(10.), px(10.));
        cx.simulate_mouse_down(press, MouseButton::Right, Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            !selection_menu_open.get(),
            "mouse-down must not show an empty Copy menu"
        );
        cx.simulate_mouse_up(press, MouseButton::Right, Modifiers::none());
        assert_eq!(
            link_clicks.load(Ordering::SeqCst),
            1,
            "mouse-up must reach the link handler"
        );
        assert!(!selection_menu_open.get());
    }
}
