//! The sign-in Nexus runs, on a fake: the test plays the login's events and answers each read by hand, so no agent
//! runs, no folder is made and no settings file changes.

use std::cell::RefCell;
use std::rc::Rc;

use core::prelude::v1::test;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use nexus_workbench::event::SessionAgent;
use nexus_workbench::plans::PlanUsage;

use super::login::{Command, Event, Exit, Mode, Runner};
use super::sign_in::{Ops, POLL, Step, signed_in_note};
use super::{Backend, Load, Plans, fold_key};

fn login(agent: SessionAgent, account: Option<&str>, email: &str, plan: Option<&str>, signed_in: bool) -> PlanUsage {
    PlanUsage {
        agent,
        account: account.map(Into::into),
        email: Some(email.into()),
        plan: plan.map(Into::into),
        signed_in,
        // An empty reading has every other field, whatever fields `PlanUsage` gains.
        ..nexus_workbench::plans::read_claude(None, &serde_json::json!({}), None)
    }
}

/// Claude's own login, never signed in.
fn own_signed_out() -> PlanUsage {
    PlanUsage {
        email: None,
        plan: None,
        ..login(SessionAgent::Claude, None, "", None, false)
    }
}

/// One login Nexus started: what it was asked, its events, and what the page asked of it.
struct Started {
    agent: SessionAgent,
    account: Option<String>,
    email: Option<String>,
    mode: Mode,
    events: UnboundedSender<Event>,
    commands: UnboundedReceiver<Command>,
}

/// A read the page asked for: of which login, and where its answer goes.
type Read = (
    (SessionAgent, Option<String>),
    oneshot::Sender<Result<PlanUsage, String>>,
);
type Reads = Rc<RefCell<Vec<Read>>>;

#[derive(Default)]
struct Fake {
    started: Rc<RefCell<Vec<Started>>>,
    reads: Reads,
    /// The ids Add account hands out, in order.
    ids: Rc<RefCell<Vec<String>>>,
    forgot: Rc<RefCell<Vec<(SessionAgent, String)>>>,
    selected: Rc<RefCell<Vec<Login>>>,
    probes: Rc<RefCell<usize>>,
}

/// A login: its agent and its account.
type Login = (SessionAgent, Option<String>);

impl Fake {
    fn plans(&self, list: Vec<PlanUsage>, cx: &mut TestAppContext) -> Entity<Plans> {
        let (started, reads, ids) = (self.started.clone(), self.reads.clone(), self.ids.clone());
        let (forgot, selected, probes) = (self.forgot.clone(), self.selected.clone(), self.probes.clone());
        let ops = Ops {
            start: Rc::new(move |request| {
                let (runner, commands) = Runner::fake();
                let (events, receive) = unbounded();
                started.borrow_mut().push(Started {
                    agent: request.agent,
                    account: request.account,
                    email: request.email,
                    mode: request.mode,
                    events,
                    commands,
                });
                (runner, receive)
            }),
            read: Rc::new(move |agent, account| {
                let (answer, wait) = oneshot::channel();
                reads.borrow_mut().push(((agent, account), answer));
                Box::pin(async move { wait.await.unwrap_or_else(|_| Err("stopped".into())) })
            }),
            add: Rc::new(move |_| {
                let id = ids.borrow_mut().remove(0);
                Box::pin(async move { Ok(id) })
            }),
            forget: Rc::new(move |agent, id| forgot.borrow_mut().push((agent, id.into()))),
            select: Rc::new(move |agent, account| {
                selected.borrow_mut().push((agent, account));
                Box::pin(async { Ok(Default::default()) })
            }),
        };
        let backend = Backend {
            // Checks never answer: these tests are about the sign-in.
            probe: Rc::new(move |_| {
                *probes.borrow_mut() += 1;
                Box::pin(futures::future::pending())
            }),
            remove: Rc::new(|_, _, _| Box::pin(futures::future::pending())),
            sign_in: ops,
        };
        cx.new(|_| {
            let mut plans = Plans::with(Rc::new(|_, _| {}), backend);
            plans.plans = Load::Ready(list);
            plans
        })
    }

    /// The login `index` says `event`.
    fn say(&self, index: usize, event: Event, cx: &mut TestAppContext) {
        self.started.borrow()[index].events.unbounded_send(event).unwrap();
        cx.run_until_parked();
    }

