//! The sidebar team and online privacy preference use fresh, visible-window presence calls. The section is Nexus UI's
//! `SidebarTeam` (`team-presence.tsx` and `.css`): its rows, its fade as it opens and shuts, the info icon whose
//! popover says what teammates see, "Manage" to the preference's own row, and the team's sessions while
//! `multiplayerSessions` is on.
use super::code_dock::menu_box;
use super::folder_motion::Transition;
use super::overlay::{Overlay, OverlaySlot};
use super::sidebar::{ROW, ROW_TEXT};
use super::{Auth, SettingsSection, View, Workspace, disclosure, glyph};
use crate::workspace::MenuTrigger as _;
use crate::{
    gateway::Verb,
    motion::{self, TweenExt as _},
    pages::{PageId, ui},
    palette::Palette,
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::{
    component::{h_flex, tooltip::Tooltip, v_flex},
    prelude::*,
    *,
};
use nexus_desktop_core::presence::{self, Member, Settings, Snapshot};
use serde_json::json;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Scope {
    gateway: String,
    marker: Option<String>,
    user_id: Option<String>,
    organization: String,
}

pub(super) struct TeamPresence {
    scope: Option<Scope>,
    pub(super) own: Option<Member>,
    pub(super) members: Vec<Member>,
    loaded: bool,
    read_failed: bool,
    show_online: Option<bool>,
    saving: Option<bool>,
    preference_revision: u64,
    error: Option<String>,
    pub(super) disabled: bool,
    open: bool,
    activity: &'static str,
    poll: Option<Task<()>>,
    activity_change: Option<Task<()>>,
    /// The body's fade as the section opens and shuts (`.tp-body`).
    body: Transition,
    /// The organization's `multiplayerSessions`: members watch each other's sessions.
    pub(super) sessions: bool,
    /// The pointer is over the New session row's online stack, so its discs spread apart.
    stack_hovered: bool,
    /// The team-visible sessions you follow, for the sidebar's Following.
    pub(super) following: Vec<super::team_following::Followed>,
    /// Polls since the scope began; the organization's settings are read on every [`SETTINGS_EVERY`]th.
    polls: u64,
    /// Memory-only native watch viewer; an account reset drops its tasks and decrypted data.
    pub(super) viewer: super::team_sessions::Viewer,
    /// The head's info icon, so a click or Tab gives it the keys that open its popover. Made on the first sync.
    sharing_focus: Option<FocusHandle>,
}

/// The organization's settings change rarely: read them on the first poll, then every five minutes.
const SETTINGS_EVERY: u64 = 10;
impl Default for TeamPresence {
    fn default() -> Self {
        Self {
            scope: None,
            own: None,
            members: Vec::new(),
            loaded: false,
            read_failed: false,
            show_online: None,
            saving: None,
            preference_revision: 0,
            error: None,
            disabled: false,
            open: remembered_open(
                open_file()
                    .and_then(|path| std::fs::read_to_string(path).ok())
                    .as_deref(),
            ),
            activity: "idle",
            poll: None,
            activity_change: None,
            body: Transition::default(),
            sessions: false,
            stack_hovered: false,
            following: Vec::new(),
            polls: 0,
            viewer: Default::default(),
            sharing_focus: None,
        }
        .settled()
    }
}
impl TeamPresence {
    pub(super) fn refresh_following(&mut self) {
        self.poll = None;
    }
    /// The body drawn as the remembered open state says, with no motion.
    fn settled(mut self) -> Self {
        self.body = Transition::settled(if self.open { 1. } else { 0. });
        self
    }
}
/// The Team section starts collapsed: it opens only when the saved file says "open".
fn remembered_open(saved: Option<&str>) -> bool {
    saved == Some("open")
}
fn open_file() -> Option<std::path::PathBuf> {
    crate::paths::own_data_dir().map(|dir| dir.join("sidebar-team"))
}
fn unsupported(error: &nexus_client::Error) -> bool {
    matches!(error, nexus_client::Error::Core(error) if matches!(error.status,403|404))
}
fn preference_read_current(started: u64, current: u64, saving: bool) -> bool {
    started == current && !saving
}
/// A request that may not have been made this poll.
async fn optional<T>(request: Option<impl std::future::Future<Output = T>>) -> Option<T> {
    match request {
        Some(request) => Some(request.await),
        None => None,
    }
}
fn body(activity: &str) -> serde_json::Value {
    json!({"activity":{"kind":activity}})
}

/// Keep the signed-in person's row apart from teammates, so online counts and avatar stacks still exclude it.
fn split_members(members: Vec<Member>, self_id: Option<&str>, self_email: &str) -> (Option<Member>, Vec<Member>) {
    let own = members
        .iter()
        .find(|member| match self_id {
            Some(id) => member.user_id == id,
            None => member
                .email
                .as_deref()
                .is_some_and(|email| email.eq_ignore_ascii_case(self_email)),
        })
        .cloned();
    (own, presence::teammates(members, self_id, self_email))
}

/// Gateway may mark the person online to themselves while hiding them from teammates.
fn visible_self(member: &Member, show_online: Option<bool>) -> Member {
    let mut visible = member.clone();
    if show_online != Some(true) {
        visible.online = false;
        visible.activity = None;
    }
    visible
}

fn own_status(member: &Member, show_online: Option<bool>) -> String {
    match show_online {
        None => "Visibility unavailable".into(),
        Some(false) => "Hidden from teammates".into(),
        Some(true) if !member.online => "Offline · visible to teammates".into(),
        Some(true) => presence::activity_label(member).map_or_else(
            || "Online · visible to teammates".into(),
            |activity| format!("{activity} · visible to teammates"),
        ),
    }
}

/// What the Team head's info popover says. Nexus UI's `SHARING` (`presence.ts`) has the same words; its test reads
/// this constant.
const SHARING: [&str; 2] = [
    "Teammates see online status, chatting activity, and running agent counts. Page titles appear only to people with access.",
    "This does not grant access to chats, pages, or agent sessions. Check each item\u{2019}s sharing controls for access.",
];

/// "Team sessions" and "View all": the groups' quiet link under their rows (`.wb-group-more`, "All sessions").
fn more_link(id: &'static str, label: SharedString, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(28.))
        .px(px(8.))
        .rounded(px(8.))
        .text_xs()
        .text_color(palette.faint)
        .cursor_pointer()
        .hover(|style| style.text_color(palette.foreground))
        .child(label)
}

fn visibility_link(show_online: Option<bool>) -> &'static str {
    match show_online {
        Some(true) => "Your activity is visible · Manage",
        Some(false) => "Your activity is hidden · Manage",
        None => "Manage online visibility",
    }
}

