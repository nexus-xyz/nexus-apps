//! `nexus skills` (`skills.ts`): a person's own SKILL.md files, in versions that never change (the API is
//! `/v1/skills`). `import` takes a file, or a folder that holds a SKILL.md and sends that file alone, naming
//! the files it left out. `export` writes `<dir>/<name>/SKILL.md` as it was written: the bytes Gateway stored, not a
//! decoded copy, so a byte-order mark and CRLF line ends survive. `create` opens `$VISUAL` or `$EDITOR` on a skeleton.
//! Every refusal Gateway gives for a skill prints its code and the fix (`skill_errors.rs`). The verbs for promotion
//! requests and the library are in `promotion_verbs.rs` and `skill_library.rs`; `export` also exports a
//! library skill, found as chat finds it.

use std::path::Path;

use nexus_client::blocking::Download;
use serde_json::{Value, json};

use crate::catalog::SKILL_COMMANDS;
use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{CliError, CoreError, Error, Result, usage};
use crate::json::{a, js_string, s};
use crate::promotion_verbs;
use crate::safe_text::safe_text;
use crate::skill_errors::refused;
use crate::skill_library::{exported_from, library_for, resolve};
use crate::style::{Style, col, flex, list, pad_end, quote, right, rows_of};
use nexus_client::base::sha256_hex;

/// The file names shown after "Not sent", before "and N more".
const NOT_SENT_SHOWN: usize = 10;

pub fn skills_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus skills <command>", style.bold("Usage")),
        String::new(),
        "Skills are SKILL.md files, kept in versions that never change. Import one to use it in your agents."
            .to_string(),
        String::new(),
        style.bold("Commands"),
    ];
    // The column is as wide as the longest usage and two spaces, so no usage runs into its text.
    let width = SKILL_COMMANDS
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    lines.extend(
        SKILL_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, width)))),
    );
    lines
}

/// The command names of `nexus skills help --json`, once each.
pub fn skill_command_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for (name, _) in SKILL_COMMANDS {
        let first = name.split(' ').next().unwrap_or("");
        if !names.contains(&first) {
            names.push(first);
        }
    }
    names
}

/// 1 to 64 characters of `a-z`, `0-9` and single hyphens, none at either end: the open format's rule for a name, as
/// Gateway checks it. Checked here so an editor session is never lost to a name Gateway refuses.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

/// What `nexus skills create` opens: valid as it stands, so it is told apart from an edited file by comparing text.
fn skeleton(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Say what this skill does and when an agent should use it.\n---\n\n# {name}\n\nWrite the instructions here.\n"
    )
}

pub(crate) fn version_number(value: Option<&str>, what: &str) -> Result<i64> {
    let digits = value.filter(|value| value.bytes().all(|byte| byte.is_ascii_digit()));
    match digits.and_then(|value| value.parse::<i64>().ok()) {
        Some(number) if number >= 1 => Ok(number),
        _ => usage(format!("{what} must be a version number (1 or more), or latest."), None),
    }
}

/// Where a library skill is: the organization's library, or a team's.
pub(crate) struct Source {
    pub scope: String,
    pub team_id: Option<String>,
}

/// A skill, found by name or id: one of the person's own, or (`library`) a library skill at its approved version.
pub(crate) struct Found {
    pub id: String,
    pub name: String,
    pub latest: i64,
    pub library: Option<Source>,
}

fn found_in(skills: &Value, who: &str) -> Option<Found> {
    let wanted = who.trim();
    let name = wanted.to_lowercase();
    a(skills, "skills")
        .iter()
        .find(|skill| s(skill, "id") == Some(wanted) || s(skill, "name") == Some(name.as_str()))
        .map(|skill| Found {
            id: js_string(skill.get("id")),
            name: js_string(skill.get("name")),
            latest: skill.get("latestVersion").and_then(Value::as_i64).unwrap_or(0),
            library: None,
        })
}

/// One of the person's own skills, found by name or id.
pub(crate) fn find(cmd: &Cmd, who: &str) -> Result<Found> {
    let skills = cmd.core.get("/v1/skills").map_err(refused)?;
    match found_in(&skills, who) {
        Some(found) => Ok(found),
        None => Err(refused(Error::Core(Box::new(CoreError {
            status: 404,
            message: format!("you have no skill named {who}"),
            code: Some("skill_not_found".into()),
            interval: None,
            details: None,
        })))),
    }
}

/// One line for a description: runs of white space become one space.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What `import --update` answers for a file that is already the skill's latest version, in place of a write's result:
/// `unchanged: true`, the skill, and the version without its text. `written` prints it with the verb `Unchanged`.
fn unchanged(skill_id: &str, latest: &Value) -> Value {
    let mut version = latest.clone();
    if let Some(fields) = version.as_object_mut() {
        fields.shift_remove("content");
    }
    json!({ "unchanged": true, "skill": { "id": skill_id, "name": js_string(latest.get("name")) }, "version": version })
}

