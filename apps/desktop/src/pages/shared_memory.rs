#[cfg(test)]
mod tests;
mod view;

use std::io;
use std::path::{Path, PathBuf};

use gpui_kit::component::input::TextareaState;
use gpui_kit::*;
use nexus_agent_memory::{Entry, Repository, Scope, Source, Store, preview};
use nexus_client::local_agents::native_memory;
use nexus_workbench::{accounts, event::SessionAgent};
use uuid::Uuid;

use super::{desktop_app::off_thread, download::downloads_folder};

pub fn build(cx: &mut App) -> Option<AnyView> {
    #[cfg(not(test))]
    let store = Store::local().map_err(|error| error.to_string());
    #[cfg(test)]
    let store = Ok(Store::new(crate::paths::test_root().join("shared-memory")));
    let selection_file =
        crate::paths::app_settings_file().map(|file| file.with_file_name("shared-memory-repository.json"));
    Some(build_view(store, selection_file, cx).into())
}

fn build_view(store: Result<Store, String>, selection_file: Option<PathBuf>, cx: &mut App) -> Entity<SharedMemory> {
    cx.new(|cx| {
        let mut page = SharedMemory {
            store,
            selection_file,
            snapshot: Snapshot::default(),
            busy: false,
            confirming_off: false,
            confirming_forget: None,
            entries_scroll: UniformListScrollHandle::new(),
            editor: None,
            notice: None,
            error: None,
        };
        page.run(RepositoryRequest::Restore, |_, _| Ok(Update::Finished(None)), cx);
        page
    })
}

enum RepositoryRequest {
    Restore,
    Choose(Option<PathBuf>),
    Current(Option<PathBuf>),
}

enum Update {
    Finished(Option<String>),
    ConfirmDisable,
}

#[derive(Default)]
struct Snapshot {
    enabled: bool,
    checkout: Option<PathBuf>,
    entries: Vec<MemoryRow>,
    skipped: usize,
}

struct MemoryRow {
    entry: Entry,
    preview: String,
}

