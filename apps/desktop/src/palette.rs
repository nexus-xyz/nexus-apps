//! Nexus UI's Brand v5 colors (`ui/src/app/app/brand-v5.css`), light and dark, picked by the window's appearance.
//! Copied by hand for the spike; a shared token file generated for CSS and Rust is the follow-up (README.md).
//!
//! The text, control and fill tones are the design's light and dark token sheet. Fills (hover, selected, the person's messages, a menu's highlight) are black or white ink at a
//! low alpha rather than solid greys, so a row reads the same over the frosted window, the sidebar card or a menu; in
//! Opaque they composite over the solid window to the same tones.

use gpui_kit::{BoxShadow, Hsla, WindowAppearance, point, px, rgb, rgba};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// The chat column (`--background`).
    pub background: Hsla,
    /// The sidebar (`--nx-bg-subtle`).
    pub sidebar: Hsla,
    /// The composer and the person's own messages (`--card`).
    pub raised: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub foreground: Hsla,
    /// Rows that are not the selected one: the sidebar's places, projects and sessions (the board's "Secondary text").
    pub secondary: Hsla,
    /// Sub lines under a row's title. Canon `--nx-app-text-2`; `text_2()` names it that way.
    pub muted: Hsla,
    /// Ages, shortcut hints and group titles. Canon `--nx-app-text-3`; `text_3()` names it that way.
    pub faint: Hsla,
    /// Row icons (`--nx-app-icon`).
    pub icon: Hsla,
    pub border: Hsla,
    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub destructive: Hsla,
    /// A billing status mark (brand v5 token `yel`): someone must act. Billing is amber, never red. Never text.
    pub warning: Hsla,
    /// The keyboard focus ring's colour (`--nx-focus-ring`): the text ink in light, its inverse in dark. Drawn 2 px wide
    /// (`tokens::FOCUS_RING`); use `focus_ring()` for the shadow.
    pub focus_ring: Hsla,
    /// Needs-you dot and drop markers (`--nx-status-info`).
    pub status_info: Hsla,
    /// Running (`--nx-status-running`), the machine dot. `working()` stays the darker spinner tone.
    pub status_running: Hsla,
    /// Waiting (`--nx-status-waiting`). `waiting()` stays the darker tone that keeps 3:1 on the sidebar.
    pub status_waiting: Hsla,
    /// Failed (`--nx-status-failed`): the failed mark and Deny. `destructive` stays the text-safe red.
    pub status_failed: Hsla,
    /// Presence (`--nx-live`).
    pub live: Hsla,
    /// The strongest text: titles, the stop button's fill (`--nx-fg-strong`).
    pub strong: Hsla,
    /// The person's messages (`.msg-user`): the board's soft fill, since a raised white is invisible on the near-white
    /// page.
    pub bubble: Hsla,
    /// Menus (`--v5-menu`) and their highlighted row (`--v5-menu-hover`). Opaque: solid, since nothing blurs under
    /// it (the web's 97% fill sits on a backdrop blur; here the 3% showed the text behind, sharp). See-through: the
    /// floating tier, on glass (`workspace::glass::FloatingGround`).
    pub menu: Hsla,
    pub menu_hover: Hsla,
    /// A floating surface that is not a menu: dialogs, toasts, confirmations, the jump-to-latest button. Opaque: the
    /// raised white or #323236 as before; see-through: the floating tier, as the menus.
    pub pop: Hsla,
    /// A picked chip or segment (`pages::ui::thumb`): a step above `selected`.
    pub thumb: Hsla,
    /// The one fill for round buttons inside a raised box (the composer's `+` and its empty Send) and toast actions
    /// (`--color-control`): the text color at 8% over `raised`, baked so nothing mixes at paint time. It reads over
    /// the menu fill too.
    #[allow(dead_code)]
    pub control: Hsla,
    /// The dim over the window behind the search palette (`.nx-palette-overlay` in v5).
    pub scrim: Hsla,
    /// Whether the surfaces are at a see-through level's tiers (`frosted`): a fill that must stay solid on an opaque
    /// window picks its see-through form from this.
    pub frosted: bool,
    pub dark: bool,
}

/// The person's theme choice, as the profile menu's System / Light / Dark control sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    /// The choice saved on this computer (`theme` in this build's settings file); System when none is.
    pub fn saved() -> ThemeChoice {
        match crate::paths::app_settings().theme.as_deref() {
            Some("light") => ThemeChoice::Light,
            Some("dark") => ThemeChoice::Dark,
            _ => ThemeChoice::System,
        }
    }
}

