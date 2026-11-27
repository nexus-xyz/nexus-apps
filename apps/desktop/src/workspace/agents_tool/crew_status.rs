//! The crew outside the Agents tab: the top bar's pill and the line under the conversation while the lead waits on
//! its sub-agents and subsessions.
use super::super::glyph;
use super::super::side_pane::Tool;
use super::*;
use crate::palette::Palette;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;

/// What the top bar's pill shows. None while the session has no sub-agents and no background commands.
#[derive(Debug, PartialEq)]
pub(super) struct Pill {
    /// "2 agents running", or "3 tasks" once all have ended.
    label: String,
    /// Something still works: a spinner in place of the agent icon.
    running: bool,
    /// An agent needs you: the ring's hue.
    ring: Option<Hsla>,
}

pub(super) fn pill(crew: &Crew) -> Option<Pill> {
    let counts = &crew.counts;
    (counts.total > 0).then(|| Pill {
        label: crew_fold::pill_label(counts),
        running: counts.agents_running + counts.shells_running > 0,
        ring: model::waiting_hue(crew),
    })
}

/// The pill's count alone, for a tight bar: what still runs while something does, else every task.
fn short_count(counts: &nexus_workbench::crew_fold::CrewCounts) -> u64 {
    match counts.agents_running + counts.shells_running {
        0 => counts.total,
        running => running,
    }
}

/// "Waiting on 2 sub-agents and 1 subsession, one needs you": live `agents` (the harness's own) and `sessions`
/// (subsessions), `needs` nodes that need the person.
pub(super) fn waiting_line(agents: usize, sessions: usize, needs: usize) -> String {
    let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let what = match (agents, sessions) {
        (0, sessions) => count(sessions, "subsession", "subsessions"),
        (agents, 0) => count(agents, "sub-agent", "sub-agents"),
        (agents, sessions) => format!(
            "{} and {}",
            count(agents, "sub-agent", "sub-agents"),
            count(sessions, "subsession", "subsessions")
        ),
    };
    let live = agents + sessions;
    let who = format!("Waiting on {what}");
    match needs {
        0 => who,
        1 if live == 1 => format!("{who}, it needs you"),
        1 => format!("{who}, one needs you"),
        n => format!("{who}, {n} need you"),
    }
}

/// Whether the line under the conversation says the lead waits on its crew (`crewWaits`): while sub-agents live, and
/// the lead's turn has ended or the approval on screen is one of theirs. It takes the working line's place then.
pub(super) fn crew_waits(crew: &Crew, working: bool, approval: bool) -> bool {
    crew.counts.agents_running > 0 && if approval { !crew.needs.is_empty() } else { !working }
}

impl Workspace {
    /// Whether the open session's line ticks for the crew: its clock shows while sub-agents live.
    pub(in crate::workspace) fn crew_clock_shown(&self) -> bool {
        self.crew().is_some_and(|crew| crew.counts.agents_running > 0)
    }

    /// The top bar pill's words ("7 tasks") and its count alone ("7"), while the session has a crew.
    pub(in crate::workspace) fn crew_pill_words(&self) -> Option<(String, String)> {
        let crew = self.crew()?;
        let pill = pill(crew)?;
        Some((pill.label, short_count(&crew.counts).to_string()))
    }

