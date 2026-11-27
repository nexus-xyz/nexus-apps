//! The Code session's Browser tab: up to eight tabs, each its own page with its own history, over an address bar with
//! back, forward and reload. Each page is a [`WebView`] the engine opens as a session browser's tab: it goes only to
//! web addresses (`nexus_browser`'s rule), and a sign-in popup opens in a window of its own ([`popup::Popup`]).
//! Hidden tabs keep their pages, so switching back finds them where they were. The page draws at the pane's width, or at
//! a phone's ([`width::Width`]).
//!
//! A cloud session's browser shows the machine's ports that the Cloud panel's Preview forwards here, in the machine's
//! own unsaved profile, without its agent, server checks or sign-in import, as in the Nexus app
//! (`nexus_browser::place`). A server session's browser is the same, but it checks and opens the server's dev servers
//! through an SSH forward ([`server_preview`]).
//!
//! The agent's browser tools reach a tab by its id: [`Browser::tabs`] and [`Browser::page`].

pub mod address;
pub mod agent;
pub mod auto_preview;
pub mod cookies;
pub mod popup;
pub mod preview;
mod retention;
pub mod server_preview;
#[cfg(test)]
mod server_preview_tests;
pub mod tabs;
pub(crate) use retention::RetainedBrowser;
pub mod width;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::workspace::{GoBack, GoForward};

use self::address::Entered;
use self::cookies::{CookieForm, FormEvent};
use self::tabs::{Asked, Tab, Tabs};
use self::width::Width;
use crate::palette::Palette;
use crate::web::{WebEvent, WebView};
use nexus_browser::place::Place;

/// The local addresses opened in this run (newest first), for the empty page to offer again. Not kept across runs,
/// and kept without query or fragment (an OAuth callback's `?code=` never lands here).
static RECENT: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
const RECENT_MAX: usize = 4;
/// The address bar's placeholder, as the Nexus app's.
const PLACEHOLDER: &str = "Address or search";

/// How long an import waits for the engine to set its cookies, as the Nexus app's.
const IMPORT_WAIT: Duration = Duration::from_secs(10);
/// The DevTools ids of an import's messages start here, apart from any other's. DevTools ids are 32-bit integers: a
/// larger one is dropped unanswered.
const IMPORT_IDS: u64 = 1 << 30;

/// Cookies being written into the engine's store, through tab `tab`'s page.
struct Import {
    tab: String,
    read: cookies::Read,
    pending: HashSet<u64>,
    set: usize,
    _wait: Task<()>,
}

/// What the browser asks of the screen around it.
pub enum BrowserEvent {
    /// "Screenshot into the message": the page on screen as an image for the composer.
    Screenshot(crate::attachments::Incoming),
    /// Text for the message: the console's lines ("Add to message"), or an element the person picked.
    Text(String),
    /// "Stop": the agent's control ended, and its turn stops too.
    StopAgent,
    /// The person chose a page, a tab or a server here: the turn's automatic Browser opens nothing more
    /// (`auto_preview.rs`, the Nexus app's `BROWSER_NAVIGATED`).
    Navigated,
    /// A completed action on this exact agent-controlled tab. No visible selection is implied.
    Activity(String),
    /// A permission request changed; the conversation can show it without selecting Browser.
    Permission,
}

impl EventEmitter<BrowserEvent> for Browser {}

/// A screenshot the page was asked for: its tab and DevTools id.
struct Capture {
    tab: String,
    id: u64,
    _wait: Task<()>,
}

/// The DevTools ids of screenshots start here, apart from an import's (32-bit, as [`IMPORT_IDS`]).
const CAPTURE_IDS: u64 = 1 << 29;

pub struct Browser {
    retired: bool,
    /// The session this browser belongs to: its pages' popups count against it.
    session: String,
    /// Opaque internal permission owner; never a displayed session or notification destination.
    permission_owner: agent::PermissionOwner,
    permission_instance: String,
    permission_guard: Option<agent::Guard>,
    agent_alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Where it shows pages: this computer, a cloud machine's ports forwarded here, or a server's dev servers. `None`
    /// for a session no machine here holds.
    place: Option<Place>,
    /// A server session's forwards to its dev servers, and the one on its way for the tab on screen.
    server: Option<server_preview::ServerPreview>,
    forwarding: Option<Task<()>>,
    /// Its website profile: the sign-in's, or a cloud machine's own. Every tab's page, and their popups, keep their
    /// cookies there.
    profile: nexus_browser::profile::Profile,
    /// The local addresses the session printed, newest first: its dev servers. Their checks, and whether the person
    /// chose an address, after which a server that starts answering no longer takes the tab.
    found: Vec<String>,
    probes: Vec<nexus_browser::preview::Probe>,
    typed: bool,
    navigation_managed: bool,
    /// A server the checks chose to open, on the next draw (which has the window).
    pending_open: Option<String>,
    capture: Option<Capture>,
    next_capture: u64,
    /// When the pane last drew: the checks run only while it shows.
    drawn: Instant,
    _checks: Task<()>,
    /// Whether an agent may drive this browser (`allow_agent`), and who, for the control bar ("Claude Code is using
    /// the browser").
    agent_control: bool,
    agent_paused: bool,
    /// The tab the agent last acted on while it was on screen, and when, for the aura's linger (`aura_lit`), and the
    /// draw that ends it. Cleared when the person takes control back (`end_aura`).
    agent_seen: Option<(String, Instant)>,
    _aura_wake: Option<Task<()>>,
    pending_activity_runs: std::collections::BTreeMap<String, agent::Guard>,
    activity_source: Option<agent::Guard>,
    agent_tab: Option<String>,
    agent_name: String,
    title: String,
    /// The page's console on screen, while its list is open, and the page script's picker.
    logs: Vec<(String, String)>,
    console_open: bool,
    _console: Task<()>,
    /// The page's width: the pane's, or a phone's.
    width: Width,
    address: Entity<InputState>,
    tabs: Tabs,
    /// Each tab's page, by tab id, once it has an address.
    pages: HashMap<String, (Entity<WebView>, Subscription)>,
    /// Popup windows, with the tab that opened them.
    popups: Vec<(String, AnyWindowHandle)>,
    problem: Option<SharedString>,
    /// "Import sign-ins": its form while open, and the cookies being written.
    cookie_form: Option<(Entity<CookieForm>, Subscription)>,
    import: Option<Import>,
    next_import: u64,
    _subscriptions: Vec<Subscription>,
}

