#[path = "budgets_test_support.rs"]
mod support;

use super::{Access, ConnectorsPage, Group, Load, Rights, group_cards};
use gpui_kit::{AnyWindowHandle, AppContext, Entity, TestAppContext};
use serde_json::{Value, json};
use std::sync::Arc;
use support::{answer, backend::BackendFixture};

fn fixture(cx: &mut TestAppContext) -> (Entity<ConnectorsPage>, Arc<BackendFixture>, AnyWindowHandle) {
    fixture_on(super::Side::Org, cx)
}

fn fixture_on(
    side: super::Side,
    cx: &mut TestAppContext,
) -> (Entity<ConnectorsPage>, Arc<BackendFixture>, AnyWindowHandle) {
    support::init(cx);
    let gateway = BackendFixture::new();
    let mut page = None;
    let window = cx.add_window(|window, cx| {
        let view = super::build(support::ctx(window, gateway.clone()), side, window, cx).unwrap();
        page = Some(view.clone().downcast::<ConnectorsPage>().ok().unwrap());
        support::Host(view)
    });
    cx.run_until_parked();
    (page.unwrap(), gateway, window.into())
}

fn availability(level: &str) -> Value {
    json!({"name":"github","availability":level,"teams":[],"default":false})
}

fn settle_refresh(gateway: &BackendFixture, level: &str, usable: bool, cx: &mut TestAppContext) {
    answer(
        gateway.take("/v1/tenants/me"),
        json!({"permissions":["policy.manage","connections.manage_own"]}),
        cx,
    );
    answer(
        gateway.take("/v1/connectors"),
        json!({"connectors":[{"name":"github","connects":[],"offered":true,"usable":usable}]}),
        cx,
    );
    answer(gateway.take("/v1/connections"), json!({"connections":[]}), cx);
    answer(
        gateway.take("/v1/policies/connectors"),
        json!({"connectors":[availability(level)]}),
        cx,
    );
    answer(gateway.take("/v1/policies/chat-writes"), json!({"enabled":false}), cx);
}

#[gpui_kit::test]
fn an_admins_own_page_reads_no_organization_rules(cx: &mut TestAppContext) {
    let (page, gateway, _window) = fixture_on(super::Side::Mine, cx);
    answer(
        gateway.take("/v1/tenants/me"),
        json!({"permissions":["policy.manage","connections.manage_own"]}),
        cx,
    );
    answer(
        gateway.take("/v1/connectors"),
        json!({"connectors":[{"name":"github","connects":["user"],"offered":true,"usable":true,
            "operations":[{"name":"repo","summary":"Repository metadata."}]}]}),
        cx,
    );
    answer(gateway.take("/v1/connections"), json!({"connections":[]}), cx);
    // Who can use each connector and chat's switch belong to the organization's page.
    assert!(gateway.pending.lock().unwrap().is_empty());
    page.read_with(cx, |page, _| {
        assert!(page.availability.is_none() && page.chat.is_none());
    });

    // Opening a row reads what agents can do with it, once.
    let card = page.read_with(cx, |page, _| {
        let Load::Ready(catalog) = &page.catalog else {
            panic!("the catalog must be ready")
        };
        super::card(&catalog[0], &[], None, Rights::default())
    });
    cx.update_window(_window, |_, window, cx| {
        page.update(cx, |page, cx| page.toggle(&card, window, cx));
    })
    .unwrap();
    answer(
        gateway.take("/v1/connectors/github"),
        json!({"operations":[{"name":"repo","summary":"Repository metadata.","effect":"read"}]}),
        cx,
    );
    page.read_with(cx, |page, _| {
        assert!(matches!(page.abilities.get("github"), Some(Load::Ready(list)) if list.len() == 1));
    });
    assert!(gateway.pending.lock().unwrap().is_empty());
}

#[gpui_kit::test]
fn successful_policy_changes_refresh_the_catalog_access_in_both_directions(cx: &mut TestAppContext) {
    let (page, gateway, _window) = fixture(cx);
    settle_refresh(&gateway, "disabled", false, cx);

    for (level, usable, access, group) in [
        ("available", true, Access::Usable, Group::On),
        ("disabled", false, Access::Unavailable, Group::Off),
    ] {
        page.update(cx, |page, cx| page.set_availability("github".into(), level, cx));
        let write = gateway.take("/v1/policies/connectors/github");
        assert!(matches!(write.verb, crate::gateway::Verb::Put));
        assert_eq!(write.body, Some(json!({"availability":level})));
        answer(write, availability(level), cx);

        // The successful write must initiate a fresh catalog read, not only patch the policy label.
        settle_refresh(&gateway, level, usable, cx);
        page.read_with(cx, |page, _| {
            let (Load::Ready(catalog), Some(Load::Ready(availability))) = (&page.catalog, &page.availability) else {
                panic!("the refreshed catalog and availability must be ready");
            };
            assert_eq!(catalog[0].usable, access);
            let groups = group_cards(
                catalog,
                &[],
                Some(availability),
                Rights {
                    manage: true,
                    own: true,
                },
            );
            assert_eq!(groups[0].0, group);
            assert_eq!(groups[0].1[0].connector.usable, access);
            assert!(page.refresh_errors.is_empty());
        });
    }
    assert!(gateway.pending.lock().unwrap().is_empty());
}
