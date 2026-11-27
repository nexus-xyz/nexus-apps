//! Removing a login: the row's states, the checks around it, and where new sessions go. The backend is a fake whose
//! probes and removals the test answers by hand, so no agent runs and no settings file changes.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use core::prelude::v1::test;
use futures::channel::{mpsc::UnboundedSender, oneshot};
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use nexus_workbench::event::SessionAgent;
use nexus_workbench::plans::PlanUsage;

use super::{Backend, Load, Plans, Progress, Removed, login_key, move_off, replacement};

fn login(agent: SessionAgent, account: Option<&str>, email: &str, signed_in: bool) -> PlanUsage {
    PlanUsage {
        agent,
        account: account.map(Into::into),
        email: Some(email.into()),
        plan: Some("Claude Max".into()),
        signed_in,
        active: false,
        limits: vec![],
        resets: None,
        error: None,
        google_cloud: false,
        unavailable: false,
        fetched_at: None,
    }
}

/// Ana's own Claude login and Bo's extra account, both signed in.
fn two_logins() -> Vec<PlanUsage> {
    vec![
        login(SessionAgent::Claude, None, "ana@acme.dev", true),
        login(SessionAgent::Claude, Some("a_1"), "bo@acme.dev", true),
    ]
}

fn logins_of(plans: &[PlanUsage]) -> Vec<(SessionAgent, Option<String>)> {
    plans.iter().map(|plan| (plan.agent, plan.account.clone())).collect()
}

type Probes = Rc<RefCell<Vec<(UnboundedSender<Progress>, oneshot::Sender<Result<(), String>>)>>>;
type Asked = (SessionAgent, Option<String>, Option<String>);
type Removals = Rc<RefCell<Vec<(Asked, oneshot::Sender<Result<Removed, String>>)>>>;

/// The probes and removals `Plans` started, waiting for the test to answer them.
#[derive(Default)]
struct Fake {
    probes: Probes,
    removals: Removals,
    /// How many times the app was told the settings changed.
    saved: Rc<Cell<usize>>,
}

impl Fake {
    fn plans(&self, list: Vec<PlanUsage>, cx: &mut TestAppContext) -> Entity<Plans> {
        let probes = self.probes.clone();
        let removals = self.removals.clone();
        let backend = Backend {
            probe: Rc::new(move |send| {
                let (done, wait) = oneshot::channel();
                probes.borrow_mut().push((send, done));
                Box::pin(async move { wait.await.unwrap_or(Ok(())) })
            }),
            remove: Rc::new(move |agent, id, next| {
                let (done, wait) = oneshot::channel();
                removals.borrow_mut().push(((agent, id, next), done));
                Box::pin(async move { wait.await.unwrap_or_else(|_| Err("stopped".into())) })
            }),
            sign_in: super::sign_in::Ops::inert(),
        };
        let saved = self.saved.clone();
        cx.new(|_| {
            let mut plans = Plans::with(Rc::new(move |_, _| saved.set(saved.get() + 1)), backend);
            plans.plans = Load::Ready(list);
            plans
        })
    }

    /// Answer the oldest running check: it probed `logins` and read `read`.
    fn answer_check(&self, logins: Vec<(SessionAgent, Option<String>)>, read: Vec<PlanUsage>, cx: &mut TestAppContext) {
        let (send, done) = self.probes.borrow_mut().remove(0);
        send.unbounded_send(Progress { logins, read }).unwrap();
        drop(send);
        done.send(Ok(())).unwrap();
        cx.run_until_parked();
    }

    /// Answer the oldest running removal.
    fn answer_removal(&self, answer: Result<Removed, String>, cx: &mut TestAppContext) {
        let (_, done) = self.removals.borrow_mut().remove(0);
        let _ = done.send(answer);
        cx.run_until_parked();
    }

    fn asked(&self) -> Vec<Asked> {
        self.removals.borrow().iter().map(|(asked, _)| asked.clone()).collect()
    }
}

fn press_remove(plans: &Entity<Plans>, agent: SessionAgent, account: Option<&str>, cx: &mut TestAppContext) {
    plans.update(cx, |plans, cx| plans.remove(agent, account.map(Into::into), cx));
    cx.run_until_parked();
}

fn shown(plans: &Entity<Plans>, cx: &mut TestAppContext) -> Vec<String> {
    plans.read_with(cx, |plans, _| match &plans.plans {
        Load::Ready(list) => list.iter().map(super::fold_key).collect(),
        _ => vec![],
    })
}

