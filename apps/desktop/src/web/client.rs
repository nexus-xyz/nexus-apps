//! The app's side of the web host process (`host.rs`). The first page starts the host; every page after shares it.
//! Some time after the last page closes ([`idle_limit`]), the app closes the host's stdin and the host exits, so
//! Chromium's memory goes back to the system. A reader thread turns the host's events into each page's [`Shared`] state, which its
//! [`super::WebView`] reads on its next look. When the host dies, every page says so and offers to reload, which starts
//! a new host.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

use super::protocol::{Command, Event, HOST_ARG};
use super::{SharedRef, Startup};

/// How long the host stays up with no page open ([`idle_limit`]). Opening a page in a running host is fast; starting
/// one costs about a second, and its first page more. So a page closed and another opened within a few minutes (an
/// artifact, back to the chat, the next artifact) reuses it. An idle host holds about 200MB.
const IDLE_EXIT: Duration = Duration::from_secs(180);
/// Longer while the person opens pages: [`BUSY_OPENS`] in the last [`LATELY`], with the app in front.
const BUSY_IDLE_EXIT: Duration = Duration::from_secs(600);
const BUSY_OPENS: usize = 2;
const LATELY: Duration = Duration::from_secs(900);
/// Shorter with no window of the app in front: the person is elsewhere, and the memory goes back sooner.
const AWAY_IDLE_EXIT: Duration = Duration::from_secs(60);
/// How often the host's watcher looks whether it is idle.
const IDLE_LOOK: Duration = Duration::from_secs(5);
/// How long a host asked to exit gets before it is killed.
const EXIT_WAIT: Duration = Duration::from_secs(5);

const ENGINE_STOPPED: &str = "The web engine stopped.";

/// The running host, if any.
static HOST: Mutex<Option<Arc<Host>>> = Mutex::new(None);
static NEXT_ID: AtomicU32 = AtomicU32::new(1);
/// Whether a window of the app has the focus ([`focused`]).
static FOCUSED: AtomicBool = AtomicBool::new(true);
/// When the host last had a page or was warmed, and when pages opened lately.
static USE: LazyLock<Mutex<Use>> = LazyLock::new(|| {
    Mutex::new(Use {
        last: Instant::now(),
        opens: VecDeque::new(),
    })
});

struct Use {
    last: Instant,
    opens: VecDeque<Instant>,
}

impl Use {
    /// Pages opened in the last [`LATELY`].
    fn recent_opens(&mut self, now: Instant) -> usize {
        while self.opens.front().is_some_and(|at| now.duration_since(*at) > LATELY) {
            self.opens.pop_front();
        }
        self.opens.len()
    }
}

/// The host is in use now: its idle time starts again.
fn used(opened: bool) {
    let mut usage = USE.lock().expect("web host use");
    let now = Instant::now();
    usage.last = now;
    if opened {
        usage.recent_opens(now);
        usage.opens.push_back(now);
    }
}

/// How long the host stays up with no page: [`IDLE_EXIT`], longer while the person opens pages, shorter while the app
/// is in the background.
fn idle_limit(focused: bool, recent_opens: usize) -> Duration {
    if !focused {
        AWAY_IDLE_EXIT
    } else if recent_opens >= BUSY_OPENS {
        BUSY_IDLE_EXIT
    } else {
        IDLE_EXIT
    }
}

/// A window of the app took or lost the focus.
pub(super) fn focused(focused: bool) {
    FOCUSED.store(focused, Ordering::Relaxed);
}

pub(super) type Pages = Arc<Mutex<HashMap<u32, SharedRef>>>;
type Input = Arc<Mutex<Option<ChildStdin>>>;

struct Host {
    /// Its stdin; `None` once it has exited or was asked to.
    input: Input,
    child: Mutex<Option<Child>>,
    pages: Pages,
    /// It has the GPU frame socket (`frames/linux.rs`).
    gpu: bool,
    /// The page opened ahead ([`prepare`]), if one waits. It does not keep the host from exiting when idle.
    ready: Mutex<Option<Ready>>,
}

/// A blank, hidden page with a startup script and binding in place (`super::ready`).
struct Ready {
    id: u32,
    shared: SharedRef,
    startup: Startup,
    until: Instant,
}

