//! `NEXUS_WEB_SELFTEST=surface` on Windows: each web view opens no page and draws moving stripes instead. They are a
//! BGRA texture made here, drawn as a surface by GPUI's D3D11 renderer (`vendor/gpui-pre-windows/src/shared_texture.rs`).
//! It tests the app's half of GPU frames on real hardware before the web host shares Chromium's textures.
//!
//! The picture is `selftest_pattern.rs`'s, the same as on macOS: moving colour bars, a band of black and white checks,
//! and colour swatches. Judge it from a full-resolution screenshot with `web-tests/selftest/check.py`: it says whether
//! the checks are exact (not stretched, not between pixels) and whether red and blue are swapped.
//!
//! The textures come from a pool. A texture is written again (`UpdateSubresource`) only after its owner dropped, as a
//! ring's slots will be. With `NEXUS_WEB_TRACE=1`, a line every 2s gives the owners dropped per second: about the
//! frame rate when the renderer lets each frame's textures go once the GPU drew it.
//!
//! The app has no trace helper yet. `write_line` writes to the file `NEXUS_WEB_TRACE_FILE` names, when set (a shipped
//! Windows build has no console), else to stderr. Only this self-test uses it.
//!
//! Threads: GPUI paints and draws on the main thread, so `UpdateSubresource` on the immediate context here never runs
//! at the same time as the renderer's use of it.

use std::cell::RefCell;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::*;
use gpui_windows::{SharedTexture, shared_device};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

use super::selftest_pattern::fill;

/// Textures in the pool at most. When all are in use, the last frame shows again.
const SLOTS: usize = 6;
/// The bars' speed, in device pixels a second.
const SPEED: f32 = 240.;
/// How often the trace line comes.
const TRACE_EVERY: Duration = Duration::from_secs(2);

/// Owners dropped since the app started.
static DROPS: AtomicU64 = AtomicU64::new(0);

/// Whether the self-test is on: `NEXUS_WEB_SELFTEST=surface`.
pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("NEXUS_WEB_SELFTEST").is_ok_and(|value| value == "surface"))
}

/// Paint the stripes over `bounds`, in place of the page, and ask for the next frame. `place` gives where a frame of
/// `width` by `height` device pixels goes, from a whole device pixel. False when the self-test is off.
pub(super) fn paint(bounds: Bounds<Pixels>, place: &impl Fn(u32, u32) -> Bounds<Pixels>, window: &mut Window) -> bool {
    if !on() {
        return false;
    }
    window.request_animation_frame();
    let scale = window.scale_factor();
    let device_pixels = |value: Pixels| (f32::from(value) * scale).round().clamp(1., 8192.) as u32;
    let size = (device_pixels(bounds.size.width), device_pixels(bounds.size.height));
    match STRIPES.with_borrow_mut(|stripes| next_frame(stripes, size)) {
        Ok(texture) => {
            let frame = place(size.0, size.1);
            // A surface is not clipped to the canvas as an image is: clip it here.
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                window.paint_surface(frame, texture);
            });
        }
        Err(error) => report(&format!("{error:#}")),
    }
    true
}

thread_local! {
    /// The pool and its counts. Main thread only, where GPUI paints.
    static STRIPES: RefCell<Option<Stripes>> = const { RefCell::new(None) };
}

/// The textures of one device and size, and the counts for the trace.
struct Stripes {
    device: ID3D11Device,
    size: (u32, u32),
    slots: Vec<Slot>,
    pixels: Vec<u8>,
    /// The newest frame, shown again when every texture is in use.
    last: Option<Arc<SharedTexture>>,
    meter: Meter,
}

struct Slot {
    texture: ID3D11Texture2D,
    /// Set while a frame of it is out; its owner clears it.
    busy: Arc<AtomicBool>,
}

/// Kept with each frame's texture: the renderer drops it once the GPU drew the frame.
struct Owner(Arc<AtomicBool>);

impl Drop for Owner {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
        DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

struct Meter {
    started: Instant,
    since: Instant,
    frames: u64,
    reused: u64,
    drops_before: u64,
}

impl Default for Meter {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            since: Instant::now(),
            frames: 0,
            reused: 0,
            drops_before: DROPS.load(Ordering::Relaxed),
        }
    }
}

