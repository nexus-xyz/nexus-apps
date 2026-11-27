//! Passwords and payments, as Nexus UI's `agent-vault.tsx` and `agent-vault-import.tsx`:
//! give access without giving secrets.
//!
//! - The page: the linked password manager and the logins Nexus may ask to use, each with its grant, when it
//!   was last used and Revoke; then the Nexus card, the month's spend, Freeze, and a limit per merchant.
//! - A login's menu: See where it was used, Change to: this once, Revoke now, then what a revoke stopped.
//! - The grant sheet: one login, one site, one task, for this once or until a date, Decline or Approve.
//! - The activity log: every password used and every payment, who approved it and where, and Export.
//! - The Nexus vault and the import sheet, and the person's own 1Password: the device half is
//!   `vault/local.rs`. Logins live in this Mac's Keychain, 1Password's through `op`, and this device sends
//!   the agent API an index of names, sites, masked hints, grants and last uses, never a secret.
//!
//! Nexus never sees a password or a card number: the view draws names, sites, masked accounts and grants, which is all
//! the agent API sends, and `nexus_desktop_core::agent_vault` refuses any answer shaped like a secret. While the agent
//! API has no vault (`vault_off`), the page says no password manager and no card is linked.
//!
//! A grant for a login on this device is approved here, with Touch ID (`crate::touch_id`), and ends at "approved, ready
//! on this device": no agent browser exists to type it yet (`vault_local::fill_login` is the seam). A grant for a
//! login this device does not hold keeps the web's words: the agent API asks for Face ID on the person's phone
//! (`confirm_on_device`).

mod local;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::super::{Ctx, palette, ui};
use super::{Reply, call, card, frame, head, pill, primary_pill, quiet, quiet_pill};
use crate::agent::Call;
use crate::palette::Palette;
use crate::vault_device::Device;
use local::{Fields, Local, Sheet};
use nexus_desktop_core::agent_vault::{
    self as words, Approval, Ask, Changed, Entry, For, Grant, LoginChange, ManagerKind, Read, Request, VaultView,
};
use nexus_desktop_core::vault_local::{ApprovedUse, Secret};

/// Which part of the view shows: the page, its activity log, or the Nexus vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultPart {
    Main,
    Activity,
    Nexus,
    OnePassword,
}

/// What this device does once Nexus answered a change.
enum Follow {
    Nothing,
    /// Keep a grant here (None: revoked) and send the index.
    Grant(String, Option<Grant>),
    /// Keep an approved use here (with the date an "Until" choice ran to), when Nexus took the approval.
    Approved {
        ask: Ask,
        until: Option<String>,
    },
}

/// The vault as read: loading, the view, off (drawn as nothing linked), or a failure.
enum Load {
    Loading,
    Ready(Box<Ready>),
    Failed,
}

/// The activity log as read.
enum Log {
    Loading,
    Entries(Vec<LogRow>),
    Off,
    Failed,
}

/// One login's row, its words computed when the vault was read.
struct LoginRow {
    id: String,
    name: String,
    site: String,
    grant: String,
    when: String,
    granted: bool,
    until: bool,
    source: Option<&'static str>,
}

struct MerchantRow {
    merchant: String,
    limit: String,
    approval: Approval,
}

struct AskRow {
    ask: Ask,
    login: String,
    until: Option<String>,
    filler: &'static str,
}

struct LogRow {
    entry: Entry,
    when: String,
}

/// The vault once read, with its words.
struct Ready {
    view: VaultView,
    /// The agent API has no vault here: the page says nothing is linked.
    off: bool,
    logins: Vec<LoginRow>,
    merchants: Vec<MerchantRow>,
    asks: Vec<AskRow>,
    spent: Option<(String, f32)>,
}

impl Ready {
    fn of(view: VaultView, off: bool, now: DateTime<Utc>) -> Ready {
        let zone = chrono::Local;
        let logins = view
            .logins
            .iter()
            .map(|login| LoginRow {
                id: login.id.clone(),
                name: login.name.clone(),
                site: login.site.clone(),
                grant: words::grant_label(login.grant.as_ref(), &zone),
                when: words::vault_when(login.last_used.as_deref(), now, &zone),
                granted: login.grant.is_some(),
                until: matches!(login.grant, Some(Grant::Until(_))),
                source: match login.source {
                    Some(ManagerKind::OnePassword) => Some("1Password"),
                    Some(ManagerKind::Nexus) => Some("Nexus vault"),
                    Some(ManagerKind::Bitwarden) => Some("Bitwarden"),
                    None => None,
                },
            })
            .collect();
        let merchants = view
            .merchants
            .iter()
            .map(|merchant| MerchantRow {
                merchant: merchant.merchant.clone(),
                limit: words::limit_label(merchant),
                approval: merchant.approval,
            })
            .collect();
        let asks = view
            .asks
            .iter()
            .map(|ask| AskRow {
                login: format!("{} · {} ({})", ask.login_name, ask.account, ask.manager.name()),
                until: ask
                    .until
                    .as_deref()
                    .map(|until| format!("Until {}", words::vault_day(until, &zone))),
                filler: ask.manager.filler(),
                ask: ask.clone(),
            })
            .collect();
        Ready {
            spent: view
                .card
                .as_ref()
                .map(|card| (words::spent_line(card), words::spent_share(card))),
            logins,
            merchants,
            asks,
            off,
            view,
        }
    }
}

