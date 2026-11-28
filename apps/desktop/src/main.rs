//! Nexus Desktop on GPUI: the app drawn natively by GPUI over `nexus-client`. README.md says what it covers.

// A shipped Windows build opens no console window beside the app's own.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod activity;
mod agent;
mod agents;
mod app_id;
mod app_socket;
mod appearance;
mod artifact_refs;
mod assets;
mod attachments;
mod backend;
mod canon_keys;
mod catalog;
mod channel;
mod chat_bar;
mod chat_width;
mod cloud;
mod code;
mod code_media;
mod code_run;
mod command_tray;
mod computer_name;
mod connector_gap;
mod crash;
mod crew_route;
mod desktop_settings;
mod dictation;
mod docs;
mod editor_choice;
mod external_link;
mod feedback;
mod file_icons;
mod frame_prefetch;
mod frost;
mod gateway;
mod github_cli;
#[cfg(test)]
mod guest;
mod http;
mod i18n;
mod inbox;
mod internal_access;
mod keep_awake;
mod key_bindings;
mod keychain;
mod launch;
mod layout_dump;
mod library;
mod limit_stop;
mod links;
mod live_fields;
mod local_providers_opt_in;
mod local_store;
mod machine_view;
mod machines;
mod managed;
mod mcp;
mod measure;
mod microphones;
mod model;
mod motion;
mod notify;
mod onboarding;
mod os_motion;
mod outbox;
mod outside;
mod owner;
mod pages;
mod palette;
mod panel;
mod panel_motion;
mod path_notices;
mod paths;
mod pay_notice;
mod plan_book;
mod pr_watch;
mod prefs;
mod project_commands;
mod project_groups;
mod pull_page;
mod record;
mod redact;
mod revoked;
mod rich_text;
mod secret_request;
mod selection;
mod service;
mod settings;
mod settings_fields;
mod setup;
mod shortcut_label;
mod side;
mod single_instance;
mod snooze;
mod sounds;
#[cfg(unix)]
mod stdio;
mod stream;
mod title_bar;
mod tokens;
mod touch_id;
mod tray;
mod ui_thread;
mod update_recovery;
mod updates;
mod vault_device;
#[cfg(test)]
mod vendor_names_tests;
mod voice;
mod wakeups;
mod warm_cache;
mod watchdog;
mod web;
mod windows;
mod workspace;
mod zoom;

use gpui_kit::*;

use crate::workspace::{
    AskAboutNexus, FindChat, NewErrand, NewTerminal, OpenAssistant, OpenSettings, StashPrompt, SwitchAccount,
    ToggleChatBrowser, ToggleSidebar,
};
#[cfg(test)]
use crate::workspace::{GoBack, GoForward, ToggleMaximizeWorkbench, ToggleSidePane, ToggleTerminal};

actions!(nexus, [Quit]);

/// Cmd+`key` on macOS, Ctrl+`key` elsewhere. GPUI's `secondary-` needs the OS at parse time, which
/// `KeyBinding::new` does not pass.
macro_rules! shortcut {
    ($key:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $key)
        } else {
            concat!("ctrl-", $key)
        }
    };
}

/// Back and forward in the history: the keys `BACK_HINT` and `FORWARD_HINT` show (`workspace/mod.rs`).
const GO_BACK: &str = if cfg!(target_os = "macos") { "cmd-[" } else { "alt-left" };
const GO_FORWARD: &str = if cfg!(target_os = "macos") {
    "cmd-]"
} else {
    "alt-right"
};

/// The app's keys. The Workbench's (`workspace/side_pane.rs`) take Ctrl on every platform, as Cursor and VS Code's
/// terminal keys: Cmd+` switches windows on macOS. The ones a person can change in Settings › Keyboard shortcuts come
/// from `key_bindings.rs`, with the keys they set.
fn key_bindings() -> Vec<KeyBinding> {
    key_bindings_with(canon_keys::enabled())
}

