#[path = "budgets_test_support.rs"]
mod support;
use super::{Budgets, Edit, Tab, UsagePage};
use crate::pages::Load;
use gpui_kit::{AnyWindowHandle, AppContext, Entity, TestAppContext};
use serde_json::{Value, json};
use std::sync::Arc;
use support::{answer, backend::BackendFixture};
fn fixture(cx: &mut TestAppContext) -> (Entity<UsagePage>, Arc<BackendFixture>, AnyWindowHandle) {
    support::init(cx);
    let gateway = BackendFixture::new();
    let mut page = None;
    let window = cx.add_window(|window, cx| {
        let entity = cx.new(|cx| UsagePage::new(support::ctx(window, gateway.clone()), window, cx));
        page = Some(entity.clone());
        support::Host(entity.into())
    });
    cx.run_until_parked();
    (page.unwrap(), gateway, window.into())
}
fn budgets(grant: bool, manage: bool) -> Value {
    json!({"scope":"org","canGrant":grant,"canManage":manage,"people":[
        {"userId":"owner","role":"owner","limitUsdDecimal":"100.00"},
        {"userId":"admin","role":"admin","limitManager":false},
        {"userId":"member","role":"member"}]})
}
fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }
    cx.run_until_parked();
}
#[gpui_kit::test]
fn budgets_pending_target_reveals_limits_while_reads_are_delayed_and_normal_usage_keeps_overview(
    cx: &mut TestAppContext,
) {
    let (page, _, window) = fixture(cx);
    assert_eq!(page.read_with(cx, |p, _| p.tab), Tab::Overview);
    page.update(cx, |p, cx| {
        p.ctx.settings_fields.request("usage-limits");
        cx.notify();
    });
    draw(cx, window);
    assert_eq!(page.read_with(cx, |p, _| p.tab), Tab::Limits);
    assert_eq!(page.read_with(cx, |p, _| p.ctx.settings_fields.pending()), None);
    page.update(cx, |p, cx| p.set_tab(Tab::Overview, cx));
    draw(cx, window);
    assert_eq!(page.read_with(cx, |p, _| p.tab), Tab::Overview);
}
#[gpui_kit::test]
fn budgets_response_authority_controls_default_people_and_managers_without_owner_label_inference(
    cx: &mut TestAppContext,
) {
    let (page, gateway, window) = fixture(cx);
    page.update(cx, |p, _| {
        p.budgets = Load::Ready(Budgets::parse(&budgets(false, true)).unwrap());
        assert!(!p.edit_allowed(&Edit::Default));
        assert!(!p.edit_allowed(&Edit::Person("owner".into())));
        assert!(p.edit_allowed(&Edit::Person("member".into())));
    });
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.start_edit(Edit::Default, None, window, cx);
            assert!(p.editing.is_none());
            p.set_manager("admin".into(), true, cx);
        })
    })
    .unwrap();
    assert!(
        !gateway
            .pending
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.path.contains("limit-managers"))
    );
}
#[gpui_kit::test]
fn budgets_decimal_write_and_manager_mutations_serialize_and_old_reads_cannot_replace_the_result(
    cx: &mut TestAppContext,
) {
    let (page, gateway, window) = fixture(cx);
    let stale = gateway.take("/v1/budgets");
    page.update(cx, |p, _| {
        p.budgets = Load::Ready(Budgets::parse(&budgets(true, true)).unwrap())
    });
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.start_edit(Edit::Default, Some(10.), window, cx);
            p.amount.update(cx, |state, cx| state.set_value("12.50", window, cx));
            p.save_amount(cx);
            p.set_manager("admin".into(), true, cx);
            p.clear_limit(cx);
        })
    })
    .unwrap();
    let write = gateway.take("/v1/budgets/person-default");
    assert!(matches!(write.verb, crate::gateway::Verb::Put));
    assert_eq!(write.body, Some(json!({"monthlyUsdDecimal":"12.50"})));
    assert!(
        !gateway
            .pending
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.path.contains("limit-managers"))
    );
    answer(stale, json!({"people":[],"scope":"own"}), cx);
    assert!(page.read_with(cx, |p, _| matches!(&p.budgets, Load::Ready(b) if b.people.len() == 3)));
    answer(write, json!({}), cx);
    assert!(!page.read_with(cx, |p, _| p.mutating()));
    page.update(cx, |p, cx| p.set_manager("admin".into(), true, cx));
    let manager = gateway.take("/v1/budgets/limit-managers/admin");
    assert!(matches!(manager.verb, crate::gateway::Verb::Put));
    answer(manager, json!({}), cx);
}
#[gpui_kit::test]
fn budgets_account_change_drops_reads_and_write_callbacks(cx: &mut TestAppContext) {
    let (page, gateway, window) = fixture(cx);
    page.update(cx, |p, _| {
        p.budgets = Load::Ready(Budgets::parse(&budgets(true, true)).unwrap())
    });
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| {
            p.start_edit(Edit::Default, Some(10.), window, cx);
            p.save_amount(cx);
        })
    })
    .unwrap();
    let write = gateway.take("/v1/budgets/person-default");
    *gateway.marker.lock().unwrap() = Some("account-b".into());
    answer(write, json!({}), cx);
    assert!(page.read_with(cx, |p, _| p.editing == Some(Edit::Default) && p.saving));
    let stale = gateway.take("/v1/tenants/me"); // Billing's own authority read also rejects the old marker.
    answer(
        stale,
        json!({"tenant":{"kind":"company"},"permissions":["org.billing"]}),
        cx,
    );
    assert!(!gateway.pending.lock().unwrap().iter().any(|r| r.path == "/v1/billing"));
    page.update(cx, |p, cx| {
        p.clear_limit(cx);
        p.set_manager("admin".into(), true, cx);
    });
    assert!(
        !gateway
            .pending
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.path.contains("person-default") || r.path.contains("limit-managers"))
    );
}

