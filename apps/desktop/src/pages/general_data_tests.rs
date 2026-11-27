//! Data navigation uses General's real controls and API adapter, including delayed layout and focused inputs.
#[path = "general_data_test_backend.rs"]
mod backend;
use super::super::{Ctx, Load, Theme};
use super::{GeneralPage, Part, Sharing, changed, parse_settings};
use backend::{BackendFixture, Pending};
use gpui_kit::{
    AnyWindowHandle, AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, StatefulInteractiveElement, Styled, TestAppContext, Window, WindowAppearance, div,
};
use serde_json::{Value, json};
use std::{
    rc::Rc,
    sync::{Arc, atomic::Ordering},
};

struct Host {
    page: Entity<GeneralPage>,
    scroll: ScrollHandle,
}
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("data-fixture-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(self.page.clone())
    }
}
fn fixture(cx: &mut TestAppContext) -> (Entity<GeneralPage>, Arc<BackendFixture>, AnyWindowHandle) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(Theme(crate::palette::Palette::for_choice(
            crate::palette::ThemeChoice::System,
            WindowAppearance::Light,
        )));
    });
    let gateway = BackendFixture::new();
    let mut page = None;
    let window = cx.add_window(|window, cx| {
        let scroll = ScrollHandle::new();
        let ctx = Ctx {
            device: crate::pages::DeviceCtx {
                local_workspace: false,
                settings_fields: crate::settings_fields::SettingsFields::new(window, scroll.clone()),
                gateway_url: "http://gateway.invalid".into(),
                navigate: Rc::new(|_, _, _| {}),
                navigate_settings: Rc::new(|_, _, _| {}),
                switch_gateway: Rc::new(|_, _, _| Ok(())),
                settings_saved: Rc::new(|_, _| {}),
                restart_for_update: Rc::new(|_| Ok(())),
                restart_for_service: Rc::new(|_| Ok(())),
                code: Rc::new(|_| super::super::CodeAccess::default()),
                new_code_session: Rc::new(|_, _| {}),
            },
            gateway: crate::workspace::gateway_access::AccountGateway::fixture(gateway.clone()),
            org: "example".into(),
            org_name: "Example".into(),
            email: "a@example.test".into(),
            new_cloud_session: Rc::new(|_, _| {}),
            team: Rc::new(|_| Default::default()),
            watch: Rc::new(|_, _, _| {}),
            fixer: crate::setup::Fixer::inert(),
        };
        let entity = cx.new(|cx| GeneralPage::new(ctx, window, cx));
        page = Some(entity.clone());
        Host { page: entity, scroll }
    });
    cx.run_until_parked();
    (page.unwrap(), gateway, window.into())
}
fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }
    cx.run_until_parked();
}
fn settings(queries: bool) -> Value {
    json!({"chatRetentionDays":30,"defaults":{"chatRetentionDays":90},"artifacts.external":"public",
        "artifacts.pages":true,"artifacts.pages.public":false,"artifacts.pages.queries":queries})
}
fn me(manage: bool) -> Value {
    json!({"tenant":{"id":"org-a","slug":"example","name":"Example","kind":"company"},"role":"owner",
        "permissions": if manage { vec!["org.settings"] } else { vec![] }})
}
fn answer(pending: Pending, value: Value, cx: &mut TestAppContext) {
    let consumed = pending.consumed.clone();
    pending.answer.send(Ok(value)).unwrap();
    cx.run_until_parked();
    assert!(consumed.load(Ordering::SeqCst), "the exact pending response must run");
}
fn settle_other(gateway: &BackendFixture, manage: bool, cx: &mut TestAppContext) {
    answer(gateway.take("/v1/tenants/me"), me(manage), cx);
    answer(gateway.take("/v1/policies/review-events"), json!({}), cx);
    answer(gateway.take("/v1/models?status=all"), json!({"data":[]}), cx);
}
fn fill(page: &Entity<GeneralPage>, cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |page, cx| page.fill(window, cx))
    })
    .unwrap();
}

#[gpui_kit::test]
fn data_retention_anchor_waits_for_settings_then_focuses_the_exact_native_input(cx: &mut TestAppContext) {
    let (page, gateway, window) = fixture(cx);
    page.update(cx, |page, cx| {
        page.ctx.settings_fields.request("retention-days");
        cx.notify();
    });
    draw(cx, window);
    assert_eq!(
        page.read_with(cx, |page, _| page.ctx.settings_fields.requested_target()),
        Some("retention-days")
    );
    settle_other(&gateway, true, cx);
    draw(cx, window);
    assert_eq!(
        page.read_with(cx, |page, _| page.ctx.settings_fields.requested_target()),
        Some("retention-days")
    );
    answer(gateway.take("/v1/orgs/settings"), settings(false), cx);
    draw(cx, window);
    assert_eq!(
        page.read_with(cx, |page, _| page.ctx.settings_fields.requested_target()),
        None
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.ctx.settings_fields.highlighted_target()),
        Some("retention-days")
    );
    let focused = cx
        .update_window(window, |_, window, cx| {
            page.read(cx).retention.read(cx).focus_handle(cx).is_focused(window)
        })
        .unwrap();
    assert!(focused, "search must enter the retention input after deferred loading");
    page.update(cx, |page, cx| {
        page.ctx.settings_fields.request("sharing-settings");
        cx.notify();
    });
    draw(cx, window);
    assert_eq!(
        page.read_with(cx, |page, _| page.ctx.settings_fields.highlighted_target()),
        Some("sharing-settings")
    );
}

