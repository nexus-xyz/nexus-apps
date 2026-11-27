//! A verified update, offered in the app window, as the Nexus app's update toast: a toast at its
//! bottom right ("Nexus 0.9.0 is ready") with Restart and Not now. When it shows is `Snapshot::toast_offer`
//! (nexus-desktop-updates), the Rust copy of the Nexus app's `updateOffer`: a verified update, held through a
//! background check and the restart, and gone after Not now until the next launch or a newer version. Settings › App
//! keeps the offer either way. The panel shows no toast (its Settings has the update), so a restart is offered in one
//! place at a time. Never a system notification: it matches the app on every OS.
//!
//! Every packaged platform offers Restart once the update's signature has been verified.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_updates::{Phase, Snapshot};

use super::Workspace;
use crate::pages::ui;
use crate::palette::Palette;

/// Sonner's toast width and offset, as the Nexus app draws it (`offset={20}`).
const WIDTH: f32 = 356.;
const OFFSET: f32 = 20.;

/// The toast's state in one app window: the latest update state, and the last restart's problem.
#[derive(Default)]
pub struct UpdateToast {
    snapshot: Option<Snapshot>,
    /// The version offered, so a new one clears the problem, as the Nexus app's `useEffect` on `version`.
    version: Option<String>,
    /// Why Restart did not start.
    problem: Option<SharedString>,
    /// Restart was pressed: an install that fails later says why here too.
    tried: bool,
    _watch: Option<Task<()>>,
}

/// What the main action does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    /// Install the verified update and start it.
    Restart { waiting: bool },
    /// Open the release's download if a feed only announces it.
    Download(String),
}

/// What the toast says and offers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Content {
    title: String,
    note: String,
    problem: bool,
    action: Action,
    /// The restart has begun: Not now is off.
    installing: bool,
}

/// The toast for `snapshot`, or None when nothing is offered. `download` is the release's address on a platform that
/// only announces; `problem` is why the last restart did not start.
fn content(snapshot: &Snapshot, download: Option<String>, problem: Option<&str>) -> Option<Content> {
    let version = snapshot.toast_offer()?;
    let installing = snapshot.phase == Phase::Installing;
    if let Some(url) = download {
        return Some(Content {
            title: format!("Nexus {version} is available"),
            note: "Download it, then replace this app with it.".into(),
            problem: false,
            action: Action::Download(url),
            installing: false,
        });
    }
    let note = match problem {
        Some(problem) => problem.to_string(),
        None if installing => "Restarting…".into(),
        None => "Restart to update".into(),
    };
    Some(Content {
        title: format!("Nexus {version} is ready"),
        note,
        problem: problem.is_some(),
        // A background check runs: the restart waits until it is back to "ready", a second or two.
        action: Action::Restart {
            waiting: snapshot.phase != Phase::Ready,
        },
        installing,
    })
}

