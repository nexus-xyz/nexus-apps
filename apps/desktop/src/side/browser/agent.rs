//! The agent's browser tools on this session's browser (`browser/access.rs`). The tools, their names and their answers are `nexus_desktop_mcp::browser`'s; a call reaches
//! [`Browser::agent`] from the app's socket (`app_socket.rs`) through [`post`].
//!
//! - A local Code session's browser starts with "Trust all tabs": the agent may drive every tab until the person takes
//!   over. After that, its next call asks: the pane shows the request with Allow, Always in this session and Don't
//!   allow, a notification says so, and the call waits up to ten minutes (`nexus_browser::access`).
//! - A page tool names a tab (from `browser_tabs`), else the tab on screen. One action runs on a tab at a time; it
//!   waits for the page to load first, unless it moves the page, which it waits for after.
//! - "Take over" and "Stop" start a new control epoch: the action in progress loses its result.
//! - The actions run on the page through its DevTools Protocol: the page script's read, click, type and console
//!   (`nexus_browser::PAGE_SCRIPT`), and `Page.captureScreenshot`. No debugging port is open to the computer.

use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::FutureExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_browser::access;
mod permissions;
pub(crate) use permissions::{Owner as PermissionOwner, Requests};
pub(crate) type Guard = Arc<dyn Fn() -> bool + Send + Sync>;
use nexus_desktop_mcp::browser::{moves, route};
use serde_json::{Value, json};

use super::Browser;

/// Every session's open requests for the browser, as the pane shows them.
pub(crate) static REQUESTS: LazyLock<Requests> = LazyLock::new(Requests::default);

/// How long a page has to finish loading, and to answer a script.
const LOAD_WAIT: Duration = Duration::from_secs(8);
const SCRIPT_WAIT: Duration = Duration::from_secs(4);
/// The largest answer a script may give.
const ANSWER_BYTES: usize = 1024 * 1024;
const INVALID: &str = "This browser tool credential is invalid or expired.";
const TOOK_OVER: &str = "The person took over the browser.";
const PAUSED: &str = "Browser access is paused. Wait for the person to resume it.";

/// The authenticated source of a browser call: this build's store and its organization, and the server whose
/// session made it, if any (`machines/server_tools.rs`). This is internal provenance, never request JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalOrigin {
    pub data: std::path::PathBuf,
    pub scope: String,
    /// The SSH server whose session made the call; `None` for a session on this computer.
    pub machine: Option<String>,
}

/// One call of the agent's browser server, from the app's socket.
pub struct Call {
    pub origin: LocalOrigin,
    /// The session whose run made it, and the run.
    pub thread: String,
    pub run: String,
    pub message: Value,
    /// Whether the run's credential still holds (the run goes on, the same person, the same organization).
    pub valid: Arc<dyn Fn() -> bool + Send + Sync>,
    pub reply: std::sync::mpsc::Sender<Value>,
}

static CALLS: OnceLock<UnboundedSender<Call>> = OnceLock::new();

/// Answer calls through their authenticated local source, reusing its existing session browser.
pub fn install(cx: &mut App) {
    let (send, mut calls) = unbounded::<Call>();
    if CALLS.set(send).is_err() {
        return;
    }
    cx.spawn(async move |cx| {
        use futures::StreamExt;
        while let Some(call) = calls.next().await {
            let cx = cx.clone();
            cx.spawn(async move |cx| {
                let Call {
                    origin,
                    thread,
                    run,
                    message,
                    valid,
                    reply,
                } = call;
                let answer = nexus_desktop_mcp::browser::handle(&message, env!("CARGO_PKG_VERSION"), |name, args| {
                    let cx = cx.clone();
                    async move { perform(cx, origin, thread, run, valid, name, args).await }
                })
                .await;
                let _ = reply.send(answer.unwrap_or(Value::Null));
            })
            .detach();
        }
    })
    .detach();
}

/// Hand a call to the windows. False before they answer calls (`install`).
pub fn post(call: Call) -> bool {
    CALLS.get().is_some_and(|calls| calls.unbounded_send(call).is_ok())
}

async fn perform(
    mut cx: AsyncApp,
    origin: LocalOrigin,
    thread: String,
    run: String,
    valid: Arc<dyn Fn() -> bool + Send + Sync>,
    name: String,
    args: Value,
) -> Result<Value, String> {
    if !valid() {
        return Err(INVALID.into());
    }
    let (window, browser, valid) = cx.update(|cx| crate::windows::agent_browser(&origin, &thread, &valid, cx))?;
    let task = window
        .update(&mut cx, |_, window, cx| {
            if !valid() {
                return Task::ready(Err(INVALID.into()));
            }
            browser.update(cx, |browser, cx| browser.agent(name, args, run, valid, window, cx))
        })
        .map_err(|_| "The app window is closed.".to_string())?;
    task.await
}

