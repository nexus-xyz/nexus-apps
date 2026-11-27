//! What the agent is told before the conversation: who it is, where it works and how, what Nexus has,
//! what it has in this session, and the project's own instructions.
//!
//! Every step sends this prompt, so it stays short: it holds a map of Nexus, and the rule to read the knowledge base
//! (`knowledge.rs`, the `nexus_docs` tool) before the agent says or uses anything of Nexus. What it says about this
//! session is made from what the session has: the program that drives the agent ([`Host`]) and the MCP servers it gave.
//! A tool set that is off is never named as one the agent has.

use std::path::Path;
use std::process::Command;

/// The most of a project's AGENTS.md (or CLAUDE.md) the agent is given.
const MAX_INSTRUCTIONS: usize = 8_000;

/// The variable in which the program that starts `nexus __agent` says what it is. Nexus for desktop sets `desktop`.
pub const HOST_VARIABLE: &str = "NEXUS_AGENT_HOST";

/// The desktop app's own MCP servers, by the names it gives them (`nexus-workbench`'s `orchestra::SERVER`, and
/// `mcp_servers.rs`'s `BROWSER` and built-in server).
const APP_SERVER: &str = "nexus_app";
const BROWSER_SERVER: &str = "nexus_browser";
const NEXUS_SERVER: &str = "nexus";

/// Where the agent runs, which decides what it is told about its surroundings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// Agent mode of the `nexus` terminal session.
    Terminal,
    /// A Code session of Nexus for desktop, which starts `nexus __agent`.
    Desktop,
    /// `nexus __agent` under a program that did not say what it is.
    Program,
}

impl Host {
    /// The host of `nexus __agent`: what the program says in [`HOST_VARIABLE`], and an app from before it said so
    /// is known by its own MCP server.
    pub fn of_program(named: Option<&str>, servers: &[Server]) -> Host {
        let app = servers.iter().any(|(name, _)| name == APP_SERVER);
        match named.map(str::trim) {
            Some("desktop") => Host::Desktop,
            _ if app => Host::Desktop,
            _ => Host::Program,
        }
    }
}

/// An MCP server of this run and the names of its tools (`mcp.rs`'s `Mcp::servers`).
pub type Server = (String, Vec<String>);

/// What the prompt is made from.
pub struct Setting<'a> {
    pub workspace: &'a Path,
    pub host: Host,
    pub servers: &'a [Server],
}

const WORK: &str = "How you work:\n\
- Read before you edit, and keep to the style of the code around your change.\n\
- For a task of several steps, keep a plan with update_plan.\n\
- Work in small steps and check them (build, test) when you can.\n\
- Give a search or a separate piece of work to a subagent with agent, when it would fill your context.\n\
- When a task needs a fact that you do not have, look in the workspace, then ask the person.\n\
- Keep answers short and plain. When you finish, say what you changed and what is left.";

/// The map of Nexus. Each topic in brackets is one `nexus_docs` reads; a test holds the two lists equal.
const NEXUS: &str = "What Nexus has. The words in brackets are its topics in nexus_docs:\n\
- Models: every model the organization allows, through one API. nexus/auto lets Nexus choose. [models]\n\
- Spend: budgets, a limit for each person, and a record of every call. [spend]\n\
- Pages: a file published as a private page, a design canvas or a slide deck, then shared. [pages, design]\n\
- Chat sessions with any model the organization allows. [chat]\n\
- Coding agents, billing routes and shared memory. [coding-agents, plan-agents, shared-memory]\n\
- Organizations, teams, roles, keys and the audit log. [organization, auth]\n\
- Connectors, skills, the desktop app, and @nexus, the person's own assistant, which is not you. [connectors, \
skills, desktop, assistant, slack-and-inbox, errands]\n\
The nexus CLI reaches most of it. More topics: [overview], [session] for the terminal's keys and slash commands, \
[agent] for your own tools and limits, [api], [errors], [status] for who you act as now, [commands] for every nexus \
command, and the topic `nexus <command>`, such as `nexus pages share`, for one command's usage and flags.";

