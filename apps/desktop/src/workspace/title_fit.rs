//! How an open Code session's top bar fits its width. A crowded bar used to run past the window's edge: its right side
//! kept its full width and cut the branch, the pull request and the Workbench's button. Now the bar sheds what it can
//! in [`Step`]s, one at a time and always in the same order, until its right side fits beside the title's floor.
//!
//! Each frame measures three widths (the bar's row, its right side and the tags beside the title) and shapes the texts
//! that collapse. The parts that never collapse are the right side's measured width less what the collapsing parts
//! drew in that frame, so the step depends on the room alone, never on the step drawn before it. A step that would grow
//! the bar again needs [`SLACK`] spare, so a bar at a step's edge does not flicker between two.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::*;

use super::Workspace;

/// The title keeps this much before the right side sheds anything; a shorter title keeps its own width.
pub(in crate::workspace) const TITLE_FLOOR: f32 = 180.;
/// Extra room a step must have to show more than the step drawn before it.
const SLACK: f32 = 8.;
/// The branch's widest, and its widest once the bar is tight.
pub(in crate::workspace) const BRANCH_MAX: f32 = 220.;
pub(in crate::workspace) const BRANCH_TIGHT: f32 = 120.;
/// A server's name, at its widest.
pub(in crate::workspace) const MACHINE_MAX: f32 = 160.;
/// The row's gap between its parts, and the right side's.
const ROW_GAP: f32 = 8.;
const FACTS_GAP: f32 = 4.;
/// The row's padding: 16 before the title, 4 after the right side.
const ROW_PAD: f32 = 20.;
/// The title's button around its text: its chevron, the gap before it and its padding, less the 6 it reaches back.
const TITLE_CHROME: f32 = 4. + 14. + 12. - 6.;

/// How far the bar has shed, from nothing (0) to its tightest. Each step keeps what the one before it shed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::workspace) struct Step(u8);

impl Step {
    pub(in crate::workspace) const FULL: Step = Step(0);
    pub(in crate::workspace) const TIGHTEST: Step = Step(6);

    fn all() -> impl Iterator<Item = Step> {
        (Self::FULL.0..=Self::TIGHTEST.0).map(Step)
    }

    /// 1: the meter drops the running turn's cost ("· $0.42 this turn"); the card still has it.
    pub(in crate::workspace) fn turn_cost(self) -> bool {
        self.0 < 1
    }

    /// 2: the branch truncates at [`BRANCH_TIGHT`].
    pub(in crate::workspace) fn branch_max(self) -> f32 {
        if self.0 < 2 { BRANCH_MAX } else { BRANCH_TIGHT }
    }

    /// 3: the crew pill says its count alone ("7"), not "7 tasks".
    pub(in crate::workspace) fn crew_words(self) -> bool {
        self.0 < 3
    }

    /// 4: a server's or the cloud's name goes; its icon stays.
    pub(in crate::workspace) fn machine_name(self) -> bool {
        self.0 < 4
    }

    /// 5: the branch's name goes; its icon stays, and the tooltip and the git panel still name it.
    pub(in crate::workspace) fn branch_name(self) -> bool {
        self.0 < 5
    }

    /// 6: the cost goes; the Agents tab still has it.
    pub(in crate::workspace) fn cost(self) -> bool {
        self.0 < 6
    }
}

/// The widths of what collapses, as shaped this frame. Each text is its shaped width, before truncation.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::workspace) struct Parts {
    /// The crew pill's words and its count alone, when the session has a crew.
    pub(in crate::workspace) crew: Option<(f32, f32)>,
    /// The session's cost.
    pub(in crate::workspace) cost: f32,
    /// The running turn's cost line ("· $0.42 this turn"), when a turn runs.
    pub(in crate::workspace) turn: Option<f32>,
    /// Where it runs, when that isn't this computer.
    pub(in crate::workspace) machine: Option<f32>,
    /// The branch, or "Git".
    pub(in crate::workspace) branch: f32,
}

