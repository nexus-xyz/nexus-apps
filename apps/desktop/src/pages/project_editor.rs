//! One project's settings, opened from Settings › Worktrees › Projects (the Nexus app's `setup-settings.tsx`,
//! `project-state-settings.tsx`, `project-scripts-settings.tsx` and `session-start-settings.tsx`): pin and archive, the
//! setup script, the commands with their keys, what happens when an agent hands off work, what a new session starts
//! with (the Nexus app's `project-defaults.tsx`), and whether Claude and OpenCode load the project's own settings (`nexus_workbench::claude_trust`).
//! Each change goes through the runner's `Projects` commands, or a server project's host ([`ProjectHost`]), so
//! `nexus_workbench` checks and keeps it as the Nexus app does.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::api::AgentInfo;
use nexus_workbench::claude_trust::ClaudeTrust;
use nexus_workbench::driver::label as agent_label;
use nexus_workbench::event::SessionAgent;
use nexus_workbench::project_defaults::{ProjectDefaults, Workspace};
use nexus_workbench::scripts::ProjectScript;

use super::project_host::{ProjectHost, Reply};
use super::{palette, ui};
use crate::palette::Palette;
use crate::project_commands::key_label;
use crate::workspace::PERMISSION_MODES;

/// What the editor opens with, read from the store.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub id: String,
    pub name: String,
    pub pinned: bool,
    pub archived: bool,
    /// A git project: its new sessions may work in a worktree.
    pub git: bool,
    pub setup: Option<String>,
    pub scripts: Vec<ProjectScript>,
}

/// One command as edited: its fields, and the key it keeps.
struct ScriptRow {
    id: String,
    name: Entity<InputState>,
    command: Entity<InputState>,
    shortcut: Option<String>,
}

/// Which part of the editor a save or an error belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    State,
    Defaults,
    Setup,
    Scripts,
    Trust,
}

pub struct ProjectEditor {
    projects: ProjectHost,
    saved: Saved,
    setup: Entity<TextareaState>,
    rows: Vec<ScriptRow>,
    /// The project's new-session defaults, once read.
    defaults: Option<ProjectDefaults>,
    /// Whether Claude loads the project's own settings, once read.
    trust: Option<ClaudeTrust>,
    /// The agents here and their models, for the model choice.
    agents: Vec<AgentInfo>,
    /// The part being saved: one at a time.
    busy: Option<Part>,
    problem: Option<(Part, SharedString)>,
    /// The command whose key is being recorded, and the recorder.
    recording: Option<(String, Subscription)>,
    /// Why the key pressed can't be the command's.
    key_note: Option<(String, SharedString)>,
    /// Tell the page to read the projects again.
    changed: Box<dyn Fn(&mut App)>,
    _subscriptions: Vec<Subscription>,
}

/// The key button's width, so every command's fields line up whatever its key.
const KEY_WIDTH: f32 = 120.;

const KEY_HINT: &str = "Press the keys, Delete to clear, Esc to stop";

/// The keys the system keeps, though Code has no shortcut on them (the Nexus app's `script-keys.ts`).
const RESERVED: [&str; 4] = [
    "Mod+Shift+KeyZ",
    "Mod+Shift+Digit3",
    "Mod+Shift+Digit4",
    "Mod+Shift+Digit5",
];

