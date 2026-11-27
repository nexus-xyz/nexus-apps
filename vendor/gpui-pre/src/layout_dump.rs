//! Nexus: a dump of what each window drew, for tools that measure the UI (`desktop-gpui/polish/`).
//!
//! Off unless [`enable`] was called: every hook starts with [`enabled`], one relaxed atomic load, and records nothing.
//! On, each window's paint pass records its boxes (bounds, padding, border widths, corner radii, fill and border
//! colors, the source location that built them in debug builds), the text lines it painted (the rows' bounds, the
//! glyph band inside them, size, weight, color) and the icons (an `svg()`'s path), as a tree. View caching is off
//! while it records, so every frame paints every element. [`flush`] writes the last frame of every window to the
//! path given to [`enable`], as JSON, when something was painted since the last flush.
//!
//! The format (`"version": 1`) is documented in `desktop-gpui/polish/README.md`.

use crate::{
    App, Bounds, Corners, Edges, Fill, FlexDirection, Hsla, Overflow, Pixels, Rgba, Style, TextAlign, Window, WindowId,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

#[derive(Default)]
struct State {
    path: Option<PathBuf>,
    /// The frame being painted, by window.
    current: Option<Frame>,
    /// Ancestors of the next node: indices into `current.nodes`.
    stack: Vec<usize>,
    /// The last whole frame of each window.
    done: BTreeMap<u64, Frame>,
    dirty: bool,
    /// Frames that differed from the window's last one.
    frames: u64,
}

struct Frame {
    window: u64,
    viewport: [f32; 2],
    scale: f32,
    rem: f32,
    nodes: Vec<Value>,
}

impl Frame {
    fn same_as(&self, other: &Frame) -> bool {
        self.viewport == other.viewport
            && self.scale == other.scale
            && self.rem == other.rem
            && self.nodes == other.nodes
    }
}

/// Whether paint passes are being recorded.
#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Start recording; [`flush`] writes to `path`. Call on the main thread.
pub fn enable(path: PathBuf) {
    STATE.with(|state| state.borrow_mut().path = Some(path));
    ENABLED.store(true, Ordering::Relaxed);
}

/// Write the last frame of every window to the path [`enable`] took, if a frame ended since the last write. The file
/// is replaced whole (a temporary file, then a rename), so a reader never sees half of it. Returns whether it wrote.
pub fn flush() -> std::io::Result<bool> {
    if !enabled() {
        return Ok(false);
    }
    let (path, body) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !state.dirty {
            return (None, Value::Null);
        }
        state.dirty = false;
        let windows: Vec<Value> = state
            .done
            .values()
            .map(|frame| {
                json!({
                    "id": frame.window,
                    "viewport": frame.viewport,
                    "scale": frame.scale,
                    "zoom": crate::ui_zoom(),
                    "rem": frame.rem,
                    "nodes": frame.nodes,
                })
            })
            .collect();
        (
            state.path.clone(),
            json!({ "version": 1, "frames": state.frames, "windows": windows }),
        )
    });
    let Some(path) = path else { return Ok(false) };
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(&body).unwrap_or_default())?;
    std::fs::rename(&tmp, &path)?;
    Ok(true)
}

pub(crate) fn begin_frame(window: &Window) {
    let viewport = window.viewport_size();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.stack.clear();
        state.current = Some(Frame {
            window: window_id(window.window_handle().window_id()),
            viewport: [viewport.width.as_f32(), viewport.height.as_f32()],
            scale: window.scale_factor(),
            rem: window.rem_size().as_f32(),
            nodes: Vec::new(),
        });
    });
}

pub(crate) fn end_frame() {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.stack.clear();
        if let Some(frame) = state.current.take() {
            // A frame that drew the same thing (a caret blink, a hover-free repaint) is not a change: the file stays
            // as it is, so a reader can wait for it to stop changing.
            if state.done.get(&frame.window).is_some_and(|last| last.same_as(&frame)) {
                return;
            }
            state.done.insert(frame.window, frame);
            state.dirty = true;
            state.frames += 1;
        }
    });
}

fn window_id(id: WindowId) -> u64 {
    id.as_u64()
}

fn push(node: Value) -> Option<usize> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let parent = state.stack.last().copied();
        let frame = state.current.as_mut()?;
        let index = frame.nodes.len();
        let mut node = node;
        node["i"] = json!(index);
        node["parent"] = json!(parent);
        frame.nodes.push(node);
        Some(index)
    })
}

fn rect(bounds: Bounds<Pixels>) -> [f32; 4] {
    [
        round(bounds.origin.x.as_f32()),
        round(bounds.origin.y.as_f32()),
        round(bounds.size.width.as_f32()),
        round(bounds.size.height.as_f32()),
    ]
}

fn round(value: f32) -> f32 {
    (value * 100.).round() / 100.
}

fn edges(edges: Edges<Pixels>) -> [f32; 4] {
    [
        round(edges.top.as_f32()),
        round(edges.right.as_f32()),
        round(edges.bottom.as_f32()),
        round(edges.left.as_f32()),
    ]
}

fn corners(corners: Corners<Pixels>) -> [f32; 4] {
    [
        round(corners.top_left.as_f32()),
        round(corners.top_right.as_f32()),
        round(corners.bottom_right.as_f32()),
        round(corners.bottom_left.as_f32()),
    ]
}

fn color(color: Hsla) -> Option<String> {
    if color.is_transparent() {
        return None;
    }
    let rgba = Rgba::from(color);
    let byte = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    Some(format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        byte(rgba.r),
        byte(rgba.g),
        byte(rgba.b),
        byte(rgba.a)
    ))
}

