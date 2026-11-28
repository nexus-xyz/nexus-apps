//! Settings › Keyboard shortcuts, as the Nexus app's page (`desktop/src/settings/shortcuts.tsx`): every key
//! Nexus listens for, in one list a person searches and filters. Code's shortcuts that this build has are changed here
//! (click the key, press the new keys), cleared or reset, and Reset all. A key the system or another shortcut has is
//! refused before it saves, with what has it. The keys go to `settings.json`, which the Nexus app reads too
//! (`key_bindings.rs`). The keys that do not change sit in the same list, quieter. While the Nexus app runs, the page
//! is read-only and says why, as the other pages of this computer's settings.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use nexus_desktop_core::key_bindings::Bindings;

use super::{AskAboutNexus, FIND_HINT, Workspace, glyph};
use crate::key_bindings::{self, Outcome, REBINDABLE};
use crate::pages::ui;
use crate::palette::Palette;

const MOD: &str = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl " };

/// Where a key acts, as the page groups and filters them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Area {
    App,
    SessionList,
    Session,
    MessageBox,
    Workbench,
    Navigation,
}

impl Area {
    const ALL: [Area; 6] = [
        Area::App,
        Area::SessionList,
        Area::Session,
        Area::MessageBox,
        Area::Workbench,
        Area::Navigation,
    ];

    fn label(self) -> &'static str {
        match self {
            Area::App => "App",
            Area::SessionList => "Session list",
            Area::Session => "In a session",
            Area::MessageBox => "Message box",
            Area::Workbench => "Workbench",
            Area::Navigation => "Navigation",
        }
    }

    /// A changeable shortcut's area, from its group in the Nexus app (`REBINDABLE`).
    fn of(group: &str) -> Area {
        match group {
            "Sessions" => Area::SessionList,
            "Session" => Area::Session,
            "Workbench" => Area::Workbench,
            "Navigation" => Area::Navigation,
            _ => Area::App,
        }
    }
}

/// Which keys the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Changed,
    Area(Area),
}

/// One key of the list: a changeable shortcut (its `REBINDABLE` id) or one that does not change here.
struct Entry {
    id: Option<&'static str>,
    label: &'static str,
    area: Area,
    keys: String,
    /// Why a key that does not change here cannot, or where it changes.
    note: Option<&'static str>,
}

impl Entry {
    fn changed(&self, current: &Bindings) -> bool {
        self.id.is_some_and(|id| current.contains_key(id))
    }

    fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || self.label.to_lowercase().contains(query)
            || self.area.label().to_lowercase().contains(query)
    }
}

/// Every key the page lists: the changeable ones, then those that do not change here, each in its area.
fn entries(cx: &App) -> Vec<Entry> {
    let changeable = REBINDABLE.iter().map(|shortcut| Entry {
        id: Some(shortcut.id),
        label: shortcut.label,
        area: Area::of(shortcut.group),
        keys: key_bindings::label(shortcut.id),
        note: None,
    });
    let fixed = |label, area, keys: String| Entry {
        id: None,
        label,
        area,
        keys,
        note: None,
    };
    let canon = crate::canon_keys::enabled();
    let new_window = if cfg!(target_os = "macos") && canon {
        "⌃⌘N, or ⇧⌘N"
    } else if cfg!(target_os = "macos") {
        "⇧⌘N"
    } else {
        "Ctrl Shift N"
    };
    let ask = canon.then(|| {
        fixed(
            "Ask about Nexus",
            Area::App,
            format!("{MOD}{}/", if cfg!(target_os = "macos") { "⇧" } else { "Shift " }),
        )
    });
    let fixed = [
        Entry {
            note: Some("Change it in This computer › App."),
            ..fixed("Open Nexus from any app", Area::App, global_shortcut(cx))
        },
        fixed("Search", Area::App, FIND_HINT.into()),
        fixed("Settings", Area::App, format!("{MOD},")),
        fixed("New window", Area::App, new_window.into()),
        fixed("Show or hide the sidebar", Area::App, format!("{MOD}B")),
        fixed("Close a menu, or stop the answer", Area::App, "Esc".into()),
        fixed("Quit", Area::App, format!("{MOD}Q")),
        fixed("Send", Area::MessageBox, "Enter".into()),
        fixed("New line", Area::MessageBox, "Shift Enter".into()),
        fixed("Take the suggested prompt", Area::MessageBox, "Tab".into()),
        fixed(
            "New terminal",
            Area::Workbench,
            crate::shortcut_label::label(crate::shortcut_label::NEW_TERMINAL),
        ),
    ];
    let mut all: Vec<Entry> = changeable.chain(fixed).collect();
    if let Some(ask) = ask {
        let at = all
            .iter()
            .position(|entry| entry.label == "New window")
            .map_or(all.len(), |at| at + 1);
        all.insert(at, ask);
    }
    // By area, in the page's order; within one, as listed above (a stable sort).
    all.sort_by_key(|entry| Area::ALL.iter().position(|area| *area == entry.area));
    all
}

