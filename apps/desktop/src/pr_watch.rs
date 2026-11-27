//! The pull request watch (`nexus_pull_requests::watch`), run by this build while it owns the Code sessions store
//! (`owner.rs`). A failed check, a review that asks for
//! changes and a new conflict each show a notification (`notify.rs`). With a session's switch on, the agent is told
//! what to fix.
//!
//! The runner starts it with its workbench ([`start`]) and stops it when it goes ([`stop`]). Each watch then ends at
//! its next read, since the workbench is gone. The agent's own `watch_pull_request` tools reach it over the app's
//! socket (`app_socket.rs`).
//!
//! While the background service runs the sessions (`service.rs`), the runner starts it over the service instead
//! ([`start_over_service`]): it reads the sessions from the store, as the windows do, and settles, wakes and tells a
//! session through the service's commands.
//!
//! It watches the sessions of the person's own servers too (`machines/pull_sessions.rs`): this computer's `gh` reads
//! their pull requests by the repository's name, and the server's host takes the watch's changes to them.
//!
//! The windows show what it last read ([`known`]), as the Nexus app's windows keep `wb:pull`: the Workbench's pull
//! request section (`workspace/pull_card.rs`) and the sidebar's dot ([`needs_you`]). Its actions are the Nexus app's
//! `wb_pull_watch*` commands: [`read`], [`send`], [`set_auto`] and [`agent_off`].

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use nexus_desktop_core::notify::Notifier;
use nexus_desktop_core::settings::Settings;
use nexus_pull_requests::watch::{Changed, PullWatch, Shell, Watches, on_workbench};
use nexus_workbench::Workbench;
use nexus_workbench::event::ThreadSummary;
use serde_json::json;

use crate::notify::GpuiNotifier;

/// How long after the runner starts the first look at the sessions waits: the workbench ends the sessions the last run
/// left open first (the Nexus app's 15 seconds).
const FIRST_LOOK_AFTER: Duration = Duration::from_secs(15);

/// The background service that runs the sessions: `nexus_pull_requests`'s.
pub(crate) use nexus_pull_requests::over_service::OverService as Service;

struct Known {
    watch: PullWatch,
    summary: Option<Arc<nexus_pull_requests::summary::Summary>>,
}

struct Held {
    watches: Watches,
    /// Each session's watch as the windows last got it, and the windows to tell when one changes.
    known: Mutex<HashMap<String, Known>>,
    /// The saved links ([`saw_links`]) the watch was asked to read, as `session\nurl`: each once a run.
    asked: Mutex<HashSet<String>>,
    listeners: Mutex<Vec<UnboundedSender<()>>>,
    workbench: RwLock<Option<Workbench>>,
    service: RwLock<Option<Service>>,
    runtime: RwLock<Option<tokio::runtime::Handle>>,
}

fn held() -> &'static Held {
    static HELD: OnceLock<Held> = OnceLock::new();
    HELD.get_or_init(|| Held {
        watches: Watches::default(),
        known: Mutex::default(),
        asked: Mutex::default(),
        listeners: Mutex::default(),
        workbench: RwLock::new(None),
        service: RwLock::new(None),
        runtime: RwLock::new(None),
    })
}

fn service() -> Option<Service> {
    held().service.read().ok()?.clone()
}

impl GpuiShell {
    /// Whether the watch runs: this build owns the store, or its background service does.
    pub fn running() -> bool {
        held().workbench.read().is_ok_and(|workbench| workbench.is_some()) || service().is_some()
    }
}

/// The watch's [`Shell`] over this build: the runner's workbench and runtime, the Nexus app's `settings.json`, and
/// [`GpuiNotifier`].
#[derive(Clone, Copy, Default)]
pub struct GpuiShell;

impl Notifier for GpuiShell {
    fn looking_at(&self, thread_id: &str) -> bool {
        GpuiNotifier.looking_at(thread_id)
    }

    fn show(&self, machine: Option<&str>, thread_id: &str, title: &str, body: &str) {
        // A click opens the session's pull request page.
        crate::notify::show_pull(machine, thread_id, title, body);
    }
}