/// A key press as a command's key, as the Nexus app writes it (`Mod+Alt+Shift+KeyR`): Mod, then Alt or Shift or
/// both, then a letter, a digit or F1 to F12. None: a modifier alone, so wait for more. `mac`: Mod is ⌘, else Ctrl.
fn record_key(keystroke: &Keystroke, others: &[Option<String>], mac: bool) -> Option<Result<String, String>> {
    let modifiers = keystroke.modifiers;
    let key = keystroke.key.as_str();
    if matches!(key, "shift" | "control" | "alt" | "platform" | "function" | "capslock") {
        return None;
    }
    let (mod_key, other) = if mac {
        (modifiers.platform, modifiers.control)
    } else {
        (modifiers.control, modifiers.platform)
    };
    let (ctrl, pair) = if mac {
        ("⌘", "⌥ or ⇧")
    } else {
        ("Ctrl", "Alt or Shift")
    };
    if !mod_key || other {
        return Some(Err(format!("Start with {ctrl}, then add {pair} and a key.")));
    }
    if !modifiers.alt && !modifiers.shift {
        return Some(Err(format!("Add {pair}: {ctrl} with a key alone belongs to the app.")));
    }
    let code = match key.as_bytes() {
        [letter] if letter.is_ascii_alphabetic() => format!("Key{}", key.to_ascii_uppercase()),
        [digit] if digit.is_ascii_digit() => format!("Digit{key}"),
        _ => match key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
            Some(n @ 1..=12) => format!("F{n}"),
            _ => return Some(Err("Use a letter, a digit or F1 to F12.".into())),
        },
    };
    let mut parts = vec!["Mod"];
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    parts.push(&code);
    let text = parts.join("+");
    if RESERVED.contains(&text.as_str()) {
        return Some(Err("The system keeps that key.".into()));
    }
    if others.iter().flatten().any(|other| *other == text) {
        return Some(Err("Another command of this project has that key.".into()));
    }
    Some(Ok(text))
}

