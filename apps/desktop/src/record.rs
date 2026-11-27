//! Screen recordings for agents: the record shortcut (⌥⇧S, `prefs.rs`), the tray's Record for an Agent, Finish and
//! Cancel (`tray.rs`) and the panel's ⋮ menu start and stop one. The recorder is the `nexus-desktop-record` crate: the
//! overlay, keyframes, transcript, folder and clipboard. macOS only, and off when the organization sets
//! `ScreenCaptureEnabled` to false.
//!
//! The recordings go to the shared data folder (`paths::data_dir`), so `nexus recordings` lists them beside an older
//! Nexus install's. `capture.json` names this build: `desktop/<version>`.
//!
//! The recorder runs on its own threads. What it asks of the app comes here over a channel and runs on GPUI's main
//! thread, which on macOS is AppKit's: the menu-bar item's text, the overlay and the clipboard, and closing the panel.

use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::{App, Global};
use nexus_desktop_record::{Config, Recorder, Shell};

/// The prefix of this build's log lines, and of `--frames`' errors.
const LOG: &str = "nexus-desktop-gpui";

/// What the recorder asks of the app, on GPUI's main thread.
enum Ask {
    /// The text beside the menu-bar item, and its menu rebuilt.
    Status(Option<String>),
    /// AppKit's work: the overlay, the clipboard.
    Main(Box<dyn FnOnce() + Send>),
    HidePanel,
}

struct GpuiShell {
    asks: UnboundedSender<Ask>,
}

impl Shell for GpuiShell {
    fn allowed(&self) -> bool {
        crate::managed::policy().screen_capture_enabled
    }

    fn status(&self, title: Option<&str>) {
        let _ = self.asks.unbounded_send(Ask::Status(title.map(String::from)));
    }

    fn on_main(&self, work: Box<dyn FnOnce() + Send>) {
        let _ = self.asks.unbounded_send(Ask::Main(work));
    }

    fn hide_panel(&self) {
        let _ = self.asks.unbounded_send(Ask::HidePanel);
    }
}

/// The app's recorder.
struct Recording(Recorder);

impl Global for Recording {}

/// `capture.json`'s `app` for this build.
fn app_name() -> String {
    format!("desktop/{}", crate::channel::VERSION)
}

/// Where the recordings go: the shared data folder's `recordings/`.
pub fn folder() -> Option<std::path::PathBuf> {
    crate::paths::data_dir().map(|dir| nexus_desktop_record::folder(&dir))
}

/// Whether this computer records: macOS, and the organization allows it.
pub fn available() -> bool {
    nexus_desktop_record::available(crate::managed::policy().screen_capture_enabled)
}

/// `nexus-desktop-gpui --process-recording <folder>` and `--frames <folder> --from … --to …`: work on a recording,
/// then exit. None when neither was asked for.
pub fn cli() -> Option<i32> {
    nexus_desktop_record::cli(&app_name(), LOG)
}

/// At launch: keep the recorder, and run what it asks on the main thread.
pub fn start(cx: &mut App) {
    let (asks, mut asked) = unbounded();
    let recorder = Recorder::new(
        Arc::new(GpuiShell { asks }),
        Config {
            folder: folder(),
            app: app_name(),
            log: LOG,
        },
    );
    cx.set_global(Recording(recorder));
    cx.spawn(async move |cx| {
        while let Some(ask) = asked.next().await {
            cx.update(|cx| match ask {
                Ask::Status(title) => {
                    crate::tray::set_title(cx, title.as_deref());
                    crate::tray::refresh(cx);
                }
                Ask::Main(work) => work(),
                Ask::HidePanel => crate::panel::hide(cx),
            });
        }
    })
    .detach();
    // Development and checks: a recording `NEXUS_DESKTOP_RECORD` seconds long, five seconds after launch, as the
    // shortcut pressed twice does.
    if let Some(seconds) = std::env::var("NEXUS_DESKTOP_RECORD")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
            cx.update(toggle);
            cx.background_executor()
                .timer(std::time::Duration::from_secs(seconds.max(2)))
                .await;
            cx.update(toggle);
        })
        .detach();
    }
}

/// The shortcut, the tray's rows and the panel's menu: start a recording, or stop the one in progress.
pub fn toggle(cx: &mut App) {
    if let Some(Recording(recorder)) = cx.try_global::<Recording>() {
        recorder.toggle();
    }
}

/// Stop the recording in progress and keep nothing.
pub fn cancel(cx: &mut App) {
    if let Some(Recording(recorder)) = cx.try_global::<Recording>() {
        recorder.cancel();
    }
}

pub fn is_recording(cx: &App) -> bool {
    cx.try_global::<Recording>()
        .is_some_and(|Recording(recorder)| recorder.is_recording())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recordings_go_to_the_shared_data_folder_named_for_this_build() {
        if let Some(data) = crate::paths::data_dir() {
            assert_eq!(folder(), Some(data.join("recordings")));
            assert!(data.ends_with("xyz.nexus.desktop"));
        }
        assert!(app_name().starts_with("desktop/"));
        if !cfg!(target_os = "macos") {
            assert!(!available());
        }
    }

    /// The shell's asks reach the channel, in order, from any thread.
    #[test]
    fn the_recorders_asks_reach_the_main_thread_queue_in_order() {
        let (asks, mut asked) = unbounded();
        let shell: Arc<dyn Shell> = Arc::new(GpuiShell { asks });
        let from = shell.clone();
        std::thread::spawn(move || {
            from.status(Some("● 0:01"));
            from.on_main(Box::new(|| {}));
            from.hide_panel();
            from.status(None);
        })
        .join()
        .expect("the recorder's thread");
        let mut kinds = Vec::new();
        while let Ok(ask) = asked.try_recv() {
            kinds.push(match ask {
                Ask::Status(title) => format!("status {}", title.unwrap_or_default()),
                Ask::Main(work) => {
                    work();
                    "main".into()
                }
                Ask::HidePanel => "hide".into(),
            });
        }
        assert_eq!(kinds, ["status ● 0:01", "main", "hide", "status "]);
    }
}
