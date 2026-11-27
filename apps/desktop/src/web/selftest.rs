//! `NEXUS_WEB_SELFTEST=surface` (macOS): a web view shows moving stripes instead of a page, and opens no page. GPUI's
//! Metal renderer draws them from an IOSurface-backed BGRA pixel buffer (`Window::paint_surface_with_owner`, the
//! Nexus patch in `vendor/README.md`). This is the path a page's GPU frames will take, checked on a Mac before the web
//! host sends any: the stripes show, sharp, in the right colours, and each buffer's owner drops after the GPU read it.
//!
//! The picture is `selftest_pattern.rs`'s, the same as on Windows. Judge it from a full-resolution screenshot with
//! `web-tests/selftest/check.py`, not by eye: on a Retina screen its 1 pixel checks look grey even when sharp.
//!
//! The view writes a ring of [`SLOTS`] buffers. It writes a slot again only after the slot's owner dropped, as the
//! host's ring of a page's frames will. With `NEXUS_WEB_TRACE=1`, the log has `owner drops/s` every 2 s: about the
//! frame rate when owners drop as they should, 0 when they never drop (then `no free slot/s` climbs). Keep the window
//! in front: macOS slows a window in the background to 30 frames a second or less, and both numbers drop with it.
//!
//! `NEXUS_WEB_SELFTEST=surface-cpu` draws the same pattern as an image, as CPU frames are drawn today. Images have no
//! owner, so its trace line has frames/s only. Screenshots of the two should match.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    Bounds, ContentMask, Corners, Div, EntityId, Pixels, RenderImage, Stateful, Window, canvas, div, point, px,
};
use objc2_core_foundation::{CFDictionary, CFRetained, CFString, CFType, kCFBooleanTrue};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey, kCVPixelFormatType_32BGRA,
    kCVReturnSuccess,
};
use smallvec::SmallVec;

use super::selftest_pattern::fill;

/// Buffers in the ring, as the host's ring of a page's frames.
const SLOTS: usize = 4;
/// How fast the top bars move, in device pixels a second: a whole number of pixels a frame at 60 and 120 Hz.
const SPEED: f64 = 240.;

#[derive(Clone, Copy)]
enum Mode {
    /// An IOSurface-backed buffer, drawn as a surface.
    Gpu,
    /// The same pattern as an image, as CPU frames.
    Cpu,
}

fn mode() -> Option<Mode> {
    static MODE: OnceLock<Option<Mode>> = OnceLock::new();
    *MODE.get_or_init(|| {
        let mode = match std::env::var("NEXUS_WEB_SELFTEST").ok()?.as_str() {
            "surface" => Mode::Gpu,
            "surface-cpu" => Mode::Cpu,
            _ => return None,
        };
        eprintln!(
            "nexus-desktop-gpui: web self-test: web views show moving stripes drawn as {}, no page",
            match mode {
                Mode::Gpu => "an IOSurface surface",
                Mode::Cpu => "an image",
            }
        );
        Some(mode)
    })
}

fn traced() -> bool {
    static TRACED: OnceLock<bool> = OnceLock::new();
    *TRACED.get_or_init(|| std::env::var("NEXUS_WEB_TRACE").is_ok_and(|value| !value.is_empty() && value != "0"))
}

/// Whether the self-test is on: a web view then opens no page.
pub(super) fn on() -> bool {
    mode().is_some()
}

/// What the web view `view` shows instead of its page, when the self-test is on. Each view has its own ring of
/// buffers, so two views of different sizes do not make each other allocate new ones on every frame.
pub(super) fn element(view: EntityId, window: &mut Window) -> Option<Stateful<Div>> {
    let mode = mode()?;
    // The stripes move: draw again next frame.
    window.request_animation_frame();
    let stripes = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| paint(mode, view, bounds, window),
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    Some(
        div()
            .id("web-view")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(gpui_kit::white())
            .child(stripes),
    )
}

thread_local! {
    /// Each web view's state, by view. A view that has not painted for [`GONE`] has closed: its state goes.
    static STATE: RefCell<HashMap<EntityId, State>> = RefCell::new(HashMap::new());
}

/// How long a view may go without painting before its state is dropped.
const GONE: Duration = Duration::from_secs(5);