    /// Answer the oldest read.
    fn answer(&self, plan: PlanUsage, cx: &mut TestAppContext) {
        let (_, answer) = self.reads.borrow_mut().remove(0);
        answer.send(Ok(plan)).unwrap();
        cx.run_until_parked();
    }

    /// What the page asked of login `index` since the last look; `closed` once the page let go of it.
    fn asked(&self, index: usize) -> (Vec<Command>, bool) {
        let mut started = self.started.borrow_mut();
        let commands = &mut started[index].commands;
        let mut asked = vec![];
        loop {
            match commands.try_recv() {
                Ok(command) => asked.push(command),
                Err(error) => return (asked, error.is_closed()),
            }
        }
    }
}

fn step(plans: &Entity<Plans>, cx: &mut TestAppContext) -> Option<Step> {
    plans.read_with(cx, |plans, _| {
        plans.sign_in.as_ref().map(|sign_in| sign_in.step.clone())
    })
}

fn keys(plans: &Entity<Plans>, cx: &mut TestAppContext) -> Vec<String> {
    plans.read_with(cx, |plans, _| match &plans.plans {
        Load::Ready(list) => list.iter().map(fold_key).collect(),
        _ => vec![],
    })
}

fn press_sign_in(plans: &Entity<Plans>, plan: &PlanUsage, cx: &mut TestAppContext) {
    plans.update(cx, |plans, cx| plans.sign_in(plan, cx));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn one_press_starts_the_login_and_nothing_else_starts_meanwhile(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let own = own_signed_out();
    let plans = fake.plans(vec![own.clone()], cx);
    press_sign_in(&plans, &own, cx);
    assert_eq!(fake.started.borrow().len(), 1);
    {
        let started = &fake.started.borrow()[0];
        assert_eq!(
            (started.agent, started.account.clone(), started.mode),
            (SessionAgent::Claude, None, Mode::Browser)
        );
    }
    assert_eq!(step(&plans, cx), Some(Step::Starting));
    fake.say(0, Event::Link("https://claude.com/cai/oauth/authorize?x=1".into()), cx);
    assert_eq!(step(&plans, cx), Some(Step::Waiting));
    // A second press, Add account and Remove all wait: one sign-in at a time, and it holds `busy`.
    press_sign_in(&plans, &own, cx);
    plans.update(cx, |plans, cx| {
        plans.add(SessionAgent::Claude, cx);
        plans.remove(SessionAgent::Claude, None, cx);
    });
    cx.run_until_parked();
    assert_eq!(fake.started.borrow().len(), 1);
    plans.read_with(cx, |plans, _| {
        assert!(plans.busy);
        assert_eq!(plans.confirm_remove, None);
        assert_eq!(
            plans
                .sign_in
                .as_ref()
                .and_then(|sign_in| sign_in.link.clone())
                .as_deref(),
            Some("https://claude.com/cai/oauth/authorize?x=1")
        );
    });
}

/// New session's Connect: the own login's sign-in starts in the browser, no terminal, and names the account
/// once it is in.
#[gpui_kit::test]
fn connect_signs_in_the_own_login_in_the_browser_and_names_the_account(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let plans = fake.plans(vec![own_signed_out()], cx);
    plans.update(cx, |plans, cx| plans.connect(SessionAgent::Claude, None, cx));
    cx.run_until_parked();
    {
        let started = &fake.started.borrow()[0];
        assert_eq!(
            (started.agent, started.account.clone(), started.mode),
            (SessionAgent::Claude, None, Mode::Browser)
        );
    }
    plans.read_with(cx, |plans, _| {
        let sign_in = plans.sign_in.as_ref().expect("a sign-in card");
        assert!(!sign_in.terminal && !sign_in.terminal_open, "no terminal");
    });
    fake.say(0, Event::Link("https://claude.com/cai/oauth/authorize?x=1".into()), cx);
    assert_eq!(step(&plans, cx), Some(Step::Waiting));
    fake.say(0, Event::Exited(Exit::Status(Some(0))), cx);
    fake.answer(
        login(SessionAgent::Claude, None, "ana@acme.dev", Some("Claude Max"), true),
        cx,
    );
    plans.read_with(cx, |plans, _| {
        assert_eq!(
            plans.removed_note,
            Some((
                SessionAgent::Claude,
                "Connected as ana@acme.dev. New sessions start on it.".to_string()
            ))
        );
    });
}

#[gpui_kit::test]
fn connect_signs_in_the_selected_named_profile(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let plans = fake.plans(vec![own_signed_out()], cx);
    plans.update(cx, |plans, cx| {
        plans.connect(SessionAgent::Claude, Some("a_12".into()), cx)
    });
    cx.run_until_parked();

    let started = fake.started.borrow();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].account.as_deref(), Some("a_12"));
    assert_eq!(started[0].mode, Mode::Browser);
}