/// [`key_bindings`], with the canon's new keys (`canon_keys.rs`) in or out.
fn key_bindings_with(canon: bool) -> Vec<KeyBinding> {
    let mut bindings = vec![
        KeyBinding::new(shortcut!("q"), Quit, None),
        // Another app window, from any window or field.
        KeyBinding::new(shortcut!("shift-n"), windows::NewWindow, None),
        KeyBinding::new(shortcut!("b"), ToggleSidebar, Some("Workspace")),
        KeyBinding::new(shortcut!("k"), FindChat, Some("Workspace")),
        // In the Workbench's Changes, its own find (`side/changes.rs`), as the Nexus app's `changes-find-view.ts`.
        KeyBinding::new(shortcut!("f"), side::changes::FindInChanges, Some("Changes")),
        // Park the draft, or bring a stashed one back (`workspace/stash.rs`), as the Nexus app's `stash-prompt`.
        KeyBinding::new(shortcut!("s"), StashPrompt, Some("Workspace")),
        // In an edited file in Files, save it instead (`side/file_view.rs`): the deeper context wins.
        KeyBinding::new(shortcut!("s"), side::file_view::SaveFile, Some("FileView")),
        // Shift+` is ~ on most layouts, and some platforms report the shifted key.
        KeyBinding::new(shortcut_label::NEW_TERMINAL, NewTerminal, Some("Workspace")),
        KeyBinding::new("ctrl-~", NewTerminal, Some("Workspace")),
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-shift-b"
            } else {
                "ctrl-shift-b"
            },
            ToggleChatBrowser,
            Some("Workspace"),
        ),
        KeyBinding::new(shortcut!(","), OpenSettings, Some("Workspace")),
        // The Assistant, as its sidebar row's hint shows (`workspace/sidebar.rs`).
        KeyBinding::new(shortcut!("j"), OpenAssistant, Some("Workspace")),
        // A new errand, as ⌘K's New errand row shows (`workspace/assistant_errands.rs`).
        KeyBinding::new(shortcut!("shift-j"), NewErrand, Some("Workspace")),
        // The account switcher, as its last row shows (`workspace/org_menu.rs`).
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "ctrl-cmd-o"
            } else {
                "ctrl-alt-o"
            },
            SwitchAccount,
            Some("Workspace"),
        ),
    ];
    if canon {
        bindings.extend([
            // ⌃⌘N is the canon's key (nav.yaml) for another window. Off a Mac, Ctrl+Alt is AltGr, so Ctrl+Shift+N is
            // the one key.
            #[cfg(target_os = "macos")]
            KeyBinding::new("ctrl-cmd-n", windows::NewWindow, None),
            // Ask about Nexus (the Assistant). Cmd+/ stays "Pick the model", the Nexus app's key (`key_bindings.rs`).
            // Shift+/ is ? where a platform reports the shifted key.
            KeyBinding::new(shortcut!("shift-/"), AskAboutNexus, Some("Workspace")),
            KeyBinding::new(shortcut!("shift-?"), AskAboutNexus, Some("Workspace")),
        ]);
    }
    bindings.extend(key_bindings::bindings(&key_bindings::current()));
    bindings
}

