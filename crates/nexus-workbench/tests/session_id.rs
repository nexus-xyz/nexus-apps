mod support;

use std::path::Path;

use nexus_workbench::event::{PermissionMode, SessionAgent, Via};
use nexus_workbench::store::Store;
use nexus_workbench::{Config, Workbench};
use serde_json::Value;

use support::{TestHost, new_root, until};

async fn recorded_session_id(path: &Path) -> String {
    let read = || {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    };
    until("the agent to record its launch", || read().is_some()).await;
    read().expect("launch record")["env"]["NEXUS_SESSION_ID"]
        .as_str()
        .expect("NEXUS_SESSION_ID in the agent process")
        .to_owned()
}

async fn check_session_ids(agent: SessionAgent, via: Via, mode: PermissionMode) {
    let dir = std::env::temp_dir().join(format!("wb-session-id-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("test directory");
    let host = std::sync::Arc::new(TestHost {
        memory: dir.clone(),
        ..TestHost::default()
    });
    let workbench = Workbench::new(
        Store::in_memory().expect("store"),
        host.clone(),
        Config {
            scope: "gw|org".into(),
            instructions: Some(dir.join("instructions")),
            ..Config::default()
        },
    )
    .expect("workbench");
    let project = workbench.add_project(&dir).expect("project");
    let mut ids = Vec::new();
    for index in 0..2 {
        let path = dir.join(format!("launch-{index}.json"));
        host.extra.lock().expect("launch overrides").extend([
            ("FAKE_LAUNCH_RECORD".into(), path.to_string_lossy().into_owned()),
            ("NEXUS_SESSION_ID".into(), "t_inherited".into()),
        ]);
        let mut request = new_root(&project.id, "hello");
        request.agent = agent;
        request.via = via;
        request.permission_mode = mode;
        request.orchestration = false;
        let session = workbench.create_thread(request).await.expect("start session");
        assert!(session.id.starts_with("t_"));
        assert_eq!(recorded_session_id(&path).await, session.id);
        ids.push(session.id);
    }
    assert_ne!(ids[0], ids[1]);
    workbench.shutdown().await;
    std::fs::remove_dir_all(dir).expect("remove test directory");
}

#[tokio::test]
async fn session_id_reaches_claude_code() {
    check_session_ids(SessionAgent::Claude, Via::Nexus, PermissionMode::Default).await;
}

#[tokio::test]
async fn session_id_reaches_codex() {
    check_session_ids(SessionAgent::Codex, Via::Nexus, PermissionMode::Default).await;
}

#[tokio::test]
async fn session_id_reaches_antigravity() {
    check_session_ids(SessionAgent::Antigravity, Via::Nexus, PermissionMode::Default).await;
}

#[tokio::test]
async fn session_id_reaches_nexus_agent() {
    check_session_ids(SessionAgent::Nexus, Via::Nexus, PermissionMode::Default).await;
}

#[cfg(feature = "cursor")]
#[tokio::test]
async fn session_id_reaches_cursor() {
    check_session_ids(SessionAgent::Cursor, Via::Subscription, PermissionMode::AcceptEdits).await;
}

#[cfg(feature = "opencode")]
#[tokio::test]
async fn session_id_reaches_opencode() {
    check_session_ids(SessionAgent::Opencode, Via::Nexus, PermissionMode::Default).await;
}
