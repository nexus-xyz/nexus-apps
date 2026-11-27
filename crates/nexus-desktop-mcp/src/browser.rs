//! The agent's browser server, `nexus_browser`: its tools, how a call names a tab, the JSON-RPC answers, and the stdio
//! process that relays each message to the app's socket ([`crate::app_socket`]). The app does the work: Nexus GPUI
//! on CEF (`desktop-gpui/src/side/browser/agent.rs`).
//!
//! The server is the app's own binary started with [`ARG`]. Its credential is one per agent run, and reaches it only
//! through its environment ([`ADDRESS`], [`TOKEN`]). Its relayed calls carry no `tools`, which sends them to the
//! session's browser.

use std::future::Future;

use serde_json::{Value, json};

/// The server's launch for one run (`configuration`), its flag, variables, name and limits: `nexus_workbench`'s, which
/// the background service (nexus-host) gives its agents too.
pub use nexus_workbench::mcp_servers::{
    BROWSER as NAME, BROWSER_ADDRESS_VAR as ADDRESS, BROWSER_ARG as ARG, BROWSER_TOKEN_VAR as TOKEN,
    BROWSER_TOOL_TIMEOUT_SEC as TOOL_TIMEOUT_SEC, BROWSER_WAIT as WAIT, browser as configuration,
};

