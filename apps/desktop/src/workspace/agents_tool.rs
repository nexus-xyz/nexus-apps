//! The Workbench's Agents tab. The shared crew fold owns agent semantics; this file owns selection and controls.
//! Rows open activity and controls inside the panel; the conversation remains visible.
#[path = "agent_view.rs"]
mod agent_view;
mod detail;
mod facts;

mod crew_status;

mod loading;
#[path = "crew_map.rs"]
mod map;
mod mini_map;
mod model;
mod render;
mod rows;
mod side_chats;
use gpui_kit::component::text::TextViewState;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
static NEXT: AtomicU64 = AtomicU64::new(1);

use gpui_kit::*;
use nexus_workbench::crew_fold::{self, AgentKind, AgentNode, AgentStatus, Crew, CrewFolder, StatusSource};
use nexus_workbench::event::{AgentCapabilities, SessionAgent, StoredEvent, ThreadSummary, WbEvent};
use serde_json::{Value, json};

use super::{View, Workspace};

pub(super) struct AgentsTool {
    id: String,
    epoch: u64,
    crew: Option<Crew>,
    events: Arc<Vec<StoredEvent>>,
    folds: Arc<std::sync::Mutex<CrewFolder>>,
    /// The session's plan, folded with its events (`nexus_workbench::plan_steps`).
    plans: Arc<std::sync::Mutex<nexus_workbench::plan_steps::PlanFolder>>,
    /// The plan has a step: the tab shows it, not "Nothing open" (`side_pane/nothing_open.rs`).
    has_plan: bool,
    pub(super) summary: Option<ThreadSummary>,
    facts_open: std::collections::HashSet<&'static str>,
    side_chats: Vec<ThreadSummary>,
    /// When each side chat's title last changed while listed, for its row's motion (`motion::retitle`).
    side_chat_retitled: std::collections::HashMap<String, Instant>,
    /// Every side chat shows, not the latest eight ("All side chats").
    show_all_side_chats: bool,
    selected: Option<String>,
    pub(super) pending_session: Option<String>,
    cursor: Option<String>,
    pub(super) focus: FocusHandle,
    show_finished: bool,
    collapsed: std::collections::HashSet<String>,
    arriving: std::collections::HashMap<String, Instant>,
    loading: bool,
    busy: bool,
    next: Instant,
    error: Option<String>,
    action_error: Option<String>,
    output: Option<String>,
    generation: u64,
    detail_limit: usize,
    /// Agent details inside the pane leave the conversation and panel geometry unchanged.
    detail_in_pane: bool,
    overview_scroll: ScrollHandle,
    mini_scroll: ScrollHandle,
    map: Option<map::MapState>,
    detail: Vec<model::Activity>,
    live: Option<(String, Entity<TextViewState>)>,
    open_tools: std::collections::HashSet<i64>,
    markdown: std::collections::HashMap<i64, (String, Entity<TextViewState>)>,
    /// The row whose last steps show under it (Space).
    peek: Option<String>,
}

impl AgentsTool {
    /// A test's tool for session `id` that reads nothing for an hour: a test that draws an open session then opens no
    /// store (the read in `sync_agents_tool` falls back to the store in the person's data folder).
    #[cfg(test)]
    pub(super) fn resting(id: String, cx: &mut App) -> Self {
        let mut tool = Self::new(id, SessionAgent::Claude, cx);
        tool.next = Instant::now() + Duration::from_secs(3600);
        tool
    }

