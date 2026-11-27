//! Usage's teams against their caps (`ui/.../nexus-ui/usage/team-caps.tsx`, `usage/model.ts`), for owners and admins
//! (`budgets.manage`): each team with a cap, what it spent and holds in the cap's window, where the window ends at this
//! pace, and whether the team is at its cap, will pass it, or will end at 80% of it or more. Overview's Teams table
//! shows each team's cap on its row, and Limits sums them up. Read only, as on the web: a cap is set on the team's page
//! or with `nexus budget set team/<slug> <amount>/month`.
//!
//! `GET /v1/budgets` holds no team caps, so the page reads `GET /v1/teams`, then `GET /v1/teams/{slug}/budget` for each
//! team, four at a time (`CAP_READS_AT_ONCE`).

use futures::StreamExt;
use serde_json::Value;

use super::usage::{amount, budget_text, cents, share};

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;
/// How many cap reads run at once: each scans the organization's spend since the cap's window began.
const CAP_READS_AT_ONCE: usize = 4;

/// A cap's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Period {
    Day,
    Week,
    Month,
}

impl Period {
    fn parse(text: &str) -> Option<Period> {
        match text {
            "day" => Some(Period::Day),
            "week" => Some(Period::Week),
            "month" => Some(Period::Month),
            _ => None,
        }
    }

    fn window_word(self) -> &'static str {
        match self {
            Period::Day => "today",
            Period::Week => "this week",
            Period::Month => "this month",
        }
    }
}

/// "at": spent and held reach the cap; "over": the estimate passes it; "near": the estimate, or what is spent and
/// held, reaches 80% of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Verdict {
    At,
    Over,
    Near,
    Within,
}

impl Verdict {
    /// The word after a team's cap in Overview's Teams table: "$9,000 · close".
    pub(super) fn word(self) -> &'static str {
        match self {
            Verdict::At => "at cap",
            Verdict::Over => "over pace",
            Verdict::Near => "close",
            Verdict::Within => "within",
        }
    }
}

/// A cap's pace over its window (`capPace`), in dollars.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Pace {
    pub limit: f64,
    pub spent: f64,
    pub reserved: f64,
    pub projected: f64,
    pub period: Period,
    pub resets_at: String,
    pub verdict: Verdict,
}

/// One team's read: its answer, or None when the read failed.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Read {
    pub slug: String,
    pub name: String,
    pub answer: Option<Value>,
}

/// A team with its cap.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Cap {
    pub slug: String,
    pub name: String,
    pub pace: Pace,
}

impl Pace {
    /// The shares of the cap spent, held, and reached by the window's end at this pace, each 0 to 1. A $0 cap is full.
    pub(super) fn shares(&self) -> (f64, f64, f64) {
        if self.limit <= 0. {
            return (1., 0., 1.);
        }
        (
            share(self.spent, self.limit).min(1.),
            share(self.reserved, self.limit),
            share(self.projected, self.limit).min(1.),
        )
    }
}

fn parse_ms(at: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.timestamp_millis())
}

/// A cap's window, ending at its reset: a UTC day, an ISO week, or the month before the reset (`capWindow`).
fn window(period: Period, end: i64) -> (i64, i64) {
    use chrono::{Datelike, TimeZone, Utc};
    match period {
        Period::Day => (end - DAY_MS, end),
        Period::Week => (end - 7 * DAY_MS, end),
        Period::Month => {
            let reset = Utc.timestamp_millis_opt(end).single();
            let start = reset.and_then(|reset| {
                let (year, month) = if reset.month() == 1 {
                    (reset.year() - 1, 12)
                } else {
                    (reset.year(), reset.month() - 1)
                };
                Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).single()
            });
            (
                start.map(|start| start.timestamp_millis()).unwrap_or(end - 30 * DAY_MS),
                end,
            )
        }
    }
}

/// The pace of one `budget` object; None for a member's view (no limit) or a reset that is not a date.
pub(super) fn pace(budget: &Value, now: i64) -> Option<Pace> {
    let limit = amount(budget, "limitUsd")?;
    let spent = amount(budget, "spentUsd").unwrap_or(0.);
    let reserved = amount(budget, "reservedUsd").unwrap_or(0.);
    let period = Period::parse(budget.get("period")?.as_str()?)?;
    let resets_at = budget.get("resetsAt")?.as_str()?.to_string();
    let (start, end) = window(period, parse_ms(&resets_at)?);
    let length = (end - start).max(1) as f64;
    let least = if period == Period::Day { HOUR_MS } else { DAY_MS };
    let elapsed = ((now - start).max(least) as f64).min(length);
    let projected = (spent / elapsed * length * 100.).round() / 100.;
    let used = spent + reserved;
    let verdict = if used >= limit {
        Verdict::At
    } else if projected > limit {
        Verdict::Over
    } else if projected.max(used) * 5. >= limit * 4. {
        Verdict::Near
    } else {
        Verdict::Within
    };
    Some(Pace {
        limit,
        spent,
        reserved,
        projected,
        period,
        resets_at,
        verdict,
    })
}

