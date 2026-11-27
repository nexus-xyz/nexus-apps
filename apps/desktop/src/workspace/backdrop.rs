//! The new session's background: a monochrome dot-matrix art behind the launch head and the composer, picked with the
//! "Background" button at the bottom right. Six arts, None, or Daily (one of the six, a new one each day),
//! at three strengths. The choice is kept on this computer (`newSessionBackground` in this build's own settings). None is
//! the default: the window's frost is the background now. A saved pick, Eclipse included, stays.
//!
//! Each art is a 2400x1500 grey map in `assets/backgrounds/` (0 is the window's background, 255 its ink), made by
//! `assets/backgrounds/make/`. It is tinted with the theme's two colors when drawn, so it suits light and dark alike.
//!
//! GPUI has no backdrop blur. The art never moves under the panels, though, so a panel on it draws a blurred copy of
//! the art's part under it, then its own fill: the composer, the recent sessions and the picker look like frosted
//! glass. The copy is the grey map shrunk eight times and blurred once, so it costs little.
//!
//! Motion (on by default, a switch in the picker): the art moves. The planets go round, the sphere turns, the horizon's
//! floor rolls toward the viewer (`backdrop_live.rs`). The moving art is drawn dot by dot, 20 frames a second, on its
//! own clock, which runs only while the window is on screen and has the focus, so a stopped art continues from where it
//! stopped. Until a frame of it is ready, and while the window changes size, the still image shows. Reduce motion
//! (Settings or the system) keeps the art still.

use crate::workspace::overlay::OverlaySlot;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use image::{GrayImage, ImageFormat, RgbaImage};
use serde_json::{Value, json};
use smallvec::SmallVec;

use super::backdrop_live as live;
use super::code_dock::{MenuTrigger as _, menu_box, menu_item_hover, menu_label, menu_layer};
use super::new_code::LaunchMenu;
use super::{View, Workspace};
use crate::pages::ui;
use crate::palette::Palette;

/// One of the arts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Art {
    Eclipse,
    Lattice,
    Orbits,
    Convergence,
    Moire,
    Horizon,
}

impl Art {
    pub const ALL: [Art; 6] = [
        Art::Eclipse,
        Art::Lattice,
        Art::Orbits,
        Art::Convergence,
        Art::Moire,
        Art::Horizon,
    ];

    fn key(self) -> &'static str {
        match self {
            Art::Eclipse => "eclipse",
            Art::Lattice => "lattice",
            Art::Orbits => "orbits",
            Art::Convergence => "convergence",
            Art::Moire => "moire",
            Art::Horizon => "horizon",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Art::Eclipse => "Eclipse",
            Art::Lattice => "Lattice",
            Art::Orbits => "Orbits",
            Art::Convergence => "Convergence",
            Art::Moire => "Moiré",
            Art::Horizon => "Horizon",
        }
    }

    fn parse(key: &str) -> Option<Art> {
        Art::ALL.into_iter().find(|art| art.key() == key)
    }

    fn bytes(self) -> &'static [u8] {
        match self {
            Art::Eclipse => include_bytes!("../../assets/backgrounds/eclipse.png"),
            Art::Lattice => include_bytes!("../../assets/backgrounds/lattice.png"),
            Art::Orbits => include_bytes!("../../assets/backgrounds/orbits.png"),
            Art::Convergence => include_bytes!("../../assets/backgrounds/convergence.png"),
            Art::Moire => include_bytes!("../../assets/backgrounds/moire.png"),
            Art::Horizon => include_bytes!("../../assets/backgrounds/horizon.png"),
        }
    }
}

/// What the picker offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pick {
    None,
    Art(Art),
    /// One of the arts, a new one each day.
    Daily,
}

/// How strongly the art shows: a veil of the background over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Strength {
    Faint,
    Normal,
    Bold,
}

impl Strength {
    const ALL: [Strength; 3] = [Strength::Faint, Strength::Normal, Strength::Bold];

    fn key(self) -> &'static str {
        match self {
            Strength::Faint => "faint",
            Strength::Normal => "normal",
            Strength::Bold => "bold",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Strength::Faint => "Faint",
            Strength::Normal => "Normal",
            Strength::Bold => "Bold",
        }
    }

    /// The veil's opacity.
    fn veil(self) -> f32 {
        match self {
            Strength::Faint => 0.55,
            Strength::Normal => 0.15,
            Strength::Bold => 0.,
        }
    }
}

/// The person's choice, as saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Choice {
    pub pick: Pick,
    pub strength: Strength,
    /// The art moves (`backdrop_live.rs`).
    pub motion: bool,
}

impl Default for Choice {
    fn default() -> Self {
        Choice {
            pick: Pick::None,
            strength: Strength::Normal,
            motion: true,
        }
    }
}

impl Choice {
    /// The saved choice; the default when there is none or it cannot be read.
    pub fn load() -> Choice {
        crate::paths::new_session_background().map_or_else(Choice::default, |value| Choice::from_json(&value))
    }

    fn from_json(value: &Value) -> Choice {
        let fallback = Choice::default();
        let pick = match value.get("pick").and_then(Value::as_str) {
            Some("none") => Pick::None,
            Some("daily") => Pick::Daily,
            Some(key) => Art::parse(key).map_or(fallback.pick, Pick::Art),
            None => fallback.pick,
        };
        let strength = value
            .get("strength")
            .and_then(Value::as_str)
            .and_then(|key| Strength::ALL.into_iter().find(|strength| strength.key() == key))
            .unwrap_or(fallback.strength);
        let motion = value.get("motion").and_then(Value::as_bool).unwrap_or(fallback.motion);
        Choice { pick, strength, motion }
    }

