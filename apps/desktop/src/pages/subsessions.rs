//! Settings › Subsessions. Device settings and the current account's saved project choices.
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use nexus_desktop_core::settings::{OrchestrationLimits, Settings};
use nexus_workbench::session_choice::Preference;

use super::desktop_app::{read_only_note, save_settings};
use super::{DeviceCtx, palette, ui};
use crate::desktop_settings::Changed;
use crate::palette::Palette;

mod limits;
use limits::Limit;

#[cfg(test)]
#[path = "subsessions_tests.rs"]
mod tests;

pub(super) fn build(device: DeviceCtx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| {
        let mut page = SubsessionsPage::new(window, cx);
        page._subscriptions
            .push(cx.observe_global_in::<Changed>(window, |this, window, cx| {
                if this.device.is_some() && !this.saving {
                    this.settings = cx.global::<Changed>().settings.clone();
                    this.sync_inputs(window, cx);
                    cx.notify();
                }
            }));
        let read_revision = Changed::revision(cx);
        cx.spawn_in(window, async move |this, cx| {
            let settings = cx
                .background_executor()
                .spawn(async { crate::desktop_settings::load() })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.settings = cx
                    .try_global::<Changed>()
                    .filter(|changed| changed.revision > read_revision)
                    .map(|changed| changed.settings.clone())
                    .unwrap_or(settings);
                this.sync_inputs(window, cx);
                this.refresh_overrides(&device, false, cx);
                this.device = Some(device);
                cx.notify();
            })
            .ok();
        })
        .detach();
        page
    });
    Some(view.into())
}

struct LimitInput {
    kind: Limit,
    number: Entity<InputState>,
    slider: Entity<SliderState>,
    problem: Option<String>,
}

struct SubsessionsPage {
    device: Option<DeviceCtx>,
    settings: Settings,
    limits: Vec<LimitInput>,
    saving: bool,
    problem: Option<String>,
    overrides: Option<Result<u32, String>>,
    checking_overrides: bool,
    service_restart_needed: bool,
    _subscriptions: Vec<Subscription>,
}

