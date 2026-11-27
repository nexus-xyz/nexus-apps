//! System notifications for Code sessions: a session that needs the
//! person (an approval, a question) or finished a turn, and a pull request change (`pr_watch.rs`). The text and the
//! rules are `nexus_desktop_core::notify`'s; this module is GPUI's [`Notifier`].
//!
//! They show through GPUI's own platform layer (`App::show_system_notification`): the XDG notification service over
//! D-Bus on Linux (through `notify-rust`), the user notification center on macOS
//! and toasts on Windows. A click comes back as `on_system_notification_response` on every platform, and opens the
//! session ([`clicked`]).
//!
//! A notification is shown unless "Your sessions" is off (Settings, Notifications, On this computer; `sessionNotifications` in
//! the Nexus app's `settings.json`, on by default), or the person is looking at that session: a focused window with
//! the session on screen ([`viewing`]). Each window reports what it shows; only the focused one counts.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
pub use nexus_desktop_core::notify::{AgentNames, Notifier, message, screen_key, wanted};

/// What one window shows now, as its workspace last drew it.
#[derive(Default)]
struct Seen {
    focused: bool,
    on_screen: Option<String>,
}

/// What the person looks at across the windows: the focused window's session, or nothing when no window has the focus.
fn looking(windows: &HashMap<u64, Seen>) -> (bool, Option<String>) {
    windows
        .values()
        .find(|seen| seen.focused)
        .map(|seen| (true, seen.on_screen.clone()))
        .unwrap_or((false, None))
}

/// A notification for the UI thread to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    /// `session:<n>`: a click hands it back, and [`clicked`] finds the session by it.
    pub tag: String,
    pub title: String,
    pub body: String,
}

/// What a clicked notification opens: a session, or its pull request page for a pull request change.
#[derive(Debug, Clone, PartialEq)]
pub struct Opens {
    pub thread_id: String,
    pub pull: bool,
}

struct State {
    /// By window (`WindowId::as_u64`).
    seen: Mutex<HashMap<u64, Seen>>,
    out: UnboundedSender<Shown>,
    /// What each shown notification opens, by tag: a server's session too (`machines.rs`), by its own id.
    opens: Mutex<HashMap<String, Opens>>,
    /// The Assistant's notifications: the conversation each opens its pane on, by tag.
    agent_opens: Mutex<HashMap<String, Option<String>>>,
    /// The organization's notifications (`path_notices.rs`): the app page each opens, by tag.
    nexus_opens: Mutex<HashMap<String, Option<String>>>,
    next: Mutex<u64>,
}

/// The app's one notifier state, set at launch ([`start`]). Tests make their own [`State`], so a test's notification
/// never reaches another test's queue: the socket tests' secret requests notify too.
static STATE: OnceLock<State> = OnceLock::new();

/// At launch: the notifications to show, for the UI thread.
pub fn start() -> UnboundedReceiver<Shown> {
    let (state, shown) = State::new();
    let _ = STATE.set(state);
    shown
}

/// The workspace of `window` drew `on_screen` (a Code session's id, or None), in a window that has the focus or not.
pub fn viewing(window: u64, focused: bool, on_screen: Option<&str>) {
    if let Some(thread_id) = STATE.get().and_then(|state| state.viewing(window, focused, on_screen)) {
        crate::pr_watch::seen(&thread_id);
    }
}

/// `window` closed: it shows nothing any more.
pub fn closed(window: u64) {
    if let Some(state) = STATE.get() {
        state.closed(window);
    }
}

/// What a clicked notification opens.
pub fn clicked(tag: &str) -> Option<Opens> {
    STATE.get()?.clicked(tag)
}

/// The pane a clicked notification of the Assistant opens: on its conversation, when it has one.
pub fn agent_clicked(tag: &str) -> Option<Option<String>> {
    STATE.get()?.agent_clicked(tag)
}

/// The page a clicked notification of the organization opens (`path_notices.rs`): Gateway's app path, when it has one.
pub fn nexus_clicked(tag: &str) -> Option<Option<String>> {
    STATE.get()?.nexus_clicked(tag)
}

/// One of the organization's notifications (`path_notices.rs`), as the Nexus app's `show_opening`: shown whatever has
/// the focus. The loop checks its own switch ("Your organization"); "Your sessions" does not mute it.
pub fn nexus(notice: nexus_desktop_core::path_notices::Notice) {
    if let Some(state) = STATE.get() {
        state.nexus(notice);
    }
}

