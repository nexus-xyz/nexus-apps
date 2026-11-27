//! Native personal Skills, scoped Library and version-pinned promotion requests and reviews.
//! Uses the same Gateway routes as SkillsPage; personal ownership is established by the scoped own-list.
#[path = "skills_model.rs"]
mod model;
#[path = "skills_promotions.rs"]
mod promotions;
#[path = "skills_routes.rs"]
mod routes;
pub(crate) use routes::Target;
#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
#[path = "skills_view.rs"]
mod view;
pub(super) use view::build_locked;

use gpui_kit::component::input::TextareaState;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

use super::{Ctx, Load, Verb, fetch};
use model::{Detail, File, LibrarySkill, Skill};

pub const NATIVE: bool = true;

pub fn build(ctx: Ctx, _window: &mut Window, cx: &mut App) -> Option<AnyView> {
    Some(cx.new(|cx| SkillsPage::new(ctx, cx)).into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Own,
    Library,
    Requests,
}

struct Editor {
    text: Entity<TextareaState>,
    /// The exact original bytes, before the text widget normalizes line endings.
    initial: String,
    displayed_initial: String,
    /// None means create; otherwise pin the latest version observed when Edit opened.
    target: Option<(String, u64)>,
}

struct Selection {
    id: String,
    detail: Load<Detail>,
    file: Load<File>,
    version: Option<u64>,
}

pub(crate) struct SkillsPage {
    ctx: Ctx,
    marker: Option<String>,
    scope: Scope,
    own: Load<Vec<Skill>>,
    library: Load<Option<Vec<LibrarySkill>>>,
    selection: Option<Selection>,
    editor: Option<Editor>,
    confirming: bool,
    busy: bool,
    problem: Option<String>,
    notice: Option<String>,
    /// Every selection, retry and mutation invalidates answers started before it.
    epoch: u64,
    lists_epoch: u64,
    promotions: promotions::State,
}

impl SkillsPage {
    fn new(ctx: Ctx, cx: &mut Context<Self>) -> Self {
        let marker = ctx.gateway.marker();
        let mut page = Self {
            ctx,
            marker,
            scope: Scope::Own,
            own: Load::Loading,
            library: Load::Loading,
            selection: None,
            editor: None,
            confirming: false,
            busy: false,
            problem: None,
            notice: None,
            epoch: 0,
            lists_epoch: 0,
            promotions: promotions::State::default(),
        };
        page.load(cx);
        page
    }

    pub(crate) fn current(&self) -> bool {
        self.marker.is_some() && self.marker == self.ctx.gateway.marker()
    }

    fn accepts(&self, epoch: u64) -> bool {
        self.current() && self.epoch == epoch
    }

    fn owns(&self, id: &str) -> bool {
        let own = match &self.own {
            Load::Ready(rows) => Some(rows.as_slice()),
            _ => None,
        };
        let library = match &self.library {
            Load::Ready(Some(rows)) => Some(rows.as_slice()),
            _ => None,
        };
        self.current() && !matches!(self.library, Load::Loading) && model::writable(id, own, library)
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if !self.current() || self.busy {
            return;
        }
        self.lists_epoch += 1;
        let epoch = self.lists_epoch;
        self.own = Load::Loading;
        self.library = Load::Loading;
        let own = self.ctx.gateway.request_typed(Verb::Get, "/v1/skills".into(), None);
        fetch(cx, own, move |this, result, _| {
            if !this.current() || this.lists_epoch != epoch {
                return;
            }
            this.own = match result.map_err(model::problem).and_then(model::own) {
                Ok(rows) => Load::Ready(rows),
                Err(error) => Load::Failed(error.into()),
            };
        });
        let library = self
            .ctx
            .gateway
            .request_typed(Verb::Get, "/v1/skill-library".into(), None);
        fetch(cx, library, move |this, result, _| {
            if !this.current() || this.lists_epoch != epoch {
                return;
            }
            this.library = match result {
                Err(nexus_client::Error::Core(error)) if error.status == 404 => Load::Ready(None),
                result => match result.map_err(model::problem).and_then(model::library) {
                    Ok(rows) => Load::Ready(Some(rows)),
                    Err(error) => Load::Failed(error.into()),
                },
            };
        });
        cx.notify();
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.current() || self.busy || self.editor.is_some() || self.promotion_form_open() {
            return;
        }
        self.epoch += 1;
        let epoch = self.epoch;
        self.confirming = false;
        self.problem = None;
        self.notice = None;
        self.selection = Some(Selection {
            id: id.clone(),
            detail: Load::Loading,
            file: Load::Loading,
            version: None,
        });
        let request = self.ctx.gateway.request_typed(Verb::Get, model::path(&id), None);
        fetch(cx, request, move |this, result, cx| {
            if !this.accepts(epoch) {
                return;
            }
            match result.map_err(model::problem).and_then(model::parse::<Detail>) {
                Ok(detail) if detail.skill.id != id => {
                    if let Some(selection) = &mut this.selection {
                        selection.detail = Load::Failed("Nexus returned a different skill. Try again.".into());
                    }
                }
                Ok(detail) => {
                    // Only versions actually returned by Gateway are offered, including library approval gaps.
                    let latest = detail.versions.iter().map(|v| v.version).max();
                    if let Some(selection) = &mut this.selection {
                        selection.detail = Load::Ready(detail);
                    }
                    if let Some(version) = latest {
                        this.load_version(version, cx);
                    } else if let Some(selection) = &mut this.selection {
                        selection.file = Load::Failed("This skill has no available versions.".into());
                    }
                }
                Err(error) => {
                    if let Some(selection) = &mut this.selection {
                        selection.detail = Load::Failed(error.into());
                    }
                }
            }
        });
        cx.notify();
    }

    fn load_version(&mut self, version: u64, cx: &mut Context<Self>) {
        if !self.current() || self.busy || self.editor.is_some() || self.promotion_form_open() {
            return;
        }
        let Some(selection) = &mut self.selection else {
            return;
        };
        let id = selection.id.clone();
        selection.version = Some(version);
        selection.file = Load::Loading;
        self.epoch += 1;
        let epoch = self.epoch;
        let request =
            self.ctx
                .gateway
                .request_typed(Verb::Get, format!("{}/versions/{version}", model::path(&id)), None);
        fetch(cx, request, move |this, result, _| {
            if !this.accepts(epoch) {
                return;
            }
            let file = result
                .map_err(model::problem)
                .and_then(model::parse::<File>)
                .and_then(|file| {
                    if file.skill_id != id || file.version != version {
                        Err("Nexus returned a different skill version. Retry this version.".into())
                    } else {
                        Ok(file)
                    }
                });
            if let Some(selection) = &mut this.selection {
                selection.file = match file {
                    Ok(file) => Load::Ready(file),
                    Err(error) => Load::Failed(error.into()),
                };
            }
        });
        cx.notify();
    }

    fn edit(&mut self, initial: String, target: Option<(String, u64)>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.current()
            || self.busy
            || self.editor.is_some()
            || self.promotion_form_open()
            || target.as_ref().is_some_and(|(id, _)| !self.owns(id))
        {
            return;
        }
        self.epoch += 1;
        self.confirming = false;
        self.problem = None;
        self.notice = None;
        let text = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx).rows(16);
            input.set_value(initial.clone(), window, cx);
            input
        });
        let displayed_initial = text.read(cx).value().to_string();
        // The editor opens ready for typing.
        text.update(cx, |state, cx| state.focus(window, cx));
        if target.is_none() {
            self.selection = None;
        }
        self.editor = Some(Editor {
            text,
            initial,
            displayed_initial,
            target,
        });
        cx.notify();
    }

    fn edit_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Selection {
            id,
            detail: Load::Ready(detail),
            file: Load::Ready(file),
            ..
        }) = &self.selection
        else {
            return;
        };
        self.edit(
            file.content.clone(),
            Some((id.clone(), detail.skill.latest_version)),
            window,
            cx,
        );
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let target = self.editor.take().and_then(|editor| editor.target.map(|(id, _)| id));
        self.problem = None;
        self.epoch += 1;
        // Another client may have saved while this draft was open. Reopen the latest version,
        // including after a conflict, before offering Edit again.
        if let Some(id) = target {
            self.select(id, cx);
        }
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.current() || self.busy {
            return;
        }
        let Some(editor) = &self.editor else {
            return;
        };
        if editor.target.as_ref().is_some_and(|(id, _)| !self.owns(id)) {
            return;
        }
        let target = editor.target.clone();
        let unchanged = if target.is_some() { editor.initial.as_str() } else { "" };
        let typed = editor.text.read(cx).value().to_string();
        let content = if typed == editor.displayed_initial {
            Ok(editor.initial.clone())
        } else {
            model::edited(&typed, &editor.initial)
        };
        let content = match content.and_then(|text| {
            model::validate(&text, unchanged)?;
            Ok(text)
        }) {
            Ok(content) => content,
            Err(error) => {
                self.problem = Some(error);
                cx.notify();
                return;
            }
        };
        let (path, body) = match target {
            Some((id, base)) => (
                format!("{}/versions", model::path(&id)),
                json!({"content":content,"baseVersion":base}),
            ),
            None => ("/v1/skills".into(), json!({"content":content})),
        };
        self.busy = true;
        self.epoch += 1;
        let epoch = self.epoch;
        self.problem = None;
        let request = self.ctx.gateway.request_typed(Verb::Post, path, Some(body));
        fetch(cx, request, move |this, result, cx| {
            if !this.accepts(epoch) {
                return;
            }
            this.busy = false;
            match result.map_err(model::problem).and_then(|value| {
                value
                    .get("skill")
                    .cloned()
                    .ok_or_else(|| "Nexus returned no saved skill.".into())
                    .and_then(model::parse::<Skill>)
            }) {
                Ok(skill) => {
                    this.editor = None;
                    this.load(cx);
                    this.select(skill.id, cx);
                }
                Err(error) => this.problem = Some(error),
            }
        });
        cx.notify();
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        if !self.current() || self.busy || !self.confirming || self.editor.is_some() || self.promotion_form_open() {
            return;
        }
        let Some(selection) = &self.selection else {
            return;
        };
        if !self.owns(&selection.id) {
            return;
        }
        let path = model::path(&selection.id);
        self.busy = true;
        self.epoch += 1;
        let epoch = self.epoch;
        self.problem = None;
        let request = self.ctx.gateway.request_typed(Verb::Delete, path, None);
        fetch(cx, request, move |this, result, cx| {
            if !this.accepts(epoch) {
                return;
            }
            this.busy = false;
            match result.map_err(model::problem).and_then(|value| {
                if value.get("deleted") == Some(&Value::Bool(true)) {
                    Ok(())
                } else {
                    Err("Nexus did not confirm deletion. Refresh before trying again.".into())
                }
            }) {
                Ok(()) => {
                    this.selection = None;
                    this.confirming = false;
                    this.load(cx);
                }
                Err(error) => this.problem = Some(error),
            }
        });
        cx.notify();
    }

    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.current() || self.busy || self.editor.is_some() || self.promotion_form_open() {
            return;
        }
        self.selection = None;
        self.confirming = false;
        self.epoch += 1;
        let epoch = self.epoch;
        let executor = cx.background_executor().clone();
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import SKILL.md".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.accepts(epoch) {
                            this.problem = Some(error.to_string());
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            let Some(path) = result else {
                return;
            };
            // Bound file reads even when a file changes after its metadata was inspected.
            let result = executor
                .spawn(async move {
                    use std::io::Read;
                    let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                    let mut bytes = Vec::new();
                    file.take((model::MAX_BYTES + 1) as u64)
                        .read_to_end(&mut bytes)
                        .map_err(|e| e.to_string())?;
                    model::decode(bytes)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if !this.accepts(epoch) {
                    return;
                }
                match result {
                    Ok(text) => this.edit(text, None, window, cx),
                    Err(error) => this.problem = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        if !self.current() || self.busy {
            return;
        }
        let Some(Selection {
            file: Load::Ready(file),
            ..
        }) = &self.selection
        else {
            return;
        };
        let text = file.content.clone();
        let epoch = self.epoch;
        let executor = cx.background_executor().clone();
        let picked = cx.prompt_for_new_path(&super::download::downloads_folder(), Some("SKILL.md"));
        cx.spawn(async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.accepts(epoch) {
                            this.problem = Some(error.to_string());
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            // Recheck account and selection after the system dialog before writing private content.
            if !this.update(cx, |this, _| this.accepts(epoch)).unwrap_or(false) {
                return;
            }
            let result = executor
                .spawn(async move { nexus_desktop_core::atomic_file::write_atomic(&path, text.as_bytes()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.accepts(epoch) {
                    return;
                }
                match result {
                    Ok(()) => this.notice = Some("Saved SKILL.md.".into()),
                    Err(error) => this.problem = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
