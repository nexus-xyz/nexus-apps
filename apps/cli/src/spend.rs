//! `nexus spend calls` and `nexus spend --csv` (`spend.ts`): each model call of the organization, newest
//! first, with the spend list's filters; `--csv` and `--jsonl` write every matching call to a file.

use serde_json::Value;

use crate::cli::Cmd;
use crate::client::search;
use crate::error::{Result, usage};
use crate::exports;
use crate::json::{a, js_string, s};
use crate::style::{col, flex, list, rows_of};

/// `nexus spend calls …`, or `nexus spend` with `--csv` or `--jsonl`. `None` when this is neither.
pub fn spend_calls_command(rest: &[String], cmd: &Cmd) -> Result<Option<i32>> {
    let format = exports::format(cmd)?;
    let listing = rest.first().map(String::as_str) == Some("calls");
    if format.is_none() && !listing {
        return Ok(None);
    }
    if let Some(format) = format {
        exports::check_out(cmd)?;
        if exports::given(cmd, "by") {
            return usage(
                "--by does not apply to the calls file. For the usage file: nexus analytics --csv.",
                None,
            );
        }
        let query = exports::filters(cmd, exports::SPEND_FILTERS, None);
        return exports::write_file(cmd, "/v1/spend/calls/export", format, &query).map(Some);
    }
    if exports::given(cmd, "by") {
        return usage(
            "--by does not apply to the calls list. For the table: nexus spend --by team.",
            None,
        );
    }
    let mut query: Vec<(&str, Option<String>)> = exports::SPEND_FILTERS
        .iter()
        .map(|name| (*name, cmd.flag_set_str(name)))
        .collect();
    query.push(("limit", cmd.flag_set_str("limit")));
    let page = cmd.core.get(&format!("/v1/spend/calls{}", search(&query)))?;
    if cmd.json {
        return Ok(Some(cmd.print(&page)));
    }
    let style = cmd.style;
    let range = format!("{} to {}", s(&page, "from").unwrap_or(""), s(&page, "to").unwrap_or(""));
    let calls = a(&page, "calls");
    let next = s(&page, "nextCursor").filter(|next| !next.is_empty());
    // A page stops at Gateway's scan bound, so an empty page can still have more calls after it.
    if calls.is_empty() {
        cmd.out(style.dim(&if next.is_some() {
            format!("No calls on this page, from {range}.")
        } else {
            format!("No calls from {range}.")
        }));
        more(cmd, next);
        return Ok(Some(0));
    }
    cmd.out(format!("{} {} {range}", style.bold("Calls"), style.bullet));
    cmd.out("");
    let rows: Vec<Vec<String>> = calls
        .iter()
        .map(|call| {
            vec![
                style.dim(
                    &s(call, "at")
                        .unwrap_or("")
                        .chars()
                        .take(19)
                        .collect::<String>()
                        .replacen('T', " ", 1),
                ),
                js_string(call.get("status")),
                product_text(call),
                s(call, "model").unwrap_or("-").to_string(),
                js_string(call.get("costUsd")),
                s(call, "userId").unwrap_or("-").to_string(),
            ]
        })
        .collect();
    cmd.lines(list(
        style,
        &[
            col("Time"),
            col("Status"),
            col("Product"),
            flex("Model"),
            col("Cost"),
            col("Person"),
        ],
        rows_of(rows),
        None,
    ));
    more(cmd, next);
    Ok(Some(0))
}

/// The next page's cursor, as the command to run.
fn more(cmd: &Cmd, next: Option<&str>) {
    if let Some(next) = next {
        cmd.out("");
        cmd.out(
            cmd.style
                .dim(&format!("More: add --cursor {next} to the same command.")),
        );
    }
}

/// Analytics' key: the product, or `product/capability`.
fn product_text(call: &Value) -> String {
    let product = js_string(call.get("product"));
    match s(call, "capability") {
        Some(capability) => format!("{product}/{capability}"),
        None => product,
    }
}
