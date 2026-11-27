//! First turns from the signed-out window, through the same two owners as the app.

use std::os::unix::fs::PermissionsExt;
use std::rc::Rc;

use nexus_providers::{KeySource, KeyVendor};
const LOCAL_SCOPE: &str = "|";

use crate::workspace::first_connection_tests::{
    Connection, check_concurrent_setup, check_dismissal, check_sign_in_route, start_first_turn,
    start_first_turn_from_added_projects,
};

use super::*;

#[test]
#[ignore = "needs NEXUS_HOST_BIN, NEXUS_FAKE_CLAUDE_BIN, NEXUS_FAKE_CODEX_BIN and a test Secret Service"]
fn signed_out_first_connection_finishes_each_local_turn() {
    const CHILD: &str = "NEXUS_TEST_FIRST_CONNECTION";
    if std::env::var_os(CHILD).is_none() {
        // The CI service step has no desktop keyring and only builds fake-claude.
        let keyed_fixtures = std::env::var_os("NEXUS_FAKE_CODEX_BIN").is_some();
        let stage = root("first-connection-stage");
        let fake = stage.join("opencode.py");
        std::fs::write(&fake, SAVED_SERVER_AGENT).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        for owner in ["gpui", "service"] {
            for mode in [
                "subscription",
                "project-add",
                "anthropic",
                "openai",
                "server",
                "keyed-server",
                "removed-server",
                "concurrent-setup",
                "dismiss",
                "unavailable-at-spawn",
                "cloud",
                "flag-off",
            ] {
                if owner == "service"
                    && matches!(
                        mode,
                        "removed-server" | "concurrent-setup" | "dismiss" | "cloud" | "flag-off"
                    )
                {
                    continue;
                }
                if !keyed_fixtures && matches!(mode, "anthropic" | "openai" | "keyed-server" | "concurrent-setup") {
                    continue;
                }
                let profile = stage.join(format!("{owner}-{mode}"));
                std::fs::create_dir_all(profile.join("home")).unwrap();
                let mut command = process::command(std::env::current_exe().unwrap());
                if keyed_fixtures {
                    command.env("NEXUS_CODEX_BIN", built("NEXUS_FAKE_CODEX_BIN"));
                }
                let output = command
                    .args([
                        "--ignored",
                        "--exact",
                        "code_run::tests::service::first_connection::signed_out_first_connection_finishes_each_local_turn",
                    ])
                    .env(CHILD, format!("{owner}:{mode}"))
                    .env("NEXUS_BYO_PROVIDERS", if mode == "flag-off" { "0" } else { "1" })
                    .env("HOME", profile.join("home"))
                    .env("XDG_CONFIG_HOME", profile.join("config"))
                    .env("XDG_DATA_HOME", profile.join("data"))
                    .env("NEXUS_DESKTOP_KEYCHAIN_SERVICE", format!("nexus-first-connection-{}-{owner}-{mode}", std::process::id()))
                    .env("NEXUS_CLAUDE_BIN", built("NEXUS_FAKE_CLAUDE_BIN"))
                    .env("NEXUS_OPENCODE_BIN", &fake)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
                    "{owner} {mode}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                );
            }
        }
        return;
    }

    let selection = std::env::var(CHILD).unwrap();
    let (owner, mode) = selection.split_once(':').unwrap();
    let data = root("first-connection");
    let repo = data.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let profile = nexus_desktop_core::settings::config_dir().unwrap();

    let gateway_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    gateway_listener.set_nonblocking(true).unwrap();
    let gateway_url = format!("http://{}", gateway_listener.local_addr().unwrap());
    let gateway = gateway_url.as_str();
    let assert_no_gateway = || {
        assert_eq!(
            gateway_listener
                .accept()
                .expect_err("Local connected to Gateway")
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
    };
    let local_providers_opt_in = crate::local_providers_opt_in::from_env();
    let mut local = stand_in(&data, gateway, LOCAL_SCOPE);
    local.sign_in = Arc::new(|| Some(json!({"localMcp": {"servers": [], "values": {}}, "accounts": []})));
    local.app_program = Some(built("NEXUS_HOST_BIN"));
    let session_data = crate::paths::data_dir().expect("isolated desktop data dir");
    local.data = session_data.clone();
    let (runner, mut events, _stop) = if owner == "service" {
        let service = local.clone();
        let (runner, events) = CodeRunner::start_with(
            session_data.clone(),
            gateway,
            LOCAL_SCOPE,
            Loops {
                local_providers_opt_in,
                ..switch(Arc::new(|| true))
            },
            |_| Ok(service),
        )
        .unwrap()
        .unwrap();
        (runner, events, Some(StopService(local.clone())))
    } else {
        let (runner, events) = CodeRunner::start_in(
            session_data.clone(),
            gateway,
            LOCAL_SCOPE,
            Loops {
                local_providers_opt_in,
                ..quick(Duration::from_secs(600))
            },
        )
        .unwrap()
        .unwrap();
        (runner, events, None)
    };
    let runner = Rc::new(runner);
    if matches!(mode, "cloud" | "flag-off") {
        check_sign_in_route(runner.clone(), gateway, mode == "cloud");
        runner.shutdown();
        return;
    }
    if mode == "concurrent-setup" {
        check_concurrent_setup(runner.clone(), gateway, &profile);
        assert_no_gateway();
        runner.shutdown();
        return;
    }
    if mode == "dismiss" {
        check_dismissal(runner.clone(), gateway);
        assert_no_gateway();
        runner.shutdown();
        return;
    }
    let project = if mode == "project-add" {
        None
    } else {
        runner.add_project(repo.clone(), None);
        let project = next(&mut events, &local, |event| match event {
            CodeEvent::ProjectAdded { id, .. } => Some(id),
            _ => None,
        });
        if mode == "openai" {
            let defaults = nexus_workbench::project_defaults::ProjectDefaults {
                agent: Some(SessionAgent::Codex),
                ..Default::default()
            };
            let saved = runner.projects().set_defaults(project.clone(), defaults.clone());
            assert_eq!(futures::executor::block_on(saved).unwrap().unwrap(), defaults);
        }
        Some(project)
    };
    let (connection, model_turn) = match mode {
        "subscription" | "project-add" => (Connection::Subscription, None),
        "anthropic" => (Connection::Key(KeyVendor::Anthropic), None),
        "openai" => (Connection::Key(KeyVendor::Openai), None),
        "removed-server" => (Connection::RemovedServer, None),
        "unavailable-at-spawn" => (
            Connection::Server {
                address: format!("{gateway}/v1"),
                keyed: false,
            },
            None,
        ),
        "server" | "keyed-server" => {
            let keyed = mode == "keyed-server";
            let (address, turn) = loopback_model_turn(keyed);
            (Connection::Server { address, keyed }, Some(turn))
        }
        _ => panic!("unknown mode {mode}"),
    };
    // The picker and preflight see this settings snapshot. The host independently sees the row
    // missing, as it would after a second window removed the row during launch.
    let picker_profile = if mode == "unavailable-at-spawn" {
        profile.join("picker-snapshot")
    } else {
        profile.clone()
    };
    let events = std::cell::RefCell::new(&mut events);
    let mut id = None;
    let mut first_created = || {
        let created = next_with_timeout(
            &mut events.borrow_mut(),
            &local,
            Duration::from_secs(45),
            |event| match event {
                CodeEvent::Created { .. } => Some(event),
                CodeEvent::StartFailed { error, .. } => panic!("first turn failed: {error}"),
                _ => None,
            },
        );
        if let CodeEvent::Created { id: created_id, .. } = &created {
            id = Some(created_id.clone());
        }
        created
    };
    let saved = if let Some(project) = project {
        start_first_turn(runner.clone(), gateway, &picker_profile, project, connection, || {
            (mode != "unavailable-at-spawn").then(&mut first_created)
        })
    } else {
        let dialog_repo = data.join("dialog-repo");
        std::fs::create_dir(&dialog_repo).unwrap();
        start_first_turn_from_added_projects(
            runner.clone(),
            gateway,
            &repo,
            &dialog_repo,
            || {
                next(&mut events.borrow_mut(), &local, |event| match event {
                    CodeEvent::ProjectAdded { .. } => Some(event),
                    CodeEvent::ProjectAddFailed { error, .. } => {
                        panic!("the selected folder could not be added: {error}")
                    }
                    _ => None,
                })
            },
            first_created,
        );
        None
    };
    if mode == "unavailable-at-spawn" {
        let failed = next_with_timeout(
            &mut events.borrow_mut(),
            &local,
            Duration::from_secs(45),
            |event| match event {
                CodeEvent::Created { .. } => panic!("the host's rejected start was reported as created"),
                CodeEvent::StartFailed { error, .. } => Some(error),
                _ => None,
            },
        );
        assert!(failed.contains("local provider could not start"));
        let store = Store::open(&session_data.join("workbench.sqlite3")).unwrap();
        assert!(store.threads(LOCAL_SCOPE).unwrap().is_empty());
        assert_no_gateway();
        runner.shutdown();
        return;
    }
    if mode == "removed-server" {
        assert!(saved.is_none());
        let store = Store::open(&session_data.join("workbench.sqlite3")).unwrap();
        assert!(store.threads(LOCAL_SCOPE).unwrap().is_empty());
        assert_no_gateway();
        runner.shutdown();
        return;
    }
    let id = id.expect("the workspace handled the Created event");
    assert!(
        wait_for(Duration::from_secs(20), || turn_ends(&session_data, &id) == 1),
        "{}",
        report(&local)
    );
    let store = Store::open(&session_data.join("workbench.sqlite3")).unwrap();
    let row = store.thread(&id).unwrap().unwrap();
    assert_eq!(row.scope, LOCAL_SCOPE);
    assert_eq!(row.summary.provider, saved);
    assert_eq!(
        row.summary.via,
        if saved.is_some() { Via::Nexus } else { Via::Subscription },
    );
    assert!(row.summary.nexus_sessions.is_empty());
    let events = store.events_after(&id, 0).unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, WbEvent::Text { text, .. } if !text.is_empty()))
    );
    assert!(events.iter().any(|event| matches!(
        event.event,
        WbEvent::TurnEnd {
            status: nexus_workbench::event::TurnStatus::Completed,
            ..
        }
    )));
    if let Some(turn) = model_turn {
        assert_eq!(row.summary.model.as_deref(), Some("team/model"));
        turn.join().unwrap();
    }
    assert_no_gateway();
    runner.shutdown();
    let settings = nexus_desktop_core::provider_settings::ProviderSettingsStore::in_config(&profile);
    if let Some(id) = saved {
        settings
            .remove(
                &nexus_desktop_core::provider_keys::OsProviderKeys as &dyn KeySource,
                &id,
            )
            .unwrap();
    }
}
