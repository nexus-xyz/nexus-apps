//! `nexus` alone, on a terminal: a chat session in the style of Codex's terminal UI. A header, a composer,
//! and answers that stream through Nexus Gateway's chat sessions, the same ones `nexus chat` and Nexus UI use. Each
//! launch starts a new session; when it ends, `nexus chat` continues it.
//!
//! It runs only when stdin and stdout are a terminal, outside CI, and signed in (`cli.rs`); otherwise `nexus` prints
//! its home screen as before, so piped output, `--json` and the TypeScript CLI's twin (`cli-parity.sh`) do not change.
//!
//! - `app.rs`: the state, the keys and the frames, without a terminal or a network; `app/slash.rs`: the commands.
//! - `commands.rs`: the slash commands and the `/` list; `list.rs`: the lists `/model` and `/resume` open.
//! - `markdown.rs`: answers in Markdown, drawn as they stream.
//! - `mentions.rs`: `@` file mentions; `clipboard.rs`: pasted images.
//! - `suggest.rs`: suggested next prompts.
//! - `agent/`: agent mode (⇧tab), the model working in this folder with tools; `agent/stdio.rs` serves it to programs
//!   as `nexus __agent`.
//! - `composer.rs`: the text being written.
//! - `logo.rs`: the Nexus AI mark (F5) above an empty session, turning.
//! - `view.rs`: the lines each part draws.
//! - `turn.rs`: a prompt and its answer, on a thread.
//! - `terminal.rs`: raw mode, the background colour, drawing in place.

mod agent;
mod app;
mod clipboard;
mod commands;
mod composer;
mod list;
mod logo;
mod markdown;
mod mentions;
mod side;
mod suggest;
mod terminal;
mod turn;
mod view;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::Value;

use crate::config::{Config, load_config, save_config};
use crate::error::{CliError, Error, Result};
use crate::style::Style;
use crate::{Context, VERSION};

use agent::tools::Decision;
use app::{App, Command, Key};
use turn::Gateway;
use view::Theme;

/// How often a frame comes while the mark turns or an answer streams (30 a second; Codex draws 20), and how long to
/// wait for a key otherwise.
const ANIMATION: Duration = Duration::from_micros(33_333);
const IDLE: Duration = Duration::from_millis(250);

/// Run the session until the person quits. `identity` is the header's organization and person; `budget` is
/// `$4 of $5 left`, when Gateway said.
pub fn run(
    ctx: &Context,
    config: &Config,
    style: &Style,
    identity: &[Option<String>],
    budget: Option<String>,
) -> Result<i32> {
    let Some(api_key) = config.api_key.clone() else {
        return Ok(0);
    };
    let model = config.chat_model().map(|model| match model {
        "auto" | "default" => format!("nexus/{model}"),
        model => model.to_string(),
    });
    let gateway = Gateway {
        url: config.url.clone(),
        api_key,
        model,
        team: ctx.var("NEXUS_TEAM"),
    };

    // Suggested prompts are on unless `/suggestions off` saved them off or NEXUS_NO_SUGGESTIONS is set.
    let off = |name: &str| ctx.var(name).is_some_and(|value| !value.is_empty() && value != "0");
    let suggestions =
        !off("NEXUS_NO_SUGGESTIONS") && config.extra.get("suggestions").and_then(Value::as_bool) != Some(false);
    session(ctx, style, gateway, identity, budget, suggestions).map_err(|error| {
        Error::Cli(CliError::new(format!(
            "The session could not use this terminal: {error}"
        )))
    })?;
    Ok(0)
}

