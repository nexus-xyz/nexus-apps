use std::cell::Cell;

use super::*;
use crate::Capture;
use crate::args::parse_argv;

type Read = std::result::Result<Value, CliError>;

fn shown(status: &str, turn: Option<&str>) -> Read {
    let turns: Vec<_> = turn.into_iter().map(|status| json!({"status": status})).collect();
    Ok(json!({"id": "t_abc", "status": status, "turns": turns}))
}

fn run(polls: impl IntoIterator<Item = Read>, timeout: u64) -> (Outcome, Duration) {
    let mut polls = polls.into_iter();
    let elapsed = Cell::new(Duration::ZERO);
    let outcome = wait(
        Duration::from_millis(timeout),
        Duration::from_millis(10),
        |_| polls.next().expect("unexpected poll"),
        |sleep| elapsed.set(elapsed.get() + sleep),
        || elapsed.get(),
    );
    (outcome, elapsed.get())
}

fn report(outcome: Outcome, code: i32, line: &str) {
    let capture = Capture::default();
    assert_eq!(
        outcome.report(&capture, &Style::new(false), "t_abc", DEFAULT_TIMEOUT),
        code
    );
    if code == 0 {
        assert_eq!(*capture.0.out.borrow(), [line]);
        assert!(capture.0.err.borrow().is_empty());
    } else {
        assert_eq!(*capture.0.err.borrow(), [line]);
        assert!(capture.0.out.borrow().is_empty());
    }
}

#[test]
fn finishes_when_the_latest_turn_completes() {
    let (outcome, elapsed) = run(
        [
            shown("running", Some("completed")),
            shown("running", None),
            shown("running", Some("running")),
            shown("idle", Some("completed")),
        ],
        100,
    );
    assert_eq!(elapsed, Duration::from_millis(30));
    report(outcome, 0, "session t_abc finished");
}

#[test]
fn reports_a_failure_or_stop() {
    for (poll, line) in [
        (shown("error", Some("completed")), "session t_abc failed"),
        (shown("idle", Some("error")), "session t_abc failed"),
        (shown("interrupted", None), "session t_abc was stopped"),
        (shown("idle", Some("interrupted")), "session t_abc was stopped"),
    ] {
        report(run([poll], 100).0, 1, line);
    }
}

#[test]
fn stops_after_three_failed_reads() {
    let (outcome, elapsed) = run(
        ["first", "second", "third"].map(|message| Err(CliError::new(message))),
        100,
    );
    assert_eq!(elapsed, Duration::from_millis(20));
    report(outcome, 2, "session t_abc could not be read: third");
}

fn app_gone() -> Read {
    Err(CliError {
        code: Some("desktop_unavailable"),
        ..CliError::new("gone")
    })
}

#[test]
fn an_app_restart_after_a_read_is_retried() {
    let (outcome, elapsed) = run(
        [
            shown("running", Some("running")),
            app_gone(),
            app_gone(),
            shown("idle", Some("completed")),
        ],
        100,
    );
    assert_eq!(elapsed, Duration::from_millis(30));
    report(outcome, 0, "session t_abc finished");
    let (outcome, _) = run([shown("running", None), app_gone(), app_gone(), app_gone()], 100);
    report(outcome, 2, "session t_abc could not be read: gone");
}

#[test]
fn a_missing_app_before_any_read_ends_the_wait() {
    let (outcome, elapsed) = run([app_gone()], 100);
    assert_eq!(elapsed, Duration::ZERO);
    report(outcome, 2, "session t_abc could not be read: gone");
}

#[test]
fn reports_approval_from_the_session_or_idle_turn() {
    for poll in [shown("approval", None), shown("idle", Some("approval"))] {
        report(run([poll], 100).0, 3, "session t_abc needs approval in the app");
    }
}

#[test]
fn times_out_using_the_clock_without_sleeping_past_the_limit() {
    let (outcome, elapsed) = run(
        [
            shown("idle", Some("running")),
            shown("running", None),
            shown("running", None),
        ],
        15,
    );
    assert_eq!(elapsed, Duration::from_millis(15));
    report(outcome, 124, "session t_abc did not finish in 1800 seconds");
}

#[test]
fn finishes_when_a_read_reaches_the_limit() {
    let elapsed = Cell::new(Duration::ZERO);
    let timeout = Duration::from_secs(1);
    let outcome = wait(
        timeout,
        POLL_INTERVAL,
        |final_read| {
            assert!(!final_read);
            elapsed.set(timeout);
            shown("idle", Some("completed"))
        },
        |_| panic!("slept after the timeout"),
        || elapsed.get(),
    );
    assert_eq!(outcome, Outcome::Finished);
}

#[test]
fn finishes_when_a_busy_session_goes_idle_without_turns() {
    for busy in ["starting", "running"] {
        assert_eq!(run([shown(busy, None), shown("idle", None)], 100).0, Outcome::Finished);
    }
}

