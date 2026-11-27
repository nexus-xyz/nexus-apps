//! Settings › Members: who is in the organization, with their role and what they spent. Owners and admins invite
//! people by email, change roles and remove members; the owner hands the organization to someone else. Owners and
//! admins also keep the email domains whose people can join. It copies the desktop body of Nexus UI's page
//! (`ui/src/components/nexus-ui/settings/org/members.tsx`), which reads Gateway straight.
//!
//! Pending invitations are listed by the sign-in provider on Nexus UI's server only (Gateway has no list of them), so
//! they stay in the browser, behind a link.
//!
//! Each row also shows who is online and what they are doing, from the sidebar's presence poll
//! (`members_presence.rs`). With shared sessions on, a teammate who runs agents has a Watch button.

use super::members_presence::{Filter, Live, Status, Team};
use crate::workspace::MenuTrigger;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::time::Duration;

use super::{Ctx, Load, Verb, palette, row, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;
/// The person column: wide enough for a name and an email, so Role and Spent line up row to row.
const MEMBER_WIDTH: f32 = 260.;
/// How often the page reads the workspace's presence again. The workspace polls Gateway every 30 seconds; this only
/// picks its answer up sooner.
const LIVE_SECONDS: u64 = 5;
/// The green of an online dot, as the sidebar's team section draws it.
const ONLINE: u32 = 0x30d158;

pub fn build(ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| MembersPage::new(ctx, window, cx));
    // The presence callback reads Workspace. Wait until its page-construction update has ended.
    let live = view.clone();
    cx.defer(move |cx| live.update(cx, |page, cx| page.sync_team(cx)));
    Some(view.into())
}

/// The ways people join through an email domain, as Gateway names them, and how the page says each.
const DOMAIN_MODES: [(&str, &str); 3] = [
    ("automatic_invitation", "Invited automatically"),
    ("automatic_suggestion", "Suggested; an admin accepts each"),
    ("manual_invitation", "Invitation only"),
];

/// Who is looking: `GET /v1/tenants/me`.
#[derive(Debug, Clone, Default, PartialEq)]
struct Me {
    user_id: String,
    role: String,
    permissions: Vec<String>,
    personal: bool,
    email: String,
}

impl Me {
    fn can(&self, permission: &str) -> bool {
        self.permissions.iter().any(|each| each == permission)
    }
}

