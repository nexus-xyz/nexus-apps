//! The notice for a withdrawn build (`revoked.rs`), as the Nexus app's `desktop/src/revoked-notice.tsx`: one card at
//! the bottom center of each app window, over every page, until the app restarts. It has no Close: the build stays
//! withdrawn. It says Gateway's words, follows the update check that the refusal started, and offers the way out:
//! Restart to update when a verified update is ready, else the download page (`nexus_desktop_updates::DOWNLOAD_PAGE`,
//! or the internal channel's install command). Sign out stays on it, since the account page cannot load. The update
//! toast (`update_toast.rs`) gives way to it, so a restart is offered in one place.
//!
//! It locks nothing, as the Nexus app's did not: what needs no Gateway keeps working under it.

use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_updates::{DOWNLOAD_PAGE, Phase, Snapshot};

use super::{Auth, Workspace};
use crate::pages::ui;
use crate::palette::Palette;
use crate::revoked::{Latch, Revoked};

const WIDTH: f32 = 480.;
const OFFSET: f32 = 20.;

/// The notice's state in one app window: the refusal it shows, and the last action's problem.
#[derive(Default)]
pub struct RevokedNotice {
    revoked: Option<Revoked>,
    /// Why Restart to update did not start.
    problem: Option<SharedString>,
    /// The install command was copied.
    copied: bool,
    _watch: Option<Task<()>>,
}

/// What the main action does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Offer {
    /// Install the verified update and start it.
    Restart,
    /// The restart has begun.
    Restarting,
    /// The update check runs, or the update downloads or is verified.
    Waiting,
    /// Open this address in the browser: the release itself, or the download page.
    Download(String),
    /// Copy the internal channel's install command.
    Command(String),
}

/// What the notice says and offers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Content {
    title: String,
    note: String,
    problem: bool,
    offer: Offer,
    /// Check again, once a check ended without an update.
    check_again: bool,
}

const NO_UPDATE: &str = "No update was found. Download the latest Nexus, then replace this app with it.";

/// The notice for `revoked` while the updater is at `snapshot` (`None`: this build has no updater). `verified` says the
/// ready release is verified bytes; `announced` is its address on a platform that only announces it; `problem` is why
/// the last restart did not start.
fn content(
    revoked: &Revoked,
    snapshot: Option<&Snapshot>,
    verified: bool,
    announced: Option<String>,
    problem: Option<&str>,
) -> Content {
    let title = revoked.message.clone();
    let manual = |snapshot: Option<&Snapshot>| {
        let manual = snapshot.and_then(|snapshot| snapshot.manual.as_ref());
        match manual.and_then(|manual| manual.install_command) {
            Some(command) => Offer::Command(command.to_string()),
            None => Offer::Download(
                manual
                    .and_then(|manual| manual.download_url)
                    .unwrap_or(DOWNLOAD_PAGE)
                    .to_string(),
            ),
        }
    };
    let Some(snapshot) = snapshot else {
        return Content {
            title,
            note: problem.unwrap_or(NO_UPDATE).to_string(),
            problem: problem.is_some(),
            offer: manual(None),
            check_again: false,
        };
    };
    let status = |announced: bool| crate::updates::status(snapshot, announced);
    let (note, offer, check_again) = match snapshot.phase {
        Phase::Ready if announced.is_some() => (status(true), Offer::Download(announced.unwrap_or_default()), false),
        Phase::Ready if verified => (status(false), Offer::Restart, false),
        Phase::Installing => (status(false), Offer::Restarting, false),
        Phase::Checking | Phase::Downloading | Phase::Verifying => (status(false), Offer::Waiting, false),
        Phase::Disabled => (status(false), manual(Some(snapshot)), false),
        Phase::Error => (
            snapshot
                .error
                .as_ref()
                .map(|error| error.message.clone())
                .unwrap_or_else(|| status(false)),
            manual(Some(snapshot)),
            true,
        ),
        Phase::Idle | Phase::Ready => (NO_UPDATE.to_string(), manual(Some(snapshot)), true),
    };
    Content {
        title,
        note: problem.map(str::to_string).unwrap_or(note),
        problem: problem.is_some(),
        offer,
        check_again,
    }
}

