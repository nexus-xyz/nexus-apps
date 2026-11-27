//! Each scenario runs through both fake agents.
mod support;
use nexus_workbench::event::{Sender, SessionAgent, TurnStatus, WbEvent};
use nexus_workbench::store::Store;
use nexus_workbench::{Limits, NewThread, OrchCode};
use support::*;

fn root_for(project: &str, agent: SessionAgent, text: &str) -> NewThread {
    NewThread {
        name: None,
        agent,
        ..new_root(project, text)
    }
}
fn halted(s: &Setup, id: &str) -> Option<String> {
    Store::open(&s.root.join("workbench.sqlite3"))
        .expect("test operation succeeds")
        .thread(id)
        .expect("test operation succeeds")
        .expect("test operation succeeds")
        .halt
}
async fn cleanup(s: Setup) {
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(s.root);
}
async fn usage(s: &Setup, root: &str) {
    let before = s
        .workbench
        .events(root, 0)
        .expect("test operation succeeds")
        .iter()
        .filter(|e| matches!(e.event, WbEvent::Usage { .. }))
        .count();
    trigger_with(s, "relay", "USAGE");
    wait_for(&s.workbench, root, |events| {
        events
            .iter()
            .filter(|e| matches!(e.event, WbEvent::Usage { .. }))
            .count()
            > before
    })
    .await;
}
async fn trip(s: &Setup, root: &str) {
    for _ in 0..20 {
        if s.workbench
            .list_children(root)
            .expect("test operation succeeds")
            .limits
            .spend_left_usd
            == Some(0.0)
        {
            return;
        }
        usage(s, root).await;
    }
    panic!("spend never tripped");
}

#[tokio::test]
async fn f3_stop_survives() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(&format!("f3-{agent:?}"), bounded());
        let lead = root(&s, root_for(&s.project, agent, "Lead")).await;
        let c = start(&s, &lead.id, agent, "Child", "slow Hold").await;
        wait_for(&s.workbench, &c.id, |events| {
            events.iter().any(|row| matches!(row.event, WbEvent::Read { .. }))
        })
        .await;
        s.workbench.stop(&c.id).expect("test operation succeeds");
        until_stopped(&s.workbench, &c.id).await;
        assert_eq!(halted(&s, &c.id).as_deref(), Some("person"));
        s.workbench.shutdown().await;
        let wb = reopen(&s, bounded());
        wb.restart(&c.id).expect("restart preserves halt");
        wb.send(&lead.id, "Lead again").await.expect("test operation succeeds");
        assert_eq!(
            wb.message_child(&lead.id, &c.id, "Try again")
                .await
                .expect_err("the halt refuses this call")
                .code,
            OrchCode::Halted
        );
        wb.send(&c.id, "A person resumes you")
            .await
            .expect("test operation succeeds");
        wait_for(&wb, &c.id, |e| turn_ends(e).last() == Some(&TurnStatus::Completed)).await;
        assert!(
            Store::open(&s.root.join("workbench.sqlite3"))
                .expect("test operation succeeds")
                .thread(&c.id)
                .expect("test operation succeeds")
                .expect("test operation succeeds")
                .halt
                .is_none()
        );
        wb.shutdown().await;
        cleanup(s).await;
    }
}

#[tokio::test]
async fn f4_stop_reaches_a_starting_child() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(&format!("f4-start-{agent:?}"), bounded());
        let lead = root(&s, root_for(&s.project, agent, "Lead")).await;
        let c = start(&s, &lead.id, agent, "Child", "Write nothing").await;
        s.workbench.stop(&lead.id).expect("test operation succeeds");
        assert_eq!(halted(&s, &c.id).as_deref(), Some("parent"));
        assert_eq!(
            turn_ends(&s.workbench.events(&c.id, 0).expect("test operation succeeds")),
            [TurnStatus::Interrupted]
        );
        s.workbench
            .send(&lead.id, "Continue")
            .await
            .expect("test operation succeeds");
        wait_for(&s.workbench, &lead.id, |e| turn_ends(e).len() >= 2).await;
        assert!(launches_of(&s, &c.id).is_empty());
        assert!(halted(&s, &c.id).is_none());
        cleanup(s).await;
    }
}

