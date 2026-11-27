//! A session browser's tabs, as the Nexus app's (`preview-open.ts` and `browser.rs`): at most
//! [`MAX_TABS`], the one on screen, and the closed ones Reopen closed tab brings back. Each tab's page keeps its own
//! history in the engine, so Back and Forward are the page's.

use std::sync::atomic::{AtomicU64, Ordering};

pub use nexus_browser::MAX_TABS;

use crate::web::Loading;

/// The most closed tabs Reopen closed tab remembers.
pub const MAX_CLOSED: usize = 10;

/// What the pane says when every tab is taken.
pub const FULL: &str = "Close a browser tab before opening another.";

static SERIAL: AtomicU64 = AtomicU64::new(0);

/// A new tab's id: unique while the app runs, and only letters, digits and dashes, as the Nexus app's `newTabId`. The
/// agent's browser tools name tabs by it.
pub fn new_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64);
    format!("tab-{}-{}", base36(millis), SERIAL.fetch_add(1, Ordering::Relaxed) + 1)
}

fn base36(mut value: u64) -> String {
    let mut digits = Vec::new();
    loop {
        digits.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    digits.reverse();
    String::from_utf8(digits).unwrap_or_default()
}

/// One tab: its id, the address it shows (empty for a new tab), its page's title and loading state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tab {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    pub loading: Loading,
    /// Whether the agent may drive this tab, the tool it runs now, and the control epoch: "Allow agent control",
    /// "Take over" and "Stop" each start a new one, which ends the action in progress (`nexus_browser::control`).
    pub agent_allowed: bool,
    pub agent_created: bool,
    pub prepared_for_agent: bool,
    pub activity: Option<String>,
    pub control_epoch: u64,
}

impl Tab {
    /// Allow or end the agent's control of this tab. Either ends its action in progress.
    pub fn control(&mut self, enabled: bool) {
        self.agent_allowed = enabled;
        self.control_epoch = self.control_epoch.wrapping_add(1);
        if !enabled {
            self.activity = None;
        }
    }

    /// The agent's tool `action` starts on this tab: its control epoch, which [`Tab::finish`] checks.
    pub fn begin(&mut self, action: &str) -> Result<u64, String> {
        nexus_browser::control::may_begin(
            self.agent_allowed,
            self.loading.loading,
            self.activity.is_some(),
            action,
        )?;
        self.prepared_for_agent = false;
        self.activity = Some(action.into());
        Ok(self.control_epoch)
    }

    /// The action ends: whether it may report its result (the tab is still the agent's, in the same epoch).
    pub fn finish(&mut self, epoch: u64) -> bool {
        if self.control_epoch != epoch {
            return false;
        }
        self.activity = None;
        self.agent_allowed
    }

    fn new(url: &str) -> Tab {
        Tab {
            id: new_id(),
            url: url.to_string(),
            ..Tab::default()
        }
    }

    /// Its name in the tab strip: the page's title, else its host, else "New tab".
    pub fn label(&self) -> String {
        self.title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| super::address::tab_label(&self.url))
    }
}

/// Where an address asked for from outside the pane (a link's menu) goes: [`Tabs::ask`].
#[derive(Debug, PartialEq, Eq)]
pub enum Asked {
    /// The tab that already shows it, selected.
    Shown(usize),
    /// This tab, which was blank or is the one on screen: open the address in it.
    Visit(usize),
}

#[derive(Debug, Default)]
pub struct Tabs {
    pub tabs: Vec<Tab>,
    pub current: usize,
    /// The addresses of closed tabs, the last closed last.
    pub closed: Vec<String>,
    /// "Trust all tabs": every tab, a new one too, allows agent control until the person takes over. A local Code
    /// session's browser starts trusted, as in the Nexus app.
    pub trust_all: bool,
}

impl Tabs {
    pub fn current(&self) -> Option<&Tab> {
        self.tabs.get(self.current)
    }