impl Palette {
    /// The palette the first window will most likely draw with, before it opens: the saved choice, and for System the
    /// appearance the app saw last (`appearance.rs`).
    pub fn expected_at_launch() -> Palette {
        let appearance = match crate::paths::last_appearance_dark() {
            Some(true) => WindowAppearance::Dark,
            _ => WindowAppearance::Light,
        };
        Palette::for_choice(ThemeChoice::saved(), appearance)
    }

    /// The palette for the choice, at the Transparency level the main windows draw (`frost::shown`); System follows
    /// the window's appearance. A window that stays opaque (the panel, a detached window, a theme's preview) takes
    /// `solid()` of it.
    pub fn for_choice(choice: ThemeChoice, appearance: WindowAppearance) -> Palette {
        Palette::solid_for(Palette::is_dark(choice, appearance)).frosted(crate::frost::shown())
    }

    /// The solid palette, as an opaque window draws it.
    pub fn solid_for(dark: bool) -> Palette {
        if dark { Palette::dark() } else { Palette::light() }
    }

    /// This palette's theme, solid.
    pub fn solid(self) -> Palette {
        Palette::solid_for(self.dark)
    }

    /// The palette with its surfaces at `level`'s tiers (`frost::Tiers`): pages, wells, cards, floating things,
    /// controls and hairlines all follow the Transparency setting from here, so a call site keeps naming the field.
    /// Opaque leaves it as it is. Text, icon and state colors never change.
    pub fn frosted(self, level: impl Into<crate::frost::Frost>) -> Palette {
        let level = level.into();
        if level.is_opaque() {
            return self;
        }
        let tiers = crate::frost::Tiers::at(level, self.dark);
        Palette {
            background: tiers.page,
            sidebar: tiers.well,
            raised: tiers.card,
            menu: tiers.floating,
            pop: tiers.floating,
            control: tiers.chip,
            thumb: tiers.thumb,
            border: tiers.line,
            frosted: true,
            ..self
        }
    }

    pub fn is_dark(choice: ThemeChoice, appearance: WindowAppearance) -> bool {
        match choice {
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
            ThemeChoice::System => matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark),
        }
    }

    fn light() -> Palette {
        Palette {
            background: rgb(0xFBFBFC).into(),
            sidebar: rgb(0xF2F2F4).into(),
            raised: rgb(0xFFFFFF).into(),
            // The fills are ink at a low alpha, so they read the same over the frosted window, a card or a menu.
            hover: rgba(0x00000009).into(),
            selected: rgba(0x0000000D).into(),
            foreground: rgb(0x1C1C1E).into(),
            secondary: rgb(0x2C2C2E).into(),
            muted: rgb(0x5C5C60).into(),
            faint: rgb(0x676767).into(),
            icon: rgb(0x6E6E73).into(),
            border: rgb(0xDCDCDF).into(),
            primary: rgb(0x111113).into(),
            primary_foreground: rgb(0xFFFFFF).into(),
            destructive: rgb(0x961C27).into(),
            warning: rgb(0xC79A00).into(),
            focus_ring: rgb(0x111113).into(),
            status_info: rgb(0x0A84FF).into(),
            status_running: rgb(0x30D158).into(),
            status_waiting: rgb(0xFFD60A).into(),
            status_failed: rgb(0xFF453A).into(),
            live: rgb(0x30D158).into(),
            strong: rgb(0x111113).into(),
            bubble: rgba(0x0000000D).into(),
            menu: rgb(0xFAFAFB).into(),
            menu_hover: rgba(0x0000000D).into(),
            pop: rgb(0xFFFFFF).into(),
            thumb: rgb(0xDCDCE0).into(),
            control: rgb(0xEDEDED).into(),
            // 12% black: the palette and the other modals share it, and 24% read grey.
            scrim: rgba(0x0000001F).into(),
            frosted: false,
            dark: false,
        }
    }

    fn dark() -> Palette {
        Palette {
            background: rgb(0x18181A).into(),
            sidebar: rgb(0x1C1C1E).into(),
            raised: rgb(0x323236).into(),
            hover: rgba(0xFFFFFF0D).into(),
            selected: rgba(0xFFFFFF14).into(),
            foreground: rgb(0xF2F2F4).into(),
            secondary: rgb(0xE5E5EA).into(),
            muted: rgb(0xA1A1A6).into(),
            faint: rgb(0x919191).into(),
            icon: rgb(0x8E8E93).into(),
            border: rgb(0x2E2E30).into(),
            primary: rgb(0xF2F2F4).into(),
            primary_foreground: rgb(0x111113).into(),
            destructive: rgb(0xFF8A84).into(),
            warning: rgb(0xFFD60A).into(),
            focus_ring: rgb(0xF5F5F7).into(),
            status_info: rgb(0x0A84FF).into(),
            status_running: rgb(0x30D158).into(),
            status_waiting: rgb(0xFFD60A).into(),
            status_failed: rgb(0xFF453A).into(),
            live: rgb(0x30D158).into(),
            strong: rgb(0xFFFFFF).into(),
            bubble: rgba(0xFFFFFF14).into(),
            menu: rgb(0x28282C).into(),
            menu_hover: rgba(0xFFFFFF14).into(),
            pop: rgb(0x323236).into(),
            thumb: rgb(0x3A3A3D).into(),
            control: rgb(0x313133).into(),
            scrim: rgba(0x18181A80).into(),
            frosted: false,
            dark: true,
        }
    }
}

