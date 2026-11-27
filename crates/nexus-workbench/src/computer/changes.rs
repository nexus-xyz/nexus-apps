use super::*;
use crate::session_sync::Backend;
use crate::{changes_source, git, turn_files};

const MAX_PATCH: usize = 24 * 1024;
const MAX_PATH: usize = 512;
const MAX_FILES: usize = 32;

impl Api {
    pub(super) async fn session_changes(
        &self,
        backend: &dyn Backend,
        request: &RequestTarget,
        thread_id: &str,
    ) -> Result<Outcome> {
        let local = self.owned_session_target(backend, thread_id)?;
        let workbench = self.workbench();
        let guard = crate::CreationGuard {
            identity: backend.identity().clone(),
            expires_at: request.expires_at,
            generation: request.generation,
            sender: request.sender.clone(),
            selection: None,
        };
        workbench.check_creation_admission(Some(&guard)).await?;
        workbench.checkpoint_written(&local.local).await;
        let record = workbench.record(&local.local)?;
        if record.scope != backend.identity().scope {
            return Err("Account changed.".into());
        }
        let dir = workbench.workdir(&record).ok_or("This session’s folder is gone.")?;
        let base = record.summary.worktree.map(|worktree| worktree.base);
        let local_id = local.local.clone();
        let (files, patch, truncated) = tokio::task::spawn_blocking(move || {
            let refs = git::refs(&dir, &changes_source::checkpoint_prefix(&local_id))?;
            if refs.is_empty() {
                return Ok::<_, String>((Vec::new(), String::new(), false));
            }
            let (from, to) = changes_source::session_refs(&dir, &refs, base.as_deref())?;
            let (patch, mut truncated) = git::phone_diff(&dir, &from, &to, MAX_PATCH)?;
            let changes = turn_files::file_stats(&dir, &from, &to)?;
            let sections = patch.lines().filter(|line| line.starts_with("diff --git ")).count();
            let limit = sections.min(MAX_FILES);
            truncated |= changes.len() > limit;
            let files = changes
                .into_iter()
                .take(limit)
                .map(|file| {
                    let mut path = file.path;
                    truncated |= git::cut_at(&mut path, MAX_PATH);
                    path
                })
                .collect();
            Ok((files, patch, truncated))
        })
        .await
        .map_err(|error| error.to_string())??;
        workbench.check_creation_admission(Some(&guard)).await?;
        if self.owned_session_target(backend, thread_id)?.local != local.local {
            return Err("Session ownership changed.".into());
        }
        Ok(Outcome::SessionChanges {
            thread_id: thread_id.to_string(),
            files,
            patch,
            truncated,
        })
    }
}
