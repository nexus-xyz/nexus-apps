//! `--csv` and `--jsonl` on `nexus spend`, `nexus analytics` and `nexus agents` (`exports.ts`): one exported
//! file, written as Gateway sent it, byte for byte, so its manifest's count and checksum hold. The request
//! is sent once and the file is written only once it has fully arrived: a file cut part way writes nothing and fails.
//! A JSON Lines file ends with its manifest line; a CSV file's manifest is its audit event's, found by the export's id.

use nexus_client::blocking::Download;
use serde_json::{Value, json};

use crate::cli::Cmd;
use crate::client::search;
use crate::error::{CliError, Error, Result, usage};

/// The spend calls' filters (`SpendCallFilters`), in their query order; each is the flag of the same name.
pub const SPEND_FILTERS: &[&str] = &[
    "from", "to", "team", "person", "model", "provider", "product", "status", "session", "cursor",
];
/// The usage file's filters (`UsageFilters`).
pub const USAGE_FILTERS: &[&str] = &["from", "to", "team", "person", "model", "provider", "product", "cursor"];
/// The agent sessions file's filters (`AgentSessionFilters`).
pub const SESSION_FILTERS: &[&str] = &["from", "to", "team", "person", "agent", "via", "status", "cursor"];

const CUT: &str =
    "The file is cut: Nexus Gateway closed the connection before the end. Nothing was written; run the command again.";
const TIMED_OUT: &str =
    "The file is cut: it did not finish within 300 s. Nothing was written; narrow the dates or the filters.";
const NO_MANIFEST: &str =
    "The file is cut: its last line is not the manifest. Nothing was written; run the command again.";

/// The file format the command asks for (`--csv` or `--jsonl`), if any.
pub fn format(cmd: &Cmd) -> Result<Option<&'static str>> {
    match (cmd.has("csv"), cmd.has("jsonl")) {
        (true, true) => usage("Choose one of --csv and --jsonl.", None),
        (true, false) => Ok(Some("csv")),
        (false, true) => Ok(Some("jsonl")),
        (false, false) => Ok(None),
    }
}

/// `--out` named without a file.
pub fn check_out(cmd: &Cmd) -> Result<()> {
    if cmd.has("out") {
        return usage("--out needs a file name.", None);
    }
    Ok(())
}

/// Whether a flag was given at all, with a value or as a switch.
pub fn given(cmd: &Cmd, name: &str) -> bool {
    cmd.flag(name).is_some() || cmd.has(name)
}

/// The query of these filters: each set flag, in order. `from` may come from elsewhere (`--days`).
pub fn filters(cmd: &Cmd, names: &[&str], from: Option<String>) -> String {
    let query: Vec<(&str, Option<String>)> = names
        .iter()
        .map(|name| match *name {
            "from" => ("from", from.clone().or_else(|| cmd.flag_set_str("from"))),
            // A person may name an agent by its command (`agy`); Gateway filters by its id (`antigravity`).
            "agent" => (
                "agent",
                cmd.flag_set_str("agent").map(|agent| {
                    nexus_client::agent_entries::Agent::parse_named(&agent)
                        .map_or(agent, |known| known.as_str().to_string())
                }),
            ),
            name => (name, cmd.flag_set_str(name)),
        })
        .collect();
    search(&query)
}

/// Download `<base>.<format><query>` and write it to stdout or `--out`.
pub fn write_file(cmd: &Cmd, base: &str, format: &str, query: &str) -> Result<i32> {
    check_out(cmd)?;
    let file = match cmd.core.download(&format!("{base}.{format}{query}"))? {
        Download::File(file) => file,
        Download::Cut(_) => return Err(CliError::new(CUT).into()),
        Download::TimedOut => return Err(CliError::new(TIMED_OUT).into()),
    };
    let manifest = if format == "jsonl" {
        match manifest_of(&file.body) {
            Some(manifest) => Some(manifest),
            None => return Err(CliError::new(NO_MANIFEST).into()),
        }
    } else {
        None
    };
    if let Some(out) = cmd.args.flag_set("out") {
        std::fs::write(cmd.ctx.cwd.join(out), &file.body)
            .map_err(|error| Error::Cli(CliError::new(format!("Could not write {out}: {error}"))))?;
        if cmd.json {
            return Ok(cmd.print(&json!({ "path": out, "bytes": file.body.len(), "exportId": file.export_id })));
        }
        cmd.out(format!("{} Wrote {out} ({} bytes).", cmd.style.ok, file.body.len()));
    } else {
        let text = String::from_utf8_lossy(&file.body);
        if !cmd.ctx.io.write(&text) {
            cmd.out(text.strip_suffix('\n').unwrap_or(&text));
        }
    }
    if cmd.json {
        return Ok(0);
    }
    if let (Some(id), "csv") = (&file.export_id, format) {
        cmd.ctx.io.err(&format!(
            "Export {id}: its manifest (rows, checksum) is in the audit log: nexus audit --target {id} --json"
        ));
    }
    if let Some(manifest) = manifest
        && manifest.get("complete") == Some(&Value::Bool(false))
        && let Some(next) = manifest.get("nextCursor").and_then(Value::as_str)
    {
        cmd.ctx.io.err(&format!(
            "The file stops at {} rows, the limit. For the rest: the same command with --cursor {next}",
            manifest.get("rows").map(Value::to_string).unwrap_or_default()
        ));
    }
    Ok(0)
}

/// A JSON Lines file's manifest: its last line, `{"manifest": {…}}`.
fn manifest_of(body: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(body).ok()?;
    let last = text.lines().rev().find(|line| !line.trim().is_empty())?;
    let line: Value = serde_json::from_str(last).ok()?;
    line.get("manifest").filter(|manifest| manifest.is_object()).cloned()
}

#[cfg(test)]
mod tests {
    use super::manifest_of;
    use serde_json::json;

    #[test]
    fn the_manifest_is_the_last_line() {
        let body = b"{\"id\":\"a\"}\n{\"manifest\":{\"rows\":1,\"complete\":true}}\n";
        assert_eq!(manifest_of(body), Some(json!({ "rows": 1, "complete": true })));
    }

    #[test]
    fn a_file_without_its_manifest_line_has_none() {
        assert_eq!(manifest_of(b"{\"id\":\"a\"}\n{\"id\":\"b\"}\n"), None);
        assert_eq!(manifest_of(b"{\"id\":\"a\"}\n{\"manifest\":"), None);
        assert_eq!(manifest_of(b""), None);
    }
}
