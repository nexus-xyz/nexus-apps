//! Native Skills controls, separated from asynchronous API state.
use super::super::{Load, palette, ui};
use super::{Scope, SkillsPage, model};
use crate::palette::Palette;
use gpui_kit::component::input::Textarea;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

impl SkillsPage {
    fn failed(&self, error: SharedString, cx: &mut Context<Self>, palette: Palette) -> Div {
        v_flex().gap(px(8.)).child(ui::error_line(error, palette)).child(
            ui::button("skills-retry", "Try again", palette).on_click(cx.listener(|this, _, _, cx| this.load(cx))),
        )
    }

    fn list(&self, cx: &mut Context<Self>, palette: Palette) -> Div {
        let mut list = v_flex().gap(px(12.));
        match self.scope {
            Scope::Own => match &self.own {
                Load::Loading => list = list.child(ui::note("Loading your skills…", palette)),
                Load::Failed(error) => list = list.child(self.failed(error.clone(), cx, palette)),
                Load::Ready(rows) if rows.is_empty() => {
                    list = list.child(ui::empty(
                        "No skills yet",
                        "Write a SKILL.md here, or import a UTF-8 SKILL.md file.",
                        palette,
                    ))
                }
                Load::Ready(rows) => {
                    for skill in rows {
                        let id = skill.id.clone();
                        let open = if self.busy || self.editor.is_some() || self.promotion_form_open() {
                            ui::disabled_button(
                                SharedString::from(format!("skill-{}", skill.id)),
                                skill.name.clone(),
                                palette,
                            )
                            .into_any_element()
                        } else {
                            ui::button(
                                SharedString::from(format!("skill-{}", skill.id)),
                                skill.name.clone(),
                                palette,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
                            .into_any_element()
                        };
                        list = list.child(
                            ui::card(palette)
                                .gap(px(4.))
                                .child(open)
                                .child(ui::note(skill.description.clone(), palette))
                                .child(ui::note(
                                    format!("Version {} · {}", skill.latest_version, skill.updated_at),
                                    palette,
                                )),
                        );
                    }
                }
            },
            Scope::Requests => return self.promotion_requests_view(palette, cx),
            Scope::Library => match &self.library {
                Load::Loading => list = list.child(ui::note("Loading the skill library…", palette)),
                Load::Failed(error) => list = list.child(self.failed(error.clone(), cx, palette)),
                Load::Ready(None) => {
                    list = list.child(ui::empty(
                        "This Gateway has no skill library yet",
                        "The library needs a newer Nexus Gateway.",
                        palette,
                    ))
                }
                Load::Ready(Some(rows)) if rows.is_empty() => {
                    list = list.child(ui::empty(
                        "The library is empty",
                        "An approved promotion adds a skill to your organization or team's library.",
                        palette,
                    ))
                }
                Load::Ready(Some(rows)) => {
                    for skill in rows {
                        let id = skill.id.clone();
                        let open = if self.busy || self.editor.is_some() || self.promotion_form_open() {
                            ui::disabled_button(
                                SharedString::from(format!("library-{}", skill.id)),
                                skill.name.clone(),
                                palette,
                            )
                            .into_any_element()
                        } else {
                            ui::button(
                                SharedString::from(format!("library-{}", skill.id)),
                                skill.name.clone(),
                                palette,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
                            .into_any_element()
                        };
                        let scope = if skill.scope == "org" {
                            "The organization".into()
                        } else {
                            format!("Team {}", skill.team_id.as_deref().unwrap_or("unknown"))
                        };
                        let mut card = ui::card(palette)
                            .gap(px(4.))
                            .child(open)
                            .child(ui::note(model::visible(&skill.description), palette))
                            .child(ui::note(
                                format!("{scope} · Version {} · Approved {}", skill.version, skill.approved_at),
                                palette,
                            ));
                        if matches!(&self.own, Load::Ready(own) if own.iter().any(|candidate| candidate.name == skill.name))
                        {
                            card = card.child(ui::note("You have a skill with this name. If its text differs, neither loads. Rename or delete yours.", palette));
                        }
                        list = list.child(card);
                    }
                }
            },
        }
        list
    }

    fn selected(&self, cx: &mut Context<Self>, palette: Palette) -> Div {
        let Some(selection) = &self.selection else {
            return div();
        };
        let mut panel = ui::card(palette).mt(px(16.)).gap(px(10.));
        match &selection.detail {
            Load::Loading => return panel.child(ui::note("Loading skill…", palette)),
            Load::Failed(error) => {
                let id = selection.id.clone();
                return panel.child(ui::error_line(error.clone(), palette)).child(
                    ui::button("skill-detail-retry", "Try again", palette)
                        .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx))),
                );
            }
            Load::Ready(detail) => {
                panel = panel
                    .child(ui::section_head(detail.skill.name.clone(), palette))
                    .child(ui::note(model::visible(&detail.skill.description), palette));
                let versions = detail.versions.iter().map(|version| {
                    let number = version.version;
                    if self.busy || self.editor.is_some() || self.promotion_form_open() {
                        ui::disabled_button(
                            SharedString::from(format!("skill-version-{number}")),
                            format!("v{number}"),
                            palette,
                        )
                        .into_any_element()
                    } else {
                        ui::chip(
                            SharedString::from(format!("skill-version-{number}")),
                            format!("v{number}"),
                            selection.version == Some(number),
                            palette,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.load_version(number, cx)))
                        .into_any_element()
                    }
                });
                panel = panel.child(h_flex().gap(px(6.)).flex_wrap().children(versions));
                if let Some(version) = detail.versions.iter().find(|v| Some(v.version) == selection.version) {
                    panel = panel.child(ui::note(
                        format!(
                            "{} · {} · {} bytes · SHA-256 {}",
                            version.name, version.created_at, version.bytes, version.content_sha256
                        ),
                        palette,
                    ));
                }
            }
        }
        match &selection.file {
            Load::Loading => panel = panel.child(ui::note("Loading SKILL.md…", palette)),
            Load::Failed(error) => {
                let version = selection.version;
                panel = panel.child(ui::error_line(error.clone(), palette));
                if let Some(version) = version {
                    panel = panel.child(
                        ui::button("skill-file-retry", "Retry this version", palette)
                            .on_click(cx.listener(move |this, _, _, cx| this.load_version(version, cx))),
                    );
                }
            }
            Load::Ready(file) => {
                panel = panel.child(ui::note("SKILL.md", palette));
                if file.content.starts_with('\u{feff}') {
                    panel = panel.child(ui::note(
                        "This file starts with a byte-order mark. Export preserves it.",
                        palette,
                    ));
                }
                panel = panel.child(
                    div()
                        .font_family(ui::MONO)
                        .text_size(px(12.))
                        .text_color(palette.strong)
                        .child(model::visible(file.content.trim_start_matches('\u{feff}'))),
                );
                let mut actions = h_flex().flex_wrap().gap(px(8.));
                if self.busy {
                    actions = actions.child(ui::disabled_button("skill-export", "Export", palette));
                } else {
                    actions = actions.child(
                        ui::button("skill-export", "Export SKILL.md…", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.export(cx))),
                    );
                }
                if self.owns(&selection.id) && self.editor.is_none() {
                    if self.busy {
                        actions = actions.child(ui::disabled_button("skill-edit", "Edit", palette));
                    } else {
                        actions = actions.child(
                            ui::button("skill-edit", "Edit as next version", palette)
                                .on_click(cx.listener(|this, _, window, cx| this.edit_selected(window, cx))),
                        );
                    }
                    if !self.busy {
                        actions = actions.child(
                            ui::button("skill-promote", "Request promotion…", palette)
                                .on_click(cx.listener(|this, _, window, cx| this.begin_promotion(window, cx))),
                        );
                    }
                } else {
                    panel = panel.child(ui::note(
                        "Library skills are read-only. Only approved versions appear here.",
                        palette,
                    ));
                }
                panel = panel.child(actions);
            }
        }
        if self.owns(&selection.id) && self.editor.is_none() {
            if self.confirming {
                panel = panel.child(ui::note(
                    "Every version goes, and agents you start later no longer get this skill.",
                    palette,
                ));
                if self.busy {
                    panel = panel.child(ui::disabled_button("skill-delete-confirm", "Deleting…", palette));
                } else {
                    panel = panel.child(
                        h_flex()
                            .gap(px(8.))
                            .child(
                                ui::danger_button("skill-delete-confirm", "Delete every version", palette)
                                    .debug_selector(|| "skill-delete-confirmation-native".into())
                                    .on_click(cx.listener(|this, _, _, cx| this.remove(cx))),
                            )
                            .child(ui::ghost_button("skill-keep", "Keep", palette).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.confirming = false;
                                    cx.notify();
                                },
                            ))),
                    );
                }
            } else if !self.busy {
                panel = panel.child(
                    ui::ghost_button("skill-delete", "Delete…", palette)
                        .debug_selector(|| "skill-delete-native".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.busy {
                                this.confirming = true;
                                cx.notify();
                            }
                        })),
                );
            }
        }
        panel
    }
}

