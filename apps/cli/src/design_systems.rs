//! `nexus design-system` (`design-systems.ts`): Nexus Design's design systems, the tokens and notes designs
//! are made, served and checked with. The built-in `@nexus/brand` (brand v5.0) is the default until the organization
//! picks another. `sync` walks a folder by the same rules as the TypeScript CLI and sends the files as they are;
//! Gateway reads the tokens out of them.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::catalog::DESIGN_SYSTEM_COMMANDS;
use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{CliError, Error, Result, usage};
use crate::json::{a, js_string};
use crate::style::{Style, col, list, pad_end, pairs, right, rows_of};

/// The folders a sync never reads.
pub const SKIPPED_DIRS: [&str; 7] = ["node_modules", "dist", "build", "out", "target", "vendor", "coverage"];
pub const MAX_DEPTH: usize = 8;
pub const MAX_VISITED: usize = 20_000;
/// What Gateway takes: at most 40 files, 4 MiB together.
pub const MAX_FILES: usize = 40;
pub const MAX_FILES_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_TOKEN_FILE_BYTES: u64 = 512 * 1024;
pub const MAX_CSS_FILE_BYTES: u64 = 256 * 1024;
pub const MAX_NOTES_BYTES: usize = 64 * 1024;
pub const MAX_COMPONENTS: usize = 500;
const NOTES_FILES: [&str; 4] = [
    "DESIGN.md",
    "design-system.md",
    "docs/DESIGN.md",
    "docs/design-system.md",
];

pub fn design_system_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!("{}  nexus design-system <command>", style.bold("Usage")),
        String::new(),
        "A design system is the tokens and notes Nexus Design makes, serves and checks designs with.".to_string(),
        String::new(),
        style.bold("Commands"),
    ];
    lines.extend(
        DESIGN_SYSTEM_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 40)))),
    );
    lines
}

/// The command names of `nexus design-system help --json`, once each.
pub fn design_system_command_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for (name, _) in DESIGN_SYSTEM_COMMANDS {
        let first = name.split(' ').next().unwrap_or("");
        if !names.contains(&first) {
            names.push(first);
        }
    }
    names
}

/// What a file is to a sync: `css`, `tailwind`, `tokens`, or nothing.
pub fn kind_of(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name == "tokens.json" || name.ends_with(".tokens.json") {
        return Some("tokens");
    }
    if let Some(ext) = name.strip_prefix("tailwind.config.")
        && matches!(ext, "js" | "cjs" | "mjs" | "ts")
    {
        return Some("tailwind");
    }
    (name.ends_with(".css") || name.ends_with(".scss")).then_some("css")
}

/// A component's name from its path under a `components/` folder: `Button.tsx`, or `Card/index.tsx`.
pub fn component_of(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    let file = parts.last()?;
    let (stem, ext) = file.rsplit_once('.')?;
    if !matches!(ext, "tsx" | "jsx" | "vue" | "svelte") || !parts[..parts.len() - 1].contains(&"components") {
        return None;
    }
    let name = if stem == "index" {
        *parts.get(parts.len().checked_sub(2)?)?
    } else {
        stem
    };
    let mut chars = name.chars();
    (chars.next().is_some_and(|c| c.is_ascii_uppercase()) && chars.all(|c| c.is_ascii_alphanumeric()))
        .then(|| name.to_string())
}

/// What a sync of a folder sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    pub files: Vec<(String, String)>,
    pub notes: Option<String>,
    pub components: Vec<String>,
    pub skipped: Vec<String>,
}

fn entries(dir: &Path) -> Vec<(String, bool)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<(String, bool)> = read
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            (kind.is_dir() || kind.is_file()).then(|| (entry.file_name().to_string_lossy().into_owned(), kind.is_dir()))
        })
        .collect();
    entries.sort();
    entries
}

fn file_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
}

