//! A session's budget of watch notices: at most 10 until the person types a message.
#![cfg(unix)]

mod support;

use nexus_workbench::event::Sender;
use support::watch::*;
use support::*;

/// Waits until the delivery attempt of every command that already ended has finished. Nexus delivers notices one at a
/// time, in the order the commands ended (`ExitQueue`), so a marker in another session arrives only after them. Without
/// it, a person's reset could run before the attempt checks the budget, and an over-budget notice would go out.
async fn settled(wb: &nexus_workbench::Workbench, other: &str, name: &str) {
    marker(wb, other, name).await;
}

fn rows(wb: &nexus_workbench::Workbench, thread_id: &str) -> usize {
    budget_rows(&wb.events(thread_id, 0).expect("events"))
}

/// A session receives at most 10 watch notices. `queue` (PR watcher) and linked `send_from` do not reset the budget;
/// a person-typed message (`send_message`, `send_message_as`, `send_from(..., None)`) resets it. The first drop in each
/// budget period writes one row, later drops write none, and a private command's drop writes the same row.
#[tokio::test]
async fn budget_caps_at_ten_queue_does_not_reset_and_person_message_resets() {
    let s = setup_terminal("watch-budget");
    let thread = idle_session(&s).await;
    let other = idle_session(&s).await;
    let wb = &s.workbench;

    for n in 1..=10 {
        run_watched(wb, &thread.id, &format!("b-{n}"), &format!("printf 'run-{n}'")).await;
        wait_for(wb, &thread.id, |e| {
            watch_notices(e).len() == n && turn_ends(e).len() == n + 1
        })
        .await;
    }

    // The 11th watched exit hits the spent budget: one row says so.
    run_watched(wb, &thread.id, "b-11", "printf 'run-11'").await;
    let events = wait_for(wb, &thread.id, |e| budget_rows(e) == 1).await;
    assert_eq!(watch_notices(&events).len(), 10);

    // PR watcher's `queue` does NOT reset the budget. The 12th drop writes no row.
    wb.queue(&thread.id, "checks failed").await.expect("queue");
    wait_for(wb, &thread.id, |e| turn_ends(e).len() == 12).await;
    let twelfth = run_watched(wb, &thread.id, "b-12", "printf 'run-12'").await;
    wait_command_ended(wb, &thread.id, &twelfth).await;

    // `send_from` with a sender does NOT reset the budget.
    wb.send_from(
        &thread.id,
        "peer ping",
        vec![],
        Some(Sender::Session {
            thread_id: "t_other".into(),
            title: "Peer".into(),
        }),
    )
    .await
    .expect("send_from peer");
    wait_for(wb, &thread.id, |e| turn_ends(e).len() == 13).await;
    let thirteenth = run_watched(wb, &thread.id, "b-13", "printf 'run-13'").await;
    wait_command_ended(wb, &thread.id, &thirteenth).await;
    settled(wb, &other.id, "settled-13").await;
    assert_eq!(rows(wb, &thread.id), 1, "the 12th and 13th drops write no row");

    // Person-typed `send_message` resets the budget: the next notice is the 11th, and none of runs 11 to 13 sent one.
    wb.send_message(&thread.id, "keep going", vec![])
        .await
        .expect("person send_message");
    wait_for(wb, &thread.id, |e| turn_ends(e).len() == 14).await;
    // The marker's notice is the 11th, and the last (`marker` checks it).
    let notices = marker(wb, &thread.id, "run-14").await;
    assert_eq!(notices.len(), 11, "{notices:?}");

    // Spend the rest of the refreshed budget, then verify `send_message_as(..., queued: true)` and
    // `send_from(..., None)` also reset it. Each period's first drop writes a row again.
    let mut expected_notices = 11;
    let mut expected_turns = 15;
    let mut expected_rows = 1;
    wait_for(wb, &thread.id, |e| turn_ends(e).len() == expected_turns).await;
    for (reset_name, reset_kind) in [("send_message_as", 0u8), ("send_from_none", 1u8)] {
        let remaining = 10 - (expected_notices % 10);
        for k in 0..remaining {
            let id = format!("{reset_name}-fill-{k}");
            run_watched(wb, &thread.id, &id, &format!("printf '{id}'")).await;
            expected_notices += 1;
            expected_turns += 1;
            wait_for(wb, &thread.id, |e| {
                watch_notices(e).len() == expected_notices && turn_ends(e).len() == expected_turns
            })
            .await;
        }
        // The first drop of this period, private in the first round: the same row, with no private data.
        let gate = s.root.join(format!("{reset_name}-gate"));
        let blocked = run_watched(
            wb,
            &thread.id,
            &format!("{reset_name}-blocked"),
            &gated(&gate, "printf 'SECRET_BLOCKED'"),
        )
        .await;
        if reset_kind == 0 {
            wb.terminal(&thread.id, "private", serde_json::json!({"id": blocked}), true)
                .await
                .expect("mark private");
        }
        open(&gate);
        expected_rows += 1;
        wait_for(wb, &thread.id, |e| budget_rows(e) == expected_rows).await;

        if reset_kind == 0 {
            wb.send_message_as(&thread.id, "queued by person", vec![], "", true)
                .await
                .expect("send_message_as");
        } else {
            wb.send_from(&thread.id, "typed via send_from", vec![], None)
                .await
                .expect("send_from None");
        }
        expected_turns += 1;
        wait_for(wb, &thread.id, |e| turn_ends(e).len() == expected_turns).await;

        let after_id = format!("{reset_name}-after");
        let notices = marker(wb, &thread.id, &after_id).await;
        expected_notices += 1;
        expected_turns += 1;
        assert_eq!(notices.len(), expected_notices, "{notices:?}");
        wait_for(wb, &thread.id, |e| turn_ends(e).len() == expected_turns).await;
    }
    let events = wb.events(&thread.id, 0).expect("events");
    assert_eq!(budget_rows(&events), 3);
    assert!(!format!("{events:?}").contains("SECRET_BLOCKED"));

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A notice the agent could not take gives its slot back: ten more notices still arrive with no reset between.
#[tokio::test]
async fn a_refused_notice_gives_its_budget_slot_back() {
    let s = setup_terminal("watch-refund");
    let thread = idle_session(&s).await;
    let wb = &s.workbench;

    wb.restart(&thread.id).expect("restart");
    until_stopped(wb, &thread.id).await;
    let refused = run_watched(wb, &thread.id, "rf-0", "printf 'nobody-listens'").await;
    let events = wait_for(wb, &thread.id, |e| !undelivered(e).is_empty()).await;
    assert!(undelivered(&events)[0].contains(&refused));

    // `queue` starts the agent again and leaves the budget as it is.
    wb.queue(&thread.id, "wake up").await.expect("queue");
    wait_for(wb, &thread.id, |e| turn_ends(e).len() == 2).await;
    for n in 1..=10 {
        run_watched(wb, &thread.id, &format!("rf-{n}"), &format!("printf 'refund-{n}'")).await;
        wait_for(wb, &thread.id, |e| {
            watch_notices(e).len() == n && turn_ends(e).len() == n + 2
        })
        .await;
    }

    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}
