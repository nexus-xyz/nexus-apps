//! The agent's knowledge base about Nexus: what Nexus does today and how, which the agent reads with its
//! `nexus_docs` tool when a question or a task needs it. The system prompt (`prompt.rs`) holds only a map of it,
//! because every step sends the prompt again.
//!
//! - `knowledge/<topic>.md`: the hand-written topics, built into the binary. Each says what one part of Nexus is, the
//!   commands that reach it, its limits and who may use it. A test checks every `nexus` command and flag they show
//!   against the CLI's own catalog (`catalog.rs`), so a renamed command fails here, not in front of a person.
//! - `commands` and `nexus <command>`: made from that catalog when asked, so they are never stale.
//! - `status`: read from Gateway when asked: who the agent acts as, and what this Gateway has on.
//!
//! The tool reads only: it never asks the person, and a subagent has it too.

use serde_json::{Value, json};

use super::prompt::Host;
use super::tools::Action;
use crate::agent_routes::prose_list;
use crate::catalog::{command_catalog, find_command};
use crate::client::Client;
use nexus_client::agent_entries::Agent;

/// The tool that reads the knowledge base.
pub const TOOL: &str = "nexus_docs";

/// The topics made when asked, with what the index says of each.
const STATUS: &str = "status";
const COMMANDS: &str = "commands";
const MADE: [(&str, &str); 3] = [
    (
        STATUS,
        "who you act as now: the organization, the person, their role, and what this Gateway has on (live)",
    ),
    (COMMANDS, "every `nexus` command, one line each"),
    (
        "nexus <command>",
        "one command's usage, flags and examples, such as `nexus pages share`",
    ),
];

/// One hand-written topic: its name, what it covers (for the index), and its text.
struct Topic {
    name: &'static str,
    covers: String,
    text: String,
}