fn parse_me(value: &Value) -> Me {
    Me {
        user_id: value
            .pointer("/user/id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        role: ui::text(value, "role").to_string(),
        permissions: value
            .get("permissions")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
            .unwrap_or_default(),
        personal: value.pointer("/tenant/kind").and_then(Value::as_str) == Some("personal"),
        email: value
            .pointer("/user/email")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

/// A member: `GET /v1/orgs/members`.
#[derive(Debug, Clone, PartialEq)]
struct Member {
    user_id: String,
    email: String,
    name: String,
    /// `owner`, `admin` or `member`.
    role: String,
    /// Settled spend; `Null` when Gateway does not show it.
    spent: Value,
}

impl Member {
    fn who(&self) -> String {
        [&self.name, &self.email, &self.user_id]
            .into_iter()
            .find(|each| !each.is_empty())
            .cloned()
            .unwrap_or_else(|| "Someone".into())
    }
}

fn role_order(role: &str) -> u8 {
    match role {
        "owner" => 0,
        "admin" => 1,
        _ => 2,
    }
}

fn role_label(role: &str) -> &'static str {
    match role {
        "owner" => "Owner",
        "admin" => "Admin",
        _ => "Member",
    }
}

/// The members, owner first, then admins, then members, each group by name.
fn parse_members(value: &Value) -> Vec<Member> {
    let mut members: Vec<Member> = value
        .get("members")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|row| {
                    let user_id = ui::text(row, "userId");
                    (!user_id.is_empty()).then(|| Member {
                        user_id: user_id.to_string(),
                        email: ui::text(row, "email").to_string(),
                        name: ui::text(row, "name").to_string(),
                        role: match ui::text(row, "role") {
                            "owner" => "owner",
                            "admin" => "admin",
                            _ => "member",
                        }
                        .to_string(),
                        spent: row.get("spentUsd").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    members.sort_by(|a, b| {
        role_order(&a.role)
            .cmp(&role_order(&b.role))
            .then_with(|| a.who().to_lowercase().cmp(&b.who().to_lowercase()))
    });
    members
}

/// An email domain: `GET /v1/orgs/domains`.
#[derive(Debug, Clone, PartialEq)]
struct Domain {
    id: String,
    domain: String,
    mode: String,
    verified: bool,
    pending: u64,
}

fn parse_domains(value: &Value) -> Vec<Domain> {
    value
        .get("domains")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|row| {
                    let id = ui::text(row, "id");
                    (!id.is_empty()).then(|| Domain {
                        id: id.to_string(),
                        domain: ui::text(row, "domain").to_string(),
                        mode: ui::text(row, "enrollmentMode").to_string(),
                        verified: row.get("verified").and_then(Value::as_bool).unwrap_or(false),
                        pending: row.get("pendingInvitations").and_then(Value::as_u64).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn domain_mode_label(mode: &str) -> String {
    if mode == "enterprise_sso" {
        return "Through SSO".into();
    }
    DOMAIN_MODES
        .iter()
        .find(|(value, _)| *value == mode)
        .map(|(_, label)| label.to_string())
        .unwrap_or_else(|| mode.to_string())
}

/// The emails typed in the invite field, split on commas, spaces and semicolons. An error when one is not an email.
fn parse_emails(text: &str) -> Result<Vec<String>, &'static str> {
    let list: Vec<String> = text
        .split([',', ';', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|each| !each.is_empty())
        .map(String::from)
        .collect();
    let valid = |email: &str| {
        let Some((local, host)) = email.split_once('@') else {
            return false;
        };
        !local.is_empty()
            && !host.contains('@')
            && host.split('.').filter(|part| !part.is_empty()).count() >= 2
            && !host.starts_with('.')
            && !host.ends_with('.')
    };
    if list.iter().any(|email| !valid(email)) {
        return Err("Enter valid email addresses, separated by commas.");
    }
    Ok(list)
}

/// What `POST /v1/orgs/invitations` did, one sentence per email, and whether every one failed.
fn invite_outcome(value: &Value) -> (String, bool) {
    let results = value
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut failed = 0;
    let text = results
        .iter()
        .map(|result| {
            let email = ui::text(result, "email");
            let status = ui::text(result, "status");
            match status {
                "sent" => format!("Invited {email}."),
                "would_invite" => format!("{email} would be invited."),
                "already_invited" => format!("{email} already has an invitation."),
                "already_member" => format!("{email} is already in the organization."),
                "not_signed_in" => {
                    format!("{email} is already in the organization: ask them to sign in to Nexus once.")
                }
                "removed" => format!("{email} was removed; recover them with nexus members recover."),
                "failed" => {
                    failed += 1;
                    let why = ui::text(result, "message");
                    format!(
                        "{email}: {}.",
                        if why.is_empty() {
                            "not sent"
                        } else {
                            why.trim_end_matches('.')
                        }
                    )
                }
                other => format!("{email}: {other}."),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let all_failed = !results.is_empty() && failed == results.len();
    (text, all_failed)
}

/// Gateway's refusal said plainly: a 403 names who may do it; anything else is Gateway's own words.
fn refusal(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.starts_with("only the org's owner or admins") {
        "Only organization admins can change this.".into()
    } else if lower.starts_with("only the org's owner,") {
        "Only organization admins and team leads can change this.".into()
    } else if lower.starts_with("only the org's owner") {
        "Only the organization's owner can do this.".into()
    } else {
        format!("{}.", message.trim().trim_end_matches('.'))
    }
}

/// Two letters for a person's circle: `Ada Lovelace` is `AL`.
fn initials(text: &str) -> String {
    let letters: String = text
        .split(|c: char| c.is_whitespace() || "@._-".contains(c))
        .filter(|part| !part.is_empty())
        .take(2)
        .filter_map(|part| part.chars().next())
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() { "?".into() } else { letters }
}

/// A line under a list: what a change did, or why it did not happen.
#[derive(Debug, Clone)]
struct Outcome {
    text: SharedString,
    error: bool,
}

impl Outcome {
    fn done(text: impl Into<SharedString>) -> Option<Outcome> {
        Some(Outcome {
            text: text.into(),
            error: false,
        })
    }

    fn failed(title: &str, message: &str) -> Option<Outcome> {
        Some(Outcome {
            text: format!("{title}: {}", refusal(message)).into(),
            error: true,
        })
    }
}

/// A button pressed once that waits for the second press.
#[derive(Debug, Clone, PartialEq)]
enum Confirm {
    Remove(String),
    Owner(String),
    Domain(String),
}

/// A row's open menu, by member id.
#[derive(Debug, Clone, PartialEq)]
enum RowMenu {
    Role(String),
    More(String),
}

/// What a member spent: `$1.41`, and `<$0.01` for the tiny amounts, so the column keeps two decimals.
fn spent_text(value: &Value) -> String {
    let amount = match value {
        Value::String(text) => text.parse::<f64>().ok(),
        Value::Number(number) => number.as_f64(),
        _ => None,
    };
    match amount {
        Some(amount) if amount > 0. && amount < 0.01 => "<$0.01".into(),
        _ => ui::dollars(value),
    }
}

/// Await a Gateway call and apply its answer with the window at hand (to clear a field), then redraw.
fn fetch_in<V: 'static>(
    window: &mut Window,
    cx: &mut Context<V>,
    request: impl std::future::Future<Output = Result<Value, String>> + 'static,
    apply: impl FnOnce(&mut V, Result<Value, String>, &mut Window, &mut Context<V>) + 'static,
) {
    cx.spawn_in(window, async move |this, cx| {
        let answer = request.await;
        this.update_in(cx, |view, window, cx| {
            apply(view, answer, window, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

struct MembersPage {
    ctx: Ctx,
    me: Load<Me>,
    members: Load<Vec<Member>>,
    /// `None` until we know the person may see them (`members.invite`).
    domains: Option<Load<Vec<Domain>>>,
    invite: Entity<InputState>,
    invite_admin: bool,
    sending: bool,
    outcome: Option<Outcome>,
    confirm: Option<Confirm>,
    /// The row menu that is open.
    menu: Option<RowMenu>,
    /// A change is on its way: the row buttons wait for it.
    working: bool,
    adding_domain: bool,
    domain: Entity<InputState>,
    domain_mode: usize,
    domain_outcome: Option<Outcome>,
    /// The presence the page last drew.
    team: Team,
    filter: Filter,
    _subscriptions: Vec<Subscription>,
    _live: Task<()>,
}

impl MembersPage {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> MembersPage {
        let invite = cx.new(|cx| InputState::new(window, cx).placeholder("Emails, separated by commas"));
        let domain = cx.new(|cx| InputState::new(window, cx).placeholder("acme.com"));
        let subscriptions = vec![
            cx.subscribe_in(&invite, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.send_invites(window, cx);
                }
            }),
            cx.subscribe_in(&domain, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_domain(window, cx);
                }
            }),
        ];
        let live = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(LIVE_SECONDS)).await;
                if this.update(cx, |page, cx| page.sync_team(cx)).is_err() {
                    break;
                }
            }
        });
        let mut page = MembersPage {
            ctx,
            me: Load::Loading,
            members: Load::Loading,
            domains: None,
            invite,
            invite_admin: false,
            sending: false,
            outcome: None,
            confirm: None,
            menu: None,
            working: false,
            adding_domain: false,
            domain,
            domain_mode: 0,
            domain_outcome: None,
            team: Team::default(),
            filter: Filter::All,
            _subscriptions: subscriptions,
            _live: live,
        };
        page.load(cx);
        page
    }

    /// Draw again when the workspace's presence changed.
    fn sync_team(&mut self, cx: &mut Context<Self>) {
        let team = (self.ctx.team)(cx);
        if team != self.team {
            self.team = team;
            cx.notify();
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.me = Load::Loading;
        self.members = Load::Loading;
        let me = self.ctx.gateway.query("/v1/tenants/me".into());
        super::read(cx, me, |this, answer, cx| match answer {
            Ok(value) => {
                let me = parse_me(&value);
                if me.can("members.invite") && !me.personal {
                    this.load_domains(cx);
                }
                this.me = Load::Ready(me);
            }
            Err(error) => this.me = Load::Failed(error.into()),
        });
        self.load_members(cx);
    }

    fn load_members(&mut self, cx: &mut Context<Self>) {
        let request = self.ctx.gateway.query("/v1/orgs/members".into());
        super::read(cx, request, |this, answer, _| {
            this.members = match answer {
                Ok(value) => Load::Ready(parse_members(&value)),
                Err(error) => Load::Failed(error.into()),
            };
        });
    }

    fn load_domains(&mut self, cx: &mut Context<Self>) {
        if self.domains.is_none() {
            self.domains = Some(Load::Loading);
        }
        let request = self.ctx.gateway.query("/v1/orgs/domains".into());
        super::read(cx, request, |this, answer, _| {
            this.domains = Some(match answer {
                Ok(value) => Load::Ready(parse_domains(&value)),
                Err(error) => {
                    let lower = error.to_lowercase();
                    if lower.contains("404") || lower.contains("not found") {
                        Load::Failed(
                            "This Gateway does not serve email domains; use the CLI: nexus org domains.".into(),
                        )
                    } else {
                        Load::Failed(format!("Email domains could not be loaded: {error}").into())
                    }
                }
            });
        });
    }

    fn send_invites(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let typed = self.invite.read(cx).value().to_string();
        let emails = match parse_emails(&typed) {
            Ok(list) if list.is_empty() => return,
            Ok(list) => list,
            Err(why) => {
                self.outcome = Some(Outcome {
                    text: why.into(),
                    error: true,
                });
                cx.notify();
                return;
            }
        };
        let role = if self.invite_admin { "admin" } else { "member" };
        self.sending = true;
        self.outcome = None;
        cx.notify();
        if emails.len() == 1 {
            self.send(json!({ "emails": emails, "role": role }), window, cx);
            return;
        }
        // More than one email: Gateway sends only the count we confirm, so ask it first how many it would send.
        let body = json!({ "emails": emails, "role": role, "dryRun": true });
        let request = self
            .ctx
            .gateway
            .request(Verb::Post, "/v1/orgs/invitations".into(), Some(body));
        fetch_in(window, cx, request, move |this, answer, window, cx| match answer {
            Ok(value) => {
                let to_send = value.get("toSend").and_then(Value::as_u64).unwrap_or(0);
                if to_send == 0 {
                    this.sending = false;
                    let (text, _) = invite_outcome(&value);
                    this.outcome = Some(Outcome {
                        text: format!("No invitations were sent. {text}").into(),
                        error: true,
                    });
                } else {
                    this.send(json!({ "emails": emails, "role": role, "expect": to_send }), window, cx);
                }
            }
            Err(error) => {
                this.sending = false;
                this.outcome = Outcome::failed("No invitations were sent", &error);
            }
        });
    }

    fn send(&mut self, body: Value, window: &mut Window, cx: &mut Context<Self>) {
        let request = self
            .ctx
            .gateway
            .request(Verb::Post, "/v1/orgs/invitations".into(), Some(body));
        fetch_in(window, cx, request, |this, answer, window, cx| {
            this.sending = false;
            match answer {
                Ok(value) => {
                    let (text, all_failed) = invite_outcome(&value);
                    this.outcome = Some(Outcome {
                        text: text.into(),
                        error: all_failed,
                    });
                    if !all_failed {
                        this.invite.update(cx, |state, cx| state.set_value("", window, cx));
                        this.load_members(cx);
                    }
                }
                Err(error) => this.outcome = Outcome::failed("No invitations were sent", &error),
            }
        });
    }

    /// Run a change on a member, then read the list again.
    fn change(
        &mut self,
        verb: Verb,
        path: String,
        body: Option<Value>,
        done: String,
        failed: String,
        cx: &mut Context<Self>,
    ) {
        if self.working {
            return;
        }
        self.working = true;
        self.confirm = None;
        self.outcome = None;
        let request = self.ctx.gateway.request(verb, path, body);
        super::fetch(cx, request, move |this, answer, cx| {
            this.working = false;
            match answer {
                Ok(_) => {
                    this.outcome = Outcome::done(done);
                    this.load_members(cx);
                    // Ownership moved: what we may do changed too.
                    let me = this.ctx.gateway.query("/v1/tenants/me".into());
                    super::read(cx, me, |this, answer, _| {
                        if let Ok(value) = answer {
                            this.me = Load::Ready(parse_me(&value));
                        }
                    });
                }
                Err(error) => this.outcome = Outcome::failed(&failed, &error),
            }
        });
        cx.notify();
    }

    fn toggle_menu(&mut self, menu: RowMenu, cx: &mut Context<Self>) {
        self.menu = (self.menu.as_ref() != Some(&menu)).then_some(menu);
        self.confirm = None;
        cx.notify();
    }

    fn set_role(&mut self, member: &Member, admin: bool, cx: &mut Context<Self>) {
        let who = member.who();
        let role = if admin { "admin" } else { "member" };
        self.change(
            Verb::Post,
            format!("/v1/orgs/members/{}/role", encode(&member.user_id)),
            Some(json!({ "role": role })),
            format!("{who} is now {}.", if admin { "an admin" } else { "a member" }),
            format!("{who}'s role didn't change"),
            cx,
        );
    }

    fn add_domain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.domain.read(cx).value().trim().to_lowercase();
        if name.is_empty() || self.working {
            return;
        }
        self.working = true;
        self.domain_outcome = None;
        let body = json!({ "domain": name, "enrollmentMode": DOMAIN_MODES[self.domain_mode].0 });
        let request = self
            .ctx
            .gateway
            .request(Verb::Post, "/v1/orgs/domains".into(), Some(body));
        fetch_in(window, cx, request, move |this, answer, window, cx| {
            this.working = false;
            match answer {
                Ok(value) => {
                    let added = ui::text(&value, "domain");
                    let added = if added.is_empty() { name.as_str() } else { added };
                    this.domain_outcome = Outcome::done(format!(
                        "Added {added}. People with a verified email there can now join."
                    ));
                    this.adding_domain = false;
                    this.domain.update(cx, |state, cx| state.set_value("", window, cx));
                    this.load_domains(cx);
                }
                Err(error) => this.domain_outcome = Outcome::failed(&format!("{name} wasn't added"), &error),
            }
        });
        cx.notify();
    }

    fn remove_domain(&mut self, domain: &Domain, cx: &mut Context<Self>) {
        if self.working {
            return;
        }
        self.working = true;
        self.confirm = None;
        self.domain_outcome = None;
        let name = domain.domain.clone();
        let request = self
            .ctx
            .gateway
            .request(Verb::Delete, format!("/v1/orgs/domains/{}", encode(&domain.id)), None);
        super::fetch(cx, request, move |this, answer, cx| {
            this.working = false;
            match answer {
                Ok(_) => {
                    this.domain_outcome =
                        Outcome::done(format!("{name} was removed. Who already joined through it stays."));
                    this.load_domains(cx);
                }
                Err(error) => this.domain_outcome = Outcome::failed(&format!("{name} wasn't removed"), &error),
            }
        });
        cx.notify();
    }

    fn render_invite(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        invite_form(&self.invite, self.invite_admin, self.sending, palette, Some(cx))
    }

    fn render_row(&self, ix: usize, member: &Member, me: &Me, palette: Palette, cx: &mut Context<Self>) -> Div {
        let manage = me.can("members.manage");
        let is_owner = me.role == "owner";
        let own = member.user_id == me.user_id;
        let editable = manage && member.role != "owner" && !own;
        let who = member.who();
        let detail = [
            (!member.name.is_empty())
                .then(|| member.email.clone())
                .filter(|email| !email.is_empty()),
            own.then(|| "you".to_string()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let title = if member.name.is_empty() {
            who.clone()
        } else {
            member.name.clone()
        };

        let group = SharedString::from(format!("members-row-{ix}"));
        let role_open = self.menu == Some(RowMenu::Role(member.user_id.clone()));
        let role: AnyElement = if editable {
            // The role as text with a chevron; the menu changes it. A control on each of 40 rows was heavy.
            let id = member.user_id.clone();
            let pick = |admin: bool| {
                let target = member.clone();
                let picked = (member.role == "admin") == admin;
                row::menu_item(
                    SharedString::from(format!("members-role-{ix}-{admin}")),
                    if admin { "Admin" } else { "Member" },
                    picked,
                    0,
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.menu = None;
                    if !picked {
                        this.set_role(&target, admin, cx);
                    }
                    cx.notify();
                }))
            };
            div()
                .relative()
                .child(
                    h_flex()
                        .id(SharedString::from(format!("members-role-{ix}")))
                        .h(px(ui::CONTROL_HEIGHT))
                        .ml(px(-8.))
                        .px(px(8.))
                        .gap(px(4.))
                        .rounded(px(8.))
                        .text_size(px(13.))
                        .text_color(palette.foreground)
                        .cursor_pointer()
                        .when(role_open, |button| button.bg(palette.hover))
                        .hover(|style| style.bg(palette.hover))
                        .child(role_label(&member.role))
                        // The chevron shows with the row's hover, so 40 rows don't each carry a control.
                        .child(
                            div()
                                .when(!role_open, |chevron| {
                                    chevron.invisible().group_hover(group.clone(), |style| style.visible())
                                })
                                .child(ui::icon(Lucide::ChevronDown, palette.icon)),
                        )
                        .menu_trigger(
                            role_open,
                            cx,
                            {
                                let menu = RowMenu::Role(id.clone());
                                move |this| this.menu.as_ref() == Some(&menu)
                            },
                            move |this, _, cx| this.toggle_menu(RowMenu::Role(id.clone()), cx),
                        ),
                )
                .when(role_open, |anchor| {
                    anchor.child(row::popover(
                        v_flex()
                            .w(px(160.))
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                cx.notify();
                            }))
                            .child(pick(false))
                            .child(pick(true)),
                        palette,
                    ))
                })
                .into_any_element()
        } else {
            div()
                .text_size(px(13.))
                .text_color(palette.muted)
                .child(role_label(&member.role))
                .into_any_element()
        };

        let spent: SharedString = if member.spent.is_null() {
            "None".into()
        } else {
            spent_text(&member.spent).into()
        };

        // The row's other actions (Make owner, Remove) live in a "…" menu that shows on hover, or while it is open.
        let more_open = self.menu == Some(RowMenu::More(member.user_id.clone()));
        let mut items: Vec<AnyElement> = Vec::new();
        if is_owner && !own {
            let confirming = self.confirm == Some(Confirm::Owner(member.user_id.clone()));
            let id = member.user_id.clone();
            let name = who.clone();
            items.push(
                row::menu_item(
                    SharedString::from(format!("members-owner-{ix}")),
                    if confirming {
                        "Make owner? Press again"
                    } else {
                        "Make owner"
                    },
                    false,
                    0,
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.confirm == Some(Confirm::Owner(id.clone())) {
                        this.menu = None;
                        this.change(
                            Verb::Post,
                            "/v1/orgs/owner".into(),
                            Some(json!({ "userId": id })),
                            format!("{name} now owns the organization; you are an admin."),
                            "Ownership didn't move".into(),
                            cx,
                        );
                    } else {
                        this.confirm = Some(Confirm::Owner(id.clone()));
                        cx.notify();
                    }
                }))
                .into_any_element(),
            );
        }
        if editable {
            let confirming = self.confirm == Some(Confirm::Remove(member.user_id.clone()));
            let id = member.user_id.clone();
            let name = who.clone();
            items.push(
                row::menu_item(
                    SharedString::from(format!("members-remove-{ix}")),
                    if confirming {
                        format!("Remove {}? Press again", short(&who))
                    } else {
                        "Remove from organization".into()
                    },
                    false,
                    0,
                    palette,
                )
                .text_color(palette.destructive)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.confirm == Some(Confirm::Remove(id.clone())) {
                        this.menu = None;
                        this.change(
                            Verb::Delete,
                            format!("/v1/orgs/members/{}", encode(&id)),
                            None,
                            format!("{name} was removed, and their Nexus tokens revoked."),
                            format!("{name} wasn't removed"),
                            cx,
                        );
                    } else {
                        this.confirm = Some(Confirm::Remove(id.clone()));
                        cx.notify();
                    }
                }))
                .into_any_element(),
            );
        }
        let live = self.team.live(&member.user_id, own);
        let activity = self
            .team
            .shown()
            .then(|| self.render_activity(ix, member, live.clone(), palette));
        let actions = h_flex().flex_1().justify_end().when(!items.is_empty(), |actions| {
            let id = member.user_id.clone();
            actions.child(
                div()
                    .relative()
                    .when(!more_open, |anchor| {
                        anchor.invisible().group_hover(group.clone(), |style| style.visible())
                    })
                    .child(
                        row::icon_button(
                            SharedString::from(format!("members-more-{ix}")),
                            Lucide::Ellipsis,
                            "More",
                            palette,
                        )
                        .menu_trigger(
                            more_open,
                            cx,
                            {
                                let menu = RowMenu::More(id.clone());
                                move |this| this.menu.as_ref() == Some(&menu)
                            },
                            move |this, _, cx| this.toggle_menu(RowMenu::More(id.clone()), cx),
                        ),
                    )
                    .when(more_open, |anchor| {
                        anchor.child(row::popover(
                            v_flex()
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.menu = None;
                                    this.confirm = None;
                                    cx.notify();
                                }))
                                .children(items),
                            palette,
                        ))
                    }),
            )
        });

        ui::row(palette)
            .group(group)
            .hover(|style| style.bg(palette.hover))
            .child(
                h_flex()
                    .w(px(MEMBER_WIDTH))
                    .flex_none()
                    .min_w(px(0.))
                    .gap(px(10.))
                    .child(avatar(&who, live.map(|live| live.status), palette))
                    .child(ui::two_lines(title, detail, palette)),
            )
            .child(div().w(px(150.)).flex_none().child(role))
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(8.))
                    .children(activity)
                    .child(actions),
            )
            .child(
                div()
                    .w(px(80.))
                    .flex_none()
                    .text_right()
                    .text_size(px(13.))
                    .text_color(if member.spent.is_null() {
                        palette.faint
                    } else {
                        palette.foreground
                    })
                    .child(spent),
            )
    }

    /// The Activity column: what the person is doing, a pulse while agents run, and Watch for a teammate's sessions.
    /// A grey bar until the first presence poll answers.
    fn render_activity(&self, ix: usize, member: &Member, live: Option<Live>, palette: Palette) -> AnyElement {
        let Some(live) = live else {
            return div()
                .w(px(96.))
                .h(px(10.))
                .rounded(px(5.))
                .bg(palette.hover)
                .into_any_element();
        };
        let quiet = matches!(live.status, Status::Idle | Status::Offline | Status::Hidden);
        let person = member.user_id.clone();
        let watch = self.ctx.watch.clone();
        h_flex()
            .flex_1()
            .min_w(px(0.))
            .gap(px(8.))
            .text_size(px(13.))
            .text_color(if quiet { palette.faint } else { palette.foreground })
            .when(live.running, |cell| {
                cell.child(div().flex_none().size(px(6.)).rounded_full().bg(rgb(ONLINE)))
            })
            .child(div().min_w(px(0.)).truncate().child(live.text))
            .when(live.watch, |cell| {
                cell.child(
                    ui::button(SharedString::from(format!("members-watch-{ix}")), "Watch", palette)
                        .flex_none()
                        .on_click(move |_, window, cx| watch(person.clone(), window, cx)),
                )
            })
            .into_any_element()
    }

    /// "Everyone", "Online" and "Running agents" over the list, with their counts.
    fn render_filter(&self, members: &[Member], me: &Me, palette: Palette, cx: &mut Context<Self>) -> Div {
        let count = |filter: Filter| {
            members
                .iter()
                .filter(|member| self.team.passes(filter, &member.user_id, member.user_id == me.user_id))
                .count()
        };
        let tab = |id: &'static str, text: String, filter: Filter| {
            ui::tab(id, text, self.filter == filter, palette).on_click(cx.listener(move |this, _, _, cx| {
                this.filter = filter;
                cx.notify();
            }))
        };
        let running = count(Filter::Running);
        ui::tabs(palette)
            .flex_none()
            .child(tab("members-filter-all", "Everyone".into(), Filter::All))
            .child(tab(
                "members-filter-online",
                format!("Online {}", count(Filter::Online)),
                Filter::Online,
            ))
            .when(running > 0 || self.filter == Filter::Running, |tabs| {
                tabs.child(tab(
                    "members-filter-running",
                    format!("Running agents {running}"),
                    Filter::Running,
                ))
            })
    }

    fn render_domains(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let domains = self.domains.as_ref()?;
        let head = h_flex()
            .justify_between()
            .items_end()
            .child(ui::section_head("Email domains", palette))
            .when(!matches!(domains, Load::Failed(_)), |head| {
                head.child(
                    ui::button(
                        "members-domain-toggle",
                        if self.adding_domain { "Cancel" } else { "Add domain" },
                        palette,
                    )
                    .mb(px(8.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.adding_domain = !this.adding_domain;
                        cx.notify();
                    })),
                )
            });
        let mut column = v_flex()
            .child(head)
            .child(ui::note("People with a verified email on these domains can join.", palette).mb(px(8.)));
        if self.adding_domain {
            let modes = DOMAIN_MODES.iter().enumerate().map(|(at, (_, label))| {
                ui::chip(
                    SharedString::from(format!("members-domain-mode-{at}")),
                    *label,
                    self.domain_mode == at,
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.domain_mode = at;
                    cx.notify();
                }))
            });
            column = column.child(
                v_flex()
                    .gap(px(8.))
                    .mb(px(8.))
                    .child(
                        h_flex().gap(px(8.)).child(field(&self.domain, palette)).child(
                            ui::primary_button("members-domain-add", "Add", palette)
                                .on_click(cx.listener(|this, _, window, cx| this.add_domain(window, cx))),
                        ),
                    )
                    .child(h_flex().flex_wrap().gap(px(6.)).children(modes)),
            );
        }
        let list: AnyElement = match domains {
            Load::Loading => ui::skeleton(1, palette).into_any_element(),
            Load::Failed(message) => ui::note(message.clone(), palette).into_any_element(),
            Load::Ready(list) if list.is_empty() => {
                if self.adding_domain {
                    div().into_any_element()
                } else {
                    ui::note(
                        "No email domains yet. Add one you have a verified email on; public mail domains are refused.",
                        palette,
                    )
                    .into_any_element()
                }
            }
            Load::Ready(list) => v_flex()
                .children(list.iter().enumerate().map(|(ix, entry)| {
                    let confirming = self.confirm == Some(Confirm::Domain(entry.id.clone()));
                    let target = entry.clone();
                    let detail = [
                        Some(domain_mode_label(&entry.mode)),
                        (entry.pending > 0).then(|| format!("{} pending", entry.pending)),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                    ui::row(palette)
                        .child(ui::icon(gpui_kit::assets::IconName::Globe, palette.icon))
                        .child(ui::two_lines(entry.domain.clone(), detail, palette).flex_1())
                        .when(entry.verified, |row| row.child(ui::badge("Verified", palette)))
                        .child(
                            ui::danger_button(
                                SharedString::from(format!("members-domain-remove-{ix}")),
                                if confirming {
                                    format!("Remove {}", entry.domain)
                                } else {
                                    "Remove".into()
                                },
                                palette,
                            )
                            .when(confirming, |button| button.bg(palette.destructive.opacity(0.14)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.confirm == Some(Confirm::Domain(target.id.clone())) {
                                    this.remove_domain(&target, cx);
                                } else {
                                    this.confirm = Some(Confirm::Domain(target.id.clone()));
                                    cx.notify();
                                }
                            })),
                        )
                }))
                .into_any_element(),
        };
        Some(
            column
                .child(list)
                .children(
                    self.domain_outcome
                        .clone()
                        .map(|outcome| outcome_line(outcome, palette)),
                )
                .into_any_element(),
        )
    }
}