    /// The top bar's pill: a spinner and "2 agents running" while they work, a plain count once they are done, and a
    /// ring in the waiting agent's hue while one needs you. It opens the side pane on the Agents tab. A tight bar
    /// shows the count alone (`words` false); the tooltip says what it counts.
    pub(in crate::workspace) fn render_crew_pill(
        &self,
        words: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let crew = self.crew()?;
        let mut pill = pill(crew)?;
        let tip: SharedString = if words {
            crate::key_bindings::with_hint("Show them in the side pane", Tool::Agents.shortcut_id()).into()
        } else {
            let tip = crate::key_bindings::with_hint(
                &format!("{}: show them in the side pane", pill.label),
                Tool::Agents.shortcut_id(),
            );
            pill.label = short_count(&crew.counts).to_string();
            tip.into()
        };
        let icon = if pill.running {
            Icon::new(Lucide::LoaderCircle)
                .size(px(12.))
                .text_color(palette.foreground)
                .with_animation(
                    "crew-pill-spinner",
                    Animation::new(Duration::from_millis(800))
                        .repeat()
                        .with_max_fps(crate::motion::LOOP_FPS),
                    |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
                )
                .into_any_element()
        } else {
            glyph(Lucide::Bot, palette.icon).size(px(13.)).into_any_element()
        };
        Some(
            h_flex()
                .id("code-crew-pill")
                .flex_none()
                .mr(px(4.))
                .h(px(22.))
                .px(px(8.))
                .gap(px(6.))
                .rounded_full()
                .bg(palette.hover)
                .when_some(pill.ring, |pill, hue| pill.border(px(1.5)).border_color(hue))
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if pill.running {
                    palette.foreground
                } else {
                    palette.muted
                })
                .cursor_pointer()
                .hover(|style| style.bg(palette.selected).text_color(palette.foreground))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(cx.listener(|this, _, window, cx| this.pick_tool(Tool::Agents, window, cx)))
                .child(icon)
                .child(pill.label)
                .into_any_element(),
        )
    }

    /// The line under the conversation while the lead waits on its sub-agents and subsessions: three dots, "Waiting
    /// on 3 sub-agents, one needs you" and the time since the earliest live one started. None when [`crew_waits`] is
    /// not.
    pub(in crate::workspace) fn render_crew_waiting(
        &self,
        working: bool,
        approval: bool,
        palette: Palette,
    ) -> Option<AnyElement> {
        let crew = self.crew().filter(|crew| crew_waits(crew, working, approval))?;
        let (agents, sessions) = model::live_split(crew);
        if agents + sessions == 0 {
            return None;
        }
        let dot = |at: fn(Div) -> Div| at(div().absolute().size(px(5.)).rounded_full().bg(palette.faint));
        let dots = div()
            .relative()
            .flex_none()
            .size(px(16.))
            .child(dot(|d| d.top(px(1.)).left(px(5.5))))
            .child(dot(|d| d.bottom(px(2.)).left(px(1.))))
            .child(dot(|d| d.bottom(px(2.)).right(px(1.))));
        let since = model::live_since(crew).map(|since| crew_fold::elapsed(crate::code::now_ms() - since));
        Some(
            h_flex()
                .pt(px(4.))
                .gap(px(8.))
                .text_sm()
                .text_color(palette.muted)
                .child(dots)
                .child(waiting_line(agents, sessions, crew.needs.len()))
                .children(since.map(|since| div().text_color(palette.faint).child(format!("· {since}"))))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{crew_waits, pill, waiting_line};
    use nexus_workbench::crew_fold::{CrewCounts, fold_crew};
    use nexus_workbench::event::SessionAgent;

    #[test]
    fn the_waiting_line_counts_sub_agents_and_who_needs_you() {
        assert_eq!(waiting_line(1, 0, 0), "Waiting on 1 sub-agent");
        assert_eq!(waiting_line(3, 0, 0), "Waiting on 3 sub-agents");
        assert_eq!(waiting_line(1, 0, 1), "Waiting on 1 sub-agent, it needs you");
        assert_eq!(waiting_line(3, 0, 1), "Waiting on 3 sub-agents, one needs you");
        assert_eq!(waiting_line(3, 0, 2), "Waiting on 3 sub-agents, 2 need you");
    }

    #[test]
    fn the_waiting_line_names_subsessions_apart_from_sub_agents() {
        assert_eq!(waiting_line(0, 1, 0), "Waiting on 1 subsession");
        assert_eq!(waiting_line(0, 1, 1), "Waiting on 1 subsession, it needs you");
        assert_eq!(waiting_line(0, 2, 0), "Waiting on 2 subsessions");
        assert_eq!(
            waiting_line(1, 2, 1),
            "Waiting on 1 sub-agent and 2 subsessions, one needs you"
        );
    }

    #[test]
    fn the_pill_counts_what_runs_then_what_ran() {
        let mut crew = fold_crew(&[], SessionAgent::Claude);
        assert_eq!(pill(&crew), None);
        let label = |crew: &mut nexus_workbench::crew_fold::Crew, agents, shells, total| {
            crew.counts = CrewCounts {
                agents_running: agents,
                shells_running: shells,
                total,
                ..CrewCounts::default()
            };
            pill(crew).map(|pill| (pill.label, pill.running))
        };
        assert_eq!(label(&mut crew, 2, 0, 2), Some(("2 agents running".into(), true)));
        assert_eq!(
            label(&mut crew, 1, 1, 3),
            Some(("1 agent and 1 command running".into(), true))
        );
        assert_eq!(label(&mut crew, 0, 1, 1), Some(("1 command running".into(), true)));
        assert_eq!(label(&mut crew, 0, 0, 3), Some(("3 tasks".into(), false)));
        assert_eq!(label(&mut crew, 0, 0, 1), Some(("1 task".into(), false)));
    }

    #[test]
    fn when_the_crew_line_takes_the_working_lines_place() {
        let mut crew = fold_crew(&[], SessionAgent::Claude);
        crew.counts.agents_running = 2;
        // The lead's turn ended: the crew line shows.
        assert!(crew_waits(&crew, false, false));
        // The lead works: its own working line shows.
        assert!(!crew_waits(&crew, true, false));
        // An approval nobody in the crew holds: "Waiting for you" stays.
        assert!(!crew_waits(&crew, true, true));
        crew.needs.push("a".into());
        assert!(crew_waits(&crew, true, true));
        crew.counts.agents_running = 0;
        assert!(!crew_waits(&crew, false, false));
    }
}
