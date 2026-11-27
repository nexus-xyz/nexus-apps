//! Read-only here: while the Nexus app runs the Code sessions (`owner.rs`), a setup line over the composer names who
//! runs them and offers Take over, the same call as Settings' ownership banner (`Workspace::take_over`). The composer
//! stays dimmed under it, keeping its text, and comes alive in place over `UI_LONG` once this window runs them.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace};
use crate::motion::{self, Phase};
use crate::owner::Owner;
use crate::palette::Palette;
use crate::setup::{self, Fix, SetupLine, Tone};

/// The line over a session's composer, and its fix's key.
pub(super) const CODE_KEY: &str = "code:read-only";
/// The line that stands in for a new cloud session's composer.
pub(super) const CLOUD_KEY: &str = "cloud:read-only";
/// The composer's opacity while it is read-only.
const DIM: f32 = 0.5;

/// Which sessions the line is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Runs {
    Code,
    Cloud,
}

impl Runs {
    fn name(self) -> &'static str {
        match self {
            Runs::Code => "Code",
            Runs::Cloud => "Cloud",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Runs::Code => CODE_KEY,
            Runs::Cloud => CLOUD_KEY,
        }
    }
}

/// What the line says for `owner`, and whether it offers Take over.
#[derive(Clone, Debug, PartialEq)]
struct Words {
    what: String,
    why: Option<String>,
    tone: Tone,
    take_over: bool,
}

fn words(owner: &Owner, runs: Runs) -> Words {
    let name = runs.name();
    let line = |what: String, why: Option<String>, tone: Tone, take_over: bool| Words {
        what,
        why,
        tone,
        take_over,
    };
    match owner {
        Owner::Elsewhere { waited: false } => {
            let (what, why) = match runs {
                Runs::Code => (
                    "The Nexus app runs Code sessions on this computer.",
                    "This window can only read them.",
                ),
                Runs::Cloud => (
                    "The Nexus app runs Cloud sessions while it is open.",
                    "This window can't start them.",
                ),
            };
            line(what.into(), Some(why.into()), Tone::Fixable, true)
        }
        Owner::Elsewhere { waited: true } => line(
            format!("The Nexus app still runs {name} sessions."),
            Some("Quit it. If it keeps agents running in the background, turn that off in its settings first.".into()),
            Tone::Fixable,
            true,
        ),
        Owner::TakingOver { .. } => line(
            "Waiting for the Nexus app to quit.".into(),
            Some(format!("{name} sessions move here as soon as it closes.")),
            Tone::Fixable,
            false,
        ),
        Owner::Failed(error) => line(
            format!("{name} sessions can't open here."),
            Some(error.clone()),
            Tone::Failed,
            false,
        ),
        // Nobody can take over yet: no organization's store was tried.
        Owner::Unknown | Owner::Here => line(
            format!("{name} sessions aren't open in this window yet."),
            Some("They open once you're signed in to an organization.".into()),
            Tone::Info,
            false,
        ),
    }
}

/// Who runs the sessions instead of this window, as a setup line with Take over when the Nexus app can let go.
pub(super) fn owner_line(owner: &Owner, runs: Runs) -> SetupLine {
    let words = words(owner, runs);
    let mut line = SetupLine::new(runs.key(), Lucide::Monitor, words.what).tone(words.tone);
    if let Some(why) = words.why {
        line = line.why(why);
    }
    if words.take_over {
        line = line.fix("Take over", Fix::TakeOver);
    }
    line
}

impl Workspace {
    /// The read-only line over the composer shows in this view: an action refused there needs no note of its own.
    fn shows_read_only_line(&self) -> bool {
        match self.view {
            View::Code(_) => true,
            View::NewCode => self.owner.note().is_some(),
            _ => false,
        }
    }

    /// An action that needs this window to run the Code sessions was refused. Where the read-only line shows, it
    /// already says why and offers Take over; elsewhere the composer's note says it.
    pub(super) fn refuse_read_only(&mut self) {
        if !self.shows_read_only_line() {
            self.composer_note = Some(self.read_only_note());
        }
    }

