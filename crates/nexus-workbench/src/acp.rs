//! A session protocol for the Agent Client Protocol (JSON-RPC, one message per line).
//!
//! The driver initializes, then creates or loads a session. It waits for settings before sending the first prompt.
//! It streams updates, returns the person's approval choice, and queues prompts while a turn runs.
//! Agents read files and run commands in their process. The client offers no filesystem or terminal services.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::driver::{Out, Protocol, Say, Start, cap};
use crate::event::{Effort, Image, PermissionMode, PermissionSuggestion, Usage, WbEvent};
use crate::mcp_servers::SessionMcp;

/// Compatibility export for the original launch-argument helper.
pub use crate::harness::legacy_acp_args as args;

/// Protocol details supplied by an ACP harness. The driver takes ownership of its profile.
pub struct Profile {
    pub name: Cow<'static, str>,
    pub sign_in: Option<Cow<'static, str>>,
    pub sign_in_help: Cow<'static, str>,
    pub meta: fn() -> Value,
    pub modes: Option<ModeMapping>,
    pub auto: Option<AutoMode>,
    /// Whether mode evidence updates the workbench's launch settings.
    pub reports_auto_offered: bool,
    pub levels: Option<ThinkingLevels>,
    pub tool_presentation: Option<ToolPresentation>,
    /// A quiet period ends a turn the agent started without a prompt request.
    pub own_turn_quiet: Duration,
}

#[derive(Clone, Copy)]
pub struct ModeMapping {
    pub native: fn(PermissionMode) -> Option<&'static str>,
    pub observed: fn(Option<&str>) -> Option<PermissionMode>,
    /// Require a known opening mode and a successful switch before releasing the first prompt.
    pub confirm_opening: bool,
    /// Selects a native mode using any available Auto evidence.
    /// Returning the original mode after a refusal means there is no fallback.
    pub requested: fn(PermissionMode, Option<bool>) -> Option<&'static str>,
}

pub struct AutoMode {
    pub id: &'static str,
    pub unknown_mode_error: i64,
    /// Shown once a mode is set whose native id differs from the one asked for (a fallback), and for the modes
    /// `notice_for` names.
    pub policy_notice: Cow<'static, str>,
    /// Modes that also show `policy_notice` once set, given what the agent said of its Auto mode: a mode that never
    /// falls back, but whose unasked actions the account's policy still decides.
    pub notice_for: fn(PermissionMode, Option<bool>) -> bool,
}

type EditTexts<'a> = (Option<&'a str>, Option<&'a str>);

/// Vendor-specific tool payloads that need normalizing before they reach the session log.
pub struct ToolPresentation {
    pub edit_text: fn(Option<&Value>) -> EditTexts<'_>,
    pub output: fn(&Value) -> Option<Vec<String>>,
}

pub struct ThinkingLevels {
    pub option: &'static str,
    pub value: fn(Effort) -> Option<&'static str>,
    pub unsupported: Cow<'static, str>,
}

/// agy names a tool of a session's MCP server `mcp_<server>_<tool>` (agy 1.5.15: `mcp_nexus_createPage`); the app reads
/// Claude Code's `mcp__<server>__<tool>` (page links, browser routing, the tool rows). The longest launched server
/// name that fits decides where the server ends, so `nexus_browser` wins over `nexus`. None for any other tool,
/// including those of servers the agent configured itself.
fn mcp_tool_name(name: &str, servers: &[String]) -> Option<String> {
    let rest = name.strip_prefix("mcp_")?;
    servers
        .iter()
        .filter_map(|server| Some((server, rest.strip_prefix(server.as_str())?.strip_prefix('_')?)))
        .filter(|(_, tool)| !tool.is_empty())
        .max_by_key(|(server, _)| server.len())
        .map(|(server, tool)| format!("mcp__{server}__{tool}"))
}

/// JSON-RPC's code for a method the client does not offer.
const NO_SUCH_METHOD: i64 = -32601;

/// ACP's code for a request that needs the person signed in (`auth_required`).
const AUTH_REQUIRED: i64 = -32000;