struct State {
    /// When this view last painted.
    seen: Instant,
    start: Instant,
    size: (usize, usize),
    slots: Vec<Slot>,
    /// The slot drawn last and its owner, drawn again when no slot is free.
    last: Option<(usize, Arc<Lease>)>,
    /// The image drawn last (`surface-cpu`).
    image: Option<Arc<RenderImage>>,
    failed: bool,
    drops: Arc<AtomicU64>,
    painted: u64,
    full: u64,
    since: Instant,
}

struct Slot {
    buffer: CFRetained<CVPixelBuffer>,
    /// Set while an owner of this slot is alive: the scene or the GPU may read it.
    busy: Arc<AtomicBool>,
}

/// The owner handed to GPUI with a slot. GPUI drops it once the GPU has finished the frame that read the slot.
struct Lease {
    busy: Arc<AtomicBool>,
    drops: Arc<AtomicU64>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

fn paint(mode: Mode, view: EntityId, bounds: Bounds<Pixels>, window: &mut Window) {
    let scale = window.scale_factor();
    let width = (f32::from(bounds.size.width) * scale).round() as usize;
    let height = (f32::from(bounds.size.height) * scale).round() as usize;
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return;
    }
    // From a whole device pixel, at the pattern's own size, as a page's frame (`WebView::render`).
    let snap = |value: Pixels| px((f32::from(value) * scale).round() / scale);
    let frame = Bounds {
        origin: point(snap(bounds.origin.x), snap(bounds.origin.y)),
        size: gpui_kit::size(px(width as f32 / scale), px(height as f32 / scale)),
    };
    STATE.with(|state| {
        let mut states = state.borrow_mut();
        states.retain(|id, state| *id == view || state.seen.elapsed() < GONE);
        let state = states.entry(view).or_insert_with(|| State {
            seen: Instant::now(),
            start: Instant::now(),
            size: (0, 0),
            slots: Vec::new(),
            last: None,
            image: None,
            failed: false,
            drops: Arc::new(AtomicU64::new(0)),
            painted: 0,
            full: 0,
            since: Instant::now(),
        });
        state.seen = Instant::now();
        let offset = (state.start.elapsed().as_secs_f64() * SPEED) as usize;
        match mode {
            Mode::Gpu => window.with_content_mask(Some(ContentMask { bounds }), |window| {
                state.paint_surface(frame, width, height, offset, window)
            }),
            Mode::Cpu => window.with_content_mask(Some(ContentMask { bounds }), |window| {
                state.paint_image(frame, width, height, offset, window)
            }),
        }
        state.trace(mode, view, width, height);
    });
}

impl State {
    fn paint_surface(
        &mut self,
        frame: Bounds<Pixels>,
        width: usize,
        height: usize,
        offset: usize,
        window: &mut Window,
    ) {
        if self.size != (width, height) {
            self.size = (width, height);
            self.last = None;
            self.slots = match (0..SLOTS).map(|_| new_buffer(width, height)).collect::<Result<_, _>>() {
                Ok(slots) => slots,
                Err(status) => {
                    if !self.failed {
                        eprintln!(
                            "nexus-desktop-gpui: web self-test: no {width}x{height} BGRA IOSurface buffer (CVReturn {status})"
                        );
                        self.failed = true;
                    }
                    Vec::new()
                }
            };
        }
        if self.slots.is_empty() {
            return;
        }
        let free = self.slots.iter().position(|slot| !slot.busy.load(Ordering::Acquire));
        let Some(index) = free else {
            // Every slot is still held: draw the newest again, unchanged, under its own owner.
            self.full += 1;
            if let Some((index, lease)) = &self.last {
                window.paint_surface_with_owner(frame, self.slots[*index].buffer.clone(), lease.clone());
            }
            return;
        };
        let slot = &self.slots[index];
        if !write(&slot.buffer, width, height, offset) {
            return;
        }
        slot.busy.store(true, Ordering::Release);
        let lease = Arc::new(Lease {
            busy: slot.busy.clone(),
            drops: self.drops.clone(),
        });
        window.paint_surface_with_owner(frame, slot.buffer.clone(), lease.clone());
        self.last = Some((index, lease));
        self.painted += 1;
    }

