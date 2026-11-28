//! Pending tool requests above the composer. Every decision targets its original request.

use std::collections::{HashMap, HashSet};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::event::{Decision, PermissionSuggestion};
use serde_json::{Value, json};

use super::code_dock::{Menu, PLAN_TOOL, QUESTION_TOOL, answer_button, describe_all, reason_field, text_button};
use super::code_view::CodeView;
use super::message_text::MessageText;
use super::{View, Workspace, glyph};
use crate::code::Block;
use crate::pages::ui::{ghost_button, permission_button, permission_footer};
use crate::palette::Palette;

#[derive(Default)]
pub(super) struct PermissionQueue {
    pub(super) open: Option<String>,
    busy: HashSet<String>,
    settled: HashSet<String>,
    seen: HashSet<String>,
    errors: HashMap<String, String>,
    initialized: bool,
    pub(super) collapsed: bool,
    pub(super) reviewing: bool,
    compact: std::cell::Cell<bool>,
    pub(super) keep_focus: bool,
    /// "Deny with a reason…" opened the reason field under the open request; TR-X2: the reason follows Deny.
    pub(super) noting: bool,
    scroll: ScrollHandle,
}

/// The card's answers, with the canon's words (`canon_transcript.py`, `parts/approval-card.html`).
pub(super) const ALLOW: &str = "Allow";
pub(super) const ALLOW_SESSION: &str = "Allow for this session";
pub(super) const DENY: &str = "Deny";
/// The rules menu, behind its own quiet link: the canon's proposed "Allow always for a pattern" (TR-X1).
pub(super) const ALLOW_ALWAYS: &str = "Allow always…";
/// Opens the reason sent with Deny.
pub(super) const DENY_WITH_REASON: &str = "Deny with a reason…";

/// Where the session runs, for the card's location line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RunsOn {
    Here,
    Cloud,
    /// A server reached over SSH, by its saved name.
    Server(String),
}

/// The card's location line: "On this computer", "On a cloud machine", or the server's name.
pub(super) fn place_line(place: &RunsOn) -> String {
    match place {
        RunsOn::Here => "On this computer".into(),
        RunsOn::Cloud => "On a cloud machine".into(),
        RunsOn::Server(name) => format!("On {name}"),
    }
}

/// The card's title: "Claude Code wants to run a command".
pub(super) fn ask_title(agent: &str, tool: &str) -> String {
    format!("{agent} wants to {}", super::code_view::ask_verb(tool))
}

/// The rule "Allow for this session" keeps, as the phone's answer picks it (`session_sync/control/answer.rs`'s
/// `session_rule`): Codex's and Nexus's `acceptForSession`, else a rule the agent already scoped to the session. A rule
/// saved past the session never stands in for it, so with none the button does not show.
pub(super) fn session_suggestion(suggestions: &[PermissionSuggestion]) -> Option<usize> {
    let raw = |ix: usize| &suggestions[ix].raw;
    (0..suggestions.len())
        .find(|&ix| raw(ix).as_str() == Some("acceptForSession"))
        .or_else(|| {
            (0..suggestions.len()).find(|&ix| {
                let raw = raw(ix);
                let field = |key: &str| raw.get(key).and_then(Value::as_str);
                field("destination") == Some("session")
                    && field("behavior") != Some("deny")
                    && match field("type") {
                        Some("setMode") => field("mode") == Some("acceptEdits"),
                        Some("addRules" | "addDirectories") => true,
                        _ => false,
                    }
            })
        })
}

pub(super) fn tool_request(block: &Block) -> Option<&str> {
    match block {
        Block::Approval {
            request_id,
            tool,
            answer: None,
            ..
        } if tool != QUESTION_TOOL && tool != PLAN_TOOL => Some(request_id),
        _ => None,
    }
}

