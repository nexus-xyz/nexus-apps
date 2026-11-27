//! A named session shows its name: a tag on its sidebar row and beside its title in the top bar, drawn
//! by the real workspace. An unnamed session shows none. A person picks a session by its name, so on the row the name
//! comes first: it shows whole up to about 60% of the row's text, and the title takes what is left.
use core::prelude::v1::test;

use gpui_kit::{Entity, TestAppContext, VisualTestContext, px};

use crate::code::CodeProject;
use crate::tokens::SIDEBAR;
use crate::workspace::agents_tool::AgentsTool;
use crate::workspace::code_view::CodeView;
use crate::workspace::machine_source::MachineSource;
use crate::workspace::session_keys::tests::session;
use crate::workspace::{Auth, View, Workspace};

const LONG_TITLE: &str =
    "A title long enough to fill the sidebar row many times over, so it must truncate before the tag";

/// A 60-character title and a 40-character name (`session_name::MAX`): the row's worst case.
const SIXTY: &str = "Ship the release notes for the gateway and check the deploys";
const FORTY: &str = "orchestrator-of-the-release-train-north1";
/// The long session's row tag, by its selector (`<row id>-name=<name>`).
const LONG_TAG: &str = "code-long-name=orchestrator-of-the-release-train-north1";
/// A title keeps at least this much of the row beside a 40-character name.
const TITLE_FLOOR: f32 = 24.;
/// The orchestrator's five sessions, as a person names them, each with a title of about 40 characters: the name
/// (also the session's id), the title, and the selectors of its row's tag (`<row id>-name=<name>`) and title.
const CREW: [(&str, &str, &str, &str); 5] = [
    (
        "worker",
        "Port the sandbox's cache key to the core",
        "code-worker-name=worker",
        "code-worker-title",
    ),
    (
        "worker-quality",
        "Write the fifteen-minute journey for keys",
        "code-worker-quality-name=worker-quality",
        "code-worker-quality-title",
    ),
    (
        "worker-simulation",
        "Add a negative control to the bad day run",
        "code-worker-simulation-name=worker-simulation",
        "code-worker-simulation-title",
    ),
    (
        "worker-benchmarking",
        "Guard the added latency against a wrong sign",
        "code-worker-benchmarking-name=worker-benchmarking",
        "code-worker-benchmarking-title",
    ),
    (
        "worker-ci",
        "Trim the CI checks down to the changed paths",
        "code-worker-ci-name=worker-ci",
        "code-worker-ci-title",
    ),
];

/// A signed-in workspace with one project holding a named and an unnamed session, `open` open. No store, no agent.
fn fixture<'a>(open: &str, cx: &'a mut TestAppContext) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let mut named = session("named");
    named.title = LONG_TITLE.into();
    named.name = Some("orchestrator".into());
    let plain = session("plain");
    let mut long = session("long");
    long.title = SIXTY.into();
    long.name = Some(FORTY.into());
    let title = if open == "named" { LONG_TITLE } else { "plain" };
    workspace_with(open, title, vec![named, plain, long], cx)
}

/// The orchestrator's five named sessions in one project, the first open.
fn crew(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    let sessions = CREW
        .iter()
        .map(|(name, title, _, _)| {
            let mut crew = session(name);
            crew.title = (*title).into();
            crew.name = Some((*name).into());
            crew
        })
        .collect();
    workspace_with(CREW[0].0, CREW[0].1, sessions, cx)
}