impl RevokedNotice {
    /// An app window's notice, following `latch`. The panel shows none: its window is the chat alone.
    pub(super) fn new(panel: bool, latch: Option<Arc<Latch>>, cx: &mut Context<Workspace>) -> Self {
        let Some(latch) = latch.filter(|_| !panel) else {
            return Self::default();
        };
        let mut notice = Self::default();
        notice.accept(latch.current());
        let mut changes = latch.subscribe();
        notice._watch = Some(cx.spawn(async move |this, cx| {
            while changes.changed().await.is_ok() {
                let revoked = changes.borrow_and_update().clone();
                let updated = this.update(cx, |workspace, cx| {
                    if workspace.revoked_notice.accept(revoked) {
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        }));
        notice
    }

    /// Keep the first refusal; true when the notice changed. Later ones change nothing, so it shows once.
    fn accept(&mut self, revoked: Option<Revoked>) -> bool {
        if self.revoked.is_some() || revoked.is_none() {
            return false;
        }
        self.revoked = revoked;
        true
    }

    /// Whether this window shows the notice.
    pub(super) fn shown(&self) -> bool {
        self.revoked.is_some()
    }
}

impl Workspace {
    fn restart_from_revoked_notice(&mut self, cx: &mut Context<Self>) {
        self.revoked_notice.problem = None;
        if let Err(message) = self.restart_for_update(cx) {
            self.revoked_notice.problem = Some(message.into());
        }
        cx.notify();
    }

    /// The notice at the window's bottom center, over everything but the dialogs.
    pub(super) fn render_revoked_notice(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let revoked = self.revoked_notice.revoked.as_ref()?;
        let snapshot = self.update_toast.snapshot();
        let content = content(
            revoked,
            snapshot,
            crate::updates::offered().is_some(),
            crate::updates::download_url(),
            self.revoked_notice.problem.as_deref(),
        );
        let action = match &content.offer {
            Offer::Restart => ui::primary_button("revoked-restart", "Restart to update", palette)
                .on_click(cx.listener(|this, _, _, cx| this.restart_from_revoked_notice(cx))),
            Offer::Restarting => ui::disabled_button("revoked-restart", "Restarting…", palette),
            Offer::Waiting => ui::disabled_button("revoked-restart", "Updating…", palette),
            Offer::Download(url) => {
                let url = url.clone();
                ui::primary_button("revoked-download", "Download", palette)
                    .on_click(move |_, _, cx| crate::external_link::open(&url, cx))
            }
            Offer::Command(command) => {
                let command = command.clone();
                let label = if self.revoked_notice.copied {
                    "Copied"
                } else {
                    "Copy install command"
                };
                ui::primary_button("revoked-command", label, palette).on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
                    this.revoked_notice.copied = true;
                    cx.notify();
                }))
            }
        };
        let check = content
            .check_again
            .then(|| ui::button("revoked-check", "Check again", palette).on_click(|_, _, _| crate::updates::check()));
        let sign_out = matches!(self.auth, Auth::SignedIn).then(|| {
            ui::ghost_button("revoked-sign-out", "Sign out", palette).on_click(cx.listener(|this, _, _, cx| {
                this.request_sign_out();
                cx.notify();
            }))
        });
        let command = match &content.offer {
            Offer::Command(command) => Some(
                ui::selectable_note("revoked-command-text", command.clone(), palette)
                    .font_family("monospace")
                    .text_size(px(11.)),
            ),
            _ => None,
        };
        let card = v_flex()
            .id("revoked-notice")
            .debug_selector(|| "revoked-notice".into())
            .occlude()
            .w(px(WIDTH))
            .gap(px(10.))
            .p(px(16.))
            .rounded(px(12.))
            .bg(palette.raised)
            .border_1()
            .border_color(palette.warning)
            .shadow(palette.pop_shadow(palette.border))
            .text_color(palette.foreground)
            .child(
                h_flex()
                    .items_start()
                    .gap(px(10.))
                    .child(
                        ui::icon(Lucide::TriangleAlert, palette.warning)
                            .size(px(16.))
                            .flex_none(),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(palette.strong)
                                    .child(content.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(if content.problem {
                                        palette.destructive
                                    } else {
                                        palette.muted
                                    })
                                    .child(content.note.clone()),
                            )
                            .children(command),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .justify_end()
                    .children(sign_out)
                    .children(check)
                    .child(action),
            );
        Some(
            deferred(
                h_flex()
                    .absolute()
                    .bottom(px(OFFSET))
                    .left_0()
                    .right_0()
                    .justify_center()
                    .child(div().max_w(relative(0.94)).child(card)),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "revoked_notice_tests.rs"]
mod tests;