impl PermissionQueue {
    pub(super) fn pending<'a>(&self, blocks: &'a [Block]) -> Vec<&'a Block> {
        blocks
            .iter()
            .filter(|block| tool_request(block).is_some_and(|id| !self.settled.contains(id)))
            .collect()
    }

    pub(super) fn selected<'a>(&self, blocks: &'a [Block]) -> Option<&'a Block> {
        if self.collapsed || (self.compact.get() && !self.reviewing) {
            return None;
        }
        self.pending(blocks)
            .into_iter()
            .find(|block| tool_request(block) == self.open.as_deref())
    }

    /// Questions and plans retain their existing dock priority over tool decisions.
    pub(super) fn current<'a>(&self, blocks: &'a [Block]) -> Option<&'a Block> {
        match crate::code::pending_approval(blocks) {
            Some(block @ Block::Approval { tool, .. }) if tool == QUESTION_TOOL || tool == PLAN_TOOL => Some(block),
            _ => self.selected(blocks),
        }
    }

    pub(super) fn follow(&mut self, blocks: &[Block]) {
        let before = self.open.clone();
        let ids: HashSet<_> = blocks.iter().filter_map(tool_request).map(str::to_owned).collect();
        self.busy.retain(|id| ids.contains(id));
        self.settled.retain(|id| ids.contains(id));
        self.errors.retain(|id, _| ids.contains(id));
        let pending = self.pending(blocks);
        // A request that just arrived opens when none is open; one the person closed stays closed.
        let arrived = pending
            .iter()
            .filter_map(|block| tool_request(block))
            .find(|id| !self.seen.contains(*id))
            .map(str::to_owned);
        self.seen = pending
            .iter()
            .filter_map(|block| tool_request(block))
            .map(str::to_owned)
            .collect();
        if self.initialized
            && self.open.is_none()
            && let Some(id) = arrived
        {
            self.open = Some(id);
        } else if !self.initialized
            || self
                .open
                .as_deref()
                .is_some_and(|id| !pending.iter().any(|block| tool_request(block) == Some(id)))
        {
            self.open = pending.first().and_then(|block| tool_request(block)).map(str::to_owned);
        }
        self.initialized = !pending.is_empty();
        if self.open != before {
            self.noting = false;
        }
        if !self.initialized {
            self.reviewing = false;
            self.keep_focus = false;
        }
    }

    fn begin(&mut self, blocks: &[Block], id: &str) -> bool {
        if !self.pending(blocks).iter().any(|block| tool_request(block) == Some(id)) {
            return false;
        }
        if !self.busy.insert(id.to_string()) {
            return false;
        }
        self.errors.remove(id);
        true
    }

    fn finish(&mut self, id: String, result: Result<(), String>) {
        self.keep_focus = true;
        self.busy.remove(&id);
        match result {
            Ok(()) => {
                self.settled.insert(id);
                self.noting = false;
            }
            Err(error) => {
                self.errors.insert(id, error);
            }
        }
    }
}

/// Exact command text, including compound and multiline commands. Generic tools retain their full input.
pub(super) fn payload(tool: &str, input: &Value) -> String {
    for key in ["command", "cmd"] {
        if let Some(command) = input.get(key).and_then(Value::as_str) {
            return command.to_string();
        }
    }
    if tool == "exec_command"
        && let Some(command) = input.as_str()
    {
        return command.to_string();
    }
    serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string())
}

pub(super) fn directory(input: &Value) -> Option<&str> {
    ["cwd", "workdir", "working_directory"]
        .into_iter()
        .find_map(|key| input.get(key).and_then(Value::as_str))
}

/// The tray's title: "1 command needs permission", "3 requests need permission".
pub(super) fn needs_label(pending: &[&Block]) -> String {
    let count = pending.len();
    let commands = pending.iter().all(|block| {
        matches!(block,
        Block::Approval { input, .. } if input.get("command").is_some() || input.get("cmd").is_some())
    });
    match (commands, count) {
        (true, 1) => "1 command needs permission".into(),
        (true, _) => format!("{count} commands need permission"),
        (false, 1) => "1 request needs permission".into(),
        (false, _) => format!("{count} requests need permission"),
    }
}

/// The primary dot before the tray's title.
pub(super) fn needs_dot(palette: Palette) -> Div {
    div()
        .size(px(14.))
        .flex_none()
        .rounded_full()
        .bg(palette.primary.opacity(0.14))
        .flex()
        .items_center()
        .justify_center()
        .child(div().size(px(8.)).rounded_full().bg(palette.primary))
}