#[tokio::test]
async fn f4_stop_on_a_lead_that_is_not_running() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(&format!("f4-idle-{agent:?}"), bounded());
        let lead = root(&s, root_for(&s.project, agent, "Lead")).await;
        let c = start(&s, &lead.id, agent, "Child", "slow Hold").await;
        wait_for(&s.workbench, &c.id, |_| s.workbench.is_live(&c.id)).await;
        s.workbench.restart(&lead.id).expect("test operation succeeds");
        until_stopped(&s.workbench, &lead.id).await;
        s.workbench.stop(&lead.id).expect("test operation succeeds");
        until_stopped(&s.workbench, &c.id).await;
        assert_eq!(halted(&s, &c.id).as_deref(), Some("parent"));
        cleanup(s).await;
    }
}

#[tokio::test]
async fn u14_delete_ends_a_starting_child() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(&format!("u14-{agent:?}"), bounded());
        let lead = root(&s, root_for(&s.project, agent, "Lead")).await;
        let c = start(&s, &lead.id, agent, "Child", "Write nothing").await;
        s.workbench
            .delete_thread(&lead.id)
            .await
            .expect("test operation succeeds");
        let saved = thread(&s.workbench, &c.id);
        assert!(saved.parent_thread_id.is_none());
        assert!(halted(&s, &c.id).is_none());
        assert!(launches_of(&s, &c.id).is_empty());
        assert_eq!(
            turn_ends(&s.workbench.events(&c.id, 0).expect("test operation succeeds")),
            [TurnStatus::Interrupted]
        );
        cleanup(s).await;
    }
}

#[tokio::test]
async fn delete_frees_the_children() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(
            &format!("delete-tree-{agent:?}"),
            Limits {
                max_depth: Some(2),
                ..bounded()
            },
        );
        let lead = root(&s, root_for(&s.project, agent, "Lead")).await;
        let c = finished_child(&s, &lead.id, child(agent, "Child")).await;
        let g = start(&s, &c.id, agent, "Grandchild", "slow Hold").await;
        wait_for(&s.workbench, &g.id, |_| s.workbench.is_live(&g.id)).await;
        s.workbench
            .delete_thread(&lead.id)
            .await
            .expect("test operation succeeds");
        until_stopped(&s.workbench, &g.id).await;
        for id in [&c.id, &g.id] {
            let saved = thread(&s.workbench, id);
            assert!(saved.parent_thread_id.is_none());
            assert_eq!(saved.depth, 0);
            assert!(halted(&s, id).is_none());
            assert!(saved.worktree.is_some());
        }
        cleanup(s).await;
    }
}

#[tokio::test]
async fn u2_interrupted_child_runs_no_turn_of_its_own() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(&format!("u2-{agent:?}"), bounded());
        let lead = root(&s, root_for(&s.project, agent, "Lead")).await;
        s.workbench.restart(&lead.id).expect("test operation succeeds");
        until_stopped(&s.workbench, &lead.id).await;
        let c = finished_child(&s, &lead.id, child(agent, "bgagent Survey")).await;
        s.workbench.interrupt(&c.id).expect("test operation succeeds");
        trigger(&s, "bgagent-end");
        trigger(&s, "bgagent-turn");
        let events = wait_for(&s.workbench, &c.id, |e| turn_ends(e).len() >= 2).await;
        assert_eq!(turn_ends(&events)[1], TurnStatus::Interrupted);
        assert_eq!(halted(&s, &c.id).as_deref(), Some("person"));
        cleanup(s).await;
    }
}

#[tokio::test]
async fn u7_background_calls_follow_the_halt() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(&format!("u7-{agent:?}"), bounded());
        s.host
            .extra
            .lock()
            .expect("environment")
            .insert("FAKE_CODEX_CHILD_MS".into(), "30000".into());
        let lead = root(
            &s,
            root_for(
                &s.project,
                agent,
                if agent == SessionAgent::Claude {
                    "team"
                } else {
                    "sleeper"
                },
            ),
        )
        .await;
        s.workbench.interrupt(&lead.id).expect("test operation succeeds");
        assert_eq!(
            code(s.workbench.start_child(&lead.id, child(agent, "Child")).await),
            OrchCode::Halted
        );
        assert_eq!(
            s.workbench
                .wait_children(&lead.id, Default::default())
                .await
                .expect_err("the halt refuses this call")
                .code,
            OrchCode::Halted
        );
        s.workbench
            .send_from(
                &lead.id,
                "Forged resume",
                vec![],
                Some(Sender::Session {
                    thread_id: "t_other".into(),
                    title: "Other".into(),
                }),
            )
            .await
            .expect_err("the halt refuses this call");
        assert_eq!(halted(&s, &lead.id).as_deref(), Some("person"));
        cleanup(s).await;
    }
}