/// What one of the client's requests was, for its answer.
#[derive(Debug, Clone, PartialEq)]
enum Asked {
    Initialize,
    Authenticate,
    /// `session/new` or `session/load`.
    Open,
    Prompt,
    /// A request from the workbench: its answer is an [`Out::Ack`].
    Ack(String),
    /// A setting applied as the conversation opens: only a refusal is said.
    Setting(&'static str),
    /// A mode request, with the native `acp_mode` and the sequence number for its answer.
    /// `request` is None for the opening mode.
    Mode {
        request: Option<String>,
        mode: PermissionMode,
        acp_mode: &'static str,
        sent: u64,
    },
    /// A model asked for, which counts once the agent takes it: the workbench's request, or none as the session opens.
    Model {
        request: Option<String>,
        model: String,
    },
}

/// An approval the agent waits on.
struct Asking {
    /// The JSON-RPC id of the agent's request, as the agent sent it.
    rpc: Value,
    options: Vec<Value>,
    /// The tool call it asks to run.
    call: Option<String>,
}

/// A tool call as its updates describe it so far.
#[derive(Default)]
struct Call {
    kind: String,
    title: String,
    /// The agent's own name for the tool (`write_file`).
    name: String,
    path: Option<String>,
    raw_input: Value,
    content: Vec<Value>,
    raw_output: Value,
    status: String,
    /// Its `Tool` event went out.
    shown: bool,
    /// Its `ToolResult` went out.
    ended: bool,
}

pub struct Driver {
    profile: Profile,
    cwd: String,
    native_id: String,
    resume: bool,
    /// Why this run cannot go on (a fork), said as it opens.
    refused: Option<String>,
    /// A message edit needs a cut the agent cannot make: the first prompt is refused.
    rewind_required: bool,
    /// A revert took the session back: the agent keeps the reverted turns, which is said once the conversation opens.
    reverted: bool,
    mode: PermissionMode,
    /// Whether the agent offers its `auto` mode: false once the session's answer lists modes without it, or the agent refused it.
    /// None while not known: a session's answer that lists no modes.
    auto_offered: Option<bool>,
    /// The `session/set_mode` requests sent so far. the agent answers out of order, but runs the mode of the last one it
    /// took, so an answer to an older one does not change the mode in force.
    modes_sent: u64,
    /// The session's mode the agent runs, as far as its answers say, with the mode request that set it: the mode it opened
    /// in (0), then the latest mode request it took.
    in_force: (u64, PermissionMode),
    /// The model the agent runs, as it said or took it.
    model: Option<String>,
    /// The model to ask for as the conversation opens.
    wanted_model: Option<String>,
    required_launch_model: bool,
    effort: Option<Effort>,
    /// What "Default" is, for a change back to it: the agent's first model (the picker's Default row names it), and the
    /// thinking level a new conversation opened at. None until the agent said.
    default_model: Option<String>,
    default_level: Option<String>,
    version: Option<String>,
    /// The conversation, once the agent opened it.
    session: Option<String>,
    /// A load replays the conversation before its answer.
    loading: bool,
    /// The client asked the agent to sign in, after it said it needs a sign-in.
    signed_in: bool,
    /// Settings sent as the conversation opened that the agent has not answered: the first turn waits for them.
    settling: usize,
    first_prompt_sent: bool,
    opening_mode_failed: bool,
    reported_opening_mode: Option<PermissionMode>,
    /// Thinking levels that wait for a model change the agent has not answered ([`Driver::set_level`]).
    held_levels: Vec<(Asked, String)>,
    /// The agent's echo of the person's message under way, until its next update.
    echo: Option<String>,
    /// The last prompt while its read receipt is pending. The first action can serve as the receipt.
    prompted: Option<String>,
    /// The agent runs a turn it started itself, as it does when one of its background tasks ends. No `session/prompt`
    /// stands for it, so no answer ends it: a silence does ([`Protocol::quiet`]), or Stop.
    own_turn: bool,
    /// Tool calls the agent started and has not ended.
    open_calls: HashSet<String>,
    next_rpc: u64,
    asked: HashMap<u64, Asked>,
    asking: HashMap<String, Asking>,
    calls: HashMap<String, Call>,
    /// The text block streaming now, and the thought, with their ids.
    text: Option<(String, String)>,
    thought: Option<(String, String)>,
    next_block: u64,
    /// A turn is under way, or waits for the conversation to open (`deferred`).
    running: bool,
    deferred: Option<(String, Vec<Image>)>,
    waiting: Vec<(String, Vec<Image>)>,
    started: Option<Instant>,
    context_window: Option<u64>,
    /// The MCP servers the host launched with ([`Protocol::session_mcp`]), sent as the conversation opens.
    mcp: SessionMcp,
}

impl Driver {
    pub fn new(start: &Start, profile: Profile) -> Driver {
        let name = &profile.name;
        let refused = (start.resume && start.fork)
            .then(|| format!("{name} can't fork a conversation. Start a new {name} session instead."));
        Driver {
            profile,
            cwd: start.cwd.to_string(),
            native_id: start.native_id.to_string(),
            resume: start.resume,
            refused,
            rewind_required: start.resume && start.rewind_required,
            reverted: start.resume && start.rewind.is_some() && !start.rewind_required,
            mode: start.mode,
            auto_offered: None,
            modes_sent: 0,
            in_force: (0, PermissionMode::Default),
            model: None,
            wanted_model: start.model.map(String::from),
            required_launch_model: false,
            effort: start.effort,
            default_model: None,
            default_level: None,
            version: None,
            session: None,
            loading: false,
            signed_in: false,
            settling: 0,
            first_prompt_sent: false,
            opening_mode_failed: false,
            reported_opening_mode: None,
            held_levels: Vec::new(),
            echo: None,
            prompted: None,
            own_turn: false,
            open_calls: HashSet::new(),
            next_rpc: 0,
            asked: HashMap::new(),
            asking: HashMap::new(),
            calls: HashMap::new(),
            text: None,
            thought: None,
            next_block: 0,
            running: false,
            deferred: None,
            waiting: Vec::new(),
            started: None,
            context_window: None,
            mcp: SessionMcp::default(),
        }
    }

    fn request(&mut self, asked: Asked, method: &str, params: Value) -> Out {
        self.next_rpc += 1;
        self.asked.insert(self.next_rpc, asked);
        Out::Reply(json!({ "jsonrpc": "2.0", "id": self.next_rpc, "method": method, "params": params }).to_string())
    }

    fn block(&mut self, kind: &str) -> String {
        self.next_block += 1;
        format!("acp-{kind}-{}", self.next_block)
    }

    fn answer(rpc: &Value, result: Value) -> Out {
        Out::Reply(json!({ "jsonrpc": "2.0", "id": rpc, "result": result }).to_string())
    }

    /// The run cannot go on: say why and close the agent's input.
    fn fail(text: String) -> Vec<Out> {
        vec![Out::Event(WbEvent::Error { text, detail: None }), Out::Close]
    }

    /// A load takes the servers again: an ACP agent starts a conversation's servers each time it opens it.
    fn open_session(&mut self) -> Out {
        let meta = (self.profile.meta)();
        let servers = &self.mcp.servers;
        if self.resume {
            self.loading = true;
            let params = json!({ "sessionId": self.native_id, "cwd": self.cwd, "mcpServers": servers, "_meta": meta });
            self.request(Asked::Open, "session/load", params)
        } else {
            let params = json!({ "cwd": self.cwd, "mcpServers": servers, "_meta": meta });
            self.request(Asked::Open, "session/new", params)
        }
    }

    /// Said once, as a new conversation opens: the remote servers in Settings that this agent cannot take.
    fn skipped_notice(&self) -> Option<Out> {
        let skipped = &self.mcp.skipped;
        if self.resume || skipped.is_empty() {
            return None;
        }
        let names = skipped.join(", ");
        let them = if skipped.len() == 1 {
            "this server"
        } else {
            "these servers"
        };
        Some(Out::Event(WbEvent::Notice {
            text: format!(
                "{} takes only local MCP servers, so it can't use {them} from Settings: {names}.",
                self.profile.name
            ),
        }))
    }

