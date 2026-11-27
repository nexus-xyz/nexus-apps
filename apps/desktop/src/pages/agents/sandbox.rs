//! Antigravity's Sandbox switch (Settings › Agents › Antigravity): agy starts with `--sandbox`, its terminal sandbox,
//! in every mode. It is the Nexus app's `antigravitySandbox` setting, off by default. The workbench reads it each time
//! agy starts (`GpuiHost::antigravity_sandbox`), and an open session whose agy started with the other value starts agy
//! again at its next message sent between turns (`Workbench::antigravity_args_changed`).

use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::super::desktop_app::{off_thread, save_settings};
use super::super::{DeviceCtx, palette, ui};

/// What the switch says.
const DETAIL: &str = "Applies to new Antigravity sessions, and to open ones from their next message.";

pub(super) struct Sandbox {
    ctx: DeviceCtx,
    /// The saved switch; None until the settings are read.
    on: Option<bool>,
    saving: bool,
    error: Option<SharedString>,
}

impl Sandbox {
    pub(super) fn new(ctx: DeviceCtx, cx: &mut Context<Self>) -> Self {
        off_thread(cx, crate::desktop_settings::load, |this: &mut Sandbox, settings, _| {
            this.on = Some(settings.antigravity_sandboxed());
        });
        Sandbox {
            ctx,
            on: None,
            saving: false,
            error: None,
        }
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        let Some(on) = self.on.map(|on| !on) else {
            return;
        };
        if self.saving || !crate::desktop_settings::writable() {
            return;
        }
        self.saving = true;
        self.error = None;
        save_settings(
            &self.ctx,
            cx,
            move |settings| {
                settings.antigravity_sandbox = Some(on);
                Ok(())
            },
            |this, saved, _| {
                this.saving = false;
                match saved {
                    Ok((settings, ())) => this.on = Some(settings.antigravity_sandboxed()),
                    Err(error) => this.error = Some(error.into()),
                }
            },
        );
        cx.notify();
    }
}

impl Render for Sandbox {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let locked = self.on.is_none() || self.saving || !crate::desktop_settings::writable();
        let switch = ui::switch("agents-antigravity-sandbox", self.on.unwrap_or(false), locked, palette);
        let control = if locked {
            switch.into_any_element()
        } else {
            switch
                .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)))
                .into_any_element()
        };
        v_flex()
            .child(ui::setting_row("Sandbox", DETAIL, control, palette))
            .children(
                self.error
                    .clone()
                    .map(|error| ui::error_line(error, palette).px(px(16.)).pb(px(10.))),
            )
    }
}
