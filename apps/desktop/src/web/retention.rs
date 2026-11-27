//! Move-only CEF page ownership across window destruction. No GPUI view, window or subscription survives.
use super::{Engine, SharedRef, Startup, WebView, client::HostPage, protocol};
use gpui_kit::{Context, Task, Window};

pub(crate) struct RetainedPage {
    pub(super) page: HostPage,
    pub(super) shared: SharedRef,
    pub(super) next_call: u64,
    url: String,
    browser: Option<String>,
    profile: Option<protocol::WebProfile>,
    startup: Option<Startup>,
    document: bool,
}
impl WebView {
    pub(crate) fn can_transfer(&self) -> bool {
        matches!(self.engine, Some(Engine::Cef(_)))
    }
    pub(crate) fn cef_selected() -> bool {
        !super::servo::chosen()
    }
    pub(crate) fn retain(&mut self) -> Option<RetainedPage> {
        if !matches!(self.engine, Some(Engine::Cef(_))) {
            return None;
        }
        self._tick = Task::ready(());
        self._fills = None;
        self.fill_wake = None;
        self.calls.clear();
        let Some(Engine::Cef(page)) = self.engine.take() else {
            unreachable!()
        };
        page.send(protocol::Command::Focus {
            id: page.id(),
            focused: false,
        });
        page.send(protocol::Command::Hidden {
            id: page.id(),
            hidden: true,
        });
        let opened = {
            let mut shared = self.shared.lock().expect("web view state");
            shared.parked = true;
            shared.wake = None;
            shared.cdp.clear();
            shared.popups.clear();
            std::mem::take(&mut shared.opened)
        };
        for popup in opened {
            page.close_unattached(popup.id);
        }
        Some(RetainedPage {
            page,
            shared: self.shared.clone(),
            next_call: self.next_call,
            url: self.url.clone(),
            browser: self.browser.clone(),
            profile: self.profile.clone(),
            startup: self.startup,
            document: self.document,
        })
    }
    pub(crate) fn restore(saved: RetainedPage, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url = saved
            .shared
            .lock()
            .expect("web view state")
            .address
            .clone()
            .unwrap_or_else(|| saved.url.clone());
        let browser = saved.browser.clone();
        let profile = saved.profile.clone();
        let startup = saved.startup;
        let document = saved.document;
        Self::attach(
            &url,
            browser,
            profile,
            startup,
            None,
            document,
            None,
            Some(saved),
            false,
            window,
            cx,
        )
    }
}

#[cfg(test)]
pub(crate) struct PageProbe {
    pages: super::client::Pages,
    shared: SharedRef,
    id: u32,
}
#[cfg(test)]
impl PageProbe {
    pub(crate) fn alive(&self) -> bool {
        self.pages.lock().unwrap().contains_key(&self.id)
    }
    pub(crate) fn parked(&self) -> bool {
        self.shared.lock().unwrap().parked
    }
    pub(crate) fn address(&self, url: &str) {
        let mut shared = self.shared.lock().unwrap();
        shared.address = Some(url.into());
        shared.changed();
    }
}
#[cfg(test)]
pub(crate) fn fixture_page(id: u32) -> (RetainedPage, PageProbe) {
    let shared: SharedRef = Default::default();
    let (page, pages) = super::client::fixture_page(id, shared.clone());
    (
        RetainedPage {
            page,
            shared: shared.clone(),
            next_call: super::CALL_IDS,
            url: "https://fixture.invalid/form".into(),
            browser: Some("retained-session".into()),
            profile: None,
            startup: None,
            document: false,
        },
        PageProbe { pages, shared, id },
    )
}

#[cfg(test)]
mod tests {
    use super::fixture_page;
    use crate::web::{Engine, WebView};
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn retained_page_rebinds_fresh_window_and_cancels_old_cdp(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (saved, probe) = fixture_page(71);
        let (saved, old_id, old_call_id) = {
            let (view, cx) = cx.add_window_view(|window, cx| WebView::restore(saved, window, cx));
            let id = view.entity_id();
            let result = view.update_in(cx, |view, window, _| {
                let mut pending = view.call("Runtime.evaluate", serde_json::json!({"expression":"1"}));
                let call_id = view.next_call;
                let retained = view.retain().unwrap();
                assert!(pending.try_recv().is_err(), "old callbacks must be cancelled");
                assert!(view.engine.is_none());
                assert!(view.calls.is_empty());
                window.remove_window();
                (retained, id, call_id)
            });
            drop(view);
            result
        };
        assert!(probe.alive());
        assert!(probe.parked());
        probe.address("https://fixture.invalid/history/2");
        let (view, cx) = cx.add_window_view(|window, cx| WebView::restore(saved, window, cx));
        assert_ne!(view.entity_id(), old_id);
        view.update_in(cx, |view, window, cx| {
            assert!(matches!(&view.engine, Some(Engine::Cef(page)) if page.id() == 71));
            assert_eq!(view.address().as_deref(), Some("https://fixture.invalid/history/2"));
            let mut fresh = view.call("Runtime.evaluate", serde_json::json!({"expression":"2"}));
            assert!(view.next_call > old_call_id);
            // A reply already in flight from the destroyed view cannot satisfy the new call.
            {
                let mut state = view.shared.lock().unwrap();
                state
                    .cdp
                    .push(serde_json::json!({"id":old_call_id,"result":{"stale":true}}).to_string());
                state.changed();
            }
            view.take_changes(window, cx);
            assert_eq!(fresh.try_recv().unwrap(), None);
            {
                let mut state = view.shared.lock().unwrap();
                state
                    .cdp
                    .push(serde_json::json!({"id":view.next_call,"result":{"fresh":true}}).to_string());
                state.changed();
            }
            view.take_changes(window, cx);
            assert_eq!(fresh.try_recv().unwrap(), Some(Ok(serde_json::json!({"fresh":true}))));
            let saved = view.retain().unwrap();
            assert!(probe.parked());
            drop(saved);
            assert!(!probe.alive());
        });
    }
}
