//! Settings › MCP servers (the Nexus app's `mcp-settings.tsx`): the servers every Code session gets, on top of the
//! agent's own, from the Nexus app's `settings.json` (`mcpServers`), as `src/mcp.rs` passes them to this build's
//! sessions. Each row says what is known without starting the server: Off, Needs a value (a variable or header with
//! nothing in the keychain), or Ready. Values are never read into the page, only which names have one.
//!
//! Two settings cards: Included with Nexus (the built-in server and the way to Connectors), then the person's servers,
//! where Edit opens the editor in the row's place and Remove sits in the editor's footer.
//!
//! Add, Edit, Remove and the switch save the list as the Nexus app does (`wb_mcp_servers_set`), while this build owns
//! the store: `nexus-desktop-mcp`'s `mcp::check` first, then the keychain entry with `mcp::merge` (an empty value
//! keeps the saved one), then `settings.json`. A change reaches a session the next time its agent starts.
//!
//! Under the list, while this build owns the store: Outside access, as the Nexus app has it on this page
//! (`outside_access.rs`).

use std::collections::BTreeSet;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputState, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::settings::Settings;

use super::desktop_app::{off_thread, read_only_note};
use super::{DeviceCtx, Load, palette, ui};
use crate::mcp::{McpServer, VAULT_ACCOUNT, Values};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

/// The server every session gets from the app itself, unless one of the person's has its name (`mcp::builtin`).
const BUILTIN: &str = "nexus";

pub fn build(ctx: DeviceCtx, _window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let view = cx.new(|cx| {
        let mut page = McpPage {
            ctx,
            servers: Load::Loading,
            editor: None,
            confirm_remove: None,
            saving: false,
            error: None,
            outside_problem: None,
        };
        page.load(cx);
        page
    });
    Some(view.into())
}

/// What a row says (`mcpStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Off,
    NeedsValue,
    Configured,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Off => "Off",
            Status::NeedsValue => "Needs a value",
            Status::Configured => "Ready",
        }
    }

    /// Its dot, as Outside access colours its own: mint when ready, amber for a value to add (a step to take, not an
    /// error), grey when off.
    fn color(self, palette: Palette) -> Hsla {
        match self {
            Status::Off => palette.icon,
            Status::NeedsValue => palette.waiting(),
            Status::Configured => Palette::success(),
        }
    }
}

/// A server and the names of its secrets that have a value saved.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    server: McpServer,
    saved: BTreeSet<String>,
}

impl Row {
    fn status(&self) -> Status {
        if !self.server.enabled {
            Status::Off
        } else if self.missing().is_empty() {
            Status::Configured
        } else {
            Status::NeedsValue
        }
    }

    fn missing(&self) -> Vec<&str> {
        self.server
            .secrets
            .iter()
            .filter(|secret| !self.saved.contains(*secret))
            .map(String::as_str)
            .collect()
    }

