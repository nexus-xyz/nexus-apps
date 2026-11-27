//! Frost: the window as frosted glass. What is behind the window shows through, blurred by the desktop, under
//! a light tint over the whole window (the wash). The sidebar floats on the wash as a card (the panel), 8 px in from the
//! window's edges, and everything around the card lies on a denser tint (the veil). As in Cursor's glass window on macOS, the sidebar
//! is the most see-through part and the pane, where the person reads, the least. Names follow the web's Frost
//! (`ui/src/app/app/v5/frost-tokens.css`: `--frost-veil`, `--frost-panel`, `--frost-panel-shadow`).
//!
//! Transparency, kept on this computer beside the theme (`transparency`, `transparencySeeThrough` and
//! `transparencyBlur` in this build's settings file), is a `Frost`: how see-through the window is and how much its own
//! glass blurs, each from 0 to 100. Opaque (a solid veil and panel, the same card shape), Frost (the default) and
//! Glass (both more see-through) are three points on it (`Transparency::frost`); Settings' Fine-tune sliders reach the
//! points between and past them. The alphas between two points are interpolated, so a point draws exactly as the
//! preset did, and the pane always hides at least 90% of the desktop.
//!
//! The desktop's blur is the system's (`WindowBackgroundAppearance::Blurred`): macOS and Windows blur, and on Linux a
//! Wayland compositor with ext-background-effect-v1 (niri, KWin 6.7+) or KDE's blur. An app can turn it on or off, not
//! set its strength, so the Blur value sets the app's own glass (`glass.rs`), and at 0 turns the desktop's blur off
//! (`Transparent`). Where nothing blurs (X11, other Wayland compositors), a see-through window would show a sharp
//! desktop under the text, so every choice draws as Opaque there.
//!
//! When the system asks apps to reduce transparency (macOS's Reduce transparency, Windows' Transparency effects off),
//! every level draws as Opaque too (`system_solid`). Linux has no such setting here.
//!
//! `NEXUS_FROST_PREVIEW=1` is for checking the look where nothing blurs (the parity sandbox): the window stays opaque,
//! and a stand-in wallpaper is painted under the veil, so the tints and the card read as they would over a desktop.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::palette::Palette;
use gpui_kit::{
    BorderStyle, BoxShadow, Corners, Hsla, IntoElement, Styled, Window, WindowBackgroundAppearance, canvas, fill,
    linear_color_stop, linear_gradient, point, px, quad, rgb, rgba, transparent_black,
};

/// The window's padding around the sidebar card (top, left and bottom), and the gap between the card and the pane.
pub const INSET: f32 = 8.;
/// The sidebar card's corners.
pub const PANEL_RADIUS: f32 = 12.;
/// What the sidebar column takes beyond the card's width: the inset on its left and the gap on its right.
pub const GUTTER: f32 = 2. * INSET;

/// A preset: a named point on the `Frost` scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transparency {
    Opaque,
    #[default]
    Frost,
    Glass,
}

impl Transparency {
    pub const ALL: [Transparency; 3] = [Transparency::Opaque, Transparency::Frost, Transparency::Glass];

    pub fn label(self) -> &'static str {
        match self {
            Transparency::Opaque => "Opaque",
            Transparency::Frost => "Frost",
            Transparency::Glass => "Glass",
        }
    }

    /// The preset's point: Frost at 40, Glass at 70, both with today's 16 px glass.
    pub const fn frost(self) -> Frost {
        let see_through = match self {
            Transparency::Opaque => 0,
            Transparency::Frost => 40,
            Transparency::Glass => 70,
        };
        Frost {
            see_through,
            blur: Frost::DEFAULT_BLUR,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Transparency::Opaque => "opaque",
            Transparency::Frost => "frost",
            Transparency::Glass => "glass",
        }
    }

    fn from_key(key: &str) -> Option<Transparency> {
        Transparency::ALL.into_iter().find(|level| level.key() == key)
    }
}

/// How see-through the window is and how much its own glass blurs, each from 0 to 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frost {
    /// 0 is opaque; 100 is the clearest the window goes, where the pane still hides 90% of the desktop.
    pub see_through: u8,
    /// The glass's blur: 0 is none (and the desktop's blur is off), 50 is 16 px, 100 is 32 px.
    pub blur: u8,
}

impl Default for Frost {
    fn default() -> Frost {
        Transparency::default().frost()
    }
}

impl From<Transparency> for Frost {
    fn from(level: Transparency) -> Frost {
        level.frost()
    }
}

impl Frost {
    pub const MAX: u8 = 100;
    /// Today's glass, 16 px.
    pub const DEFAULT_BLUR: u8 = 50;
    /// The glass's blur at `blur` 100, in logical pixels (the Gaussian's sigma).
    const MAX_GLASS_PX: f32 = 32.;
    pub const OPAQUE: Frost = Transparency::Opaque.frost();

