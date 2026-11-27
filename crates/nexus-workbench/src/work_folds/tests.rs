//! the Nexus web app, ported.
use super::*;

fn bash() -> Step {
    Step::call("Bash")
}
fn read() -> Step {
    Step::call("Read")
}
fn edit() -> Step {
    Step::call("Edit")
}
fn thought() -> Step {
    Step::Thinking
}
fn failed(name: &str) -> Step {
    Step::Call {
        name: name.into(),
        running: false,
        failed: true,
        marked: false,
    }
}
fn running(name: &str) -> Step {
    Step::Call {
        name: name.into(),
        running: true,
        failed: false,
        marked: false,
    }
}
fn marked(name: &str) -> Step {
    Step::Call {
        name: name.into(),
        running: false,
        failed: false,
        marked: true,
    }
}

#[test]
fn keeps_file_changes_sub_agents_plans_and_approvals_in_view() {
    assert!(important(&edit()));
    assert!(important(&Step::call("Write")));
    assert!(important(&Step::call("Task")));
    assert!(important(&Step::call("ExitPlanMode")));
    assert!(!important(&bash()));
    // Waiting for the person, or denied: marked.
    assert!(important(&marked("Bash")));
    assert!(!important(&thought()));
}

#[test]
fn folds_failed_calls_with_the_rest() {
    assert!(!important(&failed("Bash")));
}

#[test]
fn folds_the_routine_steps_between_important_ones_in_order() {
    let steps = [bash(), read(), thought(), edit(), bash(), bash()];
    assert_eq!(
        segments(&steps, false, 2),
        vec![Segment::Fold(0..3), Segment::Step(3), Segment::Fold(4..6)]
    );
}

#[test]
fn shows_a_lone_routine_step_as_itself() {
    let steps = [edit(), bash(), edit()];
    assert!(
        segments(&steps, false, 2)
            .iter()
            .all(|part| matches!(part, Segment::Step(_)))
    );
}

#[test]
fn shows_the_step_running_now_below_its_fold_while_the_agent_works() {
    let steps = [bash(), read(), running("Bash")];
    assert_eq!(segments(&steps, true, 2), vec![Segment::Fold(0..2), Segment::Step(2)]);
    // Once the turn is not live, it folds with the rest.
    assert_eq!(segments(&steps, false, 2), vec![Segment::Fold(0..3)]);
}

#[test]
fn says_what_the_steps_did_exploring_first() {
    let steps = [
        bash(),
        read(),
        bash(),
        Step::call("Grep"),
        Step::call("Glob"),
        read(),
        bash(),
    ];
    assert_eq!(fold_summary(&steps), "Explored 2 files, ran 3 commands");
    assert_eq!(
        fold_summary(&[read(), Step::call("WebFetch"), Step::call("mcp__linear__get_issue")]),
        "Explored a file, fetched a page, used a tool"
    );
    // Searches with nothing read explored no file.
    assert_eq!(
        fold_summary(&[Step::call("Grep"), Step::call("Grep")]),
        "Searched 2 times"
    );
    assert_eq!(
        fold_summary(&[edit(), edit(), Step::call("Write")]),
        "Edited 2 files, wrote a file"
    );
    assert_eq!(fold_summary(&[thought(), thought()]), "Thought");
    // A card's header is the same line.
    assert_eq!(
        fold_summary(&[read(), thought(), bash()]),
        "Explored a file, ran a command"
    );
}

#[test]
fn counts_the_failures() {
    assert_eq!(fold_failures(&[bash(), failed("Bash")]), 1);
    assert_eq!(fold_facts(&[bash(), failed("Bash")]), "2 steps · 1 failed");
    assert_eq!(fold_facts(&[bash()]), "1 step");
}

#[test]
fn puts_calls_between_prose_in_one_card_agents_in_another_and_a_plan_alone() {
    let steps = [
        read(),
        bash(),
        Step::call("Task"),
        Step::call("Task"),
        edit(),
        Step::call("ExitPlanMode"),
        bash(),
    ];
    let shape: Vec<String> = cards(&steps, false)
        .iter()
        .map(|card| match card {
            Card::Plan(_) => "plan".to_string(),
            Card::Agents(range) => format!("agents:{}", range.len()),
            Card::Tools { steps, .. } => format!("tools:{}", steps.len()),
        })
        .collect();
    assert_eq!(shape, ["tools:2", "agents:2", "tools:1", "plan", "tools:1"]);
}

#[test]
fn shows_each_step_of_a_short_card_and_folds_a_long_routine_stretch_beside_an_important_step() {
    let short = cards(&[bash(), read(), edit(), bash()], false);
    let Some(Card::Tools { parts, important, .. }) = short.first() else {
        panic!("a card");
    };
    assert!(*important);
    assert!(parts.iter().all(|part| matches!(part, Segment::Step(_))));
    let long = cards(&[read(), read(), read(), read(), edit()], false);
    let Some(Card::Tools { parts, .. }) = long.first() else {
        panic!("a card");
    };
    assert_eq!(parts, &vec![Segment::Fold(0..4), Segment::Step(4)]);
    // A card of routine steps only is its own fold: every step is a row, and the card closes.
    let routine = cards(&[read(), read(), read(), read(), read()], false);
    let Some(Card::Tools { parts, important, .. }) = routine.first() else {
        panic!("a card");
    };
    assert!(!*important);
    assert!(parts.iter().all(|part| matches!(part, Segment::Step(_))));
}

#[test]
fn a_cards_parts_index_the_whole_run() {
    let steps = [Step::call("Task"), read(), read(), read(), read(), edit()];
    let Some(Card::Tools {
        steps: range, parts, ..
    }) = cards(&steps, false).into_iter().nth(1)
    else {
        panic!("a tools card after the agents");
    };
    assert_eq!(range, 1..6);
    assert_eq!(parts, vec![Segment::Fold(1..5), Segment::Step(5)]);
}

#[test]
fn says_how_long_it_thought_once_it_took_a_second() {
    assert_eq!(thinking_label(None), "Thought");
    assert_eq!(thinking_label(Some(400)), "Thought");
    assert_eq!(thinking_label(Some(14_200)), "Thought for 14 s");
    assert_eq!(thinking_label(Some(125_000)), "Thought for 2 min 5 s");
}

#[test]
fn offers_the_earlier_steps() {
    assert_eq!(earlier_label(1), "Show 1 earlier step");
    assert_eq!(earlier_label(12), "Show 12 earlier steps");
}