fn session(
    ctx: &Context,
    style: &Style,
    gateway: Gateway,
    identity: &[Option<String>],
    budget: Option<String>,
    suggestions: bool,
) -> std::io::Result<()> {
    let raw = terminal::Raw::enter()?;
    let mut theme = Theme::new(style.clone(), terminal::background());
    theme.aspect = terminal::aspect();
    let mut header = view::header(&theme, VERSION, identity);
    header.extend(view::tip(
        &theme,
        "Every answer goes through Nexus Gateway. `nexus models pick` sets the model a new session uses.",
    ));
    let model = gateway.model.clone().unwrap_or_else(|| "default model".into());
    let mut app = App::new(theme, model, budget, header);
    app.requested = gateway.model.clone();
    app.suggestions = suggestions;
    let unknown = || "unknown".to_string();
    app.about = vec![
        ("Version".into(), VERSION.to_string()),
        (
            "Organization".into(),
            identity.first().cloned().flatten().unwrap_or_else(unknown),
        ),
        (
            "Signed in as".into(),
            identity.get(1).cloned().flatten().unwrap_or_else(unknown),
        ),
        ("Gateway".into(), gateway.url.clone()),
    ];
    let (events, received) = mpsc::channel();
    let mut screen = terminal::Screen::default();
    let started = Instant::now();
    let now = || started.elapsed().as_millis() as u64;
    // A terminal that does not say its size (0 × 0) is drawn as 80 × 24, as the picker does.
    let size = || match crossterm::terminal::size() {
        Ok((columns, rows)) if columns > 0 && rows > 0 => (usize::from(columns), usize::from(rows)),
        _ => (80, 24),
    };
    let width = || size().0;

    // The agent's conversation, and how to steer its running turn (its answers to approvals, and esc).
    let workspace = std::fs::canonicalize(&ctx.cwd).unwrap_or_else(|_| ctx.cwd.clone());
    // `nexus` in the agent's commands, and in the person's own (`!`), is this CLI.
    if let Some(folder) = ctx.config_path.parent() {
        agent::tools::link_this_cli(folder);
    }
    let history: Arc<Mutex<Vec<Value>>> = Arc::default();
    // The agent conversation's id among the terminal's saved ones (`agent/sessions.rs`); set at its first save.
    let conversation: std::cell::RefCell<Option<String>> = std::cell::RefCell::default();
    // Its subagents' transcripts, saved beside it; and the ones running now, by their turn's id.
    let subagents: std::cell::RefCell<Vec<agent::sessions::Subagent>> = std::cell::RefCell::default();
    let starting: std::cell::RefCell<std::collections::HashMap<String, (String, String, String)>> =
        std::cell::RefCell::default();
    // What the person ran with `!` in chat, waiting for their next chat prompt.
    let carried: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut steering: Option<(mpsc::Sender<Decision>, Arc<AtomicBool>)> = None;
    let handle = |command: Command, steering: &mut Option<(mpsc::Sender<Decision>, Arc<AtomicBool>)>| match command {
        Command::Send {
            turn,
            prompt,
            suggest,
            model,
            session,
            images,
        } => {
            // Mentioned files' text after the prompt.
            let paths = mentions::mentioned(&prompt, &workspace);
            let (mut content, notes) = mentions::attach(&prompt, &paths, &workspace);
            // Commands the person ran since their last chat prompt (`!`), after the files.
            if let Ok(mut carried) = carried.lock() {
                content.extend(carried.drain(..));
            }
            for note in notes {
                let _ = events.send((0, turn::TurnEvent::Note(note)));
            }
            let gateway = Gateway {
                model,
                ..gateway.clone()
            };
            turn::spawn(gateway, session, content, images, suggest, turn, events.clone());
        }
        Command::Agent {
            turn,
            prompt,
            model,
            images,
        } => {
            let (decide, decisions) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            *steering = Some((decide, stop.clone()));
            let model = model.unwrap_or_else(|| "nexus/auto".into());
            let control = agent::Control { decisions, stop };
            let work = agent::Work {
                gateway: gateway.clone(),
                model,
                workspace: workspace.clone(),
                host: agent::prompt::Host::Terminal,
                memory: nexus_agent_memory::Store::from_env(&ctx.env_fn()).ok(),
                history: history.clone(),
                mcp: None,
            };
            agent::spawn(work, prompt, images, turn, events.clone(), control);
        }
        Command::Decide(decision) => {
            if let Some((decide, _)) = steering {
                let _ = decide.send(decision);
            }
        }
        Command::Stop => {
            if let Some((_, stop)) = steering {
                stop.store(true, Ordering::Relaxed);
            }
        }
        Command::Reset => {
            if let Ok(mut history) = history.lock() {
                history.clear();
            }
            // The old conversation stays saved, for /resume.
            conversation.replace(None);
            subagents.borrow_mut().clear();
        }
        Command::Load(turn::Load::Sessions { current, .. }) => {
            let saved = agent::sessions::list(&ctx.config_path, &workspace);
            let folder = workspace.file_name().map_or_else(
                || workspace.display().to_string(),
                |name| name.to_string_lossy().to_string(),
            );
            let saved = turn::saved_items(&saved, conversation.borrow().as_deref(), &folder);
            turn::load(gateway.clone(), turn::Load::Sessions { current, saved }, events.clone());
        }
        Command::Load(load) => turn::load(gateway.clone(), load, events.clone()),
        Command::ShowMain => {
            let messages = history.lock().map(|history| history.clone()).unwrap_or_default();
            let _ = events.send((0, turn::TurnEvent::MainView(messages)));
        }
        Command::Btw {
            question,
            agent: for_agent,
            session,
            model,
            earlier,
        } => {
            let context = if for_agent {
                let told = agent::system_prompt(
                    &workspace,
                    agent::prompt::Host::Terminal,
                    None,
                    nexus_agent_memory::Store::from_env(&ctx.env_fn()).ok().as_ref(),
                );
                let mut messages = vec![serde_json::json!({ "role": "system", "content": told })];
                messages.extend(history.lock().map(|history| history.clone()).unwrap_or_default());
                side::Context::Agent {
                    messages,
                    tools: agent::specs(None),
                }
            } else {
                side::Context::Chat(session)
            };
            let model = model.unwrap_or_else(|| "nexus/auto".into());
            side::ask(gateway.clone(), model, context, earlier, question, events.clone());
        }
        // The conversation already open: nothing to switch.
        Command::OpenAgent(id) if conversation.borrow().as_deref() == Some(id.as_str()) => {}
        Command::OpenAgent(id) => {
            let opened = agent::sessions::load(&ctx.config_path, &id).map(|messages| {
                if let Ok(mut history) = history.lock() {
                    *history = messages.clone();
                }
                conversation.replace(Some(id.clone()));
                let agents = agent::sessions::load_agents(&ctx.config_path, &id);
                subagents.replace(agents.clone());
                (agent::sessions::title(&messages), messages, agents)
            });
            let _ = events.send((0, turn::TurnEvent::AgentOpened(opened)));
        }
        Command::Diff => {
            let (workspace, events) = (workspace.clone(), events.clone());
            std::thread::spawn(move || {
                let _ = events.send((0, turn::TurnEvent::Diff(git_diff(&workspace))));
            });
        }
        Command::Shell { command, for_agent } => {
            let (workspace, events) = (workspace.clone(), events.clone());
            let (history, carried) = (history.clone(), carried.clone());
            std::thread::spawn(move || {
                let action = match agent::tools::run_command(&workspace, &command, agent::tools::COMMAND_TIMEOUT) {
                    Ok((exit, output)) => agent::tools::Action::Ran {
                        command: command.clone(),
                        exit,
                        output,
                    },
                    Err(error) => agent::tools::Action::Failed {
                        what: format!("run {command}"),
                        error,
                    },
                };
                // What the person ran joins the conversation: the agent's now, or the next chat prompt.
                if let agent::tools::Action::Ran { exit, output, .. } = &action {
                    let block = mentions::terminal_block(&command, *exit, output);
                    if for_agent {
                        if let Ok(mut history) = history.lock() {
                            let content = format!("I ran this in my terminal:{block}");
                            history.push(serde_json::json!({ "role": "user", "content": content }));
                        }
                    } else if let Ok(mut carried) = carried.lock() {
                        carried.push(block);
                    }
                }
                let _ = events.send((0, turn::TurnEvent::Shell(action)));
            });
        }
        Command::Compact { model } => {
            let work = agent::Work {
                gateway: gateway.clone(),
                model: model.unwrap_or_else(|| "nexus/auto".into()),
                workspace: workspace.clone(),
                host: agent::prompt::Host::Terminal,
                memory: nexus_agent_memory::Store::from_env(&ctx.env_fn()).ok(),
                history: history.clone(),
                mcp: None,
            };
            let events = events.clone();
            std::thread::spawn(move || {
                let _ = events.send((0, turn::TurnEvent::Compacted(agent::compact(&work))));
            });
        }
        Command::ListFiles => {
            let (workspace, events) = (workspace.clone(), events.clone());
            std::thread::spawn(move || {
                let _ = events.send((0, turn::TurnEvent::Files(mentions::list(&workspace))));
            });
        }
        Command::PasteImage => {
            let events = events.clone();
            std::thread::spawn(move || {
                let _ = events.send((0, turn::TurnEvent::Image(clipboard::image())));
            });
        }
        Command::SaveSuggestions(on) => save_suggestions(ctx, on),
    };

    let mut dirty = true;
    // Frames keep to a fixed clock, so the time a frame takes to draw does not slow the animation down.
    let mut next_frame = Instant::now();
    while !app.quit {
        while let Ok((turn, event)) = received.try_recv() {
            if let turn::TurnEvent::Session(id) = &event {
                remember(ctx, id);
            }
            // A subagent's transcript, kept for the session's agents panel and saved with the conversation.
            if let turn::TurnEvent::Task { id, task } = &event {
                match task {
                    agent::Task::Started {
                        description,
                        prompt,
                        call,
                    } => {
                        starting
                            .borrow_mut()
                            .insert(id.clone(), (call.clone(), description.clone(), prompt.clone()));
                    }
                    agent::Task::Finished {
                        result,
                        error,
                        messages,
                    } => {
                        if let Some((call, description, prompt)) = starting.borrow_mut().remove(id) {
                            subagents.borrow_mut().push(agent::sessions::Subagent {
                                call,
                                description,
                                prompt,
                                messages: messages.clone(),
                                result: result.clone(),
                                error: *error,
                            });
                        }
                    }
                    agent::Task::Tool(_) => {}
                }
            }
            // An agent turn ended: its conversation is saved, so /resume can come back to it.
            if let turn::TurnEvent::Finished(result) = &event {
                let model = result.as_ref().ok().and_then(|summary| summary.model.clone());
                let id = conversation
                    .borrow_mut()
                    .get_or_insert_with(agent::sessions::new_id)
                    .clone();
                let messages = history.lock().map(|history| history.clone()).unwrap_or_default();
                let saved = agent::sessions::save(&ctx.config_path, &id, &workspace, &messages, model.as_deref())
                    .and_then(|()| agent::sessions::save_agents(&ctx.config_path, &id, &subagents.borrow()));
                if let Err(error) = saved {
                    let note = format!("The agent's conversation could not be saved: {error}");
                    let _ = events.send((0, turn::TurnEvent::Note(note)));
                }
            }
            if let Some(command) = app.event(turn, event, width(), now()) {
                handle(command, &mut steering);
            }
            dirty = true;
        }
        let moving = app.running() || app.animating(now());
        let due = moving && Instant::now() >= next_frame;
        if dirty || due {
            let (width, height) = size();
            screen.draw(&app.frame(width, height, now()), width, height)?;
            dirty = false;
        }
        if due {
            next_frame = (next_frame + ANIMATION).max(Instant::now());
        }
        let wait = if moving {
            next_frame.saturating_duration_since(Instant::now())
        } else {
            IDLE
        };
        if !crossterm::event::poll(wait)? {
            continue;
        }
        let key = match crossterm::event::read()? {
            Event::Key(event) => key_of(&event),
            // A pasted path to an image is that image.
            Event::Paste(text) => match clipboard::image_file(&text) {
                Some(image) => {
                    app.event(0, turn::TurnEvent::Image(image), width(), now());
                    dirty = true;
                    None
                }
                None => Some(Key::Paste(text)),
            },
            Event::Resize(..) => {
                // A font change resizes the window: the cells may have a new shape.
                app.theme.aspect = terminal::aspect();
                screen.resized();
                dirty = true;
                None
            }
            _ => None,
        };
        if let Some(key) = key {
            dirty = true;
            if let Some(command) = app.key(key, width(), now()) {
                handle(command, &mut steering);
            }
        }
    }
    screen.finish(&app.goodbye())?;
    drop(raw);
    Ok(())
}

