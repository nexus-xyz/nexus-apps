//! Startup: the window opens on what this computer already knows, never on a sign-in page it then leaves.
//!
//! `main.rs` reads the saved sign-in before the first window (`Gateway::preload`). The window then starts signed in
//! (the workspace, whose Gateway parts show their own loading states) or signed out. Signed out, the workspace's
//! sidebar collapses away and the sign-in card arrives, as it does at a sign-out. A keychain slower than the read's
//! deadline leaves the window on the startup shell ([`Auth::Restoring`]) until its answer comes. A sign-in that is
//! approved opens the sidebar again.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Auth, Workspace};
use crate::motion;
use crate::palette::Palette;

/// What the window starts on, from what the saved sign-in already says (`Backend::saved_sign_in`). None: not read yet.
pub(super) fn first_auth(saved: Option<bool>, remembered_local: bool) -> Auth {
    match saved {
        Some(true) => Auth::SignedIn,
        Some(false) if remembered_local => Auth::Local,
        Some(false) => Auth::SignedOut { error: None },
        None => Auth::Restoring,
    }
}

/// The collapse's curve: `cubic-bezier(0.4, 0, 1, 1)`, an exit that speeds up as it leaves.
fn ease_in(t: f32) -> f32 {
    motion::cubic_bezier(0.4, 0.0, 1.0, 1.0, t)
}

impl Workspace {
    /// The auth changed from `before` to what it is now: the sidebar collapses before the sign-in card, and opens
    /// again after an approved sign-in. The panel has no sidebar.
    pub(super) fn auth_moved(&mut self, before: &Auth) {
        if self.panel {
            return;
        }
        let card_before = matches!(before, Auth::SignedOut { .. } | Auth::Waiting { .. } | Auth::Checking);
        match self.auth {
            Auth::SignedOut { .. } if !card_before => self.collapse_to_sign_in(),
            Auth::SignedIn if card_before => {
                self.sign_in_collapse = None;
                // A first sign-in goes on to onboarding; the sidebar opens when it ends (`onboarding.rs`).
                if !self.onboarding_after_sign_in() && self.sidebar_open {
                    // The sidebar's own open animation (`render_sidebar`), from no width to its width.
                    self.sidebar_toggles += 1;
                }
            }
            Auth::SignedIn => {
                self.sign_in_collapse = None;
                // A slow keychain said the window is signed in: as a launch signed in.
                if matches!(before, Auth::Restoring) {
                    self.onboarding_at_launch();
                }
            }
            _ => {}
        }
    }

    /// Start the sidebar's collapse onto the sign-in page. Its mark unfurls again as the sidebar goes (`sign_in.rs`).
    pub(super) fn collapse_to_sign_in(&mut self) {
        if self.panel {
            return;
        }
        if let Some(form) = &mut self.sign_in_form {
            form.arrive();
        }
        // The welcome mark spins in again (`motion::SpinningMark`); a flow on screen goes, and resumes at the next sign-in.
        self.onboarding.welcome.again();
        self.onboarding.leave();
        self.sign_in_collapses += 1;
        self.sign_in_collapse = Some(self.sign_in_collapses);
    }

    /// The startup shell: the sidebar at its saved width, its rows still loading, and the empty page beside it.
    pub(super) fn render_startup_shell(&self, palette: Palette) -> AnyElement {
        h_flex()
            .id("startup-shell")
            .size_full()
            // The rows lie on the sidebar card (`render_sidebar_card`), the page on the window's ground.
            .when(self.sidebar_open, |shell| {
                shell.child(
                    div()
                        .h_full()
                        .flex_none()
                        .pl(px(crate::frost::INSET))
                        .pr(px(crate::frost::INSET))
                        .pb(px(crate::frost::INSET))
                        .child(self.render_loading_sidebar(palette)),
                )
            })
            .child(div().flex_1().h_full())
            .into_any_element()
    }

    /// A sidebar whose rows are still loading: the startup shell's, and the one that collapses onto the sign-in card.
    fn render_loading_sidebar(&self, palette: Palette) -> Div {
        let bar = |width: f32| div().h(px(10.)).w(relative(width)).rounded(px(4.)).bg(palette.selected);
        v_flex()
            .w(px(self.sidebar_width()))
            .flex_none()
            .h_full()
            .pt(px(14.))
            .px(px(16.))
            .gap(px(14.))
            .child(bar(0.45))
            .child(div().h(px(6.)))
            .child(bar(0.6))
            .child(bar(0.4))
            .child(bar(0.5))
            .child(div().h(px(10.)))
            .child(super::sidebar::render_skeleton(palette, 4))
    }

    /// The sign-in page, with the sidebar collapsing over its left edge when the window just left the workspace or
    /// the startup shell. Reduced motion shows the animation's end at once (`zoom.rs`).
    pub(super) fn render_signed_out(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let page = self.render_sign_in(palette, cx).into_any_element();
        let Some(run) = self.sign_in_collapse else {
            return page;
        };
        let width = self.sidebar_width();
        let sidebar = self
            .render_loading_sidebar(palette)
            .bg(palette.sidebar)
            .absolute()
            .top_0()
            .left_0()
            .with_animation(
                ("sign-in-collapse", run),
                Animation::new(motion::PANEL).with_easing(ease_in),
                move |sidebar, delta| sidebar.left(px(-width * delta)).opacity(1. - delta),
            );
        div()
            .relative()
            .size_full()
            .child(page)
            .when(self.sidebar_open, |stack| stack.child(sidebar))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // The standard test attribute: gpui-kit's glob import brings its own `test`.
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn the_window_starts_on_what_the_saved_sign_in_says() {
        assert!(matches!(first_auth(Some(true), false), Auth::SignedIn));
        assert!(matches!(first_auth(Some(true), true), Auth::SignedIn));
        assert!(matches!(first_auth(Some(false), true), Auth::Local));
        assert!(matches!(
            first_auth(Some(false), false),
            Auth::SignedOut { error: None }
        ));
        assert!(matches!(first_auth(None, false), Auth::Restoring));
    }

    #[test]
    fn the_collapse_starts_slow_and_ends_in_place() {
        assert!(ease_in(0.0).abs() < 1e-3);
        assert!((ease_in(1.0) - 1.0).abs() < 1e-3);
        assert!(ease_in(0.25) < 0.25);
    }
}
