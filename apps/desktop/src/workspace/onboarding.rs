//! First-run onboarding: after sign-in, three optional steps about this computer, then
//! the app. Its saved state and its rules are `crate::onboarding`.
//!
//! - Look: theme, text size, transparency (its presets and Fine-tune), conversation width, reduce motion and open at
//!   login, beside a live screen of the app. The theme, the text size, the transparency and the motion apply to the
//!   whole window at once; the width shows on the screen.
//! - Plan: Settings › Agents and plans, embedded, so linking a Claude, ChatGPT or Google AI plan runs the same sign-in.
//! - Project: the projects on this computer, or a new folder.
//!
//! Every step can be skipped; "Set up later" skips the rest. A skipped step stays on the sidebar's checklist ("Finish
//! setup"), which opens that step again. The person menu's "Set up Nexus" runs the whole flow again.
//!
//! Motion: the progress bar's current step fills (SLOW), and each step's blocks rise in 60 ms apart (`motion::rise`). The
//! app arrives after the last step as the sign-in's does: the sidebar opens and the page rises. Less motion shows each
//! state at once.

use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::chat_width::{CH_PX, ChatWidth, Preset};

use super::Workspace;
use super::settings_view::theme_cards;
use crate::frost::Transparency;
use crate::motion;
use crate::onboarding::{State, Step};
use crate::palette::Palette;

/// The welcome mark, as the sign-in's.
pub(super) const MARK: f32 = 56.;
/// How far apart a step's blocks rise in.
const STAGGER: Duration = Duration::from_millis(60);
/// The text sizes Look offers, as zooms: Small, Default, Large.
const SIZES: [(&str, f32); 3] = [("Small", 0.9), ("Default", 1.), ("Large", 1.1)];
/// The widths Look offers. Custom stays in Settings › Appearance, with its sliders.
const WIDTHS: [(&str, Preset); 3] = [
    ("Standard", Preset::Standard),
    ("Wide", Preset::Wide),
    ("Fit window", Preset::FitWindow),
];

/// The flow on screen.
struct Flow {
    step: Step,
    /// Opened from the checklist: one step, then back to the app.
    revisit: bool,
    /// Which arrival this is, for the blocks' rise.
    run: usize,
    /// Plan's embedded Agents and plans page, built when the step opens.
    plans: Option<AnyView>,
    /// Project's pick: a project id.
    pick: Option<String>,
}

/// Onboarding's part of the workspace.
pub(super) struct Onboarding {
    /// The saved state; None while this computer has none (a first sign-in starts the flow).
    state: Option<State>,
    flow: Option<Flow>,
    runs: usize,
    /// The app arrived after the flow: its run, for the page's rise.
    landing: Option<usize>,
    /// A setting Look could not keep, in one line.
    note: Option<SharedString>,
    /// The welcome mark (`motion::SpinningMark`) and its arrival.
    pub(super) welcome: motion::Arrival,
    pub(super) mark: Entity<motion::SpinningMark>,
    preview: Entity<Preview>,
}

impl Onboarding {
    /// Drop the account's embedded Agents and plans page.
    pub(super) fn forget_plans(&mut self) {
        if let Some(flow) = &mut self.flow {
            flow.plans = None;
        }
    }

    pub(super) fn new(cx: &mut Context<Workspace>) -> Onboarding {
        let welcome = motion::Arrival::new();
        let arrival = welcome.clone();
        Onboarding {
            state: if crate::onboarding::forced() {
                Some(State::start())
            } else {
                crate::onboarding::load()
            },
            flow: None,
            runs: 0,
            landing: None,
            note: None,
            welcome,
            mark: cx.new(|_| motion::SpinningMark::new(MARK, arrival)),
            preview: cx.new(|_| Preview),
        }
    }

    #[cfg(test)]
    pub(super) fn reset_saved_state_for_test(&mut self) {
        self.state = None;
    }

    /// The flow is on screen.
    pub(super) fn active(&self) -> bool {
        self.flow.is_some()
    }

    /// The sign-in is the flow's first step: no saved state, or a flow not done.
    pub(super) fn first_run(&self) -> bool {
        self.state.as_ref().is_none_or(|state| !state.done)
    }

    /// A sign-out: the flow on screen goes. Its step is saved, so the next sign-in resumes it.
    pub(super) fn leave(&mut self) {
        self.flow = None;
        self.landing = None;
    }

    /// The app arrived from the flow: its run.
    pub(super) fn landing(&self) -> Option<usize> {
        self.landing
    }
}

impl Workspace {
    /// The window opened signed in, or a slow keychain just said so: a flow left half way resumes; a person with no
    /// saved state used the app before, and never sees it.
    pub(super) fn onboarding_at_launch(&mut self) {
        if self.panel || self.onboarding.active() {
            return;
        }
        let (state, step) = State::at_launch(self.onboarding.state.take());
        crate::onboarding::save(&state);
        self.onboarding.state = Some(state);
        if let Some(step) = step {
            self.begin_onboarding(step, false);
        }
    }