impl Ready {
    fn usable(&self, now: Instant) -> bool {
        let shared = self.shared.lock().expect("web view state");
        now < self.until && shared.stopped.is_none() && !shared.closed
    }

    fn refresh(&mut self, startup: Startup, now: Instant) -> bool {
        if self.startup != startup || !self.usable(now) {
            return false;
        }
        self.until = now + super::ready::KEEP_FOR;
        true
    }
}

impl Host {
    /// The running host, or a new one.
    fn get() -> Result<Arc<Host>, String> {
        let mut slot = HOST.lock().expect("web host");
        if let Some(host) = slot.as_ref()
            && host.alive()
        {
            return Ok(host.clone());
        }
        let host = Arc::new(Host::start()?);
        *slot = Some(host.clone());
        used(false);
        Host::exit_when_idle(&host);
        Ok(host)
    }

    fn start() -> Result<Host, String> {
        crate::measure::page_mark("host-spawn");
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut process = Process::new(&exe);
        process
            .arg(HOST_ARG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(target_os = "linux")]
        let gpu = gpu_socket(&mut process);
        let mut child = process.spawn().map_err(|error| format!("{}: {error}", exe.display()))?;
        let input: Input = Arc::new(Mutex::new(child.stdin.take()));
        let output = child.stdout.take().expect("piped stdout");
        let pages: Pages = Arc::default();
        {
            let (input, pages) = (input.clone(), pages.clone());
            std::thread::Builder::new()
                .name("web-host-events".into())
                .spawn(move || read_events(output, &pages, &input))
                .map_err(|error| error.to_string())?;
        }
        // The app closes its copy of the host's end here: the host has its own now.
        #[cfg(target_os = "linux")]
        let gpu = gpu.and_then(|(app_end, _host_end)| {
            let pages = pages.clone();
            std::thread::Builder::new()
                .name("web-host-gpu-frames".into())
                .spawn(move || super::frames::read_frames(app_end, pages))
                .ok()
        });
        Ok(Host {
            input,
            child: Mutex::new(Some(child)),
            pages,
            #[cfg(target_os = "linux")]
            gpu: gpu.is_some(),
            #[cfg(not(target_os = "linux"))]
            gpu: false,
            ready: Mutex::new(None),
        })
    }

    fn alive(&self) -> bool {
        self.input.lock().expect("web host input").is_some()
    }

    fn send(&self, command: &Command) {
        send(&self.input, command);
    }

    fn has_attached_pages(&self) -> bool {
        let ready = self.ready.lock().expect("ready page").as_ref().map(|ready| ready.id);
        self.pages
            .lock()
            .expect("web host pages")
            .keys()
            .any(|id| Some(*id) != ready)
    }

    /// Retire stale preparation without touching adopted pages. The watcher also releases a preparation-only host.
    fn expire_ready(&self, now: Instant) -> bool {
        let mut ready = self.ready.lock().expect("ready page");
        if ready.as_ref().is_none_or(|ready| ready.usable(now)) {
            return false;
        }
        if let Some(ready) = ready.take() {
            self.close_ready(ready);
        }
        true
    }

    fn close_ready(&self, ready: Ready) {
        self.send(&Command::Close { id: ready.id });
        self.pages.lock().expect("web host pages").remove(&ready.id);
    }

    /// Watch the host while it is the running one: once it has had no page for its [`idle_limit`], it exits.
    fn exit_when_idle(host: &Arc<Host>) {
        let host = Arc::downgrade(host);
        let _ = std::thread::Builder::new().name("web-host-idle".into()).spawn(move || {
            loop {
                std::thread::sleep(IDLE_LOOK);
                let mut slot = HOST.lock().expect("web host");
                let current = match (slot.as_ref(), Weak::upgrade(&host)) {
                    (Some(current), Some(host)) if Arc::ptr_eq(current, &host) => host,
                    _ => return,
                };
                let expired = current.expire_ready(Instant::now());
                if current.has_attached_pages() {
                    used(false);
                    continue;
                }
                let idle = {
                    let mut usage = USE.lock().expect("web host use");
                    let now = Instant::now();
                    let limit = idle_limit(FOCUSED.load(Ordering::Relaxed), usage.recent_opens(now));
                    now.duration_since(usage.last) >= limit
                };
                if expired || idle {
                    drop(current);
                    // The last reference goes with the slot: `Drop` closes its stdin.
                    *slot = None;
                    return;
                }
            }
        });
    }
}