/// The redesign draws each tab with data: the budget panel, the chart with the month's estimate, the teams
/// with a cap, Calls with a call's panel open, and Limits' rows with one being edited.
#[gpui_kit::test]
fn every_tab_draws_with_data(cx: &mut TestAppContext) {
    use super::{By, CallPage, Range, Report, Spend};
    let (page, _, window) = fixture(cx);
    let report = |by: &str| {
        json!({"scope":"org","totals":{"spentUsd":30,"calls":40,
            "overspendPrevented":{"calls":2,"amountUsd":1.5,"byLevel":{"person":{"calls":2,"amountUsd":1.5}}}},
          "groups":[{"key":"t1","label":if by == "team" {"Platform"} else {"anthropic/claude-opus-5.5"},"share":0.8,
                     "slug":"platform","spentUsd":24,"calls":30},
                    {"key":"t2","label":"Data","share":0.2,"slug":"data","spentUsd":6,"calls":10}],
          "series":[{"day":"2026-11-01","spentUsd":10,"calls":20,"groups":{"t1":8,"t2":2}},
                    {"day":"2026-11-02","spentUsd":20,"calls":20,"groups":{"t1":16,"t2":4}}]})
    };
    let cap = json!({"budget":{"period":"month","resetsAt":"2099-12-01T00:00:00Z","limitUsd":100,"spentUsd":24}});
    page.update(cx, |p, cx| {
        p.me = Load::Ready(super::Me {
            permissions: ["spend.read_all", "spend.export", "budgets.manage", "org.billing"]
                .map(String::from)
                .to_vec(),
            ..Default::default()
        });
        p.range = Range::Month;
        p.chart_by = By::Team;
        p.now = Load::Ready(Report::parse(&report("team")).unwrap());
        p.before = Load::Ready(Report::parse(&report("team")).unwrap());
        p.now_other = Load::Ready(Report::parse(&report("model")).unwrap());
        p.before_other = Load::Ready(Report::parse(&report("model")).unwrap());
        p.spend = Load::Ready(
            Spend::parse(
                &json!({"budgetUsd":100,"periodSpentUsd":30,"reservedUsd":2,"remainingUsd":68,
                "budgetPeriod":"month","resetsAt":"2099-12-01T00:00:00Z"}),
            )
            .unwrap(),
        );
        p.budgets = Load::Ready(Budgets::parse(&budgets(true, true)).unwrap());
        p.caps = Load::Ready(vec![crate::pages::usage_caps::Read {
            slug: "platform".into(),
            name: "Platform".into(),
            answer: Some(cap),
        }]);
        cx.notify();
    });
    for tab in [Tab::Overview, Tab::Breakdown, Tab::Adoption, Tab::Limits] {
        page.update(cx, |p, cx| p.set_tab(tab, cx));
        draw(cx, window);
    }
    cx.update_window(window, |_, window, cx| {
        page.update(cx, |p, cx| p.start_edit(Edit::Default, Some(5.), window, cx))
    })
    .unwrap();
    draw(cx, window);
    let calls = json!({"calls":[{"id":"c1","at":"2026-11-02T10:01:02Z","status":"refused","model":"anthropic/claude-opus-5.5",
        "product":"agents","userId":"owner","costUsd":"0.000000","overspendLevel":"person"}]});
    page.update(cx, |p, cx| {
        p.set_tab(Tab::Calls, cx);
        p.calls = Load::Ready(CallPage::parse(&calls).unwrap());
        p.open_call("c1".into(), cx);
    });
    draw(cx, window);
    page.read_with(cx, |p, _| {
        assert_eq!(p.tab, Tab::Calls);
        assert!(matches!(p.detail, Some(Load::Ready(_))));
    });
}