fn source(location: Option<&'static core::panic::Location<'static>>) -> Value {
    match location {
        Some(at) => json!(format!("{}:{}", at.file(), at.line())),
        None => Value::Null,
    }
}

/// A box an `Interactivity` is about to paint (a `div`, an `svg`, an `img`): pushed as the parent of what paints
/// inside it, until [`pop_box`]. Returns whether it pushed.
pub(crate) fn push_box(
    style: &Style,
    bounds: Bounds<Pixels>,
    id: Option<String>,
    source_location: Option<&'static core::panic::Location<'static>>,
    clickable: bool,
    window: &Window,
) -> bool {
    let rem = window.rem_size();
    let background = match &style.background {
        Some(Fill::Color(background)) if !background.is_transparent() => match background.as_solid() {
            Some(solid) => color(solid).map(Value::from).unwrap_or(Value::Null),
            None => json!("gradient"),
        },
        _ => Value::Null,
    };
    let border = edges(style.border_widths.to_pixels(rem));
    let border_color = style.border_color.and_then(color);
    let overflow = |overflow: Overflow| !matches!(overflow, Overflow::Visible);
    let node = json!({
        "kind": "box",
        "bounds": rect(bounds),
        "clip": rect(window.content_mask().bounds),
        "source": source(source_location),
        "id": id,
        "padding": edges(style.padding.to_pixels(bounds.size.map(|size| size.into()), rem)),
        "border": border,
        "border_color": border_color,
        "radius": corners(style.corner_radii.to_pixels(rem)),
        "background": background,
        "shadow": !style.box_shadow.is_empty(),
        "clickable": clickable,
        "clips": [overflow(style.overflow.x), overflow(style.overflow.y)],
        "flex": match style.flex_direction {
            FlexDirection::Row | FlexDirection::RowReverse => "row",
            FlexDirection::Column | FlexDirection::ColumnReverse => "column",
        },
        "opacity": style.opacity.unwrap_or(1.),
    });
    let Some(index) = push(node) else { return false };
    STATE.with(|state| state.borrow_mut().stack.push(index));
    true
}

pub(crate) fn pop_box() {
    STATE.with(|state| {
        state.borrow_mut().stack.pop();
    });
}

/// The innermost box being painted is an icon: `svg()` with this path.
pub(crate) fn mark_icon(path: &str) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(&index) = state.stack.last() else { return };
        if let Some(node) = state.current.as_mut().and_then(|frame| frame.nodes.get_mut(index)) {
            node["icon"] = json!(path);
        }
    });
}

/// The innermost box being painted is an image.
pub(crate) fn mark_image() {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(&index) = state.stack.last() else { return };
        if let Some(node) = state.current.as_mut().and_then(|frame| frame.nodes.get_mut(index)) {
            node["image"] = json!(true);
        }
    });
}

/// A line of text about to be painted at `origin`, `line_height` per row, wrapped into `row_widths`, aligned within
/// `align_width`. `layout_box` is the text element's own box when the caller knows it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text(
    text: &str,
    origin: crate::Point<Pixels>,
    line_height: Pixels,
    layout: &crate::LineLayout,
    row_widths: &[Pixels],
    align: TextAlign,
    align_width: Option<Pixels>,
    layout_box: Option<Bounds<Pixels>>,
    color_of_first_run: Option<Hsla>,
    window: &Window,
    cx: &App,
) {
    if text.is_empty() {
        return;
    }
    let align_width = align_width.unwrap_or(layout.width);
    let leading = (line_height - layout.ascent - layout.descent) / 2.;
    let mut rows = Vec::with_capacity(row_widths.len());
    let mut union: Option<Bounds<Pixels>> = None;
    let mut ink: Option<Bounds<Pixels>> = None;
    for (row, width) in row_widths.iter().enumerate() {
        let offset = match align {
            TextAlign::Left => Pixels::ZERO,
            TextAlign::Center => (align_width - *width) / 2.,
            TextAlign::Right => align_width - *width,
        };
        let top = origin.y + line_height * row as f32;
        let row_box = Bounds::new(crate::point(origin.x + offset, top), crate::size(*width, line_height));
        let row_ink = Bounds::new(
            crate::point(origin.x + offset, top + leading),
            crate::size(*width, layout.ascent + layout.descent),
        );
        rows.push(rect(row_box));
        union = Some(union.map_or(row_box, |union| union.union(&row_box)));
        ink = Some(ink.map_or(row_ink, |ink| ink.union(&row_ink)));
    }
    let (Some(union), Some(ink)) = (union, ink) else { return };
    let font = layout
        .runs
        .first()
        .and_then(|run| cx.text_system().get_font_for_id(run.font_id));
    let node = json!({
        "kind": "text",
        "text": text,
        "bounds": rect(union),
        "ink": rect(ink),
        "rows": rows,
        "clip": rect(window.content_mask().bounds),
        "layout_box": layout_box.map(rect),
        "font_size": round(layout.font_size.as_f32()),
        "line_height": round(line_height.as_f32()),
        "font_weight": font.as_ref().map(|font| font.weight.0),
        "font_family": font.as_ref().map(|font| font.family.to_string()),
        "color": color_of_first_run.and_then(color),
        "align": match align {
            TextAlign::Left => "left",
            TextAlign::Center => "center",
            TextAlign::Right => "right",
        },
    });
    push(node);
}