fn main() {
    // `npm run measure`'s first mark, the process start (`measure.rs`).
    measure::start();
    // Chromium's helper processes, the web host process, Code sessions' built-in MCP server and the launcher of an
    // Antigravity session's servers, the background service that keeps agents running (`service.rs`) and the work on
    // a screen recording (`--process-recording`, `--frames`, `record.rs`) are this binary too: they stop here.
    if let Some(code) = web::run_subprocess()
        .or_else(web::run_host)
        .or_else(mcp::serve)
        .or_else(nexus_desktop_mcp::browser::serve)
        .or_else(nexus_desktop_mcp::launch::serve)
        .or_else(service::serve)
        .or_else(record::cli)
    {
        std::process::exit(code);
    }
    // Before anything prints: a print to a stderr whose reader is gone panics the app (`stdio.rs`).
    #[cfg(unix)]
    stdio::guard();
    // First in the app's own process: the version the core reports, and a panic anywhere after is kept for the next
    // start's crash prompt (`crash.rs`).
    crash::install();
    // One copy per person: a second launch hands its arguments to the first, which comes forward, and exits.
    // A copy the running one started in its place (`workspace/sign_in.rs`) waits for it to quit instead.
    let run = paths::cache_dir().join("run");
    let launch = if std::env::var_os("NEXUS_RESTARTED").is_some() {
        single_instance::claim_after_restart(&run)
    } else {
        single_instance::claim(&run)
    };
    let instance = match launch {
        single_instance::Launch::First(instance) => Some(instance),
        single_instance::Launch::HandedOff => return,
        single_instance::Launch::Unchecked(why) => {
            eprintln!("nexus-desktop-gpui: starting without the single-instance check: {why}");
            None
        }
    };
    // What a later launch brings: its `nexus://` prompt links, or nothing (`links.rs`).
    let (arrive, arrivals) = futures::channel::mpsc::unbounded();
    if let Some(instance) = instance {
        instance.serve(move |handoff| {
            // A login item started while this copy runs is skipped.
            if handoff
                .args
                .iter()
                .any(|arg| arg == nexus_desktop_core::prefs::AUTOSTART_ARG)
            {
                return;
            }
            for arrival in links::arrivals(&handoff.args) {
                let _ = arrive.unbounded_send(arrival);
            }
        });
    }
    // Code sessions' notifications, shown on the UI thread (`notify.rs`).
    let notifications = notify::start();
    nexus_client::install_tls();
    // Checks wait for the workspace to say who is signed in (`updates::session`).
    updates::start();
    // The new session's background, built while the saved sign-in is read, so the first screen draws it at once. Not
    // for a launch that starts in the tray: its window may never open.
    if !(nexus_desktop_core::prefs::menu_bar_only(&desktop_settings::load()) || prefs::autostarted()) {
        workspace::prewarm_backdrop(palette::Palette::expected_at_launch());
    }
    let (gateway, events) = gateway::Gateway::start();
    // Who is signed in, before the first window: it opens on the workspace or on sign-in, never on one then the other.
    gateway.preload(std::time::Duration::from_millis(1500));
    let gateway: std::sync::Arc<dyn backend::Backend> = std::sync::Arc::new(gateway);
    // The last light or dark, for windows that open before the desktop says which (`appearance.rs`).
    appearance::start();
    // The interface's language, before any text is drawn (`i18n.rs`).
    i18n::start();
    let mut app = gpui_kit::application().with_assets(assets::Assets);
    if let Some(http) = http::Http::new() {
        app = app.with_http_client(http);
    }
    app.run(move |cx: &mut App| {
        ui_thread::mark();
        gpui_kit::init(cx);
        local_providers_opt_in::start(cx);
        pages::local_providers::start(cx);
        // Grayscale text, as browsers draw it on Linux and macOS draws it today: GPUI's subpixel (ClearType-style) text
        // fringes every glyph with red and blue, which reads as cheap next to the web, and is wrong over a frosted
        // window anyway.
        cx.set_text_rendering_mode(gpui_kit::TextRenderingMode::Grayscale);
        // Nexus UI's text faces (brand v5), bundled so it does not depend on what the computer has.
        let fonts = [
            include_bytes!("../assets/fonts/DMSans-Regular.ttf").as_slice(),
            include_bytes!("../assets/fonts/DMSans-Medium.ttf").as_slice(),
            include_bytes!("../assets/fonts/DMSans-SemiBold.ttf").as_slice(),
            include_bytes!("../assets/fonts/DMSans-Bold.ttf").as_slice(),
            // Its code face, which the Code views and hints ask for by name.
            include_bytes!("../assets/fonts/GeistMono-Regular.ttf").as_slice(),
            include_bytes!("../assets/fonts/GeistMono-Medium.ttf").as_slice(),
        ];
        if let Err(error) = cx
            .text_system()
            .add_fonts(fonts.into_iter().map(std::borrow::Cow::Borrowed).collect())
        {
            eprintln!("nexus-desktop-gpui: fonts: {error}");
        }
        // The keys a person set in Settings › Keyboard shortcuts (`key_bindings.rs`), before the keymap is made.
        key_bindings::load();
        cx.bind_keys(key_bindings());
        cx.on_action(|_: &Quit, cx| cx.quit());
        // macOS keeps Check for Updates… in the application menu, New Window in File and Send Feedback… in Help;
        // Windows and Linux have no app menu, and updates stay in the tray and Settings.
        if cfg!(target_os = "macos") {
            cx.set_menus([
                Menu::new("Nexus").items([
                    MenuItem::action("Check for Updates…", windows::CheckForUpdates),
                    MenuItem::separator(),
                    MenuItem::action("Quit Nexus", Quit),
                ]),
                Menu::new("File").items([MenuItem::action("New Window", windows::NewWindow)]),
                Menu::new("Help").items([MenuItem::action("Send Feedback…", windows::SendFeedback)]),
            ]);
        }
        // Text size and reduce motion (Settings › Appearance), before the first window draws.
        zoom::start(cx);
        // The system's reduce transparency (macOS, Windows), before the first window picks its background.
        frost::read_system();
        chat_width::start(cx);
        // The UI-thread stall log, in debug builds or with NEXUS_UI_WATCHDOG=1 (`watchdog.rs`).
        watchdog::start(cx);
        // Started by the login item, or menu-bar only: just the tray item, the window when asked for. With no tray,
        // the window opens anyway, or nothing would show.
        let menu_bar_only = nexus_desktop_core::prefs::menu_bar_only(&desktop_settings::load());
        let window_at_launch = !(menu_bar_only || prefs::autostarted());
        // What each window drew, as JSON, with NEXUS_LAYOUT_DUMP=<file> (`layout_dump.rs`, `polish/`).
        layout_dump::start(cx);
        // The tray item: with it, closing the window keeps the app running there. Without one (no
        // StatusNotifierItem host or libappindicator), closing the window quits. With a window at launch, it goes up
        // once that window has drawn (`launch.rs`).
        let commands = tray::start(cx, window_at_launch);
        // Screen recordings for agents (macOS): the recorder, before the shortcut that starts it.
        record::start(cx);
        // The global shortcuts and the login item (`prefs.rs`).
        prefs::start(cx);
        keep_awake::start(cx);
        #[cfg(target_os = "macos")]
        if menu_bar_only {
            // No Dock icon and no app menu, as `LSUIElement` gives an app.
            cx.set_activation_policy(ActivationPolicy::Accessory);
        }
        let open_now = commands.is_none() || window_at_launch;
        windows::start(gateway, events, arrivals, commands, open_now, cx);
        // Development and checks: open the panel once launched, as the tray's Ask Nexus does.
        if std::env::var("NEXUS_DESKTOP_SHOW_PANEL").is_ok_and(|value| !value.is_empty()) {
            cx.spawn(async |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                cx.update(panel::show);
            })
            .detach();
        }
        // Development and checks: open the feedback dialog once launched, as the tray's Send Feedback… does.
        if std::env::var("NEXUS_DESKTOP_SHOW_FEEDBACK").is_ok_and(|value| !value.is_empty()) {
            cx.spawn(async |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                cx.update(|cx| windows::run(tray::Command::Feedback, cx));
            })
            .detach();
        }
        show_notifications(notifications, cx);
    });
}