    /// The modes a session's answer offers: ACP's `modes.availableModes`, and the agent's `mode` config option. None when it
    /// lists neither.
    fn offered_modes(result: &Value) -> Option<Vec<String>> {
        let listed = |list: Option<&Value>, key: &str| -> Option<Vec<String>> {
            let list = list?.as_array()?;
            Some(
                list.iter()
                    .filter_map(|mode| mode.get(key).and_then(Value::as_str).map(String::from))
                    .collect(),
            )
        };
        let modes = listed(result.pointer("/modes/availableModes"), "id");
        let option = result
            .get("configOptions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|option| option.get("id").and_then(Value::as_str) == Some("mode"));
        let options = listed(option.and_then(|option| option.get("options")), "value");
        match (modes, options) {
            (None, None) => None,
            (modes, options) => Some(modes.into_iter().chain(options).flatten().collect()),
        }
    }

    fn thought_level(&self, effort: Effort) -> Option<&'static str> {
        (self.profile.levels.as_ref()?.value)(effort)
    }

    fn unsupported_level(&self) -> &str {
        self.profile
            .levels
            .as_ref()
            .map_or("This agent takes no thinking level.", |levels| &levels.unsupported)
    }

    /// Sets `mode` in the agent: the workbench's change `request`, or None as the conversation opens.
    fn set_mode(&mut self, mode: PermissionMode, request: Option<String>) -> Option<Out> {
        let session = self.session.clone()?;
        let acp_mode = (self.profile.modes?.requested)(mode, self.auto_offered)?;
        let params = json!({ "sessionId": session, "modeId": acp_mode });
        self.modes_sent += 1;
        self.reported_opening_mode = None;
        let asked = Asked::Mode {
            request,
            mode,
            acp_mode,
            sent: self.modes_sent,
        };
        Some(self.request(asked, "session/set_mode", params))
    }

    fn set_option(&mut self, asked: Asked, option: &str, value: &str) -> Option<Out> {
        let session = self.session.clone()?;
        let params = json!({ "sessionId": session, "configId": option, "value": value });
        Some(self.request(asked, "session/set_config_option", params))
    }

    /// A thinking level waits for the model change, so the new model supplies its valid levels.
    fn set_level(&mut self, asked: Asked, level: &str) -> Vec<Out> {
        if self.changing_model() {
            self.held_levels.push((asked, level.to_string()));
            return Vec::new();
        }
        let Some(levels) = &self.profile.levels else {
            return Vec::new();
        };
        self.set_option(asked, levels.option, level).into_iter().collect()
    }

    fn changing_model(&self) -> bool {
        self.asked.values().any(|asked| matches!(asked, Asked::Model { .. }))
    }

    /// The levels that waited, once no model change is under way.
    fn release_levels(&mut self) -> Vec<Out> {
        if self.changing_model() {
            return Vec::new();
        }
        let Some(levels) = &self.profile.levels else {
            return Vec::new();
        };
        let option = levels.option;
        std::mem::take(&mut self.held_levels)
            .into_iter()
            .filter_map(|(asked, level)| self.set_option(asked, option, &level))
            .collect()
    }

    /// The conversation is open: report it, apply the session's settings, and send the turn that waited for it.
    fn opened(&mut self, result: &Value) -> Vec<Out> {
        self.loading = false;
        let session = result
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .unwrap_or(&self.native_id)
            .to_string();
        self.session = Some(session.clone());
        let option = |id: &str| {
            result
                .get("configOptions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|option| option.get("id").and_then(Value::as_str) == Some(id))
        };
        let current = |id: &str| {
            option(id)
                .and_then(|option| option.get("currentValue"))
                .and_then(Value::as_str)
                .map(String::from)
        };
        self.model = current("model");
        self.default_model = option("model")
            .and_then(|option| option.pointer("/options/0/value"))
            .and_then(Value::as_str)
            .map(String::from);
        // A loaded conversation keeps its own level, which is not the default.
        if !self.resume
            && let Some(levels) = &self.profile.levels
        {
            self.default_level = current(levels.option);
        }
        self.auto_offered = self
            .profile
            .auto
            .as_ref()
            .and_then(|auto| Driver::offered_modes(result).map(|modes| modes.iter().any(|mode| mode == auto.id)));
        // A mode list can omit Auto. Confirm a listed mode only when the agent takes it.
        let offered = self
            .auto_offered
            .filter(|offered| !offered)
            .and_then(|offered| self.auto_evidence(offered));
        let opened_in = result
            .pointer("/modes/currentModeId")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| current("mode"));
        if let Some(modes) = self.profile.modes {
            let observed = (modes.observed)(opened_in.as_deref());
            if modes.confirm_opening {
                if (modes.requested)(self.mode, self.auto_offered).is_none() {
                    return self.refuse_opening_mode("This mode is unavailable.");
                }
                if observed.is_none() {
                    return self.refuse_opening_mode("The agent did not report a supported mode.");
                }
            }
            self.in_force = (0, observed.unwrap_or(PermissionMode::Default));
        }
        let mut out = vec![Out::Init {
            native_id: session,
            model: self.model.clone(),
            version: self.version.clone(),
        }];
        out.extend(offered);
        out.extend(self.skipped_notice());
        let mut settings = Vec::new();
        if let Some(model) = self
            .wanted_model
            .take()
            .filter(|model| self.model.as_ref() != Some(model))
        {
            let asked = Asked::Model {
                request: None,
                model: model.clone(),
            };
            settings.extend(self.set_option(asked, "model", &model));
        }
        if let Some(modes) = self.profile.modes
            && (modes.requested)(self.mode, self.auto_offered) != (modes.native)(self.in_force.1)
        {
            settings.extend(self.set_mode(self.mode, None));
        }
        if let Some(effort) = self.effort {
            match self.thought_level(effort) {
                Some(level) => settings.extend(self.set_level(Asked::Setting("thinking level"), level)),
                None => out.push(Out::Event(WbEvent::Notice {
                    text: format!("{} This session uses its default.", self.unsupported_level()),
                })),
            }
        }
        // A level that waits for the model still holds the first turn.
        self.settling = settings.len() + self.held_levels.len();
        out.extend(settings);
        if self.reverted {
            out.push(Out::Event(WbEvent::Notice {
                text: format!(
                    "{} keeps the reverted turns in its conversation, so it may still refer to them.",
                    self.profile.name
                ),
            }));
        }
        out.extend(self.send_deferred());
        out
    }

    fn refuse_opening_mode(&mut self, reason: &str) -> Vec<Out> {
        self.opening_mode_failed = true;
        self.session = None;
        self.deferred = None;
        self.waiting.clear();
        Driver::fail(format!(
            "{} could not start in the requested mode: {reason}",
            self.profile.name
        ))
    }

