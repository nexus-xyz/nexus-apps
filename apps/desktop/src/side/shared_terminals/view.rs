//! The Workbench's Commands tab: Active and History, a search over both, one row per command, and the keyboard.
//! The tray above the composer (`inline.rs`) draws the same commands while the tab is closed.
use super::list;
use super::rows::RowState;
use super::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};

impl SharedTerminals {
    fn show_history(&self) -> bool {
        self.history_allowed && self.show_history
    }

    /// Active holds what runs or waits, and an accepted command for the 5 seconds after it ends (`finished.rs`);
    /// History holds the rest.
    fn in_tab(&self, entry: &Entry, history: bool) -> bool {
        self.read_only || (entry.active() || self.lingering(&entry.id)) != history
    }

    /// The rows the pane shows now, newest first: the open tab's commands that match the search, as their places in
    /// `entries`.
    fn visible(&self) -> Vec<usize> {
        let words = list::words(&self.query);
        let history = self.show_history();
        newest_first_places(&self.entries)
            .into_iter()
            .filter(|&at| {
                let entry = &self.entries[at];
                self.in_tab(entry, history) && list::matches(entry, &words)
            })
            .collect()
    }

    /// The rows the pane shows now, newest first: the open tab's commands that match the search.
    pub(super) fn visible_ids(&self) -> Vec<String> {
        self.visible()
            .into_iter()
            .map(|at| self.entries[at].id.clone())
            .collect()
    }

    /// Bring the Commands list up to date with the rows it shows. Each row's digest holds what sets its height, so
    /// only a row whose command or state changed is measured again. True when the list needs one more frame to take
    /// its row heights (`PaneList::sync`).
    fn sync_pane(&mut self, cx: &App) -> bool {
        use std::hash::{Hash, Hasher};
        let history = self.show_history();
        let rows = self
            .visible()
            .into_iter()
            .map(|at| {
                let entry = &self.entries[at];
                let open = !self.collapsed.contains(&entry.id);
                let output = self.outputs.get(&entry.id).filter(|_| open);
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                (
                    entry,
                    open,
                    output.is_some(),
                    self.editing.as_ref().is_some_and(|(id, _)| id == &entry.id),
                    open.then(|| self.submitted_inputs.get(&entry.id)).flatten(),
                    // Whether the details show the command box, which the row's width decides.
                    open.then(|| self.cut.borrow().get(&entry.id).copied()).flatten(),
                    open && self.inputs_open.contains(&entry.id),
                    !history && !self.read_only && self.lingering(&entry.id),
                )
                    .hash(&mut hasher);
                pane::PaneRow {
                    id: entry.id.clone(),
                    at,
                    digest: hasher.finish(),
                    focus: output.map(|output| output.view.read(cx).focus_handle().clone()),
                }
            })
            .collect();
        self.pane_list.sync(rows)
    }

    fn search_focused(&self, window: &Window, cx: &App) -> bool {
        self.search
            .as_ref()
            .is_some_and(|search| search.focus_handle(cx).is_focused(window))
    }