#[tokio::test]
async fn limits_spend_interrupts() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(
            &format!("spend-{agent:?}"),
            Limits {
                crew_spend_usd: Some(0.02),
                ..bounded()
            },
        );
        let lead = root_at_work(&s, root_for(&s.project, agent, "relay Lead")).await;
        let c = start(&s, &lead.id, agent, "Child", "slow Hold").await;
        wait_for(&s.workbench, &c.id, |_| s.workbench.is_live(&c.id)).await;
        trip(&s, &lead.id).await;
        wait_for(&s.workbench, &c.id, |e| turn_ends(e).contains(&TurnStatus::Interrupted)).await;
        assert!(
            turn_ends(&s.workbench.events(&lead.id, 0).expect("test operation succeeds")).is_empty(),
            "the root's person turn continues"
        );
        assert!(halted(&s, &c.id).is_none());
        assert_eq!(
            code(s.workbench.start_child(&lead.id, child(agent, "More")).await),
            OrchCode::Paused
        );
        s.workbench
            .send(&c.id, "A person continues")
            .await
            .expect("test operation succeeds");
        assert_eq!(
            s.workbench
                .list_children(&lead.id)
                .expect("test operation succeeds")
                .limits
                .spend_left_usd,
            Some(0.0)
        );
        cleanup(s).await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn u5_paused_tree_starts_no_child() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(
            &format!("u5-{agent:?}"),
            Limits {
                crew_spend_usd: Some(0.02),
                ..bounded()
            },
        );
        let lead = root_at_work(&s, root_for(&s.project, agent, "relay Lead")).await;
        hold_fetch(&s).await;
        let c = start(&s, &lead.id, agent, "Preparing", "Never launch").await;
        until("fetch entered", || s.root.join("fetch-entered").exists()).await;
        trip(&s, &lead.id).await;
        std::fs::write(s.root.join("fetch-release"), "").expect("release fetch");
        wait_for(&s.workbench, &c.id, |events| {
            turn_ends(events) == [TurnStatus::Interrupted]
        })
        .await;
        until("setup finishes", || thread(&s.workbench, &c.id).worktree.is_some()).await;
        assert!(launches_of(&s, &c.id).is_empty());
        assert_eq!(
            code(s.workbench.start_child(&lead.id, child(agent, "No start")).await),
            OrchCode::Paused
        );
        cleanup(s).await;
    }
}

#[tokio::test]
async fn cascade_halts_survive_crash() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(
            &format!("cascade-crash-{agent:?}"),
            Limits {
                max_depth: Some(2),
                ..bounded()
            },
        );
        let lead = root(&s, root_for(&s.project, agent, "Lead")).await;
        let c = finished_child(&s, &lead.id, child(agent, "Child")).await;
        let g = finished_child(&s, &c.id, child(agent, "Grandchild")).await;
        // Persist exactly the state between the atomic halt write and process signals, then recover it.
        Store::open(&s.root.join("workbench.sqlite3"))
            .expect("test operation succeeds")
            .halt_crew(&lead.id)
            .expect("test operation succeeds");
        s.workbench.shutdown().await;
        let wb = reopen(&s, bounded());
        let db = Store::open(&s.root.join("workbench.sqlite3")).expect("test operation succeeds");
        assert_eq!(
            db.thread(&lead.id)
                .expect("test operation succeeds")
                .expect("test operation succeeds")
                .halt
                .as_deref(),
            Some("person")
        );
        for id in [&c.id, &g.id] {
            assert_eq!(
                db.thread(id)
                    .expect("test operation succeeds")
                    .expect("test operation succeeds")
                    .halt
                    .as_deref(),
                Some("parent")
            );
        }
        wb.send(&lead.id, "Resume").await.expect("test operation succeeds");
        for id in [&c.id, &g.id] {
            assert!(
                db.thread(id)
                    .expect("test operation succeeds")
                    .expect("test operation succeeds")
                    .halt
                    .is_none()
            );
        }
        wb.shutdown().await;
        cleanup(s).await;
    }
}

