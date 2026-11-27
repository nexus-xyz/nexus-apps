//! What is known of each login's plan limits, from two sources: a probe's readings ([`crate::plans`]) and the limits a
//! running session's agent reports ([`PlanReport`]). The newest of a limit's readings is shown:
//!
//! - A later window replaces an earlier one.
//! - In one window, the reading fetched last wins, a lower one included: a reset or a refill lowers the use.
//! - A session's report is fetched as it arrives, from the headers of the call it made.
//! - A probe's reading is dated when Claude Code's cache says when it fetched it ([`crate::plans::claude_fetched_at`]).
//!   Claude Code answers a probe from that cache, which another `claude` process can write back older.
//! - Between a dated and an undated probe reading in one window, a reading may only raise the limit. An undated
//!   Claude reading did not come from the cache as the file holds it, so it is likely a fetch, but which is newer is
//!   not known. A session's report replaces an undated reading.

use serde::{Deserialize, Serialize};

use crate::event::SessionAgent;
use crate::plans::{Limit, PlanUsage};

/// Two reset times this close apart are one window: Claude Code says a window's reset in whole seconds in its
/// `rate_limit_event` and with microseconds in its `get_usage` answer.
const SAME_WINDOW_MS: i64 = 60_000;

/// The limits a running session's agent reported for the login it runs on: Claude Code's `rate_limit_event`, Codex's
/// `account/rateLimits/updated`. Its labels are a probe's (`Session`, `Weekly`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReport {
    pub agent: SessionAgent,
    /// The agent's extra account; None: its own login.
    pub account: Option<String>,
    pub limits: Vec<Limit>,
    /// When the agent reported them (epoch milliseconds).
    pub at: i64,
}

/// Each login's newest limits.
#[derive(Debug, Default)]
pub struct PlanBook {
    /// By agent and account: a computer has a few logins.
    logins: Vec<((SessionAgent, Option<String>), Known)>,
    changes: u64,
}

#[derive(Debug, Default)]
struct Known {
    /// Who the login was signed in as when it was read; None until a probe reads it.
    email: Option<String>,
    plan: Option<String>,
    limits: Vec<Dated>,
}

/// A limit as read, and when it was fetched when that is known.
#[derive(Debug, Clone)]
struct Dated {
    limit: Limit,
    at: Option<i64>,
    /// A session's report, from the call it made: never a cache.
    reported: bool,
}

impl Known {
    fn is(&self, plan: &PlanUsage) -> bool {
        (self.email.is_none() || self.email == plan.email) && (self.plan.is_none() || self.plan == plan.plan)
    }

    /// `fresh` joins the limit of its label, where it [`replaces`] what is known.
    fn take(&mut self, fresh: Dated) {
        match self.limits.iter_mut().find(|old| old.limit.label == fresh.limit.label) {
            Some(old) if replaces(old, &fresh) => *old = fresh,
            Some(_) => {}
            None => self.limits.push(fresh),
        }
    }
}

impl PlanBook {
    /// A probe's readings. A login now signed in as another person or on another plan starts over. A reading with no
    /// limits (a failed one, or Claude Code's "unavailable" during a turn) changes nothing. A limit the reading no
    /// longer has is gone, such as a model's own week that ended.
    pub fn read(&mut self, plans: &[PlanUsage]) {
        for plan in plans.iter().filter(|plan| plan.signed_in && plan.error.is_none()) {
            let known = self.known_mut(plan.agent, &plan.account);
            if !known.is(plan) {
                *known = Known::default();
            }
            known.email = plan.email.clone();
            known.plan = plan.plan.clone();
            if plan.limits.is_empty() {
                continue;
            }
            // A limit the reading lacks is gone, unless the reading is older than what is known.
            let newest = known.limits.iter().filter_map(|old| old.at).max();
            if plan.fetched_at.zip(newest).is_none_or(|(at, newest)| at >= newest) {
                known
                    .limits
                    .retain(|old| plan.limits.iter().any(|fresh| fresh.label == old.limit.label));
            }
            for fresh in &plan.limits {
                known.take(Dated {
                    limit: fresh.clone(),
                    at: plan.fetched_at,
                    reported: false,
                });
            }
            // In the reading's order.
            known.limits.sort_by_key(|old| {
                plan.limits
                    .iter()
                    .position(|fresh| fresh.label == old.limit.label)
                    .unwrap_or(usize::MAX)
            });
            self.changes += 1;
        }
    }

