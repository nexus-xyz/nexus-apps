//! Billing (`billing.ts`, Rust gateway, owners), pricing v2.0 "pay as you go": the card on file through
//! Stripe Checkout, the free start, Nexus credits and their auto top-up, paying after the fact
//! up to a monthly usage limit, the receipts, an unpaid month, the monthly statements and Stripe's billing portal. Plans
//! are in `plans.rs`, built only with `--features plans`: pricing v2.0 sells none.
//! Money is a decimal string from Gateway, printed digit for digit and never through a float (root rule 9). Billing
//! off on a Gateway (503 `not_configured`) and a Gateway without billing (404) are states, not errors.

use std::time::Duration;

use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::after_the_fact;
use crate::budgets::month_name;
use crate::catalog::{billing_commands, command_rows};
use crate::cli::Cmd;
use crate::client::search;
use crate::error::{CliError, Error, Result, Transport, usage};
use crate::json::{a, js_string, n, s};
use crate::org::encode_segment;
use crate::style::{Row, Style, col, flex, kv, list, now_ms, pad_end, pairs, parse_ms, right};

/// 402 `payment_past_due`: the calls Nexus pays for stopped for an unpaid month, and only an owner can pay it.
pub const PAST_DUE_HINT: &str = "Only an owner can pay it: `nexus billing` shows the unpaid month and its invoice.";

/// 402 `usage_limit_reached`: paying after the fact reached this month's usage limit.
pub const USAGE_LIMIT_HINT: &str = "An owner can raise the monthly usage limit (`nexus billing after-the-fact limit <usd>`) or buy Nexus credits (`nexus billing credits buy <usd>`).";

/// 402 `payment_failed`: the last charge after the fact failed, so calls on the card stopped.
pub const PAYMENT_FAILED_HINT: &str = "Only an owner can fix it: `nexus billing` shows the failed charge, and `nexus billing card add` replaces the card.";

/// 402 `payment_failed` for a payment the bank wants the cardholder to confirm (`authentication_required`).
pub const CARD_CONFIRM_HINT: &str = "The bank wants the cardholder to confirm a payment: an owner runs `nexus billing confirm` and approves it on the bank's page.";

/// 402 `charge_processing`: a charge for usage is still going through, so the card pays for no more calls.
/// The billing glossary's title and body, then the CLI's action (`kit/src/fixtures/billing-words.json`).
pub const CHARGE_PROCESSING_HINT: &str = "A payment is still going through. Models Nexus runs on the card are paused until it's paid. A card payment takes seconds, and a bank payment can take a few days. Credits and your own keys still work. An owner can buy Nexus credits: `nexus billing credits buy <usd>`.";

/// 402 `credit_exhausted`: the credits left are less than the call may cost, and the card does not pay after the fact.
/// The billing glossary's refusal title (`refusals` in `kit/src/fixtures/billing-words.json`), then the
/// CLI's action.
pub const CREDITS_OUT_HINT: &str = "Your Nexus credits don't cover this call. An owner buys Nexus credits (`nexus billing credits buy <usd>`) or turns on paying after the fact (`nexus billing after-the-fact on`). Your own keys keep working (`nexus keys add <provider> <key>`).";

/// 402 `subsidy_exhausted`: the free start is used up or expired.
pub const FREE_START_HINT: &str = "An owner switches to pay as you go: `nexus billing credits buy <usd>` or `nexus billing after-the-fact on`. Your own keys keep working (`nexus keys add <provider> <key>`).";

/// 402 `subsidy_exhausted` with `freeStart: cheaper_models_only`: the free start has money left, for cheaper models only.
pub const CHEAPER_MODELS_HINT: &str = "The free start pays for cheaper models only: try `--model nexus/auto`.";

/// `card add` (and `plan`, with `--features plans`) wait this long for Stripe's webhook: 300 polls, 2 s apart.
const POLL_MS: u64 = 2000;
const POLLS: usize = 300;

/// The least one purchase of credits buys; Gateway refuses less (400 `invalid_amount`).
const MIN_PURCHASE_USD: &str = "25";

pub fn billing_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus billing <command> [options]", style.bold("Usage")),
        String::new(),
    ];
    lines.extend(
        billing_commands()
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 40)))),
    );
    lines.push(String::new());
    lines.push(
        style.dim("Owners only. Stripe keeps the card: Nexus sees only its brand, its last 4 digits and its expiry."),
    );
    lines.push(style.dim(
        "Nexus-run AI spends the free start first, then Nexus credits, then the card when paying after the fact is on. Your own keys are free.",
    ));
    lines.push(style.dim(
        "Months are UTC. On a contract, a month closes an hour after it ends; then Stripe charges the card. Under $0.50 carries over.",
    ));
    lines
}

/// A decimal dollar string as Gateway sent it, with at least cents and no digit lost: "1.234500" is "$1.2345".
pub fn dollars(decimal: &str) -> String {
    let text = decimal.trim();
    let (sign, body) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text),
    };
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    let digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
    if whole.is_empty() || !digits(whole) || !digits(fraction) || (body.contains('.') && fraction.is_empty()) {
        return decimal.to_string();
    }
    let fraction = fraction.trim_end_matches('0');
    format!("{sign}${whole}.{fraction:0<2}")
}

/// Whole cents as a string ("1234") in dollars ("$12.34").
pub fn cents(amount: &str) -> String {
    if amount.is_empty() || !amount.chars().all(|c| c.is_ascii_digit()) {
        return amount.to_string();
    }
    let digits = format!("{amount:0>3}");
    let (whole, fraction) = digits.split_at(digits.len() - 2);
    format!("${whole}.{fraction}")
}

/// Basis points as a percent, in integers only: 500 is "5%", 250 is "2.5%".
pub fn percent(bps: i64) -> String {
    let rest = bps % 100;
    let fraction = if rest != 0 {
        format!(".{}", format!("{rest:02}").trim_end_matches('0'))
    } else {
        String::new()
    };
    format!("{}{fraction}%", bps / 100)
}

