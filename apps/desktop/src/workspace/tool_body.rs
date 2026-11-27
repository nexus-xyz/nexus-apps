//! A tool call's row opens to what it did, as the Nexus app's does (`ToolBody` and `OutputPreview` in Nexus UI's
//! `work-run.tsx`): the file it acts on, then its command, its edit as lines taken out and put in, the file it wrote,
//! the plan's steps or its input, then its output cut to four lines with "Show all N lines". A command the agent
//! waits for has "Move to background" (`tool-view.tsx`'s `MoveToBackground`, `wb_task_background`). The rules
//! (`preview_output`, `diff_rows`, `clip`) are `nexus_workbench::tool_rows`'s.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::tool_rows::{self, Sign};
use serde_json::{Value, json};

use super::code_view::tool_row;
use super::{View, Workspace, glyph};
use crate::code::ToolCall;
use crate::palette::Palette;

/// What an added line reads in, as the Changes tool's count does.
const ADDED: u32 = 0x3a8f3e;

/// A string field of the call's input; empty when it has none.
fn text<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Whether a running call is a command the agent can stop waiting for: a Bash call not already in the background.
pub(super) fn can_background(name: &str, call: &ToolCall) -> bool {
    name == "Bash"
        && call.output.is_none()
        && call.input.get("run_in_background").and_then(Value::as_bool) != Some(true)
}

/// The file a call acts on and the line to open it at, for Mod-click (the Nexus app's `FILE_KINDS`, `filePathOf` and
/// `lineOf`): a Read, Write or Edit names its file, and a Read opens at its offset. None for any other call.
pub(super) fn editor_target(name: &str, input: &Value) -> Option<(String, Option<u32>)> {
    if !matches!(name, "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit") {
        return None;
    }
    let path = [text(input, "file_path"), text(input, "notebook_path")]
        .into_iter()
        .find(|path| !path.is_empty())?;
    let line = (name == "Read")
        .then(|| input.get("offset").and_then(Value::as_u64))
        .flatten()
        .filter(|offset| *offset > 0)
        .and_then(|offset| u32::try_from(offset).ok());
    Some((path.to_string(), line))
}

/// The lines an edit takes out and puts in, as the row's "+3 −1" says them; None for calls that edit nothing.
fn edit_counts(name: &str, call: &ToolCall) -> Option<(usize, usize)> {
    let (before, after) = match name {
        "Edit" => (text(&call.input, "old_string"), text(&call.input, "new_string")),
        "Write" => ("", text(&call.input, "content")),
        _ => return None,
    };
    let rows = tool_rows::diff_rows(before, after);
    let added = rows.iter().filter(|(sign, _)| matches!(sign, Sign::Added)).count();
    let removed = rows.iter().filter(|(sign, _)| matches!(sign, Sign::Removed)).count();
    (added + removed > 0).then_some((added, removed))
}

