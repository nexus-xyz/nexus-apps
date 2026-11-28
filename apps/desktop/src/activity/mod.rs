//! A browser access indicator shared by every app window. It never grants access.
// The pill's window is off (`Activity::refresh`). Its code stays for when it comes back.
#[allow(dead_code)]
mod pill;
#[allow(dead_code)]
mod placement;
mod state;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::*;

use crate::side::browser::Browser;

type Valid = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub agent: String,
    pub title: String,
    pub action: String,
    pub site: String,
    pub paused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    id: String,
    status: Status,
}

struct Session {
    browser: WeakEntity<Browser>,
    runs: BTreeMap<String, Valid>,
    presentation: Valid,
    owner: AnyWindowHandle,
}

struct Current(Entity<Activity>);
impl Global for Current {}

struct Activity {
    sessions: BTreeMap<String, Session>,
    window: Option<(AnyWindowHandle, Entity<pill::Pill>)>,
    polling: bool,
    _poll: Task<()>,
}

/// Called only after the browser's consent check succeeds. A run's credential controls its lifetime.
pub(crate) fn using(browser: &Entity<Browser>, run: &str, valid: Valid, owner: AnyWindowHandle, cx: &mut App) {
    let presentation = browser.read(cx).activity_guard(Arc::new(|| true));
    if !presentation() || !valid() || browser.read(cx).is_retired() {
        return;
    }
    let activity = match cx.try_global::<Current>() {
        Some(current) => current.0.clone(),
        None => {
            let activity = cx.new(Activity::new);
            cx.set_global(Current(activity.clone()));
            activity
        }
    };
    let id = browser.read(cx).activity_id().to_string();
    activity.update(cx, |this, cx| {
        let session = this.sessions.entry(id).or_insert_with(|| Session {
            browser: browser.downgrade(),
            runs: BTreeMap::new(),
            presentation,
            owner,
        });
        session.browser = browser.downgrade();
        session.owner = owner;
        session.runs.insert(run.to_string(), valid);
        this.refresh(cx);
        this.poll(cx);
    });
}

/// Move credentials, never a retired presentation's authority, into its retained model.
pub(crate) fn retain(id: &str, cx: &mut App) -> BTreeMap<String, Valid> {
    let Some(activity) = cx.try_global::<Current>().map(|current| current.0.clone()) else {
        return BTreeMap::new();
    };
    activity.update(cx, |activity, _| {
        activity
            .sessions
            .remove(id)
            .map(|session| session.runs)
            .unwrap_or_default()
    })
}

#[derive(Clone, Copy)]
enum Control {
    Pause,
    TakeOver,
    Stop,
}

fn control(id: &str, control: Control, cx: &mut App) {
    let browser = cx
        .try_global::<Current>()
        .and_then(|current| current.0.read(cx).sessions.get(id))
        .filter(|session| (session.presentation)() && session.runs.values().any(|valid| valid()))
        .and_then(|session| session.browser.upgrade())
        .filter(|browser| !browser.read(cx).is_retired());
    if let Some(browser) = browser {
        browser.update(cx, |browser, cx| match control {
            Control::Pause => browser.pause_agent(!browser.agent_paused(), cx),
            Control::TakeOver => browser.take_over_agent(cx),
            Control::Stop => {
                browser.take_over_agent(cx);
                cx.emit(crate::side::browser::BrowserEvent::StopAgent);
            }
        });
    }
}

impl Activity {
    fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            sessions: BTreeMap::new(),
            window: None,
            polling: false,
            _poll: Task::ready(()),
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.polling || self.sessions.is_empty() {
            return;
        }
        self.polling = true;
        self._poll = cx.spawn(async |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let active = this.update(cx, |this, cx| {
                    this.refresh(cx);
                    this.polling = !this.sessions.is_empty();
                    this.polling
                });
                if !active.unwrap_or(false) {
                    break;
                }
            }
        });
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.sessions.retain(|_, session| {
            if !(session.presentation)() {
                return false;
            }
            session.runs.retain(|_, valid| valid());
            let Some(browser) = session.browser.upgrade() else {
                return false;
            };
            if browser.read(cx).is_retired() {
                return false;
            }
            if session.runs.is_empty() {
                // An expired or signed-out run cannot leave a suspended grant behind.
                if browser.read(cx).agent_paused() {
                    browser.update(cx, |browser, cx| browser.take_over_agent(cx));
                }
                return false;
            }
            browser.read(cx).activity_status().is_some()
        });
        let items: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(id, session)| {
                session
                    .browser
                    .read_with(cx, |browser, _| browser.activity_status())
                    .ok()
                    .flatten()
                    .map(|status| Item { id: id.clone(), status })
            })
            .collect();
        if items.is_empty() {
            if let Some((handle, _)) = self.window.take() {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
            return;
        }
        if let Some((handle, pill)) = &self.window {
            let changed = pill.read(cx).items != items;
            if handle
                .update(cx, |_, window, cx| {
                    if changed {
                        pill.update(cx, |pill, cx| pill.set_items(items, window, cx));
                    }
                })
                .is_err()
            {
                self.window = None;
            }
            return;
        }
        // The floating pill is off: it no longer opens a window over other apps.
        // Sessions are still tracked above, so `retain` and the expiry takeover keep working.
        // let owner = self.sessions.values().next().map(|session| session.owner);
        // let options = placement::options(owner, cx);
        // let open = |options, cx: &mut App| {
        //     gpui_kit::open_window(options, cx, |window, cx| {
        //         #[cfg(target_os = "linux")]
        //         crate::panel::float_activity(window);
        //         cx.new(|cx| pill::Pill::new(items.clone(), window, cx))
        //     })
        // };
        // let opened = open(options, cx);
        // #[cfg(target_os = "linux")]
        // let opened = match opened {
        //     Err(error) if error.is::<gpui_kit::layer_shell::LayerShellNotSupportedError>() => {
        //         // Compositors without layer-shell still get a transient indicator above the app.
        //         let mut options = placement::options(owner, cx);
        //         options.kind = WindowKind::Floating;
        //         open(options, cx)
        //     }
        //     opened => opened,
        // };
        // match opened {
        //     Ok(opened) => self.window = Some(opened),
        //     Err(error) => log::warn!("could not show browser activity: {error}"),
        // }
    }
}

/// Tool names are mapped to fixed copy. Selectors, typed text, and URL queries never reach the indicator.
pub(crate) fn action_label(action: Option<&str>) -> &'static str {
    match action {
        Some("browser_open" | "browser_new_tab") => "Opening a page",
        Some("browser_read") => "Reading the page",
        Some("browser_click") => "Clicking in the page",
        Some("browser_type") => "Typing in the page",
        Some("browser_screenshot") => "Capturing the browser tab",
        Some("browser_console") => "Reading the browser console",
        Some("browser_back" | "browser_forward" | "browser_reload") => "Navigating the browser",
        _ => "Browser access active",
    }
}

#[cfg(test)]
mod tests;