/// A signed-in workspace with one project holding `sessions`, `open` open under `title`. No store, no agent.
fn workspace_with<'a>(
    open: &str,
    title: &str,
    sessions: Vec<crate::code::CodeSession>,
    cx: &'a mut TestAppContext,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let (gateway, _) = crate::gateway::Gateway::start();
    let (_, events) = futures::channel::mpsc::unbounded();
    let (_, arrivals) = futures::channel::mpsc::unbounded();
    let open = open.to_string();
    let title = title.to_string();
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        let mut workspace = Workspace::with_machine_source(
            std::sync::Arc::new(gateway),
            events,
            arrivals,
            false,
            MachineSource::isolated(),
            window,
            cx,
        );
        workspace._code_poll = gpui_kit::Task::ready(());
        workspace.auth = Auth::SignedIn;
        workspace.title = title.into();
        workspace.projects = vec![CodeProject {
            id: "project".into(),
            name: "project".into(),
            branch: None,
            pinned: false,
            remotes: Vec::new(),
            sessions,
            machine: None,
            added_at: 0,
        }];
        // Drawn, an open session reads its agents and its project's commands from the store in the person's data
        // folder (`sync_agents_tool`, `project_commands::read`). Both are known up front here, so no test opens it.
        workspace.side.fixture_session(&open);
        workspace.side.agents = Some(AgentsTool::resting(open.clone(), cx));
        crate::project_commands::saved("project", Vec::new(), cx);
        let mut view = CodeView::new(open, window, cx);
        view.loaded = true;
        workspace.view = View::Code(view);
        workspace
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (workspace, cx)
}

#[gpui_kit::test]
fn a_named_sessions_row_and_title_show_its_name_beside_a_truncated_title(cx: &mut TestAppContext) {
    let (_, cx) = fixture("named", cx);
    let row = cx
        .debug_bounds("code-named-name=orchestrator")
        .expect("the named session's row shows its name");
    assert!(
        row.size.width > px(0.) && row.size.height > px(0.),
        "the row's tag is visible: {row:?}"
    );
    assert!(
        row.right() <= px(SIDEBAR),
        "the long title truncates and the tag stays inside the sidebar: {row:?}"
    );
    assert!(
        row.size.height < px(super::ROW),
        "the tag fits the row's height: {row:?}"
    );
    let title = cx
        .debug_bounds("title-session-name=orchestrator")
        .expect("the open named session's title shows its name");
    assert!(
        title.size.width > px(0.) && title.size.height > px(0.),
        "the title's tag is visible: {title:?}"
    );
    let window = cx.update(|window, _| window.viewport_size());
    assert!(
        title.right() <= window.width,
        "the title's tag stays in the window: {title:?}"
    );
}