/// `/diff`: `git status --short` and `git diff` in the folder, or why not (not a repository, no git).
fn git_diff(workspace: &std::path::Path) -> std::result::Result<(Vec<String>, Vec<String>), String> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .map_err(|error| format!("git could not run: {error}"))
    };
    let status = git(&["status", "--short"])?;
    if !status.status.success() {
        return Err(String::from_utf8_lossy(&status.stderr).trim().to_string());
    }
    let diff = git(&["diff", "--no-color", "--no-ext-diff"])?;
    let lines = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(String::from)
            .collect::<Vec<_>>()
    };
    Ok((lines(&status.stdout), lines(&diff.stdout)))
}

/// `nexus __agent [--session <id>]`: the agent for programs, JSON lines on stdin and stdout (`agent/stdio.rs`). It signs
/// in with the CLI's config, or with `NEXUS_API_KEY` (and `NEXUS_URL`) when the program passes its own. The program
/// says what it is in `NEXUS_AGENT_HOST`. `nexus __agent --handshake` only says which protocol it speaks.
pub fn serve_agent(args: &[String], ctx: &Context) -> i32 {
    // First, with no config, sign-in or request: a program checks with it that this `nexus` can be its agent.
    if args.iter().any(|arg| arg == "--handshake") {
        ctx.io.out(&agent::stdio::handshake().to_string());
        return 0;
    }
    let fail = |message: &str| {
        println!("{}", serde_json::json!({ "type": "error", "message": message }));
        2
    };
    let mut config = load_config(&ctx.config_path, &ctx.env_fn());
    if let Some(key) = ctx.var("NEXUS_API_KEY").filter(|key| !key.is_empty()) {
        config.api_key = Some(key);
        if let Some(url) = ctx.var("NEXUS_URL").filter(|url| !url.is_empty()) {
            config.url = url;
        }
    }
    let Some(api_key) = config.api_key.clone() else {
        return fail("Not signed in: run `nexus auth login`, or set NEXUS_API_KEY.");
    };
    let value = |flag: &str| args.windows(2).find(|pair| pair[0] == flag).map(|pair| pair[1].clone());
    let rewind = match value("--rewind").map(|n| n.parse::<usize>()) {
        Some(Ok(n)) => Some(n),
        Some(Err(_)) => return fail("--rewind takes a number of messages"),
        None => None,
    };
    let fork = args.iter().any(|arg| arg == "--fork");
    let session = match value("--session") {
        Some(id) => match agent::stdio::open(&ctx.config_path, &id, fork, rewind) {
            Ok(session) => Some(session),
            Err(error) => return fail(&error),
        },
        None if fork || rewind.is_some() => return fail("--fork and --rewind need --session"),
        None => None,
    };
    let url = if config.url.is_empty() {
        crate::config::DEFAULT_URL.to_string()
    } else {
        config.url.clone()
    };
    let default_model = match config.chat_model() {
        Some("auto" | "default") | None => "nexus/auto".to_string(),
        Some(model) => model.to_string(),
    };
    let gateway = Gateway {
        url,
        api_key,
        model: None,
        team: ctx.var("NEXUS_TEAM"),
    };
    let workspace = std::fs::canonicalize(&ctx.cwd).unwrap_or_else(|_| ctx.cwd.clone());
    // `nexus` in the agent's commands is this CLI, wherever the program that started it found it.
    if let Some(folder) = ctx.config_path.parent() {
        agent::tools::link_this_cli(folder);
    }
    // `--mcp-config=<json>` and `--allowedTools=<a,b>`, as Claude Code takes them; or with the value after a space.
    let option = |flag: &str| {
        args.iter()
            .find_map(|arg| arg.strip_prefix(&format!("{flag}=")).map(String::from))
            .or_else(|| value(flag))
    };
    let servers = match option("--mcp-config").map(|config| serde_json::from_str::<serde_json::Value>(&config)) {
        Some(Ok(config)) => {
            // Claude Code's variable for one call's limit, in milliseconds; ten minutes when unset.
            let timeout = ctx
                .var("MCP_TOOL_TIMEOUT")
                .and_then(|ms| ms.parse::<u64>().ok())
                .map_or(std::time::Duration::from_secs(600), std::time::Duration::from_millis);
            let var = |name: &str| ctx.var(name);
            let (mcp, notes) = agent::mcp::Mcp::start(&config, &var, timeout);
            let allowed = option("--allowedTools")
                .map(|list| list.split(',').map(|name| name.trim().to_string()).collect())
                .unwrap_or_default();
            Some(agent::stdio::Servers {
                mcp: Arc::new(mcp),
                allowed,
                notes,
            })
        }
        Some(Err(_)) => return fail("--mcp-config takes JSON: {\"mcpServers\": {…}}"),
        None => None,
    };
    // The program says what it is, so the agent is told where it runs (`agent/prompt.rs`).
    let given = servers
        .as_ref()
        .map(|servers| servers.mcp.servers())
        .unwrap_or_default();
    let host = agent::prompt::Host::of_program(ctx.var(agent::prompt::HOST_VARIABLE).as_deref(), &given);
    let input = std::io::BufReader::new(std::io::stdin());
    let memory = (host == agent::prompt::Host::Desktop
        && ctx.var(nexus_agent_memory::tools::AGENT_ENV).as_deref() == Some("nexus"))
    .then(|| nexus_agent_memory::Store::from_env(&ctx.env_fn()).ok())
    .flatten();
    match agent::stdio::serve(
        gateway,
        default_model,
        workspace,
        host,
        memory,
        session,
        servers,
        input,
        std::io::stdout(),
    ) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// `/suggestions on|off`, kept for the next sessions.
fn save_suggestions(ctx: &Context, on: bool) {
    let mut config = load_config(&ctx.config_path, &ctx.env_fn());
    config.extra.insert("suggestions".into(), Value::Bool(on));
    let _ = save_config(&ctx.config_path, &config);
}

/// The session the first prompt made becomes the one `nexus chat` continues.
fn remember(ctx: &Context, session: &str) {
    let mut config = load_config(&ctx.config_path, &ctx.env_fn());
    config.chat_session = Some(session.to_string());
    let _ = save_config(&ctx.config_path, &config);
}

/// A key crossterm read, as the session sees it. ⇧⏎ and ⌥⏎ arrive as ⏎ with a modifier only where the terminal tells
/// them apart; ⌃j always makes a new line.
fn key_of(event: &KeyEvent) -> Option<Key> {
    if event.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
    let shifted = event.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
    Some(match event.code {
        KeyCode::Enter if shifted => Key::NewLine,
        KeyCode::Enter => Key::Enter,
        KeyCode::Char('j') if ctrl => Key::NewLine,
        KeyCode::Char('c') if ctrl => Key::CtrlC,
        KeyCode::Char('d') if ctrl => Key::CtrlD,
        KeyCode::Char('u') if ctrl => Key::CtrlU,
        KeyCode::Char('f') if ctrl => Key::CtrlF,
        KeyCode::Char('k') if ctrl => Key::CtrlK,
        KeyCode::Char('w') if ctrl => Key::CtrlW,
        KeyCode::Char('v') if ctrl => Key::PasteImage,
        KeyCode::Char('a') if ctrl => Key::Home,
        KeyCode::Char('e') if ctrl => Key::End,
        KeyCode::Char('b') if ctrl => Key::Left,
        KeyCode::Char('f') if ctrl => Key::Right,
        KeyCode::Char('p') if ctrl => Key::Up,
        KeyCode::Char('n') if ctrl => Key::Down,
        KeyCode::Char(_) if ctrl => return None,
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        _ => return None,
    })
}

/// Whether `nexus` alone opens the session: an interactive terminal, outside CI, signed in, and not turned off with
/// `NEXUS_NO_TUI=1`.
pub fn wanted(ctx: &Context, config: &Config, me: Option<&Value>) -> bool {
    let set = |name: &str| ctx.var(name).is_some_and(|value| !value.is_empty() && value != "0");
    ctx.picker && !set("CI") && !set("NEXUS_NO_TUI") && config.api_key.is_some() && me.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Option<Key> {
        key_of(&KeyEvent::new(code, modifiers))
    }

    #[test]
    fn maps_new_line_keys() {
        assert_eq!(key(KeyCode::Enter, KeyModifiers::NONE), Some(Key::Enter));
        assert_eq!(key(KeyCode::Enter, KeyModifiers::SHIFT), Some(Key::NewLine));
        assert_eq!(key(KeyCode::Char('j'), KeyModifiers::CONTROL), Some(Key::NewLine));
        assert_eq!(key(KeyCode::Char('x'), KeyModifiers::CONTROL), None);
        assert_eq!(key(KeyCode::Char('X'), KeyModifiers::SHIFT), Some(Key::Char('X')));
    }
}