    fn to_json(self) -> Value {
        let pick = match self.pick {
            Pick::None => "none",
            Pick::Daily => "daily",
            Pick::Art(art) => art.key(),
        };
        json!({ "pick": pick, "strength": self.strength.key(), "motion": self.motion })
    }

    /// The art drawn on day `day` (days since 1970, UTC), if any.
    fn art(self, day: u64) -> Option<Art> {
        match self.pick {
            Pick::None => None,
            Pick::Art(art) => Some(art),
            Pick::Daily => Some(daily(day)),
        }
    }
}

/// Daily's art on `day`.
fn daily(day: u64) -> Art {
    Art::ALL[(day % Art::ALL.len() as u64) as usize]
}

fn today() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() / 86_400)
        .unwrap_or_default()
}

/// Which image of an art.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    /// The full art, behind the screen.
    Full,
    /// Shrunk and blurred, under a frosted panel.
    Frost,
    /// The picker's tile.
    Thumb,
}

/// An image's key: the art, which image, and the two colors it is tinted with.
type Key = (Art, Kind, u32, u32);

fn images() -> &'static Mutex<HashMap<Key, Arc<RenderImage>>> {
    static IMAGES: OnceLock<Mutex<HashMap<Key, Arc<RenderImage>>>> = OnceLock::new();
    IMAGES.get_or_init(Default::default)
}

fn pending() -> &'static Mutex<HashSet<Key>> {
    static PENDING: OnceLock<Mutex<HashSet<Key>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}

/// Full images that were replaced, for the next paint to take out of the window's sprite atlas: each one holds 14 MB.
fn stale() -> &'static Mutex<Vec<Arc<RenderImage>>> {
    static STALE: OnceLock<Mutex<Vec<Arc<RenderImage>>>> = OnceLock::new();
    STALE.get_or_init(Default::default)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The images the launch builds ahead of the first window (`prewarm`), until they are in [`images`]. A screen that
/// asks for one meanwhile waits on it and draws again when it is ready.
type Ready = futures::future::Shared<futures::channel::oneshot::Receiver<()>>;

fn warming() -> &'static Mutex<HashMap<Key, Ready>> {
    static WARMING: OnceLock<Mutex<HashMap<Key, Ready>>> = OnceLock::new();
    WARMING.get_or_init(Default::default)
}

/// Build the saved art for `palette` before the first window, off the UI thread: the first new session's screen then
/// draws it at once, instead of a frame or more of plain background. One decode makes the full image and its blurred
/// copy. A palette that turns out wrong (the desktop's appearance changed) builds as before, on first use.
pub(crate) fn prewarm(palette: Palette) {
    let choice = Choice::load();
    let Some(art) = choice.art(today()) else {
        return;
    };
    let (back, ink) = tints(palette, choice.strength);
    let (back_key, ink_key) = (u32::from_be_bytes(back), u32::from_be_bytes(ink));
    let full_key = (art, Kind::Full, back_key, ink_key);
    let frost_key = (art, Kind::Frost, back_key, ink_key);
    let (done, ready) = futures::channel::oneshot::channel::<()>();
    let ready = futures::FutureExt::shared(ready);
    {
        let mut pending = lock(pending());
        let mut warming = lock(warming());
        for key in [full_key, frost_key] {
            pending.insert(key);
            warming.insert(key, ready.clone());
        }
    }
    let finish = move |built: Option<(Arc<RenderImage>, Arc<RenderImage>)>| {
        if let Some((full, frost)) = built {
            let mut images = lock(images());
            images.insert(full_key, full);
            images.insert(frost_key, frost);
        }
        let mut pending = lock(pending());
        let mut warming = lock(warming());
        for key in [full_key, frost_key] {
            pending.remove(&key);
            warming.remove(&key);
        }
        let _ = done.send(());
    };
    let spawned = std::thread::Builder::new()
        .name("nexus-backdrop".into())
        .spawn(move || finish(build_full_and_frost(art, back, ink)));
    if let Err(error) = spawned {
        eprintln!("nexus-desktop-gpui: background prewarm: {error}");
        // The thread did not start, so `finish` (moved into it) is gone: forget the keys, and build on first use.
        let mut pending = lock(pending());
        let mut warming = lock(warming());
        for key in [full_key, frost_key] {
            pending.remove(&key);
            warming.remove(&key);
        }
    }
}

/// The full image and its blurred copy from one decode of the art.
fn build_full_and_frost(art: Art, back: [u8; 4], ink: [u8; 4]) -> Option<(Arc<RenderImage>, Arc<RenderImage>)> {
    use image::imageops::{FilterType, blur, resize};
    let full = levels(art)?;
    let (width, height) = full.dimensions();
    let frost = blur(&resize(&full, width / 8, height / 8, FilterType::Triangle), 4.);
    Some((tinted(&full, back, ink), tinted(&frost, back, ink)))
}

/// An art's grey map, decoded for one build and dropped after it: six of them held for the life of the app would
/// be about 22 MB, and the picker builds all six thumbnails.
fn levels(art: Art) -> Option<GrayImage> {
    image::load_from_memory_with_format(art.bytes(), ImageFormat::Png)
        .map(|image| image.to_luma8())
        .inspect_err(|error| eprintln!("nexus-desktop-gpui: background {}: {error}", art.key()))
        .ok()
}

/// A color as its 8-bit RGBA, for a key and for tinting.
fn bytes_of(color: Hsla) -> [u8; 4] {
    let rgba = Rgba::from(color);
    let byte = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    [byte(rgba.r), byte(rgba.g), byte(rgba.b), 255]
}