    /// The URL, or the command with its arguments.
    fn line(&self) -> String {
        match &self.server.url {
            Some(url) => url.clone(),
            None => std::iter::once(self.server.command.clone().unwrap_or_default())
                .chain(self.server.args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// Which names have a value, from the keychain entry's `{"<server>": {"<name>": "<value>"}}`: the values are dropped.
fn saved_names(values: &Values, server: &str) -> BTreeSet<String> {
    values
        .get(server)
        .map(|names| names.keys().cloned().collect())
        .unwrap_or_default()
}

fn rows(servers: Vec<McpServer>, values: &Values) -> Vec<Row> {
    servers
        .into_iter()
        .map(|server| Row {
            saved: saved_names(values, &server.name),
            server,
        })
        .collect()
}

/// The editor's fields, as text (`Draft` in `mcp-settings.tsx`).
#[derive(Debug, Clone, Default, PartialEq)]
struct Draft {
    remote: bool,
    name: String,
    command: String,
    /// One argument per line.
    args: String,
    url: String,
    /// Each variable's (or header's) name, and a value typed now; empty keeps the saved one.
    secrets: Vec<(String, String)>,
}

impl Draft {
    fn of(server: &McpServer) -> Draft {
        Draft {
            remote: server.url.is_some(),
            name: server.name.clone(),
            command: server.command.clone().unwrap_or_default(),
            args: server.args.join("\n"),
            url: server.url.clone().unwrap_or_default(),
            secrets: server
                .secrets
                .iter()
                .map(|name| (name.clone(), String::new()))
                .collect(),
        }
    }

    /// The server as it is saved (`serverOf`).
    fn server(&self, enabled: bool) -> McpServer {
        let secrets = self
            .secrets
            .iter()
            .map(|(name, _)| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();
        let name = self.name.trim().to_string();
        if self.remote {
            McpServer {
                name,
                enabled,
                command: None,
                args: Vec::new(),
                url: Some(self.url.trim().to_string()),
                secrets,
                tool_timeout_sec: None,
                plugin: None,
            }
        } else {
            McpServer {
                name,
                enabled,
                command: Some(self.command.trim().to_string()),
                args: self
                    .args
                    .lines()
                    .map(str::trim)
                    .filter(|arg| !arg.is_empty())
                    .map(String::from)
                    .collect(),
                url: None,
                secrets,
                tool_timeout_sec: None,
                plugin: None,
            }
        }
    }

    /// The values typed now, by this server's name.
    fn values(&self) -> Values {
        let typed: std::collections::BTreeMap<String, String> = self
            .secrets
            .iter()
            .filter(|(name, value)| !name.trim().is_empty() && !value.is_empty())
            .map(|(name, value)| (name.trim().to_string(), value.clone()))
            .collect();
        if typed.is_empty() {
            Values::new()
        } else {
            Values::from([(self.name.trim().to_string(), typed)])
        }
    }
}

/// The list with `server` put at `index`, or added at the end.
fn with_server(servers: &[McpServer], index: Option<usize>, server: McpServer) -> Vec<McpServer> {
    let mut servers = servers.to_vec();
    match index.filter(|index| *index < servers.len()) {
        Some(index) => servers[index] = server,
        None => servers.push(server),
    }
    servers
}

/// The list in the settings, checked first as the Nexus app checks it.
fn apply(settings: &mut Settings, servers: Vec<McpServer>) -> Result<(), String> {
    nexus_desktop_mcp::mcp::check(&servers)?;
    settings.mcp_servers = servers;
    Ok(())
}

/// Save the list and the values typed now (the Nexus app's `wb_mcp_servers_set`): checked, then the keychain entry,
/// then the file. Answers the rows as saved.
fn write(servers: Vec<McpServer>, given: Values) -> Result<Vec<Row>, String> {
    if !crate::desktop_settings::writable() {
        return Err(crate::desktop_settings::READ_ONLY.into());
    }
    nexus_desktop_mcp::mcp::check(&servers)?;
    let vault = crate::keychain::vault();
    let saved = crate::mcp::saved_values();
    let kept = nexus_desktop_mcp::mcp::merge(&servers, &saved, &given);
    if kept != saved {
        if kept.is_empty() {
            vault.delete(VAULT_ACCOUNT)?;
        } else {
            vault.set(VAULT_ACCOUNT, &serde_json::to_string(&kept).unwrap_or_default())?;
        }
    }
    let (settings, ()) = crate::desktop_settings::update(|settings| apply(settings, servers))?;
    Ok(rows(settings.mcp_servers, &kept))
}

/// The open editor: `index` None for a new server.
struct Editor {
    index: Option<usize>,
    enabled: bool,
    remote: bool,
    name: Entity<InputState>,
    command: Entity<InputState>,
    args: Entity<TextareaState>,
    url: Entity<InputState>,
    /// Each secret's name and value fields; `saved` says a value is in the keychain.
    secrets: Vec<(Entity<InputState>, Entity<InputState>, bool)>,
}

impl Editor {
    fn new(index: Option<usize>, row: Option<&Row>, window: &mut Window, cx: &mut App) -> Editor {
        let draft = row.map(|row| Draft::of(&row.server)).unwrap_or_default();
        let input = |value: &str, placeholder: &str, window: &mut Window, cx: &mut App| {
            let value = value.to_string();
            let placeholder = placeholder.to_string();
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(value, window, cx);
                state
            })
        };
        let args = {
            let value = draft.args.clone();
            cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .rows(3)
                    .placeholder("One argument per line");
                state.set_value(value, window, cx);
                state
            })
        };
        let mut editor = Editor {
            index,
            enabled: row.is_none_or(|row| row.server.enabled),
            remote: draft.remote,
            name: input(&draft.name, "github", window, cx),
            command: input(&draft.command, "npx", window, cx),
            args,
            url: input(&draft.url, "https://mcp.example.com/mcp", window, cx),
            secrets: Vec::new(),
        };
        for (name, _) in &draft.secrets {
            let saved = row.is_some_and(|row| row.saved.contains(name));
            editor.add_secret(name, saved, window, cx);
        }
        editor
    }

    fn add_secret(&mut self, name: &str, saved: bool, window: &mut Window, cx: &mut App) {
        let name_field = {
            let name = name.to_string();
            let placeholder = if self.remote { "Authorization" } else { "API_KEY" };
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(name, window, cx);
                state
            })
        };
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(if saved { "Saved; type to replace" } else { "Value" })
        });
        self.secrets.push((name_field, value, saved));
    }

    fn draft(&self, cx: &App) -> Draft {
        let text = |input: &Entity<InputState>| input.read(cx).value().to_string();
        Draft {
            remote: self.remote,
            name: text(&self.name),
            command: text(&self.command),
            args: self.args.read(cx).value().to_string(),
            url: text(&self.url),
            secrets: self
                .secrets
                .iter()
                .map(|(name, value, _)| (text(name), text(value)))
                .collect(),
        }
    }
}