/// The line under a team's meter (`capNote`).
pub(super) fn note(pace: &Pace) -> String {
    let date = short_date(&pace.resets_at);
    if pace.verdict == Verdict::At {
        return if pace.limit == 0. {
            "A $0 cap: its calls are refused.".into()
        } else if pace.spent < pace.limit {
            "Calls in flight hold the rest. New calls are refused until they settle.".into()
        } else {
            format!("Its calls stop until the cap resets on {date}.")
        };
    }
    let estimate = format!("Estimated {} by {date}.", cents(pace.projected));
    if pace.verdict == Verdict::Over {
        format!("{estimate} Calls stop at the {} cap.", budget_text(pace.limit))
    } else {
        estimate
    }
}

fn short_date(at: &str) -> String {
    super::usage::short_date(at.get(..10).unwrap_or(at))
}

/// The teams with a cap, flagged ones first, then by the share used, then by name; and the teams whose read failed
/// (`capsShown`).
pub(super) fn shown(reads: &[Read], now: i64) -> (Vec<Cap>, Vec<String>) {
    let mut caps = Vec::new();
    let mut failed = Vec::new();
    for read in reads {
        let Some(answer) = &read.answer else {
            failed.push(read.name.clone());
            continue;
        };
        let budget = answer.get("budget").filter(|budget| !budget.is_null());
        match budget.and_then(|budget| pace(budget, now)) {
            Some(pace) => caps.push(Cap {
                slug: read.slug.clone(),
                name: read.name.clone(),
                pace,
            }),
            None if budget.is_some_and(|budget| amount(budget, "limitUsd").is_some()) => failed.push(read.name.clone()),
            None => {}
        }
    }
    let used = |pace: &Pace| {
        if pace.limit > 0. {
            share(pace.spent + pace.reserved, pace.limit)
        } else {
            f64::INFINITY
        }
    };
    caps.sort_by(|a, b| {
        a.pace
            .verdict
            .cmp(&b.pace.verdict)
            .then(used(&b.pace).total_cmp(&used(&a.pace)))
            .then(a.name.cmp(&b.name))
    });
    (caps, failed)
}

