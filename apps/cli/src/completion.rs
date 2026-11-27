//! Tab-completion for the `nexus` CLI (pod: factory-services-frontend-frontend-interfaces).
//!
//! `nexus completion <bash|zsh|fish>` prints a small shell script to source; that script registers a
//! function which calls the hidden `nexus __complete <words…>`. The completer walks the command
//! catalog (`catalog::command_catalog`, the same source as `nexus help --json`) and prints the
//! candidates for the word being typed, one per line, already filtered by that word's prefix.
//!
//! Intentionally basic (an internal decision catalog only): command names, subcommand verbs at any depth, and flag
//! names. It does *not* track flag arity or complete flag *values* (a team slug, a model, a session
//! id) — those need the Gateway and are out of scope here; a flag's value is simply treated like any
//! other word. When the completer yields nothing, bash and zsh fall back to their own file completion
//! for path arguments. See the scope note in the PR before adding value-aware handling.
//!
//! `nexus completion install` turns it on with no manual step; the installer and `nexus update` run it.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::catalog::command_catalog;

/// The script `nexus completion <shell>` prints, or `None` for an unknown shell.
pub fn script(shell: &str) -> Option<&'static str> {
    match shell {
        "bash" => Some(BASH),
        "zsh" => Some(ZSH),
        "fish" => Some(FISH),
        _ => None,
    }
}

/// The shells `nexus completion` knows, for its usage message.
pub const SHELLS: &str = "bash, zsh, fish";

/// One shell `install` set up: a line for the user, and whether it is new (a line added or a file written
/// for the first time), so `nexus update` speaks only when something changed.
pub struct Setup {
    pub shell: &'static str,
    pub line: String,
    pub new: bool,
}