const BO: &str = "claude:a_1";

#[gpui_kit::test]
fn the_second_press_marks_the_row_pending_and_other_presses_start_nothing(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let plans = fake.plans(two_logins(), cx);
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    plans.read_with(cx, |plans, _| {
        assert!(plans.removal.is_none(), "the first press only asks");
        assert_eq!(plans.confirm_remove, Some((SessionAgent::Claude, Some("a_1".into()))));
    });
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    plans.read_with(cx, |plans, _| {
        assert_eq!(plans.removal.as_ref().map(|removal| removal.key()).as_deref(), Some(BO));
        assert!(plans.busy, "a removal keeps the card busy");
        assert_eq!(plans.confirm_remove, None);
    });
    // Bo's account goes; new sessions would move to Ana's own login.
    assert_eq!(fake.asked(), [(SessionAgent::Claude, Some("a_1".into()), None)]);
    // The same row again, another row, and Try again: none starts a second removal, nor asks to confirm.
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    press_remove(&plans, SessionAgent::Claude, None, cx);
    press_remove(&plans, SessionAgent::Claude, None, cx);
    plans.update(cx, |plans, cx| plans.retry_remove(SessionAgent::Claude, None, cx));
    assert_eq!(fake.asked().len(), 1);
    plans.read_with(cx, |plans, _| assert_eq!(plans.confirm_remove, None));
}

#[gpui_kit::test]
fn a_removed_row_folds_away_and_the_card_says_so(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let plans = fake.plans(two_logins(), cx);
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    fake.answer_removal(Ok(Removed { settings: None }), cx);
    plans.read_with(cx, |plans, _| {
        assert!(plans.removal.is_none() && !plans.busy);
        assert!(plans.removed.contains_key(BO));
        assert_eq!(
            plans.removed_note,
            Some((SessionAgent::Claude, "Removed bo@acme.dev from this computer.".into()))
        );
        assert!(plans.removal_errors.is_empty() && plans.problem.is_none());
    });
    assert_eq!(fake.probes.borrow().len(), 1, "the page reads again at once");
    // The row stays, folding, for the collapse; then it goes.
    assert_eq!(shown(&plans, cx), ["claude:own", BO]);
    cx.executor().advance_clock(crate::motion::COLLAPSE);
    cx.run_until_parked();
    assert_eq!(shown(&plans, cx), ["claude:own"]);
    assert_eq!(fake.saved.get(), 0, "new sessions did not start on Bo's account");
    // The next action on the card clears the note.
    press_remove(&plans, SessionAgent::Claude, None, cx);
    plans.read_with(cx, |plans, _| assert_eq!(plans.removed_note, None));
}

#[gpui_kit::test]
fn a_check_from_before_the_removal_cannot_bring_the_row_back(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let plans = fake.plans(two_logins(), cx);
    plans.update(cx, |plans, cx| plans.check(cx));
    cx.run_until_parked();
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    fake.answer_removal(Ok(Removed { settings: None }), cx);
    // More checks asked for while the first runs: they run once, after it.
    plans.update(cx, |plans, cx| {
        plans.check(cx);
        plans.check(cx);
    });
    cx.executor().advance_clock(crate::motion::COLLAPSE);
    cx.run_until_parked();
    assert_eq!(fake.probes.borrow().len(), 1, "a check asked for while one runs waits");
    // The first check started before the removal, and reads Bo's account as it was.
    let before = two_logins();
    fake.answer_check(logins_of(&before), before.clone(), cx);
    assert_eq!(
        shown(&plans, cx),
        ["claude:own"],
        "the stale reading does not show the row"
    );
    assert_eq!(fake.probes.borrow().len(), 1, "the queued check runs once");
    plans.read_with(cx, |plans, _| assert!(plans.checking));
    // Bo signed in again: the later check reads the account, and the row is back.
    fake.answer_check(logins_of(&before), before, cx);
    assert_eq!(shown(&plans, cx), ["claude:own", BO]);
    plans.read_with(cx, |plans, _| {
        assert!(plans.removed.is_empty());
        assert!(!plans.checking);
    });
    assert!(fake.probes.borrow().is_empty(), "nothing else was queued");
}

