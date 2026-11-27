//! A cloud session's history from Gateway. Its machine sends Gateway every event its log
//! appends, so a session opens with its messages while the machine sleeps, on any device, and opening it never wakes
//! the machine. The machine's own page and its pushed events (`wb:event`) still come once it is awake:
//! all three carry the machine's event numbers, so [`Followed::events`] keeps one copy of each.
//!
//! - **Opening** reads Gateway's latest page. When it holds events, they show, and older pages come from Gateway too.
//!   A machine connected now is read as well, for what it logged after Gateway's copy; one that connects later is
//!   read then ([`Machines::connected`]).
//! - **Nothing stored** (history turned off for the organization, or a session from before it was stored): the
//!   session reads from its machine as before, which answers only while it is awake.
//! - **Gateway failed**: the machine is asked as before; while it sleeps, the session says why and offers Try again
//!   ([`Machines::read_history_again`]).
//! - **An event over Gateway's per-event cap** keeps its place as a [`OMITTED`] notice, drawn as its own row.

use std::sync::Arc;

use nexus_cloud::desktop::history::{HistoryPage, MAX_PAGE, Page};
use nexus_remote::machine::Status;
use nexus_workbench::event::WbEvent;

use super::{Caller, Followed, Machines, remote_page};

/// How many pages of the machine's [`bridge`] reads, back from its latest, to meet Gateway's copy.
pub(super) const BRIDGE_PAGES: usize = 20;

/// What an event Gateway did not keep (over its per-event cap) shows in its place.
pub const OMITTED: &str = "This item was too large to keep.";

/// What Gateway's history held for a cloud session, as last asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryRead {
    /// The ask is on its way.
    Reading,
    /// Gateway held its events: they show, and its older pages come from Gateway.
    Kept,
    /// Gateway holds none for it yet.
    Empty,
    /// The organization turned cloud history off (`cloudHistory`): only the machine, awake, has it.
    Off,
    /// Gateway could not be read, and why.
    Failed(String),
}

/// Gateway's events as the workbench's, in order: an event this build does not know is left out, as a pushed one
/// would be, and one Gateway did not keep is an [`OMITTED`] notice at its number.
pub(super) fn events_of(page: &HistoryPage) -> Vec<(i64, i64, WbEvent)> {
    page.events
        .iter()
        .filter_map(|event| {
            let read = match &event.body {
                Some(body) => serde_json::from_str::<WbEvent>(body).ok()?,
                None => WbEvent::Notice { text: OMITTED.into() },
            };
            Some((event.seq, event.at, read))
        })
        .collect()
}

impl Followed {
    /// Takes a page of its events: the first latest page (which drops what was pushed before it, older than it), a
    /// later one that catches up, or an older one (`older`). `more` says whether events exist before the page.
    pub(super) fn take_page(&mut self, events: Vec<(i64, i64, WbEvent)>, more: bool, older: bool) {
        let first = events.first().map(|event| event.0);
        if older {
            self.loading_older = false;
            self.more = more;
            // The page met Gateway's last event: older pages are Gateway's again.
            if self.gap && first.is_some_and(|first| first <= self.gateway_last + 1) {
                self.gap = false;
            }
        } else if !self.loaded {
            if let Some(first) = first {
                self.events.retain(|seq, _| *seq >= first);
            }
            self.loading = false;
            self.loaded = true;
            self.error = None;
            self.more = more;
        } else if first.is_some_and(|first| self.events.keys().next().is_none_or(|held| first <= *held)) {
            // A catch-up that reaches as far back as what is held says what is before it.
            self.more = more;
        }
        for (seq, at, event) in events {
            self.events.insert(seq, (at, event));
        }
    }
}

impl Followed {
    /// Whether the machine's latest page, starting at `first`, leaves events between Gateway's last and it unread.
    pub(super) fn hole_before(&self, first: i64) -> bool {
        self.history == Some(HistoryRead::Kept)
            && self.gateway_last > 0
            && first > self.gateway_last + 1
            && !self.events.contains_key(&(self.gateway_last + 1))
    }
}

