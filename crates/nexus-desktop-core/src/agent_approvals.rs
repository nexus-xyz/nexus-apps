//! Approvals for a native shell: `GET /agent/approvals/settings`'s body read strictly, as the kit's
//! `approvalsViewOf`, and the words of the view, as Nexus UI's `lib/agent-approvals.ts`. Nexus GPUI draws it
//! (`desktop-gpui/src/pages/agent/approvals.rs`).

use serde_json::{Value, json};

use crate::agent::Problem;

/// One "Always allow" rule: a kind of write, to one recipient when the card had one.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: String,
    /// Such as `send`, or `write:<connector>.<operation>` for a write of no known kind.
    pub kind: String,
    pub to: Option<String>,
    pub since: String,
}

/// `GET` or `PUT /agent/approvals/settings`. Money is a decimal string of dollars.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalsView {
    /// Off: the agent does not sort writes by tier in this deployment yet, so every write gets a card.
    pub tiers_on: bool,
    pub spend_limit_usd: String,
    pub own_line: bool,
    pub default_spend_limit_usd: String,
    pub max_spend_limit_usd: String,
    pub rules: Vec<Rule>,
    /// How long a card waits for an answer, in minutes. None from an older agent API.
    pub expires_after_minutes: Option<u32>,
}

/// A decimal string of dollars as the agent API writes one: 1 to 13 digits, a point and 2 digits.
fn dollars_amount(text: &str) -> bool {
    let Some((whole, cents)) = text.split_once('.') else {
        return false;
    };
    (1..=13).contains(&whole.len())
        && cents.len() == 2
        && whole.bytes().chain(cents.bytes()).all(|byte| byte.is_ascii_digit())
}

/// The body, read strictly; None when it is not one. A rule that is not one is left out.
pub fn approvals_view_of(body: &Value) -> Option<ApprovalsView> {
    let tiers_on = match body.get("tiers")?.as_str()? {
        "on" => true,
        "off" => false,
        _ => return None,
    };
    let own_line = body.get("ownLine")?.as_bool()?;
    let rules = body.get("rules")?.as_array()?;
    let amount = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .filter(|text| dollars_amount(text))
            .map(String::from)
    };
    let rules = rules
        .iter()
        .filter_map(|rule| {
            let text = |key: &str| rule.get(key).and_then(Value::as_str).map(String::from);
            Some(Rule {
                id: text("id")?,
                kind: text("kind")?,
                to: text("to"),
                since: text("since")?,
            })
        })
        .collect();
    let expires_after_minutes = body
        .get("expiresAfterMinutes")
        .and_then(Value::as_u64)
        .filter(|minutes| (1..=24 * 60).contains(minutes))
        .and_then(|minutes| u32::try_from(minutes).ok());
    Some(ApprovalsView {
        tiers_on,
        spend_limit_usd: amount("spendLimitUsd")?,
        own_line,
        default_spend_limit_usd: amount("defaultSpendLimitUsd")?,
        max_spend_limit_usd: amount("maxSpendLimitUsd")?,
        rules,
        expires_after_minutes,
    })
}

/// A decimal string of dollars as the person reads it: `$50` for whole dollars, `$49.99` otherwise.
pub fn dollars(amount: &str) -> String {
    format!("${}", amount.strip_suffix(".00").unwrap_or(amount))
}

/// Each kind that can ask first, as the person reads it.
fn kind_words(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "send" => "Send or reply",
        "book" => "Book",
        "spend" => "Spend",
        "rsvp" => "Accept or decline invites",
        "share-inside" => "Share inside your organization",
        _ => return None,
    })
}

/// A rule in words: what it lets the agent do, and to whom. A write of no known kind is named by its operation.
pub fn rule_text(kind: &str, to: Option<&str>) -> (String, String) {
    let what = match kind.strip_prefix("write:") {
        Some(operation) => operation.to_string(),
        None => kind_words(kind).unwrap_or(kind).to_string(),
    };
    let to = match to.filter(|to| !to.is_empty()) {
        Some(to) => format!("To {to}"),
        None => "When it names no one".to_string(),
    };
    (what, to)
}

