//! The sidebar's moving marks (a running session's spinner, a waiting one's breathing ring), drawn over the sidebar's
//! pane by a view of their own rather than inside it.
//!
//! An animation asks GPUI to draw again the view that drew it, on every frame it moves. Inside the sidebar's pane
//! (`pane.rs`), one spinning mark drew the whole sidebar again on every frame: while a chat answers, or while any Code
//! session runs. So the list draws each moving mark still, as an empty box of its size that records where it landed,
//! and [`Overlay`] draws the moving shape there, over the pane. The pane keeps its last layout and paint until
//! something in it changes; it records its marks again each time it draws.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::code::Status;
use crate::motion;
use crate::palette::Palette;

/// Where the sidebar's pane put its moving marks the last time it drew, and the view that moves them.
#[derive(Clone)]
pub(super) struct Marks {
    placed: Placements,
    overlay: Entity<Overlay>,
}

type Placements = Rc<RefCell<Vec<Placed>>>;

struct Placed {
    id: SharedString,
    status: Status,
    bounds: Bounds<Pixels>,
    palette: Palette,
}

thread_local! {
    /// The marks being collected while the sidebar's list is built ([`Marks::collect`]).
    static COLLECTING: RefCell<Option<Placements>> = const { RefCell::new(None) };
}

impl Marks {
    /// `list` is the sessions list's scroll: a mark scrolled out of it is cut as the list cuts it.
    pub(super) fn new(list: &ScrollHandle, cx: &mut App) -> Self {
        let placed = Placements::default();
        let overlay = cx.new(|_| Overlay {
            placed: placed.clone(),
            list: list.clone(),
        });
        Self { placed, overlay }
    }

    /// The sidebar's pane draws again: the marks it shows now replace the last ones.
    pub(super) fn clear(&self) {
        self.placed.borrow_mut().clear();
    }

    /// Build the sessions list with `build`: each moving mark in it is drawn still ([`still`]) and recorded here.
    pub(super) fn collect<R>(&self, build: impl FnOnce() -> R) -> R {
        let previous = COLLECTING.with(|collecting| collecting.replace(Some(self.placed.clone())));
        let built = build();
        COLLECTING.with(|collecting| *collecting.borrow_mut() = previous);
        built
    }

    /// The waiting marks the list drew last, each where it landed: a waiting session's row, or a folded project's head
    /// that shows a wait inside it (`sidebar_edge_cue.rs`).
    pub(super) fn waiting(&self) -> Vec<(SharedString, Bounds<Pixels>)> {
        self.placed
            .borrow()
            .iter()
            .filter(|placed| placed.status == Status::NeedsYou)
            .map(|placed| (placed.id.clone(), placed.bounds))
            .collect()
    }

    /// How many marks move over the sidebar, and which.
    #[cfg(test)]
    pub(super) fn shown(&self) -> Vec<(SharedString, Status)> {
        self.placed
            .borrow()
            .iter()
            .map(|placed| (placed.id.clone(), placed.status))
            .collect()
    }

    /// The moving marks, over the sidebar's pane: put this after the pane, in the box the pane fills.
    pub(super) fn overlay(&self) -> Entity<Overlay> {
        self.overlay.clone()
    }
}

/// The moving marks' own view. An animation asks for the next frame for the view that draws it. Here that is this
/// view, which nothing observes. Drawn by the Workspace, the panes would draw again: they observe the Workspace.
pub(super) struct Overlay {
    placed: Placements,
    list: ScrollHandle,
}

