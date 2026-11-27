//! Memory ("What Nexus knows" before), as Nexus UI's `agent-memory.tsx`: the notes as Markdown files by kind (people,
//! preferences, places, routines, notes) and one per errand, with their fact counts; the open file's lines numbered in
//! the code theme, each with where it came from; Show where, with Open the errand when an errand taught it; Edit and
//! Delete on a line; Forget all; and with the memory controls on, the learning switch, Add a note and Export, a ZIP of
//! the files and `memory.json`. The side says whose memory it is: Personal, or "Kept by <Org>'s Assistant".
//!
//! A note is the agent's own text: its Markdown markers are stripped (`note_lines`), and it is drawn as plain text.
//! The words, the files and the export are `nexus_desktop_core::agent_memory`'s.

use crate::workspace::FloatingGround;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::super::{Ctx, PageId, palette, ui};
use super::{Moves, Reply, call, day, pill, primary_pill, problem, quiet, quiet_pill};
use crate::agent::Call;
use crate::palette::Palette;
use nexus_desktop_core::agent::Problem;
use nexus_desktop_core::agent_memory::{self as words, MemoryFile, MemoryView, Note};

/// A line of a file: its note's index in the view, and its place in that note.
pub type LineKey = (usize, usize);

/// What the picked line shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineMode {
    /// Show where, Edit and Delete.
    Actions,
    /// Where it came from (Learned from).
    Where,
    /// Delete waits for the person's yes.
    Confirm,
    /// Its note's text in the editor.
    Edit,
}

enum State {
    Loading,
    Ready,
    Failed(Problem),
}

pub struct MemoryPage {
    ctx: Ctx,
    moves: Moves,
    state: State,
    view: Option<MemoryView>,
    /// The files of `view`, computed when it changed.
    files: Vec<MemoryFile>,
    /// The open file's path; the first file when none is.
    open: Option<String>,
    picked: Option<(LineKey, LineMode)>,
    forget_all: bool,
    adding: bool,
    editor: Entity<TextareaState>,
    adder: Entity<TextareaState>,
    draft_error: Option<String>,
    refused: Option<Problem>,
    status: Option<String>,
    working: bool,
    _read: Task<()>,
    _change: Option<Task<()>>,
    _export: Option<Task<()>>,
}

pub fn build(ctx: Ctx, moves: Moves, window: &mut Window, cx: &mut App) -> Entity<MemoryPage> {
    cx.new(|cx| {
        let editor = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 8));
        let adder = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Lily is in 1st grade at Rooftop.")
                .auto_grow(2, 8)
        });
        let read = call(&ctx, Call::Memory, window, cx, |page: &mut MemoryPage, reply, _, _| {
            page.read(reply)
        });
        MemoryPage {
            ctx,
            moves,
            state: State::Loading,
            view: None,
            files: Vec::new(),
            open: None,
            picked: None,
            forget_all: false,
            adding: false,
            editor,
            adder,
            draft_error: None,
            refused: None,
            status: None,
            working: false,
            _read: read,
            _change: None,
            _export: None,
        }
    })
}

impl MemoryPage {
    fn read(&mut self, reply: Reply) {
        match (reply.ok(), reply.body.as_ref().and_then(words::memory_view_of)) {
            (true, Some(view)) => {
                self.state = State::Ready;
                self.set_view(view);
            }
            (true, None) => self.state = State::Failed(words::memory_problem(502, None)),
            (false, _) => self.state = State::Failed(words::memory_problem(reply.status, reply.body.as_ref())),
        }
    }

    /// The view changed: its files are computed again, and a picked line that went is let go.
    fn set_view(&mut self, view: MemoryView) {
        self.files = words::memory_files(&view.notes);
        self.view = Some(view);
        if self.file().is_none_or(|file| Some(&file.path) != self.open.as_ref()) {
            self.open = None;
        }
        self.picked = None;
    }

    /// The open file: the picked one, else the first.
    fn file(&self) -> Option<&MemoryFile> {
        self.open
            .as_ref()
            .and_then(|path| self.files.iter().find(|file| file.path == *path))
            .or(self.files.first())
    }

