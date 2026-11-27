//! The Library's native overflow and context menus share one action list.

use super::*;
use crate::library::Action;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};

#[path = "library_dialog.rs"]
mod dialog;
pub(super) use dialog::LibraryDialog;

impl Workspace {
    pub(super) fn library_menu(&self, menu: PopupMenu, row: &Row, cx: &Context<Self>) -> PopupMenu {
        self.item_menu(menu, row, &[], cx)
    }

    /// The item's actions, less `skip`; its other shares keep all of theirs.
    fn item_menu(&self, mut menu: PopupMenu, row: &Row, skip: &[Action], cx: &Context<Self>) -> PopupMenu {
        let admin = self
            .account
            .as_ref()
            .is_some_and(|a| matches!(a.role.as_str(), "owner" | "admin"));
        if row.revoked {
            menu = menu.label("Revoked link");
        }
        for action in row.actions(admin).into_iter().filter(|action| !skip.contains(action)) {
            if matches!(action, Action::Share | Action::Revoke | Action::Delete) {
                menu = menu.separator();
            }
            let row = row.clone();
            let workspace = cx.entity().downgrade();
            let marker = self.gateway.marker();
            let gateway_url = self.gateway.url().to_string();
            menu = menu.item(PopupMenuItem::new(action.label()).on_click(move |_, window, cx| {
                cx.stop_propagation();
                // Let the menu dismiss and restore its trigger before a dialog takes focus.
                let workspace = workspace.clone();
                let row = row.clone();
                let marker = marker.clone();
                let gateway_url = gateway_url.clone();
                window.defer(cx, move |window, cx| {
                    let _ = workspace.update(cx, |this, cx| {
                        if this.gateway.marker() == marker && this.gateway.url() == gateway_url {
                            this.library_action(action, &row, window, cx);
                        }
                    });
                });
            }));
        }
        // Grouping changes the list's shape, not which individual share links can be managed.
        // The original share rows have no nested groups; each section uses the same guarded actions.
        let others: Vec<_> = row.shares.iter().filter(|share| share.key != row.key).collect();
        if !others.is_empty() {
            menu = menu.separator().label(format!("Other shares ({})", others.len()));
            for share in others {
                menu = menu
                    .separator()
                    .label(format!("{} · {}", share.audience, share.version));
                menu = self.library_menu(menu, share, cx);
            }
        }
        menu
    }