/// Gateway lags the machine by more than the machine's latest page (after a full-history pause, or a backfill): reads
/// the machine's pages back from `first` (its latest page's start) until they meet Gateway's last event, for the host
/// has no read forward from a number. After [`BRIDGE_PAGES`] pages, or a failed one, the events read so far stay,
/// the older ones are dropped and older pages come from the machine ([`Followed::gap`]) so none is skipped.
pub(super) async fn bridge(machines: &Arc<Machines>, remote: &Caller, machine: &str, thread: &str, first: i64) {
    let mut cursor = first;
    for _ in 0..BRIDGE_PAGES {
        let Ok((events, more)) = remote_page(remote, thread, Some(cursor)).await else {
            break;
        };
        let lowest = events.first().map(|event| event.0);
        let met = machines
            .with_server(machine, |server| {
                let followed = server.followed.entry(thread.to_string()).or_default();
                let met = lowest.is_none_or(|lowest| lowest <= followed.gateway_last + 1) || !more;
                followed.take_page(events, more, false);
                met
            })
            .unwrap_or(true);
        match lowest {
            Some(lowest) if !met => cursor = lowest,
            _ => return,
        }
    }
    machines.with_server(machine, |server| {
        let followed = server.followed.entry(thread.to_string()).or_default();
        followed.events.retain(|seq, _| *seq >= cursor);
        followed.more = true;
        followed.gap = true;
    });
}

impl Machines {
    /// What Gateway's history held for session `thread` on cloud machine `machine`; None before it was asked, on a
    /// server, or while the cloud machines do not run here.
    pub fn history(&self, machine: &str, thread: &str) -> Option<HistoryRead> {
        self.with_server(machine, |server| {
            server
                .followed
                .get(thread)
                .and_then(|followed| followed.history.clone())
        })
        .flatten()
    }

    /// Try again after Gateway failed: the session's next read asks it again.
    pub fn read_history_again(&self, machine: &str, thread: &str) {
        let again = self
            .with_server(machine, |server| {
                let followed = server.followed.get_mut(thread)?;
                if !matches!(followed.history, Some(HistoryRead::Failed(_))) || followed.loaded {
                    return None;
                }
                followed.history = None;
                followed.error = None;
                followed.loading = false;
                Some(())
            })
            .flatten();
        if again.is_some() {
            self.changed(machine);
        }
    }

    /// Reads [`Machines::read_page`]'s page from Gateway when session `thread` is a cloud machine's: true when it
    /// went there (the machine is asked after, when needed), false when only the machine can answer.
    pub(super) fn read_history(&self, machine: &str, thread: &str, before: Option<i64>) -> bool {
        let (Some(cloud), Some(machines)) = (self.cloud_machines(), self.me.upgrade()) else {
            return false;
        };
        if !self.is_cloud(machine) {
            return false;
        }
        let go = self.with_server(machine, |server| {
            let followed = server.followed.entry(thread.to_string()).or_default();
            match (before, &followed.history) {
                // Older pages come from where the session's latest came.
                (Some(_), history) => *history == Some(HistoryRead::Kept) && !followed.gap,
                // Gateway said it holds none: the machine has it.
                (None, Some(HistoryRead::Empty | HistoryRead::Off)) => false,
                (None, _) => {
                    followed.history = Some(HistoryRead::Reading);
                    true
                }
            }
        });
        if go != Some(true) {
            return false;
        }
        let (machine, thread) = (machine.to_string(), thread.to_string());
        self.runtime.spawn(async move {
            match before {
                None => latest(machines, cloud, machine, thread).await,
                Some(before) => older(machines, cloud, machine, thread, before).await,
            }
        });
        true
    }
}

/// Session `thread`'s latest page from Gateway, then the machine's own when Gateway had nothing or the machine is
/// connected now.
async fn latest(machines: Arc<Machines>, cloud: nexus_cloud::desktop::Cloud, machine: String, thread: String) {
    let page = cloud.history(&machine, &thread, Page::Tail, MAX_PAGE).await;
    let connected = matches!(cloud.status(&machine), Status::Connected { .. });
    let last = page
        .as_ref()
        .ok()
        .filter(|page| page.enabled)
        .and_then(|page| page.events.last().map(|event| event.seq));
    let relay = machines
        .with_server(&machine, |server| {
            let followed = server.followed.entry(thread.clone()).or_default();
            match page {
                Ok(page) if page.enabled && !page.events.is_empty() => {
                    followed.history = Some(HistoryRead::Kept);
                    followed.gateway_last = last.unwrap_or(0);
                    followed.take_page(events_of(&page), page.more_before(), false);
                    connected
                }
                Ok(page) => {
                    followed.history = Some(if page.enabled {
                        HistoryRead::Empty
                    } else {
                        HistoryRead::Off
                    });
                    true
                }
                Err(error) => {
                    followed.history = Some(HistoryRead::Failed(error));
                    true
                }
            }
        })
        .unwrap_or(false);
    machines.changed(&machine);
    if relay {
        machines.relay_page(&machine, &thread, None);
    }
}

