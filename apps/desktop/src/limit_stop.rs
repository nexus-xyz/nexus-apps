//! A budget or a limit stopped a call Nexus pays (board P5 of the setup lines): the organization's budget, a team's,
//! the person's own monthly limit, or a billing stop (the usage limit, credits, the free start, a payment). This module
//! has no UI: it reads the stop from a refusal's words and says the setup line's words and fixes. A limit takes the
//! amber dot, never red; only a charge that couldn't be collected takes the red ✕ (the design system's billing rule).

use gpui_kit::App;
use gpui_kit::assets::IconName as Lucide;
use serde_json::Value;

use crate::pages::PageId;
use crate::setup::{BillingBlock, Fix, FixKey, SetupLine, Tone};

/// Who may raise a budget or a limit, and who may change billing.
pub(crate) const BUDGETS: &str = "budgets.manage";
pub(crate) const BILLING: &str = "org.billing";

/// The rows of Usage › Limits a fix opens on (`SettingsFields` anchors).
pub(crate) const ORG_ROW: &str = "limit-org";
pub(crate) const TEAM_ROW: &str = "limit-team";
pub(crate) const PERSON_ROW: &str = "limit-person";
/// The card's monthly usage limit, which Billing pays (`billing_usage_limit.rs`).
pub(crate) const USAGE_ROW: &str = "limit-usage";

/// Completed when a budget or a limit was saved in Usage › Limits: what waited on it (a held send, a refused chat
/// answer) goes on.
pub(crate) const RAISED: &str = "limit:raised";

/// Whether `anchor` is one of Usage › Limits' rows: the page opens its Limits tab for it.
pub(crate) fn is_limit_row(anchor: &str) -> bool {
    [ORG_ROW, TEAM_ROW, PERSON_ROW, USAGE_ROW].contains(&anchor)
}

/// The budget's level, as Gateway's 402 `budget_exceeded` names it in `details.level`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Level {
    Organization,
    Team,
    Person,
}

impl Level {
    pub(crate) fn wire(level: &str) -> Option<Level> {
        match level {
            "organization" => Some(Level::Organization),
            "team" => Some(Level::Team),
            "person" => Some(Level::Person),
            _ => None,
        }
    }

    /// The row of Usage › Limits that sets it.
    pub(crate) fn row(self) -> &'static str {
        match self {
            Level::Organization => ORG_ROW,
            Level::Team => TEAM_ROW,
            Level::Person => PERSON_ROW,
        }
    }

    /// How a refused chat answer says it: the start of the refusal's words.
    fn subject(self) -> &'static str {
        match self {
            Level::Organization => "The organization's budget",
            Level::Team => "Your team's budget",
            Level::Person => "Your monthly limit",
        }
    }
}

/// What stopped the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    /// A budget: `used_up` when nothing is left (else the call may cost more than what is left), and the day calls
    /// resume ("Dec 1") when Gateway said it.
    Budget {
        level: Level,
        used_up: bool,
        resumes: Option<String>,
    },
    /// The card's monthly usage limit, which an owner changes.
    UsageLimit {
        used_up: bool,
    },
    FreeStart,
    CreditsOut,
    /// A charge that couldn't be collected, or a disputed one: the only red ✕ here.
    PaymentFailed,
    ConfirmPayment,
    PastDue,
    /// A charge still going through: nothing to do but wait.
    Processing,
}

const USED_UP: &str = " is used up.";
const NOT_COVERED: &str = " doesn't cover this call.";
const RESUME: &str = " Calls resume ";

/// The day calls resume, from an RFC 3339 time, as the person's calendar says it: "Dec 1".
pub(crate) fn resume_day(at: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.with_timezone(&chrono::Local).format("%b %-d").to_string())
}

