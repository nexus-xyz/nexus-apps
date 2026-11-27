//! Settings › Hooks (the Nexus app's `hooks-settings.tsx`): a shell command run when a Code session event
//! happens on this computer, and each hook's last runs. The hooks are `settings.json`'s, saved through
//! nexus-desktop-core (`Settings::set_hooks`) only while this build owns the store. The rules and the runner are
//! nexus-workbench's `hooks`, which runs them where the sessions run. An organization can turn them all off
//! (`HooksEnabled`): the list then stays, read-only.

use chrono::TimeZone;
use gpui_kit::component::input::InputState;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::hooks::{DEFAULT_TIMEOUT_SECS, Hook, HookEvent, HookRun, MAX_TIMEOUT_SECS, OFF_BY_POLICY};

use super::desktop_app::{off_thread, owned_note, read_only_note, save_settings};
use super::{DeviceCtx, palette, ui};
use crate::palette::Palette;

/// Whether the settings nav opens this page here (true) or in the browser.
pub const NATIVE: bool = true;

/// The lines of a run's output shown under it.
const SHOWN_LINES: usize = 3;

pub fn build(ctx: DeviceCtx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let hooks = crate::desktop_settings::load().hooks;
    let view = cx.new(|cx| {
        let rows = hooks.into_iter().map(|hook| Row::new(Some(hook), window, cx)).collect();
        let mut page = HooksPage {
            ctx,
            rows,
            runs: Vec::new(),
            problem: None,
            busy: false,
        };
        page.load_runs(cx);
        page
    });
    Some(view.into())
}

/// One hook as the form holds it.
struct Row {
    event: HookEvent,
    command: Entity<InputState>,
    /// Seconds as typed; empty: the default.
    timeout: Entity<InputState>,
    /// The saved hook's id, for its runs; None for a hook not saved yet.
    id: Option<String>,
}

impl Row {
    fn new(hook: Option<Hook>, window: &mut Window, cx: &mut Context<HooksPage>) -> Row {
        let id = hook.as_ref().map(Hook::id);
        let event = hook.as_ref().map_or(HookEvent::TurnFinished, |hook| hook.event);
        let command_text = hook.as_ref().map(|hook| hook.command.clone()).unwrap_or_default();
        let timeout_text = hook
            .as_ref()
            .and_then(|hook| hook.timeout_secs)
            .map(|secs| secs.to_string())
            .unwrap_or_default();
        let command = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("say Done")
                .default_value(command_text)
        });
        let timeout = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(DEFAULT_TIMEOUT_SECS.to_string())
                .default_value(timeout_text)
        });
        Row {
            event,
            command,
            timeout,
            id,
        }
    }
}

/// The timeout as typed: empty is the default (None), else whole seconds, clamped by the core when saved.
fn parse_timeout(typed: &str) -> Result<Option<u64>, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Ok(None);
    }
    typed
        .parse::<u64>()
        .map(Some)
        .map_err(|_| "Type a number of seconds.".to_string())
        .and_then(|secs| match secs {
            Some(secs) if !(1..=MAX_TIMEOUT_SECS).contains(&secs) => {
                Err(format!("Pick 1 to {MAX_TIMEOUT_SECS} seconds."))
            }
            secs => Ok(secs),
        })
}

/// How a run reads on the page: when, how it ended, and the end of its output.
fn run_lines(run: &HookRun) -> (String, String) {
    let when = chrono::Local
        .timestamp_millis_opt(run.started_at)
        .single()
        .map(|at| at.format("%b %-d, %H:%M").to_string())
        .unwrap_or_default();
    let output = nexus_workbench::hooks::last_lines(&run.output, SHOWN_LINES);
    (
        format!("{when} · {}", run.outcome()),
        if output.is_empty() { "No output.".into() } else { output },
    )
}

struct HooksPage {
    ctx: DeviceCtx,
    rows: Vec<Row>,
    runs: Vec<HookRun>,
    problem: Option<SharedString>,
    busy: bool,
}

impl HooksPage {
    fn load_runs(&mut self, cx: &mut Context<Self>) {
        off_thread(
            cx,
            || {
                crate::paths::data_dir()
                    .map(|data| nexus_desktop_core::hooks::runs_in(&data))
                    .unwrap_or_default()
            },
            |this, runs, _| this.runs = runs,
        );
    }

    fn hooks(&self, cx: &App) -> Result<Vec<Hook>, String> {
        self.rows
            .iter()
            .map(|row| {
                Ok(Hook {
                    event: row.event,
                    command: row.command.read(cx).value().to_string(),
                    timeout_secs: parse_timeout(row.timeout.read(cx).value().as_ref())?,
                })
            })
            .collect()
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let hooks = match self.hooks(cx) {
            Ok(hooks) => hooks,
            Err(error) => {
                self.problem = Some(error.into());
                cx.notify();
                return;
            }
        };
        self.busy = true;
        self.problem = None;
        save_settings(
            &self.ctx,
            cx,
            move |settings| settings.set_hooks(hooks),
            |this, result, cx| {
                this.busy = false;
                match result {
                    Ok((settings, ())) => {
                        for (row, hook) in this.rows.iter_mut().zip(&settings.hooks) {
                            row.id = Some(hook.id());
                        }
                        this.load_runs(cx);
                    }
                    Err(error) => this.problem = Some(error.into()),
                }
            },
        );
    }