impl Browser {
    /// `session` names the browser (a Code session's id); `url` opens first, like the dev server the agent started.
    pub fn new(
        session: String,
        place: Option<Place>,
        profile: nexus_browser::profile::Profile,
        url: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Browser {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder(PLACEHOLDER));
        let subscriptions = vec![cx.subscribe_in(
            &address,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.enter(window, cx),
                InputEvent::Change => {
                    if this.problem.take().is_some() {
                        cx.notify();
                    }
                }
                _ => {}
            },
        )];
        // The page's console and a picked element, read from the page script while the pane shows, as the Nexus app
        // reads them every 750 ms.
        let console = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(750)).await;
                let Ok((tab, shown)) = this.read_with(cx, |browser: &Browser, _| {
                    (
                        browser
                            .tabs
                            .current()
                            .filter(|tab| !tab.url.is_empty())
                            .map(|tab| tab.id.clone()),
                        browser.drawn.elapsed() < Duration::from_secs(2),
                    )
                }) else {
                    break;
                };
                let Some(tab) = tab.filter(|_| shown) else { continue };
                if let Ok(info) = agent::evaluate(&this, cx, &tab, "window.__nxPreview?.info() ?? {}").await {
                    let _ = this.update(cx, |browser, cx| browser.took_info(&tab, &info, cx));
                }
            }
        });
        let checks = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(preview::PROBE_EVERY).await;
                if this.update(cx, |this, cx| this.check(cx)).is_err() {
                    break;
                }
            }
        });
        let mut browser = Browser {
            retired: false,
            permission_owner: agent::PermissionOwner::Isolated(uuid::Uuid::new_v4().to_string()),
            permission_instance: uuid::Uuid::new_v4().to_string(),
            permission_guard: None,
            agent_alive: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            session,
            server: place
                .as_ref()
                .and_then(Place::server)
                .map(server_preview::ServerPreview::new),
            forwarding: None,
            place,
            profile,
            found: Vec::new(),
            probes: Vec::new(),
            typed: false,
            navigation_managed: false,
            pending_open: None,
            capture: None,
            next_capture: CAPTURE_IDS,
            drawn: Instant::now(),
            _checks: checks,
            agent_control: false,
            agent_paused: false,
            agent_seen: None,
            _aura_wake: None,
            pending_activity_runs: std::collections::BTreeMap::new(),
            activity_source: None,
            agent_tab: None,
            agent_name: "The agent".into(),
            title: "Your session".into(),
            logs: Vec::new(),
            console_open: false,
            _console: console,
            width: Width::default(),
            address,
            tabs: Tabs::default(),
            pages: HashMap::new(),
            popups: Vec::new(),
            problem: None,
            cookie_form: None,
            import: None,
            next_import: IMPORT_IDS,
            _subscriptions: subscriptions,
        };
        if let Some(url) = url.as_deref().and_then(address::preview_url) {
            browser.open(&url, window, cx);
            // The dev server the agent started, not the person's choice: a server that answers may take its place.
            browser.typed = false;
        }
        browser
    }

    /// A Code session's browser on this computer or on a server: its agent may drive it, and it starts trusting the
    /// agent with every tab until the person takes over, as in the Nexus app. Another browser (home, a project, a cloud
    /// session) has no agent.
    pub fn allow_agent(&mut self) {
        self.agent_control = true;
        self.tabs.control_all(true);
    }

    /// Who drives the browser, by name, for the control bar, and the session's title, for its notification.
    pub fn set_agent(&mut self, name: &str, title: &str) {
        self.agent_name = name.to_string();
        self.title = title.to_string();
    }

    pub(crate) fn agent_paused(&self) -> bool {
        self.agent_paused
    }

    /// Suspend actions without changing any tab's grant. In-flight results lose their control epoch.
    pub(crate) fn pause_agent(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.agent_paused = paused;
        if paused {
            self.end_aura();
            for tab in &mut self.tabs.tabs {
                tab.control(tab.agent_allowed);
                tab.activity = None;
            }
        }
        cx.notify();
    }

    pub(crate) fn take_over_agent(&mut self, cx: &mut Context<Self>) {
        self.agent_paused = false;
        self.tabs.control_all(false);
        self.end_aura();
        cx.notify();
    }

    /// The aura goes the moment control leaves the agent, with no linger.
    fn end_aura(&mut self) {
        self.agent_seen = None;
        self._aura_wake = None;
    }

    pub(crate) fn activity_status(&self) -> Option<crate::activity::Status> {
        if self.retired
            || !self.agent_control
            || (!self.agent_paused && !self.tabs.trust_all && !self.tabs.tabs.iter().any(|tab| tab.agent_allowed))
        {
            return None;
        }
        let tab = self
            .tabs
            .tabs
            .iter()
            .find(|tab| tab.activity.is_some())
            .or_else(|| {
                self.tabs
                    .agent_target(self.agent_tab.as_deref())
                    .filter(|tab| tab.agent_allowed)
            })
            .or_else(|| self.tabs.current().filter(|tab| tab.agent_allowed))
            .or_else(|| self.tabs.tabs.iter().find(|tab| tab.agent_allowed));
        let site = tab
            .and_then(|tab| url::Url::parse(&tab.url).ok())
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_else(|| "Nexus browser".into());
        Some(crate::activity::Status {
            agent: self.agent_name.clone(),
            title: self.title.clone(),
            action: if self.agent_paused {
                "Browser access paused"
            } else {
                crate::activity::action_label(tab.and_then(|tab| tab.activity.as_deref()))
            }
            .into(),
            site,
            paused: self.agent_paused,
        })
    }

    /// What the page script said about the tab on screen: its console, and an element the person picked.
    fn took_info(&mut self, tab: &str, info: &serde_json::Value, cx: &mut Context<Self>) {
        if self.tabs.current().is_none_or(|current| current.id != tab) {
            return;
        }
        let logs: Vec<(String, String)> = info["logs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|entry| {
                (
                    entry["level"].as_str().unwrap_or("log").to_string(),
                    entry["text"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        if logs != self.logs {
            self.logs = logs;
            cx.notify();
        }
        if let Some(picked) = info["picked"].as_object() {
            let url = self.tabs.current().map(|tab| tab.url.clone()).unwrap_or_default();
            cx.emit(BrowserEvent::Text(format!(
                "Element from {url}:\n{}\n{} × {}\n{}",
                picked
                    .get("selector")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default(),
                picked.get("width").and_then(|value| value.as_i64()).unwrap_or_default(),
                picked
                    .get("height")
                    .and_then(|value| value.as_i64())
                    .unwrap_or_default(),
                picked.get("text").and_then(|value| value.as_str()).unwrap_or_default(),
            )));
        }
    }

    /// "Pick an element for the message": the page script waits for a click on the page.
    fn pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.current().map(|tab| tab.id.clone()) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            if let Err(problem) = agent::evaluate(&this, cx, &tab, "window.__nxPreview.pick()").await {
                let _ = this.update(cx, |browser, cx| {
                    browser.problem = Some(problem.into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// The agent's request and the console, as the Nexus app's (`native-browser.tsx`). Who drives the page shows on
    /// the page itself (`render_aura`) and in the toolbar (`render_agent_toggle`), not in a bar of its own.
    fn render_agent(&self, palette: Palette, cx: &mut Context<Self>) -> (Option<Div>, Div) {
        let action = |id: &'static str, label: &'static str| action_button(id, label, palette);
        let request = self.permission_request().map(|request| {
            let host = url::Url::parse(&request.url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_string))
                .map(|host| format!(" on {host}"))
                .unwrap_or_default();
            h_flex()
                .flex_none()
                .px(px(10.))
                .py(px(6.))
                .gap(px(6.))
                .border_b_1()
                .border_color(palette.border)
                .text_size(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_color(palette.foreground)
                        .child(format!("{} wants to use the browser{host}", self.agent_name)),
                )
                .child(
                    action("browser-request-allow", "Allow")
                        .on_click(cx.listener(|this, _, _, cx| this.answer_request(agent::Answer::Allow, cx))),
                )
                .child(
                    action("browser-request-always", "Always in this session")
                        .tooltip(|window, cx| {
                            Tooltip::new("Every tab of this session's browser, until you take over").build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.answer_request(agent::Answer::Always, cx))),
                )
                .child(
                    action("browser-request-decline", "Don't allow")
                        .on_click(cx.listener(|this, _, _, cx| this.answer_request(agent::Answer::Decline, cx))),
                )
        });
        let errors = self.logs.iter().filter(|(level, _)| level == "error").count();
        let warnings = self.logs.iter().filter(|(level, _)| level == "warn").count();
        let open = self.console_open;
        let console = v_flex()
            .flex_none()
            .border_t_1()
            .border_color(palette.border)
            .text_size(px(12.))
            .child(
                h_flex()
                    .id("browser-console")
                    .h(px(28.))
                    .px(px(10.))
                    .gap(px(6.))
                    .cursor_pointer()
                    .text_color(palette.muted)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.console_open = !this.console_open;
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if open {
                            Lucide::ChevronDown
                        } else {
                            Lucide::ChevronRight
                        })
                        .size(px(12.)),
                    )
                    .child(format!("Console · {errors} errors · {warnings} warnings")),
            )
            .when(open, |console| {
                let has_logs = !self.logs.is_empty();
                console
                    .child(
                        h_flex().px(px(10.)).pb(px(4.)).child(
                            action("browser-console-add", "Add to message")
                                .when(!has_logs, |button| button.opacity(0.5))
                                .when(has_logs, |button| {
                                    button.on_click(cx.listener(|this, _, _, cx| {
                                        let url = this.tabs.current().map(|tab| tab.url.clone()).unwrap_or_default();
                                        let lines: Vec<String> = this
                                            .logs
                                            .iter()
                                            .map(|(level, text)| format!("[{level}] {text}"))
                                            .collect();
                                        cx.emit(BrowserEvent::Text(format!(
                                            "Browser console ({url}):\n{}",
                                            lines.join("\n")
                                        )));
                                    }))
                                }),
                        ),
                    )
                    .child(
                        v_flex()
                            .id("browser-console-lines")
                            .max_h(px(160.))
                            .overflow_y_scroll()
                            .px(px(10.))
                            .pb(px(6.))
                            .font_family("monospace")
                            .children(self.logs.iter().map(|(level, text)| {
                                div()
                                    .text_color(match level.as_str() {
                                        "error" => palette.destructive,
                                        "warn" => gpui_kit::rgb(0xb45309).into(),
                                        _ => palette.foreground,
                                    })
                                    .child(text.clone())
                            })),
                    )
            });
        (request, console)
    }

    /// The agent's button in the toolbar, for a local Code session's browser: lit while the agent may use the browser.
    /// Clicking it takes the browser back (every tab), or hands every tab to the agent again, as the session starts.
    fn render_agent_toggle(&self, palette: Palette, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if !self.agent_control {
            return None;
        }
        let allowed = self.tabs.trust_all || self.tabs.tabs.iter().any(|tab| tab.agent_allowed);
        let tip: SharedString = if allowed {
            format!("{} can use this browser. Click to take it back.", self.agent_name)
        } else {
            format!("Let {} use this browser", self.agent_name)
        }
        .into();
        Some(
            div()
                .id("browser-agent")
                .size(px(26.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .active(|style| style.opacity(0.8))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .when(allowed, |button| button.bg(aura_color().opacity(0.16)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if allowed {
                        this.take_over_agent(cx);
                    } else {
                        this.agent_paused = false;
                        this.tabs.control_all(true);
                        this.end_aura();
                        cx.notify();
                    }
                }))
                .child(Icon::new(Lucide::Bot).size(px(14.)).text_color(if allowed {
                    aura_color()
                } else {
                    palette.icon
                })),
        )
    }

    /// Whether the aura shows: the agent acts on the tab on screen now, or did a moment ago, so the glow does not
    /// blink between its steps. It lingers only while the agent still has control of that same tab, on screen.
    /// Lingering, it asks for the draw that ends it.
    fn aura_lit(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self
            .tabs
            .current()
            .filter(|tab| !self.agent_paused && tab.agent_allowed)
        else {
            return false;
        };
        if tab.activity.is_some() {
            self.agent_seen = Some((tab.id.clone(), Instant::now()));
            return true;
        }
        let Some(left) = self
            .agent_seen
            .as_ref()
            .filter(|(seen, _)| *seen == tab.id)
            .and_then(|(_, at)| AURA_LINGER.checked_sub(at.elapsed()))
        else {
            return false;
        };
        self._aura_wake = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(left).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        }));
        true
    }

    /// Over the page while the agent drives it: a soft glow in from each edge, breathing unless the person asked for
    /// less motion, and a pill at the foot that says so, with Take over and Stop. It takes no clicks but its buttons'.
    fn render_aura(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let glow = aura_color();
        let edge = |angle: f32| {
            linear_gradient(
                angle,
                linear_color_stop(glow.opacity(0.32), 0.),
                linear_color_stop(glow.opacity(0.), 1.),
            )
        };
        let depth = px(AURA_DEPTH);
        let ring = div()
            .absolute()
            .inset_0()
            .child(div().absolute().top_0().left_0().right_0().h(depth).bg(edge(180.)))
            .child(div().absolute().bottom_0().left_0().right_0().h(depth).bg(edge(0.)))
            .child(div().absolute().top_0().bottom_0().left_0().w(depth).bg(edge(90.)))
            .child(div().absolute().top_0().bottom_0().right_0().w(depth).bg(edge(270.)))
            .child(div().absolute().inset_0().border_2().border_color(glow.opacity(0.55)));
        let ring = if cx.reduce_motion() {
            ring.into_any_element()
        } else {
            ring.with_animation(
                "browser-aura",
                Animation::new(Duration::from_millis(2400))
                    .repeat()
                    .with_max_fps(crate::motion::LOOP_FPS),
                |ring, delta| ring.opacity(0.65 + 0.35 * (delta * std::f32::consts::TAU).cos().abs()),
            )
            .into_any_element()
        };
        let action = |id: &'static str| {
            div()
                .id(id)
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(px(12.))
                .text_color(palette.foreground)
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
        };
        let pill = h_flex()
            .id("browser-agent-pill")
            .debug_selector(|| "browser-agent-pill".into())
            .occlude()
            .h(px(32.))
            .pl(px(10.))
            .pr(px(4.))
            .gap(px(6.))
            .rounded(px(16.))
            .bg(palette.menu)
            .border_1()
            .border_color(palette.border)
            .shadow_md()
            .text_size(px(12.))
            .child(div().flex_none().size(px(6.)).rounded_full().bg(glow))
            .child(
                div()
                    .text_color(palette.foreground)
                    .whitespace_nowrap()
                    .child(format!("{} is browsing", self.agent_name)),
            )
            .child(
                action("browser-take-over")
                    .tooltip(|window, cx| {
                        Tooltip::new("Use the page yourself; the agent keeps working on the rest").build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.take_over_agent(cx)))
                    .child("Take over"),
            )
            .child(
                action("browser-stop")
                    .tooltip(|window, cx| Tooltip::new("Stop the agent").build(window, cx))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.take_over_agent(cx);
                        cx.emit(BrowserEvent::StopAgent);
                    }))
                    .child("Stop"),
            );
        div()
            .absolute()
            .inset_0()
            .debug_selector(|| "browser-aura".into())
            .child(ring)
            .child(
                h_flex()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(16.))
                    .justify_center()
                    .child(pill),
            )
            .into_any_element()
    }

    /// Code-session navigation belongs to the Follow / Manual controller. Server probes remain visible choices.
    pub fn manage_navigation(&mut self) {
        self.navigation_managed = true;
        self.pending_open = None;
    }

    /// The website profile its pages use.
    pub fn profile(&self) -> nexus_browser::profile::Profile {
        self.profile
    }

    /// The tabs, in order, and the one on screen: for the agent's browser tools.
    #[allow(dead_code)]
    pub fn tabs(&self) -> (&[Tab], Option<&Tab>) {
        (&self.tabs.tabs, self.tabs.current())
    }

    /// Tab `id`'s page, once it has one: for the agent's browser tools.
    #[allow(dead_code)]
    pub fn page(&self, id: &str) -> Option<Entity<WebView>> {
        self.pages.get(id).map(|(page, _)| page.clone())
    }

    /// Show `url` (already a web address): the tab that shows it, else a blank tab, else a new one, else the tab on
    /// screen, as an address asked from a link's menu in the Nexus app.
    pub fn open(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_retired() {
            return;
        }
        self.typed = true;
        match self.tabs.ask(url) {
            Asked::Shown(_) => self.show_current(window, cx),
            Asked::Visit(_) => self.visit(url, window, cx),
        }
    }

    /// The person chose where the browser goes: the session's servers no longer open on their own.
    fn chose(&mut self, cx: &mut Context<Self>) {
        self.typed = true;
        cx.emit(BrowserEvent::Navigated);
    }

    /// `url` in a new tab, as the person's "+" then the address would.
    pub fn open_new(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_retired() {
            return;
        }
        let Some(url) = address::preview_url(url) else {
            return;
        };
        match self.tabs.add("") {
            Ok(_) => self.visit(&url, window, cx),
            Err(full) => {
                self.problem = Some(full.into());
                cx.notify();
            }
        }
    }

    /// What the person typed: open it in the tab on screen (a new tab when there is none), search for it, or say why
    /// not.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_retired() {
            return;
        }
        let typed = self.address.read(cx).value().to_string();
        match address::entered(&typed) {
            Entered::Nothing => {}
            Entered::Refused => {
                self.problem = Some(address::REFUSED.into());
                cx.notify();
            }
            Entered::Open(url) => {
                self.chose(cx);
                if self.tabs.current().is_none()
                    && let Err(full) = self.tabs.add("")
                {
                    self.problem = Some(full.into());
                    cx.notify();
                    return;
                }
                let same = self.tabs.current().is_some_and(|tab| tab.url == url);
                if same {
                    self.go(0, cx);
                }
                self.visit(&url, window, cx);
            }
        }
    }

    /// Open `url` in the tab on screen: its page goes there, keeping its history, or the tab gets its page.
    fn visit(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_retired() {
            return;
        }
        let Some(tab) = self.tabs.current_mut() else {
            return;
        };
        let route = server_preview::route(self.place.as_ref(), url);
        // The recent addresses are this computer's: a server's `localhost` would open the wrong one here.
        if route == server_preview::Route::Direct
            && preview::is_local(url)
            && let Ok(mut recent) = RECENT.lock()
        {
            let kept = preview::without_query(url);
            recent.retain(|seen| *seen != kept);
            recent.insert(0, kept);
            recent.truncate(RECENT_MAX);
        }
        tab.url = url.to_string();
        let id = tab.id.clone();
        self.problem = None;
        self.forwarding = None;
        match route {
            server_preview::Route::Direct => self.load(&id, url, window, cx),
            server_preview::Route::Forward(port) => self.forward(id, url.to_string(), port, window, cx),
        }
        self.address
            .update(cx, |input, cx| input.set_value(url.to_string(), window, cx));
        cx.notify();
    }

    /// Tab `id`'s page goes to `url`, or the tab gets its page.
    fn load(&mut self, id: &str, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.pages.get(id) {
            Some((page, _)) => page.read(cx).navigate(url),
            None => self.open_page(id, url, window, cx),
        }
    }

    /// A server's `url` in tab `id`: its port there is forwarded here (off the UI thread), then the page loads it
    /// through the forward. The tab keeps the server's address.
    fn forward(&mut self, id: String, url: String, port: u16, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = &self.server else {
            return;
        };
        let job = crate::code_run::runtime().spawn((server.opener)(port));
        self.forwarding = Some(cx.spawn_in(window, async move |this, cx| {
            let opened = job.await.unwrap_or_else(|error| Err(error.to_string()));
            let updated = this.update_in(cx, |this, window, cx| {
                this.forwarded(&id, &url, port, opened, window, cx)
            });
            if let Err(error) = updated {
                eprintln!("nexus-desktop-gpui: the forward to port {port} finished after its browser closed: {error}");
            }
        }));
    }

    /// The forward for tab `id`'s `url` started, or why not. The tab may have moved on meanwhile.
    fn forwarded(
        &mut self,
        id: &str,
        url: &str,
        port: u16,
        opened: Result<u16, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_retired() || self.tabs.find_mut(id).is_none_or(|tab| tab.url != url) {
            return;
        }
        let Some(server) = &mut self.server else {
            return;
        };
        match opened {
            Ok(local) => {
                server.remember(port, local);
                if let Some(page_url) = server.page_url(url) {
                    self.load(id, &page_url, window, cx);
                }
            }
            Err(error) => {
                self.problem = Some(format!("Could not reach port {port} on {}: {error}", server.name).into());
            }
        }
        cx.notify();
    }

    fn open_page(&mut self, tab: &str, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_retired() {
            return;
        }
        let session = self.session.clone();
        let profile = self.profile;
        let page = cx.new(|cx| WebView::browser(url, &session, profile, window, cx));
        let id = tab.to_string();
        let subscription = cx.subscribe_in(&page, window, move |this, _, event: &WebEvent, window, cx| {
            this.on_page(&id, event, window, cx);
        });
        self.pages.insert(tab.to_string(), (page, subscription));
    }

    fn on_page(&mut self, id: &str, event: &WebEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_retired() {
            return;
        }
        let on_screen = self.tabs.current().is_some_and(|tab| tab.id == id);
        match event {
            WebEvent::TitleChanged(title) => {
                if let Some(tab) = self.tabs.find_mut(id) {
                    tab.title = Some(title.clone()).filter(|title| !title.trim().is_empty());
                }
            }
            // A blank page (a new page before its first address) or an engine page is not an address the tab shows,
            // as in the Nexus app.
            WebEvent::Address(url) if nexus_browser::preview::browser_url(url).is_err() => return,
            WebEvent::Address(url) => {
                // A server's page shows the server's address, not the forward's.
                let url = &self
                    .server
                    .as_ref()
                    .map_or_else(|| url.clone(), |server| server.on_server(url));
                if let Some(tab) = self.tabs.find_mut(id) {
                    tab.url = url.clone();
                }
                // The person may be typing: the page's address waits until they are done.
                let typing = self.address.read(cx).focus_handle(cx).is_focused(window);
                if on_screen && !typing {
                    self.address
                        .update(cx, |input, cx| input.set_value(url.clone(), window, cx));
                }
            }
            WebEvent::Loading(loading) => {
                if let Some(tab) = self.tabs.find_mut(id) {
                    tab.loading = *loading;
                }
            }
            WebEvent::LoadFailed(_) => {}
            WebEvent::Popup(opened) => {
                if let Some(popup) = popup::Popup::open(opened.clone(), opened.user_gesture, cx) {
                    self.popups.push((id.to_string(), popup));
                }
                return;
            }
            WebEvent::Cdp(message) => {
                self.import_answer(id, message, cx);
                self.capture_answer(id, message, cx);
                return;
            }
            // A browser tab is never veiled.
            WebEvent::Closed | WebEvent::Shown | WebEvent::Handoff(_) => return,
        }
        cx.notify();
    }

    /// The address bar follows the tab on screen.
    pub fn show_agent_tab(&mut self, tab: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.tabs.iter().position(|page| page.id == tab) {
            self.tabs.select(index);
            self.show_current(window, cx);
        }
    }

    fn show_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.tabs.current().map(|tab| tab.url.clone()).unwrap_or_default();
        self.problem = None;
        self.address.update(cx, |input, cx| input.set_value(url, window, cx));
        cx.notify();
    }

    fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.chose(cx);
        self.tabs.select(index);
        self.show_current(window, cx);
    }

    /// The Browser was shown: a blank tab puts the keys in the address bar, ready for typing. A tab with
    /// a page keeps them where they were, so a shown page never loses a click's focus.
    pub fn focus_if_blank(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.current().is_none_or(|tab| tab.url.is_empty()) {
            self.address.update(cx, |input, cx| input.focus(window, cx));
        }
    }

    fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_retired() {
            return;
        }
        self.chose(cx);
        if let Err(full) = self.tabs.add("") {
            self.problem = Some(full.into());
            cx.notify();
            return;
        }
        self.show_current(window, cx);
        self.address.update(cx, |input, cx| input.focus(window, cx));
    }

    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.close(index) else {
            return;
        };
        self.chose(cx);
        if self.agent_tab.as_deref() == Some(&tab.id) {
            self.agent_tab = None;
        }
        self.pages.remove(&tab.id);
        self.close_popups(Some(&tab.id), cx);
        self.show_current(window, cx);
    }

    fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_retired() {
            return;
        }
        let Some(index) = self.tabs.reopen() else {
            return;
        };
        self.chose(cx);
        let tab = &self.tabs.tabs[index];
        let (id, url) = (tab.id.clone(), tab.url.clone());
        match server_preview::route(self.place.as_ref(), &url) {
            server_preview::Route::Direct => self.open_page(&id, &url, window, cx),
            server_preview::Route::Forward(port) => self.forward(id, url, port, window, cx),
        }
        self.show_current(window, cx);
    }

    /// Back (-1), forward (1) or reload (0), in the tab on screen.
    fn go(&mut self, step: i32, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.current() else {
            return;
        };
        if let Some((page, _)) = self.pages.get(&tab.id) {
            page.update(cx, |page, cx| page.go(step, cx));
        }
    }

    /// The local addresses the session printed, newest first. New ones are checked at once.
    /// A cloud machine's addresses are the machine's own, not this computer's: they are not checked or opened here.
    pub fn set_found(&mut self, found: Vec<String>, cx: &mut Context<Self>) {
        if !self.place.as_ref().is_some_and(Place::checks_servers) {
            return;
        }
        let found: Vec<String> = found
            .iter()
            .filter_map(|address| address::preview_url(address))
            .filter(|address| preview::is_local(address))
            .fold(Vec::new(), |mut all, address| {
                if !all.contains(&address) {
                    all.push(address);
                }
                all
            });
        let found = found.into_iter().take(tabs::MAX_TABS).collect::<Vec<_>>();
        if found == self.found {
            return;
        }
        self.found = found;
        self.check(cx);
    }

    /// Check the tabs' and the servers' addresses once, while the pane shows, as the Nexus app does every few
    /// seconds: no redirect, proxy, cookie or credential reaches a dev server, and a website is not fetched at all.
    fn check(&mut self, cx: &mut Context<Self>) {
        if self.is_retired() {
            return;
        }
        if self.drawn.elapsed() > preview::PROBE_EVERY * 2 {
            return;
        }
        let tab_urls: Vec<String> = self.tabs.tabs.iter().map(|tab| tab.url.clone()).collect();
        let addresses = preview::addresses(&tab_urls, &self.found);
        if addresses.is_empty() {
            self.probes.clear();
            return;
        }
        let runtime = crate::code_run::runtime();
        // A server's addresses are checked through their forwards.
        let opener = self.server.as_ref().map(|server| server.opener.clone());
        let jobs: Vec<_> = addresses
            .into_iter()
            .map(|address| match &opener {
                Some(opener) => runtime.spawn(server_preview::probe(opener.clone(), address)),
                None => runtime.spawn(async move { (nexus_browser::preview::probe(address).await, None) }),
            })
            .collect();
        cx.spawn(async move |this, cx| {
            let mut probes = Vec::new();
            let mut forwards = Vec::new();
            for job in jobs {
                if let Ok((probe, forward)) = job.await {
                    probes.push(probe);
                    forwards.extend(forward);
                }
            }
            let _ = this.update(cx, |this, cx| this.checked(probes, forwards, cx));
        })
        .detach();
    }

    /// The checks answered: a server that answers opens until the person chooses an address. A server's forward that
    /// started again on another port takes the page on screen with it.
    fn checked(
        &mut self,
        probes: Vec<nexus_browser::preview::Probe>,
        forwards: Vec<(u16, u16)>,
        cx: &mut Context<Self>,
    ) {
        if self.is_retired() {
            return;
        }
        if let Some(server) = &mut self.server {
            let moved: Vec<u16> = forwards
                .into_iter()
                .filter(|(remote, local)| server.remember(*remote, *local))
                .map(|(remote, _)| remote)
                .collect();
            for tab in &self.tabs.tabs {
                let Some(page_url) = server_preview::server_port(&tab.url)
                    .filter(|port| moved.contains(port))
                    .and_then(|_| server.page_url(&tab.url))
                else {
                    continue;
                };
                if let Some((page, _)) = self.pages.get(&tab.id) {
                    page.read(cx).navigate(&page_url);
                }
            }
        }
        self.probes = probes;
        let current = self.tabs.current().map(|tab| tab.url.clone()).unwrap_or_default();
        if let Some(url) = preview::auto_open(
            self.typed || self.navigation_managed,
            &self.found,
            &self.probes,
            &current,
        ) {
            self.pending_open = Some(url);
        }
        cx.notify();
    }

    /// A recovery button of a server that does not answer.
    fn recover(&mut self, recovery: &preview::Recovery, window: &mut Window, cx: &mut Context<Self>) {
        match recovery {
            preview::Recovery::TryAgain => {
                self.check(cx);
                self.go(0, cx);
            }
            preview::Recovery::Open(address) => {
                self.chose(cx);
                self.open_found(address, window, cx);
            }
            preview::Recovery::AskAgent(text) => cx.emit(BrowserEvent::Text(text.clone())),
        }
    }

    /// A server to open that the checks chose, in the tab on screen.
    fn open_found(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.current().is_none() && self.tabs.add("").is_err() {
            return;
        }
        self.visit(url, window, cx);
    }

    /// "Screenshot into the message": ask the page on screen for what it shows.
    fn screenshot(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.current() else {
            return;
        };
        let Some((page, _)) = self.pages.get(&tab.id) else {
            return;
        };
        self.next_capture += 1;
        let id = self.next_capture;
        page.read(cx).send_cdp(preview::capture_message(id));
        let wait = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(preview::CAPTURE_WAIT).await;
            let _ = this.update(cx, |this, cx| {
                if this.capture.as_ref().is_some_and(|capture| capture.id == id) {
                    this.capture = None;
                    this.problem = Some("The screenshot timed out.".into());
                    cx.notify();
                }
            });
        });
        self.capture = Some(Capture {
            tab: tab.id.clone(),
            id,
            _wait: wait,
        });
    }

    fn capture_answer(&mut self, tab: &str, message: &str, cx: &mut Context<Self>) {
        let Some(capture) = self.capture.as_ref().filter(|capture| capture.tab == tab) else {
            return;
        };
        let Some(answer) = preview::capture_answer(capture.id, message) else {
            return;
        };
        self.capture = None;
        match answer {
            Ok(image) => cx.emit(BrowserEvent::Screenshot(image)),
            Err(problem) => {
                self.problem = Some(problem.into());
                cx.notify();
            }
        }
    }

    /// Whether "Import sign-ins" shows: only on this computer, as in the Nexus app (`nexus_browser::place`).
    fn imports_sign_ins(&self) -> bool {
        self.place.as_ref().is_some_and(Place::imports_sign_ins)
    }

    /// Show or hide "Import sign-ins", with the page's site in its Sites field.
    fn toggle_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.imports_sign_ins() {
            return;
        }
        if self.cookie_form.take().is_some() {
            cx.notify();
            return;
        }
        let site = self
            .tabs
            .current()
            .map(|tab| cookies::site_of(&tab.url))
            .unwrap_or_default();
        let form = cx.new(|cx| CookieForm::new(site, window, cx));
        let subscription = cx.subscribe(&form, |this, form, event: &FormEvent, cx| match event {
            FormEvent::Close => {
                this.cookie_form = None;
                cx.notify();
            }
            FormEvent::Write(read) => this.write_cookies(form, read.clone(), cx),
        });
        self.cookie_form = Some((form, subscription));
        cx.notify();
    }

    /// Write the imported cookies into the engine's store, through the page on screen. The store is the whole
    /// browser's, so every tab gets them.
    fn write_cookies(&mut self, form: Entity<CookieForm>, read: cookies::Read, cx: &mut Context<Self>) {
        let page = self
            .tabs
            .current()
            .and_then(|tab| self.pages.get(&tab.id).map(|(page, _)| (tab.id.clone(), page.clone())));
        let Some((tab, page)) = page else {
            form.update(cx, |form, cx| form.finish(cookies::NO_PAGE.into(), cx));
            return;
        };
        if read.cookies.is_empty() {
            form.update(cx, |form, cx| form.finish(cookies::summary(&read, 0), cx));
            return;
        }
        let mut pending = HashSet::new();
        for cookie in &read.cookies {
            self.next_import += 1;
            pending.insert(self.next_import);
            page.read(cx).send_cdp(cookies::set_cookie(self.next_import, cookie));
        }
        let wait = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(IMPORT_WAIT).await;
            let _ = this.update(cx, |this, cx| this.finish_import(cx));
        });
        self.import = Some(Import {
            tab,
            read,
            pending,
            set: 0,
            _wait: wait,
        });
    }

    /// The engine's answer to one of the import's cookies.
    fn import_answer(&mut self, tab: &str, message: &str, cx: &mut Context<Self>) {
        let Some(import) = self.import.as_mut().filter(|import| import.tab == tab) else {
            return;
        };
        let Some((id, set)) = cookies::set_answer(message) else {
            return;
        };
        if import.pending.remove(&id) && set {
            import.set += 1;
        }
        if import.pending.is_empty() {
            self.finish_import(cx);
        }
    }

    /// Every cookie answered, or the wait ran out: the form says how many were set, and the page reloads to use them.
    fn finish_import(&mut self, cx: &mut Context<Self>) {
        let Some(import) = self.import.take() else {
            return;
        };
        if let Some((form, _)) = &self.cookie_form {
            let said = cookies::summary(&import.read, import.set);
            form.update(cx, |form, cx| form.finish(said, cx));
        }
        if import.set > 0 {
            self.go(0, cx);
        }
        cx.notify();
    }

    /// Close the popups of `tab`, or every popup: the tab, or the whole browser, closed.
    pub fn close_popups(&mut self, tab: Option<&str>, cx: &mut App) {
        let (closing, kept) = std::mem::take(&mut self.popups)
            .into_iter()
            .partition(|(opener, _)| tab.is_none_or(|tab| tab == opener));
        self.popups = kept;
        for (_, window) in closing {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    }

    /// The session's servers, each by its port with a dot for its check, while no website is on screen.
    fn render_servers(&self, url: &str, palette: Palette, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let servers = preview::servers(&self.found);
        if servers.is_empty() || !(url.is_empty() || preview::is_local(url)) {
            return None;
        }
        Some(
            h_flex()
                .flex_none()
                .h(px(32.))
                .px(px(10.))
                .gap(px(4.))
                .text_size(px(12.))
                .child(div().mr(px(4.)).text_color(palette.muted).child("Servers"))
                .children(servers.into_iter().enumerate().map(|(index, address)| {
                    let probe = preview::probe_of(&self.probes, &address);
                    let dot = match probe {
                        Some(probe) if probe.reachable => gpui_kit::rgb(0x22c55e).into(),
                        Some(_) => palette.destructive,
                        None => palette.faint,
                    };
                    let tip = probe
                        .and_then(|probe| probe.error.clone())
                        .unwrap_or_else(|| address.clone());
                    let chosen = url == address;
                    let label = preview::port_label(&address);
                    h_flex()
                        .id(("browser-server", index))
                        .h(px(24.))
                        .px(px(8.))
                        .gap(px(6.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .when(chosen, |chip| chip.bg(palette.selected).text_color(palette.foreground))
                        .when(!chosen, |chip| {
                            chip.text_color(palette.muted).hover(|style| style.bg(palette.hover))
                        })
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.chose(cx);
                            this.open_found(&address, window, cx);
                        }))
                        .child(div().size(px(6.)).rounded_full().bg(dot))
                        .child(label)
                })),
        )
    }

    /// Desktop and Phone, as Cursor's browser: the chosen one is filled.
    fn render_widths(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .flex_none()
            .gap(px(2.))
            .children(Width::ALL.into_iter().map(|width| {
                tool(width.id(), width.icon(), width.label(), true, palette)
                    .when(self.width == width, |button| button.bg(palette.selected))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.width = width;
                        cx.notify();
                    }))
            }))
    }

    /// The page at the chosen width: the pane's, or a device's, centred and never wider than the pane.
    fn render_page(&self, page: Entity<WebView>) -> Div {
        let frame = h_flex().flex_1().min_h(px(0.)).w_full().justify_center();
        let surface = div().h_full().min_w(px(0.)).child(page);
        match self.width.pixels() {
            None => frame.child(surface.flex_1()),
            Some(pixels) => frame.child(surface.w(px(pixels)).max_w_full()),
        }
    }

    fn render_tabs(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let full = self.tabs.full();
        h_flex()
            .id("browser-tabs")
            .flex_none()
            .h(px(34.))
            .px(px(6.))
            .gap(px(2.))
            .overflow_x_scroll()
            .children(self.tabs.tabs.iter().enumerate().map(|(index, tab)| {
                let selected = index == self.tabs.current;
                let label = tab.label();
                let tip = tab.title.clone().filter(|_| !tab.url.is_empty()).unwrap_or_else(|| {
                    if tab.url.is_empty() {
                        "New tab".into()
                    } else {
                        tab.url.clone()
                    }
                });
                h_flex()
                    .id(("browser-tab", index))
                    .flex_none()
                    .max_w(px(180.))
                    .h(px(26.))
                    .pl(px(8.))
                    .pr(px(2.))
                    .gap(px(6.))
                    .rounded(px(6.))
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .when(selected, |tab| tab.bg(palette.selected).text_color(palette.foreground))
                    .when(!selected, |tab| {
                        tab.text_color(palette.muted).hover(|style| style.bg(palette.hover))
                    })
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| this.select(index, window, cx)))
                    .child(Icon::new(Lucide::Globe).size(px(14.)).text_color(palette.icon))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(label.clone()))
                    .child(
                        div()
                            .id(("browser-tab-close", index))
                            .size(px(20.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .hover(|style| style.bg(palette.hover))
                            .tooltip(move |window, cx| {
                                Tooltip::new(format!("Close {}", address::tab_label(&label))).build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(index, window, cx);
                            }))
                            .child(Icon::new(Lucide::X).size(px(12.)).text_color(palette.icon)),
                    )
            }))
            .child(
                tool("browser-new-tab", Lucide::Plus, "New browser tab", !full, palette)
                    .on_click(cx.listener(|this, _, window, cx| this.new_tab(window, cx))),
            )
            .when(!self.tabs.closed.is_empty(), |strip| {
                strip.child(
                    tool("browser-reopen", Lucide::Undo2, "Reopen closed tab", !full, palette)
                        .on_click(cx.listener(|this, _, window, cx| this.reopen(window, cx))),
                )
            })
    }
}

