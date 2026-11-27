//! The Sessions group's rules, as the web's sidebar draws its Sessions (Nexus UI's `sidebar-nav.ts`): 12 sessions,
//! then Show more lists up to 50, then All sessions lists the rest. Past 12, headings group them: Today, This week and
//! Older. Its head folds it to the newest four. With four or fewer, nothing folds, so the head is a plain title.

use chrono::{Local, TimeZone};

/// What the group lists before Show more (`SESSIONS_SHOWN`).
pub(super) const SHOWN: usize = 12;
/// What Show more lists before All sessions (`SESSIONS_MORE`).
pub(super) const MORE: usize = 50;
/// What a folded group keeps (`SESSIONS_FOLDED`).
pub(super) const FOLDED: usize = 4;

/// How far the person opened the list: the first 12, Show more's 50, or every one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Step {
    #[default]
    Few,
    More,
    All,
}

/// The link under the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Link {
    ShowMore,
    AllSessions,
}

impl Link {
    pub(super) fn label(self) -> &'static str {
        match self {
            Link::ShowMore => "Show more",
            Link::AllSessions => "All sessions",
        }
    }
}

/// Whether the head folds the group: only when folding would hide a session.
pub(super) fn foldable(total: usize) -> bool {
    total > FOLDED
}

/// How many of `total` sessions show, and the link under them. A search shows every match with no link. Folded, the
/// newest four. Open: 12, then Show more; 50, then All sessions; then all of them.
pub(super) fn shown(total: usize, filtering: bool, folded: bool, step: Step) -> (usize, Option<Link>) {
    if filtering {
        return (total, None);
    }
    if folded && foldable(total) {
        return (FOLDED, None);
    }
    let (limit, link) = match step {
        Step::Few => (SHOWN, Link::ShowMore),
        Step::More => (MORE, Link::AllSessions),
        Step::All => return (total, None),
    };
    if total > limit {
        (limit, Some(link))
    } else {
        (total, None)
    }
}

/// An age heading over the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Age {
    Today,
    ThisWeek,
    Older,
}

impl Age {
    pub(super) fn label(self) -> &'static str {
        match self {
            Age::Today => "Today",
            Age::ThisWeek => "This week",
            Age::Older => "Older",
        }
    }
}

/// The age heading of a session that moved at `at_ms`: Today is the local calendar day, This week the six days before
/// it, Older the rest. A session with no time is Older.
pub(super) fn age_of<Zone: TimeZone>(at_ms: Option<i64>, now_ms: i64, zone: &Zone) -> Age {
    let start_of = |ms: i64| {
        zone.timestamp_millis_opt(ms)
            .single()
            .and_then(|at| at.date_naive().and_hms_opt(0, 0, 0))
            .and_then(|midnight| zone.from_local_datetime(&midnight).earliest())
            .map(|midnight| midnight.timestamp_millis())
    };
    let (Some(at), Some(today)) = (at_ms, start_of(now_ms)) else {
        return Age::Older;
    };
    if at >= today {
        Age::Today
    } else if at >= today - 6 * 86_400_000 {
        Age::ThisWeek
    } else {
        Age::Older
    }
}

/// Whether the list carries age headings: only past 12 sessions, and never folded or while searching.
pub(super) fn headed(total: usize, filtering: bool, folded: bool) -> bool {
    !filtering && !(folded && foldable(total)) && total > SHOWN
}

/// [`age_of`] in this computer's time zone.
pub(super) fn local_age(at_ms: Option<i64>, now_ms: i64) -> Age {
    age_of(at_ms, now_ms, &Local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    #[test]
    fn the_list_opens_as_the_web_one_does() {
        assert_eq!(shown(30, false, false, Step::Few), (12, Some(Link::ShowMore)));
        assert_eq!(shown(30, false, false, Step::More), (30, None));
        assert_eq!(shown(80, false, false, Step::More), (50, Some(Link::AllSessions)));
        assert_eq!(shown(80, false, false, Step::All), (80, None));
        assert_eq!(shown(12, false, false, Step::Few), (12, None));
        assert_eq!(shown(80, true, true, Step::Few), (80, None));
    }

    #[test]
    fn a_fold_keeps_the_newest_four_and_never_does_nothing() {
        assert_eq!(shown(30, false, true, Step::More), (4, None));
        // Four or fewer: nothing to fold, so the head is a plain title and the list stays whole.
        assert!(!foldable(4));
        assert!(foldable(5));
        assert_eq!(shown(2, false, true, Step::Few), (2, None));
    }

    #[test]
    fn headings_show_only_past_twelve() {
        assert!(!headed(12, false, false));
        assert!(headed(13, false, false));
        assert!(!headed(13, false, true));
        assert!(!headed(13, true, false));
    }

    #[test]
    fn ages_follow_the_local_calendar_day() {
        let zone = FixedOffset::west_opt(3 * 3600).unwrap();
        let at = |d: u32, h: u32| zone.with_ymd_and_hms(2026, 11, d, h, 0, 0).unwrap().timestamp_millis();
        let now = at(13, 9);
        assert_eq!(age_of(Some(at(13, 0)), now, &zone), Age::Today);
        assert_eq!(age_of(Some(at(12, 23)), now, &zone), Age::ThisWeek);
        assert_eq!(age_of(Some(at(7, 0)), now, &zone), Age::ThisWeek);
        assert_eq!(age_of(Some(at(6, 23)), now, &zone), Age::Older);
        assert_eq!(age_of(None, now, &zone), Age::Older);
    }
}
