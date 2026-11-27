use super::*;
// The gpui_kit glob brings its own `test` macro; this is the standard one.
use core::prelude::v1::test;
use serde_json::json;

fn report() -> Value {
    serde_json::from_str::<Value>(r#"{
        "from": "2026-11-01", "to": "2026-11-03", "groupBy": "team", "scope": "org",
        "totals": {
            "spentUsd": 12.5, "spentUsdDecimal": "12.500000", "calls": 40, "inputTokens": 4100000, "outputTokens": 680000,
            "callsByStatus": { "settled": 38, "failed": 1, "refused": 2, "released": 1 }, "errorRate": 0.073,
            "overspendPrevented": { "calls": 2, "amountUsd": 1.5, "amountUsdDecimal": "1.500000",
                "byLevel": { "organization": { "calls": 0, "amountUsd": 0 }, "team": { "calls": 0, "amountUsd": 0 },
                             "person": { "calls": 2, "amountUsd": 1.5, "amountUsdDecimal": "1.500000" }, "product": { "calls": 0, "amountUsd": 0 } } }
        },
        "groups": [
            { "key": "t1", "label": "Engineering", "share": 0.8, "slug": "eng", "hasChildren": true, "level": "Department",
              "spentUsd": 10, "spentUsdDecimal": "10.000000", "calls": 30, "inputTokens": 1, "outputTokens": 2,
              "callsByStatus": { "settled": 30, "failed": 0, "refused": 0, "released": 0 }, "errorRate": 0 },
            { "key": "unassigned", "label": "No team", "share": 0.2, "spentUsd": 2.5, "calls": 10, "inputTokens": 0, "outputTokens": 0,
              "callsByStatus": { "settled": 10, "failed": 0, "refused": 0, "released": 0 }, "errorRate": null }
        ],
        "series": [
            { "day": "2026-11-01", "spentUsd": 5, "calls": 20, "inputTokens": 0, "outputTokens": 0,
              "callsByStatus": { "settled": 20, "failed": 0, "refused": 0, "released": 0 }, "errorRate": null,
              "groups": { "t1": 4, "unassigned": 1 }, "groupsDecimal": { "t1": "4.000000", "unassigned": "1.000000" } },
            { "day": "2026-11-02", "spentUsd": 0, "spentUsdDecimal": null, "calls": 0, "inputTokens": 0, "outputTokens": 0,
              "callsByStatus": { "settled": 0, "failed": 0, "refused": 0, "released": 0 }, "errorRate": null, "groups": {} }
        ]
    }"#).unwrap()
}

#[test]
fn reads_a_usage_report() {
    let report = Report::parse(&report()).unwrap();
    assert_eq!(report.scope, "org");
    assert_eq!(report.totals.spent, 12.5);
    assert_eq!(report.totals.failed, 1);
    assert_eq!(report.groups.len(), 2);
    assert!(report.groups[0].has_children);
    assert_eq!(report.groups[0].slug.as_deref(), Some("eng"));
    assert_eq!(report.series[0].groups["t1"], 4.);
    assert_eq!(report.active_days(), 1);
    let prevented = report.prevented.unwrap();
    assert_eq!(prevented.levels, vec![("Person", 2, 1.5)]);
    assert_eq!(compact(report.totals.input_tokens), "4.1M");
}

#[test]
fn stacks_the_top_groups() {
    let stack = stack_of(&Report::parse(&report()).unwrap());
    assert_eq!(stack.series.len(), 2);
    assert_eq!(stack.series[0].hue, 1);
    assert_eq!(stack.columns[0].values, vec![4., 1.]);
    assert_eq!(stack.max, 5.);
    assert_eq!(axis(1.2), (1.5, [0., 0.5, 1., 1.5]));
}