/// A 26px icon button in the browser's bars, with its name as a tooltip; dimmed and inert when not `enabled`.
fn tool(id: &'static str, icon: Lucide, name: &'static str, enabled: bool, palette: Palette) -> Stateful<Div> {
    frame(id, name, enabled, palette).child(Icon::new(icon).size(px(14.)).text_color(palette.icon))
}

/// A bordered text button, as the control bar's and the Nexus app's `head-action`.
fn action_button(id: impl Into<ElementId>, label: impl Into<SharedString>, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.))
        .px(px(8.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(palette.border)
        .text_color(palette.foreground)
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .child(label.into())
}

/// [`tool`]'s button, around any icon.
fn frame(id: &'static str, name: &'static str, enabled: bool, palette: Palette) -> Stateful<Div> {
    let button = div()
        .id(id)
        .size(px(26.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .tooltip(move |window, cx| Tooltip::new(name).build(window, cx));
    if enabled {
        button
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .active(|style| style.opacity(0.8))
    } else {
        button.opacity(0.4)
    }
}

impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<crate::pages::Theme>().0;
        if self.is_retired() {
            return super::empty_state(
                None,
                Some("Browser moved."),
                "Use its current window or return it to the session.",
                palette,
            )
            .into_any_element();
        }
        if self.place.is_none() {
            // A server's session, as the Nexus app says it.
            return super::empty_state(
                Some(Lucide::Globe),
                Some("Browser is local to this computer."),
                "Remote sessions can't use the browser yet.",
                palette,
            )
            .into_any_element();
        }
        let hidden_for = self.drawn.elapsed();
        self.drawn = Instant::now();
        if hidden_for > preview::PROBE_EVERY * 2 {
            // Back on screen: check now, not at the next turn.
            self.check(cx);
        }
        if let Some(url) = self.pending_open.take() {
            self.open_found(&url, window, cx);
        }
        let tab = self.tabs.current().cloned();
        let url = tab.as_ref().map(|tab| tab.url.clone()).unwrap_or_default();
        let loading = tab.as_ref().map(|tab| tab.loading).unwrap_or_default();
        let has_page = !url.is_empty();
        let frame_problem = preview::frame_problem(&self.probes, &url);
        let reload = if loading.loading {
            Icon::new(Lucide::RotateCw)
                .size(px(14.))
                .text_color(palette.icon)
                .with_animation(
                    "browser-loading",
                    Animation::new(Duration::from_millis(900))
                        .repeat()
                        .with_max_fps(crate::motion::LOOP_FPS),
                    |icon, delta| icon.transform(Transformation::rotate(percentage(delta))),
                )
                .into_any_element()
        } else {
            Icon::new(Lucide::RotateCw)
                .size(px(14.))
                .text_color(palette.icon)
                .into_any_element()
        };
        let reload_button = frame("browser-reload", "Reload", has_page, palette).child(reload);
        let bar = h_flex()
            .flex_none()
            .h(px(40.))
            .px(px(6.))
            .gap(px(2.))
            .child({
                let button = tool("browser-back", Lucide::ArrowLeft, "Back", loading.back, palette);
                if loading.back {
                    button.on_click(cx.listener(|this, _, _, cx| this.go(-1, cx)))
                } else {
                    button
                }
            })
            .child({
                let button = tool(
                    "browser-forward",
                    Lucide::ArrowRight,
                    "Forward",
                    loading.forward,
                    palette,
                );
                if loading.forward {
                    button.on_click(cx.listener(|this, _, _, cx| this.go(1, cx)))
                } else {
                    button
                }
            })
            .child(if has_page {
                reload_button.on_click(cx.listener(|this, _, _, cx| this.go(0, cx)))
            } else {
                reload_button
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .ml(px(4.))
                    .h(px(28.))
                    .px(px(10.))
                    .gap(px(8.))
                    .rounded(px(8.))
                    .bg(super::field_fill(palette))
                    .text_size(px(12.5))
                    // Ctrl+` goes to the terminal from this field as from every other. The key binding does it; this
                    // is for a keystroke the field's input takes before the binding matches.
                    .on_key_down(|event, window, cx| {
                        let keys = &event.keystroke;
                        if keys.key == "`" && keys.modifiers.control && !keys.modifiers.shift {
                            window.dispatch_action(crate::workspace::ToggleTerminal.boxed_clone(), cx);
                            cx.stop_propagation();
                        }
                    })
                    // The page's dot: up once it has loaded.
                    .child(div().flex_none().size(px(6.)).rounded_full().bg(
                        if has_page && !loading.loading && frame_problem.is_none() {
                            gpui_kit::rgb(0x22c55e).into()
                        } else {
                            palette.faint
                        },
                    ))
                    // A server session's addresses are the server's: the bar names it.
                    .children(self.server.as_ref().map(|server| {
                        let tip = server.tooltip();
                        div()
                            .id("browser-server-name")
                            .flex_none()
                            .max_w(px(120.))
                            .truncate()
                            .text_color(palette.muted)
                            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                            .child(server.name.clone())
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(Input::new(&self.address).appearance(false)),
                    ),
            )
            .child(self.render_widths(palette, cx))
            .child({
                let button = tool(
                    "browser-screenshot",
                    Lucide::Camera,
                    "Screenshot into the message",
                    has_page,
                    palette,
                );
                if has_page {
                    button.on_click(cx.listener(|this, _, _, cx| this.screenshot(cx)))
                } else {
                    button
                }
            })
            .child({
                let button = tool(
                    "browser-pick",
                    Lucide::MousePointer2,
                    "Pick an element for the message",
                    has_page,
                    palette,
                );
                if has_page {
                    button.on_click(cx.listener(|this, _, window, cx| this.pick(window, cx)))
                } else {
                    button
                }
            })
            .when(self.imports_sign_ins(), |bar| {
                bar.child({
                    let importing = self.cookie_form.is_some();
                    let button = tool(
                        "browser-import",
                        Lucide::KeyRound,
                        "Import sign-ins from your browser",
                        has_page,
                        palette,
                    )
                    .when(importing, |button| button.bg(palette.selected));
                    if has_page {
                        button.on_click(cx.listener(|this, _, window, cx| this.toggle_import(window, cx)))
                    } else {
                        button
                    }
                })
            })
            .children(self.render_agent_toggle(palette, cx))
            .child({
                let button = tool(
                    "browser-external",
                    Lucide::ExternalLink,
                    "Open in your browser",
                    has_page,
                    palette,
                );
                if has_page {
                    // A server's page opens through its forward: the server's `localhost` is not this computer's.
                    let url = self
                        .server
                        .as_ref()
                        .and_then(|server| server.page_url(&url))
                        .unwrap_or_else(|| url.clone());
                    button.on_click(move |_, _, cx| crate::external_link::open(&url, cx))
                } else {
                    button
                }
            });
        let page = tab
            .as_ref()
            .and_then(|tab| self.pages.get(&tab.id))
            .map(|(page, _)| page.clone());
        let (request, console) = self.render_agent(palette, cx);
        let aura = has_page && self.aura_lit(cx);
        let body = match page {
            Some(page) if has_page => self
                .render_page(page)
                .relative()
                .when(aura, |page| page.child(self.render_aura(palette, cx)))
                .into_any_element(),
            _ => {
                // This computer's recent addresses: a server's browser would open them on the server.
                let recent = match &self.server {
                    Some(_) => Vec::new(),
                    None => RECENT.lock().map(|recent| recent.clone()).unwrap_or_default(),
                };
                empty(self.place.as_ref(), palette)
                    // Where it was last on this computer: a chip each, one click to go again.
                    .when(!recent.is_empty(), |state| {
                        state.child(h_flex().mt(px(10.)).gap(px(6.)).flex_wrap().justify_center().children(
                            recent.into_iter().enumerate().map(|(index, url)| {
                                let label = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
                                let label = label.trim_end_matches('/').to_string();
                                div()
                                    .id(("browser-recent", index))
                                    .h(px(26.))
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(13.))
                                    .bg(palette.raised)
                                    .text_size(px(12.))
                                    .text_color(palette.muted)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if this.tabs.current().is_none()
                                            && let Err(full) = this.tabs.add("")
                                        {
                                            this.problem = Some(full.into());
                                            cx.notify();
                                            return;
                                        }
                                        this.visit(&url, window, cx);
                                    }))
                                    .child(label)
                            }),
                        ))
                    })
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            // Back and forward over the browser, from the keys or the mouse's side buttons, are the page's, as in
            // a browser: not the window's history (`Workspace::go`).
            .on_action(cx.listener(|this, _: &GoBack, _, cx| this.go(-1, cx)))
            .on_action(cx.listener(|this, _: &GoForward, _, cx| this.go(1, cx)))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.go(-1, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.go(1, cx);
                }),
            )
            .when(!self.tabs.tabs.is_empty(), |pane| {
                pane.child(self.render_tabs(palette, cx))
            })
            .child(bar)
            .children(self.render_servers(&url, palette, cx))
            .children(
                self.cookie_form
                    .as_ref()
                    .filter(|_| has_page)
                    .map(|(form, _)| form.clone()),
            )
            .children(self.problem.clone().map(|problem| {
                div()
                    .flex_none()
                    .px(px(12.))
                    .py(px(6.))
                    .text_size(px(12.5))
                    .text_color(palette.destructive)
                    .child(problem)
            }))
            .children(frame_problem.map(|problem| {
                let recovery = preview::recovery(&self.probes, &url, self.agent_control);
                v_flex()
                    .flex_none()
                    .px(px(12.))
                    .py(px(6.))
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .text_color(palette.muted)
                    .child(problem)
                    .child(h_flex().gap(px(6.)).flex_wrap().text_size(px(12.)).children(
                        recovery.into_iter().enumerate().map(|(index, recovery)| {
                            action_button(("browser-recovery", index), recovery.label(), palette)
                                .on_click(cx.listener(move |this, _, window, cx| this.recover(&recovery, window, cx)))
                        }),
                    ))
            }))
            .children(request)
            .child(body)
            .when(has_page, |pane| pane.child(console))
            .into_any_element()
    }
}

