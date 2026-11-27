//! Fine-tune, under the Transparency presets in Settings › Appearance and onboarding's Look: a quiet "› Fine-tune"
//! link that opens two sliders, See-through and Blur (`frost::Frost`). The link only shows or hides them; it never
//! changes the look. A preset is a point on the sliders, so picking one moves them; moving one off a point is Custom.
//! A slider draws every window as it moves and saves when let go, as Conversation width's do (`chat_column.rs`).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::super::Workspace;
use super::super::settings_shell::RowStatus;
use crate::frost::Frost;
use crate::i18n::tr;
use crate::pages::ui;
use crate::palette::Palette;

/// Both sliders move in steps of 5.
const STEP: f32 = 5.;

/// Whether Fine-tune is open, and its sliders while it is.
#[derive(Default)]
pub(in crate::workspace) struct Tune {
    open: bool,
    sliders: Option<Sliders>,
}

struct Sliders {
    see_through: Entity<SliderState>,
    blur: Entity<SliderState>,
    _changed: [Subscription; 2],
}

impl Workspace {
    /// Pick a preset or a Fine-tune value: every main window follows at once, and it is kept for the next launch.
    /// Open sliders move to it.
    pub(in crate::workspace) fn set_frost(&mut self, frost: Frost, cx: &mut Context<Self>) {
        self.save_frost(frost, cx);
        if self.settings.tune.open {
            self.build_tune_sliders(cx);
        }
    }

    fn save_frost(&mut self, frost: Frost, cx: &mut Context<Self>) {
        let status = match crate::frost::choose(frost) {
            Ok(()) => RowStatus::Saved(std::time::Instant::now()),
            Err(error) => RowStatus::Failed(format!("Applied until Nexus quits, but not kept. {error}").into()),
        };
        crate::frost::apply(cx);
        self.set_row_status("transparency", status, cx);
    }

    fn toggle_tune(&mut self, cx: &mut Context<Self>) {
        self.settings.tune.open = !self.settings.tune.open;
        if self.settings.tune.open {
            self.build_tune_sliders(cx);
        } else {
            self.settings.tune.sliders = None;
        }
        cx.notify();
    }

    /// The sliders at the choice now.
    fn build_tune_sliders(&mut self, cx: &mut Context<Self>) {
        let frost = crate::frost::chosen();
        let slider = |value: u8, cx: &mut Context<Self>| {
            cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(f32::from(Frost::MAX))
                    .step(STEP)
                    .default_value(f32::from(value))
            })
        };
        let see_through = slider(frost.see_through, cx);
        let blur = slider(frost.blur, cx);
        let changed = [
            cx.subscribe(&see_through, |this, _, event: &SliderEvent, cx| {
                this.on_tune_slider(event, cx)
            }),
            cx.subscribe(&blur, |this, _, event: &SliderEvent, cx| this.on_tune_slider(event, cx)),
        ];
        self.settings.tune.sliders = Some(Sliders {
            see_through,
            blur,
            _changed: changed,
        });
    }

    /// A slider moves: every window follows it; let go, the choice is kept.
    fn on_tune_slider(&mut self, event: &SliderEvent, cx: &mut Context<Self>) {
        let Some(sliders) = &self.settings.tune.sliders else {
            return;
        };
        let value = |state: &Entity<SliderState>| state.read(cx).value().start().round().clamp(0., 100.) as u8;
        let frost = Frost::new(value(&sliders.see_through), value(&sliders.blur));
        match event {
            SliderEvent::Change(_) => {
                if frost != crate::frost::chosen() {
                    crate::frost::try_on(frost);
                    crate::frost::apply(cx);
                    cx.notify();
                }
            }
            SliderEvent::Release(_) => self.save_frost(frost, cx),
        }
    }

    /// The "› Fine-tune" link, and the sliders under it while it is open. `id` names the place it is drawn.
    pub(in crate::workspace) fn render_tune(&self, id: &'static str, palette: Palette, cx: &mut Context<Self>) -> Div {
        let frost = crate::frost::chosen();
        let tune = &self.settings.tune;
        let custom = frost.preset().is_none();
        let link = ui::focusable(
            h_flex()
                .id(SharedString::from(format!("{id}-fine-tune")))
                .flex_none()
                .h(px(24.))
                .px(px(4.))
                .ml(px(-4.))
                .gap(px(6.))
                .rounded(px(6.))
                .text_size(px(13.))
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| style.text_color(palette.foreground))
                .child(
                    Icon::new(if tune.open {
                        Lucide::ChevronDown
                    } else {
                        Lucide::ChevronRight
                    })
                    .size(px(12.))
                    .text_color(palette.icon),
                )
                .child(tr("Fine-tune"))
                .when(!tune.open && custom, |link| {
                    link.child(
                        div()
                            .text_color(palette.faint)
                            .child(format!("\u{b7} {}", tr("Custom"))),
                    )
                })
                .on_click(cx.listener(|this, _, _, cx| this.toggle_tune(cx))),
            palette,
        );
        v_flex()
            .gap(px(8.))
            .child(h_flex().child(link))
            .when_some(tune.sliders.as_ref(), |column, sliders| {
                column.child(panel(frost, sliders, palette, cx))
            })
    }
}

