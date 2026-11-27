//! The Nexus API's client core: everything a native client needs to talk to Nexus Gateway, and nothing about how it
//! shows it. The `nexus` CLI (`nexus-cli`) and Nexus Desktop (`desktop-gpui`) use it. The public API stays plain:
//! owned values, `serde_json::Value` bodies passed through as Gateway sends them, no printing, no terminal, no global
//! state besides the TLS provider.
//!
//! - [`Client`]: async (reqwest on tokio), one connection pool kept warm, HTTP/2 where the server offers it; raw
//!   exchanges for a bridge ([`Client::exchange`], [`Client::stream`]) and typed JSON calls.
//! - [`blocking::Client`] (feature `blocking`): the same calls, synchronous, for the CLI.
//! - [`sse`]: the server-sent events decoder and the chat stream's outcome, shared by both.
//! - [`device`]: the device-code sign-in (`nexus auth login`): start, poll, and what each answer means.
//! - [`config`]: the config file (`~/.nexus/factory/config.json`), the managed config (enterprise presets) and links.
//! - [`agent_entries`]: the coding agents, one entry each ([`agent_entries::Agent`]).
//! - [`agents`], [`agent_setup`]: how `nexus claude` / `nexus codex` launch on each route, and what each route needs.
//! - [`commit_trailer`]: the git hook that marks a commit made in a Nexus session with a `Co-authored-by` trailer.
//! - [`staff`]: whether the person signed in is one of Nexus's own people.
//! - [`auto_route`]: Gateway's auto pick for a coding agent's session before it starts.
//! - [`base`]: client-safe copies of Gateway's shared helpers (the CLI's version order, chat image limits).
//! - [`machine_key`]: a cloud machine's own Nexus sign-in, for `nexus mcp` there.
//! - [`session_share`]: a coding-session snapshot cut to Gateway's limits.
//! - [`qr`]: a QR code in a terminal.
//! - [`url`]: paths and query strings encoded as the browser encodes them.
//! - [`local_agents`]: the coding agents on this machine (installed? signed in to the person's plan?), for
//!   `nexus agents` and Nexus for desktop.
//! - [`local_sessions`]: the agents' own conversations on this machine (Claude Code's transcripts, Codex's rollouts),
//!   with the command that resumes each, for Nexus for desktop.
//! - [`error`]: Gateway's errors and transport failures.
//! - [`fence`]: a runner's hold on a native session: the take, the renewals, and the fencing token on connector changes.
//! - [`e2e`]: end-to-end encryption of synced sessions: device keys, per-thread content keys, sealed events.
//! - `mcp` and `mcp_ask` (feature `mcp`): the Nexus MCP server's tools and protocol, for `nexus mcp` and Nexus Desktop.

pub mod agent_entries;
pub mod agent_setup;
pub mod agents;
pub mod auto_route;
pub mod base;
pub mod commit_trailer;
pub mod config;
pub mod device;
pub mod e2e;
pub mod error;
mod etag;
pub mod fence;
pub use nexus_local_agents as local_agents;
pub mod local_sessions;
pub mod machine_key;
pub mod qr;
pub mod session_share;
pub mod sse;
pub mod staff;
pub mod url;

mod client;
pub use client::{Answer, Client, ClientOptions, Exchange, Method, Observer, StatusCode};

#[cfg(feature = "blocking")]
pub mod blocking;
#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "mcp")]
pub mod mcp_ask;

pub use error::{CoreError, Error, Result, Transport};
pub use sse::ChatOutcome;

/// `x-nexus-cli-version`: a CLI says which version it is, and Gateway answers 426 `cli_too_old` below its
/// minimum. Only the CLI sends it; other clients identify themselves with [`CLIENT_HEADER`] alone.
pub const CLI_VERSION_HEADER: &str = "x-nexus-cli-version";
/// `x-nexus-client: <surface>/<version>` (`cli/0.5.0+abc1234`, `desktop/0.5.0+abc1234`): the audit log's surface.
pub const CLIENT_HEADER: &str = "x-nexus-client";

/// The crypto provider rustls uses (ring, as the gateway's sqlx), installed once per process. reqwest panics building
/// a client without one, so every client constructor calls this first.
pub fn install_tls() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