#[gpui_kit::test]
fn checklist_signs_in_the_selected_provider_profile_and_keeps_cancel_inline(cx: &mut TestAppContext) {
    for (agent, account) in [
        (SessionAgent::Claude, None),
        (SessionAgent::Codex, Some("a_12".to_string())),
    ] {
        let fake = Fake::default();
        let plans = fake.plans(vec![], cx);
        plans.update(cx, |plans, cx| plans.connect_from_checklist(agent, account.clone(), cx));
        cx.run_until_parked();
        {
            let requests = fake.started.borrow();
            assert_eq!(requests.len(), 1);
            assert_eq!(
                (requests[0].agent, &requests[0].account, requests[0].mode),
                (agent, &account, Mode::Browser)
            );
        }
        plans.read_with(cx, |plans, _| {
            let login = plans.sign_in.as_ref().expect("checklist sign-in");
            assert!(login.from_checklist && !login.terminal && !login.new);
        });
        plans.update(cx, |plans, cx| plans.close_sign_in(cx));
        cx.run_until_parked();
        let (asked, closed) = fake.asked(0);
        assert!(asked.contains(&Command::Cancel) || closed);
    }
}

#[gpui_kit::test]
fn connect_can_be_cancelled_and_stops_the_login(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let plans = fake.plans(vec![own_signed_out()], cx);
    plans.update(cx, |plans, cx| plans.connect(SessionAgent::Codex, None, cx));
    cx.run_until_parked();
    plans.update(cx, |plans, cx| plans.close_sign_in(cx));
    cx.run_until_parked();
    let (asked, closed) = fake.asked(0);
    assert!(asked.contains(&Command::Cancel) || closed, "{asked:?}");
    plans.read_with(cx, |plans, _| assert!(plans.sign_in.is_none() && !plans.busy));
}

#[gpui_kit::test]
fn sign_in_again_pre_fills_the_email(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let ana = login(
        SessionAgent::Claude,
        Some("a_1"),
        "ana@acme.dev",
        Some("Claude Max"),
        true,
    );
    let plans = fake.plans(vec![own_signed_out(), ana.clone()], cx);
    press_sign_in(&plans, &ana, cx);
    let started = &fake.started.borrow()[0];
    assert_eq!(started.account.as_deref(), Some("a_1"));
    assert_eq!(started.email.as_deref(), Some("ana@acme.dev"));
}

#[gpui_kit::test]
fn exit_zero_reads_just_that_login_and_its_row_takes_it(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let own = own_signed_out();
    let plans = fake.plans(vec![own.clone()], cx);
    press_sign_in(&plans, &own, cx);
    fake.say(0, Event::Link("https://claude.com/x".into()), cx);
    fake.say(0, Event::Exited(Exit::Status(Some(0))), cx);
    assert_eq!(step(&plans, cx), Some(Step::Reading));
    assert_eq!(
        fake.reads.borrow()[0].0,
        (SessionAgent::Claude, None),
        "only this login is read"
    );
    let probes = *fake.probes.borrow();
    fake.answer(
        login(SessionAgent::Claude, None, "ana@acme.dev", Some("Claude Max"), true),
        cx,
    );
    plans.read_with(cx, |plans, _| {
        assert!(plans.sign_in.is_none(), "the card folds");
        assert!(!plans.busy);
        let Load::Ready(list) = &plans.plans else { panic!() };
        assert!(list[0].signed_in && list[0].email.as_deref() == Some("ana@acme.dev"));
        // The agent's only account: new sessions start on it.
        assert_eq!(
            plans.removed_note,
            Some((SessionAgent::Claude, signed_in_note("ana@acme.dev", true)))
        );
    });
    assert_eq!(*fake.selected.borrow(), [(SessionAgent::Claude, None)]);
    assert!(*fake.probes.borrow() > probes, "then every login is read again");
}