#[gpui_kit::test]
fn data_live_page_policy_sends_only_the_changed_existing_setting_and_serializes_writes(cx: &mut TestAppContext) {
    let (page, gateway, window) = fixture(cx);
    settle_other(&gateway, true, cx);
    answer(gateway.take("/v1/orgs/settings"), settings(false), cx);
    fill(&page, cx, window);
    page.update(cx, |page, cx| {
        page.edit_sharing(|draft| draft.pages_queries = true, cx);
        page.save_sharing(cx);
        page.save_sharing(cx);
        page.save_retention(cx);
    });
    let write = gateway.take("/v1/orgs/settings");
    assert!(matches!(write.verb, crate::gateway::Verb::Put));
    assert_eq!(write.body, Some(json!({"artifacts.pages.queries":true})));
    assert!(gateway.pending.lock().unwrap().is_empty());
    answer(write, settings(true), cx);
    assert!(page.read_with(
        cx,
        |page, _| matches!(&page.settings,Load::Ready(settings) if settings.sharing.as_ref().unwrap().pages_queries)
    ));
}

#[gpui_kit::test]
fn data_owner_label_without_permission_cannot_edit_or_save(cx: &mut TestAppContext) {
    let (page, gateway, window) = fixture(cx);
    settle_other(&gateway, false, cx);
    answer(gateway.take("/v1/orgs/settings"), settings(false), cx);
    fill(&page, cx, window);
    page.update(cx, |page, cx| {
        page.edit_sharing(|draft| draft.pages_queries = true, cx);
        page.save_sharing(cx);
        page.save_retention(cx);
        page.put_settings(
            Part::Sharing,
            json!({"artifacts.pages.queries":true}),
            "Saved".into(),
            cx,
        );
    });
    assert!(page.read_with(cx, |page, _| page.draft.is_none() && !page.can_manage()));
    assert!(gateway.pending.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn data_changed_accounts_and_superseded_settings_replies_do_not_reset_the_form(cx: &mut TestAppContext) {
    let (page, gateway, window) = fixture(cx);
    settle_other(&gateway, true, cx);
    let old = gateway.take("/v1/orgs/settings");
    page.update(cx, |page, cx| page.read_settings(cx));
    answer(gateway.take("/v1/orgs/settings"), settings(false), cx);
    fill(&page, cx, window);
    answer(old, settings(true), cx);
    assert!(page.read_with(
        cx,
        |page, _| matches!(&page.settings,Load::Ready(settings) if !settings.sharing.as_ref().unwrap().pages_queries)
    ));
    page.update(cx, |page, cx| {
        page.edit_sharing(|draft| draft.pages_queries = true, cx);
        page.save_sharing(cx);
    });
    let write = gateway.take("/v1/orgs/settings");
    *gateway.marker.lock().unwrap() = Some("account-b".into());
    answer(write, settings(true), cx);
    assert!(page.read_with(cx, |page, _| !page.current() && page.sharing_note.is_none()));
    page.update(cx, |page, cx| page.save_sharing(cx));
    assert!(gateway.pending.lock().unwrap().is_empty());
}

#[test]
fn data_live_queries_match_kit_defaults_and_are_not_sent_to_gateways_without_pages() {
    assert!(!Sharing::read(&json!({})).pages_queries);
    let current = Sharing::read(&settings(false));
    let mut next = current.clone();
    next.pages_queries = true;
    assert_eq!(
        changed(&current, &next, true),
        serde_json::from_value::<serde_json::Map<String, Value>>(json!({"artifacts.pages.queries":true})).unwrap()
    );
    assert!(changed(&current, &next, false).is_empty());
    assert!(
        parse_settings(&json!({"chatRetentionDays":90}))
            .unwrap()
            .sharing
            .is_none()
    );
}

#[gpui_kit::test]
fn data_retry_preserves_a_pending_anchor_until_the_failed_read_recovers(cx: &mut TestAppContext) {
    let (page, gateway, window) = fixture(cx);
    settle_other(&gateway, true, cx);
    page.update(cx, |page, cx| {
        page.ctx.settings_fields.request("retention-days");
        cx.notify();
    });
    let failed = gateway.take("/v1/orgs/settings");
    let consumed = failed.consumed.clone();
    failed
        .answer
        .send(Err(nexus_client::error::Transport::Other("offline".into()).into()))
        .unwrap();
    cx.run_until_parked();
    assert!(consumed.load(Ordering::SeqCst));
    draw(cx, window);
    assert_eq!(
        page.read_with(cx, |page, _| page.ctx.settings_fields.requested_target()),
        Some("retention-days")
    );
    page.update(cx, |page, cx| page.load(cx));
    settle_other(&gateway, true, cx);
    answer(gateway.take("/v1/orgs/settings"), settings(false), cx);
    draw(cx, window);
    assert_eq!(
        page.read_with(cx, |page, _| page.ctx.settings_fields.requested_target()),
        None
    );
    assert!(
        cx.update_window(window, |_, window, cx| page
            .read(cx)
            .retention
            .read(cx)
            .focus_handle(cx)
            .is_focused(window))
            .unwrap()
    );
}
