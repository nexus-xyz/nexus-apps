//! The TypeScript CLI's argument grammar (`parseArgv` in `cli.ts`): `--name value`, `--name=value` and a fixed set of
//! switches; `-h` is `--help`, `-v` is `--version`; everything else is positional. A flag that is not a switch takes
//! the next argument as its value unless that one starts with `--`; with none it counts as a switch.

use std::collections::{HashMap, HashSet};

const BOOLEAN_FLAGS: &[&str] = &[
    "new",
    "undo",
    "all",
    "clear",
    "csv",
    "jsonl",
    "json",
    "no-wait",
    "help",
    "no-color",
    "version",
    "no-browser",
    "yearly",
    "with-token",
    "payment-token-stdin",
    "yes",
    "lead",
    "home",
    "root",
    "dry-run",
    "prune",
    "cascade",
    "invite",
    "pending",
    "check",
    "rollback",
    "org",
    "allow-continue",
    "md",
    "mine",
    "shared-with-me",
    "revoked",
    "public",
    "include-code",
    "external",
    "force",
    "private",
    "replace",
    "source",
    "update",
    "all-versions",
    "unread",
    "include-thinking",
    "archived",
    "worktree",
    "folder",
    "read-only",
    "no-web",
    "skip-export",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    pub flags: HashMap<String, String>,
    pub booleans: HashSet<String>,
    pub positionals: Vec<String>,
}

impl Args {
    pub fn flag(&self, name: &str) -> Option<&str> {
        self.flags.get(name).map(String::as_str)
    }

    /// A flag with a non-empty value (`flags.x` is truthy).
    pub fn flag_set(&self, name: &str) -> Option<&str> {
        self.flag(name).filter(|value| !value.is_empty())
    }

    pub fn has(&self, name: &str) -> bool {
        self.booleans.contains(name)
    }
}

pub fn parse_argv(argv: &[String]) -> Args {
    let mut args = Args::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        if arg == "-h" {
            args.booleans.insert("help".into());
        } else if arg == "-v" {
            args.booleans.insert("version".into());
        } else if !arg.starts_with("--") {
            args.positionals.push(arg.clone());
        } else {
            let rest = &arg[2..];
            let (name, inline) = match rest.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (rest, None),
            };
            if BOOLEAN_FLAGS.contains(&name) {
                args.booleans.insert(name.into());
            } else if let Some(value) = inline {
                args.flags.insert(name.into(), value.into());
            } else if let Some(next) = argv.get(i + 1).filter(|next| !next.starts_with("--")) {
                args.flags.insert(name.into(), next.clone());
                i += 1;
            } else {
                args.booleans.insert(name.into());
            }
        }
        i += 1;
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn reads_the_typescript_grammar() {
        let args = parse_argv(&argv(&[
            "team",
            "create",
            "web",
            "--name",
            "Web",
            "--level=dept",
            "--json",
            "-h",
            "--parent",
            "--root",
            "--x=",
            "--flag",
        ]));
        assert_eq!(args.positionals, argv(&["team", "create", "web"]));
        assert_eq!(args.flag("name"), Some("Web"));
        assert_eq!(args.flag("level"), Some("dept"));
        assert_eq!(args.flag("x"), Some(""));
        assert!(args.has("json") && args.has("help") && args.has("root") && args.has("parent") && args.has("flag"));
        let negative = parse_argv(&argv(&["budgets", "set", "a@b", "-5", "--days", "-3"]));
        assert_eq!(negative.positionals, argv(&["budgets", "set", "a@b", "-5"]));
        assert_eq!(negative.flag("days"), Some("-3"));
    }
}