/// What the person typed as a spend line: dollars with up to two decimals, up to the maximum. Err is why not.
pub fn line_of(text: &str, max_usd: &str) -> Result<String, String> {
    let value = text.trim();
    let value = value.strip_prefix('$').unwrap_or(value);
    let (whole, cents) = value.split_once('.').unwrap_or((value, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let shaped = (1..=7).contains(&whole.len())
        && digits(whole)
        && (!value.contains('.') || ((1..=2).contains(&cents.len()) && digits(cents)));
    if !shaped {
        return Err("Enter an amount in dollars, such as 50 or 49.99.".into());
    }
    let number = |text: &str| text.parse::<f64>().unwrap_or(f64::MAX);
    if number(value) > number(max_usd) {
        return Err(format!("Your organization allows {} at most.", dollars(max_usd)));
    }
    Ok(value.to_string())
}

/// `PUT /agent/approvals/settings`'s body: the line, or `null` to go back to the organization's default.
pub fn line_body(usd: Option<&str>) -> String {
    json!({ "spendLimitUsd": usd }).to_string()
}

/// The line under the spend field. It names the organization's maximum and the default only when each differs from the
/// person's line, so a line at both says its amount once.
pub fn line_hint(view: &ApprovalsView) -> String {
    let line = format!("Over {}, you always confirm in Nexus.", dollars(&view.spend_limit_usd));
    let max = dollars(&view.max_spend_limit_usd);
    let fallback = dollars(&view.default_spend_limit_usd);
    let show_max = view.max_spend_limit_usd != view.spend_limit_usd;
    let show_default = view.default_spend_limit_usd != view.spend_limit_usd;
    match (show_max, show_default) {
        (true, true) => format!("{line} Your organization allows up to {max}; the default is {fallback}."),
        (true, false) => format!("{line} Your organization allows up to {max}."),
        (false, true) => format!("{line} The default is {fallback}."),
        (false, false) => line,
    }
}

/// One of the three tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Freely,
    AsksFirst,
    AlwaysAsks,
}

/// Each kind's tier, as the kit's `AGENT_KIND_TIERS` (the agent core's `TIER_OF`).
pub fn kind_tier(kind: &str) -> Option<Tier> {
    Some(match kind {
        "draft" | "remind" | "list" | "track" | "answer" => Tier::Freely,
        "send" | "book" | "spend" | "rsvp" | "share-inside" | "write" => Tier::AsksFirst,
        "book-travel" | "password" | "share-outside" | "delete" | "cancel" | "unsubscribe" => Tier::AlwaysAsks,
        _ => return None,
    })
}

/// One tier as a column of the page: its name, one line on what it means, and what falls in it.
#[derive(Debug, Clone, PartialEq)]
pub struct TierColumn {
    pub tier: Tier,
    pub title: &'static str,
    pub note: &'static str,
    pub lines: Vec<String>,
}

/// Where a line of the columns goes: its first kind's tier, or a tier of the core's own that no kind names.
enum Place {
    Kinds(&'static [&'static str]),
    Tier(Tier),
}

/// Each line, in order. `$` in a line is the person's spend line.
const TIER_LINES: [(Place, &str); 17] = [
    (Place::Tier(Tier::Freely), "Read mail, calendars and files"),
    (Place::Kinds(&["draft"]), "Draft replies, itineraries and carts"),
    (Place::Kinds(&["remind", "list"]), "Remind you; keep lists"),
    (Place::Kinds(&["track"]), "Track packages and flights"),
    (
        Place::Kinds(&["answer"]),
        "Answer people it already works with on your errand",
    ),
    (Place::Kinds(&["send"]), "Send or reply to someone"),
    (Place::Kinds(&["book"]), "Book a table, a ride or a meeting"),
    (Place::Kinds(&["spend"]), "Spend up to $"),
    (Place::Kinds(&["rsvp"]), "Accept or decline invites"),
    (Place::Kinds(&["share-inside"]), "Share a file inside your organization"),
    (Place::Kinds(&["write"]), "Any other write it does not know"),
    (
        Place::Tier(Tier::AlwaysAsks),
        "Spend over $, or an amount it cannot read",
    ),
    (Place::Kinds(&["book-travel"]), "Any booking with travel"),
    (Place::Kinds(&["password"]), "Use a password"),
    (
        Place::Kinds(&["share-outside"]),
        "Share a file outside your organization",
    ),
    (
        Place::Kinds(&["delete", "cancel", "unsubscribe"]),
        "Delete, cancel or unsubscribe",
    ),
    (Place::Tier(Tier::AlwaysAsks), "Anything that cannot be undone"),
];

/// The three tiers as columns, with the person's spend line in the two spend lines.
pub fn tier_columns(spend_limit_usd: &str) -> Vec<TierColumn> {
    let line = dollars(spend_limit_usd);
    let columns = [
        (
            Tier::Freely,
            "Does freely",
            "Nothing leaves you, or it can be undone. You see it after.",
        ),
        (
            Tier::AsksFirst,
            "Asks first",
            "One tap on its card in the chat. Always allow can learn it.",
        ),
        (
            Tier::AlwaysAsks,
            "Always asks",
            "You confirm it in Nexus, signed in. Never learned away.",
        ),
    ];
    columns
        .into_iter()
        .map(|(tier, title, note)| TierColumn {
            tier,
            title,
            note,
            lines: TIER_LINES
                .iter()
                .filter(|(place, _)| match place {
                    Place::Tier(own) => *own == tier,
                    Place::Kinds(kinds) => {
                        kinds
                            .first()
                            .and_then(|kind| kind_tier(kind))
                            .unwrap_or(Tier::AsksFirst)
                            == tier
                    }
                })
                .map(|(_, text)| text.replace('$', &line))
                .collect(),
        })
        .collect()
}

