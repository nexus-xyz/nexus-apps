//! What the workbench says to a running agent and what it hears back, whichever agent it is: Claude Code
//! ([`crate::claude`]) or Codex ([`crate::codex`]). Each agent's [`Protocol`] writes a [`Say`] as the lines its stdin
//! takes and reads each line of its stdout as [`Out`]s, so the rest of the workbench never sees a protocol.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::event::{
    AgentCapabilities, Effort, Image, PLAN_TOOL, PermissionMode, QUESTION_TOOL, SessionAgent, Usage, WbEvent,
};
use crate::harness::{RunFacts, harness};

/// The most output an agent tool result keeps in its event.
pub const MAX_TOOL_OUTPUT: usize = 64 * 1024;

pub(crate) fn cap(text: String) -> (String, bool) {
    if text.len() <= MAX_TOOL_OUTPUT {
        return (text, false);
    }
    let mut end = MAX_TOOL_OUTPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// The desktop can show structured questions and return the person's answer to the same call.
pub const QUESTION_GUIDANCE: &str = "When you need a decision from the person, use your structured user-input tool instead of listing choices only in prose. Wait for required answers before dependent work. Continue independent work only when the question supports nonblocking input. Never treat an unanswered question as approval.";

/// Desktop pages use the app's built-in MCP publisher and sign-in. The standalone CLI can name another Gateway,
/// so its publishing instructions must not enter a desktop session.
pub const PAGE_INSTRUCTIONS: &str = include_str!("page-instructions.txt").trim_ascii_end();

/// Requested websites should reach a running result beside the conversation.
pub const WEB_OUTPUT_GUIDANCE: &str = "When asked to build a website or web app, create the working result, start its local server, and print its complete HTTP or HTTPS localhost URL. Keep the server running so Nexus can show the result beside the conversation. Verify the result with the Nexus browser tools when agent control is allowed. Never claim browser verification without using those tools. Ordinary questions need only a conversational answer.";

/// Proof the person sees in the reply: screenshots and short videos, saved in the project and linked, show
/// inline in Nexus ([`crate::output_media`] sets the types and caps named here).
pub const MEDIA_GUIDANCE: &str = "When the person asks to see the work, such as a screenshot or a video of a feature, save each file under `.nexus/artifacts/` in the project. Before the first file, make sure that the file at `git rev-parse --git-path info/exclude` has the line `.nexus/artifacts/`, so git ignores the folder. Do not change `.gitignore` for this. In your reply, link each file with its path from the folder you started in: `![Login page](.nexus/artifacts/login.png)` for an image, `[Login flow](.nexus/artifacts/login.webm)` for a video. Nexus shows PNG, JPEG, GIF and WebP images up to 10 MB and MP4, WebM and MOV videos up to 50 MB in the reply. Keep each video shorter than 60 seconds. With the Playwright browser tools, give `browser_take_screenshot` a `filename` under `.nexus/artifacts/`. To record when the tools have `browser_start_video`, call it with a `filename` under `.nexus/artifacts/`, do the steps, then call `browser_stop_video`. When `ffmpeg` is available, use it to cut a long video or to make a GIF. Never claim that a file shows the result before you saved it.";

/// A file an answer names opens in the Workbench on a click, and following the agent opens a linked one on its own.
/// Both read the path from the folder the session started in, so a bare name or a path from another folder opens
/// "File not found".
pub const FILE_LINK_GUIDANCE: &str = "Nexus opens each file your reply links or names in code, so write a path that leads to it. Use the path from the folder you started in, such as `[side_pane.rs](src/workspace/side_pane.rs:42)`, even after you changed folder. Never shorten it to the file name alone. Use the absolute path for a file outside that folder, and never a path inside a sandbox that the person cannot reach. Link a file only after you saved it, and do not link a file you deleted or moved. Write an example path that names no real file in a fenced code block.";

/// Commands use the desktop's execution broker even when they need no approval.
pub const TERMINAL_GUIDANCE: &str = include_str!("terminal/instructions.txt").trim_ascii_end();

/// How an agent reaches another of the app's sessions by its name: the `nexus` CLI's `sessions` commands, through the
/// app's Outside access. No tool of its own.
pub const SESSIONS_GUIDANCE: &str = "To reach another Nexus for desktop session, run `nexus sessions list` to find its name, then `nexus sessions send <name> \"<message>\"`.";

/// What the workbench adds to every agent's system prompt: Claude Code's `--append-system-prompt`, Codex's
/// `developerInstructions`. The commit trailer's note holds with or without the hook (only some accounts get it), so every session gets it.
pub fn instructions() -> String {
    let commits = nexus_client::commit_trailer::INSTRUCTIONS;
    let browser = crate::browser_routing::GUIDANCE;
    format!(
        "{QUESTION_GUIDANCE}\n\n{WEB_OUTPUT_GUIDANCE}\n\n{MEDIA_GUIDANCE}\n\n{FILE_LINK_GUIDANCE}\n\n{TERMINAL_GUIDANCE}\n\n{SESSIONS_GUIDANCE}\n\n{browser}\n\n{commits}\n\n{PAGE_INSTRUCTIONS}"
    )
}

/// [`instructions`] for an agent that takes its MCP servers in its session (Antigravity, [`crate::acp`]): it gets the
/// built-in `nexus` server (pages, connectors) and `nexus_browser`, but not the app's session tools (`nexus_app`: the
/// shared terminal) or a structured question tool. The guidance that names a tool it lacks would only have it explain
/// that the tool is missing.
pub fn session_tool_instructions() -> String {
    let commits = nexus_client::commit_trailer::INSTRUCTIONS;
    let browser = crate::browser_routing::GUIDANCE;
    format!(
        "{WEB_OUTPUT_GUIDANCE}\n\n{MEDIA_GUIDANCE}\n\n{FILE_LINK_GUIDANCE}\n\n{SESSIONS_GUIDANCE}\n\n{browser}\n\n{commits}\n\n{PAGE_INSTRUCTIONS}"
    )
}

/// [`instructions`] as a file in `dir`, for Claude Code's `--append-system-prompt-file`. They span several lines, and
/// Windows can't take a line break in an argument to a batch file: Rust refuses to start one with it (CVE-2024-24576),
/// and npm installs Claude Code as `claude.cmd`. Named by its content, so app versions that say different things never
/// share a file, and one already there with the same bytes is left alone (a running agent may be reading it). Written
/// under another name and renamed into place, so a launch at the same moment never reads half of it.
pub fn instructions_file(dir: &Path) -> std::io::Result<PathBuf> {
    instructions_file_with(dir, None)
}

/// [`instructions_file`] with `extra` after the instructions: a project's own instructions, when Claude Code will not
/// load them itself ([`crate::claude_trust::project_instructions`]).
pub fn instructions_file_with(dir: &Path, extra: Option<&str>) -> std::io::Result<PathBuf> {
    match extra {
        Some(extra) => instructions_file_with_text(dir, &format!("{}\n\n{extra}", instructions())),
        None => instructions_file_with_text(dir, &instructions()),
    }
}

pub(crate) fn instructions_file_with_text(dir: &Path, text: &str) -> std::io::Result<PathBuf> {
    let id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, text.as_bytes()).simple();
    let path = dir.join(format!("instructions-{id}.md"));
    let written = || std::fs::read(&path).is_ok_and(|bytes| bytes == text.as_bytes());
    if written() {
        return Ok(path);
    }
    std::fs::create_dir_all(dir)?;
    let staged = dir.join(format!(".instructions-{id}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&staged)?;
    let wrote = file.write_all(text.as_bytes());
    drop(file);
    if let Err(error) = wrote {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    let renamed = std::fs::rename(&staged, &path);
    if renamed.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    match renamed {
        Ok(()) => Ok(path),
        // Windows refuses to replace a file another process has open: one with the right bytes will do.
        Err(_) if written() => Ok(path),
        Err(error) => Err(error),
    }
}

/// What the workbench asks of the running agent. `request` ids come back in [`Out::Ack`].
#[derive(Debug, Clone, PartialEq)]
pub enum Say {
    /// Drop metadata request bookkeeping after cancellation. Writes nothing to the runtime.
    ForgetInspection {
        request: String,
    },
    /// Read metadata through this process, without creating a turn.
    Inspect {
        request: String,
        field: &'static str,
        cursor: Option<String>,
    },
    /// A native history operation, never an ordinary model prompt.
    Compact {
        request: String,
        instructions: Option<String>,
    },
    User {
        text: String,
        images: Vec<Image>,
    },
    /// Claude Code's own `/rename <name>` for the session's accepted name: it takes it at once, with no model turn.
    Rename {
        name: String,
    },
    /// A message for the turn under way, to take in as it goes ("Send now"); with no turn under way, it starts one.
    /// `person`: the person's message, not Nexus's own notice ([`crate::Workbench::steer_notice`]). A driver that
    /// refuses the steer says it back in [`Out::TurnQueued`].
    Steer {
        text: String,
        images: Vec<Image>,
        person: bool,
    },
    Interrupt {
        request: String,
    },
    Mode {
        request: String,
        mode: PermissionMode,
    },
    Model {
        request: String,
        model: Option<String>,
    },
    Effort {
        request: String,
        effort: Option<Effort>,
    },
    /// Claude Code's fast mode on or off ([`crate::claude::set_fast`]). Only a Claude Code session is sent one.
    Fast {
        request: String,
        on: bool,
    },
    /// Allows what the agent asked about (`input`, with `answers` for a question), remembering `remember`: one of the
    /// approval's suggestions.
    Allow {
        request_id: String,
        input: Value,
        answers: Option<Map<String, Value>>,
        remember: Option<Value>,
    },
    /// Denies it; `message` is the person's reason, if they gave one.
    Deny {
        request_id: String,
        message: Option<String>,
    },
    /// Stops one task: a sub-agent or a background command. The answer comes back in [`Out::Answered`].
    TaskStop {
        request: String,
        task_id: String,
    },
    /// Moves the foreground command of this tool call to the background; answered in [`Out::Answered`].
    TaskBackground {
        request: String,
        tool_use_id: String,
    },
    /// Reads a background command's output; answered in [`Out::Answered`].
    TaskOutput {
        request: String,
        task_id: String,
    },
}

/// What one line of the agent's output means for the session.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// Runtime observations. The workbench whitelists fields before storing or displaying them.
    Observe(Value),
    /// A navigation title from the session's own agent. Manual titles always win.
    Title(String),
    /// An event for the session's log.
    Event(WbEvent),
    /// A running task's update as the agent reports it (a `task` event with `detail.progress`): the core merges it into
    /// its record and decides whether it reaches the log (agents.rs).
    Progress(WbEvent),
    /// Streamed text of the message block `id` so far, for the screen only. `parent`: the sub-agent's tool call.
    /// The text is append-mostly: the screen gets only the new tail while it extends what was sent, else the whole text.
    Delta {
        id: String,
        text: String,
        parent: Option<String>,
    },
    /// The conversation is live under this id, on this model, with this version of the agent.
    Init {
        native_id: String,
        model: Option<String>,
        version: Option<String>,
    },
    /// One API call finished: what it used. `parent` is the sub-agent's tool call, if any.
    Call {
        model: String,
        usage: Usage,
        parent: Option<String>,
    },
    /// A refused steer will run as another turn. Reserve its place before the active turn ends. `person`: the steer's
    /// own (`Say::Steer`), so a notice's turn is never the person's.
    TurnQueued { person: bool },
    /// The turn ended. `total_cost_usd` counts every turn of this process so far, when the agent reports it.
    Result {
        total_cost_usd: Option<f64>,
        duration_ms: u64,
        error: Option<String>,
        interrupted: bool,
        context_window: Option<u64>,
    },
    /// The agent answered one of our requests.
    Ack { request_id: String, error: Option<String> },
    /// The agent answered a request about one task (`Say::TaskStop`, `TaskBackground`, `TaskOutput`): what it said, or why it refused. A refusal the
    /// protocol makes itself, without asking the agent, comes back the same way.
    Answered {
        request_id: String,
        result: Result<Value, String>,
    },
    /// The agent withdrew an approval it asked for (the turn was interrupted).
    Cancelled { request_id: String },
    /// A line to write to the agent at once.
    Reply(String),
    /// Claude Code's `rate_limit_event` (Codex's `account/rateLimits/updated`): how much of the plan's five-hour
    /// (`session`) and weekly limits the last call left used, 0 to 100, and when each window resets (epoch
    /// milliseconds) when the agent said. `refused`: the call hit a limit, which resets at `resets_at`.
    RateLimit {
        session: Option<f64>,
        weekly: Option<f64>,
        session_resets: Option<i64>,
        weekly_resets: Option<i64>,
        refused: bool,
        resets_at: Option<i64>,
    },
    /// Work the agent left running past its turn (a background shell or sub-agent) started, ended, or is now exactly
    /// this list. While any runs, the agent is not idle.
    Background(Background),
    /// A running sub-agent now has a protocol target for Stop. The core uses this to stop background work.
    TaskReady(String),
    /// The conversation cannot go on in this process: close the agent's input, so it ends.
    Close,
    /// Where the agent's own conversation now stands, for a revert to go back to: Claude Code's latest message in the
    /// main conversation (its `uuid`), or the Codex turn under way.
    Cursor(String),
    /// The agent took one of the person's messages into its conversation: Claude Code echoed it
    /// (`--replay-user-messages`), or Codex recorded it as a `userMessage` item. `texts`: its text parts (Codex joins a
    /// message sent before its turn was named into the one before, so one echo can hold two).
    Taken { texts: Vec<String> },
    /// Antigravity: whether agy offers its `auto` mode on this sign-in, as its session's answer or its refusal of
    /// `auto` said. Its next start's arguments depend on it ([`crate::harness::antigravity::args`]).
    AutoOffered(bool),
}

/// A change to the agent's background work, by task id.
#[derive(Debug, Clone, PartialEq)]
pub enum Background {
    Started(String),
    Ended(String),
    /// Every task still running.
    Now(Vec<String>),
}

/// One agent's protocol, for one run of its process.
pub trait Protocol: Send {
    /// The MCP servers the host launched this run with, for an agent that takes them in its session
    /// ([`crate::mcp_servers::session_entries`]). Comes after [`Protocol::open`] and before the agent's first line is
    /// read, so before the conversation opens; other agents ignore it.
    fn session_mcp(&mut self, _mcp: crate::mcp_servers::SessionMcp) {}
    /// The model the host resolved for this run (for example, after Auto routing).
    fn launch_model(&mut self, _model: &str) {}
    /// A saved server's native model reference must be active before the first prompt.
    fn require_launch_model(&mut self, _model: &str) {}
    /// What starts the conversation, before any message.
    fn open(&mut self) -> Vec<Out>;
    fn say(&mut self, say: Say) -> Vec<Out>;
    /// One line of the agent's stdout.
    fn read(&mut self, line: &str) -> Vec<Out>;
    /// How long the agent may say nothing before [`Protocol::quiet`] runs. None: its silence means nothing now.
    fn quiet_after(&self) -> Option<std::time::Duration> {
        None
    }
    /// The agent said nothing for [`Protocol::quiet_after`] since its last line.
    fn quiet(&mut self) -> Vec<Out> {
        Vec::new()
    }
}

/// How a session's agent process starts.
#[derive(Debug, Clone, Copy)]
pub struct Start<'a> {
    pub mode: PermissionMode,
    pub model: Option<&'a str>,
    pub effort: Option<Effort>,
    /// Claude Code's fast mode ([`crate::event::ThreadSummary::fast`]).
    pub fast: bool,
    /// The agent's own id for the conversation.
    pub native_id: &'a str,
    /// The conversation exists: continue it.
    pub resume: bool,
    /// With `resume`: continue it as a new conversation (a fork), leaving the one at `native_id` as it is.
    pub fork: bool,
    pub cwd: &'a str,
    /// A revert took the session back (`Workbench::revert_to_turn`): Claude Code resumes only up to this message,
    /// Codex drops this turn and every later one before its first turn. Only with `resume`.
    pub rewind: Option<&'a str>,
    /// A message edit requires a real cut. Refuse the prompt if the provider cannot rewind.
    pub rewind_required: bool,
    /// [`instructions`] as a file ([`instructions_file`]), for Claude Code; None: they go inline on the command line,
    /// which a batch-file install on Windows can't be started with.
    pub instructions: Option<&'a Path>,
    pub prompt: Option<&'a str>,
    /// The session's name (session_name.rs), which Claude Code gets as `--name`; None: no name.
    pub name: Option<&'a str>,
}

/// The agent's own arguments for one run. `sandbox`: Antigravity's Sandbox switch ([`crate::Host::antigravity_sandbox`]),
/// and `auto`: whether agy offers its `auto` mode, as its last run said ([`Out::AutoOffered`]); the other agents have
/// neither.
pub fn args(agent: SessionAgent, start: &Start, sandbox: bool, auto: Option<bool>) -> Vec<String> {
    let run = RunFacts {
        sandbox,
        auto_offered: auto,
    };
    harness(agent).args(start, &run)
}

pub fn protocol(agent: SessionAgent, start: &Start) -> Box<dyn Protocol> {
    harness(agent).protocol(start)
}

/// What a running agent of this version accepts. None for an older or unknown version: no task control. The oldest
/// versions with task controls are each agent's (`src/harness/<agent>.rs`).
pub fn capabilities(agent: SessionAgent, version: Option<&str>) -> Option<AgentCapabilities> {
    harness(agent).capabilities(version)
}

/// "2.1.292" (or "0.164.0-alpha.1") as numbers; None when it is not a version.
pub(crate) fn semver(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim().split(['-', '+', ' ']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// Refuse a mode the agent does not offer before it can start or change a session.
pub(crate) fn check_mode(agent: SessionAgent, mode: PermissionMode) -> Result<(), String> {
    let harness = harness(agent);
    if harness.offered_modes().contains(&mode) {
        Ok(())
    } else {
        Err(format!("{} does not offer this permission mode.", harness.label()))
    }
}

/// The agent as people name it.
pub fn label(agent: SessionAgent) -> &'static str {
    harness(agent).label()
}

/// Whether the workbench answers an ask about `tool` without the person. It answers only Nexus's own agent's
/// permission asks, in Auto or Full access ([`PermissionMode::allows_unasked`]), as its harness says
/// ([`crate::harness::Harness::answers_asks`]).
///
/// A coding agent ([`SessionAgent::coding`]) maps each mode to its own setting. Claude Code takes `--permission-mode`.
/// Codex takes `approvalPolicy` and `approvalsReviewer`. Antigravity takes `toolPermission`. That setting decides what
/// runs without asking. So an ask that still comes is for the person.
///
/// A question ([`QUESTION_TOOL`]) or a plan ([`PLAN_TOOL`]) is the person's for every agent.
pub fn answers_ask(agent: SessionAgent, mode: PermissionMode, tool: &str) -> bool {
    harness(agent).answers_asks() && mode.allows_unasked() && tool != QUESTION_TOOL && tool != PLAN_TOOL
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The workbench answers no coding agent's ask, in any mode. It answers Nexus's own agent's permission asks in
    /// Auto and Full access. It never answers a question or a plan.
    #[test]
    fn workbench_answers_only_nexus_agents_permission_asks_in_auto_and_full_access() {
        let modes = [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
            PermissionMode::Auto,
            PermissionMode::BypassPermissions,
        ];
        for agent in SessionAgent::ALL {
            for mode in modes {
                for tool in ["Bash", "Edit", "mcp__nexus_app__terminal_run", QUESTION_TOOL, PLAN_TOOL] {
                    let permission = tool != QUESTION_TOOL && tool != PLAN_TOOL;
                    let expected = agent == SessionAgent::Nexus
                        && matches!(mode, PermissionMode::Auto | PermissionMode::BypassPermissions)
                        && permission;
                    assert_eq!(answers_ask(agent, mode, tool), expected, "{agent:?} {mode:?} {tool}");
                }
            }
        }
    }

    /// Antigravity's Sandbox switch and its `auto` answer change only agy's arguments, in every mode.
    #[test]
    fn sandbox_switch_and_auto_change_only_agys_arguments() {
        for mode in [
            PermissionMode::Default,
            PermissionMode::AcceptEdits,
            PermissionMode::Plan,
            PermissionMode::Auto,
            PermissionMode::BypassPermissions,
        ] {
            let start = Start {
                mode,
                model: Some("m"),
                effort: Some(Effort::High),
                fast: false,
                native_id: "s1",
                resume: true,
                fork: false,
                cwd: "/work",
                rewind: None,
                rewind_required: false,
                instructions: None,
                prompt: None,
                name: None,
            };
            for sandbox in [true, false] {
                for auto in [None, Some(true), Some(false)] {
                    for agent in [SessionAgent::Claude, SessionAgent::Codex, SessionAgent::Nexus] {
                        assert_eq!(
                            args(agent, &start, sandbox, auto),
                            args(agent, &start, false, None),
                            "{agent:?} {mode:?}"
                        );
                    }
                    assert_eq!(
                        args(SessionAgent::Antigravity, &start, sandbox, auto),
                        crate::harness::antigravity::args(mode, sandbox, auto)
                    );
                }
            }
        }
    }

    /// A session saved in fast mode starts in it: Claude Code's SDK opt-in, on its command line.
    #[test]
    fn a_fast_session_starts_claude_code_in_fast_mode() {
        let mut start = Start {
            mode: PermissionMode::Default,
            model: None,
            effort: None,
            fast: false,
            native_id: "s1",
            resume: false,
            fork: false,
            cwd: "/work",
            rewind: None,
            rewind_required: false,
            instructions: None,
            prompt: None,
            name: None,
        };
        assert!(!args(SessionAgent::Claude, &start, false, None).contains(&"--settings".to_string()));
        start.fast = true;
        let fast = args(SessionAgent::Claude, &start, false, None);
        assert!(
            fast.windows(2)
                .any(|w| w == ["--settings", crate::claude::FAST_SETTINGS])
        );
    }

    /// A long command in the workbench is watched, not polled or checked with a wake-up: the hook comes first, and a
    /// wake-up is the fallback for what Nexus cannot watch. The sentence names `watch` only as `terminal_run` offers it,
    /// so an agent whose `terminal_run` has none (`terminal::definitions`) is not told to use it.
    #[test]
    fn sessions_are_told_to_watch_a_long_command_and_keep_wake_ups_for_the_rest() {
        let text = instructions();
        let watch = TERMINAL_GUIDANCE
            .lines()
            .find(|line| line.starts_with("When terminal_run offers watch,"))
            .expect("the watch guidance");
        assert!(text.contains(watch));
        assert!(watch.contains("watch set to true"));
        assert!(watch.contains("do not schedule a wake-up to check on it"));
        assert!(watch.contains("only for what Nexus cannot watch"));
        // An agent with no shared terminal is not told about it.
        assert!(!session_tool_instructions().contains("terminal_run"));
    }

    #[test]
    fn every_session_is_told_about_pages() {
        let text = instructions();
        assert!(text.starts_with(QUESTION_GUIDANCE));
        assert!(text.ends_with(PAGE_INSTRUCTIONS));
        assert!(text.contains(WEB_OUTPUT_GUIDANCE));
        assert!(text.contains(MEDIA_GUIDANCE));
        assert!(text.contains(FILE_LINK_GUIDANCE));
        assert!(session_tool_instructions().contains(FILE_LINK_GUIDANCE));
        assert!(text.contains(TERMINAL_GUIDANCE));
        assert!(text.contains(crate::browser_routing::GUIDANCE));
        assert!(text.contains(nexus_client::commit_trailer::INSTRUCTIONS));
        assert!(text.contains("`createPage`") && text.contains("`writePageVersion`"));
        // A working CLI may publish to a different database. It must never be the desktop's publishing route.
        assert!(!text.contains("nexus pages publish") && !text.contains("nexus pages update"));
        // Both drivers send it: Claude Code as its one system-prompt addition, Codex as the thread's instructions.
        let args = crate::claude::args(PermissionMode::Default, None, None, "s1", false, None, None, None);
        let at = args.iter().position(|arg| arg == "--append-system-prompt");
        assert_eq!(at.and_then(|at| args.get(at + 1)), Some(&text));
        assert_eq!(args.iter().filter(|arg| *arg == "--append-system-prompt").count(), 1);
        // From a file, Claude Code reads the same text.
        let dir = std::env::temp_dir().join(format!("wb-instructions-{}", uuid::Uuid::new_v4().simple()));
        let file = instructions_file(&dir).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(std::fs::read_to_string(&file).ok(), Some(text));
        let args = crate::claude::args(
            PermissionMode::Default,
            None,
            None,
            "s1",
            false,
            None,
            Some(&file),
            None,
        );
        let at = args.iter().position(|arg| arg == "--append-system-prompt-file");
        assert_eq!(
            at.and_then(|at| args.get(at + 1)),
            Some(&file.to_string_lossy().into_owned())
        );
        assert!(!args.iter().any(|arg| arg == "--append-system-prompt"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Every agent is told how to reach another desktop session by its name, through the CLI.
    #[test]
    fn every_agent_is_told_how_to_reach_another_session() {
        let text = instructions();
        for command in ["nexus sessions list", "nexus sessions send"] {
            assert!(text.contains(command), "the instructions lack `{command}`");
        }
    }

    #[test]
    fn instructions_file_is_named_by_its_content_and_rewritten_when_wrong() {
        let dir = std::env::temp_dir().join(format!("wb-instructions-{}", uuid::Uuid::new_v4().simple()));
        let first = instructions_file(&dir).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(instructions_file(&dir).ok().as_ref(), Some(&first));
        // A file cut short (or changed) under the same name is put right, never passed on.
        std::fs::write(&first, "truncated").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(instructions_file(&dir).ok().as_ref(), Some(&first));
        assert_eq!(std::fs::read_to_string(&first).ok(), Some(instructions()));
        // Nothing staged is left behind.
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{e}"))
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Windows can't start a batch file (`claude.cmd`, `codex.cmd`: npm installs) with a line break in an argument
    /// (CVE-2024-24576), so no argument the workbench gives an agent may hold one: every agent, fresh or resumed, a fork,
    /// a rewind, with the app's tools added.
    #[test]
    fn no_launch_argument_has_a_line_break() {
        let dir = std::env::temp_dir().join(format!("wb-instructions-{}", uuid::Uuid::new_v4().simple()));
        let file = instructions_file(&dir).unwrap_or_else(|e| panic!("{e}"));
        for agent in SessionAgent::ALL {
            for (resume, fork, rewind) in [(false, false, None), (true, false, Some("m1")), (true, true, None)] {
                let start = Start {
                    mode: PermissionMode::AcceptEdits,
                    model: Some("claude-sonnet-4-5"),
                    effort: Some(Effort::High),
                    fast: false,
                    native_id: "11111111-2222-3333-4444-555555555555",
                    resume,
                    fork,
                    cwd: "/work/repo",
                    rewind,
                    rewind_required: false,
                    instructions: Some(&file),
                    prompt: None,
                    name: Some("orchestrator"),
                };
                let mut spawn = crate::process::Spawn {
                    program: std::path::PathBuf::from(format!("C:\\npm\\{}.cmd", agent.as_str())),
                    args: args(agent, &start, false, None),
                    env: Default::default(),
                    cwd: std::path::PathBuf::from("/work/repo"),
                };
                let mut grant = crate::orchestra::Grant::new();
                grant.toolsets.terminal = true;
                crate::orchestra::inject(&mut spawn, agent, &grant, 4123);
                for arg in &spawn.args {
                    assert!(!arg.contains(['\r', '\n']), "{agent:?} gets a line break in {arg:?}");
                }
                assert_eq!(crate::process::batch_line_break(&spawn), None);
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn capabilities_by_version() {
        let claude = capabilities(SessionAgent::Claude, Some("2.1.292")).unwrap_or_else(|| panic!("no controls"));
        assert_eq!(claude.version.as_deref(), Some("2.1.292"));
        assert_eq!(claude.stop, ["agent", "shell"]);
        assert_eq!(claude.background, ["shell"]);
        assert_eq!(claude.output, ["shell"]);
        assert!(claude.message.is_empty());
        assert_eq!(claude.interrupt_spares_agents, crate::claude::PER_TASK_STOP);
        assert!(capabilities(SessionAgent::Claude, Some("2.2.0")).is_some());
        let codex = capabilities(SessionAgent::Codex, Some("0.164.0")).unwrap_or_else(|| panic!("no controls"));
        assert_eq!(codex.stop, ["agent"]);
        assert!(codex.background.is_empty() && codex.output.is_empty());
        assert!(!codex.interrupt_spares_agents);
        assert!(capabilities(SessionAgent::Codex, Some("0.165.0-alpha.2")).is_some());
        for (agent, version) in [
            (SessionAgent::Claude, Some("2.1.291")),
            (SessionAgent::Claude, Some("2.1")),
            (SessionAgent::Claude, Some("latest")),
            (SessionAgent::Claude, None),
            (SessionAgent::Codex, Some("0.163.9")),
            (SessionAgent::Codex, Some("")),
        ] {
            assert_eq!(capabilities(agent, version), None, "{agent:?} {version:?}");
        }
    }
}