/// A new command's id, in the form the workbench accepts (`c-` and eight hex digits, as the Nexus app makes them).
fn new_id() -> String {
    format!("c-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

impl ProjectEditor {
    pub fn new(
        projects: ProjectHost,
        saved: Saved,
        changed: impl Fn(&mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let setup = cx.new(|cx| TextareaState::new(window, cx).rows(2).placeholder("No setup script"));
        let text = saved.setup.clone().unwrap_or_default();
        setup.update(cx, |state, cx| state.set_value(text, window, cx));
        let _subscriptions = vec![cx.observe(&setup, |_, _, cx| cx.notify())];
        let mut editor = ProjectEditor {
            projects,
            saved,
            setup,
            rows: Vec::new(),
            defaults: None,
            trust: None,
            agents: Vec::new(),
            busy: None,
            problem: None,
            recording: None,
            key_note: None,
            changed: Box::new(changed),
            _subscriptions,
        };
        editor.reset_scripts(window, cx);
        let defaults = editor.projects.defaults(editor.saved.id.clone());
        editor.answer(
            Part::Defaults,
            defaults,
            |this, defaults, _| this.defaults = Some(defaults),
            cx,
        );
        let agents = editor.projects.agent_models();
        editor.answer(Part::Defaults, agents, |this, agents, _| this.agents = agents, cx);
        let trust = editor.projects.claude_trust(editor.saved.id.clone());
        editor.answer(Part::Trust, trust, |this, trust, _| this.trust = Some(trust), cx);
        editor
    }

    /// The command rows as saved.
    fn reset_scripts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = None;
        self.key_note = None;
        self.rows = self
            .saved
            .scripts
            .clone()
            .into_iter()
            .map(|script| self.script_row(script, window, cx))
            .collect();
    }

    fn script_row(&self, script: ProjectScript, window: &mut Window, cx: &mut Context<Self>) -> ScriptRow {
        let field = |text: String, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            input.update(cx, |state, cx| state.set_value(text, window, cx));
            cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
            input
        };
        ScriptRow {
            id: script.id,
            name: field(script.name, "Name", window, cx),
            command: field(script.command, "npm run dev", window, cx),
            shortcut: script.shortcut,
        }
    }

    /// The commands as typed now.
    fn typed_scripts(&self, cx: &App) -> Vec<ProjectScript> {
        self.rows
            .iter()
            .map(|row| ProjectScript {
                id: row.id.clone(),
                name: row.name.read(cx).value().to_string(),
                command: row.command.read(cx).value().to_string(),
                shortcut: row.shortcut.clone(),
            })
            .collect()
    }

    /// Whether the typed commands differ from the saved ones, as the workbench would keep them.
    fn scripts_changed(&self, cx: &App) -> bool {
        let typed = self.typed_scripts(cx);
        typed.len() != self.saved.scripts.len()
            || typed.iter().zip(&self.saved.scripts).any(|(typed, saved)| {
                typed.id != saved.id
                    || typed.name.trim() != saved.name
                    || typed.command.trim() != saved.command
                    || typed.shortcut != saved.shortcut
            })
    }

    fn setup_changed(&self, cx: &App) -> bool {
        self.setup.read(cx).value().trim() != self.saved.setup.as_deref().unwrap_or("")
    }

    /// Await a command's answer for `part`, then apply it.
    fn answer<T: Send + 'static>(
        &mut self,
        part: Part,
        answer: Reply<T>,
        apply: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = answer.await;
            this.update(cx, |this, cx| {
                if this.busy == Some(part) {
                    this.busy = None;
                }
                match result {
                    Ok(value) => apply(this, value, cx),
                    Err(problem) => this.problem = Some((part, problem.into())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Start a save for `part`, unless one is running.
    fn begin(&mut self, part: Part, cx: &mut Context<Self>) -> bool {
        if self.busy.is_some() {
            return false;
        }
        self.busy = Some(part);
        self.problem = None;
        cx.notify();
        true
    }

    fn set_state(&mut self, pinned: Option<bool>, archived: Option<bool>, cx: &mut Context<Self>) {
        if !self.begin(Part::State, cx) {
            return;
        }
        let answer = self.projects.update(self.saved.id.clone(), pinned, archived);
        self.answer(
            Part::State,
            answer,
            move |this, (), cx| {
                if let Some(pinned) = pinned {
                    this.saved.pinned = pinned;
                }
                if let Some(archived) = archived {
                    this.saved.archived = archived;
                }
                (this.changed)(cx);
            },
            cx,
        );
    }

    fn save_setup(&mut self, cx: &mut Context<Self>) {
        if !self.begin(Part::Setup, cx) {
            return;
        }
        let typed = self.setup.read(cx).value().to_string();
        let answer = self.projects.set_setup(self.saved.id.clone(), typed);
        self.answer(
            Part::Setup,
            answer,
            |this, saved, cx| {
                this.saved.setup = saved;
                (this.changed)(cx);
            },
            cx,
        );
    }

    fn cancel_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.saved.setup.clone().unwrap_or_default();
        self.setup.update(cx, |state, cx| state.set_value(text, window, cx));
        self.problem = None;
        cx.notify();
    }

    fn save_scripts(&mut self, cx: &mut Context<Self>) {
        if !self.begin(Part::Scripts, cx) {
            return;
        }
        let answer = self.projects.set_scripts(self.saved.id.clone(), self.typed_scripts(cx));
        self.answer(
            Part::Scripts,
            answer,
            |this, saved, cx| {
                crate::project_commands::saved(&this.saved.id, saved.clone(), cx);
                this.saved.scripts = saved;
                this.recording = None;
                this.key_note = None;
                (this.changed)(cx);
            },
            cx,
        );
    }

    fn add_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.script_row(
            ProjectScript {
                id: new_id(),
                name: String::new(),
                command: String::new(),
                shortcut: None,
            },
            window,
            cx,
        );
        row.name.update(cx, |state, cx| state.focus(window, cx));
        self.rows.push(row);
        cx.notify();
    }

    fn remove_script(&mut self, id: &str, cx: &mut Context<Self>) {
        self.rows.retain(|row| row.id != id);
        if self.recording.as_ref().is_some_and(|(recording, _)| recording == id) {
            self.recording = None;
        }
        cx.notify();
    }

    /// Record the command's key: the next key press is its, not the page's or Code's.
    fn record(&mut self, id: String, cx: &mut Context<Self>) {
        if self.recording.as_ref().is_some_and(|(recording, _)| *recording == id) {
            self.recording = None;
            cx.notify();
            return;
        }
        let this = cx.entity().downgrade();
        let recorder = cx.intercept_keystrokes(move |event, _, cx| {
            let keystroke = event.keystroke.clone();
            cx.stop_propagation();
            let _ = this.update(cx, |editor, cx| editor.recorded(&keystroke, cx));
        });
        self.key_note = None;
        self.recording = Some((id, recorder));
        cx.notify();
    }

    fn recorded(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let Some(id) = self.recording.as_ref().map(|(id, _)| id.clone()) else {
            return;
        };
        let plain = !keystroke.modifiers.modified();
        match keystroke.key.as_str() {
            "escape" if plain => self.recording = None,
            "backspace" | "delete" if plain => {
                self.set_key(&id, None);
                self.recording = None;
                self.key_note = None;
            }
            _ => {
                let others: Vec<Option<String>> = self
                    .rows
                    .iter()
                    .filter(|row| row.id != id)
                    .map(|row| row.shortcut.clone())
                    .collect();
                match record_key(keystroke, &others, cfg!(target_os = "macos")) {
                    None => return,
                    Some(Err(problem)) => self.key_note = Some((id, problem.into())),
                    Some(Ok(key)) => {
                        self.set_key(&id, Some(key));
                        self.recording = None;
                        self.key_note = None;
                    }
                }
            }
        }
        cx.notify();
    }

    fn set_key(&mut self, id: &str, key: Option<String>) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.id == id) {
            row.shortcut = key;
        }
    }

    fn set_trust(&mut self, trusted: bool, cx: &mut Context<Self>) {
        if !self.begin(Part::Trust, cx) {
            return;
        }
        let answer = self.projects.set_claude_trust(self.saved.id.clone(), trusted);
        self.answer(Part::Trust, answer, |this, saved, _| this.trust = Some(saved), cx);
    }

    /// Save the defaults as `change` makes them.
    fn set_defaults(&mut self, change: impl FnOnce(&mut ProjectDefaults), cx: &mut Context<Self>) {
        let Some(mut next) = self.defaults.clone() else { return };
        change(&mut next);
        if !self.begin(Part::Defaults, cx) {
            return;
        }
        let answer = self.projects.set_defaults(self.saved.id.clone(), next);
        self.answer(Part::Defaults, answer, |this, saved, _| this.defaults = Some(saved), cx);
    }

    /// One default's choices as chips: "Last used" first, then `choices`; the picked one raised.
    fn chips<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        picked: Option<T>,
        choices: Vec<(Option<T>, SharedString)>,
        set: fn(&mut ProjectDefaults, Option<T>),
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.busy.is_some() || self.defaults.is_none();
        h_flex().gap(px(6.)).flex_wrap().children(
            std::iter::once((None, SharedString::from("Last used")))
                .chain(choices)
                .enumerate()
                .map(|(at, (value, text))| {
                    let chip = ui::chip(SharedString::from(format!("{id}-{at}")), text, picked == value, palette);
                    if busy {
                        chip
                    } else {
                        chip.on_click(
                            cx.listener(move |this, _, _, cx| this.set_defaults(|defaults| set(defaults, value), cx)),
                        )
                    }
                }),
        )
    }

    fn render_defaults(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let defaults = self.defaults.clone().unwrap_or_default();
        let agents = SessionAgent::ALL
            .into_iter()
            .filter(|agent| agent.supported_on_current_platform())
            .map(|agent| (Some(agent), SharedString::from(agent_label(agent))))
            .collect();
        let agent_row = self.chips(
            "defaults-agent",
            defaults.agent,
            agents,
            |defaults, agent| {
                defaults.agent = agent;
                defaults.model = None;
            },
            palette,
            cx,
        );
        let models = defaults.agent.map(|agent| {
            let mut ids: Vec<String> = self
                .agents
                .iter()
                .find(|info| info.agent == agent)
                .and_then(|info| info.models.as_ref())
                .map(|models| models.iter().map(|model| model.id.clone()).collect())
                .unwrap_or_default();
            // The saved model shows even when this computer's agent does not list it.
            if let Some(model) = &defaults.model
                && !ids.contains(model)
            {
                ids.push(model.clone());
            }
            self.model_chips(defaults.model.as_deref(), ids, palette, cx)
        });
        let mode_row = if let Some(name) = defaults.agent.and_then(SessionAgent::permission_settings) {
            ui::note(format!("{name} · The agent decides when to ask."), palette)
        } else {
            let modes = PERMISSION_MODES
                .iter()
                .map(|(mode, label, _)| (Some(*mode), SharedString::from(*label)))
                .collect();
            self.chips(
                "defaults-mode",
                defaults.mode,
                modes,
                |defaults, mode| defaults.mode = mode,
                palette,
                cx,
            )
        };
        let workspace_row = self.saved.git.then(|| {
            self.chips(
                "defaults-workspace",
                defaults.workspace,
                vec![
                    (Some(Workspace::Folder), "This folder".into()),
                    (Some(Workspace::Worktree), "New worktree".into()),
                ],
                |defaults, workspace| defaults.workspace = workspace,
                palette,
                cx,
            )
        });
        v_flex()
            .gap(px(8.))
            .child(ui::label("New sessions start with", palette))
            .child(ui::note(
                "Each one left on Last used follows what you last picked for a new session.",
                palette,
            ))
            .child(ui::setting_lines("Agent and model", "", palette))
            .child(agent_row)
            .children(models)
            .child(ui::setting_lines("Permissions", "", palette))
            .child(mode_row)
            .children(workspace_row.map(|row| {
                v_flex()
                    .gap(px(8.))
                    .child(ui::setting_lines("Works in", "", palette))
                    .child(row)
            }))
            .children(self.problem_of(Part::Defaults, palette))
    }

    /// The agent's models: the organization's default first, then each listed one.
    fn model_chips(&self, picked: Option<&str>, ids: Vec<String>, palette: Palette, cx: &mut Context<Self>) -> Div {
        let busy = self.busy.is_some();
        h_flex().gap(px(6.)).flex_wrap().children(
            std::iter::once(None)
                .chain(ids.into_iter().map(Some))
                .enumerate()
                .map(|(at, model)| {
                    let text: SharedString = model.clone().unwrap_or_else(|| "Organization default".into()).into();
                    let chip = ui::chip(
                        SharedString::from(format!("defaults-model-{at}")),
                        text,
                        picked == model.as_deref(),
                        palette,
                    );
                    if busy {
                        chip
                    } else {
                        chip.on_click(cx.listener(move |this, _, _, cx| {
                            let model = model.clone();
                            this.set_defaults(|defaults| defaults.model = model, cx)
                        }))
                    }
                }),
        )
    }

    fn problem_of(&self, part: Part, palette: Palette) -> Option<Div> {
        self.problem
            .as_ref()
            .filter(|(at, _)| *at == part)
            .map(|(_, problem)| ui::error_line(problem.clone(), palette))
    }

    /// Cancel and Save, shown while `part` has unsaved changes.
    fn save_bar(
        &self,
        part: Part,
        on_cancel: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        on_save: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = |name: &str| SharedString::from(format!("project-{part:?}-{name}"));
        if self.busy.is_some() {
            return h_flex()
                .gap(px(6.))
                .child(ui::disabled_button(id("cancel"), "Cancel", palette))
                .child(ui::disabled_button(id("save"), "Save", palette));
        }
        h_flex()
            .gap(px(6.))
            .child(
                ui::ghost_button(id("cancel"), "Cancel", palette)
                    .on_click(cx.listener(move |this, _, window, cx| on_cancel(this, window, cx))),
            )
            .child(
                ui::primary_button(id("save"), "Save", palette)
                    .on_click(cx.listener(move |this, _, _, cx| on_save(this, cx))),
            )
    }

    fn render_state(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let (pinned, archived) = (self.saved.pinned, self.saved.archived);
        let button = |id: &'static str, text: &'static str| {
            if self.busy.is_some() {
                ui::disabled_button(id, text, palette)
            } else {
                ui::button(id, text, palette)
            }
        };
        let busy = self.busy.is_some();
        h_flex()
            .gap(px(6.))
            .child(
                button("project-pin", if pinned { "Unpin" } else { "Pin" }).when(!busy, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| this.set_state(Some(!pinned), None, cx)))
                }),
            )
            .child(
                button("project-archive", if archived { "Unarchive" } else { "Archive" }).when(!busy, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| this.set_state(None, Some(!archived), cx)))
                }),
            )
    }

    fn render_setup(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        v_flex()
            .gap(px(6.))
            .child(ui::label("Setup script", palette))
            .child(
                div()
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .bg(palette.hover)
                    .text_size(px(13.))
                    .font_family(ui::MONO)
                    .text_color(palette.foreground)
                    .child(Textarea::new(&self.setup).appearance(false)),
            )
            .children(self.problem_of(Part::Setup, palette))
            .when(self.setup_changed(cx), |column| {
                column.child(h_flex().justify_end().child(self.save_bar(
                    Part::Setup,
                    |this, window, cx| this.cancel_setup(window, cx),
                    |this, cx| this.save_setup(cx),
                    palette,
                    cx,
                )))
            })
    }

    fn render_script(&self, row: &ScriptRow, palette: Palette, cx: &mut Context<Self>) -> Div {
        let id = row.id.clone();
        let recording = self.recording.as_ref().is_some_and(|(at, _)| *at == id);
        let label = row
            .shortcut
            .as_deref()
            .map(|key| key_label(key, cfg!(target_os = "macos")))
            .filter(|label| !label.is_empty());
        let key_text: SharedString = match (recording, label) {
            (true, _) => "Press keys…".into(),
            (false, Some(label)) => label.into(),
            (false, None) => "Add key".into(),
        };
        let element_id = |name: &str| SharedString::from(format!("script-{id}-{name}"));
        let busy = self.busy.is_some();
        v_flex()
            .gap(px(4.))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(div().w(px(144.)).child(ui::input_box(&row.name, palette)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(ui::input_box(&row.command, palette).font_family(ui::MONO)),
                    )
                    .child(if busy {
                        ui::disabled_button(element_id("key"), key_text, palette)
                            .w(px(KEY_WIDTH))
                            .into_any_element()
                    } else {
                        let key_id = id.clone();
                        ui::ghost_button(element_id("key"), key_text, palette)
                            .w(px(KEY_WIDTH))
                            .font_family(ui::MONO)
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(KEY_HINT).build(window, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.record(key_id.clone(), cx)))
                            .into_any_element()
                    })
                    .child({
                        let remove_id = id.clone();
                        div()
                            .id(element_id("remove"))
                            .flex_none()
                            .p(px(6.))
                            .rounded(px(6.))
                            .when(!busy, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(palette.hover))
                                    .on_click(cx.listener(move |this, _, _, cx| this.remove_script(&remove_id, cx)))
                            })
                            .child(ui::icon(Lucide::X, palette.muted).size(px(13.)))
                    }),
            )
            .children(
                self.key_note
                    .as_ref()
                    .filter(|(at, _)| *at == id)
                    .map(|(_, note)| ui::note(note.clone(), palette)),
            )
    }

    fn render_scripts(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let busy = self.busy.is_some();
        let add = if busy {
            ui::disabled_button("script-add", "Add a command", palette)
        } else {
            ui::ghost_button("script-add", "Add a command", palette)
                .on_click(cx.listener(|this, _, window, cx| this.add_script(window, cx)))
        };
        v_flex()
            .gap(px(6.))
            .child(ui::label("Commands", palette))
            .child(ui::note(
                "Each runs in a new terminal of the session's folder, from the Nexus app's top bar, Ctrl+K or its key.",
                palette,
            ))
            .children(self.rows.iter().map(|row| self.render_script(row, palette, cx)))
            .child(
                h_flex()
                    .justify_between()
                    .child(add)
                    .when(self.scripts_changed(cx), |bar| {
                        bar.child(self.save_bar(
                            Part::Scripts,
                            |this, window, cx| {
                                this.reset_scripts(window, cx);
                                this.problem = None;
                                cx.notify();
                            },
                            |this, cx| this.save_scripts(cx),
                            palette,
                            cx,
                        ))
                    }),
            )
            .children(self.problem_of(Part::Scripts, palette))
    }

    fn render_trust(&self, palette: Palette, cx: &mut Context<Self>) -> Div {
        let busy = self.busy.is_some() || self.trust.is_none();
        let trusted = self.trust.as_ref().and_then(|trust| trust.trusted) == Some(true);
        let switch = ui::switch("project-claude-trust", trusted, busy, palette);
        let switch = if busy {
            switch.into_any_element()
        } else {
            switch
                .on_click(cx.listener(move |this, _, _, cx| this.set_trust(!trusted, cx)))
                .into_any_element()
        };
        v_flex()
            .gap(px(6.))
            .child(ui::label("Agent settings", palette))
            .child(ui::setting(
                "Trust this project's Claude and OpenCode settings",
                trust_note(self.trust.as_ref()),
                switch,
                palette,
            ))
            .children(self.problem_of(Part::Trust, palette))
    }
}

