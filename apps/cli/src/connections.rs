//! `nexus connections`, `nexus connect` and `nexus disconnect`, as `connections.ts` does them.

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{CliError, Result, usage};
use crate::json::{a, js_string, s, t};
use crate::style::{Style, ago, col, flex, list, rows_of};

pub const CONNECT_USAGE: &str = "Usage: nexus connect <connector> [--org] [--write] [field=value ...]";
pub const DISCONNECT_USAGE: &str = "Usage: nexus disconnect <connector> <account> [--org]";
/// Under a form's usage: where its secrets come from.
pub const FORM_HINT: &str = "A field that ends in =- is read from stdin, and one that ends in =@<file> from a file only you can read, so a secret stays out of your shell history.";

/// `field=value` arguments as a body. A secret stays out of argv and shell history: a value of `-` is
/// read from stdin, for one field, and a value of `@<file>` from a file only its owner can read, for any field.
pub fn connect_fields(args: &[String], cwd: &Path, read_stdin: impl FnOnce() -> String) -> Result<Map<String, Value>> {
    let mut fields = Map::new();
    let mut read_stdin = Some(read_stdin);
    for arg in args {
        let Some((name, value)) = arg.split_once('=').filter(|(name, _)| !name.is_empty()) else {
            return Err(CliError::usage(
                CONNECT_USAGE,
                Some(&format!("Give each field as name=value; `{arg}` has no name.")),
            )
            .into());
        };
        let value = if value == "-" {
            let Some(read) = read_stdin.take() else {
                return usage(
                    "Only one field can read stdin (-).",
                    Some("Read each other secret from a file: field=@<file>."),
                );
            };
            let text = without_final_newline(read());
            if text.is_empty() {
                return usage(
                    format!("No value for {name} on stdin."),
                    Some(&format!("Pipe it in: echo \"$TOKEN\" | nexus connect … {name}=-")),
                );
            }
            text
        } else if let Some(path) = value.strip_prefix('@') {
            read_secret_file(name, path, cwd)?
        } else {
            value.to_string()
        };
        fields.insert(name.to_string(), json!(value));
    }
    Ok(fields)
}

/// The text without one final line break, as `echo` and an editor leave it.
fn without_final_newline(text: String) -> String {
    let text = text.strip_suffix('\n').unwrap_or(&text);
    text.strip_suffix('\r').unwrap_or(text).to_string()
}