/// The rules that hold whatever the topic. The last two are here, not only in a topic, because the map above tells the
/// agent of commands that delete, share and take keys before it has read anything.
const RULES: &str = "Rules about Nexus:\n\
- Before you answer a question about Nexus, or use one of its features, read its topic with nexus_docs. Do not answer \
from memory, and do not run a command to learn its usage.\n\
- Say only what the docs or a command's output say. When they do not cover something, say that you do not know.\n\
- Never offer a Nexus feature, a nexus command or a flag of one that they do not name.\n\
- Read from Nexus freely, and publish a private page when it helps. Make any other change in Nexus only when the \
person asked for exactly that change: a share, a delete, a revoke, an invite, a policy, a budget, a switch of \
organization or a sign-out.\n\
- Never run a command that takes or prints a secret, and never repeat a secret. [auth] says what to do instead.";

/// What the agent is told before the conversation.
pub fn system_prompt(setting: &Setting) -> String {
    let place = match setting.host {
        Host::Terminal => "You run in the person's terminal, in agent mode of the `nexus` session.",
        Host::Desktop => "You run in a Code session of Nexus for desktop.",
        Host::Program => "You run with no screen of your own: another program drives you and shows your work.",
    };
    let mut prompt = format!(
        "You are the Nexus agent: the coding agent built into Nexus, the AI gateway that a company owns. {place}\n\
         Your model calls go through Nexus Gateway as the signed-in person. Each call follows their organization's \
         model policy, counts against their budget and is recorded under their name.\n\n\
         Workspace: {} ({}).{}\n\
         Your tools read, search and edit files there and run commands. By default the person approves each edit and \
         command first, and may decline.\n\n\
         {WORK}\n\n{NEXUS}\n\n{RULES}\n\n{}",
        setting.workspace.display(),
        std::env::consts::OS,
        git(setting.workspace).map(|git| format!(" {git}")).unwrap_or_default(),
        here(setting.host, setting.servers),
    );
    for name in ["AGENTS.md", "CLAUDE.md"] {
        if let Ok(text) = std::fs::read_to_string(setting.workspace.join(name)) {
            let shown: String = text.chars().take(MAX_INSTRUCTIONS).collect();
            prompt.push_str(&format!("\n\nThe project's instructions ({name}):\n{shown}"));
            // Said, so the agent reads on instead of taking the cut for the end.
            if shown.len() < text.len() {
                prompt.push_str(&format!(
                    "\n[{name} goes on: this is its first {MAX_INSTRUCTIONS} characters. Read the rest with read_file \
                     when the task needs it.]"
                ));
            }
            break;
        }
    }
    prompt
}

/// What this session has, by where the agent runs and the servers it was given.
fn here(host: Host, servers: &[Server]) -> String {
    let mut lines: Vec<String> = Vec::new();
    match host {
        Host::Terminal => {
            lines.push("Here, in the terminal session:".into());
            lines.push(
                "- The person sees your text and a line for each tool. Unless they turned the questions off, they \
                 answer an edit or a command with y, a (always) or n."
                    .into(),
            );
            lines.push(
                "- A nexus command that you run with run_command acts as the same person, on the same Gateway.".into(),
            );
            lines.push(
                "- Slash commands and keys are the person's, not yours. Name the one that helps: /model, /approvals, \
                 /status, /spend, /resume, /new, /compact, /cost, /diff, /btw, /help, and shift+tab to switch the mode."
                    .into(),
            );
            lines.extend(servers.iter().map(server_line));
        }
        Host::Desktop => {
            lines.push("Here, in Nexus for desktop:".into());
            lines.push(
                "- The person sees your text, your tools as cards and your plan, beside a pane with Overview, Changes, \
                 Terminal, Browser and Artifact tabs."
                    .into(),
            );
            lines.extend(servers.iter().map(server_line));
            if servers
                .iter()
                .any(|(name, tools)| name == APP_SERVER && tools.iter().any(|tool| tool == "terminal_run"))
            {
                lines.push("- Use the shared terminal tools for every command, including reads and sub-agent work. Full access changes approval, not routing. Native run_command is disabled. Keep interactive input on the same execution. Never bypass approval, denial, private input, or takeover. Explain technical routing failures and never replay an uncertain launch.".into());
            }
            // What the app has and this session does not, said plainly, so the agent neither calls it nor claims it.
            let has = |server: &str| servers.iter().any(|(name, _)| name == server);
            let linked = servers
                .iter()
                .any(|(name, tools)| name == APP_SERVER && tools.iter().any(|tool| tool == "start_session"));
            if !linked {
                lines.push(
                    "- You have no tools for subsessions or schedules here. New sessions get them by default. The \
                     person turns them on for this session with Subsessions in its title menu."
                        .into(),
                );
            }
            if !has(BROWSER_SERVER) {
                lines.push(
                    "- You have no browser tools here: you cannot open, read or click in the Browser tab.".into(),
                );
            }
            if !has(NEXUS_SERVER) {
                lines.push("- You have no page tools of the app here.".into());
            }
            lines.push(format!(
                "- A nexus command in the shell uses the CLI's own saved sign-in, not this session's. It can be another \
                 organization, or signed out. Before one that publishes or changes something, run `nexus auth \
                 status`, tell the person which organization it acts in, and go on only when they agree.{}",
                if has(NEXUS_SERVER) {
                    ""
                } else {
                    " That is how you publish a page here."
                }
            ));
            lines.push(
                "- To show a result: for a web app, keep its server running and write its full address, such as \
                 \"running at http://localhost:5173\". The app shows it in the Browser tab. For an image or a video, \
                 save it under .nexus/artifacts/ and link it in your answer."
                    .into(),
            );
            lines.push("- Read [desktop] before you use the app's tools or explain the app.".into());
        }
        Host::Program => {
            lines.push("Here:".into());
            lines.push(
                "- A nexus command that you run uses the CLI's own saved sign-in. Before one that publishes or changes \
                 something, run `nexus auth status` and say which organization it acts in."
                    .into(),
            );
            lines.extend(servers.iter().map(server_line));
        }
    }
    lines.join("\n")
}

