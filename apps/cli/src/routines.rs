//! `nexus routines`, as `routines.ts` does it (W4.1): your routines, such as a morning brief on a schedule.
//! Create, list, show, pause, resume and delete them.

use serde_json::{Value, json};

use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{Result, usage};
use crate::json::{js_string, s};
use crate::service_accounts::asked;
use crate::style::{kv, pairs};

const CREATE_USAGE: &str = "Usage: nexus routines create --at <HH:MM> [--days mon-fri] | --every <interval> [--timezone <zone>] [--title <title>] [--max-runs-per-day <n>] [--team <slug>]";
const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

pub fn routines_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let action = args.first().map(String::as_str).unwrap_or("");
    let id = args.get(1).map(String::as_str).filter(|id| !id.is_empty());
    match action {
        "list" => list(cmd),
        "create" => create(cmd),
        "show" | "pause" | "resume" | "delete" => {
            let Some(id) = id else {
                return usage(
                    format!("Usage: nexus routines {action} <id>"),
                    Some("`nexus routines list` prints your routines' ids."),
                );
            };
            match action {
                "show" => show(id, cmd),
                "delete" => delete(id, cmd),
                "pause" => set_state(id, "paused", cmd),
                _ => set_state(id, "active", cmd),
            }
        }
        "" => usage(
            "Usage: nexus routines <list|show|create|pause|resume|delete>",
            Some("Run `nexus routines --help`."),
        ),
        other => usage(
            format!("Unknown command `nexus routines {other}`."),
            Some("Run `nexus routines --help`."),
        ),
    }
}

/// `30m`, `2h`, `1d`, `900s` or `900`: seconds.
pub fn interval_seconds(text: &str) -> Option<u64> {
    let text = text.trim();
    let (digits, unit) = match text.char_indices().last() {
        Some((at, unit @ ('s' | 'm' | 'h' | 'd'))) => (&text[..at], unit),
        _ => (text, 's'),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let size = match unit {
        'm' => 60,
        'h' => 3600,
        'd' => 86400,
        _ => 1,
    };
    digits
        .parse::<u64>()
        .ok()?
        .checked_mul(size)
        .filter(|seconds| *seconds > 0 && *seconds <= (1 << 53))
}

/// `mon-fri`, `mon,wed,fri` or `sat-sun`: the days, in week order.
pub fn days_of(text: &str) -> Option<Vec<&'static str>> {
    let mut named = [false; 7];
    for part in text.split(',').map(|part| part.trim().to_ascii_lowercase()) {
        let mut ends = part.splitn(2, '-');
        let from = ends.next().unwrap_or_default();
        let start = DAYS.iter().position(|day| *day == from)?;
        let end = match ends.next() {
            None => start,
            Some(to) => DAYS.iter().position(|day| *day == to)?,
        };
        if end < start {
            return None;
        }
        for day in named.iter_mut().take(end + 1).skip(start) {
            *day = true;
        }
    }
    let days: Vec<&'static str> = DAYS
        .iter()
        .zip(named)
        .filter(|(_, on)| *on)
        .map(|(day, _)| *day)
        .collect();
    (!days.is_empty()).then_some(days)
}

/// `every 1h`, `at 08:00 on mon,tue` or `at 08:00 every day`.
pub fn when_of(routine: &Value) -> String {
    let trigger = &routine["trigger"];
    if let Some(every) = trigger["everySeconds"].as_u64() {
        return format!("every {}", duration_text(every));
    }
    let at = s(trigger, "at").unwrap_or("?");
    match trigger["days"].as_array().filter(|days| !days.is_empty()) {
        Some(days) => {
            let days: Vec<&str> = days.iter().filter_map(Value::as_str).collect();
            format!("at {at} on {}", days.join(","))
        }
        None => format!("at {at} every day"),
    }
}