#[gpui_kit::test]
fn an_exit_during_a_poll_read_still_reads_after_the_exit(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let own = own_signed_out();
    let plans = fake.plans(vec![own.clone()], cx);
    press_sign_in(&plans, &own, cx);
    fake.say(0, Event::Link("https://claude.com/x".into()), cx);
    // A poll read starts, then the login exits 0 before it answers.
    cx.executor().advance_clock(POLL);
    cx.run_until_parked();
    assert_eq!(fake.reads.borrow().len(), 1);
    fake.say(0, Event::Exited(Exit::Status(Some(0))), cx);
    assert_eq!(step(&plans, cx), Some(Step::Reading));
    assert_eq!(fake.reads.borrow().len(), 1, "one read at a time");
    // The poll read saw the login before the sign-in landed. The card reads again instead of waiting forever.
    fake.answer(own.clone(), cx);
    assert_eq!(step(&plans, cx), Some(Step::Reading));
    assert_eq!(fake.reads.borrow().len(), 1, "the read after the exit runs");
    fake.answer(
        login(SessionAgent::Claude, None, "ana@acme.dev", Some("Claude Max"), true),
        cx,
    );
    plans.read_with(cx, |plans, _| {
        assert!(plans.sign_in.is_none(), "the card folds");
        assert!(!plans.busy, "the page is free again");
    });
}

#[gpui_kit::test]
fn a_poll_finds_the_login_signed_in_while_its_browser_waits(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let ana = login(SessionAgent::Claude, None, "ana@acme.dev", Some("Claude Max"), true);
    let bo = login(
        SessionAgent::Claude,
        Some("a_1"),
        "bo@acme.dev",
        Some("Claude Max"),
        false,
    );
    let plans = fake.plans(vec![ana, bo.clone()], cx);
    press_sign_in(&plans, &bo, cx);
    fake.say(0, Event::Link("https://claude.com/x".into()), cx);
    assert!(fake.reads.borrow().is_empty());
    cx.executor().advance_clock(POLL);
    cx.run_until_parked();
    assert_eq!(fake.reads.borrow().len(), 1);
    // Not yet: it keeps waiting.
    fake.answer(bo.clone(), cx);
    assert_eq!(step(&plans, cx), Some(Step::Waiting));
    cx.executor().advance_clock(POLL);
    cx.run_until_parked();
    fake.answer(PlanUsage { signed_in: true, ..bo }, cx);
    assert_eq!(step(&plans, cx), None);
    // The login still waited for the browser: Nexus let go of it, which kills it.
    assert!(fake.asked(0).1);
    // Another account is signed in: new sessions stay where they were.
    assert!(fake.selected.borrow().is_empty());
    plans.read_with(cx, |plans, _| {
        assert_eq!(
            plans.removed_note,
            Some((SessionAgent::Claude, signed_in_note("bo@acme.dev", false)))
        );
    });
}

#[gpui_kit::test]
fn a_new_account_is_hidden_while_it_signs_in_and_goes_when_cancelled(cx: &mut TestAppContext) {
    let fake = Fake::default();
    fake.ids.borrow_mut().push("a_9".into());
    let ana = login(SessionAgent::Claude, None, "ana@acme.dev", Some("Claude Max"), true);
    let plans = fake.plans(vec![ana], cx);
    plans.update(cx, |plans, cx| plans.add(SessionAgent::Claude, cx));
    cx.run_until_parked();
    assert_eq!(fake.started.borrow()[0].account.as_deref(), Some("a_9"));
    assert_eq!(keys(&plans, cx), ["claude:own", "claude:a_9"]);
    plans.read_with(cx, |plans, _| assert_eq!(plans.hidden().as_deref(), Some("claude:a_9")));
    plans.update(cx, |plans, cx| plans.close_sign_in(cx));
    cx.run_until_parked();
    assert_eq!(*fake.forgot.borrow(), [(SessionAgent::Claude, "a_9".to_string())]);
    assert_eq!(keys(&plans, cx), ["claude:own"]);
    assert!(fake.asked(0).1, "Cancel kills the login");
    plans.read_with(cx, |plans, _| assert!(!plans.busy && plans.hidden().is_none()));
}

