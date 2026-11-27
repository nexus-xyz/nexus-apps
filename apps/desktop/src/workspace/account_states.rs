//! Each account's Assistant in the account switcher (brand v5 A24·f and g), and the switch itself with the Assistant
//! open.
//!
//! The switcher's rows end with the Assistant's state for that account: Paused, Needs you, an unread count, or Not
//! turned on. The account on screen is live. Another account's state is what this app read last:
//!
//! - GPUI keeps one sign-in, and a switch (`POST /v1/orgs/switch`) replaces its token. The token it leaves is held in
//!   memory (`Gateway::peers`, never in the keychain), so each account the person has been in this launch is read with
//!   its own token, as often as `POLL` while a window is active and when the switcher opens. A token the agent API
//!   refuses is dropped, and a refusal of it never signs the person out.
//! - An account not visited this launch shows the state kept on disk (`assistant-accounts.json`, with the policy's
//!   `AllowLocalCache` off nothing is written), and says how old it is once it is past `STALE_MS`. A state past
//!   `MAX_AGE_MS` shows nothing, since it may be wrong by now.
//! - The unread count of another account is the one its chat kept (`assistant.rs`): the agent API has no count to ask.
//!   Needs you is "an errand waits on the person"; Not turned on is "errands are off at the agent API" (404).
//!
//! After a switch with the Assistant open, the window lands on the same view of the new account (A24·g), and ⌘[
//! switches back to the account it left, until the person goes anywhere else.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::org_menu::Org;
use super::sidebar::{Trailing, assistant_state, assistant_trailing};
use super::{View, Workspace};
use crate::agent::Call;
use crate::pages::PageId;
use crate::pages::agent::RowState;
use crate::pages::assistant_views::{Gather, Reply};
use crate::palette::Palette;
use nexus_desktop_core::agent_errands::{self as words, Status};
use nexus_desktop_core::agent_privacy::{Request as Privacy, pause_view_of};

/// How often another account's Assistant is read while a window is active.
const POLL: Duration = Duration::from_secs(90);
/// The switcher asks again when it opens, unless it asked this recently.
const SOON_MS: i64 = 15_000;
/// After this, a kept state shows how old it is.
const STALE_MS: i64 = 15 * 60 * 1000;
/// After this, a kept state shows nothing.
const MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;

/// What the switcher says of one account's Assistant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Peek {
    pub needs_you: bool,
    pub unread: u32,
    pub paused: bool,
    /// The agent API says errands are off for this account: "Not turned on".
    pub off: bool,
    /// When it was read (epoch ms).
    pub at: i64,
}

impl Peek {
    fn same_state(&self, other: &Peek) -> bool {
        self.needs_you == other.needs_you
            && self.unread == other.unread
            && self.paused == other.paused
            && self.off == other.off
    }
}

/// What the window holds for the switcher and the switch.
#[derive(Default)]
pub(super) struct AccountStates {
    peeks: HashMap<String, Peek>,
    loaded: bool,
    /// The organizations of `GET /v1/orgs`, as the switcher last read them.
    pub(super) orgs: Vec<Org>,
    /// Errands are off for the account on screen (its own read said 404).
    pub(super) off: bool,
    /// The Assistant view the person had open when they switched, to open on the new account.
    land: Option<PageId>,
    /// The account the switch leaves, by its key, and its entry in the list.
    leaving: Option<String>,
    came_from: Option<Org>,
    /// After landing: the account to go back to with ⌘[, and the view that must still be on screen.
    back: Option<(Org, PageId)>,
    polled_at: i64,
    _poll: Option<Task<()>>,
    _read: Option<Task<()>>,
}

#[cfg(test)]
impl AccountStates {
    pub(super) fn set_peek(&mut self, org: &str, peek: Peek) {
        self.loaded = true;
        self.peeks.insert(org.to_string(), peek);
    }