/// The tools, as `tools/list` gives them. Every page tool names a tab (from `browser_tabs`); without one, the tab on
/// screen.
pub fn tool_definitions() -> Value {
    let tab = json!({"type":"string","maxLength":64,"description":"The tab's id from browser_tabs. Default: this session's active agent tab, or its visible tab before first use."});
    let only_tab = json!({"type":"object","properties":{"tab":tab},"additionalProperties":false});
    let selector = json!({"type":"object","properties":{"selector":{"type":"string","maxLength":2000},"tab":tab},"required":["selector"],"additionalProperties":false});
    json!([
        {"name":"browser_open","description":"Open an HTTP or HTTPS website or local server in this session. On first use, Nexus opens Browser and waits for the person's Allow control or Trust all tabs choice, with a desktop notification when they are away. Authorized tabs work while hidden. Reusing the current address preserves its page state.","inputSchema":{"type":"object","properties":{"url":{"type":"string"},"tab":tab},"required":["url"],"additionalProperties":false}},
        {"name":"browser_read","description":"Read the authorized browser page, field labels, and selectors, even while its pane is hidden. Waits for navigation to finish. Use browser_open first for a new website. Do not substitute desktop capture if access is declined.","inputSchema":only_tab},
        {"name":"browser_click","description":"Click an element by a selector returned by browser_read.","inputSchema":selector},
        {"name":"browser_type","description":"Fill an enabled text field, or choose a select option by its exact value or visible label from browser_read. Focuses the field and dispatches input and change events.","inputSchema":{"type":"object","properties":{"selector":{"type":"string","maxLength":2000},"text":{"type":"string","maxLength":16000},"tab":tab},"required":["selector","text"],"additionalProperties":false}},
        {"name":"browser_screenshot","description":"Capture only the authorized tab's webpage viewport, including when its pane is hidden. Never capture the desktop or the Nexus window as a substitute. Report a closed-tab or authorization error to the person.", "inputSchema":only_tab},
        {"name":"browser_console","description":"Read the preview's latest console messages and errors.","inputSchema":only_tab},
        {"name":"browser_back","description":"Go back in this tab's native browser history, preserving saved page state.","inputSchema":only_tab},
        {"name":"browser_forward","description":"Go forward in this tab's native browser history.","inputSchema":only_tab},
        {"name":"browser_reload","description":"Reload this tab from the website. Use this when fresh content is needed.","inputSchema":only_tab},
        {"name":"browser_tabs","description":"List the session browser's tabs: each tab's id, address, and whether it is on screen and allows agent control.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"browser_new_tab","description":"Open an HTTP or HTTPS address in a new authorized session tab. Keep it in the background when the pane is hidden. Returns the tab's id. The source tab must allow agent control.","inputSchema":{"type":"object","properties":{"url":{"type":"string"}},"required":["url"],"additionalProperties":false}},
        nexus_workbench::browser_routing::bypass_definition()
    ])
}

/// The browser action a page tool runs, and the tab it names (else the active session tab).
pub fn route(name: &str, args: &Value) -> Result<(&'static str, Option<String>), String> {
    let action = match name {
        "browser_open" => "navigate",
        "browser_read" => "read",
        "browser_click" => "click",
        "browser_type" => "type",
        "browser_screenshot" => "screenshot",
        "browser_console" => "info",
        "browser_back" => "back",
        "browser_forward" => "forward",
        "browser_reload" => "reload",
        _ => return Err("Unknown browser tool.".into()),
    };
    let tab = match &args["tab"] {
        Value::Null => None,
        Value::String(tab) if !tab.is_empty() && tab.len() <= 64 => Some(tab.clone()),
        _ => return Err("Name a tab by its id from browser_tabs.".into()),
    };
    Ok((action, tab))
}

/// Whether an action moves the page: it does not wait for the page to finish loading first, and waits after.
pub fn moves(action: &str) -> bool {
    matches!(action, "navigate" | "back" | "forward" | "reload")
}

/// The protocol version to answer: the one asked for, when the server speaks it, else the newest.
pub fn protocol(asked: &Value) -> &str {
    match asked.as_str() {
        Some("2025-11-25") => "2025-11-25",
        Some("2025-06-18") => "2025-06-18",
        Some("2025-03-26") => "2025-03-26",
        Some("2024-11-05") => "2024-11-05",
        _ => "2025-11-25",
    }
}

/// A tool's result as MCP text content.
pub fn content(value: Value, failed: bool) -> Value {
    json!({"content":[{"type":"text","text":value.to_string()}],"isError":failed})
}

/// The answer to one JSON-RPC message: `initialize`, `ping`, `tools/list`, or a `tools/call` that `perform` runs
/// (with the tool's name and arguments). A screenshot comes back as image content. `None` for a notification.
pub async fn handle<F, Fut>(message: &Value, version: &str, perform: F) -> Option<Value>
where
    F: FnOnce(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let id = message.get("id")?.clone();
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => {
            json!({"protocolVersion": protocol(&message["params"]["protocolVersion"]), "capabilities":{"tools":{}},"serverInfo":{"name":NAME,"version":version}})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":tool_definitions()}),
        "tools/call" => {
            let params = &message["params"];
            let name = params["name"].as_str().unwrap_or_default().to_string();
            let screenshot = name == "browser_screenshot";
            // A bypass touches no browser: the workbench reads it from the call and shows it in the conversation.
            let answer = if name == nexus_workbench::browser_routing::BYPASS_TOOL {
                nexus_workbench::browser_routing::bypass_answer(&params["arguments"])
            } else {
                perform(name, params["arguments"].clone()).await
            };
            match answer {
                Ok(value) if screenshot => {
                    let mut content = vec![json!({"type":"image","data":value["data"],"mimeType":value["mediaType"]})];
                    // The captured tab as text too: the app's Follow agent finds a call's tab in its result's text.
                    if let Some(tab) = value["tab"].as_str() {
                        content.push(json!({"type":"text","text":json!({"tab":tab}).to_string()}));
                    }
                    json!({"content":content})
                }
                Ok(value) => content(value, false),
                Err(error) => content(json!({"error":error}), true),
            }
        }
        _ => return Some(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})),
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

/// The answer to a call whose credential the app refused.
pub fn refused(message: &Value, error: &str) -> Value {
    json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32001,"message":error}})
}

