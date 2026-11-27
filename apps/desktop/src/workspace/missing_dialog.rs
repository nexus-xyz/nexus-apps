//! "<repo> isn't on this computer": a repository picked to run where it has no copy. The choice blocks
//! the screen, so it is a dialog and not a card above the composer: clone it there (the default, Enter), or run the
//! session in the cloud, where nothing downloads. Clone is the dialog's second step, with the repository fixed and
//! only the folder to pick.

use crate::workspace::glass::FloatingGround;
use crate::workspace::overlay::OverlaySlot;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{View, Workspace, glyph, menu_item_hover};
use crate::motion;
use crate::palette::Palette;

impl Workspace {
    /// The dialog shows while a repository waits for a choice on the new session's screen.
    fn missing_open(&self) -> bool {
        matches!(self.view, View::NewCode) && !self.cloud_ui.on && self.cloud_ui.missing.is_some()
    }

    /// Where the repository is missing: this computer, or the server it was picked for.
    fn missing_place(&self) -> String {
        self.cloud_ui
            .missing_on
            .as_deref()
            .and_then(|id| self.servers.iter().find(|server| server.id == id))
            .map_or_else(|| "this computer".to_string(), |server| server.name.to_string())
    }

    /// The clone form the second step shows, while it is open.
    fn missing_clone_form(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.cloud_ui.missing_on.clone() {
            Some(server) => self.render_server_folder(&server, true, palette, cx),
            None => self.render_menu_clone(palette, cx),
        }
    }

    fn missing_cloning(&self) -> bool {
        match &self.cloud_ui.missing_on {
            Some(_) => self
                .server_folder
                .as_ref()
                .is_some_and(|folder| folder.in_menu && folder.clone.is_some()),
            None => self.clone_form.as_ref().is_some_and(|form| form.in_menu),
        }
    }

