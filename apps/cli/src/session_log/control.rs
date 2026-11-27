//! Phone control of one CLI run. The run takes only its own thread's commands from this computer's device
//! inbox (`control_thread_once`), so two runs never take each other's. Its thread's lease is enabled only while the
//! agent can apply commands and the person turned Phone control on here (`nexus devices control on`). When the run
//! ends, its lease ends too, so the phone reads the closed terminal as offline.

use std::sync::Arc;
use std::time::Duration;

use nexus_workbench::Workbench;
use nexus_workbench::session_sync::control::Action;
use nexus_workbench::session_sync::control_loop::{Performer, ThreadControl};
use tokio::sync::watch;

/// How often the run reads its commands: the desktop's pace.
const PASS: Duration = Duration::from_secs(2);
/// How long a closing run waits for Gateway to take the lease's end. Otherwise the lease lapses in 45 seconds.
const END: Duration = Duration::from_secs(3);

/// What [`Performer::perform`] returns.
pub type Performed<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>;

/// An agent the CLI cannot direct: its runs stay read-only from the phone.
pub struct ReadOnly;

impl Performer for ReadOnly {
    fn ready(&self) -> bool {
        false
    }
    fn perform<'a>(&'a self, _: &'a str, _: Option<i64>, _: Action) -> Performed<'a> {
        Box::pin(async { Err("This terminal session cannot take phone commands.".to_string()) })
    }
}

/// The run's performer. With a Codex app-server socket, the CLI's client of that server (`codex_remote::client`): it
/// attaches to the conversation once the log knows it, and its live events go to the run's thread. Otherwise, or when
/// the server is not reachable, the run is read-only from the phone.
pub async fn performer(
    socket: Option<&std::path::Path>,
    workbench: &Workbench,
    thread: watch::Receiver<Option<String>>,
    conversation: watch::Receiver<Option<String>>,
    stop: watch::Receiver<bool>,
) -> Arc<dyn Performer> {
    #[cfg(unix)]
    if let Some(socket) = socket {
        let (events, mut live) = tokio::sync::mpsc::unbounded_channel();
        if let Ok(client) = crate::codex_remote::client::Client::connect(socket, events).await {
            let logging = workbench.clone();
            tokio::spawn(async move {
                while let Some(event) = live.recv().await {
                    let local = thread.borrow().clone();
                    if let Some(local) = local {
                        let _ = logging.cli_events(&local, &[(super::now_ms(), event)]);
                    }
                }
            });
            tokio::spawn(attach(client.clone(), conversation, stop));
            return client;
        }
    }
    #[cfg(not(unix))]
    let _ = (socket, workbench, thread, conversation, stop);
    Arc::new(ReadOnly)
}

/// Attaches the client to the conversation once the log knows it. Codex refuses until the conversation's first turn
/// wrote its rollout, so it tries again at each pass until the run ends.
#[cfg(unix)]
async fn attach(
    client: Arc<crate::codex_remote::client::Client>,
    mut conversation: watch::Receiver<Option<String>>,
    mut stop: watch::Receiver<bool>,
) {
    let id = tokio::select! {
        found = conversation.wait_for(Option::is_some) => match found {
            Ok(id) => id.clone().unwrap_or_default(),
            Err(_) => return,
        },
        _ = stop.wait_for(|stopped| *stopped) => return,
    };
    loop {
        if client.attach(&id).await.is_ok() {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(PASS) => {}
            _ = stop.wait_for(|stopped| *stopped) => return,
        }
    }
}

/// Passes until `stop`, once the run's thread is known, then ends the thread's lease.
pub async fn run(
    workbench: Workbench,
    mut thread: watch::Receiver<Option<String>>,
    performer: Arc<dyn Performer>,
    mut stop: watch::Receiver<bool>,
) {
    let mut control: Option<ThreadControl> = None;
    let mut ticks = tokio::time::interval(PASS);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *stop.borrow() {
            break;
        }
        if control.is_none() {
            control = thread.borrow_and_update().clone().map(ThreadControl::new);
        }
        // A pass dropped at the run's end is safe: a command is reserved before it runs, and is never run twice.
        let pass = async {
            if let Some(control) = control.as_mut() {
                workbench.control_thread_once(control, performer.as_ref()).await;
            }
            ticks.tick().await;
        };
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            _ = pass => {}
        }
    }
    if let Some(mut control) = control {
        let _ = tokio::time::timeout(END, workbench.control_thread_end(&mut control)).await;
    }
}