fn team_note(team: &TeamPresence) -> Option<&'static str> {
    read_note(
        team.disabled,
        team.read_failed,
        team.loaded,
        team.own.is_none() && team.members.is_empty(),
    )
}

fn read_note(disabled: bool, read_failed: bool, loaded: bool, empty: bool) -> Option<&'static str> {
    if disabled {
        Some("Team presence needs a newer Nexus Gateway.")
    } else if read_failed {
        Some(if loaded {
            "Team presence could not be refreshed. Retrying."
        } else {
            "Team presence could not be loaded. Retrying."
        })
    } else if !loaded {
        Some("Loading team…")
    } else if empty {
        Some("No team presence is available yet.")
    } else {
        None
    }
}

impl Workspace {
    pub(super) fn team_scope(&self) -> Option<Scope> {
        let account = self.account.as_ref()?;
        account.organization_kind.as_deref()?;
        if self.panel || !presence::eligible(matches!(self.auth, Auth::SignedIn), account.personal, true) {
            return None;
        }
        Some(Scope {
            gateway: self.gateway.url().into(),
            marker: self.gateway.marker(),
            user_id: account.user_id.clone(),
            organization: account.org.clone(),
        })
    }
    /// The presence poll for Settings › Members, which shares it rather than opening its own.
    pub(super) fn team_live(&self) -> crate::pages::members_presence::Team {
        let team = &self.team_presence;
        crate::pages::members_presence::Team {
            loaded: team.loaded,
            unavailable: team.disabled || self.team_scope().is_none(),
            own: team.own.clone(),
            members: team.members.clone(),
            show_online: team.show_online,
            sessions: team.sessions,
        }
    }

    pub(super) fn online_preference_available(&self) -> bool {
        self.team_scope().is_some() && !self.team_presence.disabled
    }

