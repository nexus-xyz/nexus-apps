//! Browser model and live CEF owners, without its old window-bound entities or tasks.
use super::{Browser, Tabs, Width};
use crate::web::{RetainedPage, WebEvent, WebView};
use gpui_kit::{AppContext, Context, Task, Window};
use std::collections::HashMap;

pub(crate) struct RetainedBrowser {
    session: String,
    place: Option<super::Place>,
    profile: nexus_browser::profile::Profile,
    tabs: Tabs,
    pages: HashMap<String, RetainedPage>,
    found: Vec<String>,
    probes: Vec<nexus_browser::preview::Probe>,
    typed: bool,
    navigation_managed: bool,
    pending_open: Option<String>,
    agent_control: bool,
    agent_paused: bool,
    activity_runs: std::collections::BTreeMap<String, super::agent::Guard>,
    activity_source: Option<super::agent::Guard>,
    agent_tab: Option<String>,
    agent_name: String,
    title: String,
    width: Width,
    next_capture: u64,
    next_import: u64,
}
impl Browser {
    /// Freeze the already-bound local authority before retiring its native presentation.
    pub(crate) fn local_owner(
        &self,
    ) -> Option<(
        super::agent::LocalOrigin,
        crate::windows::retention::Identity,
        super::agent::Guard,
    )> {
        if !matches!(self.place, Some(super::Place::Local)) {
            return None;
        }
        let super::agent::PermissionOwner::Local { origin, identity, .. } = &self.permission_owner else {
            return None;
        };
        Some((origin.clone(), identity.clone(), self.permission_guard.clone()?))
    }

    pub(crate) fn is_retired(&self) -> bool {
        self.retired
    }
    /// Validate every page before moving any owner; Servo and sign-in popup windows stay outside detachment.
    pub(crate) fn detach_eligible(&self, cx: &gpui_kit::App) -> Result<(), String> {
        if self.retired || !matches!(self.place, Some(nexus_browser::place::Place::Local)) {
            return Err("Only an active local Code session browser can move to a window.".into());
        }
        if !WebView::cef_selected() || self.pages.values().any(|(page, _)| !page.read(cx).can_transfer()) {
            return Err("Move to window requires live CEF pages.".into());
        }
        if !self.popups.is_empty() {
            return Err("Close the browser's sign-in popup before moving its tabs.".into());
        }
        Ok(())
    }
    pub(crate) fn retain(&mut self, cx: &mut Context<Self>) -> RetainedBrowser {
        let mut activity_runs = crate::activity::retain(&self.permission_instance, cx);
        activity_runs.extend(std::mem::take(&mut self.pending_activity_runs));
        self.retired = true;
        self._checks = Task::ready(());
        self._console = Task::ready(());
        self._subscriptions.clear();
        self.capture = None;
        self.import = None;
        self.cookie_form = None;
        self.close_popups(None, cx);
        self.retire_permissions();
        for tab in &mut self.tabs.tabs {
            tab.control(tab.agent_allowed); // New epoch rejects actions issued by the old view.
            tab.activity = None;
        }
        let pages = std::mem::take(&mut self.pages)
            .into_iter()
            .filter_map(|(id, (page, subscription))| {
                drop(subscription);
                page.update(cx, |page, _| page.retain()).map(|page| (id, page))
            })
            .collect();
        RetainedBrowser {
            session: self.session.clone(),
            place: self.place.clone(),
            profile: self.profile,
            tabs: std::mem::take(&mut self.tabs),
            pages,
            found: std::mem::take(&mut self.found),
            probes: std::mem::take(&mut self.probes),
            typed: self.typed,
            navigation_managed: self.navigation_managed,
            pending_open: self.pending_open.take(),
            agent_control: self.agent_control,
            agent_paused: self.agent_paused,
            activity_runs,
            activity_source: self.activity_source.clone(),
            agent_tab: self.agent_tab.take(),
            agent_name: self.agent_name.clone(),
            title: self.title.clone(),
            width: self.width,
            next_capture: self.next_capture,
            next_import: self.next_import,
        }
    }
    pub(crate) fn restore(saved: RetainedBrowser, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut browser = Self::new(saved.session, saved.place, saved.profile, None, window, cx);
        browser.tabs = saved.tabs;
        browser.found = saved.found;
        browser.probes = saved.probes;
        browser.typed = saved.typed;
        browser.navigation_managed = saved.navigation_managed;
        browser.pending_open = saved.pending_open;
        browser.agent_control = saved.agent_control;
        browser.agent_paused = saved.agent_paused;
        browser.pending_activity_runs = saved.activity_runs;
        browser.activity_source = saved.activity_source;
        browser.agent_tab = saved.agent_tab;
        browser.agent_name = saved.agent_name;
        browser.title = saved.title;
        browser.width = saved.width;
        browser.next_capture = saved.next_capture;
        browser.next_import = saved.next_import;
        for (id, retained) in saved.pages {
            let page = cx.new(|cx| WebView::restore(retained, window, cx));
            let tab = id.clone();
            let subscription = cx.subscribe_in(&page, window, move |this, _, event: &WebEvent, window, cx| {
                this.on_page(&tab, event, window, cx)
            });
            browser.pages.insert(id, (page, subscription));
        }
        browser.show_current(window, cx);
        let entity = cx.entity();
        let owner = window.window_handle();
        cx.defer(move |cx| {
            // The model owns credentials until registration: constructor rollback can retain it before this runs.
            let runs = entity.update(cx, |browser, _| {
                if browser.is_retired() {
                    return std::collections::BTreeMap::new();
                }
                std::mem::take(&mut browser.pending_activity_runs)
            });
            if !runs.is_empty() && !runs.values().any(|valid| valid()) {
                entity.update(cx, |browser, cx| {
                    if !browser.is_retired() {
                        browser.take_over_agent(cx);
                    }
                });
                return;
            }
            for (run, valid) in runs {
                crate::activity::using(&entity, &run, valid, owner, cx);
            }
        });
        browser
    }
}

