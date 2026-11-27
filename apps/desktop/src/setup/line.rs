//! The setup line: one 40 px line that says what is missing, names the account, and carries its fix on the right
//! (`product/nexus/DESIGN.md`, "Setup line"). Never red text: the red ✕ marks only a failure, the amber dot only a
//! limit. Who can't fix it sees Ask, naming who can.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::access::{self, Decision};
use super::fix::{Ask, BillingBlock, Fix};
use super::state::{self, Run};
use super::{FixKey, Fixer};
use crate::motion::{self, Phase};
use crate::palette::Palette;

/// The line's height at least, its corner and its buttons' height.
const HEIGHT: f32 = 40.;
const RADIUS: f32 = crate::tokens::radius::CARD;
const PILL: f32 = 28.;
/// The narrowest the text gets beside the buttons; narrower, the buttons wrap below it.
const TEXT_MIN: f32 = 200.;

/// `what` and `why` as one paragraph, and where `why` sits in it.
fn paragraph(what: &str, why: Option<&str>) -> (SharedString, Option<std::ops::Range<usize>>) {
    match why.filter(|why| !why.is_empty()) {
        Some(why) => {
            let start = what.len() + 1;
            (format!("{what} {why}").into(), Some(start..start + why.len()))
        }
        None => (what.to_string().into(), None),
    }
}

/// What the line says about its state, by its mark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    /// Setup is missing and someone can add it: the icon in the icon colour. Who can't sees Ask instead of the fix.
    #[default]
    Fixable,
    /// Nobody in the app can fix it: quiet text, no button, a guide link at most.
    Info,
    /// A limit was reached: the 7 px amber dot.
    Limit,
    /// Something failed: the red ✕. The text stays the foreground colour.
    Failed,
}

/// One button.
#[derive(Clone)]
struct Action {
    label: SharedString,
    fix: Fix,
}

/// A setup line, built by its call site each frame. `key` names the blocked action (`"start:no-key"`): it keys the
/// line's motion, its Ask state, its terminal step and what resumes when the fix is done.
#[derive(Clone)]
pub struct SetupLine {
    key: FixKey,
    icon: Lucide,
    what: SharedString,
    why: Option<SharedString>,
    tone: Tone,
    actions: Vec<Action>,
    guide: Option<(SharedString, SharedString)>,
    ask_message: Option<String>,
    ask_billing: Option<BillingBlock>,
    doing: SharedString,
    main: bool,
}

