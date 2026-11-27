use super::*;

/// A row's figures (`UsageFigures`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(in crate::pages) struct Figures {
    pub spent: f64,
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub failed: u64,
    pub refused: u64,
    pub error_rate: Option<f64>,
}

impl Figures {
    pub(super) fn parse(value: &Value) -> Figures {
        let status = value.get("callsByStatus").cloned().unwrap_or(Value::Null);
        Figures {
            spent: amount(value, "spentUsd").unwrap_or(0.),
            calls: number(value, "calls"),
            input_tokens: number(value, "inputTokens"),
            output_tokens: number(value, "outputTokens"),
            failed: number(&status, "failed"),
            refused: number(&status, "refused"),
            error_rate: value.get("errorRate").and_then(Value::as_f64),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::pages) struct Group {
    pub key: String,
    pub label: String,
    pub share: f64,
    pub slug: Option<String>,
    pub level: Option<String>,
    pub has_children: bool,
    pub direct: bool,
    pub deleted: bool,
    pub figures: Figures,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::pages) struct Day {
    pub day: String,
    pub figures: Figures,
    /// Spend per group key that day.
    pub groups: HashMap<String, f64>,
}

/// What a limit stopped (`overspendPrevented`), by level.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::pages) struct Prevented {
    pub calls: u64,
    pub amount: f64,
    /// (label, calls, amount) for each level that stopped any.
    pub levels: Vec<(&'static str, u64, f64)>,
}

/// The team whose child teams a report shows, with its path.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::pages) struct Under {
    pub name: String,
    pub slug: String,
    pub level: Option<String>,
    pub path: Vec<(String, String)>,
}

/// `GET /v1/analytics/usage`.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::pages) struct Report {
    pub scope: String,
    pub totals: Figures,
    pub prevented: Option<Prevented>,
    pub groups: Vec<Group>,
    pub series: Vec<Day>,
    pub under: Option<Under>,
}