/// Turn completion on for each shell in use: `$SHELL`'s, and any whose startup file exists. Writes the bash
/// and zsh scripts to `<install dir>/completions/` and adds one line that loads it to `~/.bashrc` / `.zshrc`,
/// unless the file already mentions `nexus completion` (this line, or one added by hand). Fish loads
/// `~/.config/fish/completions/nexus.fish` by itself. No `HOME`: nothing to do.
pub fn install(env: &HashMap<String, String>) -> std::io::Result<Vec<Setup>> {
    let var = |name: &str| env.get(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    let Some(home) = var("HOME") else {
        return Ok(Vec::new());
    };
    let shell = env.get("SHELL").cloned().unwrap_or_default();
    let uses = |name: &str, startup: &Path| shell.ends_with(&format!("/{name}")) || startup.exists();
    let dir = var("NEXUS_INSTALL_DIR")
        .unwrap_or_else(|| home.join(".nexus"))
        .join("completions");
    let mut done = Vec::new();
    let bashrc = home.join(".bashrc");
    if uses("bash", &bashrc) {
        done.push(sourced("bash", &dir.join("nexus.bash"), BASH, &bashrc)?);
    }
    let zshrc = var("ZDOTDIR").unwrap_or_else(|| home.clone()).join(".zshrc");
    if uses("zsh", &zshrc) {
        let text = format!("{ZSH_COMPINIT}{ZSH}");
        done.push(sourced("zsh", &dir.join("nexus.zsh"), &text, &zshrc)?);
    }
    let fish = var("XDG_CONFIG_HOME")
        .unwrap_or_else(|| home.join(".config"))
        .join("fish");
    if uses("fish", &fish) {
        let file = fish.join("completions").join("nexus.fish");
        let new = !file.exists();
        write_script(&file, FISH)?;
        done.push(Setup {
            shell: "fish",
            line: format!("fish: {} (fish loads it by itself)", file.display()),
            new,
        });
    }
    Ok(done)
}

/// Write `file` and make `startup` load it, unless `startup` already mentions `nexus completion`.
fn sourced(shell: &'static str, file: &Path, text: &str, startup: &Path) -> std::io::Result<Setup> {
    let new_file = !file.exists();
    write_script(file, text)?;
    let current = match std::fs::read_to_string(startup) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    if current.contains("nexus completion") {
        return Ok(Setup {
            shell,
            line: format!("{shell}: {} already loads it", startup.display()),
            new: new_file,
        });
    }
    let quoted = file.display().to_string().replace('\'', "'\\''");
    let gap = if current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(startup)?
        .write_all(
            format!("{gap}\n# nexus completion: tab completion for the nexus CLI\n[ -f '{quoted}' ] && . '{quoted}'\n")
                .as_bytes(),
        )?;
    Ok(Setup {
        shell,
        line: format!("{shell}: {} loads {}", startup.display(), file.display()),
        new: true,
    })
}

fn write_script(file: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(file, text)
}

/// Candidates for the last word in `words` — the tokens after `nexus`, the last being the word under
/// the cursor (possibly empty). One per line, sorted, de-duplicated and filtered by that word's prefix.
pub fn complete(words: &[String], version: &str) -> Vec<String> {
    let catalog = command_catalog(version);
    let specs = flatten(
        catalog
            .get("commands")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
    );
    let (cur, prior): (&str, &[String]) = match words.split_last() {
        Some((cur, prior)) => (cur.as_str(), prior),
        None => ("", &[]),
    };

    // Flags aren't part of the command path. We keep this deliberately simple: a flag's value (the
    // `abc` in `--session abc`) isn't tracked, so it's just treated as another word. See the module
    // scope note — value-aware handling is out of scope.
    let path = positional(prior);

    let mut out: Vec<String> = Vec::new();
    if cur.starts_with('-') {
        // A flag: the resolved command's own flags, then the global ones.
        if let Some(spec) = resolve(&specs, &path) {
            out.extend(flag_names(spec));
        }
        out.extend(global_flags(&catalog));
        // Don't re-suggest a flag already on the command line.
        out.retain(|flag| !prior.iter().any(|word| word == flag));
    } else {
        // A command or subcommand verb: the next token of every catalog name whose first `path.len()`
        // tokens are exactly `path` (so `nexus ` lists commands, `nexus chat ` lists chat's verbs).
        let want = path.len();
        for spec in &specs {
            let Some(name) = spec.get("name").and_then(Value::as_str) else {
                continue;
            };
            if name == "<product>" {
                continue;
            }
            let tokens: Vec<&str> = name.split(' ').collect();
            if tokens.len() > want && tokens[..want].iter().zip(&path).all(|(a, b)| *a == b) {
                out.push(tokens[want].to_string());
            }
        }
    }

    out.sort();
    out.dedup();
    out.retain(|candidate| candidate.starts_with(cur));
    out
}

/// `prior` with flag tokens removed — leaving just the command-path words. Flag *values* aren't
/// tracked (see the module scope note), so `--session abc` leaves `abc` in the path; harmless, as no
/// command name matches it and completion simply falls through to nothing.
fn positional(prior: &[String]) -> Vec<String> {
    prior.iter().filter(|token| !token.starts_with('-')).cloned().collect()
}

/// Every command spec, top level and nested subcommands, flattened in catalog order.
fn flatten(commands: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for command in commands {
        out.push(command.clone());
        if let Some(subs) = command.get("subcommands").and_then(Value::as_array) {
            out.extend(flatten(subs));
        }
    }
    out
}

/// The spec for the command the cursor is in: the longest catalog name that `prior` names exactly
/// (`team create`), else the top-level command (`team`). `None` for a product or an unknown command.
fn resolve<'a>(specs: &'a [Value], prior: &[String]) -> Option<&'a Value> {
    (1..=prior.len()).rev().find_map(|depth| {
        let name = prior[..depth].join(" ");
        specs
            .iter()
            .find(|spec| spec.get("name").and_then(Value::as_str) == Some(name.as_str()))
    })
}

