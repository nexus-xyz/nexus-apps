//! Scheduled sessions through the public API, with the stand-in agents: a due run starts a session the
//! way a person's does (the host's launch, a worktree, the schedule's route), once; the webhook listens on the loopback
//! and starts nothing without its schedule's token; the daily cap holds across every trigger; and an agent schedules
//! only within its own session's limits.

mod support;

use std::time::Duration;

use nexus_workbench::event::{PermissionMode, SessionAgent, Via, WbEvent};
use nexus_workbench::schedules::agents::AgentSchedule;
use nexus_workbench::schedules::{Clock, RunStatus, ScheduleInput, ScheduleView, Trigger, When};
use nexus_workbench::{Limits, OrchCode};
use serde_json::Value;
use support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const HOUR: i64 = 3_600_000;

/// 23:59:59.9 today, local time, in epoch milliseconds: the last tenth of a second before the daily cap resets.
fn just_before_midnight() -> i64 {
    chrono::Local::now()
        .date_naive()
        .and_hms_milli_opt(23, 59, 59, 900)
        .and_then(|at| at.and_local_timezone(chrono::Local).earliest())
        .map(|at| at.timestamp_millis())
        .unwrap_or_else(|| panic!("no 23:59:59.9 today"))
}

/// A workbench whose schedules read `clock`, not the computer's: a test's steps then fall on the day it says, even
/// when the computer's clock passes midnight between them. `trigger_port`: a free port for webhooks.
fn setup_at(name: &str, clock: &Clock, trigger_port: bool) -> Setup {
    setup_with(name, bounded(), true, |config| nexus_workbench::Config {
        trigger_port: trigger_port.then_some(0),
        schedule_clock: clock.clone(),
        ..config
    })
}

fn input(project: &str, when: When) -> ScheduleInput {
    ScheduleInput {
        project_id: project.into(),
        title: Some("Nightly triage".into()),
        prompt: "Triage the new issues".into(),
        agent: SessionAgent::Claude,
        model: None,
        permission_mode: PermissionMode::AcceptEdits,
        effort: None,
        via: Via::Subscription,
        worktree: true,
        when,
        enabled: true,
        max_runs_per_day: None,
        catch_up: true,
    }
}

fn saved(s: &Setup, id: &str) -> ScheduleView {
    s.workbench
        .schedules()
        .unwrap_or_default()
        .into_iter()
        .find(|view| view.schedule.id == id)
        .unwrap_or_else(|| panic!("no schedule {id}"))
}