#[gpui_kit::test]
fn an_answer_for_another_run_leaves_the_running_removal(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let plans = fake.plans(two_logins(), cx);
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    plans.update(cx, |plans, cx| {
        let stale = plans.removal_seq + 1;
        plans.finish_removal(stale, Ok(Removed { settings: None }), cx);
        plans.finish_removal(stale, Err("late".into()), cx);
    });
    plans.read_with(cx, |plans, _| {
        assert_eq!(plans.removal.as_ref().map(|removal| removal.key()).as_deref(), Some(BO));
        assert!(plans.busy);
        assert!(plans.removed.is_empty() && plans.removal_errors.is_empty());
        assert_eq!(plans.removed_note, None);
    });
}

#[gpui_kit::test]
fn a_failed_removal_keeps_the_row_and_says_why_under_it(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let plans = fake.plans(two_logins(), cx);
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    fake.answer_removal(Err("Sign-out timed out. Try again.".into()), cx);
    plans.read_with(cx, |plans, _| {
        assert!(plans.removal.is_none() && !plans.busy);
        assert_eq!(
            plans.removal_errors,
            HashMap::from([(
                BO.to_string(),
                "bo@acme.dev is still on this computer. Sign-out timed out. Try again.".to_string()
            )])
        );
        assert_eq!(plans.problem, None, "the error is the row's, not the page's");
        assert!(plans.removed.is_empty() && plans.removed_note.is_none());
    });
    assert_eq!(shown(&plans, cx), ["claude:own", BO]);
    assert_eq!(fake.saved.get(), 0, "a failed removal changes no setting");
    assert!(fake.probes.borrow().is_empty(), "nothing to read again");
    // Try again runs at once and clears the error; a second failure and Dismiss clear it too.
    plans.update(cx, |plans, cx| {
        plans.retry_remove(SessionAgent::Claude, Some("a_1".into()), cx)
    });
    cx.run_until_parked();
    plans.read_with(cx, |plans, _| {
        assert!(plans.removal.is_some() && plans.removal_errors.is_empty());
    });
    // The first removal answered; the retry is the one running now.
    assert_eq!(fake.asked(), [(SessionAgent::Claude, Some("a_1".into()), None)]);
    fake.answer_removal(Err("No.".into()), cx);
    plans.update(cx, |plans, cx| {
        plans.dismiss_removal_error(SessionAgent::Claude, Some("a_1"), cx)
    });
    plans.read_with(cx, |plans, _| assert!(plans.removal_errors.is_empty()));
}

#[gpui_kit::test]
fn signing_out_the_own_login_moves_new_sessions_and_keeps_the_card(cx: &mut TestAppContext) {
    let fake = Fake::default();
    let plans = fake.plans(two_logins(), cx);
    press_remove(&plans, SessionAgent::Claude, None, cx);
    press_remove(&plans, SessionAgent::Claude, None, cx);
    // New sessions started on Ana's own login: they move to Bo's account in the same operation.
    assert_eq!(fake.asked(), [(SessionAgent::Claude, None, Some("a_1".into()))]);
    let settings = nexus_desktop_core::settings::Settings {
        claude_account: Some("a_1".into()),
        ..Default::default()
    };
    fake.answer_removal(
        Ok(Removed {
            settings: Some(Ok(settings)),
        }),
        cx,
    );
    assert_eq!(fake.saved.get(), 1, "the app hears of the new choice once");
    plans.read_with(cx, |plans, _| {
        assert_eq!(
            plans.removed_note.as_ref().map(|(_, note)| note.as_str()),
            Some(
                "Signed ana@acme.dev out of Claude on this computer. Your Claude settings stay. New sessions start \
                 on bo@acme.dev."
            )
        );
    });
    cx.executor().advance_clock(crate::motion::COLLAPSE);
    cx.run_until_parked();
    // The own login stays, signed out, so the card keeps its agent; the Connect suggestion shows when it is alone.
    let own = plans.read_with(cx, |plans, _| match &plans.plans {
        Load::Ready(list) => list[0].clone(),
        _ => panic!("read"),
    });
    assert!(!own.signed_in && own.account.is_none() && own.email.is_none());
    // A failed settings save is said with the removal; the login is still gone.
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    press_remove(&plans, SessionAgent::Claude, Some("a_1"), cx);
    fake.answer_removal(
        Ok(Removed {
            settings: Some(Err("Disk full.".into())),
        }),
        cx,
    );
    plans.read_with(cx, |plans, _| {
        let note = plans
            .removed_note
            .as_ref()
            .map(|(_, note)| note.clone())
            .unwrap_or_default();
        assert!(note.starts_with("Removed bo@acme.dev from this computer."));
        assert!(note.ends_with("Nexus could not move new sessions to another account: Disk full."));
        assert!(plans.removed.contains_key(BO));
    });
}

