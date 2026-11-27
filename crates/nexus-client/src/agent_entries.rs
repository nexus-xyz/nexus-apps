//! The coding agents and their entries, under the path clients name them by. These are `nexus_agents` types,
//! re-exported for callers. Per-agent client code dispatches exhaustively on [`Agent`].

pub use nexus_agents::{
    Agent, AgentEntry, Billing, GatewayBinding, Identity, Install, Platform, Profile, Session, SignIn,
};
