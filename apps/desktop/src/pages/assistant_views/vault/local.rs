//! The Passwords view's device half: the Nexus vault in this Mac's Keychain, 1Password through `op`, Touch ID,
//! and the index sent to the agent API. The rules are `nexus_desktop_core::vault_local`, `vault_import` and `vault_op`,
//! and the blocking work is `crate::vault_device::Device`, run on the background executor.
//!
//! The page holds metadata only. A password is in the page for one case: Show, after Touch ID, for fifteen seconds.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::super::super::ui;
use super::super::{call, modal, pill, primary_pill, quiet, quiet_pill, sheet};
use super::{Load, Ready, VaultPage, VaultPart, foot, label, table_row, text};
use crate::agent::Call;
use crate::palette::Palette;
use crate::vault_device::{Device, Loaded};
use nexus_desktop_core::agent_vault::{self as words, Flag, Grant, ManagerKind, Request};
use nexus_desktop_core::vault_local::{Edit, LoginMeta, NewLogin, Secret};
use nexus_desktop_core::vault_op::{self, OpState};

/// What the device's vault read as.
pub(super) enum Local {
    Loading,
    Ready(Box<Loaded>),
    Failed(String),
}

/// The login sheet that is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Sheet {
    /// Add (None) or edit one login of the Nexus vault.
    Login(Option<String>),
    /// The question before Delete all.
    DeleteAll,
}

/// The login sheet's four fields.
pub(super) struct Fields {
    name: Entity<InputState>,
    site: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
}

impl Fields {
    pub(super) fn new(window: &mut Window, cx: &mut Context<VaultPage>) -> Fields {
        Fields {
            name: cx.new(|cx| InputState::new(window, cx).placeholder("Delta")),
            site: cx.new(|cx| InputState::new(window, cx).placeholder("delta.com")),
            username: cx.new(|cx| InputState::new(window, cx).placeholder("Email or username")),
            password: cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("Password")),
        }
    }

    pub(super) fn subscribe(&self, window: &mut Window, cx: &mut Context<VaultPage>) -> Vec<Subscription> {
        [&self.name, &self.site, &self.username, &self.password]
            .into_iter()
            .map(|field| {
                cx.subscribe_in(
                    field,
                    window,
                    |page: &mut VaultPage, _, event: &InputEvent, window, cx| {
                        if let InputEvent::PressEnter { .. } = event {
                            page.save_login(window, cx);
                        }
                    },
                )
            })
            .collect()
    }

    fn set(&self, name: &str, site: &str, username: &str, window: &mut Window, cx: &mut Context<VaultPage>) {
        for (field, value) in [
            (&self.name, name),
            (&self.site, site),
            (&self.username, username),
            (&self.password, ""),
        ] {
            field.update(cx, |state, cx| state.set_value(value.to_string(), window, cx));
        }
    }

    fn clear_password(&self, window: &mut Window, cx: &mut Context<VaultPage>) {
        self.password
            .update(cx, |state, cx| state.set_value(String::new(), window, cx));
    }
}

/// The foot of the Nexus vault: where a login lives and who sees what.
pub(super) const DEVICE_FOOT: &str = "Kept in this Mac’s Keychain. A password never leaves this device: Nexus gets names and sites only, and each use takes Touch ID.";
/// What an approved grant ends at, while no browser exists to type the login.
pub(super) const READY_ON_DEVICE: &str = "Approved. It is ready on this device. Nexus has no browser of its own to type it into yet, so nothing was filled in.";

/// Whether a login id belongs to this device's vault (the Nexus vault or 1Password), not to a source this device does
/// not hold.
pub(super) fn on_device(id: &str) -> bool {
    id.starts_with("nx_") || id.starts_with("op_")
}

/// The sources an import names.
const IMPORT_FROM: [(&str, &str); 4] = [
    ("Chrome", "Settings, then Passwords, then Export passwords"),
    ("1Password", "File, then Export, then CSV"),
    ("Bitwarden", "Tools, then Export vault, then .csv"),
    ("A CSV file", "From any password manager"),
];