#[cfg(unix)]
async fn hold_fetch(s: &Setup) {
    let repo = s.root.join("repo");
    let remote = s.root.join("remote");
    git(
        &s.root,
        &[
            "clone",
            "--quiet",
            "--bare",
            &repo.to_string_lossy(),
            &remote.to_string_lossy(),
        ],
    );
    git(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
    git(&repo, &["fetch", "--quiet", "origin"]);
    git(&repo, &["remote", "set-head", "origin", "main"]);
    let script = s.root.join("hold-fetch.sh");
    std::fs::write(
        &script,
        format!(
            "touch '{}'; while [ ! -f '{}' ]; do sleep 0.02; done; exec git-upload-pack \"$@\"\n",
            s.root.join("fetch-entered").display(),
            s.root.join("fetch-release").display()
        ),
    )
    .expect("write fetch barrier");
    git(
        &repo,
        &[
            "config",
            "remote.origin.uploadpack",
            &format!("sh '{}'", script.display()),
        ],
    );
}

#[cfg(unix)]
#[tokio::test]
async fn org_switch_refuses_the_child() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(&format!("org-ready-{agent:?}"), bounded());
        let lead = root(&s, root_for(&s.project, agent, "Lead")).await;
        hold_fetch(&s).await;
        let c = start(&s, &lead.id, agent, "Child", "Never run").await;
        until("fetch entered", || s.root.join("fetch-entered").exists()).await;
        s.workbench.set_scope("gw|other");
        std::fs::write(s.root.join("fetch-release"), "").expect("release fetch");
        wait_for(&s.workbench, &c.id, |e| turn_ends(e) == [TurnStatus::Interrupted]).await;
        assert!(launches_of(&s, &c.id).is_empty());
        cleanup(s).await;
    }
}

#[tokio::test]
async fn completion_at_spend_trip_keeps_result_pending() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        for finishes_first in [true, false] {
            let s = setup(
                &format!("completion-trip-{agent:?}-{finishes_first}"),
                Limits {
                    crew_spend_usd: Some(0.02),
                    ..bounded()
                },
            );
            let lead = root_at_work(&s, root_for(&s.project, agent, "relay Lead")).await;
            let c = start(
                &s,
                &lead.id,
                agent,
                "Child",
                if finishes_first { "Complete" } else { "slow Hold" },
            )
            .await;
            if finishes_first {
                wait_for(&s.workbench, &c.id, |e| turn_ends(e) == [TurnStatus::Completed]).await;
            } else {
                wait_for(&s.workbench, &c.id, |_| s.workbench.is_live(&c.id)).await;
            }
            trip(&s, &lead.id).await;
            wait_for(&s.workbench, &c.id, |e| !turn_ends(e).is_empty()).await;
            end_relay(&s, &lead.id).await;
            assert!(has_result(&s.workbench, &lead.id, &c.id));
            let db = Store::open(&s.root.join("workbench.sqlite3")).expect("store");
            assert_eq!(db.thread(&c.id).expect("row").expect("child").delivered_seq, 0);
            assert_eq!(turn_ends(&s.workbench.events(&lead.id, 0).expect("log")).len(), 1);
            s.workbench
                .send(&lead.id, "A person receives the result")
                .await
                .expect("resume");
            until("result confirmed", || !has_result(&s.workbench, &lead.id, &c.id)).await;
            cleanup(s).await;
        }
    }
}

#[tokio::test]
async fn unpriced_call_trips_at_top_price() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(
            &format!("unpriced-trip-{agent:?}"),
            Limits {
                crew_spend_usd: Some(0.04),
                ..bounded()
            },
        );
        let lead = root_at_work(&s, root_for(&s.project, agent, "relay Lead")).await;
        let c = s
            .workbench
            .start_child(
                &lead.id,
                nexus_workbench::StartChild {
                    model: Some(UNPRICED.into()),
                    ..child(agent, "Complete")
                },
            )
            .await
            .expect("start unpriced child");
        wait_for(&s.workbench, &c.id, |events| {
            events
                .iter()
                .any(|e| matches!(&e.event, WbEvent::Usage { model, .. } if model == UNPRICED))
        })
        .await;
        assert_eq!(
            s.workbench
                .list_children(&lead.id)
                .expect("children")
                .limits
                .spend_left_usd,
            Some(0.0)
        );
        cleanup(s).await;
    }
}

