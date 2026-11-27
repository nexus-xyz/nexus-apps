//! CEF on macOS. CEF is a framework there, not a library the binary links: each process loads it at run time from the
//! app bundle, before any other CEF call. The bundle (`packaging/package.sh`) holds:
//!
//! - `Contents/MacOS/nexus-desktop-gpui`: the app, and the web host process.
//! - `Contents/Frameworks/Chromium Embedded Framework.framework`: CEF.
//! - `Contents/Frameworks/nexus-desktop-gpui Helper*.app`: Chromium's helper processes, each holding the small helper
//!   binary (`src/helper.rs`).
//!
//! The web host process also needs an `NSApplication` that implements `CefAppProtocol`: Chromium's message pump asks
//! the application whether it is in the middle of sending an event.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use cef::library_loader::LibraryLoader;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObjectProtocol};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSResponder};
use objc2_foundation::NSObject;

const FRAMEWORK: &str = "Chromium Embedded Framework.framework/Chromium Embedded Framework";

/// Load CEF's framework for this process. `helper` is true in Chromium's helper processes, whose bundles sit in
/// `Contents/Frameworks`. Keep the loader alive while CEF runs: dropping it unloads the framework. `None` when the
/// binary is not in a bundle (a plain `cargo run`) or the framework does not load.
pub(super) fn load_framework(exe: &Path, helper: bool) -> Option<LibraryLoader> {
    let frameworks = exe.parent()?.join(if helper { "../../.." } else { "../Frameworks" });
    if !frameworks.join(FRAMEWORK).exists() {
        eprintln!(
            "nexus web: no CEF framework at {}; web views need the app bundle (packaging/package.sh)",
            frameworks.display()
        );
        return None;
    }
    let loader = LibraryLoader::new(exe, helper);
    loader.load().then_some(loader)
}

static HANDLING_SEND_EVENT: AtomicBool = AtomicBool::new(false);

define_class!(
    /// The web host process's `NSApplication`, with the protocols Chromium's message pump reads.
    #[unsafe(super(NSApplication, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "NexusWebHostApplication"]
    struct HostApplication;

    unsafe impl NSObjectProtocol for HostApplication {}

    unsafe impl CrAppProtocol for HostApplication {
        #[unsafe(method(isHandlingSendEvent))]
        fn is_handling_send_event(&self) -> Bool {
            Bool::new(HANDLING_SEND_EVENT.load(Ordering::Relaxed))
        }
    }

    unsafe impl CrAppControlProtocol for HostApplication {
        #[unsafe(method(setHandlingSendEvent:))]
        fn set_handling_send_event(&self, handling: Bool) {
            HANDLING_SEND_EVENT.store(handling.as_bool(), Ordering::Relaxed);
        }
    }

    unsafe impl CefAppProtocol for HostApplication {}
);

/// Make [`HostApplication`] the process's `NSApplication`, before CEF starts. It has no Dock icon or menu bar: the
/// app's own process shows the window. Call it on the main thread.
pub(super) fn install_application() {
    if MainThreadMarker::new().is_none() {
        eprintln!("nexus web host: not on the main thread; CEF may not start");
        return;
    }
    // SAFETY: `sharedApplication` is a class method of `NSApplication`, which `HostApplication` inherits.
    let app: Retained<NSApplication> = unsafe { msg_send![HostApplication::class(), sharedApplication] };
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
}