/// A field's value from `field=@<file>`, relative to the working directory. Only the file's owner may read it (no
/// group or other read bit), as the CLI keeps its own secrets (0600). An error names the field and the path, never
/// the file's text.
fn read_secret_file(name: &str, path: &str, cwd: &Path) -> Result<String> {
    if path.is_empty() {
        return usage(format!("Give the file after @: {name}=@<file>."), None);
    }
    let full = cwd.join(path);
    let cannot_read = || -> crate::error::Error { CliError::new(format!("Cannot read {path} for {name}.")).into() };
    let metadata = match std::fs::metadata(&full) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CliError::hint(
                format!("No file {path} for {name}."),
                "Give the path of the file that holds the secret.",
            )
            .into());
        }
        Err(_) => return Err(cannot_read()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o044 != 0 {
            return Err(CliError::hint(
                format!("Other users can read {path}, so it is not sent as {name}."),
                format!("Make it yours alone: chmod 600 {path}"),
            )
            .into());
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    let text = std::fs::read_to_string(&full).map_err(|_| cannot_read())?;
    let text = without_final_newline(text);
    if text.is_empty() {
        return Err(CliError::new(format!("No value for {name} in {path}.")).into());
    }
    Ok(text)
}

/// The fields a connector's form takes, as `nexus connect` arguments: the first secret one reads stdin, and each
/// other secret one a file.
fn form_args(connector: &Value) -> Vec<String> {
    let mut stdin_taken = false;
    connector
        .pointer("/form/properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .map(|(name, field)| {
                    if !t(field, "x-nexus-secret") {
                        format!("{name}=<{name}>")
                    } else if std::mem::replace(&mut stdin_taken, true) {
                        format!("{name}=@<file>")
                    } else {
                        format!("{name}=-")
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A form connector's usage line, with its fields as `form_args` names them.
pub fn form_usage(connector: &Value) -> String {
    format!(
        "Usage: nexus connect {} [--org] {}",
        s(connector, "name").unwrap_or_default(),
        form_args(connector).join(" ")
    )
}

/// Under an OAuth link: the key form, for a holder who prefers a key, in the scope the link was for.
fn key_instead(connector: &Value, scope: &str) -> Option<String> {
    let fields = form_args(connector);
    if fields.is_empty() {
        return None;
    }
    let org = if scope == "org" { " --org" } else { "" };
    Some(format!(
        "Or paste a key instead: nexus connect {}{org} {}",
        s(connector, "name").unwrap_or_default(),
        fields.join(" ")
    ))
}

fn scope(cmd: &Cmd) -> &'static str {
    if cmd.has("org") { "org" } else { "user" }
}

fn holder_of(connection: &Value) -> &'static str {
    if s(connection, "scope") == Some("org") {
        "organization"
    } else {
        "you"
    }
}

fn status_of(style: &Style, connection: &Value) -> String {
    if s(connection, "status") == Some("active") {
        return style.green("active");
    }
    let org = if s(connection, "scope") == Some("org") {
        " --org"
    } else {
        ""
    };
    style.yellow(&format!(
        "needs reconnect: nexus connect {}{org}",
        js_string(connection.get("connector"))
    ))
}

fn connected_line(style: &Style, connection: &Value) -> String {
    let identity = s(connection, "identity")
        .map(|who| format!(" as {who}"))
        .unwrap_or_default();
    let whose = if s(connection, "scope") == Some("org") {
        " for the organization"
    } else {
        ""
    };
    format!(
        "{} Connected {} {}{identity}{whose}.",
        style.ok,
        js_string(connection.get("connector")),
        js_string(connection.get("name"))
    )
}

pub fn connections_command(cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let response = cmd.core.get("/v1/connections")?;
    if cmd.json {
        return Ok(cmd.print(&response));
    }
    let connections = a(&response, "connections");
    if connections.is_empty() {
        cmd.out(format!(
            "No connections yet. Connect one: {} (nexus connectors lists them).",
            style.cyan("nexus connect <connector> field=value …")
        ));
        return Ok(0);
    }
    let rows: Vec<Vec<String>> = connections
        .iter()
        .map(|c| {
            vec![
                js_string(c.get("connector")),
                js_string(c.get("name")),
                holder_of(c).to_string(),
                s(c, "identity").unwrap_or("—").to_string(),
                ago(s(c, "createdAt").unwrap_or_default()),
                status_of(style, c),
            ]
        })
        .collect();
    cmd.lines(list(
        style,
        &[
            col("Connector"),
            col("Account"),
            col("Holder"),
            col("Connected as"),
            col("Added"),
            flex("Status"),
        ],
        rows_of(rows),
        None,
    ));
    Ok(0)
}

pub fn connect_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let Some(name) = args.first() else {
        return usage(CONNECT_USAGE, Some("List the connectors with `nexus connectors`."));
    };
    let scope = scope(cmd);
    // `--write` asks the vendor for the write scopes a write refused with `write_consent_required` needs.
    // It is not a global switch, so the grammar may have taken the next argument as its value: a field all the same.
    let written = cmd.flag("write");
    let write = cmd.has("write") || written.is_some();
    if write && (args.len() > 1 || written.is_some_and(|value| !value.is_empty())) {
        return usage(
            CONNECT_USAGE,
            Some("--write allows writes through the vendor's OAuth link: give it no fields."),
        );
    }
    let mut fields = connect_fields(&args[1..], &cmd.ctx.cwd, || cmd.ctx.io.read_stdin())?;
    if fields.is_empty() {
        let response = cmd.core.get("/v1/connectors")?;
        let Some(connector) = a(&response, "connectors")
            .iter()
            .find(|entry| s(entry, "name") == Some(name.as_str()))
        else {
            return Err(CliError::hint(
                format!("No connector {name}."),
                "List the connectors with `nexus connectors`.",
            )
            .into());
        };
        let has_form = connector.get("form").is_some_and(|form| !form.is_null());
        let connects = !a(connector, "connects").is_empty();
        // OAuth first when this Gateway has Nexus's app at the vendor, as Nexus UI does: the key form,
        // if the connector has one, is the fallback named under the link.
        let oauth = connects && t(connector, "oauth");
        if has_form && !oauth && !write {
            return usage(form_usage(connector), Some(FORM_HINT));
        }
        if !connects {
            return Err(CliError::new(format!(
                "{name} has nothing to connect: it uses the organization's own setup."
            ))
            .into());
        }
        if !oauth {
            return Err(CliError::hint(
                format!("This Gateway has no way to connect {name} yet."),
                "Ask whoever runs it to set up the connector's OAuth app.",
            )
            .into());
        }
        let body = if write {
            json!({ "scope": scope, "access": "write" })
        } else {
            json!({ "scope": scope })
        };
        let started = cmd.core.post(&format!("/v1/connections/{}/oauth", enc(name)), body)?;
        let url = js_string(started.get("authorizeUrl"));
        if cmd.json {
            return Ok(cmd.print(&json!({ "authorizeUrl": url })));
        }
        if write {
            cmd.out(format!(
                "Open this link to allow writes to {name}. It works once, for ten minutes:"
            ));
            cmd.out(format!("  {url}"));
            return Ok(0);
        }
        cmd.out(format!(
            "Open this link to connect {name}. It works once, for ten minutes:"
        ));
        cmd.out(format!("  {url}"));
        if let Some(key) = key_instead(connector, scope) {
            cmd.out(key);
        }
        return Ok(0);
    }
    fields.insert("scope".to_string(), json!(scope));
    let response = cmd
        .core
        .post(&format!("/v1/connections/{}", enc(name)), Value::Object(fields))?;
    if cmd.json {
        return Ok(cmd.print(&json!({ "connections": response.get("connections") })));
    }
    cmd.lines(a(&response, "connections").iter().map(|c| connected_line(cmd.style, c)));
    Ok(0)
}

pub fn disconnect_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let (Some(name), Some(account)) = (args.first(), args.get(1)) else {
        return usage(DISCONNECT_USAGE, Some("`nexus connections` lists your accounts."));
    };
    let query = if scope(cmd) == "org" { "?scope=org" } else { "" };
    let response = cmd
        .core
        .delete(&format!("/v1/connections/{}/{}{query}", enc(name), enc(account)))?;
    if cmd.json {
        return Ok(cmd.print(&response));
    }
    let disconnected = response.get("disconnected").cloned().unwrap_or(Value::Null);
    cmd.out(format!(
        "{} Disconnected {} {}.",
        cmd.style.ok,
        js_string(disconnected.get("connector")),
        js_string(disconnected.get("name"))
    ));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    /// A fresh folder with files of the given text and mode.
    fn folder(files: &[(&str, &str, u32)]) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "nexus-connect-fields-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        for (name, text, mode) in files {
            let path = dir.join(name);
            std::fs::write(&path, text).expect("write");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(*mode)).expect("chmod");
            }
            #[cfg(not(unix))]
            let _ = mode;
        }
        dir
    }

    fn message(error: crate::error::Error) -> String {
        match error {
            crate::error::Error::Cli(error) => format!("{} {}", error.message, error.hint.unwrap_or_default()),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn fields_read_one_secret_from_stdin() {
        let fields = connect_fields(&args(&["site=acme.atlassian.net", "token=-"]), Path::new("."), || {
            "s3cret\n".into()
        })
        .expect("fields");
        assert_eq!(
            Value::Object(fields),
            json!({ "site": "acme.atlassian.net", "token": "s3cret" })
        );
    }

    #[test]
    fn fields_read_two_secrets_one_from_stdin_and_one_from_a_file() {
        let dir = folder(&[("dd-app.key", "app-s3cret\r\n", 0o600)]);
        let fields = connect_fields(
            &args(&["site=datadoghq.com", "api_key=-", "application_key=@dd-app.key"]),
            &dir,
            || "api-s3cret\n".into(),
        )
        .expect("fields");
        assert_eq!(
            Value::Object(fields),
            json!({ "site": "datadoghq.com", "api_key": "api-s3cret", "application_key": "app-s3cret" })
        );
    }

    #[test]
    fn fields_refuse_two_stdin_values_an_empty_one_and_no_name() {
        let two = connect_fields(&args(&["a=-", "b=-"]), Path::new("."), || "x".into()).expect_err("two");
        assert!(message(two).contains("field=@<file>"));
        assert!(connect_fields(&args(&["token=-"]), Path::new("."), String::new).is_err());
        assert!(connect_fields(&args(&["=value"]), Path::new("."), String::new).is_err());
        assert!(connect_fields(&args(&["value"]), Path::new("."), String::new).is_err());
    }

    #[test]
    fn a_file_that_is_missing_empty_or_open_to_others_is_refused_without_its_text() {
        let dir = folder(&[("empty.key", "\n", 0o600), ("open.key", "open-s3cret", 0o644)]);
        let read = |arg: &str| {
            connect_fields(&args(&[arg]), &dir, String::new)
                .map(|_| ())
                .map_err(message)
        };
        let missing = read("api_key=@nope.key").expect_err("missing");
        assert!(missing.contains("No file nope.key for api_key."), "{missing}");
        assert_eq!(
            read("api_key=@empty.key").expect_err("empty"),
            "No value for api_key in empty.key. "
        );
        assert!(
            read("api_key=@")
                .expect_err("no path")
                .contains("Give the file after @")
        );
        #[cfg(unix)]
        {
            let open = read("api_key=@open.key").expect_err("open");
            assert!(open.contains("Other users can read open.key"), "{open}");
            assert!(open.contains("chmod 600 open.key"), "{open}");
            assert!(!open.contains("open-s3cret"), "{open}");
        }
    }

    #[test]
    fn form_usage_marks_the_first_secret_for_stdin_and_the_others_for_a_file() {
        let jira = json!({
            "name": "jira",
            "form": { "properties": { "email": {}, "site": {}, "token": { "x-nexus-secret": true } } }
        });
        assert_eq!(
            form_usage(&jira),
            "Usage: nexus connect jira [--org] email=<email> site=<site> token=-"
        );
        let datadog = json!({
            "name": "datadog",
            "form": { "properties": { "site": {}, "api_key": { "x-nexus-secret": true }, "application_key": { "x-nexus-secret": true } } }
        });
        assert_eq!(
            form_usage(&datadog),
            "Usage: nexus connect datadog [--org] site=<site> api_key=- application_key=@<file>"
        );
    }

    #[test]
    fn key_instead_names_the_form_in_the_links_scope_and_nothing_without_a_form() {
        let linear = json!({
            "name": "linear", "oauth": true,
            "form": { "properties": { "token": { "x-nexus-secret": true } } }
        });
        assert_eq!(
            key_instead(&linear, "user").as_deref(),
            Some("Or paste a key instead: nexus connect linear token=-")
        );
        assert_eq!(
            key_instead(&linear, "org").as_deref(),
            Some("Or paste a key instead: nexus connect linear --org token=-")
        );
        assert_eq!(key_instead(&json!({ "name": "notion", "form": null }), "user"), None);
    }
}