/// The answer to a script the page script runs, as the Nexus app evaluates it: its value, or its error.
pub fn script(expression: &str) -> String {
    format!(
        "(() => {{ if (!window.__nxPreview) {{ {} }} return JSON.stringify((() => {{ try {{ return {{ value: ({expression}) }}; }} catch (error) {{ return {{ error: String(error) }}; }} }})()); }})()",
        nexus_browser::PAGE_SCRIPT
    )
}

/// The value of a [`script`]'s answer (`Runtime.evaluate`'s result), or its error.
pub fn script_answer(result: &Value) -> Result<Value, String> {
    if let Some(error) = result["exceptionDetails"]["text"].as_str() {
        return Err(error.to_string());
    }
    let text = result["result"]["value"]
        .as_str()
        .ok_or("The preview did not answer.")?;
    if text.len() > ANSWER_BYTES {
        return Err("The browser response is too large.".into());
    }
    let mut value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if let Some(error) = value["error"].as_str() {
        return Err(error.to_string());
    }
    Ok(value["value"].take())
}

/// The page's screenshot as the tools give it, from `Page.captureScreenshot`'s result.
pub fn screenshot_answer(result: &Value) -> Result<Value, String> {
    let data = result["data"].as_str().ok_or("The preview could not be captured.")?;
    let size = STANDARD
        .decode(data)
        .map_err(|_| "The screenshot could not be decoded.")?
        .len();
    if size > super::preview::CAPTURE_BYTES {
        return Err("The screenshot is too large.".into());
    }
    Ok(json!({"mediaType":"image/png","data":data,"name":super::preview::CAPTURE_NAME}))
}

/// Wait for `future`, at most `wait`.
async fn within<T>(cx: &mut AsyncWindowContext, wait: Duration, future: impl Future<Output = T>) -> Option<T> {
    let timer = cx.background_executor().timer(wait).fuse();
    let future = future.fuse();
    futures::pin_mut!(timer, future);
    futures::select! {
        value = future => Some(value),
        _ = timer => None,
    }
}

async fn pause(cx: &mut AsyncWindowContext) {
    cx.background_executor().timer(Duration::from_millis(50)).await;
}

type Weak = WeakEntity<Browser>;

/// Read the browser, or say it closed.
fn read<T>(this: &Weak, cx: &mut AsyncWindowContext, read: impl FnOnce(&Browser) -> T) -> Result<T, String> {
    this.read_with(cx, |browser, _| read(browser))
        .map_err(|_| "The person closed the browser.".to_string())
}