    /// Clone to <place>: the dialog's second step.
    fn missing_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.cloud_ui.missing.clone() else {
            return;
        };
        let on = self.cloud_ui.missing_on.clone();
        self.clone_here(repo.url, repo.name, on, window, cx);
        self.overlay.close_launch_menu();
        cx.notify();
    }

    /// Back to the two choices. A clone that runs keeps its form.
    fn missing_back(&mut self, cx: &mut Context<Self>) {
        if self.cloud_ui.missing_on.is_some() {
            self.drop_idle_menu_server_folder();
        } else {
            self.drop_idle_menu_clone();
        }
        cx.notify();
    }

    /// Cancel: no repository picked, and the session stays where it was asked to run.
    fn missing_cancel(&mut self, cx: &mut Context<Self>) {
        if self.cloning() || self.server_folder.as_ref().is_some_and(|folder| folder.cloning()) {
            return;
        }
        self.missing_back(cx);
        self.cloud_ui.missing = None;
        self.cloud_ui.missing_on = None;
        cx.notify();
    }

    /// Esc steps back, then cancels; Enter on the first step clones. Every other key stays out of the composer under
    /// the dialog. Answers whether the key was the dialog's.
    pub(super) fn missing_dialog_key(&mut self, key: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.missing_open() {
            return false;
        }
        let step_two = self.missing_cloning();
        match key.key.as_str() {
            "escape" if step_two => self.missing_back(cx),
            "escape" => self.missing_cancel(cx),
            "enter" if !step_two => self.missing_clone(window, cx),
            // The clone form's fields take their own keys.
            _ if step_two => return false,
            _ => {}
        }
        cx.stop_propagation();
        true
    }

    pub(super) fn render_missing_dialog(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.missing_open() {
            return None;
        }
        let repo = self.cloud_ui.missing.as_ref()?;
        let place = self.missing_place();
        let step_two = self.missing_cloning();
        let body = if step_two {
            v_flex()
                .gap(px(8.))
                .child(
                    h_flex()
                        .id("missing-back")
                        .gap(px(6.))
                        .px(px(4.))
                        .h(px(24.))
                        .rounded(px(6.))
                        .text_size(px(12.5))
                        .text_color(palette.muted)
                        .cursor_pointer()
                        .hover(|style| menu_item_hover(style, palette).text_color(palette.foreground))
                        .on_click(cx.listener(|this, _, _, cx| this.missing_back(cx)))
                        .child(glyph(Lucide::ArrowLeft, palette.icon).size(px(14.)))
                        .child(format!("Clone to {place}")),
                )
                .children(self.missing_clone_form(palette, cx))
                .into_any_element()
        } else {
            v_flex()
                .gap(px(14.))
                .child(
                    v_flex()
                        .gap(px(4.))
                        .px(px(4.))
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(format!("{} isn't on {place}", repo.name)),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(palette.muted)
                                .child("Clone it here, or run the session in the cloud with nothing downloaded."),
                        ),
                )
                .child(
                    v_flex()
                        .gap(px(2.))
                        .child(
                            choice(
                                "missing-clone",
                                Lucide::Download,
                                format!("Clone to {place}"),
                                "Pick a folder, then the session starts there",
                                true,
                                palette,
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.missing_clone(window, cx))),
                        )
                        .child(
                            choice(
                                "missing-cloud",
                                Lucide::Cloud,
                                "Run in the cloud".into(),
                                "Nothing downloads to this computer",
                                false,
                                palette,
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.pick_cloud(window, cx))),
                        ),
                )
                .child(
                    h_flex().justify_end().child(
                        h_flex()
                            .id("missing-cancel")
                            .gap(px(6.))
                            .h(px(28.))
                            .px(px(10.))
                            .rounded(px(6.))
                            .text_size(px(12.5))
                            .text_color(palette.muted)
                            .cursor_pointer()
                            .hover(|style| menu_item_hover(style, palette).text_color(palette.foreground))
                            .on_click(cx.listener(|this, _, _, cx| this.missing_cancel(cx)))
                            .child("Cancel")
                            .child(key_hint("esc", palette)),
                    ),
                )
                .into_any_element()
        };
        // Opens over 200ms: the screen dims, and the dialog fades in as it rises 6px (`motion::UI_LONG`). A step
        // change fades the new step in over 160ms (`motion::UI`).
        let dialog = v_flex()
            .id("missing-dialog")
            .occlude()
            .w(px(440.))
            .p(px(16.))
            .rounded(px(16.))
            .floating_ground(16., palette.menu, true, &palette)
            .border_1()
            .border_color(palette.menu_ring())
            .shadow(palette.pop_drop())
            .text_color(palette.foreground)
            .text_size(px(13.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(motion::fade_in(
                div().child(body),
                if step_two { "missing-step-2" } else { "missing-step-1" },
            ));
        let reduce = cx.reduce_motion();
        Some(
            deferred(
                div()
                    .id("missing-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .bg(palette.scrim)
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.missing_cancel(cx)))
                    .child(dialog.with_animation(
                        "missing-in",
                        Animation::new(motion::UI_LONG).with_easing(motion::ease_out),
                        move |dialog, delta| {
                            let rise = if reduce { 0. } else { 6. * (1. - delta) };
                            dialog.opacity(delta).mt(px(rise))
                        },
                    ))
                    .with_animation(
                        "missing-scrim-in",
                        Animation::new(motion::UI_LONG).with_easing(motion::ease_out),
                        |overlay, delta| overlay.opacity(delta),
                    ),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

/// One of the dialog's two choices: an icon, the action and what it means; the default one carries Enter's mark.
fn choice(
    id: &'static str,
    icon: Lucide,
    title: String,
    hint: &'static str,
    default: bool,
    palette: Palette,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .gap(px(12.))
        .p(px(10.))
        .rounded(px(10.))
        .cursor_pointer()
        .when(default, |row| row.bg(palette.menu_hover))
        .hover(|style| menu_item_hover(style, palette))
        .child(
            div()
                .flex_none()
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .bg(palette.raised)
                .child(glyph(icon, palette.icon).size(px(15.))),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(px(1.))
                .child(div().text_size(px(13.5)).child(title))
                .child(div().text_size(px(12.)).text_color(palette.faint).child(hint)),
        )
        .when(default, |row| row.child(key_hint("↵", palette)))
}

fn key_hint(key: &'static str, palette: Palette) -> Div {
    div()
        .flex_none()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(palette.raised)
        .text_size(px(11.))
        .text_color(palette.muted)
        .child(key)
}