    fn paint_image(&mut self, frame: Bounds<Pixels>, width: usize, height: usize, offset: usize, window: &mut Window) {
        let mut bytes = vec![0; width * height * 4];
        fill(&mut bytes, width * 4, width, height, offset);
        let Some(buffer) = image::RgbaImage::from_raw(width as u32, height as u32, bytes) else {
            return;
        };
        // GPUI's images are BGRA, as the pattern: no conversion.
        let image = Arc::new(RenderImage::new(SmallVec::from_elem(image::Frame::new(buffer), 1)));
        // Drawn at `frame` and clipped by the content mask, as `WebView::render` does: see there.
        let _ = window.paint_image(frame, frame, Corners::default(), image.clone(), 0, false);
        if let Some(old) = self.image.replace(image) {
            let _ = window.drop_image(old);
        }
        self.painted += 1;
    }

    fn trace(&mut self, mode: Mode, view: EntityId, width: usize, height: usize) {
        let elapsed = self.since.elapsed();
        if !traced() || elapsed < Duration::from_secs(2) {
            return;
        }
        let seconds = elapsed.as_secs_f64();
        let drops = self.drops.swap(0, Ordering::Relaxed);
        let frames = self.painted as f64 / seconds;
        match mode {
            Mode::Gpu => eprintln!(
                "nexus web trace: self-test view {view:?} {width}x{height} px: frames/s {frames:.1}, owner drops/s {:.1}, \
                 no free slot/s {:.1}",
                drops as f64 / seconds,
                self.full as f64 / seconds,
            ),
            // An image has no owner to drop.
            Mode::Cpu => eprintln!(
                "nexus web trace: self-test view {view:?} {width}x{height} px: frames/s {frames:.1} (image: no owners)"
            ),
        }
        self.painted = 0;
        self.full = 0;
        self.since = Instant::now();
    }
}

/// A BGRA pixel buffer on an IOSurface, which Metal can read.
fn new_buffer(width: usize, height: usize) -> Result<Slot, i32> {
    let empty = CFDictionary::<CFType, CFType>::empty();
    // SAFETY: CoreVideo's keys and CoreFoundation's true are immutable statics.
    let (surface_key, metal_key, yes) = unsafe {
        (
            kCVPixelBufferIOSurfacePropertiesKey,
            kCVPixelBufferMetalCompatibilityKey,
            kCFBooleanTrue,
        )
    };
    let Some(yes) = yes else {
        return Err(-1);
    };
    let keys: [&CFString; 2] = [surface_key, metal_key];
    let values: [&CFType; 2] = [&empty, yes];
    let attributes = CFDictionary::from_slices(&keys, &values);
    let mut buffer = std::ptr::null_mut();
    // SAFETY: The attributes are a CFString to CFType dictionary, and the output pointer is writable.
    let status = unsafe {
        CVPixelBufferCreate(
            None,
            width,
            height,
            kCVPixelFormatType_32BGRA,
            Some(attributes.as_opaque()),
            NonNull::from(&mut buffer),
        )
    };
    if status != kCVReturnSuccess {
        return Err(status);
    }
    let buffer = NonNull::new(buffer).ok_or(-1)?;
    Ok(Slot {
        // SAFETY: A successful create hands over this non-null buffer.
        buffer: unsafe { CFRetained::from_raw(buffer) },
        busy: Arc::new(AtomicBool::new(false)),
    })
}

/// Write the pattern into `buffer`, locked: the lock keeps the IOSurface's GPU copy in step on a Mac with its own GPU
/// memory.
fn write(buffer: &CVPixelBuffer, width: usize, height: usize, offset: usize) -> bool {
    // SAFETY: The buffer is live; it is unlocked below.
    if unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::empty()) } != kCVReturnSuccess {
        return false;
    }
    let base = CVPixelBufferGetBaseAddress(buffer).cast::<u8>();
    let stride = CVPixelBufferGetBytesPerRow(buffer);
    let written = !base.is_null() && stride >= width * 4;
    if written {
        // SAFETY: Locked, the buffer's memory is `stride` bytes for each of its `height` rows.
        let bytes = unsafe { std::slice::from_raw_parts_mut(base, stride * height) };
        fill(bytes, stride, width, height, offset);
    }
    // SAFETY: Locked above with the same flags.
    unsafe { CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::empty()) };
    written
}
