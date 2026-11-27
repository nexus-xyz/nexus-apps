use super::*;

/// The three route hues of Nexus UI's charts (`--nx-route-1..3`); the rest is grey Other.
pub(in crate::pages) fn hue(slot: usize, palette: Palette) -> Hsla {
    match slot {
        1 => rgb(0x68CCF5).into(),
        2 => rgb(0xF1AD71).into(),
        3 => rgb(0x6FD5B0).into(),
        _ => palette.faint,
    }
}

/// Dollars from a field and its exact decimal twin (`spentUsdDecimal`), the twin first.
pub(in crate::pages) fn amount(value: &Value, key: &str) -> Option<f64> {
    let twin = value
        .get(format!("{key}Decimal"))
        .and_then(Value::as_str)
        .and_then(|text| text.trim().parse::<f64>().ok());
    twin.or_else(|| value.get(key).and_then(Value::as_f64))
}

pub(super) fn text_of(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(String::from)
}

pub(super) fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.).max(0.) as u64
}

/// Money in whole cents: "$15.40", "$1,234.00", "−$0.31".
pub(in crate::pages) fn cents(value: f64) -> String {
    let rounded = (value * 100.).round() / 100.;
    let whole = rounded.abs().trunc() as u64;
    let frac = ((rounded.abs() - rounded.abs().trunc()) * 100.).round() as u64;
    let text = format!("${}.{:02}", ui::count(whole), frac.min(99));
    if rounded < 0. { format!("−{text}") } else { text }
}

/// Money with a sign: "+$3.12", "−$1.00".
pub(in crate::pages) fn cents_signed(value: f64) -> String {
    let rounded = (value * 100.).round() / 100.;
    if rounded > 0. {
        format!("+{}", cents(rounded))
    } else {
        cents(rounded)
    }
}

/// A budget's size: whole dollars without cents ("$100"), else as `cents`.
pub(in crate::pages) fn budget_text(value: f64) -> String {
    let rounded = (value * 100.).round() / 100.;
    if rounded.fract() == 0. {
        format!("${}", ui::count(rounded.abs() as u64))
    } else {
        cents(rounded)
    }
}

/// A share 0 to 1 as "62%", or "0.4%" under 1%.
pub(in crate::pages) fn percent(share: f64) -> String {
    let value = share * 100.;
    if value > 0. && value < 1. {
        format!("{value:.1}%")
    } else {
        format!("{}%", value.round() as i64)
    }
}

/// Tokens, short: "4.1M", "680K", "912".
pub(super) fn compact(value: u64) -> String {
    let value = value as f64;
    let (unit, suffix) = if value >= 1e9 {
        (1e9, "B")
    } else if value >= 1e6 {
        (1e6, "M")
    } else if value >= 1e3 {
        (1e3, "K")
    } else {
        return format!("{}", value.round());
    };
    let scaled = value / unit;
    if scaled >= 100. {
        format!("{}{suffix}", scaled.round())
    } else {
        let text = format!("{scaled:.1}");
        format!("{}{suffix}", text.trim_end_matches(".0"))
    }
}

/// `part` of `whole`, 0 when the whole is 0.
pub(in crate::pages) fn share(part: f64, whole: f64) -> f64 {
    if whole > 0. { (part / whole).max(0.) } else { 0. }
}

/// Today as days since 1970 (UTC).
pub(in crate::pages) fn today() -> i64 {
    chrono_lite::now().div_euclid(86_400)
}