    fn confirming_opening_mode(&self) -> bool {
        !self.first_prompt_sent && self.profile.modes.is_some_and(|modes| modes.confirm_opening)
    }

    /// The turn that waited for the conversation and its settings, once both are there.
    fn send_deferred(&mut self) -> Vec<Out> {
        if self.session.is_none() || self.settling > 0 {
            return Vec::new();
        }
        if self.confirming_opening_mode()
            && (self.in_force.1 != self.mode || self.asked.values().any(|asked| matches!(asked, Asked::Mode { .. })))
        {
            return Vec::new();
        }
        match self.deferred.take() {
            Some((text, images)) => self.send_prompt(text, images),
            None => Vec::new(),
        }
    }

    /// One setting sent as the conversation opened has its answer.
    fn settled(&mut self) -> Vec<Out> {
        self.settling = self.settling.saturating_sub(1);
        self.send_deferred()
    }

    fn prompt(&mut self, text: String, images: Vec<Image>) -> Vec<Out> {
        self.running = true;
        self.started = Some(Instant::now());
        self.deferred = Some((text, images));
        self.send_deferred()
    }

    /// The person's message as the agent took it: its echo, once whole (the agent may split it across updates). Without an echo,
    /// the message itself once the agent `acted` after it.
    fn taken(&mut self, acted: bool) -> Vec<Out> {
        let text = match self.echo.take() {
            Some(echo) => {
                self.prompted = None;
                Some(echo)
            }
            None if acted => self.prompted.take(),
            None => None,
        };
        text.map(|text| vec![Out::Taken { texts: vec![text] }])
            .unwrap_or_default()
    }

    /// A visible update outside a prompt starts a turn.
    fn own_output(&mut self) {
        if !self.running && !self.own_turn {
            self.own_turn = true;
            self.started = Some(Instant::now());
        }
    }

    /// Whether the agent's own turn may end: nothing of it is still under way.
    fn own_turn_settled(&self) -> bool {
        self.own_turn && !self.running && self.asking.is_empty() && self.open_calls.is_empty()
    }

    /// The agent's own turn ends: its last text, then its result.
    fn end_own_turn(&mut self, interrupted: bool) -> Vec<Out> {
        self.own_turn = false;
        self.open_calls.clear();
        let mut out = self.end_text();
        out.push(Out::Result {
            total_cost_usd: None,
            duration_ms: self
                .started
                .take()
                .map_or(0, |started| started.elapsed().as_millis() as u64),
            error: None,
            interrupted,
            context_window: self.context_window,
        });
        out
    }

    fn send_prompt(&mut self, text: String, images: Vec<Image>) -> Vec<Out> {
        if std::mem::take(&mut self.rewind_required) {
            self.running = false;
            self.waiting.clear();
            return vec![
                Out::Result {
                    total_cost_usd: None,
                    duration_ms: 0,
                    error: Some(format!(
                        "{} can't rewind a conversation. Send again to continue from the saved transcript.",
                        self.profile.name
                    )),
                    interrupted: false,
                    context_window: self.context_window,
                },
                Out::Close,
            ];
        }
        let Some(session) = self.session.clone() else {
            return Vec::new();
        };
        let mut prompt: Vec<Value> = images
            .into_iter()
            .map(|image| json!({ "type": "image", "mimeType": image.media_type, "data": image.data }))
            .collect();
        prompt.push(json!({ "type": "text", "text": text.clone() }));
        self.prompted = Some(text);
        self.first_prompt_sent = true;
        vec![self.request(
            Asked::Prompt,
            "session/prompt",
            json!({ "sessionId": session, "prompt": prompt }),
        )]
    }

    fn end_text(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        if let Some((id, text)) = self.thought.take().filter(|(_, text)| !text.trim().is_empty()) {
            out.push(Out::Event(WbEvent::Thinking { id, text, parent: None }));
        }
        if let Some((id, text)) = self.text.take().filter(|(_, text)| !text.trim().is_empty()) {
            out.push(Out::Event(WbEvent::Text { id, text, parent: None }));
        }
        out
    }

    /// The turn ended: its tokens, its result, then the next message that waited for it.
    fn end_turn(&mut self, result: Option<&Value>, error: Option<String>) -> Vec<Out> {
        let stop = result
            .and_then(|result| result.get("stopReason"))
            .and_then(Value::as_str);
        // A turn that ran took its message. One refused or cancelled may not have: only an echo says so then.
        let mut out = self.taken(error.is_none() && stop != Some("cancelled"));
        self.prompted = None;
        // The prompt's answer ends the turn the agent may have run before it too: it streamed into this one.
        self.own_turn = false;
        self.open_calls.clear();
        out.extend(self.end_text());
        if let Some(usage) = result.and_then(|result| result.get("usage")) {
            let tokens = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
            out.push(Out::Call {
                model: self.model.clone().unwrap_or_else(|| "unknown".into()),
                usage: Usage {
                    input_tokens: tokens("inputTokens"),
                    output_tokens: tokens("outputTokens"),
                    cache_read_tokens: tokens("cachedReadTokens"),
                    cache_write_tokens: tokens("cachedWriteTokens"),
                    cache_writes: None,
                },
                parent: None,
            });
        }
        let error = error.or_else(|| match stop {
            Some("refusal") => Some(format!("{} refused to go on with this prompt.", self.profile.name)),
            Some("max_tokens") => Some(format!(
                "{} stopped: the reply reached its token limit.",
                self.profile.name
            )),
            Some("max_turn_requests") => Some(format!(
                "{} stopped: the turn reached its limit of model calls.",
                self.profile.name
            )),
            _ => None,
        });
        for (request_id, _) in self.asking.drain() {
            out.push(Out::Cancelled { request_id });
        }
        self.running = false;
        out.push(Out::Result {
            total_cost_usd: None,
            duration_ms: self
                .started
                .take()
                .map_or(0, |started| started.elapsed().as_millis() as u64),
            error,
            interrupted: stop == Some("cancelled"),
            context_window: self.context_window,
        });
        if !self.waiting.is_empty() {
            let (text, images) = self.waiting.remove(0);
            out.extend(self.prompt(text, images));
        }
        out
    }