/// A refused chat answer's words for Gateway's 402 `budget_exceeded`, from its `details`; `None` for a level this build
/// does not know (a product budget), which keeps Gateway's words.
pub(crate) fn budget_refusal(details: Option<&Value>) -> Option<String> {
    let details = details?;
    let level = Level::wire(details.get("level")?.as_str()?)?;
    let left = details
        .get("remainingUsdDecimal")
        .and_then(Value::as_str)
        .and_then(crate::pay_notice::micros_text);
    let verb = if left == Some(0) { USED_UP } else { NOT_COVERED };
    let resume = details
        .get("resetsAt")
        .and_then(Value::as_str)
        .and_then(resume_day)
        .map_or(String::new(), |day| format!("{RESUME}{day}."));
    Some(format!("{}{verb}{resume}", level.subject()))
}

/// The stop a refused chat answer's words name (`pay_notice::refusal`); `None` for any other refusal.
pub(crate) fn stop_of(refusal: &str) -> Option<Stop> {
    for level in [Level::Organization, Level::Team, Level::Person] {
        if let Some(rest) = refusal.strip_prefix(level.subject()) {
            let resumes = rest
                .split_once(RESUME)
                .map(|(_, day)| day.trim_end_matches('.').to_string());
            return Some(Stop::Budget {
                level,
                used_up: rest.starts_with(USED_UP),
                resumes,
            });
        }
    }
    let starts = |prefix: &str| refusal.starts_with(prefix);
    Some(if starts("Nothing is left of this month's usage limit") {
        Stop::UsageLimit { used_up: true }
    } else if starts("This month's usage limit doesn't cover")
        || (starts("This call may cost up to") && refusal.contains("usage limit"))
    {
        Stop::UsageLimit { used_up: false }
    } else if starts("Your free start doesn't cover this call") {
        Stop::FreeStart
    } else if starts("Your Nexus credits don't cover this call") {
        Stop::CreditsOut
    } else if starts("A payment didn't go through") || starts("A payment on the card was disputed") {
        Stop::PaymentFailed
    } else if starts("The bank wants you to confirm a payment") {
        Stop::ConfirmPayment
    } else if starts("A payment is past due") {
        Stop::PastDue
    } else if starts("A payment is still going through") {
        Stop::Processing
    } else {
        return None;
    })
}

