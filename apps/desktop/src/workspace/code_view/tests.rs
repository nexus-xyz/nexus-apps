use core::prelude::v1::test;

use super::*;

#[gpui_kit::test]
fn opening_a_long_transcript_leaves_markdown_unparsed(cx: &mut TestAppContext) {
    let blocks: Vec<_> = (0..600)
        .map(|ix| Block::Text(format!("Answer {ix}\n\n```rust\nfn main() {{}}\n```")))
        .collect();
    cx.update(|cx| {
        let mut texts = retain_texts(&[], &mut [], &blocks, cx);
        assert_eq!(texts.len(), blocks.len());
        assert!(texts.iter().all(Option::is_none));
        // The list asks for one answer. A refresh preserves that entity without parsing the other 599.
        let Block::Text(markdown) = &blocks[599] else {
            unreachable!()
        };
        let visible = cx.new(|cx| TextViewState::markdown(markdown, cx));
        texts[599] = Some(visible.clone());
        let refreshed = retain_texts(&blocks, &mut texts, &blocks, cx);
        assert!(refreshed[..599].iter().all(Option::is_none));
        assert_eq!(refreshed[599].as_ref(), Some(&visible));
    });
}

fn tool(name: &str, summary: &str) -> Block {
    Block::Tool {
        call_id: format!("{name}:{summary}"),
        name: name.into(),
        summary: summary.into(),
        error: None,
        edit: None,
        images: Vec::new(),
        call: Default::default(),
    }
}

#[test]
fn every_row_of_an_answer_hovers_its_turn() {
    let blocks = vec![
        user(1, "first", false, false),
        Block::Text("answer".into()),
        turn_end(2),
        user(3, "second", false, false),
        Block::Text("still working".into()),
    ];
    assert_eq!(turn_of(&blocks, 0), None);
    assert_eq!(turn_of(&blocks, 1), Some(2));
    assert_eq!(turn_of(&blocks, 2), Some(2));
    assert_eq!(turn_of(&blocks, 3), None);
    assert_eq!(turn_of(&blocks, 4), None);
    assert_eq!(turn_of(&blocks, 5), None);
}

fn turn_end(seq: i64) -> Block {
    Block::Turn {
        seq,
        at: 0,
        status: TurnStatus::Completed,
        duration_ms: 1000,
        cost_usd: None,
        tokens: 0,
        model: None,
        reverted: false,
    }
}

#[test]
fn the_minimap_has_a_tick_per_message_and_per_answer() {
    let blocks = [
        user(1, "Fix the build", false, false),
        Block::Thinking("Look first".into(), None),
        tool("Read", "src/main.rs"),
        Block::Text("## Fixed\nIt builds.".into()),
        turn_end(5),
        user(6, "Now test it", false, false),
        tool("ToolSearch", "select:Bash"),
        tool("Bash", "cargo test"),
        turn_end(9),
        Block::Queued {
            seq: 10,
            text: "and lint".into(),
        },
    ];
    let rows = minimap_rows(&blocks);
    let ticks: Vec<(usize, bool)> = rows.iter().map(|row| (row.item, row.user)).collect();
    // The answers start at their first row that shows: the thinking, and the Bash call past the plumbing.
    assert_eq!(ticks, [(0, true), (1, false), (5, true), (7, false)]);
    assert_eq!(minimap_preview(&blocks, &rows, 0), ("You", "Fix the build".into()));
    assert_eq!(minimap_preview(&blocks, &rows, 1), ("Answer", "Fixed".into()));
    // An answer with no text reads as its first tool.
    assert_eq!(minimap_preview(&blocks, &rows, 3), ("Answer", "Run cargo test".into()));
}

