//! First-run onboarding's saved state. The flow is sign-in, then three optional
//! steps about this computer: Look, Plan and Project. This file keeps which step the flow is on, which steps the person
//! skipped, and whether the flow is done. It lives in this build's settings file (`onboarding`), per computer, as the
//! three steps are. The screens are `workspace/onboarding.rs`.
//!
//! Who sees what:
//! - A person who signs in on a computer with no saved state starts at Look.
//! - A person already signed in when this build first starts never sees it: the state is saved as done.
//! - A flow left half way (quit, crash) resumes at its step at the next launch.
//! - A skipped step stays on the checklist ("Finish setup") until it is done or the checklist is dismissed.
//! - `--onboarding` (or `NEXUS_ONBOARDING=1`) starts the flow again, for a test or a demo.

use serde::{Deserialize, Serialize};

/// One optional step, in the flow's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    Look,
    Plan,
    Project,
}

impl Step {
    pub const ALL: [Step; 3] = [Step::Look, Step::Plan, Step::Project];

    /// The progress bar's label.
    pub fn label(self) -> &'static str {
        match self {
            Step::Look => "Look",
            Step::Plan => "Plan",
            Step::Project => "Project",
        }
    }

    /// The checklist's line for a skipped step.
    pub fn task(self) -> &'static str {
        match self {
            Step::Look => "Choose how Nexus looks",
            Step::Plan => "Link your Claude or ChatGPT plan",
            Step::Project => "Open a project",
        }
    }

    /// Its place in the flow, from 0.
    pub fn index(self) -> usize {
        Step::ALL.iter().position(|step| *step == self).unwrap_or(0)
    }

    /// The step after this one; None after the last.
    pub fn next(self) -> Option<Step> {
        Step::ALL.get(self.index() + 1).copied()
    }

    /// The step before this one; None before the first.
    pub fn prev(self) -> Option<Step> {
        self.index().checked_sub(1).map(|index| Step::ALL[index])
    }
}

/// The saved state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// The step the flow shows; None once it is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<Step>,
    /// The steps the person skipped and has not done since: the checklist.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<Step>,
    /// The flow ended: finished, or set up later.
    #[serde(default)]
    pub done: bool,
    /// The checklist's ✕: it never shows again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dismissed: bool,
}

impl State {
    /// A flow from its first step.
    pub fn start() -> State {
        State {
            step: Some(Step::Look),
            ..State::default()
        }
    }

    /// Done, with nothing left: a person who used the app before onboarding existed.
    fn known() -> State {
        State {
            done: true,
            ..State::default()
        }
    }

    /// The window opens already signed in: the flow resumes when it was left half way; a person with no saved state is
    /// one who used the app before, so the state is saved as done. Answers the state, and the step to show.
    pub fn at_launch(saved: Option<State>) -> (State, Option<Step>) {
        match saved {
            None => (State::known(), None),
            Some(state) => {
                let step = state.pending();
                (state, step)
            }
        }
    }

    /// The person just signed in: the flow starts, or resumes where it was left. Answers the state, and the step.
    pub fn at_sign_in(saved: Option<State>) -> (State, Option<Step>) {
        let state = saved.unwrap_or_else(State::start);
        let step = state.pending();
        (state, step)
    }

    /// The step to show now: None when the flow is done.
    pub fn pending(&self) -> Option<Step> {
        if self.done {
            None
        } else {
            Some(self.step.unwrap_or(Step::Look))
        }
    }

    /// Leave `from`, done or `skipped`, for the next step. Answers the next step; None when the flow just ended.
    pub fn advance(&mut self, from: Step, skipped: bool) -> Option<Step> {
        self.mark(from, skipped);
        let next = from.next();
        self.step = next;
        if next.is_none() {
            self.done = true;
        }
        next
    }

    /// Set up later from `from`: it and every step after it go on the checklist, and the flow ends.
    pub fn later(&mut self, from: Step) {
        for step in Step::ALL.into_iter().skip(from.index()) {
            self.mark(step, true);
        }
        self.step = None;
        self.done = true;
    }

    /// A step opened again from the checklist ends, done or skipped again. The flow stays done.
    pub fn revisited(&mut self, step: Step, skipped: bool) {
        self.mark(step, skipped);
    }