struct McpPage {
    ctx: DeviceCtx,
    servers: Load<Vec<Row>>,
    editor: Option<Editor>,
    /// The server whose Remove was pressed once.
    confirm_remove: Option<String>,
    saving: bool,
    error: Option<SharedString>,
    /// Why outside access's switch or a Revoke failed (`outside_access.rs`).
    outside_problem: Option<SharedString>,
}

impl McpPage {
    fn load(&mut self, cx: &mut Context<Self>) {
        self.servers = Load::Loading;
        off_thread(
            cx,
            || {
                let servers = crate::desktop_settings::load().mcp_servers;
                // The keychain is read only when an enabled server has a secret (`mcp::values`).
                let values = crate::mcp::values(&servers);
                rows(servers, &values)
            },
            |this, rows, _| this.servers = Load::Ready(rows),
        );
    }

    fn current(&self) -> Vec<McpServer> {
        match &self.servers {
            Load::Ready(rows) => rows.iter().map(|row| row.server.clone()).collect(),
            _ => Vec::new(),
        }
    }

    /// Save `servers` and `given`, off the UI thread; the editor closes once saved.
    fn save(&mut self, servers: Vec<McpServer>, given: Values, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.saving = true;
        self.error = None;
        let saved = self.ctx.settings_saved.clone();
        off_thread(
            cx,
            move || write(servers, given),
            move |this, written, cx| {
                this.saving = false;
                this.confirm_remove = None;
                match written {
                    Ok(rows) => {
                        this.servers = Load::Ready(rows);
                        this.editor = None;
                        saved(&crate::desktop_settings::load(), cx);
                    }
                    Err(error) => this.error = Some(error.into()),
                }
            },
        );
        cx.notify();
    }

    fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut servers = self.current();
        if let Some(server) = servers.get_mut(index) {
            server.enabled = !server.enabled;
            self.save(servers, Values::new(), cx);
        }
    }

    fn remove(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut servers = self.current();
        let Some(name) = servers.get(index).map(|server| server.name.clone()) else {
            return;
        };
        if self.confirm_remove.as_deref() != Some(name.as_str()) {
            self.confirm_remove = Some(name);
            cx.notify();
            return;
        }
        servers.remove(index);
        self.save(servers, Values::new(), cx);
    }

    /// "Add it ›" on a server that needs a value: its editor opens with the caret in the first value not saved.
    fn add_value(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.edit(Some(index), window, cx);
        if let Some(editor) = &self.editor
            && let Some((_, value, _)) = editor.secrets.iter().find(|(_, _, saved)| !saved)
        {
            value.update(cx, |state, cx| state.focus(window, cx));
        }
    }

    fn edit(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let row = match (&self.servers, index) {
            (Load::Ready(rows), Some(index)) => rows.get(index).cloned(),
            _ => None,
        };
        self.error = None;
        self.confirm_remove = None;
        let editor = Editor::new(index, row.as_ref(), window, cx);
        // The editor opens ready for typing in its first field, the name.
        editor.name.update(cx, |state, cx| state.focus(window, cx));
        self.editor = Some(editor);
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else { return };
        let draft = editor.draft(cx);
        let servers = with_server(&self.current(), editor.index, draft.server(editor.enabled));
        self.save(servers, draft.values(), cx);
    }

    /// A server as a row of the Servers card: its mark, its name and kind over its command or URL, then its status, a
    /// quiet Edit and its switch. Remove is in the editor, so no row carries a red button.
    fn render_row(&self, index: usize, row: &Row, palette: Palette, cx: &mut Context<Self>) -> Div {
        let status = row.status();
        let remote = row.server.url.is_some();
        let missing = row.missing();
        let editable = crate::desktop_settings::writable() && !self.saving;
        let name = row.server.name.clone();
        let switch = ui::switch(
            SharedString::from(format!("mcp-on-{name}")),
            row.server.enabled,
            !editable,
            palette,
        );
        let switch = if editable {
            switch
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(index, cx)))
                .into_any_element()
        } else {
            switch.into_any_element()
        };
        let edit = editable.then(|| {
            ui::ghost_button(SharedString::from(format!("mcp-edit-{name}")), "Edit", palette)
                .on_click(cx.listener(move |this, _, window, cx| this.edit(Some(index), window, cx)))
        });
        // The command, or the URL; a server waiting on a value names it after.
        let mut line = h_flex()
            .min_w(px(0.))
            .gap(px(4.))
            .text_size(px(12.))
            .text_color(palette.faint)
            .child(
                div()
                    .min_w(px(0.))
                    .when(!remote, |line| line.font_family(ui::MONO))
                    .truncate()
                    .child(row.line()),
            );
        if status == Status::NeedsValue {
            line = line.child(
                div()
                    .flex_none()
                    .child(format!("· No value for {}", missing.join(", "))),
            );
            // The fix is on this page: the editor opens with the caret in the value.
            if editable {
                line = line.child(
                    crate::setup::quiet::link(SharedString::from(format!("mcp-add-value-{name}")), "Add it", palette)
                        .on_click(cx.listener(move |this, _, window, cx| this.add_value(index, window, cx))),
                );
            }
        }
        ui::card_row()
            .child(server_tile(&row.server.name, remote, palette))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .when(status == Status::Off, |column| column.opacity(0.6))
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .child(div().text_size(px(13.)).text_color(palette.strong).child(name.clone()))
                            .child(ui::pill(if remote { "Remote" } else { "Local" }, palette)),
                    )
                    .child(line),
            )
            .child(ui::status(status.label(), status.color(palette), palette))
            .children(edit)
            .child(switch)
    }

    /// The editor, in the Servers card where its row was (or at the card's end for a new server): the kind, the fields,
    /// the variables or headers, and a footer with Remove at the left and Save at the right.
    fn render_editor(&self, editor: &Editor, rows: &[Row], palette: Palette, cx: &mut Context<Self>) -> Div {
        let editing = editor.index.and_then(|index| rows.get(index).map(|row| (index, row)));
        let title = match editing {
            Some((_, row)) => format!("Edit {}", row.server.name),
            None => "Add a server".to_string(),
        };
        let what = if editor.remote {
            "Headers"
        } else {
            "Environment variables"
        };
        let head = h_flex()
            .gap(px(12.))
            .child(match editing {
                Some((_, row)) => server_tile(&row.server.name, editor.remote, palette),
                None => ui::icon_tile(Lucide::Plus, palette),
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.strong)
                    .truncate()
                    .child(title),
            )
            .child(
                ui::tabs(palette)
                    .child(
                        ui::tab("mcp-kind-local", "Local command", !editor.remote, palette).on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(editor) = &mut this.editor {
                                    editor.remote = false;
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        ui::tab("mcp-kind-remote", "Remote URL", editor.remote, palette).on_click(cx.listener(
                            |this, _, _, cx| {
                                if let Some(editor) = &mut this.editor {
                                    editor.remote = true;
                                }
                                cx.notify();
                            },
                        )),
                    ),
            );
        // Name beside the command or URL, as one line: both are short, and the URL wants the width.
        let target = if editor.remote {
            ui::field(
                "URL",
                ui::input_box(&editor.url, palette).font_family(ui::MONO),
                palette,
            )
        } else {
            ui::field(
                "Command",
                ui::input_box(&editor.command, palette).font_family(ui::MONO),
                palette,
            )
        };
        let mut form = v_flex().gap(px(12.)).child(head).child(
            h_flex()
                .gap(px(10.))
                .items_start()
                .child(div().w(px(200.)).flex_none().child(ui::field(
                    "Name",
                    ui::input_box(&editor.name, palette),
                    palette,
                )))
                .child(div().flex_1().min_w(px(0.)).child(target)),
        );
        if !editor.remote {
            form = form.child(ui::field(
                "Arguments",
                div()
                    .p(px(8.))
                    .rounded(px(8.))
                    .bg(palette.hover)
                    .text_size(px(13.))
                    .font_family(ui::MONO)
                    .child(Textarea::new(&editor.args).appearance(false)),
                palette,
            ));
        }
        let mut secrets = v_flex().gap(px(6.)).child(ui::label(what, palette));
        for (ix, (name, value, _)) in editor.secrets.iter().enumerate() {
            secrets = secrets.child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .w(px(200.))
                            .flex_none()
                            .child(ui::input_box(name, palette).font_family(ui::MONO)),
                    )
                    .child(ui::input_box(value, palette))
                    .child(
                        ui::focusable(
                            h_flex()
                                .id(("mcp-secret-remove", ix))
                                .flex_none()
                                .size(px(ui::CONTROL_HEIGHT))
                                .justify_center()
                                .rounded(px(8.))
                                .cursor_pointer()
                                .hover(|style| style.bg(palette.hover))
                                .active(|style| style.opacity(0.8)),
                            palette,
                        )
                        .child(ui::icon(Lucide::X, palette.icon))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(editor) = &mut this.editor
                                && ix < editor.secrets.len()
                            {
                                editor.secrets.remove(ix);
                            }
                            cx.notify();
                        })),
                    ),
            );
        }
        secrets = secrets
            .child(
                h_flex().child(
                    ui::ghost_button(
                        "mcp-secret-add",
                        if editor.remote {
                            "Add a header"
                        } else {
                            "Add a variable"
                        },
                        palette,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(editor) = &mut this.editor {
                            editor.add_secret("", false, window, cx);
                        }
                        cx.notify();
                    })),
                ),
            )
            .child(ui::setting_detail(
                "Values go to the keychain. An empty field keeps the saved value.",
                palette,
            ));
        // Remove asks once, here: the question names the server, and the button keeps its width.
        let remove = editing.map(|(index, row)| {
            let asking = self.confirm_remove.as_deref() == Some(row.server.name.as_str());
            let label = if asking {
                format!("Remove {}?", row.server.name)
            } else {
                "Remove server".to_string()
            };
            let button = if asking {
                ui::danger_button("mcp-remove", label, palette)
            } else {
                ui::ghost_button("mcp-remove", label, palette).text_color(palette.destructive)
            };
            button.on_click(cx.listener(move |this, _, _, cx| this.remove(index, cx)))
        });
        let footer = h_flex()
            .gap(px(8.))
            .px(px(16.))
            .py(px(12.))
            .border_t_1()
            .border_color(palette.border)
            .children(remove)
            .child(div().flex_1())
            .child(
                ui::button("mcp-cancel", "Cancel", palette).on_click(cx.listener(|this, _, _, cx| {
                    this.editor = None;
                    this.error = None;
                    this.confirm_remove = None;
                    cx.notify();
                })),
            )
            .child(
                ui::primary_button("mcp-save", if self.saving { "Saving…" } else { "Save" }, palette)
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
            );
        v_flex()
            .child(form.child(secrets).px(px(16.)).pt(px(14.)).pb(px(14.)))
            .child(footer)
    }

    /// The Included with Nexus card: the built-in server, or the note that a server of the person's replaces it, and
    /// the row that opens Connectors, where Nexus's own tools are managed.
    fn render_builtin(&self, rows: &[Row], palette: Palette) -> Div {
        let replaced = rows.iter().any(|row| row.server.name.eq_ignore_ascii_case(BUILTIN));
        let builtin = ui::card_row()
            .child(ui::mark_tile(crate::assets::APP_MARK, None, palette))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .when(replaced, |column| column.opacity(0.6))
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .child(div().text_size(px(13.)).text_color(palette.strong).child(BUILTIN))
                            .child(ui::pill(if replaced { "Replaced" } else { "Built in" }, palette)),
                    )
                    .child(ui::setting_detail(
                        if replaced {
                            "Your server named nexus replaces it. Its configuration decides which Nexus tools your agents receive."
                        } else {
                            "Nexus tools and connectors, signed in as this app."
                        },
                        palette,
                    )),
            );
        let navigate = self.ctx.navigate.clone();
        let connectors = ui::nav_row(
            "mcp-manage-connectors",
            Lucide::Blocks,
            "Connectors",
            "Connect accounts and see which services your organization allows. No server per service.",
            false,
            palette,
        )
        .on_click(move |_, window, cx| navigate(super::PageId::Connectors, window, cx));
        v_flex()
            .child(ui::group_head("Included with Nexus", palette))
            .child(ui::group(
                [Some(builtin.into_any_element()), Some(connectors.into_any_element())],
                palette,
            ))
    }
}