    /// A choice, each value held to 0..=100.
    pub fn new(see_through: u8, blur: u8) -> Frost {
        Frost {
            see_through: see_through.min(Frost::MAX),
            blur: blur.min(Frost::MAX),
        }
    }

    /// Nothing shows through.
    pub fn is_opaque(self) -> bool {
        self.see_through == 0
    }

    /// The preset this choice is, if any. Opaque is every choice at 0: nothing shows through, so nothing blurs.
    pub fn preset(self) -> Option<Transparency> {
        if self.is_opaque() {
            return Some(Transparency::Opaque);
        }
        Transparency::ALL.into_iter().find(|level| level.frost() == self)
    }

    /// The glass's blur in logical pixels; none on an opaque window.
    pub fn glass_px(self) -> f32 {
        if self.is_opaque() {
            0.
        } else {
            f32::from(self.blur) / f32::from(Frost::MAX) * Frost::MAX_GLASS_PX
        }
    }

    /// Whether the desktop behind the window is blurred: see-through, with a blur above 0.
    pub fn blurs_desktop(self) -> bool {
        !self.is_opaque() && self.blur > 0
    }

    fn pack(self) -> u32 {
        LOADED | u32::from(self.see_through) << 8 | u32::from(self.blur)
    }

    fn unpack(code: u32) -> Option<Frost> {
        (code & LOADED != 0).then(|| Frost::new((code >> 8) as u8, code as u8))
    }

    /// The choice in the settings file: a preset's name, or `custom` with its two values. A missing or unknown name
    /// is Frost; a custom choice with a missing value takes the default's.
    fn from_stored(key: Option<&str>, see_through: Option<u64>, blur: Option<u64>) -> Frost {
        if key == Some(CUSTOM)
            && let Some(see_through) = see_through
        {
            let blur = blur.unwrap_or(u64::from(Frost::DEFAULT_BLUR));
            return Frost::new(see_through.min(100) as u8, blur.min(100) as u8);
        }
        key.and_then(Transparency::from_key).unwrap_or_default().frost()
    }

    /// The name the settings file keeps: the preset's, or `custom`.
    fn key(self) -> &'static str {
        match self.preset() {
            Some(level) if level.frost() == self => level.key(),
            _ => CUSTOM,
        }
    }
}

const CUSTOM: &str = "custom";
/// Marks `CHOSEN` as read from the settings file.
const LOADED: u32 = 1 << 16;

/// The person's choice this launch: 0 until read from the settings file.
static CHOSEN: AtomicU32 = AtomicU32::new(0);

/// The choice the person made (Frost when none is saved), whether or not this desktop can show it.
pub fn chosen() -> Frost {
    if let Some(frost) = Frost::unpack(CHOSEN.load(Ordering::Relaxed)) {
        return frost;
    }
    let (see_through, blur) = crate::paths::transparency_values();
    let frost = Frost::from_stored(crate::paths::transparency().as_deref(), see_through, blur);
    CHOSEN.store(frost.pack(), Ordering::Relaxed);
    frost
}

/// Draw every window at `frost` without keeping it: a Fine-tune slider while it moves. The caller applies it.
pub fn try_on(frost: Frost) {
    CHOSEN.store(frost.pack(), Ordering::Relaxed);
}

/// Choose `frost` for every window, and keep it for the next launch. The windows follow on their next frame
/// (`sync_window`); the caller applies it.
pub fn choose(frost: Frost) -> Result<(), String> {
    CHOSEN.store(frost.pack(), Ordering::Relaxed);
    crate::paths::save_transparency(frost.key(), frost.see_through, frost.blur)
}

