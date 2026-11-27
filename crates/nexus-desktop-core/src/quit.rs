//! How the desktop app quits, and the Linux checks behind it.
//!
//! - Closing the primary window hides it to the tray. On Linux a desktop can lack a tray host: GNOME shows tray icons
//!   only with an AppIndicator extension. There, closing the last app window quits ([`close_quits`]), since nothing
//!   else would bring the app back or end it.
//! - A terminal's Ctrl+C (SIGINT), SIGTERM and SIGHUP quit the app. The first signal quits cleanly; a second one, or a
//!   clean quit still running after [`SIGNAL_DEADLINE`], ends the process at once ([`on_signal`]).
//! - WebKitGTK's DMA-BUF renderer can freeze a window on NVIDIA's driver: it stops painting and leaves a ghost image.
//!   [`dmabuf_off`] says when the app turns it off before its first webview starts.

use std::path::Path;
use std::time::Duration;

/// The variable WebKitGTK reads at its start to use its older renderer instead of the DMA-BUF one.
pub const DMABUF_VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

/// Whether a tray icon shows on this desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tray {
    /// The icon was made and a tray host shows it.
    Shown,
    /// No icon could be made, or no tray host runs.
    Missing,
    /// Not known yet: the check has not answered.
    Unknown,
}

/// Linux: whether closing an app window quits the app. Only when no tray shows the app and no other app window is
/// open. A check that has not answered counts as no tray, so the person always has a way out.
pub fn close_quits(tray: Tray, other_windows_open: bool) -> bool {
    tray != Tray::Shown && !other_windows_open
}

/// How long a clean quit after a signal may take before the process ends anyway. Above the workbench's own limit (its
/// agents' grace, 5 s and 2 s, plus 1 s) and the sign-out's 5 s.
pub const SIGNAL_DEADLINE: Duration = Duration::from_secs(20);

/// What a quit signal does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnSignal {
    /// Quit as the tray's Quit does: each agent ends its turn or is stopped.
    Quit,
    /// End the process now: a second signal, as a terminal's second Ctrl+C.
    QuitNow,
}

/// What the signal does, given how many quit signals came before it.
pub fn on_signal(earlier: usize) -> OnSignal {
    if earlier == 0 {
        OnSignal::Quit
    } else {
        OnSignal::QuitNow
    }
}

/// The exit code for a process ended by a signal, as a shell reports it (128 + the signal's number).
pub fn signal_exit_code(signal: i32) -> i32 {
    128 + signal
}

/// The quit signals' numbers, the same on Linux and macOS.
pub const SIGHUP: i32 = 1;
pub const SIGINT: i32 = 2;
pub const SIGTERM: i32 = 15;

/// Whether the process started with `signal` ignored, from `/proc/self/status`'s `SigIgn` mask (`status`: that file's
/// text). `nohup` ignores SIGHUP, and a shell ignores SIGINT for a job it starts in the background: the app keeps that
/// choice instead of quitting on them. False when the mask is missing (no `/proc`: macOS).
pub fn ignored(status: &str, signal: i32) -> bool {
    let Some(mask) = status
        .lines()
        .find_map(|line| line.strip_prefix("SigIgn:"))
        .and_then(|mask| u64::from_str_radix(mask.trim(), 16).ok())
    else {
        return false;
    };
    (1..=64).contains(&signal) && mask & (1 << (signal - 1)) != 0
}

/// Whether to turn off WebKitGTK's DMA-BUF renderer: an NVIDIA GPU is in use, and the person did not set the variable
/// themselves (`already_set`: to any value, "0" included, which keeps the renderer on).
pub fn dmabuf_off(already_set: bool, nvidia: bool) -> bool {
    nvidia && !already_set
}

/// Whether NVIDIA's kernel driver is loaded, under `root` (`/` on a real system).
pub fn nvidia_present(root: &Path) -> bool {
    root.join("proc/driver/nvidia").exists() || root.join("sys/module/nvidia").exists()
}

/// Linux: whether a StatusNotifier host runs on the session bus, the part of the desktop that shows tray icons
/// (AppIndicator). False with no session bus, no watcher, or a watcher with no host registered.
#[cfg(target_os = "linux")]
pub fn status_notifier_host() -> bool {
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return false;
    };
    let proxy = zbus::blocking::proxy::Builder::<zbus::blocking::Proxy>::new(&connection)
        .destination("org.kde.StatusNotifierWatcher")
        .and_then(|builder| builder.path("/StatusNotifierWatcher"))
        .and_then(|builder| builder.interface("org.kde.StatusNotifierWatcher"))
        .map(|builder| builder.cache_properties(zbus::proxy::CacheProperties::No))
        .and_then(|builder| builder.build());
    proxy
        .and_then(|proxy| proxy.get_property::<bool>("IsStatusNotifierHostRegistered"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_quits_only_with_no_tray_and_no_other_window() {
        assert!(!close_quits(Tray::Shown, false));
        assert!(!close_quits(Tray::Shown, true));
        assert!(close_quits(Tray::Missing, false));
        assert!(close_quits(Tray::Unknown, false));
        assert!(!close_quits(Tray::Missing, true));
    }

    #[test]
    fn a_second_signal_quits_at_once() {
        assert_eq!(on_signal(0), OnSignal::Quit);
        assert_eq!(on_signal(1), OnSignal::QuitNow);
        assert_eq!(on_signal(5), OnSignal::QuitNow);
        // SIGINT (2), as a shell reports Ctrl+C.
        assert_eq!(signal_exit_code(2), 130);
    }

    #[test]
    fn an_ignored_signal_is_read_from_the_status_mask() {
        // nohup: SIGHUP (bit 0) ignored.
        let status = "Name:\tnexus-desktop\nSigBlk:\t0000000000000000\nSigIgn:\t0000000000000001\nSigCgt:\t0\n";
        assert!(ignored(status, SIGHUP));
        assert!(!ignored(status, SIGINT));
        assert!(!ignored(status, SIGTERM));
        // A background job: SIGINT (bit 1) and SIGQUIT (bit 2) ignored.
        assert!(ignored("SigIgn:\t0000000000000006\n", SIGINT));
        assert!(!ignored("SigIgn:\t0000000000000006\n", SIGHUP));
        assert!(!ignored("", SIGHUP));
        assert!(!ignored("SigIgn:\tzz\n", SIGHUP));
    }

    #[test]
    fn the_deadline_outlasts_a_clean_quit() {
        assert!(SIGNAL_DEADLINE > Duration::from_secs(5 + 2 + 1 + 5));
    }

    #[test]
    fn dmabuf_goes_off_on_nvidia_unless_the_person_chose() {
        assert!(dmabuf_off(false, true));
        assert!(!dmabuf_off(true, true));
        assert!(!dmabuf_off(false, false));
        assert!(!dmabuf_off(true, false));
    }

    #[test]
    fn nvidia_is_found_by_its_driver_folders() {
        let root = std::env::temp_dir().join(format!("nexus-quit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(!nvidia_present(&root));
        std::fs::create_dir_all(root.join("proc/driver/nvidia")).expect("a temp folder");
        assert!(nvidia_present(&root));
        std::fs::remove_dir_all(root.join("proc")).expect("a temp folder");
        std::fs::create_dir_all(root.join("sys/module/nvidia")).expect("a temp folder");
        assert!(nvidia_present(&root));
        let _ = std::fs::remove_dir_all(&root);
    }
}