#[gpui_kit::test]
fn a_failure_keeps_what_the_login_said_and_close_deletes_the_new_account(cx: &mut TestAppContext) {
    let fake = Fake::default();
    fake.ids.borrow_mut().push("a_9".into());
    let plans = fake.plans(vec![own_signed_out()], cx);
    plans.update(cx, |plans, cx| plans.add(SessionAgent::Claude, cx));
    cx.run_until_parked();
    fake.say(0, Event::Line("Error: OAuth token exchange failed (400)".into()), cx);
    fake.say(0, Event::Exited(Exit::Status(Some(1))), cx);
    assert_eq!(
        step(&plans, cx),
        Some(Step::Failed(Some("Error: OAuth token exchange failed (400)".into())))
    );
    plans.read_with(cx, |plans, _| assert!(!plans.busy));
    assert!(fake.forgot.borrow().is_empty(), "Try again keeps the folder");
    // Try again starts the login once more, on the same account.
    plans.update(cx, |plans, cx| plans.try_sign_in_again(cx));
    cx.run_until_parked();
    assert_eq!(fake.started.borrow().len(), 2);
    assert_eq!(fake.started.borrow()[1].account.as_deref(), Some("a_9"));
    fake.say(1, Event::Exited(Exit::TimedOut), cx);
    assert_eq!(step(&plans, cx), Some(Step::TimedOut));
    plans.update(cx, |plans, cx| plans.close_sign_in(cx));
    cx.run_until_parked();
    assert_eq!(*fake.forgot.borrow(), [(SessionAgent::Claude, "a_9".to_string())]);
}

#[gpui_kit::test]
fn a_new_account_on_a_login_already_here_is_not_kept(cx: &mut TestAppContext) {
    let fake = Fake::default();
    fake.ids.borrow_mut().push("a_9".into());
    let ana = login(SessionAgent::Claude, None, "ana@acme.dev", Some("Claude Max"), true);
    let plans = fake.plans(vec![ana], cx);
    plans.update(cx, |plans, cx| plans.add(SessionAgent::Claude, cx));
    cx.run_until_parked();
    fake.say(0, Event::Exited(Exit::Status(Some(0))), cx);
    fake.answer(
        login(
            SessionAgent::Claude,
            Some("a_9"),
            "Ana@Acme.dev",
            Some("Claude Max"),
            true,
        ),
        cx,
    );
    assert_eq!(step(&plans, cx), Some(Step::Duplicate("ana@acme.dev".into())));
    assert_eq!(*fake.forgot.borrow(), [(SessionAgent::Claude, "a_9".to_string())]);
    assert_eq!(keys(&plans, cx), ["claude:own"]);
    // Closing the card deletes nothing more.
    plans.update(cx, |plans, cx| plans.close_sign_in(cx));
    cx.run_until_parked();
    assert_eq!(fake.forgot.borrow().len(), 1);
}

#[gpui_kit::test]
fn a_codex_login_with_no_plan_stays_and_says_so(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let own = PlanUsage {
        email: None,
        plan: None,
        ..login(SessionAgent::Codex, None, "", None, false)
    };
    let plans = fake.plans(vec![own.clone()], cx);
    press_sign_in(&plans, &own, cx);
    fake.say(0, Event::Exited(Exit::Status(Some(0))), cx);
    fake.answer(login(SessionAgent::Codex, None, "cy@acme.dev", None, true), cx);
    assert_eq!(step(&plans, cx), Some(Step::NoPlan("cy@acme.dev".into())));
    plans.read_with(cx, |plans, _| {
        let Load::Ready(list) = &plans.plans else { panic!() };
        assert!(list[0].signed_in, "it stays, to remove or keep");
        assert!(!plans.busy);
    });
}

#[gpui_kit::test]
fn a_pasted_code_reaches_the_login_once(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let own = own_signed_out();
    let plans = fake.plans(vec![own.clone()], cx);
    press_sign_in(&plans, &own, cx);
    plans.update(cx, |plans, cx| plans.submit_code("  abc#123 \n".into(), cx));
    cx.run_until_parked();
    assert_eq!(fake.asked(0), (vec![Command::Code("abc#123".into())], false));
    // An empty paste sends nothing.
    plans.update(cx, |plans, cx| plans.submit_code("   ".into(), cx));
    assert_eq!(fake.asked(0), (vec![], false));
    // A wrong code: the login's error shows under the field.
    fake.say(0, Event::Line("OAuth error: Invalid code".into()), cx);
    plans.read_with(cx, |plans, _| {
        let sign_in = plans.sign_in.as_ref().unwrap();
        assert!(sign_in.code_sent);
        assert_eq!(sign_in.last_line.as_deref(), Some("OAuth error: Invalid code"));
    });
}