/// One server the session has: what its tools are for, and how they are named. The desktop app's are known by name;
/// any other is counted.
fn server_line((name, tools): &Server) -> String {
    let has = |tool: &str| tools.iter().any(|name| name == tool);
    let named = format!("named mcp__{name}__<tool>");
    match name.as_str() {
        APP_SERVER => {
            let mut sets = Vec::new();
            if has("start_session") {
                sets.push("subsessions (start_session, message_session, wait_sessions, list_sessions, stop_session)");
            }
            if has("schedule_session") {
                sets.push("schedules (schedule_session, list_schedules, cancel_schedule)");
            }
            if has("terminal_run") {
                sets.push(
                    "the session's Terminal tab (terminal_run, terminal_read and more: use them for a server that \
                     must keep running)",
                );
            }
            if sets.is_empty() {
                return format!("- The app gave you its own tools, {named}: {} of them.", tools.len());
            }
            format!("- The app gave you its own tools, {named}: {}.", sets.join("; "))
        }
        BROWSER_SERVER => format!(
            "- The app gave you its browser tools, {named}: open, read, click, type and take a screenshot in the \
             session's Browser tab. Check a page that you built with them."
        ),
        NEXUS_SERVER => format!(
            "- The app gave you Nexus's own tools, {named}, under this session's sign-in. Publish a page with \
             createPage and change it with writePageVersion."
        ),
        _ => format!(
            "- You have the MCP server {name}: {} tool{}, {named}.",
            tools.len(),
            if tools.len() == 1 { "" } else { "s" }
        ),
    }
}

/// The workspace's branch and where it comes from, when it is a git repository and git is installed.
fn git(workspace: &Path) -> Option<String> {
    let run = |args: &[&str]| {
        let output = Command::new("git").args(args).current_dir(workspace).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    // It fails outside a repository, and prints nothing on a detached HEAD.
    let branch = run(&["branch", "--show-current"])?;
    let branch = if branch.is_empty() {
        "no branch (a detached HEAD)".to_string()
    } else {
        format!("branch {branch}")
    };
    Some(
        match run(&["config", "--get", "remote.origin.url"]).filter(|url| !url.is_empty()) {
            Some(url) => format!("Git: {branch}, origin {}.", remote(&url)),
            None => format!("Git: {branch}."),
        },
    )
}

/// A remote's address without its credentials or its scheme: `https://user:token@host/a/b.git` and
/// `git@host:a/b.git` are both `host/a/b`.
fn remote(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((_, rest)) => (true, rest),
        None => (false, url),
    };
    // What comes before the last `@` of the host part is a user, or a user and a token.
    let host_end = rest.find('/').unwrap_or(rest.len());
    let rest = match rest[..host_end].rfind('@') {
        Some(at) => &rest[at + 1..],
        None => rest,
    };
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    if scheme {
        rest.to_string()
    } else {
        rest.replacen(':', "/", 1)
    }
}

#[cfg(test)]
mod tests {
    use super::super::knowledge;
    use super::*;