impl Shell for GpuiShell {
    fn watches(&self) -> Option<&Watches> {
        Some(&held().watches)
    }

    /// The pull request page reads again when the watch read the pull request (`pull_page.rs`).
    /// The windows keep it ([`known`]), and draw again.
    fn emit(&self, state: &PullWatch) {
        crate::pull_page::watched(&state.thread_id, state.read_at);
        put(state.clone());
    }

    fn settings(&self) -> Settings {
        crate::desktop_settings::load()
    }

    /// The sessions' automatic wakes, written only while this build owns the store (`desktop_settings::update`).
    fn update_settings(&self, change: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
        crate::desktop_settings::update(|settings| {
            change(settings);
            Ok(())
        })
        .map(|(settings, ())| settings)
    }

    fn workbench(&self) -> Option<Workbench> {
        held().workbench.read().ok()?.clone()
    }

    /// This computer's sessions, and the SSH servers' (`machines/pull_sessions.rs`).
    fn threads(&self) -> Option<Vec<ThreadSummary>> {
        let mut threads = match service() {
            Some(service) => service.threads().ok(),
            None => self.workbench()?.threads().ok(),
        }?;
        threads.extend(crate::machines::get().ssh_sessions());
        Some(threads)
    }

    fn folder(&self, thread_id: &str) -> Result<std::path::PathBuf, String> {
        match service() {
            Some(service) => service.folder(thread_id),
            None => self.workbench().ok_or("The workbench is not open.")?.folder(thread_id),
        }
    }

    /// A server session's repository: the watch reads its pull request by name, with this computer's `gh`.
    fn repository(&self, thread_id: &str) -> Option<nexus_pull_requests::remote::Repository> {
        crate::machines::get()
            .ssh_session(thread_id)
            .map(|(_, _, repository)| repository)
    }

    fn settle_merged(&self, thread_id: &str, number: u64, archive: bool) -> Changed {
        if let Some(machine) = server_of(thread_id) {
            let args = json!({ "threadId": thread_id, "number": number, "archive": archive });
            return crate::machines::get().pull_change(&machine, "wb_settle_merged", args);
        }
        match service() {
            Some(service) => service.settle_merged(thread_id, number, archive),
            None => on_workbench::settle_merged(self.workbench(), thread_id, number, archive),
        }
    }

    fn wake(&self, thread_id: &str) -> Changed {
        let machines = crate::machines::get();
        if let Some((machine, thread, _)) = machines.ssh_session(thread_id) {
            return machines.pull_wake(&machine, &thread);
        }
        match service() {
            Some(service) => service.wake(thread_id),
            None => on_workbench::wake(self.workbench(), thread_id),
        }
    }

    fn check_failed(&self, thread_id: &str, name: &str, url: Option<&str>) -> Changed {
        if let Some(machine) = server_of(thread_id) {
            let args = json!({ "threadId": thread_id, "name": name, "url": url });
            return crate::machines::get().pull_change(&machine, "wb_check_failed", args);
        }
        match service() {
            Some(service) => service.check_failed(thread_id, name, url),
            None => on_workbench::check_failed(self.workbench(), thread_id, name, url),
        }
    }

    fn link_pull_request(&self, thread_id: &str, url: &str) -> Changed {
        if let Some(machine) = server_of(thread_id) {
            let args = json!({ "threadId": thread_id, "url": url });
            return crate::machines::get().pull_change(&machine, "wb_link_pull_request", args);
        }
        match service() {
            Some(service) => service.link_pull_request(thread_id, url),
            None => on_workbench::link_pull_request(self.workbench(), thread_id, url),
        }
    }

    fn queue(&self, thread_id: &str, text: &str) -> Changed {
        if let Some(machine) = server_of(thread_id) {
            let args = json!({ "threadId": thread_id, "text": text, "images": [] });
            return crate::machines::get().pull_change(&machine, "wb_send", args);
        }
        match service() {
            Some(service) => service.queue(thread_id, text),
            None => on_workbench::queue(self.workbench(), thread_id, text),
        }
    }

    fn agent_path(&self) -> String {
        crate::code_run::agent_env().remove("PATH").unwrap_or_default()
    }