/// `2026-11-05` from days since 1970 (Howard Hinnant's civil-from-days).
pub(in crate::pages) fn iso_day(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970 of `2026-11-05`.
pub(in crate::pages) fn day_number(day: &str) -> Option<i64> {
    chrono_lite::parse(&format!("{}T00:00:00Z", day.get(..10)?)).map(|s| s.div_euclid(86_400))
}

/// "Nov 5".
pub(in crate::pages) fn short_date(day: &str) -> String {
    chrono_lite::month_day(day)
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

/// "November" from `2026-11` or a date in it.
pub(in crate::pages) fn month_name(day: &str) -> &'static str {
    day.get(5..7)
        .and_then(|m| m.parse::<usize>().ok())
        .filter(|m| (1..=12).contains(m))
        .map(|m| MONTHS[m - 1])
        .unwrap_or("This month")
}

/// "Tue, Nov 4".
pub(super) fn long_date(day: &str) -> String {
    const WEEK: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    match day_number(day) {
        Some(n) => format!("{}, {}", WEEK[(n + 4).rem_euclid(7) as usize], short_date(day)),
        None => day.to_string(),
    }
}

/// The 1st of next month, 00:00 UTC: when the organization's budget resets.
pub(in crate::pages) fn next_month() -> String {
    let today = iso_day(today());
    let (y, m) = (
        today[..4].parse::<i64>().unwrap_or(1970),
        today[5..7].parse::<i64>().unwrap_or(1),
    );
    let (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    format!("{y:04}-{m:02}-01T00:00:00Z")
}

/// A query string value, percent-encoded.
pub(in crate::pages) fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `?a=1&b=2` from the pairs with a value.
pub(in crate::pages) fn query(pairs: &[(&str, &str)]) -> String {
    let parts: Vec<String> = pairs
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{key}={}", encode(value)))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

/// A read's answer as a `Load`: the parsed value, or why there is none.
pub(in crate::pages) fn load_of<T>(answer: Result<Value, String>, parse: impl FnOnce(&Value) -> Option<T>) -> Load<T> {
    match answer {
        Ok(value) => match parse(&value) {
            Some(parsed) => Load::Ready(parsed),
            None => Load::Failed("Nexus Gateway answered in a shape this app doesn't read.".into()),
        },
        Err(error) => Load::Failed(error.into()),
    }
}

/// Whether Gateway's message is a permission refusal (403): its denials start "only ..." (`permissions.rs`).
pub(in crate::pages) fn is_refusal(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.starts_with("only ") || lower.contains("forbidden") || lower.contains("not allowed")
}

/// A refused change, in plain words: Gateway's refusal said as a permission, else Gateway's own message.
pub(in crate::pages) fn refusal_text(message: &str) -> String {
    if is_refusal(message) {
        format!("Only organization admins can change this. Gateway said: {message}.")
    } else {
        message.to_string()
    }
}

/// A meter: a track with the filled share, and an optional held part after it.
pub(in crate::pages) fn meter(width: f32, filled: f64, held: f64, color: Hsla, palette: Palette) -> Div {
    let filled = filled.clamp(0., 1.) as f32;
    let held = (held.clamp(0., 1.) as f32).min(1. - filled);
    // At most `width`, and never wider than its parent: the fill is a share of what is drawn.
    h_flex()
        .w(px(width))
        .max_w_full()
        .h(px(6.))
        .flex_none()
        .rounded(px(3.))
        .bg(palette.hover)
        .overflow_hidden()
        .child(div().h_full().w(relative(filled)).bg(color))
        .child(div().h_full().w(relative(held)).bg(color.opacity(0.35)))
}

/// A figure card: its label, the figure, and a line under it.
pub(in crate::pages) fn kpi(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    note: impl Into<SharedString>,
    palette: Palette,
) -> Div {
    // 150 wide at least, so five tiles share one line in the settings column instead of the fifth wrapping wide.
    ui::card(palette)
        .flex_1()
        .min_w(px(150.))
        .child(ui::label(label, palette))
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .child(value.into()),
        )
        .child(div().text_size(px(12.)).text_color(palette.faint).child(note.into()))
}

/// A card's head: the title, a faint line beside it, and whatever goes at the end.
pub(in crate::pages) fn card_head(
    title: impl Into<SharedString>,
    meta: impl Into<SharedString>,
    palette: Palette,
) -> Div {
    h_flex()
        .gap(px(8.))
        .mb(px(6.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .child(title.into()),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(meta.into()),
        )
}

/* ------------------------------------------------------------------ usage report */

/// Members' names by user id (`GET /v1/orgs/members`).
pub(in crate::pages) fn members_of(value: &Value) -> HashMap<String, String> {
    value
        .get("members")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|m| {
                    let id = text_of(m, "userId")?;
                    let name = text_of(m, "name")
                        .or_else(|| text_of(m, "email"))
                        .unwrap_or_else(|| id.clone());
                    Some((id, name))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// List price per million tokens (input, output) by model id (`GET /v1/models`).
pub(super) fn pricing_of(value: &Value) -> HashMap<String, (f64, f64)> {
    value
        .get("data")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|model| {
                    let pricing = model.get("pricing")?;
                    Some((
                        text_of(model, "id")?,
                        (pricing.get("input")?.as_f64()?, pricing.get("output")?.as_f64()?),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/* ------------------------------------------------------------------ drawing */

pub(super) const NUM_W: f32 = 90.;

pub(super) fn cell(width: f32, text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .w(px(width))
        .flex_none()
        .text_size(px(13.))
        .text_color(color)
        .truncate()
        .child(text.into())
}

pub(super) fn right(width: f32, text: impl Into<SharedString>, color: Hsla) -> Div {
    cell(width, text, color).text_right()
}

pub(super) fn loading_or_failed<T>(load: &Load<T>, rows: usize, palette: Palette) -> Option<AnyElement> {
    match load {
        Load::Loading => Some(ui::skeleton(rows, palette).into_any_element()),
        Load::Failed(error) => Some(ui::failed(error.clone(), palette).into_any_element()),
        Load::Ready(_) => None,
    }
}

/// A row's calls filter, by the grouping; none for a row whose calls can't be singled out.
pub(super) fn calls_filter(by: By, group: &Group) -> Option<CallFilters> {
    if group.key == "others" {
        return None;
    }
    let mut filters = CallFilters::default();
    match by {
        By::Team => {
            filters.team = Some(if group.key == "unassigned" {
                "unassigned".into()
            } else {
                group.slug.clone().unwrap_or_else(|| group.key.clone())
            })
        }
        By::Person if group.key == "unassigned" => return None,
        By::Person => filters.person = Some(group.key.clone()),
        By::Model => filters.model = Some(group.key.clone()),
        By::Product => filters.product = Some(group.key.clone()),
    }
    Some(filters)
}

/// An error rate as Breakdown prints it: "0.4%", "No rate".
pub(super) fn error_text(rate: Option<f64>) -> String {
    match rate {
        None => "No rate".into(),
        Some(rate) if rate > 0. && rate < 0.001 => "<0.1%".into(),
        Some(rate) => format!("{:.1}%", rate * 100.),
    }
}

/// A row's spend by day as thin grey bars.
pub(super) fn sparkline(values: &[f64], width: f32, palette: Palette) -> Div {
    let max = values.iter().copied().fold(0., f64::max);
    h_flex()
        .w(px(width))
        .h(px(18.))
        .flex_none()
        .items_end()
        .gap(px(if values.len() > 40 { 0. } else { 1. }))
        .children(values.iter().map(|value| {
            let height = if max > 0. { (*value / max) as f32 * 18. } else { 0. };
            div()
                .flex_1()
                .h(px(height.max(if *value > 0. { 1. } else { 0. })))
                .bg(palette.faint)
        }))
}
