//! Adapter boundary for the session-sync runner. These types do not define the Gateway HTTP schema.

use crate::Result;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A verified person in one Gateway and organization. `scope` is the workbench's Gateway/organization scope.
/// Never derive `user` from a token hash, display name, or an unverified request field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub scope: String,
    pub user: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentPolicy {
    NeverLeaves,
    UserOnly,
    UserAndOrgAdmins,
}

/// The errors the engine acts on. A `Backend` returns exactly this text; any other error counts as unavailable.
/// Gateway refused new events sealed with an older key (409 `key_version_behind`) and stored none of them. The engine
/// confirms the head, drops the refused ciphertext, and seals those events again with the key `key` returns next.
pub const KEY_ROTATED: &str = "session sync: the thread's content key rotated";
/// One person's synced threads in the organization hold Gateway's limit (413 `session_store_full`).
pub const STORE_FULL: &str = "session sync: synced sessions are full in this organization; delete some";

/// How Gateway vouches for the person's devices: `GET /v1/devices`'s `trust`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeviceTrust {
    /// Each device is trusted through a chain of signed approvals, and the person confirms its approver by its code.
    /// An absent or unknown `trust` reads as this.
    #[default]
    Approvals,
    /// Gateway vouches for every device the person signed in on: a device it lists as trusted is trusted.
    Gateway,
}

impl DeviceTrust {
    /// The trust in a `GET /v1/devices` answer. Only `"gateway"` reads as [`DeviceTrust::Gateway`].
    pub fn of(answer: &serde_json::Value) -> DeviceTrust {
        match answer.get("trust").and_then(serde_json::Value::as_str) {
            Some("gateway") => DeviceTrust::Gateway,
            _ => DeviceTrust::Approvals,
        }
    }
}

/// Returned from a fresh policy read. Unknown policy or failed reads must return an error, never a default.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub content: ContentPolicy,
}

/// Immutable credentials and a trusted crypto provider for exactly `identity()`. Every request must use that
/// account's credential, even if the host switches accounts during an await. Errors must contain no content.
/// Implement this with the final transport and the reviewed crypto, once their contracts land.
pub trait Backend: Send + Sync {
    fn identity(&self) -> &Identity;
    fn computer(&self) -> Option<&dyn crate::computer::Transport> {
        None
    }
    /// The managed local-cache switch, read locally. False forbids all sync calls, including deletion and retries.
    fn allow_local_cache(&self) -> bool;
    fn policy(&self) -> BoxFuture<'_, Result<Policy>>;
    /// Highest contiguous zero-based event number committed by Gateway. None means an empty remote log.
    fn head(&self, thread: &str) -> BoxFuture<'_, Result<Option<u64>>>;
    /// Refresh the signed holder directory and verify approval chains and pinned organization keys before returning.
    /// Remember revocations durably; rotate before sealing new events after a revoke. Refresh at every call here,
    /// including when a closed thread resumes. Publish locked keys durably before returning this key.
    fn key(
        &self,
        thread: &str,
        previous_epoch: Option<u64>,
        policy: ContentPolicy,
    ) -> BoxFuture<'_, Result<Arc<dyn ContentKey>>>;
    /// Atomically append or confirm an identical retried prefix. Never count these usage observations as new bills.
    fn append(&self, batch: Batch) -> BoxFuture<'_, Result<u64>>;
    /// Idempotently remove the remote thread and its locked keys (crypto-shredding).
    fn delete(&self, thread: &str) -> BoxFuture<'_, Result<()>>;
    /// The trust Gateway's device list named at its last read. [`DeviceTrust::Approvals`] until a read says otherwise.
    fn device_trust(&self) -> DeviceTrust {
        DeviceTrust::Approvals
    }
    /// Advertise the computer's local opt-in. Legacy backends never receive remote commands.
    fn supports_control(&self) -> bool {
        false
    }
    fn control_lease(&self, _thread: &str, _enabled: bool) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    /// Decrypt only commands signed by the same person's freshly verified trusted devices.
    fn control_commands(&self) -> BoxFuture<'_, Result<Vec<super::control::Command>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn control_claim(&self, _thread: &str, _id: &str) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async { Ok(false) })
    }
    fn control_finish(&self, _thread: &str, _id: &str, _completed: bool) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    /// Phone control's wake-up: follow the person's live command states, and notify `wake` when a command
    /// may wait in this computer's inbox, and once when the stream opens. Returns when the stream ends, and an error when
    /// it cannot open. A backend without one never wakes the loop: its inbox poll goes on.
    fn control_watch(&self, _wake: Arc<tokio::sync::Notify>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Err(NO_CONTROL_WATCH.to_string()) })
    }
}

/// [`Backend::control_watch`]'s answer from a backend that has no live stream.
pub const NO_CONTROL_WATCH: &str = "phone control: this backend has no live command stream";

/// An opaque key owned by the shared crypto core. Never expose secret bytes, Debug, or serialization here.
pub trait ContentKey: Send + Sync {
    fn epoch(&self) -> u64;
    /// AES-256-GCM, with a fresh random nonce. Bind thread UUID, epoch and zero-based sequence through
    /// length-prefixed associated data. Implementations must reject an exhausted key instead of reusing a nonce.
    fn seal(&self, thread: &str, seq: u64, plaintext: &[u8]) -> Result<Vec<u8>>;
}

/// These are runner/adapter types, not a proposed HTTP schema. Identity comes from the immutable credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub thread: String,
    pub policy: ContentPolicy,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub seq: u64,
    pub at: i64,
    pub agent: crate::event::SessionAgent,
    pub usage: Option<Usage>,
    pub status: Option<Status>,
    pub epoch: Option<u64>,
    /// Base64 of the crypto core's versioned sealed bytes. None under NeverLeaves.
    pub sealed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Running,
    Completed,
    Interrupted,
    Error,
}