impl Parts {
    /// What the collapsing parts take at `step`, with the gap after each. It mirrors `render_code_facts`.
    pub(in crate::workspace) fn width(&self, step: Step) -> f32 {
        let crew = self.crew.map_or(0., |(words, count)| {
            // Its 4 margin, 8 padding each side, the 13 icon, the 6 gap, then the right side's gap.
            4. + 16. + 13. + 6. + if step.crew_words() { words } else { count } + FACTS_GAP
        });
        let meter = if step.cost() {
            16. + self.cost + self.turn.filter(|_| step.turn_cost()).map_or(0., |turn| 6. + turn) + FACTS_GAP
        } else {
            0.
        };
        // 8 padding each side; the icon and the gap after it for where it runs, with its name when shown; the branch's
        // icon; its name after a gap.
        let machine = self.machine.map_or(0., |name| {
            14. + 6.
                + if step.machine_name() {
                    6. + name.min(MACHINE_MAX)
                } else {
                    0.
                }
        });
        let branch = if step.branch_name() {
            6. + self.branch.min(step.branch_max())
        } else {
            0.
        };
        let git = 16. + machine + 14. + branch + FACTS_GAP;
        crew + meter + git
    }
}

/// The first step whose parts fit in `room`. A step that shows more than `last` needs [`SLACK`] spare.
pub(in crate::workspace) fn fit(parts: &Parts, room: f32, last: Step) -> Step {
    Step::all()
        .find(|step| parts.width(*step) + if *step < last { SLACK } else { 0. } <= room)
        .unwrap_or(Step::TIGHTEST)
}

/// The step the bar draws, and the widths its frames measured.
#[derive(Default)]
pub(in crate::workspace) struct TitleFit {
    pub(in crate::workspace) step: Step,
    /// What the collapsing parts took in the frame drawn last.
    drawn: f32,
    /// The row's width, its right side's, and the tags' beside the title, as last laid out.
    pub(in crate::workspace) row: Rc<Cell<f32>>,
    pub(in crate::workspace) facts: Rc<Cell<f32>>,
    pub(in crate::workspace) tags: Rc<Cell<f32>>,
}

/// What the row has for the right side: its width less its padding, the title's share, the tags and the gaps. `tags`
/// is None when the title stands alone.
pub(in crate::workspace) fn facts_room(row: f32, title: f32, tags: Option<f32>) -> f32 {
    // The title, the tags, the spacer and the right side, with a gap between each.
    let gaps = ROW_GAP * if tags.is_some() { 3. } else { 2. };
    row - ROW_PAD - title.min(TITLE_FLOOR) - tags.unwrap_or(0.) - gaps
}

