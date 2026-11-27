//! A Code session's Connection details in the Workbench, as the Nexus app's (`connection-view.tsx` `PaneConnection`,
//! the side pane's "session" view, opened by /status). It takes the tools' place, with the way back to them. The words
//! and rules are `nexus_workbench::connection_view`'s, shared with the Nexus app. While open it asks the live process
//! again every 15 s (`wb_connection`), where the host offers it.

use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::connection_view as words;
use nexus_workbench::event::{SessionConnection, StoredEvent, ThreadSummary};
use serde_json::{Value, json};

use super::super::slash::Action;
use super::super::{View, Workspace, bar_button, glyph};
use crate::code::CodeStore;
use crate::palette::Palette;

/// The pane's state for one session.
pub(in crate::workspace) struct ConnectionPane {
    pub id: String,
    summary: Option<ThreadSummary>,
    /// The latest page of the session's log with the events carried past it: the last reported connection.
    events: Vec<StoredEvent>,
    problem: Option<SharedString>,
    loaded: bool,
    /// The read under way, then the wait for the next; dropping the pane cancels both.
    reading: Option<Task<()>>,
}

impl ConnectionPane {
    fn new(id: String) -> ConnectionPane {
        ConnectionPane {
            id,
            summary: None,
            events: Vec::new(),
            problem: None,
            loaded: false,
            reading: None,
        }
    }
}

/// What one read brought: the session, its last page, and how asking the live process went (None: not asked).
type Read = Result<(Option<ThreadSummary>, Vec<StoredEvent>, Option<Result<(), String>>), String>;

fn events_of(page: &Value) -> Vec<StoredEvent> {
    ["carried", "events"]
        .into_iter()
        .flat_map(|key| serde_json::from_value::<Vec<StoredEvent>>(page[key].clone()).unwrap_or_default())
        .collect()
}

