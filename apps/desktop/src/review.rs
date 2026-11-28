//! Opt-in native picker review. Uses fixture data and the production Workspace renderer.

use crate::palette::ThemeChoice;
use crate::workspace::{Workspace, review::Scene};
use gpui_kit::*;
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

static ROOT: OnceLock<PathBuf> = OnceLock::new();
actions!(review, [Connections, Standard, Fast, Light, Dark]);

pub(crate) fn root() -> Option<&'static Path> {
    ROOT.get().map(PathBuf::as_path)
}

pub(crate) fn requested() -> bool {
    std::env::args().any(|arg| arg == "--review-model-picker")
}

pub(crate) fn run() {
    let args: Vec<_> = std::env::args().collect();
    let output = args
        .iter()
        .position(|arg| arg == "--review-output")
        .and_then(|at| args.get(at + 1))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .expect("native review requires --review-output /absolute/path/to/artifacts");
    std::fs::create_dir_all(&output).expect("review output directory");
    let root = output.join("picker-review-state");
    ROOT.set(root.clone()).expect("one review state root");
    let config = root.join("config/nexus-desktop-gpui");
    std::fs::create_dir_all(&config).expect("isolated review config");
    std::fs::write(
        config.join("settings.json"),
        r#"{"theme":"light","transparency":"opaque","automaticUpdates":false,"assistantSounds":false}"#,
    )
    .expect("isolated review settings");
    let _catalog = crate::code_run::fixture_agents(crate::workspace::review::agents());
    nexus_client::install_tls();
    gpui_kit::application()
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            crate::init_ui(cx);
            crate::ui_thread::mark();
            crate::zoom::start(cx);
            crate::chat_width::start(cx);
            cx.bind_keys(crate::key_bindings());
            cx.bind_keys([
                KeyBinding::new("cmd-1", Connections, None),
                KeyBinding::new("cmd-2", Standard, None),
                KeyBinding::new("cmd-3", Fast, None),
                KeyBinding::new("cmd-l", Light, None),
                KeyBinding::new("cmd-d", Dark, None),
            ]);
            cx.on_action(|_: &crate::Quit, cx| cx.quit());
            let mut options = crate::windows::browser_window_options(cx);
            options.window_bounds = Some(WindowBounds::centered(size(px(1400.), px(900.)), cx));
            let (window, workspace) =
                gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Workspace::for_review(window, cx)))
                    .expect("native picker review window");
            bind_scene::<Connections>(window, &workspace, Scene::Connections, cx);
            bind_scene::<Standard>(window, &workspace, Scene::Standard, cx);
            bind_scene::<Fast>(window, &workspace, Scene::Fast, cx);
            bind_theme::<Light>(window, &workspace, ThemeChoice::Light, cx);
            bind_theme::<Dark>(window, &workspace, ThemeChoice::Dark, cx);
            cx.set_menus([Menu::new("Picker review").items([
                MenuItem::action("Connection prompts", Connections),
                MenuItem::action("Standard speed", Standard),
                MenuItem::action("Fast speed", Fast),
                MenuItem::separator(),
                MenuItem::action("Light appearance", Light),
                MenuItem::action("Dark appearance", Dark),
                MenuItem::separator(),
                MenuItem::action("Quit preview", crate::Quit),
            ])]);
            cx.activate(true);
            eprintln!("Native picker review: Cmd+1 connections, Cmd+2 standard, Cmd+3 fast, Cmd+L light, Cmd+D dark.");
            cx.spawn(async move |cx| {
                let control = output.join("native-review-control.json");
                let ready = output.join("native-review-ready.json");
                let mut last = None;
                loop {
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    let command = std::fs::read(&control)
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
                    let revision = command
                        .as_ref()
                        .and_then(|command| command["revision"].as_u64())
                        .unwrap_or(0);
                    if last == Some(revision) {
                        continue;
                    }
                    let result = window.update(cx, |_, window, cx| {
                        workspace.update(cx, |workspace, cx| {
                            if let Some(command) = &command {
                                let scene = match command["scene"].as_str() {
                                    Some("connections") => Some(Scene::Connections),
                                    Some("standard") => Some(Scene::Standard),
                                    Some("fast") => Some(Scene::Fast),
                                    _ => None,
                                };
                                if let Some(scene) = scene {
                                    workspace.review_scene(scene, window, cx);
                                }
                                let theme = match command["theme"].as_str() {
                                    Some("light") => Some(ThemeChoice::Light),
                                    Some("dark") => Some(ThemeChoice::Dark),
                                    _ => None,
                                };
                                if let Some(theme) = theme {
                                    workspace.review_theme(theme, window, cx);
                                }
                                if let Some(fast) = command["fast"].as_bool() {
                                    workspace.review_fast(fast, cx);
                                }
                            }
                        })
                    });
                    if result.is_err() {
                        break;
                    }
                    let settle_ms = if command.as_ref().is_some_and(|command| command["animate"] == true) {
                        10
                    } else {
                        200
                    };
                    cx.background_executor().timer(Duration::from_millis(settle_ms)).await;
                    let Ok(state) = window.update(cx, |_, window, cx| {
                        serde_json::json!({
                            "pid":std::process::id(), "windowId":native_window_number(window), "revision":revision,
                            "fast":workspace.read(cx).review_fast_on(), "quads":window.painted_quads().len()
                        })
                    }) else {
                        break;
                    };
                    if let Err(error) = nexus_desktop_core::atomic_file::write_atomic(
                        &ready,
                        &serde_json::to_vec(&state).expect("review state"),
                    ) {
                        eprintln!("native review capture bridge: {error}");
                        break;
                    }
                    last = Some(revision);
                }
            })
            .detach();
        });
}

fn bind_scene<A: Action>(window: AnyWindowHandle, workspace: &Entity<Workspace>, scene: Scene, cx: &mut App) {
    let workspace = workspace.clone();
    cx.on_action(move |_: &A, cx| {
        if let Err(error) = window.update(cx, |_, window, cx| {
            workspace.update(cx, |workspace, cx| workspace.review_scene(scene, window, cx));
        }) {
            eprintln!("review scene: {error}");
        }
    });
}

fn bind_theme<A: Action>(window: AnyWindowHandle, workspace: &Entity<Workspace>, theme: ThemeChoice, cx: &mut App) {
    let workspace = workspace.clone();
    cx.on_action(move |_: &A, cx| {
        if let Err(error) = window.update(cx, |_, window, cx| {
            workspace.update(cx, |workspace, cx| workspace.review_theme(theme, window, cx));
        }) {
            eprintln!("review theme: {error}");
        }
    });
}

/// Only this preview's NSWindow is exposed to its capture harness; no desktop window enumeration.
#[cfg(target_os = "macos")]
fn native_window_number(window: &Window) -> Option<u32> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    objc2::MainThreadMarker::new()?;
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: GPUI owns this live NSView. This function runs only on its main-thread window update.
    let view: &objc2_app_kit::NSView = unsafe { handle.ns_view.cast::<objc2_app_kit::NSView>().as_ref() };
    Some(view.window()?.windowNumber() as u32)
}

#[cfg(not(target_os = "macos"))]
fn native_window_number(_: &Window) -> Option<u32> {
    None
}
