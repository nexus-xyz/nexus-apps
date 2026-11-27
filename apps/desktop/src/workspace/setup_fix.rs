//! The window's side of setup lines (`crate::setup`): it runs a line's fix here, keeps who may fix what current for the
//! lines, shows "Back to session" in Settings that a fix opened, and follows a terminal step to its end.

use std::collections::HashMap;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::json;

use super::{Auth, Place, SettingsSection, View, Workspace, glyph};
use crate::gateway::Verb;
use crate::pages::PageId;
use crate::palette::Palette;
use crate::setup::{self, Access, Ask, Fix, FixKey, Fixer, Run, SignIn, TerminalStep, access, state};
use crate::side::terminal::TerminalEvent;

/// What this window keeps for the fixes it ran.
#[derive(Default)]
pub(super) struct FixState {
    /// Settings were opened by a fix at this history entry: while the person stays in them, their head offers Back.
    return_at: Option<usize>,
    /// Terminal steps in a session's Commands, by their token, with their line's key.
    pub(super) tokens: HashMap<u64, FixKey>,
    next_token: u64,
    /// Terminal steps in a Workbench shell: kept until each ends.
    shells: Vec<(FixKey, Subscription)>,
    /// The members read that names the owners and admins.
    members: Option<Task<()>>,
    /// A Nexus sign-in landed and its account is not read yet: then what waited on it goes on (`nexus_sign_in.rs`).
    pub(super) signed_in: bool,
}

impl Workspace {
    /// Runs a line's fix in this window, for the organization it shows.
    pub(crate) fn fixer(&self, cx: &mut Context<Self>) -> Fixer {
        let this = cx.entity().downgrade();
        Fixer::new(
            self.account.as_ref().map(|account| account.org.clone()),
            move |key, fix, window, cx| match fix {
                // The call site's own step may update the workspace: it runs outside this window's update.
                Fix::Do { run, .. } => run(window, cx),
                fix => {
                    if let Err(error) = this.update(cx, |workspace, cx| workspace.run_setup_fix(key, fix, window, cx)) {
                        eprintln!("nexus-desktop-gpui: setup fix: {error}");
                    }
                }
            },
        )
    }

    /// Do `fix` for the line `key`.
    pub(crate) fn run_setup_fix(&mut self, key: FixKey, fix: Fix, window: &mut Window, cx: &mut Context<Self>) {
        match fix {
            Fix::Open { page, focus, .. } => self.open_for_fix(page, focus, window, cx),
            Fix::SignIn(SignIn::Nexus) => {
                self.auth = Auth::Checking;
                self.gateway.begin_device_sign_in();
            }
            Fix::SignIn(SignIn::Agent(agent)) => {
                crate::pages::connect_on_open(agent);
                self.open_for_fix(PageId::Agents, None, window, cx);
            }
            Fix::SignIn(SignIn::Server(id)) => {
                crate::machines::get().ask_sign_in(&id);
                self.open_for_fix(PageId::Servers, None, window, cx);
            }
            Fix::RunInTerminal(step) => self.run_fix_in_terminal(key, step, window, cx),
            Fix::TakeOver => self.take_over(window, cx),
            Fix::Ask(ask) => self.ask_for_fix(key, ask, cx),
            Fix::Guide(url) => crate::external_link::open(&url, cx),
            // After this update: the step may update the workspace itself.
            Fix::Do { run, .. } => window.defer(cx, move |window, cx| run(window, cx)),
        }
        cx.notify();
    }

    /// Open `page`, on its row `focus` when it has one. From outside Settings, the way back is kept.
    fn open_for_fix(&mut self, page: PageId, focus: Option<&'static str>, window: &mut Window, cx: &mut Context<Self>) {
        let from_settings = matches!(self.view, View::Settings(_));
        match focus {
            // Usage is a place, and Settings › Budgets too: a row of its Limits opens there (`limit_stop.rs`).
            Some(anchor) if !page.is_place() || page == PageId::Usage => {
                self.open_settings_field(SettingsSection::Page(page), anchor, window, cx)
            }
            _ => self.open_native(page, window, cx),
        }
        self.setup.return_at = (!from_settings && matches!(self.view, View::Settings(_))).then_some(self.history_at);
    }

    /// Settings on screen were opened by a fix: once it lands, the person goes back to what it blocked.
    pub(super) fn in_settings_for_fix(&self) -> bool {
        self.setup.return_at.is_some() && matches!(self.view, View::Settings(_))
    }

