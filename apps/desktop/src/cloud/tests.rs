//! Cloud machines in this build, against nexus-cloud's fake Gateway (`/v1/cloud/machines`, on a fake provider) and a
//! Nexus host in memory (`machines/tests.rs`'s): no SSH, no network, no real machine and no real Gateway.
//! `nexus_cloud::desktop` makes, wakes, sets up and connects each machine; this covers what this
//! build adds around it: a new cloud session from start to its first message, the sidebar's machine and its words, a
//! sleeping machine waking for a call the person makes (and not for a background one), Gateway's list from another
//! device, Gateway's refusals, a failed step and its Try again, and the end of a session.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use nexus_client::{Client, ClientOptions};
use nexus_cloud::fake::{Config, FakeBoat};
use nexus_cloud::fake_gateway::{FakeGateway, GatewayConfig};
use nexus_cloud::link::Provision;
use nexus_desktop_core::secrets::Vault;
use nexus_desktop_core::settings::Settings;
use nexus_remote::install::{Fetch, Installer, Source};
use nexus_remote::machine::{Backoff, Opened, Sink, Status};
use nexus_remote::ssh::Target;
use nexus_workbench::api::PROTOCOL;
use nexus_workbench::event::{SessionAgent, Via};
use serde_json::Value;

use super::waiting::{Message, Push, Waiting};
use super::words::{self, End, StartLine};
use super::*;
use crate::code::Block;
use crate::machine_view::Action;
use crate::machines::tests::{Fake, machines, serve, until};
use crate::machines::{CloudView, HistoryRead};

const LAPTOP: &str = "nxd_laptop";
const DESKTOP: &str = "nxd_desktop";

/// A scratch folder for one test's keys, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("nexus-gpui-cloud-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch folder");
        Scratch(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// This build around the cloud machines: settings in memory, an in-memory keychain, the Nexus sign-in, its machines
/// (`machines.rs`) for the relays and events, and the machine's host in memory instead of SSH.
struct TestEnv {
    settings: Mutex<Settings>,
    vault: Vault,
    gateway: Mutex<Option<Client>>,
    machines: Arc<Machines>,
    host: Arc<Fake>,
    data: PathBuf,
    /// Each event's name, in order.
    events: Mutex<Vec<String>>,
}

impl CloudEnv for TestEnv {
    fn settings(&self) -> Settings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn update_settings(&self, change: &mut dyn FnMut(&mut Settings)) -> Result<(), String> {
        let mut settings = self.settings.lock().map_err(|e| e.to_string())?;
        change(&mut settings);
        Ok(())
    }

    fn vault(&self) -> Vault {
        self.vault.clone()
    }

    fn gateway(&self) -> Option<Client> {
        self.gateway.lock().ok().and_then(|g| g.clone())
    }

    fn emit(&self, event: &str, payload: Value) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event.to_string());
        }
        self.machines.cloud_event(event, &payload, &self.settings());
    }

    fn sink(&self, machine: &str) -> Arc<dyn Sink> {
        self.machines.sink(machine)
    }

    fn source(&self) -> Source {
        Source::Local {
            path: self.data.join("unused"),
        }
    }

    fn fetch(&self) -> Option<Fetch> {
        None
    }

    fn data_dir(&self) -> PathBuf {
        self.data.clone()
    }

    /// The machine in memory needs no setup.
    fn provision<'a>(
        &'a self,
        _target: &'a Target,
        _setup: &'a Provision,
    ) -> BoxFuture<'a, Result<HashMap<String, String>, nexus_cloud::Error>> {
        Box::pin(async { Ok(HashMap::new()) })
    }

    /// The machine's host, in memory: down while `host.up` is false, as an install that failed.
    fn open_host(&self, _installer: Installer) -> BoxFuture<'static, Result<Opened, String>> {
        let host = self.host.clone();
        Box::pin(async move {
            if !host.up.load(Ordering::SeqCst) {
                return Err("Could not install nexus-host: the download failed.".into());
            }
            host.opened.fetch_add(1, Ordering::SeqCst);
            let (client, server) = tokio::io::duplex(1 << 16);
            tokio::spawn(serve(host.clone(), server));
            Ok(Opened {
                stream: Box::new(client),
                token: "secret".into(),
                version: None,
                keep: None,
                sign_in: None,
            })
        })
    }
}

impl TestEnv {
    fn sign_in(&self, gateway: &FakeGateway, token: Option<&str>) {
        let client = token.map(|token| {
            let mut options = ClientOptions::new(&gateway.url());
            options.api_key = Some(token.into());
            Client::new(options)
        });
        if let Ok(mut slot) = self.gateway.lock() {
            *slot = client;
        }
    }

    fn saw(&self, event: &str) -> bool {
        self.events
            .lock()
            .is_ok_and(|events| events.iter().any(|seen| seen == event))
    }
}

/// The fake cloud provider and the fake Gateway over it, which takes both devices' tokens.
async fn rig() -> (FakeBoat, FakeGateway) {
    let boat = FakeBoat::start(Config::default()).await.expect("the fake boat");
    let gateway = FakeGateway::start(GatewayConfig {
        tokens: vec![LAPTOP.into(), DESKTOP.into()],
        boat_url: boat.url(),
        boat_key: "boat_fake".into(),
        capacity: None,
    })
    .await
    .expect("the fake Gateway");
    (boat, gateway)
}

