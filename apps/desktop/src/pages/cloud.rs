//! Settings › Cloud, as the Nexus app's `cloud-settings.tsx`: the two sign-ins every new cloud session's machine gets
//! from this computer, as a checklist (Claude: a setup token pasted; GitHub: this computer's own `gh` sign-in, or
//! GitHub's device code on a running machine), and one line counting the person's cloud sessions, running and asleep.
//! Nexus holds the provider account, so there is no provider key and no machine to make here: a machine starts with
//! its session. Once both steps are done, a line says so with a way to start a session. When this Nexus offers no
//! cloud machines to the person, one line says why instead.
//!
//! Every step goes through the cloud machines (`crate::cloud`, `nexus_cloud::desktop`), so a token is kept for the
//! sign-in that saved it and reaches the connected machines at once. They run only while this build owns the store;
//! while the Nexus app runs, the page shows the setup as it saved it, read-only. Below, this build lists each cloud
//! session's machine and its state.

use futures::StreamExt as _;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::cloud::{self as account, CLAUDE_ACCOUNT, GITHUB_ACCOUNT};
use nexus_desktop_core::settings::Settings;

use super::desktop_app::{off_thread, read_only_note};
use super::{Ctx, palette, ui};
use crate::cloud::CloudAccount;
use crate::cloud::setup_line::{self, Blocker};
use crate::cloud::words;
use crate::machines::{CloudView, NOT_HERE};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

/// The GitHub step's Settings anchor, for a setup line that opens it (`Fix::open_at`).
pub const GITHUB_ANCHOR: &str = "cloud-github";

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| {
        let token = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("sk-ant-oat01-…"));
        let subscription = cx.subscribe_in(
            &token,
            window,
            |this: &mut CloudPage, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.save_claude(window, cx),
                InputEvent::Change => {
                    this.typed = input.read(cx).value().to_string();
                    cx.notify();
                }
                _ => {}
            },
        );
        // The cloud machines as they change, and a GitHub code on its way.
        let changes = cx.spawn(async move |this, cx| {
            let mut changes = crate::machines::get().subscribe();
            while changes.next().await.is_some() {
                while changes.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.read_machines(cx)).is_err() {
                    break;
                }
            }
        });
        let mut page = CloudPage {
            ctx,
            account: None,
            saved: Setup::default(),
            machines: crate::machines::get().cloud(),
            token,
            typed: String::new(),
            replacing: false,
            confirm: None,
            busy: None,
            error: None,
            code: None,
            signing_on: None,
            _subscription: subscription,
            _changes: changes,
        };
        page.load(cx);
        page
    });
    Some(view.into())
}

/// The sign-ins as the Nexus app last saved them, for the read-only page while it runs the cloud machines.
#[derive(Debug, Clone, Default, PartialEq)]
struct Setup {
    claude: Option<String>,
    claude_saved_at: Option<i64>,
    github: Option<String>,
    github_saved_at: Option<i64>,
}

impl Setup {
    /// The hints saved, each only while its secret is in the keychain (`account_now`).
    fn read(settings: &Settings, claude_saved: bool, github_saved: bool) -> Setup {
        let saved = settings.cloud.clone().unwrap_or_default();
        Setup {
            claude: saved.claude_hint.filter(|_| claude_saved),
            claude_saved_at: saved.claude_saved_at.filter(|_| claude_saved),
            github: saved.github_user.filter(|_| github_saved),
            github_saved_at: saved.github_saved_at.filter(|_| github_saved),
        }
    }

    /// Off the UI thread: the keychain may be slow.
    fn load() -> Setup {
        let vault = crate::keychain::vault();
        let (claude, github) = (vault.get(CLAUDE_ACCOUNT).is_some(), vault.get(GITHUB_ACCOUNT).is_some());
        Setup::read(&crate::desktop_settings::load(), claude, github)
    }
}

/// Which sign-in a Remove is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Secret {
    Claude,
    Github,
}

