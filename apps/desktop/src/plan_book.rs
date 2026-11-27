//! The app's one [`PlanBook`]: the composer's usage donut and Settings › Subscriptions show the limits it knows for
//! this computer's logins. Probes read into it on the UI thread; the workbench's reports ([`Emit::Plan`]) reach it
//! from its own threads. A remote computer's plans do not go through it.
//!
//! [`Emit::Plan`]: nexus_workbench::Emit::Plan

use nexus_workbench::event::SessionAgent;
use nexus_workbench::plan_book::{PlanBook, PlanReport};
use nexus_workbench::plans::PlanUsage;

/// The book, and the last reading of each login, so a launch can read a login's limits without a probe.
#[derive(Default)]
struct Book {
    book: PlanBook,
    readings: Vec<PlanUsage>,
}

impl Book {
    fn keep(&mut self, plans: &[PlanUsage]) {
        for plan in plans {
            match self
                .readings
                .iter_mut()
                .find(|kept| kept.agent == plan.agent && kept.account == plan.account)
            {
                Some(kept) => *kept = plan.clone(),
                None => self.readings.push(plan.clone()),
            }
        }
    }
}

/// A probe's fresh readings, and the logins as now known. Only readings go in: what a view showed is not one, and
/// learning it again would undo [`forget`].
pub(crate) fn read(plans: &[PlanUsage]) -> Vec<PlanUsage> {
    with(|book| {
        book.keep(plans);
        book.book.read(plans);
        book.book.shown(plans)
    })
}

/// A probe's fresh readings, for a view that shows more than they cover.
pub(crate) fn learn(plans: &[PlanUsage]) {
    with(|book| {
        book.keep(plans);
        book.book.read(plans);
    });
}

/// `plans` with what is known now.
pub(crate) fn shown(plans: &[PlanUsage]) -> Vec<PlanUsage> {
    with(|book| book.book.shown(plans))
}

/// The login's limits as known now (`account`: None for the agent's own login); None when this app never read it.
pub(crate) fn current(agent: SessionAgent, account: &Option<String>) -> Option<PlanUsage> {
    with(|book| {
        let reading = book
            .readings
            .iter()
            .find(|plan| plan.agent == agent && &plan.account == account)?
            .clone();
        book.book.shown(&[reading]).into_iter().next()
    })
}

pub(crate) fn reported(report: &PlanReport) {
    with(|book| book.book.reported(report));
}

pub(crate) fn forget(agent: SessionAgent, account: Option<String>) {
    with(|book| {
        book.readings
            .retain(|plan| !(plan.agent == agent && plan.account == account));
        book.book.forget(agent, account);
    });
}

/// Grows when what is known changes.
pub(crate) fn changes() -> u64 {
    with(|book| book.book.changes())
}

#[cfg(not(test))]
fn with<T>(f: impl FnOnce(&mut Book) -> T) -> T {
    static BOOK: std::sync::LazyLock<std::sync::Mutex<Book>> = std::sync::LazyLock::new(Default::default);
    f(&mut BOOK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// Each test thread has its own, so tests running together never see each other's plans.
#[cfg(test)]
fn with<T>(f: impl FnOnce(&mut Book) -> T) -> T {
    thread_local! {
        static BOOK: std::cell::RefCell<Book> = Default::default();
    }
    BOOK.with(|book| f(&mut book.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use nexus_workbench::event::SessionAgent;
    use nexus_workbench::plans::{Limit, PlanUsage};

    fn plan(account: Option<&str>, used: f64) -> PlanUsage {
        PlanUsage {
            agent: SessionAgent::Claude,
            account: account.map(String::from),
            email: Some("ada@example.com".into()),
            plan: Some("Claude Max".into()),
            signed_in: true,
            active: false,
            limits: vec![Limit {
                models: Vec::new(),
                label: "Session".into(),
                used_percent: used,
                resets_at: None,
            }],
            resets: None,
            error: None,
            google_cloud: false,
            unavailable: false,
            fetched_at: None,
        }
    }

    #[test]
    fn a_launch_reads_each_login_s_last_limits_and_none_for_a_login_never_read() {
        assert_eq!(super::current(SessionAgent::Claude, &None), None);
        super::read(&[plan(None, 20.0), plan(Some("work"), 100.0)]);
        let own = super::current(SessionAgent::Claude, &None).expect("read");
        assert_eq!(own.limits[0].used_percent, 20.0);
        let work = super::current(SessionAgent::Claude, &Some("work".into())).expect("read");
        assert_eq!(work.limits[0].used_percent, 100.0);
        super::learn(&[plan(None, 70.0)]);
        let again = super::current(SessionAgent::Claude, &None).expect("read");
        assert_eq!(again.limits[0].used_percent, 70.0);
        assert_eq!(super::current(SessionAgent::Codex, &None), None);
    }

    #[test]
    fn a_forgotten_login_has_no_reading_until_it_is_read_again() {
        super::read(&[plan(Some("work"), 100.0), plan(None, 30.0)]);
        super::forget(SessionAgent::Claude, Some("work".into()));
        assert_eq!(super::current(SessionAgent::Claude, &Some("work".into())), None);
        assert!(super::current(SessionAgent::Claude, &None).is_some());
        super::read(&[plan(Some("work"), 0.0)]);
        let fresh = super::current(SessionAgent::Claude, &Some("work".into())).expect("read again");
        assert_eq!(fresh.limits[0].used_percent, 0.0);
    }
}