/// `--browser-mcp`: relays each bounded message on stdin, with the credential from the environment, to the app's
/// socket, and prints its answer. `None` when not started with [`ARG`]. `nexus_workbench`'s, which `nexus-host` serves
/// on a person's own server too.
pub use nexus_workbench::mcp_bridge::serve_browser as serve;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_tools_name_a_tab_or_use_the_tab_on_screen() {
        assert_eq!(route("browser_read", &json!({})).expect("read"), ("read", None));
        assert_eq!(
            route("browser_click", &json!({"selector":"#go","tab":"t2"})).expect("click"),
            ("click", Some("t2".into()))
        );
        assert_eq!(
            route("browser_console", &json!({"tab":"t1"})).expect("console").0,
            "info"
        );
        for tab in [json!(""), json!(3), json!("x".repeat(65))] {
            assert!(route("browser_read", &json!({"tab":tab})).is_err());
        }
        assert!(route("browser_tabs", &json!({})).is_err());
        let tools = tool_definitions();
        let tools = tools.as_array().expect("tools");
        let names: Vec<&str> = tools.iter().filter_map(|tool| tool["name"].as_str()).collect();
        assert!(names.contains(&"browser_tabs") && names.contains(&"browser_new_tab"));
        for tool in tools {
            let name = tool["name"].as_str().expect("name");
            if route(name, &json!({})).is_ok() {
                assert_eq!(tool["inputSchema"]["properties"]["tab"]["type"], "string", "{name}");
            }
        }
        assert!(moves("navigate") && moves("reload") && !moves("read"));
    }

    #[test]
    fn the_server_is_the_persons_when_settings_names_it() {
        let (server, values) = configuration(&[], "/app", "127.0.0.1:4000", "tok").expect("the browser server");
        assert_eq!(server.args, [ARG]);
        assert_eq!(server.tool_timeout_sec, Some(TOOL_TIMEOUT_SEC));
        assert_eq!(values[NAME][TOKEN], "tok");
        assert!(configuration(&[server], "/app", "a", "t").is_none());
    }

    #[tokio::test]
    async fn answers_the_protocol_and_shows_a_screenshot_as_an_image() {
        let init = handle(
            &json!({"id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
            "1.0",
            |_, _| async { Ok(json!(null)) },
        )
        .await
        .expect("an answer");
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(init["result"]["serverInfo"]["name"], NAME);
        let list = handle(&json!({"id":2,"method":"tools/list"}), "1.0", |_, _| async {
            Ok(json!(null))
        })
        .await
        .expect("an answer");
        assert_eq!(list["result"]["tools"].as_array().map(Vec::len), Some(12));
        let shot = handle(
            &json!({"id":3,"method":"tools/call","params":{"name":"browser_screenshot","arguments":{}}}),
            "1.0",
            |_, _| async { Ok(json!({"mediaType":"image/png","data":"AAAA","name":"preview.png"})) },
        )
        .await
        .expect("an answer");
        assert_eq!(shot["result"]["content"][0]["type"], "image");
        assert_eq!(shot["result"]["content"][0]["mimeType"], "image/png");
        assert_eq!(shot["result"]["content"].as_array().map(Vec::len), Some(1));
        let tabbed = handle(
            &json!({"id":3,"method":"tools/call","params":{"name":"browser_screenshot","arguments":{}}}),
            "1.0",
            |_, _| async { Ok(json!({"mediaType":"image/png","data":"AAAA","tab":"tab-a-1"})) },
        )
        .await
        .expect("an answer");
        assert_eq!(tabbed["result"]["content"][0]["type"], "image");
        assert_eq!(
            tabbed["result"]["content"][1],
            json!({"type":"text","text":"{\"tab\":\"tab-a-1\"}"})
        );
        let failed = handle(
            &json!({"id":4,"method":"tools/call","params":{"name":"browser_read","arguments":{}}}),
            "1.0",
            |name, _| async move { Err(format!("no {name}")) },
        )
        .await
        .expect("an answer");
        assert_eq!(failed["result"]["isError"], true);
        assert!(
            failed["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("no browser_read"))
        );
        // A bypass is answered here, without the browser; a bad one is refused.
        async fn bypass(arguments: Value) -> Option<Value> {
            let message =
                json!({"id":5,"method":"tools/call","params":{"name":"browser_bypass","arguments":arguments}});
            handle(&message, "1.0", |_, _| async {
                Err("the browser was called".to_string())
            })
            .await
        }
        let recorded =
            bypass(json!({"capability":"network log","reason":"The step reads the failed request's headers."}))
                .await
                .expect("an answer");
        assert_eq!(recorded["result"]["isError"], false);
        let refused = bypass(json!({"capability":"network log"})).await.expect("an answer");
        assert_eq!(refused["result"]["isError"], true);
        assert!(
            tool_definitions()
                .as_array()
                .is_some_and(|tools| tools.iter().any(|t| t["name"] == "browser_bypass"))
        );
        assert!(
            handle(&json!({"method":"notifications/initialized"}), "1.0", |_, _| async {
                Ok(json!(null))
            })
            .await
            .is_none()
        );
        let unknown = handle(&json!({"id":5,"method":"resources/list"}), "1.0", |_, _| async {
            Ok(json!(null))
        })
        .await
        .expect("an answer");
        assert_eq!(unknown["error"]["code"], -32601);
    }
}
