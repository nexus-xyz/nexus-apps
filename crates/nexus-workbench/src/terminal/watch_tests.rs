use super::*;

fn exit(name: &str, status: &str, exit_code: Option<u32>) -> WatchExit {
    WatchExit {
        scope: "s".into(),
        thread: "t".into(),
        id: "cmd-1".into(),
        status: status.into(),
        exit_code,
        name: notice_name(name),
    }
}

/// A notice that never matches the texts other notice readers look for ([`Workbench::steer_notice`]), in sentences
/// of 20 words or fewer. `crew_fold::PROCESS_ENDED` lists whole texts; these all carry an id, so none can equal one.
fn assert_readable_and_unmatched(text: &str) {
    assert!(!text.starts_with(crate::moving::MOVED), "{text}");
    assert!(!text.contains("compacted the conversation"), "{text}");
    for sentence in text.split(". ") {
        assert!(sentence.split_whitespace().count() <= 20, "{sentence}");
    }
}

#[test]
fn notice_name_keeps_one_clean_line_of_at_most_sixty_characters() {
    // A multi-line command: its first line.
    assert_eq!(notice_name("cargo build\ncargo test"), "cargo build");
    // ESC and CR become spaces, and the runs of whitespace one space.
    assert_eq!(notice_name("\u{1b}[31mred\r  build\t\u{7}done"), "[31mred build done");
    // Invisible and text-direction characters become spaces too.
    assert_eq!(notice_name("build\u{202E}x\u{200B}y"), "build x y");
    assert_eq!(notice_name("\u{FEFF}a\u{2066}b\u{2069}\u{200F}"), "a b");
    // 200 characters: the first 60, then "...".
    let long = "a".repeat(200);
    assert_eq!(notice_name(&long), format!("{}...", "a".repeat(60)));
    // Multi-byte characters are cut between characters, never inside one.
    let wide = "\u{1f600}".repeat(61);
    assert_eq!(notice_name(&wide), format!("{}...", "\u{1f600}".repeat(60)));
    // A double quote cannot close the quotes around the name.
    assert_eq!(notice_name("echo \"hi\""), "echo 'hi'");
    // Nothing left: no name.
    assert_eq!(notice_name(""), "");
    assert_eq!(notice_name(" \r\n second line"), "");
}

#[test]
fn the_notice_names_the_command_and_says_how_it_ended() {
    let cases = [
        (
            exit("cargo build", "completed", Some(0)),
            "The watched command \"cargo build\" (cmd-1) exited with code 0. Read its output with terminal_read.",
        ),
        (
            exit("release build", "failed", Some(101)),
            "The watched command \"release build\" (cmd-1) exited with code 101. Read its output with terminal_read.",
        ),
        // The person's Stop: killed by a signal, which the PTY reports as code 1.
        (
            exit("sleep 600", "interrupted", Some(1)),
            "The watched command \"sleep 600\" (cmd-1) was stopped. Read its output with terminal_read.",
        ),
        // Ctrl-C from `terminal_interrupt`: the command's own exit.
        (
            exit("sleep 600", "failed", Some(130)),
            "The watched command \"sleep 600\" (cmd-1) exited with code 130. Read its output with terminal_read.",
        ),
        (
            exit("cargo build", "failed", None),
            "The watched command \"cargo build\" (cmd-1) ended without an exit code. Read its output with terminal_read.",
        ),
        (
            exit("Private terminal", "failed", Some(2)),
            "The watched command \"Private terminal\" (cmd-1) exited with code 2. Read its output with terminal_read.",
        ),
        (
            exit("", "completed", Some(0)),
            "The watched command cmd-1 exited with code 0. Read its output with terminal_read.",
        ),
    ];
    for (exit, expected) in cases {
        let text = format_notice(&exit);
        assert_eq!(text, expected);
        assert_readable_and_unmatched(&text);
    }
}

#[test]
fn undelivered_row_states_the_facts_and_matches_no_other_notice_reader() {
    let text = undelivered_notice(&exit("cargo build", "failed", Some(2)));
    assert_eq!(
        text,
        "The watched command \"cargo build\" (cmd-1) exited with code 2. The agent was not told: the session was halted \
         or its agent was not running. Send a message to tell it."
    );
    let private = undelivered_notice(&exit("Private terminal", "interrupted", Some(1)));
    assert_eq!(
        private,
        "The watched command \"Private terminal\" (cmd-1) was stopped. The agent was not told: the session was halted \
         or its agent was not running. Send a message to tell it."
    );
    let no_code = undelivered_notice(&exit("cargo build", "failed", None));
    assert!(no_code.starts_with("The watched command \"cargo build\" (cmd-1) ended without an exit code. "));
    for text in [&text, &private, &no_code, &BUDGET_SPENT.to_string()] {
        assert_readable_and_unmatched(text);
    }
}