/// The Assistant's notification: an answer or an approval waits (`kind`), shown only
/// while no app window has the focus and the notifications setting is on. It never carries the reply's words. Its
/// title names the Assistant as the person named it (`assistant`).
pub fn agent(kind: &str, conversation: Option<String>, assistant: &str) {
    if let Some(state) = STATE.get() {
        state.agent(kind, conversation, assistant);
    }
}

/// Whether sessions show notifications: the Nexus app's setting (Settings, Notifications). On by default.
pub fn setting() -> bool {
    crate::desktop_settings::load().session_notifications.unwrap_or(true)
}

/// [`setting`], read at each notification.
#[cfg(not(test))]
fn enabled() -> bool {
    setting()
}

/// Tests do not read the person's own settings.
#[cfg(test)]
fn enabled() -> bool {
    true
}

/// GPUI's [`Notifier`], over the window's state and the UI thread's queue.
#[derive(Clone, Copy, Default)]
pub struct GpuiNotifier;

impl Notifier for GpuiNotifier {
    fn looking_at(&self, thread_id: &str) -> bool {
        STATE.get().is_some_and(|state| state.looking_at(thread_id))
    }

    fn show(&self, machine: Option<&str>, thread_id: &str, title: &str, body: &str) {
        if let Some(state) = STATE.get() {
            state.show(machine, thread_id, title, body, false);
        }
    }
}

/// A pull request change (`pr_watch.rs`): a click opens the session's pull request page.
pub fn show_pull(machine: Option<&str>, thread_id: &str, title: &str, body: &str) {
    if let Some(state) = STATE.get() {
        state.show(machine, thread_id, title, body, true);
    }
}

impl State {
    fn new() -> (State, UnboundedReceiver<Shown>) {
        let (out, shown) = unbounded();
        let state = State {
            seen: Mutex::default(),
            out,
            opens: Mutex::default(),
            agent_opens: Mutex::default(),
            nexus_opens: Mutex::default(),
            next: Mutex::new(0),
        };
        (state, shown)
    }

    /// Notes what `window` shows. The session the person newly looks at, for the pull request watch's "Needs you".
    fn viewing(&self, window: u64, focused: bool, on_screen: Option<&str>) -> Option<String> {
        let mut windows = self.seen.lock().ok()?;
        let seen = windows.entry(window).or_default();
        let changed = seen.on_screen.as_deref() != on_screen || seen.focused != focused;
        seen.focused = focused;
        seen.on_screen = on_screen.map(String::from);
        on_screen.filter(|_| changed && focused).map(String::from)
    }

    fn closed(&self, window: u64) {
        if let Ok(mut windows) = self.seen.lock() {
            windows.remove(&window);
        }
    }

    fn clicked(&self, tag: &str) -> Option<Opens> {
        self.opens.lock().ok()?.get(tag).cloned()
    }

    fn agent_clicked(&self, tag: &str) -> Option<Option<String>> {
        self.agent_opens.lock().ok()?.get(tag).cloned()
    }

    fn nexus_clicked(&self, tag: &str) -> Option<Option<String>> {
        self.nexus_opens.lock().ok()?.get(tag).cloned()
    }

    fn nexus(&self, notice: nexus_desktop_core::path_notices::Notice) {
        let Some(tag) = self.tag("nexus") else { return };
        if let Ok(mut opens) = self.nexus_opens.lock() {
            opens.insert(tag.clone(), notice.opens);
        }
        let _ = self.out.unbounded_send(Shown {
            tag,
            title: notice.title,
            body: notice.body,
        });
    }

    fn looking_at(&self, thread_id: &str) -> bool {
        self.seen
            .lock()
            .is_ok_and(|windows| looking(&windows) == (true, Some(thread_id.to_string())))
    }

    /// The next tag under `prefix`: `session:3`, `agent:4`.
    fn tag(&self, prefix: &str) -> Option<String> {
        let mut next = self.next.lock().ok()?;
        *next += 1;
        Some(format!("{prefix}:{next}"))
    }

    fn agent(&self, kind: &str, conversation: Option<String>, assistant: &str) {
        let Some((title, body)) = nexus_desktop_core::agent::named_notice(kind, assistant) else {
            return;
        };
        let focused = match self.seen.lock() {
            Ok(windows) => looking(&windows).0,
            Err(_) => return,
        };
        if !enabled() || focused {
            return;
        }
        let Some(tag) = self.tag("agent") else { return };
        if let Ok(mut opens) = self.agent_opens.lock() {
            opens.insert(tag.clone(), conversation);
        }
        let _ = self.out.unbounded_send(Shown {
            tag,
            title,
            body: body.into(),
        });
    }

