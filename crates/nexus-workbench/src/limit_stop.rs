//! The row over the composer when the person's plan stopped a turn at its usage limit (`ThreadSummary::limited`):
//! when it shows and what it says. The Nexus app's `workbench/limit-stop.ts` and `plan-usage.ts`'s `resetsOn` hold
//! the same rules; Nexus GPUI reads them here.

use chrono::{DateTime, TimeZone};

use crate::event::{LimitStop, ThreadStatus};

/// The tooltip on "Continue at reset".
pub const SCHEDULE_HINT: &str = "Nexus sends Continue when the limit resets, while this computer is on.";

/// The stop shows while nothing runs: a message, or Continue now, clears it in the core.
pub fn shown(limited: Option<LimitStop>, status: ThreadStatus) -> Option<LimitStop> {
    let busy = matches!(
        status,
        ThreadStatus::Running | ThreadStatus::Starting | ThreadStatus::Approval
    );
    limited.filter(|_| !busy)
}

/// What the row says: the stop, then when the limit resets, or when the session continues by itself. `now` is epoch
/// ms; `zone` is the person's (`chrono::Local` in the app).
pub fn text<Tz: TimeZone>(stop: LimitStop, now: i64, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let stopped = "Your plan's usage limit stopped this turn.";
    let Some(at) = stop.resets_at else {
        return stopped.to_string();
    };
    let when = resets_on(at, now, zone);
    if stop.scheduled {
        format!("{stopped} It continues by itself {when}.")
    } else {
        format!("{stopped} It resets {when}.")
    }
}

/// When a limit resets, in words: "today at 3:40 PM", "tomorrow at 9:00 AM", "Fri at 9:00 AM" within the week, else
/// "Nov 14 at 9:00 AM". `at` and `now` are epoch ms.
pub fn resets_on<Tz: TimeZone>(at: i64, now: i64, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let local = |ms: i64| -> Option<DateTime<Tz>> { zone.timestamp_millis_opt(ms).single() };
    let (Some(when), Some(today)) = (local(at), local(now)) else {
        return String::new();
    };
    let time = when.format("%-I:%M %p");
    let days = (when.date_naive() - today.date_naive()).num_days();
    if days <= 0 {
        format!("today at {time}")
    } else if days == 1 {
        format!("tomorrow at {time}")
    } else if days < 7 {
        format!("{} at {time}", when.format("%a"))
    } else {
        format!("{} at {time}", when.format("%b %-d"))
    }
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    fn zone() -> FixedOffset {
        FixedOffset::west_opt(3 * 3600).expect("three hours west is an offset")
    }

    fn at(day: u32, hour: u32, minute: u32) -> i64 {
        zone()
            .with_ymd_and_hms(2026, 10, day, hour, minute, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn says_when_the_limit_resets_and_when_the_session_continues_by_itself() {
        let (now, resets_at) = (at(28, 12, 0), Some(at(28, 15, 40)));
        assert_eq!(
            text(
                LimitStop {
                    resets_at,
                    scheduled: false
                },
                now,
                &zone()
            ),
            "Your plan's usage limit stopped this turn. It resets today at 3:40 PM."
        );
        assert_eq!(
            text(
                LimitStop {
                    resets_at,
                    scheduled: true
                },
                now,
                &zone()
            ),
            "Your plan's usage limit stopped this turn. It continues by itself today at 3:40 PM."
        );
        assert_eq!(
            text(
                LimitStop {
                    resets_at: None,
                    scheduled: false
                },
                now,
                &zone()
            ),
            "Your plan's usage limit stopped this turn."
        );
    }

    #[test]
    fn names_the_day_by_how_far_off_it_is() {
        // 2026-10-28 is a Wednesday.
        let now = at(28, 12, 0);
        assert_eq!(resets_on(at(28, 9, 5), now, &zone()), "today at 9:05 AM");
        assert_eq!(resets_on(at(29, 0, 30), now, &zone()), "tomorrow at 12:30 AM");
        assert_eq!(resets_on(at(30, 21, 0), now, &zone()), "Fri at 9:00 PM");
        assert_eq!(resets_on(at(31, 13, 0), now, &zone()), "Sat at 1:00 PM");
        let later = zone()
            .with_ymd_and_hms(2026, 11, 4, 9, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(resets_on(later, now, &zone()), "Nov 4 at 9:00 AM");
    }

    #[test]
    fn shows_only_while_nothing_runs() {
        let limited = Some(LimitStop {
            resets_at: Some(at(28, 15, 40)),
            scheduled: true,
        });
        assert_eq!(shown(limited, ThreadStatus::Idle), limited);
        assert_eq!(shown(limited, ThreadStatus::Error), limited);
        assert_eq!(shown(limited, ThreadStatus::Interrupted), limited);
        assert_eq!(shown(limited, ThreadStatus::Running), None);
        assert_eq!(shown(limited, ThreadStatus::Starting), None);
        assert_eq!(shown(limited, ThreadStatus::Approval), None);
        assert_eq!(shown(None, ThreadStatus::Idle), None);
    }
}