    /// An answer to one of the client's requests.
    fn auto_evidence(&self, offered: bool) -> Option<Out> {
        self.profile.reports_auto_offered.then_some(Out::AutoOffered(offered))
    }

    fn auto_mode(&self, id: &str) -> Option<&AutoMode> {
        self.profile.auto.as_ref().filter(|auto| auto.id == id)
    }

    fn answered(&mut self, id: u64, message: &Value) -> Vec<Out> {
        let Some(asked) = self.asked.remove(&id) else {
            return Vec::new();
        };
        let error = message.get("error").map(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .map_or_else(|| error.to_string(), String::from)
        });
        let code = message.pointer("/error/code").and_then(Value::as_i64);
        let signed_out = code == Some(AUTH_REQUIRED);
        let result = message.get("result").cloned().unwrap_or(Value::Null);
        match (asked, error) {
            (Asked::Initialize, None) => {
                self.version = result
                    .pointer("/agentInfo/version")
                    .and_then(Value::as_str)
                    .map(String::from);
                vec![self.open_session()]
            }
            (Asked::Initialize, Some(error)) => {
                Driver::fail(format!("{} did not start its protocol: {error}", self.profile.name))
            }
            (Asked::Authenticate, None) => vec![self.open_session()],
            (Asked::Authenticate, Some(error)) => Driver::fail(self.sign_in(&error)),
            (Asked::Open, None) => self.opened(&result),
            // Retry an opening authentication request once, using the profile's method.
            (Asked::Open, Some(_)) if signed_out && self.profile.sign_in.is_some() && !self.signed_in => {
                self.loading = false;
                self.signed_in = true;
                let params = json!({ "methodId": self.profile.sign_in.as_deref() });
                vec![self.request(Asked::Authenticate, "authenticate", params)]
            }
            (Asked::Open, Some(error)) if signed_out => Driver::fail(self.sign_in(&error)),
            (Asked::Open, Some(error)) => {
                self.loading = false;
                let what = if self.resume { "reopen" } else { "start" };
                Driver::fail(format!(
                    "{} could not {what} this conversation: {error}",
                    self.profile.name
                ))
            }
            (Asked::Prompt, Some(error)) if signed_out => self.end_turn(Some(&result), Some(self.sign_in(&error))),
            (Asked::Prompt, error) => self.end_turn(Some(&result), error),
            (Asked::Ack(request_id), error) => vec![Out::Ack { request_id, error }],
            (Asked::Model { request, model }, error) => {
                let mut out = Vec::new();
                match (request, error) {
                    (Some(request_id), error) => {
                        if error.is_none() {
                            self.model = Some(model);
                        }
                        out.push(Out::Ack { request_id, error });
                    }
                    (None, None) => {
                        self.model = Some(model);
                        out.extend(self.settled());
                    }
                    (None, Some(_)) if self.required_launch_model => {
                        self.session = None;
                        self.deferred = None;
                        self.waiting.clear();
                        return Driver::fail("OpenCode could not select this session's saved server model.".into());
                    }
                    (None, Some(error)) => {
                        out.push(Out::Event(WbEvent::Notice {
                            text: format!("{} did not take this session's model: {error}", self.profile.name),
                        }));
                        out.extend(self.settled());
                    }
                }
                out.extend(self.release_levels());
                out
            }
            (
                Asked::Mode {
                    request,
                    mode,
                    acp_mode,
                    sent,
                },
                error,
            ) => {
                let opening = request.is_none();
                if self.confirming_opening_mode() && sent == self.modes_sent {
                    if let Some(error) = error.as_deref() {
                        return self.refuse_opening_mode(error);
                    }
                    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || !result.is_object() {
                        return self.refuse_opening_mode("The agent did not confirm the mode.");
                    }
                    if let Some(reported) = result
                        .get("currentModeId")
                        .or_else(|| result.pointer("/modes/currentModeId"))
                        && reported.as_str() != Some(acp_mode)
                    {
                        return self.refuse_opening_mode("The agent confirmed a different mode.");
                    }
                    if self.reported_opening_mode.is_some_and(|reported| reported != mode) {
                        return self.refuse_opening_mode("The agent reported a different mode.");
                    }
                }
                let mut out = Vec::new();
                match (request, error) {
                    (request, None) => {
                        if self.auto_mode(acp_mode).is_some() {
                            out.extend(self.auto_evidence(true));
                        }
                        let latest = sent > self.in_force.0;
                        if latest {
                            self.in_force = (sent, mode);
                        }
                        let policy_notice = self.profile.auto.as_ref().filter(|auto| {
                            latest
                                && (self
                                    .profile
                                    .modes
                                    .is_some_and(|modes| Some(acp_mode) != (modes.native)(mode))
                                    || (auto.notice_for)(mode, self.auto_offered))
                        });
                        if let Some(request_id) = request {
                            out.push(Out::Ack {
                                request_id,
                                error: None,
                            });
                        }
                        if let Some(auto) = policy_notice
                            && (!opening || !self.resume)
                        {
                            out.push(Out::Event(WbEvent::Notice {
                                text: auto.policy_notice.to_string(),
                            }));
                        }
                    }
                    // Retry an unknown Auto mode with the profile's fallback if this is still the latest request.
                    (request, Some(_))
                        if self
                            .auto_mode(acp_mode)
                            .is_some_and(|auto| code == Some(auto.unknown_mode_error))
                            && self.profile.modes.is_some_and(|modes| {
                                (modes.requested)(mode, Some(false)).is_some_and(|fallback| fallback != acp_mode)
                            }) =>
                    {
                        self.auto_offered = Some(false);
                        out.extend(self.auto_evidence(false));
                        if sent == self.modes_sent
                            && let Some(again) = self.set_mode(mode, request.clone())
                        {
                            out.push(again);
                            return out;
                        }
                        if let Some(request_id) = request {
                            out.push(Out::Ack {
                                request_id,
                                error: None,
                            });
                        }
                    }
                    (Some(request_id), error) => out.push(Out::Ack { request_id, error }),
                    (None, Some(error)) => out.push(Out::Event(WbEvent::Notice {
                        text: format!(
                            "{} did not take this session's permission mode: {error}",
                            self.profile.name
                        ),
                    })),
                }
                if opening {
                    out.extend(self.settled());
                } else if self.confirming_opening_mode() {
                    out.extend(self.send_deferred());
                }
                out
            }
            (Asked::Setting(_), None) => self.settled(),
            (Asked::Setting(what), Some(error)) => {
                let mut out = vec![Out::Event(WbEvent::Notice {
                    text: format!("{} did not take this session's {what}: {error}", self.profile.name),
                })];
                out.extend(self.settled());
                out
            }
        }
    }

