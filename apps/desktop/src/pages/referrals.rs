//! Referrals (`ui/src/app/app/[org]/referrals/page.tsx`): one card with the reward, the person's
//! link with Share on X and Copy, how it works as one strip, and the figures (people joined, rewards earned,
//! invitations); then the people who joined, and one line of fine print that never repeats the hero. While sign-up is invite-only, the link shows only to a person Nexus approved
//! who has invitations left, with a meter of those used; a person who used them all sees that in the danger tone.
//! Reads `GET /v1/referrals`, a page at a time. A Gateway without that route answers 404, and the page
//! says referrals are not available yet.
//!
//! The strip shows what the people who open the link see: Nexus UI's sign-up page names the inviter ("Ada Lovelace
//! invited you to Nexus") and confirms the new account. The name is the person's `user.name` from
//! `GET /v1/tenants/me`, the one Gateway gives the sign-up page.
//!
//! The reward's amount comes from the Gateway (`rewardUsdDecimal`), and the words are the kit's (`kit/src/referrals.ts`).
//! While Nexus pays no reward (`rewardsActive` not true), nothing on the page promises money, and at the
//! lifetime limit the page says so. While the program is off (`enabled` false), the page says referrals are
//! paused and shows no link, as Nexus UI does: a link then records nobody.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

use super::{Ctx, Load, Verb, fetch, palette, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

/// The condition, in the kit's words (`REFERRAL_CONDITION`): the Gateway rewards only a purchase of at least $10 in the
/// referred person's personal organization.
const CONDITION: &str = "buys at least $10 of Nexus credits in their personal account";

/// What the page says while Nexus pays no reward (`NO_REWARD` in the kit).
const NO_REWARD: &str = "Nexus does not pay referral rewards now.";

/// The reward's other condition (`REFERRAL_EMAIL_CONDITION` in the kit): a reward needs the person's email.
const EMAIL_CONDITION: &str = "That person needs a confirmed email address on their Nexus account.";

/// A live person Nexus has no email for yet (`EMAIL_PENDING` in the kit).
const EMAIL_PENDING: &str = "Email pending";

/// What a referral earns: the glossary's `promotionalCredit`, in lower case. Never "Nexus credit".
const REWARD_NAME: &str = "promotional credit";

/// The offer (`referralTerms` in the kit), split so each place says one thing: the hero's title names the offer, the
/// flow strip says how it works in plain steps, and the foot gives the exact condition once. The hero has no body: a
/// sentence there would explain the offer a second time, above the steps.
#[derive(Debug, Clone, PartialEq)]
struct Terms {
    title: &'static str,
    /// The flow's last stop: its words, the part picked out, and the words after it (each may be empty).
    step: (String, String, String),
    /// The exact condition, in the kit's words: the foot's fine print.
    condition: String,
}

/// The last line of every foot.
const PROGRAM_CHANGES: &str = "Nexus can change or end the program at any time.";

/// The foot of the page: the exact condition, the one place the page states it, then that the program can change.
fn fine_print(offer: &Terms) -> String {
    format!("{} {PROGRAM_CHANGES}", offer.condition)
}

/// The offer for this person (`referralTermsFor` in the kit): at the lifetime limit (`progress`, granted and limit), the
/// limit's words, which promise no money.
fn terms(reward: Option<&str>, progress: Option<(u64, u64)>) -> Terms {
    if let Some((granted, limit)) = progress
        && granted >= limit
    {
        return Terms {
            title: "Lifetime reward limit reached",
            step: ("No more rewards".into(), String::new(), String::new()),
            condition: format!(
                "Each account earns rewards for up to {limit} referrals. Yours reached that limit, so further \
                 referrals do not earn credit."
            ),
        };
    }
    match reward {
        None => Terms {
            title: "Invite people to Nexus",
            step: ("No reward now".into(), String::new(), String::new()),
            condition: format!("{NO_REWARD} This page shows the reward when Nexus pays one."),
        },
        Some(reward) => Terms {
            title: "Invite people, earn promotional credit",
            step: (
                "You get".into(),
                reward.into(),
                // Short, so a whole name fits on the strip; the foot gives the exact condition.
                "after they buy $10".into(),
            ),
            condition: format!(
                "You get {reward} of {REWARD_NAME} when a person who joined through your link {CONDITION}. \
                 {EMAIL_CONDITION}"
            ),
        },
    }
}

/// The reward for one referral as text (`$1`, `$2.50`), from the Gateway's six-decimal string; None while Nexus pays
/// none (`referralReward` in the kit).
fn reward_text(value: &Value) -> Option<String> {
    if value.get("enabled").and_then(Value::as_bool) == Some(false)
        || value.get("rewardsActive").and_then(Value::as_bool) != Some(true)
    {
        return None;
    }
    let decimal = value.get("rewardUsdDecimal")?.as_str()?.trim();
    let (whole, fraction) = decimal.split_once('.').unwrap_or((decimal, ""));
    if whole.is_empty() || fraction.len() > 6 || !(whole.bytes().chain(fraction.bytes()).all(|b| b.is_ascii_digit())) {
        return None;
    }
    let micros = whole.parse::<i64>().ok()?.checked_mul(1_000_000)? + format!("{fraction:0<6}").parse::<i64>().ok()?;
    if micros <= 0 {
        return None;
    }
    // Whole dollars without cents, else cents (`budgetText` in the kit).
    let cents = (micros + 5_000) / 10_000;
    Some(if cents % 100 == 0 {
        format!("${}", cents / 100)
    } else {
        format!("${}.{:02}", cents / 100, cents % 100)
    })
}

/// A referral's reward (`ReferralReward` in the kit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reward {
    /// None yet, and one may come.
    Pending,
    Rewarded,
    /// Final: it never will.
    NotRewarded,
    /// The Gateway does not say.
    Unknown,
}