pub struct VaultPage {
    ctx: Ctx,
    part: VaultPart,
    /// Drawn over the chat: only the grant sheets that wait, and what the last decision did.
    over_chat: bool,
    load: Load,
    log: Log,
    /// The login whose menu is open.
    menu: Option<String>,
    /// Each waiting ask's choice on the grant sheet: this once unless the person picks the date.
    choices: HashMap<String, For>,
    /// The import sheet is open, with what its last pick said.
    importing: Option<Option<String>>,
    status: Option<String>,
    working: bool,
    now: DateTime<Utc>,
    focus: FocusHandle,
    /// This device's vault: the Keychain, 1Password's `op` and Touch ID.
    device: Arc<Device>,
    local: Local,
    /// A login sheet or the Delete all question is open.
    sheet: Option<Sheet>,
    fields: Fields,
    /// The sheet's last problem, in words.
    problem: Option<String>,
    /// A password shown after Touch ID, for fifteen seconds: its login's id and the secret.
    revealed: Option<(String, Secret)>,
    /// `op` is being asked.
    op_busy: bool,
    /// Why the last index was not sent or not taken.
    index_note: Option<String>,
    /// Device jobs. A write is never cancelled by a newer job, so they are kept until the page goes.
    tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
    _index: Option<Task<()>>,
    _index_call: Option<Task<()>>,
    _reveal: Option<Task<()>>,
    _read: Task<()>,
    _log: Option<Task<()>>,
    _change: Option<Task<()>>,
    _export: Option<Task<()>>,
}

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Entity<VaultPage> {
    build_on(ctx, false, window, cx)
}

/// The grants that wait, over the chat (the web's `AgentVaultAsks`): each one's sheet, with Decline and Approve
/// as on the page. Nothing while none waits, the vault is off, or it did not open.
pub fn asks(ctx: Ctx, window: &mut Window, cx: &mut App) -> Entity<VaultPage> {
    build_on(ctx, true, window, cx)
}

fn build_on(ctx: Ctx, over_chat: bool, window: &mut Window, cx: &mut App) -> Entity<VaultPage> {
    cx.new(|cx| {
        let read = call(&ctx, Call::Vault(Request::Read), window, cx, VaultPage::read);
        let fields = Fields::new(window, cx);
        let subscriptions = fields.subscribe(window, cx);
        let mut page = VaultPage {
            ctx,
            part: VaultPart::Main,
            over_chat,
            load: Load::Loading,
            log: Log::Loading,
            menu: None,
            choices: HashMap::new(),
            importing: None,
            status: None,
            working: false,
            now: Utc::now(),
            focus: cx.focus_handle(),
            device: Device::system(),
            local: Local::Loading,
            sheet: None,
            fields,
            problem: None,
            revealed: None,
            op_busy: false,
            index_note: None,
            tasks: Vec::new(),
            _subscriptions: subscriptions,
            _index: None,
            _index_call: None,
            _reveal: None,
            _read: read,
            _log: None,
            _change: None,
            _export: None,
        };
        // The chat's sheets only wait for an answer: the device's vault is the Passwords view's.
        if !page.over_chat {
            page.start_local(window, cx);
        }
        page
    })
}

impl VaultPage {
    fn read(&mut self, reply: Reply, _: &mut Window, _: &mut Context<Self>) {
        self.now = Utc::now();
        self.load = match words::vault_read(reply.status, reply.body.as_ref()) {
            Read::View(view) => Load::Ready(Box::new(Ready::of(*view, false, self.now))),
            Read::Off => Load::Ready(Box::new(Ready::of(VaultView::default(), true, self.now))),
            Read::Failed(_) => Load::Failed,
        };
    }

    fn read_log(&mut self, reply: Reply, _: &mut Window, _: &mut Context<Self>) {
        let zone = chrono::Local;
        self.log = match words::activity_read(reply.status, reply.body.as_ref()) {
            words::Log::Entries(entries) => Log::Entries(
                entries
                    .into_iter()
                    .map(|entry| LogRow {
                        when: words::log_when(&entry.at, self.now, &zone),
                        entry,
                    })
                    .collect(),
            ),
            words::Log::Off => Log::Off,
            words::Log::Failed => Log::Failed,
        };
    }