impl SubsessionsPage {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let limits = Limit::ALL.map(|kind| {
            let number = cx.new(|cx| InputState::new(window, cx).placeholder("No limit"));
            let slider = cx.new(|_| SliderState::new().min(0.).max(kind.slider_max()).step(1.));
            subscriptions.push(cx.subscribe_in(
                &number,
                window,
                move |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_limit(kind, window, cx),
                    InputEvent::Change => {
                        this.limits[kind as usize].problem = None;
                        cx.notify();
                    }
                    _ => {}
                },
            ));
            subscriptions.push(cx.subscribe_in(
                &slider,
                window,
                move |this: &mut Self, _, event: &SliderEvent, window, cx| match event {
                    SliderEvent::Change(value) | SliderEvent::Release(value) => {
                        if this.saving {
                            return;
                        }
                        let value = f64::from(value.start());
                        let row = &mut this.limits[kind as usize];
                        row.problem = None;
                        row.number
                            .update(cx, |state, cx| state.set_value(value.to_string(), window, cx));
                        if matches!(event, SliderEvent::Release(_)) {
                            this.commit_limit(kind, window, cx);
                        }
                        cx.notify();
                    }
                },
            ));
            LimitInput {
                kind,
                number,
                slider,
                problem: None,
            }
        });
        Self {
            device: None,
            settings: Settings::default(),
            limits: Vec::from(limits),
            saving: false,
            problem: None,
            overrides: None,
            checking_overrides: false,
            service_restart_needed: false,
            _subscriptions: subscriptions,
        }
    }

    fn sync_inputs(&mut self, window: &mut Window, cx: &mut App) {
        let limits = self.settings.orchestration.as_ref();
        for row in &mut self.limits {
            if row.number.read(cx).focus_handle(cx).is_focused(window) {
                continue;
            }
            let value = limits.and_then(|limits| row.kind.get(limits));
            let text = value.map(|value| value.to_string()).unwrap_or_default();
            if row.number.read(cx).value().as_ref() != text {
                row.number.update(cx, |state, cx| state.set_value(text, window, cx));
            }
            row.slider.update(cx, |state, cx| {
                state.set_value(row.kind.slider_value(value), window, cx);
            });
            row.problem = None;
        }
    }

    fn visible(&self) -> (bool, bool) {
        let subsessions = self.settings.subsessions_on();
        let limits = subsessions
            && self
                .settings
                .orchestration
                .as_ref()
                .is_some_and(|limits| limits.limits_on);
        (subsessions, limits)
    }

    fn locked(&self) -> bool {
        self.device.is_none() || self.saving || !crate::desktop_settings::writable()
    }

    fn save(&mut self, change: impl FnOnce(&mut Settings) + Send + 'static, window: &Window, cx: &mut Context<Self>) {
        if self.locked() {
            return;
        }
        let Some(device) = self.device.as_ref() else { return };
        let window = window.window_handle();
        self.saving = true;
        self.problem = None;
        save_settings(
            device,
            cx,
            move |settings| {
                change(settings);
                Ok(())
            },
            move |this, result, cx| {
                if let Err(error) = window.update(cx, |_, window, cx| {
                    this.finish_save(result.map(|(settings, ())| settings), window, cx);
                }) {
                    eprintln!("nexus-desktop-gpui: could not refresh subsession limits: {error}");
                }
            },
        );
        cx.notify();
    }

    fn finish_save(&mut self, result: Result<Settings, String>, window: &mut Window, cx: &mut App) {
        self.saving = false;
        match result {
            Ok(settings) => self.settings = settings,
            Err(error) => self.problem = Some(error),
        }
        self.sync_inputs(window, cx);
    }

    /// Parse the field and move the thumb while keeping the exact typed value for storage.
    fn read_limit(&mut self, kind: Limit, window: &mut Window, cx: &mut Context<Self>) -> Result<Option<f64>, String> {
        let row = &mut self.limits[kind as usize];
        let result = kind.parse(row.number.read(cx).value().as_ref());
        row.problem = result.as_ref().err().cloned();
        if let Ok(value) = &result {
            row.slider
                .update(cx, |state, cx| state.set_value(kind.slider_value(*value), window, cx));
        }
        cx.notify();
        result
    }

    fn commit_limit(&mut self, kind: Limit, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked() || !self.visible().1 {
            return;
        }
        if let Ok(value) = self.read_limit(kind, window, cx)
            && self.settings.orchestration.as_ref().and_then(|limits| kind.get(limits)) != value
        {
            self.save(
                move |settings| kind.set(settings.orchestration.get_or_insert_with(Default::default), value),
                window,
                cx,
            );
        }
    }

    fn refresh_overrides(&mut self, device: &DeviceCtx, clear: bool, cx: &mut Context<Self>) {
        let access = (device.code)(cx);
        if !clear {
            self.service_restart_needed = matches!(
                access.service,
                Some(nexus_remote::machine::Status::Connected { host, .. })
                    if !host.features.iter().any(|feature| feature == nexus_workbench::orchestra::LIMITS_FEATURE)
            );
        }
        let Some(projects) = access.projects else { return };
        self.checking_overrides = true;
        let answer = projects.session_start_overrides(clear);
        cx.spawn(async move |this, cx| {
            let result = answer
                .await
                .unwrap_or_else(|_| Err("Code stopped before it answered.".into()));
            this.update(cx, |this, cx| {
                this.checking_overrides = false;
                this.overrides = Some(result.map_err(|error| {
                    if error.contains("Unknown command wb_session_start_overrides") {
                        this.service_restart_needed = true;
                        "Restart the Nexus background service to count project choices.".into()
                    } else {
                        error
                    }
                }));
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn first_start(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let chips = [
            (Preference::Ask, "Ask first"),
            (Preference::New, "Always start a new session"),
            (Preference::Here, "Always do it here"),
        ];
        let picked = self.settings.subsessions().first_start;
        let locked = self.locked();
        let choices = h_flex()
            .flex_wrap()
            .gap(px(6.))
            .children(chips.map(|(preference, label)| {
                ui::chip(
                    SharedString::from(format!("subsessions-first-{preference:?}")),
                    label,
                    picked == preference,
                    palette,
                )
                .when(!locked && picked != preference, |chip| {
                    chip.on_click(cx.listener(move |this, _, window, cx| {
                        this.save(
                            move |settings| settings.subsessions_first_start = Some(preference),
                            window,
                            cx,
                        );
                    }))
                })
            }));
        let count = self.overrides.as_ref().and_then(|result| result.as_ref().ok()).copied();
        v_flex()
            .child(ui::setting(
                "When an agent hands off work",
                if self.service_restart_needed {
                    "Restart the Nexus background service to apply the first-start choice."
                } else {
                    "The default for projects on this computer."
                },
                choices,
                palette,
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .children(count.map(|count| match count {
                        1 => ui::note("1 project uses its own choice.", palette),
                        count => ui::note(format!("{count} projects use their own choice."), palette),
                    }))
                    .when(count.is_some_and(|count| count > 0), |row| {
                        row.child(
                            ui::button("subsessions-clear-overrides", "Use this for all projects", palette).when(
                                !locked && !self.checking_overrides,
                                |button| {
                                    button.on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(device) = this.device.clone() {
                                            this.refresh_overrides(&device, true, cx);
                                        }
                                    }))
                                },
                            ),
                        )
                    }),
            )
            .children(
                self.overrides
                    .as_ref()
                    .and_then(|result| result.as_ref().err())
                    .map(|error| ui::error_line(error.clone(), palette)),
            )
    }

    fn on_off(
        &self,
        id: &'static str,
        label: &'static str,
        detail: &'static str,
        on: bool,
        apply: impl FnOnce(&mut Settings, bool) + Copy + Send + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let palette = palette(cx);
        let locked = self.locked();
        let toggle = ui::switch(id, on, locked, palette).when(!locked, |toggle| {
            toggle.on_click(
                cx.listener(move |this, _, window, cx| this.save(move |settings| apply(settings, !on), window, cx)),
            )
        });
        ui::setting(label, detail, toggle, palette)
    }
}

impl Render for SubsessionsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let (subsessions, limits) = self.visible();
        let main_switch = self.on_off(
            "subsessions-switch",
            "Subsessions",
            "Let agents in new sessions start their own sessions.",
            subsessions,
            |settings, on| settings.subsessions = Some(on),
            cx,
        );
        let main_switch = match &self.device {
            Some(device) => device.settings_fields.field("subsessions", main_switch, None, cx),
            None => main_switch,
        };
        v_flex()
            .children(read_only_note(palette))
            .child(main_switch)
            .when(subsessions && self.device.is_some(), |page| {
                page.child(self.on_off(
                    "subsessions-cross-agent",
                    "Disable cross-agent subsessions",
                    "Only allow subsessions that use the same agent.",
                    self.settings.disable_cross_agent_subsessions,
                    |settings, on| settings.disable_cross_agent_subsessions = on,
                    cx,
                ))
                .child(self.first_start(palette, cx))
                .child(self.on_off(
                    "subsessions-limits",
                    "Limits",
                    "Set limits on subsessions.",
                    limits,
                    |settings, on| {
                        settings
                            .orchestration
                            .get_or_insert_with(OrchestrationLimits::default)
                            .limits_on = on;
                    },
                    cx,
                ))
                .when(limits, |page| {
                    let locked = self.locked();
                    let limits = self.settings.orchestration.as_ref().expect("limits are on");
                    page.child(ui::group(
                        self.limits.iter().map(|row| {
                            let value = row.kind.get(limits);
                            let control = h_flex()
                                .gap(px(12.))
                                .child(
                                    div()
                                        .w(px(114.))
                                        .when(value.is_none(), |slider| slider.opacity(0.45))
                                        .child(Slider::new(&row.slider).horizontal().disabled(locked)),
                                )
                                .child(div().w(px(110.)).child(Input::new(&row.number).disabled(locked)));
                            let detail = row.problem.clone().unwrap_or_else(|| {
                                if value == Some(0.) {
                                    row.kind.zero_detail()
                                } else {
                                    row.kind.detail()
                                }
                            });
                            Some(ui::setting_row(row.kind.label(), detail, control, palette).into_any_element())
                        }),
                        palette,
                    ))
                })
            })
            .children(self.problem.clone().map(|problem| ui::error_line(problem, palette)))
    }
}
