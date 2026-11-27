//! A halt or spend trip must interrupt a spontaneous turn before its first streamed message finishes.
mod support;

use nexus_workbench::event::{SessionAgent, TurnStatus, WbEvent};
use nexus_workbench::store::Store;
use nexus_workbench::{Limits, NewThread, StartChild};
use support::*;

#[derive(Clone, Copy, Debug)]
enum Case {
    Halt,
    Spend,
    Free,
}

async fn spontaneous(agent: SessionAgent, case: Case) {
    let s = setup(
        &format!("early-interrupt-{agent:?}-{case:?}"),
        Limits {
            crew_spend_usd: Some(0.03),
            ..bounded()
        },
    );
    let input = s.root.join("provider-input");
    s.host
        .extra
        .lock()
        .expect("fixture agent environment")
        .insert("FAKE_AGENT_INPUT_RECORD".into(), input.display().to_string());
    let lead = root_at_work(
        &s,
        NewThread {
            name: None,
            agent,
            ..new_root(&s.project, "relay Plan")
        },
    )
    .await;
    let worker = finished_child(&s, &lead.id, child(agent, "autoturn Survey")).await;
    match case {
        Case::Halt => s.workbench.interrupt(&worker.id).unwrap_or_else(|e| panic!("{e}")),
        Case::Spend => {
            finished_child(
                &s,
                &lead.id,
                StartChild {
                    model: Some(UNPRICED.into()),
                    ..child(agent, "Trip the cap")
                },
            )
            .await;
            until("the spend cap trips", || {
                s.workbench
                    .list_children(&lead.id)
                    .is_ok_and(|children| children.limits.spend_left_usd == Some(0.0))
            })
            .await;
        }
        Case::Free => {}
    }
    let native = Store::open(&s.root.join("workbench.sqlite3"))
        .unwrap_or_else(|e| panic!("{e}"))
        .thread(&worker.id)
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("no worker"))
        .native_id;
    let interrupted = s.host.memory.join(format!("interrupt-{native}"));
    let _ = std::fs::remove_file(&interrupted);
    trigger(&s, &format!("automatic-{native}"));
    // The fake cannot finish its streamed message until an interrupt or the explicit finish signal arrives.
    wait_for(&s.workbench, &worker.id, |events| {
        events
            .iter()
            .filter(|e| matches!(e.event, WbEvent::TurnStart { .. }))
            .count()
            == 2
    })
    .await;
    let expected = match case {
        Case::Free => {
            assert!(!interrupted.exists(), "a permitted continuation must run");
            trigger(&s, &format!("finish-automatic-{native}"));
            TurnStatus::Completed
        }
        Case::Halt | Case::Spend => {
            until("the fake receives its interrupt", || interrupted.exists()).await;
            TurnStatus::Interrupted
        }
    };
    let events = wait_for(&s.workbench, &worker.id, |e| turn_ends(e).len() == 2).await;
    assert_eq!(turn_ends(&events), [TurnStatus::Completed, expected]);
    assert!(s.workbench.is_live(&worker.id), "the interrupt keeps its process");
    if matches!(case, Case::Spend) {
        assert!(
            s.workbench.halt(&worker.id).unwrap_or_else(|e| panic!("{e}")).is_none(),
            "a spend trip creates no halt"
        );
    }
    let notices = events
        .iter()
        .filter(|row| matches!(&row.event, WbEvent::Notice { text } if text.contains("crew spend cap")))
        .count();
    assert_eq!(notices, usize::from(matches!(case, Case::Spend)));
    if matches!((agent, case), (SessionAgent::Codex, Case::Spend)) {
        s.workbench.restart(&worker.id).expect("restart before continuing");
    }
    s.workbench
        .send(&worker.id, "Continue the interrupted work")
        .await
        .expect("send");
    wait_for(&s.workbench, &worker.id, |events| turn_ends(events).len() == 3).await;
    let prompt = std::fs::read_to_string(&input).expect("provider input");
    assert!(prompt.contains("Continue the interrupted work"), "{prompt}");
    assert_eq!(
        prompt.contains("crew spend cap"),
        matches!(case, Case::Spend),
        "{prompt}"
    );
    if matches!(case, Case::Spend) {
        s.workbench.restart(&worker.id).expect("restart after receipt");
        s.workbench
            .send(&worker.id, "Another message")
            .await
            .expect("send again");
        wait_for(&s.workbench, &worker.id, |events| turn_ends(events).len() == 4).await;
        assert_eq!(std::fs::read_to_string(&input).expect("next input"), "Another message");
    }
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}

#[tokio::test]
async fn claude_halt_interrupts_before_the_message_finishes() {
    spontaneous(SessionAgent::Claude, Case::Halt).await;
}
#[tokio::test]
async fn codex_halt_interrupts_the_turn_it_started_itself() {
    spontaneous(SessionAgent::Codex, Case::Halt).await;
}
#[tokio::test]
async fn claude_spend_trip_interrupts_before_the_message_finishes() {
    spontaneous(SessionAgent::Claude, Case::Spend).await;
}
#[tokio::test]
async fn codex_spend_trip_interrupts_the_turn_it_started_itself() {
    spontaneous(SessionAgent::Codex, Case::Spend).await;
}
#[tokio::test]
async fn claude_permitted_continuations_complete() {
    spontaneous(SessionAgent::Claude, Case::Free).await;
}
#[tokio::test]
async fn codex_permitted_continuations_complete() {
    spontaneous(SessionAgent::Codex, Case::Free).await;
}