/// Stripe's card brand (`visa`, `amex`…) as people write it.
fn brand_name(brand: &str) -> String {
    match brand {
        "amex" => "American Express".into(),
        "diners" => "Diners Club".into(),
        "discover" => "Discover".into(),
        "jcb" => "JCB".into(),
        "mastercard" => "Mastercard".into(),
        "unionpay" => "UnionPay".into(),
        "visa" => "Visa".into(),
        "" | "unknown" => "Card".into(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

/// A card is good through the last day of its expiry month.
fn expired(card: &Value, now: OffsetDateTime) -> bool {
    let (year, month) = (f64::from(now.year()), f64::from(u8::from(now.month())));
    let (exp_year, exp_month) = (n(card, "expYear"), n(card, "expMonth"));
    exp_year < year || (exp_year == year && exp_month < month)
}

fn card_text(style: &Style, card: &Value) -> String {
    format!(
        "{} ending in {}, expires {:0>2}/{}{}",
        brand_name(s(card, "brand").unwrap_or("")),
        js_string(card.get("last4")),
        js_string(card.get("expMonth")),
        js_string(card.get("expYear")),
        if expired(card, OffsetDateTime::now_utc()) {
            style.yellow(" (expired)")
        } else {
            String::new()
        }
    )
}

/// The card on file has no id: its brand, last 4 and expiry stand in.
fn card_key(card: Option<&Value>) -> String {
    match card.filter(|card| !card.is_null()) {
        Some(card) => format!(
            "{}:{}:{}/{}",
            js_string(card.get("brand")),
            js_string(card.get("last4")),
            js_string(card.get("expMonth")),
            js_string(card.get("expYear"))
        ),
        None => "none".into(),
    }
}

/// The free start's row. In a build with plans, a plan's included usage sits in the same subsidy, so on a plan it is
/// the Nexus usage.
#[cfg_attr(not(feature = "plans"), allow(unused_variables))]
fn subsidy_label(billing: &Value) -> &'static str {
    #[cfg(feature = "plans")]
    if billing.get("plan").is_some_and(|plan| !plan.is_null()) {
        return "Nexus usage";
    }
    "Free start"
}

/// The free start's row: what is left, until when, and which models it pays for; once expired, when. The
/// TypeScript CLI writes the same (`freeStartText`).
pub(crate) fn free_start_text(free: &Value, pricing: Option<&Value>) -> String {
    let left = format!(
        "{} left of {}",
        dollars(s(free, "remainingUsd").unwrap_or("")),
        dollars(s(free, "grantedUsd").unwrap_or(""))
    );
    let day: Option<String> = s(free, "expiresAt").map(|at| at.chars().take(10).collect());
    if s(free, "expiredUsd").is_some() {
        return match day {
            Some(day) => format!("{left}; the free start expired on {day}"),
            None => left,
        };
    }
    let until = day.map(|day| format!(" until {day}")).unwrap_or_default();
    let cap = pricing
        .and_then(|pricing| s(pricing, "freeStartMaxOutputUsd"))
        .map(|cap| {
            format!(
                "; cheaper models only (up to {} per million output tokens)",
                dollars(cap)
            )
        })
        .unwrap_or_default();
    format!("{left}{until}{cap}")
}

/// An ISO time to the minute, in UTC.
pub(crate) fn when(iso: &str) -> String {
    let minute: String = iso.chars().take(16).collect();
    format!("{} UTC", minute.replacen('T', " ", 1))
}

fn state_label(state: &str) -> &str {
    match state {
        "closed" => "Carried over",
        "invoicing" => "Preparing",
        "invoiced" => "Due",
        "paid" => "Paid",
        "failed" => "Payment failed",
        "void" => "Void",
        "uncollectible" => "Uncollectible",
        other => other,
    }
}

fn state_text(style: &Style, state: &str) -> String {
    let label = state_label(state);
    if state == "failed" || state == "uncollectible" {
        style.red(label)
    } else {
        label.to_string()
    }
}

fn line_label(code: &str) -> &str {
    match code {
        "gateway.calls" => "Gateway calls",
        "gateway.provider_cost" => "Fee on provider cost",
        "gateway.nexus_usage" => "Nexus usage",
        other => other,
    }
}

fn past_due_lines(style: &Style, past_due: &Value, invoice_url: Option<&str>) -> Vec<String> {
    let month = month_name(s(past_due, "period").unwrap_or(""));
    let line = match s(past_due, "blockedFrom") {
        None => format!(
            "{} The invoice for {month} is unpaid. Stripe is still trying to charge the card.",
            style.warn
        ),
        Some(from) if parse_ms(from).is_some_and(|at| at <= now_ms()) => format!(
            "{} The invoice for {month} is unpaid. Model calls stopped on {} and start again once it is paid.",
            style.fail,
            when(from)
        ),
        Some(from) => format!(
            "{} The invoice for {month} is unpaid. Model calls stop on {} unless it is paid.",
            style.warn,
            when(from)
        ),
    };
    let pay = match invoice_url {
        Some(url) => format!("Pay the invoice on Stripe: {}", style.cyan(url)),
        None => "Its invoice: nexus billing statements".to_string(),
    };
    vec![line, format!("  {} {pay}", style.arrow)]
}

/// Why this Gateway has no billing to show.
pub(crate) enum Off {
    /// 503 `not_configured`, with Gateway's message.
    NotConfigured(String),
    /// 404: a Gateway older than billing.
    NoBillingApi(String),
}

/// Billing off on this Gateway (503 `not_configured`) or no billing API (404): a note, not an error.
pub(crate) fn unavailable(error: &Error) -> Option<Off> {
    let Error::Core(error) = error else {
        return None;
    };
    match (error.status, error.code.as_deref()) {
        (503, Some("not_configured")) => Some(Off::NotConfigured(error.message.clone())),
        (404, _) => Some(Off::NoBillingApi(error.message.clone())),
        _ => None,
    }
}

pub(crate) const NO_BILLING_API: &str =
    "This Gateway has no billing API (the TypeScript gateway, or an older Rust one)";

/// A billing read: Gateway's answer, or a note when this Gateway has no billing.
fn read(cmd: &Cmd, path: &str) -> Result<Option<Value>> {
    answered(cmd, cmd.core.get(path))
}

/// A billing read's answer, or a note when this Gateway has no billing.
pub(crate) fn answered(cmd: &Cmd, answer: Result<Value>) -> Result<Option<Value>> {
    let error = match answer {
        Ok(answer) => return Ok(Some(answer)),
        Err(error) => error,
    };
    let Some(off) = unavailable(&error) else {
        return Err(owners_only(error));
    };
    let style = cmd.style;
    let (reason, message) = match &off {
        Off::NotConfigured(message) => ("not_configured", message.clone()),
        Off::NoBillingApi(message) => ("no_billing_api", message.clone()),
    };
    if cmd.json {
        cmd.print(&json!({ "enabled": false, "reason": reason, "message": message }));
        return Ok(None);
    }
    match off {
        Off::NotConfigured(message) => {
            cmd.out(format!(
                "{} Billing is off on this Gateway, so it charges nothing.",
                style.warn
            ));
            cmd.out(format!("  {}", style.dim(&message)));
        }
        Off::NoBillingApi(_) => cmd.out(format!("{} {NO_BILLING_API}, so it charges nothing.", style.warn)),
    }
    Ok(None)
}

/// A refusal for a member or an admin, with who can do it.
pub(crate) fn owners_only(error: Error) -> Error {
    match &error {
        Error::Core(core) if core.status == 403 => CliError::hint(
            core.message.clone(),
            "Billing is for owners: ask an owner of this organization (`nexus members` lists them).",
        )
        .into(),
        _ => error,
    }
}

fn show(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let Some(billing) = read(cmd, "/v1/billing")? else {
        return Ok(0);
    };
    if cmd.json {
        return Ok(cmd.print(&billing));
    }
    cmd.out(format!(
        "{} {} {}",
        style.bold("Billing"),
        style.bullet,
        if s(&billing, "mode") == Some("test") {
            "Stripe test mode: no real charges"
        } else {
            "Stripe live mode"
        }
    ));
    if let Some(past_due) = billing.get("pastDue").filter(|due| !due.is_null()) {
        let period = s(past_due, "period").unwrap_or("");
        let statements = cmd.core.get("/v1/billing/statements").ok();
        let invoice = statements.as_ref().and_then(|answer| {
            a(answer, "statements")
                .iter()
                .find(|entry| s(entry, "period") == Some(period))
                .and_then(|entry| s(entry, "hostedInvoiceUrl"))
        });
        cmd.out("");
        cmd.lines(past_due_lines(style, past_due, invoice));
    }
    let card = billing.get("paymentMethod").filter(|card| !card.is_null());
    let mut rows = Vec::new();
    // A plan shows only in a build with plans (`--features plans`); pricing v2.0 sells none.
    #[cfg(feature = "plans")]
    if let Some(plan) = billing.get("plan").filter(|plan| !plan.is_null()) {
        rows.push(kv("Plan", crate::plans::plan_text(style, plan)));
    }
    if let Some(allowance) = billing.get("planAllowance").filter(|value| value.is_object()) {
        if let Some(cloud) = allowance.get("cloud")
            && let (Some(used), Some(left)) = (cloud["usedHours"].as_f64(), cloud["leftHours"].as_f64())
        {
            rows.push(kv("Cloud hours", format!("{used:.2} h used · {left:.2} h left")));
        }
        if let Some(ai) = allowance.get("ai")
            && let (Some(used), Some(left)) = (s(ai, "usedUsd"), s(ai, "leftUsd"))
        {
            rows.push(kv(
                "AI allowance",
                format!("{} used · {} left", dollars(used), dollars(left)),
            ));
        }
        if let Some(reset) = s(allowance, "resetAt") {
            rows.push(kv("Allowance resets", when(reset)));
        }
    }
    rows.push(kv(
        "Card",
        match card {
            Some(card) => card_text(style, card),
            None => "none: add one to buy Nexus credits or to pay after the fact".into(),
        },
    ));
    if let Some(free) = billing.get("freeStart").filter(|free| !free.is_null()) {
        rows.push(kv(
            subsidy_label(&billing),
            free_start_text(free, billing.get("pricing")),
        ));
    }
    if let Some(amount) =
        s(&billing, "promotionalCreditUsd").filter(|amount| !amount.trim_matches(['0', '.']).is_empty())
    {
        rows.push(kv("Promotional credit", dollars(amount)));
    }
    if let Some(credits) = billing.get("credits").filter(|credits| !credits.is_null()) {
        rows.push(kv(
            "Nexus credits",
            format!(
                "{} left of {} bought",
                dollars(s(credits, "remainingUsd").unwrap_or("")),
                dollars(s(credits, "purchasedUsd").unwrap_or(""))
            ),
        ));
    }
    if let Some(topup) = billing.get("autoTopup").filter(|topup| !topup.is_null()) {
        rows.push(kv("Auto top-up", topup_summary(topup)));
    }
    // A Gateway older than pricing v2.0 sends no `afterTheFact`; `null` is off.
    if let Some(after) = billing.get("afterTheFact") {
        rows.push(kv(
            "After the fact",
            after_summary(style, after, min_final_charge(&billing)),
        ));
    }
    if let Some(bps) = billing
        .get("pricing")
        .and_then(|pricing| pricing.get("markupBps"))
        .and_then(Value::as_i64)
    {
        rows.push(kv(
            "Rate",
            format!(
                "the provider's price plus {} on Nexus-run AI; your own keys are free",
                percent(bps)
            ),
        ));
    }
    cmd.out("");
    cmd.lines(pairs(style, &rows));
    cmd.out("");
    cmd.out(style.dim(&format!(
        "{}: nexus billing card add. This month so far: nexus billing statement. Receipts: nexus billing receipts.",
        if card.is_some() {
            "Replace the card"
        } else {
            "Add a card"
        }
    )));
    Ok(0)
}

fn add_card(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let started = cmd
        .core
        .get("/v1/billing")
        .and_then(|before| Ok((before, cmd.core.post("/v1/billing/payment-method", json!({}))?)));
    let (before, link) = match started {
        Ok(started) => started,
        Err(error) => {
            // A card cannot be added here: the command fails, in plain words.
            return Err(match unavailable(&error) {
                Some(Off::NotConfigured(message)) => {
                    CliError::hint("Billing is off on this Gateway, so it takes no card.", message).into()
                }
                Some(Off::NoBillingApi(_)) => CliError::new(format!("{NO_BILLING_API}, so it takes no card.")).into(),
                None => owners_only(error),
            });
        }
    };
    let has_card = before.get("paymentMethod").is_some_and(|card| !card.is_null());
    let was = card_key(before.get("paymentMethod"));
    checkout(
        cmd,
        &link,
        if has_card { "Replace the card" } else { "Add a card" },
        &Wait {
            noun: "card",
            verb: ["save", "saves", "saved"],
            key: "paymentMethod",
            done: &|card| (card_key(Some(card)) != was).then(|| format!("Saved: {}", card_text(style, card))),
        },
    )
}

/// What `checkout` waits for: the noun, Stripe's verb for it, its key in `GET /v1/billing`, and the done line.
pub(crate) struct Wait<'a> {
    /// "card" or "plan".
    pub(crate) noun: &'a str,
    /// What Stripe does to it: the verb, its third person and its past participle.
    pub(crate) verb: [&'a str; 3],
    /// The key `GET /v1/billing` sends it under.
    pub(crate) key: &'a str,
    /// The line to print once Stripe is done with it, or `None` to keep waiting.
    pub(crate) done: &'a dyn Fn(&Value) -> Option<String>,
}

/// Stripe Checkout's link for `card add` and `plan`: print it, open it, then wait for Stripe's webhook unless
/// `--no-wait`. `action` starts the first line ("Add a card", "Start Pro (monthly)").
pub(crate) fn checkout(cmd: &Cmd, link: &Value, action: &str, wait: &Wait) -> Result<i32> {
    let style = cmd.style;
    let [verb, verbs, done] = wait.verb;
    let noun = wait.noun;
    let url = js_string(link.get("url"));
    let info = |line: String| {
        if cmd.json { cmd.ctx.io.err(&line) } else { cmd.out(line) }
    };
    info(format!(
        "{} {action} on Stripe's page: {}",
        style.info,
        style.cyan(&url)
    ));
    let opened = !cmd.has("no-browser") && cmd.ctx.io.open_url(&url);
    if cmd.has("no-wait") {
        let first = if opened {
            "Opened your browser".to_string()
        } else {
            format!("Open the link before {}", when(s(link, "expiresAt").unwrap_or("")))
        };
        info(format!(
            "  {}",
            style.dim(&format!(
                "{first}; nexus billing shows the {noun} once Stripe {verbs} it."
            ))
        ));
        return Ok(if cmd.json { cmd.print(link) } else { 0 });
    }
    let waiting = format!("Waiting for Stripe to {verb} the {noun}…");
    info(format!(
        "  {}",
        style.dim(&format!(
            "{}. {waiting}",
            if opened {
                "Opened your browser"
            } else {
                "Open the link in a browser"
            }
        ))
    ));
    let waited = (|| -> Result<Option<(Value, String)>> {
        for _ in 0..POLLS {
            cmd.ctx.io.progress(Some(&waiting));
            std::thread::sleep(Duration::from_millis(cmd.ctx.poll_ms.unwrap_or(POLL_MS)));
            let billing = match cmd.core.get("/v1/billing") {
                Ok(billing) => billing,
                // A dropped connection or a slow answer: poll again, so one reset does not end the wait.
                Err(Error::Transport(Transport::Unreachable(_) | Transport::Timeout)) => continue,
                Err(error) => return Err(error),
            };
            let line = billing
                .get(wait.key)
                .filter(|value| !value.is_null())
                .and_then(|value| (wait.done)(value));
            if let Some(line) = line {
                return Ok(Some((billing, line)));
            }
        }
        Ok(None)
    })();
    cmd.ctx.io.progress(None);
    let Some((billing, line)) = waited? else {
        return Err(CliError::hint(
            format!("Stripe has not {done} the {noun} yet."),
            format!("If you finished on Stripe's page, `nexus billing` shows the {noun} within a few minutes."),
        )
        .into());
    };
    if cmd.json {
        return Ok(cmd.print(&billing));
    }
    cmd.out(format!("{} {line}.", style.ok));
    Ok(0)
}

/// `confirm`: the bank's page for the charge it wants the cardholder to confirm, printed and opened like `portal`.
fn confirm_payment(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let answer = cmd
        .core
        .post("/v1/billing/confirm-payment", json!({}))
        .map_err(|error| match &error {
            Error::Core(core) if core.code.as_deref() == Some("nothing_to_confirm") => CliError::hint(
                "No charge waits for the bank's check.",
                "To replace the card: nexus billing card add",
            )
            .into(),
            _ => unusable(error, "confirms no payment"),
        })?;
    let info = |line: String| {
        if cmd.json { cmd.ctx.io.err(&line) } else { cmd.out(line) }
    };
    let url = s(&answer, "url").filter(|_| s(&answer, "status") == Some("redirect"));
    let Some(url) = url else {
        info(if s(&answer, "status") == Some("succeeded") {
            format!("{} The payment went through.", style.ok)
        } else {
            format!(
                "{} The payment is on its way: nexus billing shows it once Stripe takes it.",
                style.info
            )
        });
        return Ok(if cmd.json { cmd.print(&answer) } else { 0 });
    };
    let url = url.to_string();
    info(format!(
        "{} Approve the payment on your bank's page: {}",
        style.info,
        style.cyan(&url)
    ));
    let opened = !cmd.has("no-browser") && cmd.ctx.io.open_url(&url);
    info(format!(
        "  {}",
        style.dim(&format!(
            "{} nexus billing shows the payment once the bank approves it.",
            if opened {
                "Opened your browser."
            } else {
                "Open the link in a browser."
            }
        ))
    ));
    Ok(if cmd.json { cmd.print(&answer) } else { 0 })
}

/// `portal`: Stripe's customer portal for cards, invoices and tax ids.
fn portal(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let link = cmd
        .core
        .post("/v1/billing/portal", json!({}))
        .map_err(|error| unusable(error, "has no portal"))?;
    let url = js_string(link.get("url"));
    let info = |line: String| {
        if cmd.json { cmd.ctx.io.err(&line) } else { cmd.out(line) }
    };
    info(format!("{} Stripe's billing portal: {}", style.info, style.cyan(&url)));
    let opened = !cmd.has("no-browser") && cmd.ctx.io.open_url(&url);
    info(format!(
        "  {}",
        style.dim(if opened {
            "Opened your browser. The link expires within minutes."
        } else {
            "Open the link in a browser now: it expires within minutes."
        })
    ));
    Ok(if cmd.json { cmd.print(&link) } else { 0 })
}

/// A write this Gateway cannot take: the command fails, in plain words (`what` it cannot do, such as "sells no credits").
fn unusable(error: Error, what: &str) -> Error {
    match unavailable(&error) {
        Some(Off::NotConfigured(message)) => {
            CliError::hint(format!("Billing is off on this Gateway, so it {what}."), message).into()
        }
        Some(Off::NoBillingApi(_)) => CliError::new(format!("{NO_BILLING_API}, so it {what}.")).into(),
        None => owners_only(error),
    }
}

/// The payment method a purchase is charged to, as people say it.
fn method_text(method: &Value) -> String {
    let last4 = js_string(method.get("last4"));
    match s(method, "type") {
        Some("card") => format!("{} ending in {last4}", brand_name(s(method, "brand").unwrap_or(""))),
        Some("us_bank_account") => format!("{} account ending in {last4}", s(method, "bankName").unwrap_or("Bank")),
        Some(other) => other.replace('_', " "),
        None => "the payment method on file".into(),
    }
}

/// `rate` is Gateway's `pricing.markupBps`, for the line before any credits are bought.
fn credits_lines(style: &Style, credits: Option<&Value>, rate: Option<i64>) -> Vec<String> {
    let Some(credits) = credits.filter(|credits| !credits.is_null()) else {
        return vec![
            format!(
                "No Nexus credits yet. Nexus-run AI spends them after the free start, at the provider's price plus {}.",
                rate.map(percent).unwrap_or_else(|| "Nexus's rate".into())
            ),
            String::new(),
            style.dim(&format!(
                "Buy some: nexus billing credits buy <usd> (from {}).",
                dollars(MIN_PURCHASE_USD)
            )),
        ];
    };
    let money = |key: &str| dollars(s(credits, key).unwrap_or(""));
    let rate = credits.get("markupBps").and_then(Value::as_i64).map(percent);
    let mut lines = vec![
        format!(
            "{} {} the provider's price plus {}",
            style.bold("Nexus credits"),
            style.bullet,
            rate.unwrap_or_else(|| "Nexus's rate".into())
        ),
        String::new(),
    ];
    lines.extend(pairs(
        style,
        &[
            kv("Left", money("remainingUsd")),
            kv("Bought", money("purchasedUsd")),
            kv("Spent", money("spentUsd")),
            kv(
                "Held",
                format!("{}  {}", money("reservedUsd"), style.dim("calls running now")),
            ),
        ],
    ));
    lines.push(String::new());
    lines.push(style.dim(&format!(
        "Credits never expire. Buy more: nexus billing credits buy <usd> (from {}).",
        dollars(MIN_PURCHASE_USD)
    )));
    lines
}

/// `credits`: the organization's Nexus credits.
fn credits(cmd: &Cmd) -> Result<i32> {
    let Some(billing) = read(cmd, "/v1/billing")? else {
        return Ok(0);
    };
    let credits = billing.get("credits").cloned().unwrap_or(Value::Null);
    if cmd.json {
        return Ok(cmd.print(&json!({ "credits": credits })));
    }
    let rate = billing
        .get("pricing")
        .and_then(|pricing| pricing.get("markupBps"))
        .and_then(Value::as_i64);
    cmd.lines(credits_lines(cmd.style, Some(&credits), rate));
    Ok(0)
}

/// `"10"` or `"10.50"`: dollars with at most two decimals, as Gateway takes them.
fn is_usd(text: &str) -> bool {
    let (whole, cents) = text.split_once('.').unwrap_or((text, ""));
    let digits = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
    digits(whole) && (!text.contains('.') || digits(cents) && cents.len() <= 2)
}

/// `credits buy <usd>`: charged to the payment method on file, else on Stripe Checkout, then wait until Stripe
/// credits it.
fn buy_credits(cmd: &Cmd, amount: Option<&str>) -> Result<i32> {
    let style = cmd.style;
    let Some(amount) = amount.filter(|amount| is_usd(amount)) else {
        return usage(
            "Give the amount in dollars, e.g. 10 or 40.50.",
            Some("Usage: nexus billing credits buy <usd> [--dry-run] [--no-browser] [--no-wait]"),
        );
    };
    let refused = |error: Error| unusable(error, "sells no credits");
    let before = cmd.core.get("/v1/billing").map_err(refused)?;
    let method = before.get("defaultPaymentMethod").filter(|method| !method.is_null());
    let delegated = cmd.has("payment-token-stdin");
    if delegated && before["sharedPaymentTokens"] != true {
        return Err(CliError::hint(
            "This Gateway does not support delegated credit purchases.",
            "Upgrade Gateway before sending a payment token; no purchase was submitted.",
        )
        .into());
    }
    let usd = dollars(amount);
    if cmd.has("dry-run") {
        if cmd.json {
            return Ok(cmd.print(&json!({
                "dryRun": true,
                "amountUsd": amount,
                "pays": if delegated { "delegated_token" } else if method.is_some() { "payment_method" } else { "checkout" },
            })));
        }
        cmd.out(if delegated {
            format!(
                "{} Would buy {usd} of Nexus credits with a delegated payment token.",
                style.info
            )
        } else {
            match method {
                Some(method) => format!(
                    "{} Would buy {usd} of Nexus credits, charged to {}.",
                    style.info,
                    method_text(method)
                ),
                None => format!(
                    "{} Would buy {usd} of Nexus credits on Stripe Checkout: no payment method is on file.",
                    style.info
                ),
            }
        });
        cmd.out(format!(
            "  {}",
            style.dim("Nothing is charged without dropping --dry-run.")
        ));
        return Ok(0);
    }
    let mut body = json!({ "amountUsd": amount });
    if delegated {
        let token = cmd.ctx.io.read_stdin().trim().to_string();
        if token.len() > 255
            || !token
                .strip_prefix("spt_")
                .is_some_and(|tail| !tail.is_empty() && tail.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
        {
            return usage(
                "A Stripe shared payment token is required on stdin.",
                Some(
                    "Pipe the agent wallet's spt_ credential to --payment-token-stdin; never pass a card number or a merchant key.",
                ),
            );
        }
        body["paymentToken"] = json!(token);
    }
    let bought = cmd.core.post("/v1/billing/credits", body).map_err(refused)?;
    let info = |line: String| {
        if cmd.json { cmd.ctx.io.err(&line) } else { cmd.out(line) }
    };
    match s(&bought, "status") {
        Some("credited") => {
            if cmd.json {
                return Ok(cmd.print(&bought));
            }
            cmd.out(format!(
                "{} Bought {usd} of Nexus credits. {} left.",
                style.ok,
                dollars(s(&bought["credits"], "remainingUsd").unwrap_or(""))
            ));
            return Ok(0);
        }
        Some("processing") => {
            info(format!(
                "{} Stripe is taking the payment of {usd}. `nexus billing credits` shows the credits once it clears.",
                style.info
            ));
            return Ok(if cmd.json { cmd.print(&bought) } else { 0 });
        }
        _ => {}
    }
    if delegated {
        return Err(CliError::hint("This Gateway did not accept the delegated payment token.", "Upgrade Gateway to a version with delegated credit purchases; no browser payment was started by this command.").into());
    }
    let url = js_string(bought.get("url"));
    info(format!(
        "{} Pay {usd} for Nexus credits on Stripe's page: {}",
        style.info,
        style.cyan(&url)
    ));
    let opened = !cmd.has("no-browser") && cmd.ctx.io.open_url(&url);
    if cmd.has("no-wait") {
        let first = if opened {
            "Opened your browser".to_string()
        } else {
            format!("Open the link before {}", when(s(&bought, "expiresAt").unwrap_or("")))
        };
        info(format!(
            "  {}",
            style.dim(&format!(
                "{first}; nexus billing credits shows the credits once Stripe takes the payment."
            ))
        ));
        return Ok(if cmd.json { cmd.print(&bought) } else { 0 });
    }
    info(format!(
        "  {}",
        style.dim(&format!(
            "{}. Waiting for Stripe to take the payment…",
            if opened {
                "Opened your browser"
            } else {
                "Open the link in a browser"
            }
        ))
    ));
    let purchased = |billing: &Value| {
        billing
            .get("credits")
            .and_then(|credits| s(credits, "purchasedUsd"))
            .map(String::from)
    };
    let was = purchased(&before);
    let waited = (|| -> Result<Option<Value>> {
        for _ in 0..POLLS {
            cmd.ctx.io.progress(Some("Waiting for Stripe to take the payment…"));
            std::thread::sleep(Duration::from_millis(cmd.ctx.poll_ms.unwrap_or(POLL_MS)));
            let billing = match cmd.core.get("/v1/billing") {
                Ok(billing) => billing,
                // A dropped connection or a slow answer: poll again, so one reset does not end the wait.
                Err(Error::Transport(Transport::Unreachable(_) | Transport::Timeout)) => continue,
                Err(error) => return Err(error),
            };
            if purchased(&billing).is_none() || purchased(&billing) == was {
                continue;
            }
            return Ok(Some(billing));
        }
        Ok(None)
    })();
    cmd.ctx.io.progress(None);
    let Some(billing) = waited? else {
        return Err(CliError::hint(
            "Stripe has not sent the payment yet.",
            "If you paid on Stripe's page, `nexus billing credits` shows the credits within a few minutes.",
        )
        .into());
    };
    let credits = billing["credits"].clone();
    if cmd.json {
        return Ok(cmd.print(&json!({ "status": "credited", "amountUsd": amount, "credits": credits })));
    }
    cmd.out(format!(
        "{} Bought {usd} of Nexus credits. {} left.",
        style.ok,
        dollars(s(&credits, "remainingUsd").unwrap_or(""))
    ));
    Ok(0)
}

/// Why Nexus turned auto top-up off, as kit's `TOPUP_OFF_REASON` says it; a test checks the line it makes
/// against `kit/src/fixtures/billing-words.json`.
fn off_reason_text(reason: &str) -> &str {
    match reason {
        "closed" => "the organization was closed",
        "declined" => "the card was declined",
        "authentication_required" => "the bank wanted the cardholder to confirm the payment",
        "payment_failed" => "a bank payment failed",
        "no_payment_method" => "no payment method is on file",
        "disputed" => "a payment on the card was disputed",
        "refunded" => "a credit purchase was refunded",
        other => other,
    }
}

/// "Auto top-up is off: the card was declined" (kit's `topupOffText`).
fn topup_off_text(reason: &str) -> String {
    format!("Auto top-up is off: {}", off_reason_text(reason))
}

/// Auto top-up in one line: "on: adds $25.00 below $10.00", or "off".
fn topup_summary(topup: &Value) -> String {
    if topup.get("enabled").and_then(Value::as_bool) == Some(true) {
        format!(
            "on: adds {} when credits fall below {}",
            dollars(s(topup, "addUsd").unwrap_or("")),
            dollars(s(topup, "belowUsd").unwrap_or(""))
        )
    } else {
        match s(topup, "offReason") {
            Some(reason) => format!("off: {}", off_reason_text(reason)),
            None => "off".into(),
        }
    }
}

fn topup_lines(style: &Style, topup: Option<&Value>) -> Vec<String> {
    let set_hint = "nexus billing auto-topup set --below <usd> --add <usd>";
    let Some(topup) = topup.filter(|topup| !topup.is_null()) else {
        return vec![
            "Auto top-up is not set. It buys Nexus credits with the payment method on file when they run low.".into(),
            String::new(),
            style.dim(&format!("Set it: {set_hint}")),
        ];
    };
    let on = topup.get("enabled").and_then(Value::as_bool) == Some(true);
    let mut lines = vec![
        format!(
            "{} {} {}",
            style.bold("Auto top-up"),
            style.bullet,
            if on { "on" } else { "off" }
        ),
        String::new(),
    ];
    if let Some(reason) = s(topup, "offReason") {
        lines.push(format!("{} {}.", style.warn, topup_off_text(reason)));
        lines.push(String::new());
    }
    lines.extend(pairs(
        style,
        &[
            kv("Below", dollars(s(topup, "belowUsd").unwrap_or(""))),
            kv(
                "Adds",
                format!(
                    "{}  {}",
                    dollars(s(topup, "addUsd").unwrap_or("")),
                    style.dim("charged to the payment method on file")
                ),
            ),
            kv(
                "Today",
                format!(
                    "{} of 3 top-ups, {} of $500.00",
                    js_string(topup.get("todayCount")),
                    dollars(s(topup, "todayUsd").unwrap_or(""))
                ),
            ),
        ],
    ));
    lines.push(String::new());
    lines.push(style.dim(&if on {
        "Turn it off: nexus billing auto-topup off".to_string()
    } else {
        format!("Turn it on: {set_hint}")
    }));
    lines
}

/// `auto-topup [set --below <usd> --add <usd> | off]`: show, set or turn off the auto top-up of Nexus credits.
fn auto_topup(cmd: &Cmd, verb: Option<&str>) -> Result<i32> {
    let style = cmd.style;
    let body = match verb {
        None => {
            let Some(billing) = read(cmd, "/v1/billing")? else {
                return Ok(0);
            };
            let topup = billing.get("autoTopup").cloned().unwrap_or(Value::Null);
            if cmd.json {
                return Ok(cmd.print(&json!({ "autoTopup": topup })));
            }
            cmd.lines(topup_lines(style, Some(&topup)));
            return Ok(0);
        }
        Some("off") => json!({ "enabled": false }),
        Some("set") => {
            let (below, add) = (cmd.flag("below"), cmd.flag("add"));
            if below.is_some_and(|below| !is_usd(below)) || add.is_some_and(|add| !is_usd(add)) {
                return usage(
                    "Give --below and --add in dollars, e.g. --below 10 --add 50.",
                    Some("Usage: nexus billing auto-topup set --below <usd> --add <usd> [--dry-run]"),
                );
            }
            let mut body = json!({ "enabled": true });
            if let Some(below) = below {
                body["belowUsd"] = json!(below);
            }
            if let Some(add) = add {
                body["addUsd"] = json!(add);
            }
            body
        }
        Some(_) => {
            return usage(
                "Usage: nexus billing auto-topup [set --below <usd> --add <usd> | off]",
                Some("Auto top-up buys Nexus credits with the payment method on file when they fall below --below."),
            );
        }
    };
    if cmd.has("dry-run") {
        if cmd.json {
            return Ok(cmd.print(&json!({ "dryRun": true, "request": body })));
        }
        let what = match (s(&body, "belowUsd"), s(&body, "addUsd")) {
            _ if body["enabled"] == json!(false) => "turn auto top-up off".to_string(),
            (Some(below), Some(add)) => format!(
                "turn auto top-up on: add {} when credits fall below {}",
                dollars(add),
                dollars(below)
            ),
            _ => "turn auto top-up on with the amounts set before".to_string(),
        };
        cmd.out(format!("{} Would {what}.", style.info));
        cmd.out(format!(
            "  {}",
            style.dim("Nothing changes without dropping --dry-run.")
        ));
        return Ok(0);
    }
    let answer = cmd
        .core
        .put("/v1/billing/auto-topup", body)
        .map_err(|error| unusable(error, "has no auto top-up"))?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    cmd.out(match answer.get("autoTopup").filter(|topup| !topup.is_null()) {
        Some(topup) if topup.get("enabled").and_then(Value::as_bool) == Some(true) => format!(
            "{} Auto top-up is on: Nexus adds {} when credits fall below {}.",
            style.ok,
            dollars(s(topup, "addUsd").unwrap_or("")),
            dollars(s(topup, "belowUsd").unwrap_or(""))
        ),
        Some(_) => format!("{} Auto top-up is off.", style.ok),
        None => "Auto top-up was not set, so nothing changed.".to_string(),
    });
    Ok(0)
}

/// A day from an ISO time: "2026-11-17".
pub(crate) fn day(iso: &str) -> &str {
    iso.get(..10).unwrap_or(iso)
}

/// The least final charge (`pricing.minFinalChargeUsd`); an older Gateway sends none, and its minimum was $0.50.
fn min_final_charge(billing: &Value) -> &str {
    billing
        .get("pricing")
        .and_then(|pricing| s(pricing, "minFinalChargeUsd"))
        .unwrap_or("0.50")
}

/// Paying after the fact in one line: "on: up to $25.00 a month; $3.20 used this month, $1.10 owed", or "off", then
/// the charge phase's words when there are some. Owed is the unpaid usage, a running charge included.
fn after_summary(style: &Style, after: &Value, min_final: &str) -> String {
    if after.is_null() {
        return "off".into();
    }
    let money = |key: &str| dollars(s(after, key).unwrap_or(""));
    let text = if after.get("enabled").and_then(Value::as_bool) == Some(true) {
        format!(
            "on: up to {} a month; {} used this month, {} owed",
            money("limitUsd"),
            money("monthSpentUsd"),
            dollars(after_the_fact::unpaid_usd(after))
        )
    } else {
        "off".to_string()
    };
    match after_the_fact::notice(after, min_final) {
        Some(notice) if notice.phase.charge_problem() => format!("{text}. {}", style.red(&notice.title)),
        Some(notice) => format!("{text}. {}", notice.title),
        None => text,
    }
}

/// The charge phase's line in `after-the-fact`: what happened, when a charge failed, what stops, and the owner's way on.
fn after_notice_lines(style: &Style, after: &Value, min_final: &str) -> Vec<String> {
    let Some(notice) = after_the_fact::notice(after, min_final) else {
        return Vec::new();
    };
    let mark = if notice.stops_calls {
        &style.fail
    } else if notice.phase.charge_problem() {
        &style.warn
    } else {
        &style.info
    };
    let at = after
        .get("failed")
        .and_then(|failed| s(failed, "at"))
        .filter(|_| notice.phase.charge_problem())
        .map(|at| format!(" ({})", when(at)))
        .unwrap_or_default();
    let body = notice.body.map(|body| format!(" {body}.")).unwrap_or_default();
    let mut lines = vec![format!("{mark} {}{at}.{body}", notice.title)];
    if let Some(action) = notice.action {
        lines.push(style.dim(&format!("{}: {}", action.label(), action.command())));
    }
    lines.push(String::new());
    lines
}

const AFTER_ON_HINT: &str = "nexus billing after-the-fact on [--limit <usd>]";

fn after_lines(style: &Style, after: Option<&Value>, min_final: &str) -> Vec<String> {
    let Some(after) = after.filter(|after| !after.is_null()) else {
        return vec![
            "Paying after the fact is off. The card on file can pay for Nexus-run AI once the free start and the credits run out, up to a monthly usage limit.".into(),
            String::new(),
            style.dim(&format!("Turn it on: {AFTER_ON_HINT}")),
        ];
    };
    let on = after.get("enabled").and_then(Value::as_bool) == Some(true);
    let money = |key: &str| dollars(s(after, key).unwrap_or(""));
    let mut lines = vec![
        format!(
            "{} {} {}",
            style.bold("Paying after the fact"),
            style.bullet,
            if on { "on" } else { "off" }
        ),
        String::new(),
    ];
    lines.extend(after_notice_lines(style, after, min_final));
    let owed_note = if after.get("pending").and_then(Value::as_bool) == Some(true) {
        "a charge is being taken now".to_string()
    } else {
        format!("charged when it reaches {}; less carries over", money("thresholdUsd"))
    };
    lines.extend(pairs(
        style,
        &[
            kv("Limit", format!("{} a month", money("limitUsd"))),
            kv("Used", format!("{} this month", money("monthSpentUsd"))),
            kv(
                "Owed",
                format!(
                    "{}  {}",
                    dollars(after_the_fact::unpaid_usd(after)),
                    style.dim(&owed_note)
                ),
            ),
            kv(
                "Held",
                format!("{}  {}", money("reservedUsd"), style.dim("calls running now")),
            ),
            kv("Left", format!("{} this month", money("leftUsd"))),
        ],
    ));
    lines.push(String::new());
    lines.push(style.dim(&if on {
        "Change the limit: nexus billing after-the-fact limit <usd>. Turn it off: nexus billing after-the-fact off"
            .to_string()
    } else {
        format!("Turn it on: {AFTER_ON_HINT}")
    }));
    lines
}

const AFTER_USAGE: &str = "Usage: nexus billing after-the-fact [on [--limit <usd>] | off | limit <usd>] [--dry-run]";

/// `after-the-fact [on [--limit <usd>] | off | limit <usd>]`: show paying after the fact, turn it on or off, or set
/// its monthly usage limit. `limit` changes only the limit: `on` is the only way to turn it on.
fn after_the_fact(cmd: &Cmd, verb: Option<&str>, amount: Option<&str>) -> Result<i32> {
    let style = cmd.style;
    let body = match verb {
        None => {
            let Some(billing) = read(cmd, "/v1/billing")? else {
                return Ok(0);
            };
            let after = billing.get("afterTheFact").cloned().unwrap_or(Value::Null);
            if cmd.json {
                return Ok(cmd.print(&json!({ "afterTheFact": after })));
            }
            cmd.lines(after_lines(style, Some(&after), min_final_charge(&billing)));
            return Ok(0);
        }
        Some("off") => json!({ "enabled": false }),
        Some("on") => {
            let limit = cmd.flag("limit");
            // A bare `--limit` parses as a switch: refuse it rather than keep the limit set before.
            if (limit.is_none() && cmd.has("limit")) || limit.is_some_and(|limit| !is_usd(limit)) {
                return usage("Give --limit in dollars, e.g. --limit 25.", Some(AFTER_USAGE));
            }
            let mut body = json!({ "enabled": true });
            if let Some(limit) = limit {
                body["limitUsd"] = json!(limit);
            }
            body
        }
        Some("limit") => {
            let Some(limit) = amount.filter(|amount| is_usd(amount)) else {
                return usage("Give the limit in dollars, e.g. 25 or 40.50.", Some(AFTER_USAGE));
            };
            // The setting keeps its value: off, or no setting yet, stays off.
            let Some(billing) = read(cmd, "/v1/billing")? else {
                return Ok(0);
            };
            let enabled = billing
                .get("afterTheFact")
                .and_then(|after| after.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            json!({ "enabled": enabled, "limitUsd": limit })
        }
        Some(_) => {
            return usage(
                AFTER_USAGE,
                Some(
                    "The card on file pays for Nexus-run AI after the free start and the credits, up to a monthly usage limit.",
                ),
            );
        }
    };
    if cmd.has("dry-run") {
        if cmd.json {
            return Ok(cmd.print(&json!({ "dryRun": true, "request": body })));
        }
        let on = body["enabled"] == json!(true);
        let what = match s(&body, "limitUsd") {
            Some(limit) if verb == Some("limit") => format!(
                "set the usage limit to {} a month, and leave paying after the fact {}",
                dollars(limit),
                if on { "on" } else { "off" }
            ),
            _ if !on => "turn paying after the fact off".to_string(),
            Some(limit) => format!("turn paying after the fact on, up to {} a month", dollars(limit)),
            None => "turn paying after the fact on with the limit set before".to_string(),
        };
        cmd.out(format!("{} Would {what}.", style.info));
        cmd.out(format!(
            "  {}",
            style.dim("Nothing changes without dropping --dry-run.")
        ));
        return Ok(0);
    }
    let answer = cmd
        .core
        .put("/v1/billing/after-the-fact", body)
        .map_err(|error| match &error {
            Error::Core(core) if core.code.as_deref() == Some("no_payment_method") => {
                CliError::hint(core.message.clone(), "Add a card first: nexus billing card add").into()
            }
            Error::Core(core) if core.code.as_deref() == Some("card_disputed") => {
                CliError::hint(core.message.clone(), "Add another card first: nexus billing card add").into()
            }
            _ => unusable(error, "takes no payment after the fact"),
        })?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    let after = answer.get("afterTheFact").filter(|after| !after.is_null());
    let on = after.and_then(|after| after.get("enabled")).and_then(Value::as_bool) == Some(true);
    if verb == Some("limit") && !on {
        // The limit changed and the setting stayed off: say so, and how to turn it on.
        let limit = after.and_then(|after| s(after, "limitUsd")).or(amount).unwrap_or("");
        cmd.out(format!(
            "{} The usage limit is {} a month. Paying after the fact stays off. Turn it on: nexus billing after-the-fact on",
            style.ok,
            dollars(limit)
        ));
        return Ok(0);
    }
    let reply = match after {
        Some(after) if after.get("enabled").and_then(Value::as_bool) == Some(true) => {
            let limit = dollars(s(after, "limitUsd").unwrap_or(""));
            // On, but a failed charge stops calls on the card: never claim it pays.
            if let Some(failed) = after.get("failed").filter(|failed| !failed.is_null()) {
                after_failed_text(style, &limit, s(failed, "reason").unwrap_or(""))
            } else {
                format!(
                    "{} Paying after the fact is on: the card on file pays up to {limit} a month.",
                    style.ok
                )
            }
        }
        Some(_) => format!("{} Paying after the fact is off.", style.ok),
        None => "Paying after the fact was not on, so nothing changed.".to_string(),
    };
    cmd.out(reply);
    Ok(0)
}

/// The reply when paying after the fact is on but the last charge failed. A disputed card needs another card; any
/// other failure clears once a charge goes through.
fn after_failed_text(style: &Style, limit: &str, reason: &str) -> String {
    if reason == "disputed" {
        format!(
            "{} Paying after the fact is on, up to {limit} a month. A payment on the card on file was disputed. Calls on it stop until you add another card: nexus billing card add.",
            style.warn
        )
    } else {
        // No reason, or one this build does not know: no empty ": ." after "failed".
        let said = after_the_fact::charge_failed_reason(reason)
            .map(|said| format!(": {said}"))
            .unwrap_or_default();
        format!(
            "{} Paying after the fact is on, up to {limit} a month. The last charge failed{said}. Calls on the card stop until a charge goes through.",
            style.warn
        )
    }
}

fn receipt_status(style: &Style, status: &str) -> String {
    match status {
        "paid" => "Paid".into(),
        "open" => "Open".into(),
        "failed" => style.red("Failed"),
        "void" => "Void".into(),
        other => other.to_string(),
    }
}

/// `receipts`: every payment, newest first.
fn receipts(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let Some(answer) = read(cmd, "/v1/billing/receipts")? else {
        return Ok(0);
    };
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    let entries = a(&answer, "receipts");
    if entries.is_empty() {
        cmd.out("No payment yet. Buying credits, a top-up and a charge after the fact each leave a receipt here.");
        return Ok(0);
    }
    let rows: Vec<Row> = entries
        .iter()
        .map(|entry| {
            Row::from(vec![
                day(s(entry, "at").unwrap_or("")).to_string(),
                s(entry, "description").unwrap_or("").to_string(),
                dollars(s(entry, "amountUsd").unwrap_or("")),
                receipt_status(style, s(entry, "status").unwrap_or("")),
                s(entry, "id").unwrap_or("").to_string(),
            ])
        })
        .collect();
    cmd.lines(list(
        style,
        &[
            col("Date"),
            col("Description"),
            right("Amount"),
            col("Status"),
            flex("Payment"),
        ],
        rows,
        None,
    ));
    cmd.out("");
    cmd.out(style.dim("Stripe's receipt of one: nexus billing receipts open <payment>"));
    Ok(0)
}

/// `receipts open <paymentId>`: Stripe's receipt page of one payment, printed and opened like `portal`.
fn open_receipt(cmd: &Cmd, id: Option<&str>) -> Result<i32> {
    let style = cmd.style;
    let Some(id) = id.filter(|id| !id.is_empty()) else {
        return usage(
            "Usage: nexus billing receipts open <paymentId> [--no-browser]",
            Some("`nexus billing receipts` lists the payments and their ids."),
        );
    };
    let link = cmd
        .core
        .get(&format!("/v1/billing/receipts/{}", encode_segment(id)))
        .map_err(|error| unusable(error, "has no receipts"))?;
    let url = js_string(link.get("url"));
    let info = |line: String| {
        if cmd.json { cmd.ctx.io.err(&line) } else { cmd.out(line) }
    };
    info(format!("{} Stripe's receipt: {}", style.info, style.cyan(&url)));
    let opened = !cmd.has("no-browser") && cmd.ctx.io.open_url(&url);
    info(format!(
        "  {}",
        style.dim(if opened {
            "Opened your browser."
        } else {
            "Open the link in a browser."
        })
    ));
    Ok(if cmd.json { cmd.print(&link) } else { 0 })
}

fn statement_lines(style: &Style, statement: &Value) -> Vec<String> {
    let invoice = statement.get("invoice").filter(|invoice| !invoice.is_null());
    let mut lines = vec![
        format!(
            "{} {} {} (UTC){}",
            style.bold("Statement"),
            style.bullet,
            month_name(s(statement, "period").unwrap_or("")),
            match invoice {
                Some(invoice) => format!(
                    " {} {}",
                    style.bullet,
                    state_text(style, s(invoice, "state").unwrap_or(""))
                ),
                None => String::new(),
            }
        ),
        String::new(),
    ];
    let money = |value: &Value, key: &str| dollars(s(value, key).unwrap_or(""));
    let rows: Vec<Row> = a(statement, "lines")
        .iter()
        .map(|line| {
            let usd = s(line, "unit") == Some("usd");
            let rate = if usd {
                line.get("rateBps")
                    .and_then(Value::as_i64)
                    .map(percent)
                    .unwrap_or_else(|| "–".into())
            } else {
                s(line, "rateUsd").map(dollars).unwrap_or_else(|| "–".into())
            };
            Row::from(vec![
                line_label(s(line, "code").unwrap_or("")).to_string(),
                if usd {
                    money(line, "quantity")
                } else {
                    js_string(line.get("quantity"))
                },
                rate,
                money(line, "amountUsd"),
            ])
        })
        .collect();
    lines.extend(list(
        style,
        &[col("Charge"), right("Quantity"), right("Rate"), right("Amount")],
        rows,
        None,
    ));
    lines.push(String::new());
    let mut entries = vec![
        kv("Carried in", money(statement, "carriedInUsd")),
        kv("Total", money(statement, "totalUsd")),
        kv("To charge", cents(s(statement, "amountCents").unwrap_or(""))),
        kv("Carries over", money(statement, "carriedOutUsd")),
        kv(
            "Provider cost",
            format!(
                "{}  {}",
                money(statement, "providerCostUsd"),
                style.dim("billed by the providers, not by Nexus")
            ),
        ),
    ];
    if let Some(invoice) = invoice {
        entries.push(kv(
            "Invoice",
            match s(invoice, "hostedInvoiceUrl") {
                Some(url) => style.cyan(url),
                None => style.dim("not on Stripe yet"),
            },
        ));
    }
    lines.extend(pairs(style, &entries));
    lines.push(String::new());
    lines.push(style.dim("Under $0.50 is not invoiced: it carries over to the next month, with what is below a cent."));
    lines
}

/// `YYYY-MM` with a real month.
fn is_period(text: &str) -> bool {
    let Some((year, month)) = text.split_once('-') else {
        return false;
    };
    year.len() == 4
        && month.len() == 2
        && year.chars().chain(month.chars()).all(|c| c.is_ascii_digit())
        && matches!(month.parse::<u8>(), Ok(1..=12))
}

fn statement(cmd: &Cmd) -> Result<i32> {
    let period = cmd.flag("period");
    if let Some(period) = period
        && !is_period(period)
    {
        return usage(
            "Give the month as YYYY-MM, e.g. 2026-09.",
            Some("Usage: nexus billing statement [--period YYYY-MM]"),
        );
    }
    let path = format!(
        "/v1/billing/statement{}",
        search(&[("period", period.map(String::from))])
    );
    let Some(answer) = read(cmd, &path)? else {
        return Ok(0);
    };
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    cmd.lines(statement_lines(cmd.style, &answer));
    Ok(0)
}

fn statements(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let Some(answer) = read(cmd, "/v1/billing/statements")? else {
        return Ok(0);
    };
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    let entries = a(&answer, "statements");
    if entries.is_empty() {
        cmd.out(
            "No month has closed yet. On a contract, a month closes an hour after it ends (UTC); then Stripe charges the card on file.",
        );
        return Ok(0);
    }
    let rows: Vec<Row> = entries
        .iter()
        .map(|entry| {
            Row::from(vec![
                month_name(s(entry, "period").unwrap_or("")),
                dollars(s(entry, "totalUsd").unwrap_or("")),
                state_text(style, s(entry, "state").unwrap_or("")),
                s(entry, "hostedInvoiceUrl").unwrap_or("").to_string(),
            ])
        })
        .collect();
    cmd.lines(list(
        style,
        &[col("Month"), right("Total"), col("State"), flex("Invoice")],
        rows,
        None,
    ));
    cmd.out("");
    cmd.out(style.dim("One month in full: nexus billing statement --period YYYY-MM"));
    Ok(0)
}

pub fn billing_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let action = args.first().map(String::as_str).unwrap_or("show");
    if action == "help" || cmd.has("help") {
        if cmd.json {
            return Ok(cmd.print(&json!({ "commands": command_rows("billing", &billing_commands()) })));
        }
        cmd.lines(billing_help(cmd.style));
        return Ok(0);
    }
    match action {
        "show" => show(cmd),
        "card" => {
            if args.get(1).map(String::as_str) != Some("add") {
                return usage(
                    "Usage: nexus billing card add [--no-browser] [--no-wait]",
                    Some("It opens Stripe Checkout to add or replace the card."),
                );
            }
            add_card(cmd)
        }
        #[cfg(feature = "plans")]
        "plan" => crate::plans::plan_command(&args[1..], cmd),
        #[cfg(feature = "plans")]
        "plans" => crate::plans::plans(cmd),
        "statement" => statement(cmd),
        "statements" => statements(cmd),
        "receipts" => match args.get(1).map(String::as_str) {
            None => receipts(cmd),
            Some("open") => open_receipt(cmd, args.get(2).map(String::as_str)),
            Some(_) => usage(
                "Usage: nexus billing receipts [open <paymentId>]",
                Some("`receipts` lists the payments; `receipts open <paymentId>` opens Stripe's receipt of one."),
            ),
        },
        "portal" => portal(cmd),
        "confirm" => confirm_payment(cmd),
        "auto-topup" => auto_topup(cmd, args.get(1).map(String::as_str)),
        "after-the-fact" => after_the_fact(cmd, args.get(1).map(String::as_str), args.get(2).map(String::as_str)),
        "credits" => match args.get(1).map(String::as_str) {
            None => credits(cmd),
            Some("buy") => buy_credits(cmd, args.get(2).map(String::as_str)),
            Some(_) => usage(
                "Usage: nexus billing credits [buy <usd>]",
                Some("`credits` shows the Nexus credits; `credits buy 25` buys $25 of them."),
            ),
        },
        other => usage(
            format!("Unknown command `nexus billing {other}`."),
            Some("Run `nexus billing --help`."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Why Nexus turned auto top-up off, one way for each reason, as every app says it.
    #[test]
    fn auto_topup_off_says_the_glossary_words() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../packages/kit/src/fixtures/billing-words.json"))
                .expect("fixture");
        let off = fixture["topupOff"].as_object().expect("topupOff");
        assert_eq!(off.len(), 7);
        for (reason, text) in off {
            assert_eq!(text.as_str(), Some(topup_off_text(reason).as_str()), "{reason}");
        }
    }

    #[test]
    fn each_statement_line_has_a_name_and_an_unknown_one_shows_its_code() {
        assert_eq!(line_label("gateway.calls"), "Gateway calls");
        assert_eq!(line_label("gateway.provider_cost"), "Fee on provider cost");
        // A contract's calls on Nexus-supplied keys.
        assert_eq!(line_label("gateway.nexus_usage"), "Nexus usage");
        assert_eq!(line_label("gateway.later"), "gateway.later");
    }

    #[test]
    fn prints_money_as_gateway_sends_it() {
        assert_eq!(dollars("1.234500"), "$1.2345");
        assert_eq!(dollars("12"), "$12.00");
        assert_eq!(dollars("0.000000"), "$0.00");
        assert_eq!(dollars("-3.5"), "-$3.50");
        assert_eq!(
            dollars("12345678901234567890.123456789"),
            "$12345678901234567890.123456789"
        );
        assert_eq!(dollars("n/a"), "n/a");
        assert_eq!(cents("1234"), "$12.34");
        assert_eq!(cents("5"), "$0.05");
        assert_eq!(cents("0"), "$0.00");
        assert_eq!(percent(500), "5%");
        assert_eq!(percent(250), "2.5%");
        assert_eq!(percent(5), "0.05%");
        assert!(is_period("2026-09") && !is_period("2026-13") && !is_period("2026-9"));
        assert!(is_usd("25") && is_usd("40.5") && is_usd("40.50"));
        assert!(!is_usd("40.505") && !is_usd("-25") && !is_usd("25.") && !is_usd("") && !is_usd("1e3"));
        assert_eq!(when("2026-10-29T14:05:09.000Z"), "2026-10-29 14:05 UTC");
        assert_eq!(percent(800), "8%");
    }
}