    /// Read the vault again: over the chat, after each answer, since an answer may ask for a login. A read in flight
    /// is replaced.
    pub fn reread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.working {
            return;
        }
        self._read = call(&self.ctx, Call::Vault(Request::Read), window, cx, VaultPage::read);
    }

    /// The grant sheets drawn over the chat, by login name, for the view tests.
    #[cfg(test)]
    pub fn sheets(&self) -> Vec<String> {
        match &self.load {
            Load::Ready(ready) => ready.asks.iter().map(|row| row.ask.login_name.clone()).collect(),
            _ => Vec::new(),
        }
    }

    /// Show a part; the activity log is read each time it opens.
    pub fn show(&mut self, part: VaultPart, window: &mut Window, cx: &mut Context<Self>) {
        self.part = part;
        self.menu = None;
        self.hide(cx);
        if part == VaultPart::Activity {
            self.log = Log::Loading;
            self._log = Some(call(
                &self.ctx,
                Call::Vault(Request::Activity),
                window,
                cx,
                VaultPage::read_log,
            ));
        }
        cx.notify();
    }

    /// Run one write, keep the vault it answers, and say what happened.
    fn act(
        &mut self,
        made: Option<Request>,
        window: &mut Window,
        cx: &mut Context<Self>,
        said: impl FnOnce(&Changed) -> String + 'static,
    ) {
        self.act_then(made, Follow::Nothing, window, cx, said);
    }

    /// `act`, and then what this device does once Nexus answered ([`Follow`]).
    fn act_then(
        &mut self,
        made: Option<Request>,
        follow: Follow,
        window: &mut Window,
        cx: &mut Context<Self>,
        said: impl FnOnce(&Changed) -> String + 'static,
    ) {
        let Some(made) = made else { return };
        if self.working {
            return;
        }
        self.working = true;
        self.menu = None;
        self._change = Some(call(
            &self.ctx,
            Call::Vault(made),
            window,
            cx,
            move |page, reply, window, cx| {
                page.working = false;
                let changed = reply
                    .ok()
                    .then(|| reply.body.as_ref().and_then(words::vault_changed_of))
                    .flatten();
                // What Nexus decided, when it answered: an approval is kept here only when it says approved.
                let decided = changed.as_ref().and_then(|changed| changed.decided);
                page.status = Some(match changed {
                    Some(changed) => {
                        let line = said(&changed);
                        page.now = Utc::now();
                        page.load = Load::Ready(Box::new(Ready::of(changed.vault, false, page.now)));
                        line
                    }
                    None if reply.ok() => "That did not save. Try again in a moment.".into(),
                    None => words::change_failed(reply.status, reply.body.as_ref()).into(),
                });
                page.follow(follow, decided, window, cx);
            },
        ));
    }

    /// What this device does after Nexus answered a change. A revoke or a change of grant always applies here, since
    /// it only ever takes access away or sets what the person chose. An approval is kept only when Nexus answered
    /// "approved": a decline, an expired ask, a failed call or a wait for the phone leaves this device with no grant.
    fn follow(&mut self, follow: Follow, decided: Option<words::Decided>, window: &mut Window, cx: &mut Context<Self>) {
        match follow {
            Follow::Nothing => {}
            Follow::Grant(id, grant) => self.device_grant(&id, grant, window, cx),
            Follow::Approved { ask, until } if decided == Some(words::Decided::Approved) => {
                let used = ApprovedUse {
                    login: ask.login_id.clone(),
                    source: ask.manager,
                    site: ask.site.clone(),
                    task: ask.task.clone(),
                };
                let name = ask.login_name.clone();
                self.work_record(used, name, until, window, cx);
            }
            Follow::Approved { .. } => {}
        }
    }

    /// Keep an approved use on this device: the grant, the use and the activity line. Then send the index.
    fn work_record(
        &mut self,
        used: ApprovedUse,
        name: String,
        until: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let device = self.device.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let done = cx
                .background_spawn(async move { device.record_approved(&used, &name, until.as_deref()) })
                .await;
            this.update_in(cx, |page, window, cx| {
                if let Err(error) = done {
                    page.status = Some(error);
                }
                page.start_local(window, cx);
            })
            .ok();
        });
        self.tasks.push(task);
    }

    /// Revoke a login now.
    pub fn revoke(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.login_name(id);
        // Nexus stops the errands that use it first, then this device keeps the revoke and sends the index.
        let follow = if local::on_device(id) {
            Follow::Grant(id.to_string(), None)
        } else {
            Follow::Nothing
        };
        self.act_then(
            Request::login(id, &LoginChange::Revoke),
            follow,
            window,
            cx,
            move |changed| words::revoked_text(&name, changed.stopped.unwrap_or(0)),
        );
    }

    /// "Change to: this once": the login asks at each use again.
    pub fn each_use(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.login_name(id);
        if local::on_device(id) {
            self.device_grant(id, Some(Grant::EachUse), window, cx);
            self.status = Some(format!("{name}: each use asks you now."));
            return;
        }
        self.act(Request::login(id, &LoginChange::EachUse), window, cx, move |_| {
            format!("{name}: each use asks you now.")
        });
    }

    /// The Nexus vault's switch: Nexus may use a login, each use with Face ID, or not.
    pub fn allow(&mut self, id: &str, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .login_meta(id)
            .map(|login| login.name)
            .unwrap_or_else(|| self.login_name(id));
        if local::on_device(id) {
            if on {
                self.device_grant(id, Some(Grant::EachUse), window, cx);
                self.status = Some(format!("Nexus may use {name} now. Each use takes Touch ID."));
            } else {
                self.revoke(id, window, cx);
            }
            return;
        }
        let change = if on { LoginChange::EachUse } else { LoginChange::Revoke };
        self.act(Request::login(id, &change), window, cx, move |_| {
            if on {
                format!("Nexus may use {name} now. Each use takes Face ID.")
            } else {
                format!("{name} revoked.")
            }
        });
    }

    /// Freeze the card, or unfreeze it.
    pub fn freeze(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(ready) = &self.load else { return };
        let Some(card) = &ready.view.card else { return };
        self.act(Request::card(!card.frozen), window, cx, |changed| {
            words::frozen_text(changed.vault.card.as_ref().is_some_and(|card| card.frozen)).into()
        });
    }

    /// Decide a waiting ask: approve it for the sheet's choice, or decline it with None.
    pub fn decide(&mut self, ask: &str, approve: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(ready) = &self.load else { return };
        let Some(found) = ready.view.asks.iter().find(|each| each.id == ask).cloned() else {
            return;
        };
        let choice = approve.then(|| self.choices.get(ask).copied().unwrap_or(For::Once));
        if approve && local::on_device(&found.login_id) {
            self.decide_with_touch_id(found, choice, window, cx);
            return;
        }
        self.act(Request::decide(ask, choice), window, cx, move |changed| {
            words::decided_text(&found, changed.decided)
        });
    }

    /// A grant for a login on this device: Touch ID first, here. Without it nothing is sent and nothing changes. With
    /// it, Nexus is told, and the use ends at "approved, ready on this device".
    fn decide_with_touch_id(&mut self, ask: Ask, choice: Option<For>, window: &mut Window, cx: &mut Context<Self>) {
        if self.working {
            return;
        }
        self.working = true;
        self.status = None;
        cx.notify();
        let device = self.device.clone();
        let reason = crate::vault_device::use_reason(&ask.login_name, &ask.site);
        let task = cx.spawn_in(window, async move |this, cx| {
            let auth = cx
                .background_spawn(async move { device.confirm(&reason).map(|_| ()) })
                .await;
            this.update_in(cx, |page, window, cx| {
                page.working = false;
                match auth {
                    Err(sentence) => {
                        page.status = Some(if sentence == "Not approved." {
                            format!(
                                "Not approved. Nexus can’t use {} until you approve with Touch ID.",
                                ask.login_name
                            )
                        } else {
                            sentence
                        });
                        cx.notify();
                    }
                    Ok(()) => {
                        let until = (choice == Some(For::Until)).then(|| ask.until.clone()).flatten();
                        let made = Request::decide(&ask.id, choice);
                        let follow = Follow::Approved {
                            ask: ask.clone(),
                            until,
                        };
                        let asked = ask.clone();
                        page.act_then(made, follow, window, cx, move |changed| match changed.decided {
                            Some(words::Decided::Approved) => local::READY_ON_DEVICE.into(),
                            None => "Nexus did not record an approval. Nothing changed.".to_string(),
                            other => words::decided_text(&asked, other),
                        });
                    }
                }
            })
            .ok();
        });
        self.tasks.push(task);
    }

    /// The grant sheet's For: this once, or until the date the ask offers.
    pub fn choose(&mut self, ask: &str, choice: For, cx: &mut Context<Self>) {
        self.choices.insert(ask.to_string(), choice);
        cx.notify();
    }

    fn login_name(&self, id: &str) -> String {
        match &self.load {
            Load::Ready(ready) => ready
                .view
                .logins
                .iter()
                .find(|login| login.id == id)
                .map(|login| login.name.clone())
                .unwrap_or_else(|| "The login".into()),
            _ => "The login".into(),
        }
    }

    fn toggle_menu(&mut self, id: String, cx: &mut Context<Self>) {
        self.menu = if self.menu.as_deref() == Some(id.as_str()) {
            None
        } else {
            Some(id)
        };
        cx.notify();
    }

    /// The import sheet, with the focus in it so Esc closes it.
    pub fn open_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.importing = Some(None);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn close_import(&mut self, cx: &mut Context<Self>) {
        self.importing = None;
        cx.notify();
    }

    /// Export: the log as a CSV file, saved where the person picks.
    fn export(&mut self, cx: &mut Context<Self>) {
        let Log::Entries(rows) = &self.log else { return };
        let entries: Vec<Entry> = rows.iter().map(|row| row.entry.clone()).collect();
        let csv = words::activity_csv(&entries);
        self._export = Some(cx.spawn(async move |this, cx| {
            let folder = super::super::download::downloads_folder();
            let picked = cx.update(|cx| cx.prompt_for_new_path(&folder, Some(words::ACTIVITY_FILE)));
            let said = match picked.await {
                Ok(Ok(Some(path))) => match std::fs::write(&path, &csv) {
                    Ok(()) => Some(format!("Saved {}.", super::super::download::file_name(&path))),
                    Err(error) => Some(format!("Couldn't save {}: {error}", path.display())),
                },
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => Some(error.to_string()),
            };
            this.update(cx, |page, cx| {
                if said.is_some() {
                    page.status = said;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    #[cfg(test)]
    pub fn view(&self) -> Option<&VaultView> {
        match &self.load {
            Load::Ready(ready) => Some(&ready.view),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn off(&self) -> bool {
        matches!(&self.load, Load::Ready(ready) if ready.off)
    }

    #[cfg(test)]
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    #[cfg(test)]
    pub fn import_said(&self) -> Option<&str> {
        self.importing.as_ref().and_then(|said| said.as_deref())
    }

    /// The activity log's When and What, as drawn.
    #[cfg(test)]
    pub fn log(&self) -> Option<Vec<(String, String)>> {
        match &self.log {
            Log::Entries(rows) => Some(
                rows.iter()
                    .map(|row| (row.when.clone(), row.entry.what.clone()))
                    .collect(),
            ),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn open_menu(&mut self, id: &str, cx: &mut Context<Self>) {
        self.toggle_menu(id.to_string(), cx);
    }
}

/// One row of a table: its cells at their widths, the last ones flexing.
fn table_row(cells: Vec<(Option<f32>, AnyElement)>, palette: Palette) -> Div {
    h_flex()
        .w_full()
        .min_h(px(40.))
        .gap(px(12.))
        .py(px(6.))
        .border_t_1()
        .border_color(palette.border)
        .text_size(px(13.))
        .children(cells.into_iter().map(|(width, cell)| match width {
            Some(width) => div().w(px(width)).flex_none().min_w(px(0.)).child(cell),
            None => div().flex_1().min_w(px(0.)).child(cell),
        }))
}

fn text(value: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div().text_color(color).child(value.into()).into_any_element()
}

fn label(value: &'static str, palette: Palette) -> AnyElement {
    div()
        .text_size(px(12.))
        .text_color(palette.faint)
        .child(value)
        .into_any_element()
}

/// A crumb link back to the page.
fn crumb(part: &'static str, cx: &mut Context<VaultPage>, palette: Palette) -> Div {
    h_flex()
        .gap(px(4.))
        .text_size(px(12.))
        .text_color(palette.faint)
        .child(
            div()
                .id("vault-crumb")
                .cursor_pointer()
                .hover(|style| style.text_color(palette.foreground))
                .child("Passwords and payments")
                .on_click(cx.listener(|page, _, window, cx| page.show(VaultPart::Main, window, cx))),
        )
        .child("›")
        .child(part)
}

/// The quiet foot with the lock: how the Nexus vault keeps a login.
fn foot(line: impl Into<SharedString>, palette: Palette) -> Div {
    h_flex()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(palette.faint)
        .child(ui::icon(Lucide::Lock, palette.faint))
        .child(line.into())
}

impl VaultPage {
    fn render_main(&self, ready: &Ready, mut page: Div, cx: &mut Context<Self>, palette: Palette) -> Div {
        let working = self.working;
        page = page.child(
            h_flex().justify_end().child(
                pill("vault-activity", "Activity", palette)
                    .on_click(cx.listener(|page, _, window, cx| page.show(VaultPart::Activity, window, cx))),
            ),
        );
        for row in &ready.asks {
            page = page.child(self.grant_sheet(row, cx, palette));
        }

        // Passwords: the two sources on this device, then what Nexus can see of them.
        page = page.child(head("Passwords", palette));
        let view = &ready.view;
        for source in self.sources(cx, palette) {
            page = page.child(source);
        }
        if ready.off {
            page = page.child(quiet(
                "Nexus can’t keep an index of your vault here yet, so it can’t use any login. Your logins stay on this Mac.",
                palette,
            ));
        }
        for note in local::notes(self, palette) {
            page = page.child(note);
        }
        if !ready.logins.is_empty() {
            let mut table = v_flex().w_full().child(
                table_row(
                    vec![
                        (Some(180.), label("Login", palette)),
                        (None, label("Site", palette)),
                        (Some(170.), label("Grant", palette)),
                        (Some(110.), label("Last used", palette)),
                        (Some(80.), div().into_any_element()),
                    ],
                    palette,
                )
                .border_t_0(),
            );
            for (at, row) in ready.logins.iter().enumerate() {
                let id = row.id.clone();
                let name = div()
                    .id(("vault-login", at))
                    .cursor_pointer()
                    .text_color(palette.foreground)
                    .hover(|style| style.underline())
                    .child(row.name.clone())
                    .on_click(cx.listener(move |page, _, _, cx| page.toggle_menu(id.clone(), cx)))
                    .into_any_element();
                let revoke = if row.granted && !working {
                    let id = row.id.clone();
                    pill(("vault-revoke", at), "Revoke", palette)
                        .on_click(cx.listener(move |page, _, window, cx| page.revoke(&id, window, cx)))
                        .into_any_element()
                } else if row.granted {
                    quiet_pill(("vault-revoke", at), "Revoke", palette).into_any_element()
                } else {
                    div().into_any_element()
                };
                let color = if row.granted { palette.foreground } else { palette.faint };
                table = table.child(table_row(
                    vec![
                        (Some(180.), name),
                        (
                            None,
                            text(
                                match row.source {
                                    Some(source) => format!("{} · {source}", row.site),
                                    None => row.site.clone(),
                                },
                                palette.faint,
                            ),
                        ),
                        (Some(170.), text(row.grant.clone(), color)),
                        (Some(110.), text(row.when.clone(), palette.muted)),
                        (Some(80.), revoke),
                    ],
                    palette,
                ));
                if self.menu.as_deref() == Some(row.id.as_str()) {
                    table = table.child(self.login_menu(row, at, cx, palette));
                }
            }
            page = page.child(table);
        }

        // Payments.
        page = page.child(head("Payments", palette).pt(px(8.)));
        match (&view.card, &ready.spent) {
            (Some(card_of), Some((spent, share))) => {
                let freeze = if working {
                    quiet_pill(
                        "vault-freeze",
                        if card_of.frozen { "Unfreeze" } else { "Freeze" },
                        palette,
                    )
                } else {
                    pill(
                        "vault-freeze",
                        if card_of.frozen { "Unfreeze" } else { "Freeze" },
                        palette,
                    )
                    .on_click(cx.listener(|page, _, window, cx| page.freeze(window, cx)))
                };
                let mut payments = card(palette)
                    .child(
                        h_flex()
                            .gap(px(12.))
                            .child(ui::icon(Lucide::CreditCard, palette.icon).size(px(16.)))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .text_color(palette.foreground)
                                    .child(format!("Nexus card · virtual ••{}", card_of.last4)),
                            )
                            .when(card_of.frozen, |line| line.child(ui::badge("Frozen", palette)))
                            .child(div().flex_1())
                            .child(quiet(spent.clone(), palette))
                            .child(freeze),
                    )
                    .child(
                        div().w_full().h(px(6.)).rounded(px(3.)).bg(palette.hover).child(
                            div()
                                .h_full()
                                .w(relative(*share))
                                .rounded(px(3.))
                                .bg(palette.foreground),
                        ),
                    );
                if !ready.merchants.is_empty() {
                    payments = payments.child(
                        table_row(
                            vec![
                                (Some(220.), label("Merchant", palette)),
                                (None, label("Limit", palette)),
                                (Some(160.), label("Approval", palette)),
                            ],
                            palette,
                        )
                        .border_t_0()
                        .mt(px(6.)),
                    );
                    for row in &ready.merchants {
                        let approval = h_flex()
                            .gap(px(6.))
                            .text_color(if row.approval == Approval::Off {
                                palette.faint
                            } else {
                                palette.foreground
                            })
                            .when(row.approval == Approval::AsksFirst, |cell| {
                                cell.child(div().size(px(6.)).rounded_full().bg(palette.foreground))
                            })
                            .when(row.approval == Approval::FaceId, |cell| {
                                cell.child(ui::icon(Lucide::ShieldCheck, palette.foreground))
                            })
                            .child(words::approval_label(row.approval))
                            .into_any_element();
                        let limit = if row.approval == Approval::Off {
                            palette.faint
                        } else {
                            palette.foreground
                        };
                        payments = payments.child(table_row(
                            vec![
                                (Some(220.), text(row.merchant.clone(), palette.foreground)),
                                (None, text(row.limit.clone(), limit)),
                                (Some(160.), approval),
                            ],
                            palette,
                        ));
                    }
                }
                page = page.child(payments);
            }
            _ => {
                page = page.child(
                    card(palette).child(
                        h_flex()
                            .gap(px(12.))
                            .child(ui::icon(Lucide::CreditCard, palette.icon).size(px(18.)))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .text_color(palette.foreground)
                                            .child("No Nexus card is linked"),
                                    )
                                    .child(quiet(
                                        "Nexus can’t pay for anything. A payment it starts opens on the merchant’s own page, for you to finish.",
                                        palette,
                                    )),
                            ),
                    ),
                );
            }
        }
        if view.apple_pay {
            page = page.child(
                h_flex()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(ui::icon(Lucide::ShieldCheck, palette.faint))
                    .child("Apple Pay works too: Face ID each time, never saved."),
            );
        }
        page
    }

    /// A login's menu, under its row. "Change to: this once" shows while the grant runs until a date.
    fn login_menu(&self, row: &LoginRow, at: usize, cx: &mut Context<Self>, palette: Palette) -> Div {
        let item = |id: (&'static str, usize), icon: Lucide, words: &'static str| {
            h_flex()
                .id(id)
                .gap(px(8.))
                .px(px(10.))
                .h(px(30.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(13.))
                .text_color(palette.foreground)
                .hover(|style| style.bg(palette.menu_hover))
                .child(ui::icon(icon, palette.icon))
                .child(words)
        };
        let mut menu = v_flex()
            .ml(px(12.))
            .mb(px(6.))
            .w(px(240.))
            .p(px(4.))
            .rounded(px(10.))
            .bg(palette.menu)
            .border_1()
            .border_color(palette.border)
            .child(
                div()
                    .px(px(10.))
                    .py(px(4.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(row.name.clone()),
            )
            .child(
                item(("vault-menu-where", at), Lucide::Eye, "See where it was used")
                    .on_click(cx.listener(|page, _, window, cx| page.show(VaultPart::Activity, window, cx))),
            );
        if row.until && !self.working {
            let id = row.id.clone();
            menu = menu.child(
                item(("vault-menu-once", at), Lucide::Clock, "Change to: this once")
                    .on_click(cx.listener(move |page, _, window, cx| page.each_use(&id, window, cx))),
            );
        }
        if row.granted && !self.working {
            let id = row.id.clone();
            menu = menu.child(
                item(("vault-menu-revoke", at), Lucide::X, "Revoke now")
                    .border_t_1()
                    .border_color(palette.border)
                    .on_click(cx.listener(move |page, _, window, cx| page.revoke(&id, window, cx))),
            );
        }
        menu
    }

    /// The grant sheet: one login, one site, one task, for this once or until the date the ask offers.
    fn grant_sheet(&self, row: &AskRow, cx: &mut Context<Self>, palette: Palette) -> Div {
        let ask = &row.ask;
        let choice = self.choices.get(&ask.id).copied().unwrap_or(For::Once);
        let line = |name: &'static str, value: AnyElement| {
            h_flex()
                .gap(px(12.))
                .items_start()
                .text_size(px(13.))
                .child(div().w(px(80.)).flex_none().text_color(palette.faint).child(name))
                .child(div().flex_1().min_w(px(0.)).text_color(palette.foreground).child(value))
        };
        let segment = |id: &'static str, words: String, picked: bool, pick: For| {
            let ask = ask.id.clone();
            ui::chip((id, 0usize), words, picked, palette)
                .on_click(cx.listener(move |page, _, _, cx| page.choose(&ask, pick, cx)))
        };
        let segments = h_flex()
            .gap(px(6.))
            .child(segment(
                "vault-for-once",
                "This once".into(),
                choice == For::Once,
                For::Once,
            ))
            .children(
                row.until
                    .clone()
                    .map(|until| segment("vault-for-until", until, choice == For::Until, For::Until)),
            );
        let here = local::on_device(&ask.login_id);
        let approve_words = if here {
            "Approve with Touch ID"
        } else {
            "Approve with Face ID"
        };
        let (decline, approve) = if self.working {
            (
                quiet_pill("vault-decline", "Decline", palette),
                quiet_pill("vault-approve", approve_words, palette),
            )
        } else {
            let (no, yes) = (ask.id.clone(), ask.id.clone());
            (
                pill("vault-decline", "Decline", palette)
                    .on_click(cx.listener(move |page, _, window, cx| page.decide(&no, false, window, cx))),
                primary_pill("vault-approve", approve_words, palette)
                    .on_click(cx.listener(move |page, _, window, cx| page.decide(&yes, true, window, cx))),
            )
        };
        card(palette)
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(16.))
                    .text_color(palette.strong)
                    .child(format!("Use your {} login?", ask.login_name)),
            )
            .child(line("Login", text(row.login.clone(), palette.foreground)))
            .child(line("Only on", text(ask.site.clone(), palette.foreground)))
            .child(line("To do", text(ask.task.clone(), palette.foreground)))
            .child(line("For", segments.into_any_element()))
            .child(foot(
                if here {
                    "Approve here with Touch ID. It is then ready on this device: Nexus has no browser to type it into yet, so it fills nothing in. Nexus never sees it, stores it or repeats it.".to_string()
                } else {
                    format!(
                        "{} fills it in, in a sealed browser. Nexus never sees it, stores it or repeats it.",
                        row.filler
                    )
                },
                palette,
            ))
            .child(h_flex().justify_end().gap(px(8.)).child(decline).child(approve))
    }

    fn render_activity(&self, mut page: Div, cx: &mut Context<Self>, palette: Palette) -> Div {
        let can_export = matches!(&self.log, Log::Entries(rows) if !rows.is_empty());
        page = page.child(h_flex().child(div().flex_1()).child(if can_export {
            pill("vault-export", "Export", palette)
                .on_click(cx.listener(|page, _, _, cx| page.export(cx)))
                .into_any_element()
        } else {
            quiet_pill("vault-export", "Export", palette).into_any_element()
        }));
        match &self.log {
            Log::Loading => page.child(quiet("Loading…", palette)),
            Log::Failed => page.child(ui::settings_notice(
                "The activity did not open",
                "Try again in a moment.",
                palette,
            )),
            Log::Off => page.child(quiet(
                "Nothing yet. Each password Nexus uses and each payment shows here.",
                palette,
            )),
            Log::Entries(rows) if rows.is_empty() => page.child(quiet(
                "Nothing yet. Each password Nexus uses and each payment shows here.",
                palette,
            )),
            Log::Entries(rows) => {
                let mut table = v_flex().w_full().child(
                    table_row(
                        vec![
                            (Some(110.), label("When", palette)),
                            (None, label("What", palette)),
                            (Some(180.), label("With", palette)),
                            (Some(130.), label("Approved", palette)),
                            (Some(160.), label("Result", palette)),
                        ],
                        palette,
                    )
                    .border_t_0(),
                );
                for row in rows {
                    let entry = &row.entry;
                    let result = if entry.outcome == words::Outcome::Refused {
                        palette.destructive
                    } else {
                        palette.faint
                    };
                    table = table.child(table_row(
                        vec![
                            (Some(110.), text(row.when.clone(), palette.foreground)),
                            (None, text(entry.what.clone(), palette.foreground)),
                            (
                                Some(180.),
                                text(entry.with.clone().unwrap_or_else(|| "—".into()), palette.muted),
                            ),
                            (
                                Some(130.),
                                text(entry.approved.clone().unwrap_or_else(|| "—".into()), palette.muted),
                            ),
                            (Some(160.), text(entry.result.clone(), result)),
                        ],
                        palette,
                    ));
                }
                page.child(table)
            }
        }
    }
}

impl VaultPage {
    /// Over the chat: the sheets that wait, then what the last decision did, in the chat's 560 px column.
    fn render_asks(&self, cx: &mut Context<Self>, palette: Palette) -> Div {
        let asks = match &self.load {
            Load::Ready(ready) => ready.asks.as_slice(),
            _ => &[],
        };
        if asks.is_empty() && self.status.is_none() {
            return div();
        }
        v_flex()
            .w_full()
            .max_w(px(560.))
            .gap(px(10.))
            .my(px(12.))
            .children(asks.iter().map(|row| self.grant_sheet(row, cx, palette)))
            .children(self.status.clone().map(|status| {
                h_flex()
                    .gap(px(6.))
                    .child(ui::icon(Lucide::Check, palette.foreground))
                    .child(quiet(status, palette))
            }))
    }
}

impl Render for VaultPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        if self.over_chat {
            return div()
                .w_full()
                .flex()
                .justify_center()
                .child(self.render_asks(cx, palette));
        }
        let (id, title, lede) = match self.part {
            VaultPart::Main => (
                "assistant-vault",
                "Passwords and payments",
                "Give access without giving secrets. Every use is in the log.",
            ),
            VaultPart::Activity => (
                "assistant-vault-activity",
                "Activity",
                "Every password used and every payment, with who approved it and where.",
            ),
            VaultPart::Nexus => (
                "assistant-vault-nexus",
                "Nexus vault",
                "Logins kept in this Mac’s Keychain, for Nexus to use only where you allow.",
            ),
            VaultPart::OnePassword => (
                "assistant-vault-1password",
                "1Password",
                "Your own 1Password, through its app. Nexus lists names and sites, and never holds a password.",
            ),
        };
        let (outer, mut page) = frame(id, title, lede, 1040., palette);
        match self.part {
            VaultPart::Activity => page = page.child(crumb("Activity", cx, palette)),
            VaultPart::Nexus => page = page.child(crumb("Nexus vault", cx, palette)),
            VaultPart::OnePassword => page = page.child(crumb("1Password", cx, palette)),
            VaultPart::Main => {}
        }
        if let Some(status) = &self.status {
            page = page.child(
                h_flex()
                    .gap(px(6.))
                    .child(ui::icon(Lucide::Check, palette.foreground))
                    .child(quiet(status.clone(), palette)),
            );
        }
        page = match (self.part, &self.load) {
            (VaultPart::Activity, _) => self.render_activity(page, cx, palette),
            (VaultPart::Nexus, _) => self.render_source(ManagerKind::Nexus, page, cx, palette),
            (VaultPart::OnePassword, _) => self.render_source(ManagerKind::OnePassword, page, cx, palette),
            (VaultPart::Main, Load::Loading) => page.child(quiet("Loading…", palette)),
            // Nexus not answering does not hide this Mac's own vault.
            (VaultPart::Main, Load::Failed) => {
                let page = page.child(ui::settings_notice(
                    "Your passwords and payments did not open",
                    "Try again in a moment.",
                    palette,
                ));
                self.sources(cx, palette)
                    .into_iter()
                    .fold(page, |page, source| page.child(source))
            }
            (VaultPart::Main, Load::Ready(ready)) => self.render_main(ready, page, cx, palette),
        };
        let import = self
            .importing
            .as_ref()
            .map(|said| self.import_sheet(said.as_deref(), cx, palette));
        let sheet = match &self.sheet {
            Some(Sheet::Login(id)) => Some(self.login_sheet(id.as_deref(), cx, palette)),
            Some(Sheet::DeleteAll) => Some(self.delete_all_sheet(cx, palette)),
            None => None,
        };
        div()
            .relative()
            .size_full()
            .child(outer.child(page))
            .children(import)
            .children(sheet)
    }
}