    fn sign_in(&self, error: &str) -> String {
        let Profile { name, sign_in_help, .. } = &self.profile;
        format!("{name} is not signed in ({error}). {sign_in_help}, then send again.")
    }

    /// What the call shows as: a known tool name where the kind says enough, else the agent's own name and input.
    fn shown_as(call: &Call, profile: &Profile) -> (String, Value) {
        let diff = call
            .content
            .iter()
            .find(|entry| entry.get("type").and_then(Value::as_str) == Some("diff"));
        let path = diff
            .and_then(|diff| diff.get("path"))
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| call.path.clone())
            .unwrap_or_default();
        match call.kind.as_str() {
            "read" => ("Read".into(), json!({ "file_path": path })),
            "edit" => {
                let (old, new) = match &profile.tool_presentation {
                    Some(presentation) => (presentation.edit_text)(diff),
                    None => (
                        diff.and_then(|diff| diff["oldText"].as_str()),
                        diff.and_then(|diff| diff["newText"].as_str()),
                    ),
                };
                match old {
                    Some(old) => (
                        "Edit".into(),
                        json!({ "file_path": path, "old_string": old, "new_string": new.unwrap_or("") }),
                    ),
                    None => (
                        "Write".into(),
                        json!({ "file_path": path, "content": new.unwrap_or("") }),
                    ),
                }
            }
            "execute" => {
                let command = ["command", "Command", "CommandLine"]
                    .iter()
                    .find_map(|key| call.raw_input.get(*key).and_then(Value::as_str))
                    .unwrap_or(&call.title);
                ("Bash".into(), json!({ "command": command }))
            }
            _ => {
                let name = [&call.name, &call.title]
                    .into_iter()
                    .find(|name| !name.is_empty())
                    .cloned()
                    .unwrap_or_else(|| profile.name.to_string());
                (name, call.raw_input.clone())
            }
        }
    }

    /// Takes in one tool call update; shows the call once enough is known, and its result once it ends.
    fn tool(&mut self, update: &Value, asked: bool) -> Vec<Out> {
        let Some(id) = update.get("toolCallId").and_then(Value::as_str).map(String::from) else {
            return Vec::new();
        };
        let mut out = self.end_text();
        let text = |key: &str| update.get(key).and_then(Value::as_str).map(String::from);
        let name = text("name").map(|name| mcp_tool_name(&name, &self.mcp.names()).unwrap_or(name));
        let call = self.calls.entry(id.clone()).or_default();
        if let Some(kind) = text("kind") {
            call.kind = kind;
        }
        if let Some(title) = text("title") {
            call.title = title;
        }
        if let Some(name) = name {
            call.name = name;
        }
        if let Some(status) = text("status") {
            call.status = status;
        }
        if let Some(path) = update.pointer("/locations/0/path").and_then(Value::as_str) {
            call.path = Some(path.to_string());
        }
        if let Some(input) = update.get("rawInput") {
            call.raw_input = input.clone();
        }
        if let Some(output) = update.get("rawOutput") {
            call.raw_output = output.clone();
        }
        // The first diff is what the person approved; the agent's last one can differ in its trailing line breaks.
        let ended = matches!(call.status.as_str(), "completed" | "failed");
        let take_content = if call.kind == "edit" {
            call.content.is_empty()
        } else {
            self.profile.tool_presentation.is_none() || (ended && !asked)
        };
        if let Some(content) = update.get("content").and_then(Value::as_array)
            && take_content
        {
            call.content = content.clone();
        }
        if ended {
            self.open_calls.remove(&id);
        } else {
            self.open_calls.insert(id.clone());
        }
        let call = self.calls.entry(id.clone()).or_default();
        let waits = matches!(call.kind.as_str(), "edit" | "delete" | "move") && call.content.is_empty();
        if !call.shown && (!waits || ended || asked) {
            call.shown = true;
            let (name, input) = Driver::shown_as(call, &self.profile);
            out.push(Out::Event(WbEvent::Tool {
                id: id.clone(),
                name,
                input,
                parent: None,
            }));
        }
        if ended && !call.ended {
            call.ended = true;
            let mut output: Vec<String> = call
                .content
                .iter()
                .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("content"))
                .filter_map(|entry| entry.pointer("/content/text").and_then(Value::as_str))
                .map(String::from)
                .collect();
            if output.is_empty() {
                if let Some(formatted) = self
                    .profile
                    .tool_presentation
                    .as_ref()
                    .and_then(|presentation| (presentation.output)(&call.raw_output))
                {
                    output = formatted;
                } else {
                    match &call.raw_output {
                        Value::String(text) => output.push(text.clone()),
                        Value::Null => {}
                        Value::Object(map) if map.is_empty() => {}
                        other => output.push(other.to_string()),
                    }
                }
            }
            let (output, truncated) = cap(output.join("\n"));
            out.push(Out::Event(WbEvent::ToolResult {
                id,
                output,
                images: Vec::new(),
                is_error: call.status == "failed",
                truncated,
                parent: None,
            }));
        }
        out
    }

    /// The agent asks before a tool runs.
    fn approve(&mut self, rpc: Value, params: &Value) -> Vec<Out> {
        let call = params.get("toolCall").cloned().unwrap_or(Value::Null);
        self.own_output();
        let mut out = self.taken(true);
        out.extend(self.tool(&call, true));
        let options: Vec<Value> = params
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let tool_use_id = call.get("toolCallId").and_then(Value::as_str).map(String::from);
        let (tool, input) = tool_use_id
            .as_ref()
            .and_then(|id| self.calls.get(id))
            .map(|call| Driver::shown_as(call, &self.profile))
            .unwrap_or_else(|| (self.profile.name.to_string(), Value::Null));
        let suggestions = options
            .iter()
            .filter(|option| option.get("kind").and_then(Value::as_str) == Some("allow_always"))
            .map(|option| PermissionSuggestion {
                kind: "allow_always".into(),
                label: option
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Always allow")
                    .to_string(),
                raw: option.get("optionId").cloned().unwrap_or(Value::Null),
            })
            .collect();
        let request_id = self.block("approval");
        self.asking.insert(
            request_id.clone(),
            Asking {
                rpc,
                options,
                call: tool_use_id.clone(),
            },
        );
        out.push(Out::Event(WbEvent::Approval {
            request_id,
            tool_use_id,
            tool,
            input,
            description: call.get("title").and_then(Value::as_str).map(String::from),
            suggestions,
            parent: None,
        }));
        out
    }

    /// The answer to an approval: the option the person picked, else the first offered of these kinds, else
    /// `cancelled`.
    fn decide(asking: &Asking, kinds: &[&str], picked: Option<&Value>) -> Out {
        let offered = |id: &Value| asking.options.iter().any(|option| option.get("optionId") == Some(id));
        let option = picked.filter(|id| offered(id)).cloned().or_else(|| {
            kinds.iter().find_map(|kind| {
                asking
                    .options
                    .iter()
                    .find(|option| option.get("kind").and_then(Value::as_str) == Some(kind))
                    .and_then(|option| option.get("optionId").cloned())
            })
        });
        let outcome = match option {
            Some(option) => json!({ "outcome": "selected", "optionId": option }),
            None => json!({ "outcome": "cancelled" }),
        };
        Driver::answer(&asking.rpc, json!({ "outcome": outcome }))
    }

    fn update(&mut self, update: &Value) -> Vec<Out> {
        let text = |key: &str| update.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let chunk = || {
            update
                .pointer("/content/text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let kind = text("sessionUpdate");
        if kind == "current_mode_update"
            && self.confirming_opening_mode()
            && let Some(modes) = self.profile.modes
        {
            let Some(reported) = (modes.observed)(update.get("currentModeId").and_then(Value::as_str)) else {
                return self.refuse_opening_mode("The agent reported an unsupported mode.");
            };
            if !self.asked.values().any(|asked| matches!(asked, Asked::Mode { .. })) && reported != self.mode {
                return self.refuse_opening_mode("The agent changed to a different mode.");
            }
            self.reported_opening_mode = Some(reported);
        }
        if kind == "user_message_chunk" {
            if self.running {
                self.echo.get_or_insert_default().push_str(&chunk());
            }
            return Vec::new();
        }
        let acted = matches!(
            kind.as_str(),
            "agent_message_chunk" | "agent_thought_chunk" | "tool_call" | "tool_call_update" | "plan"
        );
        let mut out = self.taken(acted);
        out.extend(match kind.as_str() {
            "agent_message_chunk" => {
                let mut out = Vec::new();
                if let Some((id, thought)) = self.thought.take().filter(|(_, text)| !text.trim().is_empty()) {
                    out.push(Out::Event(WbEvent::Thinking {
                        id,
                        text: thought,
                        parent: None,
                    }));
                }
                if self.text.is_none() {
                    let id = self.block("text");
                    self.text = Some((id, String::new()));
                }
                if let Some((id, so_far)) = &mut self.text {
                    so_far.push_str(&chunk());
                    out.push(Out::Delta {
                        id: id.clone(),
                        text: so_far.clone(),
                        parent: None,
                    });
                }
                out
            }
            "agent_thought_chunk" => {
                let mut out = Vec::new();
                if let Some((id, text)) = self.text.take().filter(|(_, text)| !text.trim().is_empty()) {
                    out.push(Out::Event(WbEvent::Text { id, text, parent: None }));
                }
                if self.thought.is_none() {
                    let id = self.block("thought");
                    self.thought = Some((id, String::new()));
                }
                if let Some((_, so_far)) = &mut self.thought {
                    so_far.push_str(&chunk());
                }
                out
            }
            "tool_call" | "tool_call_update" => self.tool(update, false),
            "usage_update" => {
                if let Some(size) = update.get("size").and_then(Value::as_u64) {
                    self.context_window = Some(size);
                }
                Vec::new()
            }
            "plan" => {
                let mut out = self.end_text();
                let todos: Vec<Value> = update
                    .get("entries")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|entry| {
                        let content = entry.get("content").and_then(Value::as_str).unwrap_or("");
                        json!({ "content": content, "status": entry.get("status"), "activeForm": content })
                    })
                    .collect();
                let id = self.block("plan");
                out.push(Out::Event(WbEvent::Tool {
                    id: id.clone(),
                    name: "TodoWrite".into(),
                    input: json!({ "todos": todos }),
                    parent: None,
                }));
                out.push(Out::Event(WbEvent::ToolResult {
                    id,
                    output: "Plan updated.".into(),
                    images: Vec::new(),
                    is_error: false,
                    truncated: false,
                    parent: None,
                }));
                out
            }
            _ => Vec::new(),
        });
        if out.iter().any(|out| matches!(out, Out::Event(_) | Out::Delta { .. })) {
            self.own_output();
        }
        out
    }
}

