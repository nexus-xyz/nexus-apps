//! Wake-ups an agent sets for itself: Claude Code's `ScheduleWakeup`. It is the fallback for what Nexus cannot watch,
//! such as CI, a deploy or a time of day. A watched `terminal_run` command ([`crate::terminal`]) wakes the agent when
//! it exits, so a command needs no wake-up.
//!
//! The wake-up lives in the agent's process. The workbench follows it from the agent's events, per run:
//! - A call sets it, and a later call replaces it. `stop: true` clears it, and so does a call that failed.
//! - A turn the agent begins on its own at or after its time is the wake-up: that clears it.
//! - It is lost when the run ends first, or when its time passed by its grace ([`Timing::grace`]) with no such turn.
//!   The log then says so once, as a notice for the person ([`lost_notice`]), and the session shows it no more.
//!
//! While one is set, the idle reaper leaves the agent running until that grace after its time
//! (`Workbench::reap_idle`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::event::{Wake, WbEvent};
use crate::{Live, StopReason, Workbench, now_ms};

#[cfg(test)]
#[path = "wake_tests.rs"]
mod tests;

/// Claude Code's tool.
pub const TOOL: &str = "ScheduleWakeup";
/// A turn the agent begins this long before the time is not the wake-up (a background sub-agent's turn, say).
const EARLY: i64 = 15_000;

/// How the workbench reads a wake-up's time ([`crate::Config::wake_timing`]). Tests shorten it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    /// The shortest and longest delay: Claude Code clamps a call's `delaySeconds` to them.
    pub delays: (Duration, Duration),
    /// How long past its time a wake-up may still come. A turn running then delays it: the agent wakes once free.
    pub grace: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            delays: (Duration::from_secs(60), Duration::from_secs(3600)),
            grace: Duration::from_secs(180),
        }
    }
}
/// The characters of the reason a session keeps.
const REASON_MAX_CHARS: usize = 120;

/// A wake-up call, from its input.
#[derive(Debug, PartialEq)]
pub(crate) enum Call {
    Set { delay_ms: i64, reason: String },
    Stop,
}

/// The call `name` makes with `input`, when it is Claude Code's wake-up tool, its delay clamped to `timing`'s. A call
/// without a delay sets nothing.
pub(crate) fn parse(name: &str, input: &Value, timing: Timing) -> Option<Call> {
    if name != TOOL {
        return None;
    }
    if input.get("stop").and_then(Value::as_bool) == Some(true) {
        return Some(Call::Stop);
    }
    let delay = input.get("delaySeconds").and_then(Value::as_f64)?;
    if !delay.is_finite() {
        return None;
    }
    let reason = input
        .get("reason")
        .and_then(Value::as_str)
        .map(clean_reason)
        .unwrap_or_default();
    let (least, most) = (timing.delays.0.as_secs_f64(), timing.delays.1.as_secs_f64());
    Some(Call::Set {
        delay_ms: (delay.clamp(least, most) * 1000.) as i64,
        reason,
    })
}

