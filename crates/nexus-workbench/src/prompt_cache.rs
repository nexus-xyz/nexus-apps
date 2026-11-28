//! How long a session's prompt cache stays warm. The next turn of a Claude Code session costs much less
//! while its prompt cache holds the conversation, so the screens show when the cache goes cold.
//!
//! Anthropic's prompt-caching documentation (https://docs.claude.com/en/docs/build-with-claude/prompt-caching) sets
//! the rules this follows:
//! - A cache entry lives 5 minutes by default. A request can ask for 1 hour instead (`"ttl": "1h"`).
//! - Each call that uses an entry refreshes it at no cost: its lifetime starts again.
//! - A call's `usage.cache_creation` splits what it wrote into `ephemeral_5m_input_tokens` and
//!   `ephemeral_1h_input_tokens` ([`CacheWrites`]).
//!
//! The lifetime starts when the request that reads or writes the entry starts. A usage event records when the call
//! ended, so the countdown can end up to one call's length after the cache really goes cold.
//!
//! Only calls that report the split count. Codex and the Nexus agent report no cache lifetime, so their sessions get
//! no clock. Sub-agents keep caches of their own, so only the session's own agent counts.

use crate::event::{CacheWrites, WbEvent};

/// A cache entry's default lifetime.
pub const FIVE_MINUTES_MS: i64 = 5 * 60 * 1000;
/// The lifetime of an entry written with `"ttl": "1h"`.
pub const ONE_HOUR_MS: i64 = 60 * 60 * 1000;

/// The session's prompt cache, as its usage events tell it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheClock {
    /// When the session's last call that read or wrote the cache ended, in ms. None: no call did, or none said its
    /// cache lifetime.
    last_at: Option<i64>,
    /// The session wrote 1-hour entries.
    one_hour: bool,
}

/// What the clock shows at a moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warmth {
    /// The cache holds the conversation for this many more ms.
    Warm {
        left_ms: i64,
    },
    Cold,
}

impl CacheClock {
    /// The clock after `events`, oldest first, with each event's time in ms.
    pub fn of<'a>(events: impl IntoIterator<Item = (i64, &'a WbEvent)>) -> CacheClock {
        let mut clock = CacheClock::default();
        for (at, event) in events {
            clock.add(at, event);
        }
        clock
    }

    /// Take in one event recorded at `at` ms.
    pub fn add(&mut self, at: i64, event: &WbEvent) {
        let WbEvent::Usage {
            cache_read_tokens,
            cache_writes: Some(writes),
            parent: None,
            ..
        } = event
        else {
            return;
        };
        let CacheWrites {
            five_minute_tokens,
            one_hour_tokens,
        } = *writes;
        if one_hour_tokens > 0 {
            self.one_hour = true;
        }
        // A read refreshes the entries it uses; a write starts new ones.
        if *cache_read_tokens > 0 || five_minute_tokens > 0 || one_hour_tokens > 0 {
            self.last_at = Some(self.last_at.map_or(at, |last| last.max(at)));
        }
    }

    /// How long an entry lives: 1 hour once the session writes 1-hour entries, else 5 minutes.
    pub fn lifetime_ms(&self) -> i64 {
        if self.one_hour { ONE_HOUR_MS } else { FIVE_MINUTES_MS }
    }

    /// When the cache goes cold, in ms; None when the session has no clock.
    pub fn cold_at(&self) -> Option<i64> {
        self.last_at.map(|last| last + self.lifetime_ms())
    }

    /// What the clock shows at `now` ms; None when the session has no clock.
    pub fn at(&self, now: i64) -> Option<Warmth> {
        let left_ms = self.cold_at()? - now;
        Some(if left_ms > 0 {
            Warmth::Warm { left_ms }
        } else {
            Warmth::Cold
        })
    }
}

