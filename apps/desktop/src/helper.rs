//! Chromium's helper processes on macOS (renderer, GPU, network and the rest), apart from the app. Chromium starts
//! each from a `nexus-desktop-gpui Helper*.app` beside its framework (`packaging/package.sh`). A copy of the app's
//! binary in each of the four helper apps would add about 200 MB to the bundle, so they hold this binary instead.
//! It only loads CEF's framework and runs the process Chromium asked for.
//!
//! It passes CEF no app. The app's `NexusApp` (`src/web/handlers.rs`) only adds switches to the browser process, and
//! Chromium passes those down to its helpers itself. On Linux and Windows the app's own binary is the helper
//! (`run_subprocess` in `src/web/mod.rs`): there it is one file, not one per helper, and this binary does nothing.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
fn main() {
    use cef::library_loader::LibraryLoader;

    let Ok(exe) = std::env::current_exe() else {
        std::process::exit(1);
    };
    // The helper's bundle sits in `Contents/Frameworks`, beside the framework. Keep the loader alive: dropping it
    // unloads the framework.
    let loader = LibraryLoader::new(&exe, true);
    if !loader.load() {
        std::process::exit(1);
    }
    // Before any other CEF call: it picks the API version this binary was built against.
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let args = cef::args::Args::new();
    let code = cef::execute_process(Some(args.as_main_args()), None::<&mut cef::App>, std::ptr::null_mut());
    drop(loader);
    std::process::exit(code);
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("nexus-desktop-gpui-helper is for macOS; elsewhere the app's own binary runs Chromium's helpers");
    std::process::exit(1);
}
