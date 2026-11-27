//! `nexus search` (`search.ts`; `GET /v1/search`): one query across what your role may read, grouped and ranked, as
//! Nexus UI's ⌘K palette shows it. Chats are only ever your own; matched text is bold on a terminal.

use serde_json::Value;

use crate::cli::Cmd;
use crate::client::search;
use crate::error::{CliError, Result, usage};
use crate::json::{a, js_string, s, t};
use crate::style::{Style, js_number, pad_end};
use crate::teams::js_integer;

pub const SEARCH_TYPES: [&str; 5] = ["chats", "artifacts", "people", "teams", "models"];

pub const SEARCH_OPTIONS: &[(&str, &str)] = &[
    (
        "--type <t>",
        "chats, artifacts, people, teams or models (comma-separated for several); default all",
    ),
    ("--limit <n>", "Results per type (1 to 20, default 5)"),
    (
        "--json",
        "The answer as JSON: groups, results, scores, match ranges and Nexus UI links",
    ),
];

pub fn search_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{}  nexus search \"<query>\" [--type T] [--limit N] [--json]",
            style.bold("Usage")
        ),
        String::new(),
        "Search what your role may read: your own chats (titles and messages), artifacts shared with or by you, people,"
            .to_string(),
        "teams, models and model policies. Words match as prefixes, in any order.".to_string(),
        String::new(),
    ];
    lines.extend(
        SEARCH_OPTIONS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 14)))),
    );
    lines
}

/// `text` with its matched ranges (UTF-16 offsets, as JavaScript indexes strings) in bold (`highlight`).
pub fn highlight(style: &Style, text: &str, matches: &[Value]) -> String {
    if !style.color || matches.is_empty() {
        return text.to_string();
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut ranges: Vec<(usize, usize)> = matches
        .iter()
        .map(|range| {
            let at = |n: usize| range.get(n).and_then(Value::as_f64).unwrap_or(0.0) as usize;
            (at(0), at(1))
        })
        .collect();
    ranges.sort_by_key(|range| range.0);
    let slice = |from: usize, to: usize| String::from_utf16_lossy(&units[from.min(units.len())..to.min(units.len())]);
    let mut out = String::new();
    let mut at = 0;
    for (start, end) in ranges {
        if start < at || end <= start {
            continue;
        }
        out.push_str(&slice(at, start));
        out.push_str(&style.bold(&slice(start, end)));
        at = end;
    }
    out.push_str(&slice(at, units.len()));
    out
}

/// A result's page as a link under the Nexus UI this CLI's Gateway belongs to (`resultLink`): Gateway answers
/// `/app/<org>/…`, printed as `<ui>/<org>/…`.
pub fn result_link(ui: &str, url: &str) -> String {
    let page = match url.strip_prefix("/app") {
        Some(rest) if rest.starts_with('/') => rest,
        _ => url,
    };
    format!("{}{page}", ui.strip_suffix('/').unwrap_or(ui))
}

pub fn search_command(rest: &[String], cmd: &Cmd, ui: &str) -> Result<i32> {
    let query = rest.join(" ").trim().to_string();
    if query.is_empty() {
        return usage(
            "What should I search for?",
            Some("Run `nexus search \"<query>\"`, e.g. nexus search deploy --type chats."),
        );
    }
    let mut types = None;
    if let Some(raw) = cmd.flag("type") {
        let asked: Vec<&str> = raw.split(',').map(str::trim).filter(|kind| !kind.is_empty()).collect();
        let unknown = asked.iter().find(|kind| !SEARCH_TYPES.contains(kind));
        if unknown.is_some() || asked.is_empty() {
            let named = serde_json::to_string(unknown.copied().unwrap_or(raw)).unwrap_or_default();
            return Err(CliError {
                message: format!("Unknown type {named}."),
                hint: Some(format!("Types: {}.", SEARCH_TYPES.join(", "))),
                exit_code: 2,
                code: None,
            }
            .into());
        }
        types = Some(asked.join(","));
    }
    let mut limit = None;
    if let Some(raw) = cmd.flag("limit") {
        match js_integer(raw) {
            Some(value) if (1.0..=20.0).contains(&value) => limit = Some(js_number(value)),
            _ => return usage("--limit must be a whole number from 1 to 20.", None),
        }
    }
    let answer = cmd.core.get(&format!(
        "/v1/search{}",
        search(&[("q", Some(query)), ("types", types), ("limit", limit)])
    ))?;
    if cmd.json {
        return Ok(cmd.print(&answer));
    }
    print_results(cmd, &answer, ui);
    Ok(0)
}

fn print_results(cmd: &Cmd, answer: &Value, ui: &str) {
    let style = cmd.style;
    let query = serde_json::to_string(s(answer, "query").unwrap_or("")).unwrap_or_default();
    let groups = a(answer, "groups");
    if groups.is_empty() {
        let types: Vec<String> = a(answer, "types").iter().map(|kind| js_string(Some(kind))).collect();
        let within = if types.len() < SEARCH_TYPES.len() {
            format!(
                " in {}",
                if types.is_empty() {
                    "those types".to_string()
                } else {
                    types.join(", ")
                }
            )
        } else {
            String::new()
        };
        cmd.out(style.dim(&format!("Nothing matches {query}{within}.")));
        return;
    }
    let mut lines: Vec<String> = vec![];
    for group in groups {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!("  {}", style.bold(&js_string(group.get("label")))));
        for result in a(group, "results") {
            let subtitle = match s(result, "subtitle") {
                Some(subtitle) => format!("  {}", style.dim(subtitle)),
                None => String::new(),
            };
            lines.push(format!(
                "{} {}{subtitle}",
                style.bullet,
                highlight(style, s(result, "title").unwrap_or(""), a(result, "titleMatches"))
            ));
            if let Some(snippet) = result.get("snippet").filter(|snippet| snippet.is_object())
                && s(result, "type") == Some("chats")
            {
                lines.push(format!(
                    "    {}",
                    highlight(style, s(snippet, "text").unwrap_or(""), a(snippet, "matches"))
                ));
            }
            lines.push(format!(
                "    {}",
                style.dim(&result_link(ui, s(result, "url").unwrap_or("")))
            ));
        }
        if t(group, "more") {
            lines.push(style.dim(&format!(
                "  More: nexus search {query} --type {} --limit 20",
                js_string(group.get("type"))
            )));
        }
    }
    cmd.lines(lines);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn highlights_utf16_ranges_and_links_under_nexus_ui() {
        let style = Style::new(true);
        let text = highlight(&style, "🙂 café", &[json!([3, 7])]);
        assert_eq!(text, format!("🙂 {}", style.bold("café")));
        assert_eq!(highlight(&Style::new(false), "abc", &[json!([0, 1])]), "abc");
        assert_eq!(
            result_link("https://core.example/app", "/app/acme/sessions/chs_1#m-msg_2"),
            "https://core.example/app/acme/sessions/chs_1#m-msg_2"
        );
        assert_eq!(
            result_link("http://localhost:4000/app/", "/app/acme/teams/web"),
            "http://localhost:4000/app/acme/teams/web"
        );
    }
}
