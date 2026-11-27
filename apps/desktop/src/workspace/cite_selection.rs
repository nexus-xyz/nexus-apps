//! A selected passage of a finished answer offers, over it, Start a side chat, Reply (`stacked_replies.rs`) and a
//! reply by voice. The selection rules are the Nexus app's (`cite-selection.tsx`).

use super::glass::FloatingGround;
use super::{View, Workspace};
use crate::palette::Palette;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::TextSelection;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::cite;
use std::{cell::Cell, rc::Rc, time::Duration};

/// The toolbar sits over the line the selection ended on: its bottom this far above the anchor, which is 6px under
/// the release.
const TOOLBAR_LIFT: f32 = 24.;
/// About the toolbar's width, so it stays inside the transcript.
const TOOLBAR_WIDTH: f32 = 248.;

pub(super) type BoundsCell = Rc<Cell<Option<Bounds<Pixels>>>>;
#[derive(Clone, PartialEq)]
struct Selection {
    source: EntityId,
    text: String,
}

pub(super) struct CiteState {
    selected: Option<Selection>,
    dismissed: Option<Selection>,
    pub observers: Vec<Subscription>,
    button: BoundsCell,
    focus: FocusHandle,
    pointer: Point<Pixels>,
    anchor_source: Option<EntityId>,
    anchor_offset: Option<Pixels>,
    anchor_pending: bool,
    pressed: bool,
    /// Keep the captured quote while the window selection clears on a toolbar press.
    button_pressed: bool,
    scroll_generation: u64,
    scrolling: bool,
    window_size: Option<Size<Pixels>>,
}
impl CiteState {
    pub fn new(focus: FocusHandle) -> Self {
        Self {
            selected: None,
            dismissed: None,
            observers: Vec::new(),
            button: Default::default(),
            focus,
            pointer: point(px(0.), px(0.)),
            anchor_source: None,
            anchor_offset: None,
            anchor_pending: false,
            pressed: false,
            button_pressed: false,
            scroll_generation: 0,
            scrolling: false,
            window_size: None,
        }
    }
}

/// Canvas reports the painted bounds without adding a second layout participant.
pub(super) fn bounds_probe(cell: BoundsCell) -> impl IntoElement {
    canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {})
        .absolute()
        .inset_0()
        .size_full()
}