    pub(super) fn back_target(&self) -> Option<(Org, PageId)> {
        self.back.clone()
    }
}

/// An account's key in the Gateway's tokens and here: `Account::org`, the slug, else the id.
pub(super) fn key_of(org: &Org) -> &str {
    org.slug.as_deref().unwrap_or(&org.id)
}

/// "12 min ago", "3 h ago": how old a state is, rounded down.
pub(super) fn age_text(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    if minutes < 60 {
        format!("{minutes} min ago")
    } else {
        format!("{} h ago", minutes / 60)
    }
}

/// The state a read of another account gives: `errands` is its `GET /agent/errands` and `pause` its `GET /agent/pause`.
/// None when neither answered, so the last state stays. `unread` is the count its chat kept.
pub(super) fn peek_of(errands: Option<&Reply>, pause: Option<&Reply>, unread: u32, now: i64) -> Option<Peek> {
    if errands.is_none() && pause.is_none() {
        return None;
    }
    let off = errands.is_some_and(|reply| words::errands_off(reply.status, reply.body.as_ref()));
    let needs_you = errands
        .filter(|reply| reply.ok())
        .and_then(|reply| reply.body.as_ref())
        .and_then(words::errand_list_of)
        .is_some_and(|list| {
            list.iter()
                .any(|errand| errand.status == Status::NeedsYou && !errand.archived)
        });
    let paused = pause
        .filter(|reply| reply.ok())
        .and_then(|reply| reply.body.as_ref())
        .and_then(pause_view_of)
        .is_some_and(|view| view.paused);
    Some(Peek {
        needs_you,
        unread,
        paused,
        off,
        at: now,
    })
}

/// What a switcher row's end holds for `peek`: Not turned on, else the sidebar row's own state (Paused, Needs you, a
/// count). Nothing at rest, since the switcher has no ⌘J hint to show.
fn state_of(peek: &Peek, palette: Palette) -> Option<AnyElement> {
    if peek.off {
        return Some(
            div()
                .flex_none()
                .text_size(px(11.5))
                .text_color(palette.muted)
                .child("Not turned on")
                .into_any_element(),
        );
    }
    let row = RowState {
        unread: peek.unread,
        thinking: false,
        needs_you: peek.needs_you,
    };
    (peek.paused || assistant_trailing(row) != Trailing::Hint).then(|| assistant_state(row, peek.paused, palette))
}

fn file() -> Option<PathBuf> {
    if cfg!(test) || !crate::managed::local_cache(crate::managed::policy()) {
        return None;
    }
    crate::paths::seen_file().map(|file| file.with_file_name("assistant-accounts.json"))
}

fn load_file(url: &str) -> HashMap<String, Peek> {
    let all: HashMap<String, Peek> = file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let prefix = format!("{url}|");
    all.into_iter()
        .filter_map(|(key, peek)| Some((key.strip_prefix(&prefix)?.to_string(), peek)))
        .collect()
}

fn save_file(url: &str, peeks: &HashMap<String, Peek>) {
    let Some(file) = file() else { return };
    // Keep other Gateways' entries; this one's are replaced.
    let prefix = format!("{url}|");
    let mut all: HashMap<String, Peek> = std::fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    all.retain(|key, _| !key.starts_with(&prefix));
    all.extend(peeks.iter().map(|(org, peek)| (format!("{prefix}{org}"), *peek)));
    let Ok(text) = serde_json::to_string(&all) else { return };
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&file, text));
    if let Err(error) = written {
        eprintln!("nexus-desktop-gpui: the accounts' Assistant states could not be saved: {error}");
    }
}

/// The reply of one agent call, once it ended; None when the call never answered.
async fn collect(stream: crate::agent::Stream) -> Option<Reply> {
    let crate::agent::Stream { mut chunks, _stop } = stream;
    let mut gather = Gather::default();
    while let Some(chunk) = chunks.next().await {
        if let Some(done) = gather.push(chunk) {
            return Some(done);
        }
    }
    None
}

