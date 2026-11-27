//! Bounded receipts retain the frozen draft key. Persisted Code drafts are not source-qualified.
use super::{
    Key, alive,
    model::{Origin, Registry},
};
use crate::{
    attachments::Prepared,
    workspace::{Workspace, drafts::BrowserDraft},
};
use gpui_kit::App;

pub(super) enum Output {
    Text(String),
    Attachments(Prepared),
}
pub(super) struct Receipt {
    pub target: BrowserDraft,
    pub output: Output,
}
impl Output {
    fn bytes(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Attachments(prepared) => prepared
                .images
                .iter()
                .chain(&prepared.files)
                .map(|item| item.size())
                .sum(),
        }
    }
}
const COUNT: usize = 8;
const BYTES: usize = 32 * 1024 * 1024;
const CONFLICT: &str = "Output is kept for this session. Another local source uses the same session ID; close that copy, then Retry output.";
const LOADING: &str = "Output is kept while this session's saved draft loads.";
const CHANGED: &str = "The draft was cleared or sent while this output waited. Capture or add it again.";

pub(super) fn conflict(owner: u64, origin: &Origin, cx: &App) -> bool {
    let other_owner = cx.try_global::<Registry>().is_some_and(|registry| {
        registry.entries.iter().any(|entry| {
            entry.id != owner
                && entry.current()
                && entry.origin.key.identity == origin.key.identity
                && entry.origin.key.session == origin.key.session
                && entry.origin.key.source != origin.key.source
        })
    });
    other_owner || crate::windows::browser_output_conflict(&origin.key, cx)
}
pub(super) fn reserve(owner: u64, cx: &mut App) -> bool {
    let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) else {
        return false;
    };
    if !entry.current() {
        return false;
    }
    if entry.pending.len() + entry.preparing >= COUNT {
        entry.problem = Some(
            "Output was not added: the pending output limit is reached. Resolve the other copy and try again.".into(),
        );
        return false;
    }
    entry.preparing += 1;
    true
}
pub(super) fn prepared(owner: u64, origin: Origin, receipt: Receipt, cx: &mut App) {
    if let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) {
        entry.preparing = entry.preparing.saturating_sub(1);
    }
    deliver(owner, origin, receipt, cx);
}
fn problem(owner: u64, message: &str, cx: &mut App) {
    if let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) {
        entry.problem = Some(message.into());
    }
}
fn keep(owner: u64, receipt: Receipt, loading: bool, cx: &mut App) {
    let entry = cx.global_mut::<Registry>().id_mut(owner).expect("current output owner");
    let bytes = receipt.output.bytes();
    if entry.pending.len() + entry.preparing >= COUNT || entry.pending_bytes.saturating_add(bytes) > BYTES {
        entry.problem = Some(
            "Output was not added: the pending output limit is reached. Resolve the other copy and try again.".into(),
        );
    } else {
        entry.pending_bytes += bytes;
        entry.pending.push_back(receipt);
        entry.loading_output = loading;
        entry.problem = Some(if loading { LOADING } else { CONFLICT }.into());
    }
}
pub(super) fn deliver(owner: u64, origin: Origin, receipt: Receipt, cx: &mut App) {
    if !alive(owner, cx) || !origin.current() {
        return;
    }
    let collision = conflict(owner, &origin, cx);
    if !origin.current() || !alive(owner, cx) {
        return;
    }
    if !receipt.target.generation_current(cx) {
        problem(owner, CHANGED, cx);
    } else if collision {
        // No ensure_scope, read, write or composer update occurs on a failed source admission.
        keep(owner, receipt, false, cx);
    } else if !receipt.target.ready(cx) {
        let target = receipt.target.clone();
        keep(owner, receipt, true, cx);
        if alive(owner, cx) && origin.current() {
            target.load(false, cx);
        }
    } else {
        append(owner, &origin, receipt, cx);
    }
    cx.refresh_windows();
}
fn append(owner: u64, origin: &Origin, receipt: Receipt, cx: &mut App) {
    if !origin.current() || !alive(owner, cx) {
        return;
    }
    let applied = match receipt.output {
        Output::Text(text) => receipt.target.text(&text, cx),
        Output::Attachments(prepared) => receipt.target.attachments(prepared, cx),
    };
    if !applied {
        problem(owner, CHANGED, cx);
    } else if let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) {
        if entry.pending.is_empty() {
            entry.problem = None;
        }
    }
    super::routing::notify(&origin.key, cx);
}
pub(super) fn retry(owner: u64, cx: &mut App) {
    drain(owner, true, cx);
}
fn drain(owner: u64, reopen: bool, cx: &mut App) {
    let Some(origin) = cx
        .try_global::<Registry>()
        .and_then(|registry| registry.id(owner))
        .filter(|entry| entry.current())
        .map(|entry| entry.origin.clone())
    else {
        return;
    };
    if conflict(owner, &origin, cx) || !origin.current() {
        problem(owner, CONFLICT, cx);
        return;
    }
    let target = cx
        .global::<Registry>()
        .id(owner)
        .and_then(|entry| entry.pending.front())
        .map(|receipt| receipt.target.clone());
    if let Some(target) = target {
        if target.generation_current(cx) && !target.ready(cx) {
            if let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) {
                entry.loading_output = true;
                entry.problem = Some(LOADING.into());
            }
            if origin.current() {
                target.load(reopen, cx);
            }
            return;
        }
    }
    let pending = {
        let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) else {
            return;
        };
        if !entry.current() {
            return;
        }
        entry.problem = None;
        entry.loading_output = false;
        entry.pending_bytes = 0;
        std::mem::take(&mut entry.pending)
    };
    for receipt in pending {
        if !alive(owner, cx) || !origin.current() {
            break;
        }
        if !receipt.target.generation_current(cx) {
            problem(owner, CHANGED, cx);
            continue;
        }
        append(owner, &origin, receipt, cx);
    }
    cx.refresh_windows();
}
pub(crate) fn draft_read_finished(scope: &str, key: &str, cx: &mut App) {
    let owners = cx
        .try_global::<Registry>()
        .map(|registry| {
            registry
                .entries
                .iter()
                .filter(|entry| entry.current() && entry.loading_output)
                .filter(|entry| {
                    entry
                        .pending
                        .front()
                        .is_some_and(|receipt| receipt.target.target(scope, key, cx))
                })
                .map(|entry| entry.id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for owner in owners {
        let target = cx
            .global::<Registry>()
            .id(owner)
            .and_then(|entry| entry.pending.front())
            .map(|receipt| receipt.target.clone());
        if let Some(target) = target {
            if target.ready(cx) {
                drain(owner, false, cx);
            } else if !target.reading(cx) {
                if let Some(entry) = cx.global_mut::<Registry>().id_mut(owner) {
                    entry.loading_output = false;
                    entry.problem = Some(
                        "The saved draft could not be loaded. Output is kept; Retry output when storage is available."
                            .into(),
                    );
                }
                cx.refresh_windows();
            }
        }
    }
}
pub(super) fn notice(key: &Key, cx: &App) -> Option<(u64, String, bool)> {
    let entry = cx.try_global::<Registry>()?.key(key)?;
    if !entry.current() {
        return None;
    }
    Some((entry.id, entry.problem.clone()?, !entry.pending.is_empty()))
}
impl Workspace {
    pub(crate) fn browser_output_conflict(&self, key: &Key) -> bool {
        if self.parked_browser_output_conflict(key) {
            return true;
        }
        if self.terminal_identity.as_ref() != Some(&key.identity)
            || !self.navigation_allowed()
            || self
                .gateway_account_authority
                .as_ref()
                .is_none_or(|proof| !proof.current())
        {
            return false;
        }
        let Some(code) = &self.code else { return false };
        let (data, scope) = code.store_source();
        if matches!(&key.source, super::super::runs::Source::Local { data: same, scope: same_scope } if same == data && same_scope == scope)
        {
            return false;
        }
        code.summary(&key.session).map_or(true, |summary| summary.is_some())
    }
}