/// The aura's one colour, the same for every agent: a calm blue that reads on light and dark pages.
const AURA: u32 = 0x4F8CFF;
/// How far the aura's glow reaches in from each edge.
const AURA_DEPTH: f32 = 28.;
/// How long the aura stays after the agent's last step, so it does not blink between steps.
const AURA_LINGER: Duration = Duration::from_millis(2500);

fn aura_color() -> Hsla {
    gpui_kit::rgb(AURA).into()
}

/// The pane with no page, as the Nexus app's `PaneEmpty`.
fn empty(place: Option<&Place>, palette: Palette) -> Div {
    v_flex()
        .flex_1()
        .min_h(px(0.))
        .items_center()
        .justify_center()
        .gap(px(6.))
        .p(px(20.))
        .child(Icon::new(Lucide::Globe).size(px(20.)).text_color(palette.icon))
        .child(
            div()
                .text_size(px(13.))
                .text_color(palette.foreground)
                .child("Nothing open"),
        )
        .child(div().text_size(px(12.5)).text_color(palette.muted).child(match place {
            Some(Place::Cloud(_)) => "Open a port with Preview under the session's Cloud button, or enter a URL above.",
            Some(Place::Server(_)) => "Type a dev server's address on the server, like localhost:3000, or start one.",
            _ => "Type a URL, or start a dev server and its page opens here.",
        }))
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn an_imports_devtools_ids_fit_in_32_bits() {
        // Chromium drops a DevTools message whose id is larger, unanswered.
        assert!(IMPORT_IDS + 1_000_000 < i32::MAX as u64);
    }

    #[gpui_kit::test]
    fn the_aura_shows_while_the_agent_acts_and_lingers_briefly_after(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
                crate::palette::ThemeChoice::Dark,
                WindowAppearance::Dark,
            )));
        });
        let (browser, cx) = cx.add_window_view(|window, cx| {
            Browser::new(
                "aura".into(),
                None,
                nexus_browser::profile::Profile::new("aura-test", "aura", false),
                None,
                window,
                cx,
            )
        });
        browser.update(cx, |browser, cx| {
            let palette = cx.global::<crate::pages::Theme>().0;
            // No agent: no button, no aura.
            assert!(browser.render_agent_toggle(palette, cx).is_none());
            browser.tabs.tabs.push(tabs::Tab {
                id: "page".into(),
                url: "https://example.invalid".into(),
                ..Default::default()
            });
            assert!(!browser.aura_lit(cx));
            browser.allow_agent();
            assert!(browser.render_agent_toggle(palette, cx).is_some());
            // Allowed but idle: still no aura.
            assert!(!browser.aura_lit(cx));
            let epoch = browser.tabs.tabs[0].begin("browser_click").unwrap();
            assert!(browser.aura_lit(cx));
            // Between steps it stays a moment, then goes.
            assert!(browser.tabs.tabs[0].finish(epoch));
            assert!(browser.aura_lit(cx));
            browser.agent_seen = Some(("page".into(), Instant::now() - AURA_LINGER - Duration::from_millis(10)));
            assert!(!browser.aura_lit(cx));
            // Paused mid-step, after a draw while the agent acted: it goes at once.
            browser.tabs.tabs[0].begin("browser_click").unwrap();
            assert!(browser.aura_lit(cx));
            browser.pause_agent(true, cx);
            assert!(!browser.aura_lit(cx));
            // Resumed, then taken over mid-step: it goes at once too.
            browser.pause_agent(false, cx);
            browser.tabs.tabs[0].begin("browser_click").unwrap();
            assert!(browser.aura_lit(cx));
            browser.take_over_agent(cx);
            assert!(!browser.aura_lit(cx));
        });
    }

    #[gpui_kit::test]
    fn the_aura_does_not_follow_the_person_to_another_tab(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
                crate::palette::ThemeChoice::Dark,
                WindowAppearance::Dark,
            )));
        });
        let (browser, cx) = cx.add_window_view(|window, cx| {
            Browser::new(
                "aura-tabs".into(),
                None,
                nexus_browser::profile::Profile::new("aura-tabs-test", "aura-tabs", false),
                None,
                window,
                cx,
            )
        });
        browser.update(cx, |browser, cx| {
            browser.allow_agent();
            for id in ["driven", "other"] {
                browser.tabs.tabs.push(tabs::Tab {
                    id: id.into(),
                    agent_allowed: true,
                    url: "https://example.invalid".into(),
                    ..Default::default()
                });
            }
            browser.tabs.current = 0;
            let epoch = browser.tabs.tabs[0].begin("browser_click").unwrap();
            assert!(browser.aura_lit(cx));
            assert!(browser.tabs.tabs[0].finish(epoch));
            // The person switches to a tab the agent never touched: no glow there, linger or not.
            browser.tabs.current = 1;
            assert!(!browser.aura_lit(cx));
            // Back on the agent's tab, the linger is still its own.
            browser.tabs.current = 0;
            assert!(browser.aura_lit(cx));
        });
    }

    #[gpui_kit::test]
    fn activity_pause_invalidates_inflight_work_without_widening_tab_grants(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
                crate::palette::ThemeChoice::Dark,
                WindowAppearance::Dark,
            )));
        });
        let (browser, cx) = cx.add_window_view(|window, cx| {
            Browser::new(
                "activity-epochs".into(),
                None,
                nexus_browser::profile::Profile::new("activity-test", "epochs", false),
                None,
                window,
                cx,
            )
        });
        browser.update(cx, |browser, cx| {
            browser.agent_control = true;
            browser.tabs.tabs.push(tabs::Tab {
                id: "allowed".into(),
                agent_allowed: true,
                url: "https://user:secret@example.invalid/path?token=secret".into(),
                ..Default::default()
            });
            browser.tabs.tabs.push(tabs::Tab {
                id: "refused".into(),
                ..Default::default()
            });
            let epoch = browser.tabs.tabs[0].begin("browser_read").unwrap();
            browser.pause_agent(true, cx);
            assert!(!browser.tabs.tabs[0].finish(epoch));
            assert!(browser.tabs.tabs[0].activity.is_none());
            assert!(!browser.tabs.trust_all);
            assert!(!browser.tabs.tabs[1].agent_allowed);
            assert_eq!(browser.activity_status().unwrap().site, "example.invalid");
            browser.pause_agent(false, cx);
            assert!(browser.tabs.tabs[0].begin("browser_read").is_ok());
            assert!(!browser.tabs.tabs[1].agent_allowed);
            browser.take_over_agent(cx);
            browser.pause_agent(false, cx);
            assert!(browser.tabs.tabs.iter().all(|tab| !tab.agent_allowed));
            assert!(browser.activity_status().is_none());
        });
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.retire_permissions();
    }
}
