use std::path::PathBuf;

use nexus_agent_memory::{Repository, Scope, Source, Store, markdown};
use nexus_client::agent_entries::Agent;
use nexus_client::local_agents::native_memory;
use nexus_workbench::{accounts, event::SessionAgent};
use serde_json::json;
use uuid::Uuid;

use crate::Context;
use crate::args::Args;
use crate::error::{CliError, Result};
use crate::json::pretty;

pub const COMMANDS: &[(&str, &str)] = &[
    ("status [--json]", "Show whether shared memory is on"),
    ("enable [--json]", "Turn on shared memory on this computer"),
    (
        "disable [--export <path>] [--skip-export] [--repo <path>]",
        "Turn off shared memory with a Markdown export or an explicit choice to skip it",
    ),
    ("list [--repo <path>] [--json]", "List personal and repository entries"),
    ("add <text>", "Save a personal note"),
    ("edit <id> <text> [--repo <path>]", "Replace the text of an entry"),
    ("forget <id> [--repo <path>]", "Delete an entry"),
    (
        "import <claude|codex> [--repo <path>]",
        "Import memory from the selected agent's local profiles",
    ),
    (
        "export <claude|codex|markdown> [--output <path>] [--repo <path>]",
        "Export to an agent profile or Markdown. Codex loads every repository block in every session on that profile",
    ),
];

pub fn command(words: &[String], ctx: &Context, args: &Args, as_json: bool) -> Result<i32> {
    if words.first().is_some_and(|word| word == "help") {
        ctx.io.out("Usage: nexus memory <command>");
        for (usage, detail) in COMMANDS {
            ctx.io.out(&format!("  {usage}\n    {detail}"));
        }
        return Ok(0);
    }
    let store = memory(Store::from_env(&ctx.env_fn()))?;
    run(&store, words, ctx, args, as_json)
}

fn run(store: &Store, words: &[String], ctx: &Context, args: &Args, as_json: bool) -> Result<i32> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let path = |value: &str| ctx.cwd.join(value);
    let cwd = match args.flag_set("repo") {
        Some(value) => Repository::checkout(&path(value))
            .ok_or_else(|| CliError::usage("Choose a folder in a Git repository for --repo.", None))?,
        None => ctx.cwd.clone(),
    };
    let repository = || Repository::discover(&cwd);
    let id = |value: &str| Uuid::parse_str(value).map_err(|_| CliError::usage("Enter a shared memory entry ID.", None));
    let out = |data: serde_json::Value, text: &str| {
        if as_json {
            ctx.io.out(&pretty(&data));
        } else {
            ctx.io.out(text);
        }
    };
    let status = |enabled| {
        out(
            json!({ "enabled": enabled }),
            if enabled {
                "Shared memory is on."
            } else {
                "Shared memory is off."
            },
        );
    };
    match words.as_slice() {
        [] | ["status"] => status(memory(store.enabled())?),
        ["enable"] => {
            memory(store.set_enabled(true))?;
            status(true);
        }
        ["disable"] => {
            let skip = args.has("skip-export");
            let destination = args.flag_set("export");
            if skip && destination.is_some() {
                return Err(CliError::usage("Choose either --export or --skip-export.", None).into());
            }
            if memory(store.enabled())? {
                match destination {
                    Some(destination) => {
                        let count = memory(store.export_markdown(repository().as_ref(), &path(destination)))?;
                        if !as_json {
                            ctx.io.out(&format!("Exported {count} entries to {destination}."));
                        }
                    }
                    None if !skip => {
                        return Err(CliError::usage(
                            "Shared memory is on. You can export your entries before turning it off.",
                            Some("Run `nexus memory disable --export <path>` or `nexus memory disable --skip-export`."),
                        )
                        .into());
                    }
                    None => {}
                }
                memory(store.set_enabled(false))?;
            }
            status(false);
        }
        ["list"] => {
            let snapshot = memory(store.entries_with_issues(repository().as_ref()))?;
            if snapshot.skipped > 0 {
                ctx.io.err(&format!(
                    "Skipped {} unreadable shared memory file(s).",
                    snapshot.skipped
                ));
            }
            let entries = snapshot.entries;
            out(
                json!(entries),
                &if entries.is_empty() {
                    "No shared memory entries.".into()
                } else {
                    markdown(&entries)
                },
            );
        }
        ["add", text] => {
            let entry = memory(store.remember(Scope::Personal, Source::Person, text))?;
            out(json!({"id": entry.id}), "Saved the personal note.");
        }
        ["edit", entry, text] => {
            require_entry(memory(store.edit(repository().as_ref(), id(entry)?, text))?)?;
            out(json!({"id": entry, "edited": true}), "Saved the entry.");
        }
        ["forget", entry] => {
            require_entry(memory(store.forget(repository().as_ref(), id(entry)?))?)?;
            out(json!({"id": entry, "deleted": true}), "Forgot the entry.");
        }
        ["import", from] => {
            let (agent, profiles) = profiles(from, ctx)?;
            let count = memory(native_memory::import(store, agent, &profiles, &cwd))?;
            out(json!({"imported": count}), &format!("Imported {count} entries."));
        }
        ["export", "markdown"] => {
            let repository = repository();
            if let Some(destination) = args.flag_set("output") {
                let count = memory(store.export_markdown(repository.as_ref(), &path(destination)))?;
                out(
                    json!({"exported": count, "path": destination}),
                    &format!("Exported {count} entries to {destination}."),
                );
            } else {
                let entries = memory(store.entries(repository.as_ref()))?;
                let document = format!("# Nexus shared memory\n\n{}", markdown(&entries));
                out(json!({"markdown": document, "exported": entries.len()}), &document);
            }
        }
        ["export", to] => {
            if args.flag_set("output").is_some() {
                return Err(CliError::usage("Use --output with Markdown export.", None).into());
            }
            let (agent, profiles) = profiles(to, ctx)?;
            let count = memory(native_memory::export(store, agent, &profiles[0], &cwd))?;
            out(
                json!({"exported": count}),
                &format!("Exported {count} entries to {}.", agent.entry().identity.name),
            );
        }
        _ => {
            return Err(CliError::usage(
                "Unknown shared memory command or arguments.",
                Some("Run `nexus memory --help`."),
            )
            .into());
        }
    }
    Ok(0)
}

fn profiles(name: &str, ctx: &Context) -> Result<(Agent, Vec<PathBuf>)> {
    let (agent, session) = match name {
        "claude" => (Agent::Claude, SessionAgent::Claude),
        "codex" => (Agent::Codex, SessionAgent::Codex),
        _ => return Err(CliError::usage("Choose claude or codex for native memory.", None).into()),
    };
    let env = ctx.env_fn();
    let home = nexus_client::config::home_dir(&env);
    let root = accounts::desktop_data_dir(Some(&home), &|name| ctx.var(name).map(PathBuf::from))
        .map(|data| data.join("accounts"));
    let profiles = accounts::memory_profiles(root.as_deref(), session, &home, &env);
    Ok((agent, profiles))
}

fn memory<T>(result: std::io::Result<T>) -> Result<T> {
    result.map_err(|error| CliError::new(error.to_string()).into())
}

fn require_entry(found: bool) -> Result<()> {
    if found {
        Ok(())
    } else {
        Err(CliError::new("That shared memory entry was not found.").into())
    }
}