    pub fn current_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.current)
    }

    pub fn find(&self, id: &str) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }

    /// Keep an explicit live target; after it closes, choose only another authorized agent resource.
    pub fn agent_target(&self, active: Option<&str>) -> Option<&Tab> {
        active.and_then(|id| self.find(id)).or_else(|| {
            self.tabs
                .iter()
                .rev()
                .find(|tab| tab.agent_created && tab.agent_allowed)
        })
    }

    /// Trust all tabs, or end it: every tab follows.
    pub fn control_all(&mut self, enabled: bool) {
        self.trust_all = enabled;
        for tab in &mut self.tabs {
            tab.control(enabled);
        }
    }

    /// The person's choice for tab `id` (else the tab on screen). Taking over while every tab is trusted ends the trust,
    /// as in the Nexus app.
    pub fn control(&mut self, id: Option<&str>, enabled: bool) {
        if !enabled && self.trust_all {
            self.control_all(false);
            return;
        }
        let id = id
            .map(str::to_string)
            .or_else(|| self.current().map(|tab| tab.id.clone()));
        if let Some(tab) = id.and_then(|id| self.find_mut(&id)) {
            tab.control(enabled);
        }
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }

    pub fn full(&self) -> bool {
        self.tabs.len() >= MAX_TABS
    }

    /// A new tab at `url` (empty for a blank one), on screen. [`FULL`] when every tab is taken.
    pub fn add(&mut self, url: &str) -> Result<usize, &'static str> {
        if self.full() {
            return Err(FULL);
        }
        let mut tab = Tab::new(url);
        tab.agent_allowed = self.trust_all;
        self.tabs.push(tab);
        self.current = self.tabs.len() - 1;
        Ok(self.current)
    }

    pub fn select(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.current = index;
        }
    }

    /// Close tab `index`; the tab on screen stays, or its neighbour takes its place. A tab with a page is remembered
    /// for Reopen closed tab; a blank one is not worth it.
    pub fn close(&mut self, index: usize) -> Option<Tab> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if !tab.url.is_empty() {
            self.closed.push(tab.url.clone());
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
        }
        let current = self.current.saturating_sub(usize::from(index < self.current));
        self.current = current.min(self.tabs.len().saturating_sub(1));
        Some(tab)
    }

    /// Open the last closed tab again, as a new tab on screen, while one is free.
    pub fn reopen(&mut self) -> Option<usize> {
        if self.full() {
            return None;
        }
        let url = self.closed.pop()?;
        self.add(&url).ok()
    }

    /// Where an address asked from outside goes, as the Nexus app's `withAsked`: the tab that shows it, else a blank
    /// tab, else a new tab, else (every tab taken) the tab on screen.
    pub fn ask(&mut self, url: &str) -> Asked {
        if let Some(open) = self.tabs.iter().position(|tab| tab.url == url) {
            self.current = open;
            return Asked::Shown(open);
        }
        if let Some(blank) = self.tabs.iter().position(|tab| tab.url.is_empty()) {
            self.current = blank;
            self.tabs[blank].url = url.to_string();
            return Asked::Visit(blank);
        }
        match self.add(url) {
            Ok(index) => Asked::Visit(index),
            Err(_) => {
                if self.tabs.is_empty() {
                    return Asked::Shown(0);
                }
                self.tabs[self.current].url = url.to_string();
                Asked::Visit(self.current)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn a_browser_holds_at_most_eight_tabs() {
        let mut tabs = Tabs::default();
        for index in 0..MAX_TABS {
            assert_eq!(tabs.add(&format!("http://localhost:{}", 3000 + index)), Ok(index));
        }
        assert_eq!(tabs.add("http://localhost:4000"), Err(FULL));
        assert_eq!(tabs.tabs.len(), MAX_TABS);
        assert_eq!(tabs.current, MAX_TABS - 1);
        let ids: std::collections::HashSet<_> = tabs.tabs.iter().map(|tab| tab.id.clone()).collect();
        assert_eq!(ids.len(), MAX_TABS, "each tab has its own id");
        assert!(
            ids.iter()
                .all(|id| id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
        );
    }

    #[test]
    fn closing_keeps_the_tab_on_screen_and_remembers_pages_only() {
        let mut tabs = Tabs::default();
        tabs.add("http://a.test/").unwrap();
        tabs.add("").unwrap();
        tabs.add("http://c.test/").unwrap();
        tabs.select(2);
        tabs.close(0);
        assert_eq!(tabs.current().map(|tab| tab.url.as_str()), Some("http://c.test/"));
        tabs.close(0);
        assert_eq!(tabs.closed, ["http://a.test/"], "a blank tab is not remembered");
        tabs.close(0);
        assert!(tabs.tabs.is_empty());
        assert_eq!(tabs.current, 0);
        assert_eq!(tabs.reopen(), Some(0));
        assert_eq!(tabs.current().map(|tab| tab.url.as_str()), Some("http://c.test/"));
        assert_eq!(tabs.closed, ["http://a.test/"]);
    }

    #[test]
    fn reopen_remembers_ten_and_waits_for_a_free_tab() {
        let mut tabs = Tabs::default();
        for index in 0..12 {
            tabs.add(&format!("http://t{index}.test/")).unwrap();
            tabs.close(0);
        }
        assert_eq!(tabs.closed.len(), MAX_CLOSED);
        assert_eq!(tabs.closed.first().map(String::as_str), Some("http://t2.test/"));
        for _ in 0..MAX_TABS {
            tabs.add("").unwrap();
        }
        assert_eq!(tabs.reopen(), None);
        assert_eq!(tabs.closed.len(), MAX_CLOSED);
    }

    #[test]
    fn an_asked_address_takes_its_tab_a_blank_tab_or_a_new_one() {
        let mut tabs = Tabs::default();
        tabs.add("http://a.test/").unwrap();
        tabs.add("").unwrap();
        tabs.select(0);
        assert_eq!(tabs.ask("http://a.test/"), Asked::Shown(0));
        assert_eq!(tabs.ask("http://b.test/"), Asked::Visit(1));
        assert_eq!(tabs.current, 1);
        assert_eq!(tabs.ask("http://c.test/"), Asked::Visit(2));
        while !tabs.full() {
            tabs.add("http://more.test/").unwrap();
        }
        tabs.select(0);
        assert_eq!(
            tabs.ask("http://d.test/"),
            Asked::Visit(0),
            "every tab taken: the tab on screen"
        );
        assert_eq!(tabs.tabs[0].url, "http://d.test/");
    }

    #[test]
    fn a_tab_is_named_by_its_title_else_its_host() {
        let mut tab = Tab::new("http://localhost:3000/a");
        assert_eq!(tab.label(), "localhost:3000");
        tab.title = Some("Dashboard".into());
        assert_eq!(tab.label(), "Dashboard");
        assert_eq!(Tab::new("").label(), "New tab");
    }

    #[test]
    fn take_over_ends_trust_and_the_action_in_progress() {
        let mut tabs = Tabs {
            trust_all: true,
            ..Tabs::default()
        };
        tabs.add("http://a.test/").unwrap();
        tabs.add("http://b.test/").unwrap();
        assert!(
            tabs.tabs.iter().all(|tab| tab.agent_allowed),
            "a trusted browser's tabs allow the agent"
        );
        let epoch = tabs.tabs[1].begin("browser_read").expect("allowed");
        assert!(tabs.tabs[1].begin("browser_click").is_err(), "one action at a time");
        tabs.control(None, false);
        assert!(!tabs.trust_all && tabs.tabs.iter().all(|tab| !tab.agent_allowed));
        assert!(!tabs.tabs[1].finish(epoch), "the person took over");
        tabs.control(Some(&tabs.tabs[0].id.clone()), true);
        assert!(tabs.tabs[0].agent_allowed && !tabs.tabs[1].agent_allowed);
        tabs.tabs[0].loading.loading = true;
        assert!(tabs.tabs[0].begin("browser_read").is_err(), "loading");
        let epoch = tabs.tabs[0].begin("browser_reload").expect("moves the page");
        assert!(tabs.tabs[0].finish(epoch));
    }
    #[test]
    fn a_closed_agent_target_does_not_poison_unnamed_calls_or_select_a_human_tab() {
        let mut tabs = Tabs::default();
        let human = tabs.add("https://human.test/").unwrap();
        let human_id = tabs.tabs[human].id.clone();
        let agent = tabs.add("https://agent.test/").unwrap();
        tabs.tabs[agent].agent_created = true;
        tabs.tabs[agent].agent_allowed = true;
        let agent_id = tabs.tabs[agent].id.clone();
        assert_eq!(
            tabs.agent_target(Some("closed-id")).map(|tab| tab.id.as_str()),
            Some(agent_id.as_str())
        );
        tabs.close(agent);
        assert!(tabs.agent_target(Some(&agent_id)).is_none());
        assert_eq!(tabs.current().map(|tab| tab.id.as_str()), Some(human_id.as_str()));
    }
    #[test]
    fn using_an_approved_tab_consumes_its_one_time_prepared_state() {
        let mut page = Tab {
            agent_allowed: true,
            agent_created: true,
            prepared_for_agent: true,
            ..Default::default()
        };
        page.begin("browser_open").unwrap();
        assert!(!page.prepared_for_agent);
        assert!(page.agent_allowed);
    }
}