    /// "‹ Back to session" over Settings that a fix opened, while the person stays in them.
    pub(super) fn fix_back(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let at = self.setup.return_at?;
        let stayed = matches!(self.view, View::Settings(_))
            && self.history.get(at..=self.history_at).is_some_and(|places| {
                places
                    .iter()
                    .all(|place| matches!(place, Place::Settings(_) | Place::SettingsField(..)))
            });
        if !stayed {
            return None;
        }
        let before = at.checked_sub(1).and_then(|before| self.history.get(before));
        let label = match before {
            Some(Place::Code(_) | Place::NewCode | Place::Chat(_)) => "Back to session",
            _ => "Back",
        };
        Some(
            h_flex()
                .id("setup-fix-back")
                .mb(px(8.))
                .h(px(28.))
                .gap(px(4.))
                .px(px(8.))
                .ml(px(-8.))
                .rounded(px(crate::tokens::radius::CONTROL))
                .text_size(px(13.))
                .text_color(palette.muted)
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover).text_color(palette.strong))
                .child(glyph(Lucide::ChevronLeft, palette.icon))
                .child(label)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.setup.return_at = None;
                    this.back_to_app(window, cx);
                }))
                .into_any_element(),
        )
    }

    /// Run the step where the person sees it, and follow it to its end.
    fn run_fix_in_terminal(&mut self, key: FixKey, step: TerminalStep, window: &mut Window, cx: &mut Context<Self>) {
        // Settings and Team sessions have no Workbench: the step runs beside the session the person came from.
        if self.workbench_hidden() {
            self.back_to_app(window, cx);
        }
        self.setup.next_token += 1;
        let token = self.setup.next_token;
        state::set_run(key.clone(), Run::Running, cx);
        match self.start_fix_terminal(&step, token, window, cx) {
            super::side_pane::FixTerminal::Shell(shell) => {
                let line = key.clone();
                let subscription =
                    cx.subscribe_in(&shell, window, move |this, _, event: &TerminalEvent, window, cx| {
                        let TerminalEvent::Exited { code, last_line } = event;
                        this.fix_step_ended(line.clone(), *code == Some(0), *code, last_line.clone(), window, cx);
                    });
                self.setup.shells.push((key, subscription));
            }
            super::side_pane::FixTerminal::Shared => {
                self.setup.tokens.insert(token, key);
            }
            super::side_pane::FixTerminal::Elsewhere => state::set_run(key, Run::Unwatched, cx),
            super::side_pane::FixTerminal::Refused => state::set_run(
                key,
                Run::Failed {
                    code: None,
                    last: Some("The Workbench can't run it here. Run it in a terminal yourself.".into()),
                },
                cx,
            ),
        }
    }

    /// A terminal step ended: done (what waited resumes), or failed with its last line.
    pub(super) fn fix_step_ended(
        &mut self,
        key: FixKey,
        ok: bool,
        code: Option<u32>,
        last: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.setup.shells.retain(|(each, _)| *each != key);
        if ok {
            setup::done(&key, window, cx);
        } else {
            state::set_run(key, Run::Failed { code, last }, cx);
        }
        cx.notify();
    }

    /// Ask the people who can: a billing block goes to the owners' inboxes through Gateway; anything else is copied,
    /// to send them.
    fn ask_for_fix(&mut self, key: FixKey, ask: Ask, cx: &mut Context<Self>) {
        let Some(block) = ask.billing else {
            cx.write_to_clipboard(ClipboardItem::new_string(ask.message.clone()));
            state::set_asked(key, access::asked_label(&ask.who, ask.needs, false), cx);
            return;
        };
        let Some(gateway) = self.account_gateway() else {
            cx.write_to_clipboard(ClipboardItem::new_string(ask.message.clone()));
            return;
        };
        let request = gateway.request(
            Verb::Post,
            "/v1/notifications/ask-owner".into(),
            Some(json!({ "block": block.wire() })),
        );
        cx.spawn(async move |_, cx| {
            let sent = match request.await {
                Ok(_) => true,
                Err(error) => {
                    eprintln!("nexus-desktop-gpui: ask the owners: {error}");
                    false
                }
            };
            cx.update(|cx| {
                if !sent {
                    cx.write_to_clipboard(ClipboardItem::new_string(ask.message.clone()));
                }
                state::set_asked(key, access::asked_label(&ask.who, ask.needs, sent), cx);
            });
        })
        .detach();
    }

    /// The account was read: its permissions now, and once per organization the owners and admins a line names.
    pub(super) fn read_setup_access(&mut self, cx: &mut Context<Self>) {
        let Some(account) = &self.account else {
            return;
        };
        let org = account.org.clone();
        let permissions = (!account.permissions.is_empty()).then(|| account.permissions.clone());
        Access::set_account(&org, account.user_id.clone(), permissions, cx);
        if account.personal || !Access::of(&org, cx).managers.is_empty() || self.setup.members.is_some() {
            return;
        }
        let Some(gateway) = self.account_gateway() else {
            return;
        };
        let request = gateway.query("/v1/orgs/members".into());
        self.setup.members = Some(cx.spawn(async move |this, cx| {
            let answer = request.await;
            let updated = this.update(cx, |this, cx| {
                this.setup.members = None;
                match answer {
                    Ok(value) => Access::set_managers(&org, access::parse_managers(&value), cx),
                    Err(error) => eprintln!("nexus-desktop-gpui: setup: members: {error}"),
                }
            });
            if let Err(error) = updated {
                eprintln!("nexus-desktop-gpui: setup: members: {error}");
            }
        }));
    }
}