impl Palette {
    /// A route's hue (`--nx-route-1`…`5`: Ice, Apricot, Mint, Iris, Orchid), the same in light and dark. Route 0, the
    /// lead's, wears Ice; past 5 the hues start again (`crew_route::route_of`).
    pub fn route(n: usize) -> Hsla {
        const ROUTES: [u32; 5] = [0x68CCF5, 0xF1AD71, 0x6FD5B0, 0xA9B8FF, 0xDCA7EC];
        rgb(ROUTES[n.saturating_sub(1) % ROUTES.len()]).into()
    }

    /// Done's dot (`--nx-success`).
    pub fn success() -> Hsla {
        rgb(0x6FD5B0).into()
    }

    /// Something works now: a session's spinner, the composer's working line and ring. Ice, the lead's hue, darkened
    /// in light so the mark keeps 3:1 against the sidebar and the composer.
    pub fn working(&self) -> Hsla {
        if self.dark {
            Palette::route(1)
        } else {
            rgb(0x1D8FCB).into()
        }
    }

    /// Something waits on the person: a session's mark and the composer's line and ring. Amber, darkened in light for
    /// 3:1 against the sidebar and the composer.
    pub fn waiting(&self) -> Hsla {
        rgb(if self.dark { 0xFFC53D } else { 0xB07800 }).into()
    }

    /// Canon `--nx-app-text-2`: help copy and sub lines. Same tone as `muted`.
    pub fn text_2(&self) -> Hsla {
        self.muted
    }

    /// Canon `--nx-app-text-3`: ages, hints, footers. Same tone as `faint`.
    pub fn text_3(&self) -> Hsla {
        self.faint
    }

    /// Canon `--nx-near-limit`: the usage meter near its limit. The same amber as `warning`.
    pub fn near_limit(&self) -> Hsla {
        self.warning
    }

    /// Blocked's hollow ring (`--color-metal-steel`).
    pub fn steel(&self) -> Hsla {
        rgb(if self.dark { 0x8A8A8E } else { 0x6E6E73 }).into()
    }

    /// A control's hairline ring (Stop, attach): pure white in dark and black in light at low opacity, so it composites
    /// over any surface, the composer's frosted glass included, where a solid hairline is tuned to one background.
    /// `strong`: hovered.
    pub fn control_ring(&self, strong: bool) -> Hsla {
        let ink = if self.dark {
            gpui_kit::white()
        } else {
            gpui_kit::black()
        };
        ink.opacity(match (self.dark, strong) {
            // As the solid hairline over the composer's fill; any brighter and a white ring glows in dark.
            (true, false) => 0.07,
            (true, true) => 0.12,
            (false, false) => 0.09,
            (false, true) => 0.14,
        })
    }

    /// The keyboard focus ring (`--nx-focus-ring`): a 2 px ring outside the box, no blur. Add it to the focused box's
    /// shadows.
    pub fn focus_ring(&self) -> BoxShadow {
        BoxShadow {
            color: self.focus_ring,
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(crate::tokens::FOCUS_RING),
            inset: false,
        }
    }