/// The places in a topic's text that the catalog fills, and what fills each.
fn places() -> [(&'static str, String); 7] {
    use crate::agent_routes::launchers;
    [
        // Every agent's name: `Claude Code, Codex, Antigravity and Cursor`.
        ("{{agents}}", agent_names(|_| true)),
        // The agents with a Gateway route: `Claude Code and Codex`.
        (
            "{{gateway_agents}}",
            agent_names(|agent| agent.entry().billing.gateway.is_some()),
        ),
        // The agents with no Gateway route.
        (
            "{{plan_agents}}",
            agent_names(|agent| agent.entry().billing.gateway.is_none()),
        ),
        // Each launcher: `` `nexus claude`, `nexus codex`, `nexus agy` and `nexus cursor-agent` ``.
        (
            "{{launchers}}",
            prose_list(
                &launchers()
                    .map(|agent| format!("`nexus {}`", agent.command()))
                    .collect::<Vec<_>>(),
                "and",
            ),
        ),
        // The launchers that write the person's skills into the agent's skills folder: `` `nexus claude`, `nexus codex` ``.
        (
            "{{skills_launchers}}",
            launchers()
                .filter(|agent| agent.entry().profile.skills.is_some())
                .map(|agent| format!("`nexus {}`", agent.command()))
                .collect::<Vec<_>>()
                .join(", "),
        ),
        // The agents a policy applies to, those with a Gateway route: `` `claude`, `codex` ``.
        (
            "{{policy_agents}}",
            crate::models::policy_agents()
                .map(|agent| format!("`{agent}`"))
                .collect::<Vec<_>>()
                .join(", "),
        ),
        // Where errors.md suggests `nexus triage`: only in a build with the staff-only commands.
        ("{{triage}}", triage_paragraph()),
    ]
}

/// The paragraph that suggests `nexus triage` to Nexus's own people; empty in the open-source build (`staff` off).
fn triage_paragraph() -> String {
    if cfg!(feature = "staff") {
        "\nWhen the person works at Nexus and the defect is in Nexus itself, suggest `nexus triage`.\n\
         It starts an agent that reads this computer's Nexus logs, diagnoses the defect and drafts a report.\n\
         The person approves the report, then `nexus triage send <file> --yes` posts it to the Nexus team."
            .to_string()
    } else {
        String::new()
    }
}

/// A topic's text with its places filled.
fn filled(text: &str, places: &[(&'static str, String)]) -> String {
    places
        .iter()
        .fold(text.to_string(), |text, (place, value)| text.replace(place, value))
}

macro_rules! topic {
    ($places:expr, $name:literal, $covers:expr) => {
        Topic {
            name: $name,
            covers: String::from($covers),
            text: filled(include_str!(concat!("knowledge/", $name, ".md")), $places),
        }
    };
}

/// The names of the agents `which` keeps, as a person lists them: `Claude Code, Codex, Antigravity and Cursor`.
fn agent_names(which: impl Fn(&Agent) -> bool) -> String {
    let names: Vec<String> = Agent::ALL
        .into_iter()
        .filter(|agent| which(agent))
        .map(|agent| agent.entry().identity.name.to_string())
        .collect();
    prose_list(&names, "and")
}

/// What the coding-agents topic covers: the agents with a Gateway route, from the catalog.
fn coding_agents_covers() -> String {
    format!(
        "{} through Nexus: their two routes, a run, `nexus agents` and `nexus mcp`",
        agent_names(|agent| agent.entry().billing.gateway.is_some())
    )
}

/// What the plan-agents topic covers: the agents with no Gateway route.
fn plan_agents_covers() -> String {
    format!(
        "{} without Gateway",
        agent_names(|agent| agent.entry().billing.gateway.is_none())
    )
}

/// Every topic, in the order the index lists them.
fn topics() -> Vec<Topic> {
    let places = places();
    vec![
        topic!(
            &places,
            "overview",
            "what Nexus is, where people use it, and the map of these topics"
        ),
        topic!(
            &places,
            "session",
            "the `nexus` terminal session: its modes, keys and slash commands"
        ),
        topic!(&places, "agent", "you: your tools, approvals, subagents and limits"),
        topic!(
            &places,
            "models",
            "the model catalog, `nexus/auto`, model policies and refusals"
        ),
        topic!(&places, "spend", "budgets, spend limits, usage, and who pays"),
        topic!(&places, "pages", "pages: write, publish, change, share and view one"),
        topic!(&places, "design", "design canvases, design systems and slide decks"),
        topic!(&places, "chat", "chat sessions, and sharing a chat as an artifact"),
        topic!(&places, "coding-agents", coding_agents_covers()),
        topic!(
            &places,
            "shared-memory",
            "local memory for coding agents, including import and export"
        ),
        topic!(&places, "plan-agents", plan_agents_covers()),
        topic!(
            &places,
            "organization",
            "organizations, roles, members, teams and the audit log"
        ),
        topic!(
            &places,
            "auth",
            "sign-in, API keys, provider keys, and commands that hold secrets"
        ),
        topic!(
            &places,
            "connectors",
            "connectors to outside systems such as GitHub, Jira and Linear"
        ),
        topic!(
            &places,
            "skills",
            "skills: a person's SKILL.md files and the organization's library"
        ),
        topic!(
            &places,
            "assistant",
            "the person's own assistant (`@nexus`): its commands, approvals, voice mode, the phone and passwords"
        ),
        topic!(
            &places,
            "slack-and-inbox",
            "Slack and `@nexus` in it, routines (the morning brief) and the notifications inbox"
        ),
        topic!(
            &places,
            "errands",
            "the assistant's errands: their threads, the Errands view and errands in projects"
        ),
        topic!(
            &places,
            "desktop",
            "Nexus for desktop: Code sessions, the side pane and the app's tools"
        ),
        topic!(
            &places,
            "desktop-sessions",
            "Code sessions from a script or another agent: start, stop, archive and hand off"
        ),
        topic!(
            &places,
            "api",
            "the Nexus API, for code that calls Nexus or models through it"
        ),
        topic!(&places, "errors", "error codes and what fixes each"),
    ]
}

/// Other words a model may ask a topic by. A word that is also a command gets the topic, with a line that says how to
/// get the command's usage.
const ALIASES: &[(&str, &str)] = &[
    ("nexus", "overview"),
    ("help", "overview"),
    ("tui", "session"),
    ("terminal", "session"),
    ("shortcuts", "session"),
    ("tools", "agent"),
    ("approvals", "agent"),
    ("subagents", "agent"),
    ("model", "models"),
    ("policy", "models"),
    ("policies", "models"),
    ("budget", "spend"),
    ("budgets", "spend"),
    ("billing", "spend"),
    ("usage", "spend"),
    ("analytics", "spend"),
    ("page", "pages"),
    ("publish", "pages"),
    ("slides", "design"),
    ("deck", "design"),
    ("design-system", "design"),
    ("artifacts", "chat"),
    ("sharing", "chat"),
    ("share", "chat"),
    ("claude", "coding-agents"),
    ("codex", "coding-agents"),
    ("antigravity", "plan-agents"),
    ("agy", "plan-agents"),
    ("cursor", "plan-agents"),
    ("cursor-agent", "plan-agents"),
    ("opencode", "plan-agents"),
    ("agents", "coding-agents"),
    ("mcp", "coding-agents"),
    ("memory", "shared-memory"),
    ("org", "organization"),
    ("orgs", "organization"),
    ("team", "organization"),
    ("teams", "organization"),
    ("roles", "organization"),
    ("members", "organization"),
    ("audit", "organization"),
    ("service-accounts", "organization"),
    ("login", "auth"),
    ("keys", "auth"),
    ("tokens", "auth"),
    ("secrets", "auth"),
    ("connector", "connectors"),
    ("connections", "connectors"),
    ("connect", "connectors"),
    ("skill", "skills"),
    ("slack", "slack-and-inbox"),
    ("routines", "slack-and-inbox"),
    ("notifications", "slack-and-inbox"),
    ("inbox", "slack-and-inbox"),
    ("errand", "errands"),
    ("app", "desktop"),
    ("workbench", "desktop"),
    ("sessions", "desktop-sessions"),
    ("handoff", "desktop-sessions"),
    ("hand-off", "desktop-sessions"),
    ("browser", "desktop"),
    ("recordings", "desktop"),
    ("error", "errors"),
    ("me", STATUS),
    ("whoami", STATUS),
];

/// The topics' names, for the tool's description and the system prompt's tests.
pub fn topic_names() -> Vec<&'static str> {
    topics().iter().map(|topic| topic.name).collect()
}

/// The tool, as `/v1/chat/completions` takes it.
pub fn spec() -> Value {
    let description = format!(
        "Read Nexus's built-in docs: what Nexus does today, and how. Call it before you answer a question about Nexus \
         or use one of its features. It reads only and never asks the person. Topics: {}; `{STATUS}` (who you act as, \
         live); `{COMMANDS}` (every `nexus` command); and `nexus <command>`, such as `nexus pages share`, for one \
         command's usage, flags and examples. With no topic, it lists the topics.",
        topic_names().join(", ")
    );
    json!({
        "type": "function",
        "function": {
            "name": TOOL,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "A topic's name, `status`, `commands`, or `nexus <command>`.",
                    },
                },
                "required": [],
            },
        }
    })
}