/// The global shortcut, or why it is not registered.
fn global_shortcut(cx: &App) -> String {
    let (saved, status) = crate::prefs::shortcut(cx);
    match (status.active, status.error) {
        (Some(active), _) => active,
        (None, Some(_)) => format!("{saved}, not registered"),
        (None, None) => saved,
    }
}

/// Every key Nexus listens for, the recorder of a changing one and why the last keys did not set.
pub(super) struct ShortcutsPage {
    /// The shortcut waiting for its new keys, and the window it waits in.
    listening: Option<(&'static str, WindowId)>,
    recorder: Option<Subscription>,
    /// Why the last keys pressed were not set, on that shortcut's row.
    problem: Option<(&'static str, String)>,
    /// Why a change did not save (the Nexus app runs, the disk refused).
    error: Option<SharedString>,
    search: Entity<InputState>,
    filter: Filter,
    _search_changed: Subscription,
}

impl ShortcutsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search shortcuts"));
        let search_changed = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        ShortcutsPage {
            listening: None,
            recorder: None,
            problem: None,
            error: None,
            search,
            filter: Filter::All,
            _search_changed: search_changed,
        }
    }

    pub fn recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Wait for `id`'s new keys in `window`; a second click on the same key stops waiting.
    fn listen(&mut self, id: &'static str, window: &Window, cx: &mut Context<Self>) {
        let same = self.listening.is_some_and(|(open, _)| open == id);
        self.stop(cx);
        self.problem = None;
        self.error = None;
        if same {
            return;
        }
        let window_id = window.window_handle().window_id();
        self.listening = Some((id, window_id));
        // The global shortcut stands aside too (`prefs.rs`), as for Settings › App's recorder.
        crate::prefs::set_recording(true);
        let this = cx.entity().downgrade();
        self.recorder = Some(cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle().window_id() != window_id {
                return;
            }
            // No other key binding runs while a key is recorded: the page takes the press.
            cx.stop_propagation();
            let keystroke = event.keystroke.clone();
            let _ = this.update(cx, |page, cx| page.pressed(&keystroke, cx));
        }));
        cx.notify();
    }

    fn pressed(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let Some((id, _)) = self.listening else {
            return;
        };
        match key_bindings::record(id, keystroke, &key_bindings::current(), cfg!(target_os = "macos")) {
            Outcome::Wait => {}
            Outcome::Cancel => self.stop(cx),
            Outcome::Refused(why) => {
                self.problem = Some((id, why));
                cx.notify();
            }
            Outcome::Set(next) => {
                self.stop(cx);
                self.save(next, cx);
            }
        }
    }

    /// Stop waiting for keys, if it was.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if self.recorder.take().is_some() {
            crate::prefs::set_recording(false);
        }
        self.listening = None;
        cx.notify();
    }

    fn save(&mut self, next: Bindings, cx: &mut Context<Self>) {
        self.problem = None;
        self.error = key_bindings::save(cx, next).err().map(Into::into);
        cx.notify();
    }

    fn clear(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.stop(cx);
        let mut next = key_bindings::current();
        next.insert(id.to_string(), None);
        self.save(next, cx);
    }

    fn reset(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.stop(cx);
        let mut next = key_bindings::current();
        next.remove(id);
        self.save(next, cx);
    }

    fn reset_all(&mut self, cx: &mut Context<Self>) {
        self.stop(cx);
        self.save(Default::default(), cx);
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        cx.notify();
    }

    /// A filter chip, with how many keys it holds when that helps.
    fn filter_chip(
        &self,
        filter: Filter,
        label: &'static str,
        count: Option<usize>,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = match filter {
            Filter::All => SharedString::from("shortcuts-filter-all"),
            Filter::Changed => "shortcuts-filter-changed".into(),
            Filter::Area(area) => format!("shortcuts-filter-{}", area.label()).into(),
        };
        ui::chip(id, label, self.filter == filter, palette)
            .gap(px(6.))
            .when_some(count, |chip, count| {
                chip.child(
                    div()
                        .text_color(palette.faint)
                        .font_weight(FontWeight::NORMAL)
                        .child(count.to_string()),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| this.set_filter(filter, cx)))
    }

    fn row(
        &self,
        ix: usize,
        entry: &Entry,
        current: &Bindings,
        locked: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let group: SharedString = format!("shortcut-row-{ix}").into();
        let recording = entry
            .id
            .is_some_and(|id| self.listening.is_some_and(|(open, _)| open == id));
        let problem = entry.id.and_then(|id| {
            self.problem
                .as_ref()
                .filter(|(on, _)| *on == id)
                .map(|(_, why)| why.clone())
        });
        let changed = entry.changed(current);
        let line: Option<(SharedString, Hsla)> = if let Some(why) = problem {
            Some((why.into(), palette.destructive))
        } else if recording {
            Some(("Press the new keys. Esc cancels.".into(), palette.muted))
        } else {
            entry.note.map(|note| (note.into(), palette.faint))
        };
        let title = h_flex()
            .gap(px(6.))
            .child(div().text_color(palette.strong).child(entry.label))
            .when(changed, |title| {
                title.child(div().flex_none().size(px(6.)).rounded(px(3.)).bg(palette.waiting()))
            });
        let show_area = !matches!(self.filter, Filter::Area(_));
        let mut row = h_flex()
            .group(group.clone())
            .min_h(px(40.))
            .mx(px(-ui::ROW_BLEED))
            .px(px(ui::ROW_BLEED))
            .py(px(4.))
            .mb(px(2.))
            .gap(px(8.))
            .rounded(px(8.))
            .hover(|style| style.bg(palette.hover))
            .text_size(px(13.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .child(title)
                    .when_some(line, |column, (text, color)| {
                        column.child(div().text_size(px(12.)).text_color(color).child(text))
                    }),
            )
            .when(show_area, |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(entry.area.label()),
                )
            });
        let Some(id) = entry.id else {
            // A key that does not change here: the same caps, quieter, and nothing to press.
            return row.child(key_slot(&entry.keys, palette).opacity(0.7));
        };
        // Reset and Clear wait out of sight until the row is hovered, so 41 rows do not read as 80 buttons.
        let quiet = |button: Stateful<Div>| button.opacity(0.).group_hover(group.clone(), |style| style.opacity(1.));
        if !locked {
            if changed {
                row = row.child(quiet(
                    ui::ghost_button(SharedString::from(format!("shortcut-reset-{id}")), "Reset", palette)
                        .on_click(cx.listener(move |this, _, _, cx| this.reset(id, cx))),
                ));
            }
            if !entry.keys.is_empty() {
                row = row.child(quiet(
                    ui::ghost_button(SharedString::from(format!("shortcut-clear-{id}")), "Clear", palette)
                        .on_click(cx.listener(move |this, _, _, cx| this.clear(id, cx))),
                ));
            }
        }
        let slot = if recording {
            h_flex()
                .min_w(px(96.))
                .h(px(ui::KEYCAP + 8.))
                .px(px(8.))
                .justify_center()
                .text_size(px(12.))
                .text_color(palette.strong)
                .child("Press keys…")
        } else if entry.keys.is_empty() {
            h_flex()
                .min_w(px(96.))
                .h(px(ui::KEYCAP + 8.))
                .px(px(8.))
                .justify_end()
                .text_size(px(12.))
                .text_color(palette.faint)
                .child("None")
        } else {
            key_slot(&entry.keys, palette)
        };
        let border = if recording {
            palette.strong
        } else if self.problem.as_ref().is_some_and(|(on, _)| *on == id) {
            palette.destructive
        } else {
            transparent_black()
        };
        let slot = div()
            .id(SharedString::from(format!("shortcut-{id}")))
            .flex_none()
            .rounded(px(7.))
            .border_1()
            .border_color(border)
            .child(slot);
        row.child(if locked {
            slot
        } else {
            ui::focusable(slot, palette)
                .cursor_pointer()
                .hover(|style| style.border_color(palette.border))
                .on_click(cx.listener(move |this, _, window, cx| this.listen(id, window, cx)))
        })
    }
}