impl VaultPage {
    /// Run a blocking job on the background executor, then give its answer to `then`.
    fn work<T: Send + 'static>(
        &mut self,
        job: impl FnOnce(Arc<Device>) -> T + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let device = self.device.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let done = cx.background_spawn(async move { job(device) }).await;
            this.update_in(cx, |page, window, cx| then(page, done, window, cx)).ok();
        });
        // A job may be a write: never cancel one by replacing it.
        self.tasks.push(task);
    }

    pub(super) fn start_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.work(
            |device| device.load(),
            window,
            cx,
            |page, loaded, window, cx| page.loaded(loaded, true, window, cx),
        );
    }

    /// The device's vault was read: keep it, and send the index when `push`.
    fn loaded(&mut self, loaded: Result<Loaded, String>, push: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.local = match loaded {
            Ok(loaded) => Local::Ready(Box::new(loaded)),
            Err(error) => Local::Failed(error),
        };
        self.op_busy = false;
        cx.notify();
        if push {
            self.push_index(window, cx);
        }
    }

    /// Read the device again, and send the index.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.work(
            |device| device.load(),
            window,
            cx,
            |page, loaded, window, cx| page.loaded(loaded, true, window, cx),
        );
    }

    fn op_ready(&self) -> bool {
        matches!(&self.local, Local::Ready(loaded) if matches!(loaded.op_state, OpState::Ready { .. }))
    }

    /// Send the device's index to the agent API: names, sites, masked hints, sources, grants and last uses. The body is
    /// checked twice before a request exists (`vault_local::build_index`, `Request::index`); a body that fails is not
    /// sent, and the page says so.
    pub(super) fn push_index(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.over_chat {
            return;
        }
        let op_ready = self.op_ready();
        let device = self.device.clone();
        self._index = Some(cx.spawn_in(window, async move |this, cx| {
            let built = cx.background_spawn(async move { device.index(op_ready) }).await;
            this.update_in(cx, |page, window, cx| page.send_index(built, window, cx))
                .ok();
        }));
    }

    fn send_index(
        &mut self,
        built: Result<nexus_desktop_core::vault_local::Built, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.device.native {
            return;
        }
        let built = match built {
            Ok(built) => built,
            Err(error) => {
                self.index_note = Some(error);
                cx.notify();
                return;
            }
        };
        self.index_note = (built.left_out > 0).then(|| {
            format!(
                "{} logins are not shared with Nexus: their names look like a secret. Rename them to share them.",
                built.left_out
            )
        });
        let Some(request) = Request::index(built.body) else {
            self.index_note = Some("The index was not sent: it holds something shaped like a secret.".into());
            cx.notify();
            return;
        };
        self._index_call = Some(call(
            &self.ctx,
            Call::Vault(request),
            window,
            cx,
            |page, reply, _, cx| {
                if words::vault_off(reply.status, reply.body.as_ref()) {
                    // Nexus has no vault here: nothing to tell the person.
                    return;
                }
                match reply
                    .ok()
                    .then(|| reply.body.as_ref().and_then(words::vault_changed_of))
                    .flatten()
                {
                    Some(changed) if !page.working => {
                        page.now = chrono::Utc::now();
                        page.load = Load::Ready(Box::new(Ready::of(changed.vault, false, page.now)));
                    }
                    Some(_) => {}
                    None => {
                        page.index_note =
                            Some("Nexus could not take your vault’s index now. Your logins are unchanged.".into())
                    }
                }
                cx.notify();
            },
        ));
    }

    pub(super) fn login_meta(&self, id: &str) -> Option<LoginMeta> {
        let Local::Ready(loaded) = &self.local else { return None };
        loaded
            .nexus
            .iter()
            .chain(loaded.op.iter())
            .find(|login| login.id == id)
            .cloned()
    }

    /// Set what Nexus may do with a login of this device, then send the index.
    pub(super) fn device_grant(&mut self, id: &str, grant: Option<Grant>, window: &mut Window, cx: &mut Context<Self>) {
        let id = id.to_string();
        self.work(
            move |device| device.grant(&id, grant.as_ref()),
            window,
            cx,
            |page, done, window, cx| {
                if let Err(error) = done {
                    page.status = Some(error);
                }
                page.reload(window, cx);
            },
        );
    }

    // 1Password.

    /// Check `op` again, without listing items.
    pub(super) fn check_op(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.op_busy = true;
        cx.notify();
        self.reload(window, cx);
    }

    /// Refresh 1Password's items. 1Password asks for Touch ID in its own app. Names and sites come back, never a password.
    pub(super) fn refresh_op(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.op_busy = true;
        self.status = None;
        cx.notify();
        self.work(
            |device| device.refresh_op(),
            window,
            cx,
            |page, loaded, window, cx| {
                if let Err(error) = &loaded {
                    page.status = Some(format!("1Password did not answer: {error}"));
                }
                page.loaded(loaded, true, window, cx);
            },
        );
    }

    // The Nexus vault.

    /// Open the login sheet: empty to add, filled to edit.
    pub fn open_login(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let meta = id.as_deref().and_then(|id| self.login_meta(id));
        let (name, site, user) = meta
            .as_ref()
            .map(|login| (login.name.as_str(), login.site.as_str(), login.username.as_str()))
            .unwrap_or(("", "", ""));
        self.fields.set(name, site, user, window, cx);
        self.sheet = Some(Sheet::Login(id));
        self.problem = None;
        self.hide(cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn close_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        self.problem = None;
        self.hide(cx);
        self.fields.clear_password(window, cx);
        cx.notify();
    }

    /// Save the sheet: add a login, or change the one open.
    pub fn save_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::Login(id)) = self.sheet.clone() else {
            return;
        };
        let read = |field: &Entity<InputState>| field.read(cx).value().to_string();
        let (name, site, username, password) = (
            read(&self.fields.name),
            read(&self.fields.site),
            read(&self.fields.username),
            read(&self.fields.password),
        );
        let password = Secret::new(password);
        self.fields.clear_password(window, cx);
        self.working = true;
        self.problem = None;
        let site_for_name = site.clone();
        self.work(
            move |device| match id {
                None => {
                    let name = if name.trim().is_empty() { site_for_name } else { name };
                    device.add(NewLogin {
                        name,
                        site,
                        username,
                        password,
                    })
                }
                Some(id) => device.edit(
                    &id,
                    Edit {
                        name: Some(name),
                        site: Some(site),
                        username: Some(username),
                        password: (!password.is_empty()).then_some(password),
                    },
                ),
            },
            window,
            cx,
            |page, done, window, cx| {
                page.working = false;
                match done {
                    Ok(()) => {
                        page.sheet = None;
                        page.status = Some("Saved in your Keychain.".into());
                        page.reload(window, cx);
                    }
                    Err(error) => {
                        page.problem = Some(error);
                        cx.notify();
                    }
                }
            },
        );
    }

    pub(super) fn delete_login(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(login) = self.login_meta(id) else { return };
        let was_granted = login.granted();
        self.sheet = None;
        self.hide(cx);
        self.working = true;
        let (id, name) = (id.to_string(), login.name.clone());
        self.work(
            move |device| device.delete(&id, &name),
            window,
            cx,
            move |page, done, window, cx| {
                page.working = false;
                page.status = Some(match done {
                    Ok(()) if was_granted => format!("{} deleted. Nexus can’t use it now.", login.name),
                    Ok(()) => format!("{} deleted.", login.name),
                    Err(error) => error,
                });
                page.reload(window, cx);
            },
        );
    }

    pub(super) fn ask_delete_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = Some(Sheet::DeleteAll);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Delete every login in the Keychain, and tell Nexus the vault is empty.
    pub fn delete_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sheet = None;
        self.hide(cx);
        self.working = true;
        self.work(
            |device| device.delete_all(),
            window,
            cx,
            |page, done, window, cx| {
                page.working = false;
                page.status = Some(match done {
                    Ok(()) => "Every login is deleted from this Mac’s Keychain. Nexus can’t use any.".into(),
                    Err(error) => error,
                });
                page.reload(window, cx);
            },
        );
    }

    // Import.

    /// Ask for the CSV, and import it.
    pub(super) fn pick_csv(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update_in(cx, |page, window, cx| page.import_file(path, window, cx))
                .ok();
        });
        self.tasks.push(task);
    }

    fn import_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.importing = Some(Some("Reading the file on this Mac…".into()));
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let bytes = cx
                .background_spawn(async move {
                    // The size is checked before the file is read.
                    match std::fs::metadata(&path) {
                        Ok(meta) if meta.len() as usize > nexus_desktop_core::vault_import::MAX_BYTES => {
                            Err("That file is larger than 8 MB, so it is not a password export.".to_string())
                        }
                        Ok(_) => std::fs::read(&path).map_err(|_| "That file could not be read.".to_string()),
                        Err(_) => Err("That file could not be read.".to_string()),
                    }
                })
                .await;
            this.update_in(cx, |page, window, cx| match bytes {
                Ok(bytes) => page.import_bytes(bytes, window, cx),
                Err(error) => {
                    page.importing = Some(Some(error));
                    cx.notify();
                }
            })
            .ok();
        });
        self.tasks.push(task);
    }

    /// Import an export's bytes into the Nexus vault. The text is dropped as soon as it is parsed.
    pub fn import_bytes(&mut self, bytes: Vec<u8>, window: &mut Window, cx: &mut Context<Self>) {
        self.working = true;
        self.work(
            move |device| device.import(bytes),
            window,
            cx,
            |page, done, window, cx| {
                page.working = false;
                match done {
                    Ok((format, report, skipped)) => {
                        let mut said = format!("Imported {} logins from {}.", report.added, format.name());
                        if report.duplicates > 0 {
                            said.push_str(&format!(" {} were already in the vault.", report.duplicates));
                        }
                        if skipped + report.skipped > 0 {
                            said.push_str(&format!(" {} rows were not logins.", skipped + report.skipped));
                        }
                        if report.over > 0 {
                            said.push_str(&format!(" {} did not fit: the vault holds {}.", report.over, nexus_desktop_core::vault_local::MAX_LOGINS));
                        }
                        said.push_str(" Nexus may use none until you allow it. Delete the CSV: it holds your passwords in plain text.");
                        page.importing = Some(Some(said));
                        page.reload(window, cx);
                    }
                    Err(error) => {
                        page.importing = Some(Some(error));
                        cx.notify();
                    }
                }
            },
        );
    }

    // Show a password.

    /// Show a Nexus login's password for fifteen seconds, after Touch ID.
    pub fn reveal(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(login) = self.login_meta(id) else { return };
        let (id, name) = (id.to_string(), login.name);
        self.status = None;
        self.work(
            move |device| device.reveal(&id, &name).map(|secret| (id, secret)),
            window,
            cx,
            |page, done, _, cx| match done {
                Ok((id, secret)) => {
                    page.revealed = Some((id, secret));
                    page._reveal = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(Duration::from_secs(15)).await;
                        this.update(cx, |page, cx| page.hide(cx)).ok();
                    }));
                    cx.notify();
                }
                Err(sentence) => {
                    page.status = Some(sentence);
                    cx.notify();
                }
            },
        );
    }

    /// Hide a shown password. The `Secret` is wiped as it drops.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if self.revealed.take().is_some() {
            self._reveal = None;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn local_logins(&self) -> Vec<LoginMeta> {
        match &self.local {
            Local::Ready(loaded) => loaded.nexus.iter().chain(loaded.op.iter()).cloned().collect(),
            _ => Vec::new(),
        }
    }

    /// Fill the login sheet's fields, for the view tests.
    #[cfg(test)]
    pub fn fill(
        &mut self,
        name: &str,
        site: &str,
        username: &str,
        password: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fields.set(name, site, username, window, cx);
        self.fields
            .password
            .update(cx, |state, cx| state.set_value(password.to_string(), window, cx));
    }

    #[cfg(test)]
    pub fn shown(&self) -> Option<String> {
        self.revealed.as_ref().map(|(_, secret)| secret.expose().to_string())
    }

    #[cfg(test)]
    pub fn index_problem(&self) -> Option<&str> {
        self.index_note.as_deref()
    }
}

