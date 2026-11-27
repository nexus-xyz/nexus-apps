//! A command in the Commands pane: one 34px row that reads at a glance, and its details under it when open.
use super::list::{self, Tone};
use super::*;
use crate::side::well;

/// How long a row that just arrived, or a mark that just changed, keeps its entrance.
const FRESH: Duration = Duration::from_millis(400);
pub(crate) const ROW: f32 = 34.;
/// The row's corner radius; a finished row's countdown bar is inset by it.
pub(crate) const ROW_RADIUS: f32 = 6.;
/// The space between a row's parts.
const ROW_GAP: f32 = 8.;

/// What a row needs from the pane besides its command.
pub(super) struct RowState<'a> {
    pub words: &'a [String],
    pub open: bool,
    pub cursor: bool,
    pub motion: bool,
    pub now_ms: i64,
}

impl SharedTerminals {
    pub(super) fn render_row(
        &self,
        entry: &Entry,
        state: RowState<'_>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = entry.id.clone();
        let (line, more) = list::first_line(&entry.command);
        let label = list::label(&entry.name, &entry.command);
        let tone = list::tone(entry);
        let changed_at = self.changed.get(&id).copied();
        let changed = state.motion && changed_at.is_some_and(|at| at.elapsed() < FRESH);
        let row = h_flex()
            .id(SharedString::from(format!("command-row-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("command-row-{id}")
            })
            .group("command-row")
            .w_full()
            .h(px(ROW))
            .flex_none()
            .px(px(8.))
            .gap(px(ROW_GAP))
            .rounded(px(ROW_RADIUS))
            .cursor_pointer()
            .when(state.cursor, |row| row.bg(palette.selected))
            .when(!state.cursor, |row| row.hover(|style| style.bg(palette.hover)))
            .active(|style| style.opacity(0.8))
            .child(
                glyph(
                    if state.open {
                        Lucide::ChevronDown
                    } else {
                        Lucide::ChevronRight
                    },
                    palette.faint,
                )
                .size(px(12.))
                .flex_none(),
            )
            .child(mark(entry, tone, changed, state.motion, palette))
            .children(label.clone().map(|name| {
                div()
                    .flex_none()
                    .max_w(relative(0.45))
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .child(marked(name, state.words, palette))
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_family(ui::MONO)
                    .text_size(px(12.))
                    // With no name of its own, the command is the row's title.
                    .text_color(if label.is_some() {
                        palette.faint
                    } else {
                        palette.foreground
                    })
                    .child(marked(line.to_string(), state.words, palette)),
            )
            .when(more > 0, |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(palette.faint)
                        .child(format!("+{more}")),
                )
            })
            .children(word(entry, tone, changed_at, state.motion, palette))
            .children(list::age(entry.created_at, state.now_ms).map(|age| {
                div()
                    .flex_none()
                    .min_w(px(22.))
                    .text_right()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(age)
            }))
            .children(self.render_row_actions(entry, state.cursor, palette, cx))
            .tooltip({
                let command = list::tooltip(&entry.command);
                move |window, cx| gpui_kit::component::tooltip::Tooltip::new(command.clone()).build(window, cx)
            })
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, window, cx| {
                    this.selected = Some(id.clone());
                    if !this.collapsed.remove(&id) {
                        this.collapsed.insert(id.clone());
                    }
                    this.list_focus.focus(window, cx);
                    cx.notify();
                }
            }));
        let arrived = state.motion && self.arrived.get(&id).is_some_and(|at| at.elapsed() < FRESH);
        if arrived {
            crate::motion::arrive(row, SharedString::from(format!("command-arrive-{id}")), 4.).into_any_element()
        } else {
            row.into_any_element()
        }
    }

    /// A command's actions at the row's right end, so none needs the row opened. A request shows Decline and Run;
    /// a running command Use shell (the agent's command only) and Stop; an ended one, in History, a Delete icon
    /// that shows on hover or under the keyboard. The last action always ends the row, so it sits in one place down
    /// the list. While another action applies, they stay in place, dimmed, and take no click. Once the person asks a
    /// running command to stop, its Use shell and Stop narrow out of the row and take no click.
    fn render_row_actions(
        &self,
        entry: &Entry,
        cursor: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.read_only {
            return None;
        }
        let ready = !self.busy;
        let id = entry.id.clone();
        let actions = h_flex().flex_none().gap(px(8.));
        if entry.needs_attention() {
            let decline = {
                let id = id.clone();
                row_button(&id, "decline", "Decline", false, ready, palette).when(ready, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        // The row under it opens and closes on a click; this one only acts.
                        cx.stop_propagation();
                        this.perform("decline", json!({"id": id}), cx);
                    }))
                })
            };
            let run = {
                let id = id.clone();
                let expected = entry.command.clone();
                // An edit open in the details is what runs.
                let input = self
                    .editing
                    .as_ref()
                    .filter(|(editing, _)| editing == &id)
                    .map(|(_, input)| input.clone());
                row_button(&id, "run", "Run", true, ready, palette).when(ready, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let command = input
                            .as_ref()
                            .map(|input| input.read(cx).value().to_string())
                            .unwrap_or_else(|| expected.clone());
                        this.perform(
                            "execute",
                            json!({"id": id, "command": command, "expected_command": expected}),
                            cx,
                        );
                    }))
                })
            };
            return Some(actions.child(decline).child(run).into_any_element());
        }
        if entry.attached() && entry.status == "running" {
            // A stop asked keeps the buttons drawn while they narrow out, so the row's words slide instead of jumping.
            let live = !entry.stop_requested;
            let take = {
                let id = id.clone();
                row_button(&id, "take", "Use shell", false, ready, palette).when(ready && live, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.perform("take", json!({"id": id}), cx);
                    }))
                })
            };
            let stop = {
                let id = id.clone();
                row_button(&id, "stop", "Stop", false, ready, palette).when(ready && live, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.perform("stop", json!({"id": id}), cx);
                    }))
                })
            };
            // An open row with its output in a well can give the well the whole pane.
            let maximize = (live && !self.collapsed.contains(&id) && self.outputs.contains_key(&id)).then(|| {
                let key = SharedString::from(id.clone());
                well::maximize_button(&id, self.wells.is_maximized(&id), palette).on_click(cx.listener(
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.wells.toggle_maximized(&key);
                        cx.notify();
                    },
                ))
            });
            let actions = actions
                .children(maximize)
                .when(entry.owner != "user", |actions| actions.child(take))
                .child(stop);
            return Some(slot(
                format!("command-run-actions-{id}"),
                live,
                Some(actions.into_any_element()),
            ));
        }
        if !entry.active() && !self.lingering(&entry.id) {
            let delete = div()
                .id(SharedString::from(format!("command-delete-{id}")))
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .child(glyph(Lucide::Trash, palette.icon).size(px(13.)))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Delete from command history").build(window, cx)
                })
                // Out of the way until the row is pointed at or chosen with the keyboard.
                .when(!cursor, |button| {
                    button.opacity(0.).group_hover("command-row", |style| style.opacity(1.))
                })
                .when(ready, |button| {
                    button
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.destructive.opacity(0.12)))
                        .active(|style| style.bg(palette.destructive.opacity(0.22)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.delete_confirmation = Some(vec![id.clone()]);
                            cx.notify();
                        }))
                });
            return Some(actions.child(delete).into_any_element());
        }
        None
    }

    /// The open row's details: why the agent asked, the whole command, its folder, its controls and its output.
    pub(super) fn render_details(
        &self,
        entry: &Entry,
        motion: bool,
        palette: Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = entry.id.clone();
        let mut body = v_flex()
            .w_full()
            .min_w(px(0.))
            .gap(px(8.))
            .pl(px(well::INDENT))
            .pr(px(8.))
            .pt(px(2.))
            .pb(px(well::FOOT));
        body = body.children(entry.reason.as_ref().map(|reason| {
            ui::selectable_note(
                SharedString::from(format!("command-reason-{id}")),
                reason.clone(),
                palette,
            )
        }));
        if let Some((editing, input)) = &self.editing
            && editing == &id
        {
            body = body.child(Textarea::new(input));
        } else {
            body = body.child(super::render::command_text(&id, &entry.command, palette));
        }
        // Only the path is selectable, so a copy of the line is a path the person can paste.
        body = body.child(
            h_flex()
                .items_start()
                .gap(px(4.))
                .child(ui::note("Folder:", palette))
                .child(
                    ui::selectable_note(
                        SharedString::from(format!("command-folder-{id}")),
                        entry.cwd.clone(),
                        palette,
                    )
                    .flex_1()
                    .min_w(px(0.)),
                )
                .children(
                    self.wells
                        .is_maximized(&id)
                        .then(|| well::hidden_note(self.pane_list.ids().count().saturating_sub(1), palette))
                        .flatten(),
                ),
        );
        for note in notes(entry) {
            body = body.child(ui::note(note, palette));
        }
        if !entry.private
            && let Some((count, inputs)) = self.submitted_inputs.get(&entry.id)
        {
            body = body.child(ui::note(
                format!(
                    "Agent shell input · showing {} of {count} requests. Individual command exits are unconfirmed.",
                    inputs.len()
                ),
                palette,
            ));
            for input in inputs {
                body = body.child(
                    div()
                        .px(px(12.))
                        .py(px(6.))
                        .rounded(px(6.))
                        .bg(palette.background)
                        .font_family(ui::MONO)
                        .text_size(px(12.))
                        .child(input.clone()),
                );
            }
        }
        body = body.child(self.render_controls(entry, palette, cx));
        if entry.attached()
            && !matches!(
                entry.status.as_str(),
                "awaitingUser" | "declined" | "denied" | "cancelled"
            )
            && !entry.private
            && let Some(output) = self.outputs.get(&entry.id)
        {
            // The shell's own insets clear the well's edge: the well adds none.
            let shell = output.view.read(cx);
            let look = well::Look {
                focused: shell.focus_handle().contains_focused(window, cx),
                size: Some(shell.size()),
            };
            body = body.child(well::render(
                SharedString::from(id.clone()),
                &self.wells,
                |this: &mut Self| &mut this.wells,
                output.view.clone().into_any_element(),
                look,
                palette,
                cx,
            ));
        }
        if motion {
            crate::motion::arrive(body, SharedString::from(format!("command-open-{id}")), -4.).into_any_element()
        } else {
            body.into_any_element()
        }
    }

    /// The open row's buttons, for what needs the whole command in view: edit a request before it runs, or review a
    /// declined one again. Decide, Use shell, Stop and Delete live on the row itself (`render_row_actions`).
    fn render_controls(&self, entry: &Entry, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = entry.id.clone();
        let mut controls = h_flex().gap(px(8.)).flex_wrap();
        let editing = self.editing.as_ref().is_some_and(|(editing, _)| editing == &id);
        if entry.needs_attention() && !editing && !self.busy && !self.read_only {
            let command = entry.command.clone();
            let edit = id.clone();
            controls = controls.child(
                ui::ghost_button(SharedString::from(format!("native-edit-{id}")), "Edit", palette).on_click(
                    cx.listener(move |this, _, window, cx| {
                        let command = command.clone();
                        let input = cx.new(|cx| TextareaState::new(window, cx).rows(4).default_value(command));
                        input.update(cx, |input, cx| input.focus(window, cx));
                        this.pending_rows.expand(&edit);
                        this.collapsed.remove(&edit);
                        this.editing = Some((edit.clone(), input));
                        cx.notify();
                    }),
                ),
            );
        }
        if entry.status == "declined" && !self.busy && !self.read_only {
            let command = entry.command.clone();
            controls = controls.child(
                ui::ghost_button(
                    SharedString::from(format!("review-again-{id}")),
                    "Review again",
                    palette,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.perform(
                        "review_again",
                        json!({"command":command,"request_id":uuid::Uuid::new_v4().to_string()}),
                        cx,
                    );
                })),
            );
        }
        controls.into_any_element()
    }
}