    /// A control's translucent fill, for the same reason as `control_ring`: hovered, or `pressed` (and open).
    pub fn control_fill(&self, pressed: bool) -> Hsla {
        let ink = if self.dark {
            gpui_kit::white()
        } else {
            gpui_kit::black()
        };
        ink.opacity(match (self.dark, pressed) {
            (true, false) => 0.06,
            (true, true) => 0.1,
            (false, false) => 0.045,
            (false, true) => 0.08,
        })
    }

    /// The ring around a popover, with no hairline look: none in dark, where the shadow and the lighter fill already
    /// set it off, and a 6% black edge in light, where a white menu on a white page needs one.
    pub fn pop_ring(&self) -> Hsla {
        if self.dark {
            gpui_kit::transparent_black()
        } else {
            gpui_kit::black().opacity(0.06)
        }
    }

    /// A menu's 1px outline, drawn inside its box (`border_1`): the border at 80% in light, the border in dark.
    pub fn menu_ring(&self) -> Hsla {
        if self.dark {
            self.border
        } else {
            self.border.opacity(0.8)
        }
    }

    /// `--shadow-pop` alone, for a box whose outline is its own border (menus): no ring outside the box.
    pub fn pop_drop(&self) -> Vec<BoxShadow> {
        self.pop_shadow(gpui_kit::transparent_black()).split_off(1)
    }

    /// `--nx-shadow-pop`, under a 1px ring in `ring`: popovers and the palette.
    pub fn pop_shadow(&self, ring: Hsla) -> Vec<BoxShadow> {
        let shadow = |alpha: f32, y: f32, blur: f32, spread: f32| BoxShadow {
            color: gpui_kit::black().opacity(alpha),
            offset: point(px(0.), px(y)),
            blur_radius: px(blur),
            spread_radius: px(spread),
            inset: false,
        };
        let mut shadows = vec![BoxShadow {
            color: ring,
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(1.),
            inset: false,
        }];
        if self.dark {
            shadows.extend([shadow(0.7, 16., 40., -12.), shadow(0.4, 2., 6., -2.)]);
        } else {
            shadows.extend([shadow(0.18, 12., 32., -12.), shadow(0.06, 2., 6., -2.)]);
        }
        shadows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(c: Hsla) -> u32 {
        let rgba = c.to_rgb();
        let ch = |v: f32| (v * 255.).round() as u32;
        (ch(rgba.r) << 16) | (ch(rgba.g) << 8) | ch(rgba.b)
    }

    /// The canon tokens (`product-web/ds/tokens.css`) the desktop shares, pinned so a drift shows here.
    #[test]
    fn canon_tones_match_tokens_css() {
        let (l, d) = (Palette::light(), Palette::dark());
        assert_eq!((hex(l.text_2()), hex(d.text_2())), (0x5C5C60, 0xA1A1A6));
        assert_eq!((hex(l.text_3()), hex(d.text_3())), (0x676767, 0x919191));
        assert_eq!((hex(l.icon), hex(d.icon)), (0x6E6E73, 0x8E8E93));
        assert_eq!((hex(l.focus_ring), hex(d.focus_ring)), (0x111113, 0xF5F5F7));
        for p in [l, d] {
            assert_eq!(hex(p.status_info), 0x0A84FF);
            assert_eq!(hex(p.status_running), 0x30D158);
            assert_eq!(hex(p.status_waiting), 0xFFD60A);
            assert_eq!(hex(p.status_failed), 0xFF453A);
            assert_eq!(hex(p.live), 0x30D158);
        }
        assert_eq!((hex(l.near_limit()), hex(d.near_limit())), (0xC79A00, 0xFFD60A));
    }

    #[test]
    fn focus_ring_is_two_px_and_unblurred() {
        let ring = Palette::light().focus_ring();
        assert_eq!(ring.spread_radius, px(2.));
        assert_eq!(ring.blur_radius, px(0.));
    }

    #[test]
    fn opaque_menus_and_pops_are_solid() {
        // Nothing blurs under a menu on an opaque window, so any see-through fill shows the text behind it, sharp.
        for dark in [false, true] {
            let palette = Palette::solid_for(dark);
            assert_eq!(palette.menu.a, 1., "dark {dark}: the menu fill is see-through");
            assert_eq!(palette.pop.a, 1., "dark {dark}: the pop fill is see-through");
        }
    }
}