#[test]
fn a_turn_end_closes_its_answer() {
    let blocks = [
        Block::Text("Resumed".into()),
        turn_end(2),
        Block::Error("It stopped".into(), None),
        user(4, "", false, false),
    ];
    let rows = minimap_rows(&blocks);
    let ticks: Vec<(usize, bool)> = rows.iter().map(|row| (row.item, row.user)).collect();
    assert_eq!(ticks, [(0, false), (2, false), (3, true)]);
    assert_eq!(minimap_preview(&blocks, &rows, 1), ("Answer", "No text".into()));
    assert_eq!(minimap_preview(&blocks, &rows, 2), ("You", "No text".into()));
    assert!(minimap_rows(&[Block::Plan(Vec::new())]).is_empty());
}

#[test]
fn the_now_line_names_the_call_still_waiting_on_its_result() {
    let mut running = tool("Bash", "cargo bench --bench replay");
    assert_eq!(
        now_doing(std::slice::from_ref(&running)),
        Some(("Running", "cargo bench --bench replay".into()))
    );
    if let Block::Tool { call, .. } = &mut running {
        call.output = Some(Default::default());
    }
    assert_eq!(
        now_doing(&[running]),
        None,
        "a finished call leaves the line on Thinking"
    );
    assert_eq!(now_doing(&[Block::Text("Done".into())]), None);
}

#[test]
fn the_now_line_starts_then_generates_with_a_live_summary() {
    assert_eq!(
        now_line(None, false, None, None),
        (Now::Generating, "Thinking…".into(), None)
    );
    // The latest thought is the summary; a running call outranks it.
    assert_eq!(
        now_line(None, false, None, Some("Reading the gateway routes…".into())),
        (Now::Generating, "Reading the gateway routes…".into(), None)
    );
    assert_eq!(
        now_line(
            None,
            false,
            Some(("Running", "cargo test".into())),
            Some("A thought…".into())
        ),
        (Now::Generating, "Running".into(), Some("cargo test".into()))
    );
    // A call with nothing to name shows its verb alone.
    assert_eq!(
        now_line(None, false, Some(("Working", String::new())), None),
        (Now::Generating, "Working".into(), None)
    );
    // Waiting on the person comes before the call it waits on; a step before the model starts comes before both.
    assert_eq!(
        now_line(None, true, Some(("Running", "rm -rf build".into())), None),
        (Now::Waiting, "Waiting for you".into(), None)
    );
    assert_eq!(
        now_line(
            Some(("Running the setup script".into(), Some("npm ci".into()))),
            true,
            None,
            None
        ),
        (Now::Starting, "Running the setup script".into(), Some("npm ci".into()))
    );
}

#[test]
fn a_session_being_made_says_its_step() {
    let new = |worktree: bool, review: Option<u64>, base: Option<&str>| NewSession {
        project_id: String::new(),
        agent: nexus_workbench::event::SessionAgent::Claude,
        model: None,
        permission_mode: "default".into(),
        effort: None,
        via: nexus_workbench::event::Via::Nexus,
        provider: None,
        worktree,
        review,
        base: base.map(String::from),
        text: "Fix the login".into(),
        attachments: Default::default(),
    };
    assert_eq!(
        start_words(&new(true, None, Some("origin/release")), false),
        "Creating a worktree from release…"
    );
    assert_eq!(
        start_words(&new(true, None, None), false),
        "Creating a worktree from main…"
    );
    assert_eq!(
        start_words(&new(true, Some(123), None), false),
        "Checking out pull request #123…"
    );
    assert_eq!(
        start_words(&new(false, None, None), true),
        "Starting the cloud machine…"
    );
    assert_eq!(start_words(&new(false, None, None), false), "Starting the session…");
}

#[test]
fn a_running_setup_script_is_a_step_before_the_model() {
    let mut row = nexus_workbench::setup_timeline::SetupRow::new("setup");
    row.command = "npm ci".into();
    row.status = nexus_workbench::event::SetupStatus::Running;
    assert_eq!(
        setup_running(&[Block::Setup(Box::new(row.clone()))]),
        Some("npm ci".into())
    );
    row.status = nexus_workbench::event::SetupStatus::Done;
    assert_eq!(setup_running(&[Block::Setup(Box::new(row))]), None);
}