#[gpui_kit::test]
fn an_unnamed_session_shows_no_name(cx: &mut TestAppContext) {
    let (workspace, cx) = fixture("plain", cx);
    assert!(
        cx.debug_bounds("title-session-name=orchestrator").is_none(),
        "the open unnamed session's title shows no name"
    );
    assert!(
        cx.debug_bounds("code-named-name=orchestrator").is_some(),
        "the named row keeps its tag"
    );
    // The name removed: the tag goes with it.
    workspace.update(cx, |workspace, cx| {
        workspace.projects[0].sessions[0].name = None;
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("code-named-name=orchestrator").is_none(),
        "a session whose name is removed shows no tag"
    );
}

/// The width `text` takes drawn whole in the row's tag: its text at the tag's 11px, and the tag's 6px padding each side.
fn whole_tag_width(text: &str, cx: &mut VisualTestContext) -> f32 {
    cx.update(|window, _| {
        let run = window.text_style().to_run(text.len());
        let line = window
            .text_system()
            .shape_line(text.to_string().into(), px(11.), &[run], None);
        f32::from(line.width) + 12.
    })
}

/// The five names share "worker-", so a tag must show that and two letters of its own (`worker-qu…`) to read apart.
const DISTINCT: usize = "worker-".len() + 2;

#[gpui_kit::test]
fn five_workers_rows_each_show_their_name_up_to_its_share(cx: &mut TestAppContext) {
    let (_, cx) = crew(cx);
    for (name, title, tag, row_title) in CREW {
        assert!((38..=46).contains(&title.len()), "{title:?} is about 40 characters");
        let tag = cx
            .debug_bounds(tag)
            .unwrap_or_else(|| panic!("{name}'s row shows its name"));
        let row_title = cx
            .debug_bounds(row_title)
            .unwrap_or_else(|| panic!("{name}'s row shows its title"));
        let (tag_w, title_w) = (f32::from(tag.size.width), f32::from(row_title.size.width));
        let whole = whole_tag_width(name, cx);
        // The row's text: the title, the 8px gap and the tag.
        let share = super::ROW_NAME_SHARE * (title_w + 8. + tag_w);
        eprintln!("{name}: tag {tag_w}px of {whole}px whole, share {share}px, title {title_w}px");
        if whole <= share {
            assert!(
                tag_w + 0.5 >= whole,
                "{name} fits its share, so its tag shows it whole: {tag_w}px drawn, {whole}px whole"
            );
        } else {
            assert!(
                (tag_w - share).abs() <= 0.5,
                "{name} is longer than its share, so its tag takes the whole share: {tag_w}px, share {share}px"
            );
        }
        let shown = &name[..name.len().min(DISTINCT)];
        let distinct = whole_tag_width(&format!("{shown}…"), cx).min(whole);
        assert!(
            tag_w + 0.5 >= distinct,
            "{name}'s tag shows at least {shown:?}, so the five read apart: {tag_w}px, {distinct}px needed"
        );
        assert!(tag.right() <= px(SIDEBAR), "{name}'s tag stays in the sidebar: {tag:?}");
        assert!(
            tag.size.height < px(super::ROW),
            "{name}'s tag fits the row's height: {tag:?}"
        );
    }
}

#[gpui_kit::test]
fn a_forty_character_name_still_leaves_the_title_visible(cx: &mut TestAppContext) {
    assert_eq!((SIXTY.len(), FORTY.len()), (60, 40));
    assert!(LONG_TAG.ends_with(FORTY));
    let (_, cx) = fixture("named", cx);
    let title = cx
        .debug_bounds("code-long-title")
        .expect("the long session's title is drawn");
    let tag = cx.debug_bounds(LONG_TAG).expect("the long session's name is drawn");
    let (title_w, tag_w) = (f32::from(title.size.width), f32::from(tag.size.width));
    eprintln!("title {title_w}px, tag {tag_w}px");
    assert!(
        title_w > TITLE_FLOOR,
        "the title keeps more than {TITLE_FLOOR}px beside a 40-character name: title {title_w}px, tag {tag_w}px"
    );
    assert!(
        tag_w + 0.5 < whole_tag_width(FORTY, cx),
        "a name longer than its share is cut with an ellipsis: {tag_w}px"
    );
    assert!(tag.right() <= px(SIDEBAR), "the tag stays in the sidebar: {tag:?}");
}

#[test]
fn a_named_rows_hover_text_carries_its_whole_name() {
    let title: gpui_kit::SharedString = SIXTY.into();
    assert_eq!(
        super::row_tip(&title, Some(FORTY)).as_ref(),
        format!("{SIXTY} · name: {FORTY}")
    );
    assert_eq!(super::row_tip(&title, None).as_ref(), SIXTY);
}

#[gpui_kit::test]
fn the_hover_card_wraps_its_title_and_stays_open_under_its_pointer(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, point};
    use std::time::Duration;

    let (workspace, cx) = fixture("plain", cx);
    let row = cx.debug_bounds("code-named-title").unwrap();
    cx.simulate_mouse_move(row.center(), None, Modifiers::none());
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(501));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let card = cx
        .debug_bounds("session-peek-card")
        .expect("the delayed hover card appears");
    let title = cx.debug_bounds("session-peek-title").unwrap();
    assert!(title.size.height > px(18.));
    assert!(title.left() >= card.left() && title.right() <= card.right());
    // Crossing the gap must not close the card before the pointer reaches it.
    cx.simulate_mouse_move(point(card.left() - px(3.), row.center().y), None, Modifiers::none());
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.simulate_mouse_move(card.center(), None, Modifiers::none());
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("session-peek-card").is_some());
    cx.simulate_click(card.center(), Modifiers::none());
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert!(matches!(&workspace.view, View::Code(code) if code.id == "plain"))
    });
    cx.simulate_mouse_move(point(card.right() + px(20.), card.top()), None, Modifiers::none());
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("session-peek-card").is_none(),
        "leaving the card dismisses it"
    );
}