fn written(result: &Value, verb: &str, not_sent: &[String], cmd: &Cmd) -> Result<i32> {
    if cmd.json {
        let mut shown = result.clone();
        shown["notSent"] = json!(not_sent);
        return Ok(cmd.print(&shown));
    }
    let skill = &result["skill"];
    let version = &result["version"];
    let name = js_string(skill.get("name"));
    let number = js_string(version.get("version"));
    let bytes = js_string(version.get("bytes"));
    match verb {
        "Updated" => cmd.out(format!(
            "{} Updated {name} to version {number} ({bytes} bytes).",
            cmd.style.ok
        )),
        "Unchanged" => cmd.out(format!(
            "{} {name} is already version {number} ({bytes} bytes). Nothing written.",
            cmd.style.ok
        )),
        _ => cmd.out(format!(
            "{} {verb} {name}, version {number} ({bytes} bytes).",
            cmd.style.ok
        )),
    }
    if !not_sent.is_empty() {
        let shown = not_sent
            .iter()
            .take(NOT_SENT_SHOWN)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = match not_sent.len().saturating_sub(NOT_SENT_SHOWN) {
            0 => String::new(),
            rest => format!(" and {rest} more"),
        };
        cmd.out(format!(
            "{} Sent SKILL.md only. Not sent: {shown}{more}.",
            cmd.style.warn
        ));
    }
    Ok(0)
}

/// The files in a skill folder besides its SKILL.md, as `references/guide.md` paths, sorted. Links are not followed.
fn other_files(root: &Path) -> Result<Vec<String>> {
    let cannot = |path: &Path, error: std::io::Error| {
        Error::Cli(CliError::new(format!("Could not read {}: {error}", path.display())))
    };
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|error| cannot(&dir, error))? {
            let entry = entry.map_err(|error| cannot(&dir, error))?;
            let kind = entry.file_type().map_err(|error| cannot(&dir, error))?;
            if kind.is_dir() {
                pending.push(entry.path());
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(root)
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if relative != "SKILL.md" {
                found.push(relative);
            }
        }
    }
    found.sort();
    Ok(found)
}