/// A request's row: its chevron, its tool's icon, what it runs in mono, and "Sending…" while it is answered. The open
/// request's row is the card's title instead: the agent's mark and what it wants (`title`); its command shows below.
pub(super) fn request_head(
    request_id: &str,
    tool: &str,
    summary: &str,
    title: Option<(AnyElement, String)>,
    open: bool,
    busy: bool,
    palette: Palette,
) -> Stateful<Div> {
    let (mark, label) = match title {
        Some((mark, title)) => (
            mark,
            div().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child(title),
        ),
        None => (
            glyph(super::code_view::tool_icon(tool), palette.icon).into_any_element(),
            div()
                .font_family("Geist Mono")
                .text_size(px(13.))
                .child(if summary.is_empty() {
                    tool.to_string()
                } else {
                    summary.to_string()
                }),
        ),
    };
    h_flex()
        .id(SharedString::from(format!("permission-{request_id}")))
        .min_h(px(32.))
        .px(px(6.))
        .rounded(px(6.))
        .hover(|style| style.bg(palette.hover))
        .gap(px(8.))
        .cursor_pointer()
        .child(glyph(
            if open {
                Lucide::ChevronDown
            } else {
                Lucide::ChevronRight
            },
            palette.faint,
        ))
        .child(mark)
        .child(label.flex_1().min_w(px(0.)).truncate())
        .when(busy, |row| {
            row.child(div().text_size(px(12.)).text_color(palette.muted).child("Sending…"))
        })
}

/// The folder a request runs in, under its row.
pub(super) fn request_folder(request_id: &str, cwd: String, palette: Palette) -> Div {
    div()
        .pl(px(50.))
        .mt(px(-6.))
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(palette.faint)
        .child(MessageText::new(
            SharedString::from(format!("permission-folder-{request_id}")),
            cwd,
        ))
}

/// Why the agent asks, in its own words.
pub(super) fn request_reason(request_id: &str, reason: String, palette: Palette) -> Div {
    div()
        .px(px(6.))
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(palette.muted)
        .child(MessageText::new(
            SharedString::from(format!("permission-reason-{request_id}")),
            reason,
        ))
}

/// The exact command, or the full request, in mono, with its copy button at the right.
pub(super) fn exact_request(request_id: &str, exact: String, palette: Palette) -> Div {
    let copy = exact.clone();
    h_flex()
        .items_start()
        .py(px(8.))
        .px(px(12.))
        .gap(px(8.))
        .rounded(px(6.))
        .bg(palette.sidebar)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(13.))
                .line_height(px(20.))
                .font_family("Geist Mono")
                .child(MessageText::new(
                    SharedString::from(format!("permission-exact-{request_id}")),
                    exact,
                )),
        )
        .child(
            div()
                .id(SharedString::from(format!("permission-copy-{request_id}")))
                .flex_none()
                .pt(px(2.))
                .cursor_pointer()
                .child(glyph(Lucide::Copy, palette.icon))
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))),
        )
}

/// Why the last answer to a request failed.
pub(super) fn request_error(request_id: &str, error: String, palette: Palette) -> Div {
    div()
        .text_size(px(12.))
        .text_color(palette.destructive)
        .child(MessageText::new(
            SharedString::from(format!("permission-error-{request_id}")),
            error,
        ))
}

/// The row that holds a request's decisions, wrapping when the pane is narrow (the side chat's card).
pub(super) fn request_actions() -> Div {
    h_flex().max_w(relative(1.)).flex_wrap().justify_end().gap(px(8.))
}

/// The open request's summary over the tray's footer and its decisions (`request_actions`), in the side chat.
pub(super) fn request_footer(summary: String, actions: Div, palette: Palette) -> Div {
    v_flex()
        .debug_selector(|| "permission-actions".into())
        .flex_none()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(palette.muted)
                .truncate()
                .child(summary),
        )
        .child(permission_footer("Allows this exact request once.", actions, palette))
}

/// The card's decisions in the canon's order (Allow, Allow for this session, Deny), wrapping when the pane is narrow.
fn card_actions() -> Div {
    h_flex().max_w(relative(1.)).flex_wrap().gap(px(8.))
}

/// The tray's footer: the open request's location line, then its decisions (`card_actions`).
fn card_footer(place: String, actions: Div, palette: Palette) -> Div {
    v_flex()
        .debug_selector(|| "permission-actions".into())
        .flex_none()
        .gap(px(8.))
        .pt(px(12.))
        .border_t_1()
        .border_color(palette.border)
        .child(
            div()
                .text_size(px(12.))
                .text_color(palette.muted)
                .truncate()
                .child(place),
        )
        .child(actions)
}