fn duration_text(seconds: u64) -> String {
    for (unit, size) in [("d", 86400), ("h", 3600), ("m", 60)] {
        if seconds.is_multiple_of(size) {
            return format!("{}{unit}", seconds / size);
        }
    }
    format!("{seconds}s")
}

fn create(cmd: &Cmd) -> Result<i32> {
    let (at, every) = (cmd.args.flag_set("at"), cmd.args.flag_set("every"));
    if at.is_some() == every.is_some() {
        return usage(CREATE_USAGE, Some("Give --at <HH:MM> or --every <interval>, not both."));
    }
    let days = cmd.args.flag_set("days");
    if days.is_some() && at.is_none() {
        return usage("--days goes with --at.", Some(CREATE_USAGE));
    }
    let mut trigger = json!({ "kind": "schedule" });
    if let Some(every) = every {
        let Some(seconds) = interval_seconds(every) else {
            return usage(
                format!("--every needs an interval such as 30m, 2h or 1d, not {}.", json!(every)),
                None,
            );
        };
        trigger["everySeconds"] = json!(seconds);
    } else if let Some(at) = at {
        trigger["at"] = json!(at);
        if let Some(text) = days {
            let Some(days) = days_of(text) else {
                return usage(
                    format!("--days needs days such as mon-fri or mon,wed,fri, not {}.", json!(text)),
                    None,
                );
            };
            trigger["days"] = json!(days);
        }
    }
    if let Some(zone) = cmd.args.flag_set("timezone") {
        trigger["timezone"] = json!(zone);
    }
    let mut body = json!({ "trigger": trigger, "action": { "kind": "brief" } });
    if let Some(title) = cmd.args.flag_set("title") {
        body["title"] = json!(title);
    }
    if let Some(runs) = cmd.args.flag_set("max-runs-per-day") {
        match runs.parse::<u64>() {
            Ok(runs) if runs >= 1 => body["caps"] = json!({ "maxRunsPerDay": runs }),
            _ => return usage("--max-runs-per-day needs a whole number of 1 or more.", None),
        }
    }
    if let Some(team) = cmd.args.flag_set("team") {
        body["team"] = json!(team);
    }
    let routine = asked(cmd.core.post("/v1/routines", body))?;
    if cmd.json {
        return Ok(cmd.print(&routine));
    }
    cmd.out(format!(
        "{} Created routine {}: {}, {}",
        cmd.style.ok,
        cmd.style.bold(&js_string(routine.get("id"))),
        js_string(routine.get("title")),
        when_of(&routine)
    ));
    if let Some(next) = s(&routine, "nextRunAt") {
        cmd.out(cmd.style.dim(&format!("Next run: {next}")));
    }
    let delivery = &routine["delivery"];
    if delivery["available"] != json!(true) {
        let line = format!(
            "{} Not delivered yet ({}): {}",
            cmd.style.warn,
            s(delivery, "code").unwrap_or("unavailable"),
            s(delivery, "reason").unwrap_or("")
        );
        cmd.out(line.trim_end());
    }
    Ok(0)
}

fn list(cmd: &Cmd) -> Result<i32> {
    let path = if cmd.has("all") {
        "/v1/routines?all=true"
    } else {
        "/v1/routines"
    };
    let answer = asked(cmd.core.get(path))?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    let routines = answer["routines"].as_array().cloned().unwrap_or_default();
    if routines.is_empty() {
        cmd.out("No routines.");
        cmd.out(
            cmd.style
                .dim("Create one: nexus routines create --at 08:00 --days mon-fri"),
        );
        return Ok(0);
    }
    for routine in &routines {
        let state = s(routine, "state").unwrap_or("unknown");
        let next = s(routine, "nextRunAt").map_or_else(|| state.to_string(), |next| format!("next {next}"));
        cmd.out(format!(
            "{}  {state}  {}  {next}  {}",
            cmd.style.bold(&js_string(routine.get("id"))),
            when_of(routine),
            js_string(routine.get("title"))
        ));
    }
    Ok(0)
}