/// The sliders, what they make, and a way back to the default.
fn panel(frost: Frost, sliders: &Sliders, palette: Palette, cx: &mut Context<Workspace>) -> Div {
    let blur_value = if frost.is_opaque() {
        "\u{2014}".to_string()
    } else if frost.blur == 0 {
        tr("Off").to_string()
    } else {
        format!("{} px", frost.glass_px().round())
    };
    v_flex()
        .w_full()
        .max_w(px(560.))
        .gap(px(12.))
        .p(px(14.))
        .rounded(px(12.))
        .bg(palette.raised)
        .child(slider_row(
            tr("See-through"),
            format!("{}%", frost.see_through),
            &sliders.see_through,
            false,
            palette,
        ))
        .child(slider_row(
            tr("Blur"),
            blur_value,
            &sliders.blur,
            frost.is_opaque(),
            palette,
        ))
        .when(!frost.is_opaque() && frost.blur == 0, |panel| {
            panel.child(ui::note(
                tr("Blur off: the desktop shows sharp behind the text."),
                palette,
            ))
        })
        .child(
            h_flex()
                .gap(px(12.))
                .justify_between()
                .child(ui::note(summary(frost), palette))
                .when(frost != Frost::default(), |row| {
                    row.child(ui::focusable(
                        div()
                            .id("fine-tune-reset")
                            .flex_none()
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .underline()
                            .cursor_pointer()
                            .hover(|style| style.text_color(palette.foreground))
                            .child(tr("Reset to Frost"))
                            .on_click(cx.listener(|this, _, _, cx| this.set_frost(Frost::default(), cx))),
                        palette,
                    ))
                }),
        )
}

/// What the values make, in a line: the preset's name or Custom, then the values.
fn summary(frost: Frost) -> SharedString {
    if frost.is_opaque() {
        return tr("Opaque: nothing shows through, so nothing blurs.");
    }
    let name = match frost.preset() {
        Some(level) => level.label().to_string(),
        None => tr("Custom").to_string(),
    };
    let blur = if frost.blur == 0 {
        tr("Off").to_lowercase()
    } else {
        format!("{} px", frost.glass_px().round())
    };
    tr("{name}: {percent}% see-through, blur {blur}.")
        .replace("{name}", &name)
        .replace("{percent}", &frost.see_through.to_string())
        .replace("{blur}", &blur)
        .into()
}

/// A slider with its name on the left and its value on the right, as Conversation width's.
fn slider_row(
    name: SharedString,
    value: String,
    state: &Entity<SliderState>,
    disabled: bool,
    palette: Palette,
) -> impl IntoElement {
    h_flex()
        .gap(px(12.))
        .when(disabled, |row| row.opacity(0.4))
        .child(
            div()
                .w(px(96.))
                .flex_none()
                .text_size(px(13.))
                .text_color(palette.muted)
                .child(name.clone()),
        )
        .child(
            div().flex_1().min_w(px(0.)).child(
                Slider::new(state)
                    .horizontal()
                    .disabled(disabled)
                    .accessibility_label(name),
            ),
        )
        .child(
            div()
                .w(px(48.))
                .flex_none()
                .text_size(px(13.))
                .text_color(palette.strong)
                .child(value),
        )
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;
    use crate::frost::Transparency;

    #[test]
    fn the_summary_names_the_preset_or_custom() {
        assert_eq!(
            summary(Transparency::Frost.frost()),
            "Frost: 40% see-through, blur 16 px."
        );
        assert_eq!(
            summary(Transparency::Glass.frost()),
            "Glass: 70% see-through, blur 16 px."
        );
        assert_eq!(summary(Frost::new(85, 25)), "Custom: 85% see-through, blur 8 px.");
        assert_eq!(summary(Frost::new(60, 0)), "Custom: 60% see-through, blur off.");
        assert_eq!(
            summary(Frost::OPAQUE),
            "Opaque: nothing shows through, so nothing blurs."
        );
    }
}
