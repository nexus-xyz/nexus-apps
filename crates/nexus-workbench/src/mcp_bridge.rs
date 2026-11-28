//! The stdio sides of the app's tools that need no UI toolkit, shared by the desktop app's binary and `nexus-host`
//!: the app socket's address from a run's environment, the `nexus_browser` relay loop, and Gemini's form of
//! a tool schema. `nexus-desktop-mcp` re-exports them for the apps; `nexus-host` serves them on a person's own server,
//! which has no desktop binary. Each keeps the host a static binary: nothing here reads a keychain or a GUI.

use std::io::{BufRead, BufReader, Read};
use std::net::SocketAddr;
use std::time::Duration;

use serde_json::{Value, json};

use crate::app_tools::{PULL_WATCH_TOOL_NAMES, SECRET_TOOL_NAME, pull_watch_tool_definitions, secret_tool_definition};
use crate::mcp_servers::{
    APP_ADDRESS_VAR, APP_TOKEN_VAR, BROWSER_ADDRESS_VAR, BROWSER_ARG, BROWSER_TOKEN_VAR, BROWSER_WAIT,
    PULL_REQUEST_TOOLS, REPLY_WAIT, SECRET_TOOLS, SECRET_WAIT,
};

/// The app's socket and the run's credential for it, from the built-in server's environment
/// (`mcp_servers::with_app_tools`). A loopback address only.
pub fn app_socket_env() -> Option<(SocketAddr, String)> {
    socket_env(APP_ADDRESS_VAR, APP_TOKEN_VAR)
}

/// The app's socket and a credential for it, from the two variables named. A loopback address only.
pub fn socket_env(address_var: &str, token_var: &str) -> Option<(SocketAddr, String)> {
    let address = std::env::var(address_var).ok()?;
    let token = std::env::var(token_var).ok()?;
    address
        .parse::<SocketAddr>()
        .ok()
        .filter(|address| address.ip().is_loopback())
        .map(|address| (address, token))
}

/// `--browser-mcp`: relays each bounded message on stdin, with the credential from the environment, to the app's
/// socket, and prints its answer. `None` when not started with [`BROWSER_ARG`].
pub fn serve_browser() -> Option<i32> {
    if !std::env::args().any(|arg| arg == BROWSER_ARG) {
        return None;
    }
    let (Ok(address), Ok(token)) = (std::env::var(BROWSER_ADDRESS_VAR), std::env::var(BROWSER_TOKEN_VAR)) else {
        return Some(1);
    };
    let address: SocketAddr = match address.parse::<SocketAddr>() {
        Ok(address) if address.ip().is_loopback() => address,
        _ => return Some(1),
    };
    let mut input = BufReader::new(std::io::stdin());
    loop {
        let mut line = String::new();
        let count = match (&mut input).take(crate::app_socket::LIMIT + 1).read_line(&mut line) {
            Ok(0) => return Some(0),
            Ok(count) => count,
            Err(_) => return Some(1),
        };
        if count as u64 > crate::app_socket::LIMIT || !line.ends_with('\n') {
            return Some(1);
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            return Some(1);
        };
        let reply = crate::app_socket::relay(
            &address,
            &json!({"token":token,"message":message}),
            BROWSER_WAIT + Duration::from_secs(30),
        );
        if message.get("id").is_none() {
            continue;
        }
        match reply {
            Ok(reply) => println!("{}", reply.trim()),
            Err(_) => println!(
                "{}",
                json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32000,"message":"The session browser is unavailable."}})
            ),
        }
    }
}

/// One message to the built-in server: the pull request tools' and `request_secret`'s calls go to the app through
/// `relay`, which also adds them to `tools/list`; everything else goes to `gateway` (`nexus_client::mcp::handle`).
/// Without `relay` (no app socket), the server is the Gateway's alone.
pub fn route(
    message: &Value,
    gateway: impl FnOnce(&Value) -> Option<Value>,
    relay: Option<&dyn Fn(&Value) -> Value>,
) -> Option<Value> {
    let Some(relay) = relay else {
        return gateway(message);
    };
    let tool = message["params"]["name"].as_str().unwrap_or_default();
    match message["method"].as_str().unwrap_or_default() {
        "tools/call" if PULL_WATCH_TOOL_NAMES.contains(&tool) || tool == SECRET_TOOL_NAME => {
            message.get("id")?;
            Some(relay(message))
        }
        "tools/list" => {
            let mut answer = gateway(message)?;
            if let Some(tools) = answer["result"]["tools"].as_array_mut() {
                tools.extend(pull_watch_tool_definitions());
                tools.push(secret_tool_definition());
            }
            Some(answer)
        }
        _ => gateway(message),
    }
}