#[tokio::test]
async fn spend_trip_stops_background_agents() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(
            &format!("background-cap-{agent:?}"),
            Limits {
                crew_spend_usd: Some(0.02),
                ..bounded()
            },
        );
        s.host
            .extra
            .lock()
            .expect("environment")
            .insert("FAKE_CODEX_CHILD_MS".into(), "30000".into());
        let lead = root_at_work(&s, root_for(&s.project, agent, "relay Lead")).await;
        let c = start(
            &s,
            &lead.id,
            agent,
            "Background",
            if agent == SessionAgent::Claude {
                "team"
            } else {
                "sleeper"
            },
        )
        .await;
        wait_for(&s.workbench, &c.id, |e| e.iter().any(|e| matches!(&e.event, WbEvent::Task {task_type: Some(kind), status: nexus_workbench::event::TaskStatus::Running, ..} if kind == "local_agent"))).await;
        trip(&s, &lead.id).await;
        wait_for(&s.workbench, &c.id, |events| {
            let mut tasks = std::collections::HashMap::new();
            for e in events {
                if let WbEvent::Task {
                    task_id,
                    task_type,
                    status,
                    ..
                } = &e.event
                    && (task_type.as_deref() == Some("local_agent") || tasks.contains_key(task_id))
                {
                    tasks.insert(task_id.clone(), *status);
                }
            }
            !tasks.is_empty()
                && tasks
                    .values()
                    .all(|status| *status != nexus_workbench::event::TaskStatus::Running)
        })
        .await;
        let after = s.workbench.events(&c.id, 0).expect("log").last().expect("event").seq;
        s.workbench
            .send(
                &c.id,
                if agent == SessionAgent::Claude {
                    "bgagent"
                } else {
                    "sleeper"
                },
            )
            .await
            .expect("person turn after cap");
        wait_for(&s.workbench, &c.id, |events| {
            events.iter().any(|row| {
                row.seq > after
                    && matches!(
                        &row.event,
                        WbEvent::Task {
                            status: nexus_workbench::event::TaskStatus::Stopped,
                            ..
                        }
                    )
            })
        })
        .await;
        assert!(s.workbench.is_live(&lead.id));
        assert!(turn_ends(&s.workbench.events(&lead.id, 0).expect("log")).is_empty());
        cleanup(s).await;
    }
}

#[tokio::test]
async fn f6_background_holds_the_codex_result() {
    let s = setup("f6-codex", bounded());
    s.host
        .extra
        .lock()
        .expect("environment")
        .insert("FAKE_CODEX_CHILD_MS".into(), "30000".into());
    let lead = root_at_work(&s, root_for(&s.project, SessionAgent::Codex, "relay Lead")).await;
    let c = finished_child(&s, &lead.id, child(SessionAgent::Codex, "sleeper")).await;
    assert!(!has_result(&s.workbench, &lead.id, &c.id));
    let task = s
        .workbench
        .events(&c.id, 0)
        .expect("log")
        .into_iter()
        .find_map(|row| match row.event {
            WbEvent::Task {
                task_id,
                task_type: Some(kind),
                status: nexus_workbench::event::TaskStatus::Running,
                ..
            } if kind == "local_agent" => Some(task_id),
            _ => None,
        })
        .expect("sub-agent");
    s.workbench.task_stop(&c.id, &task).await.expect("stop sub-agent");
    until("background result", || has_result(&s.workbench, &lead.id, &c.id)).await;
    cleanup(s).await;
}

#[tokio::test]
async fn root_person_spend_counts() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(
            &format!("root-spend-{agent:?}"),
            Limits {
                crew_spend_usd: Some(0.02),
                ..bounded()
            },
        );
        let lead = root_at_work(&s, root_for(&s.project, agent, "relay Lead")).await;
        // A root counts once it has a child: its child's own calls are free here, so the root's turn trips it.
        *s.host.call_price.lock().expect("fixture call price") = Some(0.0);
        finished_child(&s, &lead.id, child(agent, "Build it")).await;
        *s.host.call_price.lock().expect("fixture call price") = None;
        trip(&s, &lead.id).await;
        assert!(s.workbench.is_live(&lead.id));
        assert!(turn_ends(&s.workbench.events(&lead.id, 0).expect("log")).is_empty());
        end_relay(&s, &lead.id).await;
        s.workbench
            .send(&lead.id, "A new person turn")
            .await
            .expect("resume root");
        until("new spend budget", || {
            s.workbench
                .list_children(&lead.id)
                .expect("limits")
                .limits
                .spend_left_usd
                .is_some_and(|left| left > 0.0)
        })
        .await;
        cleanup(s).await;
    }
}