    /// The person just signed in: the flow starts or resumes. Answers whether it did, so the sidebar waits for it.
    pub(super) fn onboarding_after_sign_in(&mut self) -> bool {
        if self.panel || self.onboarding.active() {
            return self.onboarding.active();
        }
        let (state, step) = State::at_sign_in(self.onboarding.state.take());
        crate::onboarding::save(&state);
        self.onboarding.state = Some(state);
        match step {
            Some(step) => {
                self.begin_onboarding(step, false);
                true
            }
            None => false,
        }
    }

    fn begin_onboarding(&mut self, step: Step, revisit: bool) {
        self.onboarding.runs += 1;
        self.onboarding.note = None;
        self.onboarding.flow = Some(Flow {
            step,
            revisit,
            run: self.onboarding.runs,
            plans: None,
            pick: None,
        });
    }

    /// The person menu's "Set up Nexus": the whole flow again, from Look.
    pub(super) fn restart_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = State::start();
        crate::onboarding::save(&state);
        self.onboarding.state = Some(state);
        self.begin_onboarding(Step::Look, false);
        self.open_onboarding_plan(window, cx);
        cx.notify();
    }

    /// Go to `step`: Plan builds its page.
    fn go_to(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = &mut self.onboarding.flow else { return };
        flow.step = step;
        self.onboarding.runs += 1;
        flow.run = self.onboarding.runs;
        self.onboarding.note = None;
        if !flow.revisit {
            if let Some(state) = &mut self.onboarding.state {
                state.step = Some(step);
                crate::onboarding::save(state);
            }
        }
        self.open_onboarding_plan(window, cx);
        cx.notify();
    }

    /// Build the Plan page once the step is open and its account has arrived, including a resumed first run.
    pub(super) fn open_onboarding_plan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let needs_plans = self
            .onboarding
            .flow
            .as_ref()
            .is_some_and(|flow| flow.step == Step::Plan && flow.plans.is_none());
        if !needs_plans {
            return;
        }
        let built = self.page_ctx(cx).and_then(|ctx| {
            // A view test mounts the page without starting the real computer's credential probes.
            #[cfg(test)]
            if cx.has_global::<layout_tests::OfflinePlanPage>() {
                return Some(cx.new(|_| EmptyView).into());
            }
            crate::pages::build(crate::pages::PageId::Subscriptions, ctx, window, cx)
        });
        if let Some(flow) = &mut self.onboarding.flow {
            flow.plans = built;
        }
    }

    /// Leave the step on screen, done or `skipped`.
    fn onboarding_next(&mut self, skipped: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = &self.onboarding.flow else { return };
        let (step, revisit) = (flow.step, flow.revisit);
        let mut state = self.onboarding.state.take().unwrap_or_else(State::start);
        let next = if revisit {
            state.revisited(step, skipped);
            None
        } else {
            state.advance(step, skipped)
        };
        crate::onboarding::save(&state);
        self.onboarding.state = Some(state);
        match next {
            Some(next) => self.go_to(next, window, cx),
            None => self.finish_onboarding(window, cx),
        }
    }

    /// Set up later: this step and the ones after it go on the checklist, and the app opens.
    fn onboarding_later(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = &self.onboarding.flow else { return };
        let step = flow.step;
        let mut state = self.onboarding.state.take().unwrap_or_else(State::start);
        state.later(step);
        crate::onboarding::save(&state);
        self.onboarding.state = Some(state);
        self.finish_onboarding(window, cx);
    }

    /// The flow ends: the app arrives as after a sign-in, in the picked project when there is one.
    fn finish_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = self.onboarding.flow.take() else {
            return;
        };
        self.onboarding.note = None;
        if self.sidebar_open {
            // The sidebar's own open animation (`render_sidebar`, `render_sidebar_card`), from no width to its width.
            // A step opened from the checklist hid the sidebar too, so it comes back the same way.
            self.sidebar_toggles += 1;
        }
        self.onboarding.runs += 1;
        self.onboarding.landing = Some(self.onboarding.runs);
        if let Some(project) = flow.pick {
            self.open_new_code_in(project, window, cx);
        }
        cx.notify();
    }

    /// The checklist's row: the first skipped step, alone.
    fn revisit_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(step) = self
            .onboarding
            .state
            .as_ref()
            .and_then(|state| state.skipped.first().copied())
        else {
            return;
        };
        self.begin_onboarding(step, true);
        self.open_onboarding_plan(window, cx);
        cx.notify();
    }

    /// The checklist's ✕: never again.
    fn dismiss_checklist(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.onboarding.state {
            state.dismissed = true;
            crate::onboarding::save(state);
        }
        cx.notify();
    }

    /// The progress bar: Sign in, then the three steps, the current one filling. `current` counts Sign in as 0.
    pub(super) fn render_onboarding_progress(&self, current: usize, palette: Palette) -> AnyElement {
        let run = self.onboarding.runs;
        let labels = std::iter::once("Sign in").chain(Step::ALL.iter().map(|step| step.label()));
        h_flex()
            .gap(px(6.))
            .children(labels.enumerate().map(|(index, label)| {
                let track = div().w_full().h(px(2.)).rounded(px(1.)).overflow_hidden();
                let track = if index < current {
                    track.bg(palette.muted)
                } else if index == current {
                    track
                        .bg(palette.border)
                        .child(div().h_full().rounded(px(1.)).bg(palette.primary).with_animation(
                            ElementId::NamedInteger("onboarding-progress".into(), (run * 8 + index) as u64),
                            Animation::new(motion::SLOW).with_easing(motion::ease_out),
                            |fill, t| fill.w(relative(t)),
                        ))
                } else {
                    track.bg(palette.border)
                };
                let ink = if index == current {
                    palette.foreground
                } else if index < current {
                    palette.muted
                } else {
                    palette.faint
                };
                v_flex()
                    .w(px(72.))
                    .gap(px(8.))
                    .items_center()
                    .child(track)
                    .child(div().text_size(px(11.5)).text_color(ink).child(label))
            }))
            .into_any_element()
    }

    /// The flow's page, in place of the workspace.
    pub(super) fn render_onboarding(
        &self,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(flow) = &self.onboarding.flow else {
            return div().into_any_element();
        };
        let (step, run, revisit) = (flow.step, flow.run, flow.revisit);
        let rise = move |element: AnyElement, part: &'static str, order: u32| {
            motion::rise(
                div().child(element),
                ElementId::Name(format!("onboarding-{}-{part}-{run}", step.label()).into()),
                STAGGER * order,
            )
            .into_any_element()
        };
        let width = step_width(step, window.viewport_size().width);
        let columns = width >= LOOK_WIDTH;
        let body = match step {
            Step::Look => self.render_onboarding_look(&rise, palette, columns, cx),
            Step::Plan => self.render_onboarding_plan(&rise, palette, cx),
            Step::Project => self.render_onboarding_project(&rise, palette, cx),
        };
        let top_right = if revisit {
            ghost("onboarding-close", "Back to Nexus", palette)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.onboarding.flow = None;
                    cx.notify();
                }))
                .into_any_element()
        } else {
            ghost("onboarding-later", "Set up later", palette)
                .on_click(cx.listener(|this, _, window, cx| this.onboarding_later(window, cx)))
                .into_any_element()
        };
        v_flex()
            .id("onboarding")
            .relative()
            .size_full()
            .bg(palette.background)
            .text_color(palette.foreground)
            .child(
                h_flex()
                    .relative()
                    .h(px(64.))
                    .flex_none()
                    .justify_center()
                    .child(self.render_onboarding_progress(step.index() + 1, palette))
                    .child(div().absolute().right(px(20.)).top(px(16.)).child(top_right)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h(px(0.))
                    .items_center()
                    .justify_center()
                    .p(px(PAGE_INSET))
                    .child(
                        v_flex()
                            .w(px(width))
                            .min_h(px(0.))
                            .max_h(relative(1.))
                            .gap(px(16.))
                            .child(
                                div()
                                    .id("onboarding-stage")
                                    .min_h(px(0.))
                                    .w_full()
                                    .overflow_y_scroll()
                                    .child(body),
                            )
                            .child(self.render_onboarding_foot(step, revisit, palette, cx)),
                    ),
            )
            .into_any_element()
    }

    /// The foot: Back on the left; Skip and the primary on the right.
    fn render_onboarding_foot(
        &self,
        step: Step,
        revisit: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let back = (!revisit).then(|| step.prev()).flatten().map(|prev| {
            ghost("onboarding-back", "Back", palette)
                .on_click(cx.listener(move |this, _, window, cx| this.go_to(prev, window, cx)))
        });
        let skip_label = match step {
            Step::Look => "Skip",
            Step::Plan => "Use credits for now",
            Step::Project => "Start without a project",
        };
        let skip = ghost("onboarding-skip", skip_label, palette)
            .on_click(cx.listener(|this, _, window, cx| this.onboarding_next(true, window, cx)));
        let pick_name = (step == Step::Project)
            .then(|| self.project_pick())
            .flatten()
            .and_then(|id| self.projects.iter().find(|project| project.id == id))
            .map(|project| project.name.clone());
        let last = revisit || step.next().is_none();
        let primary_label: SharedString = match (&pick_name, last) {
            (Some(name), _) => format!("Open {name}").into(),
            (None, true) if step == Step::Project => "Finish".into(),
            (None, true) => "Done".into(),
            (None, false) => "Continue".into(),
        };
        // Project without a pick finishes as skipped: nothing was opened.
        let skipped = step == Step::Project && pick_name.is_none();
        let primary = pill("onboarding-continue", primary_label, true, palette)
            .on_click(cx.listener(move |this, _, window, cx| this.onboarding_next(skipped, window, cx)));
        h_flex()
            .w_full()
            .h(px(46.))
            .flex_none()
            .gap(px(12.))
            .children(back)
            .child(div().flex_1())
            .child(skip)
            .child(primary)
            .into_any_element()
    }

    /// Look: the settings beside the live screen.
    fn render_onboarding_look(
        &self,
        rise: &dyn Fn(AnyElement, &'static str, u32) -> AnyElement,
        palette: Palette,
        columns: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = gpui::ui_zoom();
        let sizes = segmented(
            SIZES.iter().enumerate().map(|(ix, &(label, size))| {
                segment(("onboarding-size", ix), label, (zoom - size).abs() < 0.05, palette)
                    .on_click(cx.listener(move |_, _, _, cx| crate::zoom::set(size, cx)))
            }),
            palette,
        );
        let picked_width = crate::chat_width::choice(cx).preset;
        let widths = segmented(
            WIDTHS.iter().enumerate().map(|(ix, &(label, preset))| {
                segment(("onboarding-width", ix), label, picked_width == preset, palette).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.onboarding.note = crate::chat_width::set(ChatWidth::preset(preset), cx)
                            .err()
                            .map(SharedString::from);
                        cx.notify();
                    },
                ))
            }),
            palette,
        );
        // A Custom choice picks no segment; Fine-tune under them shows its values.
        let picked_look = crate::frost::chosen().preset();
        let looks = segmented(
            Transparency::ALL.iter().enumerate().map(|(ix, &level)| {
                segment(
                    ("onboarding-transparency", ix),
                    level.label(),
                    picked_look == Some(level),
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_frost(level.frost(), cx)))
            }),
            palette,
        );
        let transparency_note = if crate::frost::available() {
            "Let your desktop show through"
        } else {
            "This desktop keeps Nexus opaque; your choice is kept"
        };
        let reduce = crate::paths::reduce_motion();
        let system = crate::zoom::system_reduces_motion();
        let motion_switch =
            switch("onboarding-motion", reduce, palette).on_click(cx.listener(move |this, _, _, cx| {
                this.onboarding.note = crate::zoom::set_reduce_motion(!reduce, cx)
                    .err()
                    .map(SharedString::from);
                cx.notify();
            }));
        let login = crate::prefs::launch_at_login(cx);
        let login_switch = switch("onboarding-login", login, palette).on_click(cx.listener(move |this, _, _, cx| {
            this.onboarding.note = crate::prefs::set_launch_at_login(cx, !login)
                .err()
                .map(SharedString::from);
            cx.notify();
        }));
        let settings = v_flex()
            .w_full()
            .rounded(px(14.))
            .bg(palette.raised)
            .overflow_hidden()
            .child(row(
                "Text size",
                if cfg!(target_os = "macos") {
                    "⌘ + and ⌘ − change it any time"
                } else {
                    "Ctrl + and Ctrl − change it any time"
                },
                sizes.into_any_element(),
                palette,
            ))
            .child(
                v_flex()
                    .pb(px(12.))
                    .border_t_1()
                    .border_color(palette.border)
                    .child(row(
                        "Transparency",
                        transparency_note,
                        looks.into_any_element(),
                        palette,
                    ))
                    .child(div().px(px(16.)).child(self.render_tune("onboarding", palette, cx))),
            )
            .child(
                v_flex()
                    .px(px(16.))
                    .py(px(12.))
                    .gap(px(10.))
                    .border_t_1()
                    .border_color(palette.border)
                    .child(label_pair(
                        "Conversation width",
                        "How wide the conversation and the composer are. Custom is in Settings › Appearance.",
                        palette,
                    ))
                    .child(widths),
            )
            .child(row_top(
                "Reduce motion",
                if system {
                    "Your system asks for less motion, so it is on"
                } else {
                    "Loops stop, and changes show at once"
                },
                motion_switch.into_any_element(),
                palette,
            ))
            .child(row_top(
                "Open Nexus at login",
                "Agents keep running when the window closes",
                login_switch.into_any_element(),
                palette,
            ));
        let controls = v_flex()
            .w_full()
            .when(columns, |controls| controls.w(px(CONTROLS_W)).flex_none())
            .gap(px(18.))
            .child(rise(
                v_flex()
                    .gap(px(8.))
                    .child(heading("Make it yours", palette))
                    .child(lede(
                        "We matched your system. The screen shows each change as you make it.",
                        palette,
                    ))
                    .into_any_element(),
                "head",
                0,
            ))
            .child(rise(
                theme_cards::render(self.theme, palette, &self.settings.shell.fields, cx).into_any_element(),
                "themes",
                1,
            ))
            .child(rise(settings.into_any_element(), "settings", 2))
            .children(self.onboarding.note.clone().map(|note| {
                div()
                    .text_size(px(12.5))
                    .text_color(palette.muted)
                    .child(note)
                    .into_any_element()
            }));
        h_flex()
            .w_full()
            .when(!columns, |look| look.flex_col())
            .gap(px(if columns { COLUMN_GAP } else { PAGE_INSET }))
            .items_center()
            .child(controls)
            .child(rise(self.onboarding.preview.clone().into_any_element(), "preview", 2))
            .into_any_element()
    }

    /// Plan: Agents and plans, embedded.
    fn render_onboarding_plan(
        &self,
        rise: &dyn Fn(AnyElement, &'static str, u32) -> AnyElement,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = self.onboarding.flow.as_ref().and_then(|flow| flow.plans.clone());
        let panel = match page {
            Some(page) => div()
                .id("onboarding-plans")
                .w_full()
                .h(px(440.))
                .overflow_y_scroll()
                .rounded(px(14.))
                .bg(palette.background)
                .border_1()
                .border_color(palette.border)
                .child(page)
                .into_any_element(),
            None => v_flex()
                .w_full()
                .h(px(200.))
                .items_center()
                .justify_center()
                .gap(px(12.))
                .rounded(px(14.))
                .bg(palette.raised)
                .child(
                    div()
                        .text_size(px(13.5))
                        .text_color(palette.muted)
                        .child("Your account is still loading."),
                )
                .child(
                    pill("onboarding-plans-load", "Show my plans", false, palette).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.open_onboarding_plan(window, cx);
                            cx.notify();
                        },
                    )),
                )
                .into_any_element(),
        };
        v_flex()
            .w_full()
            .gap(px(18.))
            .child(rise(
                v_flex()
                    .gap(px(8.))
                    .child(heading("Use the plan you already pay for", palette))
                    .child(lede(
                        "Link Claude, ChatGPT or Google AI. Auto uses your plan first, at no extra cost. Nexus never \
                         sees your password.",
                        palette,
                    ))
                    .into_any_element(),
                "head",
                0,
            ))
            .child(rise(panel, "plans", 1))
            .child(rise(
                div()
                    .text_size(px(12.5))
                    .text_color(palette.faint)
                    .child("No plan? Nexus credits pay per call. You can link one later from the composer.")
                    .into_any_element(),
                "foot",
                2,
            ))
            .into_any_element()
    }

    /// The project Project shows as picked: the person's pick, or the one a new folder just opened.
    fn project_pick(&self) -> Option<String> {
        self.onboarding
            .flow
            .as_ref()
            .and_then(|flow| flow.pick.clone())
            .or_else(|| self.code_form.project.clone())
            .filter(|id| self.projects.iter().any(|project| &project.id == id))
    }

    /// Project: this computer's projects, or a new folder.
    fn render_onboarding_project(
        &self,
        rise: &dyn Fn(AnyElement, &'static str, u32) -> AnyElement,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let picked = self.project_pick();
        let local: Vec<_> = self
            .projects
            .iter()
            .filter(|project| project.machine.is_none())
            .take(5)
            .collect();
        let count = local.len();
        let rows = v_flex()
            .w_full()
            .rounded(px(PROJECT_LIST_RADIUS))
            .bg(palette.raised)
            .overflow_hidden()
            .children(local.iter().enumerate().map(|(ix, project)| {
                let on = picked.as_deref() == Some(project.id.as_str());
                let id = project.id.clone();
                let (top, bottom) = outer_corners(ix, count);
                h_flex()
                    .id(("onboarding-project", ix))
                    .h(px(52.))
                    .px(px(16.))
                    .gap(px(12.))
                    .cursor_pointer()
                    .when(top, |row| row.rounded_t(px(PROJECT_LIST_RADIUS)))
                    .when(bottom, |row| row.rounded_b(px(PROJECT_LIST_RADIUS)))
                    .when(ix > 0, |row| row.border_t_1().border_color(palette.border))
                    .when(on, |row| row.bg(palette.selected))
                    .when(!on, |row| row.hover(|style| style.bg(palette.hover)))
                    .child(
                        div().w(px(16.)).child(
                            Icon::new(if on { Lucide::Check } else { Lucide::Folder })
                                .size(px(15.))
                                .text_color(if on { palette.foreground } else { palette.icon }),
                        ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .child(div().text_size(px(14.)).truncate().child(project.name.clone()))
                            .children(project.branch.clone().map(|branch| {
                                h_flex()
                                    .gap(px(4.))
                                    .font_family(crate::pages::ui::MONO)
                                    .text_size(px(11.5))
                                    .text_color(palette.faint)
                                    .child(Icon::new(Lucide::GitBranch).size(px(11.)))
                                    .child(branch)
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(flow) = &mut this.onboarding.flow {
                            flow.pick = Some(id.clone());
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }));
        let empty = local.is_empty().then(|| {
            div()
                .text_size(px(13.5))
                .text_color(palette.muted)
                .child("No projects on this computer yet.")
                .into_any_element()
        });
        let worktree = picked.is_some().then(|| {
            motion::rise(
                h_flex()
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .text_color(palette.muted)
                    .child(Icon::new(Lucide::GitBranch).size(px(13.)).text_color(palette.icon))
                    .child("New sessions start in a worktree of their own, so your checkout stays clean."),
                ElementId::Name(format!("onboarding-worktree-{}", picked.clone().unwrap_or_default()).into()),
                Duration::ZERO,
            )
            .into_any_element()
        });
        v_flex()
            .w_full()
            .gap(px(18.))
            .child(rise(
                v_flex()
                    .gap(px(8.))
                    .child(heading("Where should agents work?", palette))
                    .child(lede(
                        "Pick a folder with code. You can add more any time from the sidebar.",
                        palette,
                    ))
                    .into_any_element(),
                "head",
                0,
            ))
            .child(rise(
                v_flex()
                    .gap(px(12.))
                    .children(empty)
                    .when(!local.is_empty(), |list| list.child(rows))
                    .into_any_element(),
                "rows",
                1,
            ))
            .children(worktree)
            .child(rise(
                h_flex()
                    .gap(px(12.))
                    .child(
                        pill("onboarding-add-folder", "Add a folder…", false, palette)
                            .on_click(cx.listener(|this, _, _, cx| this.add_project(cx))),
                    )
                    .into_any_element(),
                "actions",
                2,
            ))
            .into_any_element()
    }

    /// The sidebar's checklist row: "Finish setup", how many are left, and ✕.
    pub(super) fn render_setup_checklist(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.onboarding.state.as_ref()?;
        if !state.checklist() || self.onboarding.active() {
            return None;
        }
        let left = state.skipped.len();
        let first = state.skipped.first()?.task();
        Some(
            h_flex()
                .id("finish-setup")
                .mx(px(8.))
                .mb(px(4.))
                .h(px(44.))
                .px(px(10.))
                .gap(px(10.))
                .rounded(px(10.))
                .bg(palette.raised)
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .child(Icon::new(Lucide::Sparkles).size(px(15.)).text_color(palette.icon))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .child(div().text_size(px(13.)).child("Finish setup"))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(palette.faint)
                                .truncate()
                                .child(first),
                        ),
                )
                .child(
                    div()
                        .font_family(crate::pages::ui::MONO)
                        .text_size(px(11.5))
                        .text_color(palette.faint)
                        .child(format!("{left} left")),
                )
                .child(
                    div()
                        .id("finish-setup-dismiss")
                        .p(px(4.))
                        .rounded(px(6.))
                        .hover(|style| style.bg(palette.selected))
                        .child(Icon::new(Lucide::X).size(px(12.)).text_color(palette.faint))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.dismiss_checklist(cx);
                        })),
                )
                .on_click(cx.listener(|this, _, window, cx| this.revisit_onboarding(window, cx)))
                .into_any_element(),
        )
    }
}

/// The main area, rising in once after the flow (`landing`, its run); as it is otherwise.
pub(super) fn land(landing: Option<usize>, main: Div) -> AnyElement {
    match landing {
        Some(run) => motion::rise(main, ("onboarding-land", run), Duration::from_millis(160)).into_any_element(),
        None => main.into_any_element(),
    }
}

/// Look's live screen: the app in miniature, drawn with what the window draws with now (the theme, the conversation
/// width and the motion are all app-wide). It is its own view, so its working mark and shimmer redraw only it.
pub(super) struct Preview;

// The preview's screen and frame, and the window it represents.
const SCREEN_W: f32 = 460.;
const SCREEN_H: f32 = 300.;
const FRAME: f32 = 8.;
const WINDOW_W: f32 = 1920.;
const SIDEBAR_W: f32 = 256.;

// The stage inset and Look's two-column layout.
const CONTROLS_W: f32 = 420.;
const COLUMN_GAP: f32 = 44.;
const PAGE_INSET: f32 = 24.;
const LOOK_WIDTH: f32 = CONTROLS_W + COLUMN_GAP + SCREEN_W + FRAME * 2.;

fn preview_screen_w(window_width: Pixels) -> f32 {
    SCREEN_W.min((f32::from(window_width) - 2. * (PAGE_INSET + FRAME)).max(1.))
}

/// Stage and footer share the same width. Stack Look's live preview below the controls on a small desktop window.
fn step_width(step: Step, window_width: Pixels) -> f32 {
    let available = (f32::from(window_width) - 2. * PAGE_INSET).max(0.);
    available.min(match step {
        Step::Look if available >= LOOK_WIDTH => LOOK_WIDTH,
        Step::Look => preview_screen_w(window_width) + 2. * FRAME,
        Step::Plan => 760.,
        Step::Project => 620.,
    })
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The black frame is an opaque device bezel. Translucent window fills would blend with its black interior.
        let palette = crate::pages::palette(cx).solid();
        let screen_w = preview_screen_w(window.viewport_size().width);
        let room = WINDOW_W - SIDEBAR_W;
        let widths = crate::chat_width::in_room(room, cx);
        let scale = screen_w / WINDOW_W;
        let side = SIDEBAR_W * scale;
        let column = widths.column * scale;
        let chars = (widths.column / CH_PX).round();
        let preset = crate::chat_width::choice(cx).preset;
        let text = |size: f32, color: Hsla, words: &'static str| {
            div()
                .text_size(px(size))
                .line_height(px(size * 1.5))
                .text_color(color)
                .child(words)
        };
        let bar = |width: f32, color: Hsla| div().h(px(3.)).w(px(width)).rounded(px(2.)).bg(color);
        let sidebar = v_flex()
            .w(px(side))
            .h_full()
            .flex_none()
            .pt(px(12.))
            .px(px(8.))
            .gap(px(7.))
            .bg(palette.sidebar)
            .child(bar(side * 0.5, palette.faint))
            .child(div().h(px(4.)))
            .child(bar(side * 0.7, palette.selected))
            .child(bar(side * 0.6, palette.selected))
            .child(bar(side * 0.75, palette.selected))
            .child(div().h(px(4.)))
            .child(div().h(px(10.)).w_full().rounded(px(3.)).bg(palette.selected));
        let conversation = v_flex()
            .w(px(column))
            .gap(px(6.))
            .child(
                h_flex().w_full().justify_end().child(
                    div()
                        .max_w(relative(0.7))
                        .px(px(6.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .bg(palette.raised)
                        .child(text(
                            6.5,
                            palette.foreground,
                            "The checkout test fails one run in five. Fix it.",
                        )),
                ),
            )
            .child(text(
                6.5,
                palette.foreground,
                "The failure is a race between the cart refresh and the payment form. The test clicks Pay before \
                 the total settles, so the form sometimes submits the old amount. I'll wait for the total instead of \
                 sleeping, then run the test twenty times to prove the fix holds.",
            ))
            .child(text(6., palette.muted, "✓  Read checkout.spec.ts · 214 lines"))
            .child(
                div()
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .bg(palette.sidebar)
                    .font_family(crate::pages::ui::MONO)
                    .child(text(5.5, palette.muted, "+ await expect(total).toHaveText(expected);")),
            )
            .child(
                h_flex()
                    .gap(px(5.))
                    .child(motion::breathing_mark("preview-mark", 9., palette.foreground).breathing(true))
                    .child(div().text_size(px(6.)).child(motion::shimmer(
                        "preview-shimmer",
                        "Running the checkout test 20 times…",
                        palette.faint,
                        palette.strong,
                    ))),
            );
        let composer = div()
            .w(px(column))
            .h(px(20.))
            .rounded(px(6.))
            .bg(palette.raised)
            .border_1()
            .border_color(palette.border);
        let main_w = screen_w - side;
        let guide = |left: f32| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(left))
                .w(px(1.))
                .bg(palette.faint.opacity(0.35))
        };
        let main = div()
            .relative()
            .flex_1()
            .h_full()
            .bg(palette.background)
            .child(guide((main_w - column) / 2.))
            .child(guide((main_w + column) / 2.))
            .child(
                v_flex()
                    .size_full()
                    .items_center()
                    .pt(px(16.))
                    .pb(px(12.))
                    .child(motion::rise(
                        conversation,
                        ElementId::Name(format!("preview-column-{preset:?}").into()),
                        Duration::ZERO,
                    ))
                    .child(div().flex_1())
                    .child(composer),
            );
        v_flex()
            .gap(px(10.))
            .child(
                div()
                    .p(px(FRAME))
                    .rounded(px(16.))
                    .bg(rgb(0x0b0b0c))
                    .border_1()
                    .border_color(rgb(0x2a2a2c))
                    .child(
                        h_flex()
                            .w(px(screen_w))
                            .h(px(SCREEN_H))
                            .rounded(px(8.))
                            .overflow_hidden()
                            .child(sidebar)
                            .child(main),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child("On a 1920 px window: column")
                    .child(
                        div()
                            .font_family(crate::pages::ui::MONO)
                            .text_color(palette.foreground)
                            .child(format!("{} px", widths.column.round())),
                    )
                    .child("· about")
                    .child(
                        div()
                            .font_family(crate::pages::ui::MONO)
                            .text_color(palette.foreground)
                            .child(format!("{chars}")),
                    )
                    .child("characters a line"),
            )
    }
}