/// A key cap's height.
/// A chord's caps, right-aligned in a slot as wide as the longest common chord, so the column of keys stays straight.
fn key_slot(keys: &str, palette: Palette) -> Div {
    h_flex()
        .min_w(px(96.))
        .h(px(ui::KEYCAP + 8.))
        .px(px(4.))
        .justify_end()
        .child(ui::keycaps(keys, palette))
}

impl Drop for ShortcutsPage {
    fn drop(&mut self) {
        if self.recorder.is_some() {
            crate::prefs::set_recording(false);
        }
    }
}

impl Render for ShortcutsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::pages::palette(cx);
        let locked = !crate::desktop_settings::writable();
        let current = key_bindings::current();
        let reset_all = if locked || current.is_empty() {
            ui::disabled_button("shortcuts-reset-all", "Reset all", palette)
        } else {
            ui::button("shortcuts-reset-all", "Reset all", palette)
                .on_click(cx.listener(|this, _, _, cx| this.reset_all(cx)))
        };
        let all = entries(cx);
        let changed = all.iter().filter(|entry| entry.changed(&current)).count();
        let query = self.search.read(cx).value().trim().to_lowercase();
        let shown: Vec<&Entry> = all
            .iter()
            .filter(|entry| match self.filter {
                Filter::All => true,
                Filter::Changed => entry.changed(&current),
                Filter::Area(area) => entry.area == area,
            })
            .filter(|entry| entry.matches(&query))
            .collect();
        let search = h_flex()
            .flex_1()
            .min_w(px(0.))
            .h(px(ui::CONTROL_HEIGHT + 4.))
            .px(px(10.))
            .gap(px(8.))
            .rounded(px(8.))
            .bg(palette.hover)
            .text_size(px(13.))
            .child(glyph(Lucide::Search, palette.faint))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(Input::new(&self.search).appearance(false)),
            );
        let mut chips = h_flex()
            .flex_wrap()
            .gap(px(6.))
            .mt(px(10.))
            .mb(px(12.))
            .child(self.filter_chip(Filter::All, "All", Some(all.len()), palette, cx))
            .child(self.filter_chip(Filter::Changed, "Changed", Some(changed), palette, cx));
        for area in Area::ALL {
            chips = chips.child(self.filter_chip(Filter::Area(area), area.label(), None, palette, cx));
        }
        let empty = if !query.is_empty() {
            Some(format!("No shortcut matches “{query}”."))
        } else if self.filter == Filter::Changed {
            Some("You haven't changed a shortcut. Click a key to change it.".to_string())
        } else {
            None
        };
        let rows: Vec<Div> = shown
            .iter()
            .enumerate()
            .map(|(ix, entry)| self.row(ix, entry, &current, locked, palette, cx))
            .collect();
        v_flex()
            .child(
                h_flex()
                    .gap(px(12.))
                    .items_start()
                    .child(div().flex_1().child(ui::lead(
                        "Click a key to change it. A dot marks the ones you changed.",
                        palette,
                    )))
                    .child(reset_all),
            )
            .when(locked, |page| {
                page.child(ui::note(crate::desktop_settings::READ_ONLY, palette).mb(px(12.)))
            })
            .when_some(self.error.clone(), |page, error| {
                page.child(ui::error_line(error, palette).mb(px(12.)))
            })
            .child(h_flex().child(search))
            .child(chips)
            .children(rows)
            .when(shown.is_empty(), |page| {
                page.child(ui::note(empty.unwrap_or_default(), palette).py(px(12.)))
            })
    }
}

impl Workspace {
    /// Cmd+Shift+/ (Ctrl+Shift+/): Ask about Nexus. The app has no help-only target, so this opens the Assistant, as
    /// Cmd+J does; without it (no agent API, signed out, the panel) the key does nothing.
    pub(super) fn on_ask_about_nexus(&mut self, _: &AskAboutNexus, window: &mut Window, cx: &mut Context<Self>) {
        if !self.assistant_available() {
            cx.propagate();
            return;
        }
        self.close_palette(window, cx);
        self.open_agent(None, window, cx);
    }
}