impl Render for MembersPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let page = v_flex().w_full().child(ui::lead(
            "Owners handle billing. Admins manage people, keys and policy. Members use the products.",
            palette,
        ));
        let me = match &self.me {
            Load::Ready(me) => me.clone(),
            Load::Loading => return page.child(ui::skeleton(6, palette)),
            Load::Failed(message) => return page.child(self.failed(message.clone(), palette, cx)),
        };
        if me.personal {
            let you = if me.email.is_empty() {
                "You".to_string()
            } else {
                me.email.clone()
            };
            return page
                .child(ui::section_head("Your personal organization", palette))
                .child(
                    ui::row(palette)
                        .child(avatar(&you, None, palette))
                        .child(ui::two_lines(you, "you", palette).flex_1())
                        .child(ui::note("Owner", palette)),
                )
                .child(ui::note(
                    "A personal organization is just you. To work with others, create an organization from the switcher.",
                    palette,
                ).mt(px(12.)));
        }
        let manage = me.can("members.manage");
        let page = page.when(me.can("members.invite"), |page| {
            page.child(self.render_invite(palette, cx))
        });
        let members = match &self.members {
            Load::Loading => return page.child(ui::skeleton(6, palette)),
            Load::Failed(message) => return page.child(self.failed(message.clone(), palette, cx)),
            Load::Ready(members) => members.clone(),
        };
        let count = members.len();
        let live = self.team.shown();
        let head = ui::head_row(palette)
            .child(div().w(px(MEMBER_WIDTH)).flex_none().child("Person"))
            .child(div().w(px(150.)).flex_none().child("Role"))
            .child(div().flex_1().when(live, |column| column.child("Activity")))
            .child(div().w(px(80.)).flex_none().text_right().child("Spent"));
        // Unfiltered indexes, so a row's element ids stay the same as the filter changes.
        let rows: Vec<Div> = members
            .iter()
            .enumerate()
            .filter(|(_, member)| {
                !live
                    || self
                        .team
                        .passes(self.filter, &member.user_id, member.user_id == me.user_id)
            })
            .map(|(ix, member)| self.render_row(ix, member, &me, palette, cx))
            .collect();
        let empty = rows.is_empty().then(|| {
            ui::note(
                match self.filter {
                    Filter::Running => "No one is running agents right now.",
                    _ => "No one is online right now.",
                },
                palette,
            )
            .mt(px(8.))
        });
        let filter = live.then(|| self.render_filter(&members, &me, palette, cx));
        let summary = match live && self.team.loaded {
            true => format!(
                "{count} {} · {} online",
                if count == 1 { "member" } else { "members" },
                self.team.online()
            ),
            false => format!("{count} {}", if count == 1 { "member" } else { "members" }),
        };
        let domains = self.render_domains(palette, cx);
        let org = self.ctx.clone();
        page.child(
            h_flex()
                .justify_between()
                .items_end()
                .child(ui::section_head("People", palette))
                .child(ui::label(summary, palette).mb(px(10.))),
        )
        .children(filter.map(|filter| h_flex().mb(px(8.)).child(filter)))
        .child(head)
        .children(rows)
        .children(empty)
        .children(
            self.outcome
                .clone()
                .map(|outcome| outcome_line(outcome, palette).mt(px(10.))),
        )
        .when(manage, |page| {
            page.child(
                h_flex().mt(px(12.)).child(
                    ui::web_link(
                        "members-pending-web",
                        "Pending invitations and people who haven't joined",
                        palette,
                    )
                    .on_click(move |_, _, cx| org.open_web("settings/members", cx)),
                ),
            )
        })
        .children(domains)
    }
}