#[test]
fn a_check_reads_a_removed_login_again_only_after_the_removal() {
    let before = two_logins();
    let logins = logins_of(&before);
    let mut removed = HashMap::from([(BO.to_string(), 3)]);
    let stale = Progress {
        logins: logins.clone(),
        read: before.clone(),
    };
    // Check 3 started before Bo's account went: it keeps the row's last reading, and the mark stays.
    assert_eq!(stale.over_removed(&before, &mut removed, 3), before);
    assert_eq!(stale.over_removed(&before[..1], &mut removed, 3), before[..1]);
    assert!(removed.contains_key(BO));
    // Check 4 has not read it yet: still no fresh reading.
    let waiting = Progress {
        logins: logins.clone(),
        read: vec![],
    };
    assert_eq!(waiting.over_removed(&before[..1], &mut removed, 4), before[..1]);
    assert!(removed.contains_key(BO));
    // Check 4 reads it signed in again: it shows, and the mark goes.
    let fresh = Progress {
        logins: logins.clone(),
        read: before.clone(),
    };
    assert_eq!(fresh.over_removed(&before[..1], &mut removed, 4), before);
    assert!(removed.is_empty());
    // A later check that no longer lists it (its folder went) clears the mark too.
    let mut removed = HashMap::from([(BO.to_string(), 3)]);
    let gone = Progress {
        logins: logins[..1].to_vec(),
        read: before[..1].to_vec(),
    };
    assert_eq!(gone.over_removed(&before, &mut removed, 4), before[..1]);
    assert!(removed.is_empty());
    assert_eq!(login_key(SessionAgent::Codex, None), "codex:own");
}

#[test]
fn new_sessions_move_off_a_removed_active_login() {
    let file = crate::desktop_settings::TempFile::new("removal-fallback");
    let root = file.path().with_extension("accounts");
    for id in ["a_1", "a_2"] {
        std::fs::create_dir_all(root.join("claude").join(id)).expect("temporary account");
    }
    let plans = vec![
        login(SessionAgent::Claude, None, "ana@acme.dev", true),
        login(SessionAgent::Claude, Some("a_1"), "bo@acme.dev", true),
        login(SessionAgent::Claude, Some("a_2"), "cy@acme.dev", true),
    ];
    let next = |removed: &Option<String>, plans: &[PlanUsage]| {
        replacement(plans, SessionAgent::Claude, removed).and_then(|plan| plan.account.clone())
    };
    let save = |removed: Option<String>, next: Option<String>| {
        file.update(|settings| move_off(settings, Some(&root), SessionAgent::Claude, &removed, next))
            .expect("saves")
    };
    // The active extra account goes: the next signed-in login takes it (here Ana's own, the first listed).
    file.update(|settings| {
        settings.claude_account = Some("a_1".into());
        Ok(())
    })
    .unwrap();
    let removed = Some("a_1".to_string());
    assert_eq!(next(&removed, &plans), None, "the own login is listed first");
    let (saved, moved) = save(removed.clone(), next(&removed, &plans[1..]));
    assert!(moved);
    assert_eq!(saved.claude_account.as_deref(), Some("a_2"));
    // The own login was active and goes: an extra account takes it.
    let (saved, moved) = {
        file.update(|settings| {
            settings.claude_account = None;
            Ok(())
        })
        .unwrap();
        save(None, next(&None, &plans))
    };
    assert!(moved);
    assert_eq!(saved.claude_account.as_deref(), Some("a_1"));
    // No signed-in login is left: the choice clears.
    let signed_out: Vec<PlanUsage> = plans
        .iter()
        .map(|plan| PlanUsage {
            signed_in: false,
            ..plan.clone()
        })
        .collect();
    let removed = Some("a_1".to_string());
    assert_eq!(next(&removed, &signed_out), None);
    let (saved, moved) = save(removed, None);
    assert!(moved);
    assert_eq!(saved.claude_account, None);
    // A login new sessions did not start on changes nothing.
    let (saved, moved) = save(Some("a_2".into()), None);
    assert!(!moved);
    assert_eq!(saved.claude_account, None);
    let _ = std::fs::remove_dir_all(root);
}