/// A row the core applies with no choice yet: one value and one line under it.
#[derive(Debug, Clone, PartialEq)]
pub struct FixedRow {
    pub title: &'static str,
    pub value: &'static str,
    pub hint: String,
}

/// The fixed rows, in the page's order. The spend line goes after the first.
pub fn fixed_rows(view: &ApprovalsView) -> Vec<FixedRow> {
    let expiry = match view.expires_after_minutes {
        Some(1) => "After 1 minute, the card expires.".to_string(),
        Some(minutes) => format!("After {minutes} minutes, the card expires."),
        None => "The card expires.".to_string(),
    };
    vec![
        FixedRow {
            title: "Asks first above",
            value: "$0",
            hint: "Every spend asks first. Nothing is bought freely.".into(),
        },
        FixedRow {
            title: "Sending to someone new",
            value: "Asks first",
            hint: "Every send asks first. Always allow covers only the recipient it was given for.".into(),
        },
        FixedRow {
            title: "Bookings",
            value: "Asks first",
            hint: "Tables, rides and meetings. A booking with travel, or over your line, always asks.".into(),
        },
        FixedRow {
            title: "If nobody answers",
            value: "Nothing is done",
            hint: format!("{expiry} Nothing is done, and the card says so."),
        },
    ]
}

/// The learned rules by recipient, in the order each recipient was first learned.
pub fn learned_groups(rules: &[Rule]) -> Vec<(String, Vec<Rule>)> {
    let mut groups: Vec<(Option<String>, Vec<Rule>)> = Vec::new();
    for rule in rules {
        let to = rule.to.clone().filter(|to| !to.is_empty());
        match groups.iter_mut().find(|(kept, _)| *kept == to) {
            Some((_, kept)) => kept.push(rule.clone()),
            None => groups.push((to, vec![rule.clone()])),
        }
    }
    groups
        .into_iter()
        .map(|(to, rules)| {
            let title = match to {
                Some(to) => format!("To {to}"),
                None => "When it names no one".to_string(),
            };
            (title, rules)
        })
        .collect()
}

/// What the view says once a rule or all of them are reset.
pub fn revoked_text(one: bool, count: u64) -> String {
    if one {
        return "Nexus asks first again for that write.".into();
    }
    let kinds = if count == 1 { "kind of write" } else { "kinds of write" };
    format!("Nexus asks first again for {count} {kinds}.")
}