/// The time left as `m:ss`, rounded up to the second so the countdown reads `0:01` and not `0:00` while warm.
pub fn countdown(left_ms: i64) -> String {
    let seconds = (left_ms.max(0) + 999) / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(read: u64, five_minute: u64, one_hour: u64) -> WbEvent {
        WbEvent::Usage {
            model: "claude-sonnet".into(),
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: read,
            cache_write_tokens: five_minute + one_hour,
            cache_writes: Some(CacheWrites {
                five_minute_tokens: five_minute,
                one_hour_tokens: one_hour,
            }),
            cost_usd: 0.01,
            parent: None,
        }
    }

    const SECOND: i64 = 1000;

    #[test]
    fn a_five_minute_session_counts_down_five_minutes() {
        let start = 1_000_000;
        let events = [(start, call(0, 4_000, 0))];
        let clock = CacheClock::of(events.iter().map(|(at, event)| (*at, event)));
        assert_eq!(clock.lifetime_ms(), FIVE_MINUTES_MS);
        assert_eq!(
            clock.at(start),
            Some(Warmth::Warm {
                left_ms: FIVE_MINUTES_MS
            })
        );
        assert_eq!(
            clock.at(start + 48 * SECOND),
            Some(Warmth::Warm {
                left_ms: 4 * 60 * SECOND + 12 * SECOND
            })
        );
        assert_eq!(clock.at(start + FIVE_MINUTES_MS), Some(Warmth::Cold));
    }

    #[test]
    fn a_one_hour_session_counts_down_an_hour() {
        let start = 1_000_000;
        // Later calls only read: the 1-hour entries written first still set the lifetime.
        let events = [(start, call(0, 0, 4_364)), (start + 10 * SECOND, call(27_864, 0, 0))];
        let clock = CacheClock::of(events.iter().map(|(at, event)| (*at, event)));
        assert_eq!(clock.lifetime_ms(), ONE_HOUR_MS);
        assert_eq!(
            clock.at(start + 10 * SECOND + 30 * 60 * SECOND),
            Some(Warmth::Warm {
                left_ms: 30 * 60 * SECOND
            })
        );
        assert_eq!(clock.at(start + 10 * SECOND + ONE_HOUR_MS), Some(Warmth::Cold));
    }

    #[test]
    fn a_read_restarts_the_countdown() {
        let start = 1_000_000;
        let mut clock = CacheClock::default();
        clock.add(start, &call(0, 4_000, 0));
        let read_at = start + 4 * 60 * SECOND;
        assert_eq!(clock.at(read_at), Some(Warmth::Warm { left_ms: 60 * SECOND }));
        clock.add(read_at, &call(4_000, 0, 0));
        assert_eq!(
            clock.at(read_at),
            Some(Warmth::Warm {
                left_ms: FIVE_MINUTES_MS
            })
        );
        // Warm past the first call's five minutes, cold five minutes after the read.
        assert!(matches!(clock.at(start + FIVE_MINUTES_MS), Some(Warmth::Warm { .. })));
        assert_eq!(clock.at(read_at + FIVE_MINUTES_MS), Some(Warmth::Cold));
        // A call that neither reads nor writes the cache leaves the countdown alone.
        clock.add(read_at + 60 * SECOND, &call(0, 0, 0));
        assert_eq!(clock.cold_at(), Some(read_at + FIVE_MINUTES_MS));
    }

    #[test]
    fn calls_without_a_cache_lifetime_show_no_clock() {
        // Codex reports cached input but no lifetime; a sub-agent's cache is its own.
        let codex = WbEvent::Usage {
            model: "gpt-6".into(),
            input_tokens: 2_705,
            output_tokens: 30,
            cache_read_tokens: 11_520,
            cache_write_tokens: 0,
            cache_writes: None,
            cost_usd: 0.02,
            parent: None,
        };
        let mut sub_agent = call(0, 4_000, 0);
        if let WbEvent::Usage { parent, .. } = &mut sub_agent {
            *parent = Some("toolu_1".into());
        }
        let clock = CacheClock::of([(1_000, &codex), (2_000, &sub_agent)]);
        assert_eq!(clock.at(2_000), None);
    }

    #[test]
    fn the_countdown_reads_minutes_and_seconds() {
        assert_eq!(countdown(4 * 60 * SECOND + 12 * SECOND), "4:12");
        assert_eq!(countdown(4 * 60 * SECOND + 11 * SECOND + 1), "4:12");
        assert_eq!(countdown(ONE_HOUR_MS), "60:00");
        assert_eq!(countdown(1), "0:01");
        assert_eq!(countdown(0), "0:00");
    }

    #[test]
    fn older_usage_events_parse_without_the_split() {
        let old = serde_json::json!({"kind": "usage", "model": "m", "inputTokens": 1, "outputTokens": 2,
            "cacheReadTokens": 3, "cacheWriteTokens": 4, "costUsd": 0.5});
        let event = serde_json::from_value::<WbEvent>(old.clone()).expect("old usage event");
        assert!(matches!(event, WbEvent::Usage { cache_writes: None, .. }));
        assert_eq!(serde_json::to_value(&event).expect("serialized"), old);
        let new = serde_json::json!({"kind": "usage", "model": "m", "inputTokens": 1, "outputTokens": 2,
            "cacheReadTokens": 3, "cacheWriteTokens": 4, "cacheWrites": {"fiveMinuteTokens": 0, "oneHourTokens": 4},
            "costUsd": 0.5});
        let event = serde_json::from_value::<WbEvent>(new.clone()).expect("new usage event");
        assert!(matches!(
            event,
            WbEvent::Usage {
                cache_writes: Some(CacheWrites {
                    five_minute_tokens: 0,
                    one_hour_tokens: 4
                }),
                ..
            }
        ));
        assert_eq!(serde_json::to_value(&event).expect("serialized"), new);
    }
}