/// What the trust switch's note says: what the project's settings would turn on, and what off means.
fn trust_note(trust: Option<&ClaudeTrust>) -> String {
    let off = "Off: Claude and OpenCode use only your own settings here.";
    let Some(trust) = trust else {
        return off.into();
    };
    let has: Vec<String> = [("Claude", &trust.found), ("OpenCode", &trust.opencode)]
        .into_iter()
        .filter(|(_, found)| found.asks())
        .map(|(agent, found)| format!("{agent}: {}", found.summary()))
        .collect();
    if has.is_empty() {
        format!("It has none that turn anything on. {off}")
    } else {
        format!("It has {}. {off}", has.join("; "))
    }
}

impl Render for ProjectEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        v_flex()
            .w_full()
            .gap(px(16.))
            .pt(px(4.))
            .pb(px(12.))
            .child(self.render_state(palette, cx))
            .children(self.problem_of(Part::State, palette))
            .child(self.render_defaults(palette, cx))
            .child(self.render_setup(palette, cx))
            .child(self.render_scripts(palette, cx))
            .child(self.render_trust(palette, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn press(text: &'static str) -> Keystroke {
        Keystroke::parse(text).expect("a keystroke")
    }

    #[test]
    fn records_a_key_as_the_nexus_app_writes_it() {
        assert_eq!(
            record_key(&press("ctrl-alt-r"), &[], false),
            Some(Ok("Mod+Alt+KeyR".into()))
        );
        assert_eq!(
            record_key(&press("ctrl-shift-1"), &[], false),
            Some(Ok("Mod+Shift+Digit1".into()))
        );
        assert_eq!(
            record_key(&press("ctrl-alt-shift-f5"), &[], false),
            Some(Ok("Mod+Alt+Shift+F5".into()))
        );
        assert_eq!(
            record_key(&press("cmd-alt-r"), &[], true),
            Some(Ok("Mod+Alt+KeyR".into()))
        );
        assert_eq!(record_key(&press("shift"), &[], false), None);
        assert!(matches!(record_key(&press("ctrl-r"), &[], false), Some(Err(_))));
        assert!(matches!(record_key(&press("alt-r"), &[], false), Some(Err(_))));
        assert!(matches!(record_key(&press("ctrl-alt-space"), &[], false), Some(Err(_))));
        // Redo and the Mac's screenshot keys stay the system's.
        assert!(matches!(record_key(&press("ctrl-shift-z"), &[], false), Some(Err(_))));
        assert!(matches!(record_key(&press("ctrl-shift-5"), &[], false), Some(Err(_))));
        let taken = [Some("Mod+Alt+KeyR".to_string())];
        assert!(matches!(record_key(&press("ctrl-alt-r"), &taken, false), Some(Err(_))));
    }

    #[test]
    fn labels_a_key_for_the_platform() {
        assert_eq!(key_label("Mod+Alt+KeyR", false), "Ctrl+Alt+R");
        assert_eq!(key_label("Mod+Shift+Digit1", true), "⌘⇧1");
        assert_eq!(key_label("Alt+KeyR", false), "");
        let id = new_id();
        assert!(id.starts_with("c-") && id.len() == 10);
    }
}