impl Workspace {
    /// A call's row (not a failed one): click opens it to what it did; a command it waits for offers the background.
    pub(super) fn render_tool(
        &self,
        ix: usize,
        name: &str,
        summary: &str,
        call: &ToolCall,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let View::Code(view) = &self.view else {
            return div().into_any_element();
        };
        let open = view.open_tools.contains(&call.id);
        let running = self.code_running();
        let background =
            can_background(name, call) && running && self.code_session().is_some_and(|session| session.background);
        let moving = view.moving.contains(&call.id);
        let failed = view
            .row_error
            .as_ref()
            .filter(|(id, _)| *id == call.id)
            .map(|(_, error)| error.clone());
        let call_id = call.id.clone();
        let target = editor_target(name, &call.input);
        let group: SharedString = format!("tool-row-{ix}").into();
        let counts = edit_counts(name, call);
        let row = tool_row(
            ("tool", ix),
            super::code_view::tool_mark(name, palette),
            name,
            summary,
            running && call.output.is_none(),
            palette,
        )
        .group(group.clone())
        .debug_selector(move || format!("tool-row-{ix}"))
        .cursor_pointer()
        .when_some(counts, |row, (added, removed)| {
            row.child(
                h_flex()
                    .flex_none()
                    .gap(px(4.))
                    .text_size(px(13.))
                    .when(added > 0, |counts| {
                        counts.child(div().text_color(gpui_kit::rgb(ADDED)).child(format!("+{added}")))
                    })
                    .when(removed > 0, |counts| {
                        counts.child(
                            div()
                                .text_color(palette.destructive)
                                .child(format!("\u{2212}{removed}")),
                        )
                    }),
            )
        })
        // The row opens to what it did: the chevron says so under the pointer, and stays while open.
        .child(
            div()
                .flex_none()
                .when(!open, |slot| {
                    slot.opacity(0.).group_hover(group.clone(), |style| style.opacity(1.))
                })
                .child(super::disclosure("disclosure", open, palette.faint)),
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
            // Cmd-click (Ctrl off a Mac) on a file's row opens the file in the person's editor, as in the Nexus app.
            if event.modifiers().secondary(cx)
                && let Some((path, line)) = target.clone()
            {
                this.open_tool_file(call_id.clone(), path, line, cx);
                return;
            }
            if let View::Code(view) = &mut this.view {
                if !view.open_tools.remove(&call_id) {
                    view.open_tools.insert(call_id.clone());
                }
                view.list.remeasure_items(ix..ix + 1);
                cx.notify();
            }
        }));
        let id = call.id.clone();
        v_flex()
            .w_full()
            .gap(px(4.))
            .child(row)
            .when(background, |column| {
                column.child(
                    h_flex().pl(px(22.)).child(
                        h_flex()
                            .id(("move-background", ix))
                            .h(px(24.))
                            .px(px(8.))
                            .gap(px(4.))
                            .rounded(px(6.))
                            .bg(palette.hover)
                            .text_size(px(12.))
                            .text_color(if moving { palette.faint } else { palette.muted })
                            .child(glyph(Lucide::ArrowDownToLine, palette.icon))
                            .when(!moving, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(palette.hover))
                                    .on_click(cx.listener(move |this, _, _, cx| this.move_to_background(id.clone(), cx)))
                            })
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "Stop waiting for this command: the agent goes on, and the command runs on in the background",
                                )
                                .build(window, cx)
                            })
                            .child("Move to background"),
                    ),
                )
            })
            .when_some(failed, |column, error| {
                column.child(div().pl(px(30.)).text_size(px(12.)).text_color(palette.destructive).child(error))
            })
            .when(open, |column| column.child(self.tool_body(ix, name, call, view.all_output.contains(&call.id), palette, cx)))
            .into_any_element()
    }

    /// Tell the agent to stop waiting for command `id` (`wb_task_background`); it runs on as a background task.
    fn move_to_background(&mut self, id: String, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else {
            return;
        };
        let thread = view.id.clone();
        if !view.moving.insert(id.clone()) {
            return;
        }
        view.row_error = None;
        let Some(runs) = self.runs(&thread) else {
            return;
        };
        let read = runs.call("wb_task_background", json!({ "threadId": thread, "toolUseId": id }));
        cx.spawn(async move |this, cx| {
            let answer = read
                .await
                .unwrap_or_else(|_| Err("The command's answer was lost.".into()));
            let _ = this.update(cx, |this, cx| {
                if let View::Code(view) = &mut this.view
                    && view.id == thread
                {
                    view.moving.remove(&id);
                    let error = match answer {
                        Ok(value) if value == Value::Bool(false) || value["backgrounded"] == false => {
                            Some("The agent did not move it to the background.".to_string())
                        }
                        Ok(_) => None,
                        Err(error) => Some(error),
                    };
                    view.row_error = error.map(|error| (id, error));
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Open `path` in the saved editor at `line`, in the session's folder (the Nexus app's `useOpenFile`). A session on
    /// another machine has no folder here; the reason shows under call `id`'s row.
    fn open_tool_file(&mut self, id: String, path: String, line: Option<u32>, cx: &mut Context<Self>) {
        let View::Code(view) = &mut self.view else {
            return;
        };
        view.row_error = None;
        let thread = view.id.clone();
        // The Files preview's folder (`side_pane.rs`'s `scope_folder`): it also reads a session the local background
        // service runs. A server's session has no folder here.
        let folder = if crate::machines::get().machine_of(&thread).is_some() {
            Err("Open in editor is available for sessions on this computer.".to_string())
        } else {
            self.code
                .as_ref()
                .ok_or_else(|| "Code is not ready.".to_string())
                .and_then(|code| code.folder(&thread))
                .map(|folder| folder.path)
        };
        let open = cx.background_executor().spawn(async move {
            let folder = folder?;
            crate::editor_choice::open_file(&folder, &path, line)
        });
        cx.spawn(async move |this, cx| {
            let result = open.await;
            let _ = this.update(cx, |this, cx| {
                if let View::Code(view) = &mut this.view
                    && view.id == thread
                {
                    view.row_error = result.err().map(|error| (id, error));
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// What an opened call shows (`ToolBody`).
    fn tool_body(
        &self,
        ix: usize,
        name: &str,
        call: &ToolCall,
        all: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let output: AnyElement = match &call.output {
            None => quiet("No result yet.", palette),
            Some(output) if output.text.trim().is_empty() => div().into_any_element(),
            Some(output) => {
                let preview = tool_rows::preview_output(&output.text);
                let shown = if all || preview.more == 0 {
                    output.text.trim_end_matches('\n').to_string()
                } else {
                    format!(
                        "{}\n{}\n{}",
                        preview.head,
                        tool_rows::more_lines_label(preview.more),
                        preview.tail
                    )
                };
                v_flex()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(palette.faint)
                            .child(if output.error { "Error" } else { "Output" }),
                    )
                    .child(pre(shown, palette).when(output.error, |box_| box_.text_color(palette.destructive)))
                    .when(!all && preview.more > 0, |column| {
                        let call_id = call.id.clone();
                        column.child(
                            div()
                                .id(("show-all", ix))
                                .text_size(px(12.5))
                                .text_color(palette.muted)
                                .cursor_pointer()
                                .hover(|style| style.text_color(palette.foreground))
                                .child(tool_rows::show_all_label(preview.total))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let View::Code(view) = &mut this.view {
                                        view.all_output.insert(call_id.clone());
                                        view.list.remeasure_items(ix..ix + 1);
                                        cx.notify();
                                    }
                                })),
                        )
                    })
                    .when(output.truncated, |column| {
                        column.child(quiet("Cut at 64 KiB.", palette))
                    })
                    .into_any_element()
            }
        };
        v_flex()
            .w_full()
            .pl(px(30.))
            .gap(px(6.))
            .child(tool_input(name, call, palette))
            .child(output)
            .into_any_element()
    }
}

/// Shared input details for a tool call in either the session or a sidechat.
pub(super) fn tool_input(name: &str, call: &ToolCall, palette: Palette) -> AnyElement {
    let input = &call.input;
    let path = Some(text(input, "file_path"))
        .filter(|path| !path.is_empty())
        .or_else(|| Some(text(input, "notebook_path")).filter(|path| !path.is_empty()));
    let main: Option<AnyElement> = match name {
        "Bash" => Some(pre(format!("$ {}", text(input, "command")), palette).into_any_element()),
        "Edit" => diff(text(input, "old_string"), text(input, "new_string"), palette),
        "MultiEdit" => {
            let edits = input
                .get("edits")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let diffs: Vec<_> = edits
                .iter()
                .filter_map(|edit| diff(text(edit, "old_string"), text(edit, "new_string"), palette))
                .collect();
            (!diffs.is_empty()).then(|| v_flex().gap(px(4.)).children(diffs).into_any_element())
        }
        "Write" => Some(pre(tool_rows::clip(text(input, "content")), palette).into_any_element()),
        "TodoWrite" => {
            let todos = input
                .get("todos")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Some(
                v_flex()
                    .gap(px(2.))
                    .text_size(px(12.5))
                    .children(todos.iter().map(|todo| {
                        let done = text(todo, "status") == "completed";
                        h_flex()
                            .gap(px(6.))
                            .text_color(if done { palette.faint } else { palette.muted })
                            .child(glyph(
                                if done { Lucide::SquareCheck } else { Lucide::Square },
                                palette.faint,
                            ))
                            .child(text(todo, "content").to_string())
                    }))
                    .into_any_element(),
            )
        }
        "ExitPlanMode" => Some(
            div()
                .text_size(px(12.5))
                .text_color(palette.muted)
                .child(text(input, "plan").to_string())
                .into_any_element(),
        ),
        "Read" | "Grep" | "Glob" | "LS" => None,
        _ => Some(pre(serde_json::to_string_pretty(input).unwrap_or_default(), palette).into_any_element()),
    };
    v_flex()
        .w_full()
        .gap(px(6.))
        .when_some(path, |column, path| column.child(pre(path.to_string(), palette)))
        .children(main)
        .into_any_element()
}

/// Preformatted text in the transcript's code box, as a failed call's error shows.
pub(super) fn pre(text: String, palette: Palette) -> Div {
    div()
        .w_full()
        .px(px(12.))
        .py(px(8.))
        .rounded(px(8.))
        .bg(palette.raised)
        .font_family("Geist Mono")
        .text_size(px(11.5))
        .line_height(px(18.))
        .text_color(palette.muted)
        .child(text)
}

fn quiet(text: &'static str, palette: Palette) -> AnyElement {
    div()
        .text_size(px(12.))
        .text_color(palette.faint)
        .child(text)
        .into_any_element()
}

/// An edit as the lines it takes out and puts in (`Diff`); None when it carries no strings.
fn diff(before: &str, after: &str, palette: Palette) -> Option<AnyElement> {
    let rows = tool_rows::diff_rows(before, after);
    if rows.is_empty() {
        return None;
    }
    Some(
        v_flex()
            .w_full()
            .px(px(12.))
            .py(px(8.))
            .rounded(px(8.))
            .bg(palette.raised)
            .font_family("Geist Mono")
            .text_size(px(11.5))
            .line_height(px(18.))
            .children(rows.into_iter().map(|(sign, line)| {
                let (mark, color) = match sign {
                    Sign::Same => (" ", palette.muted),
                    Sign::Removed => ("-", palette.destructive),
                    Sign::Added => ("+", gpui_kit::rgb(ADDED).into()),
                };
                h_flex()
                    .gap(px(8.))
                    .text_color(color)
                    .child(div().flex_none().w(px(8.)).child(mark))
                    .child(
                        div()
                            .min_w(px(0.))
                            .child(if line.is_empty() { " ".to_string() } else { line }),
                    )
            }))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::can_background;
    use crate::code::{ToolCall, ToolOutput};
    use serde_json::json;

    #[test]
    fn only_a_running_foreground_command_moves_to_the_background() {
        let call = |input: serde_json::Value, done: bool| ToolCall {
            id: "c".into(),
            input,
            output: done.then(ToolOutput::default),
        };
        assert!(can_background("Bash", &call(json!({ "command": "npm test" }), false)));
        assert!(!can_background("Bash", &call(json!({ "command": "npm test" }), true)));
        assert!(!can_background(
            "Bash",
            &call(json!({ "command": "npm run dev", "run_in_background": true }), false)
        ));
        assert!(!can_background("Read", &call(json!({ "file_path": "/a" }), false)));
    }
}
