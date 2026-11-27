//! The morning brief (A1) in the Assistant's thread and proactivity (A10) on Approvals, on the production Workspace:
//! each call, its answer, and the view drawn after it, with the fixture backend of the views' tests.
use super::super::navigation::test_backend::BackendProbe;
use super::views_tests::{answer, calls, fixture, open};
use crate::agent::Call;
use crate::pages::PageId;
use crate::pages::assistant_views::{ApprovalsPage, ProactivityPanel, local_zone};
use gpui_kit::TestAppContext;
use nexus_desktop_core::agent_proactivity::{Change, Kind, Level, QuietHours, level_text};
use serde_json::{Value, json};

fn setting(level: &str, muted: Value, quiet_hours: Value) -> Value {
    json!({ "level": level, "ownLevel": true, "agentProactivity": "ask_first", "muted": muted,
            "quietHours": quiet_hours, "batchMinutes": 15 })
}

fn saving(change: &Change) -> Call {
    Call::save_proactivity(change).expect("a change the agent API takes")
}

/// A10·a: Approvals draws the dial under the tiers. A step saves the level, quiet hours save in this computer's zone
/// and turn off, the kinds turned off come back on, and with proactivity off the dial is not drawn.
#[gpui_kit::test]
fn the_dial_sets_the_level_and_quiet_hours_and_is_gone_while_off(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = open(&workspace, PageId::AgentApprovals, cx)
        .downcast::<ApprovalsPage>()
        .expect("Approvals");
    let panel = page.read_with(cx, |page, _| page.proactivity());
    assert_eq!(calls(&backend), [Call::Approvals, Call::Proactivity]);
    answer(
        &backend,
        |call| *call == Call::Proactivity,
        200,
        setting("balanced", json!(["birthday"]), Value::Null),
        cx,
    );
    panel.read_with(cx, |panel, _| {
        let view = panel.view().expect("read");
        assert_eq!((view.level, view.quiet_hours.clone()), (Level::Balanced, None));
        assert_eq!(view.muted, [Kind::Birthday]);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // A step of the dial.
    panel.update_in(cx, |panel, window, cx| {
        panel.change(Change::level(Level::HandsOn), level_text, window, cx)
    });
    let level = saving(&Change::level(Level::HandsOn));
    assert_eq!(calls(&backend).last(), Some(&level));
    answer(
        &backend,
        |call| *call == level,
        200,
        setting("hands_on", json!(["birthday"]), Value::Null),
        cx,
    );
    panel.read_with(cx, |panel, _| {
        assert_eq!(panel.view().expect("saved").level, Level::HandsOn);
        assert_eq!(panel.status(), Some("Proactive is set to Hands-on."));
    });

    // Quiet hours on, from the fields' 22:00 to 07:00, in this computer's zone; then off.
    panel.update_in(cx, |panel, window, cx| panel.set_quiet_hours(true, window, cx));
    let hours = QuietHours {
        start: "22:00".into(),
        end: "07:00".into(),
        timezone: local_zone(),
    };
    let on = saving(&Change::quiet_hours(Some(hours.clone())));
    assert_eq!(calls(&backend).last(), Some(&on));
    let saved = json!({ "start": "22:00", "end": "07:00", "timezone": hours.timezone });
    answer(
        &backend,
        |call| *call == on,
        200,
        setting("hands_on", json!([]), saved),
        cx,
    );
    panel.read_with(cx, |panel, _| {
        assert_eq!(panel.view().expect("saved").quiet_hours, Some(hours.clone()));
        assert_eq!(panel.status(), Some("Quiet from 22:00 to 07:00."));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    panel.update_in(cx, |panel, window, cx| panel.set_quiet_hours(false, window, cx));
    let off = saving(&Change::quiet_hours(None));
    assert_eq!(calls(&backend).last(), Some(&off));
    // A change the agent API refuses says so and keeps the setting.
    answer(&backend, |call| *call == off, 500, json!({ "error": "internal" }), cx);
    panel.read_with(cx, |panel, _| {
        assert_eq!(panel.view().expect("kept").quiet_hours, Some(hours.clone()));
        assert_eq!(panel.status(), Some("That was not saved. Try again in a moment."));
    });

    // Off at the agent API: no dial.
    let again = open(&workspace, PageId::AgentApprovals, cx)
        .downcast::<ApprovalsPage>()
        .expect("Approvals again");
    let panel = again.read_with(cx, |page, _| page.proactivity());
    answer(
        &backend,
        |call| *call == Call::Proactivity,
        503,
        json!({ "error": "proactivity_off" }),
        cx,
    );
    assert!(panel.read_with(cx, |panel: &ProactivityPanel, _| panel.off()));
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// A1·A and A10·b: the thread opens with today's brief, its lines and the errands line; "Why am I seeing this?" reads
/// the setting, and Fewer like this at Quiet turns the brief off. With the brief off, the thread shows none.
#[gpui_kit::test]
fn the_brief_opens_the_thread_and_why_turns_it_down(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    let page = workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_agent(None, window, cx);
        workspace.assistant.kept.as_ref().expect("kept").page.clone()
    });
    cx.run_until_parked();
    assert!(
        calls(&backend).contains(&Call::Brief),
        "the page asks for today's brief"
    );
    let brief = json!({ "brief": { "day": "2026-11-18", "at": "2026-11-18T07:58:00Z",
        "head": "Morning. Two things today:", "lines": ["Rig review at 10:30.", "The SOW is due Friday."],
        "errands": "Vendor contracts needs you.",
        "why": { "kind": "brief", "level": "quiet", "lines": ["You get a brief each morning."] } } });
    answer(&backend, |call| *call == Call::Brief, 200, brief, cx);
    page.read_with(cx, |page, _| {
        let brief = page.brief().expect("today's brief");
        assert_eq!(brief.lines.len(), 2);
        assert_eq!(brief.errands.as_deref(), Some("Vendor contracts needs you."));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let why = page.read_with(cx, |page, _| page.brief_why()).expect("a Why");
    why.update_in(cx, |sheet, window, cx| sheet.toggle(window, cx));
    assert_eq!(calls(&backend).last(), Some(&Call::Proactivity));
    answer(
        &backend,
        |call| *call == Call::Proactivity,
        200,
        setting("quiet", json!([]), Value::Null),
        cx,
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    why.update_in(cx, |sheet, window, cx| sheet.act(true, window, cx));
    let fewer = saving(&Change::muted(vec![Kind::Brief]));
    assert_eq!(calls(&backend).last(), Some(&fewer));
    answer(
        &backend,
        |call| *call == fewer,
        200,
        setting("quiet", json!(["brief"]), Value::Null),
        cx,
    );
    assert_eq!(
        why.read_with(cx, |sheet, _| sheet.done().map(String::from)),
        Some("Done: no more the morning brief.".into())
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Off at the agent API: the next read shows no brief.
    page.update_in(cx, |page, window, cx| page.read_brief_now(window, cx));
    answer(
        &backend,
        |call| *call == Call::Brief,
        503,
        json!({ "error": "brief_off" }),
        cx,
    );
    assert!(page.read_with(cx, |page, _| page.brief().is_none()));
}

/// The dial's lede says the person's name for the Assistant, as the web's, and Nexus without one.
#[gpui_kit::test]
fn the_dial_says_the_assistants_own_name(cx: &mut TestAppContext) {
    let _agents = crate::code_run::fixture_agents(vec![]);
    let backend = BackendProbe::new();
    let (workspace, cx) = fixture(cx, &backend);
    // The person's own agent profile named it Ada: the views opened from now on say so.
    workspace.update(cx, |workspace, _| workspace.assistant.name = Some("Ada".into()));
    let page = open(&workspace, PageId::AgentApprovals, cx)
        .downcast::<ApprovalsPage>()
        .expect("Approvals");
    let panel = page.read_with(cx, |page, _| page.proactivity());
    answer(
        &backend,
        |call| *call == Call::Proactivity,
        200,
        setting("balanced", json!([]), Value::Null),
        cx,
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        panel.read_with(cx, |panel, _| panel.lede()),
        "How often Ada writes first. It writes first only with a reason it can show."
    );
    // Without a name, Nexus.
    assert_eq!(
        crate::pages::assistant_views::proactivity_lede(&crate::pages::assistant_views::Moves::default().name()),
        "How often Nexus writes first. It writes first only with a reason it can show."
    );
}