#[test]
fn a_turn_keeps_its_icon_and_the_next_message_draws_again() {
    let turn = [
        user(2, "Fix it", false, false),
        Block::Thinking("Reading.".into(), None),
    ];
    let seed = turn_seed("session", &turn);
    // Work within the turn does not change it.
    let mut later = turn.to_vec();
    later.push(tool("Bash", "cargo test"));
    assert_eq!(turn_seed("session", &later), seed);
    // A new message, or another session, does.
    later.push(user(9, "And the docs", false, false));
    assert_ne!(turn_seed("session", &later), seed);
    assert_ne!(turn_seed("other", &turn), seed);
}

#[test]
fn the_summary_is_the_latest_thoughts_first_sentence_of_this_turn() {
    let thought = |text: &str| Block::Thinking(text.into(), None);
    let blocks = [
        thought("An old thought."),
        user(2, "Now fix it", false, false),
        thought("**Reading the routes.** Then the tests."),
        thought("Comparing the two retry paths: the first one waits."),
    ];
    assert_eq!(thought_summary(&blocks), Some("Comparing the two retry paths…".into()));
    // Markdown's marks go, and the first sentence stands alone.
    assert_eq!(thought_summary(&blocks[..3]), Some("Reading the routes…".into()));
    // A new message starts a turn with no thought yet.
    assert_eq!(thought_summary(&blocks[..2]), None);
    // A long sentence is cut at a word, with one ellipsis.
    let long = thought(&"word ".repeat(30));
    let summary = thought_summary(&[long]).unwrap();
    assert!(
        summary.chars().count() <= SUMMARY + 1 && summary.ends_with("word…"),
        "{summary}"
    );
}

#[test]
fn a_message_sent_into_the_turn_leaves_the_now_line_on_the_call() {
    let blocks = [tool("Bash", "cargo test"), user(2, "also check the docs", false, false)];
    assert_eq!(now_doing(&blocks), Some(("Running", "cargo test".into())));
}

#[test]
fn elapsed_reads_as_the_working_line() {
    assert_eq!(elapsed(Duration::from_secs(8)), "8s");
    assert_eq!(elapsed(Duration::from_secs(125)), "2m 05s");
    assert_eq!(elapsed(Duration::from_secs(3780)), "1h 03m");
}

#[test]
fn plumbing_tools_take_no_row() {
    assert!(hidden_tool("ToolSearch"));
    assert!(hidden_tool("AskUserQuestion"));
    assert!(!hidden_tool("Bash"));
    assert!(!hidden_tool("mcp__nexus__createPage"));
}

#[test]
fn mcp_tools_read_in_words() {
    assert_eq!(tool_verb("mcp__nexus__createPage"), "Nexus · create page");
    assert_eq!(tool_verb("mcp__nexus__writePageVersion"), "Nexus · write page version");
    assert_eq!(tool_verb("mcp__claude_ai_Linear__list_issues"), "Linear · list issues");
    assert_eq!(tool_verb("mcp__slack__slack_read_channel"), "Slack · read channel");
    assert_eq!(tool_verb("mcp__plugin_linear_linear__get_issue"), "Linear · get issue");
    assert_eq!(tool_verb("mcp__my-tools__do_thing"), "my-tools · do thing");
    // A lookalike keeps its own name, so it cannot pass for the app.
    assert_eq!(tool_verb("mcp__evil-slack__post"), "evil-slack · post");
    assert_eq!(ask_verb("mcp__evil-slack__post"), "use Slack (evil-slack) · post");
    assert_eq!(ask_verb("mcp__nexus__createPage"), "use Nexus · create page");
    for name in [
        "mcp__nexus__createPage",
        "mcp__claude_ai_Linear__list_issues",
        "mcp__x__y",
    ] {
        assert!(!tool_verb(name).contains("mcp__"), "{name}");
        assert!(!ask_verb(name).contains("mcp__"), "{name}");
    }
    assert_eq!(tool_verb("Bash"), "Run");
    assert_eq!(tool_verb("mcp__"), "mcp__");
}

