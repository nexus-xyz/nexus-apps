//! The spend cap: what a tree spends at list price since the person
//! last wrote to its root, checked at every model call. It counts every call of a session below the root, and the
//! root's own calls once it has a child, including turns the person started. A call with no list price counts at the catalog's highest
//! (`Host::top_price`), since Codex reports no cost. Crossing the cap trips the tree: it pauses, every turn in it the
//! person did not start is interrupted, and every background sub-agent in it is stopped, with no halt. The person's
//! own turns still run and still spend.

use crate::event::{SpendTrip, Usage};
use crate::{InterruptSource, Live, TaskAsk, Workbench};

pub(crate) const STOP_NOTICE: &str = "Nexus interrupted the automatic turn because the crew spend cap was reached.";

/// Restore the explanation when the process ended before a later prompt reached the agent.
pub(crate) fn notice_pending(events: &[crate::event::StoredEvent]) -> bool {
    use crate::event::WbEvent;
    let Some(index) = events
        .iter()
        .rposition(|row| matches!(&row.event, WbEvent::Notice { text } if text == STOP_NOTICE))
    else {
        return false;
    };
    let later = &events[index + 1..];
    !later.iter().any(|row| match row.event {
        WbEvent::Read { seq } => later.binary_search_by_key(&seq, |event| event.seq).ok().is_some_and(
            |i| matches!(&later[i].event, WbEvent::User { text, .. } if !text.trim_start().starts_with('/')),
        ),
        _ => false,
    })
}

impl Workbench {
    /// A model call of the session was logged: its tree's spend grows, and a tree that crosses its cap trips. In a
    /// tree that already tripped, a call outside the person's turns quiets its session again (a turn that began after
    /// the trip, a first prompt that was on its way).
    pub(crate) fn count_call(&self, thread_id: &str, live: &Live, model: &str, usage: &Usage) {
        if let Err(error) = self.count(thread_id, live, model, usage) {
            eprintln!("nexus-workbench: {thread_id}: {error}");
        }
    }

    fn count(&self, thread_id: &str, live: &Live, model: &str, usage: &Usage) -> crate::Result<()> {
        let person = live.state().turns.front().is_some_and(|turn| turn.person);
        let Some(record) = self.reader().thread(thread_id)? else {
            return Ok(());
        };
        // A root counts once it has a child, and every descendant counts. New sessions get subsessions by default
        // (`Config::subsessions`), so a root with the switch on and no child is a session on its own, not a tree.
        if record.summary.parent_thread_id.is_none() && self.reader().children(thread_id)?.is_empty() {
            return Ok(());
        }
        let root = self.root_of(&record)?;
        let host = &self.inner.host;
        let cost = host
            .price(model, usage)
            .or_else(|| host.top_price(usage))
            .filter(|cost| cost.is_finite() && *cost >= 0.0);
        let (trip, again) = {
            let mut trees = self.trees();
            let tree = trees.entry(root.clone()).or_default();
            let cap = self.config().orchestration.crew_spend_usd;
            tree.spend += cost.or(cap).unwrap_or_default();
            let new_trip = cap.filter(|cap| tree.trip.is_none() && tree.spend >= *cap);
            if let Some(cap_usd) = new_trip {
                tree.trip = Some(SpendTrip {
                    cap_usd,
                    spent_usd: tree.spend,
                });
            }
            (new_trip.is_some(), tree.trip.is_some() && new_trip.is_none() && !person)
        };
        if trip {
            for id in std::iter::once(root.clone()).chain(self.descendants(&root)?.into_iter().map(|r| r.summary.id)) {
                self.quiet(&id, &root);
            }
            self.emit_tree(&root);
        } else if again {
            self.quiet(thread_id, &root);
        }
        Ok(())
    }

    /// In a tripped tree: the session's turn, unless the person started it, is interrupted, and its background
    /// sub-agents are stopped (`wb_task_stop`'s path, Claude Code's and Codex's). Each turn and task is asked once.
    fn quiet(&self, thread_id: &str, root: &str) {
        let Some(live) = self.live(thread_id) else {
            return;
        };
        let (turn, agents) = {
            let state = live.state();
            (
                state
                    .turns
                    .front()
                    .filter(|turn| !turn.person)
                    .map(|turn| turn.id.clone()),
                state
                    .book
                    .running_agents(super::harness(live.agent).linked_policy().children_ready_to_count),
            )
        };
        let (turn, agents) = {
            let mut trees = self.trees();
            let Some(tree) = trees.get_mut(root).filter(|tree| tree.trip.is_some()) else {
                return;
            };
            let asked = &mut tree.asked;
            (
                turn.filter(|turn| asked.insert(turn.clone())),
                agents
                    .into_iter()
                    .filter(|task| asked.insert(format!("{thread_id}/{task}")))
                    .collect::<Vec<_>>(),
            )
        };
        if let Some(turn) = turn
            && let Err(error) = self.stop_turn(thread_id, InterruptSource::Spend(turn))
        {
            eprintln!("nexus-workbench: {thread_id}: {error}");
        }
        for task in agents {
            let (this, id, run, root) = (self.clone(), thread_id.to_string(), live.clone(), root.to_string());
            tokio::spawn(async move {
                if let Err(error) = this.ask_about_task_on(&id, &run, TaskAsk::Stop(&task)).await {
                    eprintln!("nexus-workbench: {id}: {task}: {error}");
                }
                // A later run of the same task must receive its own stop.
                if let Some(tree) = this.trees().get_mut(&root) {
                    tree.asked.remove(&format!("{id}/{task}"));
                }
            });
        }
    }

    /// A task reported after the trip must stop too, including during a person's permitted turn.
    pub(crate) fn quiet_if_tripped(&self, thread_id: &str) {
        if self.tripped(thread_id)
            && let Ok(record) = self.record(thread_id)
            && let Ok(root) = self.root_of(&record)
        {
            self.quiet(thread_id, &root);
        }
    }

    /// The session's tree reached its spend cap since the person last wrote to its root.
    pub(crate) fn tripped(&self, thread_id: &str) -> bool {
        let Ok(Some(record)) = self.reader().thread(thread_id) else {
            return false;
        };
        let Ok(root) = self.root_of(&record) else {
            return false;
        };
        self.trees().get(&root).is_some_and(|tree| tree.trip.is_some())
    }

    /// What the tree of `root` may still spend before it trips.
    pub(crate) fn spend_left(&self, root: &str) -> Option<f64> {
        let cap = self.config().orchestration.crew_spend_usd?;
        let spent = self.trees().get(root).map_or(0.0, |tree| tree.spend);
        Some((cap - spent).max(0.0))
    }

    /// A configured cap needs a price catalog before we admit more work.
    pub(crate) fn missing_spend_prices(&self) -> bool {
        self.config().orchestration.crew_spend_usd.is_some() && self.inner.host.top_price(&Usage::default()).is_none()
    }
}
