//! The guard a child's setup reads before it makes a worktree ([`Workbench::start_gone`]): a start that raced a
//! project's removal must not work out of sight.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::event::{ThreadSummary, Usage};
use crate::store::{Store, ThreadRecord};
use crate::{Config, Emit, Host, Launch, LaunchRequest, ProcessEnd, Result, Workbench};

const SCOPE: &str = "gw|org";

/// A host that launches nothing: the guard runs before any launch.
struct Quiet;

impl Host for Quiet {
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, Result<Launch>> {
        Box::pin(async { Err("no launches here".into()) })
    }
    fn ended(&self, _: String, _: ProcessEnd) -> BoxFuture<'static, bool> {
        Box::pin(async { true })
    }
    fn price(&self, _: &str, _: &Usage) -> Option<f64> {
        None
    }
    fn emit(&self, _: Emit) {}
}

fn child(project_id: &str) -> ThreadRecord {
    let summary: ThreadSummary = serde_json::from_value(serde_json::json!({
        "id": "t_child", "projectId": project_id, "title": "Child", "agent": "claude", "model": null,
        "permissionMode": "default", "via": "nexus", "status": "starting", "costUsd": 0.0, "nexusSessions": [],
        "contextTokens": null, "contextWindow": null, "createdAt": 1, "updatedAt": 1, "archived": false,
        "parentThreadId": "t_lead", "depth": 1, "crossProject": true,
    }))
    .expect("a summary");
    ThreadRecord {
        summary,
        native_id: "n".into(),
        native_started: false,
        scope: SCOPE.into(),
        fork_from: None,
        seed: None,
        automatic_title: false,
        delivered_seq: 0,
        halt: None,
    }
}

#[tokio::test]
async fn a_child_archived_or_whose_project_was_removed_does_not_start() {
    let dir = std::env::temp_dir().join(format!("nexus-start-gone-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).expect("a folder");
    let config = Config {
        scope: SCOPE.into(),
        shell: crate::process::Shell::sh(),
        ..Config::default()
    };
    let workbench = Workbench::new(Store::in_memory().expect("a store"), Arc::new(Quiet), config).expect("a workbench");
    let project = workbench.add_project(&dir).expect("a project");
    let mut record = child(&project.id);
    workbench.store().insert_thread(&record).expect("saved");
    assert_eq!(workbench.start_gone("t_child", &project), Ok(None));

    record.summary.archived = true;
    workbench.store().save_thread(&record).expect("saved");
    assert_eq!(
        workbench.start_gone("t_child", &project),
        Ok(Some("This subsession was archived before it started.".into()))
    );

    workbench.remove_project(&project.id).expect("removed");
    let name = project.name.clone();
    assert_eq!(
        workbench.start_gone("t_child", &project),
        Ok(Some(format!(
            "The project {name} was removed from Nexus, so this subsession did not start."
        )))
    );
    std::fs::remove_dir_all(dir).ok();
}
