//! One file per agent: its type, its entry and its [`crate::AgentT`].

mod antigravity;
mod claude;
mod codex;
mod cursor;
mod opencode;

pub use antigravity::Antigravity;
pub use claude::Claude;
pub use codex::Codex;
pub use cursor::Cursor;
pub use opencode::Opencode;