/// The reason's first line, its whitespace runs as one space, at most [`REASON_MAX_CHARS`] characters.
fn clean_reason(raw: &str) -> String {
    let line = raw.lines().next().unwrap_or_default();
    let text = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= REASON_MAX_CHARS {
        return text;
    }
    let cut: String = text.chars().take(REASON_MAX_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// A turn the agent began on its own at `now` is the wake-up due `at`.
pub(crate) fn fired(at: i64, now: i64) -> bool {
    now >= at - EARLY
}

/// The agent may still wake at `now`: its time has not passed by `grace`.
pub(crate) fn may_come(at: i64, now: i64, grace: Duration) -> bool {
    now < at + grace.as_millis() as i64
}

/// Why a wake-up did not come.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Lost {
    /// Its time passed with no turn.
    Missed,
    /// The run ended first, for this reason; None: its process exited by itself.
    Ended(Option<StopReason>),
}

/// The log's notice for a lost wake-up due `at`, written for the person. `clock` writes a time of day.
pub(crate) fn lost_notice(at: i64, lost: Lost, clock: impl Fn(i64) -> String) -> String {
    let time = clock(at);
    let why = match lost {
        Lost::Missed => {
            return format!("The wake-up the agent set for {time} did not come. Send a message to continue.");
        }
        Lost::Ended(Some(StopReason::Stop | StopReason::Restart)) => "the agent was stopped",
        Lost::Ended(Some(StopReason::Quit)) => "Nexus quit",
        Lost::Ended(Some(StopReason::Idle)) => "Nexus stopped the idle agent",
        Lost::Ended(Some(StopReason::Move)) => "the session moved to a new agent process",
        Lost::Ended(None) => "the agent's process ended",
    };
    format!("The wake-up the agent set for {time} will not come: {why}. Send a message to continue.")
}

/// The local time of day of `at` (epoch milliseconds), as "14:32".
fn local_clock(at: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at)
        .map(|at| at.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// A run's wake-up and the call that set it.
#[derive(Debug)]
pub(crate) struct Pending {
    pub(crate) wake: Wake,
    call_id: String,
    /// Which setting this is: a check armed for an older one finds a newer one and leaves it.
    generation: u64,
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

impl Workbench {
    /// Follows the agent's wake-up calls in its events. A sub-agent's calls are its own: they are not the session's.
    pub(crate) fn observe_wake(&self, thread_id: &str, live: &Live, event: &WbEvent) {
        match event {
            WbEvent::Tool {
                id,
                name,
                input,
                parent: None,
            } => match parse(name, input, self.config().wake_timing) {
                Some(Call::Set { delay_ms, reason }) => {
                    let now = now_ms();
                    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
                    live.state().wake = Some(Pending {
                        wake: Wake {
                            at: now + delay_ms,
                            set_at: now,
                            reason,
                        },
                        call_id: id.clone(),
                        generation,
                    });
                    let grace = self.config().wake_timing.grace;
                    self.check_wake_after(thread_id, generation, Duration::from_millis(delay_ms as u64) + grace);
                }
                Some(Call::Stop) => live.state().wake = None,
                None => {}
            },
            // The call failed: nothing was scheduled.
            WbEvent::ToolResult {
                id,
                is_error: true,
                parent: None,
                ..
            } => {
                let mut state = live.state();
                if state.wake.as_ref().is_some_and(|pending| pending.call_id == *id) {
                    state.wake = None;
                }
            }
            _ => {}
        }
    }

    /// The agent began a turn on its own: at or after its wake-up's time, that turn is the wake-up.
    pub(crate) fn wake_turn_began(&self, live: &Live) {
        let mut state = live.state();
        if state
            .wake
            .as_ref()
            .is_some_and(|pending| fired(pending.wake.at, now_ms()))
        {
            state.wake = None;
        }
    }

    /// The run's wake-up, for the session's summary.
    pub(crate) fn wake_of(&self, thread_id: &str) -> Option<Wake> {
        let live = self.live(thread_id)?;
        let state = live.state();
        state.wake.as_ref().map(|pending| pending.wake.clone())
    }

    /// The run ended with a wake-up set: the log says it will not come. `finish` calls it with the run's last events.
    pub(crate) fn wake_ended(&self, thread_id: &str, live: &Live, pending: Pending, stop: Option<StopReason>) {
        let text = lost_notice(pending.wake.at, Lost::Ended(stop), local_clock);
        if let Err(error) = self.append_from(thread_id, live, WbEvent::Notice { text }) {
            eprintln!("nexus-workbench: {thread_id}: {error}");
        }
    }

    /// Looks at the wake-up set as `generation` once `after` has passed. Without Tokio there is no check: the run's
    /// end still reports a wake-up that did not come.
    fn check_wake_after(&self, thread_id: &str, generation: u64, after: Duration) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let inner = Arc::downgrade(&self.inner);
        let thread_id = thread_id.to_string();
        runtime.spawn(async move {
            tokio::time::sleep(after).await;
            if let Some(inner) = inner.upgrade() {
                Workbench { inner }.check_wake(&thread_id, generation);
            }
        });
    }

    /// The wake-up set as `generation` is past its time and its grace ([`Timing::grace`]). While a turn runs, it looks
    /// again later: the agent wakes once it is free. Otherwise the wake-up did not come, and the log says so.
    fn check_wake(&self, thread_id: &str, generation: u64) {
        // An ended run reported its wake-up as it ended.
        let Some(live) = self.live(thread_id) else {
            return;
        };
        let lost = {
            let mut state = live.state();
            if state.done
                || state
                    .wake
                    .as_ref()
                    .is_none_or(|pending| pending.generation != generation)
            {
                return;
            }
            if !state.turns.is_empty() {
                drop(state);
                self.check_wake_after(thread_id, generation, self.config().wake_timing.grace);
                return;
            }
            state.wake.take()
        };
        let Some(pending) = lost else {
            return;
        };
        let text = lost_notice(pending.wake.at, Lost::Missed, local_clock);
        let noted = self
            .append_from(thread_id, &live, WbEvent::Notice { text })
            // Saved again with a newer time: the session shows without its wake-up, and with a reply to read.
            .and_then(|_| self.change(thread_id, |_| {}));
        if let Err(error) = noted {
            eprintln!("nexus-workbench: {thread_id}: {error}");
        }
    }
}