/// A usable budgets reply can arrive before spend. A failed write must replace the spend read it invalidated.
fn failed_mutation_restarts_pending_spend(manager: bool, cx: &mut TestAppContext) {
    let (page, gateway, window) = fixture(cx);
    // Initial account and tab loading each request money; the tab supersedes the first pair.
    answer(gateway.take("/v1/spend"), json!({"periodSpentUsdDecimal":"99.00"}), cx);
    answer(gateway.take("/v1/budgets"), budgets(true, true), cx);
    let old_spend = gateway.take("/v1/spend");
    answer(gateway.take("/v1/budgets"), budgets(true, true), cx);
    page.read_with(cx, |p, _| {
        assert!(matches!(p.spend, Load::Loading));
        assert!(matches!(&p.budgets, Load::Ready(b) if b.can_grant && b.people.len() == 3));
    });
    let path = if manager {
        page.update(cx, |p, cx| p.set_manager("admin".into(), true, cx));
        "/v1/budgets/limit-managers/admin"
    } else {
        cx.update_window(window, |_, window, cx| {
            page.update(cx, |p, cx| {
                p.start_edit(Edit::Default, Some(10.), window, cx);
                p.save_amount(cx);
            })
        })
        .unwrap();
        "/v1/budgets/person-default"
    };
    let write = gateway.take(path);
    let consumed = write.consumed.clone();
    write.answer.send(Err(nexus_client::Transport::Timeout.into())).unwrap();
    cx.run_until_parked();
    assert!(consumed.load(std::sync::atomic::Ordering::SeqCst));
    page.read_with(cx, |p, _| {
        assert!(!p.mutating());
        assert!(if manager {
            p.manager_error.is_some()
        } else {
            p.edit_error.is_some()
        });
    });
    // A new request must exist after failure. The pre-write reply stays stale even after this read succeeds.
    let fresh_spend = gateway.take("/v1/spend");
    let fresh_budgets = gateway.take("/v1/budgets");
    answer(fresh_spend, json!({"periodSpentUsdDecimal":"7.00"}), cx);
    answer(fresh_budgets, budgets(true, true), cx);
    answer(old_spend, json!({"periodSpentUsdDecimal":"99.00"}), cx);
    page.read_with(cx, |p, _| {
        assert!(matches!(&p.spend, Load::Ready(spend) if spend.spent == 7.));
        assert!(matches!(&p.budgets, Load::Ready(budgets) if budgets.people.len() == 3));
    });
}
#[gpui_kit::test]
fn budgets_failed_limit_write_restarts_delayed_initial_spend(cx: &mut TestAppContext) {
    failed_mutation_restarts_pending_spend(false, cx);
}
#[gpui_kit::test]
fn budgets_failed_manager_write_restarts_delayed_initial_spend(cx: &mut TestAppContext) {
    failed_mutation_restarts_pending_spend(true, cx);
}

/// The sidebar's budget card opens Limits as a place, not as one control: no 2-second outline around the whole tab.
/// And at a narrow column, the two top cards and the People search stay inside it.
#[gpui_kit::test]
fn budgets_target_opens_limits_without_an_outline_and_its_cards_fit_a_narrow_column(cx: &mut TestAppContext) {
    let (page, _, window) = fixture(cx);
    page.update(cx, |p, cx| {
        p.me = Load::Ready(super::Me::default());
        p.spend = Load::Ready(
            super::Spend::parse(&json!({"budgetUsd":30,"periodSpentUsd":29.83,"remainingUsd":0.17})).unwrap(),
        );
        let mut org = budgets(true, true);
        org["personDefault"] = json!({"monthlyUsd": 1.5});
        org["period"] = json!("2026-11");
        org["resetsAt"] = json!("2026-12-01T00:00:00Z");
        p.budgets = Load::Ready(Budgets::parse(&org).unwrap());
        p.ctx.settings_fields.request("usage-limits");
        cx.notify();
    });
    let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
    for width in [720., 560.] {
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(1400.)));
        for _ in 0..3 {
            visual.run_until_parked();
            visual.update(|window, cx| window.draw(cx).clear(cx));
        }
        let edge = gpui_kit::px(width - crate::pages::ui::PAGE_GUTTER);
        for selector in [
            "usage-limits-org-card",
            "usage-limits-default-card",
            "usage-limits-search",
            "usage-limits-table",
            "usage-limits-resets",
        ] {
            let bounds = visual.debug_bounds(selector).expect(selector);
            assert!(
                bounds.right() <= edge,
                "at {width}px, {selector} ends at {:?}, past the column's edge {edge:?}",
                bounds.right()
            );
        }
    }
    let name = visual.debug_bounds("usage-limits-name-0").unwrap();
    assert!(
        name.size.width >= gpui_kit::px(140.),
        "a name collapses to {:?}",
        name.size.width
    );
    assert_eq!(page.read_with(&visual, |p, _| p.tab), Tab::Limits);
    assert_eq!(page.read_with(&visual, |p, _| p.ctx.settings_fields.pending()), None);
    let card = visual.debug_bounds("usage-limits-org-card").unwrap();
    let meter = visual.debug_bounds("usage-limits-org-meter").unwrap();
    assert!(
        meter.right() <= card.right(),
        "the org meter ends at {:?}, past its card at {:?}",
        meter.right(),
        card.right()
    );
    assert_eq!(
        page.read_with(&visual, |p, _| p.ctx.settings_fields.highlighted_target()),
        None,
        "the whole Limits tab is not one control to outline"
    );
}