impl MembersPage {
    fn failed(&self, message: SharedString, palette: Palette, cx: &mut Context<Self>) -> Div {
        ui::failed(message, palette).child(ui::button("members-retry", "Try again", palette).mt(px(8.)).on_click(
            cx.listener(|this, _, _, cx| {
                this.load(cx);
                cx.notify();
            }),
        ))
    }
}

fn invite_form(
    invite: &Entity<InputState>,
    admin: bool,
    sending: bool,
    palette: Palette,
    mut account: Option<&mut Context<MembersPage>>,
) -> Div {
    let mut roles = ui::tabs(palette).flex_none();
    for (id, label, is_admin) in [
        ("members-invite-member", "Member", false),
        ("members-invite-admin", "Admin", true),
    ] {
        roles = roles.child(match account.as_deref_mut() {
            Some(cx) => ui::tab(id, label, admin == is_admin, palette).on_click(cx.listener(move |this, _, _, cx| {
                this.invite_admin = is_admin;
                cx.notify();
            })),
            None => ui::disabled_button(id, label, palette),
        });
    }
    let label = if sending { "Sending…" } else { "Send invites" };
    let enabled = account.is_some();
    let send = match account {
        Some(cx) => ui::primary_button("members-invite-send", label, palette)
            .on_click(cx.listener(|this, _, window, cx| this.send_invites(window, cx))),
        None => ui::disabled_button("members-invite-send", label, palette),
    };
    h_flex()
        .gap(px(8.))
        .mb(px(6.))
        .child(field_with_access(invite, !enabled, palette))
        .child(roles)
        .child(send)
}