    /// A session's dock: `composer`, with the read-only line over it and dimmed while `read_only`. When this window
    /// takes the sessions over, the line leaves over `UI_EXIT` and the composer brightens over `UI_LONG`, in place.
    pub(super) fn render_owned_dock(
        &self,
        read_only: bool,
        composer: AnyElement,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fixer = self.fixer(cx);
        let line = read_only.then(|| owner_line(&self.owner, Runs::Code));
        let line = setup::slot("code-read-only", line, &fixer, palette, cx);
        let (opacity, cover, pump) = match motion::presence_for("code-read-only-dim", read_only, motion::UI_LONG, cx) {
            Some(Phase::Open { .. }) => (DIM, true, None),
            Some(Phase::Leaving { since, epoch }) => {
                let t = (since.elapsed().as_secs_f32() / motion::UI_LONG.as_secs_f32()).clamp(0., 1.);
                (DIM + (1. - DIM) * motion::ease_out(t), false, Some(epoch))
            }
            None => (1., false, None),
        };
        let composer = div()
            .relative()
            .w_full()
            .opacity(opacity)
            .child(composer)
            // A read-only composer keeps its text but takes no clicks.
            .when(cover, |dock| {
                dock.child(div().id("code-read-only-cover").absolute().inset_0().occlude())
            })
            // Frames while it brightens: the fade reads the clock.
            .when_some(pump, |dock, epoch| {
                dock.child(div().absolute().size_0().with_animation(
                    ElementId::NamedInteger("code-read-only-brighten".into(), epoch as u64),
                    Animation::new(motion::UI_LONG),
                    |pump, _| pump,
                ))
            });
        v_flex()
            .w_full()
            .gap(px(8.))
            .children(line)
            .child(composer)
            .into_any_element()
    }

    /// The new session's read-only line, under its composer, while the Nexus app runs the sessions.
    pub(super) fn render_home_read_only(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let fixer = self.fixer(cx);
        // With Cloud picked, the cloud line stands in for the composer and says it once (`render_cloud_instead`).
        let cloud_says_it = self.cloud_ui.on && self.code_form.starting.is_none() && crate::cloud::get().is_none();
        let line = self
            .owner
            .note()
            .filter(|_| !cloud_says_it)
            .map(|_| owner_line(&self.owner, Runs::Code));
        setup::slot("home-read-only", line, &fixer, palette, cx)
    }

    /// The cloud card while this window does not run the cloud machines (`machines::NOT_HERE`).
    pub(super) fn render_cloud_read_only(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let fixer = self.fixer(cx);
        owner_line(&self.owner, Runs::Cloud)
            .render(&fixer, palette, cx)
            .into_any_element()
    }
}

/// This window runs the sessions now: the read-only lines' fix is done.
pub(super) fn took_over(window: &mut Window, cx: &mut App) {
    window.defer(cx, |window, cx| {
        setup::done(&CODE_KEY.into(), window, cx);
        setup::done(&CLOUD_KEY.into(), window, cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use std::time::Instant;

    #[test]
    fn the_nexus_app_running_the_sessions_offers_take_over() {
        let words = words(&Owner::Elsewhere { waited: false }, Runs::Code);
        assert_eq!(words.what, "The Nexus app runs Code sessions on this computer.");
        assert_eq!(words.tone, Tone::Fixable);
        assert!(words.take_over);
        let cloud = super::words(&Owner::Elsewhere { waited: false }, Runs::Cloud);
        assert!(cloud.what.starts_with("The Nexus app runs Cloud sessions"));
        assert!(cloud.take_over);
    }

    #[test]
    fn a_take_over_that_ran_out_of_time_says_what_still_holds_them_and_offers_again() {
        let words = words(&Owner::Elsewhere { waited: true }, Runs::Code);
        assert_eq!(words.what, "The Nexus app still runs Code sessions.");
        assert!(words.why.is_some_and(|why| why.starts_with("Quit it.")));
        assert!(words.take_over);
    }

    #[test]
    fn while_taking_over_the_line_waits_with_no_button() {
        let until = Instant::now();
        let words = words(&Owner::TakingOver { until }, Runs::Cloud);
        assert_eq!(words.what, "Waiting for the Nexus app to quit.");
        assert!(!words.take_over);
    }

    #[test]
    fn what_nobody_can_take_over_is_quiet_or_a_failure_with_no_button() {
        let unknown = words(&Owner::Unknown, Runs::Code);
        assert_eq!(unknown.tone, Tone::Info);
        assert!(!unknown.take_over);
        let failed = words(&Owner::Failed("Could not open the store.".into()), Runs::Code);
        assert_eq!(failed.tone, Tone::Failed);
        assert_eq!(failed.why.as_deref(), Some("Could not open the store."));
        assert!(!failed.take_over);
    }

    #[test]
    fn each_line_has_its_own_key() {
        let owner = Owner::Elsewhere { waited: false };
        assert_eq!(owner_line(&owner, Runs::Code).key().as_ref(), CODE_KEY);
        assert_eq!(owner_line(&owner, Runs::Cloud).key().as_ref(), CLOUD_KEY);
    }
}