fn heading(text: &'static str, palette: Palette) -> AnyElement {
    div()
        .text_size(px(26.))
        .line_height(px(32.))
        .text_color(palette.strong)
        .child(text)
        .into_any_element()
}

fn lede(text: &'static str, palette: Palette) -> AnyElement {
    div()
        .text_size(px(14.5))
        .line_height(px(21.))
        .text_color(palette.muted)
        .child(text)
        .into_any_element()
}

/// A pill: the white primary, or a `selected` secondary.
fn pill(id: impl Into<ElementId>, label: impl Into<SharedString>, primary: bool, palette: Palette) -> Stateful<Div> {
    let label: SharedString = label.into();
    h_flex()
        .id(id)
        .h(px(38.))
        .px(px(20.))
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .when(primary, |pill| {
            pill.bg(palette.primary)
                .text_color(palette.primary_foreground)
                .hover(|style| style.opacity(0.9))
        })
        .when(!primary, |pill| {
            pill.bg(palette.selected)
                .text_color(palette.foreground)
                .hover(|style| style.bg(palette.hover))
        })
        .active(|style| style.opacity(0.85))
        .child(label)
}

/// Quiet text that acts.
fn ghost(id: &'static str, label: &'static str, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(32.))
        .px(px(12.))
        .rounded(px(8.))
        .cursor_pointer()
        .text_size(px(13.5))
        .text_color(palette.muted)
        .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
        .child(label)
}

