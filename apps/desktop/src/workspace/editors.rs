//! The Git panel editor picker and session menu action use the same launch rules.
use super::git_panel::GitPanel;
use super::{TopPanel, Workspace, glyph};
use crate::workspace::overlay::OverlaySlot;
use crate::{code::CodeSession, editor_choice, palette::Palette};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::{
    component::{h_flex, v_flex},
    prelude::*,
    *,
};
use nexus_workbench::editors;

impl Workspace {
    pub(in crate::workspace) fn load_editors(&mut self, cx: &mut Context<Self>) {
        let id = match self.overlay.code_top() {
            Some(TopPanel::Git(panel)) => panel.id.clone(),
            _ => return,
        };
        let task = cx.background_executor().spawn(async {
            (
                editors::installed(&crate::code_run::agent_env().remove("PATH").unwrap_or_default()),
                editor_choice::saved(),
            )
        });
        cx.spawn(async move |this, cx| {
            let (found, choice) = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(TopPanel::Git(panel)) = this.overlay.code_top_mut()
                    && panel.id == id
                {
                    panel.editors = Some(found);
                    panel.editor_choice = choice;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workspace) fn open_session_editor(
        &mut self,
        id: &str,
        repo: Option<String>,
        editor: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let folder = self
            .runner
            .as_ref()
            .ok_or_else(|| "Code is not ready.".to_string())
            .map(|runner| runner.editor_folder(id, repo.as_deref()));
        let task = cx.background_executor().spawn(async move {
            let folder = folder?
                .await
                .unwrap_or_else(|_| Err("The session runner stopped.".into()))?;
            let path = crate::code_run::agent_env().remove("PATH").unwrap_or_default();
            let found = editors::installed(&path);
            let selected = editor
                .as_deref()
                .and_then(|id| found.iter().find(|entry| entry.id == id))
                .or_else(|| editors::preferred(&found, editor_choice::saved().as_deref()))
                .ok_or_else(|| "No editor or terminal is installed. Choose Files from the Git panel.".to_string())?;
            editors::open_folder(&folder, selected.id, &path)?;
            editor_choice::save(selected.id);
            Ok::<_, String>(())
        });
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.toggle_git_panel_error(&id, error, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_git_panel_error(&mut self, id: &str, error: String, cx: &mut Context<Self>) {
        if !matches!(self.overlay.code_top(), Some(TopPanel::Git(panel)) if panel.id == id) {
            self.toggle_git_panel(id, cx);
        }
        if let Some(TopPanel::Git(panel)) = self.overlay.code_top_mut() {
            panel.problem = Some(error.into());
        }
    }

    pub(in crate::workspace) fn render_editor_picker(
        &self,
        session: &CodeSession,
        panel: &GitPanel,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let available = Self::commands_unavailable(&session.id).is_none();
        let selected = panel
            .editors
            .as_deref()
            .and_then(|found| editors::preferred(found, panel.editor_choice.as_deref()));
        let title = selected
            .map(|entry| format!("Open in {}", entry.name))
            .unwrap_or_else(|| "Open in editor".into());
        let row = |key: SharedString, label: SharedString| {
            h_flex()
                .id(key)
                .h(px(30.))
                .px(px(8.))
                .gap(px(8.))
                .when(available, |row| row.cursor_pointer())
                .when(!available, |row| row.text_color(palette.faint))
                .hover(|style| crate::workspace::menu_item_hover(style, palette))
                .child(glyph(Lucide::ExternalLink, palette.icon).size(px(15.)))
                .child(label)
        };
        let mut body = v_flex()
            .child(row("open-editor".into(), title.into()).on_click({
                let id = session.id.clone();
                let repo = panel.repo.clone();
                cx.listener(move |this, _, _, cx| {
                    if available {
                        this.open_session_editor(&id, repo.clone(), None, cx);
                    }
                })
            }))
            .child(
                row("choose-editor".into(), "Choose application…".into()).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if !available {
                            return;
                        }
                        if let Some(TopPanel::Git(panel)) = this.overlay.code_top_mut() {
                            panel.editor_menu = !panel.editor_menu;
                        }
                        cx.notify();
                    },
                )),
            );
        if !available {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child("Editors open folders on this computer only."),
            );
        }
        if panel.editor_menu {
            for entry in panel.editors.as_deref().unwrap_or_default() {
                let id = session.id.clone();
                let repo = panel.repo.clone();
                let editor = entry.id.to_string();
                body = body.child(
                    row(format!("editor-{}", entry.id).into(), entry.name.into()).on_click(cx.listener(
                        move |this, _, _, cx| {
                            if let Some(TopPanel::Git(panel)) = this.overlay.code_top_mut() {
                                panel.editor_menu = false;
                                panel.editor_choice = Some(editor.clone());
                            }
                            this.open_session_editor(&id, repo.clone(), Some(editor.clone()), cx);
                        },
                    )),
                );
            }
        }
        body
    }
}
