use core::prelude::v1::test;

use super::*;

fn drag(from: Place) -> Facts {
    Facts {
        id: "a".into(),
        project: "repo".into(),
        from,
        pinned: false,
        linked: false,
        with: Vec::new(),
    }
}

fn carrying(lead: Facts, others: &[(&str, Place)]) -> Facts {
    let with = others
        .iter()
        .map(|(id, from)| Facts {
            id: id.to_string(),
            ..drag(from.clone())
        })
        .collect();
    Facts { with, ..lead }
}

#[test]
fn several_sessions_land_on_a_row_only_if_each_may() {
    let lead = drag(Place::Project("repo".into()));
    let two = carrying(lead.clone(), &[("x", Place::Aside(Aside::Settled))]);
    assert!(plan(&two, &row("b", false)).is_some(), "both may land there");
    let onto_one = carrying(lead.clone(), &[("b", Place::Project("repo".into()))]);
    assert_eq!(plan(&onto_one, &row("b", false)), None, "on one of them");
    let elsewhere = Facts {
        with: vec![Facts {
            id: "y".into(),
            project: "other".into(),
            ..drag(Place::Project("other".into()))
        }],
        ..lead
    };
    assert_eq!(plan(&elsewhere, &row("b", false)), None, "one of another project");
}

#[test]
fn several_sessions_go_into_a_group_if_any_is_not_there_yet() {
    let settled = Target::Group(Place::Aside(Aside::Settled));
    let lead = drag(Place::Aside(Aside::Settled));
    let alone = lead.clone();
    assert_eq!(plan(&alone, &settled), None);
    let with_one = carrying(lead, &[("x", Place::Project("repo".into()))]);
    assert_eq!(put(plan(&with_one, &settled)), Some(Put::Settle));
    let x = &with_one.with[0];
    assert_eq!(put(plan_one(x, &settled)), Some(Put::Settle));
    assert_eq!(plan_one(&with_one, &settled), None, "the dragged one stays");
}

#[test]
fn a_snooze_preset_names_its_time() {
    let listed = drag(Place::Project("repo".into()));
    assert_eq!(put(plan(&listed, &Target::SnoozeUntil(7))), Some(Put::Snooze(Some(7))));
    let snoozed = drag(Place::Aside(Aside::Snoozed));
    assert_eq!(plan(&snoozed, &Target::SnoozeUntil(7)), None);
    let preset = plan(&listed, &Target::SnoozeUntil(7)).unwrap();
    let args = changes("a", &listed.from, &preset, Some(99)).unwrap();
    assert_eq!(args.snoozed_until, Some(7), "the preset's time, not the default");
}

fn spot(id: &str, members: &[&str], pinned: bool) -> Spot {
    Spot {
        project: "repo".into(),
        members: members.iter().map(|member| member.to_string()).collect(),
        listed: ["a", "b", "c"].map(String::from).into_iter().collect(),
        id: id.into(),
        after: true,
        pinned,
    }
}

fn row(id: &str, pinned: bool) -> Target {
    Target::Row(spot(id, &["repo"], pinned))
}

fn put(plan: Option<Plan>) -> Option<Put> {
    plan.map(|plan| plan.put)
}

#[test]
fn a_session_moves_among_its_own_projects_sessions() {
    let listed = drag(Place::Project("repo".into()));
    let plan = plan(&listed, &row("b", false)).expect("it moves");
    assert_eq!(plan.put, Put::Stay);
    assert_eq!(plan.pin, None);
    assert_eq!(plan.spot.map(|spot| spot.id), Some("b".into()));
    assert_eq!(super::plan(&listed, &row("a", false)), None, "on itself");
    assert_eq!(
        super::plan(&listed, &Target::Row(spot("b", &["other"], false))),
        None,
        "another project"
    );
    let copy = Facts {
        project: "copy".into(),
        ..listed.clone()
    };
    assert!(
        super::plan(&copy, &Target::Row(spot("b", &["repo", "copy"], false))).is_some(),
        "another copy of the same repository"
    );
    let linked = Facts { linked: true, ..listed };
    assert_eq!(super::plan(&linked, &row("b", false)), None, "a linked session");
}

#[test]
fn landing_across_the_pinned_edge_pins_or_unpins_it() {
    let listed = drag(Place::Project("repo".into()));
    assert_eq!(
        plan(&listed, &row("b", true)).unwrap().pin,
        Some(true),
        "among the pinned"
    );
    let pinned = Facts {
        pinned: true,
        ..listed.clone()
    };
    assert_eq!(
        plan(&pinned, &row("b", false)).unwrap().pin,
        Some(false),
        "among the others"
    );
    assert_eq!(plan(&pinned, &row("b", true)).unwrap().pin, None, "among its own");
}