#[cfg(test)]
impl Browser {
    pub(crate) fn fixture_cloud_identity(&mut self, machine: &str, profile: nexus_browser::profile::Profile) {
        self.place = Some(super::Place::Cloud(machine.into()));
        self.profile = profile;
    }
    pub(crate) fn fixture_local(
        session: &str,
        saved: RetainedPage,
        profile: nexus_browser::profile::Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut browser =
            Self::fixture_retained_profile(session, saved, Some(super::Place::Local), profile, window, cx);
        browser.allow_agent();
        browser
    }
    pub(crate) fn fixture_add_retained_page(
        &mut self,
        saved: RetainedPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self
            .tabs
            .add("https://fixture.invalid/retained-authority")
            .expect("retained page model");
        let id = self.tabs.tabs[index].id.clone();
        let page = cx.new(|cx| WebView::restore(saved, window, cx));
        let tab = id.clone();
        let subscription = cx.subscribe_in(&page, window, move |this, _, event: &WebEvent, window, cx| {
            this.on_page(&tab, event, window, cx)
        });
        self.pages.insert(id, (page, subscription));
    }
    pub(crate) fn fixture_retained(
        session: &str,
        saved: RetainedPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::fixture_retained_profile(
            session,
            saved,
            None,
            nexus_browser::profile::Profile::new("fixture", "isolated", false),
            window,
            cx,
        )
    }
    fn fixture_retained_profile(
        session: &str,
        saved: RetainedPage,
        place: Option<super::Place>,
        profile: nexus_browser::profile::Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut browser = Self::new(session.into(), place, profile, None, window, cx);
        let index = browser.tabs.add("https://fixture.invalid/form").unwrap();
        let id = browser.tabs.tabs[index].id.clone();
        let page = cx.new(|cx| WebView::restore(saved, window, cx));
        let tab = id.clone();
        let sub = cx.subscribe_in(&page, window, move |this, _, event: &WebEvent, window, cx| {
            this.on_page(&tab, event, window, cx)
        });
        browser.pages.insert(id, (page, sub));
        browser
    }
}