/// The art's ground: the page's color, opaque, or clear under a see-through window, so the art lies on the window's
/// veil instead of an opaque slab (`frost::art_ground`).
fn ground(palette: Palette) -> [u8; 4] {
    crate::frost::art_ground(&palette)
}

/// The art's two colors: its ground and its ink. On a clear ground the strength's veil is the ink's alpha, since a veil
/// of the page's color would lay an opaque slab over the window's.
fn tints(palette: Palette, strength: Strength) -> ([u8; 4], [u8; 4]) {
    let back = ground(palette);
    let mut ink = bytes_of(palette.foreground);
    if back[3] == 0 {
        ink[3] = ((1. - strength.veil()) * 255.).round() as u8;
    }
    (back, ink)
}

/// Build one image: the grey map, sized for its kind, tinted from `back` (0) to `ink` (255).
fn build(art: Art, kind: Kind, back: [u8; 4], ink: [u8; 4]) -> Option<Arc<RenderImage>> {
    use image::imageops::{FilterType, blur, resize};
    let full = levels(art)?;
    let (width, height) = full.dimensions();
    let map = match kind {
        Kind::Full => full,
        Kind::Frost => blur(&resize(&full, width / 8, height / 8, FilterType::Triangle), 4.),
        Kind::Thumb => resize(&full, 240, 150, FilterType::Triangle),
    };
    Some(tinted(&map, back, ink))
}

/// `map` in the two colors, as GPUI's BGRA. A clear `back` leaves the ground clear: the ink, more opaque as the level
/// rises.
fn tinted(map: &GrayImage, back: [u8; 4], ink: [u8; 4]) -> Arc<RenderImage> {
    let table: Vec<[u8; 4]> = (0..=255u16)
        .map(|level| mix(back, ink, f32::from(level) / 255.))
        .collect();
    let (width, height) = map.dimensions();
    let mut pixels = Vec::with_capacity(map.as_raw().len() * 4);
    for level in map.as_raw() {
        pixels.extend_from_slice(&table[usize::from(*level)]);
    }
    let buffer = RgbaImage::from_raw(width, height, pixels).unwrap_or_else(|| RgbaImage::new(1, 1));
    Arc::new(RenderImage::new(SmallVec::from_elem(image::Frame::new(buffer), 1)))
}

/// The color `t` of the way from `back` to `ink`, as GPUI's BGRA with straight alpha. Over a clear `back` the color stays
/// the ink's and only its alpha rises, so over the window's veil it reads as the mix would over an opaque ground.
pub(super) fn mix(back: [u8; 4], ink: [u8; 4], t: f32) -> [u8; 4] {
    let lerp = |from: u8, to: u8| (f32::from(from) + (f32::from(to) - f32::from(from)) * t).round() as u8;
    if back[3] == 0 {
        return [ink[2], ink[1], ink[0], lerp(0, ink[3])];
    }
    [lerp(back[2], ink[2]), lerp(back[1], ink[1]), lerp(back[0], ink[0]), 255]
}

/// The soft dark (or light) oval behind the launch head, so its words read on any art: the background color, opaque
/// in the middle and gone at the edge.
fn vignette(back: [u8; 4]) -> Arc<RenderImage> {
    static VIGNETTES: OnceLock<Mutex<HashMap<[u8; 4], Arc<RenderImage>>>> = OnceLock::new();
    let mut all = lock(VIGNETTES.get_or_init(Default::default));
    all.entry(back)
        .or_insert_with(|| {
            const SIDE: u32 = 96;
            let mut buffer = RgbaImage::new(SIDE, SIDE);
            for (x, y, pixel) in buffer.enumerate_pixels_mut() {
                let at = |v: u32| (v as f32 + 0.5) / SIDE as f32 * 2. - 1.;
                let r = (at(x).powi(2) + at(y).powi(2)).sqrt().min(1.);
                let fade = 1. - r * r * (3. - 2. * r);
                let alpha = (0.7 * fade * 255.).round() as u8;
                *pixel = image::Rgba([back[2], back[1], back[0], alpha]);
            }
            Arc::new(RenderImage::new(SmallVec::from_elem(image::Frame::new(buffer), 1)))
        })
        .clone()
}

/// The moving art's clock, and its latest frame. The clock runs only while the art moves on screen, so an art that
/// stopped continues from where it stopped.
#[derive(Default)]
pub(super) struct Motion {
    /// How long the art moved before `since`.
    ran: Duration,
    /// When it started moving this time. None while it holds still.
    since: Option<Instant>,
    /// The next frame is asked for.
    pending: bool,
    /// The latest frame, and where the art was drawn last (set in the art's prepaint), for the next frame.
    frame: Option<Arc<live::Frame>>,
    layout: Rc<Cell<Option<live::Layout>>>,
}

impl Motion {
    fn sync(&mut self, moving: bool, now: Instant) {
        match (moving, self.since) {
            (true, None) => self.since = Some(now),
            (false, Some(since)) => {
                self.ran += now.saturating_duration_since(since);
                self.since = None;
            }
            _ => {}
        }
    }

    fn seconds(&self, now: Instant) -> f32 {
        let running = self
            .since
            .map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
        (self.ran + running).as_secs_f32()
    }
}

/// What the art drew in this paint, for the frosted panels after it: where the whole art sits, its blurred copy, the
/// veil and the vignette. None while there is no art, or it is not built yet.
struct Drawn {
    cover: Bounds<Pixels>,
    frost: Arc<RenderImage>,
    veil: Hsla,
    vignette: (Bounds<Pixels>, Arc<RenderImage>),
}

thread_local! {
    static DRAWN: RefCell<Option<Drawn>> = const { RefCell::new(None) };
}