pub fn skills_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    let first = args.get(1).filter(|value| !value.is_empty()).map(String::as_str);
    let second = args.get(2).map(String::as_str);
    let need = |what: &str| -> Result<&str> {
        match first {
            Some(first) => Ok(first),
            None => usage(
                format!("Usage: nexus skills {action} {what}"),
                Some("`nexus skills` lists your skills."),
            ),
        }
    };

    if action == "help" || (cmd.has("help") && args.is_empty()) {
        if cmd.json {
            return Ok(cmd.print(&json!({ "usage": "nexus skills <command>", "commands": skill_command_names() })));
        }
        cmd.lines(skills_help(style));
        return Ok(0);
    }

    match action {
        "list" => {
            let skills = cmd.core.get("/v1/skills").map_err(refused)?;
            if cmd.json {
                return Ok(cmd.print(&skills));
            }
            let items = a(&skills, "skills");
            if items.is_empty() {
                cmd.out(format!(
                    "No skills yet. Import one: {}",
                    style.cyan("nexus skills import ./my-skill")
                ));
                return Ok(0);
            }
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    vec![
                        js_string(item.get("name")),
                        format!("v{}", js_string(item.get("latestVersion"))),
                        one_line(&js_string(item.get("description"))),
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[col("Skill"), right("Version"), flex("Description")],
                rows_of(rows),
                None,
            ));
            Ok(0)
        }
        "import" => {
            let path = need("<path>")?;
            let target = cmd.ctx.cwd.join(path);
            let (file, not_sent) = if target.is_dir() {
                let file = target.join("SKILL.md");
                if !file.is_file() {
                    return usage(
                        format!("{path} is a folder with no SKILL.md."),
                        Some("A skill folder holds a file named SKILL.md."),
                    );
                }
                (file, other_files(&target)?)
            } else {
                (target, Vec::new())
            };
            let content = std::fs::read_to_string(&file).or_else(|error| {
                let why = match error.kind() {
                    std::io::ErrorKind::InvalidData => "the file is not valid UTF-8".to_string(),
                    std::io::ErrorKind::NotFound => "no such file or directory".to_string(),
                    std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
                    _ => error.to_string(),
                };
                usage(format!("Could not read {path}: {why}"), None)
            })?;
            let (result, verb) = match cmd.core.post("/v1/skills", json!({ "content": content })) {
                Ok(result) => (result, "Imported"),
                Err(Error::Core(core)) if cmd.has("update") && core.code.as_deref() == Some("duplicate_name") => {
                    // Gateway names the skill that has the name, so no front matter is read here.
                    let Some(id) = core
                        .details
                        .as_ref()
                        .and_then(|details| details.get("skillId"))
                        .and_then(Value::as_str)
                    else {
                        return Err(refused(Error::Core(core)));
                    };
                    // A file that is already the latest version writes nothing, so the same import can run again
                    // (after a pull) without adding a version.
                    let latest = cmd
                        .core
                        .get(&format!("/v1/skills/{}/versions/latest", enc(id)))
                        .map_err(refused)?;
                    if js_string(latest.get("contentSha256")) == sha256_hex(&[content.as_bytes()]) {
                        (unchanged(id, &latest), "Unchanged")
                    } else {
                        // Built on the version just read: if someone wrote a newer one meanwhile, Gateway refuses
                        // (409 skill_version_conflict) and nothing of theirs is overwritten.
                        let result = cmd
                            .core
                            .post(
                                &format!("/v1/skills/{}/versions", enc(id)),
                                json!({ "content": content, "baseVersion": latest.get("version") }),
                            )
                            .map_err(refused)?;
                        (result, "Updated")
                    }
                }
                Err(error) => return Err(refused(error)),
            };
            written(&result, verb, &not_sent, cmd)
        }
        "create" => {
            let name = need("<name>")?;
            if !valid_name(name) {
                return usage(
                    format!(
                        "{name:?} is not a skill name. A name is 1 to 64 characters of a-z, 0-9 and single hyphens."
                    ),
                    None,
                );
            }
            let editor = ["VISUAL", "EDITOR"]
                .iter()
                .filter_map(|variable| cmd.ctx.var(variable))
                .find(|value| !value.trim().is_empty());
            let Some(editor) = editor else {
                return usage(
                    "No editor: set $VISUAL or $EDITOR.",
                    Some("Or write the file yourself and run `nexus skills import <path>`."),
                );
            };
            let skills = cmd.core.get("/v1/skills").map_err(refused)?;
            if found_in(&skills, name).is_some() {
                return usage(
                    format!("You already have a skill named {name}."),
                    Some(&format!(
                        "Export it, change it, then run `nexus skills import {name}/SKILL.md --update`."
                    )),
                );
            }
            // A folder of its own, made by this run for this person alone: nobody else can plant a link in it.
            let folder = std::env::temp_dir().join(format!("nexus-skill-{}", uuid::Uuid::new_v4()));
            let file = folder.join("SKILL.md");
            let kept = |what: &str| format!("{what} Your text is kept in {}.", file.display());
            let text = skeleton(name);
            let mut private = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut private, 0o700);
            private
                .create(&folder)
                .and_then(|()| std::fs::write(&file, &text))
                .map_err(|error| Error::Cli(CliError::new(format!("Could not write {}: {error}", file.display()))))?;
            let command: Vec<String> = editor.split_whitespace().map(String::from).collect();
            match cmd.ctx.io.edit(&command, &file) {
                Ok(0) => {}
                Ok(code) => {
                    return Err(CliError::new(kept(&format!(
                        "{editor} exited with code {code}, so no skill was created."
                    )))
                    .into());
                }
                Err(error) => return Err(CliError::new(kept(&format!("Could not start {editor}: {error}."))).into()),
            }
            let content = std::fs::read_to_string(&file)
                .or_else(|error| usage(kept(&format!("Could not read the file back: {error}.")), None))?;
            if content == text {
                let _ = std::fs::remove_dir_all(&folder);
                return Err(CliError::new("Nothing changed, so no skill was created.").into());
            }
            let result = match cmd.core.post("/v1/skills", json!({ "content": content })) {
                Ok(result) => result,
                Err(error) => {
                    cmd.ctx.io.err(&format!(
                        "Your text is kept in {}. Fix it, then run: nexus skills import {}",
                        file.display(),
                        file.display()
                    ));
                    return Err(refused(error));
                }
            };
            let _ = std::fs::remove_dir_all(&folder);
            written(&result, "Created", &[], cmd)
        }
        "export" => {
            let who = need("<name|id>")?;
            let explicit_version = !matches!(second, None | Some("latest"));
            let mut version = match second {
                None | Some("latest") => "latest".to_string(),
                Some(value) => version_number(Some(value), "The version")?.to_string(),
            };
            if cmd.has("out") || cmd.args.flag_set("out") == Some("") {
                return usage("--out needs a folder name.", None);
            }
            // As chat resolves a name: the person's own skills and the library.
            let own = cmd.core.get("/v1/skills").map_err(refused)?;
            let library = library_for(cmd, &own, who)?;
            let skill = resolve(cmd, &own, &library, who, explicit_version)?;
            // The name becomes a folder: a name that Gateway would never accept is not written anywhere.
            if !valid_name(&skill.name) {
                return Err(CliError::new(format!(
                    "Gateway sent {} as the skill's name, which is not a skill name. Nothing was written.",
                    quote(&safe_text(&skill.name))
                ))
                .into());
            }
            // The latest version of a library skill is the one the list showed: a version approved since is not written
            // under that number.
            if skill.library.is_some() && skill.latest >= 1 && !explicit_version {
                version = skill.latest.to_string();
            }
            let file = match cmd
                .core
                .download(&format!("/v1/skills/{}/versions/{version}/export", enc(&skill.id)))
                .map_err(refused)?
            {
                Download::File(file) => file,
                Download::Cut(_) => {
                    return Err(CliError::new(
                        "The file is cut: Nexus Gateway closed the connection before the end. Nothing was written; run the command again.",
                    )
                    .into());
                }
                Download::TimedOut => {
                    return Err(CliError::new(
                        "The file is cut: it did not arrive in time. Nothing was written; run the command again.",
                    )
                    .into());
                }
            };
            let out = cmd.args.flag_set("out");
            let folder = cmd.ctx.cwd.join(out.unwrap_or(".")).join(&skill.name);
            let shown = match out {
                Some(out) => format!("{}/{}/SKILL.md", out.trim_end_matches('/'), skill.name),
                None => format!("{}/SKILL.md", skill.name),
            };
            std::fs::create_dir_all(&folder)
                .and_then(|()| std::fs::write(folder.join("SKILL.md"), &file.body))
                .map_err(|error| Error::Cli(CliError::new(format!("Could not write {shown}: {error}"))))?;
            let number = match version.parse::<i64>() {
                Ok(number) => number,
                Err(_) => skill.latest,
            };
            if cmd.json {
                let mut printed = json!({ "id": skill.id, "name": skill.name, "version": number, "path": shown, "bytes": file.body.len() });
                if let Some(source) = &skill.library {
                    printed["scope"] = json!(source.scope);
                    if let Some(team) = &source.team_id {
                        printed["teamId"] = json!(team);
                    }
                }
                return Ok(cmd.print(&printed));
            }
            cmd.out(format!(
                "{} Wrote {shown} (version {number}, {} bytes).",
                style.ok,
                file.body.len()
            ));
            if let Some(source) = &skill.library {
                cmd.out(format!("  {}", exported_from(cmd, &skill, source, number)));
            }
            Ok(0)
        }
        "delete" => {
            let who = need("<name|id>")?;
            if !cmd.has("yes") {
                return usage(
                    format!("Deleting {who} removes every version for good."),
                    Some(&format!("nexus skills delete {who} --yes")),
                );
            }
            let skill = find(cmd, who)?;
            let deleted = cmd
                .core
                .delete(&format!("/v1/skills/{}", enc(&skill.id)))
                .map_err(refused)?;
            if cmd.json {
                return Ok(cmd.print(&deleted));
            }
            let versions = deleted.get("versions").and_then(Value::as_i64).unwrap_or(0);
            cmd.out(format!(
                "{} Deleted {} and its {versions} {}.",
                style.ok,
                skill.name,
                if versions == 1 { "version" } else { "versions" }
            ));
            Ok(0)
        }
        "promote" => promotion_verbs::promote(args, cmd),
        "promotions" => promotion_verbs::promotions(args, cmd),
        "approve" => promotion_verbs::decide(args, cmd, false),
        "refuse" => promotion_verbs::decide(args, cmd, true),
        "withdraw" => promotion_verbs::withdraw(args, cmd),
        "library" => crate::skill_library::library(cmd),
        _ => usage(
            format!("Unknown command `nexus skills {action}`."),
            Some(
                "Use list, create, import, export, delete, promote, promotions, approve, refuse, withdraw or library.",
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_the_open_formats() {
        for good in ["pdf-processing", "a", "a1-b2", &"a".repeat(64)] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "PDF", "-pdf", "pdf-", "pdf--x", "pdf_x", "pdf x", &"a".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_skeleton_is_valid_and_names_the_skill() {
        let text = skeleton("release-notes");
        assert!(text.starts_with("---\nname: release-notes\ndescription: "));
        assert!(text.contains("\n---\n\n# release-notes\n"));
    }

    #[test]
    fn a_description_is_one_line() {
        assert_eq!(one_line("Extract  text\n from PDFs.\t"), "Extract text from PDFs.");
    }
}