fn scope_tabs(picked: Scope, palette: Palette, mut cx: Option<&mut Context<SkillsPage>>) -> Div {
    h_flex().flex_wrap().gap(px(8.)).children(
        [
            (Scope::Own, "Your skills"),
            (Scope::Library, "Library"),
            (Scope::Requests, "Promotion requests"),
        ]
        .into_iter()
        .map(|(scope, label)| {
            let id = SharedString::from(format!("skills-{scope:?}"));
            match cx.as_deref_mut() {
                None => ui::disabled_button(id, label, palette),
                Some(cx) => ui::chip(id, label, picked == scope, palette)
                    .on_click(cx.listener(move |this, _, _, cx| this.change_scope(scope, cx))),
            }
        }),
    )
}

fn create_actions(palette: Palette, cx: Option<&mut Context<SkillsPage>>) -> Div {
    let row = h_flex().gap(px(8.));
    match cx {
        None => row
            .child(ui::disabled_button("skill-new", "New skill", palette))
            .child(ui::disabled_button("skill-import", "Import a file…", palette)),
        Some(cx) => row
            .child(
                ui::primary_button("skill-new", "New skill", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.edit(model::SKELETON.into(), None, window, cx))),
            )
            .child(
                ui::button("skill-import", "Import a file…", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.import(window, cx))),
            ),
    }
}

