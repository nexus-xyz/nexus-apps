# nexus-client

`nexus-client` is the Rust client core for Nexus Gateway.
Native Nexus clients (such as Nexus Desktop and the `nexus` CLI) use it to talk to the Nexus API.

The crate handles transport, sign-in, configuration, streaming, and end-to-end encryption.
It performs no terminal I/O and leaves presentation to the caller.

## Usage

```rust
use nexus_client::{Client, ClientOptions, config::GATEWAY_URL};

# async fn example(api_key: String) -> nexus_client::Result<()> {
let mut options = ClientOptions::new(GATEWAY_URL);
options.api_key = Some(api_key);
let client = Client::new(options);

let me = client.get("/v1/tenants/me").await?;
println!("{me}");
# Ok(())
# }
```

## Features

Default features are empty and enable the async `Client` over `reqwest` and `tokio`.

| Feature | Purpose |
| --- | --- |
| `blocking` | Enables `blocking::Client`, a synchronous mirror of `Client`. |
| `mcp` | Enables the `mcp` and `mcp_ask` modules for the local Nexus MCP stdio server. |

## Modules

- `Client`, `ClientOptions`, `Exchange`, `Answer`, `Observer`: async HTTP client with connection pooling, retries, and idempotency keys.
- `blocking` (`blocking` feature): synchronous HTTP client.
- `sse`: server-sent events decoder for streaming chat responses.
- `device`: device-code sign-in flow (`start`, `poll`).
- `config`: saved client config (`~/.nexus/factory/config.json`) and enterprise managed config.
- `e2e`: end-to-end encryption for synced sessions (AES-256-GCM, HPKE, Ed25519).
- `fence`: session runner leases and fencing tokens.
- `agent_entries`, `agents`, `agent_setup`, `local_agents`, `local_sessions`, `auto_route`: coding agent catalog, local detection, launch configuration, and local session discovery.
- `session_share`, `commit_trailer`, `machine_key`, `qr`, `url`, `base`, `error`, `staff`: shared client helpers.
- `mcp`, `mcp_ask` (`mcp` feature): JSON-RPC stdio MCP server core backed by the bundled API contract in `contract/`.

## License

MIT.