/// Device `token`'s app: its machines, its environment and its cloud machines, signed in to `gateway`.
fn device(dir: &Path, gateway: &FakeGateway, token: &str) -> (Arc<Machines>, Arc<TestEnv>, Cloud, Arc<Fake>) {
    let host = Fake::new(PROTOCOL);
    let machines = machines(&host);
    let env = Arc::new(TestEnv {
        settings: Mutex::new(Settings::default()),
        vault: Vault::Memory(Default::default()),
        gateway: Mutex::new(None),
        machines: machines.clone(),
        host: host.clone(),
        data: dir.join(format!("data-{token}")),
        events: Mutex::new(Vec::new()),
    });
    env.sign_in(gateway, Some(token));
    let options = Options {
        local_gh: PathBuf::from("gh"),
        backoff: Backoff {
            first: Duration::from_millis(50),
            max: Duration::from_millis(200),
            healthy: Duration::from_secs(60),
        },
        // The tests ask for the list themselves.
        poll: Duration::from_secs(3600),
        sync_fresh: Duration::ZERO,
        github_api: "http://127.0.0.1:1".into(),
        wake_wait: Duration::from_secs(60),
    };
    let cloud = Cloud::new(env.clone(), options);
    machines.set_cloud(cloud.clone());
    cloud.start();
    (machines, env, cloud, host)
}

const SESSION_1: &str = "t_00000000-0000-4000-8000-000000000001";
const SESSION_2: &str = "t_00000000-0000-4000-8000-000000000002";
const SESSION_3: &str = "t_00000000-0000-4000-8000-000000000003";
const SESSION_4: &str = "t_00000000-0000-4000-8000-000000000004";
const SESSION_5: &str = "t_00000000-0000-4000-8000-000000000005";
const SESSION_6: &str = "t_00000000-0000-4000-8000-000000000006";
const SESSION_7: &str = "t_00000000-0000-4000-8000-000000000007";
const SESSION_8: &str = "t_00000000-0000-4000-8000-000000000008";

fn start(session_id: &str) -> CloudStart {
    CloudStart {
        request_id: session_id.strip_prefix("t_").expect("session ID prefix").into(),
        repo: "https://github.com/acme/api".into(),
        new: NewSession {
            project_id: String::new(),
            agent: SessionAgent::Claude,
            model: None,
            permission_mode: "default".into(),
            effort: None,
            via: Via::Nexus,
            provider: None,
            worktree: false,
            review: None,
            base: None,
            text: "Fix the login\nand its tests".into(),
            attachments: Default::default(),
        },
        waiting: Waiting::default(),
        cloning: Default::default(),
    }
}

fn only(machines: &Machines) -> CloudView {
    machines.cloud().into_iter().next().expect("a cloud machine")
}

