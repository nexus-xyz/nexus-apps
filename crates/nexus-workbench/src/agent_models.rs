//! The installed agents' model catalogs. Discovery sends no user prompt or inference request.

use std::path::Path;

use crate::api::AgentInfo;
pub use nexus_local_agents::models::Model;

pub async fn discover(agent: &AgentInfo, path: &str) -> Option<Vec<Model>> {
    let program = Path::new(agent.path.as_deref()?);
    nexus_local_agents::agents::local(agent.agent.coding()?)
        .models(program, path, agent.version.as_deref(), None)
        .await
}