    pub(super) fn library_more(&self, row: &Row, cx: &Context<Self>) -> AnyElement {
        let row = row.clone();
        let workspace = cx.entity().downgrade();
        let title = if row.shares.is_empty() {
            format!("More actions for {}", row.title)
        } else {
            let count = row.shares.iter().filter(|share| share.artifact_id().is_some()).count();
            format!("More actions for {} ({count} shares)", row.title)
        };
        div()
            .id(SharedString::from(format!("library-actions-{}", row.key)))
            .w(px(44.))
            .flex_none()
            .flex()
            .justify_end()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                Button::new(SharedString::from(format!("library-more-{}", row.key)))
                    .icon(Lucide::Ellipsis)
                    .ghost()
                    .w(px(32.))
                    .h(px(32.))
                    .accessibility_label(title.clone())
                    .tooltip(title)
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, cx| {
                        let Some(workspace) = workspace.upgrade() else {
                            return menu;
                        };
                        workspace.update(cx, |this, cx| this.library_menu(menu, &row, cx))
                    }),
            )
            .into_any_element()
    }

    /// The open page's title menu in the Artifacts header: the Library's menu for its item, without Open (it is open)
    /// and Share (the header's own button). While the Library has no row for it, only Open in browser.
    pub(in crate::workspace) fn page_menu(&self, menu: PopupMenu, cx: &Context<Self>) -> PopupMenu {
        if let Some(row) = self.page_item() {
            return self.item_menu(menu, row, &[Action::Open, Action::Share], cx);
        }
        let Some(link) = self.page_link().map(str::to_string) else {
            return menu;
        };
        menu.item(
            PopupMenuItem::new(Action::Browser.label()).on_click(move |_, _, cx| crate::external_link::open(&link, cx)),
        )
    }

    /// The header's Share: the Library's Share dialog for the open page, beside it.
    pub(in crate::workspace) fn share_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.page_item().cloned() {
            self.library_action(Action::Share, &row, window, cx);
        }
    }

    fn library_action(&mut self, action: Action, row: &Row, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(Ok(lists)) = &self.library.lists else {
            return;
        };
        let Some(row) = lists.find(&row.key).cloned() else {
            return;
        };
        let admin = self
            .account
            .as_ref()
            .is_some_and(|a| matches!(a.role.as_str(), "owner" | "admin"));
        if !row.actions(admin).contains(&action) {
            return;
        }
        let row = &row;
        self.library.note = None;
        match action {
            Action::Open => self.open_row(row, cx),
            Action::Browser | Action::Access => {
                let value = match &row.open {
                    Open::Browser(url) => url.clone(),
                    _ => self.org_path(&row.path),
                };
                if let Some(url) = library::reader_url(&self.gateway.site_url(), &value) {
                    crate::external_link::open(&url, cx);
                }
            }
            Action::Copy => {
                if let Some(url) = row
                    .link
                    .as_deref()
                    .and_then(|link| library::reader_url(&self.gateway.site_url(), link))
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(url));
                    self.notice = Some("Link copied. Access stays unchanged.".into());
                } else {
                    self.library.note = Some("This item has no usable share link. Choose Share to create one.".into());
                }
            }
            Action::Export => self.export_library_row(row, cx),
            Action::Reference => self.open_add_to_session(row, window, cx),
            Action::Share | Action::Versions | Action::Revoke | Action::Delete => {
                let workspace = cx.entity().downgrade();
                let fixer = self.fixer(cx);
                let sheet = cx.new(|cx| {
                    LibraryDialog::new(
                        account_gateway.clone(),
                        row.clone(),
                        action,
                        self.account.as_ref().map(|a| a.org_name.clone()),
                        workspace,
                        fixer,
                        window,
                        cx,
                    )
                });
                let focus = sheet.read(cx).focus.clone();
                window.focus(&focus, cx);
                self.library.dialog = Some(sheet);
                // The open page's ⋯ opens it on any screen; the Library's own rows only on the Library.
                self.library.dialog_beside_page = !matches!(self.view, View::Library);
            }
        }
        cx.notify();
    }

    fn export_library_row(&mut self, row: &Row, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(id) = row.artifact_id() else { return };
        let request = account_gateway.text(format!("/v1/artifacts/{}.md", nexus_client::url::enc(id)));
        let name = super::super::chat_menu::markdown_filename(&row.title);
        cx.spawn(async move |this, cx| {
            let markdown = match request.await {
                Ok(markdown) => markdown,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.library.note = Some(format!("Could not download Markdown: {error}").into());
                        cx.notify();
                    });
                    return;
                }
            };
            let folder = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
            let picked = cx.update(|cx| cx.prompt_for_new_path(&folder, Some(&name)));
            if let Ok(Ok(Some(file))) = picked.await
                && let Err(error) = std::fs::write(&file, markdown)
            {
                let _ = this.update(cx, |this, cx| {
                    this.library.note = Some(format!("Could not save {}: {error}", file.display()).into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(in crate::workspace) fn render_library_dialog(&self, cx: &App) -> Option<AnyElement> {
        if !matches!(self.view, View::Library) && !(self.library.dialog_beside_page && self.page.is_some()) {
            return None;
        }
        let sheet = self.library.dialog.as_ref()?;
        (!sheet.read(cx).closed
            && self
                .account_gateway()
                .is_some_and(|gateway| sheet.read(cx).matches(&gateway)))
        .then(|| sheet.clone().into_any_element())
    }
}

#[cfg(test)]
#[path = "library_action_tests.rs"]
mod tests;
