//! Web pages' frames on the GPU: what every system shares (the switches, and the first-frame watch). The Linux way,
//! over a socket and dma-bufs, is in `linux.rs`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant};

use super::Shared;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(super) use linux::{FD_ENV, UUID_ENV, forget, host_ready, host_start, paint, pair, read_frames, ring_gpu};

/// How long a page asking for GPU frames may go without one, after it opened and after it loaded. A paced page's first
/// frame comes only once its first content is in, so a slow page is no sign of trouble until it has loaded. When
/// Chromium can't share its buffers, it paints none at all.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(8);
const AFTER_LOAD_WAIT: Duration = Duration::from_secs(3);
/// A page with no frame this long after it opened turns GPU frames off, loaded or not: a page that never finishes
/// loading (a long poll, a hung subresource) must not leave a machine that can't share buffers blank.
const HARD_WAIT: Duration = Duration::from_secs(30);
/// A page still loading after this is not watched any more.
const WATCH_FOR: Duration = Duration::from_secs(300);

/// Set once the app failed to import a frame: from then on, pages open with CPU frames.
static OFF: AtomicBool = AtomicBool::new(false);
/// Set at the first GPU frame imported, to say so once in the log.
static ON: AtomicBool = AtomicBool::new(false);

pub(super) fn off() -> bool {
    OFF.load(Ordering::Relaxed)
}

/// Whether new pages should ask for GPU frames: the app's device can import a dma-buf, X11 is there, and nothing
/// turned them off.
pub(super) fn wanted() -> bool {
    !off()
        // Off unless asked for, until it is tested on more GPUs and drivers than one AMD laptop (2026-11-05).
        && std::env::var("NEXUS_WEB_GPU").is_ok_and(|value| value == "1")
        // Chromium shares frames only through its X11 backend (XWayland on Wayland), and paints none at all when
        // asked to on another. So: an X display, and no other backend asked for.
        && std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty())
        && std::env::var("NEXUS_CEF_OZONE").map_or(true, |ozone| ozone == "x11")
        && gpui_wgpu::shared_device().is_some_and(|(device, _)| {
            device
                .features()
                .contains(gpui_wgpu::wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF)
        })
}

/// Turn GPU frames off for the app's life, and open this page again with CPU frames.
fn turn_off(shared: &mut Shared, why: &str) {
    if !OFF.swap(true, Ordering::Relaxed) {
        eprintln!("nexus-desktop-gpui: GPU frames off, pages use CPU frames: {why}");
    }
    shared.gpu_failed = true;
}

/// Page `id` asked for GPU frames: if none came [`FIRST_FRAME_WAIT`] after it opened and [`AFTER_LOAD_WAIT`] after it
/// loaded, or [`HARD_WAIT`] after it opened whether it loaded or not, turn them off.
pub(super) fn watch(id: u32, shared: Weak<Mutex<Shared>>) {
    let opened = Instant::now();
    let _ = std::thread::Builder::new()
        .name("web-gpu-first-frame".into())
        .spawn(move || {
            while opened.elapsed() < WATCH_FOR {
                std::thread::sleep(Duration::from_millis(500));
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                let mut shared = shared.lock().expect("web view state");
                if shared.gpu_framed || shared.stopped.is_some() {
                    return;
                }
                let after_load = shared
                    .loaded
                    .is_some_and(|loaded| opened.elapsed() >= FIRST_FRAME_WAIT && loaded.elapsed() >= AFTER_LOAD_WAIT);
                if after_load || opened.elapsed() >= HARD_WAIT {
                    let loaded = shared.loaded.map_or(" and had not loaded".to_string(), |loaded| {
                        format!(" and {}s after it loaded", loaded.elapsed().as_secs())
                    });
                    let why = format!(
                        "page {id} had no frame {}s after it opened{loaded}",
                        opened.elapsed().as_secs()
                    );
                    turn_off(&mut shared, &why);
                    shared.changed();
                    return;
                }
            }
        });
}