impl Browser {
    /// A presentation owns one generation. Transfer retires it; its source and identity proof must still hold.
    pub(crate) fn activity_guard(&self, valid: Guard) -> Guard {
        let alive = self.agent_alive.clone();
        let authority = self.permission_guard.clone();
        let source = self.activity_source.clone();
        Arc::new(move || {
            alive.load(std::sync::atomic::Ordering::SeqCst)
                && authority.as_ref().is_none_or(|authority| authority())
                && source.as_ref().is_none_or(|source| source())
                && valid()
        })
    }
    pub(crate) fn bind_activity_source(&mut self, source: Guard) {
        self.activity_source = Some(source);
    }
    pub(crate) fn activity_id(&self) -> &str {
        &self.permission_instance
    }
    pub(super) fn retire_permissions(&self) {
        self.agent_alive.store(false, std::sync::atomic::Ordering::SeqCst);
        REQUESTS.retire(&self.permission_instance);
    }
    pub(crate) fn permission_request(&self) -> Option<access::Request> {
        if self.retired || self.permission_guard.as_ref().is_some_and(|valid| !valid()) {
            REQUESTS.retire(&self.permission_instance);
            return None;
        }
        REQUESTS.of(&self.permission_owner)
    }
    #[cfg(test)]
    pub(crate) fn fixture_request(&self, url: &str) -> access::Request {
        REQUESTS
            .ask(
                &self.permission_owner,
                &self.permission_instance,
                &self.session,
                url,
                None,
                Arc::new(|| true),
            )
            .expect("owner permission request")
    }
    pub(crate) fn bind_permission_owner(
        &mut self,
        origin: LocalOrigin,
        identity: crate::windows::retention::Identity,
        proof: crate::backend::ExecutionAuthority,
    ) -> bool {
        let owner = PermissionOwner::Local {
            origin,
            identity,
            session: self.session.clone(),
        };
        if self.retired || !proof.current() || self.permission_guard.as_ref().is_some_and(|valid| !valid()) {
            return false;
        }
        if matches!(&self.permission_owner, PermissionOwner::Local { .. }) && self.permission_owner != owner {
            return false;
        }
        self.permission_owner = owner;
        self.permission_guard = Some(Arc::new(move || proof.current()));
        true
    }
    #[cfg(test)]
    pub(crate) fn fixture_untrusted_tab(&mut self) -> String {
        let index = self
            .tabs
            .add("https://permission.invalid/")
            .expect("model tab without engine");
        self.tabs.control_all(false);
        self.tabs.tabs[index].id.clone()
    }
    /// Run the agent's tool `name` with `args` for the run `run`. `valid` says whether its credential still holds.
    pub fn agent(
        &mut self,
        name: String,
        args: Value,
        run: String,
        valid: Arc<dyn Fn() -> bool + Send + Sync>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value, String>> {
        if self.retired {
            return Task::ready(Err(
                "This session browser is parked. Reopen the primary Nexus window.".into()
            ));
        }
        let credential = valid.clone();
        let alive = self.agent_alive.clone();
        let valid: Guard = Arc::new(move || alive.load(std::sync::atomic::Ordering::SeqCst) && valid());
        cx.spawn_in(window, async move |this, cx| {
            if !valid() {
                return Err(INVALID.into());
            }
            let result = act(&this, cx, &name, args, &run, &valid, &credential).await;
            if !valid() {
                return Err(INVALID.into());
            }
            if read(&this, cx, |browser| browser.retired).unwrap_or(true) {
                return Err("This session browser is parked. Reopen the primary Nexus window.".into());
            }
            result
        })
    }

    /// The tabs, for `browser_tabs`.
    pub fn tabs_value(&self) -> Value {
        json!({
            "active": self.tabs.current().map(|tab| tab.id.clone()),
            "tabs": self.tabs.tabs.iter().enumerate().map(|(index, tab)| json!({
                "tab": tab.id,
                "url": tab.url,
                "active": index == self.tabs.current,
                "loading": tab.loading.loading,
                "agentAllowed": tab.agent_allowed,
            })).collect::<Vec<_>>(),
        })
    }

    pub(crate) fn live_agent_tab(&self) -> Option<String> {
        self.tabs
            .agent_target(self.agent_tab.as_deref())
            .map(|tab| tab.id.clone())
    }
    pub(crate) fn prepared_agent_tab(&self, id: &str, url: &str) -> bool {
        self.tabs
            .find(id)
            .is_some_and(|tab| tab.agent_created && tab.agent_allowed && tab.prepared_for_agent && tab.url == url)
    }

    pub(crate) fn take_prepared_agent_tab(&mut self, id: &str, url: &str) -> Option<String> {
        if !self.prepared_agent_tab(id, url) {
            return None;
        }
        self.tabs.find_mut(id)?.prepared_for_agent = false;
        Some(id.into())
    }

    /// Create an authorized agent tab in the background. Only the navigation controller selects it.
    fn agent_tab(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        let source = self
            .tabs
            .agent_target(self.agent_tab.as_deref())
            .or_else(|| self.tabs.current());
        let allowed = self.tabs.trust_all || source.is_some_and(|tab| tab.agent_allowed);
        if !allowed {
            return Err("Open this session's Browser tab and enable agent control first.".into());
        }
        if self.tabs.full() {
            return Err("The browser holds no more tabs. Open the address in a tab from browser_tabs.".into());
        }
        let shown = self.tabs.current;
        let index = self.tabs.add(url).map_err(String::from)?;
        let tab = &mut self.tabs.tabs[index];
        tab.agent_allowed = true;
        tab.agent_created = true;
        tab.loading.loading = true;
        let id = tab.id.clone();
        self.open_page(&id, url, window, cx);
        self.tabs.current = shown;
        self.agent_tab = Some(id.clone());
        cx.notify();
        Ok(id)
    }

    pub(crate) fn answer_request_id(&mut self, id: &str, answer: Answer, cx: &mut Context<Self>) {
        if self.permission_request().is_some_and(|request| request.id == id) {
            self.answer_request(answer, cx);
        }
    }

    /// The person's answer to the request on screen: allow its tab, trust all tabs, or decline.
    pub(super) fn answer_request(&mut self, answer: Answer, cx: &mut Context<Self>) {
        let Some(request) = self.permission_request() else {
            return;
        };
        if !REQUESTS.answer(&self.permission_owner, &self.permission_instance, &request.id, answer) {
            cx.emit(super::BrowserEvent::Permission);
            cx.notify();
            return;
        }
        self.apply_permission_answer(&request, answer, cx);
    }
    fn apply_permission_answer(&mut self, request: &access::Request, answer: Answer, cx: &mut Context<Self>) {
        match answer {
            Answer::Allow if request.tab.is_none() => {
                let shown = self.tabs.current;
                let had_tabs = !self.tabs.tabs.is_empty();
                if let Ok(index) = self.tabs.add(&request.url) {
                    let page = &mut self.tabs.tabs[index];
                    page.agent_created = true;
                    page.prepared_for_agent = true;
                    page.agent_allowed = true;
                    self.agent_tab = Some(page.id.clone());
                    if had_tabs {
                        self.tabs.current = shown;
                    }
                }
            }
            Answer::Allow => self.tabs.control(request.tab.as_deref(), true),
            Answer::Always => self.tabs.control_all(true),
            Answer::Decline => REQUESTS.remove(&request.id),
        }
        cx.emit(super::BrowserEvent::Permission);
        cx.notify();
    }
}

/// The person's answer to an agent's request for the browser.
#[derive(Clone, Copy)]
pub(crate) enum Answer {
    Allow,
    Always,
    Decline,
}

async fn act(
    this: &Weak,
    cx: &mut AsyncWindowContext,
    name: &str,
    args: Value,
    run: &str,
    valid: &Arc<dyn Fn() -> bool + Send + Sync>,
    credential: &Guard,
) -> Result<Value, String> {
    if !valid() {
        return Err(INVALID.into());
    }
    wait_unpaused(this, cx, valid).await?;
    match name {
        "browser_tabs" => {
            show_usage(this, cx, run, credential)?;
            return read(this, cx, |browser| browser.tabs_value());
        }
        "browser_new_tab" => {
            let url = args["url"].as_str().ok_or("An address is required.")?.to_string();
            nexus_browser::preview::browser_url(&url)?;
            let source = read(this, cx, |browser| browser.live_agent_tab())?;
            let ready = ensure(this, cx, source.as_deref(), Some(&url), run, valid).await?;
            show_usage(this, cx, run, credential)?;
            let tab = this
                .update_in(cx, |browser, window, cx| {
                    if !valid() {
                        return Err(INVALID.to_string());
                    }
                    if browser.agent_paused {
                        return Err(PAUSED.to_string());
                    }
                    if let Some(id) = ready.and_then(|id| browser.take_prepared_agent_tab(&id, &url)) {
                        browser.agent_navigate(&id, &url, window, cx);
                        Ok(id)
                    } else {
                        browser.agent_tab(&url, window, cx)
                    }
                })
                .map_err(|_| "The person closed the browser.".to_string())??;
            wait_ready(this, cx, &tab, valid).await?;
            let _ = this.update(cx, |_, cx| cx.emit(super::BrowserEvent::Activity(tab.clone())));
            return Ok(json!({"tab":tab,"url":url}));
        }
        _ => {}
    }
    let (action, tab) = route(name, &args)?;
    let explicit = tab.is_some();
    let tab = tab.or(read(this, cx, |browser| browser.live_agent_tab())?);
    let url = args["url"].as_str().map(str::to_string);
    if action == "navigate" {
        nexus_browser::preview::browser_url(url.as_deref().ok_or("An address is required.")?)?;
    }
    let ready = ensure(this, cx, tab.as_deref(), url.as_deref(), run, valid).await?;
    show_usage(this, cx, run, credential)?;
    let preserve = !explicit
        && action == "navigate"
        && ready.as_deref().is_some_and(|id| {
            read(this, cx, |browser| {
                browser.tabs.find(id).is_some_and(|page| !page.agent_created)
            })
            .unwrap_or(false)
        });
    let tab = match if preserve { None } else { ready } {
        Some(tab) => tab,
        None if action == "navigate" => {
            let url = url.clone().unwrap_or_default();
            this.update_in(cx, |browser, window, cx| {
                if !valid() {
                    return Err(INVALID.to_string());
                }
                browser.agent_tab(&url, window, cx)
            })
            .map_err(|_| "The person closed the browser.".to_string())??
        }
        None => return Err("Use browser_open with the website's address first.".into()),
    };
    if !moves(action) {
        wait_ready(this, cx, &tab, valid).await?;
    }
    let epoch = this
        .update(cx, |browser, cx| {
            if !valid() {
                return Err(INVALID.to_string());
            }
            if browser.agent_paused {
                return Err(PAUSED.to_string());
            }
            cx.notify();
            browser
                .tabs
                .find_mut(&tab)
                .ok_or("The browser tab closed.".to_string())?
                .begin(name)
        })
        .map_err(|_| "The person closed the browser.".to_string())??;
    if !valid() {
        return Err(INVALID.into());
    }
    let mut result = run_action(this, cx, action, &tab, &args, valid).await;
    if result.is_ok() && moves(action) {
        result = wait_moved(this, cx, &tab, epoch, valid).await.and(result);
    }
    let still = this
        .update(cx, |browser, cx| {
            cx.notify();
            browser.tabs.find_mut(&tab).is_some_and(|page| page.finish(epoch))
        })
        .unwrap_or(false);
    if !still || !valid() {
        return Err(if read(this, cx, |browser| browser.agent_paused).unwrap_or(false) {
            PAUSED
        } else {
            TOOK_OVER
        }
        .into());
    }
    // Every successful call is activity that Follow agent shows. Only an action that drives the page makes its tab the
    // agent's: a read, a console read or a screenshot leaves the agent's tab as it was.
    if result.is_ok() {
        let drives = !matches!(name, "browser_read" | "browser_console" | "browser_screenshot");
        let _ = this.update(cx, |browser, cx| {
            if drives {
                browser.agent_tab = Some(tab.clone());
            }
            cx.emit(super::BrowserEvent::Activity(tab.clone()));
        });
    }
    if name == "browser_console" {
        // The tab beside the logs, so Follow agent finds the call's tab in its result.
        return result.map(|value| json!({"tab":tab,"logs":value["logs"]}));
    }
    result.map(|mut value| {
        if let Some(object) = value.as_object_mut() {
            object.insert("tab".into(), json!(tab));
            value
        } else {
            json!({"tab":tab,"result":value})
        }
    })
}

/// Pause covers metadata reads too. Resume never changes the existing tab grants.
async fn wait_unpaused(
    this: &Weak,
    cx: &mut AsyncWindowContext,
    valid: &Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<(), String> {
    let deadline = Instant::now() + access::WAIT;
    while read(this, cx, |browser| browser.agent_paused)? {
        if !valid() {
            return Err(INVALID.into());
        }
        if Instant::now() >= deadline {
            return Err(PAUSED.into());
        }
        pause(cx).await;
    }
    if !valid() {
        return Err(INVALID.into());
    }
    Ok(())
}

/// The tab the call may drive: the one it names, else the tab on screen, once it allows the agent. `None` when the
/// browser has no tab yet and trusts the agent: `browser_open` opens one. When the tab does not allow the agent, the
/// person is asked, and the call waits for the answer.
async fn ensure(
    this: &Weak,
    cx: &mut AsyncWindowContext,
    tab: Option<&str>,
    url: Option<&str>,
    _run: &str,
    valid: &Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Option<String>, String> {
    if !valid() {
        return Err(INVALID.into());
    }
    wait_unpaused(this, cx, valid).await?;
    enum Found {
        Ready(Option<String>),
        Ask(Option<String>, String),
    }
    let found = read(this, cx, |browser| -> Result<Found, String> {
        let page = match tab {
            Some(id) => Some(
                browser
                    .tabs
                    .find(id)
                    .ok_or_else(|| format!("No browser tab {id}. List the tabs with browser_tabs."))?,
            ),
            None => browser.tabs.current(),
        };
        match page {
            Some(page) if page.agent_allowed => Ok(Found::Ready(Some(page.id.clone()))),
            Some(page) => Ok(Found::Ask(Some(page.id.clone()), page.url.clone())),
            None if browser.tabs.trust_all => Ok(Found::Ready(None)),
            None => Ok(Found::Ask(
                None,
                url.ok_or("Use browser_open with the website's address first.")?
                    .to_string(),
            )),
        }
    })??;
    let (asked, address) = match found {
        Found::Ready(tab) => return if valid() { Ok(tab) } else { Err(INVALID.into()) },
        Found::Ask(tab, address) => (tab, address),
    };
    let address = if address.is_empty() {
        url.unwrap_or_default().to_string()
    } else {
        address
    };
    nexus_browser::preview::browser_url(&address)?;
    let (session, title) = read(this, cx, |browser| (browser.session.clone(), browser.title.clone()))?;
    if !valid() {
        return Err(INVALID.into());
    }
    let owner = read(this, cx, |browser| browser.permission_owner.clone())?;
    let request = REQUESTS.ask(
        &owner,
        &read(this, cx, |browser| browser.permission_instance.clone())?,
        &session,
        &address,
        asked.clone(),
        valid.clone(),
    )?;
    let _lease = permissions::Lease(request.id.clone());
    if !valid() {
        return Err(INVALID.into());
    }
    use nexus_desktop_core::notify::Notifier;
    crate::notify::GpuiNotifier.show(None, &session, &title, access::NOTICE);
    let _ = this.update(cx, |_, cx| {
        cx.emit(super::BrowserEvent::Permission);
        cx.notify();
    });
    let deadline = Instant::now() + access::WAIT;
    let answer = loop {
        if !valid() {
            break Err(access::RUN_ENDED.to_string());
        }
        if !REQUESTS.contains(&request.id) {
            break Err(access::DECLINED.to_string());
        }
        let _ = this.update(cx, |browser, cx| {
            if !valid() || browser.retired {
                return;
            }
            if let Some(answer) = REQUESTS.take_answer(&owner, &browser.permission_instance, &request.id) {
                browser.apply_permission_answer(&request, answer, cx);
            }
        });
        let state = read(this, cx, |browser| match &asked {
            Some(id) => browser.tabs.find(id).map(|page| page.agent_allowed).ok_or(()),
            None => Ok(browser.tabs.trust_all
                || browser
                    .tabs
                    .agent_target(browser.agent_tab.as_deref())
                    .or_else(|| browser.tabs.current())
                    .is_some_and(|page| page.agent_allowed)),
        });
        match state {
            Ok(Ok(true)) => {
                let tab = read(this, cx, |browser| {
                    asked
                        .clone()
                        .or_else(|| browser.live_agent_tab())
                        .or_else(|| browser.tabs.current().map(|page| page.id.clone()))
                })?;
                break Ok(tab);
            }
            Ok(Ok(false)) => {}
            Ok(Err(())) => break Err("The person closed that browser tab.".into()),
            Err(error) => break Err(error),
        }
        if Instant::now() >= deadline {
            break Err(access::UNANSWERED.into());
        }
        pause(cx).await;
    };
    REQUESTS.remove(&request.id);
    let _ = this.update(cx, |_, cx| {
        cx.emit(super::BrowserEvent::Permission);
        cx.notify();
    });
    answer
}

/// Wait for tab `tab` to finish loading, while it allows the agent.
async fn wait_ready(
    this: &Weak,
    cx: &mut AsyncWindowContext,
    tab: &str,
    valid: &Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<(), String> {
    let deadline = Instant::now() + LOAD_WAIT;
    loop {
        if !valid() {
            return Err(INVALID.into());
        }
        let (allowed, loading) = read(this, cx, |browser| {
            if browser.agent_paused {
                return Err(PAUSED.to_string());
            }
            Ok(browser
                .tabs
                .find(tab)
                .map(|page| (page.agent_allowed, page.loading.loading)))
        })??
        .ok_or("The browser tab closed.")?;
        if !allowed {
            return Err(TOOK_OVER.into());
        }
        if !loading {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("The website is still loading. Try reading it again shortly.".into());
        }
        pause(cx).await;
    }
}

/// After the page moved: wait for it to load, while the tab stays the agent's in the same epoch.
async fn wait_moved(
    this: &Weak,
    cx: &mut AsyncWindowContext,
    tab: &str,
    epoch: u64,
    valid: &Guard,
) -> Result<(), String> {
    // The engine says the page started loading a moment after it was asked.
    pause(cx).await;
    let deadline = Instant::now() + LOAD_WAIT;
    loop {
        if !valid() {
            return Err(INVALID.into());
        }
        let state = read(this, cx, |browser| {
            browser
                .tabs
                .find(tab)
                .map(|page| (page.agent_allowed && page.control_epoch == epoch, page.loading.loading))
        })?;
        match state {
            Some((true, false)) => return Ok(()),
            Some((true, true)) => {}
            _ => return Err(TOOK_OVER.into()),
        }
        if Instant::now() >= deadline {
            return Err("The preview is still loading. Try reading it again shortly.".into());
        }
        pause(cx).await;
    }
}

/// Run `action` on tab `tab`'s page.
async fn run_action(
    this: &Weak,
    cx: &mut AsyncWindowContext,
    action: &str,
    tab: &str,
    args: &Value,
    valid: &Guard,
) -> Result<Value, String> {
    if !valid() {
        return Err(INVALID.into());
    }
    read(this, cx, |browser| {
        if browser.agent_paused {
            return Err(PAUSED.to_string());
        }
        if !browser.tabs.find(tab).is_some_and(|tab| tab.agent_allowed) {
            return Err(TOOK_OVER.to_string());
        }
        Ok(())
    })??;
    match action {
        "navigate" => {
            let url = nexus_browser::preview::browser_url(args["url"].as_str().ok_or("An address is required.")?)?
                .to_string();
            this.update_in(cx, |browser, window, cx| {
                if !valid() {
                    return Err(INVALID.to_string());
                }
                browser.agent_navigate(tab, &url, window, cx);
                Ok(())
            })
            .map_err(|_| "The person closed the browser.".to_string())??;
            Ok(Value::Null)
        }
        "back" | "forward" | "reload" => {
            let step = match action {
                "back" => -1,
                "forward" => 1,
                _ => 0,
            };
            let page = page_of(this, cx, tab)?;
            this.update(cx, |browser, cx| {
                if !valid() {
                    return Err(INVALID.to_string());
                }
                if let Some(page) = browser.tabs.find_mut(tab) {
                    page.loading.loading =
                        step == 0 || (step < 0 && page.loading.back) || (step > 0 && page.loading.forward);
                }
                page.update(cx, |page, cx| page.go(step, cx));
                Ok(())
            })
            .map_err(|_| "The person closed the browser.".to_string())??;
            Ok(Value::Null)
        }
        "read" => {
            let mut read = evaluate_guarded(this, cx, tab, "window.__nxPreview.read()", Some(valid)).await?;
            if !read.is_object() {
                return Err("The website returned an invalid page snapshot.".into());
            }
            let url = self::read(this, cx, |browser| browser.tabs.find(tab).map(|page| page.url.clone()))?;
            read["url"] = json!(url);
            Ok(read)
        }
        "click" | "type" => {
            let selector = args["selector"]
                .as_str()
                .filter(|value| value.len() <= 2000)
                .ok_or("A selector is required.")?;
            let expression = if action == "click" {
                format!("window.__nxPreview.click({})", json!(selector))
            } else {
                let text = args["text"]
                    .as_str()
                    .filter(|text| text.len() <= 16000)
                    .ok_or("Text is required.")?;
                format!("window.__nxPreview.type({},{})", json!(selector), json!(text))
            };
            evaluate_guarded(this, cx, tab, &expression, Some(valid)).await
        }
        "screenshot" => {
            let page = page_of(this, cx, tab)?;
            let answer = page.update(cx, |page, _| {
                if !valid() {
                    return Err(INVALID.to_string());
                }
                Ok(page.call("Page.captureScreenshot", json!({"format":"png"})))
            })?;
            let result = within(cx, super::preview::CAPTURE_WAIT, answer)
                .await
                .ok_or("The screenshot timed out.")?
                .map_err(|_| "The preview closed.".to_string())??;
            screenshot_answer(&result)
        }
        "info" => {
            let info = evaluate_guarded(this, cx, tab, "window.__nxPreview?.info() ?? {}", Some(valid)).await?;
            Ok(if info.is_object() { info } else { json!({}) })
        }
        _ => Err("Unknown browser action.".into()),
    }
}

fn page_of(this: &Weak, cx: &mut AsyncWindowContext, tab: &str) -> Result<Entity<crate::web::WebView>, String> {
    read(this, cx, |browser| browser.page(tab))?.ok_or_else(|| "This browser tab is not open yet.".into())
}

fn show_usage(
    this: &Weak,
    cx: &mut AsyncWindowContext,
    run: &str,
    valid: &Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<(), String> {
    if !valid() {
        return Err(INVALID.into());
    }
    this.update_in(cx, |browser, window, cx| {
        if browser.agent_paused {
            return Err(PAUSED.to_string());
        }
        let entity = cx.entity();
        let run = run.to_string();
        browser.pending_activity_runs.insert(run.clone(), valid.clone());
        let presentation = browser.activity_guard(Arc::new(|| true));
        let owner = window.window_handle();
        cx.defer(move |cx| {
            if presentation() {
                let valid = entity.update(cx, |browser, _| browser.pending_activity_runs.remove(&run));
                if let Some(valid) = valid {
                    crate::activity::using(&entity, &run, valid, owner, cx);
                }
            }
        });
        Ok(())
    })
    .map_err(|_| "The person closed the browser.".to_string())?
}

/// Evaluate `expression` with the page script on tab `tab`'s page, as the Nexus app does.
pub(super) async fn evaluate(
    this: &Weak,
    cx: &mut AsyncWindowContext,
    tab: &str,
    expression: &str,
) -> Result<Value, String> {
    evaluate_guarded(this, cx, tab, expression, None).await
}
async fn evaluate_guarded(
    this: &Weak,
    cx: &mut AsyncWindowContext,
    tab: &str,
    expression: &str,
    valid: Option<&Guard>,
) -> Result<Value, String> {
    let page = page_of(this, cx, tab)?;
    let answer = page.update(cx, |page, _| {
        if valid.is_some_and(|valid| !valid()) {
            return Err(INVALID.to_string());
        }
        Ok(page.call(
            "Runtime.evaluate",
            json!({"expression": script(expression), "returnByValue": true}),
        ))
    })?;
    let result = within(cx, SCRIPT_WAIT, answer)
        .await
        .ok_or("The preview did not answer.")?
        .map_err(|_| "The preview closed.".to_string())??;
    script_answer(&result)
}

impl Browser {
    /// The agent's `browser_open` on tab `tab`: the address it shows already stays, with its page state, as in the
    /// Nexus app; any other loads.
    fn agent_navigate(&mut self, tab: &str, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        let on_screen = self.tabs.current().is_some_and(|page| page.id == tab);
        let Some(page) = self.tabs.find_mut(tab) else { return };
        if page.url == url && self.pages.contains_key(tab) {
            return;
        }
        page.url = url.to_string();
        page.loading.loading = true;
        match self.pages.get(tab) {
            Some((view, _)) => view.read(cx).navigate(url),
            None => self.open_page(tab, url, window, cx),
        }
        if on_screen {
            self.address
                .update(cx, |input, cx| input.set_value(url.to_string(), window, cx));
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[gpui_kit::test]
    fn activity_pause_blocks_tools_until_resume_and_cancellation_ends_the_wait(cx: &mut TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;
        use std::sync::atomic::{AtomicBool, Ordering};
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::pages::Theme(crate::palette::Palette::for_choice(
                crate::palette::ThemeChoice::Dark,
                WindowAppearance::Dark,
            )));
        });
        let (browser, cx) = cx.add_window_view(|window, cx| {
            let mut browser = Browser::new(
                "activity-gate".into(),
                None,
                nexus_browser::profile::Profile::new("activity-test", "gate", false),
                None,
                window,
                cx,
            );
            browser.allow_agent();
            browser.pause_agent(true, cx);
            browser
        });
        let result = Rc::new(RefCell::new(None));
        let observed = result.clone();
        browser.update_in(cx, |browser, window, cx| {
            let task = browser.agent(
                "browser_tabs".into(),
                json!({}),
                "resume".into(),
                Arc::new(|| true),
                window,
                cx,
            );
            cx.spawn_in(window, async move |_, _| *observed.borrow_mut() = Some(task.await))
                .detach();
        });
        cx.run_until_parked();
        assert!(
            result.borrow().is_none(),
            "a paused tool cannot read or ask for a wider grant"
        );
        browser.update(cx, |browser, cx| browser.pause_agent(false, cx));
        cx.executor().advance_clock(Duration::from_millis(60));
        cx.run_until_parked();
        assert_eq!(
            result.borrow_mut().take(),
            Some(Ok(json!({"active": null, "tabs": []})))
        );

        let valid = Arc::new(AtomicBool::new(true));
        let flag = valid.clone();
        let observed = result.clone();
        browser.update_in(cx, |browser, window, cx| {
            browser.pause_agent(true, cx);
            let task = browser.agent(
                "browser_read".into(),
                json!({}),
                "cancel".into(),
                Arc::new(move || flag.load(Ordering::SeqCst)),
                window,
                cx,
            );
            cx.spawn_in(window, async move |_, _| *observed.borrow_mut() = Some(task.await))
                .detach();
        });
        cx.run_until_parked();
        assert!(result.borrow().is_none());
        valid.store(false, Ordering::SeqCst);
        cx.executor().advance_clock(Duration::from_millis(60));
        cx.run_until_parked();
        assert_eq!(result.borrow_mut().take(), Some(Err(INVALID.into())));
    }

    #[test]
    fn a_scripts_answer_is_its_value_or_its_error() {
        let ok = json!({"result":{"type":"string","value":"{\"value\":{\"title\":\"Two\"}}"}});
        assert_eq!(script_answer(&ok), Ok(json!({"title":"Two"})));
        let thrown = json!({"result":{"type":"string","value":"{\"error\":\"Error: Element not found\"}"}});
        assert_eq!(script_answer(&thrown), Err("Error: Element not found".into()));
        let broken = json!({"exceptionDetails":{"text":"Uncaught SyntaxError"}});
        assert_eq!(script_answer(&broken), Err("Uncaught SyntaxError".into()));
        let large = json!({"result":{"value":"x".repeat(ANSWER_BYTES + 1)}});
        assert_eq!(script_answer(&large), Err("The browser response is too large.".into()));
        assert!(script("window.__nxPreview.read()").contains("__nxPreview"));
    }

    #[test]
    fn a_screenshot_is_png_within_its_limit() {
        let shot = screenshot_answer(&json!({"data": STANDARD.encode(b"png")})).expect("a screenshot");
        assert_eq!(shot["mediaType"], "image/png");
        assert_eq!(shot["name"], "preview.png");
        assert!(screenshot_answer(&json!({})).is_err());
    }
}
