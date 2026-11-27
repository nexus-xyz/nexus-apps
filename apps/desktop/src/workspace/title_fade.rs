//! A sidebar row's title, or the Artifacts tab's page title: a title too long for its row fades out at its right edge
//! instead of ending in "…".
//! The text itself fades, so the fade shows whatever lies under the row: the glass sidebar's blurred backdrop, a
//! selected or picked fill, or the hover fill as it fades in and out. GPUI has no gradient text, so the fading edge is
//! painted in narrow strips, each clipped to itself and in its own alpha, as the text shimmer does (`motion/shimmer.rs`).

use gpui_kit::{
    App, AvailableSpace, Bounds, ContentMask, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, Pixels, SharedString, Style, TextAlign, Window, point, px, relative, size,
};

/// How wide the fade is.
const FADE: f32 = 24.;
/// How far inside a device pixel's edges a strip's mask stays. GPUI snaps a content mask outward to whole device
/// pixels, so a mask edge on the grid, off by a float error, would take in the next pixel.
const INSET: f32 = 0.01;

/// The fade between `left` and `right` (logical pixels in the window) at `scale` device pixels per logical pixel:
/// where the whole text ends, and one strip per device pixel column as `(left, right, alpha)`.
///
/// GPUI snaps a content mask outward to whole device pixels. A strip edge between two device pixels would therefore
/// paint that pixel column in both strips, and the doubled column shows as a bright seam. So every edge sits on the
/// device pixel grid, a hair inside it, and each column gets its own alpha, sampled at its middle.
fn fade_strips(left: f32, right: f32, scale: f32) -> (f32, Vec<(f32, f32, f32)>) {
    let end = (right * scale).floor();
    let start = ((right - FADE).max(left) * scale).round().min(end);
    let columns = (end - start) as usize;
    let strips = (0..columns)
        .map(|index| {
            let column = start + index as f32;
            let alpha = 1. - (index as f32 + 0.5) / columns as f32;
            ((column + INSET) / scale, (column + 1. - INSET) / scale, alpha)
        })
        .collect();
    ((start - INSET) / scale, strips)
}

/// What of `part` the parent's `mask` leaves, or `None` when nothing of it shows. GPUI snaps a mask outward to whole
/// device pixels, an empty one too: a strip clipped away by its parent (the shut sidebar) came back as its own device
/// pixel column, and the fade of every long title showed over the main pane.
fn shown(part: Bounds<Pixels>, mask: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    let part = part.intersect(&mask);
    (part.size.width > px(0.) && part.size.height > px(0.)).then_some(part)
}

/// The title `text` in the inherited text style, filling its box's width and one line high. Put it in a box that
/// sets its width, such as a `.flex_1().min_w(px(0.))` child of the row.
pub(super) fn fading_title(text: SharedString) -> FadingTitle {
    FadingTitle {
        text: one_line(text),
        fit: false,
    }
}

/// The title `text` as wide as the text, and narrower when its row has less room: it then fades as `fading_title`
/// does. For a title that sits next to what follows it, such as a button's label before its caret.
pub(super) fn fitted_fading_title(text: SharedString) -> FadingTitle {
    FadingTitle {
        text: one_line(text),
        fit: true,
    }
}

/// How wide a fitted title of `text` width is in `available` width: none at its smallest, else the text's width up
/// to the room it has.
fn fitted_width(text: Pixels, available: AvailableSpace) -> Pixels {
    match available {
        AvailableSpace::MinContent => px(0.),
        AvailableSpace::MaxContent => text,
        AvailableSpace::Definite(room) => text.min(room),
    }
}

/// `text` on one line: a title's line breaks read as spaces, since the row has one line.
fn one_line(text: SharedString) -> SharedString {
    if text.contains(['\n', '\r']) {
        text.replace(['\n', '\r'], " ").into()
    } else {
        text
    }
}

pub(super) struct FadingTitle {
    text: SharedString,
    /// As wide as the text and shrinking from there, instead of filling the box's width.
    fit: bool,
}