    fn ensure_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(search) = &self.search {
            return search.clone();
        }
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search commands")
                .default_value(self.query.clone())
        });
        self._search_events = Some(
            cx.subscribe_in(&search, window, |this, state, event: &InputEvent, _, cx| {
                if !this.is_current() {
                    return;
                }
                if let InputEvent::Change = event {
                    let query = state.read(cx).value().to_string();
                    if query != this.query {
                        this.query = query;
                        let ids = this.visible_ids();
                        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
                        this.selected = list::keep(&ids, this.selected.as_deref());
                        this.pane_list.to_top();
                        cx.notify();
                    }
                }
            }),
        );
        self.search = Some(search.clone());
        search
    }

    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let search = self.ensure_search(window, cx);
        search.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = self.search.clone() {
            search.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.query.clear();
        cx.notify();
    }

    fn switch_tab(&mut self, history: bool, cx: &mut Context<Self>) {
        self.show_history = history;
        self.pane_list.to_top();
        let ids = self.visible_ids();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        self.selected = list::keep(&ids, self.selected.as_deref());
        cx.notify();
    }

    fn toggle(&mut self, id: &str) {
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id.to_owned());
        }
    }

    /// The pane's keys, read before the search field or a row takes them. They apply only while the list or the
    /// search has the focus, so a command's shell keeps its own arrows, Enter and Escape.
    fn pane_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let in_search = self.search_focused(window, cx);
        let in_list = self.list_focus.is_focused(window);
        if !in_search && !in_list {
            return false;
        }
        let keys = &event.keystroke;
        let plain = !keys.modifiers.modified();
        match keys.key.as_str() {
            "down" | "up" if plain => {
                let ids = self.visible_ids();
                let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
                self.selected = list::step(&ids, self.selected.as_deref(), if keys.key == "up" { -1 } else { 1 });
                // The list's rows may be a read behind the commands: bring them up to date before revealing.
                let _ = self.sync_pane(cx);
                if let Some(id) = &self.selected {
                    self.pane_list.reveal(id);
                }
                cx.notify();
                true
            }
            "enter" if plain => {
                if let Some(id) = self.selected.clone()
                    && self.visible_ids().contains(&id)
                {
                    self.toggle(&id);
                    cx.notify();
                }
                true
            }
            "space" if plain && in_list => {
                if let Some(id) = self.selected.clone() {
                    self.toggle(&id);
                    cx.notify();
                }
                true
            }
            "escape" if plain => {
                if self.wells.restore() {
                    // A maximized well gives the pane back first.
                    cx.notify();
                    true
                } else if !self.query.is_empty() {
                    self.clear_search(window, cx);
                    true
                } else if in_search {
                    self.list_focus.focus(window, cx);
                    cx.notify();
                    true
                } else if let Some(id) = self.selected.clone()
                    && !self.collapsed.contains(&id)
                {
                    self.collapsed.insert(id);
                    cx.notify();
                    true
                } else {
                    false
                }
            }
            "/" if plain && in_list => {
                self.focus_search(window, cx);
                true
            }
            "f" if keys.modifiers.secondary(cx) && !keys.modifiers.shift && !keys.modifiers.alt => {
                self.focus_search(window, cx);
                true
            }
            _ => false,
        }
    }

    /// The pane, as the Workbench's Commands tab shows it.
    pub(super) fn render_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = cx.global::<crate::pages::Theme>().0;
        let motion = !cx.reduce_motion();
        // Drop the finished rows whose time is up before the tabs are counted, so a row leaves Active and arrives
        // in History on the same frame.
        self.advance_finished(cx);
        let search = self.ensure_search(window, cx);
        let history = self.show_history();
        let words = list::words(&self.query);
        let active_count = self.entries.iter().filter(|entry| self.in_tab(entry, false)).count();
        if self.sync_pane(cx) {
            window.request_animation_frame();
        }
        let shown = {
            let ids: Vec<&str> = self.pane_list.ids().collect();
            if self.selected.as_ref().is_none_or(|id| !ids.contains(&id.as_str())) {
                self.selected = list::keep(&ids, self.selected.as_deref());
            }
            ids.len()
        };

        let mut header: Vec<AnyElement> = Vec::new();
        if !self.read_only {
            header.push(self.render_toolbar(active_count, history, palette, cx));
        }
        if !self.read_only && !self.undo_ids.is_empty() {
            let ids = self.undo_ids.clone();
            header.push(
                ui::ghost_button("commands-undo-delete", "Undo command deletion", palette)
                    .on_click(cx.listener(move |this, _, _, cx| this.undo_delete(ids.clone(), cx)))
                    .into_any_element(),
            );
        }
        header.push(self.render_search(&search, shown, palette, cx));
        if let Some(error) = &self.error {
            header.push(ui::error_line(error.clone(), palette).into_any_element());
        }
        let attention = self.entries.iter().filter(|entry| entry.needs_attention()).count();
        if !self.read_only && !history && attention > 0 {
            header.push(self.render_attention(attention, palette, cx));
        }
        if self.history_allowed
            && !self.read_only
            && let Some(ids) = &self.delete_confirmation
        {
            header.push(self.render_delete_confirmation(ids.len(), motion, palette, cx));
        }

        // A maximized well's row alone takes the pane, while it is listed and open.
        let maximized = self.wells.maximized().and_then(|key| {
            self.pane_list
                .ids()
                .position(|id| id == key.as_ref())
                .filter(|_| !self.collapsed.contains(key.as_ref()) && self.outputs.contains_key(key.as_ref()))
        });
        // Only the rows near the screen are made: the list asks for each by its place (`render_pane_item`).
        let body = if shown == 0 {
            div()
                .id("commands-pane-empty")
                .size_full()
                .overflow_y_scroll()
                .child(self.render_empty(history, &words, palette, cx))
                .into_any_element()
        } else if let Some(ix) = maximized {
            let row = self.render_pane_item(ix, window, cx);
            div()
                .relative()
                .size_full()
                .child(self.wells.pane_probe())
                .child(row)
                .into_any_element()
        } else {
            let rows = list(
                self.pane_list.state.clone(),
                cx.processor(|this: &mut Self, ix: usize, window, cx| this.render_pane_item(ix, window, cx)),
            )
            .size_full();
            // The other rows fade back in once after a maximized well gives the pane back.
            let rows = match self.wells.restores() {
                Some(restore) if motion => {
                    crate::motion::fade_in(rows, SharedString::from(format!("commands-restore-{restore}")))
                        .into_any_element()
                }
                _ => rows.into_any_element(),
            };
            div()
                .relative()
                .size_full()
                .child(self.wells.pane_probe())
                .child(rows)
                .into_any_element()
        };

        div()
            .id("commands-pane")
            .debug_selector(|| "commands-pane".into())
            .track_focus(&self.list_focus)
            .size_full()
            .min_h(px(0.))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.pane_key(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(pane::frame(header, body))
            .into_any_element()
    }

    /// The row at `ix` of the Commands list, with its details when open.
    fn render_pane_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.pane_list.row(ix) else {
            return div().into_any_element();
        };
        // The rows were made in this frame's render, so the place still holds the command; the search is the guard.
        let entry = self
            .entries
            .get(row.at)
            .filter(|entry| entry.id == row.id)
            .or_else(|| self.entries.iter().find(|entry| entry.id == row.id))
            .cloned();
        let Some(entry) = entry else {
            return div().into_any_element();
        };
        let palette = cx.global::<crate::pages::Theme>().0;
        let motion = !cx.reduce_motion();
        let history = self.show_history();
        let words = list::words(&self.query);
        let keyboard = self.list_focus.is_focused(window) || self.search_focused(window, cx);
        let open = !self.collapsed.contains(&entry.id);
        let state = RowState {
            words: &words,
            open,
            cursor: keyboard && self.selected.as_ref() == Some(&entry.id),
            motion,
            now_ms: now_ms(),
        };
        let group = v_flex()
            .w_full()
            .min_w(px(0.))
            .child(self.render_row(&entry, state, palette, cx))
            .when(open, |group| {
                group.child(self.render_details(&entry, motion, palette, window, cx))
            })
            .into_any_element();
        let group = match self.finished.iter().position(|card| card.entry.id == entry.id) {
            Some(ix) if !history && !self.read_only => self.render_lingering(ix, group, palette, cx),
            _ => group,
        };
        pane::item(group)
    }

    fn render_toolbar(&self, active: usize, history: bool, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap(px(8.))
            .child(
                ui::tabs(palette)
                    .child(
                        ui::tab("commands-active", format!("Active {active}"), !history, palette)
                            .on_click(cx.listener(|this, _, _, cx| this.switch_tab(false, cx))),
                    )
                    .when(self.history_allowed, |tabs| {
                        tabs.child(
                            ui::tab(
                                "commands-history",
                                format!("History {}", self.entries.len() - active),
                                history,
                                palette,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.switch_tab(true, cx))),
                        )
                    }),
            )
            .child(div().flex_1())
            .when(
                history && self.entries.iter().any(|e| e.status == "declined"),
                |toolbar| {
                    toolbar.child(
                        ui::ghost_button("commands-delete-declined", "Delete declined", palette).when(
                            !self.busy,
                            |b| {
                                b.on_click(cx.listener(|this, _, _, cx| {
                                    this.delete_confirmation = Some(
                                        this.entries
                                            .iter()
                                            .filter(|e| e.status == "declined")
                                            .map(|e| e.id.clone())
                                            .collect(),
                                    );
                                    cx.notify();
                                }))
                            },
                        ),
                    )
                },
            )
            .child(
                ui::ghost_button("commands-native-shell", "New shell", palette).when(!self.busy, |button| {
                    button.on_click(cx.listener(|this, _, _, cx| this.open_shell(cx)))
                }),
            )
            .into_any_element()
    }

    /// The search field: a filled 28px field, as the sidebar's, with the match count and a clear button.
    fn render_search(
        &self,
        search: &Entity<InputState>,
        shown: usize,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let total = if self.read_only {
            self.entries.len()
        } else {
            let history = self.show_history();
            self.entries.iter().filter(|entry| self.in_tab(entry, history)).count()
        };
        h_flex()
            .debug_selector(|| "commands-search".into())
            .w_full()
            .h(px(ui::CONTROL_HEIGHT))
            .px(px(8.))
            .gap(px(6.))
            .rounded(px(8.))
            .bg(if palette.dark { palette.selected } else { palette.raised })
            .child(glyph(Lucide::Search, palette.faint).size(px(13.)).flex_none())
            .child(div().flex_1().min_w(px(0.)).child(Input::new(search).appearance(false)))
            .when(!self.query.is_empty(), |field| {
                field
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(format!("{shown} of {total}")),
                    )
                    .child(
                        div()
                            .id("commands-search-clear")
                            .size(px(20.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .hover(|style| style.bg(palette.hover))
                            .child(glyph(Lucide::X, palette.icon).size(px(12.)))
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Clear search").build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.clear_search(window, cx))),
                    )
            })
            .into_any_element()
    }

    /// One line above the list while commands wait: how many, and the batch decisions.
    fn render_attention(&self, count: usize, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .debug_selector(|| "commands-attention".into())
            .w_full()
            .min_h(px(40.))
            .pl(px(10.))
            .pr(px(4.))
            .gap(px(8.))
            .rounded(px(8.))
            .bg(palette.hover)
            .child(super::approval::attention_mark(palette))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .child(format!(
                        "{} need{} permission",
                        list::commands(count),
                        if count == 1 { "s" } else { "" }
                    )),
            )
            .child(self.render_bulk_actions(palette, cx))
            .into_any_element()
    }

    fn render_delete_confirmation(
        &self,
        count: usize,
        motion: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card = h_flex()
            .debug_selector(|| "command-delete-confirmation".into())
            .w_full()
            .min_h(px(40.))
            .pl(px(12.))
            .pr(px(4.))
            .gap(px(8.))
            .rounded(px(8.))
            .bg(palette.raised)
            .shadow(ui::lift(palette))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .child(format!(
                        "Delete {} from History? Their execution permission stays unchanged.",
                        list::commands(count)
                    )),
            )
            .child(
                ui::ghost_button("commands-delete-cancel", "Cancel", palette).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.delete_confirmation = None;
                        cx.notify();
                    },
                )),
            )
            .child(
                ui::danger_button("commands-delete-confirm", "Delete", palette).when(!self.busy, |b| {
                    b.on_click(cx.listener(|this, _, _, cx| this.delete_declined(cx)))
                }),
            );
        if motion {
            crate::motion::arrive(card, "command-delete-confirmation-in", -4.).into_any_element()
        } else {
            card.into_any_element()
        }
    }

    /// Nothing to show: no command yet, no history, or no match for the search (and where the matches are).
    fn render_empty(&self, history: bool, words: &[String], palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        if !words.is_empty() {
            let elsewhere = if self.read_only || !self.history_allowed {
                0
            } else {
                self.entries
                    .iter()
                    .filter(|entry| self.in_tab(entry, !history) && list::matches(entry, words))
                    .count()
            };
            let detail = if elsewhere > 0 {
                format!(
                    "{} in {} match{}.",
                    list::commands(elsewhere),
                    if history { "Active" } else { "History" },
                    if elsewhere == 1 { "es" } else { "" }
                )
            } else {
                "Search looks in names, commands, folders and states.".into()
            };
            return ui::empty_state(
                Lucide::Search,
                format!("No command matches “{}”", self.query.trim()),
                detail,
                palette,
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        ui::ghost_button("commands-empty-clear", "Clear search", palette)
                            .on_click(cx.listener(|this, _, window, cx| this.clear_search(window, cx))),
                    )
                    .when(elsewhere > 0, |actions| {
                        actions.child(
                            ui::button(
                                "commands-empty-other",
                                if history { "Show in Active" } else { "Show in History" },
                                palette,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.switch_tab(!history, cx))),
                        )
                    }),
            )
            .into_any_element();
        }
        if history {
            return ui::empty_state(
                Lucide::RotateCcwClock,
                "No command history",
                "Finished, declined and stopped commands stay here until you delete them.",
                palette,
            )
            .into_any_element();
        }
        let state = ui::empty_state(
            Lucide::SquareTerminal,
            "No commands running",
            if self.read_only {
                "Commands the agent runs show here."
            } else {
                "Commands the agent runs show here. Open a shell of your own with New shell."
            },
            palette,
        );
        if self.read_only {
            return state.into_any_element();
        }
        state
            .child(
                ui::button("commands-empty-shell", "New shell", palette).when(!self.busy, |button| {
                    button.on_click(cx.listener(|this, _, _, cx| this.open_shell(cx)))
                }),
            )
            .into_any_element()
    }
}

/// Now, in the milliseconds the registry writes `created_at` in.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

impl Render for SharedTerminals {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible_output = self.history_allowed
            || self.read_only
            || (self.inline_open
                && self.inline_limit >= crate::command_tray::REVIEW_MIN
                && self.inline_selected == self.focus_output);
        if !visible_output {
            self.focus_output = None;
        }
        if let Some(id) = self.focus_output.clone()
            && let Some(output) = self.outputs.get(&id)
        {
            let focus = output.view.read(cx).focus_handle().clone();
            focus.focus(window, cx);
            self.focus_output = None;
        }
        self.last_draw = Instant::now();
        self.start_poll(cx);
        if !self.history_allowed && !self.read_only {
            let palette = cx.global::<crate::pages::Theme>().0;
            return self.render_inline(palette, cx);
        }
        self.render_pane(window, cx)
    }
}