struct CloudPage {
    ctx: Ctx,
    /// The cloud setup as the cloud machines see it (`cloud_account`), while they run here.
    account: Option<Result<CloudAccount, SharedString>>,
    /// The setup as the Nexus app saved it, while it runs the cloud machines.
    saved: Setup,
    machines: Vec<CloudView>,
    /// The Claude setup token as pasted; never read back.
    token: Entity<InputState>,
    typed: String,
    /// The Claude step's form is open over a saved token.
    replacing: bool,
    /// The sign-in whose Remove was pressed once.
    confirm: Option<Secret>,
    /// The action on its way: `save`, `remove`, `local` or `code`.
    busy: Option<&'static str>,
    error: Option<SharedString>,
    /// GitHub's device code and its page, while a sign-in on a machine waits for it.
    code: Option<(String, String)>,
    /// The machine the GitHub sign-in runs on.
    signing_on: Option<String>,
    _subscription: Subscription,
    _changes: Task<()>,
}

/// Runs `work` on the cloud machines' runtime, and hands its answer to `then` on the UI thread.
fn on_runtime<T: Send + 'static>(
    cx: &mut Context<CloudPage>,
    work: impl Future<Output = T> + Send + 'static,
    then: impl FnOnce(&mut CloudPage, T, &mut Context<CloudPage>) + 'static,
) {
    let (answer, answered) = futures::channel::oneshot::channel();
    crate::code_run::runtime().spawn(async move {
        let _ = answer.send(work.await);
    });
    cx.spawn(async move |this, cx| {
        if let Ok(value) = answered.await {
            let _ = this.update(cx, |this, cx| {
                then(this, value, cx);
                cx.notify();
            });
        }
    })
    .detach();
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// "Oct 21", or "Oct 21, 2025" in another year (`shortDate`).
fn short_date(ms: i64, now: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (year, month, day) = civil(ms);
    let (this_year, _, _) = civil(now);
    let name = MONTHS[(month - 1).clamp(0, 11) as usize];
    if year == this_year {
        format!("{name} {day}")
    } else {
        format!("{name} {day}, {year}")
    }
}

/// The civil date of Unix milliseconds (Howard Hinnant's algorithm).
fn civil(ms: i64) -> (i64, i64, i64) {
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

impl CloudPage {
    fn load(&mut self, cx: &mut Context<Self>) {
        match crate::cloud::get() {
            Some(cloud) => on_runtime(cx, async move { cloud.account().await }, |this, account, _| {
                this.account = Some(Ok(account));
            }),
            None => off_thread(cx, Setup::load, |this, setup, _| this.saved = setup),
        }
    }

    /// Try again on a setup that failed to read: the machines are listed again first, so the answer is fresh.
    fn read_again(&mut self, cx: &mut Context<Self>) {
        let Some(cloud) = crate::cloud::get() else {
            return;
        };
        self.account = None;
        on_runtime(
            cx,
            async move {
                let synced = cloud.sync().await;
                (synced, cloud.account().await)
            },
            |this, (synced, account), _| this.account = Some(setup_line::read_again(synced, account)),
        );
        cx.notify();
    }

    /// Why cloud machines can't run for the person, at the top of the page, with its fix.
    fn render_blocked(&self, blocker: &Blocker, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let retry = crate::setup::Fix::act(move |_, cx| {
            if let Err(error) = this.update(cx, |this, cx| this.read_again(cx)) {
                eprintln!("nexus-desktop-gpui: cloud settings: read again: {error}");
            }
        });
        let line = setup_line::line(blocker, &self.ctx.org_name, retry);
        div().pb(px(12.)).children(crate::setup::slot(
            "cloud-settings-setup",
            Some(line),
            &self.ctx.fixer,
            palette,
            cx,
        ))
    }

    fn read_machines(&mut self, cx: &mut Context<Self>) {
        let machines = crate::machines::get();
        self.machines = machines.cloud();
        if let Some(machine) = self.signing_on.clone()
            && let Some(login) = machines.github_login(&machine)
        {
            match login["state"].as_str() {
                Some("code") => {
                    let code = login["code"].as_str().unwrap_or_default().to_string();
                    let link = login["link"]
                        .as_str()
                        .unwrap_or("https://github.com/login/device")
                        .to_string();
                    if self.code.as_ref().is_none_or(|(shown, _)| *shown != code) {
                        crate::external_link::open(&link, cx);
                    }
                    self.code = Some((code, link));
                }
                Some("done") => {
                    self.code = None;
                    self.signing_on = None;
                    self.busy = None;
                    self.load(cx);
                }
                _ => {
                    self.code = None;
                    self.signing_on = None;
                    self.busy = None;
                    self.error = login["error"].as_str().map(|error| error.to_string().into());
                }
            }
        }
        cx.notify();
    }

    /// One of the cloud machines' account changes, on their runtime; the answer is the account as it stands.
    fn act(
        &mut self,
        what: &'static str,
        work: impl FnOnce(crate::cloud::Cloud) -> futures::future::BoxFuture<'static, Result<CloudAccount, String>>
        + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        let Some(cloud) = crate::cloud::get() else {
            self.error = Some(NOT_HERE.into());
            cx.notify();
            return;
        };
        self.busy = Some(what);
        self.error = None;
        on_runtime(cx, work(cloud), |this, done, _| {
            this.busy = None;
            this.confirm = None;
            match done {
                Ok(account) => {
                    this.account = Some(Ok(account));
                    this.replacing = false;
                }
                Err(error) => this.error = Some(error.into()),
            }
        });
        cx.notify();
    }

    fn save_claude(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let token = self.typed.trim().to_string();
        if !account::looks_like_setup_token(&token) {
            return;
        }
        self.token.update(cx, |state, cx| state.set_value("", window, cx));
        self.typed.clear();
        self.act(
            "save",
            move |cloud| Box::pin(async move { cloud.claude_set(&token) }),
            cx,
        );
    }

    fn remove(&mut self, secret: Secret, cx: &mut Context<Self>) {
        if self.confirm != Some(secret) {
            self.confirm = Some(secret);
            cx.notify();
            return;
        }
        self.act(
            "remove",
            move |cloud| {
                Box::pin(async move {
                    match secret {
                        Secret::Claude => cloud.claude_clear(),
                        Secret::Github => cloud.github_clear(),
                    }
                })
            },
            cx,
        );
    }

    fn use_local_github(&mut self, cx: &mut Context<Self>) {
        self.act(
            "local",
            |cloud| Box::pin(async move { cloud.github_use_local().await }),
            cx,
        );
    }

    /// GitHub's device code on a running machine: the code arrives on `cloud:github`, and the token it gets is kept here.
    fn sign_in_with_code(&mut self, machine: String, cx: &mut Context<Self>) {
        let Some(cloud) = crate::cloud::get() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some("code");
        self.error = None;
        self.signing_on = Some(machine.clone());
        on_runtime(
            cx,
            async move { cloud.github_login(&machine).await },
            |this, started, _| {
                if let Err(error) = started {
                    this.busy = None;
                    this.signing_on = None;
                    this.error = Some(error.into());
                }
            },
        );
        cx.notify();
    }

    fn button(&self, id: &'static str, label: &'static str, primary: bool, palette: Palette) -> Stateful<Div> {
        if self.busy.is_some() {
            ui::disabled_button(id, label, palette)
        } else if primary {
            ui::primary_button(id, label, palette)
        } else {
            ui::button(id, label, palette)
        }
    }

    fn remove_button(&self, secret: Secret, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = match secret {
            Secret::Claude => "cloud-claude-remove",
            Secret::Github => "cloud-github-remove",
        };
        if self.busy.is_some() {
            return ui::disabled_button(id, "Remove", palette).into_any_element();
        }
        let label = if self.confirm == Some(secret) {
            "Remove?"
        } else {
            "Remove"
        };
        ui::danger_button(id, label, palette)
            .on_click(cx.listener(move |this, _, _, cx| this.remove(secret, cx)))
            .into_any_element()
    }

    /// One step: its number (a check once done), its name and where it stands, its actions, and what it needs below.
    fn step(n: usize, done: bool, title: &'static str, summary: String, action: AnyElement, palette: Palette) -> Div {
        let mark = if done { "✓".to_string() } else { n.to_string() };
        h_flex()
            .gap(px(12.))
            .items_start()
            .pt(px(12.))
            .child(
                div()
                    .size(px(20.))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .bg(if done { palette.primary } else { palette.hover })
                    .text_color(if done {
                        palette.primary_foreground
                    } else {
                        palette.muted
                    })
                    .child(mark),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        v_flex()
                            .min_w(px(0.))
                            .gap(px(2.))
                            .child(div().text_size(px(13.5)).text_color(palette.strong).child(title))
                            .child(div().text_size(px(12.5)).text_color(palette.faint).child(summary)),
                    )
                    .child(h_flex().flex_none().gap(px(6.)).child(action)),
            )
    }

    /// A command to run in a terminal, with Copy, as the Nexus app's `Command`.
    fn command(text: &'static str, palette: Palette) -> Div {
        h_flex()
            .gap(px(8.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .bg(palette.hover)
                    .font_family(ui::MONO)
                    .text_size(px(12.5))
                    .text_color(palette.foreground)
                    .child(text),
            )
            .child(
                ui::button(SharedString::from(format!("cloud-copy-{text}")), "Copy", palette)
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.into()))),
            )
    }

    fn render_claude(&self, account: &CloudAccount, palette: Palette, cx: &mut Context<Self>) -> Div {
        let claude = &account.claude;
        let now = now_ms();
        let valid = account::looks_like_setup_token(&self.typed);
        let save = if valid && self.busy.is_none() {
            ui::primary_button(
                "cloud-claude-save",
                if self.busy == Some("save") { "Saving…" } else { "Save" },
                palette,
            )
            .on_click(cx.listener(|this, _, window, cx| this.save_claude(window, cx)))
            .into_any_element()
        } else {
            ui::disabled_button("cloud-claude-save", "Save", palette).into_any_element()
        };
        let form = v_flex()
            .gap(px(8.))
            .pl(px(32.))
            .pt(px(8.))
            .child(ui::note("Run this in a terminal, then paste what it prints:", palette))
            .child(Self::command("claude setup-token", palette))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(ui::input_box(&self.token, palette).font_family(ui::MONO))
                    .child(save),
            )
            .when(!self.typed.trim().is_empty() && !valid, |form| {
                form.child(ui::note(
                    "That isn't a setup token. It starts with sk-ant-oat.",
                    palette,
                ))
            })
            .child(ui::note(
                "Saved once for all your devices. Only your own cloud machines get it, and no one can read it back.",
                palette,
            ));
        if !claude.connected {
            return v_flex()
                .child(Self::step(
                    1,
                    false,
                    "Claude",
                    "Claude sessions on Cloud run on your own Claude plan.".into(),
                    div().into_any_element(),
                    palette,
                ))
                .child(form);
        }
        let mut summary = format!("Your Claude plan, token {}", claude.hint.clone().unwrap_or_default());
        if let Some(at) = claude.saved_at {
            summary.push_str(&format!(" · saved {}", short_date(at, now)));
        }
        let warning = words::token_warning(claude.saved_at, now);
        let actions = h_flex()
            .gap(px(6.))
            .child(
                self.button(
                    "cloud-claude-replace",
                    if self.replacing { "Cancel" } else { "Replace" },
                    false,
                    palette,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.replacing = !this.replacing;
                    cx.notify();
                })),
            )
            .child(self.remove_button(Secret::Claude, palette, cx));
        v_flex()
            .child(Self::step(
                1,
                true,
                "Claude",
                summary,
                actions.into_any_element(),
                palette,
            ))
            .children(warning.map(|warning| div().pl(px(32.)).child(ui::error_line(warning, palette))))
            .when(self.replacing, |step| step.child(form))
    }

    fn render_github(&self, account: &CloudAccount, palette: Palette, cx: &mut Context<Self>) -> Div {
        let github = &account.github;
        if github.connected {
            let mut summary = format!("Signed in as @{}", github.user.clone().unwrap_or_else(|| "you".into()));
            if let Some(at) = github.saved_at {
                summary.push_str(&format!(" · saved {}", short_date(at, now_ms())));
            }
            summary.push_str(". Pushes and pull requests are yours.");
            let local = if self.busy == Some("local") {
                "Checking…"
            } else {
                "Update from this computer"
            };
            let actions = h_flex()
                .gap(px(6.))
                .child(
                    self.button("cloud-github-local", local, false, palette)
                        .on_click(cx.listener(|this, _, _, cx| this.use_local_github(cx))),
                )
                .child(self.remove_button(Secret::Github, palette, cx));
            return v_flex().child(Self::step(
                2,
                true,
                "GitHub",
                summary,
                actions.into_any_element(),
                palette,
            ));
        }
        let head = Self::step(
            2,
            false,
            "GitHub",
            "So the agent can clone your repositories, push branches and open pull requests as you.".into(),
            div().into_any_element(),
            palette,
        );
        if let Some((code, link)) = self.code.clone() {
            let copy = code.clone();
            return v_flex().child(head).child(
                v_flex()
                    .gap(px(8.))
                    .pl(px(32.))
                    .pt(px(8.))
                    .child(ui::note("Enter this code on the GitHub page that opened:", palette))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_family(ui::MONO)
                                    .text_size(px(16.))
                                    .text_color(palette.strong)
                                    .child(code),
                            )
                            .child(
                                ui::button("cloud-github-copy", "Copy the code", palette).on_click(cx.listener(
                                    move |_, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                    },
                                )),
                            ),
                    )
                    .child(
                        h_flex().child(
                            ui::button("cloud-github-open", "Open GitHub again", palette)
                                .on_click(cx.listener(move |_, _, _, cx| crate::external_link::open(&link, cx))),
                        ),
                    ),
            );
        }
        // GitHub's device code runs on a machine: any cloud machine running now will do.
        let running = self
            .machines
            .iter()
            .find(|machine| matches!(machine.status, nexus_remote::machine::Status::Connected { .. }))
            .map(|machine| machine.id.clone());
        let local = if self.busy == Some("local") {
            "Checking…"
        } else {
            "Use this computer's GitHub"
        };
        let mut buttons = h_flex().gap(px(8.)).child(
            self.button("cloud-github-local", local, true, palette)
                .on_click(cx.listener(|this, _, _, cx| this.use_local_github(cx))),
        );
        if let Some(machine) = running {
            let label = if self.busy == Some("code") {
                "Waiting for GitHub…"
            } else {
                "Sign in with a code"
            };
            buttons = buttons.child(
                self.button("cloud-github-code", label, false, palette)
                    .on_click(cx.listener(move |this, _, _, cx| this.sign_in_with_code(machine.clone(), cx))),
            );
        }
        v_flex().child(head).child(
            v_flex()
                .gap(px(8.))
                .pl(px(32.))
                .pt(px(8.))
                .child(ui::note(
                    "This uses the GitHub sign-in of gh on this computer. Not signed in there? Run this first:",
                    palette,
                ))
                .child(Self::command("gh auth login", palette))
                .child(buttons),
        )
    }

    /// The setup as the Nexus app saved it, while it runs the cloud machines: no actions.
    fn render_saved(&self, palette: Palette) -> Div {
        let now = now_ms();
        let line = |name: &'static str, value: &Option<String>, at: Option<i64>, missing: &'static str| {
            let detail = match (value, at) {
                (Some(value), Some(at)) => format!("{value} · saved {}", short_date(at, now)),
                (Some(value), None) => value.clone(),
                (None, _) => missing.to_string(),
            };
            ui::setting(
                name,
                detail,
                ui::badge(if value.is_some() { "Done" } else { "Not set up" }, palette),
                palette,
            )
        };
        v_flex()
            .child(line(
                "Claude",
                &self.saved.claude,
                self.saved.claude_saved_at,
                "Claude sessions on Cloud run on your own Claude plan.",
            ))
            .child(line(
                "GitHub",
                &self.saved.github,
                self.saved.github_saved_at,
                "So the agent can clone your repositories, push branches and open pull requests as you.",
            ))
    }

    fn render_machine(&self, machine: &CloudView, palette: Palette) -> Div {
        let view = words::cloud_view(&machine.info, &machine.status, now_ms());
        let title = machine.info.title.clone().unwrap_or_else(|| "Cloud session".into());
        ui::row(palette)
            .child(v_flex().flex_1().min_w(px(0.)).child(ui::two_lines(
                title,
                view.detail.unwrap_or_else(|| machine.name.clone()),
                palette,
            )))
            .child(ui::badge(view.label, palette))
    }
}

