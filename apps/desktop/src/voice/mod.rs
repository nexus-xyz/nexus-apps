//! Voice mode in the Assistant, as the web builds it
//! (the web kit's `voice-call.ts`, `voice-ears.ts`, `voice-motion.ts`, and the web app's voice components).
//!
//! A hands-free call with Nexus. A voice button before the composer's send circle opens the pane (`pages/agent.rs`).
//! Each turn the person speaks is a normal ask in the Assistant's conversation; Nexus says the answer with the Mac's
//! own voice. The person can talk over it, by voice or with Space, mute, type, or end the call. Esc or End leaves voice:
//! the summary card lands in the thread, and each write the call asked for lands after it as its approval card. Voice
//! never approves a write.
//!
//! - `call.rs`: the call's ten states and its turns, without GPUI.
//! - `spoken.rs`: what Nexus says for an answer, the echo filter, and the end-of-call card.
//! - `motion.rs`: the mark's motion maths. `mark.rs` draws it.
//! - `ears.rs`: the microphone and the speech service, dictation's path. `ask.rs`: a turn's ask.
//! - `speaker.rs`: the system voice, macOS only.
//! - `pane.rs`: the pane, which runs the call. `summary.rs`: the card in the thread.

mod ask;
pub mod call;
pub mod ears;
mod mark;
pub mod motion;
pub mod pane;
pub mod speaker;
pub mod spoken;
mod summary;

pub use ears::Hearing;
pub use pane::{BackendSenses, VoiceEvent, VoicePane};
pub use spoken::VoiceSummary;
pub use summary::card as summary_card;
