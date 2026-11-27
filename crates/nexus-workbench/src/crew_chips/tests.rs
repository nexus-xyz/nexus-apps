//! crew-controls.test.ts's cases for the crew line's chips, on the same rules.

use super::*;

struct Row {
    id: &'static str,
    parent: Option<&'static str>,
    title: String,
    facts: Facts,
}

/// lead > a > b, and a session on its own.
fn crew(change: impl Fn(&str, &mut Facts)) -> Vec<Row> {
    [("lead", None), ("a", Some("lead")), ("b", Some("a")), ("alone", None)]
        .into_iter()
        .map(|(id, parent)| {
            let mut facts = Facts {
                status: Some(ThreadStatus::Idle),
                ..Facts::default()
            };
            change(id, &mut facts);
            Row {
                id,
                parent,
                title: format!("{id} title"),
                facts,
            }
        })
        .collect()
}

fn crews(rows: &[Row]) -> Crews<'_> {
    Crews::new(rows.iter().map(|row| Session {
        id: row.id,
        parent: row.parent,
        title: &row.title,
        facts: &row.facts,
    }))
}

fn chips_of(rows: &[Row], id: &'static str) -> Vec<String> {
    let facts = &rows
        .iter()
        .find(|row| row.id == id)
        .expect("a session of the crew")
        .facts;
    crew_chips(facts, &crews(rows).place(id))
        .into_iter()
        .map(|chip| chip.text)
        .collect()
}

#[test]
fn tells_the_root_to_send_a_message_and_a_session_below_it_to_write_to_the_root() {
    let paused = crew(|id, facts| facts.crew_paused = id != "alone");
    assert_eq!(chips_of(&paused, "lead"), ["Paused · send a message to continue"]);
    assert_eq!(
        chips_of(&paused, "a"),
        ["Paused · write to the top session «lead title» to continue"]
    );
    assert_eq!(
        chips_of(&paused, "b"),
        ["Paused · write to the top session «lead title» to continue"]
    );
    let spent = crew(|id, facts| {
        if id == "lead" || id == "b" {
            facts.crew_paused = true;
            facts.crew_tripped = true;
        }
    });
    assert_eq!(
        chips_of(&spent, "lead"),
        ["Spend cap reached · send a message to continue"]
    );
    assert_eq!(
        chips_of(&spent, "b"),
        ["Spend cap reached · write to the top session «lead title» to continue"]
    );
}

#[test]
fn shows_a_halt_only_in_a_crew_since_continue_already_says_it_of_a_session_on_its_own() {
    let rows = crew(|id, facts| {
        facts.halt = Some(
            match id {
                "a" => "parent",
                "b" => "fault",
                _ => "person",
            }
            .to_string(),
        );
        if id == "alone" {
            facts.status = Some(ThreadStatus::Interrupted);
        }
    });
    assert_eq!(chips_of(&rows, "lead"), ["Stopped by you"]);
    assert_eq!(chips_of(&rows, "a"), ["Stopped with lead title"]);
    assert_eq!(chips_of(&rows, "b"), ["Stopped by you"]);
    assert!(chips_of(&rows, "alone").is_empty());
}

#[test]
fn says_results_wait_only_while_the_session_runs_nothing() {
    let lead = |pending: u32, status: ThreadStatus| {
        crew(move |id, facts| {
            if id == "lead" {
                facts.pending_results = pending;
                facts.status = Some(status);
            }
        })
    };
    assert_eq!(
        chips_of(&lead(2, ThreadStatus::Idle), "lead"),
        ["2 results go with your next message"]
    );
    assert_eq!(
        chips_of(&lead(1, ThreadStatus::Interrupted), "lead"),
        ["1 result goes with your next message"]
    );
    assert!(chips_of(&lead(2, ThreadStatus::Running), "lead").is_empty());
    assert!(chips_of(&crew(|_, _| {}), "lead").is_empty());
}

#[test]
fn counts_the_sessions_below_that_work_background_ones_and_approvals_included() {
    let running = |change: fn(&str, &mut Facts), id: &'static str| crews(&crew(change)).running(id);
    assert_eq!(running(|_, _| {}, "lead"), 0);
    assert_eq!(
        running(
            |id, facts| match id {
                "a" => facts.status = Some(ThreadStatus::Running),
                "b" => facts.in_background = true,
                _ => {}
            },
            "lead"
        ),
        2
    );
    assert_eq!(
        running(
            |id, facts| {
                if id == "b" {
                    facts.status = Some(ThreadStatus::Starting);
                }
            },
            "a"
        ),
        1
    );
    // A child waiting on an approval still works: an idle lead whose only child waits shows the line and Stop.
    assert_eq!(
        running(
            |id, facts| {
                if id == "a" {
                    facts.status = Some(ThreadStatus::Approval);
                }
            },
            "lead"
        ),
        1
    );
    assert_eq!(
        running(
            |id, facts| match id {
                "a" => facts.status = Some(ThreadStatus::Interrupted),
                "b" => facts.status = Some(ThreadStatus::Error),
                _ => {}
            },
            "lead"
        ),
        0
    );
    assert_eq!(
        running(
            |id, facts| {
                if id == "lead" {
                    facts.status = Some(ThreadStatus::Running);
                }
            },
            "alone"
        ),
        0
    );
}

#[test]
fn finds_the_root_and_survives_a_loop() {
    let mut rows = crew(|_, _| {});
    assert_eq!(crews(&rows).root("b"), "lead");
    rows[0].parent = Some("b");
    let tree = crews(&rows);
    tree.root("b");
    assert!(tree.running("lead") == 0);
}

#[test]
fn says_how_many_subsessions_run() {
    assert_eq!(running_text(1), "1 subsession runs");
    assert_eq!(running_text(3), "3 subsessions run");
}
