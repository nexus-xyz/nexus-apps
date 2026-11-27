use super::*;

/// The grey note over billing that loaded once and then failed to refresh (plain words, the reason when known, what
/// is kept; kit's `billingStaleNote`).
pub(super) fn stale_note(error: &str) -> String {
    let reason = error.trim().trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    let mut chars = reason.chars();
    let reason = match chars.next() {
        Some(first) => format!(" {}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    };
    format!("Billing didn't refresh.{reason} The figures shown are from the last time it loaded.")
}
/// What auto top-up adds and below what, until an owner sets their own.
pub(super) const TOPUP_DEFAULT: (&str, &str) = ("10", "50");
/// Auto top-up's caps, as Gateway applies them (`TOPUP_CAPS` on the web).
pub(super) const TOPUP_CAPS: &str = "At most 3 top-ups or $500 a day, and $100 in all in a new account's first week.";

// ------------------------------------------------------------------ Money as Gateway sends it

/// A decimal string's parts: the sign, the whole digits and the fraction digits.
pub(super) fn decimal(text: &str) -> Option<(bool, &str, &str)> {
    let text = text.trim();
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = rest.split_once('.').unwrap_or((rest, ""));
    let digits = |part: &str| part.chars().all(|digit| digit.is_ascii_digit());
    (!whole.is_empty() && digits(whole) && digits(fraction) && (fraction.is_empty() == !rest.contains('.')))
        .then_some((negative, whole, fraction))
}

pub(super) fn thousands(whole: &str) -> String {
    let mut out = String::new();
    for (ix, digit) in whole.chars().enumerate() {
        if ix > 0 && (whole.len() - ix) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Money as the page shows it: thousands split, cents only when there are some (`$1,180`, `$184.20`, `$0.000042`).
pub(super) fn money(text: &str) -> String {
    let Some((negative, whole, fraction)) = decimal(text) else {
        return text.to_string();
    };
    let sign = if negative { "-" } else { "" };
    let fraction = fraction.trim_end_matches('0');
    let fraction = if fraction.is_empty() {
        String::new()
    } else {
        format!(".{fraction:0<2}")
    };
    format!("{sign}${}{fraction}", thousands(whole))
}

/// Money to the cent, for a receipt or what is owed: `$184.20`. Sub-cent digits are cut, not rounded.
pub(super) fn cents(text: &str) -> String {
    let Some((negative, whole, fraction)) = decimal(text) else {
        return text.to_string();
    };
    let sign = if negative { "-" } else { "" };
    let fraction = format!("{fraction:0<2}");
    format!("{sign}${}.{}", thousands(whole), &fraction[..2])
}

pub(super) fn number(text: &str) -> f64 {
    text.trim().parse().unwrap_or(0.)
}

/// A money field of a JSON object as its decimal string: Gateway sends strings, an older one numbers.
pub(super) fn amount(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => "0".into(),
    }
}

/// A typed amount as Gateway takes it: no thousands separators, no `$`.
pub(super) fn usd_input(text: &str) -> String {
    text.trim().replace(',', "").trim_start_matches('$').to_string()
}

/// Whole dollars or cents: `50`, `40.50`.
pub(super) fn usd(text: &str) -> bool {
    match text.split_once('.') {
        Some((whole, cents)) => {
            !whole.is_empty()
                && whole.chars().all(|digit| digit.is_ascii_digit())
                && (1..=2).contains(&cents.len())
                && cents.chars().all(|digit| digit.is_ascii_digit())
        }
        None => !text.is_empty() && text.chars().all(|digit| digit.is_ascii_digit()),
    }
}

/// Why `amount` cannot be bought as typed (`purchaseProblem`).
pub(super) fn purchase_problem(amount: &str, min: f64) -> Option<String> {
    let text = amount.trim();
    if !usd(text) {
        return Some("Type an amount in dollars, such as 50 or 40.50.".into());
    }
    let value = number(text);
    if value < min {
        return Some(format!("A purchase is at least {}.", money(&min.to_string())));
    }
    (value > 10_000.).then(|| "A purchase is at most $10,000.".into())
}

/// Why an auto top-up cannot be saved as typed (`topupProblem`).
pub(super) fn topup_problem(below: &str, add: &str, min: f64) -> Option<String> {
    if !usd(below.trim()) || !usd(add.trim()) {
        return Some("Type both amounts in dollars, such as 10 and 50.".into());
    }
    let (below, add) = (number(below), number(add));
    if below <= 0. || below > 10_000. {
        return Some("The line is above $0 and at most $10,000.".into());
    }
    (add < min || add > 500.).then(|| format!("A top-up adds from {} to $500.", money(&min.to_string())))
}

/// The most a usage limit may be when Nexus does not say (`pricing.maxLimitUsd`, kit's `MAX_LIMIT_USD`).
pub(super) const MAX_LIMIT_USD: &str = "100000";

/// Why a usage limit cannot be saved as typed (`limitProblem`): whole dollars or cents, from $1 to `max` (Gateway's
/// `pricing.maxLimitUsd`: $100 in an organization's first week, else $100,000).
pub(super) fn limit_problem(limit: &str, max: Option<&str>) -> Option<String> {
    let text = limit.trim().replace(',', "");
    if !usd(&text) {
        return Some("Type the limit in dollars, such as 25 or 500.".into());
    }
    let max = max.unwrap_or(MAX_LIMIT_USD);
    let most = crate::pay_notice::micros_text(max).unwrap_or(100_000_000_000);
    if number(&text) < 1. {
        Some("The limit is at least $1.".into())
    } else if crate::pay_notice::micros_text(&text).unwrap_or(0) > most {
        Some(format!("The limit is at most {}.", money(max)))
    } else {
        None
    }
}

/// The limit dialog's hint (`limitHint`): where new organizations start, and the most a limit may be now when Nexus
/// says. An older Gateway does not say: its cap was $100 in an organization's first week.
pub(super) fn limit_hint(pricing: &Pricing) -> String {
    match &pricing.max_limit {
        None => format!(
            "New accounts start at {}, at most $100 in their first week.",
            money(&pricing.default_limit)
        ),
        Some(max) => format!(
            "New accounts start at {}. The limit is at most {} now.",
            money(&pricing.default_limit),
            money(max)
        ),
    }
}

// ------------------------------------------------------------------ Days

/// Today in UTC: the year, the month (1 to 12) and the day.
pub(super) fn today() -> (i64, u32, u32) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0);
    civil(seconds.div_euclid(86_400))
}

/// The calendar day `days` after 1970-01-01.
pub(super) fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

pub(super) const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `2026-10` as `October 2026`.
pub(super) fn month_label(period: &str) -> String {
    let month = period
        .get(5..7)
        .and_then(|month| month.parse::<usize>().ok())
        .filter(|month| (1..=12).contains(month));
    match (period.get(..4), month) {
        (Some(year), Some(month)) if period.len() == 7 => format!("{} {year}", MONTHS[month - 1]),
        _ => period.to_string(),
    }
}

/// A date that decides something, with its UTC time: `10 Nov, 23:30 UTC` (kit's `billingCutoff`).
pub(super) fn cutoff(at: &str) -> String {
    let parts = (
        at.get(5..7).and_then(|month| month.parse::<usize>().ok()),
        at.get(8..10).filter(|day| day.chars().all(|c| c.is_ascii_digit())),
    );
    let (Some(month @ 1..=12), Some(day)) = parts else {
        return at.to_string();
    };
    let time = at
        .get(11..16)
        .filter(|time| time.as_bytes().get(2) == Some(&b':'))
        .unwrap_or("00:00");
    format!(
        "{} {}, {time} UTC",
        day.trim_start_matches('0'),
        &MONTHS[month - 1][..3]
    )
}

/// When this month's usage limit resets: the first of the next UTC month, `1 Dec, 00:00 UTC`.
pub(super) fn limit_reset_day((_, month, _): (i64, u32, u32)) -> String {
    let next = if month == 12 { 1 } else { month + 1 };
    format!("1 {}, 00:00 UTC", &MONTHS[next as usize - 1][..3])
}

/// `Resets on 1 Dec, 00:00 UTC` (kit's `usageLimitResetText`).
pub(super) fn limit_resets(today: (i64, u32, u32)) -> String {
    format!("Resets on {}", limit_reset_day(today))
}

/// An ISO time as a short UTC day: `3 Nov 2026`.
pub(super) fn day_label(at: &str) -> String {
    let parts = (
        at.get(..4),
        at.get(5..7).and_then(|month| month.parse::<usize>().ok()),
        at.get(8..10),
    );
    match parts {
        (Some(year), Some(month @ 1..=12), Some(day)) => {
            format!("{} {} {year}", day.trim_start_matches('0'), &MONTHS[month - 1][..3])
        }
        _ => at.to_string(),
    }
}

// ------------------------------------------------------------------ What Gateway answers

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Card {
    pub(super) brand: String,
    pub(super) last4: String,
    pub(super) exp_month: u32,
    pub(super) exp_year: i64,
}

impl Card {
    pub(super) fn expired(&self, (year, month, _): (i64, u32, u32)) -> bool {
        self.exp_year < year || (self.exp_year == year && self.exp_month < month)
    }

    pub(super) fn expires_this_month(&self, (year, month, _): (i64, u32, u32)) -> bool {
        self.exp_year == year && self.exp_month == month
    }

    pub(super) fn expiry(&self) -> String {
        let year = self.exp_year.to_string();
        format!("{:02}/{}", self.exp_month, &year[year.len().saturating_sub(2)..])
    }

    /// What a wait for the card compares (`cardKey`).
    pub(super) fn key(card: Option<&Card>) -> String {
        card.map(|card| format!("{}:{}:{}/{}", card.brand, card.last4, card.exp_month, card.exp_year))
            .unwrap_or_else(|| "none".into())
    }
}

/// The payment method on file, of any type.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Method {
    pub(super) kind: String,
    pub(super) brand: String,
    pub(super) bank: String,
    pub(super) last4: String,
}