#[gpui_kit::test]
fn codex_can_switch_to_a_device_code(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let own = PlanUsage {
        email: None,
        plan: None,
        ..login(SessionAgent::Codex, None, "", None, false)
    };
    let plans = fake.plans(vec![own.clone()], cx);
    press_sign_in(&plans, &own, cx);
    plans.update(cx, |plans, cx| plans.use_device_code(cx));
    assert_eq!(fake.asked(0), (vec![Command::DeviceCode], false));
    fake.say(0, Event::Started(Mode::DeviceCode), cx);
    let expires_at = std::time::Instant::now() + std::time::Duration::from_secs(900);
    fake.say(
        0,
        Event::Code {
            code: "NJSZ-6I2YS".into(),
            expires_at,
        },
        cx,
    );
    plans.read_with(cx, |plans, _| {
        let sign_in = plans.sign_in.as_ref().unwrap();
        assert_eq!(sign_in.mode, Mode::DeviceCode);
        assert_eq!(sign_in.code.as_ref().map(|(code, _)| code.as_str()), Some("NJSZ-6I2YS"));
        assert_eq!(sign_in.step, Step::Waiting);
    });
}

#[gpui_kit::test]
fn a_terminal_stops_nexuss_login_and_the_card_reads_the_login(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let own = own_signed_out();
    let plans = fake.plans(vec![own.clone()], cx);
    press_sign_in(&plans, &own, cx);
    plans.update(cx, |plans, cx| plans.copy_sign_in_command(cx));
    cx.run_until_parked();
    assert!(fake.asked(0).1, "Nexus's own login stops");
    plans.read_with(cx, |plans, _| {
        let sign_in = plans.sign_in.as_ref().unwrap();
        assert!(sign_in.terminal && plans.busy);
        assert_eq!(plans.copied.as_deref(), Some("claude auth login"));
    });
    // The window comes back to the front: one read.
    plans.update(cx, |plans, cx| plans.window_active(cx));
    cx.run_until_parked();
    assert_eq!(fake.reads.borrow().len(), 1);
    fake.answer(
        login(SessionAgent::Claude, None, "ana@acme.dev", Some("Claude Max"), true),
        cx,
    );
    assert_eq!(step(&plans, cx), None);
}

#[gpui_kit::test]
fn closing_the_page_deletes_a_new_account_that_never_signed_in(cx: &mut TestAppContext) {
    let fake = Fake::default();
    fake.ids.borrow_mut().push("a_9".into());
    let plans = fake.plans(vec![own_signed_out()], cx);
    plans.update(cx, |plans, cx| plans.add(SessionAgent::Claude, cx));
    cx.run_until_parked();
    drop(plans);
    // GPUI releases a dropped entity when it next flushes its effects.
    cx.update(|_| {});
    cx.run_until_parked();
    assert_eq!(*fake.forgot.borrow(), [(SessionAgent::Claude, "a_9".to_string())]);
    assert!(fake.asked(0).1, "the login stops with the page");
}

#[test]
fn a_finished_stream_of_events_ends() {
    // The fake's channels behave as the runner's: once the page lets go, the login sees the end.
    let (runner, mut commands) = Runner::fake();
    drop(runner);
    assert_eq!(futures::executor::block_on(commands.next()), None);
}

#[gpui_kit::test]
fn retry_after_a_storage_failure_keeps_the_selected_account(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let plan = login(SessionAgent::Claude, Some("work"), "", None, false);
    let plans = fake.plans(vec![plan.clone()], cx);
    press_sign_in(&plans, &plan, cx);
    let reason = "This Mac can’t save a Claude sign-in right now.";
    fake.say(0, Event::Exited(Exit::NotStarted(reason.into())), cx);
    assert_eq!(step(&plans, cx), Some(Step::NotStarted(reason.into())));
    cx.executor().advance_clock(POLL);
    cx.run_until_parked();
    assert!(fake.reads.borrow().is_empty());
    plans.update(cx, |plans, cx| plans.try_sign_in_again(cx));
    cx.run_until_parked();
    let started = fake.started.borrow();
    assert_eq!(started.len(), 2);
    assert_eq!(started[1].account.as_deref(), Some("work"));
    assert_eq!(started[1].mode, Mode::Browser);
}