    fn new(id: String, provider: SessionAgent, cx: &mut App) -> Self {
        Self {
            id,
            epoch: NEXT.fetch_add(1, Ordering::Relaxed),
            crew: None,
            events: Arc::new(vec![]),
            folds: Arc::new(std::sync::Mutex::new(CrewFolder::new(provider))),
            plans: Default::default(),
            has_plan: false,
            summary: None,
            facts_open: Default::default(),
            side_chats: vec![],
            side_chat_retitled: Default::default(),
            show_all_side_chats: false,
            selected: None,
            pending_session: None,
            cursor: None,
            focus: cx.focus_handle(),
            show_finished: false,
            collapsed: Default::default(),
            arriving: Default::default(),
            loading: false,
            busy: false,
            next: Instant::now(),
            error: None,
            action_error: None,
            output: None,
            generation: 0,
            detail_limit: 200,
            detail_in_pane: false,
            overview_scroll: ScrollHandle::new(),
            mini_scroll: ScrollHandle::new(),
            map: None,
            detail: vec![],
            live: None,
            open_tools: Default::default(),
            markdown: Default::default(),
            peek: None,
        }
    }
}

fn status(node: &AgentNode) -> &'static str {
    if crate::code::policy_of(node.status, node.reason.as_deref()).is_some() {
        return "Blocked";
    }
    match node.status {
        AgentStatus::Running => "Working",
        AgentStatus::Needs => "Needs you",
        AgentStatus::Waiting => "Working",
        AgentStatus::Idle => "Done",
        AgentStatus::Done => "Done",
        AgentStatus::Failed => "Failed",
        AgentStatus::Stopped => "Stopped",
    }
}

impl AgentsTool {
    fn refresh_live(&mut self, cx: &mut App) {
        let text = self
            .selected
            .as_deref()
            .and_then(|id| self.crew.as_ref()?.resolve(id))
            .filter(|node| node.kind != AgentKind::Lead)
            .and_then(|node| {
                crate::stream::agent_texts(
                    &self.id,
                    node.call_id.iter().chain(node.task_id.iter()).map(String::as_str),
                )
                .into_iter()
                .next()
            })
            .map(|(_, text)| text);
        match (text, &mut self.live) {
            (Some(text), Some((old, view))) if text != *old => {
                view.update(cx, |state, cx| state.set_text(&text, cx));
                *old = text;
            }
            (Some(text), None) => {
                let view = cx.new(|cx| TextViewState::markdown(&text, cx));
                self.live = Some((text, view));
            }
            (None, _) => self.live = None,
            _ => {}
        }
    }
    fn refresh_detail(&mut self, cx: &mut App) {
        self.detail = self
            .selected
            .as_deref()
            .and_then(|id| self.crew.as_ref()?.resolve(id))
            .map(|node| model::activity(&self.events, node))
            .unwrap_or_default();
        let start = self.detail.len().saturating_sub(self.detail_limit);
        self.markdown
            .retain(|seq, _| self.detail[start..].iter().any(|row| row.seq == *seq));
        for row in &self.detail[start..] {
            let source = if row.title.is_some() {
                model::literal(&row.text)
            } else {
                row.text.clone()
            };
            match self.markdown.get_mut(&row.seq) {
                Some((text, view)) if *text != source => {
                    view.update(cx, |state, cx| state.set_text(&source, cx));
                    *text = source;
                }
                None => {
                    let view = cx.new(|cx| TextViewState::markdown(&source, cx));
                    self.markdown.insert(row.seq, (source, view));
                }
                _ => {}
            }
        }
    }
}

impl Workspace {
    /// The open Code session's crew, read every few seconds whether or not the Agents tab shows: the tab's badge, and
    /// later the pill and the mini map, read it here.
    pub(crate) fn crew(&self) -> Option<&Crew> {
        let View::Code(view) = &self.view else {
            return None;
        };
        self.side.agents.as_ref().filter(|s| s.id == view.id)?.crew.as_ref()
    }