/// Allow with its key: Enter on the focused button allows (`approval_keys.rs`).
fn allow_button(id: SharedString, enabled: bool, palette: Palette) -> Stateful<Div> {
    permission_button(id, ALLOW, true, enabled, palette).gap(px(6.)).child(
        div()
            .text_size(px(11.))
            .text_color(palette.primary_foreground.opacity(0.7))
            .child("↵"),
    )
}

/// A rule Allow always… offers: what it allows, where, and its button.
pub(super) fn remember_rule(
    title: String,
    scope: impl Into<SharedString>,
    button: impl IntoElement,
    palette: Palette,
) -> Div {
    let scope = scope.into();
    v_flex()
        .gap(px(8.))
        .py(px(8.))
        .px(px(12.))
        .rounded(px(6.))
        .bg(palette.sidebar)
        .child(div().text_size(px(13.)).child(title))
        .child(div().text_size(px(12.)).text_color(palette.muted).child(scope))
        .child(button)
}

/// The tray's header: its title, and Review or Close beside it.
pub(super) fn needs_header(
    toggle: Stateful<Div>,
    shown: bool,
    label: String,
    review: Option<Stateful<Div>>,
    palette: Palette,
) -> Div {
    h_flex()
        .w_full()
        .gap(px(8.))
        .child(
            toggle
                .flex_1()
                .min_w(px(0.))
                .h(px(24.))
                .gap(px(8.))
                .cursor_pointer()
                .child(glyph(
                    if shown {
                        Lucide::ChevronDown
                    } else {
                        Lucide::ChevronRight
                    },
                    palette.icon,
                ))
                .child(needs_dot(palette))
                .child(
                    div()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                ),
        )
        .children(review)
}

/// Allow and Deny on a collapsed request's row, so it is answered without opening it. The row shows what it runs.
fn quick_actions(request_id: &str, busy: bool, palette: Palette, cx: &mut Context<Workspace>) -> Div {
    let deny = request_id.to_string();
    let allow = request_id.to_string();
    let (deny_selector, allow_selector) = (
        format!("permission-quick-deny-{request_id}"),
        format!("permission-quick-allow-{request_id}"),
    );
    h_flex()
        .flex_none()
        .gap(px(6.))
        .child(
            permission_button(
                SharedString::from(format!("permission-quick-allow-{request_id}")),
                ALLOW,
                true,
                !busy,
                palette,
            )
            .h(px(26.))
            .debug_selector(move || allow_selector)
            .when(!busy, |button| {
                button.on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.answer_tool_request(allow.clone(), Decision::Allow, None, cx)
                }))
            }),
        )
        .child(
            permission_button(
                SharedString::from(format!("permission-quick-deny-{request_id}")),
                DENY,
                false,
                !busy,
                palette,
            )
            .h(px(26.))
            .debug_selector(move || deny_selector)
            .when(!busy, |button| {
                button.on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.answer_tool_request(deny.clone(), Decision::Deny, None, cx)
                }))
            }),
        )
}

impl Workspace {
    fn toggle_permission_group(&mut self, review: bool, small: bool, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        let queue = &mut view.dock.permissions;
        if queue.reviewing {
            queue.reviewing = false;
        } else if review {
            queue.collapsed = false;
            queue.open = queue
                .pending(&view.blocks)
                .first()
                .and_then(|block| tool_request(block))
                .map(str::to_owned);
            queue.reviewing = small;
            queue.scroll.set_offset(point(px(0.), px(0.)));
        } else {
            queue.collapsed = !queue.collapsed;
        }
        view.dock.menu = None;
        view.dock.permissions.keep_focus = true;
        view.dock.keys.card.focus(window, cx);
        cx.notify();
    }

    pub(super) fn toggle_permission(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        if !view.blocks.iter().any(|block| tool_request(block) == Some(id.as_str())) {
            return;
        }
        view.dock.permissions.keep_focus = true;
        view.dock.keys.card.focus(window, cx);
        view.dock.permissions.open = (view.dock.permissions.open.as_ref() != Some(&id)).then_some(id);
        view.dock.permissions.noting = false;
        view.dock.menu = None;
        view.dock.reason.update(cx, |state, cx| state.set_value("", window, cx));
        let pending = view.dock.permissions.current(&view.blocks);
        view.dock.follow(pending, cx);
        cx.notify();
    }