impl IntoElement for FadingTitle {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for FadingTitle {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let text_style = window.text_style();
        let line_height = text_style.line_height_in_pixels(window.rem_size());
        let mut style = Style::default();
        style.size.height = line_height.into();
        if !self.fit {
            style.size.width = relative(1.).into();
            return (window.request_layout(style, None, cx), ());
        }
        // The text's own width, shrinking in a row that has less room: the fade then shows. The title asks for no
        // width at its smallest, so a box around it (a button, a menu's trigger) can shrink too. A set width would
        // be the least it takes, and the title would push past what follows it.
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let run = text_style.to_run(self.text.len());
        let width = window
            .text_system()
            .shape_line(self.text.clone(), font_size, &[run], None)
            .width;
        style.flex_shrink = 1.;
        style.min_size.width = px(0.).into();
        let layout = window.request_measured_layout(style, move |known, available, _, _| {
            let shown = known.width.unwrap_or(fitted_width(width, available.width));
            size(shown, line_height)
        });
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // The text style is the row's at paint time, so the title takes the hover color with the row.
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = style.line_height_in_pixels(window.rem_size());
        let run = style.to_run(self.text.len());
        let color = run.color;
        // The layout is cached by text and font, so shaping the line again in another color costs no new layout.
        let mut paint = |part: Bounds<Pixels>, color: Hsla, window: &mut Window| {
            let Some(part) = shown(part, window.content_mask().bounds) else {
                return;
            };
            let run = gpui_kit::TextRun { color, ..run.clone() };
            let line = window
                .text_system()
                .shape_line(self.text.clone(), font_size, &[run], None);
            window.with_content_mask(Some(ContentMask { bounds: part }), |window| {
                if let Err(error) = line.paint(bounds.origin, line_height, TextAlign::Left, None, window, cx) {
                    eprintln!("nexus-desktop-gpui: a fading title could not be painted: {error}");
                }
            });
        };
        let width = window
            .text_system()
            .shape_line(self.text.clone(), font_size, std::slice::from_ref(&run), None)
            .width;
        if width <= bounds.size.width {
            paint(bounds, color, window);
            return;
        }
        // Too long: the title is whole up to where the fade starts, and the strips paint the rest, fading out.
        let left = f32::from(bounds.left());
        let (whole, strips) = fade_strips(left, f32::from(bounds.right()), window.scale_factor());
        let row = |left: f32, right: f32| {
            Bounds::new(
                point(px(left), bounds.origin.y),
                size(px((right - left).max(0.)), bounds.size.height),
            )
        };
        paint(row(left, whole), color, window);
        for (left, right, alpha) in strips {
            let faded = Hsla {
                a: color.a * alpha,
                ..color
            };
            paint(row(left, right), faded, window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The device pixel columns GPUI paints for a mask from `left` to `right` (its outward snap, `cover_bounds`).
    fn snapped(left: f32, right: f32, scale: f32) -> (i32, i32) {
        ((left * scale).floor() as i32, (right * scale).ceil() as i32)
    }

    #[test]
    fn each_device_pixel_column_is_painted_by_exactly_one_strip() {
        // Rows rarely start or end on a whole pixel, and fractional scales are common on Linux and Windows.
        for scale in [1., 1.25, 1.5, 1.75, 2., 3.] {
            for (left, right) in [(12., 200.), (12.37, 211.6), (40.5, 233.33), (8.1, 20.9)] {
                let (whole, strips) = fade_strips(left, right, scale);
                let mut next = snapped(left, whole, scale).1;
                for &(strip_left, strip_right, _) in &strips {
                    let (first, end) = snapped(strip_left, strip_right, scale);
                    assert_eq!((first, end), (next, next + 1), "scale {scale}, row {left}..{right}");
                    next = end;
                }
                assert!(next as f32 <= right * scale, "the fade paints past the row");
            }
        }
    }

    #[test]
    fn the_fade_falls_from_nearly_whole_to_nearly_clear_over_its_width() {
        for scale in [1., 1.5, 2.] {
            let (whole, strips) = fade_strips(10.3, 180.7, scale);
            let alphas: Vec<f32> = strips.iter().map(|strip| strip.2).collect();
            assert!((strips.len() as f32 - FADE * scale).abs() <= 1.);
            assert!((180.7 - whole - FADE).abs() <= 1.);
            assert!(alphas[0] > 0.95 && alphas[0] < 1.);
            assert!(alphas[alphas.len() - 1] > 0. && alphas[alphas.len() - 1] < 0.05);
            assert!(alphas.windows(2).all(|pair| pair[1] < pair[0]));
        }
    }

    /// The shut sidebar: its frame clips the rows to a strip at the window's left edge. No part of a long title's fade
    /// shows there; an empty mask would be snapped back to the strip's own device pixel column.
    #[test]
    fn a_fade_clipped_away_by_its_parent_paints_nothing() {
        let frame = Bounds::new(point(px(0.), px(0.)), size(px(16.), px(900.)));
        for scale in [1., 1.25, 1.5, 2.] {
            let (whole, strips) = fade_strips(40.3, 250.7, scale);
            let row = |left: f32, right: f32| Bounds::new(point(px(left), px(300.)), size(px(right - left), px(20.)));
            assert_eq!(shown(row(40.3, whole), frame), None, "scale {scale}");
            for (left, right, _) in strips {
                assert_eq!(
                    shown(row(left, right), frame),
                    None,
                    "scale {scale}: strip {left}..{right}"
                );
            }
        }
    }

    /// The open sidebar leaves every strip as it is, a hair inside its device pixel column.
    #[test]
    fn a_fade_inside_its_parent_keeps_every_strip() {
        let sidebar = Bounds::new(point(px(0.), px(0.)), size(px(280.), px(900.)));
        let (_, strips) = fade_strips(40.3, 250.7, 1.25);
        for (left, right, _) in strips {
            let strip = Bounds::new(point(px(left), px(300.)), size(px(right - left), px(20.)));
            assert_eq!(shown(strip, sidebar), Some(strip));
        }
    }

    /// A fitted title takes no width at its smallest, so the box around it can shrink, and never more than its text.
    #[test]
    fn a_fitted_title_shrinks_to_nothing_and_grows_to_its_text() {
        let text = px(300.);
        assert_eq!(fitted_width(text, AvailableSpace::MinContent), px(0.));
        assert_eq!(fitted_width(text, AvailableSpace::MaxContent), text);
        assert_eq!(fitted_width(text, AvailableSpace::Definite(px(120.))), px(120.));
        assert_eq!(fitted_width(text, AvailableSpace::Definite(px(500.))), text);
    }

    #[test]
    fn a_title_with_line_breaks_draws_on_one_line() {
        assert_eq!(one_line("Plan\nthe fix\r\n".into()).as_ref(), "Plan the fix  ");
        assert_eq!(one_line("Plan the fix".into()).as_ref(), "Plan the fix");
    }
}