/// A 24px text button at a row's end. `strong` is the main action, filled as the app's primary button; the others
/// are quiet until hovered. One that cannot act now is dimmed; the caller leaves out its click.
pub(crate) fn row_button(
    id: &str,
    op: &'static str,
    label: &'static str,
    strong: bool,
    ready: bool,
    palette: Palette,
) -> Stateful<Div> {
    let button = div()
        .id(SharedString::from(format!("row-{op}-{id}")))
        .debug_selector({
            let id = id.to_owned();
            move || format!("row-{op}-{id}")
        })
        .h(px(24.))
        .px(px(9.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .child(label);
    let button = if strong {
        button.bg(palette.primary).text_color(palette.primary_foreground)
    } else {
        button.text_color(palette.muted)
    };
    if !ready {
        return button.opacity(0.5);
    }
    let button = button.cursor_pointer().active(|style| style.opacity(0.8));
    if strong {
        button.hover(|style| style.opacity(0.9))
    } else {
        button.hover(|style| style.bg(palette.selected).text_color(palette.foreground))
    }
}

/// The row's leading mark, 16px square. A running command's dot breathes; a mark that just changed fades in.
pub(super) fn mark(entry: &Entry, tone: Tone, changed: bool, motion: bool, palette: Palette) -> AnyElement {
    let icon = |name: Lucide, color: Hsla| glyph(name, color).size(px(13.)).into_any_element();
    let inner = match tone {
        Tone::Attention => super::approval::attention_mark(palette).into_any_element(),
        Tone::Running => {
            let dot = div().size(px(7.)).rounded_full().bg(palette.foreground);
            if motion {
                dot.with_animation(
                    SharedString::from(format!("command-breathe-{}", entry.id)),
                    Animation::new(crate::motion::PULSE)
                        .repeat()
                        .with_max_fps(crate::motion::LOOP_FPS)
                        .with_easing(pulsating_between(0.35, 1.0)),
                    |dot, delta| dot.opacity(delta),
                )
                .into_any_element()
            } else {
                dot.into_any_element()
            }
        }
        Tone::Stopping => {
            // The running dot squares off as the stop is asked: a 7px circle becomes an 8px square with 2px corners,
            // then holds still. The stopped mark is the same square, outlined.
            use crate::motion::TweenExt as _;
            let tween = crate::motion::Tween::new(crate::motion::UI_LONG, crate::motion::ease_out);
            div()
                .bg(palette.foreground)
                .with_tween(
                    SharedString::from(format!("command-stopping-{}", entry.id)),
                    1.,
                    if changed { tween.from(0.) } else { tween },
                    |square, t| square.size(px(7. + t)).rounded(px(3.5 - 1.5 * t)),
                )
                .into_any_element()
        }
        Tone::Pending => div()
            .size(px(8.))
            .rounded_full()
            .border_1()
            .border_color(palette.muted)
            .into_any_element(),
        Tone::Done => icon(Lucide::Check, palette.icon),
        Tone::Failed => icon(Lucide::X, palette.destructive),
        Tone::NotRun => icon(Lucide::Ban, palette.faint),
        Tone::Unknown => icon(Lucide::CircleQuestionMark, palette.faint),
        Tone::Ended => icon(Lucide::Square, palette.faint),
    };
    let mark = div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(inner);
    // The stopping square grows out of the dot, so it needs no fade.
    if changed && tone != Tone::Stopping {
        crate::motion::fade_in(mark, SharedString::from(format!("command-mark-{}-{tone:?}", entry.id)))
            .into_any_element()
    } else {
        mark.into_any_element()
    }
}

/// The word after a row's command, `changed_at` the moment its state last changed. A running row keeps an empty slot
/// that widens to "Stopping…" when the person stops it, so the row's words slide aside. After `HOLD_OFF` the word
/// shimmers until the command ends, so a quick stop shows no loop. A word that just changed rises into place.
fn word(entry: &Entry, tone: Tone, changed_at: Option<Instant>, motion: bool, palette: Palette) -> Option<AnyElement> {
    if matches!(tone, Tone::Running | Tone::Stopping) {
        let stopping = tone == Tone::Stopping;
        let content = stopping.then(|| {
            let word = div().flex_none().text_size(px(12.)).text_color(palette.muted);
            if motion && held_off(changed_at, Instant::now()) {
                word.child(crate::motion::shimmer(
                    SharedString::from(format!("command-stopping-word-{}", entry.id)),
                    STOPPING,
                    palette.muted,
                    palette.foreground,
                ))
                .into_any_element()
            } else {
                word.child(STOPPING).into_any_element()
            }
        });
        return Some(slot(format!("command-word-{}", entry.id), stopping, content));
    }
    let word = status_word(entry, tone, palette)?;
    Some(if motion {
        crate::motion::retitle(word, &entry.status(), changed_at)
    } else {
        word.into_any_element()
    })
}

/// Whether a stop that began at `since` has waited out `HOLD_OFF` at `now`. A stop the pane found already under way
/// (no `since`) has.
fn held_off(since: Option<Instant>, now: Instant) -> bool {
    since.is_none_or(|at| now.saturating_duration_since(at) >= crate::motion::HOLD_OFF)
}

/// What a stopping row says while the command winds down.
const STOPPING: &str = "Stopping…";

/// A part of a row that widens in and narrows out (`motion::reveal_width`). A closed slot also gives back the row's
/// gap before it, so a row with nothing in the slot lays out as if it had none.
fn slot(id: String, shown: bool, content: Option<AnyElement>) -> AnyElement {
    use crate::motion::TweenExt as _;
    let gap = crate::motion::Tween::new(crate::motion::COLLAPSE, crate::motion::ease_out);
    div()
        .flex_none()
        .child(crate::motion::reveal_width(
            SharedString::from(id.clone()),
            shown,
            content,
        ))
        .with_tween(
            SharedString::from(format!("{id}-gap")),
            if shown { 1. } else { 0. },
            gap,
            |slot, t| slot.ml(px(-ROW_GAP * (1. - t))),
        )
        .into_any_element()
}

/// The word after a row's command, only where the mark alone does not say it: "Needs you", "Exit 2", "Stopped".
fn status_word(entry: &Entry, tone: Tone, palette: Palette) -> Option<Div> {
    let (text, color, strong) = match tone {
        // A request's mark and its Decline and Run already say it needs the person; a stopping row has its own slot.
        Tone::Done | Tone::Running | Tone::Stopping | Tone::Attention => return None,
        Tone::Failed => (entry.status(), palette.destructive, false),
        Tone::Pending if entry.status == "providerPending" => ("Pending".into(), palette.muted, false),
        _ => (entry.status(), palette.muted, false),
    };
    Some(
        div()
            .flex_none()
            .text_size(px(12.))
            .text_color(color)
            .when(strong, |word| word.font_weight(FontWeight::MEDIUM))
            .child(text),
    )
}

/// What a command's details say about where it ran and what Nexus knows of it.
fn notes(entry: &Entry) -> Vec<String> {
    let mut notes = Vec::new();
    if entry.route != "workbench" {
        let reason = entry
            .bypass_reason
            .as_deref()
            .unwrap_or("The provider used its own executor; the cause is unknown.");
        notes.push(format!(
            "Outside workbench · {}: {reason}",
            entry.bypass_code.as_deref().unwrap_or("native_execution_unintercepted")
        ));
        notes.push(
            "Use the original provider activity for output and controls. Nexus did not rerun this command.".into(),
        );
    } else if entry.restored {
        notes.push("Recovered history. Process handles and raw output are unavailable after restart.".into());
    }
    if entry.status == "unknown" {
        notes.push("Nexus could not confirm this command's outcome. Check its effects before running it again.".into());
    }
    if entry.private {
        notes.push("Private output is hidden.".into());
    }
    if entry.status == "declined" {
        notes.push("Declined · never started. Delete only removes this history row.".into());
    }
    notes
}

/// `text` with the search's words marked: stronger and heavier, so a mark still reads on the selected fill.
fn marked(text: String, words: &[String], palette: Palette) -> StyledText {
    let style = HighlightStyle {
        color: Some(palette.strong),
        font_weight: Some(FontWeight::SEMIBOLD),
        ..Default::default()
    };
    let highlights: Vec<_> = list::ranges(&text, words)
        .into_iter()
        .map(|range| (range, style))
        .collect();
    StyledText::new(text).with_highlights(highlights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn entry(value: Value) -> Entry {
        let mut base = json!({"id":"one","name":"tests","command":"npm test","cwd":"/work","status":"completed",
            "private":false,"exitCode":0});
        for (key, value) in value.as_object().unwrap() {
            base[key] = value.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn the_row_names_only_what_its_mark_cannot() {
        let palette =
            crate::palette::Palette::for_choice(crate::palette::ThemeChoice::Light, gpui_kit::WindowAppearance::Light);
        let done = entry(json!({}));
        assert!(status_word(&done, Tone::Done, palette).is_none());
        let failed = entry(json!({"status":"failed","exitCode":2}));
        assert!(status_word(&failed, list::tone(&failed), palette).is_some());
        // A stopping row says it in its own slot, which widens in, not in the plain word.
        let stopping = entry(json!({"status":"running","exitCode":null,"stopRequested":true}));
        assert_eq!(list::tone(&stopping), Tone::Stopping);
        assert!(status_word(&stopping, Tone::Stopping, palette).is_none());
    }

    #[test]
    fn a_stop_shimmers_only_after_its_hold_off() {
        let start = Instant::now();
        let hold = crate::motion::HOLD_OFF;
        assert!(!held_off(Some(start), start), "a quick stop shows no loop");
        assert!(!held_off(Some(start), start + hold - Duration::from_millis(1)));
        assert!(held_off(Some(start), start + hold));
        assert!(held_off(None, start), "a stop found under way shimmers at once");
    }

    #[test]
    fn details_explain_recovered_private_and_outside_commands() {
        let restored = entry(json!({"restored":true,"private":true}));
        let said = notes(&restored);
        assert!(said.iter().any(|note| note.starts_with("Recovered history")));
        assert!(said.iter().any(|note| note == "Private output is hidden."));
        let outside = entry(json!({"route":"externalObserved","bypassCode":"native"}));
        assert!(notes(&outside)[0].starts_with("Outside workbench · native:"));
    }
}
