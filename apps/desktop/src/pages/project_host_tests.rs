//! A server project's settings against the stand-in host (`machines/tests.rs`): read and saved there, not here.

use nexus_workbench::api::PROTOCOL;

use super::*;
use crate::machines::tests::{Fake, machines, settings, until};

fn script(id: &str, name: &str, command: &str) -> ProjectScript {
    ProjectScript {
        id: id.into(),
        name: name.into(),
        command: command.into(),
        shortcut: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_projects_settings_are_read_and_saved_on_its_host() {
    let fake = Fake::new(PROTOCOL);
    let machines = machines(&fake);
    machines.sync(&settings(&[("m1", "me@box")]));
    until("the overview", || !machines.projects().is_empty()).await;
    let host = ProjectHost::Server {
        machines: machines.clone(),
        machine: "m1".into(),
    };

    let list = vec![script("c-1", "Test", "npm test"), script("c-2", "Dev", "npm run dev")];
    let saved = host.set_scripts("p1".into(), list.clone()).await;
    assert_eq!(saved, Ok(list.clone()));
    assert_eq!(
        fake.host.lock().map(|host| host.scripts.clone()).ok(),
        Some(json!(list))
    );
    let read: Result<Vec<ProjectScript>, String> =
        on_server(&machines, "m1", "wb_project_scripts", json!({ "projectId": "p1" })).await;
    assert_eq!(read, Ok(list));

    assert_eq!(
        host.set_setup("p1".into(), "  npm ci ".into()).await,
        Ok(Some("npm ci".into()))
    );
    assert_eq!(
        on_server::<Option<String>>(&machines, "m1", "wb_project_setup", json!({ "projectId": "p1" })).await,
        Ok(Some("npm ci".into()))
    );
    // A call the host does not know says to update it, in plain words.
    assert_eq!(
        host.update("p1".into(), Some(true), None).await,
        Err(OLDER_HOST.to_string())
    );
}