#[test]
fn a_question_row_reads_as_one_verb() {
    assert_eq!(ask_line(QUESTION_TOOL), "Asked you a question");
    assert_eq!(ask_line("Bash"), "Asked to run a command");
}

#[test]
fn a_rewind_puts_the_message_back_under_what_is_typed() {
    assert_eq!(restored_draft("", "fix the bug"), "fix the bug");
    assert_eq!(restored_draft("  ", "fix the bug"), "fix the bug");
    assert_eq!(
        restored_draft("also this \n", "fix the bug"),
        "also this\n\nfix the bug"
    );
}

fn user(seq: i64, text: &str, linked: bool, rewound: bool) -> Block {
    Block::User {
        seq,
        at: 0,
        text: text.into(),
        from: linked.then(|| nexus_workbench::event::Sender::Session {
            thread_id: "lead".into(),
            title: "Lead".into(),
        }),
        rewound,
        images: 0,
        image_data: Vec::new(),
    }
}

#[test]
fn an_edit_rewinds_then_sends_the_new_text() {
    assert_eq!(
        edit_calls("t-1", 7, "fix the bug, gently", 0),
        Some([
            EditCall::Rewind {
                thread: "t-1".into(),
                seq: 7
            },
            EditCall::Send {
                thread: "t-1".into(),
                text: "fix the bug, gently".into()
            },
        ])
    );
    // An empty edit sends nothing, and rewinds nothing.
    assert_eq!(edit_calls("t-1", 7, "  \n", 0), None);
}

#[test]
fn only_your_own_live_messages_take_edit_and_rewind() {
    assert!(can_act_on(true, false, false, false));
    // Read-only (the Nexus app runs the session), a linked session's message, one rewound, a session not made yet.
    assert!(!can_act_on(false, false, false, false));
    assert!(!can_act_on(true, true, false, false));
    assert!(!can_act_on(true, false, true, false));
    assert!(!can_act_on(true, false, false, true));
}

#[test]
fn a_restore_puts_back_the_first_message_it_undoes() {
    let blocks = vec![
        user(1, "first", false, false),
        user(5, "linked", true, false),
        user(6, "already rewound", false, true),
        user(8, "second", false, false),
        user(12, "third", false, false),
    ];
    assert_eq!(
        prompt_after(&blocks, 4).as_ref().map(|prompt| prompt.text.as_str()),
        Some("second")
    );
    assert_eq!(
        prompt_after(&blocks, 10).as_ref().map(|prompt| prompt.text.as_str()),
        Some("third")
    );
    assert_eq!(prompt_after(&blocks, 12), None);
}

#[test]
fn image_only_edits_and_restores_keep_the_source_prompt() {
    let image = nexus_workbench::event::Image {
        media_type: "image/png".into(),
        data: "aGVsbG8=".into(),
        name: Some("shot.png".into()),
    };
    assert!(edit_calls("t", 7, "", 1).is_some());
    let mut block = user(7, "", false, false);
    if let Block::User { image_data, images, .. } = &mut block {
        *images = 1;
        image_data.push(image.clone());
    }
    let restored = prompt_after(&[block], 3).expect("image-only prompt restores");
    assert_eq!(restored.seq, 7);
    assert_eq!(restored.text, "");
    assert_eq!(restored.images, vec![image]);
}

#[test]
fn a_restore_says_how_many_files_change() {
    assert!(restore_question(0).starts_with("The files are already as they were after this turn."));
    assert!(restore_question(1).starts_with("1 file will change back to its version from this turn."));
    assert!(restore_question(3).starts_with("3 files will change back to their version from this turn."));
}

