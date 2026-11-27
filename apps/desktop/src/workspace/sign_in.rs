//! Sign-in: the F5 mark over one column, with the browser sign-in or an
//! API key, the one-time code as tiles with its countdown while the browser approves, and "Get a new code" once it
//! expires. There is no card: the mark's field turns faintly behind the column, over a soft light well on dark.
//!
//! Motion, after the cold start's (`startup.rs`): the sidebar collapses away while the welcome mark spins in, and the
//! text rises after it, 60 ms apart. The mark then turns slowly while the screen waits (`motion::SpinningMark`,
//! as onboarding's). Once a sign-in starts, the breathing mark takes over in the same place:
//! it breathes while the browser opens and the code waits, and eases to rest when that stops. A state change rises its
//! column in again. Less motion shows each state at once, still. On a first run, onboarding's progress sits at the top.
//!
//! The Gateway and the version sit in one quiet line at the foot. Internal builds (`channel.rs`) change the Gateway
//! from there; an organization's managed Gateway locks it.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::sign_in_parts::{self as parts, Button};
use super::{Auth, Workspace};
use crate::motion;
use crate::palette::Palette;

/// How long "Copied" stays.
const COPIED: Duration = Duration::from_millis(1600);
/// The mark over the column, in every state, so the breathing mark takes over from the welcome mark in place.
const MARK: f32 = super::onboarding::MARK;
/// How far apart the column's parts rise in.
const STAGGER: Duration = Duration::from_millis(60);
/// After the welcome mark starts to spin in, when the text starts to rise: as the sidebar's collapse (`motion::PANEL`)
/// ends.
const AFTER_MARK: Duration = Duration::from_millis(360);

pub(super) struct SignInForm {
    /// The API key form instead of the browser's.
    key_mode: bool,
    gateway: Entity<InputState>,
    /// Internal builds: the Gateway field is open (Change, in the foot).
    gateway_open: bool,
    key: Entity<InputState>,
    copied: Option<Instant>,
    /// How long the code on screen lasts, for its countdown bar.
    life: Duration,
    /// A sign-in started since the screen arrived: the breathing mark has taken over from the unfurl.
    started: bool,
    _tick: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// The managed Gateway, when the organization locked it.
pub(super) fn locked_url() -> Option<String> {
    crate::managed::locked_gateway(crate::managed::policy())
}

impl SignInForm {
    pub(super) fn new(url: &str, window: &mut Window, cx: &mut Context<Workspace>) -> SignInForm {
        let gateway = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("https://api.nexus.inc");
            state.set_value(url.to_string(), window, cx);
            state
        });
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("nx_…").masked(true));
        let submit = |this: &mut Workspace,
                      _: &Entity<InputState>,
                      event: &InputEvent,
                      window: &mut Window,
                      cx: &mut Context<Workspace>| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit_sign_in(window, cx);
            }
        };
        SignInForm {
            key_mode: std::env::args().any(|arg| arg == "--sign-in-key"),
            _subscriptions: vec![
                cx.subscribe_in(&gateway, window, submit),
                cx.subscribe_in(&key, window, submit),
            ],
            gateway,
            gateway_open: false,
            key,
            copied: None,
            life: Duration::from_secs(600),
            started: false,
            _tick: None,
        }
    }

    /// The screen arrives again (a sign-out, a cold start signed out): the mark unfurls once more.
    pub(super) fn arrive(&mut self) {
        self.started = false;
    }
}

/// What the column shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Browser,
    Key,
    Waiting,
    Expired,
    Approved,
}

impl Step {
    fn key(self) -> &'static str {
        match self {
            Step::Browser => "browser",
            Step::Key => "key",
            Step::Waiting => "waiting",
            Step::Expired => "expired",
            Step::Approved => "approved",
        }
    }
}

