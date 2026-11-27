//! A run's messages onto the transcript's entries. A streamed token applies only the messages it changed, so its cost
//! does not grow with the session's history.
use super::*;

/// What the transcript last showed of a run: the entries match its messages as of `revision`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workspace) struct Projected {
    key: u64,
    revision: u64,
    len: usize,
}

/// What a projection saw of the run's last message.
pub(super) struct Shown {
    /// Its text differs from the last entry's, as it was before the projection.
    pub grew: bool,
    /// It has text.
    pub streamed: bool,
}

impl Workspace {
    /// Bring `self.entries` up to the run at `key`, reading its messages in place.
    ///
    /// The entries keep their native answers (and so their selection) while their ids and roles match the run's.
    /// A message changed since the last projection of this run is applied again. Any other message is applied
    /// already: the entries were projected from it, and its revision has not moved since.
    pub(super) fn project_gateway_entries(
        &mut self,
        state: &Rc<RefCell<State>>,
        key: u64,
        cx: &mut Context<Self>,
    ) -> Shown {
        let state = state.borrow();
        let Some(run) = state.runs.get(&key) else {
            self.chat_projected = None;
            return Shown {
                grew: false,
                streamed: false,
            };
        };
        let grew = self.entries.last().map(|entry| entry.streamed.as_str())
            != run.entries.last().map(|entry| entry.streamed.as_str());
        // Read before applying: a message changed after this is applied next time.
        let revision = model::revision();
        if self.entries.len() <= run.entries.len()
            && self
                .entries
                .iter()
                .zip(&run.entries)
                .all(|(a, b)| a.id == b.id && a.role == b.role)
        {
            let since = self
                .chat_projected
                .filter(|seen| seen.key == key && seen.len == self.entries.len())
                .map(|seen| seen.revision);
            let before = self.entries.len();
            for (entry, data) in self.entries.iter_mut().zip(&run.entries) {
                if since.is_none_or(|since| data.revision > since) {
                    data.apply(entry, cx);
                }
            }
            self.entries
                .extend(run.entries[before..].iter().map(|entry| entry.present(cx)));
        } else {
            self.entries = run.entries.iter().map(|entry| entry.present(cx)).collect();
        }
        self.chat_projected = Some(Projected {
            key,
            revision,
            len: self.entries.len(),
        });
        Shown {
            grew,
            streamed: run.entries.last().is_some_and(|entry| !entry.streamed.is_empty()),
        }
    }
}