impl Protocol for Driver {
    fn require_launch_model(&mut self, model: &str) {
        self.wanted_model = Some(model.into());
        self.required_launch_model = true;
    }

    fn session_mcp(&mut self, mcp: SessionMcp) {
        self.mcp = mcp;
    }

    fn open(&mut self) -> Vec<Out> {
        if let Some(reason) = self.refused.take() {
            return Driver::fail(reason);
        }
        let params = json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
        });
        vec![self.request(Asked::Initialize, "initialize", params)]
    }

    fn say(&mut self, say: Say) -> Vec<Out> {
        if self.opening_mode_failed {
            return Vec::new();
        }
        let ack = |request: String| {
            vec![Out::Ack {
                request_id: request,
                error: None,
            }]
        };
        let refuse = |request: String, what: &str| {
            vec![Out::Answered {
                request_id: request,
                result: Err(format!("{} does not support {what} in Nexus yet.", self.profile.name)),
            }]
        };
        match say {
            Say::Steer { text, images, person } if self.running => {
                self.waiting.push((text, images));
                vec![Out::TurnQueued { person }]
            }
            Say::User { text, images } if self.running => {
                self.waiting.push((text, images));
                vec![Out::TurnQueued { person: true }]
            }
            Say::User { text, images } | Say::Steer { text, images, .. } => self.prompt(text, images),
            Say::Interrupt { request } => {
                let mut out = Vec::new();
                if let Some(session) = self.session.clone() {
                    let cancel =
                        json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } });
                    out.push(Out::Reply(cancel.to_string()));
                }
                for (request_id, asking) in self.asking.drain() {
                    out.push(Driver::answer(
                        &asking.rpc,
                        json!({ "outcome": { "outcome": "cancelled" } }),
                    ));
                    out.push(Out::Cancelled { request_id });
                }
                // A turn the agent started itself has no answer to end it: Stop ends it here.
                if self.own_turn && !self.running {
                    out.extend(self.end_own_turn(true));
                }
                // A turn that never reached the agent ends here, and the message queued behind it runs next.
                if self.deferred.take().is_some() {
                    self.echo = None;
                    self.running = false;
                    self.started = None;
                    out.push(Out::Result {
                        total_cost_usd: None,
                        duration_ms: 0,
                        error: None,
                        interrupted: true,
                        context_window: self.context_window,
                    });
                    if !self.waiting.is_empty() {
                        let (text, images) = self.waiting.remove(0);
                        out.extend(self.prompt(text, images));
                    }
                }
                out.extend(ack(request));
                out
            }
            Say::Mode { request, mode } => {
                match self.profile.modes {
                    None => {
                        return vec![Out::Ack {
                            request_id: request,
                            error: Some(format!(
                                "{} does not support changing modes in Nexus yet.",
                                self.profile.name
                            )),
                        }];
                    }
                    Some(modes) if (modes.requested)(mode, self.auto_offered).is_none() => {
                        return vec![Out::Ack {
                            request_id: request,
                            error: Some(format!("{} does not offer this permission mode.", self.profile.name)),
                        }];
                    }
                    Some(_) => {}
                }
                self.mode = mode;
                self.set_mode(mode, Some(request.clone()))
                    .map(|out| vec![out])
                    .unwrap_or_else(|| ack(request))
            }
            // Back to the default: the agent's own default, once it named it; a conversation not open yet opens on it.
            Say::Model { request, model: None } => {
                self.wanted_model = None;
                match self
                    .default_model
                    .clone()
                    .filter(|model| self.model.as_ref() != Some(model))
                {
                    Some(model) => {
                        let asked = Asked::Model {
                            request: Some(request.clone()),
                            model: model.clone(),
                        };
                        self.set_option(asked, "model", &model)
                            .map(|out| vec![out])
                            .unwrap_or_else(|| ack(request))
                    }
                    None => ack(request),
                }
            }
            Say::Model {
                request,
                model: Some(model),
            } => {
                let asked = Asked::Model {
                    request: Some(request.clone()),
                    model: model.clone(),
                };
                match self.set_option(asked, "model", &model) {
                    Some(out) => vec![out],
                    // Asked for as the conversation opens.
                    None => {
                        self.wanted_model = Some(model);
                        ack(request)
                    }
                }
            }
            Say::Effort { request, effort: None } => {
                self.effort = None;
                match self.default_level.clone().filter(|_| self.session.is_some()) {
                    Some(level) => self.set_level(Asked::Ack(request), &level),
                    None => ack(request),
                }
            }
            // Only "off" reaches here: fast mode is Claude Code's.
            Say::Fast { request, .. } => ack(request),
            Say::Effort {
                request,
                effort: Some(effort),
            } => match self.thought_level(effort) {
                Some(level) => {
                    self.effort = Some(effort);
                    match self.session {
                        Some(_) => self.set_level(Asked::Ack(request), level),
                        // Asked for as the conversation opens.
                        None => ack(request),
                    }
                }
                None => vec![Out::Ack {
                    request_id: request,
                    error: Some(self.unsupported_level().into()),
                }],
            },
            Say::Allow {
                request_id, remember, ..
            } => match self.asking.remove(&request_id) {
                // An "always" option only as the person picked it.
                Some(asking) => vec![Driver::decide(&asking, &["allow_once"], remember.as_ref())],
                None => Vec::new(),
            },
            // ACP's answer to an approval has no text: the workbench sends the reason as the person's own queued message
            // (`Workbench::queue_denial_reason`).
            Say::Deny { request_id, .. } => {
                let Some(asking) = self.asking.remove(&request_id) else {
                    return Vec::new();
                };
                // A denied call does not run: the agent may send no end for it, and it must not hold the agent's own turn open.
                if let Some(call) = &asking.call {
                    self.open_calls.remove(call);
                }
                vec![Driver::decide(&asking, &["reject_once", "reject_always"], None)]
            }
            Say::ForgetInspection { .. } | Say::Rename { .. } => Vec::new(),
            Say::Compact { request, .. } => refuse(request, "compaction"),
            Say::Inspect { request, .. } => refuse(request, "metadata reads"),
            Say::TaskStop { request, .. } | Say::TaskBackground { request, .. } | Say::TaskOutput { request, .. } => {
                refuse(request, "task controls")
            }
        }
    }

    fn read(&mut self, line: &str) -> Vec<Out> {
        if self.opening_mode_failed {
            return Vec::new();
        }
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        let method = message.get("method").and_then(Value::as_str);
        let rpc = message.get("id").cloned();
        match (method, rpc) {
            // the agent's own request: an approval, or something this client does not offer.
            (Some("session/request_permission"), Some(rpc)) => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                self.approve(rpc, &params)
            }
            (Some(_), Some(rpc)) => vec![Out::Reply(
                json!({ "jsonrpc": "2.0", "id": rpc, "error": { "code": NO_SUCH_METHOD, "message": format!("Nexus does not offer this to {}.", self.profile.name) } })
                    .to_string(),
            )],
            // A load replays the conversation the session's log already holds.
            (Some("session/update"), None) if self.loading => Vec::new(),
            (Some("session/update"), None) => message
                .pointer("/params/update")
                .map(|update| self.update(update))
                .unwrap_or_default(),
            (Some(_), None) => Vec::new(),
            (None, Some(rpc)) => rpc.as_u64().map(|id| self.answered(id, &message)).unwrap_or_default(),
            (None, None) => Vec::new(),
        }
    }

    fn quiet_after(&self) -> Option<Duration> {
        self.own_turn_settled().then_some(self.profile.own_turn_quiet)
    }

    fn quiet(&mut self) -> Vec<Out> {
        if !self.own_turn_settled() {
            return Vec::new();
        }
        self.end_own_turn(false)
    }
}

#[cfg(test)]
mod tests;