    fn workspace() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-agent-prompt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("workspace");
        dir
    }

    fn prompt(host: Host, servers: &[Server]) -> String {
        let dir = workspace();
        let prompt = system_prompt(&Setting {
            workspace: &dir,
            host,
            servers,
        });
        let _ = std::fs::remove_dir_all(dir);
        prompt
    }

    fn server(name: &str, tools: &[&str]) -> Server {
        (name.to_string(), tools.iter().map(|tool| tool.to_string()).collect())
    }

    #[test]
    fn names_the_workspace_and_reads_agents_md() {
        let dir = workspace();
        let setting = Setting {
            workspace: &dir,
            host: Host::Terminal,
            servers: &[],
        };
        assert!(system_prompt(&setting).contains(&format!("Workspace: {} (", dir.display())));
        std::fs::write(dir.join("AGENTS.md"), "Run cargo fmt.").expect("file");
        assert!(system_prompt(&setting).ends_with("The project's instructions (AGENTS.md):\nRun cargo fmt."));
        // A long file is cut, and the agent is told that it goes on.
        std::fs::write(dir.join("AGENTS.md"), "é".repeat(MAX_INSTRUCTIONS + 1)).expect("file");
        let cut = system_prompt(&setting);
        assert!(cut.ends_with("Read the rest with read_file when the task needs it.]"));
        assert_eq!(cut.matches('é').count(), MAX_INSTRUCTIONS);
        // CLAUDE.md is read when there is no AGENTS.md.
        std::fs::remove_file(dir.join("AGENTS.md")).expect("removed");
        std::fs::write(dir.join("CLAUDE.md"), "Use tabs.").expect("file");
        assert!(system_prompt(&setting).ends_with("The project's instructions (CLAUDE.md):\nUse tabs."));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The map names every topic of the knowledge base, and no topic that is not one.
    #[test]
    fn the_map_names_every_topic_of_the_knowledge_base_and_only_those() {
        let prompt = prompt(Host::Terminal, &[]);
        for topic in knowledge::topic_names() {
            assert!(
                bracketed(NEXUS).contains(&topic.to_string()),
                "the map does not name [{topic}]"
            );
        }
        let made = ["status", "commands"];
        for named in bracketed(&prompt) {
            assert!(
                knowledge::topic_names().contains(&named.as_str()) || made.contains(&named.as_str()),
                "[{named}] is not a topic"
            );
        }
        assert!(prompt.contains("nexus_docs") && prompt.contains("`nexus <command>`"));
    }

    /// The names in square brackets, which the prompt uses for topics only.
    fn bracketed(text: &str) -> Vec<String> {
        text.split('[')
            .skip(1)
            .filter_map(|rest| rest.split_once(']'))
            .flat_map(|(names, _)| names.split(", ").map(String::from).collect::<Vec<_>>())
            .collect()
    }

    #[test]
    fn says_where_it_runs_and_only_what_that_place_has() {
        let terminal = prompt(Host::Terminal, &[]);
        assert!(terminal.contains("You run in the person's terminal") && terminal.contains("/approvals"));
        assert!(terminal.contains("acts as the same person, on the same Gateway"));
        for desktop_only in ["Nexus for desktop:", "Browser tab", ".nexus/artifacts/", "createPage"] {
            assert!(!terminal.contains(desktop_only), "the terminal names {desktop_only}");
        }

        let desktop = prompt(Host::Desktop, &[]);
        assert!(desktop.contains("You run in a Code session of Nexus for desktop."));
        assert!(desktop.contains("Overview, Changes, Terminal, Browser and Artifact tabs"));
        assert!(desktop.contains("uses the CLI's own saved sign-in, not this session's"));
        for terminal_only in ["/approvals", "shift+tab", "y, a (always) or n"] {
            assert!(!desktop.contains(terminal_only), "the desktop names {terminal_only}");
        }

        let program = prompt(Host::Program, &[]);
        assert!(program.contains("another program drives you"));
        assert!(!program.contains("/approvals") && !program.contains("Browser tab"));

        // The rules that guard a person's organization and secrets are told everywhere, before any topic is read.
        for told in [&terminal, &desktop, &program] {
            assert!(
                told.contains("only when the person asked for exactly that change"),
                "{told}"
            );
            assert!(told.contains("Never run a command that takes or prints a secret"));
            assert!(told.contains("do not run a command to learn its usage"));
        }
    }

    /// A tool set of the app is named as the agent's own only when the session has it. What is off is said to be off.
    #[test]
    fn never_names_a_tool_set_that_is_off_as_one_the_agent_has() {
        let off = [
            "You have no tools for subsessions or schedules here.",
            "You have no browser tools here",
            "You have no page tools of the app here.",
        ];
        let none = prompt(Host::Desktop, &[]);
        for line in off {
            assert!(none.contains(line), "{line}");
        }
        assert!(none.contains("go on only when they agree. That is how you publish a page here."));
        // Subsessions have no project switch: the off line names the session's title menu only.
        assert!(none.contains("turns them on for this session with Subsessions in its title menu"));
        assert!(!none.contains("for the project"));
        for tool in [
            "start_session",
            "terminal_run",
            "browser tools, named",
            "createPage",
            "schedule_session",
            "mcp__",
        ] {
            assert!(!none.contains(tool), "{tool} is off, and the prompt names it");
        }

        // Today's app: its own server, with the terminal tools only.
        let terminal_tools = [server(APP_SERVER, &["terminal_run", "terminal_read", "terminal_open"])];
        let today = prompt(Host::Desktop, &terminal_tools);
        assert!(today.contains(
            "its own tools, named mcp__nexus_app__<tool>: the session's Terminal tab (terminal_run, terminal_read"
        ));
        assert!(!today.contains("start_session") && !today.contains("schedule_session"));
        for line in off {
            assert!(today.contains(line), "{line}");
        }

        // With subsessions on, and with the servers the app does not give the agent yet.
        let all = [
            server(
                APP_SERVER,
                &["start_session", "wait_sessions", "schedule_session", "terminal_run"],
            ),
            server(BROWSER_SERVER, &["browser_open"]),
            server(NEXUS_SERVER, &["createPage", "writePageVersion"]),
            server("linear", &["list_issues"]),
        ];
        let full = prompt(Host::Desktop, &all);
        assert!(full.contains("subsessions (start_session, ") && full.contains("schedules (schedule_session, "));
        assert!(full.contains("its browser tools, named mcp__nexus_browser__<tool>"));
        assert!(full.contains("Publish a page with createPage and change it with writePageVersion."));
        assert!(full.contains("- You have the MCP server linear: 1 tool, named mcp__linear__<tool>."));
        assert!(!full.contains("You have no"), "{full}");
        assert!(!full.contains("That is how you publish a page here."));
    }

    /// Every step sends the prompt: it stays a map, not a manual.
    #[test]
    fn stays_short() {
        for (host, most) in [(Host::Terminal, 3_500), (Host::Desktop, 4_300), (Host::Program, 3_200)] {
            let size = prompt(host, &[]).chars().count();
            assert!(size <= most, "{host:?}: {size} characters, more than {most}");
        }
    }

    #[test]
    fn the_program_says_what_it_is_and_an_older_app_is_known_by_its_server() {
        let app = [server(APP_SERVER, &["terminal_run"])];
        assert_eq!(Host::of_program(Some("desktop"), &[]), Host::Desktop);
        assert_eq!(Host::of_program(Some(" desktop\n"), &[]), Host::Desktop);
        assert_eq!(Host::of_program(None, &app), Host::Desktop);
        assert_eq!(Host::of_program(None, &[]), Host::Program);
        assert_eq!(Host::of_program(Some("ci"), &[server("linear", &[])]), Host::Program);
    }

    #[test]
    fn a_remote_is_named_without_its_credentials() {
        assert_eq!(remote("git@github.com:acme/app.git"), "github.com/acme/app");
        assert_eq!(
            remote("https://x-access-token:secret@github.com/acme/app.git"),
            "github.com/acme/app"
        );
        assert_eq!(remote("https://github.com/acme/app"), "github.com/acme/app");
        assert_eq!(
            remote("ssh://git@host.example:2222/acme/app.git"),
            "host.example:2222/acme/app"
        );
        assert_eq!(remote("https://host.example/a@b/app.git"), "host.example/a@b/app");
    }

    #[test]
    fn names_the_branch_of_a_git_repository() {
        let dir = workspace();
        assert_eq!(git(&dir), None, "not a repository");
        let ran = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .is_ok_and(|output| output.status.success())
        };
        // Without git, there is nothing to check.
        if ran(&["init", "-q", "-b", "main"]) {
            assert_eq!(git(&dir).as_deref(), Some("Git: branch main."));
            assert!(ran(&[
                "remote",
                "add",
                "origin",
                "https://ana:secret@github.com/acme/app.git"
            ]));
            assert_eq!(
                git(&dir).as_deref(),
                Some("Git: branch main, origin github.com/acme/app.")
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