    fn show(&self, machine: Option<&str>, thread_id: &str, title: &str, body: &str, pull: bool) {
        let (focused, on_screen) = match self.seen.lock() {
            Ok(windows) => looking(&windows),
            Err(_) => return,
        };
        if !wanted(
            enabled(),
            focused,
            on_screen.as_deref(),
            &screen_key(machine, thread_id),
        ) {
            return;
        }
        let Some(tag) = self.tag("session") else { return };
        // A server's session opens by its id too: ids are unique across machines.
        if let Ok(mut opens) = self.opens.lock() {
            opens.insert(
                tag.clone(),
                Opens {
                    thread_id: thread_id.to_string(),
                    pull,
                },
            );
        }
        let _ = self.out.unbounded_send(Shown {
            tag,
            title: title.to_string(),
            body: body.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_on_screen_is_not_notified_and_a_click_opens_the_others() {
        // Its own state, not the app's: other tests notify too (a socket test's secret request), and their
        // notifications would land in this queue between two reads.
        let (state, mut shown) = State::new();
        assert_eq!(
            state.viewing(1, true, Some("t-1")),
            Some("t-1".into()),
            "the session newly looked at reaches the pull request watch"
        );
        // A second window, not focused, shows another session: the focused one still counts.
        state.viewing(2, false, Some("t-2"));
        assert!(state.looking_at("t-1"));
        assert!(!state.looking_at("t-2"));
        state.show(None, "t-1", "Fix the tests", "Finished", false);
        state.show(None, "t-2", "Write the docs", "Needs your approval", false);
        let first = shown.try_recv().expect("t-2 is shown");
        assert_eq!(first.title, "Write the docs");
        assert_eq!(
            state.clicked(&first.tag),
            Some(Opens {
                thread_id: "t-2".into(),
                pull: false
            })
        );
        // A pull request change opens the session's pull request page.
        state.show(None, "t-2", "Write the docs", "A check failed", true);
        let pull = shown.try_recv().expect("shown");
        assert_eq!(state.clicked(&pull.tag).map(|opens| opens.pull), Some(true));
        assert!(shown.try_recv().is_err(), "t-1 was on screen");
        // The window lost the focus: the session on screen is notified too.
        state.viewing(1, false, Some("t-1"));
        assert!(!state.looking_at("t-1"));
        // The second window takes the focus: its session is the one looked at, until it closes.
        state.viewing(2, true, Some("t-2"));
        assert!(state.looking_at("t-2"));
        state.closed(2);
        assert!(!state.looking_at("t-2"));
        state.show(Some("box"), "t-1", "Fix the tests", "Finished", false);
        let remote = shown.try_recv().expect("shown");
        assert_eq!(
            state.clicked(&remote.tag).map(|opens| opens.thread_id),
            Some("t-1".into()),
            "a server's session opens here too"
        );

        // The Assistant's: only while no window has the focus, with no words of the reply, and a click opens its pane.
        state.viewing(1, true, None);
        state.agent("answer", Some("c1".into()), "Nexus");
        assert!(shown.try_recv().is_err(), "a focused window shows the answer itself");
        state.viewing(1, false, None);
        state.agent("the answer text", None, "Nexus");
        assert!(shown.try_recv().is_err(), "only an answer or an approval");
        state.agent("approval", Some("c1".into()), "Ada");
        let approval = shown.try_recv().expect("shown");
        // The core's words (`nexus_desktop_core::agent::named_notice`), with the Assistant's name in the title.
        let (title, body) = nexus_desktop_core::agent::named_notice("approval", "Ada").expect("an approval's notice");
        assert_eq!(
            (approval.title.as_str(), approval.body.as_str()),
            (title.as_str(), body)
        );
        assert_eq!(approval.title, "Ada needs your approval");
        assert_eq!(state.agent_clicked(&approval.tag), Some(Some("c1".into())));
        assert_eq!(state.clicked(&approval.tag), None, "it opens no Code session");

        // The organization's: shown even with a window focused, and a click opens its page.
        state.viewing(1, true, None);
        state.nexus(nexus_desktop_core::path_notices::Notice {
            title: "Payment failed".into(),
            body: "Update the card.".into(),
            opens: Some("/app?next=%2Fbilling".into()),
        });
        let nexus = shown.try_recv().expect("shown");
        assert_eq!(
            (nexus.title.as_str(), nexus.body.as_str()),
            ("Payment failed", "Update the card.")
        );
        assert_eq!(
            state.nexus_clicked(&nexus.tag),
            Some(Some("/app?next=%2Fbilling".into()))
        );
        assert_eq!(state.clicked(&nexus.tag), None);
        assert_eq!(state.agent_clicked(&nexus.tag), None);
    }
}
