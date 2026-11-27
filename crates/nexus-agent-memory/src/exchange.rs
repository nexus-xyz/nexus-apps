use std::io;
use std::path::Path;

use crate::{Entry, Repository, Scope, Source, Store, valid_text, write_atomic};

pub struct NativeEntry {
    pub scope: Scope,
    pub file: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub text: String,
}

impl Store {
    pub fn import(
        &self,
        repository: Option<&Repository>,
        agent: &str,
        notes: impl IntoIterator<Item = (String, NativeEntry)>,
    ) -> io::Result<usize> {
        self.require_enabled()?;
        let _lock = self.lock()?;
        self.require_enabled()?;
        let repo_scope = repository.map(Repository::scope);
        let mut entries = self.read_entries_with_issues(repository)?.entries;
        let mut imported = 0;
        for (profile, note) in notes {
            if note.scope != Scope::Personal && Some(&note.scope) != repo_scope.as_ref() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "The entry belongs to another repository.",
                ));
            }
            let source = Source::Native {
                agent: agent.into(),
                profile,
                file: note.file,
            };
            let previous = entries
                .iter()
                .position(|entry| entry.scope == note.scope && entry.source == source)
                .or_else(|| legacy_source(&entries, &note.scope, &source, note.text.trim()));
            let text = note.text.trim();
            if let Some(index) = previous {
                if valid_text(text).is_err() {
                    let entry = entries.remove(index);
                    crate::remove(&self.entry_dir(&entry.scope).join(format!("{}.md", entry.id)))?;
                    imported += 1;
                    continue;
                }
                let entry = &mut entries[index];
                let content_changed =
                    entry.text != text || entry.name != note.name || entry.description != note.description;
                if content_changed || entry.source != source {
                    if entry.text != text {
                        let timestamp = crate::now();
                        entry.created_at = timestamp.as_secs();
                        entry.created_at_nanos = Some(timestamp.subsec_nanos());
                    }
                    if content_changed {
                        entry.text = text.to_string();
                        entry.name = note.name;
                        entry.description = note.description;
                        imported += 1;
                    }
                    entry.source = source;
                    self.write_entry(entry)?;
                }
            } else if !text.is_empty() && !entries.iter().any(|entry| entry.text == text) {
                entries.push(self.add(note.scope, source, text, note.name, note.description)?);
                imported += 1;
            }
        }
        Ok(imported)
    }

    pub fn export_markdown(&self, repository: Option<&Repository>, destination: &Path) -> io::Result<usize> {
        let entries = self.entries(repository)?;
        let markdown = format!("# Nexus shared memory\n\n{}", markdown(&entries));
        write_atomic(destination, markdown.as_bytes())?;
        Ok(entries.len())
    }
}

// Earlier headers did not identify an account. Exact content can identify the old spelling.
// When only one previous entry has that filename, let the first observed profile claim it.
fn legacy_source(entries: &[Entry], scope: &Scope, source: &Source, text: &str) -> Option<usize> {
    let Source::Native { agent, file, .. } = source else {
        return None;
    };
    let mut candidates = entries.iter().enumerate().filter(|(_, entry)| {
        &entry.scope == scope
            && matches!(&entry.source, Source::Native {
            agent: previous_agent, file: previous_file, profile,
        } if previous_agent == agent && previous_file == file && profile.is_empty())
    });
    let first = candidates.next()?;
    if first.1.text == text {
        return Some(first.0);
    }
    let Some(second) = candidates.next() else {
        return Some(first.0);
    };
    std::iter::once(second)
        .chain(candidates)
        .find_map(|(index, entry)| (entry.text == text).then_some(index))
}

pub fn markdown(entries: &[Entry]) -> String {
    let mut output = String::new();
    for entry in entries {
        let scope = if entry.scope == Scope::Personal {
            "Personal"
        } else {
            "Repository"
        };
        output.push_str(&format!("## {scope} memory ({})\n\n{}\n\n", entry.id, entry.text));
    }
    output
}