impl SetupLine {
    /// `what`'s first words say what is missing and name the account: "Acme has no Anthropic key yet."
    pub fn new(key: impl Into<FixKey>, icon: Lucide, what: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            icon,
            what: what.into(),
            why: None,
            tone: Tone::Fixable,
            actions: Vec::new(),
            guide: None,
            ask_message: None,
            ask_billing: None,
            doing: SharedString::default(),
            main: false,
        }
    }

    /// The second sentence, in secondary text: why, or what happens next ("Calls resume Dec 1.").
    pub fn why(mut self, why: impl Into<SharedString>) -> Self {
        self.why = Some(why.into());
        self
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// A button that runs `fix`. Who lacks the fix's permission sees one Ask button in its place.
    pub fn fix(mut self, label: impl Into<SharedString>, fix: Fix) -> Self {
        self.actions.push(Action {
            label: label.into(),
            fix,
        });
        self
    }

    /// A guide on the web, as a quiet link: for a line nobody in the app can fix.
    pub fn guide(mut self, label: impl Into<SharedString>, url: impl Into<SharedString>) -> Self {
        self.guide = Some((label.into(), url.into()));
        self
    }

    /// What Ask sends ("Could you add an Anthropic key for Acme in Nexus Gateway?"), and what the people who can would
    /// do, for the line's second sentence ("add one": "Ana and Dev can add one.").
    pub fn ask(mut self, message: impl Into<String>, doing: impl Into<SharedString>) -> Self {
        self.ask_message = Some(message.into());
        self.doing = doing.into();
        self
    }

    /// Ask goes to the owners' inboxes through Gateway, for a billing block.
    pub fn billing(mut self, block: BillingBlock) -> Self {
        self.ask_billing = Some(block);
        self
    }

    /// The line is the view's main action: its first fix is the one white button. At most one per view.
    pub fn main(mut self) -> Self {
        self.main = true;
        self
    }

    pub fn key(&self) -> &FixKey {
        &self.key
    }

    /// The buttons this person sees, and the second sentence, after [`access::decide`] and the line's state.
    fn resolve(&self, fixer: &Fixer, cx: &App) -> Resolved {
        let access = fixer.access(cx);
        let mut actions = Vec::new();
        let mut ask: Option<Ask> = None;
        for action in &self.actions {
            let needs = action.fix.required();
            match access::decide(needs, &access) {
                Decision::Fix => actions.push(action.clone()),
                Decision::Ask { who } if ask.is_none() => {
                    let message = self.ask_message.clone().unwrap_or_else(|| self.what.to_string());
                    let mut question = Ask::new(needs, message);
                    question.billing = self.ask_billing;
                    question.who = who;
                    actions.push(Action {
                        label: access::ask_label(&question.who, needs).into(),
                        fix: Fix::Ask(question.clone()),
                    });
                    ask = Some(question);
                }
                Decision::Ask { .. } => {}
            }
        }
        let can = ask
            .as_ref()
            .map(|ask| access::can_line(&ask.who, ask.needs, &self.doing));
        let why = match (&self.why, can) {
            (Some(why), Some(can)) => Some(format!("{why} {can}").into()),
            (why, can) => why.clone().or(can.map(SharedString::from)),
        };
        let mut resolved = Resolved {
            tone: self.tone,
            why,
            actions,
            running: false,
            asked: state::asked(&self.key, cx).map(|asked| asked.label.into()),
        };
        match state::run_state(&self.key, cx) {
            Some(Run::Running) => resolved.running = true,
            Some(Run::Failed { code, last }) => {
                resolved.tone = Tone::Failed;
                resolved.why = Some(
                    last.or_else(|| code.map(|code| format!("It exited with code {code}.")))
                        .unwrap_or_else(|| "It didn't finish.".into())
                        .into(),
                );
                for action in &mut resolved.actions {
                    if matches!(action.fix, Fix::RunInTerminal(_)) {
                        action.label = "Try again".into();
                    }
                }
            }
            Some(Run::Unwatched) => {
                resolved.why = Some("It runs in the Commands window. Check again once it ends.".into());
            }
            None => {}
        }
        resolved
    }

    /// The line as an element, at rest (no entrance): `slot` adds the motion.
    pub fn render(&self, fixer: &Fixer, palette: Palette, cx: &App) -> Stateful<Div> {
        let resolved = self.resolve(fixer, cx);
        let quiet = resolved.tone == Tone::Info;
        let mark: AnyElement = match resolved.tone {
            Tone::Limit => div().size(px(7.)).rounded_full().bg(palette.warning).into_any_element(),
            Tone::Failed => Icon::new(Lucide::X)
                .size(px(16.))
                .text_color(palette.status_failed)
                .into_any_element(),
            Tone::Fixable | Tone::Info => Icon::new(self.icon)
                .size(px(16.))
                .text_color(palette.icon)
                .into_any_element(),
        };
        // One paragraph that wraps: `what`, then `why` in secondary text. It grows and shrinks, but never below
        // `TEXT_MIN`: past that the buttons wrap under it instead of covering it.
        let (paragraph, why_at) = paragraph(&self.what, resolved.why.as_deref());
        let what_ink = if quiet { palette.muted } else { palette.foreground };
        let text = div()
            .debug_selector(|| "setup-line-text".into())
            .flex_grow(1.)
            .flex_shrink(1.)
            .flex_basis(px(TEXT_MIN))
            .min_w(px(0.))
            .text_color(what_ink)
            .child(StyledText::new(paragraph).with_highlights(why_at.map(|range| {
                (
                    range,
                    HighlightStyle {
                        color: Some(palette.muted),
                        ..Default::default()
                    },
                )
            })));
        // A fixed group on the right; on a line too narrow for both, it wraps below the text, still on the right.
        let mut buttons = h_flex()
            .debug_selector(|| "setup-line-actions".into())
            .flex_none()
            .ml_auto()
            .gap(px(6.));
        if resolved.running {
            buttons = buttons.child(
                div()
                    .text_size(px(12.))
                    .text_color(palette.muted)
                    .child("Running in the terminal…"),
            );
        } else if !quiet {
            for (ix, action) in resolved.actions.iter().enumerate() {
                let is_ask = matches!(action.fix, Fix::Ask(_));
                if is_ask && let Some(label) = resolved.asked.clone() {
                    buttons = buttons.child(asked_mark(&self.key, label, palette));
                    continue;
                }
                let primary = self.main && ix == 0 && !is_ask;
                buttons = buttons.child(self.button(ix, action, primary, fixer, palette));
            }
        }
        if let Some((label, url)) = self.guide.clone() {
            let id = ElementId::NamedInteger(format!("setup-guide-{}", self.key).into(), 0);
            buttons = buttons.child(
                div()
                    .id(id)
                    .text_size(px(13.))
                    .text_color(palette.muted)
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.strong))
                    .child(format!("{label} ›"))
                    .on_click(move |_, _, cx| crate::external_link::open(&url, cx)),
            );
        }
        h_flex()
            .id(ElementId::Name(format!("setup-line-{}", self.key).into()))
            .w_full()
            .min_h(px(HEIGHT))
            .flex_wrap()
            .px(px(12.))
            .py(px(6.))
            .gap_x(px(10.))
            .gap_y(px(6.))
            .rounded(px(RADIUS))
            .bg(palette.raised)
            .text_size(px(13.))
            .child(div().flex_none().w(px(16.)).flex().justify_center().child(mark))
            .child(text)
            .child(buttons)
    }

    fn button(&self, ix: usize, action: &Action, primary: bool, fixer: &Fixer, palette: Palette) -> Stateful<Div> {
        let id = ElementId::NamedInteger(format!("setup-action-{}", self.key).into(), ix as u64);
        let (fill, ink) = if primary {
            (palette.primary, palette.primary_foreground)
        } else {
            (palette.selected, palette.foreground)
        };
        let fixer = fixer.clone();
        let key = self.key.clone();
        let fix = action.fix.clone();
        h_flex()
            .id(id)
            .flex_none()
            .h(px(PILL))
            .px(px(12.))
            .justify_center()
            .rounded_full()
            .bg(fill)
            .text_color(ink)
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .when(!primary, |pill| pill.hover(move |style| style.bg(palette.thumb)))
            .when(primary, |pill| pill.hover(|style| style.opacity(0.9)))
            .active(|style| style.opacity(0.8))
            .child(action.label.clone())
            .on_click(move |_, window, cx| fixer.run(key.clone(), fix.clone(), window, cx))
    }
}