/// The files a sync of `root` sends, in the order Gateway merges them: CSS, then Tailwind configs, then tokens.json.
pub fn collect_sources(root: &Path) -> Sources {
    let (mut css, mut tailwind, mut tokens) = (Vec::new(), Vec::new(), Vec::new());
    let mut components: Vec<String> = Vec::new();
    let mut visited = 0usize;
    // Depth first, in name order, as the TypeScript CLI walks.
    fn walk(
        root: &Path,
        dir: &str,
        depth: usize,
        visited: &mut usize,
        found: &mut [&mut Vec<String>; 3],
        components: &mut Vec<String>,
    ) {
        for (name, is_dir) in entries(&root.join(dir)) {
            *visited += 1;
            if *visited > MAX_VISITED {
                return;
            }
            let path = if dir.is_empty() {
                name.clone()
            } else {
                format!("{dir}/{name}")
            };
            if is_dir {
                if depth < MAX_DEPTH && !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_str()) {
                    walk(root, &path, depth + 1, visited, found, components);
                }
                continue;
            }
            match kind_of(&path) {
                Some("css") => found[0].push(path.clone()),
                Some("tailwind") => found[1].push(path.clone()),
                Some("tokens") => found[2].push(path.clone()),
                _ => {}
            }
            if let Some(component) = component_of(&path)
                && !components.contains(&component)
            {
                components.push(component);
            }
        }
    }
    walk(
        root,
        "",
        0,
        &mut visited,
        &mut [&mut css, &mut tailwind, &mut tokens],
        &mut components,
    );
    css.sort();
    tailwind.sort();
    tokens.sort();
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut bytes = 0u64;
    for path in css.into_iter().chain(tailwind).chain(tokens) {
        let full = root.join(&path);
        let size = file_size(&full).unwrap_or(0);
        let limit = if kind_of(&path) == Some("css") {
            MAX_CSS_FILE_BYTES
        } else {
            MAX_TOKEN_FILE_BYTES
        };
        if size > limit {
            skipped.push(format!("{path}: larger than {} KiB", limit / 1024));
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&full) else {
            continue;
        };
        if kind_of(&path) == Some("css") && !(text.contains("--") && reads_blocks(&text)) {
            continue;
        }
        if files.len() >= MAX_FILES || bytes + size > MAX_FILES_BYTES {
            skipped.push(format!(
                "{path}: past the {MAX_FILES} files or {} MiB a sync sends",
                MAX_FILES_BYTES / (1024 * 1024)
            ));
            continue;
        }
        files.push((path, text));
        bytes += size;
    }
    let notes = NOTES_FILES
        .iter()
        .find(|path| file_size(&root.join(path)).is_some())
        .and_then(|path| std::fs::read_to_string(root.join(path)).ok())
        .map(|text| {
            if text.len() <= MAX_NOTES_BYTES {
                return text;
            }
            let mut cut = MAX_NOTES_BYTES;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text[..cut].to_string()
        });
    components.sort();
    components.truncate(MAX_COMPONENTS);
    Sources {
        files,
        notes,
        components,
        skipped,
    }
}

/// Whether a stylesheet has a block Gateway reads tokens from (`:root`, `@theme`, `html`, a theme).
fn reads_blocks(text: &str) -> bool {
    [":root", "@theme", "data-theme", ".dark", ".light"]
        .iter()
        .any(|needle| text.contains(needle))
        || text
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .any(|word| word == "html")
}

/// A design system's name from a folder: its package.json's name, else the folder's own, as Gateway takes names.
pub fn name_for(folder: &str, package_name: Option<&str>) -> String {
    let fallback = folder
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("design-system");
    let raw = package_name
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
        .to_lowercase();
    let clean = |part: &str, max: usize| -> String {
        let mut out = String::new();
        let mut dash = false;
        for c in part.chars() {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                out.push(c);
                dash = false;
            } else if !dash {
                out.push('-');
                dash = true;
            }
        }
        let out: String = out
            .trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
            .chars()
            .take(max)
            .collect();
        if out.is_empty() {
            "design-system".to_string()
        } else {
            out
        }
    };
    if let Some(scoped) = raw.strip_prefix('@')
        && let Some((scope, name)) = scoped.split_once('/')
        && scope.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && scope
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        && !name.is_empty()
    {
        let scope: String = clean(scope, 64).chars().take(31).collect();
        return format!("@{scope}/{}", clean(name, 64));
    }
    clean(&raw, 64)
}

