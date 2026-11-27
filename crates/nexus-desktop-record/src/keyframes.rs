//! Which moments of a recording become keyframes: the frames an agent looks at first. Ink and pins matter most (the
//! person pointed at something), then clicks and a change of app (something happened), then the end of each thing
//! said (what was on screen while it was said), then the start and the end, and a frame in any long quiet stretch.
//! Moments closer than [`MERGE`] become one; past [`MAX_FRAMES`], the least important and most crowded ones go. Pure.

use super::model::{Event, Phrase};

/// Keyframes per recording: a 3 × 4 contact sheet.
pub const MAX_FRAMES: usize = 12;
/// Two moments closer than this are one keyframe (seconds).
const MERGE: f64 = 1.0;
/// A stretch longer than this with no keyframe gets one in its middle (seconds).
const GAP: f64 = 8.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub t: f64,
    pub reasons: Vec<String>,
    priority: u8,
}

fn moment(t: f64, priority: u8, reason: &str) -> Pick {
    Pick {
        t,
        reasons: vec![reason.to_string()],
        priority,
    }
}

/// The keyframes of a recording `duration` seconds long, in time order.
pub fn choose(duration: f64, events: &[Event], phrases: &[Phrase]) -> Vec<Pick> {
    if duration <= 0.0 {
        return Vec::new();
    }
    let last = (duration - 0.1).max(0.0);
    let mut moments = vec![moment(0.3_f64.min(last), 2, "start"), moment(last, 1, "end")];
    for event in events {
        moments.push(match event {
            // Just after the stroke is finished, before it fades.
            Event::Ink { end, .. } => moment(*end + 0.05, 3, "ink"),
            Event::Pin { t, .. } => moment(*t + 0.15, 3, "pin"),
            // A moment later, to show what the click did.
            Event::Click { t, .. } => moment(*t + 0.5, 2, "click"),
            Event::App { t, .. } => moment(*t + 0.5, 2, "app"),
        });
    }
    for phrase in phrases {
        moments.push(moment(phrase.end, 1, "speech"));
    }
    for pick in &mut moments {
        pick.t = pick.t.clamp(0.0, last);
    }
    moments.sort_by(|a, b| a.t.total_cmp(&b.t));

    let mut picks: Vec<Pick> = Vec::new();
    for next in moments {
        match picks.last_mut() {
            Some(kept) if next.t - kept.t < MERGE => {
                for reason in next.reasons.iter() {
                    if !kept.reasons.contains(reason) {
                        kept.reasons.push(reason.clone());
                    }
                }
                // The more important moment decides the time.
                if next.priority > kept.priority {
                    kept.t = next.t;
                    kept.priority = next.priority;
                }
            }
            _ => picks.push(next),
        }
    }

    let mut filled = Vec::with_capacity(picks.len());
    for pick in picks {
        if let Some(previous) = filled.last().map(|previous: &Pick| previous.t)
            && pick.t - previous > GAP
        {
            filled.push(moment((previous + pick.t) / 2.0, 0, "gap"));
        }
        filled.push(pick);
    }

    while filled.len() > MAX_FRAMES {
        // The least important; among those, the one closest to its neighbours.
        let crowding = |index: usize| {
            let before = index.checked_sub(1).map_or(f64::MAX, |i| filled[index].t - filled[i].t);
            let after = filled.get(index + 1).map_or(f64::MAX, |next| next.t - filled[index].t);
            before.min(after)
        };
        let Some(drop) = (0..filled.len()).min_by(|&a, &b| {
            filled[a]
                .priority
                .cmp(&filled[b].priority)
                .then(crowding(a).total_cmp(&crowding(b)))
        }) else {
            break;
        };
        filled.remove(drop);
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reasons(picks: &[Pick]) -> Vec<(f64, Vec<&str>)> {
        picks
            .iter()
            .map(|pick| {
                (
                    (pick.t * 100.0).round() / 100.0,
                    pick.reasons.iter().map(String::as_str).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn ink_clicks_and_speech_each_get_a_frame() {
        let events = vec![
            Event::Ink {
                t: 2.0,
                end: 3.0,
                points: vec![],
            },
            Event::Click { t: 5.0, at: [0.0, 0.0] },
        ];
        let phrases = vec![Phrase {
            start: 6.0,
            end: 7.5,
            text: "this one".into(),
        }];
        let picks = choose(10.0, &events, &phrases);
        assert_eq!(
            reasons(&picks),
            vec![
                (0.3, vec!["start"]),
                (3.05, vec!["ink"]),
                (5.5, vec!["click"]),
                (7.5, vec!["speech"]),
                (9.9, vec!["end"]),
            ]
        );
    }

    #[test]
    fn close_moments_merge_and_the_ink_keeps_its_time() {
        let events = vec![
            Event::Click { t: 1.9, at: [0.0, 0.0] },
            Event::Ink {
                t: 1.0,
                end: 2.5,
                points: vec![],
            },
        ];
        let picks = choose(3.0, &events, &[]);
        assert_eq!(
            reasons(&picks),
            vec![(0.3, vec!["start"]), (2.55, vec!["click", "ink", "end"])]
        );
    }

    #[test]
    fn a_long_quiet_stretch_gets_a_frame_and_the_count_is_capped() {
        let picks = choose(30.0, &[], &[]);
        assert_eq!(
            reasons(&picks),
            vec![(0.3, vec!["start"]), (15.1, vec!["gap"]), (29.9, vec!["end"])]
        );

        let clicks: Vec<Event> = (0..40)
            .map(|i| Event::Click {
                t: f64::from(i) * 1.5,
                at: [0.0, 0.0],
            })
            .collect();
        let mut events = clicks;
        events.push(Event::Pin {
            t: 20.0,
            n: 1,
            at: [0.0, 0.0],
        });
        let picks = choose(62.0, &events, &[]);
        assert_eq!(picks.len(), MAX_FRAMES);
        assert!(picks.iter().any(|pick| pick.reasons.contains(&"pin".to_string())));
        assert!(picks.windows(2).all(|pair| pair[0].t < pair[1].t));
    }

    #[test]
    fn nothing_for_an_empty_recording() {
        assert!(choose(0.0, &[], &[]).is_empty());
        assert_eq!(choose(0.2, &[], &[]).len(), 1);
    }
}