fn segmented(items: impl Iterator<Item = Stateful<Div>>, palette: Palette) -> Div {
    h_flex()
        .p(px(3.))
        .gap(px(2.))
        .rounded_full()
        .bg(palette.selected)
        .flex_none()
        .children(items)
}

fn segment(id: impl Into<ElementId>, label: &'static str, on: bool, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(26.))
        .px(px(13.))
        .flex()
        .items_center()
        .rounded_full()
        .cursor_pointer()
        .text_size(px(13.))
        .when(on, |segment| {
            segment.bg(palette.primary).text_color(palette.primary_foreground)
        })
        .when(!on, |segment| {
            segment
                .text_color(palette.muted)
                .hover(|style| style.text_color(palette.foreground))
        })
        .child(label)
}

fn switch(id: &'static str, on: bool, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(32.))
        .h(px(20.))
        .flex_none()
        .rounded_full()
        .relative()
        .cursor_pointer()
        .bg(if on { palette.primary } else { palette.selected })
        .child(
            div()
                .absolute()
                .top(px(2.))
                .size(px(16.))
                .rounded_full()
                .bg(if on { palette.primary_foreground } else { palette.icon })
                .with_animation(
                    ElementId::Name(format!("{id}-{on}").into()),
                    Animation::new(motion::UI).with_easing(motion::standard),
                    move |knob, t| {
                        let (from, to) = if on { (2., 14.) } else { (14., 2.) };
                        knob.left(px(from + (to - from) * t))
                    },
                ),
        )
}