pub(in crate::pages) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        v_flex()
            .gap(px(12.))
            .child(scope_tabs(Scope::Own, palette, None))
            .child(create_actions(palette, None))
            .into_any_element()
    })
}

impl Render for SkillsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        if !self.current() {
            return ui::empty(
                "Sign-in changed",
                "Open Skills again for your current account.",
                palette,
            );
        }
        let locked = self.busy || self.editor.is_some() || self.promotion_form_open();
        let tabs = scope_tabs(self.scope, palette, (!locked).then_some(&mut *cx));
        let mut page = v_flex()
            .w_full()
            .gap(px(12.))
            .child(ui::lead(
                "Keep your own SKILL.md files in versions. Approved organization and team skills appear in Library.",
                palette,
            ))
            .child(tabs);
        if self.scope == Scope::Own {
            page = page.child(create_actions(palette, (!locked).then_some(&mut *cx)));
        }
        if let Some(editor) = &self.editor {
            let mut card = ui::card(palette).gap(px(8.))
                .child(ui::section_head(if editor.target.is_some() { "Write the next version" } else { "Create a skill" }, palette))
                .child(ui::note("SKILL.md starts with YAML front matter: name and description between --- lines, followed by your instructions. At most 1 MiB.", palette))
                .child(div().p(px(10.)).rounded(px(8.)).bg(palette.hover).font_family(ui::MONO)
                    .child(Textarea::new(&editor.text).appearance(false).disabled(self.busy)));
            let actions = if self.busy {
                h_flex().child(ui::disabled_button("skill-save", "Saving…", palette))
            } else {
                h_flex()
                    .gap(px(8.))
                    .child(
                        ui::primary_button(
                            "skill-save",
                            if editor.target.is_some() {
                                "Save next version"
                            } else {
                                "Create skill"
                            },
                            palette,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .child(
                        ui::ghost_button("skill-cancel", "Cancel", palette).on_click(cx.listener(|this, _, _, cx| {
                            this.cancel_edit(cx);
                        })),
                    )
            };
            card = card.child(actions);
            page = page.child(card);
        }
        page = page
            .children(self.problem.clone().map(|error| ui::error_line(error, palette)))
            .children(self.notice.clone().map(|notice| ui::note(notice, palette)))
            .child(self.list(cx, palette));
        if self.editor.is_none() && self.scope != Scope::Requests {
            if self.promotions.request.is_some() {
                page = page.child(self.promotion_request_view(palette, cx));
            } else {
                page = page.child(self.selected(cx, palette));
            }
        }
        page
    }
}