    fn presence_activity(&self) -> &'static str {
        if matches!(self.view, View::Chat) {
            "chatting"
        } else {
            "idle"
        }
    }

    pub(super) fn sync_team_presence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scope = self.team_scope();
        if self.team_presence.scope != scope {
            self.team_presence = TeamPresence {
                scope: scope.clone(),
                activity: self.presence_activity(),
                ..Default::default()
            }
            .settled();
        }
        if self.team_presence.sharing_focus.is_none() {
            self.team_presence.sharing_focus = Some(cx.focus_handle());
        }
        self.sync_team_sessions(window, cx);
        if scope.is_none() || !window.is_visible() || self.team_presence.disabled {
            self.team_presence.poll = None;
            self.team_presence.activity_change = None;
            return;
        }
        let activity = self.presence_activity();
        if activity != self.team_presence.activity {
            self.team_presence.activity = activity;
            let expected = scope.clone();
            self.team_presence.activity_change = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(Some(request)) = this.update_in(cx, |this, window, _| {
                    let account = this.account_gateway()?;
                    (this.team_scope() == expected
                        && window.is_visible()
                        && !this.team_presence.disabled
                        && this.presence_activity() == activity)
                        .then(|| account.request_detailed(Verb::Post, "/v1/presence".into(), Some(body(activity))))
                }) else {
                    return;
                };
                if let Err(error) = request.await
                    && unsupported(&error)
                {
                    let _ = this.update(cx, |this, cx| {
                        if this.team_scope() == expected {
                            this.team_presence.disabled = true;
                            this.team_presence.own = None;
                            this.team_presence.members.clear();
                            cx.notify();
                        }
                    });
                }
            }));
        }
        if self.team_presence.poll.is_some() {
            return;
        }
        let expected = scope;
        self.team_presence.poll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(Some((list, setting, beat, preference_revision, org, following))) =
                    this.update_in(cx, |this, window, _| {
                        if this.team_scope() != expected || !window.is_visible() || this.team_presence.disabled {
                            return None;
                        }
                        let account = this.account_gateway()?;
                        let polls = this.team_presence.polls;
                        this.team_presence.polls = polls.wrapping_add(1);
                        let org = (polls % SETTINGS_EVERY == 0)
                            .then(|| account.request_detailed(Verb::Get, "/v1/orgs/settings".into(), None));
                        // Asked with the settings too, so the first poll that turns sessions on also fills Following.
                        let following = (this.team_presence.sessions || org.is_some())
                            .then(|| account.request_detailed(Verb::Get, super::team_following::PATH.into(), None));
                        Some((
                            account.request_detailed(Verb::Get, "/v1/presence".into(), None),
                            this.team_presence
                                .show_online
                                .is_none()
                                .then(|| account.request_detailed(Verb::Get, "/v1/presence/settings".into(), None)),
                            account.request_detailed(
                                Verb::Post,
                                "/v1/presence".into(),
                                Some(body(this.presence_activity())),
                            ),
                            this.team_presence.preference_revision,
                            org,
                            following,
                        ))
                    })
                else {
                    break;
                };
                let (list, setting, beat, org, following) =
                    futures::join!(list, optional(setting), beat, optional(org), optional(following));
                let stop = [
                    list.as_ref().err(),
                    setting.as_ref().and_then(|setting| setting.as_ref().err()),
                    beat.as_ref().err(),
                ]
                .into_iter()
                .flatten()
                .any(unsupported);
                let updated = this.update_in(cx, |this, window, cx| {
                    if this.team_scope() != expected || !window.is_visible() {
                        return;
                    }
                    if stop {
                        this.team_presence.disabled = true;
                        this.team_presence.own = None;
                        this.team_presence.members.clear();
                        this.team_presence.show_online = None;
                    } else {
                        if let Some(snapshot) = list
                            .ok()
                            .and_then(|value| serde_json::from_value::<Snapshot>(value).ok())
                        {
                            let account = this.account.as_ref();
                            let (own, members) = split_members(
                                snapshot.members,
                                account.and_then(|a| a.user_id.as_deref()),
                                account.map_or("", |a| a.email.as_str()),
                            );
                            this.team_presence.own = own;
                            this.team_presence.members = members;
                            this.team_presence.loaded = true;
                            this.team_presence.read_failed = false;
                            if preference_read_current(
                                preference_revision,
                                this.team_presence.preference_revision,
                                this.team_presence.saving.is_some(),
                            ) {
                                this.team_presence.show_online = Some(snapshot.show_online);
                            }
                        } else {
                            this.team_presence.read_failed = true;
                        }
                        let team = &mut this.team_presence;
                        match org {
                            Some(Ok(value)) => team.sessions = super::team_following::sessions_on(&value),
                            // A member who cannot read the settings, or a Gateway without them: no team sessions.
                            Some(Err(error)) if unsupported(&error) => team.sessions = false,
                            _ => {}
                        }
                        if !team.sessions {
                            team.following.clear();
                        } else if let Some(Ok(value)) = following
                            && let Some(threads) = super::team_following::parse(value)
                        {
                            team.following = threads;
                        }
                        if let Some(Ok(value)) = setting
                            && let Ok(setting) = serde_json::from_value::<Settings>(value)
                            && preference_read_current(
                                preference_revision,
                                this.team_presence.preference_revision,
                                this.team_presence.saving.is_some(),
                            )
                        {
                            this.team_presence.show_online = Some(setting.show_online);
                        }
                    }
                    cx.notify();
                });
                if stop || updated.is_err() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(presence::POLL_SECONDS))
                    .await;
            }
        }));
    }

    fn toggle_team(&mut self, cx: &mut Context<Self>) {
        self.team_presence.open = !self.team_presence.open;
        let open = self.team_presence.open;
        // In with the UI's duration, out quicker (`--nx-dur-ui-exit`), as the web's `.tp-body[hidden]`.
        let duration = if open { motion::UI } else { motion::UI_EXIT };
        self.team_presence.body.retarget(if open { 1. } else { 0. }, duration);
        if !open {
            // The pane is cached: draw once more after the fade, so the shut body leaves the layout.
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(duration).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        if let Some(file) = open_file() {
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(file, if self.team_presence.open { "open" } else { "closed" });
        }
        cx.notify();
    }

    fn save_online(&mut self, show: bool, cx: &mut Context<Self>) {
        let Some(scope) = self.team_scope() else {
            return;
        };
        if self.team_presence.saving.is_some() || self.team_presence.disabled {
            return;
        }
        self.team_presence.preference_revision = self.team_presence.preference_revision.wrapping_add(1);
        let preference_revision = self.team_presence.preference_revision;
        self.team_presence.saving = Some(show);
        self.team_presence.error = None;
        let Some(account) = self.account_gateway() else { return };
        let request = account.request_detailed(
            Verb::Put,
            "/v1/presence/settings".into(),
            Some(json!({"showOnline":show})),
        );
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                if this.team_scope().as_ref() != Some(&scope)
                    || this.team_presence.preference_revision != preference_revision
                {
                    return;
                }
                this.team_presence.saving = None;
                match result {
                    Ok(value) => match serde_json::from_value::<Settings>(value) {
                        Ok(setting) => this.team_presence.show_online = Some(setting.show_online),
                        Err(_) => {
                            this.team_presence.error =
                                Some("The online preference could not be read. Try again.".into())
                        }
                    },
                    Err(error) => this.team_presence.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The online teammates on the New session row (`OnlineStack`): up to `STACK_SHOWN` discs, each laid over the one
    /// before it, then +N. Under the pointer the discs spread apart, so each one shows whole.
    pub(super) fn render_online_stack(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.team_scope().is_none() || self.team_presence.disabled {
            return None;
        }
        let online: Vec<_> = self
            .team_presence
            .members
            .iter()
            .filter(|member| member.online)
            .collect();
        if online.is_empty() {
            return None;
        }
        let label = format!(
            "{} {} online: {}",
            online.len(),
            if online.len() == 1 { "teammate" } else { "teammates" },
            online
                .iter()
                .map(|m| presence::display_name(m))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let spread = self.team_presence.stack_hovered;
        let tween = stack_tween(spread);
        Some(
            h_flex()
                .id("presence-online-stack")
                .ml_auto()
                .pl(px(6.))
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if this.team_presence.stack_hovered != *hovered {
                        this.team_presence.stack_hovered = *hovered;
                        cx.notify();
                    }
                }))
                .children(
                    online
                        .iter()
                        .take(presence::STACK_SHOWN)
                        .enumerate()
                        .map(|(index, member)| {
                            let disc = avatar(member, true, palette);
                            if index == 0 {
                                return disc.into_any_element();
                            }
                            disc.shadow(stack_shadow(palette))
                                .with_tween(
                                    SharedString::from(format!("presence-stack-step-{index}")),
                                    if spread { 1. } else { 0. },
                                    tween,
                                    |disc, value| disc.ml(px(stack_step(value))),
                                )
                                .into_any_element()
                        }),
                )
                .when(online.len() > presence::STACK_SHOWN, |stack| {
                    stack.child(
                        div()
                            .ml(px(4.))
                            .text_size(px(11.))
                            .text_color(palette.faint)
                            .child(format!("+{}", online.len() - presence::STACK_SHOWN)),
                    )
                })
                .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
                .into_any_element(),
        )
    }

    pub(super) fn render_team_presence(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.team_scope()?;
        let team = &self.team_presence;
        let own = team.own.as_ref().map(|member| visible_self(member, team.show_online));
        // Your row counts once you are visible, as Nexus UI's `onlineLabel` counts every row it shows.
        let online = own.iter().chain(team.members.iter()).filter(|m| m.online).count();
        let head = h_flex()
            .id("sidebar-team-toggle")
            .mt(px(16.))
            .h(px(28.))
            .pl(px(8.))
            .pr(px(4.))
            .gap(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_xs()
            .text_color(palette.faint)
            .hover(|style| style.bg(palette.hover))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_team(cx)))
            .child("Team")
            .when(online > 0, |head| head.child(format!("{online} online")))
            .child(div().flex_1())
            .when(!team.disabled && team.loaded, |head| {
                head.child(self.render_team_sharing(palette, cx))
            })
            .child(disclosure("sidebar-team-chevron", team.open, palette.faint).into_any_element());
        let section = v_flex().child(head);
        if !team.body.visible() {
            return Some(section.into_any_element());
        }
        let mut body = v_flex();
        if let Some(note) = team_note(team) {
            body = body.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(11.))
                    .text_color(palette.faint)
                    .child(note),
            );
        }
        let count = team.members.len() + usize::from(own.is_some());
        for (member, is_self) in own
            .iter()
            .map(|member| (member, true))
            .chain(team.members.iter().map(|member| (member, false)))
            .take(presence::TEAM_SHOWN)
        {
            let link = (team.sessions && !is_self).then(|| member.user_id.clone());
            body = body.child(self.render_team_row(member, is_self, link, palette, cx));
        }
        if team.sessions {
            body = body.child(
                more_link("sidebar-team-sessions", "Team sessions".into(), palette)
                    .on_click(cx.listener(|this, _, window, cx| this.open_team_sessions(None, window, cx))),
            );
        }
        if count > presence::TEAM_SHOWN {
            body = body.child(
                more_link("sidebar-team-all", format!("View all {count}").into(), palette)
                    .on_click(cx.listener(|this, _, window, cx| this.open_native(PageId::Members, window, cx))),
            );
        }
        if !team.disabled && team.loaded {
            body = body.child(
                div()
                    .id("team-presence-preference")
                    .mt(px(6.))
                    .ml(px(6.))
                    .mr(px(8.))
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(palette.faint)
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.foreground).underline())
                    .on_click(cx.listener(|this, _, window, cx| {
                        // Nexus UI's `/settings/preferences#show-online`: the row itself, scrolled to and marked.
                        this.open_settings_field(SettingsSection::Appearance, SHOW_ONLINE.0, window, cx);
                    }))
                    .child(visibility_link(team.show_online)),
            );
        }
        // Fades and moves 4 px as it opens and shuts; nothing is under it, so nothing jumps (`.tp-body`).
        let body = team.body.draw(body, "sidebar-team-body".into(), |body, value| {
            body.opacity(value).relative().top(px(-4. * (1. - value)))
        });
        Some(section.child(body).into_any_element())
    }

    /// The head's info icon (`SharingInfo`): a click, a tap, Enter or Space opens what teammates see under it; Esc, a
    /// click outside or a second press closes it.
    fn render_team_sharing(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.overlay.team_sharing();
        let icon = div()
            .id("team-presence-sharing")
            .role(Role::Button)
            .aria_label("Status and activity only")
            .when_some(self.team_presence.sharing_focus.as_ref(), |icon, focus| {
                icon.track_focus(focus)
            })
            .tab_index(0)
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .focus_visible(|style| style.bg(palette.hover))
            .when(open, |icon| icon.bg(palette.hover))
            .child(glyph(Lucide::Info, if open { palette.foreground } else { palette.faint }).size(px(13.)))
            // A press here is the icon's, not the head's fold.
            .on_click(|_, _, cx| cx.stop_propagation())
            .menu_trigger(
                open,
                cx,
                |this| this.overlay.team_sharing(),
                |this, window, cx| {
                    if let Some(focus) = &this.team_presence.sharing_focus {
                        window.focus(focus, cx);
                    }
                    this.toggle_team_sharing(cx);
                },
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    this.toggle_team_sharing(cx);
                }
            }));
        let card = crate::motion::presence("team-sharing", open, cx).map(|phase| {
            let card = menu_box("team-sharing", 256., palette)
                .p(px(12.))
                .gap(px(8.))
                .text_size(px(12.5))
                .line_height(px(17.))
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    if this.overlay.close_team_sharing() {
                        cx.notify();
                    }
                }))
                .children(SHARING.map(|line| div().child(line)));
            deferred(
                anchored()
                    .anchor(Anchor::TopRight)
                    .offset(point(px(0.), px(26.)))
                    .snap_to_window_with_margin(px(12.))
                    .child(phase.menu("team-sharing", -4., card)),
            )
            .with_priority(1)
        });
        div().relative().child(icon).children(card).into_any_element()
    }

    pub(super) fn toggle_team_sharing(&mut self, cx: &mut Context<Self>) {
        self.toggle_overlay(Overlay::TeamSharing);
        cx.notify();
    }

    /// One row (`TeamRow`): the avatar, the name, and under it what they are doing while online. With `link`, the name
    /// opens their sessions.
    fn render_team_row(
        &self,
        member: &Member,
        is_self: bool,
        link: Option<String>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let doing = if is_self {
            Some(own_status(member, self.team_presence.show_online))
        } else if member.online {
            presence::activity_label(member)
        } else {
            None
        };
        let label = presence::display_name(member).to_string();
        let email = member.email.clone().unwrap_or_else(|| label.clone());
        let online = member.online;
        let linked = link.is_some();
        h_flex()
            .id(SharedString::from(format!("presence-member-{}", member.user_id)))
            .group("team-row")
            .gap(px(8.))
            .pl(px(6.))
            .pr(px(8.))
            .h(px(if doing.is_some() { 40. } else { ROW }))
            .text_size(px(ROW_TEXT))
            .text_color(if online { palette.foreground } else { palette.muted })
            .child(avatar(member, false, palette).when(!online, |avatar| avatar.opacity(0.55)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .items_baseline()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .line_height(px(18.))
                                    .truncate()
                                    .when(!online, |name| name.text_color(palette.faint))
                                    .when(linked, |name| name.group_hover("team-row", |style| style.underline()))
                                    .child(label),
                            )
                            .when(is_self, |name| {
                                name.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(palette.faint)
                                        .child("you"),
                                )
                            }),
                    )
                    .children(doing.map(|doing| {
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(palette.faint)
                            .truncate()
                            .child(doing)
                    })),
            )
            .when_some(link, |row, person| {
                row.cursor_pointer().on_click(
                    cx.listener(move |this, _, window, cx| this.open_team_sessions(Some(person.clone()), window, cx)),
                )
            })
            .tooltip(move |window, cx| Tooltip::new(email.clone()).build(window, cx))
            .into_any_element()
    }

    pub(super) fn render_presence_preference(
        &self,
        account: Option<&crate::pages::Ctx>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.is_local() {
            return Some(
                super::settings_view::setting_row(
                    "Show when I'm online",
                    "Let teammates see when you have Nexus open, and what you are doing.",
                    palette,
                )
                .child(ui::requires_sign_in(palette))
                .child(ui::switch("settings-show-online", false, true, palette))
                .into_any_element(),
            );
        }
        if account.is_none() || !self.online_preference_available() {
            return None;
        }
        let checked = self.team_presence.saving.or(self.team_presence.show_online)?;
        let busy = self.team_presence.saving.is_some();
        Some(v_flex().child(super::settings_view::setting_row("Show when I'm online",
            "Applies to this organization on every device. Teammates see your online status, chatting activity, and running \
             agent counts. Page titles appear only to people with access. This does not grant access to your content. When \
             off, your status appears offline and your activity is hidden. You still see who is online.",palette)
            .child(ui::switch("settings-show-online",checked,busy,palette).when(!busy,|switch|switch.on_click(cx.listener(move|this,_,_,cx|this.save_online(!checked,cx))))))
            .children(self.team_presence.error.as_ref().map(|error|div().text_size(px(12.)).text_color(palette.destructive).child(error.clone()))).into_any_element())
    }
}