impl Workspace {
    /// The account on screen, by its key.
    fn account_key(&self) -> Option<String> {
        self.account.as_ref().map(|account| account.org.clone())
    }

    /// What the account on screen says now, from the row, the errands and the pause.
    fn live_peek(&self) -> Peek {
        let errands_wait = self
            .assistant
            .errands
            .iter()
            .any(|errand| errand.status == Status::NeedsYou && !errand.archived);
        Peek {
            needs_you: self.assistant.row.needs_you || errands_wait,
            unread: self.assistant.row.unread,
            paused: self.assistant.paused,
            off: self.accounts.off,
            at: crate::code::now_ms(),
        }
    }

    /// Keep the account on screen's state, so the switcher shows it (aged) once the person is in another account.
    pub(super) fn note_account_state(&mut self) {
        if !self.agent {
            return;
        }
        let Some(key) = self.account_key() else { return };
        self.load_account_states();
        let live = self.live_peek();
        let fresh = self
            .accounts
            .peeks
            .get(&key)
            .is_none_or(|kept| !kept.same_state(&live) || live.at - kept.at > 60_000);
        if fresh {
            self.accounts.peeks.insert(key, live);
            save_file(self.gateway.url(), &self.accounts.peeks);
        }
    }

    fn load_account_states(&mut self) {
        if !self.accounts.loaded {
            self.accounts.loaded = true;
            self.accounts.peeks = load_file(self.gateway.url());
        }
    }

    /// The state to show at `org`'s row, and whether it is live (the account on screen).
    pub(super) fn account_peek(&self, org: &Org) -> Option<(Peek, bool)> {
        if !self.agent {
            return None;
        }
        if self.account_key().as_deref() == Some(key_of(org)) {
            return Some((self.live_peek(), true));
        }
        let peek = *self.accounts.peeks.get(key_of(org))?;
        (crate::code::now_ms() - peek.at <= MAX_AGE_MS).then_some((peek, false))
    }

    /// The end of `org`'s switcher row: its Assistant's state, with how old it is when it is not live.
    pub(super) fn account_badge(&self, org: &Org, palette: Palette) -> Option<AnyElement> {
        let (peek, live) = self.account_peek(org)?;
        let state = state_of(&peek, palette)?;
        let age = crate::code::now_ms() - peek.at;
        let stale = (!live && age > STALE_MS).then(|| age_text(age));
        Some(
            h_flex()
                .flex_none()
                .gap(px(6.))
                .child(state)
                .children(stale.map(|text| {
                    div()
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(palette.faint)
                        .child(text)
                }))
                .into_any_element(),
        )
    }

    /// Read the organizations for the switcher's rows and ⌘[, once the account is known.
    pub(super) fn read_account_orgs(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let request = account_gateway.request(crate::gateway::Verb::Get, "/v1/orgs".into(), None);
        crate::pages::fetch(cx, request, |this, answer, _| {
            if let Ok(list) = answer {
                this.accounts.orgs = super::org_menu::ordered_orgs(&list);
            }
        });
    }