impl Report {
    pub(in crate::pages) fn parse(value: &Value) -> Option<Report> {
        let totals_value = value.get("totals")?;
        let groups = value
            .get("groups")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|group| {
                        let key = text_of(group, "key")?;
                        Some(Group {
                            label: text_of(group, "label").unwrap_or_else(|| key.clone()),
                            key,
                            share: group.get("share").and_then(Value::as_f64).unwrap_or(0.),
                            slug: text_of(group, "slug"),
                            level: text_of(group, "level"),
                            has_children: group.get("hasChildren").and_then(Value::as_bool).unwrap_or(false),
                            direct: group.get("direct").and_then(Value::as_bool).unwrap_or(false),
                            deleted: group.get("deleted").and_then(Value::as_bool).unwrap_or(false),
                            figures: Figures::parse(group),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let series = value
            .get("series")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|day| {
                        let mut groups = HashMap::new();
                        if let Some(map) = day.get("groups").and_then(Value::as_object) {
                            for (key, spent) in map {
                                groups.insert(key.clone(), spent.as_f64().unwrap_or(0.));
                            }
                        }
                        if let Some(map) = day.get("groupsDecimal").and_then(Value::as_object) {
                            for (key, spent) in map {
                                if let Some(spent) = spent.as_str().and_then(|s| s.parse::<f64>().ok()) {
                                    groups.insert(key.clone(), spent);
                                }
                            }
                        }
                        Some(Day {
                            day: text_of(day, "day")?,
                            figures: Figures::parse(day),
                            groups,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let prevented = totals_value.get("overspendPrevented").map(|prevented| {
            let by = prevented.get("byLevel").cloned().unwrap_or(Value::Null);
            let levels = [
                ("organization", "Organization"),
                ("team", "Team"),
                ("person", "Person"),
                ("product", "Product"),
            ]
            .into_iter()
            .filter_map(|(key, label)| {
                let figures = by.get(key)?;
                let calls = number(figures, "calls");
                (calls > 0).then(|| (label, calls, amount(figures, "amountUsd").unwrap_or(0.)))
            })
            .collect();
            Prevented {
                calls: number(prevented, "calls"),
                amount: amount(prevented, "amountUsd").unwrap_or(0.),
                levels,
            }
        });
        let under = value.get("under").and_then(|under| {
            Some(Under {
                name: text_of(under, "name")?,
                slug: text_of(under, "slug").unwrap_or_default(),
                level: text_of(under, "level"),
                path: under
                    .get("path")
                    .and_then(Value::as_array)
                    .map(|path| {
                        path.iter()
                            .filter_map(|step| Some((text_of(step, "name")?, text_of(step, "slug")?)))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        });
        Some(Report {
            scope: text_of(value, "scope").unwrap_or_else(|| "org".into()),
            totals: Figures::parse(totals_value),
            prevented,
            groups,
            series,
            under,
        })
    }

    /// Days of the series with at least one call.
    pub(super) fn active_days(&self) -> usize {
        self.series.iter().filter(|day| day.figures.calls > 0).count()
    }
}

/// The range on screen: the last `days` UTC days and the `days` before them.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Span {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) previous_from: String,
    pub(super) previous_to: String,
}

pub(super) fn span(days: i64, today: i64) -> Span {
    let from = today - (days - 1);
    let previous_to = from - 1;
    Span {
        from: iso_day(from),
        to: iso_day(today),
        previous_from: iso_day(previous_to - (days - 1)),
        previous_to: iso_day(previous_to),
    }
}

impl Span {
    /// How many days the range holds.
    pub(super) fn days(&self) -> i64 {
        match (day_number(&self.from), day_number(&self.to)) {
            (Some(from), Some(to)) => to - from + 1,
            _ => 1,
        }
    }

    /// "vs. Oct 1 – Oct 18": the range the figures compare against.
    pub(super) fn versus(&self) -> String {
        format!(
            "vs. {} – {}",
            short_date(&self.previous_from),
            short_date(&self.previous_to)
        )
    }
}

/// The range a person picks above the tabs: the last N UTC days, or this month so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Range {
    Last(i64),
    Month,
}

impl Range {
    /// The picker's choices, in its order.
    pub(super) const ALL: [Range; 4] = [Range::Last(7), Range::Month, Range::Last(30), Range::Last(90)];

    pub(super) fn label(self) -> String {
        match self {
            Range::Last(days) => format!("{days}d"),
            Range::Month => "This month".into(),
        }
    }

    pub(super) fn tip(self) -> String {
        match self {
            Range::Last(days) => format!("Last {days} days"),
            Range::Month => "Since the 1st, UTC: the budget's month".into(),
        }
    }

    /// The range's days on `today` (days since 1970), and the days it compares against. This month compares against
    /// the same days of last month, cut at its end: Nov 1 to 30 against Oct 1 to 30, Mar 1 to 31 against Feb 1 to 28.
    pub(super) fn span(self, today: i64) -> Span {
        match self {
            Range::Last(days) => span(days, today),
            Range::Month => {
                let from = first_of_month(today);
                let previous_to_end = from - 1;
                let previous_from = first_of_month(previous_to_end);
                let previous_to = (previous_from + (today - from)).min(previous_to_end);
                Span {
                    from: iso_day(from),
                    to: iso_day(today),
                    previous_from: iso_day(previous_from),
                    previous_to: iso_day(previous_to),
                }
            }
        }
    }
}

/// The 1st of `day`'s month, as days since 1970.
pub(super) fn first_of_month(day: i64) -> i64 {
    let iso = iso_day(day);
    day_number(&format!("{}-01", &iso[..7])).unwrap_or(day)
}

/// The days after `today` to the end of its month, as `2026-11-19`.
pub(super) fn rest_of_month(today: i64) -> Vec<String> {
    let month = &iso_day(today)[..7];
    (1..=31)
        .map(|n| iso_day(today + n))
        .take_while(|day| day.starts_with(month))
        .collect()
}

/* ------------------------------------------------------------------ spend and budgets */

/// `GET /v1/spend`: the organization's budget (owners and admins) or what is left for you (members).
#[derive(Debug, Clone, PartialEq)]
pub(in crate::pages) struct Spend {
    /// None for a member, whose answer carries no budget.
    pub budget: Option<f64>,
    pub spent: f64,
    pub reserved: f64,
    pub remaining: f64,
    pub period: Option<String>,
    pub resets_at: Option<String>,
}

impl Spend {
    pub(in crate::pages) fn parse(value: &Value) -> Option<Spend> {
        value.as_object()?;
        Some(Spend {
            budget: amount(value, "budgetUsd"),
            spent: amount(value, "periodSpentUsd")
                .or_else(|| amount(value, "spentUsd"))
                .unwrap_or(0.),
            reserved: amount(value, "reservedUsd").unwrap_or(0.),
            remaining: amount(value, "remainingUsd").unwrap_or(0.),
            period: text_of(value, "budgetPeriod"),
            resets_at: text_of(value, "resetsAt"),
        })
    }
}

/// One person's limit this month (`PersonBudget`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Person {
    pub(super) user_id: String,
    pub(super) email: Option<String>,
    pub(super) name: Option<String>,
    pub(super) role: String,
    pub(super) limit: Option<f64>,
    pub(super) source: String,
    pub(super) spent: f64,
    pub(super) reserved: f64,
    pub(super) remaining: Option<f64>,
    pub(super) at_limit: bool,
    pub(super) limit_manager: bool,
}

impl Person {
    pub(super) fn display(&self) -> String {
        self.name
            .clone()
            .or_else(|| self.email.clone())
            .unwrap_or_else(|| self.user_id.clone())
    }

    pub(super) fn state(&self) -> LimitState {
        if self.at_limit {
            return LimitState::At;
        }
        match self.limit {
            None => LimitState::None,
            Some(limit) if limit > 0. && (self.spent + self.reserved) * 5. >= limit * 4. => LimitState::Near,
            Some(_) => LimitState::Within,
        }
    }

    pub(super) fn used(&self) -> f64 {
        self.limit
            .map(|limit| share(self.spent + self.reserved, limit))
            .unwrap_or(0.)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LimitState {
    At,
    Near,
    Within,
    None,
}

/// `GET /v1/budgets`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Budgets {
    pub(super) period: String,
    pub(super) resets_at: String,
    pub(super) person_default: Option<f64>,
    pub(super) scope: String,
    pub(super) people: Vec<Person>,
    pub(super) can_manage: bool,
    pub(super) can_grant: bool,
}

impl Budgets {
    pub(super) fn parse(value: &Value) -> Option<Budgets> {
        let people = value
            .get("people")?
            .as_array()?
            .iter()
            .filter_map(|person| {
                Some(Person {
                    user_id: text_of(person, "userId")?,
                    email: text_of(person, "email"),
                    name: text_of(person, "name"),
                    role: text_of(person, "role").unwrap_or_else(|| "member".into()),
                    limit: amount(person, "limitUsd"),
                    source: text_of(person, "source").unwrap_or_else(|| "none".into()),
                    spent: amount(person, "spentUsd").unwrap_or(0.),
                    reserved: amount(person, "reservedUsd").unwrap_or(0.),
                    remaining: amount(person, "remainingUsd"),
                    at_limit: person.get("atLimit").and_then(Value::as_bool).unwrap_or(false),
                    limit_manager: person.get("limitManager").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect();
        Some(Budgets {
            period: text_of(value, "period").unwrap_or_default(),
            resets_at: text_of(value, "resetsAt").unwrap_or_else(next_month),
            person_default: value.get("personDefault").and_then(|d| amount(d, "monthlyUsd")),
            scope: text_of(value, "scope").unwrap_or_else(|| "own".into()),
            people,
            can_manage: value.get("canManage").and_then(Value::as_bool).unwrap_or(false),
            can_grant: value.get("canGrant").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// (at, near, own, by default).
    pub(super) fn counts(&self) -> (usize, usize, usize, usize) {
        let mut counts = (0, 0, 0, 0);
        for person in &self.people {
            match person.state() {
                LimitState::At => counts.0 += 1,
                LimitState::Near => counts.1 += 1,
                _ => {}
            }
            match person.source.as_str() {
                "override" => counts.2 += 1,
                "default" => counts.3 += 1,
                _ => {}
            }
        }
        counts
    }

    pub(super) fn can_edit(&self, person: &Person) -> bool {
        self.scope == "org" && self.can_manage && (self.can_grant || person.role != "owner")
    }
}

/// A dollar amount typed by a person ("8", "$8.00", "1,200.5") as Gateway's decimal string, or none.
pub(super) fn parse_usd(text: &str) -> Option<String> {
    let trimmed = text.trim().trim_start_matches('$').trim().replace(',', "");
    let (whole, fraction) = trimmed.split_once('.').unwrap_or((&trimmed, ""));
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if !digits(whole) || (trimmed.contains('.') && (!digits(fraction) || fraction.len() > 2)) {
        return None;
    }
    Some(format!("{whole}.{fraction:0<2}"))
}

/* ------------------------------------------------------------------ stacked chart */

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Series {
    pub(super) key: String,
    pub(super) label: String,
    /// 1 to 3, or 0 for Other.
    pub(super) hue: usize,
    pub(super) total: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Column {
    pub(super) day: String,
    pub(super) total: f64,
    pub(super) calls: u64,
    pub(super) values: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Stack {
    pub(super) series: Vec<Series>,
    pub(super) columns: Vec<Column>,
    pub(super) max: f64,
}

/// The report as stacked columns: the top three groups in hues, the rest as one grey Other (`stackOf`).
pub(super) fn stack_of(report: &Report) -> Stack {
    let mut ranked: Vec<&Group> = report.groups.iter().filter(|group| group.figures.spent > 0.).collect();
    ranked.sort_by(|a, b| b.figures.spent.total_cmp(&a.figures.spent));
    let named: Vec<&Group> = ranked.iter().take(3).copied().collect();
    let mut series: Vec<Series> = named
        .iter()
        .enumerate()
        .map(|(n, group)| Series {
            key: group.key.clone(),
            label: group.label.clone(),
            hue: n + 1,
            total: group.figures.spent,
        })
        .collect();
    let columns: Vec<Column> = report
        .series
        .iter()
        .map(|day| {
            let values: Vec<f64> = named
                .iter()
                .map(|group| *day.groups.get(&group.key).unwrap_or(&0.))
                .collect();
            let other = (day.figures.spent - values.iter().sum::<f64>()).max(0.);
            let mut values = values;
            values.push(other);
            Column {
                day: day.day.clone(),
                total: day.figures.spent,
                calls: day.figures.calls,
                values,
            }
        })
        .collect();
    let other: f64 = columns
        .iter()
        .map(|column| column.values.last().copied().unwrap_or(0.))
        .sum();
    let mut columns = columns;
    if other > 0.0005 || ranked.len() > 3 {
        series.push(Series {
            key: "__other".into(),
            label: "Other".into(),
            hue: 0,
            total: other,
        });
    } else {
        for column in &mut columns {
            column.values.pop();
        }
    }
    let max = columns.iter().map(|column| column.total).fold(0., f64::max);
    Stack { series, columns, max }
}

/// A round top for the y axis and its four lines: $1.20 is 0, 0.40, 0.80, 1.20.
pub(super) fn axis(max: f64) -> (f64, [f64; 4]) {
    if max <= 0. {
        return (1., [0., 0.33, 0.67, 1.]);
    }
    let raw = max / 3.;
    let power = 10f64.powf(raw.log10().floor());
    let step = [1., 2., 2.5, 5., 10.]
        .iter()
        .map(|m| m * power)
        .find(|candidate| *candidate >= raw)
        .unwrap_or(10. * power);
    (step * 3., [0., step, step * 2., step * 3.])
}

/// Which columns carry an x label: every day in a week, Mondays in a month, every other Monday past that, and the
/// first day.
pub(super) fn labeled_days(days: &[String]) -> HashSet<usize> {
    let mut shown = HashSet::new();
    if days.len() <= 10 {
        shown.extend(0..days.len());
        return shown;
    }
    let mut mondays = 0;
    for (n, day) in days.iter().enumerate() {
        if day_number(day).map(|d| (d + 4).rem_euclid(7)) != Some(1) {
            continue;
        }
        if days.len() <= 45 || mondays % 2 == 0 {
            shown.insert(n);
        }
        mondays += 1;
    }
    if !shown.contains(&1) && !shown.contains(&2) {
        shown.insert(0);
    }
    shown
}

/// The groups whose spend moved most against the previous range (`movers`).
pub(super) fn movers(sets: &[(&str, &Report, &Report)]) -> Vec<(String, String, f64, Option<f64>)> {
    let mut all = Vec::new();
    for (kind, now, before) in sets {
        let mut keys: Vec<&String> = now.groups.iter().map(|g| &g.key).collect();
        for group in &before.groups {
            if !keys.contains(&&group.key) {
                keys.push(&group.key);
            }
        }
        for key in keys {
            if key == "unassigned" || key == "others" {
                continue;
            }
            let current = now.groups.iter().find(|g| &g.key == key);
            let previous = before.groups.iter().find(|g| &g.key == key);
            let was = previous.map(|g| g.figures.spent).unwrap_or(0.);
            let is = current.map(|g| g.figures.spent).unwrap_or(0.);
            let change = is - was;
            if change.abs() < 0.01 {
                continue;
            }
            let label = current
                .or(previous)
                .map(|g| g.label.clone())
                .unwrap_or_else(|| key.clone());
            all.push((
                model_name(&label),
                kind.to_string(),
                change,
                (was > 0.).then(|| change / was),
            ));
        }
    }
    all.sort_by(|a, b| b.2.abs().total_cmp(&a.2.abs()));
    all.truncate(6);
    all
}

/// A model id as people say it, whichever way Gateway spelled it: "anthropic/claude-sonnet-5.5" and
/// "claude-opus-5-5" both read "Sonnet 5.5" / "Opus 5.5". Anything that isn't a Claude id stays as it is.
pub(super) fn model_name(label: &str) -> String {
    let id = label.rsplit('/').next().unwrap_or(label);
    if !id.starts_with("claude-") {
        return label.to_string();
    }
    crate::code::model_label(&id.replace('.', "-"))
}

pub(super) fn ratio_text(ratio: Option<f64>) -> String {
    match ratio {
        None => "New".into(),
        Some(ratio) => {
            let value = (ratio * 100.).round() as i64;
            let sign = if value > 0 {
                "+"
            } else if value < 0 {
                "−"
            } else {
                ""
            };
            format!("{sign}{}%", value.abs())
        }
    }
}

/// The month-end estimate from spend so far: (projected, month's last day, verdict). None without a monthly budget.
pub(super) fn budget_pace(spend: &Spend, now_seconds: i64) -> Option<(f64, String, &'static str)> {
    let budget = spend.budget.filter(|b| *b > 0.)?;
    if spend.period.as_deref().unwrap_or("month") != "month" {
        return None;
    }
    let reset = spend.resets_at.clone().unwrap_or_else(next_month);
    let end = chrono_lite::parse(&reset)?;
    let end_day = end.div_euclid(86_400);
    let reset_iso = iso_day(end_day);
    let (y, m) = (
        reset_iso[..4].parse::<i64>().ok()?,
        reset_iso[5..7].parse::<i64>().ok()?,
    );
    let (y, m) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    let start = chrono_lite::parse(&format!("{y:04}-{m:02}-01T00:00:00Z"))?;
    if now_seconds < start || now_seconds >= end {
        return None;
    }
    let month_days = ((end - start) as f64 / 86_400.).round();
    let elapsed = ((now_seconds - start) as f64 / 86_400.).max(1.);
    let projected = spend.spent / elapsed * month_days;
    let verdict = if projected > budget {
        "over"
    } else if projected * 5. > budget * 4. {
        "close"
    } else {
        "within"
    };
    Some((projected, iso_day(end_day - 1), verdict))
}

/* ------------------------------------------------------------------ calls */

/// One model call, as the ledger recorded it (`SpendCall`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Call {
    pub(super) id: String,
    pub(super) at: String,
    pub(super) status: String,
    pub(super) model: Option<String>,
    pub(super) provider: Option<String>,
    pub(super) product: String,
    pub(super) capability: Option<String>,
    pub(super) user_id: Option<String>,
    pub(super) team_id: Option<String>,
    pub(super) agent_session_id: Option<String>,
    pub(super) chat_session_id: Option<String>,
    pub(super) input_tokens: Option<u64>,
    pub(super) output_tokens: Option<u64>,
    pub(super) cost: String,
    pub(super) reserved: Option<String>,
    pub(super) overspend_level: Option<String>,
    pub(super) funded_by: Option<String>,
    /// Every field, for "All fields" and the decision.
    pub(super) raw: Value,
}

impl Call {
    pub(super) fn parse(value: &Value) -> Option<Call> {
        Some(Call {
            id: text_of(value, "id")?,
            at: text_of(value, "at").unwrap_or_default(),
            status: text_of(value, "status").unwrap_or_else(|| "settled".into()),
            model: text_of(value, "model"),
            provider: text_of(value, "provider"),
            product: text_of(value, "product").unwrap_or_default(),
            capability: text_of(value, "capability"),
            user_id: text_of(value, "userId"),
            team_id: text_of(value, "teamId"),
            agent_session_id: text_of(value, "agentSessionId"),
            chat_session_id: text_of(value, "chatSessionId"),
            input_tokens: value.get("inputTokens").and_then(Value::as_u64),
            output_tokens: value.get("outputTokens").and_then(Value::as_u64),
            cost: text_of(value, "costUsd").unwrap_or_else(|| "0".into()),
            reserved: text_of(value, "reservedUsd"),
            overspend_level: text_of(value, "overspendLevel"),
            funded_by: text_of(value, "fundedBy"),
            raw: value.clone(),
        })
    }

    /// Done, Failed, Stopped · person limit, Stopped.
    pub(super) fn state(&self) -> String {
        match self.status.as_str() {
            "settled" => "Done".into(),
            "failed" => "Failed".into(),
            "refused" => format!(
                "Stopped · {}",
                match self.overspend_level.as_deref() {
                    Some("person") => "person limit",
                    Some("team") => "team limit",
                    _ => "budget",
                }
            ),
            _ => "Stopped".into(),
        }
    }

    /// Where it came from: the agent behind its session, Chat, or its product.
    pub(super) fn source(&self, agents: &HashMap<String, String>) -> String {
        if let Some(session) = &self.agent_session_id {
            // A coding agent here has a route through Gateway (Claude Code, Codex). Nexus's own agent is no coding agent,
            // so its spend reads as "Agent".
            return agents
                .get(session)
                .and_then(|id| nexus_client::agent_entries::Agent::parse(id))
                .map_or("Agent", |agent| agent.entry().identity.name)
                .into();
        }
        if self.chat_session_id.is_some() || self.product == "chat" {
            return "Chat".into();
        }
        if self.product == "agents" {
            return "Agent".into();
        }
        capitalize(&self.product)
    }
}

/// `GET /v1/spend/calls`: a page of calls, newest first.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::pages) struct CallPage {
    pub(super) calls: Vec<Call>,
    pub(super) next_cursor: Option<String>,
}

impl CallPage {
    pub(super) fn parse(value: &Value) -> Option<CallPage> {
        Some(CallPage {
            calls: value.get("calls")?.as_array()?.iter().filter_map(Call::parse).collect(),
            next_cursor: text_of(value, "nextCursor"),
        })
    }
}

/// A decimal dollar string as money, from its digits: whole cents keep two places, smaller amounts keep theirs.
pub(super) fn usd_decimal(value: &str) -> String {
    let trimmed = value.trim();
    let (sign, rest) = trimmed.strip_prefix('-').map(|r| ("-", r)).unwrap_or(("", trimmed));
    let Some((whole, fraction)) = rest.split_once('.') else {
        return match rest.parse::<u64>() {
            Ok(n) => format!("{sign}${}.00", ui::count(n)),
            Err(_) => value.to_string(),
        };
    };
    let fraction = fraction.trim_end_matches('0');
    let fraction = format!("{fraction:0<2}");
    let whole = whole
        .parse::<u64>()
        .map(ui::count)
        .unwrap_or_else(|_| whole.to_string());
    let zero = whole == "0" && fraction.chars().all(|c| c == '0');
    format!("{}${whole}.{fraction}", if zero { "" } else { sign })
}

pub(super) fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The part of a model id after its maker.
pub(in crate::pages) fn short_model(id: &str) -> &str {
    id.split_once('/').map(|(_, rest)| rest).unwrap_or(id)
}

/// The filters of the calls list (the observe filter names).
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct CallFilters {
    /// One UTC day, when set.
    pub(super) day: Option<String>,
    pub(super) model: Option<String>,
    pub(super) person: Option<String>,
    pub(super) status: Option<String>,
    pub(super) product: Option<String>,
    pub(super) team: Option<String>,
    pub(super) session: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Menu {
    Day,
    Model,
    Person,
    Status,
    Product,
}

pub(super) const STATUSES: [(&str, &str); 4] = [
    ("settled", "Done"),
    ("failed", "Failed"),
    ("refused", "Stopped · limit"),
    ("released", "Stopped"),
];

/* ------------------------------------------------------------------ adoption */

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AdoptionRow {
    pub(super) day: String,
    pub(super) engineers: u64,
    pub(super) by_product: HashMap<String, u64>,
    pub(super) gateway: usize,
    pub(super) subscription: usize,
}

/// `GET /v1/analytics/agents`, as counts only.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Adoption {
    pub(super) member: bool,
    pub(super) products: Vec<(String, String)>,
    /// Newest first.
    pub(super) rows: Vec<AdoptionRow>,
    pub(super) active_days: usize,
    pub(super) supported: bool,
}

impl Adoption {
    pub(super) fn parse(value: &Value) -> Option<Adoption> {
        let days = value.get("days")?.as_array()?;
        let supported = days.iter().all(|day| {
            day.get("engineers").map(Value::is_number).unwrap_or(false)
                && day.get("products").map(Value::is_array).unwrap_or(false)
        });
        let mut slugs: Vec<String> = Vec::new();
        let mut rows = Vec::new();
        for day in days {
            let mut by_product = HashMap::new();
            for product in day.get("products").and_then(Value::as_array).into_iter().flatten() {
                if let Some(slug) = text_of(product, "product") {
                    by_product.insert(slug.clone(), number(product, "engineers"));
                    if !slugs.contains(&slug) {
                        slugs.push(slug);
                    }
                }
            }
            let listed = |key: &str| day.get(key).and_then(Value::as_array).map(Vec::len).unwrap_or(0);
            rows.push(AdoptionRow {
                day: text_of(day, "day").unwrap_or_default(),
                engineers: number(day, "engineers"),
                by_product,
                gateway: listed("people"),
                subscription: listed("subscription"),
            });
        }
        rows.reverse();
        const ORDER: [&str; 3] = ["core", "agents", "chat"];
        let rank = |slug: &str| ORDER.iter().position(|s| *s == slug).unwrap_or(ORDER.len());
        slugs.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
        let products = slugs
            .into_iter()
            .map(|slug| {
                let label = match slug.as_str() {
                    "core" => "Gateway".to_string(),
                    "chat" => "Chat".to_string(),
                    "agents" => "Coding agents".to_string(),
                    other => capitalize(&other.replace('-', " ")),
                };
                (slug, label)
            })
            .collect();
        let active_days = rows.iter().filter(|row| row.engineers > 0).count();
        Some(Adoption {
            member: text_of(value, "scope").as_deref() == Some("member"),
            products,
            rows,
            active_days,
            supported,
        })
    }
}

/* ------------------------------------------------------------------ the page */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Overview,
    Breakdown,
    Adoption,
    Calls,
    Limits,
}

pub(super) const TABS: [(Tab, &str); 5] = [
    (Tab::Overview, "Overview"),
    (Tab::Breakdown, "Breakdown"),
    (Tab::Calls, "Calls"),
    (Tab::Adoption, "Adoption"),
    (Tab::Limits, "Limits"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum By {
    Team,
    Person,
    Model,
    Product,
}

impl By {
    pub(super) fn slug(self) -> &'static str {
        match self {
            By::Team => "team",
            By::Person => "person",
            By::Model => "model",
            By::Product => "product",
        }
    }
    pub(super) fn label(self) -> &'static str {
        match self {
            By::Team => "Team",
            By::Person => "Person",
            By::Model => "Model",
            By::Product => "Product",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LimitFilter {
    All,
    Near,
    At,
    /// People with a limit of their own, not the default.
    Own,
}

impl LimitFilter {
    pub(super) fn keeps(self, person: &Person) -> bool {
        match self {
            LimitFilter::All => true,
            LimitFilter::Near => person.state() == LimitState::Near,
            LimitFilter::At => person.state() == LimitState::At,
            LimitFilter::Own => person.source == "override",
        }
    }
}

/// What the amount field is editing now.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Edit {
    Budget,
    Default,
    Person(String),
}

/// The person's role in the organization, from `GET /v1/tenants/me`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(in crate::pages) struct Me {
    pub permissions: Vec<String>,
    pub personal: bool,
}

impl Me {
    pub(in crate::pages) fn parse(value: &Value) -> Option<Me> {
        value.get("tenant")?;
        Some(Me {
            permissions: value
                .get("permissions")
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(Value::as_str).map(String::from).collect())
                .unwrap_or_default(),
            personal: value.get("tenant").and_then(|t| t.get("kind")).and_then(Value::as_str) == Some("personal"),
        })
    }

    pub(in crate::pages) fn can(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission)
    }
}