/// The art's rectangle: the image scaled to cover `bounds`, centered.
fn cover(bounds: Bounds<Pixels>, image: &RenderImage) -> Bounds<Pixels> {
    let pixels = image.size(0);
    let (width, height) = (pixels.width.0.max(1) as f32, pixels.height.0.max(1) as f32);
    let (room_width, room_height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let scale = (room_width / width).max(room_height / height);
    let drawn = size(px(width * scale), px(height * scale));
    Bounds::new(
        point(
            bounds.origin.x + (bounds.size.width - drawn.width) / 2.,
            bounds.origin.y + (bounds.size.height - drawn.height) / 2.,
        ),
        drawn,
    )
}

/// The vignette's oval: in the middle of `bounds`, up to 1100 by 760.
fn oval(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    let wide = px(f32::from(bounds.size.width).min(1100.));
    let tall = px(f32::from(bounds.size.height).min(760.));
    Bounds::new(
        point(
            bounds.origin.x + (bounds.size.width - wide) / 2.,
            bounds.origin.y + (bounds.size.height - tall) / 2.,
        ),
        size(wide, tall),
    )
}

thread_local! {
    /// Where the screen with the art last sat, in each window: its marker (`pane_marker`) sets it whenever the screen is
    /// laid out, and the art's layer (`art_layer`) reads it as it paints. The layer is earlier in the tree, so it draws
    /// under the head and the sidebar card, but every element is laid out before any paints. The mark is kept, not
    /// taken: the main pane is a cached view (`workspace/mod.rs`), so a frame that redraws only the sidebar (a hover)
    /// does not lay the screen out again, and a taken mark left that frame with no art: the art flickered.
    /// Whether the art draws at all is the screen's own state (`backdrop_art`), so a kept mark draws nothing elsewhere.
    static PANE: RefCell<HashMap<u64, Bounds<Pixels>>> = RefCell::new(HashMap::new());
    /// Where the sidebar card sits this frame, and in which window, the same way (`card_glass`): the sharp art stays
    /// out of it, and its glass shows the blurred art instead.
    static CARD: Cell<Option<(u64, Bounds<Pixels>)>> = const { Cell::new(None) };
}

/// `room` without `hole`: up to four strips around it.
fn around(room: Bounds<Pixels>, hole: Option<Bounds<Pixels>>) -> Vec<Bounds<Pixels>> {
    let Some(hole) = hole.map(|hole| hole.intersect(&room)).filter(|hole| !hole.is_empty()) else {
        return vec![room];
    };
    let strip = |left: Pixels, top: Pixels, right: Pixels, bottom: Pixels| {
        Bounds::from_corners(point(left, top), point(right, bottom))
    };
    [
        strip(room.left(), room.top(), room.right(), hole.top()),
        strip(room.left(), hole.bottom(), room.right(), room.bottom()),
        strip(room.left(), hole.top(), hole.left(), hole.bottom()),
        strip(hole.right(), hole.top(), room.right(), hole.bottom()),
    ]
    .into_iter()
    .filter(|strip| !strip.is_empty())
    .collect()
}

/// The art's room: the whole window from its top and left edges (under the head, and the sidebar card's glass) to the
/// screen's right edge and the window's bottom, so it has no edge of its own inside the window.
fn room_of(window: Bounds<Pixels>, pane: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::from_corners(window.origin, point(pane.right().min(window.right()), window.bottom()))
}

/// `live` if it was built for this room at this scale; else the still image shows.
fn live_for(live: &Option<Arc<live::Frame>>, room: Bounds<Pixels>, scale: f32) -> Option<Arc<live::Frame>> {
    live.as_ref()
        .filter(|frame| frame.layout == live::Layout { room, scale })
        .cloned()
}

/// Keep where the screen with the art is in window `id` ([`PANE`]).
fn mark_screen(id: u64, bounds: Bounds<Pixels>) {
    PANE.with(|pane| pane.borrow_mut().insert(id, bounds));
}

/// Where the screen with the art was last laid out in window `id`. Reading it keeps it for the next frame ([`PANE`]).
fn screen(id: u64) -> Option<Bounds<Pixels>> {
    PANE.with(|pane| pane.borrow().get(&id).copied())
}

/// Marks where the screen with the art is: put it in the screen, filling it.
fn pane_marker() -> impl IntoElement {
    canvas(
        |bounds, window, _| {
            mark_screen(window.window_handle().window_id().as_u64(), bounds);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

/// The art, filling the window behind everything but its ground (or, `local`, its parent): a frame of the moving art when one fits, else the
/// still image. It paints nothing while its images are built, or with no screen marked (`pane_marker`). `layout`
/// takes where the art is drawn, for the next moving frame.
fn art_layer(
    full: Option<Arc<RenderImage>>,
    frost: Option<Arc<RenderImage>>,
    live: Option<Arc<live::Frame>>,
    veil: Hsla,
    back: [u8; 4],
    layout: Rc<Cell<Option<live::Layout>>>,
    local: bool,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            for old in lock(stale()).drain(..) {
                let _ = window.drop_image(old);
            }
            // Local: the parent is the room, and the screen markers belong to the window's own layer.
            let (pane, card) = if local {
                (Some(bounds), None)
            } else {
                let id = window.window_handle().window_id().as_u64();
                let take = |cell: &'static std::thread::LocalKey<Cell<Option<(u64, Bounds<Pixels>)>>>| {
                    cell.with(|cell| cell.take())
                        .filter(|(window, _)| *window == id)
                        .map(|(_, bounds)| bounds)
                };
                (screen(id), take(&CARD))
            };
            let Some(pane) = pane.filter(|_| full.is_some() || live.is_some()) else {
                DRAWN.with(|drawn| drawn.borrow_mut().take());
                return;
            };
            let room = if local { bounds } else { room_of(bounds, pane) };
            let scale = window.scale_factor();
            layout.set(Some(live::Layout { room, scale }));
            let live = live_for(&live, room, scale);
            // The vignette lights the screen's middle, under its text.
            let shade = (oval(pane), vignette(back));
            let (art, frost, still) = match (&live, full.clone(), frost.clone()) {
                (Some(frame), _, _) => (frame.frost_bounds, frame.frost.clone(), None),
                (None, Some(full), Some(frost)) => (cover(room, &full), frost, Some(full)),
                _ => {
                    DRAWN.with(|drawn| drawn.borrow_mut().take());
                    return;
                }
            };
            for strip in around(room, card) {
                window.with_content_mask(Some(ContentMask { bounds: strip }), |window| {
                    if let Some(full) = &still {
                        let _ = window.paint_image(strip, art, Corners::default(), full.clone(), 0, false);
                    }
                    if let Some(frame) = &live {
                        frame.paint(strip.top(), strip.bottom(), window);
                    }
                    if veil.a > 0. {
                        window.paint_quad(fill(strip, veil));
                    }
                    let _ = window.paint_image(strip, shade.0, Corners::default(), shade.1.clone(), 0, false);
                });
            }
            DRAWN.with(|drawn| {
                *drawn.borrow_mut() = Some(Drawn {
                    cover: art,
                    frost,
                    veil,
                    vignette: shade,
                });
            });
        },
    )
    .absolute()
    .inset_0()
}

/// An art already built for `palette`, its full image and its blurred copy: the one on screen before a new pick.
fn shown(palette: Palette, strength: Strength) -> Option<(Arc<RenderImage>, Arc<RenderImage>)> {
    let (back, ink) = tints(palette, strength);
    let (back, ink) = (u32::from_be_bytes(back), u32::from_be_bytes(ink));
    let all = lock(images());
    all.iter()
        .find(|(key, _)| key.1 == Kind::Full && key.2 == back && key.3 == ink)
        .and_then(|(key, full)| Some((full.clone(), all.get(&(key.0, Kind::Frost, back, ink))?.clone())))
}

/// The blurred art under `bounds`, with the veil and the vignette as the window has them. False with no art drawn.
fn paint_glass(bounds: Bounds<Pixels>, corners: Corners<Pixels>, window: &mut Window) -> bool {
    let drawn = DRAWN.with(|drawn| {
        drawn
            .borrow()
            .as_ref()
            .map(|drawn| (drawn.cover, drawn.frost.clone(), drawn.veil, drawn.vignette.clone()))
    });
    let Some((art, image, veil, (shade, oval))) = drawn else {
        return false;
    };
    let _ = window.paint_image(bounds, art, corners, image, 0, false);
    if veil.a > 0. {
        window.paint_quad(fill(bounds, veil).corner_radii(corners));
    }
    let _ = window.paint_image(bounds, shade, corners, oval, 0, false);
    true
}

/// The sidebar card's glass on the art: the blurred art under the card, nothing with no art. The card's own fill
/// (`frost::panel`) draws over it, as it does over the blurred desktop.
pub(super) fn card_glass() -> impl IntoElement {
    canvas(
        |bounds, window, _| {
            let id = window.window_handle().window_id().as_u64();
            CARD.with(|card| card.set(Some((id, bounds))));
        },
        |bounds, _, window, _| {
            paint_glass(bounds, Corners::all(px(crate::frost::PANEL_RADIUS)), window);
        },
    )
    .absolute()
    .inset_0()
}

/// Frosted glass under a panel on the art: the blurred art under it, the veil and the vignette as the screen has them,
/// then `tint`. With no art drawn, only `tint`, opaque. The glass covers the panel's own border, so it draws `ring` as
/// the 1px border over its edge: a glass inset by the border would leave a dark seam between the two, where its
/// softened edge shows the art through.
pub(super) fn frost(radius: f32, tint: Hsla, ring: Option<Hsla>) -> impl IntoElement {
    frost_or(radius, tint, ring, None)
}

/// [`frost`], with `plain` as the fill where no art is drawn instead of the tint made opaque: a see-through window's
/// tier (`frost::Tiers`), so the control follows the Transparency setting off the art too.
pub(super) fn frost_or(radius: f32, tint: Hsla, ring: Option<Hsla>, plain: Option<Hsla>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let corners = Corners::all(px(radius));
            if paint_glass(bounds, corners, window) {
                window.paint_quad(fill(bounds, tint).corner_radii(corners));
            } else {
                window.paint_quad(fill(bounds, plain.unwrap_or(Hsla { a: 1., ..tint })).corner_radii(corners));
            }
            if let Some(ring) = ring {
                window.paint_quad(quad(
                    bounds,
                    corners,
                    transparent_black(),
                    px(1.),
                    ring,
                    BorderStyle::Solid,
                ));
            }
        },
    )
    .absolute()
    .inset_0()
}

