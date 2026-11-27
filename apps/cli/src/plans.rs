//! Plans, dormant: built only with `--features plans`. Pricing v2.0 sells no plans,
//! so the default `nexus` has no plan command and shows no plan. A Gateway built with plans and with `NEXUS_PLANS` sells
//! them; this module is the CLI side of that: `billing plans`, `billing plan <name>` (Stripe Checkout, then wait until
//! Stripe starts it), `billing plan change` and the "Plan" row of `nexus billing`.
//!
//! To bring plans back: build with the feature, and generate commands.json with it too: `interfaces/scripts/commands.mjs`
//! builds the catalog without features. The TypeScript CLI needs the same (`interfaces/src/plans.ts`).

use serde_json::{Value, json};

use crate::billing::{NO_BILLING_API, Off, Wait, answered, checkout, day, dollars, owners_only, unavailable};
use crate::cli::Cmd;
use crate::error::{CliError, Error, Result, usage};
use crate::json::{a, js_string, n, s};
use crate::style::{Row, Style, col, list, right};

/// The plan rows of `nexus billing --help` and of the catalog, after `card add`.
pub const PLAN_COMMANDS: crate::catalog::Rows = &[
    (
        "plan <name> [--yearly] [--seats N] [--no-browser] [--no-wait]",
        "Start a plan (plus, pro, teams_pro…) through Stripe Checkout, then wait until Stripe starts it (up to 10 minutes). Only where Gateway sells plans",
    ),
    (
        "plans",
        "The plans this organization can take and what they include. Prices come from Stripe",
    ),
    (
        "plan change <name> [--yearly] [--seats N] [--dry-run]",
        "Change the plan or interval: an upgrade now, prorated; anything else at the period's end. A per-seat plan has one seat per member",
    ),
    (
        "plan change --undo",
        "Cancel the plan change pending for the period's end",
    ),
];

/// The plan commands' flags, as the catalog reads them.
pub const PLAN_FLAG_TEXT: &[(&str, &str)] = &[
    (
        "billing plan --yearly",
        "The yearly price instead of the monthly one; included usage still renews each month",
    ),
    (
        "billing plan --seats",
        "Seats on a per-seat plan, such as teams_pro (default: its minimum)",
    ),
    (
        "billing plan --no-browser",
        "Only print the Stripe Checkout link; do not open a browser",
    ),
    (
        "billing plan --no-wait",
        "Print the link and return without waiting for the plan",
    ),
    (
        "billing plan --dry-run",
        "With change: say what it charges now and when it applies, and change nothing",
    ),
    (
        "billing plan --undo",
        "With change: cancel the plan change pending for the period's end",
    ),
];

/// `nexus billing plan …`: `args` after `plan`.
pub fn plan_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    match args.first().map(String::as_str) {
        Some("change") => change_plan(cmd, args.get(1).map(String::as_str)),
        name => start_plan(cmd, name),
    }
}