/// Give the host to be started one end of a GPU frame socket, when GPU frames are wanted. Returns both ends.
#[cfg(target_os = "linux")]
fn gpu_socket(process: &mut Process) -> Option<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    if !super::frames::wanted() {
        return None;
    }
    let (app_end, host_end) = super::frames::pair()
        .inspect_err(|error| eprintln!("nexus-desktop-gpui: GPU frame socket: {error}"))
        .ok()?;
    let fd = host_end.as_raw_fd();
    process.env(super::frames::FD_ENV, fd.to_string());
    if let Some(uuid) = super::frames::ring_gpu() {
        process.env(super::frames::UUID_ENV, uuid);
    }
    // SAFETY: runs in the child between fork and exec, and only clears close-on-exec on the host's end, so the host
    // inherits it and no other process the app starts does.
    unsafe {
        process.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Some((app_end, host_end))
}

impl Drop for Host {
    fn drop(&mut self) {
        // A closed stdin is the host's signal to exit; one that hangs on is killed.
        self.input.lock().expect("web host input").take();
        let Some(mut child) = self.child.lock().expect("web host process").take() else {
            return;
        };
        let _ = std::thread::Builder::new().name("web-host-exit".into()).spawn(move || {
            let deadline = Instant::now() + EXIT_WAIT;
            while Instant::now() < deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        });
    }
}

fn send(input: &Input, command: &Command) {
    let Ok(line) = serde_json::to_string(command) else {
        return;
    };
    let mut input = input.lock().expect("web host input");
    if let Some(pipe) = input.as_mut()
        && (writeln!(pipe, "{line}").is_err() || pipe.flush().is_err())
    {
        // The host is gone; the reader thread tells the pages.
        *input = None;
    }
}

/// Start the host before a page asks for it, so the page skips the engine's start. With no page, it exits after its
/// [`idle_limit`], as when the last page closes.
pub(super) fn warm() {
    match Host::get() {
        Ok(_) => used(false),
        Err(error) => eprintln!("nexus-desktop-gpui: web host: {error}"),
    }
}