impl Workspace {
    /// A code arrived: wait for approval, with a countdown that ticks every second.
    pub(super) fn sign_in_code(
        &mut self,
        user_code: String,
        link: String,
        expires_in: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::external_link::open(&link, cx);
        let seconds = if expires_in.is_finite() && expires_in > 0. {
            expires_in
        } else {
            600.
        };
        self.auth = Auth::Waiting {
            user_code: user_code.into(),
            link: link.into(),
            deadline: cx.background_executor().now() + Duration::from_secs_f64(seconds),
        };
        if let Some(form) = &mut self.sign_in_form {
            form.copied = None;
            form.life = Duration::from_secs_f64(seconds);
            form._tick = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let waiting = this
                        .update_in(cx, |this, window, cx| {
                            if this.local_mode.signing_in
                                && matches!(this.auth, Auth::Waiting { deadline, .. } if deadline <= cx.background_executor().now())
                            {
                                this.gateway.cancel_device_sign_in();
                                this.finish_sign_in_attempt(Some("The sign-in code expired.".into()), window, cx);
                            }
                            cx.notify();
                            matches!(this.auth, Auth::Waiting { .. })
                        })
                        .unwrap_or(false);
                    if !waiting {
                        break;
                    }
                }
            }));
        }
    }

    pub(super) fn submit_browser_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.sign_in_form {
            form.key_mode = false;
        }
        self.submit_sign_in(window, cx);
    }

    pub(super) fn cancel_sign_in_attempt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gateway.cancel_device_sign_in();
        self.finish_sign_in_attempt(None, window, cx);
    }

    /// Sign in the way the form shows: the browser, or the API key. On an internal build, a changed Gateway is saved
    /// first and the app starts again on it, then signs in there.
    pub(super) fn submit_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.sign_in_form else { return };
        if matches!(self.auth, Auth::Restoring | Auth::Checking | Auth::Waiting { .. }) {
            return;
        }
        form.started = true;
        let key_mode = form.key_mode;
        if crate::channel::internal() && locked_url().is_none() {
            let typed = form.gateway.read(cx).value().trim().trim_end_matches('/').to_string();
            if !typed.is_empty() && typed != self.gateway.url().trim_end_matches('/') {
                if !(typed.starts_with("https://") || typed.starts_with("http://")) {
                    form.gateway_open = true;
                    self.auth = Auth::SignedOut {
                        error: Some("The Gateway address starts with https://.".into()),
                    };
                    cx.notify();
                    return;
                }
                crate::paths::save_app_gateway_url(&typed);
                restart(if key_mode { "--sign-in-key" } else { "--sign-in" }, cx);
                return;
            }
        }
        if key_mode {
            let key = form.key.read(cx).value().to_string();
            self.auth = Auth::Checking;
            self.gateway.begin_key_sign_in(key);
        } else {
            self.auth = Auth::Checking;
            self.gateway.begin_device_sign_in();
        }
        let _ = window;
        cx.notify();
    }

    pub(super) fn render_sign_in(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(form) = &self.sign_in_form else {
            return div().size_full().into_any_element();
        };
        let expired = matches!(&self.auth, Auth::Waiting { deadline, .. } if deadline.saturating_duration_since(Instant::now()).is_zero());
        let step = match &self.auth {
            Auth::Waiting { .. } if expired => Step::Expired,
            Auth::Waiting { .. } => Step::Waiting,
            Auth::SignedIn => Step::Approved,
            _ if form.key_mode => Step::Key,
            _ => Step::Browser,
        };
        let busy = matches!(self.auth, Auth::Checking | Auth::Restoring);
        // The first arrival unfurls the mark, and the text waits for it; later steps rise in at once.
        let unfurl = !form.started && matches!(step, Step::Browser | Step::Key);
        let start = if unfurl { AFTER_MARK } else { Duration::ZERO };
        let run = self.sign_in_collapses;
        let rise = move |element: AnyElement, part: &'static str, order: u32| {
            motion::rise(
                div().w_full().flex().flex_col().items_center().child(element),
                ElementId::Name(format!("sign-in-{}-{part}-{run}", step.key()).into()),
                start + STAGGER * order,
            )
            .into_any_element()
        };
        let mark = if unfurl {
            // The welcome mark spins in, then turns slowly while the screen waits (`motion::SpinningMark`, its own
            // small view).
            self.onboarding.mark.clone().into_any_element()
        } else {
            let breathing = busy || step == Step::Waiting;
            let ink = if step == Step::Expired {
                palette.faint
            } else {
                palette.foreground
            };
            motion::breathing_mark("sign-in-mark", MARK, ink)
                .breathing(breathing)
                .into_any_element()
        };
        // The field centers on the mark: a square far larger than the window, its middle on the mark's.
        const FIELD: f32 = 2400.;
        let field_strength = match step {
            Step::Expired => 0.03,
            _ if matches!(self.auth, Auth::SignedOut { error: Some(_) }) => 0.03,
            _ => 0.07,
        };
        let mark = div()
            .relative()
            .size(px(MARK))
            .mb(px(28.))
            .child(
                div()
                    .absolute()
                    .left(px((MARK - FIELD) / 2.))
                    .top(px((MARK - FIELD) / 2.))
                    .size(px(FIELD))
                    .child(motion::field("sign-in-field", MARK, palette.foreground, field_strength)),
            )
            .child(mark);
        let body = match &self.auth {
            Auth::Waiting {
                user_code,
                link,
                deadline,
            } => self.render_waiting(form, user_code.clone(), link.clone(), *deadline, &rise, palette, cx),
            Auth::SignedIn => vec![rise(parts::heading("You're signed in", palette), "heading", 0)],
            // With no error of its own, the screen says why a saved sign-in was not used (a denied keychain dialog).
            Auth::SignedOut { error } => {
                let error = error
                    .clone()
                    .or_else(|| crate::keychain::sign_in_notice(self.gateway.url()).map(Into::into));
                self.render_rest(form, error, false, &rise, palette, cx)
            }
            Auth::Checking | Auth::Restoring | Auth::Local => self.render_rest(form, None, true, &rise, palette, cx),
        };
        div()
            .id("sign-in")
            .relative()
            .size_full()
            .overflow_hidden()
            .when(palette.dark, |stage| stage.child(parts::light_well()))
            .child(
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(v_flex().w(px(360.)).items_center().child(mark).children(body)),
            )
            .child(div().absolute().bottom(px(22.)).left_0().right_0().child(rise(
                self.render_foot(form, palette, cx),
                "foot",
                6,
            )))
            // On a first run the sign-in is onboarding's first step: its progress, at the top (`onboarding.rs`).
            .when(
                self.onboarding.first_run()
                    && !self.panel
                    && !(self.byo_entry() && matches!(self.auth, Auth::SignedOut { .. })),
                |stage| {
                    stage.child(
                        h_flex()
                            .absolute()
                            .top(px(22.))
                            .left_0()
                            .right_0()
                            .justify_center()
                            .child(self.render_onboarding_progress(0, palette)),
                    )
                },
            )
            .into_any_element()
    }

    /// At rest: the heading and its line or error, the Gateway field when open, the key or the browser button.
    fn render_rest(
        &self,
        form: &SignInForm,
        error: Option<SharedString>,
        busy: bool,
        rise: &dyn Fn(AnyElement, &'static str, u32) -> AnyElement,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let key_mode = form.key_mode;
        let mut column = vec![rise(
            parts::heading(
                if key_mode {
                    "Sign in with a key"
                } else {
                    "Sign in to Nexus"
                },
                palette,
            ),
            "heading",
            0,
        )];
        let lede = if key_mode {
            "Paste a personal API key. It stays in this computer's keychain."
        } else {
            "Sign in with your browser. Your token stays in this computer's keychain."
        };
        column.push(match error {
            Some(error) => parts::error_line(error, palette),
            None => rise(parts::lede(lede, palette), "lede", 1),
        });
        let editable_gateway = crate::channel::internal() && locked_url().is_none();
        if editable_gateway && form.gateway_open {
            column.push(rise(
                parts::field("Nexus Gateway", &form.gateway, palette),
                "gateway",
                2,
            ));
        }
        if key_mode {
            column.push(rise(parts::field("API key", &form.key, palette), "key", 2));
        }
        let byo = self.byo_entry();
        let label = match (key_mode, busy) {
            (true, true) => "Checking the key…",
            (true, false) => "Sign in",
            (false, true) => "Opening your browser…",
            (false, false) if byo => "Sign in",
            (false, false) => "Continue with your browser",
        };
        let primary = parts::button(
            "sign-in",
            Button::Primary,
            label,
            (!key_mode).then_some(Lucide::Globe),
            palette,
        )
        .when(busy, |button| button.opacity(0.6))
        .when(!busy, |button| {
            button
                .when(byo, |button| {
                    super::first_connection::focusable_connection_control(button, palette.primary_foreground)
                })
                .on_click(cx.listener(|this, _, window, cx| this.submit_sign_in(window, cx)))
        });
        let switch = parts::button(
            "sign-in-switch",
            Button::Ghost,
            if key_mode {
                "Use your browser instead"
            } else {
                "Use an API key instead"
            },
            None,
            palette,
        )
        .on_click(cx.listener(|this, _, window, cx| {
            if let Some(form) = &mut this.sign_in_form {
                form.key_mode = !form.key_mode;
                if form.key_mode {
                    form.key.update(cx, |state, cx| state.focus(window, cx));
                }
            }
            if matches!(this.auth, Auth::SignedOut { .. }) {
                this.auth = Auth::SignedOut { error: None };
            }
            cx.notify();
        }));
        column.push(rise(
            v_flex()
                .w_full()
                .mt(px(32.))
                .gap(px(10.))
                .items_center()
                .child(primary)
                .when(busy && self.local_mode.signing_in, |actions| {
                    actions.child(
                        parts::button("sign-in-cancel", Button::Ghost, "Cancel", None, palette)
                            .on_click(cx.listener(|this, _, window, cx| this.cancel_sign_in_attempt(window, cx))),
                    )
                })
                .when(!byo, |actions| actions.child(switch))
                .when(byo && !busy, |actions| {
                    actions
                        .child(
                            super::first_connection::focusable_connection_control(
                                parts::button(
                                    "continue-without-account",
                                    Button::Secondary,
                                    "Continue without an account",
                                    None,
                                    palette,
                                ),
                                palette.strong,
                            )
                            .debug_selector(|| "continue-without-account".into())
                            .on_click(cx.listener(|this, _, window, cx| this.continue_without_account(window, cx))),
                        )
                        .child(parts::lede(
                            "Use your own Claude Code or Codex sign-in, an API key, or your own model server.",
                            palette,
                        ))
                })
                .into_any_element(),
            "actions",
            3,
        ));
        column
    }

    /// Waiting for the browser: the code as tiles, its countdown and Copy, open the page again, the link.
    #[allow(clippy::too_many_arguments)]
    fn render_waiting(
        &self,
        form: &SignInForm,
        user_code: SharedString,
        link: SharedString,
        deadline: Instant,
        rise: &dyn Fn(AnyElement, &'static str, u32) -> AnyElement,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let left = deadline.saturating_duration_since(Instant::now());
        let expired = left.is_zero();
        let seconds = left.as_secs_f64().ceil() as u64;
        let mut column = vec![
            rise(
                parts::heading(
                    if expired {
                        "This code expired"
                    } else {
                        "Approve in your browser"
                    },
                    palette,
                ),
                "heading",
                0,
            ),
            rise(
                parts::lede(
                    if expired {
                        "Codes last ten minutes. Get a new one and approve it in your browser."
                    } else {
                        "We opened your browser. Check that it shows this code, then approve."
                    },
                    palette,
                ),
                "lede",
                1,
            ),
            parts::code_tiles(&user_code, expired, palette),
        ];
        if !expired {
            let copied = form.copied.is_some_and(|at| at.elapsed() < COPIED);
            let copy_code = user_code.clone();
            let copy = h_flex()
                .id("sign-in-copy")
                .h(px(26.))
                .px(px(10.))
                .gap(px(6.))
                .rounded_full()
                .border_1()
                .border_color(palette.border)
                .text_size(px(12.))
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                .child(Icon::new(if copied { Lucide::Check } else { Lucide::Copy }).size(px(12.)))
                .child(if copied { "Copied" } else { "Copy" })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_code.to_string()));
                    if let Some(form) = &mut this.sign_in_form {
                        form.copied = Some(Instant::now());
                    }
                    cx.notify();
                }));
            let share = (left.as_secs_f32() / form.life.as_secs_f32().max(1.)).clamp(0., 1.);
            column.push(rise(
                h_flex()
                    .w_full()
                    .mt(px(16.))
                    .gap(px(12.))
                    .child(parts::countdown_bar(share, palette))
                    .child(
                        div()
                            .font_family(crate::pages::ui::MONO)
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(format!("{}:{:02}", seconds / 60, seconds % 60)),
                    )
                    .child(copy)
                    .into_any_element(),
                "timer",
                4,
            ));
        }
        let cancel = parts::button("sign-in-cancel", Button::Ghost, "Cancel", None, palette)
            .on_click(cx.listener(|this, _, window, cx| this.cancel_sign_in_attempt(window, cx)));
        let main = if expired {
            parts::button(
                "sign-in-again",
                Button::Primary,
                "Get a new code",
                Some(Lucide::RotateCcw),
                palette,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.gateway.cancel_device_sign_in();
                this.auth = Auth::Checking;
                this.gateway.begin_device_sign_in();
                cx.notify();
            }))
        } else {
            let reopen = link.clone();
            parts::button(
                "sign-in-reopen",
                Button::Secondary,
                "Open the page again",
                Some(Lucide::ExternalLink),
                palette,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                crate::external_link::open(&reopen, cx);
                cx.notify();
            }))
        };
        column.push(rise(
            v_flex()
                .w_full()
                .mt(px(28.))
                .gap(px(10.))
                .items_center()
                .child(main)
                .child(cancel)
                .into_any_element(),
            "actions",
            5,
        ));
        if !expired {
            column.push(rise(parts::approval_link(link, palette), "link", 6));
        }
        column
    }

    /// The foot: the Gateway and the version. Internal builds change the Gateway here; a managed one is locked.
    fn render_foot(&self, form: &SignInForm, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let locked = locked_url();
        let url = locked.clone().unwrap_or_else(|| self.gateway.url().to_string());
        let host = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        let dot = || div().size(px(3.)).rounded_full().bg(palette.border);
        let change = (crate::channel::internal() && locked.is_none() && !form.gateway_open).then(|| {
            div()
                .id("sign-in-gateway-change")
                .text_size(px(12.5))
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| style.text_color(palette.foreground))
                .child("Change")
                .when(
                    self.byo_entry() && matches!(self.auth, Auth::SignedOut { .. }),
                    |button| {
                        super::first_connection::focusable_connection_control(button, palette.strong)
                            .aria_label("Change Nexus Gateway")
                            .px(px(2.))
                            .rounded_sm()
                    },
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    if let Some(form) = &mut this.sign_in_form {
                        form.gateway_open = true;
                        form.gateway.update(cx, |state, cx| state.focus(window, cx));
                    }
                    cx.notify();
                }))
        });
        h_flex()
            .justify_center()
            .gap(px(10.))
            .font_family(crate::pages::ui::MONO)
            .text_size(px(12.))
            .text_color(palette.faint)
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        Icon::new(if locked.is_some() { Lucide::Lock } else { Lucide::Server })
                            .size(px(12.))
                            .text_color(palette.icon),
                    )
                    .child(host),
            )
            .when(locked.is_some(), |foot| {
                foot.child(dot())
                    .child(div().text_size(px(12.5)).child("Set by your organization"))
            })
            .when_some(change, |foot, change| foot.child(dot()).child(change))
            .child(dot())
            .child(crate::channel::VERSION)
            .into_any_element()
    }
}

/// Start this app again with `arg`, on the Gateway just saved, and quit this copy.
fn restart(arg: &str, cx: &mut App) {
    if let Ok(exe) = std::env::current_exe() {
        let started = std::process::Command::new(exe)
            .arg(arg)
            .env("NEXUS_RESTARTED", "1")
            .env_remove("NEXUS_URL")
            .spawn();
        if started.is_ok() {
            cx.quit();
        }
    }
}