/// A setup line's words and fixes for a stop, before the window adds what it does around them.
pub(crate) struct Words {
    pub what: String,
    pub why: Option<String>,
    pub tone: Tone,
    pub icon: Lucide,
    /// The one fix for who may make it ("Raise limit"); who may not sees Ask.
    pub fix: Option<(&'static str, Fix)>,
    pub billing: Option<BillingBlock>,
    pub ask: String,
    pub doing: &'static str,
}

/// "Acme", or "Your organization" before the account is read.
fn account(org: &str) -> &str {
    match org.trim() {
        "" | "your organization" => "Your organization",
        org => org,
    }
}

/// The words of `stop` for organization `org`.
pub(crate) fn words(stop: &Stop, org: &str) -> Words {
    let org = account(org);
    let keeps = Some("Your own keys keep working.".to_string());
    let open_billing = |label: &'static str| Some((label, Fix::open(PageId::Billing).needs(BILLING)));
    let billing = |what: String, tone: Tone, label: &'static str, block: BillingBlock| Words {
        what,
        why: keeps.clone(),
        tone,
        icon: Lucide::CreditCard,
        fix: open_billing(label),
        billing: Some(block),
        ask: format!("Could you check {org}'s billing in Nexus? Model calls Nexus pays are paused."),
        doing: "fix it",
    };
    match stop {
        Stop::Budget {
            level,
            used_up,
            resumes,
        } => {
            let state = if *used_up {
                "is used up"
            } else {
                "doesn't cover this call"
            };
            let (what, ask) = match level {
                Level::Organization => (
                    format!("{org}'s budget {state}."),
                    format!("{org}'s Nexus budget is used up. Could you raise it?"),
                ),
                Level::Team => (
                    format!("Your team's budget in {org} {state}."),
                    format!("My team's Nexus budget in {org} is used up. Could you raise it?"),
                ),
                Level::Person => (
                    format!("Your monthly limit in {org} {state}."),
                    format!("My Nexus limit in {org} is used up. Could you raise it?"),
                ),
            };
            Words {
                what,
                why: resumes.as_ref().map(|day| format!("Calls resume {day}.")),
                tone: Tone::Limit,
                icon: Lucide::Gauge,
                fix: Some(("Raise limit", Fix::open_at(PageId::Usage, level.row()).needs(BUDGETS))),
                billing: None,
                ask,
                doing: "raise it",
            }
        }
        Stop::UsageLimit { used_up } => Words {
            what: if *used_up {
                format!("{org}'s usage limit is used up.")
            } else {
                format!("{org}'s usage limit doesn't cover this call.")
            },
            why: keeps.clone(),
            tone: Tone::Limit,
            icon: Lucide::Gauge,
            fix: Some(("Raise limit", Fix::open_at(PageId::Usage, USAGE_ROW).needs(BILLING))),
            billing: Some(BillingBlock::UsageLimit),
            ask: format!("{org}'s Nexus usage limit is used up. Could you raise it?"),
            doing: "raise it",
        },
        Stop::FreeStart => billing(
            format!("{org}'s free start doesn't cover this call."),
            Tone::Limit,
            "Set up billing",
            BillingBlock::FreeStartUsed,
        ),
        Stop::CreditsOut => billing(
            format!("{org}'s Nexus credits don't cover this call."),
            Tone::Limit,
            "Buy credits",
            BillingBlock::CreditsOut,
        ),
        Stop::PaymentFailed => billing(
            format!("A payment for {org} didn't go through."),
            Tone::Failed,
            "Fix payment",
            BillingBlock::PaymentFailed,
        ),
        Stop::ConfirmPayment => billing(
            format!("The bank wants an owner of {org} to confirm a payment."),
            Tone::Fixable,
            "Confirm payment",
            BillingBlock::PaymentFailed,
        ),
        Stop::PastDue => billing(
            format!("A payment for {org} is past due."),
            Tone::Fixable,
            "Pay invoice",
            BillingBlock::PastDue,
        ),
        Stop::Processing => Words {
            what: format!("A payment for {org} is still going through."),
            why: Some("A card payment takes seconds, and a bank payment can take a few days.".into()),
            tone: Tone::Info,
            icon: Lucide::CreditCard,
            fix: None,
            billing: None,
            ask: String::new(),
            doing: "",
        },
    }
}

/// The setup line of `stop`: `other` first (another payer the person has, "Use Max plan"), then the stop's fix, which
/// `wrap` may turn into a step of the window's (to wait for the raise). Who may not fix it sees Ask, naming who can.
pub(crate) fn line(
    key: impl Into<FixKey>,
    stop: &Stop,
    org: &str,
    other: Option<(String, Fix)>,
    wrap: impl FnOnce(Fix) -> Fix,
) -> SetupLine {
    let words = words(stop, org);
    let mut line = SetupLine::new(key, words.icon, words.what).tone(words.tone);
    if let Some(why) = words.why {
        line = line.why(why);
    }
    if let Some((label, fix)) = other {
        line = line.fix(label, fix);
    }
    if let Some((label, fix)) = words.fix {
        line = line.fix(label, wrap(fix));
    }
    if let Some(block) = words.billing {
        line = line.billing(block);
    }
    if words.ask.is_empty() {
        line
    } else {
        line.ask(words.ask, words.doing)
    }
}

/// A budget or a limit was saved: what waits on [`RAISED`] goes on, in the window the person saved it in.
pub(crate) fn raised(cx: &mut App) {
    cx.defer(|cx| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let key: FixKey = RAISED.into();
        if let Err(error) = window.update(cx, |_, window, cx| crate::setup::done(&key, window, cx)) {
            eprintln!("nexus-desktop-gpui: limit raised: {error}");
        }
    });
}

#[cfg(test)]
#[path = "limit_stop_tests.rs"]
mod tests;