/// Relays one call of the pull request tools or `request_secret` to the app's socket ([`app_socket_env`]), and
/// answers it. `request_secret` waits for the person: the app answers within its wait, so it reads a little longer.
pub fn relay_to_app(address: &SocketAddr, token: &str, message: &Value) -> Value {
    let (tools, wait) = if message["params"]["name"] == SECRET_TOOL_NAME {
        (SECRET_TOOLS, SECRET_WAIT + Duration::from_secs(30))
    } else {
        (PULL_REQUEST_TOOLS, REPLY_WAIT)
    };
    let request = json!({"token": token, "tools": tools, "message": message});
    crate::app_socket::relayed(message, crate::app_socket::relay(address, &request, wait))
}

/// The built-in server's loop (`nexus_client::mcp::serve`'s): one JSON-RPC message a line in, its answer a line out,
/// each routed by [`route`]. A line that is not JSON gets a parse error.
pub fn serve_lines(
    input: impl BufRead,
    out: &mut dyn FnMut(&str),
    gateway: &dyn Fn(&Value) -> Option<Value>,
    relay: Option<&dyn Fn(&Value) -> Value>,
) {
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(message) => route(&message, gateway, relay),
            Err(_) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}})),
        };
        if let Some(answer) = answer {
            out(&answer.to_string());
        }
    }
}

/// A tool's input schema in the form Gemini's function calling takes (Antigravity's models): exactly one `type` per
/// property. The contract writes a nullable or mixed property as a type list (`["string", "null"]`), which Gemini
/// refuses, and with it the whole request; an `anyOf` of types is refused the same way. A list becomes its one type,
/// plus `nullable` when it named `null`. A property of several types is served as `string`: the contract's
/// number-or-string amounts (`monthlyUsd`, `amountUsd`) read a decimal string exactly. Everything else stays as it is.
pub fn gemini_schema(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            if let Some(Value::Array(types)) = map.get("type").cloned() {
                let named: Vec<&Value> = types.iter().filter(|t| *t != "null").collect();
                let one = match named.as_slice() {
                    [one] => Some((*one).clone()),
                    [] => None,
                    many => Some(
                        many.iter()
                            .find(|t| **t == "string")
                            .copied()
                            .unwrap_or(many[0])
                            .clone(),
                    ),
                };
                match one {
                    Some(one) => map.insert("type".into(), one),
                    None => map.remove("type"),
                };
                if named.len() < types.len() {
                    map.insert("nullable".into(), Value::Bool(true));
                }
            }
            for value in map.values_mut() {
                gemini_schema(value);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(gemini_schema),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `setPersonBudget`'s body as the contract writes it: a nullable string and a nullable number-or-string.
    #[test]
    fn gemini_schemas_take_one_type_per_property() {
        let mut schema = json!({"type": "object", "properties": {"body": {"type": "object", "properties": {
            "monthlyUsdDecimal": {"type": ["string", "null"], "pattern": "^[0-9]+$"},
            "monthlyUsd": {"type": ["number", "string", "null"]},
            "units": {"type": "array", "items": {"type": "object", "properties": {"level": {"type": ["integer", "null"]}}}},
            "name": {"type": "string"},
        }, "anyOf": [{"required": ["monthlyUsd"]}, {"required": ["monthlyUsdDecimal"]}]}}});
        gemini_schema(&mut schema);
        let body = &schema["properties"]["body"]["properties"];
        assert_eq!(
            body["monthlyUsdDecimal"],
            json!({"type": "string", "nullable": true, "pattern": "^[0-9]+$"})
        );
        // Several types: the string form, which the contract reads exactly (Gemini refuses an `anyOf` of types too).
        assert_eq!(body["monthlyUsd"], json!({"type": "string", "nullable": true}));
        assert_eq!(
            body["units"]["items"]["properties"]["level"],
            json!({"type": "integer", "nullable": true})
        );
        assert_eq!(body["name"], json!({"type": "string"}));
    }
}