fn session() -> CodeSession {
    CodeSession {
        saved_provider: false,
        id: "plan-thread".into(),
        linked: None,
        title: "Plan it".into(),
        name: None,
        status: Status::NeedsYou,
        updated_at: 0,
        created_at: 0,
        acted_at: 0,
        agent: nexus_workbench::event::SessionAgent::Claude,
        model: Some("claude-opus-5-5".into()),
        mode: PermissionMode::Plan,
        effort: None,
        fast: false,
        account: None,
        connection: None,
        via: nexus_workbench::event::Via::Subscription,
        pinned: false,
        branch: Some("nexus/plan-it".into()),
        snoozed_until: None,
        settled: false,
        archived: false,
        machine: None,
        limited: None,
        interrupted: false,
        live: false,
        parent: None,
        background: false,
        background_busy: false,
        background_work: vec![],
        background_total: 0,
        waits_on: vec![],
        source: None,
        has_worktree: false,
        side_chat: false,
        compact: false,
        orchestration: false,
        at_depth_limit: false,
        handoff_from: None,
        agent_started: false,
        crew: Default::default(),
        start_choices: Vec::new(),
        project_id: "project".into(),
        cost_usd: 0.0,
        context: None,
        pending: None,
        pull_request: None,
        review: false,
        activity: nexus_workbench::attention::Activity {
            status: nexus_workbench::event::ThreadStatus::Approval,
            updated_at: 0,
            listed_at: None,
        },
    }
}