/// A plan's name as people read it.
pub fn plan_name(plan: &str) -> String {
    match plan {
        "teams_free" => return "Teams Free".into(),
        "teams_plus" => return "Teams Plus".into(),
        "teams_pro" => return "Teams Pro".into(),
        _ => {}
    }
    let words = plan.replace('_', " ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A plan as `GET /v1/billing` sends it: "Pro, monthly, $10.00 included each month".
pub(crate) fn plan_text(style: &Style, plan: &Value) -> String {
    let seats = n(plan, "seats");
    let status = s(plan, "status").unwrap_or("");
    let mut text = format!(
        "{}, {}{}, {} included each month",
        plan_name(s(plan, "plan").unwrap_or("")),
        if s(plan, "interval") == Some("year") {
            "yearly"
        } else {
            "monthly"
        },
        if seats > 1.0 {
            format!(", {seats} seats")
        } else {
            String::new()
        },
        dollars(s(plan, "includedUsd").unwrap_or("")),
    );
    if !matches!(status, "active" | "trialing") {
        text.push_str(&style.yellow(&format!(" ({})", status.replace('_', " "))));
    }
    // A change for the period's end.
    if let Some(pending) = plan.get("pendingChange").filter(|pending| !pending.is_null()) {
        let seats = n(pending, "seats");
        text.push_str(&format!(
            " (becomes {}, {}{} on {})",
            plan_name(s(pending, "plan").unwrap_or("")),
            if s(pending, "interval") == Some("year") {
                "yearly"
            } else {
                "monthly"
            },
            if seats > 1.0 {
                format!(", {seats} seats")
            } else {
                String::new()
            },
            day(s(pending, "at").unwrap_or(""))
        ));
    }
    // A cancelled plan ends on Stripe's `cancelAt`, which can be before the period's end or in a later period; an older
    // Gateway sends only `cancelAtPeriodEnd`.
    let ends = s(plan, "cancelAt").or_else(|| {
        (plan.get("cancelAtPeriodEnd").and_then(Value::as_bool) == Some(true))
            .then(|| s(plan, "currentPeriodEnd").unwrap_or(""))
    });
    if let Some(end) = ends {
        text.push_str(&format!(" (ends {})", end.get(..10).unwrap_or(end)));
    }
    text
}

/// A plan's state for the wait: its name, interval, seats and status.
fn plan_key(plan: Option<&Value>) -> String {
    match plan.filter(|plan| !plan.is_null()) {
        Some(plan) => format!(
            "{}:{}:{}:{}",
            js_string(plan.get("plan")),
            js_string(plan.get("interval")),
            js_string(plan.get("seats")),
            js_string(plan.get("status"))
        ),
        None => "free".into(),
    }
}

/// The plans as a table: each plan's key and name, its price each month and each year, and its included usage.
fn plans_lines(style: &Style, catalogue: &Value) -> Vec<String> {
    let personal = s(catalogue, "audience") == Some("personal");
    let entries = a(catalogue, "plans");
    let mut lines = vec![
        format!(
            "{} {} {}",
            style.bold("Plans"),
            style.bullet,
            if personal {
                "for a personal organization"
            } else {
                "for a company, priced per seat"
            }
        ),
        String::new(),
    ];
    if entries.is_empty() {
        lines.push("This Gateway sells no plan to this kind of organization.".into());
        return lines;
    }
    let interval = |entry: &Value, which: &str| {
        a(entry, "intervals")
            .iter()
            .find(|price| s(price, "interval") == Some(which))
            .cloned()
    };
    let amount = |entry: &Value, which: &str| {
        interval(entry, which)
            .and_then(|price| s(&price, "amountUsd").map(dollars))
            .unwrap_or_else(|| "–".into())
    };
    let missing = entries
        .iter()
        .any(|entry| a(entry, "intervals").iter().any(|price| !price["error"].is_null()));
    let rows: Vec<Row> = entries
        .iter()
        .map(|entry| {
            let included = interval(entry, "month")
                .or_else(|| interval(entry, "year"))
                .and_then(|price| s(&price, "includedUsd").map(dollars))
                .unwrap_or_default();
            let current = entry.get("current").and_then(Value::as_bool) == Some(true);
            Row::from(vec![
                s(entry, "plan").unwrap_or("").to_string(),
                format!(
                    "{}{}",
                    s(entry, "name").unwrap_or(""),
                    if current {
                        style.green(" (yours)")
                    } else {
                        String::new()
                    }
                ),
                amount(entry, "month"),
                amount(entry, "year"),
                included,
            ])
        })
        .collect();
    lines.extend(list(
        style,
        &[
            col("Plan"),
            col("Name"),
            right("Monthly"),
            right("Yearly"),
            right("Included a month"),
        ],
        rows,
        None,
    ));
    lines.push(String::new());
    if !personal {
        let fewest = entries
            .iter()
            .map(|entry| n(entry, "minSeats"))
            .fold(f64::MAX, f64::min);
        lines.push(style.dim(&format!("Prices and included usage are per seat, from {fewest} seats.")));
    }
    if missing {
        lines.push(style.dim("–: Stripe did not give this price. Try again in a moment."));
    }
    lines.push(style.dim(&format!(
        "Start one: nexus billing plan <plan> [--yearly]{}",
        if personal { "" } else { " [--seats N]" }
    )));
    lines
}

/// What `plans` says on a Gateway that sells none (pricing v2.0).
const NO_PLANS: &str =
    "This Gateway sells no plans: it bills pay as you go (`nexus billing credits`, `nexus billing after-the-fact`).";

/// `plans`: the plans this organization can take, priced as Stripe holds them. Dormant under pricing v2.0.
pub fn plans(cmd: &Cmd) -> Result<i32> {
    // Pricing v2.0 sells no plans: a Gateway without `NEXUS_PLANS` answers 503 `not_configured` and names it.
    let answer = match cmd.core.get("/v1/billing/plans") {
        Err(Error::Core(error)) if error.status == 503 && error.message.contains("NEXUS_PLANS") => {
            if cmd.json {
                return Ok(cmd.print(&json!({ "enabled": false, "reason": "no_plans", "message": error.message })));
            }
            cmd.out(format!("{} {NO_PLANS}", cmd.style.info));
            return Ok(0);
        }
        answer => answer,
    };
    let Some(catalogue) = answered(cmd, answer)? else {
        return Ok(0);
    };
    if cmd.json {
        return Ok(cmd.print(&catalogue));
    }
    cmd.lines(plans_lines(cmd.style, &catalogue));
    Ok(0)
}

const CHANGE_USAGE: &str =
    "Usage: nexus billing plan change <name> [--yearly] [--seats N] [--dry-run] | nexus billing plan change --undo";

/// `nexus billing plan change <name>`: change the plan or interval (a per-seat plan's seats follow its members), now when it costs more and at the
/// period's end otherwise; `--undo` cancels a change pending for the period's end.
fn change_plan(cmd: &Cmd, name: Option<&str>) -> Result<i32> {
    let style = cmd.style;
    let refused = |error: Error| -> Error {
        match unavailable(&error) {
            Some(Off::NotConfigured(message)) => {
                CliError::hint("Billing is off on this Gateway, so it changes no plan.", message).into()
            }
            Some(Off::NoBillingApi(_)) => CliError::new(format!("{NO_BILLING_API}, so it changes no plan.")).into(),
            None => owners_only(error),
        }
    };
    if cmd.has("undo") {
        let answer = cmd.core.delete("/v1/billing/plan/change").map_err(refused)?;
        if cmd.json {
            return Ok(cmd.print(&answer));
        }
        cmd.out(format!(
            "{} The pending change is cancelled. The plan stays: {}.",
            style.ok,
            plan_text(style, &answer["plan"])
        ));
        return Ok(0);
    }
    let Some(name) = name else {
        return usage(
            CHANGE_USAGE,
            Some(
                "An upgrade applies now, prorated; anything else at the period's end. A per-seat plan has one seat per member.",
            ),
        );
    };
    let seats = match cmd.flag("seats") {
        None => None,
        Some(text) => match text.parse::<i64>() {
            Ok(seats) if seats > 0 && text.chars().all(|c| c.is_ascii_digit()) => Some(seats),
            _ => return usage("Give the seats as a whole number, e.g. --seats 5.", Some(CHANGE_USAGE)),
        },
    };
    let interval = if cmd.has("yearly") { "year" } else { "month" };
    let dry_run = cmd.has("dry-run");
    let mut body = json!({ "plan": name, "interval": interval, "dryRun": dry_run });
    if let Some(seats) = seats {
        body["seats"] = json!(seats);
    }
    let answer = cmd.core.post("/v1/billing/plan/change", body).map_err(refused)?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    let now = s(&answer, "when") == Some("now");
    let at = day(s(&answer, "at").unwrap_or(""));
    let what = format!(
        "{} ({}{})",
        plan_name(name),
        if interval == "year" { "yearly" } else { "monthly" },
        seats.map(|seats| format!(", {seats} seats")).unwrap_or_default()
    );
    if dry_run {
        cmd.out(if now {
            format!(
                "{} Switching to {what} applies now: {} is charged today, prorated.",
                style.info,
                dollars(s(&answer, "amountDueUsd").unwrap_or("0"))
            )
        } else {
            format!(
                "{} Switching to {what} applies on {at}, when the period ends: nothing is charged now.",
                style.info
            )
        });
        cmd.out(format!(
            "  {}",
            style.dim("Nothing changes without dropping --dry-run.")
        ));
        return Ok(0);
    }
    if now {
        cmd.out(format!(
            "{} Changed: {}. Stripe charged the prorated difference.",
            style.ok,
            plan_text(style, &answer["plan"])
        ));
    } else {
        cmd.out(format!(
            "{} Switching to {what} on {at}, when the period ends. Until then the plan stays as it is.",
            style.ok
        ));
        cmd.out(format!(
            "  {}",
            style.dim("Cancel it: nexus billing plan change --undo")
        ));
    }
    Ok(0)
}

const PLAN_USAGE: &str = "Usage: nexus billing plan <name> [--yearly] [--seats N] [--no-browser] [--no-wait]";

/// `nexus billing plan <name>`: Stripe Checkout's link to start the plan, then wait until Stripe starts it.
fn start_plan(cmd: &Cmd, name: Option<&str>) -> Result<i32> {
    let style = cmd.style;
    let Some(name) = name else {
        return usage(
            PLAN_USAGE,
            Some(
                "It opens Stripe Checkout to start a plan, such as plus, pro or teams_pro (`nexus billing` shows yours).",
            ),
        );
    };
    let seats = match cmd.flag("seats") {
        // A bare `--seats` parses as a switch: refuse it rather than start the plan's minimum.
        None if cmd.has("seats") => {
            return usage("Give the seats as a whole number, e.g. --seats 5.", Some(PLAN_USAGE));
        }
        None => None,
        Some(text) => match text.parse::<i64>() {
            Ok(seats) if seats > 0 && text.chars().all(|c| c.is_ascii_digit()) => Some(seats),
            _ => return usage("Give the seats as a whole number, e.g. --seats 5.", Some(PLAN_USAGE)),
        },
    };
    let interval = if cmd.has("yearly") { "year" } else { "month" };
    let mut choice = json!({ "plan": name, "interval": interval });
    if let Some(seats) = seats {
        choice["seats"] = json!(seats);
    }
    let started = cmd
        .core
        .get("/v1/billing")
        .and_then(|before| Ok((before, cmd.core.post("/v1/billing/plan", choice)?)));
    let (before, link) = match started {
        Ok(started) => started,
        Err(error) => {
            // A plan cannot start here: the command fails, in plain words.
            return Err(match unavailable(&error) {
                Some(Off::NotConfigured(message)) => {
                    CliError::hint("Billing is off on this Gateway, so it starts no plan.", message).into()
                }
                Some(Off::NoBillingApi(_)) => CliError::new(format!("{NO_BILLING_API}, so it starts no plan.")).into(),
                None => owners_only(error),
            });
        }
    };
    let was = plan_key(before.get("plan"));
    checkout(
        cmd,
        &link,
        &format!(
            "Start {} ({})",
            plan_name(name),
            if interval == "year" { "yearly" } else { "monthly" }
        ),
        &Wait {
            noun: "plan",
            verb: ["start", "starts", "started"],
            key: "plan",
            // Another plan than before, and paid: an `incomplete` subscription waits for its payment.
            done: &|plan| {
                (plan_key(Some(plan)) != was && matches!(s(plan, "status"), Some("active" | "trialing")))
                    .then(|| format!("Started: {}", plan_text(style, plan)))
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_a_plan_as_people_read_it() {
        assert_eq!(plan_name("teams_free"), "Teams Free");
        assert_eq!(plan_name("teams_pro"), "Teams Pro");
        assert_eq!(plan_name("teams_plus"), "Teams Plus");
        assert_eq!(plan_name("plus"), "Plus");
        assert_eq!(plan_name("pro"), "Pro");
    }
}