pub(super) fn brand_name(brand: &str) -> String {
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

impl Method {
    /// `Visa ·· 4242`.
    pub(super) fn short(&self) -> String {
        match self.kind.as_str() {
            "card" => format!("{} ·· {}", brand_name(&self.brand), self.last4),
            "us_bank_account" => format!("{} ·· {}", or(&self.bank, "Bank"), self.last4),
            other => {
                let words = other.replace('_', " ");
                let mut chars = words.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            }
        }
    }

    /// `Visa ending in 4242`.
    pub(super) fn sentence(&self) -> String {
        match self.kind.as_str() {
            "card" => format!("{} ending in {}", brand_name(&self.brand), self.last4),
            "us_bank_account" => format!("{} account ending in {}", or(&self.bank, "Bank"), self.last4),
            other => other.replace('_', " "),
        }
    }
}

pub(super) fn or<'a>(text: &'a str, fallback: &'a str) -> &'a str {
    if text.is_empty() { fallback } else { text }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Topup {
    pub(super) enabled: bool,
    pub(super) below: String,
    pub(super) add: String,
    pub(super) off_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct After {
    pub(super) enabled: bool,
    pub(super) limit: String,
    pub(super) month_spent: String,
    pub(super) left: String,
    /// Usage not charged yet, less a running charge: about zero during a retry. Name `unpaid` instead.
    pub(super) owed: String,
    /// The usage the card has not paid, a running charge included (`unpaidUsd`); an older Gateway sends none.
    pub(super) unpaid: Option<String>,
    /// The charge phase Gateway names (`phase`, `charge::phase_of`); an older Gateway sends none.
    pub(super) phase: Option<String>,
    /// The charge that runs now (`chargeUsd`), or `None`.
    pub(super) charge: Option<String>,
    /// What open calls hold on the card: the final charge waits for them.
    pub(super) reserved: String,
    pub(super) threshold: String,
    pub(super) pending: bool,
    pub(super) failed: Option<String>,
    /// The latest hold Gateway refused on this month's limit (`refusedUsd`), or `None`: a call that size is
    /// refused while `left` is less.
    pub(super) refused: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Pricing {
    pub(super) markup_bps: u64,
    pub(super) min_purchase: String,
    pub(super) threshold: String,
    pub(super) default_limit: String,
    /// The least final charge when paying after the fact turns off; an older Gateway sends none ($0.50).
    pub(super) min_final_charge: String,
    /// The most a usage limit may be now (`maxLimitUsd`); an older Gateway sends none.
    pub(super) max_limit: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PastDue {
    pub(super) period: String,
    pub(super) blocked_from: Option<String>,
    /// Stripe's page that pays the month (`invoiceUrl`); an older Gateway sends none.
    pub(super) invoice: Option<String>,
}

/// Stripe's page that pays the unpaid month: Gateway's `invoiceUrl`, else the month's receipt from an older Gateway.
pub(super) fn past_due_invoice(due: &PastDue, receipts: &[Receipt]) -> Option<String> {
    due.invoice.clone().or_else(|| {
        receipts
            .iter()
            .find(|row| row.id == format!("month:{}", due.period))
            .and_then(|row| row.url.clone())
    })
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Allowance {
    pub(super) reset_at: String,
    pub(super) ai: Option<(String, String)>,
    pub(super) cloud: Option<(f64, f64)>,
}

impl Allowance {
    fn read(answer: &Value) -> Option<Self> {
        let allowance = answer.get("planAllowance")?;
        let reset_at = allowance.get("resetAt")?.as_str()?.into();
        let ai = allowance
            .get("ai")
            .and_then(|meter| Some((meter["usedUsd"].as_str()?.into(), meter["leftUsd"].as_str()?.into())));
        let cloud = allowance
            .get("cloud")
            .and_then(|meter| Some((meter["usedHours"].as_f64()?, meter["leftHours"].as_f64()?)));
        Some(Self { reset_at, ai, cloud })
    }
}

/// `GET /v1/billing`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Billing {
    pub(super) test_mode: bool,
    pub(super) allowance: Option<Allowance>,
    pub(super) plan_name: Option<&'static str>,
    pub(super) card: Option<Card>,
    pub(super) method: Option<Method>,
    /// What Nexus gave once, and what is left of it.
    pub(super) free_start: Option<(String, String)>,
    /// What was ever bought, and what is left.
    pub(super) credits: Option<(String, String)>,
    pub(super) promotional_credit: String,
    pub(super) topup: Option<Topup>,
    pub(super) after: Option<After>,
    pub(super) pricing: Pricing,
    pub(super) past_due: Option<PastDue>,
}

pub(super) fn object(value: &Value, key: &str) -> Option<Value> {
    value.get(key).filter(|value| value.is_object()).cloned()
}

pub(super) fn parse_billing(answer: &Value) -> Option<Billing> {
    answer.as_object()?;
    let card = object(answer, "paymentMethod").map(|card| Card {
        brand: ui::text(&card, "brand").to_string(),
        last4: ui::text(&card, "last4").to_string(),
        exp_month: card.get("expMonth").and_then(Value::as_u64).unwrap_or(0) as u32,
        exp_year: card.get("expYear").and_then(Value::as_i64).unwrap_or(0),
    });
    // An older Gateway sends only the card: it stands for the method.
    let method = match answer.get("defaultPaymentMethod") {
        Some(Value::Object(_)) => object(answer, "defaultPaymentMethod").map(|method| Method {
            kind: ui::text(&method, "type").to_string(),
            brand: ui::text(&method, "brand").to_string(),
            bank: ui::text(&method, "bankName").to_string(),
            last4: ui::text(&method, "last4").to_string(),
        }),
        Some(_) => None,
        None => card.as_ref().map(|card| Method {
            kind: "card".into(),
            brand: card.brand.clone(),
            bank: String::new(),
            last4: card.last4.clone(),
        }),
    };
    let pricing = object(answer, "pricing");
    let pricing = Pricing {
        markup_bps: pricing
            .as_ref()
            .and_then(|pricing| pricing.get("markupBps"))
            .and_then(Value::as_u64)
            .unwrap_or(800),
        min_purchase: pricing
            .as_ref()
            .map(|pricing| amount(pricing, "minPurchaseUsd"))
            .unwrap_or_else(|| "25".into()),
        threshold: pricing
            .as_ref()
            .map(|pricing| amount(pricing, "thresholdUsd"))
            .unwrap_or_else(|| "25".into()),
        default_limit: pricing
            .as_ref()
            .map(|pricing| amount(pricing, "defaultLimitUsd"))
            .unwrap_or_else(|| "25".into()),
        min_final_charge: pricing
            .as_ref()
            .filter(|pricing| pricing.get("minFinalChargeUsd").is_some())
            .map(|pricing| amount(pricing, "minFinalChargeUsd"))
            .unwrap_or_else(|| "0.50".into()),
        max_limit: pricing
            .as_ref()
            .and_then(|pricing| pricing.get("maxLimitUsd"))
            .and_then(Value::as_str)
            .map(String::from),
    };
    let allowance = Allowance::read(answer);
    let plan = &answer["plan"];
    let plan_name = match (allowance.is_some(), plan["status"].as_str(), plan["plan"].as_str()) {
        (true, Some("active" | "trialing" | "past_due" | "unpaid"), Some(name)) => match name {
            "plus" => Some("Plus"),
            "pro" => Some("Pro"),
            "teams_plus" => Some("Teams Plus"),
            "teams_pro" => Some("Teams Pro"),
            _ => None,
        },
        _ => None,
    };
    Some(Billing {
        test_mode: ui::text(answer, "mode") == "test",
        allowance,
        plan_name,
        card,
        method,
        free_start: object(answer, "freeStart")
            .map(|start| (amount(&start, "grantedUsd"), amount(&start, "remainingUsd"))),
        credits: object(answer, "credits")
            .map(|credits| (amount(&credits, "purchasedUsd"), amount(&credits, "remainingUsd"))),
        promotional_credit: amount(answer, "promotionalCreditUsd"),
        topup: object(answer, "autoTopup").map(|topup| Topup {
            enabled: topup.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            below: amount(&topup, "belowUsd"),
            add: amount(&topup, "addUsd"),
            off_reason: topup.get("offReason").and_then(Value::as_str).map(String::from),
        }),
        after: object(answer, "afterTheFact").map(|after| After {
            enabled: after.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            limit: amount(&after, "limitUsd"),
            month_spent: amount(&after, "monthSpentUsd"),
            left: amount(&after, "leftUsd"),
            owed: amount(&after, "owedUsd"),
            unpaid: after.get("unpaidUsd").and_then(Value::as_str).map(String::from),
            phase: after.get("phase").and_then(Value::as_str).map(String::from),
            charge: after.get("chargeUsd").and_then(Value::as_str).map(String::from),
            reserved: amount(&after, "reservedUsd"),
            threshold: amount(&after, "thresholdUsd"),
            pending: after.get("pending").and_then(Value::as_bool).unwrap_or(false),
            failed: object(&after, "failed").map(|failed| ui::text(&failed, "reason").to_string()),
            refused: after.get("refusedUsd").and_then(Value::as_str).map(String::from),
        }),
        pricing,
        past_due: object(answer, "pastDue").map(|due| PastDue {
            period: ui::text(&due, "period").to_string(),
            blocked_from: due.get("blockedFrom").and_then(Value::as_str).map(String::from),
            invoice: due.get("invoiceUrl").and_then(Value::as_str).map(String::from),
        }),
    })
}

/// How the organization pays, in the summary's first cell (`payMode`, plans left out: they are dormant).
pub(super) fn pay_mode(billing: &Billing) -> (&'static str, String, bool) {
    let credits = billing.credits.is_some();
    let after = billing.after.as_ref().is_some_and(|after| after.enabled);
    match (credits, after) {
        (true, true) => (words::PAY_AS_YOU_GO, "Nexus credits and after the fact".into(), true),
        (true, false) => (words::PAY_AS_YOU_GO, words::CREDITS.into(), true),
        (false, true) => (words::PAY_AS_YOU_GO, "After the fact".into(), true),
        (false, false)
            if number(&billing.promotional_credit) > 0.
                && billing
                    .free_start
                    .as_ref()
                    .is_none_or(|(_, left)| number(left) < 0.000_001) =>
        {
            (
                "Free",
                format!("{} promotional credit left", money(&billing.promotional_credit)),
                false,
            )
        }
        (false, false) => match &billing.free_start {
            Some((granted, remaining)) => (
                "Free",
                format!("{} of your free {} left", cents(remaining), money(granted)),
                false,
            ),
            None => ("Free", "Runs on your own keys".into(), false),
        },
    }
}

pub(super) fn credits_low(billing: &Billing) -> bool {
    let line = billing.topup.as_ref().map(|topup| number(&topup.below)).unwrap_or(10.);
    match &billing.credits {
        Some((_, remaining)) => !billing.topup.as_ref().is_some_and(|topup| topup.enabled) && number(remaining) < line,
        None => false,
    }
}

pub(super) fn free_start_used_up(billing: &Billing) -> bool {
    !pay_mode(billing).2
        && number(&billing.promotional_credit) < 0.000_001
        && billing
            .free_start
            .as_ref()
            .is_some_and(|(_, remaining)| number(remaining) < 0.000_001)
}

/// Under a cent left, or less left than a call Gateway refused: Gateway refuses a call that MAY
/// cost more than what is left, so the limit can stop calls while money is left.
pub(super) fn usage_limit_reached(billing: &Billing) -> bool {
    billing.after.as_ref().is_some_and(|after| {
        after.enabled
            && !charge::phase_of(after).charge_problem()
            && (number(&after.left) < 0.01
                || after
                    .refused
                    .as_deref()
                    .is_some_and(|refused| number(&after.left) < number(refused)))
    })
}

/// The usage-limit banner: used when under a cent is left, else what is left and the call refused.
pub(super) fn usage_limit_text(org: &str, after: &After, today: (i64, u32, u32)) -> String {
    // The reset is said with its day and UTC time, as on every surface.
    let rest = format!(
        "until the limit resets on {} or goes up; credits and your own keys still work.",
        limit_reset_day(today)
    );
    let limit = money(&after.limit);
    let left = crate::pay_notice::micros_text(&after.left);
    let refused = after.refused.as_deref().and_then(crate::pay_notice::micros_text);
    match (left, refused) {
        (Some(left), Some(refused)) if left >= 10_000 => format!(
            "{org} has {} left of this month's usage limit of {limit}, and a call that may cost {} was refused. Calls the card pays that may cost more than what is left are refused {rest}",
            crate::pay_notice::down_to_cent(left),
            crate::pay_notice::up_to_cent(refused)
        ),
        _ => format!("{org} has used this month's usage limit of {limit}. Nexus-run AI the card pays is paused {rest}"),
    }
}

/// Why Nexus turned auto top-up off, as kit's `TOPUP_OFF_REASON` says it; the tests check `topup_off_text`
/// against `kit/src/fixtures/billing-words.json`.
pub(super) fn topup_off_reason(reason: &str) -> &str {
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
pub(super) fn topup_off_text(reason: &str) -> String {
    format!("{} is off: {}", words::AUTO_TOPUP, topup_off_reason(reason))
}

/// What fixes auto top-up that Nexus turned off (`topupOffNotice` on the web).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TopupFix {
    UpdateCard,
    AddAnotherCard,
    BuyCredits,
}

/// The banner while Gateway keeps auto top-up off (the owners were told, `billing.auto_topup_off`): why, low credits
/// in the same line, and what fixes it. None while it is on, after an owner turned it off, or for a reason nobody acts
/// on (a closed organization).
pub(super) fn topup_off_notice(billing: &Billing) -> Option<(String, TopupFix)> {
    let topup = billing.topup.as_ref()?;
    let reason = topup.off_reason.as_deref()?;
    let fix = match reason {
        "declined" | "authentication_required" | "payment_failed" | "no_payment_method" => TopupFix::UpdateCard,
        "disputed" => TopupFix::AddAnotherCard,
        "refunded" => TopupFix::BuyCredits,
        _ => return None,
    };
    let low = if credits_low(billing) {
        format!(" Credits are under {}.", money(&topup.below))
    } else {
        String::new()
    };
    let next = match fix {
        TopupFix::UpdateCard => "Update the card, then turn it on again.",
        TopupFix::AddAnotherCard => "Add another card, then turn it on again.",
        TopupFix::BuyCredits => "Buy credits to keep going, and turn it on again when you are ready.",
    };
    Some((format!("{}.{low} {next}", topup_off_text(reason)), fix))
}

/// What turning paying after the fact off leaves to charge (`stopAfterNote` on the web): the final charge, or why none.
pub(super) fn stop_after_note(after: &After, pricing: &Pricing) -> String {
    let owed = number(&after.owed);
    let reserved = number(&after.reserved) > 0.;
    let minimum = money(&pricing.min_final_charge);
    let pending = if after.pending {
        "A charge is already in progress and will continue. "
    } else {
        ""
    };
    let failed = if after.failed.is_some() {
        "The last charge failed; unpaid usage remains due until a payment succeeds. "
    } else {
        ""
    };
    let settlement = if owed > 0. && owed >= number(&pricing.min_final_charge) {
        format!(
            "Final charge for current unpaid usage: {}. Nexus rounds down to whole cents and waits for open calls to settle.",
            cents(&after.owed)
        )
    } else if owed > 0. && reserved {
        format!(
            "Current unpaid usage: {}. Whether a final charge is taken depends on the settled total reaching {minimum}.",
            cents(&after.owed)
        )
    } else if owed > 0. {
        format!(
            "Current unpaid usage: {}. Below {minimum}, it stays unpaid until paying after the fact is on again.",
            cents(&after.owed)
        )
    } else {
        "No additional unpaid usage is currently due.".into()
    };
    let inflight = if reserved {
        format!(
            " Up to {} of calls are still in flight; the final amount is calculated after they settle.",
            money(&after.reserved)
        )
    } else {
        String::new()
    };
    format!("{pending}{failed}{settlement}{inflight}")
}

/// The summary cell's line under the unpaid usage: the charge phase's (a problem, a charge going through or the final
/// charge, `charge::summary_note`), else how it charges while on, else off.
pub(super) fn after_summary_detail(after: Option<&After>, pricing: &Pricing) -> String {
    let Some(after) = after else {
        return "Off".into();
    };
    match charge::summary_note(after, &pricing.min_final_charge) {
        Some(note) => note,
        None if after.enabled => format!("Charged at every {}", money(&after.threshold)),
        None => "Off".into(),
    }
}

/// The banner for a charge that failed, is tried again, waits for the bank's check (3-D Secure) or was disputed: its
/// text, its action, and whether it is urgent. The phase decides (`charge::notice`), and the text names the unpaid
/// usage. It is urgent only while it stops calls the card pays: with paying after the fact off, nothing runs on the
/// card. A calm note (a charge going through, the final charge) shows in the row instead.
pub(super) fn charge_banner(billing: &Billing) -> Option<(String, charge::Action, bool)> {
    let after = billing.after.as_ref()?;
    let pay = charge::notice(after, &billing.pricing.min_final_charge)?;
    let action = pay.action?;
    let mut text = format!("{}.", pay.title);
    if let Some(body) = &pay.body {
        text.push_str(&format!(" {body}."));
    }
    if action == charge::Action::ConfirmPayment {
        text.push_str(&format!(" {}", confirm_charge_note()));
    }
    Some((text, action, pay.stops_calls))
}

/// A usage charge the bank wants the cardholder to confirm (3-D Secure): Gateway confirms that same charge again on
/// session (`POST /v1/billing/confirm-payment`), and the bank's page approves it (`confirmChargeNote` on the web).
pub(super) fn confirm_charge_note() -> &'static str {
    "Approve it on your bank's page; nothing is charged twice."
}

/// Gateway found no charge that waits for the bank's check (409 `nothing_to_confirm`): the card is saved again instead.
pub(super) fn nothing_to_confirm(error: &nexus_client::Error) -> bool {
    matches!(error, nexus_client::Error::Core(error) if error.code.as_deref() == Some("nothing_to_confirm"))
}

/// What the page says after Gateway confirmed the charge with no bank page to open: paid only for `succeeded`.
pub(super) fn confirmed_note(answer: &Value) -> &'static str {
    if ui::text(answer, "status") == "succeeded" {
        "The payment went through."
    } else {
        "Stripe is taking the payment. It shows here once Stripe confirms it."
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Receipt {
    pub(super) id: String,
    pub(super) at: String,
    pub(super) description: String,
    pub(super) amount: String,
    pub(super) status: String,
    pub(super) url: Option<String>,
    pub(super) receipt: bool,
}

impl Receipt {
    /// The payment's last 8 characters, or the invoiced month.
    pub(super) fn label(&self) -> String {
        match self.id.strip_prefix("month:") {
            Some(month) => month.to_string(),
            None => format!("…{}", &self.id[self.id.len().saturating_sub(8)..]),
        }
    }
}

pub(super) fn parse_receipts(answer: &Value) -> Option<Vec<Receipt>> {
    let rows = answer.get("receipts")?.as_array()?;
    Some(
        rows.iter()
            .filter_map(|row| {
                Some(Receipt {
                    id: row.get("id")?.as_str()?.to_string(),
                    at: ui::text(row, "at").to_string(),
                    description: ui::text(row, "description").to_string(),
                    amount: amount(row, "amountUsd"),
                    status: ui::text(row, "status").to_string(),
                    url: row
                        .get("url")
                        .and_then(Value::as_str)
                        .filter(|url| !url.is_empty())
                        .map(String::from),
                    receipt: row.get("receipt").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect(),
    )
}

pub(super) fn status_word(status: &str) -> &str {
    match status {
        "paid" => "Paid",
        "open" => "Open",
        "failed" => "Failed",
        "void" => "Void",
        other => other,
    }
}

/// The receipts as a CSV file's text, newest first (`receiptsCsv`).
pub(super) fn receipts_csv(receipts: &[Receipt]) -> String {
    let cell = |text: &str| {
        if text.contains([',', '"', '\n']) {
            format!("\"{}\"", text.replace('"', "\"\""))
        } else {
            text.to_string()
        }
    };
    let mut out = String::from("Receipt,Date,For,Amount (USD),Status\n");
    for row in receipts {
        let fields = [
            cell(&row.id),
            cell(row.at.get(..10).unwrap_or(&row.at)),
            cell(&row.description),
            cell(&row.amount),
            cell(&row.status),
        ];
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    out
}

/// Who may see billing, from `GET /v1/tenants/me`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Me {
    pub(super) name: String,
    pub(super) owner: bool,
}

pub(super) fn parse_me(answer: &Value) -> Me {
    Me {
        name: answer
            .get("tenant")
            .map(|tenant| ui::text(tenant, "name").to_string())
            .unwrap_or_default(),
        owner: answer
            .get("permissions")
            .and_then(Value::as_array)
            .is_some_and(|list| list.iter().any(|held| held.as_str() == Some("org.billing"))),
    }
}

/// Billing is off on this Gateway: no route (404) or no Stripe (503 `not_configured`).
pub(super) fn billing_off(error: &str) -> bool {
    error.contains("no such endpoint") || error.contains("HTTP 404") || error.starts_with("Stripe needs")
}

/// Gateway's refusal said plainly.
pub(super) fn plain(error: &str) -> String {
    if error.starts_with("only the org's") {
        "Only the organization's owners can change billing.".into()
    } else {
        error.to_string()
    }
}

// ------------------------------------------------------------------ The page

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum WaitKind {
    Card,
    Credits,
}

/// What an owner chose before Stripe's page: turned on once Stripe's card reaches Gateway.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Deferred {
    pub(super) pay_after: bool,
    /// Auto top-up's line and amount.
    pub(super) topup: Option<(String, String)>,
}

impl Deferred {
    pub(super) fn steps(&self, after_on: bool) -> Vec<(Verb, String, Value)> {
        let mut steps = Vec::new();
        if let Some((below, add)) = &self.topup {
            steps.push((
                Verb::Put,
                "/v1/billing/auto-topup".to_string(),
                json!({ "enabled": true, "belowUsd": below, "addUsd": add }),
            ));
        }
        if self.pay_after && !after_on {
            steps.push((
                Verb::Put,
                "/v1/billing/after-the-fact".to_string(),
                json!({ "enabled": true }),
            ));
        }
        steps
    }
}

pub(super) fn watched_key(kind: WaitKind, billing: &Billing) -> String {
    match kind {
        WaitKind::Card => Card::key(billing.card.as_ref()),
        WaitKind::Credits => billing
            .credits
            .as_ref()
            .map(|(purchased, _)| purchased.clone())
            .unwrap_or_else(|| "none".into()),
    }
}

/// An inline form under How it pays, where the web page opens a dialog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Panel {
    Setup,
    /// `true`: Set up billing also chose paying after the fact, which turns on with this purchase's card.
    Credits(bool),
    Limit,
    /// Auto top-up's switch, turning on: what it charges, before it changes.
    ConfirmTopup,
    /// Paying after the fact's switch, turning on: each charge and the month's limit, before it changes.
    ConfirmAfter,
    /// Paying after the fact's switch, turning off: the final charge, before it changes.
    StopAfter,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Busy {
    Topup,
    After,
    Stripe,
    Buy,
    Limit,
    Receipt(String),
}

#[derive(Debug, Clone)]
pub(super) struct Outcome {
    pub(super) text: String,
    pub(super) error: bool,
}

pub(super) fn done(text: impl Into<String>) -> Option<Outcome> {
    Some(Outcome {
        text: text.into(),
        error: false,
    })
}

pub(super) fn failed(error: &str) -> Option<Outcome> {
    Some(Outcome {
        text: plain(error),
        error: true,
    })
}

/// A decimal string without trailing zeros, for a field: `10.000000` is `10`.
pub(super) fn plain_number(text: &str) -> String {
    match text.split_once('.') {
        Some((whole, fraction)) => {
            let fraction = fraction.trim_end_matches('0');
            if fraction.is_empty() {
                whole.to_string()
            } else {
                format!("{whole}.{fraction}")
            }
        }
        None => text.to_string(),
    }
}

/// A path segment for a URL.
pub(super) fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