#[test]
fn implement_in_a_new_session_starts_on_the_plan_then_stops_the_old_one() {
    let plan = "# Plan: Ship the thing\n\n1. Do it";
    let (new, moved) = implement_elsewhere(&session(), "request-1", plan);
    assert_eq!(new.project_id, "project");
    assert_eq!(new.agent, nexus_workbench::event::SessionAgent::Claude);
    assert_eq!(new.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(new.permission_mode, "acceptEdits");
    assert_eq!(new.via, nexus_workbench::event::Via::Subscription);
    assert!(new.worktree);
    assert_eq!(
        new.text,
        "Implement this plan. It was made and approved in an earlier session.\n\n# Plan: Ship the thing\n\n1. Do it"
    );
    assert_eq!(
        moved,
        MovedFrom {
            thread: "plan-thread".into(),
            request: "request-1".into(),
            title: "Implement Ship the thing".into(),
        }
    );
    // Once the new session exists: its title, then the planning session told to stop.
    let calls = moved_on("new-thread", moved);
    assert!(matches!(
        &calls[0],
        MovedOnCall::Title(args) if args.thread_id == "new-thread"
            && args.title.as_deref() == Some("Implement Ship the thing")
            && args.permission_mode.is_none()
    ));
    assert!(matches!(
        &calls[1],
        MovedOnCall::Stop { thread, request, message }
            if thread == "plan-thread" && request == "request-1" && message == MOVED_ON
    ));
    assert_eq!(implement_title("   "), "Implement the plan");
}

fn texts(texts: &[&str]) -> Vec<Block> {
    texts.iter().map(|text| Block::Text(text.to_string())).collect()
}

#[test]
fn an_older_page_counts_the_blocks_it_put_above() {
    let shown = texts(&["c", "d", "e", "f"]);
    assert_eq!(older_blocks(&shown, &texts(&["a", "b", "c", "d", "e", "f"])), 2);
    // The first block shown took in older events, and the last grew meanwhile.
    assert_eq!(older_blocks(&shown, &texts(&["a", "b", "bc", "d", "e", "fg"])), 2);
    // A block that repeats earlier matches where it would be.
    let repeats = texts(&["x", "d", "e", "y", "c", "d", "e", "f"]);
    assert_eq!(older_blocks(&shown, &repeats), 4);
    // Too few blocks to match: the count that was added.
    assert_eq!(older_blocks(&texts(&["c"]), &texts(&["a", "b", "c"])), 2);
}

struct Bubble(&'static str, Receipt);

impl Render for Bubble {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette =
            crate::palette::Palette::for_choice(crate::palette::ThemeChoice::System, gpui_kit::WindowAppearance::Light);
        v_flex()
            .w(px(600.))
            .items_end()
            .child(user_bubble(self.0, Vec::new(), palette))
            .child(render_receipt(1, self.1, "Claude Code", false, palette))
    }
}

#[gpui_kit::test]
fn the_checks_sit_under_the_bubble_at_its_right_and_a_long_message_wraps(cx: &mut TestAppContext) {
    let long = "And then check that the latest nightly build still launches on this machine, with all my changes, \
                and say which of them are still waiting for a review before the build can start";
    cx.update(gpui_kit::init);
    let mut widths = Vec::new();
    for (text, receipt) in [
        ("also which pull requests do we have open?", Receipt::Sent),
        (long, Receipt::Read(Some(0))),
    ] {
        let (_, cx) = cx.add_window_view(|_, _| Bubble(text, receipt));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let bubble = cx.debug_bounds("user-bubble").expect("the bubble is drawn");
        let read = cx.debug_bounds("user-read").expect("the receipt is drawn");
        // Under the bubble, not in it, and flush with its right edge.
        assert!(read.top() >= bubble.bottom());
        assert_eq!(read.right(), bubble.right());
        // The bubble never passes 80% of the column, so a long message wraps.
        assert!(bubble.size.width <= px(480.));
        widths.push(
            cx.debug_bounds("user-read-checks")
                .expect("the checks are drawn")
                .size
                .width,
        );
    }
    // Read draws a second check beside the first.
    assert!(widths[1] > widths[0]);
}

#[test]
fn a_message_reads_as_read_once_the_agent_says_so_or_answers_after_it() {
    let user = |seq| Block::User {
        seq,
        at: 0,
        text: "hi".into(),
        from: None,
        rewound: false,
        images: 0,
        image_data: Vec::new(),
    };
    let reads = std::collections::BTreeMap::from([(1, 500)]);
    let blocks = vec![user(1), Block::Text("hello".into()), user(3), user(4)];
    assert_eq!(receipt_of(1, &reads, &blocks), Receipt::Read(Some(500)));
    assert_eq!(receipt_of(3, &Default::default(), &blocks), Receipt::Sent);
    assert_eq!(receipt_of(1, &Default::default(), &blocks), Receipt::Read(None));

    // A turn that ended Interrupted or Errored with nothing said is not an answer; a Completed one is.
    let turn = |status| Block::Turn {
        seq: 2,
        at: 0,
        status,
        duration_ms: 0,
        cost_usd: None,
        tokens: 0,
        model: None,
        reverted: false,
    };
    for status in [TurnStatus::Interrupted, TurnStatus::Error] {
        let blocks = vec![user(1), turn(status)];
        assert_eq!(receipt_of(1, &Default::default(), &blocks), Receipt::Sent);
    }
    let blocks = vec![user(1), turn(TurnStatus::Completed)];
    assert_eq!(receipt_of(1, &Default::default(), &blocks), Receipt::Read(None));
}

#[test]
fn an_answered_tool_request_reads_with_the_canons_words() {
    let line = |tool, answer| answered_label(tool, answer, false);
    assert_eq!(line("Bash", Some(Decision::Allow)), "Allowed once");
    assert_eq!(line("Bash", Some(Decision::AllowSession)), "Allowed for this session");
    assert_eq!(line("Bash", Some(Decision::Deny)), "Denied");
    assert_eq!(line("Bash", Some(Decision::Expired)), "Not answered");
    assert_eq!(line("Bash", None), "Waiting for you");
    // Questions and plans keep their own words.
    assert_eq!(line(QUESTION_TOOL, Some(Decision::Allow)), "Answered");
    assert_eq!(line(PLAN_TOOL, Some(Decision::AllowSession)), "Approved");
    assert_eq!(line(PLAN_TOOL, Some(Decision::Deny)), "Not approved");
    assert_eq!(
        answered_label(PLAN_TOOL, Some(Decision::Deny), true),
        "Moved to a new session"
    );
}

#[test]
fn ask_the_agent_to_fix_it_sends_the_step_and_its_error() {
    assert_eq!(
        fix_request("Bash", "gh pr create --draft", "error: no upstream\n"),
        "This step failed: Ran gh pr create --draft\n\n```\nerror: no upstream\n```\n\nFix it."
    );
    assert!(fix_request("Bash", "", "boom").starts_with("This step failed: Ran\n"));
}