impl UpdateToast {
    /// An app window's toast, following the updater. The panel's has none.
    pub(super) fn new(panel: bool, cx: &mut Context<Workspace>) -> Self {
        if panel {
            return Self::default();
        }
        let snapshot = crate::updates::snapshot();
        let watch = crate::updates::subscribe().map(|mut changes| {
            cx.spawn(async move |this, cx| {
                while changes.changed().await.is_ok() {
                    let snapshot = changes.borrow_and_update().clone();
                    if this
                        .update(cx, |workspace, cx| {
                            workspace.update_toast.show(snapshot);
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
        });
        let mut toast = Self {
            _watch: watch,
            ..Self::default()
        };
        if let Some(snapshot) = snapshot {
            toast.show(snapshot);
        }
        toast
    }

    fn show(&mut self, snapshot: Snapshot) {
        let version = snapshot.toast_offer().map(str::to_string);
        if version != self.version {
            self.version = version;
            self.problem = None;
            self.tried = false;
        }
        self.snapshot = Some(snapshot);
    }

    /// The update state now, in an app window.
    pub(super) fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    /// Why the restart did not happen: its refusal, or an install that failed after it began.
    fn problem(&self) -> Option<String> {
        if let Some(problem) = &self.problem {
            return Some(problem.to_string());
        }
        let snapshot = self.snapshot.as_ref()?;
        snapshot
            .error
            .as_ref()
            .filter(|error| self.tried && snapshot.phase == Phase::Ready && error.code == "install_failed")
            .map(|error| error.message.clone())
    }
}

impl Workspace {
    fn restart_from_toast(&mut self, cx: &mut Context<Self>) {
        self.update_toast.problem = None;
        self.update_toast.tried = true;
        if let Err(message) = self.restart_for_update(cx) {
            self.update_toast.problem = Some(message.into());
        }
        cx.notify();
    }

    /// The toast at the window's bottom right, over everything but the dialogs.
    pub(super) fn render_update_toast(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        // A withdrawn build's notice (`revoked_notice.rs`) offers the restart in its place.
        if self.has_update_recovery(cx) || self.revoked_notice.shown() {
            return None;
        }
        let snapshot = self.update_toast.snapshot.as_ref()?;
        let problem = self.update_toast.problem();
        let content = content(snapshot, crate::updates::download_url(), problem.as_deref())?;
        let action = match &content.action {
            Action::Restart { waiting: true } => ui::disabled_button("update-toast-restart", "Restart", palette),
            Action::Restart { waiting: false } => ui::primary_button("update-toast-restart", "Restart", palette)
                .on_click(cx.listener(|this, _, _, cx| this.restart_from_toast(cx))),
            Action::Download(url) => {
                let url = url.clone();
                ui::primary_button("update-toast-download", "Download", palette)
                    .on_click(move |_, _, cx| crate::external_link::open(&url, cx))
            }
        };
        let later = if content.installing {
            ui::disabled_button("update-toast-later", "Not now", palette)
        } else {
            ui::button("update-toast-later", "Not now", palette).on_click(|_, _, _| crate::updates::later())
        };
        let card = h_flex()
            .id("update-toast")
            .occlude()
            .w(px(WIDTH))
            .gap(px(8.))
            .py(px(12.))
            .pl(px(16.))
            .pr(px(12.))
            .rounded(px(12.))
            .bg(palette.pop)
            .border_1()
            .border_color(palette.border)
            .shadow(palette.pop_shadow(palette.border))
            .text_color(palette.foreground)
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
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
                    ),
            )
            .child(later)
            .child(action);
        // It rises 12px and fades in, as the Nexus app's (`.nx-toast`); not under Reduce motion.
        let card = if crate::paths::reduce_motion() {
            card.into_any_element()
        } else {
            crate::motion::arrive(
                div().child(card),
                SharedString::from(format!("update-toast-{}", content.title)),
                12.,
            )
            .into_any_element()
        };
        Some(
            deferred(
                div()
                    .absolute()
                    .bottom(px(OFFSET))
                    .right(px(OFFSET))
                    .max_w(relative(0.94))
                    .child(card),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: the kit's glob brings its own `test` attribute.
    use super::{Action, UpdateToast, content};
    use nexus_desktop_updates::{App, Coordinator, Failure, Phase, Snapshot};

    fn ready() -> Snapshot {
        let mut c = Coordinator::of(App::Desktop, "0.1.2".into(), None, 0);
        assert!(c.begin(nexus_desktop_updates::Trigger::Manual, 0));
        c.finish(Ok(Some("0.1.3".into())), 0, 0);
        c.snapshot
    }

    #[test]
    fn a_ready_update_offers_restart_as_the_nexus_app_words_it() {
        let toast = content(&ready(), None, None).unwrap();
        assert_eq!(toast.title, "Nexus 0.1.3 is ready");
        assert_eq!(toast.note, "Restart to update");
        assert_eq!(toast.action, Action::Restart { waiting: false });
        assert!(!toast.installing);
    }

    #[test]
    fn a_background_check_holds_the_toast_and_the_restart_waits() {
        let snapshot = Snapshot {
            phase: Phase::Checking,
            ..ready()
        };
        assert_eq!(
            content(&snapshot, None, None).unwrap().action,
            Action::Restart { waiting: true }
        );
        let installing = content(
            &Snapshot {
                phase: Phase::Installing,
                ..ready()
            },
            None,
            None,
        )
        .unwrap();
        assert_eq!(installing.note, "Restarting…");
        assert!(installing.installing);
    }

    #[test]
    fn not_now_hides_it_and_nothing_shows_before_a_verified_update() {
        assert_eq!(
            content(
                &Snapshot {
                    deferred: true,
                    ..ready()
                },
                None,
                None
            ),
            None
        );
        let idle = Coordinator::of(App::Desktop, "0.1.2".into(), None, 0).snapshot;
        assert_eq!(content(&idle, None, None), None);
    }

    #[test]
    fn a_refused_restart_says_why_in_red() {
        let toast = content(&ready(), None, Some("Stop the running sessions first.")).unwrap();
        assert_eq!(toast.note, "Stop the running sessions first.");
        assert!(toast.problem);
    }

    #[test]
    fn macos_offers_the_download_in_place_of_restart() {
        let url = "https://downloads.nexus.inc/desktop/stable/Nexus.dmg".to_string();
        let toast = content(&ready(), Some(url.clone()), None).unwrap();
        assert_eq!(toast.title, "Nexus 0.1.3 is available");
        assert_eq!(toast.action, Action::Download(url));
    }

    #[test]
    fn a_failed_install_shows_only_after_restart_and_a_new_version_clears_it() {
        let mut toast = UpdateToast::default();
        let mut failed = ready();
        failed.error = Some(Failure::new("install_failed", "Installation failed: disk full.", false));
        toast.show(failed.clone());
        assert_eq!(toast.problem(), None);
        toast.tried = true;
        assert_eq!(toast.problem().as_deref(), Some("Installation failed: disk full."));
        toast.problem = Some("Refused.".into());
        toast.show(Snapshot {
            ready_version: Some("0.1.4".into()),
            ..failed
        });
        assert_eq!(toast.problem(), None);
        assert!(!toast.tried);
    }
}