    /// Where the open session runs, for the card's location line.
    fn approval_place(&self, thread: &str) -> RunsOn {
        let machines = crate::machines::get();
        match machines.machine_of(thread) {
            None => RunsOn::Here,
            Some(machine) if machines.is_cloud(&machine) => RunsOn::Cloud,
            Some(machine) => machines
                .servers()
                .into_iter()
                .find(|server| server.id == machine)
                .map_or(RunsOn::Here, |server| RunsOn::Server(server.name.clone())),
        }
    }

    /// "Deny with a reason…": the reason field shows under the open request and takes the keys; Enter in it denies.
    pub(super) fn open_deny_reason(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        view.dock.permissions.noting = true;
        view.dock.permissions.keep_focus = true;
        view.dock.menu = None;
        let reason = view.dock.reason.clone();
        reason.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    /// The reason field closes, empty, and the keys go back to the card.
    pub(super) fn close_deny_reason(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else { return };
        view.dock.permissions.noting = false;
        let (reason, card) = (view.dock.reason.clone(), view.dock.keys.card.clone());
        reason.update(cx, |state, cx| state.set_value("", window, cx));
        window.focus(&card, cx);
        cx.notify();
    }

    pub(super) fn answer_tool_request(
        &mut self,
        id: String,
        decision: Decision,
        suggestion: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.answer_tool_request_with_message(id, decision, suggestion, None, cx);
    }

    pub(super) fn answer_tool_request_with_message(
        &mut self,
        id: String,
        decision: Decision,
        suggestion: Option<usize>,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let View::Code(view) = &self.view else { return };
        let thread = view.id.clone();
        let Some(runs) = self.runs(&thread) else { return };
        let View::Code(view) = &mut self.view else { return };
        if !view.dock.permissions.begin(&view.blocks, &id) {
            return;
        }
        view.dock.menu = None;
        let result = runs.call(
            "wb_approve",
            json!({
                "threadId": thread, "requestId": id, "decision": decision,
                "suggestion": suggestion, "message": message, "answers": null,
            }),
        );
        cx.spawn(async move |this, cx| {
            let result = result
                .await
                .unwrap_or_else(|_| Err("The session runner stopped.".into()))
                .map(|_| ());
            let _ = this.update(cx, |this, cx| {
                if let View::Code(view) = &mut this.view
                    && view.id == thread
                {
                    view.dock.permissions.finish(id, result);
                    view.dock.permissions.follow(&view.blocks);
                    let pending = view.dock.permissions.current(&view.blocks);
                    view.dock.follow(pending, cx);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_permission_queue(
        &self,
        view: &CodeView,
        palette: Palette,
        limit: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pending = view.dock.permissions.pending(&view.blocks);
        if pending.is_empty() {
            return None;
        }
        let label = needs_label(&pending);
        let compact = limit < crate::command_tray::REVIEW_MIN && !view.dock.permissions.reviewing;
        view.dock.permissions.compact.set(compact);
        let shown = !view.dock.permissions.collapsed && !compact;
        let agent = self.code_session().map(|session| session.agent);
        let agent_label = agent.map_or("The agent", super::code_title::code_meter::agent_name);
        let mut footer = None;
        let rows: Vec<_> = pending
            .into_iter()
            .map(|block| {
                let Block::Approval {
                    request_id,
                    tool,
                    summary,
                    input,
                    description,
                    suggestions,
                    ..
                } = block
                else {
                    unreachable!()
                };
                let id = request_id.clone();
                let open = view.dock.permissions.open.as_ref() == Some(request_id);
                let busy = view.dock.permissions.busy.contains(request_id);
                let title = open.then(|| {
                    let mark = match agent {
                        Some(agent) => super::model_menu::maker_mark(super::new_code::agent_maker(agent), 14., palette),
                        None => glyph(super::code_view::tool_icon(tool), palette.icon).into_any_element(),
                    };
                    (mark, ask_title(agent_label, tool))
                });
                let header = request_head(request_id, tool, summary, title, open, busy, palette)
                    .on_click(cx.listener(move |this, _, window, cx| this.toggle_permission(id.clone(), window, cx)))
                    .when(!open, |head| head.child(quick_actions(request_id, busy, palette, cx)));
                let cwd = directory(input)
                    .map(str::to_owned)
                    .unwrap_or_else(|| self.agents_folder_label());
                let mut row = v_flex().w_full().gap(px(6.)).child(header);
                if !open {
                    row = row.child(request_folder(request_id, cwd, palette));
                } else {
                    let allow = request_id.clone();
                    let session = request_id.clone();
                    let deny = request_id.clone();
                    let rules = describe_all(suggestions);
                    let for_session = session_suggestion(suggestions);
                    let noting = view.dock.permissions.noting;
                    row = row
                        .children(
                            description
                                .as_ref()
                                .map(|reason| request_reason(request_id, reason.clone(), palette)),
                        )
                        .child(exact_request(request_id, payload(tool, input), palette))
                        .child(request_folder(request_id, cwd, palette).pl(px(6.)).mt(px(0.)))
                        .children(
                            view.dock
                                .permissions
                                .errors
                                .get(request_id)
                                .map(|error| request_error(request_id, error.clone(), palette)),
                        )
                        .when(noting, |row| row.child(reason_field(&view.dock.reason, palette)))
                        // The rules menu and the reason are second steps, on one quiet line under the request.
                        .child(
                            h_flex()
                                .gap(px(4.))
                                .when(!rules.is_empty(), |line| {
                                    line.child(
                                        text_button(
                                            SharedString::from(format!("permission-always-{request_id}")),
                                            ALLOW_ALWAYS,
                                            false,
                                            palette,
                                        )
                                        .when(!busy, |button| {
                                            button.on_click(
                                                cx.listener(|this, _, _, cx| this.toggle_dock_menu(Menu::Always, cx)),
                                            )
                                        }),
                                    )
                                })
                                .when(!noting, |line| {
                                    line.child(
                                        text_button(
                                            SharedString::from(format!("permission-reason-open-{request_id}")),
                                            DENY_WITH_REASON,
                                            false,
                                            palette,
                                        )
                                        .when(!busy, |button| {
                                            button.on_click(
                                                cx.listener(|this, _, window, cx| this.open_deny_reason(window, cx)),
                                            )
                                        }),
                                    )
                                }),
                        );
                    footer = Some(
                        card_footer(
                            place_line(&self.approval_place(&view.id)),
                            card_actions()
                                .child(
                                    allow_button(
                                        SharedString::from(format!("permission-allow-{request_id}")),
                                        !busy,
                                        palette,
                                    )
                                    .track_focus(&view.dock.keys.allow)
                                    .when(!busy, |button| {
                                        button.on_click(cx.listener(move |this, _, _, cx| {
                                            this.answer_tool_request(allow.clone(), Decision::Allow, None, cx)
                                        }))
                                    }),
                                )
                                .when_some(for_session, |actions, index| {
                                    actions.child(
                                        permission_button(
                                            SharedString::from(format!("permission-session-{request_id}")),
                                            ALLOW_SESSION,
                                            false,
                                            !busy,
                                            palette,
                                        )
                                        .when(!busy, |button| {
                                            button.on_click(cx.listener(move |this, _, _, cx| {
                                                this.answer_tool_request(
                                                    session.clone(),
                                                    Decision::AllowSession,
                                                    Some(index),
                                                    cx,
                                                )
                                            }))
                                        }),
                                    )
                                })
                                .child(
                                    ghost_button(
                                        SharedString::from(format!("permission-deny-{request_id}")),
                                        DENY,
                                        palette,
                                    )
                                    .h(px(32.))
                                    .when(!busy, |button| {
                                        button.on_click(cx.listener(move |this, _, window, cx| {
                                            let message = if let View::Code(view) = &mut this.view {
                                                let message = view.dock.reason.read(cx).value().trim().to_string();
                                                view.dock
                                                    .reason
                                                    .update(cx, |state, cx| state.set_value("", window, cx));
                                                Some(message).filter(|message| !message.is_empty())
                                            } else {
                                                None
                                            };
                                            this.answer_tool_request_with_message(
                                                deny.clone(),
                                                Decision::Deny,
                                                None,
                                                message,
                                                cx,
                                            )
                                        }))
                                    }),
                                ),
                            palette,
                        )
                        .into_any_element(),
                    );
                    if view.dock.menu == Some(Menu::Always) {
                        for rule in rules {
                            let id = request_id.clone();
                            let title = match rule.code {
                                Some(code) => format!("{} {code}", rule.title),
                                None => rule.title,
                            };
                            row = row.child(remember_rule(
                                title,
                                rule.scope,
                                answer_button(
                                    SharedString::from(format!("permission-rule-{request_id}-{}", rule.index)),
                                    "Allow and remember",
                                    false,
                                    !busy,
                                    palette,
                                )
                                .rounded_full()
                                .when(!busy, |button| {
                                    button.on_click(cx.listener(move |this, _, _, cx| {
                                        this.answer_tool_request(
                                            id.clone(),
                                            Decision::AllowSession,
                                            Some(rule.index),
                                            cx,
                                        )
                                    }))
                                }),
                                palette,
                            ));
                        }
                    }
                }
                row
            })
            .collect();
        let body = v_flex()
            .id("permission-queue")
            .track_scroll(&view.dock.permissions.scroll)
            .w_full()
            .min_h(px(0.))
            .max_h(px((limit - 112.).max(0.)))
            .overflow_y_scroll()
            .gap(px(4.));
        let body = if shown { body.children(rows) } else { body };
        let toggle = crate::pages::ui::focusable(h_flex().id("permission-group-toggle"), palette).on_click(
            cx.listener(move |this, _, window, cx| this.toggle_permission_group(compact, compact, window, cx)),
        );
        let review = (!shown || view.dock.permissions.reviewing).then(|| {
            let closing = view.dock.permissions.reviewing;
            crate::pages::ui::ghost_button(
                "permission-group-review",
                if closing { "Close" } else { "Review" },
                palette,
            )
            .on_click(cx.listener(move |this, _, window, cx| this.toggle_permission_group(true, compact, window, cx)))
        });
        let header = needs_header(toggle, shown, label, review, palette);
        let footer = if shown { footer } else { None };
        let body = shown.then(|| body.into_any_element());
        Some(
            div()
                .track_focus(&view.dock.keys.card)
                .child(crate::command_tray::frame(
                    "permission-tray",
                    limit,
                    header.into_any_element(),
                    body,
                    footer,
                    palette,
                ))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    fn request(id: &str, command: &str) -> Block {
        Block::Approval {
            request_id: id.into(),
            tool: "Bash".into(),
            summary: command.into(),
            description: None,
            input: json!({"command":command,"cwd":"/work"}),
            suggestions: vec![],
            answer: None,
            moved: false,
            message: None,
        }
    }
    #[test]
    fn decisions_remain_bound_to_the_original_request() {
        let mut queue = PermissionQueue::default();
        let blocks = vec![request("one", "echo one"), request("two", "echo two")];
        queue.follow(&blocks);
        assert_eq!(queue.pending(&blocks).len(), 2);
        assert!(queue.begin(&blocks, "one"));
        assert!(!queue.begin(&blocks, "one"));
        assert!(!queue.begin(&blocks, "missing"));
        queue.finish("one".into(), Ok(()));
        queue.follow(&blocks);
        assert_eq!(queue.pending(&blocks).len(), 1);
        assert_eq!(queue.open.as_deref(), Some("two"));
        assert!(!queue.begin(&blocks, "one"));
    }
    #[test]
    fn failed_and_cancelled_requests_do_not_resolve_other_rows() {
        let mut queue = PermissionQueue::default();
        let mut blocks = vec![request("one", "echo one"), request("two", "echo two")];
        queue.follow(&blocks);
        assert!(queue.begin(&blocks, "one"));
        queue.finish("one".into(), Err("offline".into()));
        assert!(queue.begin(&blocks, "one"));
        if let Block::Approval { answer, .. } = &mut blocks[0] {
            *answer = Some(Decision::Deny)
        }
        queue.follow(&blocks);
        assert!(!queue.begin(&blocks, "one"));
        assert_eq!(queue.open.as_deref(), Some("two"));
    }
    #[test]
    fn questions_and_plans_stay_outside_the_tool_queue() {
        let mut question = request("question", "");
        if let Block::Approval { tool, .. } = &mut question {
            *tool = QUESTION_TOOL.into();
        }
        let mut plan = request("plan", "");
        if let Block::Approval { tool, .. } = &mut plan {
            *tool = PLAN_TOOL.into();
        }
        let blocks = vec![request("command", "echo hello"), question, plan];
        let mut queue = PermissionQueue::default();
        queue.follow(&blocks);
        assert_eq!(queue.pending(&blocks).len(), 1);
        assert_eq!(queue.open.as_deref(), Some("command"));
        assert!(matches!(queue.current(&blocks), Some(Block::Approval { tool, .. }) if tool == PLAN_TOOL));
        assert!(matches!(queue.current(&blocks[..2]), Some(Block::Approval { tool, .. }) if tool == QUESTION_TOOL));
        assert!(
            matches!(queue.current(&blocks[..1]), Some(Block::Approval { request_id, .. }) if request_id == "command")
        );
        queue.open = None;
        queue.follow(&blocks);
        assert!(queue.selected(&blocks).is_none(), "a collapsed request stays collapsed");
    }

    #[test]
    fn a_new_request_opens_when_none_is_open() {
        let mut queue = PermissionQueue::default();
        let mut blocks = vec![request("one", "echo one")];
        queue.follow(&blocks);
        queue.open = None;
        queue.follow(&blocks);
        assert_eq!(queue.open, None, "a request the person closed stays closed");
        blocks.push(request("two", "echo two"));
        queue.follow(&blocks);
        assert_eq!(queue.open.as_deref(), Some("two"));
    }

    #[test]
    fn the_card_says_who_asks_what_and_where() {
        assert_eq!(ask_title("Claude Code", "Bash"), "Claude Code wants to run a command");
        assert_eq!(ask_title("Codex", "Edit"), "Codex wants to edit a file");
        assert_eq!(place_line(&RunsOn::Here), "On this computer");
        assert_eq!(place_line(&RunsOn::Cloud), "On a cloud machine");
        assert_eq!(place_line(&RunsOn::Server("build-box".into())), "On build-box");
        assert_eq!(
            (ALLOW, ALLOW_SESSION, DENY),
            ("Allow", "Allow for this session", "Deny")
        );
    }

    #[test]
    fn allow_for_this_session_keeps_only_a_rule_scoped_to_the_session() {
        let suggestion = |raw: Value| PermissionSuggestion {
            kind: String::new(),
            label: String::new(),
            raw,
        };
        let project = suggestion(
            json!({"type":"addRules","destination":"localSettings","behavior":"allow",
            "rules":[{"toolName":"Bash","ruleContent":"npm test:*"}]}),
        );
        let edits = suggestion(json!({"type":"setMode","mode":"acceptEdits","destination":"session"}));
        let deny = suggestion(json!({"type":"addRules","destination":"session","behavior":"deny"}));
        // A rule saved past the session never stands in: no button.
        assert_eq!(session_suggestion(&[project.clone()]), None);
        assert_eq!(session_suggestion(&[project.clone(), deny.clone(), edits]), Some(2));
        assert_eq!(
            session_suggestion(&[deny, project, suggestion(json!("acceptForSession"))]),
            Some(2)
        );
    }

    #[test]
    fn compound_commands_keep_their_full_payload_and_directory() {
        let input = json!({"command":"echo first && echo second\nprintf third","workdir":"/work/other"});
        assert_eq!(payload("Bash", &input), "echo first && echo second\nprintf third");
        assert_eq!(directory(&input), Some("/work/other"));
        assert_eq!(
            payload("Edit", &json!({"new_string":"entire text"})),
            "{\n  \"new_string\": \"entire text\"\n}"
        );
    }
    #[test]
    fn collapsed_group_preserves_requests_without_exposing_hidden_approval_keys() {
        let mut queue = PermissionQueue::default();
        let mut blocks = vec![request("one", "echo one")];
        queue.follow(&blocks);
        queue.collapsed = true;
        blocks.push(request("two", "echo two"));
        queue.follow(&blocks);
        assert_eq!(queue.pending(&blocks).len(), 2);
        assert!(queue.current(&blocks).is_none());
        assert_eq!(queue.open.as_deref(), Some("one"));
        queue.collapsed = false;
        queue.compact.set(true);
        assert!(
            queue.current(&blocks).is_none(),
            "short panes must not approve hidden requests"
        );
        queue.reviewing = true;
        assert!(
            queue.current(&blocks).is_some(),
            "full-height review restores the existing request"
        );
    }

    #[test]
    fn settling_a_request_preserves_group_collapse_and_does_not_arm_next_focus() {
        let mut queue = PermissionQueue::default();
        let blocks = vec![request("one", "echo one"), request("two", "echo two")];
        queue.follow(&blocks);
        queue.collapsed = true;
        queue.finish("one".into(), Ok(()));
        queue.follow(&blocks);
        assert!(queue.collapsed);
        assert!(queue.keep_focus);
        assert_eq!(queue.pending(&blocks).len(), 1);
        assert_eq!(queue.open.as_deref(), Some("two"));
        assert!(queue.selected(&blocks).is_none());
    }
}

#[cfg(test)]
mod view_tests;