impl Workspace {
    /// The art drawn on the screen now, if any: only a new session's screen and a new chat's have one.
    pub(super) fn backdrop_art(&self) -> Option<Art> {
        let screen = match self.view {
            View::NewCode => true,
            View::Chat => self.entries.is_empty() && !self.opening,
            _ => false,
        };
        screen.then(|| self.backdrop.art(today())).flatten()
    }

    /// Whether panels on this screen draw as frosted glass (also the home's, while they leave: `launch_motion.rs`).
    pub(super) fn frosted(&self) -> bool {
        self.backdrop_art().is_some() || self.launch_ghost()
    }

    /// The art drawn now: the screen's, or the home's while it dissolves under a session just started.
    fn drawn_art(&self) -> Option<Art> {
        self.backdrop_art().or_else(|| self.launch_art())
    }

    /// Whether the screen's art moves: Motion is on and the person did not ask for less motion.
    fn backdrop_moves(&self, cx: &App) -> bool {
        self.drawn_art().is_some() && self.backdrop.motion && !cx.reduce_motion()
    }

    /// Run the art's clock while it moves on a window that is on screen and has the focus, and ask for its next frame,
    /// built off the UI thread. Call it once per render, before the screen.
    pub(super) fn sync_backdrop_motion(&mut self, palette: Palette, window: &Window, cx: &mut Context<Self>) {
        let moves = self.backdrop_moves(cx);
        if !moves && let Some(frame) = self.backdrop_motion.frame.take() {
            lock(stale()).push(frame.frost.clone());
        }
        let moving = moves && window.is_visible() && window.is_window_active();
        self.backdrop_motion.sync(moving, Instant::now());
        if !moving || self.backdrop_motion.pending {
            return;
        }
        self.backdrop_motion.pending = true;
        let (back, ink) = tints(palette, self.backdrop.strength);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f32(1. / live::FPS))
                .await;
            let job = this
                .update(cx, |this, _| {
                    let art = this.drawn_art()?;
                    let layout = this.backdrop_motion.layout.get()?;
                    Some((art, layout, this.backdrop_motion.seconds(Instant::now())))
                })
                .ok()
                .flatten();
            let made = match job {
                Some((art, layout, seconds)) => Some(
                    cx.background_executor()
                        .spawn(async move { live::frame(art, layout, seconds, back, ink) })
                        .await,
                ),
                None => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.backdrop_motion.pending = false;
                if let Some(made) = made {
                    let old = if this.backdrop_moves(cx) {
                        this.backdrop_motion.frame.replace(Arc::new(made))
                    } else {
                        Some(Arc::new(made))
                    };
                    if let Some(old) = old {
                        lock(stale()).push(old.frost.clone());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// An image of `art`, tinted for `palette`; None while it is built, off the UI thread, with a redraw when done.
    fn backdrop_image(
        &self,
        art: Art,
        kind: Kind,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        let (back, ink) = tints(palette, self.backdrop.strength);
        let key = (art, kind, u32::from_be_bytes(back), u32::from_be_bytes(ink));
        if let Some(image) = lock(images()).get(&key) {
            return Some(image.clone());
        }
        // Built at launch (`prewarm`) and not ready yet: draw again the moment it is.
        if let Some(ready) = lock(warming()).get(&key).cloned() {
            cx.spawn(async move |this, cx| {
                let _ = ready.await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
            return None;
        }
        if !lock(pending()).insert(key) {
            return None;
        }
        let made = cx
            .background_executor()
            .spawn(async move { build(art, kind, back, ink) });
        cx.spawn(async move |this, cx| {
            let image = made.await;
            lock(pending()).remove(&key);
            if let Some(image) = image {
                lock(images()).insert(key, image);
            }
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        None
    }

    /// The art behind the window (`art_layer`): put it right after the window's ground, under the head and the sidebar
    /// card. It draws only while a screen with an art marks itself (`render_backdrop`).
    pub(super) fn render_backdrop_layer(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        self.backdrop_layer(self.backdrop_art(), false, palette, cx)
    }

    /// `art` filling its `relative` parent, on its own: the leaving home's in `launch_motion.rs`, which fades it out
    /// over the session that just started.
    pub(super) fn render_backdrop_art(&self, art: Option<Art>, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        self.backdrop_layer(art, true, palette, cx)
    }

    /// The art's layer: behind the whole window, or `local`, filling its parent.
    fn backdrop_layer(&self, art: Option<Art>, local: bool, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let full = art.and_then(|art| self.backdrop_image(art, Kind::Full, palette, cx));
        let frost = art.and_then(|art| self.backdrop_image(art, Kind::Frost, palette, cx));
        // While a newly picked art is built, the one before stays, instead of a flash of the plain background.
        let (full, frost) = match (full, frost) {
            (Some(full), Some(frost)) => {
                // One full art at a time, once the new one is whole: the others go, out of the atlas too.
                let mut all = lock(images());
                let old: Vec<Key> = all
                    .iter()
                    .filter(|(key, image)| key.1 == Kind::Full && !Arc::ptr_eq(image, &full))
                    .map(|(key, _)| *key)
                    .collect();
                for key in old {
                    if let Some(image) = all.remove(&key) {
                        lock(stale()).push(image);
                    }
                }
                (Some(full), Some(frost))
            }
            _ if art.is_some() => shown(palette, self.backdrop.strength).unzip(),
            _ => (None, None),
        };
        // On a clear ground the veil is in the ink's alpha (`tints`).
        let veil = if ground(palette)[3] == 0 {
            transparent_black()
        } else {
            crate::frost::Look::now(&palette)
                .veil
                .opacity(self.backdrop.strength.veil())
        };
        let live = self.backdrop_motion.frame.clone().filter(|frame| {
            self.backdrop_moves(cx) && Some(frame.art) == art && frame.tint == tints(palette, self.backdrop.strength)
        });
        art_layer(
            full,
            frost,
            live,
            veil,
            ground(palette),
            self.backdrop_motion.layout.clone(),
            local,
        )
        .into_any_element()
    }

    /// The marker of a new session's screen, for the art behind the window (`render_backdrop_layer`), and the
    /// "Background" button over its bottom right. Put the marker first in a `relative` parent, and the button last.
    pub(super) fn render_backdrop(&self, palette: Palette, cx: &mut Context<Self>) -> (AnyElement, AnyElement) {
        (
            pane_marker().into_any_element(),
            self.render_backdrop_button(palette, cx).into_any_element(),
        )
    }

    fn render_backdrop_button(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.overlay.launch_menu() == Some(LaunchMenu::Background);
        div()
            .absolute()
            .right(px(16.))
            .bottom(px(16.))
            .child(
                h_flex()
                    .id("backdrop-button")
                    .relative()
                    .h(px(28.))
                    .px(px(10.))
                    .gap(px(6.))
                    .rounded(px(8.))
                    .text_size(px(12.5))
                    .text_color(if open { palette.foreground } else { palette.muted })
                    .border_1()
                    .border_color(palette.border)
                    .when(open, |button| button.bg(palette.hover))
                    .child(frost_or(
                        8.,
                        palette.solid().raised.opacity(0.55),
                        Some(palette.solid().border),
                        palette.frosted.then_some(palette.pop),
                    ))
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.foreground))
                    .child(Icon::new(Lucide::Image).size(px(14.)).text_color(palette.icon))
                    .child("Background")
                    .menu_trigger(
                        open,
                        cx,
                        |this| this.overlay.launch_menu() == Some(LaunchMenu::Background),
                        |this, window, cx| this.toggle_launch_menu(LaunchMenu::Background, window, cx),
                    ),
            )
            .when_some(crate::motion::presence("backdrop-menu", open, cx), |frame, phase| {
                let menu = phase.menu("backdrop-menu", 4., self.render_backdrop_menu(palette, cx));
                frame.child(menu_layer(menu, Anchor::BottomRight))
            })
    }

    /// The picker's Motion row: the moving art on or off. Reduce motion keeps it off, and the row says so.
    fn render_backdrop_motion(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let choice = self.backdrop;
        let reduced = cx.reduce_motion();
        let on = choice.motion && !reduced;
        h_flex()
            .id("backdrop-motion")
            .mx(px(6.))
            .mt(px(4.))
            .mb(px(2.))
            .px(px(8.))
            .py(px(6.))
            .gap(px(12.))
            .rounded(px(8.))
            .opacity(if choice.pick == Pick::None { 0.5 } else { 1. })
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(1.))
                    .child(div().text_size(px(12.5)).text_color(palette.foreground).child("Motion"))
                    .child(div().text_size(px(11.5)).text_color(palette.faint).child(if reduced {
                        "Off while Reduce motion is on"
                    } else {
                        "The art moves, slowly"
                    })),
            )
            .child(ui::switch("backdrop-motion-switch", on, reduced, palette))
            .when(!reduced, |row| {
                row.cursor_pointer()
                    .hover(|style| menu_item_hover(style, palette))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_backdrop(
                            Choice {
                                motion: !this.backdrop.motion,
                                ..this.backdrop
                            },
                            cx,
                        )
                    }))
            })
    }

