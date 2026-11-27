//! The app's tools for the agents of a person's own server, served by this binary, which a server has in
//! place of the desktop app's. `nexus-host --mcp` is the built-in `nexus` server: Gateway's tools as the server's
//! sign-in, and the pull request tools and `request_secret` relayed to the app through the host's socket
//! (`app_relay`). `--browser-mcp` relays the session's browser tools there too, and `--mcp-launch=<name>` starts one
//! of a run's servers for an agent that takes them in its session. Each is `nexus_workbench`'s, as the desktop app runs
//! them, so the host stays a static binary with no keychain or GUI code.

use nexus_workbench::mcp_bridge::{app_socket_env, gemini_schema, relay_to_app, serve_lines};
use nexus_workbench::mcp_servers::{ANTIGRAVITY_CLIENT, CLIENT_VAR, SERVE_ARG, TOKEN_VAR, URL_VAR};
use serde_json::Value;

/// Serves one of the app's tools when `args` start one of them, and answers its exit code; `None` otherwise.
pub fn serve(args: &[String]) -> Option<i32> {
    if args.iter().any(|arg| arg == SERVE_ARG) {
        return Some(builtin());
    }
    nexus_workbench::mcp_bridge::serve_browser().or_else(nexus_workbench::mcp_launch::serve)
}

/// The built-in `nexus` server on stdin and stdout, as the desktop app's `--mcp` serves it.
fn builtin() -> i32 {
    let (Ok(url), Ok(token)) = (std::env::var(URL_VAR), std::env::var(TOKEN_VAR)) else {
        eprintln!("nexus-host {SERVE_ARG}: start it from a Nexus session ({URL_VAR} and {TOKEN_VAR}).");
        return 1;
    };
    nexus_client::install_tls();
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return 1;
    };
    let version = crate::VERSION;
    let gateway = |api_key: Option<String>, headers: Vec<(String, String)>| {
        nexus_client::Client::new(nexus_client::ClientOptions {
            headers,
            client: Some(format!("host-mcp/{version}")),
            ..crate::gateway::client_options(&url, api_key)
        })
    };
    let plain = |api_key: Option<String>, method: nexus_client::Method, path: &str, body: Option<Value>| {
        runtime.block_on(gateway(api_key, Vec::new()).call(method, path, body))
    };
    // Connector changes go as this session's own agent link, as from the desktop app's built-in server.
    let label = match std::env::current_dir()
        .ok()
        .and_then(|dir| dir.file_name().map(|name| name.to_string_lossy().to_string()))
    {
        Some(folder) => format!("Nexus server session in {folder}"),
        None => "Nexus server session".to_string(),
    };
    let session = nexus_client::mcp::SessionLink::new(Some(token.clone()), label);
    let unfenced = |method: nexus_client::Method, path: &str, headers: Vec<(String, String)>, body: Option<Value>| {
        let with_headers = |api_key: Option<String>, method: nexus_client::Method, path: &str, body: Option<Value>| {
            runtime.block_on(gateway(api_key, headers.clone()).call(method, path, body))
        };
        session.call(&plain, &with_headers, method, path, body)
    };
    let fence = nexus_client::fence::Fence::from_env(|name| std::env::var(name).ok());
    let send = nexus_client::mcp::fenced(fence.as_ref(), &unfenced);
    // Set when the host gave the run its socket. A loopback address only.
    let relay = app_socket_env().map(|(address, token)| move |message: &Value| relay_to_app(&address, &token, message));
    let relay = relay.as_ref().map(|relay| relay as &dyn Fn(&Value) -> Value);
    let mut tools = nexus_client::mcp::tools();
    // Antigravity's Gemini models take one type per property, or refuse the whole request.
    if std::env::var(CLIENT_VAR).is_ok_and(|client| client == ANTIGRAVITY_CLIENT) {
        for tool in &mut tools {
            gemini_schema(&mut tool.input_schema);
        }
    }
    serve_lines(
        std::io::stdin().lock(),
        &mut |line| println!("{line}"),
        &|message| nexus_client::mcp::handle(&tools, version, message, &send),
        relay,
    );
    session.end(&plain);
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_arguments_start_none_of_the_tools() {
        assert_eq!(serve(&["serve".to_string()]), None);
        assert_eq!(serve(&["status".to_string(), "--json".to_string()]), None);
    }
}