fn now() -> i64 {
    1_793_534_400_000
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cloud_session_starts_on_a_machine_of_its_own_and_runs_there() {
    let dir = Scratch::new("start");
    let (_boat, gateway) = rig().await;
    let (machines, env, cloud, host) = device(dir.path(), &gateway, LAPTOP);
    let settings = {
        let env = env.clone();
        move || env.settings()
    };

    let made = start_session(&machines, &cloud, &settings, &start(SESSION_1)).await;
    assert_eq!(
        made.as_deref(),
        Ok(SESSION_1),
        "the session is the one its machine was made for"
    );

    // One machine, for that session, connected and read: the sidebar lists the session under it.
    let machine = only(&machines);
    assert_eq!(machine.info.session_id.as_deref(), Some(SESSION_1));
    assert_eq!(machine.info.title.as_deref(), Some("Fix the login"));
    assert!(matches!(machine.status, Status::Connected { .. }) && machine.read);
    assert_eq!(words::cloud_view(&machine.info, &machine.status, now()).label, "Ready");
    assert_eq!(machines.machine_of(SESSION_1).as_deref(), Some(machine.id.as_str()));
    assert!(
        machines
            .projects()
            .iter()
            .any(|project| project.machine.as_deref() == Some(machine.id.as_str()))
    );
    assert!(
        host.host
            .lock()
            .is_ok_and(|host| host.threads.iter().any(|thread| thread["id"] == SESSION_1))
    );
    // Through Gateway: the machine for the session, this device's key, then the host was reached.
    let calls: Vec<String> = gateway
        .calls()
        .iter()
        .map(|call| format!("{} {}", call.method, call.path))
        .collect();
    assert!(calls.contains(&"POST ".to_string()), "{calls:?}");
    assert!(calls.iter().any(|call| call.ends_with("/access")), "{calls:?}");
    assert!(env.saw("cloud:progress") && env.saw("cloud:state"));
    assert_eq!(host.opened.load(Ordering::SeqCst), 1);

    // A reply goes to the machine; the same start again (a retry) finds the session and makes no other machine.
    let sent = machines
        .call(
            &machine.id,
            "wb_send",
            serde_json::json!({ "threadId": SESSION_1, "text": "And the docs" }),
        )
        .await;
    assert!(matches!(sent, Ok(Ok(_))), "{sent:?}");
    let again = start_session(&machines, &cloud, &settings, &start(SESSION_1)).await;
    assert_eq!(again.as_deref(), Ok(SESSION_1));
    assert_eq!(machines.cloud().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_machines_own_setup_is_waited_for_and_its_failure_said_once() {
    let dir = Scratch::new("setup");
    let (_boat, gateway) = rig().await;
    let (machines, env, cloud, host) = device(dir.path(), &gateway, LAPTOP);
    let settings = move || env.settings();
    start_session(&machines, &cloud, &settings, &start(SESSION_7))
        .await
        .expect("started");
    let id = only(&machines).id;
    let setup = |state: &str, project: Value, reason: Value| serde_json::json!({ "state": state, "repo": "acme/api", "stage": "Receiving objects", "percent": 43, "project": project, "reason": reason });
    let made =
        serde_json::json!({ "id": "p9", "name": "api", "path": "/home/user/api", "branch": "main", "addedAt": 9 });

    // An older host does not know it: the app clones.
    let begin = start(SESSION_7);
    assert_eq!(
        attach_setup(&machines, &id, &begin).await.map(|p| p.map(|p| p.id)),
        Ok(None)
    );
    // Cloning, then ready: its project, and the line is let go.
    host.host.lock().expect("host").setup = vec![
        setup("cloning", Value::Null, Value::Null),
        setup("ready", made.clone(), Value::Null),
    ];
    let attached = attach_setup(&machines, &id, &begin).await.expect("attached");
    assert_eq!(attached.map(|project| project.id).as_deref(), Some("p9"));
    assert_eq!(begin.cloning.get(), None);
    // Failed: the start says why, once; Try again runs it again.
    host.host.lock().expect("host").setup = vec![setup("failed", Value::Null, ".agents/setup exited with 1".into())];
    let failed = attach_setup(&machines, &id, &begin).await.map(|p| p.map(|p| p.id));
    assert_eq!(
        failed,
        Err("Setting up acme/api failed: .agents/setup exited with 1".into())
    );
    host.host.lock().expect("host").setup = vec![setup("ready", made, Value::Null)];
    let again = attach_setup(&machines, &id, &begin).await.expect("ran again");
    assert_eq!(again.map(|project| project.id).as_deref(), Some("p9"));
    assert_eq!(host.host.lock().expect("host").setup_retries, 1);
    // Another repository's setup is not this session's.
    let mut other = start(SESSION_8);
    other.repo = "https://github.com/acme/web".into();
    assert_eq!(
        attach_setup(&machines, &id, &other).await.map(|p| p.map(|p| p.id)),
        Ok(None)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sleeping_machine_wakes_for_a_call_the_person_makes() {
    let dir = Scratch::new("wake");
    let (boat, gateway) = rig().await;
    let (machines, env, cloud, host) = device(dir.path(), &gateway, LAPTOP);
    let settings = move || env.settings();
    start_session(&machines, &cloud, &settings, &start(SESSION_2))
        .await
        .expect("started");
    let id = only(&machines).id;

    // Asleep: the relay is held, and the sidebar shows only a quiet "Asleep": no Wake, no cost talk.
    cloud.sleep(&id).await.expect("asleep");
    let boat_id = gateway.boat_id(&id).expect("its boat machine");
    for _ in 0..100 {
        cloud.refresh().await;
        if only(&machines).info.state == CloudState::Sleeping {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let asleep = only(&machines);
    assert_eq!(asleep.info.state, CloudState::Sleeping);
    assert_eq!(asleep.status, Status::Closed);
    let view = words::cloud_view(&asleep.info, &asleep.status, now());
    assert_eq!((view.label, view.detail, view.action), ("Asleep", None, None));
    // Nothing is on its way until the person acts: no "Waking the machine" line above the composer.
    assert_eq!(words::coming(&asleep, now()), None);
    // Its session still belongs to it, so a message sent there goes to it (and wakes it).
    assert_eq!(machines.machine_of(SESSION_2).as_deref(), Some(id.as_str()));

    // A background read does not wake it, and neither does opening its session to read it.
    for (call, args) in [
        ("wb_overview", serde_json::json!({})),
        (
            "wb_thread_page",
            serde_json::json!({ "threadId": SESSION_2, "beforeSeq": null, "limit": 50 }),
        ),
        (
            "wb_thread_stream",
            serde_json::json!({"threadId":SESSION_2,"afterSeq":0}),
        ),
        ("wb_git_state", serde_json::json!({ "threadId": SESSION_2 })),
    ] {
        let read = machines.call(&id, call, args).await;
        assert_eq!(
            read.ok().and_then(Result::err).as_deref(),
            Some(nexus_cloud::desktop::ASLEEP),
            "{call}"
        );
    }
    assert_eq!(
        boat.state(&boat_id).as_deref(),
        Some("archived"),
        "still asleep at boat"
    );
    assert_eq!(host.opened.load(Ordering::SeqCst), 1, "no connect for a read");

    // A message the person sends wakes it, connects again and goes through.
    let sent = machines
        .call(
            &id,
            "wb_send",
            serde_json::json!({ "threadId": SESSION_2, "text": "Go on" }),
        )
        .await;
    assert!(matches!(sent, Ok(Ok(_))), "{sent:?}");
    assert_eq!(host.opened.load(Ordering::SeqCst), 2, "a second connect after the wake");
    let awake = only(&machines);
    assert_eq!(awake.info.state, CloudState::Ready);
    assert!(matches!(awake.status, Status::Connected { .. }));
}

/// The session's Cloud panel: its share link and its desktop come from Gateway, for this session's machine only. Its
/// ports and Preview run over SSH, which this test does not cover.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cloud_panel_shares_a_port_and_the_desktop_through_gateway() {
    let dir = Scratch::new("panel");
    let (_boat, gateway) = rig().await;
    let (machines, env, cloud, _) = device(dir.path(), &gateway, LAPTOP);
    let settings = move || env.settings();
    start_session(&machines, &cloud, &settings, &start(SESSION_6))
        .await
        .expect("started");
    let id = only(&machines).id;

    let link = cloud.share(&id, 3000).await.expect("a share link");
    assert!(link.starts_with("http"), "{link}");
    let screen = cloud.desktop(&id).await.expect("the desktop");
    assert!(screen.starts_with("http"), "{screen}");
    assert_ne!(link, screen);
    let links: Vec<String> = gateway
        .calls()
        .iter()
        .filter(|call| call.path.ends_with("/links"))
        .map(|call| format!("{} {}", call.method, call.path))
        .collect();
    assert_eq!(links.len(), 2, "{links:?}");
    assert!(
        links.iter().all(|call| call.starts_with("POST ") && call.contains(&id)),
        "{links:?}"
    );

    // A machine that is not this person's is refused, and nothing reaches Gateway for it.
    let before = gateway.calls().len();
    assert!(cloud.share("cm_someone_else", 3000).await.is_err());
    assert!(cloud.desktop("cm_someone_else").await.is_err());
    assert_eq!(gateway.calls().len(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_list_follows_gateway_and_a_session_ends_with_its_machine() {
    let dir = Scratch::new("list");
    let (_boat, gateway) = rig().await;
    let (laptop, laptop_env, laptop_cloud, _) = device(dir.path(), &gateway, LAPTOP);
    let (desktop, desktop_env, desktop_cloud, _) = device(dir.path(), &gateway, DESKTOP);
    let desktop_settings = move || desktop_env.settings();
    start_session(&desktop, &desktop_cloud, &desktop_settings, &start(SESSION_3))
        .await
        .expect("started on the other device");

    // The laptop lists the session made on the other device once Gateway is asked.
    laptop_cloud.sync().await.expect("listed");
    assert!(laptop_env.saw("cloud:machines"));
    let listed = only(&laptop);
    assert_eq!(listed.info.session_id.as_deref(), Some(SESSION_3));

    // Archiving it there (forced: nothing to check on a machine in memory) deletes the machine everywhere.
    let ended = end_session(&desktop, &desktop_cloud, &listed.id, true).await;
    assert_eq!(ended, Ok(Ended::Gone));
    assert!(desktop.cloud().is_empty());
    laptop_cloud.sync().await.expect("listed");
    assert!(laptop.cloud().is_empty(), "forgotten on the laptop at its next look");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refusals_and_a_failed_step_say_why_and_try_again_resumes() {
    let dir = Scratch::new("errors");
    let (_boat, gateway) = rig().await;
    let (machines, env, cloud, host) = device(dir.path(), &gateway, LAPTOP);
    let settings = {
        let env = env.clone();
        move || env.settings()
    };

    // Gateway's refusals, in the Nexus app's words; no machine is made.
    gateway.refuse(Some((503, "cloud_unavailable")));
    let refused = start_session(&machines, &cloud, &settings, &start(SESSION_4)).await;
    assert_eq!(
        refused,
        Err("Cloud machines aren't available on this Nexus yet.".into())
    );
    gateway.refuse(Some((402, "budget_exceeded")));
    let refused = start_session(&machines, &cloud, &settings, &start(SESSION_4)).await;
    assert!(refused.is_err_and(|error| error.contains("over its budget")));
    gateway.refuse(None);
    assert!(machines.cloud().is_empty());

    // The install fails: the step says why, the session's page shows it with Try again.
    host.up.store(false, Ordering::SeqCst);
    let failed = start_session(&machines, &cloud, &settings, &start(SESSION_4)).await;
    assert_eq!(failed, Err("Could not install nexus-host: the download failed.".into()));
    let machine = only(&machines);
    let progress = machine.progress.clone().expect("the failed step");
    assert_eq!((progress.step, progress.error.is_some()), (CloudStep::Install, true));
    let coming = words::coming(&machine, now()).expect("the machine's card");
    assert_eq!(coming.title, "The machine did not start");
    assert_eq!(
        coming.failed.as_deref(),
        Some("Could not install nexus-host: the download failed.")
    );

    // Try again: the same session resumes its machine from the step that failed.
    host.up.store(true, Ordering::SeqCst);
    let made = start_session(&machines, &cloud, &settings, &start(SESSION_4)).await;
    assert_eq!(made.as_deref(), Ok(SESSION_4));
    assert_eq!(machines.cloud().len(), 1, "the same machine");

    // Signed out: the machines end here, and a start says to sign in.
    env.sign_in(&gateway, None);
    let signed_out = start_session(&machines, &cloud, &settings, &start(SESSION_5)).await;
    assert_eq!(signed_out, Err("Sign in to Nexus to use your cloud machine.".into()));
    assert!(machines.cloud().is_empty());
}

/// Message `text`, typed while the session started.
fn typed(text: &str) -> Message {
    Message {
        id: 0,
        text: text.into(),
        attachments: Default::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messages_typed_while_it_starts_follow_the_first_in_order_once() {
    let dir = Scratch::new("waiting");
    let (_boat, gateway) = rig().await;
    let (machines, env, cloud, host) = device(dir.path(), &gateway, LAPTOP);
    let settings = move || env.settings();
    let sent = || host.host.lock().map(|host| host.sent.clone()).unwrap_or_default();

    let start = start(SESSION_6);
    for text in ["And the docs", "And the changelog"] {
        assert!(matches!(start.waiting.push(typed(text)), Push::Queued));
    }
    let made = start_session(&machines, &cloud, &settings, &start).await;
    assert_eq!(made.as_deref(), Ok(SESSION_6));
    // The first message made the session; the others followed it, in order.
    assert_eq!(sent(), ["And the docs", "And the changelog"]);
    assert!(!start.waiting.is_open());

    // A retry sends none of them again, and a message after the start comes back to be sent as any message is.
    let again = start_session(&machines, &cloud, &settings, &start).await;
    assert_eq!(again.as_deref(), Ok(SESSION_6));
    assert_eq!(sent().len(), 2);
    assert!(matches!(start.waiting.push(typed("late")), Push::Late(_)));
}

/// A message the host refuses goes back first, and Try again (the same start) sends every message once, in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_waiting_message_is_sent_once_on_try_again() {
    let dir = Scratch::new("waiting-retry");
    let (_boat, gateway) = rig().await;
    let (machines, env, cloud, host) = device(dir.path(), &gateway, LAPTOP);
    let settings = move || env.settings();
    let sent = || host.host.lock().map(|host| host.sent.clone()).unwrap_or_default();

    let start = start(SESSION_8);
    for text in ["And the docs", "And the changelog"] {
        assert!(matches!(start.waiting.push(typed(text)), Push::Queued));
    }
    host.host.lock().expect("the host").refuse_send = true;
    let failed = start_session(&machines, &cloud, &settings, &start).await;
    assert_eq!(failed, Err("The host refused the message.".into()));
    assert!(sent().is_empty());
    // Both still wait, the refused one first: the person can Edit them or Try again.
    let waiting: Vec<String> = start.waiting.texts().into_iter().map(|(_, text)| text).collect();
    assert_eq!(waiting, ["And the docs", "And the changelog"]);

    let made = start_session(&machines, &cloud, &settings, &start).await;
    assert_eq!(made.as_deref(), Ok(SESSION_8));
    assert_eq!(sent(), ["And the docs", "And the changelog"]);
}

/// A lost answer is the case that must not resend: the host may have the message, so it is tried once, not put back,
/// and the error says to look in the session. A refusal, which proves it did not arrive, is put back.
#[tokio::test]
async fn a_lost_answer_is_not_sent_again_but_a_refusal_is() {
    let waiting = Waiting::default();
    for text in ["one", "two"] {
        assert!(matches!(waiting.push(typed(text)), Push::Queued));
    }
    let tried = Mutex::new(Vec::new());
    let lost = super::send_each(&waiting, |message| {
        tried.lock().expect("tried").push(message.text.clone());
        async { Err(super::LOST.to_string()) }
    })
    .await;
    assert!(lost.is_err_and(|error| error.starts_with(super::LOST) && error.contains("not sent again")));
    // Sent once, and "two" waits for the next send, "one" is not put back.
    assert_eq!(*tried.lock().expect("tried"), ["one"]);
    let left: Vec<String> = waiting.texts().into_iter().map(|(_, text)| text).collect();
    assert_eq!(left, ["two"]);

    let refused = super::send_each(&waiting, |_| async { Err("The host refused the message.".to_string()) }).await;
    assert_eq!(refused, Err("The host refused the message.".into()));
    let left: Vec<String> = waiting.texts().into_iter().map(|(_, text)| text).collect();
    assert_eq!(left, ["two"], "a refused message goes back");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_start_makes_no_session_and_sends_nothing() {
    let dir = Scratch::new("ended");
    let (_boat, gateway) = rig().await;
    let (machines, env, cloud, host) = device(dir.path(), &gateway, LAPTOP);
    let settings = move || env.settings();

    let start = start(SESSION_7);
    assert!(matches!(start.waiting.push(typed("And the docs")), Push::Queued));
    start.waiting.end();
    let made = start_session(&machines, &cloud, &settings, &start).await;
    assert_eq!(made, Err("The session was ended.".into()));
    let host = host.host.lock().expect("the host");
    assert!(host.sent.is_empty());
    assert!(host.threads.is_empty());
}

/// [`super::after_lost`] in milliseconds, not minutes.
const QUICK: super::Patience = super::Patience {
    reconnect: Duration::from_millis(100),
    first: Duration::from_millis(1),
    most: Duration::from_millis(10),
    grace: Duration::from_millis(20),
};

/// Follows a clone whose answer was lost, with the machine answering `script` in turn (then `Away` for good). Answers
/// how it went and how many times it looked.
async fn follow(script: Vec<super::Cloning>, waiting: &Waiting) -> (Result<String, String>, usize) {
    let script = Mutex::new(std::collections::VecDeque::from(script));
    let looks = Mutex::new(0);
    let followed = super::after_lost(waiting, QUICK, || {
        *looks.lock().expect("looks") += 1;
        let next = script
            .lock()
            .expect("script")
            .pop_front()
            .unwrap_or(super::Cloning::Away);
        async move { next }
    })
    .await;
    let looks = *looks.lock().expect("looks");
    (followed, looks)
}

#[tokio::test]
async fn a_clone_whose_answer_was_lost_goes_on_once_the_machine_is_back() {
    use super::Cloning::{Away, Ended, Running};
    assert!(super::lost(nexus_remote::client::LOST));
    assert!(super::lost(super::LOST));
    assert!(!super::lost("Repository not found."));

    // The connection comes back while the host still clones, then the clone ends with the project.
    let waiting = Waiting::default();
    let (followed, looks) = follow(vec![Away, Away, Running, Running, Ended(Some("cp".into()))], &waiting).await;
    assert_eq!(followed, Ok("cp".into()));
    assert_eq!(looks, 5);
    // The clone ended as the connection dropped: the project shows a moment later.
    let (followed, _) = follow(vec![Ended(None), Ended(Some("cp".into()))], &waiting).await;
    assert_eq!(followed, Ok("cp".into()));
    // The clone ended with no project: it failed, and Try again clones again.
    let (followed, _) = follow(
        std::iter::once(Running)
            .chain(std::iter::repeat_n(Ended(None), 200))
            .collect(),
        &waiting,
    )
    .await;
    assert_eq!(followed, Err(super::CLONE_FAILED.into()));
    // The machine never comes back: the start fails with why.
    let (followed, looks) = follow(vec![], &waiting).await;
    assert!(
        followed.is_err_and(|error| error.starts_with(nexus_remote::client::LOST) && error.ends_with(super::NOT_BACK))
    );
    assert!(looks > 1, "it looked once and gave up");
    // The person ended the session: nothing more is asked.
    waiting.end();
    let (followed, looks) = follow(vec![Running], &waiting).await;
    assert_eq!(followed, Err(super::ENDED.into()));
    assert_eq!(looks, 0);
}

#[tokio::test]
async fn a_refused_create_says_whose_switch_it_is() {
    let role = "Your role in this organization can't use cloud machines.";
    // The list works: the role may use cloud machines, so the organization has them off (`cloud_not_enabled`).
    let refused = super::refusal(role.into(), || async { Ok(()) }).await;
    assert_eq!(refused, "Cloud machines aren't turned on for this organization yet.");
    // The list is refused too: the role's.
    let refused = super::refusal(role.into(), || async { Err("refused".into()) }).await;
    assert_eq!(refused, role);
    // Any other refusal reads as it came.
    let refused = super::refusal("Over budget.".into(), || async { Ok(()) }).await;
    assert_eq!(refused, "Over budget.");
}

#[test]
fn a_start_is_stuck_three_minutes_after_it_left_the_queue() {
    let limit = super::STUCK;
    let start = std::time::Instant::now();
    let mut deadline = super::Deadline::default();
    // Queued, it waits as long as it must.
    assert!(!deadline.passed(true, start, limit));
    assert!(!deadline.passed(true, start + limit * 2, limit));
    // Out of the queue, the clock runs from then.
    let left = start + limit * 2;
    assert!(!deadline.passed(false, left, limit));
    assert!(!deadline.passed(false, left + limit - Duration::from_secs(1), limit));
    assert!(deadline.passed(false, left + limit, limit));
    // Queued again, it starts over.
    assert!(!deadline.passed(true, left + limit, limit));
    assert!(!deadline.passed(false, left + limit * 2, limit));
}

#[test]
fn a_machine_reads_in_the_nexus_apps_words() {
    let info = |state: CloudState| CloudInfo {
        provider: "boat".into(),
        size: "default".into(),
        state,
        error: None,
        sleeping_since: None,
        session_id: Some("t_1".into()),
        title: Some("Fix the login".into()),
        queued_until: None,
        stop_reason: None,
        repo: None,
    };
    let label = |info: &CloudInfo, status: &Status| words::cloud_view(info, status, now()).label;
    assert_eq!(label(&info(CloudState::Creating), &Status::Closed), "Creating…");
    assert_eq!(label(&info(CloudState::Starting), &Status::Closed), "Waking…");
    assert_eq!(label(&info(CloudState::Stopping), &Status::Closed), "Going to sleep…");
    assert_eq!(label(&info(CloudState::Ready), &Status::Closed), "Offline");
    assert_eq!(label(&info(CloudState::Gone), &Status::Closed), "Deleted");
    let mut queued = info(CloudState::Queued);
    queued.queued_until = Some(now() + 150_000);
    assert_eq!(
        words::cloud_view(&queued, &Status::Closed, now()).detail.as_deref(),
        Some("Waiting for a machine (about 3 min)")
    );
    let mut resizing = info(CloudState::Starting);
    resizing.stop_reason = Some("resize".into());
    assert_eq!(label(&resizing, &Status::Closed), "Restarting on a bigger machine");
    let mut failed = info(CloudState::Failed);
    failed.error = Some("no capacity".into());
    let view = words::cloud_view(&failed, &Status::Closed, now());
    assert_eq!(
        (view.detail.as_deref(), view.action),
        (Some("no capacity"), Some(Action::Retry))
    );

    let progress = CloudProgress {
        machine: "cm_1".into(),
        step: CloudStep::Ssh,
        detail: None,
        error: None,
    };
    let view = |info: CloudInfo, progress: Option<CloudProgress>| CloudView {
        id: "cm_1".into(),
        name: "Cloud".into(),
        info,
        status: Status::Closed,
        progress,
        read: false,
    };
    let busy = |text: &str| StartLine::Busy(text.into());

    // The one line above the composer while a new session's machine comes up: its step, in short.
    let creating = view(info(CloudState::Creating), Some(progress.clone()));
    assert_eq!(
        words::start_line(Some(&creating), None, None, now()),
        busy("Starting a machine · Connecting")
    );
    assert_eq!(words::start_line(None, None, None, now()), busy("Starting a machine"));
    // Queued: the queue's line, with its wait.
    let waiting = view(queued, None);
    assert_eq!(
        words::start_line(Some(&waiting), None, None, now()),
        busy("Waiting for a machine (about 3 min)")
    );
    // Running: the session is made there.
    let ready = CloudProgress {
        step: CloudStep::Ready,
        ..progress.clone()
    };
    let running = view(info(CloudState::Ready), Some(ready));
    assert_eq!(words::machine_line(&running, now()), None);
    assert_eq!(
        words::start_line(Some(&running), None, None, now()),
        busy("Cloning the repository")
    );
    // While it clones: git's stage, with its percent once git counts.
    assert_eq!(
        words::start_line(
            Some(&running),
            None,
            Some(&("Receiving objects".into(), Some(43))),
            now()
        ),
        busy("Cloning the repository · Receiving objects · 43%")
    );
    assert_eq!(
        words::start_line(Some(&running), None, Some(&("Connecting".into(), None)), now()),
        busy("Cloning the repository · Connecting")
    );
    // A machine that started from a saved copy of the repository says so.
    assert_eq!(
        words::start_line(
            Some(&running),
            None,
            Some(&("Starting from a saved copy of acme/web".into(), None)),
            now()
        ),
        busy("Starting from a saved copy of acme/web")
    );
    // The machine's own setup runs the repository's .agents/setup.
    assert_eq!(
        words::start_line(Some(&running), None, Some(&("Running setup".into(), None)), now()),
        busy("Running setup")
    );
    // The machine's own line wins while it is still coming up.
    assert_eq!(
        words::start_line(Some(&creating), None, Some(&("Connecting".into(), None)), now()),
        busy("Starting a machine · Connecting")
    );
    // A failed step says why; so does a start that failed after it.
    let broken = CloudProgress {
        step: CloudStep::Install,
        error: Some("Could not install nexus-host: the download failed.".into()),
        ..progress.clone()
    };
    let broken = view(info(CloudState::Creating), Some(broken));
    assert_eq!(
        words::start_line(Some(&broken), None, None, now()),
        StartLine::Failed("Could not install nexus-host: the download failed.".into())
    );
    assert_eq!(
        words::start_line(Some(&running), Some(super::STUCK_ERROR), None, now()),
        StartLine::Failed("The machine did not start within 3 minutes.".into())
    );
    // Asleep with nothing waking it: no line at all, the session reads as it was.
    assert_eq!(
        words::machine_line(&view(info(CloudState::Sleeping), None), now()),
        None
    );
    // A wake (the person typed or sent) shows its own title with the step.
    let waking = view(info(CloudState::Sleeping), Some(progress));
    assert_eq!(
        words::machine_line(&waking, now()),
        Some(busy("Waking the machine · Connecting"))
    );
    assert_eq!(words::step_line(CloudStep::Connect), "Starting Nexus");
}

#[test]
fn the_setup_the_repositories_and_the_endings_read_as_in_the_nexus_app() {
    assert_eq!(
        words::missing(SessionAgent::Claude, false, true),
        vec![words::Check::Claude]
    );
    assert_eq!(
        words::missing(SessionAgent::Codex, false, false),
        vec![words::Check::Github]
    );
    assert!(words::plan_refused(SessionAgent::Codex, Via::Subscription));
    assert!(!words::plan_refused(SessionAgent::Claude, Via::Subscription));
    // Antigravity runs on its Google AI plan only, whose sign-in stays here: never on Cloud, and no Claude step for it.
    for via in [Via::Subscription, Via::Nexus] {
        assert!(words::plan_refused(SessionAgent::Antigravity, via), "{via:?}");
    }
    assert_eq!(words::missing(SessionAgent::Antigravity, false, false), vec![]);
    let line = |agent, via| words::plan_refused_line(agent, via).unwrap_or_default();
    assert!(line(SessionAgent::Antigravity, Via::Subscription).starts_with("Antigravity doesn't run on Cloud."));
    assert!(line(SessionAgent::Codex, Via::Subscription).starts_with("Codex on Cloud runs on Team API."));
    assert_eq!(words::plan_refused_line(SessionAgent::Codex, Via::Nexus), None);
    assert_eq!(words::plan_refused_line(SessionAgent::Claude, Via::Subscription), None);
    assert_eq!(words::session_title("  \n"), "Cloud session");
    assert_eq!(words::session_title(&"x".repeat(100)).chars().count(), 80);
    let repo = |name: &str, pushed: Option<i64>| CloudRepo {
        name: name.into(),
        url: String::new(),
        private: false,
        pushed_at: pushed,
    };
    let picked = words::pick_repos(
        &[repo("org/old", Some(1)), repo("me/new", Some(9)), repo("me/api", None)],
        "me/",
    );
    assert_eq!(
        picked.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["me/new", "me/api"]
    );
    assert_eq!(
        words::unpushed_text("Fix", &["~/nexus-worktrees/a".into()], End::Archive),
        "\u{201c}Fix\u{201d} has commits that aren\u{2019}t on GitHub: ~/nexus-worktrees/a. Archiving deletes its \
         machine and them."
    );
    assert_eq!(words::ended_text("Fix", End::Delete), "Deleted \u{201c}Fix\u{201d}");
    assert_eq!(words::counts_line((1, 2)), "1 session running, 2 asleep");
    assert_eq!(words::panel_tip("Asleep"), "Cloud · Asleep. Preview, share and watch");
    assert_eq!(words::panel_waiting("Asleep"), "Cloud is asleep");
    assert_eq!(
        words::shared_line(3000, true),
        "Link copied. Anyone with the link can open port 3000."
    );
    assert_eq!(
        words::shared_line(3000, false),
        "Anyone with the link can open port 3000."
    );
    assert_eq!(
        words::unavailable_line(Some("not_allowed")),
        "Your role in this organization can't use cloud machines. Ask an admin if you need one."
    );
    // A 403 reads by its code: the organization's switch, or the person's role.
    assert_eq!(
        words::unavailable_line(Some("not_enabled")),
        "Cloud machines aren't turned on for this organization yet."
    );
    assert_eq!(
        words::refusal("cloud_not_enabled"),
        "Cloud machines aren't turned on for this organization yet."
    );
    assert_eq!(
        words::refusal("forbidden"),
        "Your role in this organization can't use cloud machines. Ask an admin if you need one."
    );
    assert_eq!(
        words::token_warning(Some(0), 340 * 86_400_000).as_deref(),
        Some("Expires in 25 days, make a new one.")
    );
}

#[test]
fn a_github_url_and_owner_name_are_one_repository() {
    assert!(same_repo("https://github.com/Acme/Web.git", "acme/web"));
    assert!(same_repo("git@github.com:acme/web", "acme/web/"));
    assert!(!same_repo("https://github.com/acme/web", "acme/api"));
}

/// The person's message `seq`, as its machine logs it: one block each (consecutive answer text joins into one).
fn said(seq: i64) -> Value {
    serde_json::json!({ "kind": "user", "text": format!("Step {seq}") })
}

/// Machine `id` (made on `made_on`) sleeps, and the laptop has listed it asleep from Gateway.
async fn asleep_on_the_laptop(id: &str, made_on: &Cloud, laptop: &Arc<Machines>, laptop_cloud: &Cloud) {
    made_on.sleep(id).await.expect("asleep");
    for _ in 0..100 {
        laptop_cloud.sync().await.expect("listed");
        if laptop
            .cloud()
            .first()
            .is_some_and(|machine| machine.info.state == CloudState::Sleeping)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the laptop never saw the machine asleep");
}

/// The history calls the fake Gateway answered, as their queries.
fn history_reads(gateway: &FakeGateway, id: &str, thread: &str) -> Vec<String> {
    let path = format!("/{id}/history/{thread}/events");
    gateway
        .calls()
        .into_iter()
        .filter(|call| call.method == "GET" && call.path == path)
        .map(|call| call.query)
        .collect()
}

/// A sleeping session opens on another device from Gateway's history, without waking its machine; it scrolls
/// back through Gateway, shows a placeholder for an event Gateway did not keep, and once the machine is awake again its
/// own events join with no event twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sleeping_session_opens_from_gateways_history_on_another_device() {
    let dir = Scratch::new("history");
    let (boat, gateway) = rig().await;
    let (desktop, desktop_env, desktop_cloud, _) = device(dir.path(), &gateway, DESKTOP);
    let (laptop, _laptop_env, laptop_cloud, laptop_host) = device(dir.path(), &gateway, LAPTOP);
    let desktop_settings = move || desktop_env.settings();
    start_session(&desktop, &desktop_cloud, &desktop_settings, &start(SESSION_7))
        .await
        .expect("started on the other device");
    let id = only(&desktop).id;
    asleep_on_the_laptop(&id, &desktop_cloud, &laptop, &laptop_cloud).await;
    let boat_id = gateway.boat_id(&id).expect("its boat machine");

    // The machine logged 652 events; Gateway holds the first 650, and did not keep 640's body.
    for seq in 1..=652 {
        laptop_host.push_event(SESSION_7, said(seq));
    }
    let bodies: Vec<(i64, String)> = (1..=650).map(|seq| (seq, said(seq).to_string())).collect();
    let stored: Vec<(i64, i64, Option<&str>)> = bodies
        .iter()
        .map(|(seq, body)| (*seq, *seq, (*seq != 640).then_some(body.as_str())))
        .collect();
    gateway.store_history(&id, SESSION_7, &stored);

    // Opening reads Gateway's latest page: 500 events, with more before them. Gateway answers late, so the read is
    // seen in flight however fast the runner is.
    gateway.delay(Some(Duration::from_millis(300)));
    assert!(laptop.transcript(&id, SESSION_7, false).is_none(), "on its way");
    assert_eq!(laptop.history(&id, SESSION_7), Some(HistoryRead::Reading));
    until("Gateway's page", || laptop.transcript(&id, SESSION_7, false).is_some()).await;
    gateway.delay(None);
    let Some(Ok(latest)) = laptop.transcript(&id, SESSION_7, false) else {
        panic!("the session reads");
    };
    assert_eq!(laptop.history(&id, SESSION_7), Some(HistoryRead::Kept));
    assert_eq!((latest.start, latest.last, latest.more), (Some(151), 650, true));
    assert_eq!(
        latest.blocks.iter().filter(|block| **block == Block::Omitted).count(),
        1
    );
    assert_eq!(latest.blocks.len(), 500);
    assert_eq!(history_reads(&gateway, &id, SESSION_7), vec!["limit=500"]);
    assert_eq!(
        boat.state(&boat_id).as_deref(),
        Some("archived"),
        "reading never wakes it"
    );

    // Scrolling back reads the page before from Gateway, down to the first event.
    let _ = laptop.transcript(&id, SESSION_7, true);
    until(
        "the older page",
        || matches!(laptop.transcript(&id, SESSION_7, false), Some(Ok(read)) if read.start == Some(1)),
    )
    .await;
    let Some(Ok(all)) = laptop.transcript(&id, SESSION_7, false) else {
        panic!("the session reads");
    };
    assert!(!all.more);
    assert_eq!(all.blocks.len(), 650);
    assert_eq!(history_reads(&gateway, &id, SESSION_7)[1], "before=151&limit=500");

    // Awake again: the machine adds what Gateway did not have yet, and nothing shows twice.
    laptop_cloud.wake(&id).expect("waking");
    until("connected", || matches!(only(&laptop).status, Status::Connected { .. })).await;
    until(
        "the machine's own events",
        || matches!(laptop.transcript(&id, SESSION_7, false), Some(Ok(read)) if read.last == 652),
    )
    .await;
    laptop_host.push_event(SESSION_7, said(653));
    until(
        "a pushed event",
        || matches!(laptop.transcript(&id, SESSION_7, false), Some(Ok(read)) if read.last == 653),
    )
    .await;
    let Some(Ok(awake)) = laptop.transcript(&id, SESSION_7, false) else {
        panic!("the session reads");
    };
    assert_eq!(awake.start, Some(1));
    assert_eq!(awake.blocks.len(), 653, "one block per event, none twice");
    assert!(
        !awake.blocks.contains(&Block::Omitted),
        "the machine's own copy of 640 replaced Gateway's placeholder"
    );
}

/// With cloud history off for the organization, or Gateway failing, a sleeping session reads as before (nothing until
/// its machine wakes); a failure says so, and Try again asks Gateway once more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn history_off_keeps_todays_sleeping_session_and_a_failure_tries_again() {
    let dir = Scratch::new("history-off");
    let (_boat, gateway) = rig().await;
    let (desktop, desktop_env, desktop_cloud, _) = device(dir.path(), &gateway, DESKTOP);
    let (laptop, _laptop_env, laptop_cloud, _) = device(dir.path(), &gateway, LAPTOP);
    let desktop_settings = move || desktop_env.settings();
    start_session(&desktop, &desktop_cloud, &desktop_settings, &start(SESSION_8))
        .await
        .expect("started on the other device");
    let id = only(&desktop).id;
    asleep_on_the_laptop(&id, &desktop_cloud, &laptop, &laptop_cloud).await;
    let body = said(1).to_string();
    gateway.store_history(&id, SESSION_8, &[(1, 1, Some(body.as_str()))]);

    // Off: Gateway answers nothing, and the machine is asked as before: asleep, it has nothing to say.
    gateway.set_history_enabled(false);
    let _ = laptop.transcript(&id, SESSION_8, false);
    until("Gateway's answer", || {
        laptop.history(&id, SESSION_8) == Some(HistoryRead::Off)
    })
    .await;
    until("the machine's answer", || {
        laptop.transcript(&id, SESSION_8, false).is_some()
    })
    .await;
    assert_eq!(
        laptop
            .transcript(&id, SESSION_8, false)
            .and_then(Result::err)
            .as_deref(),
        Some(nexus_cloud::desktop::ASLEEP)
    );

    // Failing (a side chat on the same machine, `t_side`): the session says so; Try again reads it once Gateway is
    // back.
    gateway.set_history_enabled(true);
    gateway.store_history(&id, "t_side", &[(1, 1, Some(body.as_str()))]);
    gateway.refuse(Some((503, "overloaded")));
    let _ = laptop.transcript(&id, "t_side", false);
    until("the failure", || {
        matches!(laptop.history(&id, "t_side"), Some(HistoryRead::Failed(_)))
    })
    .await;
    gateway.refuse(None);
    laptop.read_history_again(&id, "t_side");
    assert_eq!(laptop.history(&id, "t_side"), None);
    let _ = laptop.transcript(&id, "t_side", false);
    until(
        "Gateway's page",
        || matches!(laptop.transcript(&id, "t_side", false), Some(Ok(read)) if read.last == 1),
    )
    .await;
    assert_eq!(laptop.history(&id, "t_side"), Some(HistoryRead::Kept));
}

#[test]
fn a_start_names_each_step_and_the_total() {
    let mut clock = StartClock::new("t_abc");
    clock.lap("create");
    clock.lap("machine");
    clock.lap("project-clone");
    clock.lap("session");
    let line = clock.line();
    assert!(line.starts_with("cloud start t_abc: create "), "{line}");
    for step in ["machine ", "project-clone ", "session "] {
        assert!(line.contains(step), "{line}");
    }
    assert!(line.contains("; first message in "), "{line}");
}