pub(super) const SHOW_ONLINE: crate::settings_fields::AccountField =
    crate::settings_fields::AccountField("show-online");

/// A stack avatar's disc. It has no ring: the opaque fills and their hues part one disc from the next.
const STACK_DISC: f32 = 18.;
/// How far a stacked disc covers the one before it, at rest.
const STACK_OVERLAP: f32 = 5.;
/// The gap between discs while the pointer is over the stack.
const STACK_SPREAD: f32 = 4.;

/// A disc's left margin at `value` of the spread: the overlap at 0, the gap at 1.
fn stack_step(value: f32) -> f32 {
    -STACK_OVERLAP + (STACK_OVERLAP + STACK_SPREAD) * value
}

/// The soft shadow a stacked disc casts to its left, onto the disc it covers. Two teammates can get the same colour,
/// and without it their edge disappears. It is lighter on the light sidebar, where black reads stronger.
fn stack_shadow(palette: Palette) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: black().opacity(if palette.dark { 0.55 } else { 0.2 }),
        offset: point(px(-1.), px(0.)),
        blur_radius: px(3.),
        spread_radius: px(0.),
        inset: false,
    }]
}

/// The stack spreads with the UI's open duration and closes quicker, as a menu does.
fn stack_tween(spread: bool) -> motion::Tween {
    motion::Tween::new(if spread { motion::UI } else { motion::UI_EXIT }, motion::ease_out)
}