    fn controls(&self) -> bool {
        self.view.as_ref().is_some_and(|view| view.controls)
    }

    #[cfg(test)]
    pub fn files(&self) -> &[MemoryFile] {
        &self.files
    }

    #[cfg(test)]
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    #[cfg(test)]
    pub fn open_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.pick_file(path.to_string(), cx);
    }

    #[cfg(test)]
    pub fn pick(&mut self, key: LineKey, mode: LineMode, cx: &mut Context<Self>) {
        self.picked = Some((key, mode));
        cx.notify();
    }

    #[cfg(test)]
    pub fn delete(&mut self, key: LineKey, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_line(key, window, cx);
    }

    fn pick_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.open = Some(path);
        self.picked = None;
        cx.notify();
    }

    /// Send one change; `done` gets its body once it succeeded.
    fn change(
        &mut self,
        made: Option<Call>,
        forgetting: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, serde_json::Value, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(made) = made else { return };
        if self.working {
            return;
        }
        self.working = true;
        self.refused = None;
        self._change = Some(call(&self.ctx, made, window, cx, move |page, reply, window, cx| {
            page.working = false;
            if reply.ok() {
                done(page, reply.body.unwrap_or_default(), window, cx);
            } else if forgetting {
                page.refused = Some(words::memory_problem(reply.status, reply.body.as_ref()));
            } else {
                page.refused = Some(words::memory_change_problem(reply.status, reply.body.as_ref()));
            }
        }));
        cx.notify();
    }

    /// Forget one note, or all of them.
    fn forget(&mut self, note: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let one = note.clone();
        self.forget_all = false;
        self.change(
            Call::forget(note.as_deref()),
            true,
            window,
            cx,
            move |page, body, _, _| {
                let count = body.get("forgotten").and_then(serde_json::Value::as_u64).unwrap_or(0);
                if let Some(mut view) = page.view.take() {
                    match &one {
                        Some(id) => view.notes.retain(|note| note.id != *id),
                        None => view.notes.clear(),
                    }
                    page.set_view(view);
                }
                page.status = Some(words::forgotten_text(one.is_some(), count, &page.moves.name()));
            },
        );
    }

    /// A note changed or was added: the view keeps the agent API's own copy.
    fn kept(&mut self, note: Note, added: bool) {
        let Some(mut view) = self.view.take() else { return };
        match view.notes.iter().position(|kept| kept.id == note.id) {
            Some(at) if !added => view.notes[at] = note,
            _ => view.notes.insert(0, note),
        }
        self.set_view(view);
        let name = self.moves.name();
        self.status = Some(if added {
            format!("{name} keeps the note, from its next message.")
        } else {
            format!("{name} uses the new note from its next message.")
        });
    }

    fn edit(&mut self, id: String, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.change(
            Call::edit_note(&id, &text),
            false,
            window,
            cx,
            |page, body, _, _| match body.get("note").and_then(words::note_of) {
                Some(note) => page.kept(note, false),
                None => page.refused = Some(words::memory_change_problem(502, None)),
            },
        );
    }

    /// Delete one line (the web's `LineRow`): the note's other lines stay when it can be edited, else the note goes.
    fn delete_line(&mut self, (at, index): LineKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(note) = self.view.as_ref().and_then(|view| view.notes.get(at)).cloned() else {
            return;
        };
        match (words::without_line(&note, index), self.controls()) {
            (Some(rest), true) => self.edit(note.id, rest, window, cx),
            _ => self.forget(Some(note.id), window, cx),
        }
    }

    fn save_edit(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self
            .view
            .as_ref()
            .and_then(|view| view.notes.get(at))
            .map(|note| note.id.clone())
        else {
            return;
        };
        let text = self.editor.read(cx).value().to_string();
        match words::note_draft(&text) {
            Ok(text) => {
                self.draft_error = None;
                self.edit(id, text, window, cx);
            }
            Err(error) => self.draft_error = Some(error),
        }
        cx.notify();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.adder.read(cx).value().to_string();
        let text = match words::note_draft(&text) {
            Ok(text) => text,
            Err(error) => {
                self.draft_error = Some(error);
                cx.notify();
                return;
            }
        };
        self.draft_error = None;
        self.change(
            Call::add_note(&text),
            false,
            window,
            cx,
            |page, body, window, cx| match body.get("note").and_then(words::note_of) {
                Some(note) => {
                    page.adding = false;
                    page.adder.update(cx, |state, cx| state.set_value("", window, cx));
                    page.kept(note, true);
                }
                None => page.refused = Some(words::memory_change_problem(502, None)),
            },
        );
    }

    fn pause(&mut self, paused: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.change(
            Some(Call::learning(paused)),
            false,
            window,
            cx,
            |page, body, _, _| match body.get("learning").and_then(words::learning_of) {
                Some(learning) => {
                    page.status = Some(if learning.paused {
                        "Learning is paused.".into()
                    } else {
                        "Learning is on again.".into()
                    });
                    if let Some(view) = &mut page.view {
                        view.learning = Some(learning);
                    }
                }
                None => page.refused = Some(words::memory_change_problem(502, None)),
            },
        );
    }

    /// Export: the ZIP is built from the view on screen, then saved where the person picks.
    fn export(&mut self, cx: &mut Context<Self>) {
        let Some(view) = &self.view else { return };
        let now = chrono::Utc::now();
        use chrono::{Datelike as _, Timelike as _};
        let at = words::ZipTime {
            year: u16::try_from(now.year()).unwrap_or(1980),
            month: now.month() as u8,
            day: now.day() as u8,
            hour: now.hour() as u8,
            minute: now.minute() as u8,
            second: now.second() as u8,
        };
        let org = if self.moves.personal {
            "personal".to_string()
        } else {
            self.ctx.org.clone()
        };
        let zip = words::memory_zip(
            view,
            &org,
            &now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            at,
        );
        self._export = Some(cx.spawn(async move |this, cx| {
            let folder = super::super::download::downloads_folder();
            let picked = cx.update(|cx| cx.prompt_for_new_path(&folder, Some(words::EXPORT_FILE)));
            let said = match picked.await {
                Ok(Ok(Some(path))) => match std::fs::write(&path, &zip) {
                    Ok(()) => Some(format!("Saved {}.", super::super::download::file_name(&path))),
                    Err(error) => Some(format!("Couldn't save {}: {error}", path.display())),
                },
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(error)) => Some(error.to_string()),
            };
            this.update(cx, |page, cx| {
                if said.is_some() {
                    page.status = said;
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}

/// The code theme's ground: a step darker than the page in dark, a step greyer in light.
fn code_ground(palette: Palette) -> Hsla {
    // See-through: the same step as ink over whatever it lies on (`frost::Tiers`).
    if palette.frosted {
        return gpui_kit::black().opacity(if palette.dark { 0.18 } else { 0.04 });
    }
    if palette.dark {
        rgb(0x141415).into()
    } else {
        rgb(0xF4F4F5).into()
    }
}

/// A file in the side list: its name and its fact count, indented under `errands/`.
fn file_row(id: usize, file: &MemoryFile, open: bool, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(("memory-file", id))
        .h(px(30.))
        .gap(px(8.))
        .pl(px(if file.errand.is_some() { 26. } else { 10. }))
        .pr(px(8.))
        .rounded(px(7.))
        .text_size(px(13.))
        .cursor_pointer()
        .when(open, |row| row.bg(palette.selected).text_color(palette.foreground))
        .when(!open, |row| {
            row.text_color(palette.muted).hover(|style| style.bg(palette.hover))
        })
        .child(div().flex_1().min_w(px(0.)).truncate().child(file.name.clone()))
        .child(
            div()
                .font_family(ui::MONO)
                .text_size(px(11.))
                .text_color(palette.faint)
                .child(file.lines.len().to_string()),
        )
}

/// One numbered row of the code theme.
fn code_row(number: usize, text: impl IntoElement, palette: Palette) -> Div {
    h_flex()
        .w_full()
        .min_h(px(28.))
        .gap(px(12.))
        .pr(px(12.))
        .child(
            div()
                .w(px(34.))
                .flex_none()
                .font_family(ui::MONO)
                .text_right()
                .text_color(palette.faint)
                .child(number.to_string()),
        )
        .child(text)
}

impl Render for MemoryPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let outer = div().id("assistant-memory").size_full().flex();
        let failed = match &self.state {
            State::Loading => Some(quiet("Loading your memory…", palette)),
            State::Failed(failed) => Some(problem(failed, palette)),
            State::Ready => None,
        };
        if let Some(failed) = failed {
            return outer.child(v_flex().p(px(28.)).w_full().child(failed));
        }
        let Some(view) = self.view.as_ref() else {
            return outer;
        };
        let scope = words::memory_scope(self.moves.personal, &self.ctx.org_name);
        let open_path = self.file().map(|file| file.path.clone());

        // The side: whose memory, the files, and the errands' folder.
        let mut side = v_flex()
            .id("memory-files")
            .w(px(230.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .px(px(8.))
            .py(px(12.))
            .gap(px(2.))
            .bg(palette.sidebar)
            .child(
                div()
                    .px(px(8.))
                    .pt(px(4.))
                    .pb(px(6.))
                    .text_size(px(14.))
                    .text_color(palette.foreground)
                    .child("Memory"),
            )
            .child(
                div()
                    .px(px(8.))
                    .pt(px(10.))
                    .pb(px(6.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(scope.as_ref().map_or("Personal".to_string(), |scope| scope.0.clone())),
            );
        if !self.files.is_empty() {
            side = side.child(
                div()
                    .px(px(10.))
                    .pb(px(4.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(format!(
                        "{} in {} {}",
                        words::facts_text(words::fact_count(&self.files)),
                        self.files.len(),
                        if self.files.len() == 1 { "file" } else { "files" }
                    )),
            );
        }
        let mut folder = false;
        for (at, file) in self.files.iter().enumerate() {
            if file.errand.is_some() && !folder {
                folder = true;
                side = side.child(
                    h_flex()
                        .h(px(30.))
                        .pl(px(10.))
                        .text_size(px(13.))
                        .text_color(palette.muted)
                        .child("errands/"),
                );
            }
            let path = file.path.clone();
            side = side.child(
                file_row(at, file, Some(&file.path) == open_path.as_ref(), palette)
                    .on_click(cx.listener(move |page, _, _, cx| page.pick_file(path.clone(), cx))),
            );
        }
        if let Some((_, kept)) = &scope {
            side = side.child(
                div()
                    .px(px(10.))
                    .pt(px(16.))
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(palette.faint)
                    .child(kept.clone()),
            );
        }

        // The head of the open file: its name, facts and last change, then the switches.
        let controls = view.controls;
        let name = self.moves.name();
        let paused = view.learning.as_ref().is_some_and(|learning| learning.paused);
        let working = self.working;
        let file = self.file();
        let mut head = h_flex()
            .w_full()
            .h(px(46.))
            .flex_none()
            .gap(px(10.))
            .px(px(16.))
            .border_b_1()
            .border_color(palette.border)
            .text_size(px(13.));
        if let Some(file) = file {
            head = head
                .child(
                    div()
                        .font_family(ui::MONO)
                        .text_size(px(12.))
                        .text_color(palette.muted)
                        .child(file.name.clone()),
                )
                .child(div().text_color(palette.faint).child(format!(
                    "· {} · edited {}",
                    words::facts_text(file.lines.len()),
                    day(&file.changed_at)
                )));
        }
        head = head.child(div().flex_1());
        if controls {
            head = head
                .child(
                    h_flex().gap(px(8.)).text_color(palette.muted).child("Learning").child(
                        ui::switch("memory-learning", !paused, working, palette)
                            .on_click(cx.listener(move |page, _, window, cx| page.pause(!paused, window, cx))),
                    ),
                )
                .child(
                    pill("memory-export", "Export", palette).on_click(cx.listener(|page, _, _, cx| page.export(cx))),
                );
        }
        if !self.files.is_empty() && !self.forget_all {
            head = head.child(pill("memory-forget-all", "Forget all", palette).on_click(cx.listener(
                |page, _, _, cx| {
                    page.forget_all = true;
                    cx.notify();
                },
            )));
        }

        // Over the file: what changed, what failed, the switch's state, Add a note and the saved model.
        let mut notes = v_flex().w_full().gap(px(10.)).px(px(16.)).pt(px(12.));
        if let Some(status) = &self.status {
            notes = notes.child(quiet(status.clone(), palette));
        }
        if let Some(refused) = &self.refused {
            notes = notes.child(problem(refused, palette));
        }
        if self.forget_all {
            notes = notes.child(super::confirm(
                "memory-confirm-all",
                "Forget every note?",
                &format!("{name} forgets every file, on every surface."),
                cx.listener(|page, _, window, cx| page.forget(None, window, cx)),
                cx.listener(|page, _, _, cx| {
                    page.forget_all = false;
                    cx.notify();
                }),
                palette,
            ));
        }
        if controls && paused {
            let since = view
                .learning
                .as_ref()
                .and_then(|learning| learning.paused_at.as_deref())
                .map(|at| format!(" since {}", day(at)))
                .unwrap_or_default();
            notes = notes.child(
                h_flex()
                    .gap(px(10.))
                    .p(px(12.))
                    .rounded(px(12.))
                    .bg(palette.hover)
                    .child(div().flex_1().text_size(px(13.)).text_color(palette.foreground).child(format!(
                        "Learning is paused{since}. {name} keeps no new notes from your conversations, and still uses what it knows."
                    )))
                    .child(primary_pill("memory-resume", "Resume", palette).on_click(cx.listener(|page, _, window, cx| page.pause(false, window, cx)))),
            );
        }
        if controls {
            if self.adding {
                notes = notes.child(self.editor_view(
                    "memory-add",
                    &self.adder,
                    "Add",
                    format!("{name} uses it in your direct messages, from its next message."),
                    palette,
                    cx,
                    |page, window, cx| page.add(window, cx),
                ));
            } else {
                notes = notes.child(h_flex().child(pill("memory-add-open", "Add a note", palette).on_click(
                    cx.listener(|page, _, window, cx| {
                        page.adding = true;
                        page.draft_error = None;
                        page.adder.update(cx, |state, cx| state.focus(window, cx));
                        cx.notify();
                    }),
                )));
            }
        }
        notes = notes.child(
            div()
                .text_size(px(12.))
                .text_color(palette.faint)
                .child(format!("Saved model · {}", words::saved_model_text(&view.saved_model))),
        );

        // The file in the code theme.
        let mut code = v_flex()
            .w_full()
            .py(px(12.))
            .bg(code_ground(palette))
            .text_size(px(13.))
            .line_height(px(19.5))
            .text_color(palette.foreground);
        let mut learned_from = None;
        match file {
            None => {
                code = code.child(
                    v_flex()
                        .px(px(16.))
                        .gap(px(4.))
                        .child(div().text_size(px(14.)).child(format!("{name} keeps no notes about you")))
                        .child(quiet(
                            format!(
                                "After a conversation, {name} writes a short note. It uses the note later in the same place only."
                            ),
                            palette,
                        )),
                );
            }
            Some(file) => {
                code = code
                    .child(code_row(
                        1,
                        h_flex()
                            .gap(px(8.))
                            .font_family(ui::MONO)
                            .child(div().text_color(palette.faint).child("#"))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(file.title.clone())),
                        palette,
                    ))
                    .child(code_row(2, div(), palette));
                for (i, line) in file.lines.iter().enumerate() {
                    let key = (line.note, line.index);
                    let note = &view.notes[line.note];
                    let mode = self.picked.filter(|(picked, _)| *picked == key).map(|(_, mode)| mode);
                    if mode == Some(LineMode::Where) {
                        learned_from = Some(self.learned_from(note, palette, cx));
                    }
                    if mode == Some(LineMode::Edit) {
                        let at = line.note;
                        code = code.child(code_row(
                            i + 3,
                            div().flex_1().child(self.editor_view(
                                "memory-edit",
                                &self.editor,
                                "Save",
                                format!("Edit the note's lines. {name} uses the new text from its next message."),
                                palette,
                                cx,
                                move |page, window, cx| page.save_edit(at, window, cx),
                            )),
                            palette,
                        ));
                        continue;
                    }
                    let source = format!(
                        "{} · {}",
                        words::source_text(note).unwrap_or_else(|| words::audience(note).0),
                        day(&note.created_at)
                    );
                    let trailing = match mode {
                        None => div()
                            .text_size(px(11.5))
                            .text_color(palette.faint)
                            .whitespace_nowrap()
                            .child(source)
                            .into_any_element(),
                        Some(LineMode::Confirm) => {
                            let (question, hint) = match (words::without_line(note, line.index), controls) {
                                (None, _) => (
                                    "Delete this line?".to_string(),
                                    format!("{name} forgets it on every surface."),
                                ),
                                (Some(_), true) => (
                                    "Delete this line?".to_string(),
                                    format!("{name} keeps the other lines of its note."),
                                ),
                                (Some(_), false) => (
                                    "Delete its note?".to_string(),
                                    format!(
                                        "The line is one of {} in a note. {name} forgets the whole note, on every surface.",
                                        words::note_lines(&note.text).len()
                                    ),
                                ),
                            };
                            div()
                                .child(super::confirm(
                                    "memory-confirm-line",
                                    &question,
                                    &hint,
                                    cx.listener(move |page, _, window, cx| page.delete_line(key, window, cx)),
                                    cx.listener(|page, _, _, cx| {
                                        page.picked = None;
                                        cx.notify();
                                    }),
                                    palette,
                                ))
                                .into_any_element()
                        }
                        Some(_) => {
                            let text = words::note_lines(&note.text).join("\n");
                            h_flex()
                                .gap(px(6.))
                                .child(pill(("memory-where", i), "Show where", palette).on_click(cx.listener(
                                    move |page, _, _, cx| {
                                        page.picked = Some((key, LineMode::Where));
                                        cx.notify();
                                    },
                                )))
                                .when(controls, |actions| {
                                    actions.child(if working {
                                        quiet_pill(("memory-edit-line", i), "Edit", palette)
                                    } else {
                                        pill(("memory-edit-line", i), "Edit", palette).on_click(cx.listener(
                                            move |page, _, window, cx| {
                                                page.picked = Some((key, LineMode::Edit));
                                                page.draft_error = None;
                                                let text = text.clone();
                                                page.editor.update(cx, |state, cx| {
                                                    state.set_value(text, window, cx);
                                                    state.focus(window, cx);
                                                });
                                                cx.notify();
                                            },
                                        ))
                                    })
                                })
                                .child(pill(("memory-delete", i), "Delete", palette).on_click(cx.listener(
                                    move |page, _, _, cx| {
                                        page.picked = Some((key, LineMode::Confirm));
                                        cx.notify();
                                    },
                                )))
                                .into_any_element()
                        }
                    };
                    code = code.child(
                        code_row(
                            i + 3,
                            h_flex()
                                .flex_1()
                                .min_w(px(0.))
                                .gap(px(8.))
                                .font_family(ui::MONO)
                                .child(div().flex_none().text_color(palette.faint).child("-"))
                                .child(div().flex_1().min_w(px(0.)).child(line.text.clone())),
                            palette,
                        )
                        .id(("memory-line", i))
                        .cursor_pointer()
                        .when(mode.is_some(), |row| row.bg(palette.selected))
                        .when(mode.is_none(), |row| row.hover(|style| style.bg(palette.hover)))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            if page.picked.is_none_or(|(picked, _)| picked != key) {
                                page.picked = Some((key, LineMode::Actions));
                                cx.notify();
                            }
                        }))
                        .child(trailing),
                    );
                }
            }
        }

        let main = v_flex()
            .id("memory-file")
            .relative()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .overflow_y_scroll()
            .child(head)
            .child(notes)
            .child(div().pt(px(12.)).child(code))
            .children(learned_from);
        outer.child(side).child(main)
    }
}

impl MemoryPage {
    /// The note editor (Add a note, Edit): its text, Save or Add, and Cancel, with the hint and why it cannot be kept.
    #[allow(clippy::too_many_arguments)]
    fn editor_view(
        &self,
        id: &'static str,
        state: &Entity<TextareaState>,
        submit: &'static str,
        hint: String,
        palette: Palette,
        cx: &mut Context<Self>,
        save: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Div {
        let working = self.working;
        v_flex()
            .w_full()
            .gap(px(6.))
            .py(px(6.))
            .child(
                div()
                    .w_full()
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .bg(palette.hover)
                    .text_size(px(13.))
                    .child(Textarea::new(state).appearance(false)),
            )
            .child(div().text_size(px(12.)).text_color(palette.faint).child(hint))
            .when_some(self.draft_error.clone(), |editor, error| {
                editor.child(ui::error_line(error, palette))
            })
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(if working {
                        quiet_pill((id, 0usize), "Saving…", palette)
                    } else {
                        primary_pill((id, 0usize), submit, palette)
                            .on_click(cx.listener(move |page, _, window, cx| save(page, window, cx)))
                    })
                    .child(
                        pill((id, 1usize), "Cancel", palette).on_click(cx.listener(|page, _, _, cx| {
                            page.adding = false;
                            page.picked = None;
                            page.draft_error = None;
                            cx.notify();
                        })),
                    ),
            )
    }

    /// Learned from ("Show where"): the source and its dates, where Nexus uses it, and the errand that taught it.
    fn learned_from(&self, note: &Note, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let audience = words::audience(note);
        let mut when = format!(
            "{} · {}",
            words::source_text(note).unwrap_or_else(|| {
                format!(
                    "{} did not keep where this came from. The note is older than sources.",
                    self.moves.name()
                )
            }),
            day(&note.created_at)
        );
        if let Some(edited) = &note.edited_at {
            when.push_str(&format!(" · edited {}", day(edited)));
        }
        let errand = note.source.as_ref().and_then(|source| source.errand.clone());
        v_flex()
            .absolute()
            .right(px(24.))
            .top(px(150.))
            .w(px(300.))
            .gap(px(8.))
            .p(px(14.))
            .rounded(px(12.))
            .floating_ground(12.0, palette.menu, true, &palette)
            .border_1()
            .border_color(palette.border)
            .shadow(ui::lift(palette))
            .child(div().text_size(px(12.)).text_color(palette.faint).child("Learned from"))
            .child(div().text_size(px(13.)).text_color(palette.foreground).child(when))
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(10.))
                    .bg(palette.selected)
                    .text_size(px(12.5))
                    .text_color(palette.foreground)
                    .child(format!("{}. {}", audience.0, audience.1)),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .when_some(errand, |actions, (id, _)| {
                        actions.child(
                            pill("memory-open-errand", "Open the errand", palette).on_click(cx.listener(
                                move |page, _, window, cx| {
                                    let (moves, ctx, id) = (page.moves.clone(), page.ctx.clone(), id.clone());
                                    // After this view's update: opening Errands replaces it.
                                    window.defer(cx, move |window, cx| {
                                        moves.open(&ctx, PageId::AgentErrands, Some(id), window, cx)
                                    });
                                },
                            )),
                        )
                    })
                    .child(
                        pill("memory-where-close", "Close", palette).on_click(cx.listener(|page, _, _, cx| {
                            page.picked = None;
                            cx.notify();
                        })),
                    ),
            )
            .into_any_element()
    }
}