/// A server's mark: the service's logo when its name is one the app knows (`linear`, `github`), else a terminal for a
/// command or a globe for a URL.
fn server_tile(name: &str, remote: bool, palette: Palette) -> Div {
    match crate::assets::connector_named(name) {
        Some((path, color)) => ui::mark_tile(path, color, palette),
        None => ui::icon_tile(if remote { Lucide::Globe } else { Lucide::SquareTerminal }, palette),
    }
}

impl Render for McpPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let mut page = v_flex()
            .w_full()
            .child(ui::lead(
                "Every Code session gets these, on top of the agent's own. A change applies the next time a session's \
                 agent starts.",
                palette,
            ))
            .children(read_only_note(palette));
        let editable = crate::desktop_settings::writable() && !self.saving;
        // The Servers card's head: how many, Refresh, and Add server while no editor is open.
        let servers_head = |count: Option<usize>, cx: &mut Context<Self>| {
            let add = if editable && self.editor.is_none() {
                ui::button("mcp-add", "Add server", palette)
                    .on_click(cx.listener(|this, _, window, cx| this.edit(None, window, cx)))
                    .into_any_element()
            } else {
                ui::disabled_button("mcp-add", "Add server", palette).into_any_element()
            };
            ui::group_head("Additional servers", palette)
                .children(count.filter(|count| *count > 0).map(|count| {
                    div()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(count.to_string())
                }))
                .child(
                    ui::ghost_button("mcp-refresh", "Refresh", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                )
                .child(add)
        };
        page = match &self.servers {
            Load::Loading => page.child(servers_head(None, cx)).child(ui::skeleton(3, palette)),
            Load::Failed(error) => page.child(ui::failed(error.clone(), palette)),
            Load::Ready(rows) => {
                let editing = self.editor.as_ref().and_then(|editor| editor.index);
                let mut items: Vec<Option<AnyElement>> = Vec::new();
                for (index, row) in rows.iter().enumerate() {
                    items.push(Some(match (&self.editor, editing == Some(index)) {
                        (Some(editor), true) => self.render_editor(editor, rows, palette, cx).into_any_element(),
                        _ => self.render_row(index, row, palette, cx).into_any_element(),
                    }));
                }
                if let Some(editor) = self.editor.as_ref().filter(|editor| editor.index.is_none()) {
                    items.push(Some(self.render_editor(editor, rows, palette, cx).into_any_element()));
                }
                if items.is_empty() {
                    let add = if editable {
                        ui::primary_button("mcp-add-first", "Add server", palette)
                            .on_click(cx.listener(|this, _, window, cx| this.edit(None, window, cx)))
                            .into_any_element()
                    } else {
                        ui::disabled_button("mcp-add-first", "Add server", palette).into_any_element()
                    };
                    items.push(Some(
                        v_flex()
                            .items_center()
                            .gap(px(6.))
                            .px(px(16.))
                            .py(px(24.))
                            .child(ui::icon_tile(Lucide::SquareTerminal, palette).mb(px(4.)))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(palette.strong)
                                    .child("No additional servers"),
                            )
                            .child(ui::setting_detail(
                                "Add another tool with a command or a remote server's URL.",
                                palette,
                            ))
                            .child(div().mt(px(6.)).child(add))
                            .into_any_element(),
                    ));
                }
                page.child(self.render_builtin(rows, palette))
                    .child(servers_head(Some(rows.len()), cx))
                    .child(ui::group(items, palette))
                    .children(
                        self.error
                            .clone()
                            .map(|error| ui::error_line(error, palette).mt(px(8.))),
                    )
                    .child(ui::footnote(
                        "Saved values stay in the OS credential store and are never shown again. A server named nexus \
                         replaces the built-in one.",
                        palette,
                    ))
            }
        };
        let outside = super::outside_access::render(
            (self.ctx.code)(cx).outside,
            self.outside_problem.clone(),
            |this: &mut McpPage, problem| this.outside_problem = problem,
            palette,
            cx,
        );
        page.children(outside)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use std::collections::BTreeMap;

    fn linear() -> Draft {
        Draft {
            remote: false,
            name: " linear ".into(),
            command: "npx".into(),
            args: "-y\n\nlinear-mcp\n".into(),
            url: String::new(),
            secrets: vec![("LINEAR_KEY".into(), "s3cret".into()), (" ".into(), String::new())],
        }
    }

    #[test]
    fn says_what_each_server_needs_without_starting_it() {
        let docs = Draft {
            remote: true,
            name: "docs".into(),
            url: "https://mcp.example.test/mcp".into(),
            secrets: vec![("Authorization".into(), String::new())],
            ..Draft::default()
        };
        let servers = vec![
            linear().server(true),
            docs.server(true),
            Draft {
                name: "off".into(),
                command: "srv".into(),
                ..Draft::default()
            }
            .server(false),
        ];
        let values = Values::from([(
            "linear".to_string(),
            BTreeMap::from([("LINEAR_KEY".to_string(), "s3cret".to_string())]),
        )]);
        let rows = rows(servers, &values);
        assert_eq!(rows[0].status(), Status::Configured);
        assert_eq!(rows[0].line(), "npx -y linear-mcp");
        assert_eq!(rows[1].status(), Status::NeedsValue);
        assert_eq!(rows[1].missing(), vec!["Authorization"]);
        assert_eq!(rows[1].line(), "https://mcp.example.test/mcp");
        assert_eq!(rows[2].status(), Status::Off);
    }

    #[test]
    fn the_editor_makes_the_server_and_only_the_values_typed() {
        let server = linear().server(true);
        assert_eq!(server.name, "linear");
        assert_eq!(server.args, vec!["-y", "linear-mcp"]);
        assert_eq!(server.secrets, vec!["LINEAR_KEY"]);
        assert_eq!(linear().values()["linear"]["LINEAR_KEY"], "s3cret");
        assert_eq!(Draft::of(&server).server(true), server);
        assert!(
            Draft::of(&server).values().is_empty(),
            "an empty value keeps the saved one"
        );
    }

    #[test]
    fn add_edit_toggle_and_remove_save_the_list() {
        let file = crate::desktop_settings::TempFile::new("mcp");
        let added = with_server(&[], None, linear().server(true));
        file.update(|settings| apply(settings, added.clone())).expect("added");
        let mut edited = linear();
        edited.url = "https://mcp.example.test".into();
        edited.remote = true;
        let list = with_server(&added, Some(0), edited.server(false));
        file.update(|settings| apply(settings, list)).expect("edited");
        let saved = nexus_desktop_core::settings::load(file.path()).mcp_servers;
        assert_eq!(saved.len(), 1);
        assert!(!saved[0].enabled);
        assert_eq!(saved[0].url.as_deref(), Some("https://mcp.example.test"));
        assert_eq!(saved[0].command, None);
        // A list the Nexus app would refuse is not saved.
        let twice = with_server(&saved, None, saved[0].clone());
        assert!(file.update(|settings| apply(settings, twice)).is_err());
        file.update(|settings| apply(settings, Vec::new())).expect("removed");
        assert!(nexus_desktop_core::settings::load(file.path()).mcp_servers.is_empty());
    }
}