/// The page of session `thread` before event `before`, from Gateway; from the machine when Gateway no longer has it.
async fn older(
    machines: Arc<Machines>,
    cloud: nexus_cloud::desktop::Cloud,
    machine: String,
    thread: String,
    before: i64,
) {
    match cloud.history(&machine, &thread, Page::Before(before), MAX_PAGE).await {
        Ok(page) if page.enabled => {
            machines.with_server(&machine, |server| {
                let followed = server.followed.entry(thread.clone()).or_default();
                followed.take_page(events_of(&page), page.more_before(), true);
            });
            machines.changed(&machine);
        }
        _ => machines.relay_page(&machine, &thread, Some(before)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_cloud::desktop::history::HistoryEvent;

    fn notice(text: &str) -> WbEvent {
        WbEvent::Notice { text: text.into() }
    }

    fn body(text: &str) -> Option<String> {
        Some(serde_json::to_string(&notice(text)).expect("an event"))
    }

    #[test]
    fn gateways_events_read_as_the_workbenchs_with_a_placeholder_for_one_not_kept() {
        let page = HistoryPage {
            enabled: true,
            first_seq: Some(1),
            last_seq: Some(4),
            events: vec![
                HistoryEvent {
                    seq: 2,
                    at: 20,
                    body: body("two"),
                    omitted_bytes: None,
                },
                HistoryEvent {
                    seq: 3,
                    at: 30,
                    body: None,
                    omitted_bytes: Some(300_000),
                },
                HistoryEvent {
                    seq: 4,
                    at: 40,
                    body: Some(r#"{"kind":"fromTheFuture"}"#.into()),
                    omitted_bytes: None,
                },
            ],
        };
        assert_eq!(
            events_of(&page),
            vec![(2, 20, notice("two")), (3, 30, notice(OMITTED))],
            "the unknown kind is left out, as a pushed one would be"
        );
    }

    #[test]
    fn the_first_page_drops_older_pushes_and_a_second_copy_of_a_number_is_one_event() {
        let mut followed = Followed::default();
        // Pushed before the first page landed: 3 is older than the page, 12 is newer.
        followed.events.insert(3, (3, notice("pushed 3")));
        followed.events.insert(12, (12, notice("pushed 12")));
        followed.take_page(
            (10..=12)
                .map(|seq| (seq, seq, notice(&format!("gateway {seq}"))))
                .collect(),
            true,
            false,
        );
        assert!(followed.loaded && followed.more && !followed.loading);
        assert_eq!(followed.events.keys().copied().collect::<Vec<_>>(), vec![10, 11, 12]);
        assert_eq!(followed.events[&12].1, notice("gateway 12"));

        // The machine wakes: its page overlaps Gateway's and goes past it, and its pushes repeat numbers.
        followed.take_page(
            (11..=14)
                .map(|seq| (seq, seq, notice(&format!("machine {seq}"))))
                .collect(),
            true,
            false,
        );
        followed.events.insert(14, (14, notice("pushed 14")));
        assert_eq!(
            followed.events.keys().copied().collect::<Vec<_>>(),
            vec![10, 11, 12, 13, 14]
        );
        assert!(
            followed.more,
            "a catch-up that starts later than what is held leaves `more` alone"
        );

        // Scrolling back: Gateway's page before 10 reaches the first event.
        followed.loading_older = true;
        followed.take_page((1..10).map(|seq| (seq, seq, notice("older"))).collect(), false, true);
        assert!(!followed.loading_older && !followed.more);
        assert_eq!(followed.events.len(), 14);
        assert_eq!(followed.events.keys().next(), Some(&1));
    }

    #[test]
    fn a_catch_up_that_reaches_back_further_says_what_is_before_it() {
        let mut followed = Followed::default();
        followed.take_page(vec![(5, 5, notice("5"))], true, false);
        followed.take_page(vec![(1, 1, notice("1")), (5, 5, notice("5"))], false, false);
        assert!(!followed.more);
        assert_eq!(followed.events.len(), 2);
    }
}