fn show(id: &str, cmd: &Cmd) -> Result<i32> {
    let routine = asked(cmd.core.get(&format!("/v1/routines/{}", enc(id))))?;
    if cmd.json {
        return Ok(cmd.print(&routine));
    }
    let state = s(&routine, "state").unwrap_or("unknown");
    let state = match s(&routine, "pausedReason") {
        Some(reason) => format!("{state} (by {reason})"),
        None => state.to_string(),
    };
    let proactivity = &routine["proactivity"];
    let mut rows = vec![
        kv("Routine", js_string(routine.get("id"))),
        kv("Title", js_string(routine.get("title"))),
        kv("Owner", js_string(routine.get("ownerId"))),
        kv("State", state),
        kv("When", when_of(&routine)),
        kv("Time zone", s(&routine["trigger"], "timezone").unwrap_or("UTC")),
        kv("Action", s(&routine["action"], "kind").unwrap_or("?")),
        kv("Notify", s(proactivity, "notify").unwrap_or("always")),
        kv("Writes", s(proactivity, "writes").unwrap_or("never")),
        kv("Reports to", js_string(routine.get("reportTo"))),
    ];
    if let Some(runs) = routine["caps"]["maxRunsPerDay"].as_u64() {
        rows.push(kv("Runs a day", runs.to_string()));
    }
    for (label, field) in [
        ("Team", "teamId"),
        ("Next run", "nextRunAt"),
        ("Last slot", "lastRunAt"),
    ] {
        if let Some(value) = s(&routine, field).filter(|value| !value.is_empty()) {
            rows.push(kv(label, value));
        }
    }
    let delivery = &routine["delivery"];
    let delivered = if delivery["available"] == json!(true) {
        "available".to_string()
    } else {
        format!("not yet ({})", s(delivery, "code").unwrap_or("unavailable"))
    };
    rows.push(kv("Delivery", delivered));
    cmd.lines(pairs(cmd.style, &rows));
    Ok(0)
}

fn set_state(id: &str, state: &str, cmd: &Cmd) -> Result<i32> {
    let routine = asked(
        cmd.core
            .patch(&format!("/v1/routines/{}", enc(id)), json!({ "state": state })),
    )?;
    if cmd.json {
        return Ok(cmd.print(&routine));
    }
    if state == "paused" {
        cmd.out(format!("{} Paused {id}.", cmd.style.ok));
    } else {
        let next = s(&routine, "nextRunAt").map_or_else(String::new, |next| format!(" Next run: {next}"));
        cmd.out(format!("{} Resumed {id}.{next}", cmd.style.ok));
    }
    Ok(0)
}

fn delete(id: &str, cmd: &Cmd) -> Result<i32> {
    let answer = asked(cmd.core.delete(&format!("/v1/routines/{}", enc(id))))?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    cmd.out(format!("{} Deleted {id}.", cmd.style.ok));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_and_days_read_as_the_typescript_cli_reads_them() {
        assert_eq!(interval_seconds("30m"), Some(1800));
        assert_eq!(interval_seconds("1d"), Some(86400));
        assert_eq!(interval_seconds("900"), Some(900));
        assert_eq!(interval_seconds("900s"), Some(900));
        assert_eq!(interval_seconds("1w"), None);
        assert_eq!(interval_seconds("m"), None);
        assert_eq!(interval_seconds("0"), None);
        assert_eq!(days_of("mon-fri"), Some(vec!["mon", "tue", "wed", "thu", "fri"]));
        assert_eq!(days_of("sun,mon,sun"), Some(vec!["mon", "sun"]));
        assert_eq!(days_of("fri-mon"), None);
        assert_eq!(days_of("mon-tue-wed"), None);
        assert_eq!(days_of("weekday"), None);
        assert_eq!(duration_text(7200), "2h");
        assert_eq!(duration_text(90), "90s");
    }
}
