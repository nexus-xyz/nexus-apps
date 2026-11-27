//! The `presentationGroups` fixtures, including original destinations and marking identities.
use super::{Entry, Filter, Item, Moment, Target, entries, entry, presentation_groups};
use core::prelude::v1::test;
use serde_json::json;

fn security(id: &str, action: &str, at: &str, count: u64, read: bool) -> Entry {
    entry(
        &Item::from_json(
            &json!({"id":id,"kind":"security.changed","group":"account_and_security",
        "subjectText":"An account change","subject":{"type":"user","id":"ada"},
        "sources":[{"type":"security_change","action":action}],"count":count,"updatedAt":at,
        "readAt":if read { json!(at) } else { json!(null) }}),
        )
        .unwrap(),
    )
}

#[test]
fn same_day_different_actions_group_across_intervening_activity_without_losing_rows() {
    let first = security("key", "key.created", "2026-11-12T12:00:00Z", 2, false);
    let member = security("member", "member.role_changed", "2026-11-12T10:00:00Z", 3, true);
    let mut activity = security("billing", "key.created", "2026-11-12T11:00:00Z", 1, false);
    activity.moment = Moment::Billing;
    let ordered = entries([member.clone(), activity.clone(), first.clone()], Filter::All);
    let groups = presentation_groups(ordered);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].key, "account:2026-11-12");
    assert_eq!(groups[0].title, "5 account changes");
    assert_eq!(groups[0].items, [first.clone(), member.clone()]);
    assert_eq!(groups[1].items, [activity]);
    assert_eq!(groups[0].items[0].target, Some(Target::Org("/keys".into())));
    assert_eq!(groups[0].items[1].target, Some(Target::Org("/members".into())));
    assert_eq!(groups[0].items[1].action, "Review change");
    assert!(
        groups[0].items.iter().all(|item| item.remote),
        "each child retains Gateway Mark read and Done"
    );
    assert_eq!(
        groups[0].items.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
        ["key", "member"]
    );
}

#[test]
fn identical_titles_on_different_days_never_share_a_group() {
    let today = security("today", "key.created", "2026-11-12T00:01:00Z", 1, false);
    let yesterday = security("yesterday", "key.created", "2026-11-11T23:59:00Z", 1, false);
    let groups = presentation_groups(vec![today.clone(), yesterday.clone()]);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].key, "account:2026-11-12");
    assert_eq!(groups[1].key, "account:2026-11-11");
    assert_eq!(groups[0].title, today.title, "one event keeps its original title");
    assert_eq!(groups[0].items, [today]);
    assert_eq!(groups[1].items, [yesterday]);
}

#[test]
fn required_work_never_folds_into_routine_security_activity() {
    let first = security("first", "key.created", "2026-11-12T12:00:00Z", 1, false);
    let mut required = security("required", "member.removed", "2026-11-12T11:00:00Z", 1, false);
    required.needs_you = true;
    let last = security("last", "key.revoked", "2026-11-12T10:00:00Z", 1, false);
    let groups = presentation_groups(entries([first.clone(), required.clone(), last.clone()], Filter::All));
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].key, "required");
    assert_eq!(groups[0].items, [required.clone()]);
    assert_eq!(groups[1].items, [first, last]);
    assert_eq!(groups[1].title, "2 account changes");
    let needs = presentation_groups(entries([required], Filter::Needs));
    assert_eq!(needs[0].key, "required");
}

#[test]
fn unread_filter_runs_before_grouping_and_day_identity_survives_refresh() {
    let newest = security("newest", "key.created", "2026-11-12T12:00:00Z", 4, true);
    let second = security("second", "member.role_changed", "2026-11-12T11:00:00Z", 2, false);
    let third = security("third", "key.revoked", "2026-11-12T10:00:00Z", 1, false);
    let all = presentation_groups(entries([newest.clone(), second.clone(), third.clone()], Filter::All));
    let unread = presentation_groups(entries([newest, second.clone(), third.clone()], Filter::Unread));
    assert_eq!(all[0].title, "7 account changes");
    assert_eq!(
        unread[0].key, all[0].key,
        "read/refetch must not change an expanded day's identity"
    );
    assert_eq!(unread[0].title, "3 account changes");
    assert_eq!(unread[0].items, [second.clone(), third.clone()]);
    let refreshed = presentation_groups(vec![third]);
    assert_eq!(
        refreshed[0].key, all[0].key,
        "a one-event refresh preserves the day key"
    );
}

#[test]
fn empty_dates_do_not_group_and_an_empty_inbox_has_no_groups() {
    let first = security("first", "key.created", "", 1, false);
    let second = security("second", "member.role_changed", "", 1, false);
    let groups = presentation_groups(vec![first, second]);
    assert_eq!(
        groups.iter().map(|group| group.key.as_str()).collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(presentation_groups(Vec::new()).is_empty());
}

#[test]
fn absorbing_daily_security_preserves_original_nonsecurity_run_boundaries() {
    let first = security("first", "key.created", "2026-11-12T12:00:00Z", 1, false);
    let second = security("second", "member.role_changed", "2026-11-12T10:00:00Z", 1, false);
    let mut billing_one = security("billing-one", "key.created", "2026-11-12T11:00:00Z", 1, false);
    billing_one.moment = Moment::Billing;
    billing_one.title = "A spending limit stopped a call".into();
    let mut billing_two = billing_one.clone();
    billing_two.id = "billing-two".into();
    billing_two.at = "2026-11-12T09:00:00Z".into();
    let groups = presentation_groups(vec![
        first.clone(),
        billing_one.clone(),
        second.clone(),
        billing_two.clone(),
    ]);
    assert_eq!(
        groups.len(),
        3,
        "removing a security separator must not create a new billing run"
    );
    assert_eq!(groups[0].items, [first.clone(), second.clone()]);
    assert_eq!(groups[1].items, [billing_one.clone()]);
    assert_eq!(groups[2].items, [billing_two.clone()]);
    let adjacent = presentation_groups(vec![first, billing_one.clone(), billing_two.clone(), second]);
    assert_eq!(
        adjacent.len(),
        2,
        "originally adjacent billing items retain the native fold"
    );
    assert_eq!(adjacent[1].items, [billing_one, billing_two]);
}
