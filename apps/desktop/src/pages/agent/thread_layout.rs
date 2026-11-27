//! Where the Assistant's thread puts its centered time lines (brand v5 A1·C·j, A22): one before the first message
//! with a time, and one before a message after 15 minutes of quiet or on a new day, as Messages does.
//! `pages/agent.rs` computes it when the thread changes and reads it in `render`.

use chrono::{DateTime, Datelike as _, Duration, Local};

/// One line of the thread.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// A centered time line: "Today 08:02", "Yesterday 09:14", "Monday 16:02", "Nov 3 16:02".
    Separator(String),
    /// Entry `ix` of the thread.
    Entry(usize),
}

/// A quiet this long or longer starts a new time line.
pub const QUIET: Duration = Duration::minutes(15);

/// The thread's lines for its entries' times, with `now` for the time lines' words.
pub fn layout(times: &[Option<DateTime<Local>>], now: DateTime<Local>) -> Vec<Item> {
    let mut items = Vec::with_capacity(times.len() + 2);
    let mut last_time: Option<DateTime<Local>> = None;
    for (ix, at) in times.iter().enumerate() {
        if let Some(at) = *at {
            let line = match last_time {
                None => true,
                Some(before) => at - before >= QUIET || at.date_naive() != before.date_naive(),
            };
            if line {
                items.push(Item::Separator(label(at, now)));
            }
            last_time = Some(at);
        }
        items.push(Item::Entry(ix));
    }
    items
}

/// A time line's words: the day (Today, Yesterday, the weekday within a week, else the date) and the time.
pub fn label(at: DateTime<Local>, now: DateTime<Local>) -> String {
    let time = at.format("%H:%M");
    let day = at.date_naive();
    let today = now.date_naive();
    match (today - day).num_days() {
        0 => format!("Today {time}"),
        1 => format!("Yesterday {time}"),
        2..=6 => format!("{} {time}", at.format("%A")),
        _ if day.year() == today.year() => format!("{} {time}", at.format("%b %-d")),
        _ => format!("{} {time}", at.format("%b %-d, %Y")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, day, hour, minute, 0)
            .single()
            .expect("a local time")
    }

    #[test]
    fn the_first_message_and_each_after_fifteen_quiet_minutes_or_a_new_day_get_a_time_line() {
        let now = at(18, 12, 0);
        let times = [
            Some(at(16, 16, 2)),
            Some(at(17, 9, 14)),
            Some(at(18, 7, 58)),
            // 14 minutes: no line.
            Some(at(18, 8, 12)),
            // 15 minutes: a line.
            Some(at(18, 8, 27)),
        ];
        assert_eq!(
            layout(&times, now),
            vec![
                Item::Separator("Monday 16:02".into()),
                Item::Entry(0),
                Item::Separator("Yesterday 09:14".into()),
                Item::Entry(1),
                Item::Separator("Today 07:58".into()),
                Item::Entry(2),
                Item::Entry(3),
                Item::Separator("Today 08:27".into()),
                Item::Entry(4),
            ]
        );
    }

    #[test]
    fn turns_read_back_without_a_time_get_no_line_until_one_with_a_time() {
        let now = at(18, 9, 0);
        assert_eq!(
            layout(&[None, None, Some(at(18, 8, 59))], now),
            vec![
                Item::Entry(0),
                Item::Entry(1),
                Item::Separator("Today 08:59".into()),
                Item::Entry(2),
            ]
        );
        assert!(layout(&[], now).is_empty());
    }

    #[test]
    fn a_time_line_names_the_day() {
        let now = at(18, 12, 0);
        assert_eq!(label(at(18, 8, 2), now), "Today 08:02");
        assert_eq!(label(at(17, 23, 59), now), "Yesterday 23:59");
        assert_eq!(label(at(12, 10, 0), now), "Thursday 10:00");
        assert_eq!(label(at(3, 16, 2), now), "Nov 3 16:02");
        let last_year = Local
            .with_ymd_and_hms(2025, 12, 31, 18, 20, 0)
            .single()
            .expect("a local time");
        assert_eq!(label(last_year, now), "Dec 31, 2025 18:20");
    }
}
