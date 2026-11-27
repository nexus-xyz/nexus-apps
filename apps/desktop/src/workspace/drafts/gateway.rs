//! Gateway execution draft promotion and refusal recovery, always under a captured owner.
use super::{ComposerDrafts, DetachedDraft, ParkedDraft, Submission, Workspace, ensure_scope, schedule_save};
use crate::attachments;
use crate::i18n::tr;
use gpui_kit::*;
use std::sync::Arc;
/// A Created receipt names the captured draft's frozen organization, never the newest UI account.
pub(in crate::workspace) fn promote_execution_draft(
    previous: Option<DetachedDraft>,
    id: &str,
    org: &str,
    cx: &mut App,
) {
    let Some(previous) = previous else {
        return;
    };
    let Ok(key) = serde_json::to_string(&(org, Some(id))) else {
        return;
    };
    let key = format!("chat:{key}");
    ensure_scope(&previous.owner, false, cx);
    cx.update_global::<ComposerDrafts, _>(|global, _| {
        if let Some(store) = global.scopes.get_mut(&previous.owner.scope) {
            store.promote_snapshot(&previous.owner.key, &key, previous.revision, previous.draft);
        }
    });
    schedule_save(true, cx);
}
impl Workspace {
    /// Gateway admission keeps the last synchronized owner, never a token/account recombination after preflight.
    pub(in crate::workspace) fn gateway_payload_submission(
        &self,
        text: &str,
        draft: &attachments::Draft,
    ) -> Option<Submission> {
        Some(Submission {
            owner: self.draft_at.clone()?,
            draft: ParkedDraft {
                text: text.into(),
                images: draft.images.clone(),
                files: draft.files.clone(),
                problem: None,
            },
            revision: self.draft_revision,
            recovered: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
    pub(in crate::workspace) fn gateway_draft_submission(&self, cx: &App) -> Option<Submission> {
        Some(Submission {
            owner: self.draft_at.clone()?,
            draft: self.capture_draft(cx),
            revision: self.draft_revision,
            recovered: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
}
impl Workspace {
    /// Only a refused synchronous admission restores this captured payload; accepted sends use their normal receipts.
    pub(in crate::workspace) fn draft_refused(
        &mut self,
        submission: Option<Submission>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(submitted) = submission else {
            return;
        };
        self.restore_submitted_draft(
            submitted,
            Some(&tr(
                "The account changed before this message was sent. Your draft was kept.",
            )),
            window,
            cx,
        );
    }
}
/// Per-App fixture storage uses the production draft model with no disk or process-global switch.
#[cfg(test)]
pub(in crate::workspace) fn fixture_memory_scope(workspace: &Workspace, cx: &mut App) {
    super::init(cx);
    let owner = workspace.draft_owner(cx).expect("fixture draft owner");
    cx.update_global::<ComposerDrafts, _>(|global, _| {
        let store = global
            .scopes
            .entry(owner.scope)
            .or_insert_with(|| super::ScopeStore::new(None, false));
        store.ensure(&owner.key);
    });
}

#[cfg(test)]
pub(in crate::workspace) fn fixture_submission_state(submission: &Submission, cx: &App) -> (ParkedDraft, u64) {
    let store = cx
        .global::<ComposerDrafts>()
        .scopes
        .get(&submission.owner.scope)
        .expect("frozen fixture scope");
    (store.get(&submission.owner.key), store.revision(&submission.owner.key))
}
#[cfg(test)]
pub(in crate::workspace) fn fixture_scopes(cx: &App) -> usize {
    cx.global::<ComposerDrafts>().scopes.len()
}