    fn spawn(&self, task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) {
        if let Some(runtime) = held().runtime.read().ok().and_then(|runtime| runtime.clone()) {
            runtime.spawn(task);
        }
    }
}

/// The SSH server that holds session `thread_id`; None for this computer's sessions.
fn server_of(thread_id: &str) -> Option<String> {
    crate::machines::get()
        .ssh_session(thread_id)
        .map(|(machine, _, _)| machine)
}

/// The runner took the store: watch its sessions, and look at the likely ones after [`FIRST_LOOK_AFTER`].
pub fn start(workbench: Workbench, runtime: &tokio::runtime::Handle) {
    if let Ok(mut held) = held().workbench.write() {
        *held = Some(workbench);
    }
    look_at_launch(runtime);
}

/// The runner reached the background service that holds the store: watch its sessions through it.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn start_over_service(service: Service, runtime: &tokio::runtime::Handle) {
    if let Ok(mut held) = held().service.write() {
        *held = Some(service);
    }
    look_at_launch(runtime);
}

fn look_at_launch(runtime: &tokio::runtime::Handle) {
    if let Ok(mut held) = held().runtime.write() {
        *held = Some(runtime.clone());
    }
    runtime.spawn(async {
        tokio::time::sleep(FIRST_LOOK_AFTER).await;
        nexus_pull_requests::watch::watch_at_launch(GpuiShell).await;
    });
}

/// The runner let the store, or the service, go: each watch ends at its next read.
pub fn stop() {
    if let Ok(mut held) = held().workbench.write() {
        *held = None;
    }
    if let Ok(mut held) = held().service.write() {
        *held = None;
    }
}

/// A turn of a session on this computer ended: the agent may have pushed, or opened a pull request.
pub fn turn_ended(thread_id: &str) {
    if GpuiShell::running() {
        nexus_pull_requests::watch::turn_ended(&GpuiShell, thread_id);
    }
}

/// A session's summary changed: when it saved a pull request the watch has not read yet (its agent's `gh pr create`
/// printed one, nexus-workbench's `created_pulls`), the watch reads it now, not at the turn's end. Each link is asked
/// for once a run, so a read that keeps failing does not run `gh` at every change of the session.
pub fn saw_links(thread: &ThreadSummary) {
    let pulls = thread.pulls();
    if pulls.is_empty() || !GpuiShell::running() {
        return;
    }
    let read: Vec<String> = known(&thread.id)
        .map(|watch| watch.pulls.into_iter().map(|status| status.url).collect())
        .unwrap_or_default();
    let fresh: Vec<String> = pulls
        .into_iter()
        .filter(|url| !read.iter().any(|known| known == url))
        .map(|url| format!("{}\n{url}", thread.id))
        .collect();
    if fresh.is_empty() {
        return;
    }
    let asked = held().asked.lock().is_ok_and(|mut asked| {
        let before = asked.len();
        asked.extend(fresh);
        asked.len() > before
    });
    if asked {
        nexus_pull_requests::watch::linked(&GpuiShell, &thread.id);
    }
}

/// The person looks at the session: its "Needs you" is read.
pub fn seen(thread_id: &str) {
    nexus_pull_requests::watch::seen(&GpuiShell, thread_id);
}

/// Keeps a session's watch for the windows, and tells them when it changed (the Nexus app's `putWatch`). Whether it
/// changed.
fn put(watch: PullWatch) -> bool {
    let summary = watch
        .status
        .as_ref()
        .map(|pull| Arc::new(nexus_pull_requests::summary::Summary::of(pull)));
    let changed = held().known.lock().is_ok_and(|mut known| {
        if known.get(&watch.thread_id).is_some_and(|known| known.watch == watch) {
            return false;
        }
        known.insert(watch.thread_id.clone(), Known { watch, summary });
        true
    });
    if changed && let Ok(mut listeners) = held().listeners.lock() {
        listeners.retain(|listener| listener.unbounded_send(()).is_ok());
    }
    changed
}

/// A signal each time a session's watch changes; a window draws again on it.
pub fn subscribe() -> UnboundedReceiver<()> {
    let (tx, rx) = unbounded();
    if let Ok(mut listeners) = held().listeners.lock() {
        listeners.push(tx);
    }
    rx
}