    /// Start reading the other accounts on a timer, once. The task ends with the window.
    pub(super) fn start_account_polls(&mut self, cx: &mut Context<Self>) {
        self.load_account_states();
        if self.accounts._poll.is_some() || cfg!(test) {
            return;
        }
        self.accounts._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let wait = crate::wakeups::pace(POLL, cx);
                cx.background_executor().timer(wait).await;
                if this.update(cx, |workspace, cx| workspace.poll_accounts(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// The switcher opened: read the other accounts again unless they were read a moment ago.
    pub(super) fn poll_accounts_soon(&mut self, cx: &mut Context<Self>) {
        if crate::code::now_ms() - self.accounts.polled_at > SOON_MS {
            self.poll_accounts(cx);
        }
    }

    /// Read each held account's errands and pause with its own token, one account after the other.
    pub(super) fn poll_accounts(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if !self.assistant_available() {
            return;
        }
        let tokens = account_gateway.peer_tokens();
        if tokens.is_empty() {
            return;
        }
        self.accounts.polled_at = crate::code::now_ms();
        let base = nexus_desktop_core::agent::base_url(crate::desktop_settings::load().agents_api_url.as_deref());
        let gateway = account_gateway.clone();
        self.accounts._read = Some(cx.spawn(async move |this, cx| {
            for (org, token) in tokens {
                let errands = collect(gateway.agent_as(base.clone(), token.clone(), Call::Errands)).await;
                let pause = collect(gateway.agent_as(base.clone(), token, Call::Privacy(Privacy::Pause))).await;
                let refused = [&errands, &pause]
                    .into_iter()
                    .flatten()
                    .any(|reply| matches!(reply.status, 401 | 403));
                if refused {
                    gateway.drop_peer(&org);
                    continue;
                }
                let done = this.update(cx, |workspace, cx| workspace.peer_read(&org, errands, pause, cx));
                if done.is_err() {
                    return;
                }
            }
        }));
    }

    /// Another account's read came back.
    fn peer_read(&mut self, org: &str, errands: Option<Reply>, pause: Option<Reply>, cx: &mut Context<Self>) {
        // The account was entered since: its state is live now.
        if self.account_key().as_deref() == Some(org) {
            return;
        }
        let unread = self.unread_of(org);
        let Some(peek) = peek_of(errands.as_ref(), pause.as_ref(), unread, crate::code::now_ms()) else {
            return;
        };
        if self.accounts.peeks.get(org) != Some(&peek) {
            self.accounts.peeks.insert(org.to_string(), peek);
            save_file(self.gateway.url(), &self.accounts.peeks);
            cx.notify();
        }
    }

    /// The switcher is about to leave this account for `to`: remember where the person is, so the new account opens
    /// on the same view, and who they leave, so ⌘[ goes back.
    pub(super) fn before_switch(&mut self) {
        let here = self.account_key();
        self.accounts.land = match self.view {
            View::Page(id) if id == PageId::Agent || id.is_assistant_view() => Some(id),
            _ => None,
        };
        self.accounts.came_from = here.as_deref().and_then(|key| {
            self.accounts.orgs.iter().find(|org| key_of(org) == key).map(|org| Org {
                current: false,
                ..org.clone()
            })
        });
        self.accounts.leaving = here;
        self.accounts.back = None;
        self.note_account_state();
    }

    /// The switch failed: nothing waits to land.
    pub(super) fn switch_failed(&mut self) {
        self.accounts.land = None;
        self.accounts.came_from = None;
        self.accounts.leaving = None;
    }

    /// The new account is on screen: open the view the person had (A24·g). True when it did.
    pub(super) fn land_after_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // An old account's read arriving late is not the new one.
        if self.account_key() == self.accounts.leaving {
            return false;
        }
        let came_from = self.accounts.came_from.take();
        self.accounts.leaving = None;
        let Some(id) = self.accounts.land.take() else {
            return false;
        };
        if !self.assistant_available() {
            return false;
        }
        self.open_assistant(id, None, window, cx);
        if matches!(self.view, View::Page(shown) if shown == id) {
            self.accounts.back = came_from.map(|org| (org, id));
        }
        true
    }

    /// ⌘[ right after a switch, still on the view it landed on: back to the account it left. False otherwise, and
    /// then ⌘[ goes back in the history as always.
    pub(super) fn switch_back(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((org, id)) = self.accounts.back.clone() else {
            return false;
        };
        if !matches!(self.view, View::Page(shown) if shown == id) {
            self.accounts.back = None;
            return false;
        }
        self.switch_org(&org, window, cx);
        true
    }
}

#[cfg(test)]
#[path = "account_states_tests.rs"]
mod tests;