/// A layer that records its parent's width in `cell` and draws the workspace again when it changed.
pub(in crate::workspace) fn measure(cell: &Rc<Cell<f32>>, cx: &mut Context<Workspace>) -> impl IntoElement {
    let cell = cell.clone();
    let workspace = cx.weak_entity();
    canvas(
        move |bounds, _, cx| {
            let next = f32::from(bounds.size.width).round();
            if cell.get() != next {
                cell.set(next);
                cx.defer(move |cx| {
                    let _ = workspace.update(cx, |_, cx| cx.notify());
                });
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A text's width in the window's font, at `size`, in `family` when given.
fn text_width(window: &mut Window, text: &str, size: f32, family: Option<&'static str>, weight: FontWeight) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let mut style = window.text_style();
    if let Some(family) = family {
        style.font_family = family.into();
    }
    style.font_weight = weight;
    let run = style.to_run(text.len());
    let line = window
        .text_system()
        .shape_line(text.to_string().into(), px(size), &[run], None);
    f32::from(line.width)
}

impl Workspace {
    /// Pick this frame's step for the open Code session's bar, from the last frame's measures and this frame's texts.
    pub(in crate::workspace) fn sync_title_fit(&mut self, window: &mut Window) {
        let super::View::Code(view) = &self.view else {
            return;
        };
        let Some(session) = self.session_by_id(&view.id) else {
            return;
        };
        let place = self.code_place(session);
        let (cost, turn) = self.meter_texts(session);
        let crew = self.crew_pill_words();
        let tags = self.title_has_tags(session);
        let title = self.title.clone();
        let renaming = self
            .renaming
            .as_ref()
            .is_some_and(|renaming| renaming.in_title && renaming.id == session.id);
        let parts = Parts {
            crew: crew.map(|(words, count)| {
                (
                    text_width(window, &words, 11.5, None, FontWeight::MEDIUM),
                    text_width(window, &count, 11.5, None, FontWeight::MEDIUM),
                )
            }),
            cost: text_width(window, &cost, 12., None, FontWeight::NORMAL),
            turn: turn.map(|turn| text_width(window, &format!("· {turn}"), 12., None, FontWeight::NORMAL)),
            machine: place
                .machine
                .as_ref()
                .map(|(_, name)| text_width(window, name, 12., None, FontWeight::NORMAL)),
            branch: text_width(
                window,
                &place.branch_label(),
                12.,
                Some("Geist Mono"),
                FontWeight::NORMAL,
            ),
        };
        let title = if renaming {
            TITLE_FLOOR
        } else {
            text_width(window, &title, 13., None, FontWeight::MEDIUM) + TITLE_CHROME
        };
        let fit = &mut self.title_fit;
        // The parts that never collapse: the right side as drawn, less what the collapsing parts took in that frame.
        let fixed = (fit.facts.get() - fit.drawn).max(0.);
        let room = facts_room(fit.row.get(), title, tags.then(|| fit.tags.get())) - fixed;
        // Before the first layout the row is unknown: draw everything, and fit once it is measured.
        let step = if fit.row.get() > 0. {
            self::fit(&parts, room, fit.step)
        } else {
            Step::FULL
        };
        fit.step = step;
        fit.drawn = parts.width(step);
    }
}

#[cfg(test)]
#[path = "title_fit_tests.rs"]
mod layout_tests;

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's prelude brings its own `test` attribute.
    use super::{Parts, ROW_GAP, ROW_PAD, SLACK, Step, TITLE_FLOOR, facts_room, fit};

    fn crowded() -> Parts {
        Parts {
            crew: Some((40., 7.)),
            cost: 46.,
            turn: Some(90.),
            machine: Some(80.),
            branch: 300.,
        }
    }

    #[test]
    fn each_step_takes_no_more_than_the_one_before() {
        for parts in [crowded(), Parts::default()] {
            let widths: Vec<f32> = Step::all().map(|step| parts.width(step)).collect();
            assert!(widths.windows(2).all(|pair| pair[1] <= pair[0]), "{widths:?}");
        }
    }

    #[test]
    fn every_step_sheds_something_from_a_crowded_bar() {
        let parts = crowded();
        let widths: Vec<f32> = Step::all().map(|step| parts.width(step)).collect();
        assert!(widths.windows(2).all(|pair| pair[1] < pair[0]), "{widths:?}");
    }

    #[test]
    fn a_wide_room_sheds_nothing() {
        assert_eq!(fit(&crowded(), 10_000., Step::FULL), Step::FULL);
    }

    #[test]
    fn a_room_too_small_for_everything_draws_the_tightest() {
        assert_eq!(fit(&crowded(), 0., Step::FULL), Step::TIGHTEST);
        assert_eq!(fit(&crowded(), -50., Step::FULL), Step::TIGHTEST);
    }

    #[test]
    fn it_picks_the_first_step_that_fits() {
        let parts = crowded();
        for step in Step::all() {
            let room = parts.width(step);
            assert_eq!(fit(&parts, room, step), step, "exactly its width, from itself");
            if step > Step::FULL {
                assert_eq!(
                    fit(&parts, room, Step::FULL),
                    step,
                    "exactly its width, from a looser step"
                );
            }
        }
    }

    #[test]
    fn growing_again_needs_slack() {
        let parts = crowded();
        let tight = Step(3);
        let looser = Step(2);
        let room = parts.width(looser);
        // Exactly the looser step's width: from the tighter step, it stays.
        assert_eq!(fit(&parts, room, tight), tight);
        assert_eq!(fit(&parts, room + SLACK, tight), looser);
    }

    #[test]
    fn a_short_branch_and_no_crew_shed_only_what_they_have() {
        let parts = Parts {
            crew: None,
            cost: 40.,
            turn: None,
            machine: None,
            branch: 50.,
        };
        // Nothing to drop for the turn, the branch's cap or the crew: those steps take the same width.
        assert_eq!(parts.width(Step(0)), parts.width(Step(3)));
        assert!(parts.width(Step(5)) < parts.width(Step(4)));
        assert!(parts.width(Step(6)) < parts.width(Step(5)));
    }

    #[test]
    fn the_title_keeps_its_floor_or_its_own_width() {
        assert_eq!(
            facts_room(1000., 400., None),
            1000. - ROW_PAD - TITLE_FLOOR - 2. * ROW_GAP
        );
        assert_eq!(facts_room(1000., 100., None), 1000. - ROW_PAD - 100. - 2. * ROW_GAP);
        assert_eq!(
            facts_room(1000., 100., Some(60.)),
            1000. - ROW_PAD - 100. - 60. - 3. * ROW_GAP
        );
    }
}