/// Open a blank, hidden page in the host, starting it if it is not running, and send it `messages` (a startup
/// script's), for the next page with that script to take ([`take_ready`]). Intent refreshes its lease, or replaces a
/// stale page. Nothing when pages get GPU frames: a page that paints nothing for a while there turns them off (`frames::watch`).
pub(super) fn prepare(startup: Startup, messages: Vec<String>, placed: super::ready::Placed) {
    let host = match Host::get() {
        Ok(host) => host,
        Err(error) => return eprintln!("nexus-desktop-gpui: web host: {error}"),
    };
    used(false);
    if host.gpu && !gpu_off() {
        return;
    }
    let mut ready = host.ready.lock().expect("ready page");
    let now = Instant::now();
    if ready.as_mut().is_some_and(|ready| ready.refresh(startup, now)) {
        return;
    }
    if let Some(old) = ready.take() {
        host.close_ready(old);
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    // Parked: what it reports before a view takes it is dropped.
    let shared: SharedRef = Arc::new(Mutex::new(super::Shared {
        size: placed.size,
        scale: placed.scale,
        parked: true,
        ..Default::default()
    }));
    host.pages.lock().expect("web host pages").insert(id, shared.clone());
    host.send(&Command::Open {
        id,
        url: "about:blank".into(),
        width: placed.size.0,
        height: placed.size.1,
        scale: placed.scale,
        frame_rate: placed.frame_rate,
        gpu: false,
        paced: super::paced_forced(),
        browser: None,
        profile: None,
        document: false,
        handoff: None,
        clear: false,
    });
    host.send(&Command::Hidden { id, hidden: true });
    for message in messages {
        host.send(&Command::Cdp { id, message });
    }
    *ready = Some(Ready {
        id,
        shared,
        startup,
        until: now + super::ready::KEEP_FOR,
    });
}

/// The page [`prepare`] opened for the startup script `startup`, if one waits in the running host, with its state.
pub(super) fn take_ready(startup: Startup) -> Option<(HostPage, SharedRef)> {
    let host = HOST.lock().expect("web host").clone().filter(|host| host.alive())?;
    take_ready_from(host, startup)
}

fn take_ready_from(host: Arc<Host>, startup: Startup) -> Option<(HostPage, SharedRef)> {
    let ready = {
        let mut slot = host.ready.lock().expect("ready page");
        if !slot.as_ref()?.usable(Instant::now()) {
            if let Some(old) = slot.take() {
                host.close_ready(old);
            }
            return None;
        }
        if slot.as_ref()?.startup != startup {
            return None;
        }
        slot.take()?
    };
    {
        // A frame of the blank page it waited on is not the page's.
        let mut shared = ready.shared.lock().expect("web view state");
        shared.frame = None;
        shared.dirty = false;
    }
    used(true);
    let paced = super::paced_forced();
    Some((
        HostPage {
            id: ready.id,
            host,
            paced,
        },
        ready.shared,
    ))
}

/// One page in the host.
pub(super) struct HostPage {
    id: u32,
    host: Arc<Host>,
    /// The page paints at the app's pace (`Command::BeginFrame`).
    paced: bool,
}

impl HostPage {
    pub(super) fn open(
        url: &str,
        size: (i32, i32),
        scale: f32,
        frame_rate: i32,
        paced: bool,
        shared: SharedRef,
        browser: Option<String>,
        profile: Option<super::protocol::WebProfile>,
        document: bool,
        handoff: Option<String>,
        clear: bool,
    ) -> Result<HostPage, String> {
        let host = Host::get()?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let gpu = host.gpu && !gpu_off();
        // Pacing is on by default only for GPU frames (`super::paced`). The page gets them only when the host has its
        // frame socket, which only the host knows. `NEXUS_WEB_PACED=1` still paces any page.
        let paced = paced && (gpu || super::paced_forced());
        #[cfg(target_os = "linux")]
        if gpu {
            super::frames::watch(id, Arc::downgrade(&shared));
        }
        host.pages.lock().expect("web host pages").insert(id, shared);
        used(true);
        host.send(&Command::Open {
            id,
            url: url.to_string(),
            width: size.0,
            height: size.1,
            scale,
            frame_rate,
            gpu,
            paced,
            browser,
            profile,
            document,
            handoff,
            clear,
        });
        Ok(HostPage { id, host, paced })
    }

    /// A popup the host opened (`Event::PopupOpened`): its state is in the pages already.
    pub(super) fn adopt(id: u32) -> Result<HostPage, String> {
        let host = HOST
            .lock()
            .expect("web host")
            .clone()
            .filter(|host| host.alive() && host.pages.lock().expect("web host pages").contains_key(&id))
            .ok_or_else(|| ENGINE_STOPPED.to_string())?;
        // A popup paints on its own timer (`handlers.rs`).
        Ok(HostPage { id, host, paced: false })
    }

    /// A popup queued before its view detached has no UI owner to adopt it.
    pub(super) fn close_unattached(&self, id: u32) {
        self.host.send(&Command::Close { id });
        self.host.pages.lock().expect("web host pages").remove(&id);
    }

    pub(super) fn id(&self) -> u32 {
        self.id
    }

    /// Whether the page paints at the app's pace.
    pub(super) fn paced(&self) -> bool {
        self.paced
    }

    pub(super) fn send(&self, command: Command) {
        self.host.send(&command);
    }
}

/// A local host registry exercising normal move/drop ownership without a process or global host override.
#[cfg(test)]
pub(super) fn fixture_page(id: u32, shared: SharedRef) -> (HostPage, Pages) {
    let (host, pages) = fixture_host(id, shared);
    (HostPage { id, host, paced: false }, pages)
}

#[cfg(test)]
fn fixture_host(id: u32, shared: SharedRef) -> (Arc<Host>, Pages) {
    let pages = Arc::new(Mutex::new(HashMap::from([(id, shared)])));
    let host = Arc::new(Host {
        input: Arc::new(Mutex::new(None)),
        child: Mutex::new(None),
        pages: pages.clone(),
        gpu: false,
        ready: Mutex::new(None),
    });
    (host, pages)
}

impl Drop for HostPage {
    fn drop(&mut self) {
        self.host.send(&Command::Close { id: self.id });
        self.host.pages.lock().expect("web host pages").remove(&self.id);
        used(false);
    }
}

#[cfg(target_os = "linux")]
fn gpu_off() -> bool {
    super::frames::off()
}

#[cfg(not(target_os = "linux"))]
fn gpu_off() -> bool {
    true
}

/// The host's events become its pages' state, until it exits.
fn read_events(output: impl std::io::Read, pages: &Pages, input: &Input) {
    let page = |id: u32| pages.lock().expect("web host pages").get(&id).cloned();
    // Each page's frame file, mapped once per size (`cpu.rs`).
    let mut views = super::cpu::Views::default();
    let mut stopped = ENGINE_STOPPED;
    for line in BufReader::new(output).lines() {
        let Ok(line) = line else { break };
        let event = match serde_json::from_str::<Event>(&line) {
            Ok(event) => event,
            Err(error) => {
                eprintln!("nexus-desktop-gpui: web host said something unknown ({error}): {line}");
                continue;
            }
        };
        match event {
            Event::Ready { ok } => {
                crate::measure::page_mark("host-ready");
                if !ok {
                    stopped = "Pages can't show here: the web engine did not start.";
                    break;
                }
            }
            Event::Failed { id } => {
                if let Some(shared) = page(id) {
                    let mut shared = shared.lock().expect("web view state");
                    shared.stopped = Some("The page could not open.".into());
                    shared.changed();
                }
            }
            Event::Frame {
                id,
                path,
                width,
                height,
                seq,
                dirty,
            } => {
                // Closed pages' files go: their mappings hold a frame's memory each.
                {
                    let open = pages.lock().expect("web host pages");
                    views.retain(|id| open.contains_key(&id));
                }
                let Some(shared) = page(id) else {
                    send(input, &Command::Ack { id });
                    continue;
                };
                // The last frame, if the view has not taken it: this one's changes go into it. The copy runs outside the
                // lock, so the view never waits for it.
                let mut pending = shared.lock().expect("web view state").frame.take();
                let frame = views.read(id, &path, (width, height), seq, &dirty, &mut pending);
                // Read: the host may write the next frame.
                send(input, &Command::Ack { id });
                match frame {
                    Ok(frame) => {
                        let mut shared = shared.lock().expect("web view state");
                        shared.frame = Some(frame);
                        shared.changed();
                    }
                    Err(error) => {
                        eprintln!("nexus-desktop-gpui: frame of page {id} at {path}: {error}");
                        // The view still gets the frame it had not taken.
                        let mut shared = shared.lock().expect("web view state");
                        if shared.frame.is_none() {
                            shared.frame = pending;
                        }
                    }
                }
            }
            Event::Title { id, title } => {
                if let Some(shared) = page(id) {
                    let mut shared = shared.lock().expect("web view state");
                    shared.title = Some(title).filter(|title| !title.is_empty());
                    shared.changed();
                }
            }
            Event::Cursor { id, cursor } => {
                if let Some(shared) = page(id) {
                    let mut shared = shared.lock().expect("web view state");
                    shared.cursor = cursor;
                    shared.changed();
                }
            }
            Event::Popup { id, url } => {
                if let Some(shared) = page(id) {
                    let mut shared = shared.lock().expect("web view state");
                    if !shared.parked {
                        shared.popups.push(url);
                        shared.changed();
                    }
                }
            }
            Event::Crashed { id } => {
                if let Some(shared) = page(id) {
                    let mut shared = shared.lock().expect("web view state");
                    shared.stopped = Some("This page stopped.".into());
                    shared.changed();
                }
            }
            Event::Address { id, url } => {
                if let Some(shared) = page(id) {
                    let mut shared = shared.lock().expect("web view state");
                    shared.address = Some(url);
                    shared.changed();
                }
            }
            Event::Loading {
                id,
                loading,
                back,
                forward,
            } => {
                if let Some(shared) = page(id) {
                    let mut shared = shared.lock().expect("web view state");
                    shared.loading = Some(super::Loading { loading, back, forward });
                    #[cfg(target_os = "linux")]
                    if !loading {
                        shared.loaded = Some(Instant::now());
                    }
                    shared.changed();
                }
            }
            Event::LoadFailed { id, status } => {
                if let Some(shared) = page(id) {
                    let mut shared = shared.lock().expect("web view state");
                    shared.load_failure = Some(status);
                    shared.changed();
                }
            }
            Event::PopupOpened {
                id,
                popup,
                url,
                user_gesture,
            } => {
                let Some(parent) = page(id) else { continue };
                let mut parent = parent.lock().expect("web view state");
                if parent.parked {
                    send(input, &Command::Close { id: popup });
                    continue;
                }
                // Its own state now, so its first frame and title are kept until the app shows it.
                let shared: SharedRef = Arc::new(Mutex::new(super::Shared {
                    size: (super::POPUP_SIZE.0 as i32, super::POPUP_SIZE.1 as i32),
                    scale: parent.scale,
                    address: Some(url),
                    ..Default::default()
                }));
                pages.lock().expect("web host pages").insert(popup, shared.clone());
                parent.opened.push(super::Opened {
                    id: popup,
                    shared,
                    user_gesture,
                });
                parent.changed();
            }
            Event::Closed { id } => {
                if let Some(shared) = page(id) {
                    let mut shared = shared.lock().expect("web view state");
                    shared.closed = true;
                    shared.changed();
                }
            }
            Event::Cdp { id, message } => {
                if let Some(shared) = page(id) {
                    let mut shared = shared.lock().expect("web view state");
                    if !shared.parked {
                        shared.cdp.push(message);
                        shared.changed();
                    }
                }
            }
            Event::Handoff { id, url } => {
                if let Some(shared) = page(id) {
                    let mut shared = shared.lock().expect("web view state");
                    shared.handoffs.push(url);
                    shared.changed();
                }
            }
        }
    }
    // The host exited, or could not start: no page of it draws again.
    input.lock().expect("web host input").take();
    for shared in pages.lock().expect("web host pages").values() {
        let mut shared = shared.lock().expect("web view state");
        if shared.stopped.is_none() {
            shared.stopped = Some(stopped.into());
        }
        shared.changed();
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn parked_pages_discard_cdp_and_popups_but_keep_latest_navigation() {
        let shared: SharedRef = Default::default();
        shared.lock().unwrap().parked = true;
        let (page, pages) = fixture_page(91, shared.clone());
        let events = [
            Event::Cdp {
                id: 91,
                message: "old reply".into(),
            },
            Event::Popup {
                id: 91,
                url: "https://fixture.invalid/popup".into(),
            },
            Event::PopupOpened {
                id: 91,
                popup: 92,
                url: "https://fixture.invalid/sign-in".into(),
                user_gesture: true,
            },
            Event::Address {
                id: 91,
                url: "https://fixture.invalid/next".into(),
            },
            Event::Loading {
                id: 91,
                loading: false,
                back: true,
                forward: false,
            },
        ];
        let lines = events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        read_events(std::io::Cursor::new(lines), &pages, &Arc::new(Mutex::new(None)));
        let state = shared.lock().unwrap();
        assert!(state.cdp.is_empty() && state.popups.is_empty() && state.opened.is_empty());
        assert_eq!(state.address.as_deref(), Some("https://fixture.invalid/next"));
        assert!(state.loading.unwrap().back);
        assert!(!pages.lock().unwrap().contains_key(&92));
        drop(state);
        drop(page);
        assert!(pages.lock().unwrap().is_empty());
    }

    #[test]
    fn prepared_page_keeps_its_binding_and_moves_once_without_its_blank_frame() {
        let startup = Startup {
            source: "window.ready = true",
            binding: Some("nexus"),
        };
        let shared: SharedRef = Arc::new(Mutex::new(super::super::Shared {
            frame: Some(super::super::Frame {
                pixels: vec![255; 4],
                width: 1,
                height: 1,
                ..Default::default()
            }),
            parked: true,
            dirty: true,
            ..Default::default()
        }));
        let (host, pages) = fixture_host(93, shared.clone());
        *host.ready.lock().unwrap() = Some(Ready {
            id: 93,
            shared: shared.clone(),
            startup,
            until: Instant::now() + super::super::ready::KEEP_FOR,
        });
        assert!(!host.has_attached_pages(), "a prepared page must not prevent idle exit");
        assert!(
            take_ready_from(
                host.clone(),
                Startup {
                    binding: Some("other"),
                    ..startup
                }
            )
            .is_none()
        );
        assert!(
            host.ready.lock().unwrap().is_some(),
            "a different binding cannot consume the page"
        );
        let (page, taken) = take_ready_from(host.clone(), startup).unwrap();
        assert!(Arc::ptr_eq(&shared, &taken));
        assert!(
            taken.lock().unwrap().frame.is_none(),
            "never reuse the blank document's pixels"
        );
        assert!(!taken.lock().unwrap().dirty);
        assert!(host.has_attached_pages(), "the adopted page keeps its host alive");
        assert!(take_ready_from(host.clone(), startup).is_none(), "move-only ownership");
        drop(page);
        assert!(
            pages.lock().unwrap().is_empty(),
            "dropping its view closes the same page"
        );
        assert!(!host.has_attached_pages());
    }

    fn fixture_ready(host: &Host, id: u32, until: Instant) -> Startup {
        let startup = Startup {
            source: "window.ready = true",
            binding: Some("nexus"),
        };
        let shared: SharedRef = Default::default();
        host.pages.lock().unwrap().insert(id, shared.clone());
        *host.ready.lock().unwrap() = Some(Ready {
            id,
            shared,
            startup,
            until,
        });
        startup
    }

    #[test]
    fn expired_or_stopped_preparation_falls_back_then_can_prepare_again() {
        let (host, pages) = fixture_host(94, Default::default());
        pages.lock().unwrap().clear();
        let now = Instant::now();
        let startup = fixture_ready(&host, 94, now);
        assert!(take_ready_from(host.clone(), startup).is_none());
        assert!(host.ready.lock().unwrap().is_none());
        assert!(
            pages.lock().unwrap().is_empty(),
            "expired handle is closed, not adopted"
        );
        fixture_ready(&host, 95, now + super::super::ready::KEEP_FOR);
        host.ready
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .shared
            .lock()
            .unwrap()
            .stopped = Some("stopped".into());
        assert!(take_ready_from(host.clone(), startup).is_none());
        assert!(pages.lock().unwrap().is_empty());
        fixture_ready(&host, 96, now + super::super::ready::KEEP_FOR);
        let (page, _) = take_ready_from(host.clone(), startup).unwrap();
        assert_eq!(page.id, 96, "a new request can prepare after either failure");
    }

    #[test]
    fn intent_refreshes_a_live_lease_and_expiry_preserves_active_pages() {
        let (host, pages) = fixture_host(97, Default::default());
        let now = Instant::now();
        let startup = fixture_ready(&host, 98, now + Duration::from_secs(1));
        assert!(host.ready.lock().unwrap().as_mut().unwrap().refresh(startup, now));
        assert!(!host.expire_ready(now + Duration::from_secs(2)));
        assert!(host.expire_ready(now + super::super::ready::KEEP_FOR));
        assert_eq!(pages.lock().unwrap().keys().copied().collect::<Vec<_>>(), vec![97]);
        assert!(
            host.has_attached_pages(),
            "lease expiry must not release an active page's host"
        );
        assert!(!host.expire_ready(now + super::super::ready::KEEP_FOR));
        pages.lock().unwrap().clear();
        fixture_ready(&host, 99, now);
        assert!(host.expire_ready(now));
        assert!(
            !host.has_attached_pages(),
            "a preparation-only host can exit at lease expiry"
        );
    }

    /// The host waits longer while the person opens pages, and less while the app is in the background.
    #[test]
    fn the_idle_limit_follows_what_the_person_does() {
        assert_eq!(idle_limit(true, 0), IDLE_EXIT);
        assert_eq!(idle_limit(true, 1), IDLE_EXIT);
        assert_eq!(idle_limit(true, BUSY_OPENS), BUSY_IDLE_EXIT);
        assert_eq!(idle_limit(false, BUSY_OPENS), AWAY_IDLE_EXIT);
    }

    #[test]
    fn only_recent_opens_count() {
        let now = Instant::now();
        let long_ago = now.checked_sub(LATELY + Duration::from_secs(1));
        let mut usage = Use {
            last: now,
            opens: long_ago.into_iter().chain([now]).collect(),
        };
        assert_eq!(usage.recent_opens(now), 1);
    }
}