#[test]
fn a_session_put_aside_comes_back_to_the_spot_it_lands_on() {
    for from in [
        Place::Aside(Aside::Settled),
        Place::Aside(Aside::Snoozed),
        Place::Archived,
    ] {
        let aside = drag(from.clone());
        let plan = plan(&aside, &row("b", true)).expect("it comes back");
        assert_eq!(plan.put, Put::Return, "from {from:?}");
        assert_eq!(plan.pin, Some(true));
        assert!(plan.spot.is_some());
    }
}

#[test]
fn a_session_goes_into_a_group_it_is_not_in_already() {
    let settled = Target::Group(Place::Aside(Aside::Settled));
    let snoozed = Target::Group(Place::Aside(Aside::Snoozed));
    let archived = Target::Group(Place::Archived);
    let listed = drag(Place::Project("repo".into()));
    assert_eq!(put(plan(&listed, &settled)), Some(Put::Settle));
    assert_eq!(put(plan(&listed, &snoozed)), Some(Put::Snooze(None)));
    assert_eq!(put(plan(&listed, &archived)), Some(Put::Archive));
    let from_settled = drag(Place::Aside(Aside::Settled));
    assert_eq!(plan(&from_settled, &settled), None);
    assert_eq!(put(plan(&from_settled, &snoozed)), Some(Put::Snooze(None)));
    let from_archive = drag(Place::Archived);
    assert_eq!(plan(&from_archive, &archived), None);
    assert_eq!(
        put(plan(&from_archive, &settled)),
        Some(Put::Settle),
        "straight from the archive"
    );
    assert_eq!(put(plan(&from_archive, &snoozed)), Some(Put::Snooze(None)));
}

#[test]
fn a_session_put_aside_returns_on_top_only_to_its_own_project() {
    let head = |members: &[&str]| Target::Project {
        lead: "repo".into(),
        members: members.iter().map(|member| member.to_string()).collect(),
    };
    for from in [
        Place::Aside(Aside::Settled),
        Place::Aside(Aside::Snoozed),
        Place::Archived,
    ] {
        let aside = drag(from);
        assert_eq!(put(plan(&aside, &head(&["repo"]))), Some(Put::Return));
        assert_eq!(plan(&aside, &head(&["other"])), None, "another project");
    }
    let listed = drag(Place::Project("repo".into()));
    assert_eq!(plan(&listed, &head(&["repo"])), None, "already there");
}

#[test]
fn a_drop_changes_only_what_it_must() {
    let from_project = Place::Project("repo".into());
    let stay = Plan {
        put: Put::Stay,
        pin: None,
        spot: None,
    };
    assert!(
        changes("a", &from_project, &stay, None).is_none(),
        "a reorder changes no field"
    );
    let pin = Plan {
        pin: Some(true),
        ..stay.clone()
    };
    let args = changes("a", &from_project, &pin, None).unwrap();
    assert_eq!((args.thread_id.as_str(), args.pinned), ("a", Some(true)));
    assert_eq!((args.settled, args.snoozed_until, args.archived), (None, None, None));

    let settle = Plan {
        put: Put::Settle,
        ..stay.clone()
    };
    let args = changes("a", &Place::Archived, &settle, None).unwrap();
    assert_eq!(
        (args.settled, args.archived),
        (Some(true), Some(false)),
        "out of the archive"
    );
    let args = changes("a", &Place::Aside(Aside::Snoozed), &settle, None).unwrap();
    assert_eq!((args.settled, args.snoozed_until), (Some(true), Some(0)), "woken");

    let snooze = Plan {
        put: Put::Snooze(None),
        ..stay.clone()
    };
    assert!(
        changes("a", &from_project, &snooze, None).is_none(),
        "no time to snooze to"
    );
    let args = changes("a", &Place::Aside(Aside::Settled), &snooze, Some(9)).unwrap();
    assert_eq!((args.snoozed_until, args.settled), (Some(9), Some(false)));

    let back = Plan {
        put: Put::Return,
        ..stay.clone()
    };
    let args = changes("a", &Place::Aside(Aside::Snoozed), &back, None).unwrap();
    assert_eq!((args.snoozed_until, args.settled, args.archived), (Some(0), None, None));

    let archive = Plan {
        put: Put::Archive,
        ..stay
    };
    assert!(
        changes("a", &from_project, &archive, None).is_none(),
        "archive.rs archives"
    );
}