#[test]
fn reads_spend_budgets_and_calls() {
    let spend = Spend::parse(
        &serde_json::from_str::<Value>(
            r#"{
        "budgetUsd": 100, "budgetUsdDecimal": "100.000000", "spentUsd": 400, "periodSpentUsd": 15.4,
        "periodSpentUsdDecimal": "15.400000", "reservedUsd": 0.5, "remainingUsd": 84.1, "budgetPeriod": "month",
        "resetsAt": "2026-12-01T00:00:00Z", "byProduct": [], "recent": []
    }"#,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(spend.budget, Some(100.));
    assert_eq!(spend.spent, 15.4);
    let member = Spend::parse(&json!({ "remainingUsd": 4, "byProduct": [], "recent": [] })).unwrap();
    assert_eq!(member.budget, None);

    let budgets = Budgets::parse(&serde_json::from_str::<Value>(r#"{
        "period": "2026-11", "periodStart": "2026-11-01T00:00:00Z", "resetsAt": "2026-12-01T00:00:00Z",
        "personDefault": { "monthlyUsd": 5, "monthlyUsdDecimal": "5.000000" }, "scope": "org", "canManage": true, "canGrant": true,
        "people": [
            { "userId": "u1", "email": "a@x.io", "name": "Ana", "role": "owner", "limitUsd": 5, "source": "default",
              "overrideUsd": null, "spentUsd": 4.5, "reservedUsd": 0, "remainingUsd": 0.5, "atLimit": false, "overspendsPrevented": 0 },
            { "userId": "u2", "role": "admin", "limitUsd": null, "source": "none", "overrideUsd": null, "spentUsd": 1,
              "reservedUsd": 0, "remainingUsd": null, "atLimit": false, "overspendsPrevented": 0, "limitManager": true }
        ]
    }"#).unwrap())
    .unwrap();
    assert_eq!(budgets.person_default, Some(5.));
    assert_eq!(budgets.people[0].state(), LimitState::Near);
    assert_eq!(budgets.people[1].state(), LimitState::None);
    assert!(budgets.people[1].limit_manager);
    assert_eq!(month_name(&budgets.period), "November");

    let page = CallPage::parse(&serde_json::from_str::<Value>(r#"{
        "from": "2026-11-01", "to": "2026-11-05", "nextCursor": "abc",
        "calls": [{ "id": "c1", "at": "2026-11-05T10:01:02Z", "status": "refused", "model": "anthropic/claude-opus-5.5",
            "provider": null, "product": "agents", "capability": null, "userId": "u1", "teamId": null,
            "agentSessionId": "s1", "chatSessionId": null, "inputTokens": null, "outputTokens": null, "costUsd": "0.000000",
            "reservedUsd": "0.120000", "reason": "person limit", "overspendLevel": "person", "overspendLimitUsd": "5.000000",
            "overspendAmountUsd": "0.120000", "overspendOverUsd": null, "overspendPeriod": "2026-11", "fundedBy": null }]
    }"#).unwrap())
    .unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("abc"));
    let call = &page.calls[0];
    assert_eq!(call.state(), "Stopped · person limit");
    let agents = HashMap::from([("s1".to_string(), "codex".to_string())]);
    assert_eq!(call.source(&agents), "Codex");
    assert_eq!(usd_decimal("0.120000"), "$0.12");
    assert_eq!(usd_decimal("0.001234"), "$0.001234");
    assert_eq!(usd_decimal("1234.500000"), "$1,234.50");
}

#[test]
fn reads_adoption_as_counts() {
    let adoption = Adoption::parse(&serde_json::from_str::<Value>(r#"{
        "from": "2026-11-04", "to": "2026-11-05", "scope": "org",
        "days": [
            { "day": "2026-11-04", "people": [{ "userId": "u1", "email": null, "calls": 3 }], "subscription": [],
              "engineers": 1, "products": [{ "product": "agents", "engineers": 1 }] },
            { "day": "2026-11-05", "people": [], "subscription": [{ "userId": "u2", "email": null, "sessions": 1 }],
              "engineers": 2, "products": [{ "product": "chat", "engineers": 1 }, { "product": "core", "engineers": 1 }] }
        ]
    }"#).unwrap())
    .unwrap();
    assert!(adoption.supported);
    assert_eq!(adoption.rows[0].day, "2026-11-05");
    assert_eq!(adoption.rows[0].subscription, 1);
    let slugs: Vec<&str> = adoption.products.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(slugs, vec!["core", "agents", "chat"]);
    assert_eq!(adoption.active_days, 2);
}

#[test]
fn money_dates_and_amounts() {
    assert_eq!(cents(1234.5), "$1,234.50");
    assert_eq!(cents(-0.314), "−$0.31");
    assert_eq!(budget_text(100.), "$100");
    assert_eq!(percent(0.004), "0.4%");
    assert_eq!(iso_day(day_number("2026-11-05").unwrap()), "2026-11-05");
    assert_eq!(iso_day(0), "1970-01-01");
    assert_eq!(long_date("2026-11-05"), "Thu, Nov 5");
    let s = span(7, day_number("2026-11-05").unwrap());
    assert_eq!(
        (s.from.as_str(), s.previous_to.as_str(), s.previous_from.as_str()),
        ("2026-10-30", "2026-10-29", "2026-10-23")
    );
    assert_eq!(parse_usd("$1,200.5").as_deref(), Some("1200.50"));
    assert_eq!(parse_usd("8").as_deref(), Some("8.00"));
    assert_eq!(parse_usd("8.123"), None);
    assert_eq!(parse_usd("abc"), None);
    assert_eq!(query(&[("a", "x y"), ("b", "")]), "?a=x%20y");
    assert!(is_refusal("only the org's owners or admins can manage budgets"));
}

#[test]
fn this_month_compares_against_the_same_days_of_last_month() {
    let day = |iso: &str| day_number(iso).unwrap();
    let s = Range::Month.span(day("2026-11-18"));
    assert_eq!(
        (
            s.from.as_str(),
            s.to.as_str(),
            s.previous_from.as_str(),
            s.previous_to.as_str()
        ),
        ("2026-11-01", "2026-11-18", "2026-10-01", "2026-10-18")
    );
    assert_eq!(s.days(), 18);
    assert_eq!(s.versus(), "vs. Oct 1 – Oct 18");
    // March 31 compares against all of February, not past its end.
    let march = Range::Month.span(day("2026-03-31"));
    assert_eq!(
        (march.previous_from.as_str(), march.previous_to.as_str()),
        ("2026-02-01", "2026-02-28")
    );
    // January compares against December of the year before.
    let january = Range::Month.span(day("2027-01-05"));
    assert_eq!(january.previous_from, "2026-12-01");
    assert_eq!(Range::Last(7).span(day("2026-11-05")), span(7, day("2026-11-05")));
    assert_eq!(Range::Month.label(), "This month");
    assert_eq!(Range::Last(30).label(), "30d");
}

#[test]
fn the_rest_of_the_month_ends_on_its_last_day() {
    let rest = rest_of_month(day_number("2026-11-28").unwrap());
    assert_eq!(rest, vec!["2026-11-29", "2026-11-30"]);
    assert!(rest_of_month(day_number("2026-11-30").unwrap()).is_empty());
    assert_eq!(rest_of_month(day_number("2026-02-01").unwrap()).len(), 27);
}

#[test]
fn people_filters_and_initials() {
    let person = |source: &str, limit: Option<f64>, spent: f64, at_limit: bool| Person {
        user_id: "u".into(),
        email: None,
        name: None,
        role: "member".into(),
        limit,
        source: source.into(),
        spent,
        reserved: 0.,
        remaining: None,
        at_limit,
        limit_manager: false,
    };
    let own = person("override", Some(10.), 1., false);
    let near = person("default", Some(10.), 9., false);
    let at = person("default", Some(10.), 10., true);
    assert!(LimitFilter::Own.keeps(&own) && !LimitFilter::Own.keeps(&near));
    assert!(LimitFilter::Near.keeps(&near) && !LimitFilter::Near.keeps(&at));
    assert!(LimitFilter::At.keeps(&at));
    assert!(LimitFilter::All.keeps(&own));
    assert_eq!(parts::initials("Ana Ruiz"), "AR");
    assert_eq!(parts::initials("ana@x.io"), "A");
    assert_eq!(parts::initials("leo"), "L");
    assert_eq!(parts::initials(""), "");
}

#[test]
fn paces_the_month() {
    let spend = Spend {
        budget: Some(100.),
        spent: 50.,
        reserved: 0.,
        remaining: 50.,
        period: Some("month".into()),
        resets_at: Some("2026-12-01T00:00:00Z".into()),
    };
    let now = chrono_lite::parse("2026-11-11T00:00:00Z").unwrap();
    let (projected, last, verdict) = budget_pace(&spend, now).unwrap();
    assert_eq!(projected, 150.);
    assert_eq!(last, "2026-11-30");
    assert_eq!(verdict, "over");
}