fn next_frame(stripes: &mut Option<Stripes>, size: (u32, u32)) -> anyhow::Result<Arc<SharedTexture>> {
    let (device, luid) = shared_device().context("surface selftest: GPUI has no D3D11 device to share")?;
    if stripes
        .as_ref()
        .is_none_or(|stripes| stripes.device != device || stripes.size != size)
    {
        if tracing() {
            write_line(&format!(
                "nexus web trace: surface selftest: device LUID {:08x}:{:08x}, {}x{} px",
                luid.HighPart, luid.LowPart, size.0, size.1
            ));
        }
        let meter = stripes.take().map(|stripes| stripes.meter).unwrap_or_default();
        *stripes = Some(Stripes {
            device,
            size,
            slots: Vec::new(),
            pixels: vec![0; size.0 as usize * size.1 as usize * 4],
            last: None,
            meter,
        });
    }
    let stripes = stripes.as_mut().context("surface selftest: no pool")?;
    stripes.meter.frames += 1;
    stripes.trace();
    let free = stripes.slots.iter().position(|slot| !slot.busy.load(Ordering::Acquire));
    let index = match free {
        Some(index) => index,
        None if stripes.slots.len() < SLOTS => {
            let texture = make_texture(&stripes.device, size)?;
            stripes.slots.push(Slot {
                texture,
                busy: Arc::new(AtomicBool::new(false)),
            });
            stripes.slots.len() - 1
        }
        None => {
            stripes.meter.reused += 1;
            return stripes
                .last
                .clone()
                .context("surface selftest: every texture is in use");
        }
    };
    let offset = (stripes.meter.started.elapsed().as_secs_f32() * SPEED) as usize;
    fill(
        &mut stripes.pixels,
        size.0 as usize * 4,
        size.0 as usize,
        size.1 as usize,
        offset,
    );
    let slot = &stripes.slots[index];
    let context = unsafe { stripes.device.GetImmediateContext() }.context("surface selftest: immediate context")?;
    unsafe {
        context.UpdateSubresource(&slot.texture, 0, None, stripes.pixels.as_ptr().cast(), size.0 * 4, 0);
    }
    let texture = SharedTexture::new(&slot.texture)?;
    slot.busy.store(true, Ordering::Release);
    let texture = Arc::new(texture.with_owner(Owner(slot.busy.clone())));
    stripes.last = Some(texture.clone());
    Ok(texture)
}

impl Stripes {
    /// Every [`TRACE_EVERY`], with `NEXUS_WEB_TRACE=1`: owners dropped and frames painted per second.
    fn trace(&mut self) {
        let elapsed = self.meter.since.elapsed();
        if elapsed < TRACE_EVERY {
            return;
        }
        let drops = DROPS.load(Ordering::Relaxed);
        if tracing() {
            let seconds = elapsed.as_secs_f64();
            let busy = self
                .slots
                .iter()
                .filter(|slot| slot.busy.load(Ordering::Relaxed))
                .count();
            write_line(&format!(
                "nexus web trace: surface selftest: owner drops/s {:.1}, frames/s {:.1}, textures {} ({busy} in use), \
                 frames shown again {}",
                (drops - self.meter.drops_before) as f64 / seconds,
                self.meter.frames as f64 / seconds,
                self.slots.len(),
                self.meter.reused,
            ));
        }
        self.meter.since = Instant::now();
        self.meter.frames = 0;
        self.meter.reused = 0;
        self.meter.drops_before = drops;
    }
}

fn make_texture(device: &ID3D11Device, (width, height): (u32, u32)) -> anyhow::Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }.context("surface selftest: a texture")?;
    texture.context("surface selftest: no texture")
}

fn tracing() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("NEXUS_WEB_TRACE").is_ok_and(|value| value == "1"))
}

/// A problem, once per distinct message, traced or not.
fn report(problem: &str) {
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let Ok(mut seen) = SEEN.lock() else {
        return;
    };
    if !seen.iter().any(|seen| seen == problem) {
        seen.push(problem.to_string());
        write_line(&format!("nexus-desktop-gpui: {problem}"));
    }
}

/// A line to `NEXUS_WEB_TRACE_FILE` (appended) when it is set, else to stderr.
fn write_line(line: &str) {
    static FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    let file = FILE.get_or_init(|| {
        let path = std::env::var_os("NEXUS_WEB_TRACE_FILE").filter(|path| !path.is_empty())?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(Mutex::new)
    });
    match file {
        Some(file) => {
            if let Ok(mut file) = file.lock() {
                let _ = writeln!(file, "{line}");
            }
        }
        None => eprintln!("{line}"),
    }
}