fn notices(s: &Setup, id: &str) -> Vec<String> {
    s.workbench
        .events(id, 0)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| match e.event {
            WbEvent::Notice { text } => Some(text),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_due_run_starts_a_session_the_way_a_person_does_and_only_once() {
    let s = setup("schedule-due", bounded());
    let view = s
        .workbench
        .save_schedule(None, input(&s.project, When::Interval { minutes: 60 }), None)
        .unwrap_or_else(|e| panic!("{e}"));
    let id = view.schedule.id.clone();
    let due = view.schedule.next_run_at.unwrap_or_else(|| panic!("no next run"));
    // Not yet: nothing starts.
    s.workbench.run_schedules(due - 1).await;
    assert!(s.workbench.threads().unwrap_or_default().is_empty());

    s.workbench.run_schedules(due + 1_000).await;
    let after = saved(&s, &id);
    let run = after.schedule.runs.first().cloned().unwrap_or_else(|| panic!("no run"));
    assert_eq!(
        (run.trigger, run.status),
        (Trigger::Time, RunStatus::Started),
        "{run:?}"
    );
    let thread_id = run.thread_id.unwrap_or_default();
    let events = wait_for(&s.workbench, &thread_id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(users(&events)[0].0, "Triage the new issues");
    // The host's own launch, with the schedule's route, in a worktree of its own.
    let launches = launches_of(&s, &thread_id);
    assert_eq!(launches.len(), 1);
    assert_eq!(
        (launches[0].agent, launches[0].via),
        (SessionAgent::Claude, Via::Subscription)
    );
    assert!(thread(&s.workbench, &thread_id).worktree.is_some());
    assert!(
        notices(&s, &thread_id)
            .iter()
            .any(|text| text.contains("“Nightly triage”, at its time"))
    );
    assert_eq!(after.schedule.next_run_at, Some(due + HOUR));
    assert_eq!(after.schedule.runs_on_day, 1);

    // The same minute's next tick, and two at once, start nothing more.
    let (_, _) = tokio::join!(
        s.workbench.run_schedules(due + 2_000),
        s.workbench.run_schedules(due + 3_000)
    );
    assert_eq!(s.workbench.threads().unwrap_or_default().len(), 1);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_run_nexus_missed_runs_once_or_is_skipped_as_the_schedule_says() {
    let s = setup("schedule-missed", bounded());
    let mut skip = input(&s.project, When::Interval { minutes: 60 });
    skip.catch_up = false;
    let skipping = s
        .workbench
        .save_schedule(None, skip, None)
        .unwrap_or_else(|e| panic!("{e}"));
    let catching = s
        .workbench
        .save_schedule(None, input(&s.project, When::Interval { minutes: 60 }), None)
        .unwrap_or_else(|e| panic!("{e}"));
    // The app comes back five hours after the first run's time.
    let back = catching.schedule.next_run_at.unwrap_or_default() + 5 * HOUR;
    s.workbench.run_schedules(back).await;
    let caught = saved(&s, &catching.schedule.id).schedule;
    assert_eq!(caught.runs.len(), 1);
    assert_eq!(caught.runs[0].status, RunStatus::Started);
    let skipped = saved(&s, &skipping.schedule.id).schedule;
    assert_eq!(skipped.runs.len(), 1);
    assert_eq!(skipped.runs[0].status, RunStatus::Skipped);
    assert!(
        skipped.runs[0]
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("not running")
    );
    // One session, not five.
    assert_eq!(s.workbench.threads().unwrap_or_default().len(), 1);
    assert!(caught.next_run_at.unwrap_or_default() > back);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A run gets subsessions from the computer's switch, on by default, never from its project's saved flag.
#[tokio::test]
async fn a_run_gets_subsessions_from_the_computers_switch_not_its_project() {
    let s = setup("schedule-subsessions", bounded());
    let id = s
        .workbench
        .save_schedule(None, input(&s.project, When::Manual), None)
        .unwrap_or_else(|e| panic!("{e}"))
        .schedule
        .id;
    let run = |s: &Setup| {
        let (workbench, id) = (s.workbench.clone(), id.clone());
        async move {
            let started = workbench.run_schedule_now(&id).await.unwrap_or_else(|e| panic!("{e}"));
            wait_for(&workbench, &started.id, |e| turn_ends(e).len() == 1).await;
            started.orchestration
        }
    };
    assert!(
        run(&s).await,
        "the project's flag is off, and the computer's switch is on"
    );

    s.workbench
        .set_project_orchestration(&s.project, true)
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench.set_subsessions(nexus_workbench::Subsessions {
        on: false,
        limits: bounded(),
        ..Default::default()
    });
    assert!(
        !run(&s).await,
        "the computer's switch is off, and the project's flag is not read"
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn run_now_counts_against_the_daily_cap() {
    let s = setup_at("schedule-cap", &Clock::fixed(just_before_midnight()), false);
    let mut capped = input(&s.project, When::Manual);
    capped.max_runs_per_day = Some(1);
    let id = s
        .workbench
        .save_schedule(None, capped, None)
        .unwrap_or_else(|e| panic!("{e}"))
        .schedule
        .id;
    let first = s
        .workbench
        .run_schedule_now(&id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &first.id, |e| turn_ends(e).len() == 1).await;
    let refused = s.workbench.run_schedule_now(&id).await.err().unwrap_or_default();
    assert!(refused.contains("daily limit"), "{refused}");
    assert_eq!(s.workbench.threads().unwrap_or_default().len(), 1);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// The cap counts a local calendar day by the schedules' clock: a run at 23:59:59.9 and Run now a moment later fall on
/// one day, and the first run after midnight is the next day's.
#[tokio::test]
async fn the_daily_cap_holds_to_the_last_moment_of_the_day() {
    let edge = just_before_midnight();
    let clock = Clock::fixed(edge);
    let s = setup_at("schedule-midnight", &clock, false);
    let mut capped = input(&s.project, When::Manual);
    capped.max_runs_per_day = Some(1);
    let id = s
        .workbench
        .save_schedule(None, capped, None)
        .unwrap_or_else(|e| panic!("{e}"))
        .schedule
        .id;
    let first = s
        .workbench
        .run_schedule_now(&id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &first.id, |e| turn_ends(e).len() == 1).await;
    // 23:59:59.999, however long the first run took on the computer's clock: still the same day, so refused.
    clock.set(edge + 99);
    let refused = s.workbench.run_schedule_now(&id).await.err().unwrap_or_default();
    assert!(refused.contains("daily limit"), "{refused}");
    // Midnight: a new day, a new cap. This is the step CI's computer clock took between the two checks.
    clock.set(edge + 100);
    let next_day = s
        .workbench
        .run_schedule_now(&id)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    wait_for(&s.workbench, &next_day.id, |e| turn_ends(e).len() == 1).await;
    assert_eq!(s.workbench.threads().unwrap_or_default().len(), 2);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

struct Answer {
    status: u16,
    body: Value,
}

async fn post(port: u16, path: &str, head: &[String], body: &str) -> Answer {
    let mut text = format!("POST {path} HTTP/1.1\r\n");
    for line in head {
        text.push_str(line);
        text.push_str("\r\n");
    }
    text.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ));
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    stream
        .write_all(text.as_bytes())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut got))
        .await
        .unwrap_or_else(|_| panic!("no answer"))
        .unwrap_or_else(|e| panic!("{e}"));
    let got = String::from_utf8_lossy(&got).into_owned();
    let (head, body) = got.split_once("\r\n\r\n").unwrap_or((&got, ""));
    Answer {
        status: head.split(' ').nth(1).and_then(|code| code.parse().ok()).unwrap_or(0),
        body: serde_json::from_str(body).unwrap_or(Value::Null),
    }
}

#[tokio::test]
async fn the_webhook_starts_a_run_only_with_its_schedules_token() {
    let s = setup_at("schedule-webhook", &Clock::fixed(just_before_midnight()), true);
    let mut manual = input(&s.project, When::Manual);
    manual.max_runs_per_day = Some(1);
    let id = s
        .workbench
        .save_schedule(None, manual, None)
        .unwrap_or_else(|e| panic!("{e}"))
        .schedule
        .id;
    let other = s
        .workbench
        .save_schedule(None, input(&s.project, When::Manual), None)
        .unwrap_or_else(|e| panic!("{e}"))
        .schedule
        .id;
    let secret = s
        .workbench
        .set_schedule_webhook(&id, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("no secret"));
    let theirs = s
        .workbench
        .set_schedule_webhook(&other, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("no secret"));
    let port: u16 = secret
        .url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|rest| rest.split('/').next())
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("{}", secret.url));
    let path = format!("/v1/schedules/{id}/run");
    assert_eq!(secret.url, format!("http://127.0.0.1:{port}{path}"));
    assert_eq!(saved(&s, &id).webhook_url.as_deref(), Some(secret.url.as_str()));
    let host = format!("Host: 127.0.0.1:{port}");
    let bearer = |token: &str| format!("Authorization: Bearer {token}");

    // No token, a wrong one, another schedule's, an unknown schedule: one answer, and nothing starts.
    for (path, head) in [
        (path.clone(), vec![host.clone()]),
        (path.clone(), vec![host.clone(), bearer("wrong")]),
        (path.clone(), vec![host.clone(), bearer(&theirs.token)]),
        (
            "/v1/schedules/s_nope/run".to_string(),
            vec![host.clone(), bearer(&secret.token)],
        ),
    ] {
        let answer = post(port, &path, &head, "").await;
        assert_eq!(
            (answer.status, answer.body["error"].as_str()),
            (401, Some("unauthorized")),
            "{head:?}"
        );
    }
    // A web page: another Host (DNS rebinding) or any Origin.
    let rebound = post(port, &path, &["Host: evil.test".into(), bearer(&secret.token)], "").await;
    assert_eq!(rebound.status, 403);
    let page = post(
        port,
        &path,
        &[host.clone(), bearer(&secret.token), "Origin: https://evil.test".into()],
        "",
    )
    .await;
    assert_eq!(page.status, 403);
    let huge = post(
        port,
        &path,
        &[host.clone(), bearer(&secret.token)],
        &"x".repeat(17 * 1024),
    )
    .await;
    assert_eq!(huge.status, 413);
    assert!(s.workbench.threads().unwrap_or_default().is_empty());

    // The right token starts a run, with the body framed as data.
    let started = post(
        port,
        &path,
        &[host.clone(), bearer(&secret.token)],
        "{\"ref\": \"main\"}",
    )
    .await;
    assert_eq!(started.status, 202, "{}", started.body);
    let thread_id = started.body["sessionId"].as_str().unwrap_or_default().to_string();
    // The stand-in reads "tool" in the framing as a cue to ask for a tool: the first message is what this checks.
    let events = wait_for(&s.workbench, &thread_id, |e| !users(e).is_empty()).await;
    let first = &users(&events)[0].0;
    assert!(
        first.contains("<webhook-payload>\n{\"ref\": \"main\"}\n</webhook-payload>"),
        "{first}"
    );
    // Past the daily cap: refused, and in the schedule's runs.
    let capped = post(port, &path, &[host.clone(), bearer(&secret.token)], "").await;
    assert_eq!(
        (capped.status, capped.body["error"].as_str()),
        (429, Some("daily_limit"))
    );
    let runs = saved(&s, &id).schedule.runs;
    assert_eq!(
        runs.iter().map(|run| (run.trigger, run.status)).collect::<Vec<_>>(),
        [
            (Trigger::Webhook, RunStatus::Skipped),
            (Trigger::Webhook, RunStatus::Started)
        ]
    );

    // A new token replaces the old; off, no token works.
    let rotated = s
        .workbench
        .set_schedule_webhook(&other, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"))
        .unwrap_or_else(|| panic!("no secret"));
    let other_path = format!("/v1/schedules/{other}/run");
    let old = post(port, &other_path, &[host.clone(), bearer(&theirs.token)], "").await;
    assert_eq!(old.status, 401);
    assert_eq!(s.workbench.set_schedule_webhook(&other, false).await, Ok(None));
    let off = post(port, &other_path, &[host.clone(), bearer(&rotated.token)], "").await;
    assert_eq!(off.status, 401);
    assert_eq!(s.workbench.threads().unwrap_or_default().len(), 1);
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn a_host_without_a_trigger_port_has_no_webhooks() {
    let s = setup("schedule-no-webhook", bounded());
    let id = s
        .workbench
        .save_schedule(None, input(&s.project, When::Manual), None)
        .unwrap_or_else(|e| panic!("{e}"))
        .schedule
        .id;
    let refused = s
        .workbench
        .set_schedule_webhook(&id, true)
        .await
        .err()
        .unwrap_or_default();
    assert!(refused.contains("not available"), "{refused}");
    assert!(!saved(&s, &id).schedule.webhook);
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn an_agent_schedules_within_its_own_sessions_limits() {
    let s = setup("schedule-agent", bounded());
    let mut new = new_root(&s.project, "Lead");
    new.permission_mode = PermissionMode::Default;
    let lead = root(&s, new).await;
    let ask = AgentSchedule {
        prompt: "Check the build".into(),
        title: None,
        when: When::Interval { minutes: 30 },
        agent: None,
        model: None,
        permission_mode: Some(PermissionMode::BypassPermissions),
        max_runs_per_day: Some(3),
        catch_up: None,
    };
    let view = s
        .workbench
        .schedule_for(&lead.id, ask.clone())
        .unwrap_or_else(|e| panic!("{e}"));
    // Never above the session that asked, on its project, with its route.
    assert_eq!(view.schedule.permission_mode, PermissionMode::Default);
    assert_eq!(
        (view.schedule.project_id.as_str(), view.schedule.via),
        (s.project.as_str(), lead.via)
    );
    assert_eq!(view.schedule.created_by.as_deref(), Some(lead.id.as_str()));
    let bad = AgentSchedule {
        max_runs_per_day: Some(500),
        ..ask
    };
    assert_eq!(
        s.workbench.schedule_for(&lead.id, bad).err().map(|e| e.code),
        Some(OrchCode::BadInput)
    );

    let person = s
        .workbench
        .save_schedule(None, input(&s.project, When::Manual), None)
        .unwrap_or_else(|e| panic!("{e}"))
        .schedule
        .id;
    assert_eq!(s.workbench.schedules_for(&lead.id).map(|list| list.len()), Ok(2));
    let refused = s.workbench.unschedule_for(&lead.id, &person).err().map(|e| e.code);
    assert_eq!(refused, Some(OrchCode::NotYourSession));
    assert_eq!(s.workbench.unschedule_for(&lead.id, &view.schedule.id), Ok(()));
    assert_eq!(s.workbench.schedules().map(|list| list.len()), Ok(1));
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

/// A schedule for another agent takes that agent's default model, never the caller's, and an agent Gateway has no
/// route for runs on the person's own sign-in. With "Enable cross-agent subsessions" off, another agent is refused.
#[tokio::test]
async fn a_schedule_for_another_agent_never_carries_the_callers_model() {
    let s = setup("schedule-other-agent", Limits::default());
    let mut new = new_root(&s.project, "Lead");
    new.model = Some("claude-opus-5-5".into());
    new.effort = Some(nexus_workbench::event::Effort::Max);
    let lead = root(&s, new).await;
    let ask = |agent| AgentSchedule {
        prompt: "Check the build".into(),
        title: None,
        when: When::Interval { minutes: 30 },
        agent,
        model: None,
        permission_mode: None,
        max_runs_per_day: None,
        catch_up: None,
    };
    let schedule = |agent| {
        s.workbench
            .schedule_for(&lead.id, ask(agent))
            .unwrap_or_else(|e| panic!("{e}"))
            .schedule
    };
    let own = schedule(None);
    assert_eq!(
        (own.agent, own.model.as_deref()),
        (SessionAgent::Claude, Some("claude-opus-5-5"))
    );
    let codex = schedule(Some(SessionAgent::Codex));
    assert_eq!(
        (codex.agent, codex.model, codex.via),
        (SessionAgent::Codex, None, Via::Nexus)
    );
    let agy = schedule(Some(SessionAgent::Antigravity));
    assert_eq!(
        (agy.agent, agy.model, agy.via),
        (SessionAgent::Antigravity, None, Via::Subscription)
    );
    // The nearest effort each agent has under the caller's max: Codex has max; Antigravity's levels end below it, so
    // its default.
    use nexus_workbench::event::Effort;
    assert_eq!(
        (own.effort, codex.effort, agy.effort),
        (Some(Effort::Max), Some(Effort::Max), None)
    );

    s.workbench.set_subsessions(nexus_workbench::Subsessions {
        cross_agent_off: true,
        ..Default::default()
    });
    let refused = s
        .workbench
        .schedule_for(&lead.id, ask(Some(SessionAgent::Codex)))
        .err()
        .unwrap_or_else(|| panic!("scheduled another agent"));
    assert_eq!(refused.code, OrchCode::AgentNotAllowed);
    assert!(
        refused
            .message
            .contains("turned off \"Enable cross-agent subsessions\""),
        "{}",
        refused.message
    );
    assert_eq!(
        schedule(None).agent,
        SessionAgent::Claude,
        "its own agent still schedules"
    );
    s.workbench.shutdown().await;
    let _ = std::fs::remove_dir_all(&s.root);
}

#[tokio::test]
async fn closing_a_project_takes_its_schedules_away() {
    let s = setup("schedule-project", bounded());
    s.workbench
        .save_schedule(None, input(&s.project, When::Interval { minutes: 60 }), None)
        .unwrap_or_else(|e| panic!("{e}"));
    s.workbench.remove_project(&s.project).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.workbench.schedules().map(|list| list.len()), Ok(0));
    let refused = s
        .workbench
        .save_schedule(None, input(&s.project, When::Manual), None)
        .err()
        .unwrap_or_default();
    assert_eq!(refused, "That project is no longer open.");
    let _ = std::fs::remove_dir_all(&s.root);
}