/// The buttons and words this person sees now.
struct Resolved {
    tone: Tone,
    why: Option<SharedString>,
    actions: Vec<Action>,
    running: bool,
    asked: Option<SharedString>,
}

/// "Asked · in Ana's inbox" with a check, cross-faded in over `UI` where the Ask button was.
fn asked_mark(key: &FixKey, label: SharedString, palette: Palette) -> impl IntoElement {
    let element = h_flex()
        .flex_none()
        .h(px(PILL))
        .gap(px(6.))
        .text_size(px(13.))
        .text_color(palette.muted)
        .child(Icon::new(Lucide::Check).size(px(14.)).text_color(palette.muted))
        .child(label.clone());
    motion::fade_in(element, ElementId::Name(format!("setup-asked-{key}-{label}").into()))
}

/// A line that comes and goes with its blocker: it fades in and rises 4 px over `UI_LONG` when it appears, and fades
/// out over `UI_EXIT` when the call site stops drawing it (its fix landed). Under less motion it shows and goes in place.
/// `slot` names the place in the view ("composer-setup"): one line at a time per slot.
pub fn slot<V: 'static>(
    slot: &'static str,
    line: Option<SetupLine>,
    fixer: &Fixer,
    palette: Palette,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    let key = SharedString::from(format!("setup-slot-{slot}"));
    let (phase, line) = motion::presence_of_for(key, line, motion::UI_EXIT, cx)?;
    let body = line.render(fixer, palette, cx);
    Some(match phase {
        Phase::Open { epoch } => body
            .with_animation(
                ElementId::NamedInteger(format!("setup-enter-{slot}").into(), epoch as u64),
                Animation::new(motion::UI_LONG).with_easing(motion::ease_out),
                |line, t| line.opacity(t).relative().top(px(4. * (1. - t))),
            )
            .into_any_element(),
        Phase::Leaving { since, epoch } => {
            let fade = (since.elapsed().as_secs_f32() / motion::UI_EXIT.as_secs_f32()).clamp(0., 1.);
            div()
                .relative()
                .child(body.opacity(1. - fade))
                // A leaving line takes no clicks, and asks for frames until it has gone.
                .child(div().id("setup-leaving").absolute().inset_0().occlude())
                .child(div().absolute().size_0().with_animation(
                    ElementId::NamedInteger(format!("setup-leave-{slot}").into(), epoch as u64),
                    Animation::new(motion::UI_EXIT),
                    |pump, _| pump,
                ))
                .into_any_element()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::PageId;
    use core::prelude::v1::test;
    use serde_json::json;

    fn acme(cx: &mut App, permissions: &[&str]) -> Fixer {
        access::Access::set_account(
            "acme",
            Some("u_me".into()),
            Some(permissions.iter().map(|each| each.to_string()).collect()),
            cx,
        );
        access::Access::set_managers(
            "acme",
            access::parse_managers(&json!({"members": [
                {"userId": "u_ana", "name": "Ana Souza", "role": "owner"},
                {"userId": "u_dev", "name": "Dev Patel", "role": "admin"},
            ]})),
            cx,
        );
        Fixer::new(Some("acme".into()), |_, _, _, _| {})
    }

    fn no_key() -> SetupLine {
        SetupLine::new("start:no-key", Lucide::Info, "Acme has no Anthropic key yet.")
            .fix("Use Max plan", Fix::act(|_, _| {}))
            .fix(
                "Add key",
                Fix::open_at(PageId::Providers, "provider-anthropic").needs("provider_keys.manage"),
            )
            .ask("Could you add an Anthropic key for Acme?", "add one")
    }

    /// A line `width` wide, with a long `why` and two buttons.
    struct Narrow {
        width: f32,
    }

    impl Render for Narrow {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let fixer = Fixer::new(None, |_, _, _, _| {});
            let line = SetupLine::new("narrow", Lucide::Info, "Codex isn't installed on this computer.")
                .why("npm error path /usr/lib/node_modules/@openai/codex")
                .fix("Try again", Fix::run("npm install -g @openai/codex"))
                .fix("Coding agents", Fix::open(PageId::Agents));
            let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
            div().w(px(self.width)).child(line.render(&fixer, palette, cx))
        }
    }

    #[gpui_kit::test]
    fn the_buttons_never_cover_the_text(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|_, _| Narrow { width: 420. });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let text = cx.debug_bounds("setup-line-text").expect("the text");
        let actions = cx.debug_bounds("setup-line-actions").expect("the buttons");
        assert!(
            actions.top() >= text.bottom(),
            "narrow: the buttons wrap below the text"
        );
        assert!(
            (actions.right() - text.right()).abs() < px(1.),
            "narrow: the buttons stay on the right"
        );

        view.update(cx, |view, cx| {
            view.width = 900.;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let text = cx.debug_bounds("setup-line-text").expect("the text");
        let actions = cx.debug_bounds("setup-line-actions").expect("the buttons");
        assert!(actions.left() >= text.right(), "wide: the buttons sit beside the text");
        assert!(actions.top() < text.bottom(), "wide: on the same line");
    }

    #[test]
    fn what_and_why_read_as_one_paragraph() {
        let (text, why) = paragraph("Acme's budget is used up.", Some("Calls resume Dec 1."));
        assert_eq!(text.as_ref(), "Acme's budget is used up. Calls resume Dec 1.");
        assert_eq!(&text[why.expect("why")], "Calls resume Dec 1.");
        assert_eq!(paragraph("Done.", None).1, None);
    }

    fn labels(resolved: &Resolved) -> Vec<String> {
        resolved.actions.iter().map(|action| action.label.to_string()).collect()
    }

    #[gpui_kit::test]
    fn who_can_fix_it_sees_the_fix(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let fixer = acme(cx, &["provider_keys.manage"]);
            let resolved = no_key().resolve(&fixer, cx);
            assert_eq!(labels(&resolved), ["Use Max plan", "Add key"]);
            assert_eq!(resolved.why, None);
        });
    }

    #[gpui_kit::test]
    fn anyone_else_asks_and_the_line_names_who_can(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let fixer = acme(cx, &["agents.use"]);
            let resolved = no_key().resolve(&fixer, cx);
            assert_eq!(labels(&resolved), ["Use Max plan", "Ask Ana"]);
            assert_eq!(resolved.why.as_deref(), Some("Ana and Dev can add one."));
            let Fix::Ask(ask) = &resolved.actions[1].fix else {
                panic!("the second button asks");
            };
            assert_eq!(ask.message, "Could you add an Anthropic key for Acme?");
            assert_eq!(ask.who, ["Ana", "Dev"]);
        });
    }

    #[gpui_kit::test]
    fn a_failed_terminal_step_shows_the_red_mark_its_last_line_and_try_again(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let fixer = acme(cx, &[]);
            let line = SetupLine::new("pr:gh-auth", Lucide::Info, "GitHub CLI is signed out.")
                .fix("Sign in to GitHub", Fix::run("gh auth login --web"));
            state::set_run(
                line.key().clone(),
                Run::Failed {
                    code: Some(1),
                    last: Some("error: authentication failed".into()),
                },
                cx,
            );
            let resolved = line.resolve(&fixer, cx);
            assert_eq!(resolved.tone, Tone::Failed);
            assert_eq!(resolved.why.as_deref(), Some("error: authentication failed"));
            assert_eq!(labels(&resolved), ["Try again"]);
        });
    }
}