/// One person's reward: the Gateway's `reward`, else, on an older Gateway, `rewarded` and the lifetime limit.
fn reward_of(person: &Value, limit_reached: bool) -> Reward {
    match person.get("reward").and_then(Value::as_str) {
        Some("rewarded") => Reward::Rewarded,
        Some("not_rewarded") => Reward::NotRewarded,
        Some("pending") => Reward::Pending,
        _ => match person.get("rewarded").and_then(Value::as_bool) {
            Some(true) => Reward::Rewarded,
            Some(false) if limit_reached => Reward::NotRewarded,
            Some(false) => Reward::Pending,
            None => Reward::Unknown,
        },
    }
}

/// The reward column (`referralRewardText` in the kit). "Not yet rewarded" only while Nexus pays rewards.
fn reward_line(reward: Reward, paying: bool) -> &'static str {
    match reward {
        Reward::Rewarded => "Rewarded",
        Reward::NotRewarded => "Not rewarded",
        Reward::Pending if paying => "Not yet rewarded",
        Reward::Pending => "No reward now",
        Reward::Unknown => "Reward status unavailable",
    }
}

pub fn build(ctx: Ctx, _window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| {
        let mut page = ReferralsPage {
            ctx,
            first: Load::Loading,
            more: Vec::new(),
            cursor: None,
            loading_more: false,
            more_error: None,
            copied: false,
            inviter: None,
        };
        page.load(cx);
        page.load_inviter(cx);
        page
    });
    Some(view.into())
}

/// One person who joined through the link.
#[derive(Debug, Clone, PartialEq)]
struct Person {
    /// ISO time of the sign-up.
    joined_at: String,
    /// Masked (`b***@example.com`); None once the person is erased, or while Nexus has no email for them.
    email: Option<String>,
    /// The Gateway's `erased`; on an older Gateway, a missing email.
    erased: bool,
    reward: Reward,
}

/// The Person column (`referredEmail` in Nexus UI): the masked email, Erased, or Email pending for a live person.
fn person_name(person: &Person) -> &str {
    match (&person.email, person.erased) {
        (Some(email), _) => email,
        (None, true) => "Erased",
        (None, false) => EMAIL_PENDING,
    }
}

/// `GET /v1/referrals`: the code and a page of people.
#[derive(Debug, Clone, PartialEq)]
struct ReferralList {
    code: String,
    /// False while the program is off: the link records and admits nobody. True on an older Gateway.
    enabled: bool,
    /// Lifetime rewards, granted and the limit; None on an older Gateway.
    progress: Option<(u64, u64)>,
    people: Vec<Person>,
    next_cursor: Option<String>,
    /// While sign-up is invite-only: how many more people the code admits. None while sign-up is open.
    invites_left: Option<u64>,
    /// While sign-up is invite-only: how many people Nexus approved the code to admit in all. None on older Gateways.
    invites_allowed: Option<u64>,
    /// The reward for one referral (`$1`); None while Nexus pays none.
    reward: Option<String>,
}

/// What the link can do (`referralInvitations` in the kit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invitations {
    /// Sign-up is open: the link admits anyone.
    Open,
    /// Invite-only, and the link admits `left` more, of `allowed` in all (None from an older Gateway).
    Approved { left: u64, allowed: Option<u64> },
    /// Invite-only, and the person used all of the invitations Nexus approved (this many): the link admits nobody.
    UsedUp(u64),
    /// Invite-only, and Nexus approved none (or an older Gateway cannot tell): the link admits nobody.
    NotApproved,
}

impl Invitations {
    fn closed(self) -> bool {
        matches!(self, Invitations::UsedUp(_) | Invitations::NotApproved)
    }
}

fn invitations(left: Option<u64>, allowed: Option<u64>) -> Invitations {
    let allowed = allowed.filter(|&allowed| allowed > 0);
    match (left, allowed) {
        (None, _) => Invitations::Open,
        (Some(0), Some(allowed)) => Invitations::UsedUp(allowed),
        (Some(0), None) => Invitations::NotApproved,
        (Some(left), allowed) => Invitations::Approved {
            left,
            allowed: allowed.map(|allowed| allowed.max(left)),
        },
    }
}

fn invitation_word(n: u64) -> &'static str {
    if n == 1 { "invitation" } else { "invitations" }
}

/// The notice in place of the link while it admits nobody (`invitationsNotice` in the kit).
fn invitations_notice(invites: Invitations) -> Option<(&'static str, String)> {
    match invites {
        Invitations::UsedUp(allowed) => Some((
            "You used all your invitations",
            format!(
                "All {allowed} {} Nexus approved for you are used. Your link admits nobody new until Nexus approves \
                 more.",
                invitation_word(allowed)
            ),
        )),
        Invitations::NotApproved => Some((
            "Invitations come by approval",
            "Nexus is invite-only for now. Your link admits nobody until Nexus approves invitations for you.".into(),
        )),
        _ => None,
    }
}

/// One figure under the hero's link: a label, a large number, the words after it and, for invitations, a meter.
#[derive(Debug, Clone, PartialEq)]
struct Figure {
    label: &'static str,
    value: String,
    unit: Option<String>,
    /// The share of the invitations used, for the meter.
    meter: Option<f32>,
    /// In the danger tone: every invitation is used.
    danger: bool,
}