impl Snapshot {
    fn load(store: &Store, checkout: Option<PathBuf>) -> io::Result<Self> {
        let enabled = store.enabled()?;
        let notes = if enabled {
            let repository = checkout.as_deref().and_then(Repository::discover);
            store.entries_with_issues(repository.as_ref())?
        } else {
            nexus_agent_memory::Entries::default()
        };
        let entries = notes
            .entries
            .into_iter()
            .map(|entry| MemoryRow {
                preview: preview(&entry.text),
                entry,
            })
            .collect();
        Ok(Self {
            enabled,
            checkout,
            entries,
            skipped: notes.skipped,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ForgetConfirmation {
    Entry(Uuid),
    All,
}

struct SharedMemory {
    store: Result<Store, String>,
    selection_file: Option<PathBuf>,
    snapshot: Snapshot,
    busy: bool,
    confirming_off: bool,
    confirming_forget: Option<ForgetConfirmation>,
    entries_scroll: UniformListScrollHandle,
    editor: Option<(Option<Uuid>, Entity<TextareaState>)>,
    notice: Option<String>,
    error: Option<String>,
}

impl SharedMemory {
    fn select_repository(&mut self, checkout: Option<PathBuf>, cx: &mut Context<Self>) {
        self.run(
            RepositoryRequest::Choose(checkout),
            |_, _| Ok(Update::Finished(None)),
            cx,
        );
    }

    fn operate(
        &mut self,
        action: impl FnOnce(&Store, Option<&Path>) -> io::Result<Option<String>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.run(
            RepositoryRequest::Current(self.snapshot.checkout.clone()),
            move |store, checkout| action(store, checkout).map(Update::Finished),
            cx,
        );
    }

    fn run(
        &mut self,
        request: RepositoryRequest,
        action: impl FnOnce(&Store, Option<&Path>) -> io::Result<Update> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        let store = match &self.store {
            Ok(store) => store.clone(),
            Err(error) => {
                self.error = Some(error.clone());
                cx.notify();
                return;
            }
        };
        self.busy = true;
        self.notice = None;
        self.error = None;
        let selection_file = self.selection_file.clone();
        off_thread(
            cx,
            move || {
                let checkout = match request {
                    RepositoryRequest::Restore => selection_file
                        .as_ref()
                        .and_then(|file| std::fs::read(file).ok())
                        .and_then(|bytes| serde_json::from_slice::<PathBuf>(&bytes).ok())
                        .and_then(|folder| Repository::checkout(&folder)),
                    RepositoryRequest::Choose(ref folder) | RepositoryRequest::Current(ref folder) => folder
                        .as_deref()
                        .map(|folder| {
                            Repository::checkout(folder)
                                .ok_or_else(|| io::Error::other("Choose a folder in a Git repository."))
                        })
                        .transpose()?,
                };
                let update = action(&store, checkout.as_deref())?;
                let snapshot = Snapshot::load(&store, checkout)?;
                if matches!(request, RepositoryRequest::Choose(_))
                    && let Some(file) = selection_file
                {
                    if let Some(folder) = file.parent() {
                        std::fs::create_dir_all(folder)?;
                    }
                    let saved = serde_json::to_vec(&snapshot.checkout)?;
                    nexus_desktop_core::atomic_file::write_atomic_private(&file, &saved)?;
                }
                Ok::<_, io::Error>((snapshot, update))
            },
            |this, result, _| {
                this.busy = false;
                match result {
                    Ok((snapshot, update)) => {
                        this.confirming_off = snapshot.enabled && matches!(update, Update::ConfirmDisable);
                        this.snapshot = snapshot;
                        this.notice = match update {
                            Update::Finished(notice) => notice,
                            Update::ConfirmDisable => None,
                        };
                        this.editor = None;
                        this.confirming_forget = None;
                    }
                    Err(error) => this.error = Some(error.to_string()),
                }
            },
        );
        cx.notify();
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if !self.snapshot.enabled {
            self.set_enabled(true, cx);
            return;
        }
        let show_confirmation = !self.confirming_off;
        self.run(
            RepositoryRequest::Restore,
            move |store, _| {
                if store.disable_if_empty()? || !show_confirmation {
                    Ok(Update::Finished(None))
                } else {
                    Ok(Update::ConfirmDisable)
                }
            },
            cx,
        );
    }

    fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.operate(
            move |store, _| {
                store.set_enabled(enabled)?;
                Ok(None)
            },
            cx,
        );
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a repository".into()),
        });
        cx.spawn(async move |this, cx| match picked.await {
            Ok(Ok(Some(paths))) => {
                if let Some(folder) = paths.into_iter().next()
                    && let Err(error) = this.update(cx, |view, cx| view.select_repository(Some(folder), cx))
                {
                    eprintln!("Shared memory page closed: {error}");
                }
            }
            Ok(Err(error)) => {
                if let Err(closed) = this.update(cx, |view, cx| {
                    view.error = Some(error.to_string());
                    cx.notify();
                }) {
                    eprintln!("Shared memory page closed: {closed}");
                }
            }
            _ => {}
        })
        .detach();
    }

    fn exchange(&mut self, agent: SessionAgent, importing: bool, cx: &mut Context<Self>) {
        self.operate(
            move |store, checkout| {
                let coding = agent.coding().expect("native memory is for coding agents");
                let home = dirs::home_dir().ok_or_else(|| io::Error::other("Your home folder is unavailable."))?;
                let data =
                    crate::paths::data_dir().ok_or_else(|| io::Error::other("Your app data folder is unavailable."))?;
                let env = |name: &str| std::env::var(name).ok();
                let root = data.join("accounts");
                let cwd = checkout.unwrap_or_else(|| Path::new(""));
                let count = if importing {
                    let profiles = accounts::memory_profiles(Some(&root), agent, &home, &env);
                    native_memory::import(store, coding, &profiles, cwd)?
                } else {
                    let profile = accounts::profile_dir(agent, &home, &env);
                    native_memory::export(store, coding, &profile, cwd)?
                };
                if importing && count == 0 {
                    return Ok(Some("No new entries to import.".into()));
                }
                let verb = if importing { "Imported" } else { "Exported" };
                Ok(Some(format!("{verb} {count} entries.")))
            },
            cx,
        );
    }

    fn export_markdown(&mut self, turn_off: bool, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_new_path(&downloads_folder(), Some("nexus-shared-memory.md"));
        cx.spawn(async move |this, cx| {
            let apply = match picked.await {
                Ok(Ok(Some(destination))) => this.update(cx, |view, cx| {
                    view.operate(
                        move |store, checkout| {
                            let repository = checkout.and_then(Repository::discover);
                            let count = store.export_markdown(repository.as_ref(), &destination)?;
                            if turn_off {
                                store.set_enabled(false)?;
                            }
                            Ok(Some(format!("Exported {count} entries.")))
                        },
                        cx,
                    )
                }),
                Ok(Err(error)) => this.update(cx, |view, cx| {
                    view.error = Some(error.to_string());
                    cx.notify();
                }),
                _ => return,
            };
            if let Err(error) = apply {
                eprintln!("Shared memory page closed: {error}");
            }
        })
        .detach();
    }

    fn edit(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.snapshot.entries.iter().find(|row| row.entry.id == id) else {
            return;
        };
        self.open_editor(Some(id), row.entry.text.clone(), window, cx);
    }

    fn open_editor(&mut self, id: Option<Uuid>, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let editor = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx).rows(6);
            input.set_value(text, window, cx);
            input
        });
        editor.update(cx, |input, cx| input.focus(window, cx));
        self.editor = Some((id, editor));
        self.confirming_forget = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some((id, editor)) = &self.editor else { return };
        let (id, text) = (*id, editor.read(cx).value().to_string());
        self.operate(
            move |store, checkout| {
                let repository = checkout.and_then(Repository::discover);
                if let Some(id) = id {
                    let found = store.edit(repository.as_ref(), id, &text)?;
                    entry_result(found, "Saved the entry.")
                } else {
                    store.remember(Scope::Personal, Source::Person, &text)?;
                    Ok(Some("Saved the personal note.".into()))
                }
            },
            cx,
        );
    }

    fn forget_all(&mut self, cx: &mut Context<Self>) {
        self.operate(
            |store, _| {
                store.forget_all()?;
                Ok(Some("Forgot all shared memory entries.".into()))
            },
            cx,
        );
    }

    fn forget(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.operate(
            move |store, checkout| {
                let repository = checkout.and_then(Repository::discover);
                entry_result(store.forget(repository.as_ref(), id)?, "Forgot the entry.")
            },
            cx,
        );
    }
}

fn entry_result(found: bool, text: &str) -> io::Result<Option<String>> {
    if found {
        Ok(Some(text.into()))
    } else {
        Err(io::Error::other("That shared memory entry was not found."))
    }
}
