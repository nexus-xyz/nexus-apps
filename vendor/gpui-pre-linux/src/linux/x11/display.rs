use std::time::Duration;

use anyhow::Context as _;
use uuid::Uuid;
use x11rb::{
    connection::Connection as _,
    protocol::{randr, randr::ConnectionExt as _, xproto},
    xcb_ffi::XCBConnection,
};

use gpui::{Bounds, DisplayId, Pixels, PlatformDisplay, Size, px};

#[derive(Debug)]
pub(crate) struct X11Display {
    x_screen_index: usize,
    bounds: Bounds<Pixels>,
    uuid: Uuid,
    // Nexus: see `refresh_interval`.
    refresh_interval: Option<Duration>,
}

impl X11Display {
    pub(crate) fn new(
        xcb: &XCBConnection,
        scale_factor: f32,
        x_screen_index: usize,
    ) -> anyhow::Result<Self> {
        let screen = xcb
            .setup()
            .roots
            .get(x_screen_index)
            .with_context(|| format!("No screen found with index {x_screen_index}"))?;
        Ok(Self {
            x_screen_index,
            bounds: Bounds {
                origin: Default::default(),
                size: Size {
                    width: px(screen.width_in_pixels as f32 / scale_factor),
                    height: px(screen.height_in_pixels as f32 / scale_factor),
                },
            },
            uuid: Uuid::from_bytes([0; 16]),
            refresh_interval: crtc_refresh_interval(xcb, screen.root),
        })
    }
}

/// Nexus: the refresh interval of the screen's first lit CRTC, from its RandR mode. The window's refresh timer
/// (`X11ClientState::update_refresh_loop` in `client.rs`) picks the same CRTC, so this is the rate GPUI draws at.
/// `None` without RandR or a lit CRTC.
fn crtc_refresh_interval(xcb: &XCBConnection, root: xproto::Window) -> Option<Duration> {
    let resources = xcb
        .randr_get_screen_resources_current(root)
        .ok()?
        .reply()
        .ok()?;
    resources.crtcs.iter().find_map(|crtc| {
        let info = xcb
            .randr_get_crtc_info(*crtc, x11rb::CURRENT_TIME)
            .ok()?
            .reply()
            .ok()?;
        let mode = resources.modes.iter().find(|mode| mode.id == info.mode)?;
        mode_interval(mode)
    })
}

/// Nexus: a RandR mode's refresh interval, as `mode_refresh_rate` in `client.rs` computes it; `None` for a mode
/// without timings.
fn mode_interval(mode: &randr::ModeInfo) -> Option<Duration> {
    let dots = u64::from(mode.htotal) * u64::from(mode.vtotal);
    if mode.dot_clock == 0 || dots == 0 {
        return None;
    }
    let millihertz = u64::from(mode.dot_clock) * 1_000 / dots;
    (millihertz > 0).then(|| Duration::from_micros(1_000_000_000 / millihertz))
}

impl PlatformDisplay for X11Display {
    // Nexus: an X screen spans every monitor, so upstream reports no rate. This reports the rate the refresh timer
    // draws at (`crtc_refresh_interval`): web pages paint at it (`src/web`).
    fn refresh_interval(&self) -> Option<std::time::Duration> {
        self.refresh_interval
    }

    fn id(&self) -> DisplayId {
        DisplayId::new(self.x_screen_index as u64)
    }

    fn uuid(&self) -> anyhow::Result<Uuid> {
        Ok(self.uuid)
    }

    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }
}