#[test]
fn waits_when_a_session_has_never_been_busy_and_has_no_turns() {
    assert_eq!(
        run([shown("idle", None), shown("idle", None), shown("idle", None)], 20).0,
        Outcome::TimedOut
    );
}

#[test]
fn a_successful_read_clears_failed_reads() {
    let (outcome, elapsed) = run(
        [
            Err(CliError::new("Nexus for desktop did not answer in time.")),
            shown("running", None),
            Err(CliError::new("Nexus for desktop did not answer in time.")),
            Err(CliError::new("Nexus for desktop did not answer in time.")),
            shown("idle", Some("completed")),
        ],
        100,
    );
    assert_eq!(elapsed, Duration::from_millis(40));
    assert_eq!(outcome, Outcome::Finished);
}

#[test]
fn parses_timeout_seconds() {
    let parse = |flags: &[&str]| parse_argv(&flags.iter().map(|flag| flag.to_string()).collect::<Vec<_>>());
    assert_eq!(timeout_of(&parse(&[])).expect("default timeout"), DEFAULT_TIMEOUT);
    assert_eq!(
        timeout_of(&parse(&["--timeout", "23"])).expect("explicit timeout"),
        Duration::from_secs(23)
    );
    for flags in [
        ["--timeout", ""].as_slice(),
        &["--timeout"],
        &["--timeout", "0"],
        &["--timeout", "abc"],
    ] {
        assert!(timeout_of(&parse(flags)).is_err(), "{flags:?}");
    }
}

#[test]
fn last_read_checks_the_session_at_the_deadline() {
    for (status, turn, expected) in [
        ("idle", Some("completed"), Outcome::Finished),
        ("error", None, Outcome::Failed),
        ("approval", None, Outcome::Approval),
    ] {
        let elapsed = Cell::new(Duration::ZERO);
        let timeout = Duration::from_secs(60);
        let outcome = wait(
            timeout,
            POLL_INTERVAL,
            |final_read| {
                assert_eq!(final_read, elapsed.get() == timeout);
                if elapsed.get() >= Duration::from_secs(55) {
                    shown(status, turn)
                } else {
                    shown("running", None)
                }
            },
            |sleep| elapsed.set(elapsed.get() + sleep),
            || elapsed.get(),
        );
        assert_eq!(elapsed.get(), timeout);
        assert_eq!(outcome, expected);
    }
}

#[test]
fn a_failure_at_the_deadline_counts_toward_the_read_limit() {
    let failure = || Err(CliError::new("Nexus for desktop did not answer in time."));
    let (outcome, elapsed) = run([failure(), failure()], 10);
    assert_eq!((outcome, elapsed), (Outcome::TimedOut, Duration::from_millis(10)));

    let (outcome, elapsed) = run([failure(), failure(), failure()], 20);
    assert_eq!(elapsed, Duration::from_millis(20));
    report(
        outcome,
        2,
        "session t_abc could not be read: Nexus for desktop did not answer in time.",
    );
}

#[test]
fn reports_multiline_causes() {
    report(
        Outcome::Unreadable(CliError::new("Two sessions match.\nt_1, t_2")),
        2,
        "session t_abc could not be read: Two sessions match.\nt_1, t_2",
    );
}

#[test]
fn checks_again_when_a_failed_read_passes_the_deadline() {
    let timeout = Duration::from_secs(30);
    let elapsed = Cell::new(Duration::from_secs(20));
    let mut flags = Vec::new();
    let outcome = wait(
        timeout,
        POLL_INTERVAL,
        |final_read| {
            flags.push(final_read);
            if final_read {
                shown("idle", Some("completed"))
            } else {
                elapsed.set(timeout + Duration::from_secs(1));
                Err(CliError::new("Nexus for desktop did not answer in time."))
            }
        },
        |_| panic!("slept past the timeout"),
        || elapsed.get(),
    );
    assert_eq!(flags, [false, true]);
    report(outcome, 0, "session t_abc finished");
}

#[test]
fn counts_failed_reads_when_a_read_passes_the_deadline() {
    let timeout = Duration::from_secs(30);
    let error = CliError::new("Nexus for desktop did not answer in time.");
    for (start, expected) in [(20, Outcome::TimedOut), (10, Outcome::Unreadable(error.clone()))] {
        let elapsed = Cell::new(Duration::from_secs(start));
        let mut flags = Vec::new();
        let outcome = wait(
            timeout,
            POLL_INTERVAL,
            |final_read| {
                flags.push(final_read);
                if elapsed.get() == Duration::from_secs(20) {
                    elapsed.set(timeout + Duration::from_secs(1));
                }
                Err(error.clone())
            },
            |sleep| {
                assert!(elapsed.get() < timeout);
                elapsed.set(elapsed.get() + sleep);
            },
            || elapsed.get(),
        );
        assert_eq!(flags.len(), if start == 20 { 2 } else { 3 });
        assert_eq!(flags.iter().filter(|&&final_read| final_read).count(), 1);
        assert_eq!(flags.last(), Some(&true));
        assert_eq!(outcome, expected);
    }
}