    /// Read the open Code session's crew: every 3s while the Agents tab shows, every 5s while it does not.
    pub(super) fn sync_agents_tool(&mut self, cx: &mut Context<Self>) {
        let View::Code(view) = &self.view else {
            return;
        };
        if view.is_draft() {
            return;
        }
        let every = Duration::from_secs(if self.side.showing(super::side_pane::Tool::Agents) {
            3
        } else {
            5
        });
        let id = view.id.clone();
        let provider = self.code_session().map_or(SessionAgent::Claude, |s| s.agent);
        if self.side.agents.as_ref().is_none_or(|a| a.id != id) {
            self.side.agents = Some(AgentsTool::new(id.clone(), provider, cx));
        }
        self.side.agents.as_mut().unwrap().refresh_live(cx);
        let state = self.side.agents.as_ref().unwrap();
        if state.loading || state.next > Instant::now() {
            return;
        }
        let runs = self.runs(&id);
        let scope = self.code_scope.clone().unwrap_or_default();
        let after = state.events.last().map_or(0, |e| e.seq);
        let reads = runs.as_ref().map(|runs| {
            (
                runs.call("wb_thread_events", json!({"threadId": id, "afterSeq": after})),
                runs.call("wb_overview", json!({})),
            )
        });
        let state = self.side.agents.as_mut().unwrap();
        state.loading = true;
        let old = state.events.clone();
        let folds = state.folds.clone();
        let plans = state.plans.clone();
        let selected = state.selected.clone();
        let generation = state.generation;
        let epoch = state.epoch;
        let output = selected
            .as_deref()
            .and_then(|id| state.crew.as_ref()?.resolve(id))
            // A live command's output every read; an ended one's once, as its last output.
            .filter(|node| node.kind == AgentKind::Shell && (node.status.is_live() || state.output.is_none()))
            .and_then(|node| node.task_id.clone())
            .and_then(|task| {
                Some(
                    runs.as_ref()?
                        .call("wb_task_output", json!({"threadId": id, "taskId": task})),
                )
            });
        let request_id = id.clone();
        let read = cx.background_spawn(async move {
            let read = async {
                let (new, summaries) = if let Some((events, overview)) = reads {
                    let value = events.await.map_err(|_| "The event read was lost.".to_string())??;
                    let new: Vec<StoredEvent> = serde_json::from_value(value).map_err(|e| e.to_string())?;
                    let overview = overview.await.map_err(|_| "The session read was lost.".to_string())??;
                    let summaries: Vec<ThreadSummary> =
                        serde_json::from_value(overview["threads"].clone()).map_err(|e| e.to_string())?;
                    (new, summaries)
                } else {
                    let store = crate::code::CodeStore::open(scope)?.ok_or("No session store on this computer.")?;
                    store.reader().agent_tree_events(&request_id, after)?
                };
                let summary = summaries.iter().find(|summary| summary.id == request_id).cloned();
                let side_chats = summaries
                    .iter()
                    .filter(|s| s.side_chat && s.source_thread_id.as_deref() == Some(request_id.as_str()))
                    .cloned()
                    .collect::<Vec<_>>();
                let crew = {
                    let mut folds = folds
                        .lock()
                        .map_err(|_| "The agent tree could not be read.".to_string())?;
                    folds.push(&new);
                    folds.crew().clone()
                };
                let crew = crew.with_linked_sessions(summary.as_ref(), &summaries);
                let has_plan = {
                    let mut plans = plans.lock().map_err(|_| "The plan could not be read.".to_string())?;
                    plans.push(&new);
                    !plans.plan().steps.is_empty()
                };
                let events = if new.is_empty() {
                    old
                } else {
                    let mut events = old.as_ref().clone();
                    events.extend(new);
                    Arc::new(events)
                };
                let output = match output {
                    Some(read) => Some(
                        read.await
                            .map_err(|_| "The output read was lost.".to_string())?
                            .map(|v| output_text(&v)),
                    ),
                    None => None,
                };
                Ok::<_, String>((events, crew, has_plan, summary, side_chats, output))
            };
            read.await
        });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                let Some(state) = this.side.agents.as_mut().filter(|s| s.id == id && s.epoch == epoch) else {
                    return;
                };
                state.loading = false;
                state.next = Instant::now() + every;
                match result {
                    Ok((events, crew, has_plan, summary, side_chats, output)) => {
                        state.has_plan = has_plan;
                        let changed = !Arc::ptr_eq(&state.events, &events);
                        state.events = events;
                        let now = Instant::now();
                        state
                            .arriving
                            .retain(|_, at| now.duration_since(*at) < crate::motion::UI_LONG);
                        if let Some(previous) = &state.crew {
                            for node in &crew.nodes {
                                if previous.resolve(&node.id).is_none() {
                                    state.arriving.insert(node.id.clone(), now);
                                }
                            }
                        }
                        state.crew = Some(crew);
                        state.summary = summary;
                        state
                            .side_chat_retitled
                            .retain(|_, at| now.duration_since(*at) < Duration::from_secs(1));
                        for side in &side_chats {
                            if state
                                .side_chats
                                .iter()
                                .any(|old| old.id == side.id && old.title != side.title)
                            {
                                state.side_chat_retitled.insert(side.id.clone(), now);
                            }
                        }
                        state.side_chats = side_chats;
                        state.error = None;
                        if changed {
                            state.refresh_detail(cx);
                        }
                        if state.generation == generation
                            && let Some(output) = output
                        {
                            match output {
                                Ok(text) => state.output = Some(text),
                                Err(error) => state.error = Some(error),
                            }
                        }
                    }
                    Err(error) => state.error = Some(error),
                }
                cx.notify();
            });
            cx.background_executor().timer(every).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    pub(super) fn inspect_subagent(&mut self, call: String, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_tool(super::side_pane::Tool::Agents, window, cx);
        self.sync_agents_tool(cx);
        self.open_agent_detail(call, cx);
        if let Some(state) = &self.side.agents {
            state.focus.focus(window, cx);
        }
    }

    /// Open details within Agents, without replacing the conversation or resizing the pane.
    fn open_agent_detail(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.side.agents {
            state.detail_in_pane = true;
            state.map = None;
        }
        self.select_agent(id, cx);
    }

    fn open_agent_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let linked = self.side.agents.as_ref().and_then(|state| {
            let node = state.crew.as_ref()?.resolve(state.selected.as_deref()?)?;
            (node.kind == AgentKind::Linked)
                .then(|| node.id.strip_prefix("session:").map(str::to_string))
                .flatten()
        });
        self.close_agent_view(cx);
        if let Some(session) = linked
            && let Some(state) = &mut self.side.agents
        {
            state.pending_session = Some(session);
        }
        self.focus_composer(window, cx);
    }

    pub(super) fn agent_panel_focused(&self, window: &Window, cx: &App) -> bool {
        self.side.showing(super::side_pane::Tool::Agents)
            && self
                .side
                .agents
                .as_ref()
                .is_some_and(|state| state.detail_in_pane && state.focus.contains_focused(window, cx))
    }

    pub(super) fn back_to_agents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_agent_view(cx);
        if let Some(state) = &self.side.agents {
            state.focus.focus(window, cx);
        }
    }

    /// Open an agent's view in the main area. The lead's is the conversation itself.
    fn select_agent(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(session) = self
            .side
            .agents
            .as_ref()
            .and_then(|state| state.crew.as_ref())
            .and_then(|crew| crew.resolve(&id))
            .filter(|node| node.kind == AgentKind::Linked && !self.side.agents.as_ref().unwrap().detail_in_pane)
            .and_then(|node| node.id.strip_prefix("session:"))
            .map(str::to_string)
        {
            if let Some(state) = &mut self.side.agents {
                state.pending_session = Some(session);
            }
            cx.notify();
            return;
        }
        let lead = id == crew_fold::LEAD_ID
            || self
                .side
                .agents
                .as_ref()
                .and_then(|s| s.crew.as_ref()?.resolve(&id))
                .is_some_and(|node| node.kind == AgentKind::Lead);
        if lead && !self.side.agents.as_ref().is_some_and(|s| s.detail_in_pane) {
            self.close_agent_view(cx);
            if let Some(state) = &mut self.side.agents {
                state.cursor = Some(crew_fold::LEAD_ID.into());
            }
            return;
        }
        let clock = self
            .side
            .agents
            .as_ref()
            .map(|state| (state.epoch, state.generation + 1, id.clone()));

        if let Some(state) = &mut self.side.agents {
            state.action_error = None;
            state.cursor = Some(id.clone());
            state.selected = Some(id);
            state.live = None;
            state.open_tools.clear();
            state.detail_limit = 200;
            state.output = None;
            state.error = None;
            state.generation += 1;
            state.next = Instant::now();
            state.refresh_detail(cx);
            state.refresh_live(cx);
        }
        if let Some((epoch, generation, selected)) = clock {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let keep = this
                        .update(cx, |this, cx| {
                            let active = this.side.agents.as_ref().is_some_and(|state| {
                                state.epoch == epoch
                                    && state.generation == generation
                                    && state.selected.as_deref() == Some(selected.as_str())
                                    && state
                                        .crew
                                        .as_ref()
                                        .and_then(|crew| crew.resolve(&selected))
                                        .is_some_and(|node| node.status.is_live())
                            });
                            if active {
                                cx.notify();
                            }
                            active
                        })
                        .unwrap_or(false);
                    if !keep {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn agent_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(state) = &mut self.side.agents else {
            return false;
        };
        let Some(crew) = &state.crew else {
            return false;
        };
        if key == "escape" && state.selected.is_some() {
            self.close_agent_view(cx);
        } else if key == "space" {
            let cursor = state.cursor.clone();
            state.peek = if state.peek == cursor { None } else { cursor };
        } else if key == "enter" {
            let id = state.cursor.clone().unwrap_or_else(|| "lead".into());
            self.open_agent_detail(id, cx);
        } else if key == "s" {
            let node = state.cursor.as_deref().and_then(|id| crew.resolve(id));
            let caps = state
                .summary
                .as_ref()
                .and_then(|summary| summary.capabilities.as_deref());
            let task = node
                .filter(|node| node.kind == AgentKind::Shell && node.status.is_live())
                .filter(|node| crew_fold::controls_of(node, caps).stop.available)
                .and_then(|node| node.task_id.clone());
            let id = state.id.clone();
            if state.busy || self.runs(&id).is_none() {
                return false;
            }
            if let Some(task) = task {
                self.agent_action("wb_task_stop", json!({"threadId": id, "taskId": task}), false, cx);
            }
        } else if key == "left" || key == "right" {
            let id = state.cursor.clone().unwrap_or_else(|| crew_fold::LEAD_ID.into());
            if let Some(node) = crew.resolve(&id) {
                let child = crew.children_of(node).next().map(|child| child.id.clone());
                if key == "right" {
                    if !state.collapsed.remove(&id)
                        && let Some(child) = child
                    {
                        state.cursor = Some(child);
                    }
                } else if child.is_some() && !state.collapsed.contains(&id) {
                    state.collapsed.insert(id);
                } else {
                    state.cursor = node.parent_id.clone().or(Some(crew_fold::LEAD_ID.into()));
                }
            }
        } else if ["up", "down", "home", "end"].contains(&key) {
            let rows = model::folded_rows(crew, state.show_finished, state.cursor.as_deref(), &state.collapsed);
            let ids: Vec<_> = rows
                .agents
                .iter()
                .chain(rows.commands.iter())
                .map(|n| n.id.as_str())
                .collect();
            let next = model::step(&ids, state.cursor.as_deref(), key);
            let detail = state.selected.is_some();
            state.cursor = next.clone();
            if detail && let Some(id) = next {
                self.select_agent(id, cx);
            }
        } else {
            return false;
        }
        cx.notify();
        true
    }

    fn show_agent_in_conversation(&mut self, selected: &str, cx: &mut Context<Self>) {
        let call = self
            .side
            .agents
            .as_ref()
            .and_then(|state| state.crew.as_ref()?.resolve(selected))
            .and_then(|node| node.call_id.clone());
        if let View::Code(view) = &mut self.view
            && let Some(call) = call
            && let Some(ix) = view
                .blocks
                .iter()
                .position(|b| matches!(b.inner(), crate::code::Block::Agent(row) if row.call_id == call))
        {
            let path = view.blocks[ix].path().to_vec();
            for parent in path {
                if !view.agent_open(&parent) {
                    view.toggle_agent(&parent);
                }
            }
            view.list.scroll_to_reveal_item(ix);
        }
        self.close_agent_view(cx);
        self.side.maximized = false;
        cx.notify();
    }

    /// Back to the conversation: the agent's view closes, and the Agents tab keeps its place in the list.
    pub(super) fn close_agent_view(&mut self, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.side.agents {
            state.selected = None;
            state.detail_in_pane = false;
            state.live = None;
            state.output = None;
            state.action_error = None;
            state.detail.clear();
            state.markdown.clear();
            state.generation += 1;
        }
        cx.notify();
    }

    /// Whether the main area shows an agent's view instead of the open Code session's conversation.
    pub(super) fn agent_view_open(&self) -> bool {
        let View::Code(view) = &self.view else {
            return false;
        };
        self.side.agents.as_ref().is_some_and(|state| {
            state.id == view.id
                && !state.detail_in_pane
                && state.selected.as_deref().is_some_and(|id| match &state.crew {
                    Some(crew) => crew.resolve(id).is_some_and(|node| node.kind != AgentKind::Lead),
                    None => id != crew_fold::LEAD_ID,
                })
        })
    }

    /// A row that needs you: back to the conversation, where the request waits in the dock. The Workbench gives back
    /// the room, the transcript shows the request, and the dock takes the keyboard.
    fn answer_in_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_agent_view(cx);
        self.side.maximized = false;
        if let View::Code(view) = &mut self.view {
            let ix = view
                .blocks
                .iter()
                .rposition(|b| matches!(b, crate::code::Block::Approval { answer: None, .. }))
                .unwrap_or(view.blocks.len().saturating_sub(1));
            view.list.scroll_to_reveal_item(ix);
        }
        self.focus_pending_dock(window, cx);
        cx.notify();
    }

    fn agent_action(&mut self, name: &'static str, args: Value, output: bool, cx: &mut Context<Self>) {
        let Some(id) = self.side.agents.as_ref().map(|s| s.id.clone()) else {
            return;
        };
        let Some(runs) = self.runs(&id) else {
            return;
        };
        let state = self.side.agents.as_mut().unwrap();
        if state.busy {
            return;
        }
        state.busy = true;
        state.action_error = None;
        let id = state.id.clone();
        let generation = state.generation;
        let epoch = state.epoch;
        let read = runs.call(name, args);
        cx.spawn(async move |this, cx| {
            let result = read
                .await
                .unwrap_or_else(|_| Err("The command's answer was lost.".into()));
            let _ = this.update(cx, |this, cx| {
                let Some(state) = this.side.agents.as_mut().filter(|s| s.id == id && s.epoch == epoch) else {
                    return;
                };
                state.busy = false;
                if state.generation == generation {
                    match result {
                        Ok(value) if output => state.output = Some(output_text(&value)),
                        Ok(value) if name == "wb_task_background" && value["backgrounded"] == false => {
                            state.action_error = Some("The command could not move to the background.".into())
                        }
                        Ok(_) => {}
                        Err(error) => state.action_error = Some(error),
                    }
                }
                state.next = Instant::now();
                cx.notify();
            });
        })
        .detach();
    }
}

fn output_text(value: &Value) -> String {
    let text = value["output"].as_str().unwrap_or("");
    format!(
        "{}{}",
        if value["truncated"] == true { "…\n" } else { "" },
        if text.is_empty() { "No output yet." } else { text }
    )
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod navigation_tests;

#[cfg(test)]
mod compact_tab_tests;

#[cfg(test)]
mod nothing_open_tests;