fn label_pair(label: &'static str, note: &'static str, palette: Palette) -> Div {
    v_flex()
        .flex_1()
        .min_w(px(0.))
        .child(div().text_size(px(14.)).child(label))
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(palette.faint)
                .child(note),
        )
}

fn row(label: &'static str, note: &'static str, control: AnyElement, palette: Palette) -> Div {
    h_flex()
        .min_h(px(56.))
        .px(px(16.))
        .py(px(8.))
        .gap(px(12.))
        .child(label_pair(label, note, palette))
        .child(control)
}

fn row_top(label: &'static str, note: &'static str, control: AnyElement, palette: Palette) -> Div {
    row(label, note, control, palette)
        .border_t_1()
        .border_color(palette.border)
}

/// The corner radius of onboarding's project list.
const PROJECT_LIST_RADIUS: f32 = 14.;

/// Whether row `ix` of a rounded list of `count` rows holds the list's top and bottom corners. Its highlight is drawn
/// with them: `overflow_hidden` clips to a rectangle (GPUI's `ContentMask` has only bounds), so a square highlight on an
/// end row would cover the list's rounded corners.
fn outer_corners(ix: usize, count: usize) -> (bool, bool) {
    (ix == 0, ix + 1 == count)
}

#[cfg(test)]
#[path = "onboarding_layout_tests.rs"]
mod layout_tests;

#[cfg(test)]
#[path = "onboarding_sidebar_tests.rs"]
mod sidebar_tests;

#[cfg(test)]
mod corner_tests {
    use super::outer_corners;

    #[test]
    fn end_rows_hold_the_list_corners() {
        // One project: its row is the whole list.
        assert_eq!(outer_corners(0, 1), (true, true));
        // Three: the first holds the top, the last the bottom, the middle neither.
        assert_eq!(
            [0, 1, 2].map(|ix| outer_corners(ix, 3)),
            [(true, false), (false, false), (false, true)]
        );
    }
}