/// The session's watch as the windows last got it; None before the watch read its pull request.
pub fn known(thread_id: &str) -> Option<PullWatch> {
    held()
        .known
        .lock()
        .ok()?
        .get(thread_id)
        .map(|known| known.watch.clone())
}

/// The watch and its pre-parsed hover summary from the same update.
pub fn summary(thread_id: &str) -> Option<(PullWatch, Arc<nexus_pull_requests::summary::Summary>)> {
    let known = held().known.lock().ok()?;
    let known = known.get(thread_id)?;
    Some((known.watch.clone(), known.summary.clone()?))
}

/// Whether the session's pull request needs the person: a failed check, a review that asks for changes or a conflict
/// since they last looked. The sidebar fills the session's dot for it.
pub fn needs_you(thread_id: &str) -> bool {
    held()
        .known
        .lock()
        .is_ok_and(|known| known.get(thread_id).is_some_and(|known| known.watch.needs_you))
}

const NOT_RUNNING: &str = "The pull request watch runs while this app runs the sessions.";

/// Runs `work` on the watch's runtime, where `gh` and the workbench run: it may be awaited anywhere.
async fn on_runtime<T: Send + 'static>(
    work: impl Future<Output = Result<T, String>> + Send + 'static,
) -> Result<T, String> {
    let runtime = held()
        .runtime
        .read()
        .ok()
        .and_then(|runtime| runtime.clone())
        .filter(|_| GpuiShell::running())
        .ok_or(NOT_RUNNING)?;
    runtime.spawn(work).await.map_err(|error| error.to_string())?
}

/// The session's pull request: the watch's last read, or a read now (`wb_pull_watch`).
pub async fn read(thread_id: String) -> Result<PullWatch, String> {
    let watch = on_runtime(async move { nexus_pull_requests::watch::pull_watch(&GpuiShell, &thread_id).await }).await?;
    put(watch.clone());
    Ok(watch)
}

/// "Send to agent": queues the agent the message, whether or not its switch is on (`wb_pull_watch_send`).
pub async fn send(thread_id: String) -> Result<PullWatch, String> {
    let watch = on_runtime(async move {
        let text = nexus_pull_requests::watch::message_to_send(&GpuiShell, &thread_id)?;
        GpuiShell.queue(&thread_id, &text).await?;
        Ok(nexus_pull_requests::watch::sent(&GpuiShell, &thread_id))
    })
    .await?;
    put(watch.clone());
    Ok(watch)
}

/// The session's switch: send the agent the message on its own (`wb_pull_watch_auto`).
pub fn set_auto(thread_id: &str, on: bool) -> Result<PullWatch, String> {
    if !GpuiShell::running() {
        return Err(NOT_RUNNING.into());
    }
    let watch = nexus_pull_requests::watch::set_auto(&GpuiShell, thread_id, on)?;
    put(watch.clone());
    Ok(watch)
}

/// The person turns off the watch the session's agent armed (`wb_pull_watch_agent_off`).
pub fn agent_off(thread_id: &str) -> Result<PullWatch, String> {
    if !GpuiShell::running() {
        return Err(NOT_RUNNING.into());
    }
    let watch = nexus_pull_requests::watch::agent_off(&GpuiShell, thread_id)?;
    put(watch.clone());
    Ok(watch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_hear_of_a_watch_only_when_it_changed() {
        let mut changes = subscribe();
        let watch = PullWatch {
            thread_id: "pr-watch-known".into(),
            needs_you: true,
            ..PullWatch::default()
        };
        assert!(put(watch.clone()));
        assert_eq!(known("pr-watch-known").as_ref(), Some(&watch));
        assert!(needs_you("pr-watch-known"));
        assert!(!needs_you("pr-watch-unknown"));
        assert!(changes.try_recv().is_ok());
        // The same watch again is nothing new.
        assert!(!put(watch.clone()));
        assert!(put(PullWatch {
            needs_you: false,
            ..watch
        }));
        assert!(!needs_you("pr-watch-known"));
    }
}