pub(super) fn build_locked(window: &mut Window, cx: &mut App) -> AnyView {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Emails, separated by commas"));
    ui::locked_page(cx, move |palette| {
        invite_form(&input, false, false, palette, None).into_any_element()
    })
}

/// A text field in the soft track Settings uses.
fn field(state: &Entity<InputState>, palette: Palette) -> Div {
    field_with_access(state, false, palette)
}

fn field_with_access(state: &Entity<InputState>, disabled: bool, palette: Palette) -> Div {
    h_flex()
        .flex_1()
        .min_w(px(0.))
        .h(px(32.))
        .px(px(6.))
        .rounded(px(8.))
        .bg(palette.hover)
        .text_size(px(13.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .child(Input::new(state).appearance(false).disabled(disabled)),
        )
}

fn outcome_line(outcome: Outcome, palette: Palette) -> Div {
    if outcome.error {
        ui::error_line(outcome.text, palette)
    } else {
        ui::note(outcome.text, palette)
    }
}

/// A person's initials in a circle, with a green dot while online and an amber one while idle.
fn avatar(who: &str, status: Option<Status>, palette: Palette) -> Div {
    let dot = match status {
        Some(Status::Active) => Some(rgb(ONLINE).into()),
        Some(Status::Idle) => Some(palette.warning),
        _ => None,
    };
    div()
        .relative()
        .flex_none()
        .size(px(28.))
        .rounded(px(14.))
        .bg(palette.hover)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.muted)
        .child(initials(who))
        .children(dot.map(|color: Hsla| {
            div()
                .absolute()
                .right(px(-2.))
                .bottom(px(-2.))
                .size(px(10.))
                .rounded_full()
                .bg(color)
                .border_2()
                .border_color(palette.background)
        }))
}