/// Shows each Code session notification, and opens its session when it is clicked.
fn show_notifications(mut notifications: futures::channel::mpsc::UnboundedReceiver<notify::Shown>, cx: &mut App) {
    // The name the notification service shows, and on Windows the AppUserModelID toasts need.
    cx.set_app_identity("xyz.nexus.desktop-gpui", "Nexus");
    cx.spawn(async move |cx| {
        use futures::StreamExt as _;
        while let Some(shown) = notifications.next().await {
            cx.update(|cx| {
                cx.show_system_notification(SystemNotification {
                    tag: shown.tag.into(),
                    title: shown.title.into(),
                    body: shown.body.into(),
                    actions: Vec::new(),
                })
            });
        }
    })
    .detach();
    cx.on_system_notification_response(move |response, cx| {
        let opens = notify::clicked(&response.tag);
        let agent = notify::agent_clicked(&response.tag);
        // The organization's: its page, here when this build draws it, else in the browser (`path_notices.rs`).
        let nexus = notify::nexus_clicked(&response.tag).flatten();
        // The window focused last comes forward, or a new one when all were closed to the tray (`windows.rs`).
        windows::with_workspace(cx, move |workspace, window, cx| {
            if let Some(opens) = opens {
                workspace.open_notified(&opens.thread_id, opens.pull, window, cx);
            }
            // The Assistant's: its pane, on the conversation (`agent.rs`).
            if let Some(conversation) = agent {
                workspace.open_agent(conversation, window, cx);
            }
            if let Some(path) = nexus {
                workspace.open_nexus_path(&path, window, cx);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    /// The Workbench's keys, and New Window, reach the workspace with the composer's text field focused, which has keys
    /// of its own (its context is `Input`, under `Workspace`).
    #[test]
    fn workbench_keys_reach_the_workspace_from_the_composer() {
        let keymap = Keymap::new(key_bindings_with(false));
        let stack = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Input").unwrap(),
        ];
        let action = |keys: &'static str| {
            let (bindings, pending) = keymap.bindings_for_input(&[Keystroke::parse(keys).unwrap()], &stack);
            assert!(!pending, "{keys} waits for more keys");
            bindings.first().map(|binding| binding.action().boxed_clone())
        };
        let is = |keys, expected: &dyn Action| {
            let found = action(keys).unwrap_or_else(|| panic!("{keys} is not bound"));
            assert!(found.partial_eq(expected), "{keys} runs {}", found.name());
        };
        is("ctrl-`", &ToggleTerminal);
        is(shortcut_label::NEW_TERMINAL, &NewTerminal);
        is("ctrl-~", &NewTerminal);
        is(shortcut_label::MAXIMIZE_WORKBENCH, &ToggleMaximizeWorkbench);
        is(
            if cfg!(target_os = "macos") {
                "cmd-shift-n"
            } else {
                "ctrl-shift-n"
            },
            &windows::NewWindow,
        );
        // The canon's keys are off until the cutover: the default bindings leave them out.
        assert!(action("ctrl-cmd-n").is_none());
        assert!(
            action(if cfg!(target_os = "macos") {
                "cmd-shift-/"
            } else {
                "ctrl-shift-/"
            })
            .is_none()
        );
        // Off a Mac, Ctrl+Alt is AltGr, so Ctrl+Alt+\\ leaves a text box alone, as the Nexus app's `forPlatform`.
        if cfg!(target_os = "macos") {
            is(shortcut_label::TOGGLE_SIDE_PANE, &ToggleSidePane);
        } else {
            assert!(action(shortcut_label::TOGGLE_SIDE_PANE).is_none());
        }
        // The keys the Back and Forward hints show.
        is(GO_BACK, &GoBack);
        is(GO_FORWARD, &GoForward);
    }

    /// With the canon's keys on: Ask about Nexus, and a second key for a new window on a Mac. Cmd+/ stays the Nexus
    /// app's "Pick the model".
    #[test]
    fn canon_keys_are_bound_when_on() {
        let keymap = Keymap::new(key_bindings_with(true));
        let stack = [KeyContext::parse("Workspace").unwrap()];
        let is = |keys: &str, expected: &dyn Action| {
            let (bindings, _) = keymap.bindings_for_input(&[Keystroke::parse(keys).unwrap()], &stack);
            let found = bindings.first().unwrap_or_else(|| panic!("{keys} is not bound"));
            assert!(
                found.action().partial_eq(expected),
                "{keys} runs {}",
                found.action().name()
            );
        };
        if cfg!(target_os = "macos") {
            is("ctrl-cmd-n", &windows::NewWindow);
        }
        is(
            if cfg!(target_os = "macos") {
                "cmd-shift-/"
            } else {
                "ctrl-shift-/"
            },
            &AskAboutNexus,
        );
    }

    /// The hinted Back and Forward keys run from anywhere in the window, not only from the composer.
    #[test]
    fn back_and_forward_keys_reach_the_workspace() {
        let keymap = Keymap::new(key_bindings());
        let stack = [KeyContext::parse("Workspace").unwrap()];
        for (keys, expected) in [(GO_BACK, &GoBack as &dyn Action), (GO_FORWARD, &GoForward)] {
            let (bindings, _) = keymap.bindings_for_input(&[Keystroke::parse(keys).unwrap()], &stack);
            let found = bindings.first().unwrap_or_else(|| panic!("{keys} is not bound"));
            assert!(
                found.action().partial_eq(expected),
                "{keys} runs {}",
                found.action().name()
            );
        }
        let expected = if cfg!(target_os = "macos") {
            ("cmd-[", "cmd-]")
        } else {
            ("alt-left", "alt-right")
        };
        assert_eq!((GO_BACK, GO_FORWARD), expected);
    }

    /// A real notification through GPUI's platform layer, for a person to see: run with
    /// `cargo test real_notification -- --ignored`. It goes through [`notify::GpuiNotifier`] and the UI thread's queue,
    /// as a Code session's does.
    #[test]
    #[ignore = "shows a notification on this computer's desktop"]
    fn real_notification() {
        let mut shown = notify::start();
        notify::Notifier::show(&notify::GpuiNotifier, None, "t-check", "Nexus check", "Finished");
        let shown = shown.try_recv().expect("queued");
        gpui_kit::application().run(move |cx: &mut App| {
            cx.set_app_identity("xyz.nexus.desktop-gpui", "Nexus");
            cx.show_system_notification(SystemNotification {
                tag: shown.tag.into(),
                title: shown.title.into(),
                body: shown.body.into(),
                actions: Vec::new(),
            });
            cx.spawn(async |cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }

    /// The launcher entry names the window's app_id, so the taskbar and KWin's title bar find its icon.
    #[test]
    fn the_launcher_entry_names_the_app_id() {
        let entry = include_str!("../packaging/linux/nexus-desktop-gpui.desktop");
        assert!(
            entry
                .lines()
                .any(|line| line == format!("StartupWMClass={}", app_id::DEFAULT))
        );
        assert_eq!(env!("CARGO_BIN_NAME"), app_id::DEFAULT);
    }
}
