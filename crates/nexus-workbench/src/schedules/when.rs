//! When a schedule runs: every so many minutes, at a time on chosen weekdays, or only when started (its webhook or
//! Run now). Pure functions over a time zone, so the tests give a fixed one and the app gives this computer's
//! ([`chrono::Local`]).

use chrono::{Datelike, Duration, LocalResult, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};

/// The shortest interval: a run every few minutes would spend more than it saves.
pub const MIN_INTERVAL_MINUTES: u32 = 10;
/// The longest interval: one week.
pub const MAX_INTERVAL_MINUTES: u32 = 7 * 24 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum When {
    /// Every `minutes`, counted from the schedule's anchor (when it was saved).
    Interval { minutes: u32 },
    /// At `minute` after local midnight on `days` (0 is Sunday, 6 is Saturday).
    Weekly { days: Vec<u8>, minute: u16 },
    /// Only when its webhook or Run now starts it.
    Manual,
}

impl When {
    /// The same times, checked: the reason the Settings row shows, or the days sorted and without repeats.
    pub fn checked(self) -> Result<When, String> {
        match self {
            When::Interval { minutes } if !(MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES).contains(&minutes) => Err(
                format!("An interval is {MIN_INTERVAL_MINUTES} minutes to one week ({MAX_INTERVAL_MINUTES} minutes)."),
            ),
            When::Weekly { mut days, minute } => {
                days.sort_unstable();
                days.dedup();
                if days.is_empty() || days.iter().any(|day| *day > 6) {
                    return Err("Choose at least one day of the week.".into());
                }
                if minute >= 24 * 60 {
                    return Err("The time is between 00:00 and 23:59.".into());
                }
                Ok(When::Weekly { days, minute })
            }
            when => Ok(when),
        }
    }
}

/// The first run strictly after `after` (epoch milliseconds), or None when nothing but a start runs it. An interval
/// keeps its cadence from `anchor`: runs the app missed are not made up one by one.
pub fn next_after<Tz: TimeZone>(when: &When, anchor: i64, after: i64, tz: &Tz) -> Option<i64> {
    match when {
        When::Manual => None,
        When::Interval { minutes } => {
            let step = i64::from((*minutes).max(1)) * 60_000;
            if after < anchor {
                return Some(anchor.saturating_add(step));
            }
            let steps = (after - anchor) / step + 1;
            Some(anchor.saturating_add(steps.saturating_mul(step)))
        }
        When::Weekly { days, minute } => {
            let today = local_date(after, tz)?;
            // Eight days: today's time may have passed, and the same weekday comes again in seven.
            (0..=7).find_map(|offset| {
                let date = today.checked_add_signed(Duration::days(offset))?;
                if !days.contains(&(date.weekday().num_days_from_sunday() as u8)) {
                    return None;
                }
                at_local(date, *minute, tz).filter(|at| *at > after)
            })
        }
    }
}

/// The local calendar day of `at`, as a day number: what the daily cap counts by.
pub fn day_of<Tz: TimeZone>(at: i64, tz: &Tz) -> i32 {
    local_date(at, tz).map_or(0, |date| date.num_days_from_ce())
}

fn local_date<Tz: TimeZone>(at: i64, tz: &Tz) -> Option<NaiveDate> {
    tz.timestamp_millis_opt(at)
        .single()
        .map(|time| time.naive_local().date())
}