/// The invitations as a figure (`invitationsCount` in the kit): those used of those approved and what is left, or only
/// what is left when an older Gateway does not say how many Nexus approved.
fn invitations_figure(invites: Invitations) -> Option<Figure> {
    match invites {
        Invitations::UsedUp(allowed) => Some(Figure {
            label: "Invitations used",
            value: allowed.to_string(),
            unit: Some(format!("of {allowed} · none left")),
            meter: Some(1.),
            danger: true,
        }),
        Invitations::Approved { left, allowed: None } => Some(Figure {
            label: "Invitations left",
            value: left.to_string(),
            unit: None,
            meter: None,
            danger: false,
        }),
        Invitations::Approved {
            left,
            allowed: Some(allowed),
        } => {
            let used = allowed - left;
            Some(Figure {
                label: "Invitations used",
                value: used.to_string(),
                unit: Some(format!("of {allowed} · {left} left")),
                meter: Some(used as f32 / allowed as f32),
                danger: false,
            })
        }
        _ => None,
    }
}

/// The hero's figures: the people who joined (`+` while more pages wait), the rewards against the lifetime limit, and
/// the invitations. The rewards show only while Nexus pays one or has paid one, so a quiet program promises no money.
fn figures(list: &ReferralList, joined: usize, more_left: bool) -> Vec<Figure> {
    let mut figures = vec![Figure {
        label: "People joined",
        value: format!("{joined}{}", if more_left { "+" } else { "" }),
        unit: None,
        meter: None,
        danger: false,
    }];
    if let Some((granted, limit)) = list.progress
        && (list.reward.is_some() || granted > 0)
    {
        figures.push(Figure {
            label: "Rewards earned",
            value: granted.min(limit).to_string(),
            unit: Some(format!("of {limit}")),
            meter: None,
            danger: false,
        });
    }
    figures.extend(invitations_figure(invitations(list.invites_left, list.invites_allowed)));
    figures
}

/// The person's name as Gateway has it (`user.name` of `GET /v1/tenants/me`), trimmed; None when they have none. It is
/// the name Nexus UI's sign-up page shows to the people who open the link (`GET /v1/ui/referrals/{code}`).
/// Not `Account::name`, which falls back to the email.
fn inviter_name(me: &Value) -> Option<String> {
    me.get("user")?
        .get("name")?
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(String::from)
}

/// The flow's middle stop: what the sign-up page's heading says to the people who open the link ("Ada Lovelace invited
/// you to Nexus"), cut to fit the strip. Its words, and the part picked out. `inviter` is None until Gateway
/// says who the person is, and `Some(None)` when they have no name: the page then names nobody.
fn join_stop(inviter: Option<Option<&str>>) -> (String, String) {
    match inviter {
        Some(Some(name)) => ("They see".into(), format!("{name} invited you")),
        Some(None) => ("They see".into(), "You’re invited to Nexus".into()),
        None => ("They".into(), "sign up".into()),
    }
}

/// Whether the page hides the link: the program is off, or the link admits nobody under invite-only.
fn link_closed(list: &ReferralList) -> bool {
    !list.enabled || invitations(list.invites_left, list.invites_allowed).closed()
}