/// Whether a failure reads the same in every view of the Assistant (the kit's `agentWide`): a sign-in that ended, a
/// refused credential, or an agent API that is not available here.
pub fn agent_wide(status: u16, body: Option<&Value>) -> bool {
    let code = body
        .and_then(|body| body.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("");
    matches!(status, 401 | 403)
        || (status == 503 && matches!(code, "agents_api_unconfigured" | "agents_api_refused_token"))
        || (status == 502 && code == "agents_api_unreachable")
}

/// The failure's `detail`, when it sent one.
pub fn detail_of(body: Option<&Value>) -> String {
    body.and_then(|body| body.get("detail"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// The words of a failed read or change of a view: the wide failures as the chat says them, else `own`.
pub fn view_problem(status: u16, body: Option<&Value>, own: impl FnOnce(String) -> Problem) -> Problem {
    if agent_wide(status, body) {
        return crate::agent::problem_of(status, body);
    }
    own(detail_of(body))
}

/// What the view says when a read or a change fails (Nexus UI's `approvalsProblem`).
pub fn approvals_problem(status: u16, body: Option<&Value>) -> Problem {
    view_problem(status, body, |detail| {
        let said = |title: &str, fallback: &str| Problem {
            title: title.into(),
            detail: if detail.is_empty() {
                fallback.into()
            } else {
                detail.clone()
            },
        };
        match status {
            400 if !detail.is_empty() => said("That was not saved", ""),
            503 => said("Your approval settings are not available now", "Try again in a moment."),
            _ => said(
                "Your approval settings did not open",
                "Nexus could not reach the Assistant. Try again in a moment.",
            ),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Value {
        json!({
            "tiers": "on",
            "spendLimitUsd": "50.00",
            "ownLine": true,
            "defaultSpendLimitUsd": "25.00",
            "maxSpendLimitUsd": "200.00",
            "expiresAfterMinutes": 30,
            "rules": [
                { "id": "allow_0123456789abcdef01234567", "kind": "send", "to": "Priya N.", "since": "2026-11-03T09:00:00Z" },
                { "id": "allow_1123456789abcdef01234567", "kind": "rsvp", "since": "2026-11-04T09:00:00Z" },
                { "id": "allow_2123456789abcdef01234567", "kind": "write:linear.create_issue", "to": "Priya N.", "since": "2026-11-05T09:00:00Z" },
                { "id": 4, "kind": "send", "since": "x" }
            ]
        })
    }

    #[test]
    fn reads_the_settings_strictly_and_leaves_a_bad_rule_out() {
        let view = approvals_view_of(&body()).expect("reads");
        assert!(view.tiers_on && view.own_line);
        assert_eq!(view.spend_limit_usd, "50.00");
        assert_eq!(view.expires_after_minutes, Some(30));
        assert_eq!(view.rules.len(), 3, "the rule with a number for an id is left out");
        for broken in [
            json!({}),
            json!({ "tiers": "maybe" }),
            {
                let mut body = body();
                body["spendLimitUsd"] = json!("50");
                body
            },
            {
                let mut body = body();
                body["rules"] = json!(null);
                body
            },
        ] {
            assert_eq!(approvals_view_of(&broken), None, "{broken}");
        }
        let mut old = body();
        old["expiresAfterMinutes"] = json!(0);
        assert_eq!(
            approvals_view_of(&old).and_then(|view| view.expires_after_minutes),
            None
        );
    }

    #[test]
    fn the_tiers_are_three_columns_with_the_persons_line() {
        let columns = tier_columns("50.00");
        assert_eq!(
            columns.iter().map(|column| column.title).collect::<Vec<_>>(),
            ["Does freely", "Asks first", "Always asks"]
        );
        assert_eq!(columns[0].lines.len(), 5);
        assert!(columns[1].lines.contains(&"Spend up to $50".to_string()));
        assert!(
            columns[1]
                .lines
                .contains(&"Any other write it does not know".to_string())
        );
        assert_eq!(columns[2].lines[0], "Spend over $50, or an amount it cannot read");
        assert_eq!(
            columns[2].lines.last().map(String::as_str),
            Some("Anything that cannot be undone")
        );
    }

    #[test]
    fn if_nobody_answers_names_the_minutes_the_card_waits() {
        let view = approvals_view_of(&body()).expect("reads");
        let rows = fixed_rows(&view);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[3].title, "If nobody answers");
        assert_eq!(
            rows[3].hint,
            "After 30 minutes, the card expires. Nothing is done, and the card says so."
        );
        let older = ApprovalsView {
            expires_after_minutes: None,
            ..view
        };
        assert_eq!(
            fixed_rows(&older)[3].hint,
            "The card expires. Nothing is done, and the card says so."
        );
    }

    #[test]
    fn the_line_is_dollars_up_to_the_maximum() {
        assert_eq!(line_of("$49.99", "200.00"), Ok("49.99".into()));
        assert_eq!(line_of(" 50 ", "200.00"), Ok("50".into()));
        assert!(line_of("fifty", "200.00").is_err());
        assert!(line_of("1.234", "200.00").is_err());
        assert!(line_of("12.", "200.00").is_err());
        assert_eq!(
            line_of("500", "200.00"),
            Err("Your organization allows $200 at most.".into())
        );
        assert_eq!(line_body(Some("50")), r#"{"spendLimitUsd":"50"}"#);
        assert_eq!(line_body(None), r#"{"spendLimitUsd":null}"#);
        let view = approvals_view_of(&body()).expect("reads");
        assert_eq!(
            line_hint(&view),
            "Over $50, you always confirm in Nexus. Your organization allows up to $200; the default is $25."
        );
    }

    #[test]
    fn learned_rules_group_by_recipient_in_the_order_first_learned() {
        let view = approvals_view_of(&body()).expect("reads");
        let groups = learned_groups(&view.rules);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "To Priya N.");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, "When it names no one");
        assert_eq!(rule_text("send", None).0, "Send or reply");
        assert_eq!(rule_text("write:linear.create_issue", None).0, "linear.create_issue");
        assert_eq!(revoked_text(false, 1), "Nexus asks first again for 1 kind of write.");
    }

    #[test]
    fn a_failure_says_the_views_own_words_unless_it_is_wide() {
        assert_eq!(approvals_problem(401, None).title, "Sign in again");
        assert_eq!(
            approvals_problem(503, Some(&json!({ "error": "x" }))).title,
            "Your approval settings are not available now"
        );
        let refused = approvals_problem(400, Some(&json!({ "detail": "Too high." })));
        assert_eq!(
            (refused.title.as_str(), refused.detail.as_str()),
            ("That was not saved", "Too high.")
        );
        assert_eq!(
            approvals_problem(500, None).title,
            "Your approval settings did not open"
        );
    }
}
