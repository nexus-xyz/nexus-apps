use nexus_agent_memory::tools::{Session, definitions};
use nexus_client::mcp::{LocalTool, tool_result};
use serde_json::json;

pub fn local_tools(session: &Session) -> Vec<LocalTool<'_>> {
    definitions()
        .into_iter()
        .map(|definition| {
            let name = definition["name"].as_str().expect("tool name").to_string();
            LocalTool {
                name: name.clone(),
                description: definition["description"]
                    .as_str()
                    .expect("tool description")
                    .to_string(),
                input_schema: definition["inputSchema"].clone(),
                read_only: definition["annotations"]["readOnlyHint"] == true,
                call: Box::new(move |arguments| match session.call(&name, arguments) {
                    Ok(result) => tool_result(&result, false),
                    Err(message) => tool_result(&json!({"message": message}), true),
                }),
            }
        })
        .collect()
}