    /// The picker: a tile for None, each art and Daily, then the strength and the motion.
    fn render_backdrop_menu(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let choice = self.backdrop;
        let today = daily(today());
        let mut tiles: Vec<AnyElement> = vec![self.backdrop_tile(
            "backdrop-none",
            "None",
            None,
            choice.pick == Pick::None,
            Pick::None,
            palette,
            cx,
        )];
        for art in Art::ALL {
            let pick = Pick::Art(art);
            tiles.push(self.backdrop_tile(
                SharedString::from(format!("backdrop-{}", art.key())),
                art.name(),
                Some(art),
                choice.pick == pick,
                pick,
                palette,
                cx,
            ));
        }
        tiles.push(self.backdrop_tile(
            "backdrop-daily",
            "Daily",
            Some(today),
            choice.pick == Pick::Daily,
            Pick::Daily,
            palette,
            cx,
        ));
        let strengths = Strength::ALL.map(|strength| {
            let picked = choice.strength == strength;
            div()
                .id(SharedString::from(format!("backdrop-strength-{}", strength.key())))
                .flex_1()
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .text_size(px(12.5))
                .text_color(if picked { palette.foreground } else { palette.muted })
                .when(picked, |segment| segment.bg(palette.menu_hover))
                .cursor_pointer()
                .hover(|style| style.text_color(palette.foreground))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_backdrop(
                        Choice {
                            strength,
                            ..this.backdrop
                        },
                        cx,
                    )
                }))
                .child(strength.name())
        });
        menu_box("backdrop-menu", 316., palette)
            .relative()
            .bg(transparent_black())
            .child(frost_or(
                12.,
                palette.solid().menu.opacity(0.62),
                None,
                palette.frosted.then_some(palette.menu),
            ))
            .p(px(8.))
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                if this.overlay.launch_menu() == Some(LaunchMenu::Background) {
                    this.overlay.close_launch_menu();
                    cx.notify();
                }
            }))
            .child(menu_label("Background", palette))
            // Two even rows of four: None, the six arts, Daily.
            .child({
                let mut tiles = tiles.into_iter();
                let row = |tiles: &mut std::vec::IntoIter<AnyElement>| {
                    h_flex().justify_between().children(tiles.by_ref().take(4))
                };
                v_flex()
                    .gap(px(10.))
                    .p(px(6.))
                    .child(row(&mut tiles))
                    .child(row(&mut tiles))
            })
            .child(menu_label("Strength", palette))
            .child(
                h_flex()
                    .m(px(6.))
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(palette.border)
                    .opacity(if choice.pick == Pick::None { 0.5 } else { 1. })
                    .children(strengths),
            )
            .child(self.render_backdrop_motion(palette, cx))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn backdrop_tile(
        &self,
        id: impl Into<ElementId>,
        name: &'static str,
        art: Option<Art>,
        picked: bool,
        pick: Pick,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let thumb = art.and_then(|art| self.backdrop_image(art, Kind::Thumb, palette, cx));
        // A fixed face: the thumbnail's own size must not widen it.
        let face = div()
            .flex_none()
            .w(px(60.))
            .h(px(38.))
            .overflow_hidden()
            .rounded(px(6.))
            .bg(palette.background)
            .flex()
            .items_center()
            .justify_center()
            .when_some(thumb, |face, thumb| {
                face.child(
                    img(ImageSource::Render(thumb))
                        .w(px(60.))
                        .h(px(38.))
                        .rounded(px(6.))
                        .object_fit(ObjectFit::Cover),
                )
            })
            .when(art.is_none(), |face| {
                face.child(Icon::new(Lucide::Ban).size(px(14.)).text_color(palette.faint))
            });
        v_flex()
            .id(id)
            .w(px(66.))
            .flex_none()
            .items_center()
            .gap(px(4.))
            .cursor_pointer()
            .group("backdrop-tile")
            .on_click(cx.listener(move |this, _, _, cx| this.set_backdrop(Choice { pick, ..this.backdrop }, cx)))
            .child(
                div()
                    .p(px(1.))
                    .rounded(px(8.))
                    .border(px(1.5))
                    .border_color(if picked {
                        palette.foreground
                    } else {
                        transparent_black()
                    })
                    .hover(|style| if picked { style } else { menu_item_hover(style, palette) })
                    .child(face),
            )
            .child(
                div()
                    .w_full()
                    .text_center()
                    .whitespace_nowrap()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(if picked { palette.foreground } else { palette.muted })
                    .child(name),
            )
            .into_any_element()
    }

    /// Take `choice`, and keep it on this computer.
    fn set_backdrop(&mut self, choice: Choice, cx: &mut Context<Self>) {
        if self.backdrop == choice {
            return;
        }
        self.backdrop = choice;
        crate::paths::save_new_session_background(choice.to_json());
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    /// A frame that does not lay the screen out again (the cached main pane under a sidebar hover) still knows where to
    /// draw the art: reading the mark keeps it.
    #[test]
    fn the_screen_mark_outlives_a_frame_that_does_not_lay_the_screen_out() {
        let bounds = Bounds::new(point(px(240.), px(0.)), size(px(1000.), px(800.)));
        mark_screen(7, bounds);
        assert_eq!(screen(7), Some(bounds), "the frame that laid the screen out");
        assert_eq!(screen(7), Some(bounds), "the next frame, with the screen cached");
        assert_eq!(screen(8), None, "another window has its own mark");
    }

    #[test]
    fn none_at_normal_strength_is_the_default() {
        assert_eq!(Choice::default().pick, Pick::None);
        assert_eq!(Choice::default().strength, Strength::Normal);
        assert_eq!(Choice::from_json(&Value::Null), Choice::default());
        assert_eq!(
            Choice::from_json(&json!({ "pick": "nebula", "strength": "loud" })),
            Choice::default()
        );
    }

    #[test]
    fn a_choice_is_saved_and_read_back() {
        for pick in [Pick::None, Pick::Daily, Pick::Art(Art::Moire)] {
            for strength in Strength::ALL {
                for motion in [true, false] {
                    let choice = Choice { pick, strength, motion };
                    assert_eq!(Choice::from_json(&choice.to_json()), choice);
                }
            }
        }
    }

    #[test]
    fn a_choice_saved_before_motion_has_motion_on() {
        let choice = Choice::from_json(&json!({ "pick": "orbits", "strength": "faint" }));
        assert!(choice.motion);
        assert_eq!(choice.pick, Pick::Art(Art::Orbits));
        assert!(!Choice::from_json(&json!({ "pick": "orbits", "motion": false })).motion);
    }

    #[test]
    fn the_art_continues_from_where_it_stopped() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut motion = Motion::default();
        assert_eq!(motion.seconds(at(0)), 0.);
        motion.sync(true, at(10));
        assert_eq!(motion.seconds(at(14)), 4.);
        // A hidden or inactive window: the clock holds.
        motion.sync(false, at(14));
        assert_eq!(motion.seconds(at(100)), 4.);
        motion.sync(true, at(100));
        assert_eq!(motion.seconds(at(103)), 7.);
    }

    #[test]
    fn daily_shows_each_art_once_in_six_days() {
        let week: HashSet<Art> = (100..106).map(daily).collect();
        assert_eq!(week.len(), Art::ALL.len());
        let choice = Choice {
            pick: Pick::Daily,
            strength: Strength::Normal,
            motion: true,
        };
        assert_eq!(choice.art(7), Some(daily(7)));
        assert_eq!(
            Choice {
                pick: Pick::None,
                ..choice
            }
            .art(7),
            None
        );
    }

    #[test]
    fn every_art_reads() {
        for art in Art::ALL {
            let map = levels(art).expect("the art decodes");
            assert_eq!(map.dimensions(), (2400, 1500));
        }
    }

    #[test]
    fn the_art_covers_its_room_centered() {
        let full = tinted(&GrayImage::new(160, 100), [0, 0, 0, 255], [255, 255, 255, 255]);
        let room = Bounds::new(point(px(10.), px(0.)), size(px(100.), px(100.)));
        let art = cover(room, &full);
        assert_eq!(art.size, size(px(160.), px(100.)));
        assert_eq!(art.origin, point(px(-20.), px(0.)));
    }
}