#[cfg(test)]
mod tests {
    use super::Browser;
    use crate::web::{WebEvent, WebView, fixture_page};
    use gpui_kit::{AppContext, TestAppContext};
    use nexus_browser::profile::Profile;

    #[gpui_kit::test]
    fn retained_browser_moves_pages_tabs_selection_and_rejects_old_actions(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
                crate::palette::ThemeChoice::Light,
                gpui_kit::WindowAppearance::Light,
            )));
        });
        let (first, first_probe) = fixture_page(81);
        let (second, second_probe) = fixture_page(82);
        let (saved, old_entity, ids, old_epoch) = {
            let (browser, cx) = cx.add_window_view(|window, cx| {
                let mut browser = Browser::new(
                    "retained-session".into(),
                    None,
                    Profile::new("fixture", "isolated", false),
                    None,
                    window,
                    cx,
                );
                for (url, retained) in [
                    ("https://fixture.invalid/one", first),
                    ("https://fixture.invalid/two", second),
                ] {
                    let index = browser.tabs.add(url).unwrap();
                    let id = browser.tabs.tabs[index].id.clone();
                    let page = cx.new(|cx| WebView::restore(retained, window, cx));
                    let tab = id.clone();
                    let sub = cx.subscribe_in(&page, window, move |this, _, event: &WebEvent, window, cx| {
                        this.on_page(&tab, event, window, cx);
                    });
                    browser.pages.insert(id, (page, sub));
                }
                browser.tabs.control_all(true);
                browser.tabs.closed.push("https://fixture.invalid/closed".into());
                browser
            });
            let old_entity = browser.entity_id();
            browser.update_in(cx, |browser, window, cx| {
                let ids: Vec<_> = browser.tabs.tabs.iter().map(|tab| tab.id.clone()).collect();
                let epoch = browser.tabs.tabs[0].begin("browser_read").unwrap();
                let saved = browser.retain(cx);
                assert!(browser.retired);
                assert!(browser.pages.is_empty());
                assert!(browser.tabs.tabs.is_empty());
                window.remove_window();
                (saved, old_entity, ids, epoch)
            })
        };
        assert!(first_probe.alive() && second_probe.alive());
        assert!(first_probe.parked() && second_probe.parked());
        let (browser, cx) = cx.add_window_view(|window, cx| Browser::restore(saved, window, cx));
        assert_ne!(browser.entity_id(), old_entity);
        let call = browser.update_in(cx, |browser, window, cx| {
            browser.agent(
                "browser_tabs".into(),
                serde_json::json!({}),
                "fixture-run".into(),
                std::sync::Arc::new(|| true),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        let answer = futures::FutureExt::now_or_never(call)
            .expect("fresh restored browser must answer")
            .unwrap();
        assert_eq!(answer["tabs"].as_array().unwrap().len(), 2);
        browser.update_in(cx, |browser, _, cx| {
            assert_eq!(browser.session, "retained-session");
            assert_eq!(
                browser.tabs.tabs.iter().map(|tab| tab.id.clone()).collect::<Vec<_>>(),
                ids
            );
            assert_eq!(browser.tabs.current, 1);
            assert_eq!(browser.tabs.closed, ["https://fixture.invalid/closed"]);
            assert!(browser.tabs.trust_all);
            assert_eq!(browser.pages.len(), 2);
            assert!(!browser.tabs.tabs[0].finish(old_epoch));
            let saved = browser.retain(cx);
            drop(saved);
            assert!(!first_probe.alive() && !second_probe.alive());
        });
    }
}