    /// A running session's report: each limit it names joins the login's.
    pub fn reported(&mut self, report: &PlanReport) {
        let known = self.known_mut(report.agent, &report.account);
        for fresh in &report.limits {
            known.take(Dated {
                limit: fresh.clone(),
                at: Some(report.at),
                reported: true,
            });
        }
        self.changes += 1;
    }

    /// The login's limits are no longer what is known: a Codex limit reset was used, which clears them in their window.
    pub fn forget(&mut self, agent: SessionAgent, account: Option<String>) {
        let before = self.logins.len();
        self.logins.retain(|(login, _)| *login != (agent, account.clone()));
        if self.logins.len() != before {
            self.changes += 1;
        }
    }

    /// `plans`, each signed-in login with the limits known for it.
    pub fn shown(&self, plans: &[PlanUsage]) -> Vec<PlanUsage> {
        plans
            .iter()
            .map(|plan| match self.known(plan.agent, &plan.account) {
                Some(known) if plan.signed_in && plan.error.is_none() && !known.limits.is_empty() && known.is(plan) => {
                    PlanUsage {
                        limits: known.limits.iter().map(|dated| dated.limit.clone()).collect(),
                        ..plan.clone()
                    }
                }
                _ => plan.clone(),
            })
            .collect()
    }

    fn known(&self, agent: SessionAgent, account: &Option<String>) -> Option<&Known> {
        self.logins
            .iter()
            .find(|((each, its), _)| *each == agent && its == account)
            .map(|(_, known)| known)
    }

    fn known_mut(&mut self, agent: SessionAgent, account: &Option<String>) -> &mut Known {
        let at = match self
            .logins
            .iter()
            .position(|((each, its), _)| *each == agent && its == account)
        {
            Some(at) => at,
            None => {
                self.logins.push(((agent, account.clone()), Known::default()));
                self.logins.len() - 1
            }
        };
        &mut self.logins[at].1
    }

    /// Grows each time what is known changes, so a view knows to show it again.
    pub fn changes(&self) -> u64 {
        self.changes
    }
}