#[test]
fn the_notice_takes_the_name_the_agent_may_see() {
    let terminals = Terminals::default();
    let entry = staged(&terminals, "t");
    assert!(end(&entry));
    assert_eq!(entry.take_watch_exit().expect("due").name, "echo hi");

    let renamed = staged(&terminals, "t2");
    renamed.state().info.name = "nightly build".into();
    assert!(end(&renamed));
    assert_eq!(renamed.take_watch_exit().expect("due").name, "nightly build");

    let private = staged(&terminals, "t3");
    private.state().info.private = true;
    assert!(end(&private));
    assert_eq!(private.take_watch_exit().expect("due").name, "Private terminal");
}

#[test]
fn budget_caps_at_ten_refunds_matching_epoch_and_resets() {
    let mut budget = Budget::default();
    let mut last_epoch = 0;
    for _ in 0..WATCH_MAX_NOTICES {
        let Slot::Taken(epoch) = budget.try_take("t") else {
            panic!("within budget");
        };
        last_epoch = epoch;
    }
    // The period's first drop is noted once; later drops are not.
    assert_eq!(budget.try_take("t"), Slot::FirstDrop);
    assert_eq!(budget.try_take("t"), Slot::Dropped);
    budget.refund("t", last_epoch);
    assert_eq!(budget.try_take("t"), Slot::Taken(last_epoch));
    assert_eq!(budget.try_take("t"), Slot::Dropped);
    budget.reset("t");
    // Stale refund from the old epoch does nothing.
    budget.refund("t", last_epoch);
    for _ in 0..WATCH_MAX_NOTICES {
        assert!(matches!(budget.try_take("t"), Slot::Taken(_)));
    }
    // A new period notes its first drop again.
    assert_eq!(budget.try_take("t"), Slot::FirstDrop);
    assert_eq!(budget.try_take("t"), Slot::Dropped);
}

#[test]
fn the_budget_row_is_short_and_matches_no_other_notice_reader() {
    assert!(!BUDGET_SPENT.starts_with("Watched ") && !BUDGET_SPENT.starts_with("The watched "));
    assert!(!BUDGET_SPENT.starts_with(crate::moving::MOVED));
    assert!(!BUDGET_SPENT.contains("compacted the conversation"));
    for sentence in BUDGET_SPENT.split(". ") {
        assert!(sentence.split_whitespace().count() <= 20, "{sentence}");
    }
}

#[tokio::test]
async fn exits_are_delivered_one_at_a_time_in_the_order_they_ended() {
    let terminals = Terminals::default();
    let (sent, mut heard) = tokio::sync::mpsc::unbounded_channel();
    terminals.on_exit(Arc::new(move |entry: Arc<Execution>| {
        let _ = sent.send(entry.thread.clone());
    }));
    let runtime = tokio::runtime::Handle::current();
    let threads = ["t1", "t2", "t3", "t4", "t5"];
    for thread in threads {
        let entry = staged(&terminals, thread);
        let mut state = entry.state();
        assert!(state.watch.exit());
        entry.notify_exit(Some(&runtime));
    }
    for thread in threads {
        assert_eq!(heard.recv().await.as_deref(), Some(thread));
    }
}

/// Queues an exit for each thread, as an exit does, while holding its state lock.
fn exit_all(terminals: &Terminals, threads: &[&str], runtime: Option<&tokio::runtime::Handle>) {
    for thread in threads {
        let entry = staged(terminals, thread);
        let mut state = entry.state();
        assert!(state.watch.exit());
        entry.notify_exit(runtime);
    }
}

#[tokio::test]
async fn a_delivery_that_panics_is_skipped_and_the_rest_still_arrive_in_order() {
    let terminals = Terminals::default();
    let (sent, mut heard) = tokio::sync::mpsc::unbounded_channel();
    terminals.on_exit(Arc::new(move |entry: Arc<Execution>| {
        assert_ne!(entry.thread, "A", "delivery of A panics");
        let _ = sent.send(entry.thread.clone());
    }));
    let runtime = tokio::runtime::Handle::current();
    exit_all(&terminals, &["A", "B", "C"], Some(&runtime));
    assert_eq!(heard.recv().await.as_deref(), Some("B"));
    assert_eq!(heard.recv().await.as_deref(), Some("C"));
    // The turn is not poisoned: a later exit is delivered too.
    exit_all(&terminals, &["D"], Some(&runtime));
    assert_eq!(heard.recv().await.as_deref(), Some("D"));
}

