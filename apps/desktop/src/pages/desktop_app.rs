//! Settings › App (the Nexus app's `settings/this-computer.tsx`: `panel.tsx`, `update-settings.tsx`,
//! `background-service-settings.tsx`): what this build is, where it keeps its files, and the Nexus app's App choices.
//! Open at login and the global shortcuts (the panel's, and on macOS the screen recording's) apply here at once,
//! through `prefs.rs`: each change registers with the OS first, and a failure keeps the old value and says why. Menu
//! bar only hides the Dock icon at once on macOS, as in the Nexus app; elsewhere it means no window at the next launch.
//! Keep agents running applies at the next start in both apps, or at Restart now (`service.rs`). Open the browser when
//! the agent starts a server applies at once in both apps (`side::browser::auto_preview`). Updates show here as
//! in the Nexus app's
//! Desktop updates (`crate::updates`): the version, channel, state, Check for updates and Restart to update.
//!
//! It also holds what the other "This computer" pages share: a read or write off the UI thread, and the line that
//! says why they are read-only.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::prefs::Slot;
use nexus_desktop_core::settings::Settings;
use nexus_desktop_updates::Phase;

use super::{DeviceCtx, palette, row, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

pub fn build(ctx: DeviceCtx, _window: &mut Window, cx: &mut App) -> Option<AnyView> {
    Some(page(ctx, false, cx).into())
}

/// The panel's Settings (`workspace/panel_view.rs`): the same controls as this page, compact: the shared choices,
/// then the updates.
pub fn build_panel(ctx: DeviceCtx, cx: &mut App) -> AnyView {
    page(ctx, true, cx).into()
}

fn page(ctx: DeviceCtx, panel: bool, cx: &mut App) -> Entity<DesktopAppPage> {
    cx.new(|cx| {
        let mut page = DesktopAppPage {
            ctx,
            panel,
            settings: Settings::default(),
            loaded: false,
            saving: None,
            error: None,
            updates: crate::updates::snapshot(),
            update_error: None,
            service_error: None,
            recording: None,
            listening: Slot::Panel,
        };
        page.load(cx);
        page.watch_updates(cx);
        page
    })
}

// ------------------------------------------------------------------ Shared by the "This computer" pages

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// A path with the home folder as `~`.
pub(super) fn tilde(path: &str) -> String {
    match home().and_then(|home| home.to_str().map(String::from)) {
        Some(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

/// A folder in the home folder, for a default: `~/nexus-worktrees`.
pub(super) fn in_home(name: &str) -> String {
    home()
        .map(|home| home.join(name).to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("~/{name}"))
}

/// Run `work` off the UI thread (a file, the keychain, the store), then apply what it returned and redraw.
pub(super) fn off_thread<V: 'static, T: Send + 'static>(
    cx: &mut Context<V>,
    work: impl FnOnce() -> T + Send + 'static,
    apply: impl FnOnce(&mut V, T, &mut Context<V>) + 'static,
) {
    let task = cx.background_executor().spawn(async move { work() });
    cx.spawn(async move |this, cx| {
        let result = task.await;
        this.update(cx, |view, cx| {
            apply(view, result, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

/// A note under a "This computer" page's title, in a card.
pub(super) fn owned_note(text: impl Into<SharedString>, palette: Palette) -> Div {
    ui::card(palette)
        .mb(px(6.))
        .child(div().text_size(px(13.)).text_color(palette.muted).child(text.into()))
}

/// The one line that says why the page changes nothing now; None while this build may write.
pub(super) fn read_only_note(palette: Palette) -> Option<Div> {
    (!crate::desktop_settings::writable()).then(|| owned_note(crate::desktop_settings::READ_ONLY, palette))
}

/// Save a change to the Nexus app's settings off the UI thread (`desktop_settings::update`), then tell the workspace
/// (`DeviceCtx::settings_saved`) and apply the answer to the page.
pub(super) fn save_settings<V: 'static, T: Send + 'static>(
    ctx: &DeviceCtx,
    cx: &mut Context<V>,
    change: impl FnOnce(&mut Settings) -> Result<T, String> + Send + 'static,
    apply: impl FnOnce(&mut V, Result<(Settings, T), String>, &mut Context<V>) + 'static,
) {
    let saved = ctx.settings_saved.clone();
    off_thread(
        cx,
        move || crate::desktop_settings::update(change),
        move |view, result, cx| {
            if let Ok((settings, _)) = &result {
                saved(settings, cx);
                crate::desktop_settings::Changed::publish(settings, cx);
            }
            apply(view, result, cx);
        },
    );
}

// ------------------------------------------------------------------ The page

/// The App choices this page saves for the Nexus app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    /// "Follow the agent's edits" (`workspace/side_pane/follow_edits.rs`). On by default.
    FollowEdits,
    MenuBarOnly,
    KeepAgentsRunning,
    /// "Open the browser when the agent starts a server" (`side::browser::auto_preview`). On by default.
    OpenBrowserOnServer,
    KeepAwake,
    /// "Archive merged sessions" (nexus-pull-requests' watch). Off by default.
    ArchiveMerged,
    /// "Chat bar" (`chat_bar.rs`, macOS). On by default.
    ChatBar,
}

impl Choice {
    fn of(self, settings: &Settings) -> bool {
        match self {
            Choice::FollowEdits => settings.follows_edits(),
            Choice::MenuBarOnly => nexus_desktop_core::prefs::menu_bar_only(settings),
            Choice::KeepAgentsRunning => settings.keep_agents_running.unwrap_or(false),
            Choice::OpenBrowserOnServer => settings.opens_browser_on_server(),
            Choice::KeepAwake => settings.keep_awake.unwrap_or(true),
            Choice::ArchiveMerged => settings.archives_merged(),
            Choice::ChatBar => settings.opens_chat_bar(),
        }
    }

    /// What the organization holds this choice at, when it does: with nothing kept on this computer
    /// (`AllowLocalCache` off), sessions can't outlive the app (the Nexus app's `service::unsupported`).
    fn forced(self, policy: &nexus_desktop_core::policy::Policy) -> Option<bool> {
        match self {
            Choice::KeepAgentsRunning if !crate::managed::local_cache(policy) => Some(false),
            _ => None,
        }
    }

    /// Save `on` as this choice.
    fn set(self, settings: &mut Settings, on: bool) {
        match self {
            Choice::FollowEdits => settings.follow_edits = Some(on),
            Choice::MenuBarOnly => settings.menu_bar_only = Some(on),
            Choice::KeepAgentsRunning => settings.keep_agents_running = Some(on),
            Choice::OpenBrowserOnServer => settings.open_browser_on_server = Some(on),
            Choice::KeepAwake => settings.keep_awake = Some(on),
            Choice::ArchiveMerged => settings.archive_merged = Some(on),
            Choice::ChatBar => settings.chat_bar = Some(on),
        }
    }
}

struct DesktopAppPage {
    ctx: DeviceCtx,
    settings: Settings,
    /// Defaults are placeholders until the asynchronous read finishes.
    loaded: bool,
    /// Drawn in the panel's Settings: the shared choices and the updates only.
    panel: bool,
    /// The choice being saved: one at a time.
    saving: Option<Choice>,
    error: Option<SharedString>,
    /// The update state, kept current by `watch_updates`.
    updates: Option<nexus_desktop_updates::Snapshot>,
    /// Why Restart to update did not start.
    update_error: Option<SharedString>,
    /// Why Restart now for Keep agents running did not start.
    service_error: Option<SharedString>,
    /// Waiting for the new global shortcut's keys: every key press comes here first.
    recording: Option<Subscription>,
    /// Which shortcut the keys are for.
    listening: Slot,
}

impl Drop for DesktopAppPage {
    fn drop(&mut self) {
        if self.recording.is_some() {
            crate::prefs::set_recording(false);
        }
    }
}

impl DesktopAppPage {
    /// Redraw on each new update state while the page is open.
    fn watch_updates(&mut self, cx: &mut Context<Self>) {
        let Some(mut changes) = crate::updates::subscribe() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            while changes.changed().await.is_ok() {
                let snapshot = changes.borrow_and_update().clone();
                if this
                    .update(cx, |page, cx| {
                        page.updates = Some(snapshot);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Whether this launch runs the sessions in the background service, and the line that says what the switch does now.
    fn service_line(&self, cx: &App) -> String {
        let code = (self.ctx.code)(cx);
        let enabled = Choice::KeepAgentsRunning.of(&self.settings);
        let line = crate::service::note(
            crate::managed::local_cache(crate::managed::policy()),
            enabled,
            code.service.is_some(),
            code.service.as_ref(),
        );
        match &self.service_error {
            Some(error) => format!("{line} {error}"),
            None => line,
        }
    }

    /// Restart now, while the switch as saved differs from what this launch does.
    fn service_restart(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Div> {
        let active = (self.ctx.code)(cx).service.is_some();
        if Choice::KeepAgentsRunning.of(&self.settings) == active
            || nexus_desktop_core::service::unsupported(crate::managed::local_cache(crate::managed::policy())).is_some()
        {
            return None;
        }
        let button =
            ui::button("app-keep-agents-restart", "Restart now", palette).on_click(cx.listener(|this, _, _, cx| {
                this.service_error = (this.ctx.restart_for_service)(cx).err().map(Into::into);
                cx.notify();
            }));
        Some(ui::setting_row(
            "Apply the change",
            "Nexus restarts. It won't while sessions are running.",
            button,
            palette,
        ))
    }

    fn restart_for_update(&mut self, cx: &mut Context<Self>) {
        self.update_error = (self.ctx.restart_for_update)(cx).err().map(Into::into);
        cx.notify();
    }

    /// What the updater says now: the version and channel, its state, and what the person can do. None when this build
    /// has no updater.
    fn update_parts(&self, palette: Palette, cx: &mut Context<Self>) -> Option<UpdateParts> {
        let snapshot = self.updates.clone()?;
        let download = crate::updates::download_url();
        let busy = matches!(
            snapshot.phase,
            Phase::Checking | Phase::Downloading | Phase::Verifying | Phase::Installing
        );
        let channel = match snapshot.channel {
            Some(nexus_desktop_updates::Channel::Internal) => "internal",
            Some(nexus_desktop_updates::Channel::Stable) => "stable",
            Some(nexus_desktop_updates::Channel::Beta) => "beta",
            None => "unconfigured",
        };
        let mut detail = crate::updates::status(&snapshot, download.is_some());
        if snapshot.phase == Phase::Downloading {
            detail = match snapshot.total {
                Some(total) if total > 0 => format!(
                    "{detail} {}%",
                    (snapshot.downloaded.saturating_mul(100) / total).min(100)
                ),
                _ => format!("{detail} {:.1} MB", snapshot.downloaded as f64 / 1_048_576.0),
            };
        }
        let check = if busy || snapshot.phase == Phase::Disabled {
            ui::disabled_button("app-update-check", "Check for updates", palette).into_any_element()
        } else {
            ui::button("app-update-check", "Check for updates", palette)
                .on_click(cx.listener(|_, _, _, cx| {
                    crate::updates::check();
                    cx.notify();
                }))
                .into_any_element()
        };
        let act = match (snapshot.phase, download) {
            (Phase::Ready, Some(url)) => Some(
                ui::primary_button("app-update-download", "Download", palette)
                    .on_click(move |_, _, cx| crate::external_link::open(&url, cx))
                    .into_any_element(),
            ),
            (Phase::Ready, None) => Some(
                ui::primary_button("app-update-restart", "Restart to update", palette)
                    .on_click(cx.listener(|this, _, _, cx| this.restart_for_update(cx)))
                    .into_any_element(),
            ),
            _ => None,
        };
        let last_check = match snapshot.last_check {
            Some(at) => chrono::DateTime::from_timestamp_millis(at as i64)
                .map(|at| at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default(),
            None => "Not checked yet".into(),
        };
        let error = self
            .update_error
            .clone()
            .or_else(|| snapshot.error.as_ref().map(|error| error.message.clone().into()));
        Some(UpdateParts {
            title: format!("Version {} · {channel} channel", snapshot.installed_version),
            detail: format!("{detail} Last check: {last_check}."),
            status: update_status(&snapshot, palette),
            controls: h_flex().gap(px(8.)).flex_none().child(check).children(act),
            error,
        })
    }

    /// The panel's Updates section: the state, the progress, and what the person can do now.
    fn updates_section(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let section = v_flex().child(ui::section_head("Updates", palette));
        let Some(parts) = self.update_parts(palette, cx) else {
            return section.child(ui::note("This build has no updater.", palette));
        };
        let disabled = self
            .updates
            .as_ref()
            .is_none_or(|snapshot| snapshot.phase == Phase::Disabled);
        section
            .child(ui::setting(parts.title, parts.detail, parts.controls, palette))
            .children(parts.error.map(|error| ui::error_line(error, palette)))
            .child(ui::setting(
                "Automatic updates",
                "Check on startup and while Nexus is open. Download and verify updates in the background.",
                ui::switch("app-automatic-updates", crate::updates::automatic(), disabled, palette).when(
                    !disabled,
                    |control| {
                        control.on_click(cx.listener(|_, _, _, cx| {
                            let enabled = !crate::updates::automatic();
                            off_thread(
                                cx,
                                move || crate::updates::set_automatic(enabled),
                                |this, result, _| {
                                    this.update_error = result.err().map(Into::into);
                                },
                            );
                        }))
                    },
                ),
                palette,
            ))
            .child(ui::note(
                "You choose when to restart. Nexus won't interrupt running agents without confirmation.",
                palette,
            ))
    }

    /// The page's first card: this build, its version and its updates in one row.
    fn about_card(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let parts = self.update_parts(palette, cx);
        let detail = match &parts {
            Some(parts) => format!("{}. {}", parts.title, parts.detail),
            None => format!(
                "Version {}, built from source. This build has no updater.",
                crate::channel::VERSION
            ),
        };
        let name = h_flex()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .child("Nexus for desktop"),
            )
            .child(ui::badge("Experimental", palette));
        let (status, controls, error) = match parts {
            Some(parts) => (parts.status, Some(parts.controls), parts.error),
            None => (None, None, None),
        };
        v_flex()
            .child(
                ui::card_frame(palette).mt(px(20.)).child(
                    ui::card_row()
                        .min_h(px(64.))
                        .child(
                            div()
                                .flex_none()
                                .size(px(40.))
                                .rounded(px(10.))
                                .bg(palette.primary)
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(16.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(palette.primary_foreground)
                                .child("N"),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w(px(0.))
                                .gap(px(2.))
                                .child(name)
                                .child(ui::setting_detail(detail, palette)),
                        )
                        .children(status.map(|(text, dot)| ui::status(text, dot, palette)))
                        .children(controls),
                ),
            )
            .children(error.map(|error| ui::error_line(error, palette).mt(px(8.))))
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        off_thread(cx, crate::desktop_settings::load, |this, settings, _| {
            this.settings = settings;
            this.loaded = true;
        });
    }

    fn toggle(&mut self, choice: Choice, cx: &mut Context<Self>) {
        if !self.loaded
            || !crate::desktop_settings::writable()
            || self.saving.is_some()
            || choice.forced(crate::managed::policy()).is_some()
        {
            return;
        }
        let on = !choice.of(&self.settings);
        self.saving = Some(choice);
        self.error = None;
        save_settings(
            &self.ctx,
            cx,
            move |settings| {
                choice.set(settings, on);
                Ok(())
            },
            move |this, saved, cx| {
                this.saving = None;
                match saved {
                    Ok((settings, ())) => {
                        this.settings = settings;
                        if choice == Choice::MenuBarOnly {
                            apply_menu_bar_only(on, cx);
                        }
                    }
                    Err(error) => this.error = Some(error.into()),
                }
            },
        );
        cx.notify();
    }

    /// Turn the login item on or off; it changes on the OS first.
    fn toggle_login(&mut self, cx: &mut Context<Self>) {
        let on = !crate::prefs::launch_at_login(cx);
        self.error = crate::prefs::set_launch_at_login(cx, on).err().map(Into::into);
        cx.notify();
    }

    fn login_switch(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if let Some(on) = crate::managed::policy().launch_at_login {
            return set_by_org("app-open-at-login", on, palette);
        }
        let locked = !crate::desktop_settings::writable();
        let switch = ui::switch("app-open-at-login", crate::prefs::launch_at_login(cx), locked, palette);
        if locked {
            switch.into_any_element()
        } else {
            switch
                .on_click(cx.listener(|this, _, _, cx| this.toggle_login(cx)))
                .into_any_element()
        }
    }

    /// Wait for `slot`'s new keys: Esc cancels, a press with a modifier (or a function key) sets it.
    fn record(&mut self, slot: Slot, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            let same = self.listening == slot;
            self.stop_recording(cx);
            if same {
                return;
            }
        }
        self.listening = slot;
        crate::prefs::set_recording(true);
        self.error = None;
        let this = cx.entity().downgrade();
        self.recording = Some(cx.intercept_keystrokes(move |event, _, cx| {
            let keystroke = event.keystroke.clone();
            cx.stop_propagation();
            let _ = this.update(cx, |page, cx| page.recorded(&keystroke, cx));
        }));
        cx.notify();
    }

    fn recorded(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        if keystroke.key == "escape" && !keystroke.modifiers.modified() {
            self.stop_recording(cx);
            return;
        }
        let Some(accelerator) = crate::prefs::accelerator_of(keystroke) else {
            return;
        };
        self.stop_recording(cx);
        self.error = crate::prefs::set_shortcut(cx, self.listening, &accelerator)
            .err()
            .map(Into::into);
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        self.recording = None;
        crate::prefs::set_recording(false);
        cx.notify();
    }

    /// Whether the page waits for `slot`'s new keys.
    fn listening_for(&self, slot: Slot) -> bool {
        self.recording.is_some() && self.listening == slot
    }

    fn shortcut_control(&self, slot: Slot, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let (id, allowed) = match slot {
            Slot::Panel => ("app-shortcut", crate::managed::policy().global_shortcut_enabled),
            Slot::Record => ("app-record-shortcut", crate::managed::policy().screen_capture_enabled),
        };
        if !allowed {
            return set_by_org(id, false, palette);
        }
        let saved = crate::prefs::shortcut_of(cx, slot).0;
        if !crate::desktop_settings::writable() {
            return h_flex()
                .id(id)
                .opacity(0.6)
                .child(ui::keycaps(&saved, palette))
                .into_any_element();
        }
        if self.listening_for(slot) {
            return ui::button(id, "Press keys…", palette)
                .on_click(cx.listener(move |this, _, _, cx| this.record(slot, cx)))
                .into_any_element();
        }
        // The keys themselves are the control: a press records new ones, as on the Shortcuts page.
        ui::focusable(h_flex().id(id), palette)
            .p(px(4.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .active(|style| style.opacity(0.8))
            .child(ui::keycaps(&saved, palette))
            .on_click(cx.listener(move |this, _, _, cx| this.record(slot, cx)))
            .into_any_element()
    }

    fn choice_switch(&self, id: &'static str, choice: Choice, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if let Some(on) = choice.forced(crate::managed::policy()) {
            return set_by_org(id, on, palette);
        }
        let locked = !self.loaded || !crate::desktop_settings::writable() || self.saving.is_some();
        let switch = ui::switch(id, choice.of(&self.settings), locked, palette);
        if locked {
            switch.into_any_element()
        } else {
            switch
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(choice, cx)))
                .into_any_element()
        }
    }

    /// A folder or file this build keeps: its name (Shared when the Nexus app uses it too) over its path, and Show.
    fn folder_row(
        &self,
        id: &'static str,
        title: &'static str,
        shared: bool,
        path: Option<PathBuf>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let shown = path
            .as_deref()
            .map(|path| tilde(&path.to_string_lossy()))
            .unwrap_or_else(|| "–".into());
        let tip = if cfg!(target_os = "macos") {
            "Show in Finder"
        } else {
            "Show in its folder"
        };
        let control = match path {
            Some(path) => row::icon_button(id, Lucide::FolderOpen, tip, palette)
                .on_click(cx.listener(move |_, _, _, cx| reveal(&path, cx)))
                .into_any_element(),
            None => div()
                .flex_none()
                .size(px(ui::CONTROL_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .child(ui::icon(Lucide::FolderOpen, palette.faint))
                .into_any_element(),
        };
        Some(
            ui::card_row()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap(px(2.))
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .child(div().text_size(px(13.)).text_color(palette.strong).child(title))
                                .when(shared, |name| name.child(ui::badge("Shared", palette))),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_family(ui::MONO)
                                .text_color(palette.faint)
                                .truncate()
                                .child(shown),
                        ),
                )
                .child(control)
                .into_any_element(),
        )
    }
}

/// The updater's state for the About card: the parts `update_parts` hands to a row.
struct UpdateParts {
    title: String,
    detail: String,
    status: Option<(&'static str, Hsla)>,
    controls: Div,
    error: Option<SharedString>,
}

/// The updater's state in a few words and its dot, when it has one worth a glance.
fn update_status(snapshot: &nexus_desktop_updates::Snapshot, palette: Palette) -> Option<(&'static str, Hsla)> {
    match snapshot.phase {
        Phase::Ready => Some(("Update ready", palette.waiting())),
        Phase::Checking => Some(("Checking…", palette.working())),
        Phase::Downloading | Phase::Verifying => Some(("Downloading…", palette.working())),
        Phase::Installing => Some(("Installing…", palette.working())),
        Phase::Error => Some(("Update failed", palette.destructive)),
        Phase::Idle if snapshot.last_check.is_some() && snapshot.available_version.is_none() => {
            Some(("Up to date", Palette::success()))
        }
        Phase::Idle | Phase::Disabled => None,
    }
}

/// A switch the organization set: locked, with the note beside it.
/// Menu bar only, at once, as the Nexus app's `apply_activation`: on macOS the accessory policy hides the Dock icon and
/// the app menu, and the regular one shows them. Elsewhere it only means no window at the next launch.
fn apply_menu_bar_only(on: bool, cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.set_activation_policy(if on {
        ActivationPolicy::Accessory
    } else {
        ActivationPolicy::Regular
    });
    #[cfg(not(target_os = "macos"))]
    let _ = (on, cx);
}

/// The Menu bar only row's name and line, as the Nexus app words them on each platform.
fn menu_bar_only_text() -> (&'static str, &'static str) {
    if cfg!(target_os = "macos") {
        ("Menu bar only", "No Dock icon; Nexus stays in the menu bar.")
    } else {
        ("Start in the tray", "No window at launch.")
    }
}

fn set_by_org(id: &'static str, on: bool, palette: Palette) -> AnyElement {
    h_flex()
        .gap(px(8.))
        .flex_none()
        .child(ui::badge(crate::managed::SET_BY_ORG, palette))
        .child(ui::switch(id, on, true, palette))
        .into_any_element()
}

/// Open at login's line: the organization's choice when it decides (`LaunchAtLogin`).
fn launch_at_login_line(policy: &nexus_desktop_core::policy::Policy) -> String {
    match policy.launch_at_login {
        Some(on) => format!("{}: {}.", crate::managed::SET_BY_ORG, if on { "on" } else { "off" }),
        None => {
            "Start Nexus in the tray when you log in. Of the two apps, the one that turned it on last opens.".into()
        }
    }
}

/// The global shortcut's line: off when the organization turned it off (`GlobalShortcutEnabled`), else whether it is
/// registered and why not.
fn shortcut_line(
    policy: &nexus_desktop_core::policy::Policy,
    status: &nexus_desktop_core::prefs::ShortcutStatus,
    recording: bool,
) -> String {
    if !policy.global_shortcut_enabled {
        return "Your organization turned the global shortcut off.".into();
    }
    if recording {
        return "Press the new keys. Esc cancels.".into();
    }
    match &status.error {
        Some(error) => error.clone(),
        None => "Opens Nexus on a new chat from any app. The Nexus app opens its panel with it.".into(),
    }
}

/// The screen recording shortcut's line: off when the organization turned recording off (`ScreenCaptureEnabled`),
/// else whether it is registered and why not.
fn record_line(
    policy: &nexus_desktop_core::policy::Policy,
    status: &nexus_desktop_core::prefs::ShortcutStatus,
    listening: bool,
) -> String {
    if !policy.screen_capture_enabled {
        return "Your organization turned screen recording off.".into();
    }
    if listening {
        return "Press the new keys. Esc cancels.".into();
    }
    match &status.error {
        Some(error) => error.clone(),
        None => "Records the screen and your voice; hold ⇧ to draw. Press again to copy it for your agent.".into(),
    }
}

/// Show a folder in the system's file manager; a file is shown in its folder.
fn reveal(path: &Path, cx: &mut App) {
    if path.is_dir() {
        cx.open_with_system(path);
    } else {
        cx.reveal_path(path);
    }
}

impl Render for DesktopAppPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let policy = crate::managed::policy();
        let login_line = launch_at_login_line(policy);
        let (_, shortcut_status) = crate::prefs::shortcut(cx);
        let (_, record_status) = crate::prefs::shortcut_of(cx, Slot::Record);
        // macOS only; the organization's switch shows on it.
        let records = nexus_desktop_record::available(true);
        let forced = crate::managed::forced_names(policy);
        if self.panel {
            return v_flex()
                .w_full()
                .children(read_only_note(palette))
                .child(self.ctx.settings_fields.field(
                    "global-shortcut",
                    ui::setting(
                        "Global shortcut",
                        shortcut_line(policy, &shortcut_status, self.listening_for(Slot::Panel)),
                        self.shortcut_control(Slot::Panel, palette, cx),
                        palette,
                    ),
                    None,
                    cx,
                ))
                // The panel's "Record for an agent" row.
                .when(records, |page| {
                    page.child(self.ctx.settings_fields.field(
                        "record-shortcut",
                        ui::setting(
                            "Record for an agent",
                            record_line(policy, &record_status, self.listening_for(Slot::Record)),
                            self.shortcut_control(Slot::Record, palette, cx),
                            palette,
                        ),
                        None,
                        cx,
                    ))
                })
                .child(self.ctx.settings_fields.field(
                    "open-at-login",
                    ui::setting("Open at login", login_line, self.login_switch(palette, cx), palette),
                    None,
                    cx,
                ))
                .child(self.ctx.settings_fields.field(
                    "menu-bar-only",
                    ui::setting(
                        "Menu bar only",
                        "Open as a tray or menu-bar item, with no window at launch. Applies at the next start.",
                        self.choice_switch("app-menu-bar-only", Choice::MenuBarOnly, palette, cx),
                        palette,
                    ),
                    None,
                    cx,
                ))
                .child(
                    self.ctx
                        .settings_fields
                        .field("desktop-updates", self.updates_section(palette, cx), None, cx),
                )
                .children(self.error.clone().map(|error| ui::error_line(error, palette)));
        }
        let fields = self.ctx.settings_fields.clone();
        let startup = ui::group(
            [
                Some(
                    fields
                        .field(
                            "open-at-login",
                            ui::setting_row("Open at login", login_line, self.login_switch(palette, cx), palette),
                            None,
                            cx,
                        )
                        .into_any_element(),
                ),
                Some(
                    fields
                        .field(
                            "menu-bar-only",
                            ui::setting_row(
                                menu_bar_only_text().0,
                                menu_bar_only_text().1,
                                self.choice_switch("app-menu-bar-only", Choice::MenuBarOnly, palette, cx),
                                palette,
                            ),
                            None,
                            cx,
                        )
                        .into_any_element(),
                ),
            ],
            palette,
        );
        let shortcuts = ui::group(
            [
                Some(
                    fields
                        .field(
                            "global-shortcut",
                            ui::setting_row(
                                "Global shortcut",
                                shortcut_line(policy, &shortcut_status, self.listening_for(Slot::Panel)),
                                self.shortcut_control(Slot::Panel, palette, cx),
                                palette,
                            ),
                            None,
                            cx,
                        )
                        .into_any_element(),
                ),
                cfg!(target_os = "macos").then(|| {
                    fields
                        .field(
                            "chat-bar",
                            ui::setting_row(
                                "Chat bar (⌥Space)",
                                "The shortcut opens a small bar over any app to ask the Assistant. Off, it opens the Ask Nexus panel.",
                                self.choice_switch("app-chat-bar", Choice::ChatBar, palette, cx),
                                palette,
                            ),
                            None,
                            cx,
                        )
                        .into_any_element()
                }),
                records.then(|| {
                    fields
                        .field(
                            "record-shortcut",
                            ui::setting_row(
                                "Record for an agent",
                                record_line(policy, &record_status, self.listening_for(Slot::Record)),
                                self.shortcut_control(Slot::Record, palette, cx),
                                palette,
                            ),
                            None,
                            cx,
                        )
                        .into_any_element()
                }),
            ],
            palette,
        );
        let agents = ui::group(
            [
                Some(
                    fields
                        .field(
                            "follow-edits",
                            ui::setting_row(
                                "Follow the agent's edits",
                                "Changes opens each file the agent edits, until you scroll or comment there.",
                                self.choice_switch("app-follow-edits", Choice::FollowEdits, palette, cx),
                                palette,
                            ),
                            None,
                            cx,
                        )
                        .into_any_element(),
                ),
                Some(
                    ui::setting_row(
                        "Open the browser when the agent starts a server",
                        "A session's Browser opens on the dev server once it answers.",
                        self.choice_switch("app-open-browser", Choice::OpenBrowserOnServer, palette, cx),
                        palette,
                    )
                    .into_any_element(),
                ),
                Some(
                    fields
                        .field(
                            "archive-merged",
                            ui::setting_row(
                                "Archive merged sessions",
                                "A session whose pull request merges goes to Archived instead of Settled.",
                                self.choice_switch("app-archive-merged", Choice::ArchiveMerged, palette, cx),
                                palette,
                            ),
                            None,
                            cx,
                        )
                        .into_any_element(),
                ),
                Some(
                    ui::setting_row(
                        "Keep awake while agents work",
                        crate::keep_awake::note(),
                        self.choice_switch("app-keep-awake", Choice::KeepAwake, palette, cx),
                        palette,
                    )
                    .into_any_element(),
                ),
                Some(
                    ui::setting_row(
                        "Keep agents running when Nexus quits",
                        self.service_line(cx),
                        self.choice_switch("app-keep-agents", Choice::KeepAgentsRunning, palette, cx),
                        palette,
                    )
                    .into_any_element(),
                ),
                self.service_restart(palette, cx).map(IntoElement::into_any_element),
            ],
            palette,
        );
        let files = ui::group(
            [
                self.folder_row("app-data", "Code sessions", true, crate::paths::data_dir(), palette, cx),
                self.folder_row(
                    "app-nexus-settings",
                    "Settings",
                    true,
                    crate::desktop_settings::file(),
                    palette,
                    cx,
                ),
                self.folder_row(
                    "app-settings",
                    "This build's settings",
                    false,
                    crate::paths::app_settings_file(),
                    palette,
                    cx,
                ),
                records
                    .then(|| {
                        self.folder_row(
                            "app-recordings",
                            "Screen recordings",
                            true,
                            crate::record::folder(),
                            palette,
                            cx,
                        )
                    })
                    .flatten(),
                self.folder_row(
                    "app-cache",
                    "Cache",
                    false,
                    Some(crate::paths::cache_dir()),
                    palette,
                    cx,
                ),
            ],
            palette,
        );
        v_flex()
            .w_full()
            .child(ui::lead(
                "How this build starts, runs and updates. The Nexus app shares these choices.",
                palette,
            ))
            .child(fields.field("desktop-updates", self.about_card(palette, cx), None, cx))
            .child(ui::group_head("Startup", palette))
            .child(startup)
            .child(ui::group_head("Shortcuts", palette))
            .child(shortcuts)
            .child(if records {
                ui::footnote("Click the keys, then press the new ones. Esc cancels.", palette)
            } else {
                fields.field(
                    "record-shortcut",
                    ui::footnote("Record for an agent is available on macOS only.", palette),
                    None,
                    cx,
                )
            })
            .child(ui::group_head("While agents work", palette))
            .child(agents)
            .child(ui::footnote(
                "Both apps apply Keep agents running when they start.",
                palette,
            ))
            .child(ui::group_head("Files", palette))
            .child(files)
            .child(ui::footnote(
                "Shared: the Nexus app reads and writes it too. The cache is safe to delete while this build is closed.",
                palette,
            ))
            .children((!forced.is_empty()).then(|| {
                v_flex()
                    .child(ui::group_head("Managed by your organization", palette))
                    .child(ui::setting_detail(
                        format!("{}. Sources: {}.", forced.join(", "), policy.sources.join(", ")),
                        palette,
                    ))
            }))
            .children(self.error.clone().map(|error| ui::error_line(error, palette).mt(px(8.))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn keep_awake_defaults_on_for_existing_settings() {
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert!(Choice::KeepAwake.of(&old));
        let disabled: Settings = serde_json::from_str(r#"{"keepAwake":false}"#).unwrap();
        assert!(!Choice::KeepAwake.of(&disabled));
    }

    #[test]
    fn saves_the_nexus_apps_app_choices() {
        let file = crate::desktop_settings::TempFile::new("app");
        for choice in [Choice::MenuBarOnly, Choice::KeepAgentsRunning, Choice::KeepAwake] {
            for on in [true, false] {
                let (saved, ()) = file
                    .update(|settings| {
                        choice.set(settings, on);
                        Ok(())
                    })
                    .expect("saved");
                assert_eq!(choice.of(&saved), on);
                assert_eq!(choice.of(&nexus_desktop_core::settings::load(file.path())), on);
            }
        }
    }

    #[test]
    fn the_browser_choice_is_on_until_turned_off_and_saves_for_both_apps() {
        let file = crate::desktop_settings::TempFile::new("open-browser");
        let choice = Choice::OpenBrowserOnServer;
        assert!(choice.of(&Settings::default()), "on by default");
        let (saved, ()) = file
            .update(|settings| {
                choice.set(settings, !choice.of(settings));
                Ok(())
            })
            .expect("saved");
        assert!(!choice.of(&saved));
        let shared = nexus_desktop_core::settings::load(file.path());
        assert_eq!(
            shared.open_browser_on_server,
            Some(false),
            "the Nexus app's `openBrowserOnServer`"
        );
    }

    #[test]
    fn follow_edits_is_on_until_turned_off_and_saves_for_both_apps() {
        let file = crate::desktop_settings::TempFile::new("follow-edits");
        let choice = Choice::FollowEdits;
        assert!(choice.of(&Settings::default()), "on by default");
        let (saved, ()) = file
            .update(|settings| {
                choice.set(settings, !choice.of(settings));
                Ok(())
            })
            .expect("saved");
        assert!(!choice.of(&saved));
        let shared = nexus_desktop_core::settings::load(file.path());
        assert_eq!(shared.follow_edits, Some(false), "the Nexus app's `followEdits`");
    }

    #[test]
    fn archive_merged_is_off_until_turned_on_and_saves_for_both_apps() {
        let file = crate::desktop_settings::TempFile::new("archive-merged");
        let choice = Choice::ArchiveMerged;
        assert!(!choice.of(&Settings::default()), "off by default");
        let (saved, ()) = file
            .update(|settings| {
                choice.set(settings, !choice.of(settings));
                Ok(())
            })
            .expect("saved");
        assert!(choice.of(&saved));
        let shared = nexus_desktop_core::settings::load(file.path());
        assert_eq!(shared.archive_merged, Some(true), "the Nexus app's `archiveMerged`");
    }

    #[test]
    fn home_reads_as_a_tilde() {
        if let Some(home) = home().and_then(|home| home.to_str().map(String::from)) {
            assert_eq!(tilde(&format!("{home}/nexus-worktrees")), "~/nexus-worktrees");
        }
        assert_eq!(tilde("/opt/x"), "/opt/x");
    }

    #[test]
    fn the_policy_locks_open_at_login_the_shortcut_and_keep_agents_running() {
        use nexus_desktop_core::policy::{Value as Pref, key};
        let policy = crate::managed::tests::policy_of(
            &[
                (key::LAUNCH_AT_LOGIN, Pref::Bool(true)),
                (key::GLOBAL_SHORTCUT_ENABLED, Pref::Bool(false)),
                (key::ALLOW_LOCAL_CACHE, Pref::Bool(false)),
            ],
            None,
        );
        let status = nexus_desktop_core::prefs::ShortcutStatus::default();
        assert!(launch_at_login_line(&policy).starts_with(crate::managed::SET_BY_ORG));
        assert!(shortcut_line(&policy, &status, false).contains("turned the global shortcut off"));
        assert_eq!(Choice::KeepAgentsRunning.forced(&policy), Some(false));
        assert_eq!(Choice::MenuBarOnly.forced(&policy), None);

        let free = crate::managed::tests::policy_of(&[], None);
        assert!(!launch_at_login_line(&free).starts_with(crate::managed::SET_BY_ORG));
        let taken = nexus_desktop_core::prefs::ShortcutStatus {
            active: None,
            error: Some("Another app uses that shortcut. Try other keys.".into()),
        };
        assert!(shortcut_line(&free, &taken, false).starts_with("Another app"));
        assert_eq!(Choice::KeepAgentsRunning.forced(&free), None);
    }

    #[test]
    fn the_record_shortcuts_line_says_why_it_is_off_or_what_it_does() {
        use nexus_desktop_core::policy::{Value as Pref, key};
        let off = crate::managed::tests::policy_of(&[(key::SCREEN_CAPTURE_ENABLED, Pref::Bool(false))], None);
        let status = nexus_desktop_core::prefs::ShortcutStatus::default();
        assert_eq!(
            record_line(&off, &status, false),
            "Your organization turned screen recording off."
        );
        let free = crate::managed::tests::policy_of(&[], None);
        assert!(record_line(&free, &status, false).starts_with("Records the screen and your voice"));
        assert_eq!(record_line(&free, &status, true), "Press the new keys. Esc cancels.");
        let taken = nexus_desktop_core::prefs::ShortcutStatus {
            active: None,
            error: Some("Alt+Space is already Nexus's other shortcut.".into()),
        };
        assert_eq!(
            record_line(&free, &taken, false),
            "Alt+Space is already Nexus's other shortcut."
        );
    }
}