    fn render_row(&self, ix: usize, row: &Row, editable: bool, palette: Palette, cx: &mut Context<Self>) -> Div {
        let chips = HookEvent::ALL.into_iter().map(|event| {
            let chip = ui::chip(
                SharedString::from(format!("hook-{ix}-{}", event.as_str())),
                event.label(),
                event == row.event,
                palette,
            );
            if editable {
                chip.on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(row) = this.rows.get_mut(ix) {
                        row.event = event;
                    }
                    cx.notify();
                }))
            } else {
                chip
            }
        });
        let remove = if editable {
            ui::ghost_button(("hook-remove", ix), "Remove", palette)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if ix < this.rows.len() {
                        this.rows.remove(ix);
                    }
                    cx.notify();
                }))
                .into_any_element()
        } else {
            ui::disabled_button(("hook-remove", ix), "Remove", palette).into_any_element()
        };
        let runs: Vec<&HookRun> = row
            .id
            .as_ref()
            .map(|id| self.runs.iter().filter(|run| &run.hook == id).collect())
            .unwrap_or_default();
        let mut card = ui::card(palette)
            .mt(px(8.))
            .gap(px(8.))
            .child(h_flex().flex_wrap().gap(px(6.)).children(chips))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(ui::input_box(&row.command, palette).font_family(ui::MONO))
                    .child(ui::input_box(&row.timeout, palette).flex_none().w(px(72.)))
                    .child(remove),
            );
        if runs.is_empty() {
            card = card.child(ui::note("No runs yet.", palette));
        }
        for run in runs {
            let (head, output) = run_lines(run);
            card = card.child(
                v_flex()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(if run.failed() {
                                palette.destructive
                            } else {
                                palette.muted
                            })
                            .child(head),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_family(ui::MONO)
                            .text_color(palette.faint)
                            .child(output),
                    ),
            );
        }
        card
    }
}

impl Render for HooksPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette(cx);
        let enabled = crate::managed::policy().hooks_enabled;
        let editable = enabled && crate::desktop_settings::writable() && !self.busy;
        let mut page = v_flex()
            .w_full()
            .child(ui::lead(
                format!(
                    "Run a shell command when a Code session on this computer finishes a turn, needs you, merges, is \
                     snoozed or archived, or fails a check. It runs in the session's folder with NEXUS_SESSION_ID and \
                     the other NEXUS_ variables, and stops after its timeout ({DEFAULT_TIMEOUT_SECS} seconds unless \
                     you set one)."
                ),
                palette,
            ))
            .children(read_only_note(palette))
            .children((!enabled).then(|| owned_note(OFF_BY_POLICY, palette)))
            .child(ui::section_head("Hooks", palette));
        let rows: Vec<Div> = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, row)| self.render_row(ix, row, editable, palette, cx))
            .collect();
        if rows.is_empty() {
            page = page.child(ui::note("No hooks yet.", palette));
        }
        page = page.children(rows);
        let add = if editable {
            ui::button("hook-add", "Add hook", palette)
                .on_click(cx.listener(|this, _, window, cx| {
                    let row = Row::new(None, window, cx);
                    this.rows.push(row);
                    cx.notify();
                }))
                .into_any_element()
        } else {
            ui::disabled_button("hook-add", "Add hook", palette).into_any_element()
        };
        let save = if editable {
            ui::primary_button("hook-save", "Save", palette)
                .on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                .into_any_element()
        } else {
            ui::disabled_button("hook-save", "Save", palette).into_any_element()
        };
        page.child(h_flex().mt(px(12.)).gap(px(8.)).child(add).child(save))
            .children(
                self.problem
                    .clone()
                    .map(|problem| ui::error_line(problem, palette).mt(px(8.))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui_kit::*` brings GPUI's own `test` attribute: the standard one is meant here.
    use core::prelude::v1::test;

    #[test]
    fn hooks_page_reads_the_timeout_as_whole_seconds() {
        assert_eq!(parse_timeout(""), Ok(None));
        assert_eq!(parse_timeout(" 90 "), Ok(Some(90)));
        assert!(parse_timeout("0").is_err());
        assert!(parse_timeout("601").is_err());
        assert_eq!(parse_timeout("abc"), Err("Type a number of seconds.".into()));
    }

    #[test]
    fn hooks_page_shows_how_a_run_ended_and_its_last_lines() {
        let run = HookRun {
            hook: "h".into(),
            event: "settled".into(),
            thread_id: "t".into(),
            started_at: 0,
            exit_code: None,
            timed_out: true,
            output: "one\ntwo\nthree\nfour".into(),
        };
        let (head, output) = run_lines(&run);
        assert!(head.ends_with("· timed out"), "{head}");
        assert_eq!(output, "two\nthree\nfour");
    }

    #[test]
    fn hooks_page_saves_through_the_core_and_refuses_an_empty_command() {
        let file = crate::desktop_settings::TempFile::new("hooks");
        let hook = Hook {
            event: HookEvent::CheckFailed,
            command: " notify-send failed ".into(),
            timeout_secs: Some(30),
        };
        let (saved, ()) = file.update(|settings| settings.set_hooks(vec![hook])).expect("saved");
        assert_eq!(saved.hooks[0].command, "notify-send failed");
        assert_eq!(nexus_desktop_core::settings::load(file.path()).hooks, saved.hooks);
        let refused = file.update(|settings| {
            settings.set_hooks(vec![Hook {
                event: HookEvent::Settled,
                command: "  ".into(),
                timeout_secs: None,
            }])
        });
        assert!(refused.is_err());
    }
}