impl Workspace {
    pub(super) fn refresh_cite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &self.view else { return };
        if view.cite.focus.is_focused(window) || view.cite.button_pressed {
            return;
        }
        let whole = TextSelection::selected_text(window, cx);
        let parts: Vec<(usize, EntityId, String)> = view
            .texts
            .iter()
            .enumerate()
            .filter_map(|(index, text)| {
                let text = text.as_ref()?;
                let selected = text.read(cx).selected_text();
                (!selected.trim().is_empty()).then_some((index, text.entity_id(), selected))
            })
            .collect();
        let candidate = cite::eligible(parts.iter().map(|(_, _, text)| text.as_str()), &whole).and_then(|text| {
            let (index, source, _) = parts.first()?;
            let bounds = view.list.bounds_for_item(*index)?;
            let viewport = view.list.viewport_bounds();
            bounds
                .intersects(&viewport)
                .then_some((Selection { source: *source, text }, bounds, viewport))
        });
        let View::Code(view) = &mut self.view else { return };
        let selected = candidate.and_then(|(selected, bounds, viewport)| {
            if view.cite.anchor_pending || view.cite.anchor_source != Some(selected.source) {
                view.cite.anchor_offset = Some(view.cite.pointer.y - bounds.top());
                view.cite.anchor_source = Some(selected.source);
                view.cite.anchor_pending = false;
            }
            let y = bounds.top() + view.cite.anchor_offset.unwrap_or(px(0.));
            if y < viewport.top() || y > viewport.bottom() + px(6.) {
                return None;
            }
            view.cite.pointer.y = y.min(viewport.bottom() - px(8.));
            view.cite.pointer.x = view
                .cite
                .pointer
                .x
                .max(viewport.left() + px(8.))
                .min((viewport.right() - px(TOOLBAR_WIDTH)).max(viewport.left() + px(8.)));
            Some(selected)
        });
        let next = if !window.is_window_active()
            || view.cite.pressed
            || view.cite.scrolling
            || selected == view.cite.dismissed
        {
            None
        } else {
            selected
        };
        if view.cite.selected != next {
            view.cite.selected = next;
            cx.notify();
        }
    }

    pub(super) fn dismiss_cite(&mut self, cx: &mut Context<Self>) {
        if let View::Code(view) = &mut self.view {
            if let Some(selected) = view.cite.selected.take() {
                view.cite.dismissed = Some(selected);
            }
            cx.notify();
        }
    }

    pub(super) fn cite_window_changed(&mut self, window: &Window) {
        if let View::Code(view) = &mut self.view {
            let size = window.viewport_size();
            if view.cite.window_size.is_some_and(|before| before != size) {
                view.cite.dismissed = view.cite.selected.take();
            }
            view.cite.window_size = Some(size);
        }
    }

    pub(super) fn cite_pointer_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // A press in the reply box is the box's; one outside closes it.
        if self.reply_box_pressed(event.position, window, cx) {
            return;
        }
        if let View::Code(view) = &mut self.view {
            if view
                .cite
                .button
                .get()
                .is_some_and(|bounds| view.cite.selected.is_some() && bounds.contains(&event.position))
            {
                view.cite.button_pressed = event.button == MouseButton::Left;
                return;
            }
            view.cite.button_pressed = false;
            view.cite.pressed = event.button == MouseButton::Left;
            view.cite.dismissed = None;
            view.cite.selected = None;
            view.cite.pointer = event.position;
            cx.notify();
        }
    }

    pub(super) fn cite_pointer_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left {
            return;
        }
        if let View::Code(view) = &mut self.view {
            if view.cite.button_pressed {
                // The window selection clears on press. Keep our snapshot through the click dispatch;
                // if the pointer was released elsewhere, refresh dismisses it afterward.
                cx.defer_in(window, |this, window, cx| {
                    if let View::Code(view) = &mut this.view {
                        view.cite.button_pressed = false;
                    }
                    this.refresh_cite(window, cx);
                });
                return;
            }
            if !view.cite.pressed {
                return;
            }
            view.cite.pressed = false;
            view.cite.pointer = event.position + point(px(0.), px(6.));
            view.cite.anchor_pending = true;
        }
        cx.defer_in(window, |this, window, cx| this.refresh_cite(window, cx));
    }

    pub(super) fn cite_scrolled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let held = matches!(&self.view, View::Code(view) if view.cite.selected.is_some());
        if !held && !TextSelection::has_selection(window, cx) {
            return;
        }
        let View::Code(view) = &mut self.view else { return };
        view.cite.selected = None;
        view.cite.scrolling = true;
        view.cite.scroll_generation = view.cite.scroll_generation.wrapping_add(1);
        let generation = view.cite.scroll_generation;
        let thread = view.id.clone();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let View::Code(view) = &mut this.view else { return };
                if view.id != thread || view.cite.scroll_generation != generation {
                    return;
                }
                view.cite.scrolling = false;
                this.refresh_cite(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn cite_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &self.view else { return };
        if view.cite.selected.is_none() {
            return;
        }
        if event.keystroke.key == "escape" {
            let focused = view.cite.focus.is_focused(window);
            self.dismiss_cite(cx);
            if focused {
                self.focus_composer(window, cx);
            }
            cx.stop_propagation();
        } else if event.keystroke.key == "tab"
            && !event.keystroke.modifiers.modified()
            && view
                .cite
                .selected
                .as_ref()
                .is_some_and(|s| s.text.encode_utf16().count() <= cite::CITE_MAX)
        {
            view.cite.focus.focus(window, cx);
            cx.stop_propagation();
        }
    }

    /// Take the selected passage off the toolbar, keeping it from showing again for the same selection.
    fn take_selection(&mut self) -> Option<(String, Point<Pixels>)> {
        let View::Code(view) = &mut self.view else { return None };
        let selected = view.cite.selected.take()?;
        let text = selected.text.clone();
        view.cite.dismissed = Some(selected);
        Some((text, view.cite.pointer - point(px(0.), px(TOOLBAR_LIFT))))
    }

    /// Reply: open the reply box over the passage (`stacked_replies.rs`); `listen`: dictate the reply.
    fn reply_to_selection(&mut self, listen: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((text, at)) = self.take_selection() else {
            return;
        };
        self.begin_reply(text, at, listen, window, cx);
    }

    /// Start a side chat with the passage quoted in its composer.
    fn side_chat_from_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((text, _)) = self.take_selection() else { return };
        let quote = cite::quote_block(&text);
        if quote.is_empty() {
            return;
        }
        TextSelection::clear(window, cx);
        self.create_side_chat_with_draft(format!("{quote}\n\n"), window, cx);
    }

    /// Over a selected passage: Start a side chat, Reply, and the microphone, which replies by voice.
    pub(super) fn render_cite(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let View::Code(view) = &self.view else {
            return div().into_any_element();
        };
        let Some(selected) = &view.cite.selected else {
            return div().into_any_element();
        };
        let too_long = selected.text.encode_utf16().count() > cite::CITE_MAX;
        let item = |id: &'static str| {
            h_flex()
                .id(id)
                .h(px(28.))
                .px(px(9.))
                .rounded(px(6.))
                .text_size(px(13.))
                .text_color(palette.foreground)
                .cursor_pointer()
                .hover(|style| style.bg(palette.menu_hover))
        };
        let divider = || div().flex_none().w(px(1.)).h(px(16.)).bg(palette.border);
        let toolbar = h_flex()
            .debug_selector(|| "cite-toolbar".into())
            .relative()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(8.))
            .border_1()
            .border_color(palette.border)
            .shadow_md()
            .floating_ground(8., palette.menu, true, &palette)
            .when(too_long, |bar| {
                bar.child(
                    div()
                        .h(px(28.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .text_size(px(13.))
                        .text_color(palette.faint)
                        .child("Selection too long"),
                )
            })
            .when(!too_long, |bar| {
                bar.child(
                    item("cite-side-chat")
                        .child("Start a side chat")
                        .on_click(cx.listener(|this, _, window, cx| this.side_chat_from_selection(window, cx))),
                )
                .child(divider())
                .child(
                    item("cite-reply")
                        .debug_selector(|| "cite-reply".into())
                        .track_focus(&view.cite.focus)
                        // Tab focuses Reply from the keyboard; a press does not, so a release elsewhere still
                        // dismisses the toolbar.
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                this.reply_to_selection(false, window, cx);
                            }
                        }))
                        .child("Reply")
                        .on_click(cx.listener(|this, _, window, cx| this.reply_to_selection(false, window, cx))),
                )
                .child(divider())
                .child(
                    item("cite-reply-voice")
                        .w(px(28.))
                        .px(px(0.))
                        .justify_center()
                        .child(Icon::new(Lucide::Mic).size(px(15.)).text_color(palette.muted))
                        .tooltip(|window, cx| Tooltip::new("Reply by voice").build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.reply_to_selection(true, window, cx))),
                )
            })
            .child(bounds_probe(view.cite.button.clone()));
        deferred(
            anchored()
                .position(view.cite.pointer - point(px(0.), px(TOOLBAR_LIFT)))
                .anchor(Anchor::BottomLeft)
                .snap_to_window_with_margin(px(8.))
                .child(toolbar),
        )
        .with_priority(3)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Selection, View, Workspace};
    use crate::palette::{Palette, ThemeChoice};
    use crate::workspace::{code_view::CodeView, machine_source::MachineSource, message_text::MessageText};
    use gpui_kit::base::{TextSelection, TextSelectionLayer};
    use gpui_kit::prelude::*;
    use gpui_kit::{
        Context, Entity, Focusable, IntoElement, Modifiers, MouseButton, Render, TestAppContext, Window,
        WindowAppearance, div, point, px,
    };

    struct Toolbar(Entity<Workspace>);

    impl Render for Toolbar {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |workspace, cx| {
                div()
                    .size_full()
                    .child(TextSelectionLayer)
                    .capture_any_mouse_down(
                        cx.listener(|this, event, window, cx| this.cite_pointer_down(event, window, cx)),
                    )
                    .capture_any_mouse_up(
                        cx.listener(|this, event, window, cx| this.cite_pointer_up(event, window, cx)),
                    )
                    .child(
                        div()
                            .w(px(300.))
                            .text_size(px(14.))
                            .line_height(px(22.))
                            .child(MessageText::new("answer", "selected answer")),
                    )
                    .child(workspace.render_cite(Palette::for_choice(ThemeChoice::Dark, WindowAppearance::Dark), cx))
            })
        }
    }

    fn cite_gesture(cancel: bool, cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (gateway, _gateway_events) = crate::gateway::Gateway::start();
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let (toolbar, cx) = cx.add_window_view(|window, cx| {
            Toolbar(cx.new(|cx| {
                let mut workspace = Workspace::with_machine_source(
                    std::sync::Arc::new(gateway),
                    events,
                    arrivals,
                    false,
                    MachineSource::isolated(),
                    window,
                    cx,
                );
                workspace.view = View::Code(CodeView::new("cite-test".into(), window, cx));
                workspace.draft.files.push(crate::attachments::persisted_attachment(
                    "notes.txt".into(),
                    None,
                    std::sync::Arc::new(b"kept".to_vec()),
                ));
                workspace
                    .composer
                    .update(cx, |state, cx| state.set_value("Existing draft", window, cx));
                workspace
            }))
        });
        let workspace = toolbar.read_with(cx, |toolbar, _| toolbar.0.clone());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let start = point(px(0.), px(10.));
        let end = point(px(220.), px(10.));
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        workspace.update_in(cx, |workspace, window, cx| {
            assert_eq!(TextSelection::selected_text(window, cx), "selected answer");
            let View::Code(view) = &mut workspace.view else {
                panic!()
            };
            view.cite.selected = Some(Selection {
                source: cx.entity_id(),
                text: "selected answer".into(),
            });
            view.cite.pointer = point(px(20.), px(50.));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let button = cx.debug_bounds("cite-reply").expect("Reply is drawn").center();
        cx.simulate_mouse_down(button, MouseButton::Left, Modifiers::none());
        // Text selection observers run between mouse-down and the button's click.
        workspace.update_in(cx, |workspace, window, cx| {
            let View::Code(view) = &workspace.view else { panic!() };
            assert!(
                view.cite.button_pressed,
                "press reaches the citation surface: {button:?}, bounds {:?}",
                view.cite.button.get()
            );
            assert!(view.cite.selected.is_some(), "press keeps snapshot");
            workspace.refresh_cite(window, cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        workspace.read_with(cx, |workspace, _| {
            let View::Code(view) = &workspace.view else { panic!() };
            assert!(view.cite.selected.is_some(), "draw keeps snapshot");
        });
        let release = if cancel { point(px(400.), px(200.)) } else { button };
        cx.simulate_mouse_move(release, Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(release, MouseButton::Left, Modifiers::none());
        cx.run_until_parked();
        workspace.update_in(cx, |workspace, window, cx| {
            // Reply opens the reply box over the passage; the composer and its files stay as they were.
            assert_eq!(workspace.composer.read(cx).value().as_ref(), "Existing draft");
            assert_eq!(workspace.draft.files.len(), 1);
            assert_eq!(workspace.draft.files[0].bytes.as_slice(), b"kept");
            assert_eq!(workspace.replies.is_composing(), !cancel);
            if !cancel {
                assert!(workspace.replies.input.read(cx).focus_handle(cx).is_focused(window));
            }
            let View::Code(view) = &workspace.view else { panic!() };
            assert!(view.cite.selected.is_none());
            assert!(!view.cite.button_pressed);
            assert!(!TextSelection::has_selection(window, cx));
        });
    }

    #[gpui_kit::test]
    fn pressing_reply_keeps_the_snapshot_until_click(cx: &mut TestAppContext) {
        cite_gesture(false, cx);
    }

    #[gpui_kit::test]
    fn releasing_outside_reply_dismisses_without_changing_the_draft(cx: &mut TestAppContext) {
        cite_gesture(true, cx);
    }
}