/// A name short enough for a button: the part before `@` of an email.
fn short(who: &str) -> String {
    let name = who.split('@').next().unwrap_or(who);
    if name.chars().count() > 18 {
        format!("{}…", name.chars().take(17).collect::<String>())
    } else {
        name.to_string()
    }
}

/// A path segment, percent-encoded.
fn encode(segment: &str) -> String {
    let mut out = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    // The glob import of gpui_kit brings its own `test` macro: name the built-in one.
    use core::prelude::v1::test;

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).expect("a JSON sample")
    }

    #[test]
    fn reads_who_is_looking() {
        let me = parse_me(&parse(
            r#"{
            "tenant": { "id": "t_1", "name": "Acme", "createdAt": "2026-01-01T00:00:00Z", "slug": "acme", "kind": "company" },
            "user": { "id": "u_1", "email": "ada@acme.com" },
            "role": "admin",
            "permissions": ["members.manage", "members.invite", "teams.manage"]
        }"#,
        ));
        assert_eq!(me.user_id, "u_1");
        assert_eq!(me.role, "admin");
        assert!(me.can("members.manage"));
        assert!(!me.can("org.transfer"));
        assert!(!me.personal);
        assert!(parse_me(&parse(r#"{ "tenant": { "kind": "personal" } }"#)).personal);
    }

    #[test]
    fn reads_members_owner_first() {
        let members = parse_members(&parse(
            r#"{
            "members": [
                { "userId": "u_3", "email": "zed@acme.com", "role": "member", "joinedAt": "2026-02-01T00:00:00Z" },
                { "userId": "u_2", "email": "bo@acme.com", "name": "Bo", "role": "admin", "joinedAt": "2026-02-01T00:00:00Z", "spentUsd": 12.3, "calls": 40 },
                { "userId": "u_1", "email": "ada@acme.com", "role": "owner", "joinedAt": "2026-01-01T00:00:00Z", "spentUsd": 0 },
                { "email": "no-id@acme.com", "role": "member" }
            ]
        }"#,
        ));
        let order: Vec<&str> = members.iter().map(|m| m.user_id.as_str()).collect();
        assert_eq!(order, ["u_1", "u_2", "u_3"]);
        assert_eq!(members[1].who(), "Bo");
        assert_eq!(members[2].who(), "zed@acme.com");
        assert!(members[2].spent.is_null());
        assert_eq!(ui::dollars(&members[1].spent), "$12.30");
    }

    #[test]
    fn reads_domains() {
        let domains = parse_domains(&parse(
            r#"{
            "domains": [{ "id": "dmn_1", "domain": "acme.com", "enrollmentMode": "automatic_invitation", "verified": true, "pendingInvitations": 2 }]
        }"#,
        ));
        assert_eq!(domains.len(), 1);
        assert!(domains[0].verified);
        assert_eq!(domains[0].pending, 2);
        assert_eq!(domain_mode_label(&domains[0].mode), "Invited automatically");
        assert_eq!(domain_mode_label("enterprise_sso"), "Through SSO");
    }

    #[test]
    fn splits_and_checks_emails() {
        assert_eq!(
            parse_emails("ada@acme.com, bo@acme.com;cy@acme.io").unwrap(),
            ["ada@acme.com", "bo@acme.com", "cy@acme.io"]
        );
        assert!(parse_emails("  ").unwrap().is_empty());
        assert!(parse_emails("ada@acme").is_err());
        assert!(parse_emails("ada, bo@acme.com").is_err());
    }

    #[test]
    fn says_what_invitations_did() {
        let (text, all_failed) = invite_outcome(&parse(
            r#"{
            "dryRun": false, "role": "member", "toSend": 1,
            "results": [
                { "email": "ada@acme.com", "status": "sent", "invitationId": "inv_1" },
                { "email": "bo@acme.com", "status": "already_member" },
                { "email": "cy@acme.com", "status": "failed", "message": "Refused." }
            ],
            "counts": { "sent": 1, "already_member": 1, "failed": 1 }
        }"#,
        ));
        assert_eq!(
            text,
            "Invited ada@acme.com. bo@acme.com is already in the organization. cy@acme.com: Refused."
        );
        assert!(!all_failed);
        let (_, all_failed) = invite_outcome(&parse(r#"{ "results": [{ "email": "a@b.co", "status": "failed" }] }"#));
        assert!(all_failed);
    }

    #[test]
    fn says_refusals_plainly() {
        assert_eq!(
            refusal("only the org's owner or admins can change roles (members.manage)"),
            "Only organization admins can change this."
        );
        assert_eq!(
            refusal("only the org's owner can transfer ownership (org.transfer)"),
            "Only the organization's owner can do this."
        );
        assert_eq!(refusal("the owner cannot be removed"), "the owner cannot be removed.");
        assert_eq!(initials("Ada Lovelace"), "AL");
        assert_eq!(initials("bo@acme.com"), "BA");
        assert_eq!(encode("user_1/x"), "user_1%2Fx");
    }
}