fn start_button(palette: Palette, enabled: bool) -> Stateful<Div> {
    if enabled {
        ui::primary_button("cloud-start", "Start a session", palette)
    } else {
        ui::disabled_button("cloud-start", "Start a session", palette)
    }
}

pub(super) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        v_flex().child(start_button(palette, false)).into_any_element()
    })
}

impl Render for CloudPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let mut page = v_flex().w_full().children(read_only_note(palette));
        if crate::cloud::get().is_none() {
            page = page
                .child(ui::section_head("Set up", palette))
                .child(self.render_saved(palette));
        } else {
            let blocker = self.account.as_ref().and_then(Blocker::of);
            match (self.account.clone(), blocker) {
                (None, _) => page = page.child(ui::note("Reading your cloud setup…", palette)),
                (Some(_), Some(blocker)) => page = page.child(self.render_blocked(&blocker, palette, cx)),
                (Some(Err(_)), None) => {}
                (Some(Ok(account)), None) => {
                    let done = usize::from(account.claude.connected) + usize::from(account.github.connected);
                    if done == 2 {
                        let start = self.ctx.new_cloud_session.clone();
                        page = page.child(
                            h_flex()
                                .gap(px(12.))
                                .py(px(8.))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(13.5))
                                        .text_color(palette.strong)
                                        .child("Ready. Pick Cloud when you start a session."),
                                )
                                .child(start_button(palette, true).on_click(move |_, window, cx| start(window, cx))),
                        );
                    }
                    page = page
                        .child(
                            h_flex()
                                .justify_between()
                                .child(ui::section_head("Set up", palette))
                                .when(done < 2, |head| {
                                    head.child(ui::note(format!("{done} of 2 done"), palette))
                                }),
                        )
                        .child(self.render_claude(&account, palette, cx))
                        // The project menu's "GitHub isn't connected" opens here (`GITHUB_ANCHOR`).
                        .child({
                            let step = self.render_github(&account, palette, cx);
                            self.ctx.settings_fields.field(GITHUB_ANCHOR, step, None, cx)
                        })
                        .child(div().pt(px(12.)).child(ui::note(
                            "Codex needs nothing here: on Cloud it runs on Team API. Your ChatGPT sign-in stays on \
                             this computer.",
                            palette,
                        )));
                }
            }
        }
        page = page
            .children(
                self.error
                    .clone()
                    .map(|error| div().pt(px(8.)).child(ui::error_line(error, palette))),
            )
            .child(div().pt(px(16.)).child(ui::note(
                format!(
                    "{}. Each cloud session has its own machine. It sleeps when the agent is done and you are away, \
                     and archiving the session deletes it.",
                    words::counts_line(words::session_counts(&self.machines))
                ),
                palette,
            )));
        let shown: Vec<&CloudView> = self
            .machines
            .iter()
            .filter(|machine| machine.info.state != crate::cloud::CloudState::Gone)
            .collect();
        if !shown.is_empty() {
            page = page.child(ui::section_head("Your cloud sessions", palette));
            for machine in shown {
                page = page.child(self.render_machine(machine, palette));
            }
        }
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn reads_what_the_nexus_app_set_up() {
        let mut settings = Settings::default();
        account::change_account(&mut settings, |saved| {
            account::claude_saved(saved, "sk-ant-oat01-abcdefghijklmnopqrstuvwxyz0123", 1_762_300_800_000);
            account::github_saved(saved, "octo".into(), 1_762_300_800_000);
        });
        let setup = Setup::read(&settings, true, true);
        assert_eq!(setup.claude.as_deref(), Some("sk-ant-oat…0123"));
        assert_eq!(setup.github.as_deref(), Some("octo"));
        // A hint whose secret left the keychain is not shown.
        assert_eq!(Setup::read(&settings, false, true).claude, None);
    }

    #[test]
    fn dates_read_as_the_nexus_app_writes_them() {
        let now = 1_762_300_800_000; // 2025-11-05
        assert_eq!(short_date(now, now), "Nov 5");
        assert_eq!(short_date(1_729_468_800_000, now), "Oct 21, 2024");
    }
}