/// The avatar's fill: its hue at 30% over the sidebar, opaque (`color-mix` in `.tp-avatar`). A translucent fill lets
/// the avatar under it show through, so a stack reads as overlapping circles instead of discs laid on each other.
/// The mix is over the solid theme's sidebar: on a see-through window the sidebar is a low-alpha tier, and `blend`
/// keeps that alpha, so the disc lost its fill.
fn avatar_fill(tint: Hsla, palette: Palette) -> Hsla {
    palette.solid().sidebar.blend(tint.opacity(0.3))
}

fn avatar(member: &Member, stack: bool, palette: Palette) -> Stateful<Div> {
    let colors = [0x68ccf5, 0xf1ad71, 0xa9b8ff, 0xdca7ec];
    let tint: Hsla = rgb(colors[presence::hue(&member.user_id)]).into();
    let initials = presence::initials(member);
    div()
        .id(SharedString::from(format!("presence-{}-{stack}", member.user_id)))
        .relative()
        .flex_none()
        .size(px(if stack { STACK_DISC } else { 20. }))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(avatar_fill(tint, palette))
        .text_color(palette.foreground)
        .text_size(px(9.5))
        .font_weight(FontWeight::MEDIUM)
        .child(if stack {
            initials.chars().take(1).collect::<String>()
        } else {
            initials
        })
        .when(member.online && !stack, |avatar| {
            avatar.child(
                div()
                    .absolute()
                    .right(px(-4.))
                    .bottom(px(-4.))
                    .size(px(11.))
                    .rounded_full()
                    .bg(rgb(0x30d158))
                    .border_2()
                    .border_color(palette.sidebar),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn member(id: &str, online: bool) -> Member {
        Member {
            user_id: id.into(),
            name: Some(id.into()),
            email: Some(format!("{id}@example.test")),
            online,
            activity: Some(presence::Activity::Chatting),
        }
    }

    /// A stacked disc hides the one under it: its fill is opaque in both themes at every Transparency level, and
    /// still carries its hue.
    #[test]
    fn a_stacked_avatar_is_opaque_so_the_one_under_it_does_not_show_through() {
        for dark in [false, true] {
            for level in crate::frost::Transparency::ALL {
                let palette = Palette::solid_for(dark).frosted(level);
                let tint: Hsla = rgb(0x68ccf5).into();
                let fill = avatar_fill(tint, palette);
                assert!(
                    (fill.a - 1.).abs() < f32::EPSILON,
                    "dark {dark}, {level:?}: alpha {}",
                    fill.a
                );
                assert_ne!(fill, palette.solid().sidebar, "dark {dark}, {level:?}: no hue");
            }
        }
    }

    /// At rest a disc covers the one before it; under the pointer the two part by a gap, and the spread closes quicker
    /// than it opens.
    #[test]
    fn the_stack_overlaps_at_rest_and_spreads_apart_on_hover() {
        assert_eq!(stack_step(0.), -STACK_OVERLAP);
        assert_eq!(stack_step(1.), STACK_SPREAD);
        assert!(stack_step(0.5) > stack_step(0.) && stack_step(0.5) < stack_step(1.));
        assert_eq!(stack_tween(true).duration, motion::UI);
        assert!(stack_tween(false).duration < stack_tween(true).duration);
    }

    #[test]
    fn a_stacked_disc_casts_a_soft_shadow_left_and_lighter_on_the_light_sidebar() {
        let alpha = |choice| {
            let shadow = stack_shadow(Palette::for_choice(choice, gpui_kit::WindowAppearance::Light));
            assert_eq!(shadow.len(), 1);
            assert!(shadow[0].offset.x < px(0.), "the shadow falls on the disc to the left");
            assert!(!shadow[0].inset);
            shadow[0].color.a
        };
        let dark = alpha(crate::palette::ThemeChoice::Dark);
        let light = alpha(crate::palette::ThemeChoice::Light);
        assert!(light > 0. && light < dark, "light {light}, dark {dark}");
    }

    #[test]
    fn the_privacy_text_is_kept_for_the_header_popover() {
        let text = SHARING.join("\n");
        assert!(text.contains("online status"));
        assert!(text.contains("does not grant access"));
    }

    #[test]
    fn team_section_starts_collapsed_and_opens_only_when_saved_open() {
        assert!(!remembered_open(None));
        assert!(!remembered_open(Some("")));
        assert!(!remembered_open(Some("closed")));
        assert!(remembered_open(Some("open")));
    }

    #[test]
    fn your_row_is_kept_apart_from_sorted_teammates_and_email_fallback_matches_the_gateway() {
        let members = vec![member("offline", false), member("self", true), member("online", true)];
        let (own, others) = split_members(members.clone(), Some("self"), "unrelated@example.test");
        assert_eq!(own.unwrap().user_id, "self");
        assert_eq!(
            others.iter().map(|member| member.user_id.as_str()).collect::<Vec<_>>(),
            ["online", "offline"]
        );
        let (own, others) = split_members(members.clone(), None, "SELF@example.test");
        assert_eq!(own.unwrap().user_id, "self");
        assert_eq!(others.len(), 2);
        let (own, others) = split_members(members, Some("missing"), "self@example.test");
        assert!(own.is_none());
        assert_eq!(others.len(), 3);
        let (own, others) = split_members(vec![member("self", true)], Some("self"), "");
        assert!(own.is_some());
        assert!(others.is_empty());
    }

    #[test]
    fn your_row_uses_confirmed_visibility_instead_of_the_gateways_private_online_bit() {
        let own = member("self", true);
        let hidden = visible_self(&own, Some(false));
        assert!(!hidden.online);
        assert!(hidden.activity.is_none());
        assert_eq!(own_status(&hidden, Some(false)), "Hidden from teammates");
        assert!(own.online);
        assert!(own.activity.is_some());
        let unknown = visible_self(&own, None);
        assert!(!unknown.online);
        assert!(unknown.activity.is_none());
        assert_eq!(own_status(&unknown, None), "Visibility unavailable");
        assert_eq!(visibility_link(None), "Manage online visibility");
        let visible = visible_self(&own, Some(true));
        assert!(visible.online);
        assert_eq!(own_status(&visible, Some(true)), "chatting · visible to teammates");
        let offline = member("self", false);
        assert_eq!(own_status(&offline, Some(true)), "Offline · visible to teammates");
    }

    #[test]
    fn loading_unavailable_empty_and_failed_reads_do_not_claim_hidden_visibility() {
        assert_eq!(read_note(false, false, false, true), Some("Loading team…"));
        assert_eq!(
            read_note(false, true, false, true),
            Some("Team presence could not be loaded. Retrying.")
        );
        assert_eq!(
            read_note(false, true, true, false),
            Some("Team presence could not be refreshed. Retrying.")
        );
        assert_eq!(
            read_note(false, false, true, true),
            Some("No team presence is available yet.")
        );
        assert_eq!(
            read_note(true, false, false, true),
            Some("Team presence needs a newer Nexus Gateway.")
        );
        assert_eq!(read_note(false, false, true, false), None);
    }

    #[test]
    fn a_read_started_before_a_privacy_change_cannot_overwrite_the_saved_choice() {
        assert!(preference_read_current(3, 3, false));
        assert!(!preference_read_current(3, 4, false));
        assert!(!preference_read_current(4, 4, true));
    }
    #[test]
    fn unsupported_presence_stops_forbidden_or_missing_routes_but_not_transient_errors() {
        let error = |status| {
            nexus_client::CoreError {
                status,
                message: "failed".into(),
                code: None,
                interval: None,
                details: None,
            }
            .into()
        };
        assert!(unsupported(&error(403)));
        assert!(unsupported(&error(404)));
        assert!(!unsupported(&error(429)));
        assert!(!unsupported(&error(503)));
        assert!(!unsupported(&nexus_client::error::Transport::Timeout.into()));
        assert_eq!(body("chatting"), json!({"activity":{"kind":"chatting"}}));
        assert_eq!(body("idle"), json!({"activity":{"kind":"idle"}}));
    }
}