fn read_list(value: &Value) -> Option<ReferralList> {
    let code = value.get("code")?.as_str()?.to_string();
    let progress = value.get("rewardProgress");
    let count = |key: &str| progress.and_then(|progress| progress.get(key)).and_then(Value::as_u64);
    let progress = count("granted").zip(count("limit"));
    let limit_reached = progress.is_some_and(|(granted, limit)| granted >= limit);
    let people = value
        .get("referred")
        .and_then(Value::as_array)
        .map(|people| {
            people
                .iter()
                .filter_map(|person| {
                    let email = person.get("email").and_then(Value::as_str).map(String::from);
                    Some(Person {
                        joined_at: person.get("joinedAt")?.as_str()?.to_string(),
                        erased: person.get("erased").and_then(Value::as_bool).unwrap_or(email.is_none()),
                        email,
                        reward: reward_of(person, limit_reached),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ReferralList {
        code,
        enabled: value.get("enabled").and_then(Value::as_bool) != Some(false),
        progress,
        people,
        next_cursor: value
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())
            .map(String::from),
        invites_left: value.get("invitesLeft").and_then(Value::as_u64),
        invites_allowed: value.get("invitesAllowed").and_then(Value::as_u64),
        reward: reward_text(value),
    })
}

/// The link to share: `/r/<code>` on Nexus UI's site.
fn referral_link(site: &str, code: &str) -> String {
    format!("{}/r/{}", site.trim_end_matches('/'), encode(code))
}

fn share_on_x(link: &str) -> String {
    format!(
        "https://x.com/intent/post?text={}",
        encode(&format!("Join me on Nexus: {link}"))
    )
}

/// Percent-encoding for a URL's query value or path segment.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Gateway's 404 for a route it does not have.
fn is_missing_route(error: &str) -> bool {
    error.contains("no such endpoint")
}

struct ReferralsPage {
    ctx: Ctx,
    /// The first page; `Ready(None)` when this Gateway has no referrals.
    first: Load<Option<ReferralList>>,
    /// People from later pages.
    more: Vec<Person>,
    cursor: Option<String>,
    loading_more: bool,
    more_error: Option<SharedString>,
    copied: bool,
    /// The name the sign-up page shows for this person's link: None until Gateway answers (or when it fails), and
    /// `Some(None)` when the person has no name.
    inviter: Option<Option<String>>,
}

impl ReferralsPage {
    /// Who the person is, for the step that shows what their link's sign-up page says. A failure leaves the plain step.
    fn load_inviter(&mut self, cx: &mut Context<Self>) {
        let request = self.ctx.gateway.query("/v1/tenants/me".into());
        super::read(cx, request, |this, answer, _| {
            if let Ok(me) = answer {
                this.inviter = Some(inviter_name(&me));
            }
        });
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.first = Load::Loading;
        self.more.clear();
        self.more_error = None;
        let request = self.ctx.gateway.query("/v1/referrals".into());
        super::read(cx, request, |this, answer, _| {
            if this.loading_more || !this.more.is_empty() {
                return;
            }
            this.first = match answer {
                Ok(value) => match read_list(&value) {
                    Some(list) => {
                        this.cursor = list.next_cursor.clone();
                        Load::Ready(Some(list))
                    }
                    None => Load::Failed("Gateway's answer had no referral code.".into()),
                },
                Err(error) if is_missing_route(&error) => Load::Ready(None),
                Err(error) => Load::Failed(error.into()),
            };
        });
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(cursor) = self.cursor.clone() else {
            return;
        };
        if self.loading_more {
            return;
        }
        self.loading_more = true;
        self.more_error = None;
        let path = format!("/v1/referrals?cursor={}", encode(&cursor));
        let request = self.ctx.gateway.request(Verb::Get, path, None);
        fetch(cx, request, |this, answer, _| {
            this.loading_more = false;
            match answer.map(|value| read_list(&value)) {
                Ok(Some(page)) => {
                    this.more.extend(page.people);
                    this.cursor = page.next_cursor;
                }
                Ok(None) => this.more_error = Some("The next people didn't load.".into()),
                Err(error) => this.more_error = Some(format!("The next people didn't load: {error}").into()),
            }
        });
        cx.notify();
    }
}

/// The hero's head: an icon on a rounded square, the title and, for a notice, the line under it. A title alone sits
/// centered on the icon.
fn offer_head(icon: Lucide, tone: Hsla, title: impl Into<SharedString>, body: Option<String>, palette: Palette) -> Div {
    h_flex()
        .when(body.is_some(), |head| head.items_start())
        .when(body.is_none(), |head| head.items_center())
        .gap(px(12.))
        .child(
            h_flex()
                .flex_none()
                .justify_center()
                .size(px(32.))
                .rounded(px(8.))
                .bg(soft_fill(palette))
                .child(ui::icon(icon, tone).size(px(16.))),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(16.))
                        .line_height(px(22.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.strong)
                        .child(title.into()),
                )
                .when_some(body, |head, body| {
                    head.child(
                        div()
                            .max_w(px(480.))
                            .text_size(px(13.))
                            .line_height(px(19.))
                            .text_color(palette.faint)
                            .child(body),
                    )
                }),
        )
}

/// The fill behind a small mark on a raised card: `selected` in dark, the soft grey in light (`ui::icon_tile`'s).
fn soft_fill(palette: Palette) -> Hsla {
    if palette.dark { palette.selected } else { palette.hover }
}

/// The figures under the hero's link, split by hairlines.
fn figure_row(figures: Vec<Figure>, palette: Palette) -> Div {
    h_flex()
        .items_stretch()
        .border_t_1()
        .border_color(palette.border)
        .children(figures.into_iter().enumerate().map(|(ix, figure)| {
            let tone = if figure.danger {
                palette.destructive
            } else {
                palette.strong
            };
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .gap(px(4.))
                .px(px(20.))
                .pt(px(14.))
                .pb(px(16.))
                .when(ix > 0, |cell| cell.border_l_1().border_color(palette.border))
                .child(ui::label(figure.label, palette))
                .child(
                    h_flex()
                        .items_baseline()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(20.))
                                .line_height(px(24.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(tone)
                                .child(figure.value),
                        )
                        .when_some(figure.unit, |row, unit| {
                            row.child(div().text_size(px(13.)).text_color(palette.faint).child(unit))
                        }),
                )
                .when_some(figure.meter, |cell, share| {
                    cell.child(
                        div()
                            .mt(px(6.))
                            .w_full()
                            .h(px(4.))
                            .rounded(px(2.))
                            .overflow_hidden()
                            .bg(if figure.danger {
                                palette.destructive.opacity(0.2)
                            } else {
                                ui::inset(palette)
                            })
                            .child(div().h_full().w(relative(share.clamp(0., 1.))).rounded(px(2.)).bg(tone)),
                    )
                })
        }))
}

/// One stop's words: each piece, and whether it is the part picked out. An empty piece draws nothing.
type Words = Vec<(String, bool)>;

/// How it works, folded into the hero as one quiet strip at its foot: "Share your link › They see Ada Lovelace invited
/// you › You get $1". Each stop has an icon, its picked-out part in the text color and the rest faint. A long name
/// cuts with an ellipsis; nothing wraps.
fn flow_strip(stops: [(Lucide, Words); 3], palette: Palette) -> Div {
    let mut strip = h_flex()
        .gap(px(10.))
        .px(px(20.))
        .py(px(12.))
        .border_t_1()
        .border_color(palette.border)
        .text_size(px(12.5));
    for (ix, (icon, words)) in stops.into_iter().enumerate() {
        if ix > 0 {
            strip = strip.child(
                div()
                    .flex_none()
                    .child(ui::icon(Lucide::ChevronRight, ui::thumb(palette))),
            );
        }
        strip = strip.child(
            h_flex()
                .min_w(px(0.))
                .gap(px(8.))
                .child(div().flex_none().child(ui::icon(icon, palette.icon)))
                .child(
                    h_flex().min_w(px(0.)).gap(px(4.)).children(
                        words
                            .into_iter()
                            .filter(|(text, _)| !text.is_empty())
                            .map(|(text, picked)| {
                                let word = div().child(text);
                                if picked {
                                    word.min_w(px(0.))
                                        .truncate()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(palette.foreground)
                                } else {
                                    word.flex_none().whitespace_nowrap().text_color(palette.faint)
                                }
                            }),
                    ),
                ),
        );
    }
    strip
}

/// A referral's reward as a state after a dot: mint when rewarded, a ring while one may come, grey otherwise.
fn reward_status(reward: Reward, paying: bool, palette: Palette) -> Div {
    let dot = div().flex_none().size(px(7.)).rounded(px(4.));
    let dot = match reward {
        Reward::Rewarded => dot.bg(Palette::success()),
        Reward::Pending if paying => dot.border_1().border_color(palette.steel()),
        _ => dot.bg(palette.steel().opacity(0.55)),
    };
    h_flex()
        .flex_none()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(palette.muted)
        .child(dot)
        .child(reward_line(reward, paying))
}

/// A person's round mark: the first letter of their masked email, else a person icon.
fn person_avatar(person: &Person, palette: Palette) -> Div {
    let letter = person
        .email
        .as_deref()
        .and_then(|email| email.chars().next())
        .filter(|letter| letter.is_alphanumeric());
    h_flex()
        .flex_none()
        .justify_center()
        .size(px(28.))
        .rounded(px(14.))
        .bg(soft_fill(palette))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.muted)
        .map(|avatar| match letter {
            Some(letter) => avatar.child(letter.to_uppercase().to_string()),
            None => avatar.child(ui::icon(Lucide::UserRound, palette.muted)),
        })
}

/// Where the rows' text starts in the people card: its inset, the avatar and the gap after it. The rule between rows
/// starts here.
const PERSON_TEXT: f32 = 16. + 28. + 16.;

fn person_row(person: &Person, paying: bool, palette: Palette) -> Div {
    h_flex()
        .min_h(px(52.))
        .gap(px(16.))
        .px(px(16.))
        .text_size(px(13.))
        .child(person_avatar(person, palette))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(if person.email.is_some() {
                    palette.strong
                } else {
                    palette.faint
                })
                .child(person_name(person).to_string()),
        )
        .child(
            div()
                .flex_none()
                .w(px(96.))
                .text_right()
                .text_color(palette.faint)
                .child(ui::day(&person.joined_at).to_string()),
        )
        .child(
            h_flex()
                .flex_none()
                .w(px(150.))
                .justify_end()
                .child(reward_status(person.reward, paying, palette)),
        )
}

fn share_button(palette: Palette, enabled: bool) -> Stateful<Div> {
    if enabled {
        ui::button("referrals-share-x", "Share on X", palette)
    } else {
        ui::disabled_button("referrals-share-x", "Share on X", palette)
    }
}

fn copy_button(copied: bool, palette: Palette, enabled: bool) -> Stateful<Div> {
    let label = if copied { "Copied" } else { "Copy link" };
    if enabled {
        ui::primary_button("referrals-copy", label, palette)
    } else {
        ui::disabled_button("referrals-copy", label, palette)
    }
}

pub(super) fn build_locked(cx: &mut App) -> AnyView {
    ui::locked_page(cx, |palette| {
        h_flex()
            .gap(px(8.))
            .child(share_button(palette, false))
            .child(copy_button(false, palette, false))
            .into_any_element()
    })
}

impl Render for ReferralsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let column = v_flex().w_full().min_w(px(0.));
        let list = match &self.first {
            Load::Loading => return column.child(ui::skeleton(3, palette)),
            Load::Failed(error) => {
                return column.child(ui::failed(error.clone(), palette)).child(
                    h_flex().justify_center().child(
                        ui::button("referrals-retry", "Try again", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                    ),
                );
            }
            Load::Ready(None) => {
                return column.child(ui::empty(
                    "Referrals are not available yet",
                    "This Gateway does not have referrals. Check back after it updates.",
                    palette,
                ));
            }
            Load::Ready(Some(list)) => list,
        };
        let invites = invitations(list.invites_left, list.invites_allowed);
        let paused = !list.enabled;
        let closed = link_closed(list);
        let used_up = matches!(invites, Invitations::UsedUp(_));
        let notice = invitations_notice(invites);
        let offer = terms(list.reward.as_deref(), list.progress);
        let paying = list.reward.is_some();
        let people: Vec<&Person> = list.people.iter().chain(self.more.iter()).collect();
        let more_left = self.cursor.is_some();

        let hero = if paused {
            ui::card_frame(palette).child(v_flex().p(px(20.)).child(offer_head(
                Lucide::CirclePause,
                palette.muted,
                "Referrals are paused",
                Some("The program is not accepting new referrals. Your previous referrals are shown below.".into()),
                palette,
            )))
        } else {
            let head = match &notice {
                Some((title, body)) => offer_head(
                    Lucide::TicketX,
                    if used_up { palette.destructive } else { palette.muted },
                    *title,
                    Some(body.clone()),
                    palette,
                ),
                // The strip under the link says how the offer works, so the title stands alone.
                None => offer_head(Lucide::Gift, palette.muted, offer.title, None, palette),
            };
            let mut top = v_flex().p(px(20.)).gap(px(16.)).child(head);
            if !closed {
                let link = referral_link(&self.ctx.gateway.site_url(), &list.code);
                let (host, code) = link.split_at(link.len() - encode(&list.code).len());
                let copy = link.clone();
                let share = share_on_x(&link);
                top = top.child(
                    h_flex()
                        .gap(px(8.))
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w(px(0.))
                                .h(px(ui::CONTROL_HEIGHT))
                                .gap(px(8.))
                                .px(px(10.))
                                .rounded(px(8.))
                                .bg(ui::inset(palette))
                                .font_family(ui::MONO)
                                .text_size(px(12.))
                                .child(ui::icon(Lucide::Link, palette.icon))
                                .child(
                                    div()
                                        .min_w(px(0.))
                                        .truncate()
                                        .text_color(palette.faint)
                                        .child(host.to_string()),
                                )
                                .child(div().flex_none().text_color(palette.foreground).child(code.to_string())),
                        )
                        .child(
                            share_button(palette, true)
                                .on_click(move |_, _, cx| crate::external_link::open(&share, cx)),
                        )
                        .child(
                            copy_button(self.copied, palette, true).on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                this.copied = true;
                                cx.notify();
                            })),
                        ),
                );
            }
            // How it works, only while the link admits someone: a closed link has nothing to share.
            let (seen, name) = join_stop(self.inviter.as_ref().map(Option::as_deref));
            let (reward_lead, reward, reward_when) = offer.step.clone();
            let flow = [
                (
                    Lucide::Send,
                    vec![("Share".to_string(), true), ("your link".to_string(), false)],
                ),
                (Lucide::UserCheck, vec![(seen, false), (name, true)]),
                (
                    Lucide::CircleDollarSign,
                    vec![(reward_lead, false), (reward, true), (reward_when, false)],
                ),
            ];
            // People joined alone says no more than the list's own count, so it needs no row.
            let figures = figures(list, people.len(), more_left);
            ui::card_frame(palette)
                .child(top)
                .when(!closed, |card| card.child(flow_strip(flow, palette)))
                .when(figures.len() > 1, |card| card.child(figure_row(figures, palette)))
        };

        let mut card = ui::card_frame(palette);
        if people.is_empty() {
            card = card.child(
                v_flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(16.))
                    .py(px(28.))
                    .child(ui::icon_tile(Lucide::Users, palette).mb(px(4.)))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.strong)
                            .child("Nobody has joined through your link yet"),
                    )
                    .when(!closed, |empty| {
                        empty.child(ui::setting_detail(
                            "Share your link. Each person who signs up through it shows here.",
                            palette,
                        ))
                    }),
            );
        } else {
            card = card.child(
                h_flex()
                    .h(px(30.))
                    .gap(px(16.))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(palette.border)
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.faint)
                    .child(div().flex_none().w(px(28.)))
                    .child(div().flex_1().child("Person"))
                    .child(div().flex_none().w(px(96.)).text_right().child("Joined"))
                    .child(div().flex_none().w(px(150.)).text_right().child("Reward")),
            );
            for (ix, person) in people.iter().enumerate() {
                if ix > 0 {
                    card = card.child(div().h(px(1.)).ml(px(PERSON_TEXT)).bg(palette.border));
                }
                card = card.child(person_row(person, paying, palette));
            }
            if more_left {
                card = card.child(
                    h_flex()
                        .justify_center()
                        .p(px(8.))
                        .border_t_1()
                        .border_color(palette.border)
                        .child(
                            ui::ghost_button(
                                "referrals-more",
                                if self.loading_more { "Loading…" } else { "Load more" },
                                palette,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.load_more(cx))),
                        ),
                );
            }
        }
        let mut joined = v_flex()
            .child(
                ui::group_head("People who joined", palette).when(!people.is_empty(), |head| {
                    head.child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(format!("{}{}", people.len(), if more_left { "+" } else { "" })),
                    )
                }),
            )
            .child(card);
        if let Some(error) = &self.more_error {
            joined = joined.child(ui::error_line(error.clone(), palette).mt(px(8.)));
        }

        let foot = ui::setting_detail(fine_print(&offer), palette)
            .mt(px(16.))
            .max_w(px(560.));

        column.child(hero).child(joined).child(foot)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONDITION, EMAIL_CONDITION, Figure, Invitations, NO_REWARD, REWARD_NAME, Reward, Terms, figures, fine_print,
        invitations, invitations_figure, invitations_notice, inviter_name, is_missing_route, join_stop, link_closed,
        person_name, read_list, referral_link, reward_line, share_on_x, terms,
    };
    use serde_json::json;

    #[test]
    fn reads_a_page_of_referrals() {
        let list = read_list(&json!({
            "code": "ab12cd",
            "referred": [
                {"joinedAt": "2026-10-30T12:00:00Z", "email": "b***@example.com"},
                {"joinedAt": "2026-10-29T09:30:00Z", "email": null},
                {"email": "no-date@example.com"}
            ],
            "nextCursor": "c_2",
            "invitesLeft": 3
        }))
        .expect("a list");
        assert_eq!(list.code, "ab12cd");
        assert_eq!(list.people.len(), 2, "a person with no join time is dropped");
        assert_eq!(list.people[1].email, None);
        assert_eq!(list.next_cursor.as_deref(), Some("c_2"));
        assert_eq!(
            invitations(list.invites_left, list.invites_allowed),
            Invitations::Approved { left: 3, allowed: None }
        );
    }

    #[test]
    fn open_closed_and_missing() {
        let open = read_list(&json!({"code": "x", "referred": []})).expect("a list");
        assert_eq!(invitations(open.invites_left, open.invites_allowed), Invitations::Open);
        assert_eq!(open.next_cursor, None);
        assert_eq!(invitations(Some(0), None), Invitations::NotApproved);
        assert_eq!(invitations(Some(0), Some(0)), Invitations::NotApproved);
        assert_eq!(read_list(&json!({"referred": []})), None);
        assert!(is_missing_route("no such endpoint"));
    }

    #[test]
    fn a_person_who_used_every_invitation_is_told_so_as_the_kit_says() {
        let list =
            read_list(&json!({"code": "x", "referred": [], "invitesLeft": 0, "invitesAllowed": 6})).expect("a list");
        let used_up = invitations(list.invites_left, list.invites_allowed);
        assert_eq!(used_up, Invitations::UsedUp(6));
        assert!(used_up.closed());
        let (title, body) = invitations_notice(used_up).expect("a notice");
        assert_eq!(title, "You used all your invitations");
        assert!(
            body.starts_with("All 6 invitations Nexus approved for you are used."),
            "{body}"
        );
        let figure = invitations_figure(used_up).expect("a figure");
        assert_eq!(
            (figure.label, figure.value.as_str(), figure.unit.as_deref()),
            ("Invitations used", "6", Some("of 6 · none left"))
        );
        assert_eq!(figure.meter, Some(1.));
        assert!(figure.danger, "in the danger tone");
        let some = invitations(Some(3), Some(10));
        assert!(!some.closed());
        assert_eq!(invitations_notice(some), None);
        let figure = invitations_figure(some).expect("a figure");
        assert_eq!(
            (
                figure.value.as_str(),
                figure.unit.as_deref(),
                figure.meter,
                figure.danger
            ),
            ("7", Some("of 10 · 3 left"), Some(0.7), false)
        );
        let older = invitations_figure(invitations(Some(3), None)).expect("a figure");
        assert_eq!(
            (older.label, older.value.as_str(), older.unit, older.meter),
            ("Invitations left", "3", None, None),
            "an older Gateway does not say how many Nexus approved"
        );
        assert_eq!(invitations_figure(Invitations::Open), None);
    }

    #[test]
    fn the_figures_count_people_and_promise_no_money_without_a_reward() {
        let list = |extra: serde_json::Value| {
            let mut value = json!({"code": "x", "referred": []});
            value
                .as_object_mut()
                .expect("object")
                .extend(extra.as_object().expect("object").clone());
            read_list(&value).expect("a list")
        };
        let labels = |figures: Vec<Figure>| figures.into_iter().map(|figure| figure.label).collect::<Vec<_>>();
        let paying = list(json!({
            "rewardsActive": true, "rewardUsdDecimal": "1.000000",
            "rewardProgress": {"granted": 2, "limit": 10}, "invitesLeft": 3, "invitesAllowed": 10
        }));
        let all = figures(&paying, 5, true);
        assert_eq!(all[0].value, "5+", "more pages wait");
        assert_eq!((all[1].value.as_str(), all[1].unit.as_deref()), ("2", Some("of 10")));
        assert_eq!(labels(all), ["People joined", "Rewards earned", "Invitations used"]);
        let quiet = list(json!({"rewardProgress": {"granted": 0, "limit": 10}}));
        assert_eq!(labels(figures(&quiet, 0, false)), ["People joined"]);
        let paid_before = list(json!({"rewardProgress": {"granted": 3, "limit": 10}}));
        assert_eq!(
            labels(figures(&paid_before, 4, false)),
            ["People joined", "Rewards earned"],
            "rewards already earned still show"
        );
    }

    #[test]
    fn the_second_step_says_what_the_sign_up_page_says() {
        let me = |name: serde_json::Value| json!({"tenant": {"id": "ten_1"}, "user": {"id": "usr_1", "name": name}});
        assert_eq!(
            inviter_name(&me(json!("  Ada Lovelace "))).as_deref(),
            Some("Ada Lovelace")
        );
        assert_eq!(inviter_name(&me(json!("   "))), None);
        assert_eq!(inviter_name(&me(json!(null))), None);
        assert_eq!(
            inviter_name(&json!({"tenant": {"id": "ten_1"}})),
            None,
            "an organization key"
        );
        // The sign-up page's headings (`app/sign-up/page.tsx`): "Ada Lovelace invited you to Nexus", else "You’re invited
        // to Nexus" for a person with no name.
        let stop = |inviter| {
            let (lead, picked) = join_stop(inviter);
            format!("{lead} {picked}")
        };
        assert_eq!(stop(Some(Some("Ada Lovelace"))), "They see Ada Lovelace invited you");
        assert_eq!(stop(Some(None)), "They see You’re invited to Nexus");
        assert_eq!(stop(None), "They sign up", "before Gateway says who the person is");
    }

    /// The flow's last stop names the reward only while Nexus pays one, in plain words; the foot keeps the exact
    /// condition.
    #[test]
    fn the_last_stop_promises_no_money_without_a_reward() {
        let words = |terms: Terms| {
            let (lead, picked, tail) = terms.step;
            [lead, picked, tail].join(" ").trim().to_string()
        };
        assert_eq!(words(terms(Some("$1"), None)), "You get $1 after they buy $10");
        assert_eq!(words(terms(None, None)), "No reward now");
        assert_eq!(words(terms(Some("$1"), Some((10, 10)))), "No more rewards");
    }

    /// The page states the exact condition once, in the foot: the hero has only a title, and the strip plain steps.
    #[test]
    fn the_foot_states_the_condition_once() {
        let paying = terms(Some("$1"), None);
        assert_eq!(
            fine_print(&paying),
            format!(
                "You get $1 of promotional credit when a person who joined through your link {CONDITION}. \
                 {EMAIL_CONDITION} Nexus can change or end the program at any time."
            )
        );
        assert!(fine_print(&terms(None, None)).starts_with(NO_REWARD));
        assert!(
            fine_print(&terms(Some("$1"), Some((10, 10)))).contains("do not earn credit"),
            "the limit"
        );
    }

    #[test]
    fn links_are_encoded() {
        assert_eq!(referral_link("https://nexus.inc/", "a b"), "https://nexus.inc/r/a%20b");
        assert!(share_on_x("https://nexus.inc/r/x").starts_with("https://x.com/intent/post?text=Join%20me"));
    }

    #[test]
    fn the_amount_comes_from_the_gateway_and_only_while_it_pays() {
        let list = |extra: serde_json::Value| {
            let mut value = json!({"code": "x", "referred": []});
            value
                .as_object_mut()
                .expect("object")
                .extend(extra.as_object().expect("object").clone());
            read_list(&value).expect("a list").reward
        };
        let paying = json!({"rewardsActive": true, "rewardUsdDecimal": "1.000000"});
        assert_eq!(list(paying.clone()).as_deref(), Some("$1"));
        assert_eq!(
            list(json!({"rewardsActive": true, "rewardUsdDecimal": "2.500000"})).as_deref(),
            Some("$2.50")
        );
        assert_eq!(
            list(json!({"rewardsActive": false, "rewardUsdDecimal": "1.000000"})),
            None
        );
        assert_eq!(
            list(json!({"rewardsActive": true, "rewardUsdDecimal": "0.000000"})),
            None
        );
        assert_eq!(
            list(json!({"rewardsActive": true, "rewardUsdDecimal": "1.000000", "enabled": false})),
            None
        );
        assert_eq!(list(json!({})), None, "an older Gateway sends neither field");
    }

    #[test]
    fn the_words_match_the_kit_and_promise_no_money_without_a_reward() {
        let paying = terms(Some("$1"), None);
        assert!(paying.condition.contains(CONDITION));
        assert!(paying.condition.ends_with(EMAIL_CONDITION), "{}", paying.condition);
        let quiet = terms(None, None);
        assert!(!quiet.condition.contains(EMAIL_CONDITION));
        let all = format!(
            "{} {} {} {} {}",
            quiet.title, quiet.step.0, quiet.step.1, quiet.step.2, quiet.condition
        );
        assert!(!all.contains('$') && !all.to_lowercase().contains("earn"), "{all}");
    }

    /// The reward's name is the glossary's, as `kit/src/fixtures/billing-words.json` holds it.
    #[test]
    fn the_reward_has_the_glossary_name() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../packages/kit/src/fixtures/billing-words.json"))
                .expect("fixture");
        let name = fixture["terms"]["promotionalCredit"]
            .as_str()
            .expect("promotionalCredit");
        assert_eq!(REWARD_NAME, name.to_lowercase());
        let paying = terms(Some("$1"), None);
        for text in [paying.title, paying.condition.as_str()] {
            assert!(text.contains(REWARD_NAME) && !text.contains("Nexus credit "), "{text}");
        }
    }

    #[test]
    fn the_limit_promises_no_money_as_the_kit_says() {
        assert_eq!(terms(Some("$1"), Some((9, 10))), terms(Some("$1"), None));
        for granted in [10, 11] {
            let capped = terms(Some("$1"), Some((granted, 10)));
            assert_eq!(capped.title, "Lifetime reward limit reached");
            assert!(
                capped.condition.contains("further referrals do not earn credit"),
                "{}",
                capped.condition
            );
            let all = format!(
                "{} {} {} {} {}",
                capped.title, capped.step.0, capped.step.1, capped.step.2, capped.condition
            );
            assert!(!all.contains('$'), "{all}");
        }
        let list = read_list(&json!({"code": "x", "referred": [], "rewardProgress": {"granted": 10, "limit": 10}}))
            .expect("a list");
        assert_eq!(list.progress, Some((10, 10)));
    }

    #[test]
    fn a_paused_program_shows_no_link_as_web_does() {
        let paused = read_list(&json!({"code": "x", "referred": [], "enabled": false})).expect("a list");
        assert!(!paused.enabled);
        assert!(link_closed(&paused), "a link that records nothing");
        let open = read_list(&json!({"code": "x", "referred": []})).expect("a list");
        assert!(open.enabled, "an older Gateway sends no switch");
        assert!(!link_closed(&open));
    }

    #[test]
    fn only_an_erased_person_reads_erased() {
        let page = read_list(&json!({
            "code": "x",
            "referred": [
                {"joinedAt": "2026-10-30T12:00:00Z", "email": null, "erased": true},
                {"joinedAt": "2026-10-30T12:00:00Z", "email": null, "erased": false},
                {"joinedAt": "2026-10-30T12:00:00Z", "email": "b***@example.com", "erased": false},
                {"joinedAt": "2026-10-30T12:00:00Z", "email": null}
            ]
        }))
        .expect("a list");
        let names: Vec<&str> = page.people.iter().map(person_name).collect();
        assert_eq!(names, ["Erased", "Email pending", "b***@example.com", "Erased"]);
    }

    #[test]
    fn each_referral_shows_its_reward_and_a_final_state() {
        let page = read_list(&json!({
            "code": "x",
            "rewardProgress": {"granted": 10, "limit": 10},
            "referred": [
                {"joinedAt": "2026-10-30T12:00:00Z", "email": null, "reward": "pending", "rewarded": false},
                {"joinedAt": "2026-10-30T12:00:00Z", "email": null, "reward": "not_rewarded", "rewarded": false},
                {"joinedAt": "2026-10-30T12:00:00Z", "email": null, "reward": "rewarded", "rewarded": true},
                {"joinedAt": "2026-10-30T12:00:00Z", "email": null, "rewarded": false},
                {"joinedAt": "2026-10-30T12:00:00Z", "email": null}
            ]
        }))
        .expect("a list");
        let rewards: Vec<Reward> = page.people.iter().map(|person| person.reward).collect();
        assert_eq!(
            rewards,
            [
                Reward::Pending,
                Reward::NotRewarded,
                Reward::Rewarded,
                Reward::NotRewarded,
                Reward::Unknown
            ]
        );
        assert_eq!(reward_line(Reward::Pending, true), "Not yet rewarded");
        assert_eq!(
            reward_line(Reward::Pending, false),
            "No reward now",
            "never waits for a reward no one pays"
        );
    }
}
