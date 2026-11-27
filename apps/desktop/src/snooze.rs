//! Snoozing a Code session, as the Nexus app does it (`workbench/snooze.ts`): the menu's presets and how a wake time
//! reads. The workbench keeps the wake time (nexus-workbench's `snooze.rs`); a session is snoozed while the clock is
//! before it, and sits in the sidebar's "Snoozed" group until then.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone};

/// The hour "tonight", "tomorrow morning" and "next week" (a Monday) come back at, local time.
const TONIGHT_HOUR: u32 = 19;
const MORNING_HOUR: u32 = 9;
const HOUR_MS: i64 = 60 * 60 * 1000;

/// The snooze menu's choices at `now`, soonest first, with their wake times in ms: in an hour, tonight (left out once
/// it is less than an hour away), tomorrow morning, and next week (Monday morning, a week on when today is Monday).
pub fn presets(now: DateTime<Local>) -> Vec<(&'static str, i64)> {
    let at = |days: i64, hour: u32| {
        let day = (now + Duration::days(days)).date_naive();
        let time = day.and_time(NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or_default());
        Local
            .from_local_datetime(&time)
            .earliest()
            .map(|at| at.timestamp_millis())
            .unwrap_or_else(|| now.timestamp_millis() + days * 24 * HOUR_MS)
    };
    let now_ms = now.timestamp_millis();
    let mut presets = vec![("For 1 hour", now_ms + HOUR_MS)];
    let tonight = at(0, TONIGHT_HOUR);
    if tonight - now_ms > HOUR_MS {
        presets.push(("Until tonight", tonight));
    }
    presets.push(("Until tomorrow morning", at(1, MORNING_HOUR)));
    let to_monday = match (8 - now.weekday().number_from_monday() as i64) % 7 {
        0 => 7,
        days => days,
    };
    presets.push(("Until next week", at(to_monday, MORNING_HOUR)));
    presets
}

/// When a snoozed session comes back: "9:00 AM", "tomorrow, 9:00 AM", "Mon 3 Nov, 9:00 AM".
pub fn wake_text(at: i64, now: DateTime<Local>) -> String {
    let Some(when) = Local.timestamp_millis_opt(at).single() else {
        return String::new();
    };
    let time = when.format("%-I:%M %p").to_string();
    let day = when.date_naive();
    if day == now.date_naive() {
        time
    } else if day == (now + Duration::days(1)).date_naive() {
        format!("tomorrow, {time}")
    } else {
        format!("{}, {time}", when.format("%a %-d %b"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(y: i32, m: u32, d: u32, h: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    #[test]
    fn tonight_drops_within_the_hour() {
        let labels = |now| presets(now).into_iter().map(|(label, _)| label).collect::<Vec<_>>();
        // 2026-11-04 is a Wednesday.
        assert_eq!(
            labels(local(2026, 11, 4, 10)),
            [
                "For 1 hour",
                "Until tonight",
                "Until tomorrow morning",
                "Until next week"
            ]
        );
        assert_eq!(
            labels(local(2026, 11, 4, 18)),
            ["For 1 hour", "Until tomorrow morning", "Until next week"]
        );
    }

    #[test]
    fn next_week_is_a_monday_morning() {
        let week = |now| presets(now).last().unwrap().1;
        assert_eq!(week(local(2026, 11, 4, 10)), local(2026, 11, 9, 9).timestamp_millis());
        // On a Monday, it is the Monday after.
        assert_eq!(week(local(2026, 11, 9, 10)), local(2026, 11, 16, 9).timestamp_millis());
    }

    #[test]
    fn wake_text_names_the_day() {
        let now = local(2026, 11, 4, 10);
        assert_eq!(wake_text(local(2026, 11, 4, 19).timestamp_millis(), now), "7:00 PM");
        assert_eq!(
            wake_text(local(2026, 11, 5, 9).timestamp_millis(), now),
            "tomorrow, 9:00 AM"
        );
        assert_eq!(
            wake_text(local(2026, 11, 9, 9).timestamp_millis(), now),
            "Mon 9 Nov, 9:00 AM"
        );
    }
}
