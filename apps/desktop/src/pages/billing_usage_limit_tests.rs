#[path = "budgets_test_support.rs"]
mod support;
use super::UsageLimit;
use crate::pages::Load;
use gpui_kit::{AnyWindowHandle, AppContext, Entity, TestAppContext};
use serde_json::{Value, json};
use std::{cell::Cell, rc::Rc, sync::Arc};
use support::{answer, backend::BackendFixture};
fn fixture(cx: &mut TestAppContext) -> (Entity<UsageLimit>, Arc<BackendFixture>, AnyWindowHandle, Rc<Cell<bool>>) {
    support::init(cx);
    let gateway = BackendFixture::new();
    let mutation = Rc::new(Cell::new(false));
    let mut page = None;
    let window = cx.add_window(|window, cx| {
        let entity = cx.new(|cx| UsageLimit::new(support::ctx(window, gateway.clone()), mutation.clone(), window, cx));
        page = Some(entity.clone());
        support::Host(entity.into())
    });
    cx.run_until_parked();
    (page.unwrap(), gateway, window.into(), mutation)
}
fn billing(enabled: bool) -> Value {
    json!({"afterTheFact":{"enabled":enabled,"limitUsd":"25.00","monthSpentUsd":"3.12"},
        "pricing":{"defaultLimitUsd":"25.00"}})
}
fn allowed(gateway: &BackendFixture, cx: &mut TestAppContext) {
    answer(
        gateway.take("/v1/tenants/me"),
        json!({"tenant":{"kind":"company"},"role":"member","permissions":["org.billing"]}),
        cx,
    );
}
#[gpui_kit::test]
fn budgets_card_limit_uses_billing_permission_decimal_contract_and_shared_mutation_guard(cx: &mut TestAppContext) {
    let (page, gateway, window, mutation) = fixture(cx);
    allowed(&gateway, cx);
    answer(gateway.take("/v1/billing"), billing(true), cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.edit(window, cx);
            p.amount.update(cx, |state, cx| state.set_value("0", window, cx));
            p.save(cx);
            assert!(p.error.is_some());
            p.amount.update(cx, |state, cx| state.set_value("75.50", window, cx));
            mutation.set(true);
            p.save(cx);
            mutation.set(false);
            assert!(!p.busy);
            p.save(cx);
            p.save(cx);
        })
    })
    .unwrap();
    assert!(mutation.get());
    let write = gateway.take("/v1/billing/after-the-fact");
    assert!(matches!(write.verb, crate::gateway::Verb::Put));
    assert_eq!(write.body, Some(json!({"enabled":true,"limitUsd":"75.50"})));
    assert!(
        !gateway
            .pending
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.path == "/v1/billing/after-the-fact")
    );
    answer(write, json!({}), cx);
    assert!(!mutation.get());
    assert!(!page.read_with(cx, |p, _| p.editing));
    answer(gateway.take("/v1/billing"), billing(true), cx);
}
#[gpui_kit::test]
fn budgets_card_limit_does_not_query_billing_from_owner_label_without_permission(cx: &mut TestAppContext) {
    let (page, gateway, window, _) = fixture(cx);
    answer(
        gateway.take("/v1/tenants/me"),
        json!({"tenant":{"kind":"company"},"role":"owner","permissions":[]}),
        cx,
    );
    assert!(gateway.pending.lock().unwrap().is_empty());
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.edit(window, cx);
            p.save(cx);
        })
    })
    .unwrap();
    assert!(gateway.pending.lock().unwrap().is_empty());
}
#[gpui_kit::test]
fn budgets_card_limit_rejects_old_reads_account_callbacks_and_implicit_payment_enablement(cx: &mut TestAppContext) {
    let (page, gateway, window, _) = fixture(cx);
    allowed(&gateway, cx);
    let old = gateway.take("/v1/billing");
    page.update(cx, |p, cx| p.reload(cx));
    let latest = gateway.take("/v1/billing");
    answer(latest, billing(false), cx);
    answer(old, billing(true), cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.edit(window, cx);
            p.save(cx);
        })
    })
    .unwrap();
    // The latest read is off: the limit still changes, and saving it never turns paying after the fact on.
    let off = gateway.take("/v1/billing/after-the-fact");
    assert_eq!(off.body, Some(json!({"enabled":false,"limitUsd":"25"})));
    answer(off, json!({}), cx);
    assert!(!page.read_with(cx, |p, _| p.editing));
    answer(gateway.take("/v1/billing"), billing(true), cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.edit(window, cx);
            p.save(cx);
        })
    })
    .unwrap();
    let write = gateway.take("/v1/billing/after-the-fact");
    *gateway.marker.lock().unwrap() = Some("account-b".into());
    answer(write, json!({}), cx);
    assert!(page.read_with(cx, |p, _| p.busy && p.editing));
    assert!(!gateway.pending.lock().unwrap().iter().any(|r| r.path == "/v1/billing"));
    assert!(page.read_with(cx, |p, _| matches!(p.billing, Load::Ready(_))));
}

#[gpui_kit::test]
fn budgets_card_limit_recovers_from_bad_reply_and_preserves_disabled_payment_mode(cx: &mut TestAppContext) {
    let (page, gateway, window, _) = fixture(cx);
    allowed(&gateway, cx);
    answer(gateway.take("/v1/billing"), json!([]), cx);
    assert!(page.read_with(cx, |p, _| matches!(p.billing, Load::Failed(_))));
    page.update(cx, |p, cx| p.reload(cx));
    answer(gateway.take("/v1/billing"), billing(false), cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.edit(window, cx);
            p.amount.update(cx, |state, cx| state.set_value("40", window, cx));
            p.save(cx);
        })
    })
    .unwrap();
    // A limit changes while paying after the fact is off (as on the web), and the setting stays off.
    let write = gateway.take("/v1/billing/after-the-fact");
    assert_eq!(write.body, Some(json!({"enabled":false,"limitUsd":"40"})));
    answer(write, json!({}), cx);
    assert!(page.read_with(cx, |p, _| !p.editing));
}

/// The most a limit may be is Nexus's (`pricing.maxLimitUsd`): $100 in an organization's first week.
#[gpui_kit::test]
fn budgets_card_limit_reads_the_most_a_limit_may_be(cx: &mut TestAppContext) {
    let (page, gateway, window, _) = fixture(cx);
    allowed(&gateway, cx);
    let mut first_week = billing(true);
    first_week["pricing"]["maxLimitUsd"] = json!("100.000000");
    answer(gateway.take("/v1/billing"), first_week, cx);
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.edit(window, cx);
            p.amount.update(cx, |state, cx| state.set_value("150", window, cx));
            p.save(cx);
            assert_eq!(p.error.as_deref(), Some("The limit is at most $100."));
        })
    })
    .unwrap();
    assert!(gateway.pending.lock().unwrap().is_empty());
}