fn package_name(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("package.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value.get("name")?.as_str().map(str::to_string)
}

/// A design system by id or name: a name with a `/` is looked up in the list, which the path cannot carry.
fn find(cmd: &Cmd, key: &str) -> Result<Value> {
    if !key.contains('/') {
        return cmd.core.get(&format!("/v1/design-systems/{}", enc(key)));
    }
    let all = cmd.core.get("/v1/design-systems")?;
    let wanted = key.to_lowercase();
    let Some(found) = a(&all, "designSystems")
        .iter()
        .find(|system| js_string(system.get("name")) == wanted)
    else {
        return Err(Error::Cli(CliError::hint(
            format!("No design system {key}."),
            "`nexus design-system list` shows them.",
        )));
    };
    cmd.core
        .get(&format!("/v1/design-systems/{}", enc(&js_string(found.get("id")))))
}

fn summary(style: &Style, system: &Value) -> String {
    let components = system.get("componentCount").and_then(Value::as_i64).unwrap_or(0);
    let components = if components > 0 {
        format!(" · {components} components")
    } else {
        String::new()
    };
    format!(
        "{} {}",
        style.bold(&js_string(system.get("name"))),
        style.dim(&format!(
            "{} · {} variables{components}",
            js_string(system.get("id")),
            js_string(system.get("variableCount"))
        ))
    )
}

pub fn design_system_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let action = args.first().map(String::as_str).unwrap_or("list");
    let first = args.get(1).filter(|value| !value.is_empty()).map(String::as_str);

    if action == "help" || (cmd.has("help") && args.is_empty()) {
        if cmd.json {
            return Ok(cmd.print(
                &json!({ "usage": "nexus design-system <command>", "commands": design_system_command_names() }),
            ));
        }
        cmd.lines(design_system_help(style));
        return Ok(0);
    }

    match action {
        "list" => {
            let all = cmd.core.get("/v1/design-systems")?;
            if cmd.json {
                return Ok(cmd.print(&all));
            }
            let rows: Vec<Vec<String>> = a(&all, "designSystems")
                .iter()
                .map(|system| {
                    let source = js_string(system.get("source"));
                    let source = if source.chars().count() > 40 {
                        format!("{}…", source.chars().take(39).collect::<String>())
                    } else {
                        source
                    };
                    vec![
                        js_string(system.get("name")),
                        js_string(system.get("id")),
                        js_string(system.get("variableCount")),
                        if system.get("isDefault").and_then(Value::as_bool) == Some(true) {
                            "yes".into()
                        } else {
                            String::new()
                        },
                        source,
                    ]
                })
                .collect();
            cmd.lines(list(
                style,
                &[
                    col("Design system"),
                    col("Id"),
                    right("Variables"),
                    col("Default"),
                    col("Source"),
                ],
                rows_of(rows),
                None,
            ));
            Ok(0)
        }
        "show" => {
            let key = match first {
                Some(key) => key.to_string(),
                None => js_string(cmd.core.get("/v1/design-systems")?.get("defaultId")),
            };
            let system = find(cmd, &key)?;
            if cmd.json {
                return Ok(cmd.print(&system));
            }
            let name = js_string(system.get("name"));
            cmd.out(summary(style, &system));
            cmd.out("");
            let source = js_string(system.get("source"));
            cmd.lines(pairs(
                style,
                &[
                    (
                        "Default".into(),
                        if system.get("isDefault").and_then(Value::as_bool) == Some(true) {
                            "yes: new designs use it".into()
                        } else {
                            format!("no (nexus design-system use {name})")
                        },
                    ),
                    ("Source".into(), if source.is_empty() { "—".into() } else { source }),
                    (
                        "Link".into(),
                        format!("<meta name=\"nexus-design-system\" content=\"{name}\">"),
                    ),
                ],
            ));
            cmd.out("");
            cmd.out(style.bold("Tokens"));
            cmd.out(js_string(system.get("css")).trim_end_matches('\n'));
            let components: Vec<String> = a(&system, "components")
                .iter()
                .map(|component| js_string(Some(component)))
                .collect();
            if !components.is_empty() {
                cmd.out("");
                cmd.out(style.bold("Components"));
                cmd.out(components.join(", "));
            }
            let notes = js_string(system.get("notes"));
            if !notes.trim().is_empty() {
                cmd.out("");
                cmd.out(style.bold("Notes"));
                cmd.out(notes.trim_end_matches('\n'));
            }
            Ok(0)
        }
        "sync" => {
            let Some(folder) = first else {
                return usage(
                    "Usage: nexus design-system sync <path> [--name N] [--default]",
                    Some("Point it at a repository or a folder of tokens."),
                );
            };
            if cmd.has("name") {
                return usage("--name needs a name.", None);
            }
            let root: PathBuf = cmd.ctx.cwd.join(folder);
            if !root.is_dir() {
                return usage(format!("{folder} is not a folder."), None);
            }
            let sources = collect_sources(&root);
            if sources.files.is_empty() {
                return Err(Error::Cli(CliError::hint(
                    format!("No tokens found in {folder}."),
                    "A sync reads CSS custom properties (:root, @theme), a tailwind.config.* and tokens.json files.",
                )));
            }
            let name = match cmd.flag_set_str("name") {
                Some(name) => name.to_lowercase(),
                None => name_for(folder, package_name(&root).as_deref()),
            };
            let count = sources.files.len();
            let mut body = json!({
                "name": name,
                "files": sources.files.iter().map(|(path, text)| json!({ "path": path, "text": text })).collect::<Vec<_>>(),
                "components": sources.components,
                "source": format!("nexus design-system sync {folder} ({count} {})", if count == 1 { "file" } else { "files" }),
            });
            if let Some(notes) = &sources.notes {
                body["notes"] = json!(notes);
            }
            if cmd.has("default") {
                body["makeDefault"] = json!(true);
            }
            let saved = cmd.core.post("/v1/design-systems", body)?;
            if cmd.json {
                let mut shown = saved.clone();
                shown["skipped"] = json!(sources.skipped);
                return Ok(cmd.print(&shown));
            }
            let system = &saved["designSystem"];
            let verb = if saved.get("created").and_then(Value::as_bool) == Some(true) {
                "Created"
            } else {
                "Updated"
            };
            cmd.out(format!("{} {verb} {}", style.ok, summary(style, system)));
            for line in a(&saved, "read")
                .iter()
                .map(|line| js_string(Some(line)))
                .chain(sources.skipped.iter().cloned())
            {
                cmd.out(format!("  {}", style.dim(&line)));
            }
            if system.get("isDefault").and_then(Value::as_bool) == Some(true) {
                cmd.out(format!("{} New designs use it.", style.arrow));
            } else {
                cmd.out(format!(
                    "{} Make it the default: {}",
                    style.arrow,
                    style.cyan(&format!("nexus design-system use {}", js_string(system.get("name"))))
                ));
            }
            Ok(0)
        }
        "use" => {
            let Some(key) = first else {
                return usage(
                    "Usage: nexus design-system use <name>",
                    Some("`nexus design-system list` shows them."),
                );
            };
            let system = find(cmd, key)?;
            let made = cmd.core.post(
                &format!("/v1/design-systems/{}/default", enc(&js_string(system.get("id")))),
                json!({}),
            )?;
            if cmd.json {
                return Ok(cmd.print(&made));
            }
            cmd.out(format!(
                "{} New designs use {}.",
                style.ok,
                style.bold(&js_string(made.get("name")))
            ));
            Ok(0)
        }
        _ => usage(
            format!("Unknown command `nexus design-system {action}`."),
            Some("Use list, show, sync or use."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_each_file_by_its_name() {
        assert_eq!(kind_of("a/tokens.json"), Some("tokens"));
        assert_eq!(kind_of("brand.tokens.json"), Some("tokens"));
        assert_eq!(kind_of("tailwind.config.cjs"), Some("tailwind"));
        assert_eq!(kind_of("app.scss"), Some("css"));
        assert_eq!(kind_of("package.json"), None);
        assert_eq!(component_of("ui/components/Modal.vue").as_deref(), Some("Modal"));
        assert_eq!(component_of("src/components/Card/index.tsx").as_deref(), Some("Card"));
        assert_eq!(component_of("ui/Button.tsx"), None);
        assert_eq!(component_of("src/components/use-thing.tsx"), None);
    }

    #[test]
    fn names_it_as_the_typescript_cli_does() {
        assert_eq!(name_for("./web", Some("@Acme/Web App")), "@acme/web-app");
        assert_eq!(name_for("/repos/My Site/", None), "my-site");
    }
}
