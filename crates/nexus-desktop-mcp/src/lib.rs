//! Nexus for desktop's agent tools, with no UI toolkit: the MCP servers every Code session's agent gets and the
//! built-in Nexus server's routing ([`mcp`]) and the app's socket it relays to ([`app_socket`]), `request_secret`
//! ([`secret_request`]), and outside access for scripts, the `nexus` CLI and other agents ([`outside`]). The background
//! service's agents reach the same tools through the service ([`service_relay`]). Nexus GPUI uses
//! it.
//! The app's side comes in through each module's trait (`secret_request::Shell`, `outside::Host`) and
//! `nexus_desktop_core`'s seams (the app's version, the `Notifier`).

/// The app's socket: `nexus_workbench`'s, which the background service (nexus-host) serves too.
pub use nexus_workbench::app_socket;
pub mod browser;
/// `--mcp-launch`: one of a run's servers for an agent that takes them in its session.
pub use nexus_workbench::mcp_launch as launch;
pub mod mcp;
pub mod outside;
pub mod secret_request;
pub mod service_relay;