#[test]
fn without_a_runtime_a_plain_thread_delivers_the_exit_outside_tokio() {
    let terminals = Terminals::default();
    let (sent, heard) = std::sync::mpsc::channel();
    terminals.on_exit(Arc::new(move |entry: Arc<Execution>| {
        let outside_tokio = tokio::runtime::Handle::try_current().is_err();
        let _ = sent.send((entry.thread.clone(), outside_tokio));
    }));
    exit_all(&terminals, &["t"], None);
    let heard = heard.recv_timeout(std::time::Duration::from_secs(10));
    assert_eq!(heard, Ok(("t".to_string(), true)));
}

#[test]
fn unsupported_error_names_the_steering_agents() {
    let error = unsupported_error();
    assert!(error.starts_with("terminal_watch_unsupported:"));
    assert!(error.contains("Claude Code and Codex"), "{error}");
    assert!(error.contains("terminal_read"), "{error}");
}

fn staged(terminals: &Terminals, thread: &str) -> Arc<Execution> {
    let entry = terminals
        .stage(
            "",
            thread,
            std::path::PathBuf::from("/unused"),
            &serde_json::json!({"command": "echo hi", "request_id": "r1"}),
        )
        .expect("stage");
    entry.state().watch.requested = true;
    entry
}

fn end(entry: &Execution) -> bool {
    let mut state = entry.state();
    state.info.status = "completed".into();
    state.info.exit_code = Some(0);
    state.watch.exit()
}

#[test]
fn take_watch_exit_suppresses_when_already_read_or_reader_waiting() {
    let terminals = Terminals::default();
    let entry = staged(&terminals, "t");
    entry.state().watch.saw_end();
    assert!(!end(&entry));
    assert_eq!(entry.take_watch_exit(), None);

    let entry = staged(&terminals, "t2");
    entry.state().watch.readers = 1;
    assert!(!end(&entry));
    assert_eq!(entry.take_watch_exit(), None);

    let entry = staged(&terminals, "t3");
    assert!(end(&entry));
    let exit = entry.take_watch_exit().expect("watch exit");
    assert_eq!(exit.exit_code, Some(0));
    assert_eq!(exit.status, "completed");
    // Second call returns None (at most one notice per command).
    assert_eq!(entry.take_watch_exit(), None);
}

#[tokio::test]
async fn a_dropped_reader_hands_back_a_notice_held_back_at_exit() {
    let terminals = Terminals::default();
    let (sent, mut heard) = tokio::sync::mpsc::unbounded_channel();
    terminals.on_exit(Arc::new(move |entry: Arc<Execution>| {
        let _ = sent.send(entry.take_watch_exit());
    }));
    let entry = staged(&terminals, "t");

    // A read waits, the command exits, then the read is cancelled before it saw the end.
    let mut reader = ReaderGuard::new(entry.clone());
    reader.arm(&mut entry.state());
    assert!(!end(&entry), "a waiting reader holds the notice back");
    drop(reader);
    let exit = heard.recv().await.expect("callback ran").expect("notice claimed");
    assert_eq!(exit.exit_code, Some(0));

    // A reader that saw the end hands back nothing.
    let entry = staged(&terminals, "t2");
    let mut reader = ReaderGuard::new(entry.clone());
    reader.arm(&mut entry.state());
    assert!(!end(&entry));
    entry.state().watch.saw_end();
    drop(reader);
    tokio::task::yield_now().await;
    assert!(heard.try_recv().is_err());

    // A reader dropped while the command still runs hands back nothing: the exit sends it.
    let entry = staged(&terminals, "t3");
    let mut reader = ReaderGuard::new(entry.clone());
    reader.arm(&mut entry.state());
    drop(reader);
    tokio::task::yield_now().await;
    assert!(heard.try_recv().is_err());
    assert!(end(&entry));
}

#[test]
fn closing_a_session_cancels_its_notices_and_forgets_its_budget() {
    let terminals = Terminals::default();
    let entry = staged(&terminals, "t");
    terminals.watch_budget(|budget| budget.try_take("t"));
    terminals.close_thread("t");
    assert!(!end(&entry), "a deleted session's command sends no notice");
    assert_eq!(entry.take_watch_exit(), None);
    assert!(!terminals.watch_budget(|budget| budget.tracks("t")));
}