/// Every team's cap read (`teamCaps`): the teams, then their caps, four at a time. Fails when every read fails.
pub(super) async fn read_all(gateway: crate::workspace::gateway_access::AccountGateway) -> Result<Vec<Read>, String> {
    let teams = gateway.query("/v1/teams".into()).await?;
    let teams: Vec<(String, String)> = teams
        .get("teams")
        .and_then(Value::as_array)
        .map(|teams| {
            teams
                .iter()
                .filter_map(|team| {
                    let slug = team.get("slug")?.as_str()?.to_string();
                    let name = team.get("name").and_then(Value::as_str).unwrap_or(&slug).to_string();
                    Some((slug, name))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut failure = None;
    let reads: Vec<(Read, Option<String>)> = futures::stream::iter(teams)
        .map(|(slug, name)| {
            let request = gateway.query(format!("/v1/teams/{}/budget", super::usage::encode(&slug)));
            async move {
                match request.await {
                    Ok(answer) => (
                        Read {
                            slug,
                            name,
                            answer: Some(answer),
                        },
                        None,
                    ),
                    Err(error) => (
                        Read {
                            slug,
                            name,
                            answer: None,
                        },
                        Some(error),
                    ),
                }
            }
        })
        .buffered(CAP_READS_AT_ONCE)
        .collect()
        .await;
    let all_failed = !reads.is_empty() && reads.iter().all(|(read, _)| read.answer.is_none());
    let reads: Vec<Read> = reads
        .into_iter()
        .map(|(read, error)| {
            if error.is_some() {
                failure = error;
            }
            read
        })
        .collect();
    match (all_failed, failure) {
        (true, Some(error)) => Err(error),
        _ => Ok(reads),
    }
}

/// A cap's figures: "$62.00 of $100 this month · $3.00 held".
pub(super) fn figures(pace: &Pace) -> String {
    let mut text = format!(
        "{} of {} {}",
        cents(pace.spent),
        budget_text(pace.limit),
        pace.period.window_word()
    );
    if pace.reserved > 0. {
        text.push_str(&format!(" · {} held", cents(pace.reserved)));
    }
    text
}

/// What to say when some teams' caps did not load; None when every read worked.
pub(super) fn failed_text(failed: &[String]) -> Option<String> {
    match failed {
        [] => None,
        [one] => Some(format!("{one}'s cap did not load.")),
        many => Some(format!("The caps of {} teams did not load.", many.len())),
    }
}

/// Limits' line about the caps: how many teams have one, and how many will pass it.
pub(super) fn summary(caps: &[Cap]) -> String {
    if caps.is_empty() {
        return "No team has a cap. A cap stops a team's calls when its spend reaches it.".into();
    }
    let flagged = caps
        .iter()
        .filter(|cap| matches!(cap.pace.verdict, Verdict::At | Verdict::Over))
        .count();
    let teams = if caps.len() == 1 {
        "1 team has a cap".to_string()
    } else {
        format!("{} teams have a cap", caps.len())
    };
    match flagged {
        0 => format!("{teams}. All are within it at this pace."),
        1 => format!("{teams}. 1 is at it or will pass it at this pace."),
        n => format!("{teams}. {n} are at theirs or will pass them at this pace."),
    }
}

#[cfg(test)]
mod tests {
    use super::{Read, Verdict, failed_text, figures, note, pace, shown, summary};
    use serde_json::json;

    fn ms(at: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(at).unwrap().timestamp_millis()
    }

    #[test]
    fn paces_a_month_cap_over_its_window() {
        // The window is Nov 1 to Dec 1; on Nov 16, $60 spent is on course for about $120.
        let budget = json!({ "period": "month", "resetsAt": "2026-12-01T00:00:00Z", "limitUsdDecimal": "100",
                             "spentUsdDecimal": "60", "reservedUsd": 0, "remainingUsd": 40 });
        let month = pace(&budget, ms("2026-11-16T00:00:00Z")).expect("a cap");
        assert_eq!(month.verdict, Verdict::Over);
        assert_eq!(month.projected, 120.);
        assert_eq!(note(&month), "Estimated $120.00 by Dec 1. Calls stop at the $100 cap.");
        let near = json!({ "period": "day", "resetsAt": "2026-11-17T00:00:00Z", "limitUsd": 10, "spentUsd": 8.5 });
        assert_eq!(pace(&near, ms("2026-11-16T23:00:00Z")).unwrap().verdict, Verdict::Near);
        let at = json!({ "period": "week", "resetsAt": "2026-11-23T00:00:00Z", "limitUsd": 10, "spentUsd": 6, "reservedUsd": 4 });
        let at = pace(&at, ms("2026-11-18T00:00:00Z")).unwrap();
        assert_eq!(at.verdict, Verdict::At);
        assert_eq!(
            note(&at),
            "Calls in flight hold the rest. New calls are refused until they settle."
        );
        assert!(
            pace(
                &json!({ "period": "month", "resetsAt": "2026-12-01T00:00:00Z", "remainingUsd": 3 }),
                0
            )
            .is_none()
        );
    }

    #[test]
    fn sums_the_caps_up_for_limits() {
        let now = ms("2026-11-16T00:00:00Z");
        let read = |slug: &str, spent: f64| Read {
            slug: slug.into(),
            name: slug.to_uppercase(),
            answer: Some(
                json!({ "budget": { "period": "month", "resetsAt": "2026-12-01T00:00:00Z", "limitUsd": 100, "spentUsd": spent } }),
            ),
        };
        let (caps, _) = shown(&[read("a", 10.), read("b", 80.)], now);
        assert_eq!(caps[0].slug, "b");
        assert_eq!(
            summary(&caps),
            "2 teams have a cap. 1 is at it or will pass it at this pace."
        );
        assert_eq!(caps[0].pace.verdict.word(), "over pace");
        assert_eq!(figures(&caps[1].pace), "$10.00 of $100 this month");
        assert!(summary(&[]).starts_with("No team has a cap."));
        assert_eq!(failed_text(&[]), None);
        assert_eq!(
            failed_text(&["Core".into()]).as_deref(),
            Some("Core's cap did not load.")
        );
    }

    #[test]
    fn lists_flagged_teams_first_and_names_failed_reads() {
        let now = ms("2026-11-16T00:00:00Z");
        let cap = |spent: f64| json!({ "budget": { "period": "month", "resetsAt": "2026-12-01T00:00:00Z", "limitUsd": 100, "spentUsd": spent } });
        let reads = vec![
            Read {
                slug: "a".into(),
                name: "Alpha".into(),
                answer: Some(cap(10.)),
            },
            Read {
                slug: "b".into(),
                name: "Beta".into(),
                answer: Some(cap(100.)),
            },
            Read {
                slug: "c".into(),
                name: "Core".into(),
                answer: None,
            },
            Read {
                slug: "d".into(),
                name: "Docs".into(),
                answer: Some(json!({ "budget": null })),
            },
        ];
        let (caps, failed) = shown(&reads, now);
        assert_eq!(
            caps.iter().map(|cap| cap.name.as_str()).collect::<Vec<_>>(),
            ["Beta", "Alpha"]
        );
        assert_eq!(failed, ["Core"]);
    }
}