/// The index: every topic and what it covers.
fn index() -> String {
    let mut lines = vec![
        format!("Nexus docs. Call {TOOL} with one of these topics."),
        String::new(),
    ];
    lines.extend(
        topics()
            .iter()
            .map(|topic| format!("- {}: {}", topic.name, topic.covers)),
    );
    lines.extend(MADE.iter().map(|(name, covers)| format!("- {name}: {covers}")));
    lines.join("\n")
}

/// A topic as a model may write it: lower case, with no quotes or marks around it, and its words one space apart.
fn normal(topic: &str) -> String {
    topic
        .trim()
        .trim_matches(|c: char| matches!(c, '`' | '"' | '\'' | '.' | '?' | ':'))
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the tool answers for `topic`: the topic's name as the person sees it, and its text. `status` reads Gateway.
pub fn read(topic: &str, status: &dyn Fn() -> String) -> (String, String) {
    let asked = normal(topic);
    if asked.is_empty() || asked == "index" || asked == "topics" {
        return ("index".into(), index());
    }
    let catalog = command_catalog(crate::VERSION);
    // `nexus pages share`: a command, by its own words.
    if let Some(words) = asked.strip_prefix("nexus ") {
        return command_help(&catalog, words);
    }
    if asked == COMMANDS || asked == "command" {
        return (COMMANDS.into(), commands(&catalog));
    }
    let name = asked.replace([' ', '_'], "-");
    let aliased = ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map(|(_, topic)| *topic);
    if name == STATUS || aliased == Some(STATUS) {
        return (STATUS.into(), status());
    }
    let topics = topics();
    if let Some(topic) = topics.iter().find(|topic| topic.name == name) {
        return (topic.name.into(), topic.text.clone());
    }
    if let Some(topic) = aliased.and_then(|name| topics.iter().find(|topic| topic.name == name)) {
        // The word may be a command too: say how to get its usage.
        let usage = match find_command(commands_of(&catalog), std::slice::from_ref(&name)) {
            Some(_) => format!("\n\nFor the command's own usage and flags, ask for `nexus {name}`."),
            None => String::new(),
        };
        return (topic.name.into(), format!("{}{usage}", topic.text));
    }
    // A command asked for without `nexus` before it, such as `pages share`.
    let words: Vec<String> = asked.split(' ').map(String::from).collect();
    if find_command(commands_of(&catalog), &words).is_some() {
        return command_help(&catalog, &asked);
    }
    (
        asked.clone(),
        format!("There is no topic named `{asked}`.\n\n{}", index()),
    )
}

/// Run the tool: what to tell the model, and what to show the person.
pub fn run(arguments: &str, status: &dyn Fn() -> String) -> (String, Action) {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let asked = args.get("topic").and_then(Value::as_str).unwrap_or("");
    let (topic, text) = read(asked, status);
    (text.clone(), Action::Docs { topic, text })
}

fn commands_of(catalog: &Value) -> &[Value] {
    catalog["commands"].as_array().map(Vec::as_slice).unwrap_or_default()
}

fn text_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `commands`: every command of this CLI with what it does, then the flags and exit codes they share.
fn commands(catalog: &Value) -> String {
    let mut lines = vec![
        format!(
            "Every `nexus` command of this CLI ({}). Ask for `nexus <command>` to get one command's usage, flags \
             and examples.",
            text_of(catalog, "version")
        ),
        String::new(),
    ];
    for command in commands_of(catalog) {
        lines.push(format!(
            "- nexus {}: {}",
            text_of(command, "name"),
            text_of(command, "description")
        ));
    }
    lines.push(String::new());
    lines.push("Flags that every command takes:".into());
    for flag in catalog["globalFlags"].as_array().into_iter().flatten() {
        lines.push(format!("- {}: {}", text_of(flag, "name"), text_of(flag, "description")));
    }
    lines.push(String::new());
    lines.push("Exit codes:".into());
    for code in catalog["exitCodes"].as_array().into_iter().flatten() {
        lines.push(format!(
            "- {}: {}",
            code.get("code").and_then(Value::as_i64).unwrap_or(0),
            text_of(code, "meaning")
        ));
    }
    lines.join("\n")
}

/// `nexus <command>`: the deepest command `words` name, as its own help says it: usage, flags, the other forms, its
/// subcommands and examples. Answers the command's name too. A word that names no built-in command gets the product
/// commands' entry, since a Gateway's modules add their own.
fn command_help(catalog: &Value, words: &str) -> (String, String) {
    let all = commands_of(catalog);
    // Up to the first flag or placeholder: `pages share <id> --org` names `pages share`.
    let path: Vec<String> = words
        .split(' ')
        .take_while(|word| word.chars().next().is_some_and(|first| first.is_ascii_alphanumeric()))
        .map(String::from)
        .collect();
    let Some(command) = find_command(all, &path) else {
        let first = path.first().map(String::as_str).unwrap_or(words);
        let product = all
            .iter()
            .find(|command| text_of(command, "name") == "<product>")
            .map(|command| format!("\n\n{}", render(command)))
            .unwrap_or_default();
        return (
            format!("nexus {first}"),
            format!(
                "`{first}` is not a built-in `nexus` command. Ask for `{COMMANDS}` to list them. A product's commands \
                 come from the modules its Gateway registers: `nexus modules` lists them, and `nexus {first} --help` \
                 shows that product's own.{product}"
            ),
        );
    };
    (format!("nexus {}", text_of(command, "name")), render(command))
}

/// One command of the catalog as text.
fn render(command: &Value) -> String {
    let mut lines = vec![
        text_of(command, "usage").to_string(),
        text_of(command, "description").to_string(),
    ];
    let list = |key: &str| command[key].as_array().cloned().unwrap_or_default();
    let variants = list("variants");
    if !variants.is_empty() {
        lines.push(String::new());
        lines.push("The same command also takes these forms:".into());
        for variant in &variants {
            let about = text_of(variant, "description");
            let about = if about.is_empty() {
                String::new()
            } else {
                format!(": {about}")
            };
            lines.push(format!("- {}{about}", text_of(variant, "usage")));
        }
    }
    let flags = list("flags");
    if !flags.is_empty() {
        lines.push(String::new());
        lines.push("Flags:".into());
        for flag in &flags {
            let value = text_of(flag, "value");
            let value = if value.is_empty() {
                String::new()
            } else {
                format!(" {value}")
            };
            lines.push(format!(
                "- {}{value}: {}",
                text_of(flag, "name"),
                text_of(flag, "description")
            ));
        }
    }
    let subcommands = list("subcommands");
    if !subcommands.is_empty() {
        lines.push(String::new());
        lines.push("Subcommands:".into());
        for subcommand in &subcommands {
            lines.push(format!(
                "- {}: {}",
                text_of(subcommand, "usage"),
                text_of(subcommand, "description")
            ));
        }
    }
    let examples = list("examples");
    if !examples.is_empty() {
        lines.push(String::new());
        lines.push("Examples:".into());
        lines.extend(
            examples
                .iter()
                .filter_map(Value::as_str)
                .map(|example| format!("- {example}")),
        );
    }
    lines.join("\n")
}

/// `status`: who the agent acts as, from `GET /v1/tenants/me`, and what this Gateway has on. What Gateway did not say
/// is left out; when it does not answer, the text says so and keeps what this computer knows.
pub fn status(client: &Client, url: &str, host: Host) -> String {
    let mut lines = Vec::new();
    match client.get("/v1/tenants/me") {
        Ok(me) => {
            lines.push("Who you act as, read from Gateway just now:".to_string());
            let part = |pointer: &str| {
                me.pointer(pointer)
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(String::from)
            };
            let organization = match (part("/tenant/slug"), part("/tenant/name")) {
                (Some(slug), Some(name)) if slug != name => Some(format!("{slug} ({name})")),
                (slug, name) => slug.or(name),
            };
            if let Some(organization) = organization {
                let kind = part("/tenant/kind")
                    .map(|kind| format!(", a {kind} organization"))
                    .unwrap_or_default();
                lines.push(format!("- Organization: {organization}{kind}"));
            }
            if let Some(person) = part("/user/email").or_else(|| part("/user/id")) {
                lines.push(format!("- Person: {person}"));
            }
            if let Some(role) = part("/role") {
                lines.push(format!("- Role: {role}"));
            }
            if let Some(team) = part("/team/slug") {
                lines.push(format!("- Default team: {team}"));
            }
        }
        Err(error) => {
            lines.push(format!(
                "Gateway did not say who you act as (GET /v1/tenants/me): {}",
                crate::error::to_cli_error(&error, url).message
            ));
            lines.push("Run `nexus auth status` to see the CLI's sign-in.".into());
        }
    }
    lines.push(format!("- Gateway: {url}"));
    lines.push(format!("- CLI: nexus {}", crate::VERSION));
    lines.push(format!(
        "- You run in: {}",
        match host {
            Host::Terminal => "the `nexus` terminal session, in agent mode",
            Host::Desktop => "a Code session of Nexus for desktop",
            Host::Program => "another program, with no screen of your own",
        }
    ));
    lines.push(
        "More: `nexus models` lists the models this person may use, `nexus spend` what is left of the budget, and \
         `nexus modules` the products this Gateway registers."
            .into(),
    );
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offline() -> String {
        "offline".into()
    }

    #[test]
    fn every_topic_is_a_short_document_that_starts_with_its_title() {
        for topic in topics() {
            assert!(!topic.text.contains("{{"), "{} leaves a place unfilled", topic.name);
            assert!(topic.text.starts_with("# "), "{} starts with its title", topic.name);
            assert!(topic.text.ends_with('\n'), "{} ends with a new line", topic.name);
            // A topic stays in the conversation once read, so it is paid for at every later step.
            let size = topic.text.chars().count();
            assert!(size <= 6_000, "{} holds {size} characters; split it", topic.name);
            for marker in ["TODO", "TBD", "FIXME"] {
                assert!(!topic.text.contains(marker), "{} holds {marker}", topic.name);
            }
            assert!(!topic.covers.is_empty());
        }
        let mut names = topic_names();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), topics().len(), "each topic has its own name");
    }

    /// Every customer's agent reads these topics, so none names a staff-only route.
    #[test]
    fn no_topic_names_a_staff_route() {
        for topic in topics() {
            assert!(!topic.text.contains("/v1/staff/"), "{} names a staff route", topic.name);
        }
    }

    /// Only a build with the staff-only commands suggests `nexus triage`.
    #[test]
    fn only_the_staff_build_suggests_triage() {
        let errors = topics()
            .into_iter()
            .find(|topic| topic.name == "errors")
            .map_or_else(String::new, |topic| topic.text);
        assert_eq!(errors.contains("nexus triage"), cfg!(feature = "staff"), "{errors}");
    }

    fn topic_text(name: &str) -> String {
        topics()
            .into_iter()
            .find(|topic| topic.name == name)
            .map_or_else(String::new, |topic| topic.text)
    }

    /// The Nexus agent, in a desktop session, is told how to reach another session by its name. The desktop topic
    /// points to the topic that says it, and `sessions` leads there.
    #[test]
    fn the_desktop_sessions_topic_says_how_to_reach_another_session() {
        let sessions = topic_text("desktop-sessions");
        for command in ["nexus sessions list", "nexus sessions send"] {
            assert!(sessions.contains(command), "desktop-sessions.md lacks `{command}`");
        }
        assert!(topic_text("desktop").contains("See `desktop-sessions` to use Code sessions from outside the app."));
        assert_eq!(read("sessions", &offline).0, "desktop-sessions");
        assert_eq!(read("hand off", &offline).0, "desktop-sessions");
    }

    /// The phrases that the workbench's `CREW_INSTRUCTIONS` and `SESSIONS_GUIDANCE` carry too (nexus-workbench's
    /// `both_texts_say_to_pass_the_project_and_commit_before_a_hand_over`): a bare `nexus sessions start` fails from a
    /// worktree, and handing over with it leaves uncommitted work behind.
    #[test]
    fn the_desktop_sessions_topic_says_to_pass_the_project_and_commit_before_a_hand_over() {
        let sessions = topic_text("desktop-sessions");
        for phrase in [
            "--project <owner/repo or the project's folder>",
            "From a worktree, always pass --project",
            "first commit your work on its branch",
            "name the branch in",
        ] {
            assert!(sessions.contains(phrase), "desktop-sessions.md lacks: {phrase}");
        }
        assert!(
            !sessions.contains("push your"),
            "the topic must not tell agents to push"
        );
    }

    /// The person's words pick the tool, a stop is a pause, and the agent archives or hands off its own session only
    /// when the person asks.
    #[test]
    fn the_desktop_sessions_topic_says_which_tool_and_when_to_archive() {
        let sessions = topic_text("desktop-sessions");
        for phrase in [
            "Ask whether the person wants a result back through you, or owns the new session; the noun they use does \
             not decide.",
            "| Work you coordinate or need a result from, in any wording, even \"new session\" | Use `start_session`.",
            "| The person says it is independent or theirs, or hands it over | Run `nexus sessions start`.",
            "| Another project | Run `nexus sessions start` with `--project`. For its result, ask for a report to \
             `$NEXUS_SESSION_ID`, or wait |",
            "\"independent\", \"I own it\", \"I'll take it from here\", \"don't wait for it\", \"no need to report back\"",
            "When the words are unclear, use `start_session`, and say which tool you chose and why.",
            "`start_session` can't reach another project yet.",
            "`nexus sessions start \"<prompt>\" --project <owner/repo or the project's folder>`",
            "`--project` takes a project's id, a folder inside it, its GitHub owner/repo or its name.",
            "To hand work over without replacing this session, first commit your work on its branch.",
            "say which tool you chose and why",
            "Use `handoff_session` only when the person asks to replace or archive this session.",
            "`$NEXUS_SESSION_ID`",
            "`nexus sessions wait <id>`",
            "Stop is a pause. Stop a subsession only when its work may resume.",
            "Archive a subsession you started when you replaced it, or the person cancelled it.",
            "the person won't need it in the sidebar.",
            "Its branch stays. Its worktree stays until the person's worktree cleanup removes it, which never drops \
             uncommitted or unpushed work.",
            "Never archive your own session (`archive_session` with no `session_id`) unless the person says so.",
            "A saved memory that says to always do so counts. Do not guess.",
            "Save it with shared memory (`remember`)",
            "If shared memory is unavailable, say so.",
        ] {
            assert!(sessions.contains(phrase), "desktop-sessions.md lacks: {phrase}");
        }
        // "New session" is coordinated work by default: no row sends it to `nexus sessions start`.
        assert!(
            !sessions.lines().any(|line| line.starts_with("| \"new session\"")),
            "{sessions}"
        );
    }

    #[test]
    fn the_index_and_the_tool_name_every_topic() {
        let (shown, text) = read("", &offline);
        assert_eq!(shown, "index");
        let description = spec()["function"]["description"].as_str().unwrap_or("").to_string();
        for name in topic_names() {
            assert!(text.contains(&format!("- {name}: ")), "the index lists {name}");
            assert!(description.contains(name), "the tool names {name}");
        }
        for made in [STATUS, COMMANDS, "nexus <command>"] {
            assert!(text.contains(made) && description.contains(made), "{made}");
        }
        assert_eq!(spec()["function"]["name"], TOOL);
    }

    /// Every agent's id and command lead to the topic that covers it, and that topic names its command. The index and
    /// the topics' lists of agents come from the catalog.
    #[test]
    fn the_agent_lists_come_from_the_catalog() {
        for agent in Agent::ALL {
            let topic = if agent.entry().billing.gateway.is_some() {
                "coding-agents"
            } else {
                "plan-agents"
            };
            for word in [agent.as_str(), agent.command()] {
                let (shown, text) = read(word, &offline);
                assert_eq!(shown, topic, "{word}");
                if agent.entry().session.cli_launcher {
                    assert!(text.contains(&format!("`nexus {}", agent.command())), "{word}");
                } else {
                    assert!(text.contains(agent.entry().identity.name), "{word}");
                }
            }
        }
        let index = index();
        for line in [
            "- coding-agents: Claude Code and Codex through Nexus: their two routes, a run, `nexus agents` and `nexus mcp`",
            "- plan-agents: Antigravity, Cursor and OpenCode without Gateway",
        ] {
            assert!(index.contains(line), "{index}");
        }
        assert!(
            read("coding-agents", &offline)
                .1
                .starts_with("# Claude Code and Codex through Nexus\n")
        );
        assert!(
            read("plan-agents", &offline)
                .1
                .starts_with("# Antigravity, Cursor and OpenCode without Gateway\n")
        );
        assert!(
            read("models", &offline)
                .1
                .contains("for each coding agent (`claude`, `codex`).")
        );
        assert!(read("overview", &offline).1.contains(
            "| Claude Code, Codex, Antigravity, Cursor and OpenCode | Coding agents. For CLI sessions, use `nexus claude`, \
             `nexus codex`, `nexus agy` and `nexus cursor-agent`. |"
        ));
        assert!(
            read("skills", &offline)
                .1
                .contains("| `nexus claude`, `nexus codex` | The launcher writes")
        );
    }

    #[test]
    fn a_topic_is_found_by_its_name_however_it_is_written_and_by_other_words_for_it() {
        let pages = read("pages", &offline).1;
        assert!(pages.starts_with("# Pages"));
        assert_eq!(read(" `Pages`. ", &offline).1, pages);
        assert_eq!(read("coding agents", &offline).0, "coding-agents");
        assert_eq!(read("Coding_Agents", &offline).0, "coding-agents");
        // Another word for a topic; one that is a command too says how to get the command.
        let (shown, text) = read("billing", &offline);
        assert_eq!(shown, "spend");
        assert!(text.ends_with("ask for `nexus billing`."), "{text}");
        assert!(!read("slides", &offline).1.contains("ask for `nexus slides`"));
        for (alias, topic) in ALIASES {
            assert!(
                *topic == STATUS || topic_names().contains(topic),
                "{alias} names the topic {topic}, which does not exist"
            );
            assert!(!topic_names().contains(alias), "{alias} is a topic's own name");
        }
        // No such topic: the index, so the next call can name one.
        let (shown, text) = read("teleport", &offline);
        assert_eq!(shown, "teleport");
        assert!(text.starts_with("There is no topic named `teleport`.") && text.contains("- pages: "));
    }

    #[test]
    fn status_is_read_when_asked_and_only_then() {
        let asked = std::cell::Cell::new(0);
        let status = || {
            asked.set(asked.get() + 1);
            "live".to_string()
        };
        assert_eq!(read("pages", &status).0, "pages");
        assert_eq!(asked.get(), 0);
        assert_eq!(read("status", &status), ("status".into(), "live".into()));
        assert_eq!(read("whoami", &status).1, "live");
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn commands_come_from_the_cli_s_own_catalog() {
        let (shown, all) = read("commands", &offline);
        assert_eq!(shown, "commands");
        let catalog = command_catalog(crate::VERSION);
        for command in commands_of(&catalog) {
            let line = format!("- nexus {}: ", text_of(command, "name"));
            assert!(all.contains(&line), "{line}");
        }
        assert!(all.contains("- --json: ") && all.contains("- 2: "), "{all}");

        let (shown, share) = read("nexus pages share", &offline);
        assert_eq!(shown, "nexus pages share");
        assert!(share.starts_with("nexus pages share <id> "), "{share}");
        assert!(
            share.contains("\nFlags:\n") && share.contains("- --pin <N>: "),
            "{share}"
        );
        // Its arguments and flags may follow, and `nexus` may be left out.
        assert_eq!(read("nexus pages share pag_1 --org", &offline).1, share);
        assert_eq!(read("pages share", &offline).1, share);
        let pages = read("nexus pages", &offline).1;
        assert!(
            pages.contains("\nSubcommands:\n") && pages.contains("- nexus pages publish <file>"),
            "{pages}"
        );
        assert!(
            pages.contains("\nExamples:\n- nexus pages publish report.html"),
            "{pages}"
        );
        let login = read("nexus auth login", &offline).1;
        assert!(
            login.contains("\nThe same command also takes these forms:\n- nexus auth login --with-token"),
            "{login}"
        );
        // A product's commands are its Gateway's, not the catalog's.
        let product = read("nexus robustness run", &offline).1;
        assert!(
            product.starts_with("`robustness` is not a built-in `nexus` command."),
            "{product}"
        );
        assert!(product.contains("nexus <product> <verb>"), "{product}");
    }

    #[test]
    fn the_tool_answers_the_text_and_shows_which_topic_it_read() {
        let (text, action) = run(r#"{"topic":"chat"}"#, &offline);
        assert!(text.starts_with("# Chat"), "{text}");
        assert_eq!(
            action,
            Action::Docs {
                topic: "chat".into(),
                text
            }
        );
        // No arguments, or ones that are not JSON: the index.
        assert!(run("{}", &offline).0.starts_with("Nexus docs."));
        assert!(run("", &offline).0.starts_with("Nexus docs."));
    }

    /// The words after `nexus` in a line of a topic, up to a comment: `nexus pages share <page-id> --org   # …`.
    fn command_lines(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut fenced = false;
        for line in text.lines() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                // A pipe's last command is the `nexus` one: `echo "$KEY" | nexus auth login --with-token`.
                let line = line.split("  #").next().unwrap_or(line).trim();
                let command = line.rsplit(" | ").next().unwrap_or(line).trim();
                if let Some(rest) = command.strip_prefix("nexus ") {
                    found.push(rest.to_string());
                }
                continue;
            }
            // In prose, each `nexus …` in backticks.
            let mut parts = line.split('`');
            parts.next();
            while let Some(code) = parts.next() {
                if let Some(rest) = code.strip_prefix("nexus ") {
                    found.push(rest.to_string());
                }
                parts.next();
            }
        }
        found
    }

    /// A command a topic shows exists in this CLI, with every flag it uses. The catalog is the one `nexus help --json`
    /// prints, which a test holds equal to `commands.json`.
    #[test]
    fn every_command_the_topics_show_is_in_the_cli_with_its_flags() {
        let catalog = command_catalog(crate::VERSION);
        let all = commands_of(&catalog);
        let global: Vec<&str> = catalog["globalFlags"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|flag| text_of(flag, "name"))
            .collect();
        // The products a Gateway's modules add, as the catalog's own examples name them (`nexus robustness run`).
        let products: Vec<String> = all
            .iter()
            .filter(|command| text_of(command, "name") == "<product>")
            .flat_map(|command| command["examples"].as_array().cloned().unwrap_or_default())
            .filter_map(|example| example.as_str()?.split(' ').nth(1).map(String::from))
            .collect();
        let mut checked = 0;
        for topic in topics() {
            for line in command_lines(&topic.text) {
                let words: Vec<&str> = line.split_whitespace().collect();
                let first = words.first().copied().unwrap_or("");
                // `nexus <command>` and `nexus <product> …` are placeholders, not commands.
                if first.starts_with('<') || first.starts_with("--") || first == "…" {
                    continue;
                }
                if products.iter().any(|product| product == first) {
                    continue;
                }
                let path: Vec<String> = words
                    .iter()
                    .take_while(|word| word.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
                    .map(|word| word.to_string())
                    .collect();
                let command = find_command(all, &path)
                    .unwrap_or_else(|| panic!("{}: `nexus {line}` names no command of this CLI", topic.name));
                let name = text_of(command, "name");
                // A command that only groups others needs one of them: `nexus pages publsh` is a mistake.
                let groups = text_of(command, "usage").ends_with("<command> [options]")
                    || text_of(command, "usage").ends_with("<command>");
                let next = words.get(name.split(' ').count()).copied();
                if groups && let Some(next) = next {
                    assert!(
                        !next.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                        "{}: `nexus {line}`: `{name}` has no subcommand `{next}`",
                        topic.name
                    );
                }
                // Every flag is one the command, or a form of it, takes; a variant's flags are in its usage.
                let own: Vec<String> = command["flags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|flag| text_of(flag, "name").to_string())
                    .collect();
                let usages: Vec<&str> = std::iter::once(text_of(command, "usage"))
                    .chain(
                        command["variants"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|variant| text_of(variant, "usage")),
                    )
                    .collect();
                for flag in words.iter().filter(|word| word.starts_with("--")) {
                    let flag = flag.trim_end_matches([',', '.', ')', ':', ';']);
                    let known = own.iter().any(|name| name == flag)
                        || global.contains(&flag)
                        || usages.iter().any(|usage| usage.contains(flag));
                    assert!(known, "{}: `nexus {line}`: `nexus {name}` takes no {flag}", topic.name);
                }
                checked += 1;
            }
        }
        assert!(
            checked > 100,
            "only {checked} commands were checked: the topics lost their examples"
        );
    }

    #[test]
    fn status_says_who_the_agent_acts_as_and_what_is_off() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let address = listener.local_addr().expect("its address");
        std::thread::spawn(move || {
            for (served, mut stream) in listener.incoming().flatten().enumerate() {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                // The first answer is a person's; every later one is a refusal.
                let (status, body) = if served == 0 {
                    (
                        "200 OK",
                        json!({
                            "tenant": { "slug": "acme", "name": "Acme", "kind": "company" },
                            "user": { "id": "usr_1", "email": "ana@acme.example" },
                            "role": "admin",
                            "team": { "slug": "platform" },
                        }),
                    )
                } else {
                    (
                        "401 Unauthorized",
                        json!({ "error": { "message": "unknown token", "code": "unauthorized" } }),
                    )
                };
                let body = body.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let url = format!("http://{address}");
        let client = crate::cli::cli_client(&url, Some("k"));
        let text = status(&client, &url, Host::Desktop);
        for line in [
            "- Organization: acme (Acme), a company organization",
            "- Person: ana@acme.example",
            "- Role: admin",
            "- Default team: platform",
            "- You run in: a Code session of Nexus for desktop",
        ] {
            assert!(text.contains(line), "{line}\n{text}");
        }
        assert!(text.contains(&format!("- Gateway: {url}")), "{text}");
        let refused = status(&client, &url, Host::Terminal);
        assert!(
            refused.starts_with("Gateway did not say who you act as (GET /v1/tenants/me): "),
            "{refused}"
        );
        assert!(
            !refused.contains("- Role:") && refused.contains("- You run in: the `nexus` terminal session"),
            "{refused}"
        );
    }
}