impl Workspace {
    /// /status: the open session's Connection details in the Workbench, shown and read.
    pub(in crate::workspace) fn show_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let View::Code(view) = &self.view else {
            self.composer_note = Some("Start a session first.".into());
            cx.notify();
            return;
        };
        let id = view.id.clone();
        self.sync_side(window, cx);
        self.side.open = true;
        self.side.side_chat_visible = false;
        if self.side.connection.as_ref().is_none_or(|pane| pane.id != id) {
            self.side.connection = Some(ConnectionPane::new(id));
        }
        self.read_connection(cx);
        cx.notify();
    }

    /// Back to the tools.
    fn close_connection(&mut self, cx: &mut Context<Self>) {
        self.side.connection = None;
        cx.notify();
    }

    /// The pane's session, while it is the open one.
    fn connection_shown(&self) -> Option<&ConnectionPane> {
        let View::Code(view) = &self.view else {
            return None;
        };
        self.side.connection.as_ref().filter(|pane| pane.id == view.id)
    }

    /// Where the session runs, as the pane names it, and whether its process answers `wb_connection` there.
    fn connection_host(&self, id: &str) -> (words::Host, bool) {
        let machines = crate::machines::get();
        match machines.machine_of(id) {
            None => (words::execution_host(None), self.runner.is_some()),
            Some(machine) => {
                let name = self
                    .servers
                    .iter()
                    .find(|server| server.id == machine)
                    .map(|server| server.name.clone())
                    .or_else(|| {
                        self.cloud_machines
                            .iter()
                            .find(|cloud| cloud.id == machine)
                            .map(|cloud| cloud.name.clone())
                    });
                let connected_as = machines.connected_as(&machine);
                let host = words::execution_host(Some(words::Machine {
                    name: name.as_deref(),
                    connected_as: connected_as.as_deref(),
                }));
                let supported = machines.has_feature(&machine, nexus_workbench::connection::FEATURE);
                (host, supported)
            }
        }
    }

    /// Read the session and its last reported connection, and ask the live process again where it can answer. Then
    /// wait and read again, while the pane stays open on it.
    fn read_connection(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.side.connection.as_ref().map(|pane| pane.id.clone()) else {
            return;
        };
        let (host, supported) = self.connection_host(&id);
        let machine = crate::machines::get().machine_of(&id);
        let runs = self.runs(&id);
        let local = self.code.as_ref().map(CodeStore::reader);
        // The calls answer on the runtime; only the store's scan runs on the background executor.
        let local = runs.is_none().then(|| {
            let id = id.clone();
            cx.background_spawn(async move {
                let reader = local.ok_or("No session store on this computer.")?;
                reader.connection_read(&id)
            })
        });
        let request = id.clone();
        let read = async move {
            let id = request;
            let Some(runs) = runs else {
                let (summary, events) = match local {
                    Some(read) => read.await?,
                    None => return Err("No session store on this computer.".to_string()),
                };
                return Ok((summary, events, None));
            };
            let lost = |_| "The answer was lost.".to_string();
            let summary = match &machine {
                Some(machine) => crate::machines::get().thread(machine, &id),
                None => {
                    let overview = runs.call("wb_overview", json!({})).await.map_err(lost)??;
                    serde_json::from_value::<Vec<ThreadSummary>>(overview["threads"].clone())
                        .map_err(|error| error.to_string())?
                        .into_iter()
                        .find(|summary| summary.id == id)
                }
            };
            let page = runs
                .call("wb_thread_page", json!({ "threadId": id, "limit": 1 }))
                .await
                .map_err(lost)??;
            let events = events_of(&page);
            let generation = summary
                .as_ref()
                .and_then(|summary| summary.capabilities.as_ref()?.connection.as_ref())
                .map(|connection| connection.generation.clone());
            // As the Nexus app: only a live process on a connected host that offers it is asked.
            let asked = if supported && generation.is_some() && host.connected {
                Some(
                    runs.call("wb_connection", json!({ "threadId": id }))
                        .await
                        .map_err(lost)
                        .and_then(|answer| answer)
                        .and_then(|value| {
                            serde_json::from_value::<SessionConnection>(value).map_err(|error| error.to_string())
                        })
                        .map(|fresh| (summary.clone(), fresh)),
                )
            } else {
                None
            };
            let (summary, asked) = match asked {
                Some(Ok((Some(mut summary), fresh))) => {
                    // The fresh observation is the live process's own.
                    if let Some(caps) = summary.capabilities.as_mut() {
                        caps.connection = Some(Box::new(fresh));
                    }
                    (Some(summary), Some(Ok(())))
                }
                Some(Ok((None, _))) => (None, Some(Ok(()))),
                Some(Err(error)) => (summary, Some(Err(error))),
                None => (summary, None),
            };
            Ok((summary, events, asked))
        };
        let Some(pane) = self.side.connection.as_mut() else {
            return;
        };
        pane.reading = Some(cx.spawn(async move |this, cx| {
            let read: Read = read.await;
            let still = this
                .update(cx, |this, cx| {
                    let Some(pane) = this.side.connection.as_mut().filter(|pane| pane.id == id) else {
                        return false;
                    };
                    pane.loaded = true;
                    match read {
                        Ok((summary, events, asked)) => {
                            pane.summary = summary.or(pane.summary.take());
                            pane.events = events;
                            pane.problem = match asked {
                                Some(Err(_)) => Some(words::REFRESH_FAILED.into()),
                                _ => None,
                            };
                        }
                        Err(error) => pane.problem = Some(error.into()),
                    }
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !still {
                return;
            }
            cx.background_executor()
                .timer(Duration::from_secs(words::REFRESH_SECS))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.side.connection.as_ref().is_some_and(|pane| pane.id == id) {
                    this.read_connection(cx);
                }
            });
        }));
    }

    /// Restart agent: end the process; the next message resumes the conversation (the Nexus app's /restart).
    fn restart_agent(&mut self, id: &str, cx: &mut Context<Self>) {
        match self.runs(id) {
            Some(runs) => {
                // It runs on the runtime whether or not its answer is read; the session's status shows the end.
                drop(runs.call("wb_stop", json!({ "threadId": id, "restart": true })));
            }
            None => self.refuse_read_only(),
        }
        cx.notify();
    }

    /// The pane's header in the tools' place: back to them, its title, and the Workbench's close.
    pub(in crate::workspace) fn render_connection_header(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.connection_shown()?;
        Some(
            h_flex()
                .flex_none()
                .h(px(40.))
                .pl(px(crate::side::INSET - 8.))
                .pr(px(6.))
                .gap(px(4.))
                .child(
                    h_flex()
                        .id("connection-back")
                        .h(px(28.))
                        .px(px(8.))
                        .gap(px(6.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_size(px(12.5))
                        .text_color(palette.muted)
                        .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                        .child(glyph(Lucide::ChevronLeft, palette.icon).size(px(14.)))
                        .child("Tools")
                        .on_click(cx.listener(|this, _, _, cx| this.close_connection(cx))),
                )
                .child(div().flex_1())
                .child(
                    bar_button("connection-close", Lucide::X, palette)
                        .flex_none()
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_side_pane(window, cx))),
                )
                .into_any_element(),
        )
    }

    /// The pane's body, while it shows the open session.
    pub(in crate::workspace) fn render_connection_body(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pane = self.connection_shown()?;
        let Some(thread) = pane.summary.as_ref() else {
            let text = match (&pane.problem, pane.loaded) {
                (Some(problem), _) => problem.clone(),
                (None, true) => "This session's details are unavailable.".into(),
                (None, false) => "Reading the connection…".into(),
            };
            return Some(crate::side::note(text, palette).into_any_element());
        };
        let (host, supported) = self.connection_host(&pane.id);
        let connection = words::last_connection(thread, &pane.events);
        let live = words::is_live(thread, connection, host.connected);
        let quiet = |text: SharedString| {
            div()
                .text_size(px(12.))
                .line_height(px(17.))
                .text_color(palette.muted)
                .child(text)
        };
        let heading = |title: &'static str, tag: Option<&'static str>| {
            h_flex()
                .pt(px(10.))
                .gap(px(6.))
                .items_baseline()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.foreground)
                        .child(title),
                )
                .children(tag.map(|tag| div().text_size(px(11.5)).text_color(palette.faint).child(tag)))
        };
        let list = |names: Vec<String>| {
            v_flex()
                .gap(px(2.))
                .text_size(px(12.5))
                .children(names.into_iter().map(|name| div().child(format!("· {name}"))))
        };
        let button = |id: &'static str, label: &'static str, enabled: bool| {
            div()
                .id(id)
                .flex_none()
                .h(px(28.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(8.))
                .bg(palette.hover)
                .text_size(px(12.5))
                .when(enabled, |button| {
                    button.cursor_pointer().hover(|style| style.bg(palette.selected))
                })
                .when(!enabled, |button| button.opacity(0.45))
                .child(label)
        };
        let facts = words::facts(thread, connection, &host);
        let id = pane.id.clone();
        let mut body = v_flex()
            .id("connection-pane")
            .size_full()
            .overflow_y_scroll()
            .px(px(crate::side::INSET))
            .pb(px(16.))
            .gap(px(8.))
            .text_size(px(13.))
            .text_color(palette.foreground)
            .child(heading("Connection", Some(words::state(live, connection))))
            .child(div().text_color(palette.muted).child(words::headline(thread, &host)));
        if !host.connected {
            body = body.child(quiet(words::HOST_DISCONNECTED.into()));
        }
        if let Some(problem) = &pane.problem {
            body = body.child(quiet(problem.clone()));
        }
        body = body.child(v_flex().gap(px(4.)).children(facts.into_iter().map(|(label, value)| {
            h_flex()
                .gap(px(12.))
                .items_start()
                .text_size(px(12.5))
                .child(div().w(px(160.)).flex_none().text_color(palette.muted).child(label))
                .child(div().flex_1().min_w(px(0.)).child(value))
        })));
        body = body.children(
            words::notes(thread, connection)
                .into_iter()
                .map(|note| quiet(note.into())),
        );
        body = body.child(
            h_flex()
                .flex_wrap()
                .gap(px(6.))
                .child(
                    button("connection-refresh", "Refresh connection", live && supported)
                        .when(live && supported, |button| {
                            button.on_click(cx.listener(|this, _, _, cx| this.read_connection(cx)))
                        }),
                )
                .child(
                    button("connection-mcp", "This computer's account and MCP settings", true)
                        .on_click(cx.listener(|this, _, window, cx| this.run_slash_action(Action::Mcp, window, cx))),
                )
                .child(
                    button("connection-restart", "Restart agent", true)
                        .on_click(cx.listener(move |this, _, _, cx| this.restart_agent(&id, cx))),
                ),
        );
        body = body.child(heading("MCP connections", Some(words::mcp_state(connection, live))));
        body = match (connection.and_then(|c| c.mcp.as_ref()), words::mcp_empty(connection)) {
            (Some(servers), None) => body.child(v_flex().gap(px(4.)).children(servers.iter().map(|server| {
                v_flex()
                    .text_size(px(12.5))
                    .child(format!("{} · {}", server.name, server.status))
                    .children(server.detail.clone().map(|detail| quiet(detail.into())))
            }))),
            (_, Some(empty)) => body.child(quiet(empty.into())),
            (None, None) => body,
        };
        for (index, (title, names)) in words::inventories(thread, connection).into_iter().enumerate() {
            body = body.child(heading(title, None));
            body = match names {
                Ok(names) => body.child(list(names)),
                Err(line) => body.child(quiet(line.into())),
            };
            body = body.children(match index {
                0 => Some(quiet(words::MCP_NOTE.into())),
                1 => Some(quiet(words::SKILLS_NOTE.into())),
                _ => None,
            });
        }
        Some(body.child(quiet(words::RESTART_NOTE.into())).into_any_element())
    }
}