fn flag_names(spec: &Value) -> Vec<String> {
    spec.get("flags")
        .and_then(Value::as_array)
        .map(|flags| {
            flags
                .iter()
                .filter_map(|flag| flag.get("name").and_then(Value::as_str))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn global_flags(catalog: &Value) -> Vec<String> {
    catalog
        .get("globalFlags")
        .and_then(Value::as_array)
        .map(|flags| {
            flags
                .iter()
                .filter_map(|flag| flag.get("name").and_then(Value::as_str))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

// The glue each shell sources. It passes the words after `nexus` to `nexus __complete`, keeping the
// current word (even when empty) as the last argument so the completer knows the cursor's position.

const BASH: &str = r#"# nexus bash completion. Add to your shell:  source <(nexus completion bash)
_nexus() {
    local IFS=$'\n'
    COMPREPLY=($(nexus __complete "${COMP_WORDS[@]:1:COMP_CWORD-1}" "${COMP_WORDS[COMP_CWORD]}"))
    return 0
}
complete -o default -F _nexus nexus
"#;

const ZSH: &str = r#"#compdef nexus
# nexus zsh completion. Add to your shell:  source <(nexus completion zsh)
_nexus() {
    local out
    out="$(nexus __complete ${words[2,CURRENT-1]} "${words[CURRENT]}")"
    if [[ -n "$out" ]]; then
        compadd -- ${(f)out}
    else
        _files
    fi
}
compdef _nexus nexus
"#;

// `compdef` exists only once the completion system is loaded; a `.zshrc` without `compinit` loads it here.
const ZSH_COMPINIT: &str = "(( $+functions[compdef] )) || { autoload -Uz compinit && compinit; }\n";

const FISH: &str = r#"# nexus fish completion. Add to your shell:  nexus completion fish | source
function __nexus_complete
    set -l prior (commandline -opc)
    set -l current (commandline -ct)
    set -e prior[1]
    nexus __complete $prior "$current"
end
complete -c nexus -f -a '(__nexus_complete)'
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SEMVER;

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn complete_v(items: &[&str]) -> Vec<String> {
        complete(&words(items), SEMVER)
    }

    #[test]
    fn completes_top_level_commands() {
        // `nexus ` (empty current word) lists every built-in command.
        let all = complete_v(&[""]);
        assert!(all.contains(&"team".to_string()));
        assert!(all.contains(&"chat".to_string()));
        assert!(all.contains(&"completion".to_string()));
        // The product placeholder is never a real command.
        assert!(!all.contains(&"<product>".to_string()));
        // A nested verb (e.g. `team create`, `team rename`) must not leak into the top level. `sessions` is one of its
        // own (`nexus sessions`), besides `chat sessions`.
        assert!(!all.contains(&"create".to_string()));
        assert!(!all.contains(&"rename".to_string()));
        // `nexus te` narrows by prefix.
        assert_eq!(complete_v(&["te"]), vec!["team".to_string()]);
    }

    #[test]
    fn completes_subcommand_verbs_at_any_depth() {
        let chat = complete_v(&["chat", ""]);
        assert!(chat.contains(&"sessions".to_string()));
        assert!(chat.contains(&"share".to_string()));
        assert!(chat.contains(&"export".to_string()));
        // One level deeper: `nexus chat sessions `.
        let sessions = complete_v(&["chat", "sessions", ""]);
        assert!(sessions.contains(&"show".to_string()));
        assert!(sessions.contains(&"rename".to_string()));
        // `nexus team cr` narrows to create.
        assert_eq!(complete_v(&["team", "cr"]), vec!["create".to_string()]);
    }

    #[test]
    fn completes_flags_for_the_resolved_command() {
        // The subcommand's own flags, plus the global ones.
        let flags = complete_v(&["team", "create", "--"]);
        assert!(flags.contains(&"--parent".to_string()));
        assert!(flags.contains(&"--name".to_string()));
        assert!(flags.contains(&"--json".to_string())); // global
        // Narrowed by prefix.
        assert_eq!(complete_v(&["team", "create", "--pa"]), vec!["--parent".to_string()]);
        // A command with no flags of its own still offers the global ones.
        assert!(complete_v(&["spend", "--"]).contains(&"--json".to_string()));
        // A flag already on the command line isn't offered again.
        let after_name = complete_v(&["team", "create", "--name", "foo", "--"]);
        assert!(!after_name.contains(&"--name".to_string()));
        assert!(after_name.contains(&"--parent".to_string()));
    }

    #[test]
    fn flags_before_the_cursor_are_ignored_for_the_command_path() {
        // A global switch before the command doesn't block command/verb completion.
        assert_eq!(complete_v(&["--json", ""]), complete_v(&[""]));
        assert_eq!(complete_v(&["chat", "--json", ""]), complete_v(&["chat", ""]));
        // A verb still narrows with a flag interleaved.
        assert_eq!(
            complete_v(&["chat", "--json", "sessions", "sh"]),
            complete_v(&["chat", "sessions", "sh"])
        );
        // resolve() ignores the interleaved flag: these are `chat sessions`' flags, not `chat`'s.
        let flags = complete_v(&["chat", "--json", "sessions", "--"]);
        assert!(flags.contains(&"--limit".to_string())); // chat sessions' own
        assert!(!flags.contains(&"--mode".to_string())); // chat's — must not leak in
        // Scope note: flag *values* aren't tracked, so `--session abc` leaves `abc` in the path and
        // matches no command — harmless, completion just falls through. That's deliberate.
    }

    #[test]
    fn unknown_command_offers_only_global_flags() {
        assert!(complete_v(&["intelligence", "--"]).contains(&"--json".to_string()));
        assert!(complete_v(&["intelligence", ""]).is_empty());
    }

    #[test]
    fn scripts_exist_for_known_shells_only() {
        for shell in ["bash", "zsh", "fish"] {
            assert!(script(shell).is_some_and(|text| text.contains("nexus __complete")));
        }
        assert!(script("powershell").is_none());
        // fish must drop the leading `nexus` token explicitly, not slice `[2..-1]` — that reversed
        // range breaks `nexus <TAB>` (one-element list) and gave no top-level completions.
        let fish = script("fish").expect("fish has a script");
        assert!(fish.contains("set -e prior[1]"));
        assert!(!fish.contains("[2..-1]"));
    }

    fn temp_home() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-completion-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp home");
        dir
    }

    fn env(home: &Path, shell: &str) -> HashMap<String, String> {
        HashMap::from([
            ("HOME".to_string(), home.display().to_string()),
            ("SHELL".to_string(), shell.to_string()),
        ])
    }

    #[test]
    fn install_loads_the_script_from_the_shells_startup_once() {
        let home = temp_home();
        let first = install(&env(&home, "/bin/bash")).expect("install");
        assert_eq!(first.len(), 1);
        assert!(first[0].new);
        let script = home.join(".nexus/completions/nexus.bash");
        assert_eq!(std::fs::read_to_string(&script).expect("script"), BASH);
        let bashrc = std::fs::read_to_string(home.join(".bashrc")).expect("bashrc");
        assert!(bashrc.contains(&format!("[ -f '{0}' ] && . '{0}'", script.display())));

        // Again (as `nexus update` does): the script is rewritten, the line is not added twice.
        let again = install(&env(&home, "/bin/bash")).expect("install");
        assert!(!again[0].new);
        assert_eq!(std::fs::read_to_string(home.join(".bashrc")).expect("bashrc"), bashrc);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn install_keeps_a_line_added_by_hand_and_covers_every_shell_in_use() {
        let home = temp_home();
        std::fs::write(home.join(".bashrc"), "source <(nexus completion bash)\n").expect("bashrc");
        let done = install(&env(&home, "/usr/bin/zsh")).expect("install");
        let shells: Vec<&str> = done.iter().map(|setup| setup.shell).collect();
        assert_eq!(shells, ["bash", "zsh"]);
        assert_eq!(
            std::fs::read_to_string(home.join(".bashrc")).expect("bashrc"),
            "source <(nexus completion bash)\n"
        );
        let zsh = std::fs::read_to_string(home.join(".nexus/completions/nexus.zsh")).expect("zsh");
        assert!(zsh.starts_with(ZSH_COMPINIT));
        assert!(
            std::fs::read_to_string(home.join(".zshrc"))
                .expect("zshrc")
                .contains("nexus.zsh")
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn install_puts_fish_in_its_own_completions_folder_and_needs_home() {
        let home = temp_home();
        let done = install(&env(&home, "/usr/bin/fish")).expect("install");
        assert_eq!(done.len(), 1);
        let file = home.join(".config/fish/completions/nexus.fish");
        assert_eq!(std::fs::read_to_string(file).expect("fish"), FISH);
        assert!(install(&HashMap::new()).expect("no home").is_empty());
        std::fs::remove_dir_all(&home).ok();
    }
}