// Drawing.

fn source_card(icon: Lucide, name: &'static str, line: impl Into<SharedString>, palette: Palette) -> Div {
    super::super::card(palette).child(
        h_flex()
            .gap(px(12.))
            .child(ui::icon(icon, palette.icon).size(px(18.)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(div().text_size(px(14.)).text_color(palette.foreground).child(name))
                    .child(quiet(line, palette)),
            ),
    )
}

impl VaultPage {
    /// The two sources on the page: the Nexus vault on this Mac, and the person's 1Password.
    pub(super) fn sources(&self, cx: &mut Context<Self>, palette: Palette) -> Vec<AnyElement> {
        let Local::Ready(loaded) = &self.local else {
            return vec![match &self.local {
                Local::Failed(error) => {
                    ui::settings_notice("The vault on this Mac did not open", error.clone(), palette).into_any_element()
                }
                _ => quiet("Loading…", palette).into_any_element(),
            }];
        };
        if !self.device.native {
            return vec![
                source_card(
                    Lucide::KeyRound,
                    "Not connected",
                    "The Nexus vault and 1Password work on the Mac. Nexus can’t use a login here, and it never reads, stores or repeats a password.",
                    palette,
                )
                .into_any_element(),
            ];
        }
        let granted = |logins: &[LoginMeta]| logins.iter().filter(|login| login.granted()).count();
        let nexus_line = if loaded.nexus.is_empty() {
            "Logins kept in this Mac’s Keychain. None yet.".to_string()
        } else {
            format!(
                "{} logins in this Mac’s Keychain · Nexus may use {}",
                loaded.nexus.len(),
                granted(&loaded.nexus)
            )
        };
        let nexus = source_card(Lucide::KeyRound, "Nexus vault · on this Mac", nexus_line, palette);
        let nexus = nexus.child(
            h_flex()
                .gap(px(8.))
                .child(
                    pill("vault-open-nexus", "Open", palette)
                        .on_click(cx.listener(|page, _, window, cx| page.show(VaultPart::Nexus, window, cx))),
                )
                .child(
                    pill("vault-import", "Import", palette)
                        .on_click(cx.listener(|page, _, window, cx| page.open_import(window, cx))),
                ),
        );
        let (op_line, op_action): (String, Option<(&'static str, bool)>) = match &loaded.op_state {
            OpState::Ready { .. } if loaded.op.is_empty() => (
                "Connected through the 1Password app. Refresh to list your logins: names and sites only.".into(),
                Some(("Refresh", true)),
            ),
            OpState::Ready { .. } => (
                format!(
                    "{} logins · Nexus may use {}. 1Password keeps the passwords and asks for Touch ID itself.",
                    loaded.op.len(),
                    granted(&loaded.op)
                ),
                Some(("Choose logins", false)),
            ),
            OpState::NotInstalled => (
                "Not connected: the 1Password CLI (`op`) is not installed.".into(),
                Some(("See the steps", false)),
            ),
            OpState::IntegrationOff => (
                "Not connected: turn on “Integrate with 1Password CLI” in the 1Password app.".into(),
                Some(("See the steps", false)),
            ),
            OpState::Failed(why) => (format!("Not connected: {why}"), Some(("See the steps", false))),
        };
        let mut op = source_card(Lucide::KeyRound, "1Password", op_line, palette);
        if let Some((words, refresh)) = op_action {
            let button = if refresh {
                pill("vault-op-main", words, palette)
                    .on_click(cx.listener(|page, _, window, cx| page.refresh_op(window, cx)))
            } else {
                pill("vault-op-main", words, palette)
                    .on_click(cx.listener(|page, _, window, cx| page.show(VaultPart::OnePassword, window, cx)))
            };
            op = op.child(h_flex().child(button));
        }
        vec![nexus.into_any_element(), op.into_any_element()]
    }

    /// A source's own page: the Nexus vault or 1Password.
    pub(super) fn render_source(
        &self,
        kind: ManagerKind,
        mut page: Div,
        cx: &mut Context<Self>,
        palette: Palette,
    ) -> Div {
        let nexus = kind == ManagerKind::Nexus;
        let Local::Ready(loaded) = &self.local else {
            return page.child(match &self.local {
                Local::Failed(error) => {
                    ui::settings_notice("The vault on this Mac did not open", error.clone(), palette).into_any_element()
                }
                _ => quiet("Loading…", palette).into_any_element(),
            });
        };
        if !self.device.native {
            return page.child(quiet(
                "The Nexus vault and 1Password work on the Mac. They are not connected here.",
                palette,
            ));
        }
        if !nexus {
            if let OpState::Ready { .. } = loaded.op_state {
            } else {
                return page.child(self.op_steps(&loaded.op_state, cx, palette));
            }
        }
        let logins: &[LoginMeta] = if nexus { &loaded.nexus } else { &loaded.op };
        let may_use = logins.iter().filter(|login| login.granted()).count();
        let sub = format!("{} logins · Nexus may use {may_use}", logins.len());
        let mut head = h_flex().gap(px(8.)).child(div().flex_1().child(quiet(sub, palette)));
        if nexus {
            head = head
                .child(
                    pill("vault-nexus-import", "Import", palette)
                        .on_click(cx.listener(|page, _, window, cx| page.open_import(window, cx))),
                )
                .child(
                    primary_pill("vault-nexus-add", "Add", palette)
                        .on_click(cx.listener(|page, _, window, cx| page.open_login(None, window, cx))),
                );
        } else if self.op_busy {
            head = head.child(quiet_pill("vault-op-refresh", "Refreshing…", palette));
        } else {
            head = head.child(
                pill("vault-op-refresh", "Refresh", palette)
                    .on_click(cx.listener(|page, _, window, cx| page.refresh_op(window, cx))),
            );
        }
        page = page.child(head);
        if !nexus {
            page = page.child(quiet(
                "Nexus lists names and sites only. It reads a password from 1Password only for a use you approve, here, with Touch ID.",
                palette,
            ));
        }
        if logins.is_empty() {
            page = page.child(quiet(
                if nexus {
                    "No logins yet. Add one, or import a CSV."
                } else {
                    "No logins listed yet. Choose Refresh."
                },
                palette,
            ));
        } else {
            let mut table = v_flex().w_full().child(
                table_row(
                    vec![
                        (Some(190.), label("Login", palette)),
                        (None, label("Site", palette)),
                        (Some(110.), label("Account", palette)),
                        (Some(110.), label("Nexus may use", palette)),
                        (Some(90.), label("Note", palette)),
                        (Some(130.), div().into_any_element()),
                    ],
                    palette,
                )
                .border_t_0(),
            );
            for (at, login) in logins.iter().enumerate() {
                let (switch_id, edit_id, show_id) = (login.id.clone(), login.id.clone(), login.id.clone());
                let on = login.granted();
                let switch = ui::switch(("vault-source-switch", at), on, self.working, palette)
                    .when(!self.working, |switch| {
                        switch.on_click(cx.listener(move |page, _, window, cx| page.allow(&switch_id, !on, window, cx)))
                    })
                    .into_any_element();
                let shown = self.revealed.as_ref().filter(|(id, _)| *id == login.id);
                let account = match shown {
                    Some((_, secret)) => text(secret.expose().to_string(), palette.foreground),
                    None => text(
                        if login.hint.is_empty() {
                            "—".to_string()
                        } else {
                            login.hint.clone()
                        },
                        palette.muted,
                    ),
                };
                let mut actions = h_flex().gap(px(6.));
                if nexus {
                    actions = actions
                        .child(if shown.is_some() {
                            pill(("vault-hide", at), "Hide", palette)
                                .on_click(cx.listener(|page, _, _, cx| page.hide(cx)))
                        } else {
                            pill(("vault-show", at), "Show", palette)
                                .on_click(cx.listener(move |page, _, window, cx| page.reveal(&show_id, window, cx)))
                        })
                        .child(pill(("vault-edit", at), "Edit", palette).on_click(
                            cx.listener(move |page, _, window, cx| page.open_login(Some(edit_id.clone()), window, cx)),
                        ));
                }
                let note = match (on, login.flag) {
                    (true, _) => text(words::grant_label(login.grant.as_ref(), &chrono::Local), palette.faint),
                    (false, Some(Flag::Weak)) => text("Weak", palette.faint),
                    (false, Some(Flag::Reused)) => text("Reused", palette.faint),
                    (false, None) => div().into_any_element(),
                };
                table = table.child(table_row(
                    vec![
                        (Some(190.), text(login.name.clone(), palette.foreground)),
                        (None, text(login.site.clone(), palette.faint)),
                        (Some(110.), account),
                        (Some(110.), switch),
                        (Some(90.), note),
                        (Some(130.), actions.into_any_element()),
                    ],
                    palette,
                ));
            }
            page = page.child(table);
        }
        page = page.child(foot(
            if nexus {
                DEVICE_FOOT
            } else {
                "1Password keeps the passwords. Nexus never sees them, stores them or repeats them."
            },
            palette,
        ));
        if nexus && !loaded.nexus.is_empty() {
            page = page.child(
                h_flex().child(
                    pill("vault-delete-all", "Delete all", palette)
                        .on_click(cx.listener(|page, _, window, cx| page.ask_delete_all(window, cx))),
                ),
            );
        }
        page
    }

    /// What the person does when `op` is missing or its app integration is off.
    fn op_steps(&self, state: &OpState, cx: &mut Context<Self>, palette: Palette) -> Div {
        let (title, lede) = match state {
            OpState::NotInstalled => (
                "1Password is not connected",
                "Nexus uses the 1Password CLI (`op`) with your own 1Password app. It needs no admin and no service account, and it never installs anything.",
            ),
            OpState::IntegrationOff => (
                "Turn on 1Password’s CLI integration",
                "`op` is installed, and the 1Password app does not share an account with it yet.",
            ),
            OpState::Failed(why) => ("1Password did not answer", why.as_str()),
            OpState::Ready { .. } => ("1Password is connected", ""),
        };
        let steps = vault_op::STEPS.iter().enumerate().map(|(at, step)| {
            h_flex()
                .gap(px(10.))
                .items_start()
                .child(
                    div()
                        .w(px(18.))
                        .flex_none()
                        .text_size(px(13.))
                        .text_color(palette.faint)
                        .child(format!("{}.", at + 1)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.))
                        .text_color(palette.foreground)
                        .child(*step),
                )
        });
        super::super::card(palette)
            .gap(px(10.))
            .child(div().text_size(px(15.)).text_color(palette.strong).child(title))
            .child(quiet(lede.to_string(), palette))
            .children(steps)
            .child(h_flex().child(if self.op_busy {
                quiet_pill("vault-op-check", "Checking…", palette)
            } else {
                pill("vault-op-check", "Check again", palette)
                    .on_click(cx.listener(|page, _, window, cx| page.check_op(window, cx)))
            }))
    }

    /// The login sheet: add or edit one Nexus login. The password field is masked and is wiped when the sheet closes.
    pub(super) fn login_sheet(&self, id: Option<&str>, cx: &mut Context<Self>, palette: Palette) -> Stateful<Div> {
        let editing = id.is_some();
        let field = |name: &'static str, state: &Entity<InputState>| {
            v_flex().gap(px(4.)).child(label(name, palette)).child(
                h_flex()
                    .w_full()
                    .h(px(32.))
                    .px(px(6.))
                    .rounded(px(8.))
                    .bg(palette.hover)
                    .text_size(px(13.))
                    .child(div().flex_1().min_w(px(0.)).child(Input::new(state).appearance(false))),
            )
        };
        let save = if self.working {
            quiet_pill("vault-login-save", "Saving…", palette)
        } else {
            primary_pill("vault-login-save", "Save", palette)
                .on_click(cx.listener(|page, _, window, cx| page.save_login(window, cx)))
        };
        let delete = id.map(|id| {
            let id = id.to_string();
            pill("vault-login-delete", "Delete", palette)
                .on_click(cx.listener(move |page, _, window, cx| page.delete_login(&id, window, cx)))
        });
        modal(
            "vault-scrim",
            sheet("vault-login-sheet", &self.focus, 420., palette)
                .on_key_down(cx.listener(|page, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        page.close_sheet(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(div().text_size(px(16.)).text_color(palette.strong).child(if editing {
                    "Edit login"
                } else {
                    "Add a login"
                }))
                .child(field("Name", &self.fields.name))
                .child(field("Site", &self.fields.site))
                .child(field("Username", &self.fields.username))
                .child(field("Password", &self.fields.password))
                .when(editing, |sheet| {
                    sheet.child(quiet("Leave the password empty to keep it.", palette))
                })
                .children(self.problem.clone().map(|problem| ui::error_line(problem, palette)))
                .child(foot(DEVICE_FOOT, palette))
                .child(
                    h_flex()
                        .gap(px(8.))
                        .children(delete)
                        .child(div().flex_1())
                        .child(
                            pill("vault-login-cancel", "Cancel", palette)
                                .on_click(cx.listener(|page, _, window, cx| page.close_sheet(window, cx))),
                        )
                        .child(save),
                ),
            palette,
        )
    }

    /// The question before Delete all.
    pub(super) fn delete_all_sheet(&self, cx: &mut Context<Self>, palette: Palette) -> Stateful<Div> {
        modal(
            "vault-scrim",
            sheet("vault-delete-sheet", &self.focus, 400., palette)
                .on_key_down(cx.listener(|page, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        page.close_sheet(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(div().text_size(px(16.)).text_color(palette.strong).child("Delete every login?"))
                .child(quiet(
                    "This removes each login from this Mac’s Keychain, and Nexus loses every grant. It can’t be undone. Your 1Password is not touched.",
                    palette,
                ))
                .child(
                    h_flex()
                        .gap(px(8.))
                        .justify_end()
                        .child(pill("vault-delete-cancel", "Cancel", palette).on_click(cx.listener(|page, _, window, cx| page.close_sheet(window, cx))))
                        .child(
                            super::super::danger_pill("vault-delete-confirm", "Delete all", palette)
                                .on_click(cx.listener(|page, _, window, cx| page.delete_all(window, cx))),
                        ),
                ),
            palette,
        )
    }

    /// The import sheet: choose the CSV a password manager exported. It is read on this Mac.
    pub(super) fn import_sheet(&self, said: Option<&str>, cx: &mut Context<Self>, palette: Palette) -> Stateful<Div> {
        let sources = v_flex()
            .gap(px(4.))
            .children(IMPORT_FROM.iter().enumerate().map(|(at, (name, does))| {
                h_flex()
                    .id(("vault-source", at))
                    .gap(px(10.))
                    .p(px(10.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(palette.border)
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.hover))
                    .child(ui::icon(
                        if at == 3 { Lucide::File } else { Lucide::KeyRound },
                        palette.icon,
                    ))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .child(div().text_size(px(13.)).text_color(palette.foreground).child(*name))
                            .child(div().text_size(px(12.)).text_color(palette.faint).child(*does)),
                    )
                    .child(ui::icon(Lucide::ChevronRight, palette.icon))
                    .on_click(cx.listener(|page, _, window, cx| page.pick_csv(window, cx)))
            }));
        modal(
            "vault-scrim",
            sheet("vault-import-sheet", &self.focus, 460., palette)
                .on_key_down(cx.listener(|page, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        page.close_import(cx);
                        cx.stop_propagation();
                    }
                }))
                .child(
                    h_flex()
                        .child(div().flex_1().text_size(px(16.)).text_color(palette.strong).child("Import passwords"))
                        .child(pill("vault-import-close", "Close", palette).on_click(cx.listener(|page, _, _, cx| page.close_import(cx)))),
                )
                .child(quiet("Export a CSV from your password manager, then choose the file. It is read on this Mac and nothing is sent.", palette))
                .child(sources)
                .when_some(said.map(str::to_string), |sheet, said| sheet.child(ui::settings_notice("Import", said, palette)))
                .child(foot(DEVICE_FOOT, palette)),
            palette,
        )
    }
}

/// The index note and the status, for the page.
pub(super) fn notes(page: &VaultPage, palette: Palette) -> Vec<AnyElement> {
    page.index_note
        .iter()
        .map(|note| ui::error_line(note.clone(), palette).into_any_element())
        .collect()
}