/// New sessions get subsessions by default: one that starts none is a session on its own, and a cap on a tree's spend
/// never pauses it or stops its background sub-agents.
#[tokio::test]
async fn a_root_with_no_child_spends_outside_the_cap() {
    let s = setup(
        "root-alone-spend",
        Limits {
            crew_spend_usd: Some(0.02),
            ..bounded()
        },
    );
    let lead = root_at_work(&s, root_for(&s.project, SessionAgent::Claude, "relay Lead")).await;
    assert!(thread(&s.workbench, &lead.id).orchestration);
    for _ in 0..20 {
        usage(&s, &lead.id).await;
    }
    let summary = thread(&s.workbench, &lead.id);
    assert!(!summary.crew_tripped && !summary.crew_paused, "{summary:?}");
    assert_eq!(
        s.workbench
            .list_children(&lead.id)
            .expect("limits")
            .limits
            .spend_left_usd,
        Some(0.02),
        "nothing counted"
    );
    cleanup(s).await;
}

#[tokio::test]
async fn cascade_halt_write_is_atomic_on_failure() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(&format!("atomic-halts-{agent:?}"), bounded());
        let lead = root(&s, root_for(&s.project, agent, "Lead")).await;
        let c = finished_child(&s, &lead.id, child(agent, "Child")).await;
        let db = rusqlite::Connection::open(s.root.join("workbench.sqlite3")).expect("connection");
        db.execute_batch(&format!(
            "create trigger reject_halt before update of halt on threads
            when new.id = '{}' begin select raise(abort, 'injected write failure'); end;",
            c.id
        ))
        .expect("trigger");
        assert!(s.workbench.stop(&lead.id).is_err());
        assert!(halted(&s, &lead.id).is_none());
        assert!(halted(&s, &c.id).is_none());
        db.execute_batch("drop trigger reject_halt").expect("remove trigger");
        cleanup(s).await;
    }
}

/// A bounded sequence checks stored safety after each action, like the model's simulated steps.
#[tokio::test]
async fn sim_safety() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(&format!("sim-safety-{agent:?}"), bounded());
        let lead = root(&s, root_for(&s.project, agent, "Lead")).await;
        let c = finished_child(&s, &lead.id, child(agent, "Child")).await;
        for step in 0..12 {
            if step % 3 == 0 {
                s.workbench.stop(&c.id).expect("halt");
                until_stopped(&s.workbench, &c.id).await;
            } else if step % 3 == 1 {
                assert_eq!(
                    s.workbench
                        .message_child(&lead.id, &c.id, "Automatic")
                        .await
                        .expect_err("halt holds")
                        .code,
                    OrchCode::Halted
                );
            } else {
                let before = turn_ends(&s.workbench.events(&c.id, 0).expect("log")).len();
                s.workbench.send(&c.id, "Person").await.expect("person turn");
                wait_for(&s.workbench, &c.id, |e| turn_ends(e).len() > before).await;
            }
            let saved = Store::open(&s.root.join("workbench.sqlite3"))
                .expect("store")
                .thread(&c.id)
                .expect("read")
                .expect("row");
            assert_eq!(saved.summary.parent_thread_id.as_deref(), Some(lead.id.as_str()));
            assert!(saved.summary.depth <= bounded().depth());
            if saved.halt.is_some() {
                assert!(!s.workbench.is_live(&c.id));
            }
            let last = turn_end_seqs(&s.workbench.events(&c.id, 0).expect("log"))
                .last()
                .copied()
                .expect("end");
            assert!(saved.delivered_seq <= last);
        }
        cleanup(s).await;
    }
}

#[tokio::test]
async fn interrupt_idle_survives() {
    for agent in [SessionAgent::Claude, SessionAgent::Codex] {
        let s = setup(&format!("interrupt-idle-{agent:?}"), bounded());
        let lead = root(&s, root_for(&s.project, agent, "Lead")).await;
        let c = finished_child(&s, &lead.id, child(agent, "Child")).await;
        s.workbench.interrupt(&c.id).expect("halt idle child");
        s.workbench.shutdown().await;
        let wb = reopen(&s, bounded());
        wb.send(&lead.id, "Person at root").await.expect("resume root");
        assert_eq!(
            wb.message_child(&lead.id, &c.id, "Automatic")
                .await
                .expect_err("halt survives")
                .code,
            OrchCode::Halted
        );
        wb.send(&c.id, "Person at child").await.expect("clear halt");
        wb.shutdown().await;
        cleanup(s).await;
    }
}