/// `minute` after midnight on `date`, local time. A time a clock change skips runs an hour later; a time it repeats
/// runs the first time.
fn at_local<Tz: TimeZone>(date: NaiveDate, minute: u16, tz: &Tz) -> Option<i64> {
    let time = date.and_hms_opt(u32::from(minute / 60), u32::from(minute % 60), 0)?;
    match tz.from_local_datetime(&time) {
        LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => Some(at.timestamp_millis()),
        LocalResult::None => tz
            .from_local_datetime(&(time + Duration::hours(1)))
            .earliest()
            .map(|at| at.timestamp_millis()),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, Utc};

    use super::*;

    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    /// Thursday 2026-10-29 00:00 UTC.
    const THURSDAY: i64 = 1_793_232_000_000;

    #[test]
    fn an_interval_keeps_its_cadence_and_skips_what_the_app_missed() {
        let every = When::Interval { minutes: 30 };
        let anchor = THURSDAY;
        assert_eq!(next_after(&every, anchor, anchor, &Utc), Some(anchor + 30 * MINUTE));
        assert_eq!(
            next_after(&every, anchor, anchor + 29 * MINUTE, &Utc),
            Some(anchor + 30 * MINUTE)
        );
        // Exactly at a run: the next one.
        assert_eq!(
            next_after(&every, anchor, anchor + 30 * MINUTE, &Utc),
            Some(anchor + 60 * MINUTE)
        );
        // Closed for five hours: one run next, on the cadence, not ten.
        assert_eq!(
            next_after(&every, anchor, anchor + 5 * HOUR + MINUTE, &Utc),
            Some(anchor + 5 * HOUR + 30 * MINUTE)
        );
        // A clock before the anchor still gives the first run.
        assert_eq!(
            next_after(&every, anchor, anchor - DAY, &Utc),
            Some(anchor + 30 * MINUTE)
        );
    }

    #[test]
    fn a_weekly_time_is_local_and_comes_on_its_days() {
        // Mondays and Thursdays at 09:00, in UTC-3.
        let weekly = When::Weekly {
            days: vec![1, 4],
            minute: 9 * 60,
        };
        let tz = FixedOffset::west_opt(3 * 3600).unwrap_or_else(|| panic!("offset"));
        // Thursday 00:00 UTC is Wednesday 21:00 there: Thursday 09:00 local is 12:00 UTC.
        assert_eq!(next_after(&weekly, 0, THURSDAY, &tz), Some(THURSDAY + 12 * HOUR));
        // Just after it: Monday 09:00 local.
        assert_eq!(
            next_after(&weekly, 0, THURSDAY + 12 * HOUR, &tz),
            Some(THURSDAY + 4 * DAY + 12 * HOUR)
        );
        // One day only, its time passed today: the same day next week.
        let thursdays = When::Weekly {
            days: vec![4],
            minute: 9 * 60,
        };
        assert_eq!(
            next_after(&thursdays, 0, THURSDAY + 13 * HOUR, &tz),
            Some(THURSDAY + 7 * DAY + 12 * HOUR)
        );
    }

    #[test]
    fn manual_never_runs_by_itself() {
        assert_eq!(next_after(&When::Manual, THURSDAY, THURSDAY, &Utc), None);
    }

    #[test]
    fn the_day_is_the_local_calendar_day() {
        let tz = FixedOffset::west_opt(3 * 3600).unwrap_or_else(|| panic!("offset"));
        // 02:00 UTC Thursday is 23:00 Wednesday in UTC-3.
        assert_eq!(day_of(THURSDAY + 2 * HOUR, &tz) + 1, day_of(THURSDAY + 4 * HOUR, &tz));
        assert_eq!(day_of(THURSDAY + 2 * HOUR, &Utc), day_of(THURSDAY + 4 * HOUR, &Utc));
    }

    #[test]
    fn bad_times_are_refused_and_days_are_tidied() {
        assert!(When::Interval { minutes: 5 }.checked().is_err());
        assert!(
            When::Interval {
                minutes: MAX_INTERVAL_MINUTES + 1
            }
            .checked()
            .is_err()
        );
        assert!(
            When::Weekly {
                days: vec![],
                minute: 0
            }
            .checked()
            .is_err()
        );
        assert!(
            When::Weekly {
                days: vec![7],
                minute: 0
            }
            .checked()
            .is_err()
        );
        assert!(
            When::Weekly {
                days: vec![1],
                minute: 1440
            }
            .checked()
            .is_err()
        );
        assert_eq!(
            When::Weekly {
                days: vec![5, 1, 5],
                minute: 0
            }
            .checked(),
            Ok(When::Weekly {
                days: vec![1, 5],
                minute: 0
            })
        );
    }
}