impl Render for Overlay {
    /// A mark out of sight (the sidebar shut, the row scrolled away) is not drawn and asks for no frames.
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let placed = self.placed.clone();
        let list = self.list.clone();
        canvas(
            move |_, window, cx| {
                let clip = ContentMask { bounds: list.bounds() };
                let seen = clip.bounds.intersect(&window.content_mask().bounds);
                let elements = window.with_content_mask(Some(clip.clone()), |window| {
                    placed
                        .borrow()
                        .iter()
                        .filter(|placed| seen.intersects(&placed.bounds))
                        .map(|placed| {
                            let mut element = match (cx.reduce_motion(), placed.status) {
                                (true, Status::Background | Status::BackgroundUnread) => {
                                    activity_pulse(&placed.id, ORBIT, resting(placed.status, placed.palette))
                                }
                                _ => moving(&placed.id, placed.status, placed.palette),
                            };
                            element.layout_as_root(Size::<AvailableSpace>::from(placed.bounds.size), window, cx);
                            element.prepaint_at(placed.bounds.origin, window, cx);
                            element
                        })
                        .collect::<Vec<_>>()
                });
                (clip, elements)
            },
            |_, (clip, elements), window, cx| {
                window.with_content_mask(Some(clip), |window| {
                    for mut element in elements {
                        element.paint(window, cx);
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

/// Build `build` with its moving marks drawn where they are when `inline` holds, not over the list.
/// A folding group clips and fades its rows. The overlay knows neither, so a mark it drew stayed in full light,
/// over the rows below the group, until the fold ended. The fold redraws the pane on every frame anyway.
pub(super) fn in_place_while<R>(inline: bool, build: impl FnOnce() -> R) -> R {
    if !inline {
        return build();
    }
    let previous = COLLECTING.with(|collecting| collecting.replace(None));
    let built = build();
    COLLECTING.with(|collecting| *collecting.borrow_mut() = previous);
    built
}

/// While the sidebar's list is built ([`Marks::collect`]): a moving mark's place, empty, recording where it lands.
/// Elsewhere `None`, and the mark moves where it is drawn.
pub(super) fn still(id: &str, status: Status, palette: Palette) -> Option<AnyElement> {
    let placements = COLLECTING.with(|collecting| collecting.borrow().clone())?;
    let size = match status {
        Status::Running => SPINNER,
        // The ring with its dot: the frame (`sidebar::status_mark`) centres it.
        Status::NeedsYou => BREATH,
        Status::Background | Status::BackgroundUnread => ORBIT,
        // Still marks are drawn where they are (`sidebar::status_mark`).
        Status::Failed | Status::Idle | Status::Scheduled | Status::ScheduledUnread => return None,
    };
    let id = SharedString::from(id.to_string());
    let place = canvas(
        move |bounds, _, _| {
            placements.borrow_mut().push(Placed {
                id,
                status,
                bounds,
                palette,
            })
        },
        |_, _, _, _| {},
    )
    .size(px(size))
    .flex_none();
    Some(place.into_any_element())
}

/// The spinner's size: it turns inside a 12px track (`sidebar::status_mark`).
const SPINNER: f32 = 13.;
const SPINNER_TURN: Duration = Duration::from_millis(900);
const SLOW_SPINNER_TURN: Duration = Duration::from_secs(3);
/// The least opacity of an active mark's pulse under Reduce motion (1 is its full light).
const ACTIVITY_PULSE_LOW: f32 = 0.62;
/// The breathing ring's size, around an 8px dot.
pub(super) const BREATH: f32 = 14.;
/// The waiting mark's solid dot, in the middle of its ring.
const NEEDS_DOT: f32 = 8.;
/// The background mark's ring, which its dot goes round (`sidebar::status_mark` places its box).
pub(super) const ORBIT: f32 = 12.;
/// The box the ring and its dot are drawn in (`assets/sidebar/orbit-*.svg`): the ring, and room for the dot that
/// rides on its line and stands out of it.
const ORBIT_SPRITE: f32 = 16.;
/// The dot in the ring's middle: a reply the person has not read.
const UNREAD_DOT: f32 = 4.;
/// One turn of the dot: slower than the spinner, so it never reads as a turn at work.
const ORBIT_TURN: Duration = Duration::from_millis(2400);

/// A circle `diameter` wide around `center`, on whole device pixels at `scale`: centred on the device pixel corner
/// nearest `center`, and an even number of device pixels wide. GPUI rounds each edge of a quad to a device pixel on its
/// own, so at a fractional scale (1.6: an 8px dot is 12.8 device pixels) circles of different sizes around one centre
/// came out up to a device pixel off each other's middle. Circles placed here have whole edges, and stay concentric.
fn on_pixels(center: Point<Pixels>, diameter: f32, scale: f32) -> Bounds<Pixels> {
    let half = (diameter * scale / 2.).round().max(1.);
    let x = (f32::from(center.x) * scale).round();
    let y = (f32::from(center.y) * scale).round();
    Bounds::from_corners(
        point(px((x - half) / scale), px((y - half) / scale)),
        point(px((x + half) / scale), px((y + half) / scale)),
    )
}

/// A filled circle in `bounds`.
fn disc(bounds: Bounds<Pixels>, color: Hsla) -> PaintQuad {
    fill(bounds, color).corner_radii(Corners::all(bounds.size.width / 2.))
}

/// The background mark at `turn` (0 to 1, from the top, clockwise): the faint ring, the unread dot in its middle, and
/// the dot going round. The ring and the going-round dot are two sprites in one box, so the dot rides the middle of the
/// ring's line at any scale; the dot is turned about the box's centre, as the spinner is, so it moves between device
/// pixels. Painted as a quad, it jumped a whole device pixel at a time and changed width as it went.
fn orbit_mark(turn: f32, unread: bool, palette: Palette) -> impl IntoElement {
    let working = palette.working();
    let waiting = palette.waiting();
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, cx| {
            let scale = window.scale_factor();
            let center = bounds.center();
            let sprite = on_pixels(center, ORBIT_SPRITE, scale);
            let still = TransformationMatrix::unit();
            if let Err(error) = window.paint_svg(
                sprite,
                crate::assets::ORBIT_RING.into(),
                None,
                still,
                working.opacity(0.45),
                cx,
            ) {
                eprintln!("nexus-desktop-gpui: the background mark's ring was not drawn: {error:#}");
            }
            if unread {
                window.paint_quad(disc(on_pixels(center, UNREAD_DOT, scale), waiting));
            }
            let pivot = sprite.center();
            let turned = TransformationMatrix::unit()
                .translate(pivot.scale(scale))
                .rotate(radians(turn * std::f32::consts::TAU))
                .translate(pivot.scale(-scale));
            if let Err(error) = window.paint_svg(sprite, crate::assets::ORBIT_DOT.into(), None, turned, working, cx) {
                eprintln!("nexus-desktop-gpui: the background mark's dot was not drawn: {error:#}");
            }
        },
    )
    .size(px(ORBIT))
    .flex_none()
}

/// The scheduled mark: the background mark's ring with still clock hands in it, in the info colour. It is the
/// background mark at rest: the dot that goes round while work runs stands still as hands while the session waits on
/// time. The ring and the hands are sprites in one 16px box, so the hands sit in the ring's middle at any scale. An
/// unread reply's dot is a badge at the ring's top right ([`WAKE_BADGE`]): in the middle, as the background mark puts
/// it, it hid the hands at 1.6x and read as that mark. Nothing moves: the session waits on time, not on work (render
/// rule 6).
pub(super) fn wake_mark(unread: bool, palette: Palette) -> impl IntoElement {
    let info = palette.status_info;
    let waiting = palette.waiting();
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, cx| {
            let scale = window.scale_factor();
            let center = bounds.center();
            let sprite = on_pixels(center, ORBIT_SPRITE, scale);
            let still = TransformationMatrix::unit();
            for (path, color, what) in [
                (crate::assets::ORBIT_RING, info.opacity(0.45), "ring"),
                (crate::assets::WAKE_HANDS, info, "hands"),
            ] {
                if let Err(error) = window.paint_svg(sprite, path.into(), None, still, color, cx) {
                    eprintln!("nexus-desktop-gpui: the scheduled mark's {what} were not drawn: {error:#}");
                }
            }
            if unread {
                window.paint_quad(disc(on_pixels(center + WAKE_BADGE, UNREAD_DOT, scale), waiting));
            }
        },
    )
    .size(px(ORBIT))
    .flex_none()
}

/// Where the scheduled mark's unread badge sits from the ring's middle: at its top right (45°), just past the ring's
/// line and inside the row's 16px mark frame.
const WAKE_BADGE: Point<Pixels> = point(px(4.6), px(-4.6));

/// The waiting mark: the solid dot, and the ring around it at `breath` (0 to 1) of its light. Both are painted around
/// one centre on whole device pixels ([`on_pixels`]), so the dot sits in the ring's middle at any scale.
fn needs_mark(breath: f32, palette: Palette) -> impl IntoElement {
    let waiting = palette.waiting();
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _| {
            let scale = window.scale_factor();
            let center = bounds.center();
            window.paint_quad(disc(on_pixels(center, BREATH, scale), waiting.opacity(0.3 * breath)));
            window.paint_quad(disc(on_pixels(center, NEEDS_DOT, scale), waiting));
        },
    )
    .size(px(BREATH))
    .flex_none()
}

/// The background mark under reduce motion: its dot stops at the top of the ring.
pub(super) fn resting(status: Status, palette: Palette) -> AnyElement {
    orbit_mark(0., status == Status::BackgroundUnread, palette).into_any_element()
}

/// Under Reduce motion, the background mark pulses in place.
fn activity_pulse(id: &str, size: f32, mark: impl IntoElement) -> AnyElement {
    div()
        .size(px(size))
        .child(mark)
        .with_animation(
            SharedString::from(format!("activity-pulse-{id}")),
            Animation::new(motion::BREATHE)
                .repeat_synced()
                .with_max_fps(motion::LOOP_FPS)
                .with_easing(pulsating_between(ACTIVITY_PULSE_LOW, 1.)),
            |mark, opacity| mark.opacity(opacity),
        )
        .animate_with_reduced_motion()
        .into_any_element()
}

#[derive(IntoElement)]
struct RunningSpinner {
    id: SharedString,
    palette: Palette,
}

impl RenderOnce for RunningSpinner {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        // A slow turn keeps progress visible with Reduce motion on, in a row or in the overlay.
        let turn = if cx.reduce_motion() {
            SLOW_SPINNER_TURN
        } else {
            SPINNER_TURN
        };
        Icon::new(Lucide::LoaderCircle)
            .size(px(SPINNER))
            .text_color(self.palette.working())
            .with_animation(
                self.id,
                Animation::new(turn).repeat_synced().with_max_fps(motion::LOOP_FPS),
                |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
            )
            .animate_with_reduced_motion()
    }
}

/// A moving sidebar mark. Its animation uses an app-wide clock (`repeat_synced`).
pub(super) fn moving(id: &str, status: Status, palette: Palette) -> AnyElement {
    match status {
        Status::Running => RunningSpinner {
            id: format!("spin-{id}").into(),
            palette,
        }
        .into_any_element(),
        // The solid dot is drawn here with its ring, as one shape. Drawn apart (the dot in the list, the ring over it),
        // the two snapped to different pixels, and the dot sat 1px below the ring's middle.
        Status::NeedsYou => div()
            .size(px(BREATH))
            .with_animation(
                SharedString::from(format!("needs-{id}")),
                Animation::new(motion::PULSE)
                    .repeat_synced()
                    .with_max_fps(crate::motion::LOOP_FPS)
                    .with_easing(pulsating_between(0., 1.)),
                move |mark, breath| mark.child(needs_mark(breath, palette)),
            )
            .into_any_element(),
        Status::Background | Status::BackgroundUnread => {
            let unread = status == Status::BackgroundUnread;
            div()
                .size(px(ORBIT))
                .with_animation(
                    SharedString::from(format!("orbit-{id}")),
                    Animation::new(ORBIT_TURN)
                        .repeat_synced()
                        .with_max_fps(crate::motion::LOOP_FPS),
                    move |orbit, turn| orbit.child(orbit_mark(turn, unread, palette)),
                )
                .into_any_element()
        }
        Status::Failed | Status::Idle | Status::Scheduled | Status::ScheduledUnread => div().into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    /// A sidebar row's mark alone, at the window's corner, built as the sessions list builds it.
    struct Probe {
        marks: Marks,
        status: Status,
    }

    impl Render for Probe {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, window.appearance());
            self.marks.clear();
            let status = self.status;
            let mark = self
                .marks
                .collect(|| super::super::sidebar::status_mark("s", status, palette));
            div().size(px(64.)).child(mark)
        }
    }

    /// The orbit's box is the ring's: 12px, 2px in from the 16px frame's corner. It was placed from the frame's middle,
    /// and the dot went round a point outside the ring.
    #[gpui_kit::test]
    fn a_background_marks_orbit_sits_on_its_ring(cx: &mut gpui_kit::TestAppContext) {
        for status in [Status::Background, Status::BackgroundUnread] {
            let (probe, cx) = cx.add_window_view(|_, cx| Probe {
                marks: Marks::new(&ScrollHandle::new(), cx),
                status,
            });
            cx.run_until_parked();
            let placed = probe.read_with(cx, |probe, _| {
                probe
                    .marks
                    .placed
                    .borrow()
                    .iter()
                    .map(|placed| placed.bounds)
                    .collect::<Vec<_>>()
            });
            assert_eq!(
                placed,
                vec![Bounds::new(point(px(2.), px(2.)), size(px(ORBIT), px(ORBIT)))],
                "{status:?}"
            );
        }
    }

    /// The breathing ring is centred on the 8px dot: 1px in from the 16px frame's corner. With no offset it sat at the
    /// corner, and the dot sat 1px off its middle.
    #[gpui_kit::test]
    fn a_waiting_marks_ring_is_centred_on_its_dot(cx: &mut gpui_kit::TestAppContext) {
        let (probe, cx) = cx.add_window_view(|_, cx| Probe {
            marks: Marks::new(&ScrollHandle::new(), cx),
            status: Status::NeedsYou,
        });
        cx.run_until_parked();
        let placed = probe.read_with(cx, |probe, _| {
            probe
                .marks
                .placed
                .borrow()
                .iter()
                .map(|placed| placed.bounds)
                .collect::<Vec<_>>()
        });
        assert_eq!(
            placed,
            vec![Bounds::new(point(px(1.), px(1.)), size(px(BREATH), px(BREATH)))]
        );
    }

    /// A row in a folding group: the group clips and fades it, so its moving mark is drawn there, not over the list.
    /// Drawn over the list, a folding project's spinner stayed in full light over the folder row below it.
    struct Folding {
        marks: Marks,
        folding: bool,
    }

    impl Render for Folding {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, window.appearance());
            self.marks.clear();
            let folding = self.folding;
            let mark = self.marks.collect(|| {
                let marks: Vec<AnyElement> = in_place_while(folding, || {
                    [Status::Running, Status::NeedsYou, Status::Background]
                        .into_iter()
                        .map(|status| super::super::sidebar::status_mark("s", status, palette))
                        .collect()
                });
                div().children(marks)
            });
            div().size(px(64.)).child(mark)
        }
    }

    #[gpui_kit::test]
    fn a_folding_groups_marks_are_drawn_in_their_rows(cx: &mut gpui_kit::TestAppContext) {
        for (folding, overlaid) in [(false, 3), (true, 0)] {
            let (view, cx) = cx.add_window_view(|_, cx| Folding {
                marks: Marks::new(&ScrollHandle::new(), cx),
                folding,
            });
            cx.run_until_parked();
            let shown = view.read_with(cx, |view, _| view.marks.shown().len());
            assert_eq!(shown, overlaid, "folding: {folding}");
        }
    }

    /// A running mark drawn inline, as it is while its sidebar group folds.
    struct InlineSpinner;

    impl Render for InlineSpinner {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, window.appearance());
            super::super::sidebar::status_mark("s", Status::Running, palette)
        }
    }

    #[gpui_kit::test]
    fn a_running_mark_drawn_inline_keeps_animating_with_reduce_motion(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (row, cx) = cx.add_window_view(|_, _| InlineSpinner);
        let redraws = Rc::new(std::cell::Cell::new(0));
        let count = redraws.clone();
        cx.update(|window, cx| {
            cx.observe(&row, move |_, _| count.set(count.get() + 1)).detach();
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();

        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
        assert!(redraws.get() > 0, "a running spinner requests another frame");
    }

    /// A sidebar row with the overlay over it, as the sidebar draws them: the overlay clips to the list's bounds.
    struct Row {
        marks: Marks,
        list: ScrollHandle,
        status: Status,
    }

    impl Render for Row {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, window.appearance());
            self.marks.clear();
            let status = self.status;
            let mark = self
                .marks
                .collect(|| super::super::sidebar::status_mark("s", status, palette));
            div()
                .relative()
                .size(px(64.))
                .child(
                    div()
                        .id("list")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.list)
                        .child(div().pt(px(8.)).h(px(400.)).child(mark)),
                )
                .child(self.marks.overlay())
        }
    }

    /// Under Reduce motion the unread background mark keeps its place and only its light changes. With motion on, its
    /// dot is still and the orbit turns. The unread dot is the quad this finds: the orbit and the ring are sprites.
    #[gpui_kit::test]
    fn the_background_mark_pulses_in_place_with_reduce_motion(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (row, cx) = cx.add_window_view(|_, cx| {
            let list = ScrollHandle::new();
            Row {
                marks: Marks::new(&list, cx),
                list,
                status: Status::BackgroundUnread,
            }
        });
        cx.run_until_parked();
        // Draw a frame of the overlay now that the list's bounds are known.
        let overlay = row.read_with(cx, |row, _| row.marks.overlay());
        overlay.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();

        let unread = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                let width = on_pixels(point(px(0.), px(0.)), UNREAD_DOT, window.scale_factor())
                    .size
                    .width
                    .scale(window.scale_factor());
                window
                    .painted_quads()
                    .into_iter()
                    .find(|quad| quad.bounds.size.width == width)
                    .expect("the unread dot is drawn")
            })
        };
        let first = unread(cx);
        cx.executor().advance_clock(motion::BREATHE / 4);
        cx.run_until_parked();
        let second = unread(cx);
        assert_eq!(first.bounds, second.bounds, "the dot stays where it is");
        assert_ne!(first.background, second.background, "its light changes");

        cx.update(|_, cx| cx.set_reduce_motion(false));
        let first = unread(cx);
        cx.executor().advance_clock(ORBIT_TURN / 4);
        cx.run_until_parked();
        let second = unread(cx);
        assert_eq!(first.bounds, second.bounds);
        assert_eq!(
            first.background, second.background,
            "with motion on, the dot's light is still"
        );
    }

    /// A circle's edges in device pixels at `scale`.
    fn device_edges(bounds: Bounds<Pixels>, scale: f32) -> [f32; 4] {
        [bounds.left(), bounds.top(), bounds.right(), bounds.bottom()].map(|edge| f32::from(edge) * scale)
    }

    /// Every pair of circles a mark draws around one centre lands on whole device pixels with the same middle, at
    /// whole and fractional scales and wherever the row lands. GPUI rounds each edge of a quad on its own; edges that
    /// are already whole are left where they are, so the dot cannot sit off its ring's middle.
    #[test]
    fn a_marks_circles_share_their_middle_on_device_pixels_at_any_scale() {
        for scale in [1., 1.25, 1.5, 1.6, 1.75, 2., 2.5, 3.] {
            for (x, y) in [(8., 8.), (8.3, 8.5), (10.25, 30.75), (7.9, 101.45), (0.6, 0.2)] {
                let center = point(px(x), px(y));
                for (ring, dot) in [(BREATH, NEEDS_DOT), (ORBIT_SPRITE, UNREAD_DOT)] {
                    let ring = device_edges(on_pixels(center, ring, scale), scale);
                    let dot = device_edges(on_pixels(center, dot, scale), scale);
                    for edge in ring.iter().chain(&dot) {
                        assert!((edge - edge.round()).abs() < 1e-3, "scale {scale}: edge {edge}");
                    }
                    let middle = |edges: [f32; 4]| ((edges[0] + edges[2]) / 2., (edges[1] + edges[3]) / 2.);
                    let (ring_middle, dot_middle) = (middle(ring), middle(dot));
                    assert!(
                        (ring_middle.0 - dot_middle.0).abs() < 1e-3 && (ring_middle.1 - dot_middle.1).abs() < 1e-3,
                        "scale {scale} at ({x}, {y}): ring {ring:?}, dot {dot:?}"
                    );
                }
            }
        }
    }

    /// A number attribute of the first `<circle>` in `svg`.
    fn circle_attribute(svg: &str, name: &str) -> f32 {
        let circle = &svg[svg.find("<circle").expect("a circle")..];
        let value = &circle[circle.find(&format!(" {name}=\"")).expect(name) + name.len() + 3..];
        value[..value.find('"').expect("a closing quote")]
            .parse()
            .expect("a number")
    }

    /// The going-round dot rides the middle of the ring's line: its centre is as far from the box's centre as the
    /// line's middle is. It went round 4px out, inside the line's inner edge (4.5px), and read as sunk into the ring.
    /// The ring is the 12px mark with its 1.5px line, centred in the box.
    #[test]
    fn the_orbit_dot_rides_the_middle_of_its_rings_line() {
        let ring = include_str!("../../assets/sidebar/orbit-ring.svg");
        let dot = include_str!("../../assets/sidebar/orbit-dot.svg");
        let middle = ORBIT_SPRITE / 2.;
        let line = circle_attribute(ring, "stroke-width");
        let radius = circle_attribute(ring, "r");
        assert_eq!(
            (circle_attribute(ring, "cx"), circle_attribute(ring, "cy")),
            (middle, middle)
        );
        assert_eq!(radius + line / 2., ORBIT / 2., "the ring keeps its 12px size");
        assert_eq!(circle_attribute(dot, "cx"), middle);
        assert_eq!(
            middle - circle_attribute(dot, "cy"),
            radius,
            "the dot's centre is on the line's middle"
        );
        assert!(
            circle_attribute(dot, "cy") - circle_attribute(dot, "r") >= 0.,
            "the dot stays inside its box"
        );
        assert!(ring.contains("viewBox=\"0 0 16 16\"") && dot.contains("viewBox=\"0 0 16 16\""));
    }

    /// A scheduled session waits on time, not on work: its mark is drawn in its row, never over the list, and asks
    /// for no frame after the first (render rule 6).
    #[gpui_kit::test]
    fn a_scheduled_mark_is_still_and_drawn_in_its_row(cx: &mut gpui_kit::TestAppContext) {
        for status in [Status::Scheduled, Status::ScheduledUnread] {
            let (row, cx) = cx.add_window_view(|_, cx| {
                let list = ScrollHandle::new();
                Row {
                    marks: Marks::new(&list, cx),
                    list,
                    status,
                }
            });
            cx.run_until_parked();
            assert!(row.read_with(cx, |row, _| row.marks.shown().is_empty()), "{status:?}");
            let redraws = Rc::new(std::cell::Cell::new(0));
            let count = redraws.clone();
            cx.update(|window, cx| {
                cx.observe(&row, move |_, _| count.set(count.get() + 1)).detach();
                window.draw(cx).clear(cx);
            });
            cx.executor().advance_clock(Duration::from_secs(2));
            cx.run_until_parked();
            assert_eq!(redraws.get(), 0, "{status:?} asked for frames");
        }
    }

    /// The unread badge sits at the clock's top right, never over its hands, and inside the 16px mark frame. In the
    /// middle, as the background mark puts its dot, it hid the hands at 1.6x.
    #[gpui_kit::test]
    fn a_scheduled_marks_unread_badge_leaves_the_hands_clear(cx: &mut gpui_kit::TestAppContext) {
        let (_probe, cx) = cx.add_window_view(|_, cx| Probe {
            marks: Marks::new(&ScrollHandle::new(), cx),
            status: Status::ScheduledUnread,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let scale = window.scale_factor();
            let width = on_pixels(point(px(0.), px(0.)), UNREAD_DOT, scale)
                .size
                .width
                .scale(scale);
            let badge = window
                .painted_quads()
                .into_iter()
                .find(|quad| quad.bounds.size.width == width)
                .expect("the unread badge is drawn");
            // The probe's 16px frame is at the window's corner: its middle is (8, 8).
            let middle = 8. * scale;
            let (x, y) = (badge.bounds.center().x.as_f32(), badge.bounds.center().y.as_f32());
            assert!(x > middle && y < middle, "top right of the middle: ({x}, {y})");
            let frame = MARK_FRAME * scale;
            for edge in [
                badge.bounds.left(),
                badge.bounds.top(),
                badge.bounds.right(),
                badge.bounds.bottom(),
            ] {
                let edge = edge.as_f32();
                assert!((0. ..=frame).contains(&edge), "inside the frame: {edge}");
            }
        });
    }

    /// The row's mark frame (`sidebar::status_mark`).
    const MARK_FRAME: f32 = 16.;

    /// The numbers of an SVG path's `d`, in order.
    fn path_numbers(svg: &str) -> Vec<f32> {
        let d = &svg[svg.find(" d=\"").expect("a path") + 4..];
        d[..d.find('"').expect("a closing quote")]
            .split(|c: char| c.is_ascii_alphabetic() || c == ' ' || c == ',')
            .filter(|part| !part.is_empty())
            .map(|part| part.parse().expect("a number"))
            .collect()
    }

    /// The hands start at the ring's middle and, with their round caps, end inside the ring's inner edge: they read
    /// as a clock face, not as marks on the ring. Their line is the ring's, so the two look like one drawing.
    #[test]
    fn the_clock_hands_sit_inside_the_ring() {
        let ring = include_str!("../../assets/sidebar/orbit-ring.svg");
        let hands = include_str!("../../assets/sidebar/wake-hands.svg");
        assert!(hands.contains("viewBox=\"0 0 16 16\""));
        let line = circle_attribute(ring, "stroke-width");
        let inner = circle_attribute(ring, "r") - line / 2.;
        assert!(
            hands.contains(&format!("stroke-width=\"{line}\"")),
            "the hands' line is the ring's"
        );
        // `M x tip V y l dx dy`: the minute hand down from its tip to the middle, then the hour hand out of the middle.
        let [x, tip, y, dx, dy] = path_numbers(hands)[..] else {
            panic!("the hands are one path: M, V, l");
        };
        let middle = ORBIT_SPRITE / 2.;
        assert_eq!((x, y), (middle, middle), "the hands meet at the ring's middle");
        let cap = line / 2.;
        let (minute, hour) = (middle - tip, (dx * dx + dy * dy).sqrt());
        assert!(minute + cap < inner, "the minute hand's tip stays inside the ring");
        assert!(hour + cap < inner, "the hour hand's tip stays inside the ring");
        assert!(minute > hour, "the minute hand is the longer one");
    }

    /// A circle keeps its size to within a device pixel, and its middle stays within half a device pixel of where the
    /// layout put it.
    #[test]
    fn a_circle_on_device_pixels_keeps_its_size_and_place() {
        for scale in [1., 1.25, 1.6, 2.] {
            let center = point(px(20.3), px(9.7));
            for diameter in [UNREAD_DOT, NEEDS_DOT, ORBIT, BREATH] {
                let circle = on_pixels(center, diameter, scale);
                assert!((f32::from(circle.size.width) - diameter).abs() * scale <= 1. + 1e-3);
                assert!((f32::from(circle.size.width) - f32::from(circle.size.height)).abs() * scale < 1e-3);
                let off = circle.center() - center;
                assert!(f32::from(off.x).abs() * scale <= 0.5 + 1e-3 && f32::from(off.y).abs() * scale <= 0.5 + 1e-3);
            }
        }
    }
}