/// Whether the desktop blurs what is behind a see-through window. Known once the platform has started.
pub fn can_blur() -> bool {
    if preview() {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        gpui_linux::window_blur_available()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// Whether the system asked for less transparency, as last read (`read_system`).
static SYSTEM_SOLID: AtomicBool = AtomicBool::new(false);

/// Read the system's reduce transparency setting again. At startup, before the first window opens, and when the
/// person opens Settings › Appearance. True when it changed.
pub fn read_system() -> bool {
    let now = os_reduces_transparency();
    SYSTEM_SOLID.swap(now, Ordering::Relaxed) != now
}

/// Whether the system keeps the windows solid (its reduce transparency setting), as last read.
pub fn system_solid() -> bool {
    SYSTEM_SOLID.load(Ordering::Relaxed)
}

/// What the windows draw: the choice, or Opaque where nothing blurs or the system asks for less transparency.
pub fn shown() -> Frost {
    level_shown(chosen(), can_blur(), system_solid())
}

/// `shown`'s rule, apart from where its inputs come from.
fn level_shown(chosen: Frost, can_blur: bool, system_solid: bool) -> Frost {
    if can_blur && !system_solid {
        chosen
    } else {
        Frost::OPAQUE
    }
}

/// Whether a choice made now shows: false where nothing blurs or the system asks for less transparency.
pub fn available() -> bool {
    can_blur() && !system_solid()
}

/// macOS's Accessibility › Display › Reduce transparency.
#[cfg(target_os = "macos")]
fn os_reduces_transparency() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency()
}

/// Windows' Personalization › Colors › Transparency effects: `EnableTransparency` 0 under
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`. Missing means on.
#[cfg(windows)]
fn os_reduces_transparency() -> bool {
    const HKEY_CURRENT_USER: isize = 0x8000_0001_u32 as i32 as isize;
    const RRF_RT_REG_DWORD: u32 = 0x10;
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            key: isize,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut core::ffi::c_void,
            size: *mut u32,
        ) -> i32;
    }
    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value = wide("EnableTransparency");
    let mut enabled: u32 = 1;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: both names are NUL-terminated and live for the call; the value is one DWORD written to `enabled`.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            core::ptr::null_mut(),
            (&raw mut enabled).cast(),
            &raw mut size,
        )
    };
    read == 0 && enabled == 0
}

#[cfg(not(any(target_os = "macos", windows)))]
fn os_reduces_transparency() -> bool {
    false
}

/// The sandbox's stand-in for a desktop that blurs (see the module's doc).
pub fn preview() -> bool {
    static PREVIEW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PREVIEW.get_or_init(|| std::env::var_os("NEXUS_FROST_PREVIEW").is_some_and(|value| value == "1"))
}

/// The background a main window asks the platform for: blurred, or clear when the Blur is 0.
pub fn window_background() -> WindowBackgroundAppearance {
    background_for(shown(), preview())
}

fn background_for(shown: Frost, preview: bool) -> WindowBackgroundAppearance {
    if shown.is_opaque() || preview {
        WindowBackgroundAppearance::Opaque
    } else if shown.blurs_desktop() {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Transparent
    }
}

/// A main window's options: the chosen background, and on macOS the traffic lights moved down into the sidebar card,
/// whose head starts `INSET` below the window's top.
pub fn main_window(mut options: gpui_kit::WindowOptions) -> gpui_kit::WindowOptions {
    options.window_background = window_background();
    if let Some(at) = options
        .titlebar
        .as_mut()
        .and_then(|titlebar| titlebar.traffic_light_position.as_mut())
    {
        at.y += px(INSET);
    }
    options
}

/// Give every main window the chosen level's background now. The panel and the detached windows stay opaque.
pub fn apply(cx: &mut gpui_kit::App) {
    let panel = crate::windows::panel(cx).map(|(window, _)| window);
    for (window, _) in crate::windows::windows(cx) {
        if Some(window) == panel {
            continue;
        }
        if let Err(error) = window.update(cx, |_, window, _| window.set_background_appearance(window_background())) {
            eprintln!("nexus-desktop-gpui: transparency: {error}");
        }
    }
    cx.refresh_windows();
}

/// GPUI Kit's root fills the whole window with its theme's background, under everything the app draws. On a
/// see-through window that is an opaque slab under the veil, so the desktop never showed through (the frame's alpha
/// was 1 everywhere). Clear it on a main window: the window's own ground (`ground`) is its fill.
pub fn clear_root(window: gpui_kit::AnyWindowHandle, cx: &mut gpui_kit::App) {
    let _ = window.update(cx, |view, _, cx| {
        if let Ok(root) = view.downcast::<gpui_kit::base::Root>() {
            root.update(cx, |root, cx| {
                root.style().background = Some(transparent_black().into());
                cx.notify();
            });
        }
    });
}

/// The window's colors at one level, light or dark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// The whole window's tint, under the sidebar card and the main pane. Opaque: the veil, solid.
    pub wash: Hsla,
    /// The main pane's ground (`--frost-veil`), over the wash.
    pub veil: Hsla,
    /// The sidebar card's fill (`--frost-panel`), over the wash.
    pub panel: Hsla,
    /// The card's only edge: a 1px ring (`--frost-ring`), and its soft drop.
    pub panel_ring: Hsla,
    pub panel_drop: Hsla,
    /// The fill of a surface that sits on the ground in the main pane: the composer, and the new session's recent
    /// sessions card on an art (the board's Composer: white or #323236). Frost a little under the board's 94%, Glass
    /// clearly see-through, Opaque solid.
    pub surface: Hsla,
    /// A faint 1px ring just inside the window's edge, where the window is see-through.
    pub window_ring: Option<Hsla>,
    /// The surface tiers every other fill goes through (`Palette::frosted` puts them in the palette's fields, so a call
    /// site keeps naming the palette). Opaque: today's solid palette, unchanged. Frost and Glass: see the values in
    /// `Look::at`.
    pub tiers: Tiers,
    pub dark: bool,
}

/// The fills of everything that is not the window, the sidebar card or the composer, at one level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tiers {
    /// A page's own fill where it sits on the ground: a side pane, a page column, a field, a secondary button
    /// (`palette.background`). See-through: barely there, so the ground reads through.
    pub page: Hsla,
    /// A well inside the window: code blocks, terminal output, quiet strips (`palette.sidebar`). The board's Code fill.
    pub well: Hsla,
    /// A card in the window: tool cards, settings groups, a page's cards (`palette.raised`). The board's Card fill.
    pub card: Hsla,
    /// Anything that floats over the page: menus, pickers, popovers, dialogs, toasts, tooltips (`palette.menu`,
    /// `palette.pop`). It covers text, not only the wallpaper. It lies on glass (`workspace::glass::FloatingGround`), so
    /// the text under it blurs; a renderer that cannot blur shows the tint alone, so it stays the heaviest tier. The pop
    /// shadow sets it off.
    pub floating: Hsla,
    /// A small control's fill: round buttons, chips (`palette.control`). The board's Chip fill.
    pub chip: Hsla,
    /// A picked segment or chip (`palette.thumb`): a step above `selected`.
    pub thumb: Hsla,
    /// Hairlines (`palette.border`).
    pub line: Hsla,
}

impl Look {
    /// The look the windows draw now, in `palette`'s theme.
    pub fn now(palette: &Palette) -> Look {
        Look::at(shown(), palette)
    }

    pub fn at(level: impl Into<Frost>, palette: &Palette) -> Look {
        let level = level.into();
        let dark = palette.dark;
        // The board's colors for the ground and the card. Opaque draws them solid, so
        // the white card stands off a grey window as it does on Frost. Fills inside are ink at a low alpha
        // (`palette.rs`), so they keep their contrast on either.
        let (veil, panel) = if dark {
            (0x1C1C1E, 0x2C2C2E)
        } else {
            (0xF4F4F6, 0xFFFFFF)
        };
        // (wash, panel, veil) at each point of `POINTS`. The panel and the veil each lie on the wash, so what the
        // desktop shows through is `(1 - wash) * (1 - panel)` under the sidebar and `(1 - wash) * (1 - veil)` under the
        // pane. Glass (70) matches Cursor's glass window on macOS (3.31, dark, measured from a screenshot over a
        // wallpaper): about 12% of the desktop shows through the sidebar and 4% through the chat, so the glass is a
        // hint of the wallpaper's color, not a view of it. Light, with no screenshot yet, lets a little more through
        // (20% and 6%). Frost (40), the default, is a step more solid (8% / 3% dark, 14% / 4% light), and still shows
        // the blur under the sidebar. The clearest (100) lets about 30% through the sidebar and 10% through the pane.
        let (wash, panel_alpha, veil_alpha) = if dark {
            ([1., 0.70, 0.60, 0.45], [1., 0.73, 0.70, 0.49], [1., 0.90, 0.90, 0.82])
        } else {
            ([1., 0.60, 0.50, 0.40], [1., 0.65, 0.60, 0.45], [1., 0.90, 0.88, 0.84])
        };
        // The board's composer is .94; on a real blurred wallpaper that white box read harsh on the ground, so Frost
        // softens it, light more than dark.
        let surface_alpha = if dark {
            [1., 0.86, 0.65, 0.55]
        } else {
            [1., 0.78, 0.65, 0.55]
        };
        let at = |points: [f32; 4]| along(points, level.see_through);
        let ink = |color: u32, alpha: f32| Hsla::from(rgb(color)).opacity(alpha);
        let (wash_alpha, panel_alpha, veil_alpha, surface_alpha) =
            (at(wash), at(panel_alpha), at(veil_alpha), at(surface_alpha));
        Look {
            tiers: Tiers::at(level, dark),
            wash: ink(veil, wash_alpha),
            veil: ink(veil, veil_alpha),
            panel: ink(panel, panel_alpha),
            surface: ink(if dark { 0x323236 } else { 0xFFFFFF }, surface_alpha),
            panel_ring: if dark {
                rgba(0xFFFFFF0F).into()
            } else {
                rgba(0x0000000D).into()
            },
            panel_drop: if dark {
                rgba(0x00000026).into()
            } else {
                rgba(0x0000000D).into()
            },
            window_ring: (!level.is_opaque() && cfg!(target_os = "linux")).then(|| {
                if dark {
                    rgba(0xFFFFFF14).into()
                } else {
                    rgba(0x00000014).into()
                }
            }),
            dark,
        }
    }

    /// How much of the desktop the sidebar card hides: 1 is solid.
    pub fn sidebar_cover(&self) -> f32 {
        1. - (1. - self.wash.a) * (1. - self.panel.a)
    }

    /// How much of the desktop the main pane hides: 1 is solid.
    pub fn pane_cover(&self) -> f32 {
        1. - (1. - self.wash.a) * (1. - self.veil.a)
    }

    /// `--frost-panel-shadow`: the 1px ring and a 3px drop.
    pub fn panel_shadow(&self) -> Vec<BoxShadow> {
        vec![
            BoxShadow {
                color: self.panel_ring,
                offset: point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(1.),
                inset: false,
            },
            BoxShadow {
                color: self.panel_drop,
                offset: point(px(0.), px(1.)),
                blur_radius: px(3.),
                spread_radius: px(0.),
                inset: false,
            },
        ]
    }
}

impl Tiers {
    /// Opaque is the solid palette as it was before the tiers. Frost takes the board's fills inside a panel (Card white
    /// .72 / white .05, Code black .035 / white .04, Chip black .045 / white .07); Glass keeps the same inks and lets
    /// the cards and the floating things through further. Floating stays near solid on both (.95 / .96, Glass .92 /
    /// .93): it lies over text, which no blur softens; at .88 the text under a picker read through. Up to Frost's
    /// point a see-through window takes Frost's tiers; between Frost and Glass they blend; past Glass they stay Glass's,
    /// since only the ground clears further.
    pub fn at(level: impl Into<Frost>, dark: bool) -> Tiers {
        let level = level.into();
        let solid = crate::palette::Palette::solid_for(dark);
        if level.is_opaque() {
            return Tiers {
                page: solid.background,
                well: solid.sidebar,
                card: solid.raised,
                floating: solid.menu,
                chip: solid.control,
                thumb: solid.thumb,
                line: solid.border,
            };
        }
        let (from, to) = (
            Transparency::Frost.frost().see_through,
            Transparency::Glass.frost().see_through,
        );
        let toward_glass = (f32::from(level.see_through.clamp(from, to) - from)) / f32::from(to - from);
        let white = |alpha: f32| Hsla::from(rgb(0xFFFFFF)).opacity(alpha);
        let black = |alpha: f32| Hsla::from(rgb(0x000000)).opacity(alpha);
        let pick = |frost: f32, glass: f32| blend(frost, glass, toward_glass);
        if dark {
            Tiers {
                page: black(pick(0.14, 0.10)),
                well: white(0.04),
                card: white(pick(0.06, 0.05)),
                floating: Hsla::from(rgb(0x28282C)).opacity(pick(0.96, 0.93)),
                chip: white(0.08),
                thumb: white(0.13),
                line: white(0.08),
            }
        } else {
            Tiers {
                page: white(pick(0.35, 0.22)),
                well: black(0.035),
                card: white(pick(0.72, 0.45)),
                floating: Hsla::from(rgb(0xFAFAFB)).opacity(pick(0.95, 0.92)),
                chip: black(0.05),
                thumb: black(0.10),
                line: black(0.10),
            }
        }
    }
}

/// The see-through values at which `Look::at` names its alphas: Opaque, Frost, Glass and the clearest.
const POINTS: [u8; 4] = [0, 40, 70, 100];

/// A value named at each of `POINTS`, at `see_through`: exactly the named value at a point, a straight line between.
fn along(values: [f32; 4], see_through: u8) -> f32 {
    let see_through = see_through.min(Frost::MAX);
    for ix in 1..POINTS.len() {
        let (low, high) = (POINTS[ix - 1], POINTS[ix]);
        if see_through <= high {
            let t = f32::from(see_through - low) / f32::from(high - low);
            return blend(values[ix - 1], values[ix], t);
        }
    }
    values[POINTS.len() - 1]
}

/// `from` at 0, `to` at 1, a straight line between; exact at both ends.
fn blend(from: f32, to: f32, t: f32) -> f32 {
    if t <= 0. {
        from
    } else if t >= 1. {
        to
    } else {
        from + (to - from) * t
    }
}

/// The fill of something that covers the page as it scrolls or opens over it: a sticky header, a strip, an overlay on
/// the pane. Opaque: the page's own color. See-through: the veil's color at 94%, the board's composer alpha, so what
/// scrolls under it is hidden and the desktop still tints it as it does the ground around it. A solid page color there
/// reads as a band laid over the frost.
pub fn cover(palette: &Palette) -> Hsla {
    let veil = Look::now(palette).veil;
    Hsla {
        a: veil.a.max(COVER_ALPHA),
        ..veil
    }
}

const COVER_ALPHA: f32 = 0.94;

/// The new session's art's ground (`backdrop.rs`): the opaque window's color, or clear under a see-through window, so
/// the art lies on the veil instead of an opaque slab.
pub fn art_ground(palette: &Palette) -> [u8; 4] {
    let rgba = gpui_kit::Rgba::from(Look::now(palette).veil);
    let byte = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    let alpha = if shown().is_opaque() { 255 } else { 0 };
    [byte(rgba.r), byte(rgba.g), byte(rgba.b), alpha]
}

/// The window's ground: the stand-in wallpaper in preview, the wash, and the faint ring at the edge. Put it first in
/// the window's root, and the pane's veil (`pane_ground`) over it.
pub fn ground(look: Look) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            if preview() && !shown().is_opaque() {
                paint_wallpaper(bounds, look.dark, window);
            }
            window.paint_quad(fill(bounds, look.wash));
            if let Some(ring) = look.window_ring {
                window.paint_quad(quad(
                    bounds,
                    Corners::default(),
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
    .size_full()
}

/// The sidebar card: its drop, its fill and its 1px ring just outside it, the card's only edge. A ring drawn as a
/// spread shadow would also darken the see-through fill under it. Put it in a box of the card's size.
pub fn panel(look: Look) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let corners = Corners::all(px(PANEL_RADIUS));
            window.paint_drop_shadows(bounds, corners, &look.panel_shadow()[1..]);
            window.paint_quad(fill(bounds, look.panel).corner_radii(corners));
            window.paint_quad(quad(
                bounds.dilate(px(1.)),
                Corners::all(px(PANEL_RADIUS + 1.)),
                transparent_black(),
                px(1.),
                look.panel_ring,
                BorderStyle::Solid,
            ));
        },
    )
    .size_full()
}

/// `veil` over the whole window except a box the sidebar card's size and shape: a ring as wide as any window, whose
/// inner edge is the card's rounded edge. Put it in a box of the card's size.
pub fn around_panel(veil: Hsla) -> impl IntoElement {
    /// Wider than any window, so the ring's own rounded outer corners stay off screen.
    const REACH: f32 = 20_000.;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            window.paint_quad(quad(
                bounds.dilate(px(REACH)),
                Corners::all(px(PANEL_RADIUS + REACH)),
                transparent_black(),
                px(REACH),
                veil,
                BorderStyle::Solid,
            ));
        },
    )
    .size_full()
}

/// A soft desktop for `NEXUS_FROST_PREVIEW`: a diagonal wash and a few blurred blobs, as a blurred wallpaper looks
/// under the veil.
fn paint_wallpaper(bounds: gpui_kit::Bounds<gpui_kit::Pixels>, dark: bool, window: &mut Window) {
    let (from, to, blobs): (u32, u32, [u32; 3]) = if dark {
        (0x2B3A4E, 0x101820, [0x4A6A8C, 0x6B4E3A, 0x22303F])
    } else {
        (0xC9D3E0, 0x9AA7B0, [0xBCC5D6, 0xE9D9CB, 0x7E8C70])
    };
    window.paint_quad(fill(
        bounds,
        linear_gradient(155., linear_color_stop(rgb(from), 0.), linear_color_stop(rgb(to), 1.)),
    ));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    for (ix, color) in blobs.into_iter().enumerate() {
        let (cx, cy, r) = [(0.22, 0.25, 0.32), (0.78, 0.2, 0.28), (0.62, 0.82, 0.4)][ix];
        let side = r * w.max(h);
        let at = point(
            bounds.origin.x + px(cx * w - side / 2.),
            bounds.origin.y + px(cy * h - side / 2.),
        );
        let blob = gpui_kit::Bounds::new(at, gpui_kit::size(px(side), px(side)));
        window.paint_quad(fill(blob, Hsla::from(rgb(color)).opacity(0.55)).corner_radii(px(side / 2.)));
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    fn palettes() -> [Palette; 2] {
        use crate::palette::ThemeChoice;
        use gpui_kit::WindowAppearance;
        [
            Palette::for_choice(ThemeChoice::Light, WindowAppearance::Light),
            Palette::for_choice(ThemeChoice::Dark, WindowAppearance::Dark),
        ]
    }

    #[test]
    fn the_systems_reduce_transparency_keeps_every_level_opaque() {
        for level in Transparency::ALL
            .map(Transparency::frost)
            .into_iter()
            .chain([Frost::new(85, 0)])
        {
            assert_eq!(level_shown(level, true, false), level);
            assert_eq!(level_shown(level, true, true), Frost::OPAQUE);
            assert_eq!(level_shown(level, false, false), Frost::OPAQUE);
            assert_eq!(level_shown(level, false, true), Frost::OPAQUE);
        }
    }

    #[test]
    fn frost_is_the_default_and_keys_round_trip() {
        assert_eq!(Frost::default(), Transparency::Frost.frost());
        for level in Transparency::ALL {
            assert_eq!(Transparency::from_key(level.key()), Some(level));
            let frost = level.frost();
            assert_eq!(frost.preset(), Some(level));
            assert_eq!(frost.key(), level.key());
            assert_eq!(
                Frost::from_stored(Some(frost.key()), None, None),
                frost,
                "an old file with only the name"
            );
            assert_eq!(Frost::unpack(frost.pack()), Some(frost));
        }
        assert_eq!(Frost::unpack(0), None, "not read yet");
        assert_eq!(Transparency::from_key("clear"), None);
    }

    #[test]
    fn a_custom_choice_round_trips_and_a_bad_file_falls_back_to_frost() {
        let custom = Frost::new(85, 25);
        assert_eq!(custom.preset(), None);
        assert_eq!(custom.key(), CUSTOM);
        assert_eq!(Frost::from_stored(Some(CUSTOM), Some(85), Some(25)), custom);
        assert_eq!(Frost::unpack(custom.pack()), Some(custom));
        // Values past the scale hold at its end; a missing blur is the default's.
        assert_eq!(
            Frost::from_stored(Some(CUSTOM), Some(400), Some(900)),
            Frost::new(100, 100)
        );
        assert_eq!(
            Frost::from_stored(Some(CUSTOM), Some(60), None),
            Frost::new(60, Frost::DEFAULT_BLUR)
        );
        // Custom without its values, an unknown name or nothing: Frost.
        for (key, see_through) in [(Some(CUSTOM), None), (Some("clear"), Some(10)), (None, Some(10))] {
            assert_eq!(Frost::from_stored(key, see_through, Some(10)), Frost::default());
        }
        // Opaque at any blur is Opaque, and keeps its blur in the file.
        assert_eq!(Frost::new(0, 10).preset(), Some(Transparency::Opaque));
        assert_eq!(Frost::new(0, 10).key(), CUSTOM);
    }

    #[test]
    fn the_blur_sets_the_glass_and_at_zero_turns_the_desktops_blur_off() {
        assert_eq!(Transparency::Frost.frost().glass_px(), 16., "today's glass");
        assert_eq!(Frost::new(40, 100).glass_px(), 32.);
        assert_eq!(Frost::new(40, 0).glass_px(), 0.);
        assert_eq!(Frost::new(0, 100).glass_px(), 0., "an opaque window has no glass");
        assert_eq!(
            background_for(Transparency::Frost.frost(), false),
            WindowBackgroundAppearance::Blurred
        );
        assert_eq!(
            background_for(Frost::new(40, 0), false),
            WindowBackgroundAppearance::Transparent
        );
        assert_eq!(background_for(Frost::OPAQUE, false), WindowBackgroundAppearance::Opaque);
        assert_eq!(
            background_for(Transparency::Glass.frost(), true),
            WindowBackgroundAppearance::Opaque,
            "the preview paints its own desktop"
        );
    }

    #[test]
    fn every_value_between_the_points_stays_readable_and_clears_steadily() {
        for palette in palettes() {
            let mut last = Look::at(Frost::OPAQUE, &palette);
            for see_through in 1..=Frost::MAX {
                let look = Look::at(Frost::new(see_through, Frost::DEFAULT_BLUR), &palette);
                assert!(
                    look.pane_cover() >= 0.90 - 1e-4 && look.sidebar_cover() >= 0.66,
                    "{see_through}: pane {} sidebar {}",
                    look.pane_cover(),
                    look.sidebar_cover()
                );
                assert!(look.sidebar_cover() < look.pane_cover(), "{see_through}");
                assert!(
                    look.sidebar_cover() <= last.sidebar_cover() && look.pane_cover() <= last.pane_cover(),
                    "{see_through}: clearer, never more solid"
                );
                assert!(
                    look.tiers.floating.a >= 0.9,
                    "{see_through}: a menu over text still reads"
                );
                last = look;
            }
        }
        // The points draw exactly as the presets did.
        for palette in palettes() {
            assert_eq!(
                Look::at(Frost::new(40, 0), &palette),
                Look::at(Transparency::Frost, &palette),
                "the blur does not change the tints"
            );
        }
    }

    #[test]
    fn opaque_is_solid_and_glass_is_clearer_than_frost() {
        for palette in palettes() {
            let opaque = Look::at(Transparency::Opaque, &palette);
            let frost = Look::at(Transparency::Frost, &palette);
            let glass = Look::at(Transparency::Glass, &palette);
            assert_eq!((opaque.wash.a, opaque.veil.a, opaque.panel.a), (1., 1., 1.));
            // The card stands off the window, and the pane needs no ground of its own over the solid wash.
            assert_ne!(opaque.veil, opaque.panel);
            assert_eq!(opaque.wash, opaque.veil);
            assert!(opaque.window_ring.is_none());
            for cover in [Look::sidebar_cover, Look::pane_cover] {
                assert!(cover(&glass) < cover(&frost) && cover(&frost) < 1.);
            }
            // As in Cursor's glass: the sidebar shows the desktop more than the pane, where the person reads.
            for look in [frost, glass] {
                assert!(look.sidebar_cover() < look.pane_cover());
                // Readable: the pane hides at least 94% of the desktop, the sidebar at least 80%.
                assert!(look.pane_cover() >= 0.94 && look.sidebar_cover() >= 0.8);
            }
        }
    }

    #[test]
    fn the_covers_follow_cursors_glass_window() {
        let cover = |level: Frost, dark: usize| {
            let look = Look::at(level, &palettes()[dark]);
            [look.sidebar_cover(), look.pane_cover()].map(|value| (value * 100.).round() as u8)
        };
        // Light, then dark: [sidebar, pane], in percent hidden.
        assert_eq!(cover(Transparency::Frost.frost(), 0), [86, 96]);
        assert_eq!(cover(Transparency::Frost.frost(), 1), [92, 97]);
        assert_eq!(cover(Transparency::Glass.frost(), 0), [80, 94]);
        assert_eq!(cover(Transparency::Glass.frost(), 1), [88, 96]);
        let clearest = Frost::new(Frost::MAX, Frost::DEFAULT_BLUR);
        assert_eq!(cover(clearest, 0), [67, 90]);
        assert_eq!(cover(clearest, 1), [72, 90]);
    }

    #[test]
    fn the_board_colors_and_the_surface() {
        let [light, dark] = palettes().map(|palette| Look::at(Transparency::Frost, &palette));
        let rgb = |color: Hsla| {
            let c = gpui_kit::Rgba::from(color);
            [c.r, c.g, c.b].map(|v| (v * 255.).round() as u8)
        };
        assert_eq!(
            (rgb(light.wash), rgb(light.veil), rgb(light.panel)),
            ([244, 244, 246], [244, 244, 246], [255; 3])
        );
        assert_eq!(
            (rgb(dark.wash), rgb(dark.veil), rgb(dark.panel)),
            ([28, 28, 30], [28, 28, 30], [44, 44, 46])
        );
        assert_eq!((light.surface.a * 100.).round(), 78.);
        assert_eq!((dark.surface.a * 100.).round(), 86.);
        for palette in palettes() {
            let at = |level| Look::at(level, &palette).surface.a;
            assert!(at(Transparency::Glass) < at(Transparency::Frost) && at(Transparency::Opaque) == 1.);
        }
    }

    #[test]
    fn opaque_tiers_are_the_solid_palette_and_see_through_tiers_are_pinned() {
        let rgba = |color: Hsla| {
            let c = gpui_kit::Rgba::from(color);
            [c.r, c.g, c.b, c.a].map(|v| (v * 255.).round() as u8)
        };
        for dark in [false, true] {
            let solid = Palette::solid_for(dark);
            // Opaque changes nothing.
            let opaque = solid.frosted(Transparency::Opaque);
            assert!(!opaque.frosted);
            assert_eq!(
                [
                    opaque.background,
                    opaque.sidebar,
                    opaque.raised,
                    opaque.menu,
                    opaque.pop,
                    opaque.control,
                    opaque.border
                ],
                [
                    solid.background,
                    solid.sidebar,
                    solid.raised,
                    solid.menu,
                    solid.pop,
                    solid.control,
                    solid.border
                ]
            );
            let tiers = Tiers::at(Transparency::Opaque, dark);
            assert_eq!(
                (tiers.page, tiers.card, tiers.floating),
                (solid.background, solid.raised, solid.menu)
            );
            // Nothing blurs under a floating thing on an opaque window: it is solid.
            assert_eq!(tiers.floating.a, 1.);
            for level in [Transparency::Frost, Transparency::Glass] {
                let tiers = Tiers::at(level, dark);
                let frosted = solid.frosted(level);
                assert!(frosted.frosted);
                assert_eq!(
                    (frosted.raised, frosted.menu, frosted.pop),
                    (tiers.card, tiers.floating, tiers.floating)
                );
                // Text never changes; floating stays the heaviest tier, so a menu over text reads.
                assert_eq!(frosted.foreground, solid.foreground);
                assert!(tiers.floating.a > tiers.card.a && tiers.floating.a >= 0.9);
            }
            let (frost, glass) = (
                Tiers::at(Transparency::Frost, dark),
                Tiers::at(Transparency::Glass, dark),
            );
            assert!(glass.card.a < frost.card.a && glass.floating.a < frost.floating.a && glass.page.a < frost.page.a);
        }
        let [light, dark] = [false, true].map(|dark| Tiers::at(Transparency::Frost, dark));
        assert_eq!(rgba(light.card), [255, 255, 255, 184]);
        assert_eq!(rgba(light.floating), [250, 250, 251, 242]);
        assert_eq!(rgba(light.well), [0, 0, 0, 9]);
        assert_eq!(rgba(dark.card), [255, 255, 255, 15]);
        assert_eq!(rgba(dark.floating), [40, 40, 44, 245]);
        let [light, dark] = [false, true].map(|dark| Tiers::at(Transparency::Glass, dark));
        assert_eq!(rgba(light.card), [255, 255, 255, 115]);
        assert_eq!(rgba(light.floating), [250, 250, 251, 235]);
        assert_eq!(rgba(dark.floating), [40, 40, 44, 237]);
    }
}