/// Whether `fresh` is the newer reading of `old`'s limit (see the module's rules).
fn replaces(old: &Dated, fresh: &Dated) -> bool {
    if let (Some(old_window), Some(fresh_window)) = (old.limit.resets_at, fresh.limit.resets_at) {
        if fresh_window - old_window > SAME_WINDOW_MS {
            return true;
        }
        if old_window - fresh_window > SAME_WINDOW_MS {
            return false;
        }
        return match (old.at, fresh.at) {
            (Some(old_at), Some(fresh_at)) => fresh_at >= old_at,
            (None, Some(_)) if fresh.reported => true,
            _ => fresh.limit.used_percent >= old.limit.used_percent,
        };
    }
    // No window to compare: the later of two dated readings. An undated reading may only raise a dated one, which may
    // be a session's report; else the fresh one.
    match (old.at, fresh.at) {
        (Some(old_at), Some(fresh_at)) => fresh_at >= old_at,
        (Some(_), None) => fresh.limit.used_percent >= old.limit.used_percent,
        (None, _) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESET: i64 = 1_795_017_000_000;
    /// 20:18 and 20:56 UTC on 2026-11-17: when Claude Code fetched the cache it restored, and when it wrote it back.
    const CACHED_AT: i64 = 1_794_946_719_179;
    const FETCHED_AT: i64 = CACHED_AT + 38 * 60_000;

    fn limit(label: &str, used: f64, resets_at: Option<i64>) -> Limit {
        Limit {
            models: Vec::new(),
            label: label.into(),
            used_percent: used,
            resets_at,
        }
    }

    fn reading(email: &str, limits: Vec<Limit>, fetched_at: Option<i64>) -> PlanUsage {
        PlanUsage {
            agent: SessionAgent::Claude,
            account: None,
            email: Some(email.into()),
            plan: Some("Claude Max".into()),
            signed_in: true,
            active: true,
            limits,
            resets: None,
            error: None,
            google_cloud: false,
            unavailable: false,
            fetched_at,
        }
    }

    fn session(used: f64, fetched_at: Option<i64>) -> PlanUsage {
        reading("you@example.com", vec![limit("Session", used, Some(RESET))], fetched_at)
    }

    fn report(used: f64, at: i64) -> PlanReport {
        PlanReport {
            agent: SessionAgent::Claude,
            account: None,
            // A session's report says the window in whole seconds.
            limits: vec![limit("Session", used, Some(RESET / 1000 * 1000 + 58))],
            at,
        }
    }

    fn session_used(book: &PlanBook, plan: &PlanUsage) -> Option<f64> {
        book.shown(std::slice::from_ref(plan))[0]
            .limits
            .iter()
            .find(|limit| limit.label == "Session")
            .map(|limit| limit.used_percent)
    }

    #[test]
    fn restored_cache_loses_to_a_later_fetch() {
        let mut book = PlanBook::default();
        // Settings reads 70% used, fetched at 20:56; a later probe gets the 57% cache another process restored.
        book.read(&[session(70., Some(FETCHED_AT))]);
        let restored = session(57., Some(CACHED_AT));
        book.read(std::slice::from_ref(&restored));
        assert_eq!(session_used(&book, &restored), Some(70.));
    }

    #[test]
    fn later_fetch_shows_a_refill_in_the_same_window() {
        let mut book = PlanBook::default();
        book.read(&[session(70., Some(CACHED_AT))]);
        // The provider gave the usage back: a later fetch says less, in the same window.
        let refilled = session(10., Some(FETCHED_AT));
        book.read(std::slice::from_ref(&refilled));
        assert_eq!(session_used(&book, &refilled), Some(10.));
    }

    #[test]
    fn session_report_replaces_an_older_reading_either_way() {
        let mut book = PlanBook::default();
        let probed = session(57., Some(CACHED_AT));
        book.read(std::slice::from_ref(&probed));
        let before = book.changes();
        // A subagent spends the plan.
        book.reported(&report(67., FETCHED_AT));
        assert!(book.changes() > before);
        assert_eq!(session_used(&book, &probed), Some(67.));
        // After a refill, the session's next call says less.
        book.reported(&report(5., FETCHED_AT + 60_000));
        assert_eq!(session_used(&book, &probed), Some(5.));
        // The restored cache, read again, is older than both.
        book.read(std::slice::from_ref(&probed));
        assert_eq!(session_used(&book, &probed), Some(5.));
        // Another account's report is not this login's.
        book.reported(&PlanReport {
            account: Some("a_1".into()),
            ..report(99., FETCHED_AT + 120_000)
        });
        assert_eq!(session_used(&book, &probed), Some(5.));
    }

    #[test]
    fn dated_cache_does_not_lower_a_fetch_it_cannot_date() {
        let mut book = PlanBook::default();
        // A probe fetched 70% itself, so it is undated; the next probe answers from the 20:18 cache.
        book.read(&[session(70., None)]);
        let cached = session(57., Some(CACHED_AT));
        book.read(std::slice::from_ref(&cached));
        assert_eq!(session_used(&book, &cached), Some(70.));
        // A session's report replaces it either way.
        book.reported(&report(5., FETCHED_AT));
        assert_eq!(session_used(&book, &cached), Some(5.));
    }

    #[test]
    fn older_reading_does_not_drop_a_limit_it_lacks() {
        let mut book = PlanBook::default();
        let both = reading(
            "you@example.com",
            vec![
                limit("Session", 50., Some(RESET)),
                limit("Weekly · Fable", 80., Some(RESET)),
            ],
            Some(FETCHED_AT),
        );
        book.read(std::slice::from_ref(&both));
        let older = session(50., Some(CACHED_AT));
        book.read(std::slice::from_ref(&older));
        assert_eq!(book.shown(std::slice::from_ref(&older))[0].limits, both.limits);
    }

    #[test]
    fn undated_reading_only_raises_a_limit_in_its_window() {
        let mut book = PlanBook::default();
        book.reported(&report(67., FETCHED_AT));
        let lower = session(57., None);
        book.read(std::slice::from_ref(&lower));
        assert_eq!(session_used(&book, &lower), Some(67.));
        let higher = session(80., None);
        book.read(std::slice::from_ref(&higher));
        assert_eq!(session_used(&book, &higher), Some(80.));
    }

    #[test]
    fn next_window_replaces_and_an_earlier_one_does_not() {
        let mut book = PlanBook::default();
        book.read(&[session(70., Some(FETCHED_AT))]);
        let next = reading(
            "you@example.com",
            vec![limit("Session", 4., Some(RESET + 5 * 3_600_000))],
            None,
        );
        book.read(std::slice::from_ref(&next));
        assert_eq!(session_used(&book, &next), Some(4.));
        let earlier = session(90., Some(FETCHED_AT + 60_000));
        book.read(std::slice::from_ref(&earlier));
        assert_eq!(session_used(&book, &earlier), Some(4.));
    }

    #[test]
    fn reading_without_limits_keeps_the_last_ones() {
        let mut book = PlanBook::default();
        let probed = session(57., Some(CACHED_AT));
        book.read(std::slice::from_ref(&probed));
        // Mid-turn, Claude Code says its limits are unavailable.
        let mid_turn = PlanUsage {
            unavailable: true,
            ..reading("you@example.com", vec![], None)
        };
        book.read(std::slice::from_ref(&mid_turn));
        let shown = book.shown(std::slice::from_ref(&mid_turn));
        assert_eq!(shown[0].limits, probed.limits);
        assert!(shown[0].unavailable);

        // A failed reading shows its error, not old limits.
        let failed = PlanUsage {
            error: Some("timed out".into()),
            ..mid_turn.clone()
        };
        assert_eq!(book.shown(std::slice::from_ref(&failed)), [failed]);
        // A plan that never reported limits shows none.
        assert_eq!(PlanBook::default().shown(std::slice::from_ref(&mid_turn)), [mid_turn]);
    }

    #[test]
    fn login_signed_in_as_someone_else_starts_over() {
        let mut book = PlanBook::default();
        book.read(&[session(90., Some(FETCHED_AT))]);
        let other = reading("other@example.com", vec![limit("Session", 10., Some(RESET))], None);
        book.read(std::slice::from_ref(&other));
        assert_eq!(session_used(&book, &other), Some(10.));
        // Another plan on the same email starts over too, and shows its own reading.
        let enterprise = PlanUsage {
            plan: Some("Claude Enterprise".into()),
            ..reading("other@example.com", vec![], None)
        };
        assert_eq!(book.shown(std::slice::from_ref(&enterprise)), [enterprise]);
    }

    #[test]
    fn limit_gone_from_a_reading_is_dropped_and_a_used_reset_forgets_the_login() {
        let mut book = PlanBook::default();
        book.read(&[reading(
            "you@example.com",
            vec![
                limit("Session", 50., Some(RESET)),
                limit("Weekly · Fable", 80., Some(RESET)),
            ],
            None,
        )]);
        let later = session(60., None);
        book.read(std::slice::from_ref(&later));
        assert_eq!(book.shown(std::slice::from_ref(&later))[0].limits, later.limits);

        let cleared = session(0., None);
        book.forget(SessionAgent::Claude, None);
        book.read(std::slice::from_ref(&cleared));
        assert_eq!(session_used(&book, &cleared), Some(0.));
    }
}
