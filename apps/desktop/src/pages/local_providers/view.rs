use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_providers::KeyVendor;

use super::form::Action;
use super::{Check, Editor, Row, Saved};
use crate::pages::{palette, ui};
use crate::palette::Palette;

pub(super) const PAGE_SIZE: usize = 20;

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let root = v_flex()
            .id("local-provider-editor")
            .track_focus(&self.focus)
            .w_full()
            .gap(px(16.));
        let Some(store) = &self.store else {
            let root = root.child(ui::error_line(
                "Nexus could not open its local configuration folder.",
                palette,
            ));
            return root.when(self.setup, |root| {
                root.child(
                    ui::button("cancel-provider-edit", "Cancel", palette)
                        .on_click(cx.listener(|this, _, window, cx| this.cancel_form(window, cx))),
                )
            });
        };
        let store = store.read(cx);
        let pending = matches!(store.snapshot, Some(Ok((_, true))));
        let root = root
            .when(pending, |root| {
                root.child(ui::settings_notice(
                    "Key deletion pending",
                    "Nexus will retry deleting unused keys from the keychain the next time it starts.",
                    palette,
                ))
            })
            .when(self.draft_changed, |root| {
                root.child(ui::error_line(
                    "This provider changed. Open it again to make changes.",
                    palette,
                ))
            })
            .children(
                store
                    .error
                    .as_ref()
                    .map(|error| ui::error_line(error.to_string(), palette)),
            );
        if store.saving {
            return root.child(ui::note("Saving provider settings…", palette));
        }
        if self.setup {
            return root.children(self.form.as_ref().map(|form| form.render(self.setup, palette, cx)));
        }
        match &store.snapshot {
            None => return root.child(ui::note("Loading local providers…", palette)),
            Some(Err(error)) => {
                return root.child(ui::error_line(error.to_string(), palette)).child(
                    ui::button("reload-local-providers", "Retry", palette).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(store) = &this.store {
                            store.update(cx, super::Store::refresh);
                        }
                    })),
                );
            }
            Some(Ok(_)) => {}
        }
        if let Some(form) = &self.form {
            return root.child(form.render(self.setup, palette, cx));
        }
        let start = self.page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(self.rows.len());
        root.child(
            h_flex()
                .flex_wrap()
                .gap(px(8.))
                .child(self.action(
                    "add-anthropic-key",
                    "Add Anthropic key",
                    Action::AddKey(KeyVendor::Anthropic),
                    palette,
                    cx,
                ))
                .child(self.action(
                    "add-openai-key",
                    "Add OpenAI key",
                    Action::AddKey(KeyVendor::Openai),
                    palette,
                    cx,
                ))
                .child(self.action("add-model-server", "Add model server", Action::AddServer, palette, cx)),
        )
        .when(self.rows.is_empty(), |root| {
            root.child(ui::note("No local providers saved.", palette))
        })
        .children(
            self.rows[start..end]
                .iter()
                .map(|row| self.render_row(row, palette, cx)),
        )
        .when(self.rows.len() > PAGE_SIZE, |root| {
            root.child(
                h_flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .when(start > 0, |row| {
                        row.child(
                            ui::button("previous-providers", "Previous", palette).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.page = this.page.saturating_sub(1);
                                    cx.notify();
                                },
                            )),
                        )
                    })
                    .child(ui::note(
                        format!("{}–{} of {}", start + 1, end, self.rows.len()),
                        palette,
                    ))
                    .when(end < self.rows.len(), |row| {
                        row.child(ui::button("next-providers", "Next", palette).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.page += 1;
                                cx.notify();
                            },
                        )))
                    }),
            )
        })
    }
}

impl Editor {
    fn action(
        &self,
        id: &'static str,
        label: &'static str,
        action: Action,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        ui::button(id, label, palette)
            .on_click(cx.listener(move |this, _, window, cx| this.open_form(action.clone(), window, cx)))
    }

    fn render_row(&self, row: &Row, palette: Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let saved = row.saved.clone();
        let id = saved.id();
        let status = match self.checks.get(id) {
            Some(Check::Running(_)) => Some(ui::note("Checking…", palette)),
            Some(Check::Finished {
                result: Ok(_),
                missing_models: true,
            }) => Some(ui::error_line(
                "Check passed. The server did not list one or more saved model IDs.",
                palette,
            )),
            Some(Check::Finished { result: Ok(models), .. }) if models.is_empty() => {
                Some(ui::note("Check passed.", palette))
            }
            Some(Check::Finished { result: Ok(_), .. }) => {
                Some(ui::note("Check passed. Edit the server to pick its models.", palette))
            }
            Some(Check::Finished { result: Err(error), .. }) => Some(ui::error_line(error.to_string(), palette)),
            None => None,
        };
        let check = (!matches!(self.checks.get(id), Some(Check::Running(_)))).then(|| {
            let saved = saved.clone();
            ui::button("check-provider", "Check", palette)
                .on_click(cx.listener(move |this, _, _, cx| this.check(saved.clone(), cx)))
        });
        ui::card(palette)
            .id(SharedString::from(id.as_str().to_owned()))
            .gap(px(8.))
            .min_w_0()
            .child(ui::lead(row.name.clone(), palette))
            .child(ui::note(row.detail.clone(), palette))
            .children(row.setup.as_ref().map(|setup| ui::note(setup.clone(), palette)))
            .children(status)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .children(check)
                    .map(|actions| match &saved {
                        Saved::Key(_) => actions.child(self.action(
                            "rotate-provider",
                            "Replace key",
                            Action::Rotate(saved.clone()),
                            palette,
                            cx,
                        )),
                        Saved::Server(server) => actions
                            .child(self.action(
                                "edit-provider",
                                "Edit",
                                Action::EditServer(server.clone()),
                                palette,
                                cx,
                            ))
                            .child(self.action(
                                "rotate-provider",
                                if server.key.is_some() { "Replace key" } else { "Add key" },
                                Action::Rotate(saved.clone()),
                                palette,
                                cx,
                            ))
                            .when(server.key.is_some(), |actions| {
                                actions.child(self.action(
                                    "clear-provider-key",
                                    "Clear key",
                                    Action::Clear(server.clone()),
                                    palette,
                                    cx,
                                ))
                            }),
                    })
                    .child(self.action(
                        "remove-provider",
                        "Remove",
                        Action::Remove(saved, row.name.clone()),
                        palette,
                        cx,
                    )),
            )
    }
}