    /// The checklist shows: the flow is done, some step was skipped, and the person did not dismiss it.
    pub fn checklist(&self) -> bool {
        self.done && !self.dismissed && !self.skipped.is_empty()
    }

    fn mark(&mut self, step: Step, skipped: bool) {
        self.skipped.retain(|s| *s != step);
        if skipped {
            self.skipped.push(step);
            self.skipped.sort_by_key(|step| step.index());
        }
    }
}

/// The saved state; None when this computer never saved one.
pub fn load() -> Option<State> {
    crate::paths::onboarding().and_then(|value| serde_json::from_value(value).ok())
}

/// Keep `state`.
pub fn save(state: &State) {
    if let Ok(value) = serde_json::to_value(state) {
        crate::paths::save_onboarding(value);
    }
}

/// The flow was asked for at launch: `--onboarding`, or `NEXUS_ONBOARDING=1`.
pub fn forced() -> bool {
    std::env::args().any(|arg| arg == "--onboarding")
        || std::env::var("NEXUS_ONBOARDING").is_ok_and(|value| value == "1" || value == "true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_person_who_used_the_app_before_never_sees_it() {
        let (state, step) = State::at_launch(None);
        assert_eq!(step, None);
        assert!(state.done);
        assert!(!state.checklist());
    }

    #[test]
    fn a_first_sign_in_starts_at_look() {
        let (state, step) = State::at_sign_in(None);
        assert_eq!(step, Some(Step::Look));
        assert!(!state.done);
    }

    #[test]
    fn a_flow_left_half_way_resumes_at_its_step() {
        let mut state = State::start();
        state.advance(Step::Look, false);
        let (_, step) = State::at_launch(Some(state.clone()));
        assert_eq!(step, Some(Step::Plan));
        let (_, step) = State::at_sign_in(Some(state));
        assert_eq!(step, Some(Step::Plan));
    }

    #[test]
    fn a_finished_flow_shows_nothing_at_the_next_sign_in() {
        let mut state = State::start();
        for step in Step::ALL {
            state.advance(step, false);
        }
        assert!(state.done);
        assert_eq!(State::at_sign_in(Some(state)).1, None);
    }

    #[test]
    fn a_skipped_step_goes_on_the_checklist_and_leaves_it_when_done() {
        let mut state = State::start();
        assert_eq!(state.advance(Step::Look, false), Some(Step::Plan));
        assert_eq!(state.advance(Step::Plan, true), Some(Step::Project));
        assert_eq!(state.advance(Step::Project, false), None);
        assert!(state.done);
        assert_eq!(state.skipped, vec![Step::Plan]);
        assert!(state.checklist());
        state.revisited(Step::Plan, false);
        assert!(state.skipped.is_empty());
        assert!(!state.checklist());
    }

    #[test]
    fn set_up_later_skips_this_step_and_every_one_after_it() {
        let mut state = State::start();
        state.advance(Step::Look, false);
        state.later(Step::Plan);
        assert!(state.done);
        assert_eq!(state.pending(), None);
        assert_eq!(state.skipped, vec![Step::Plan, Step::Project]);
    }

    #[test]
    fn a_dismissed_checklist_stays_hidden() {
        let mut state = State::start();
        state.later(Step::Look);
        state.dismissed = true;
        assert!(!state.checklist());
    }

    #[test]
    fn the_state_round_trips_through_json_and_a_bad_value_reads_as_none() {
        let mut state = State::start();
        state.advance(Step::Look, true);
        let value = serde_json::to_value(&state).expect("json");
        assert_eq!(
            value,
            serde_json::json!({"step": "plan", "skipped": ["look"], "done": false})
        );
        assert_eq!(serde_json::from_value::<State>(value).ok(), Some(state));
        assert!(serde_json::from_value::<State>(serde_json::json!("nonsense")).is_err());
    }

    #[test]
    fn steps_know_their_neighbours() {
        assert_eq!(Step::Look.prev(), None);
        assert_eq!(Step::Look.next(), Some(Step::Plan));
        assert_eq!(Step::Project.next(), None);
        assert_eq!(Step::Project.prev(), Some(Step::Plan));
    }
}
