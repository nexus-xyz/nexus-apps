//! An interactive picker on a terminal (`picker.ts` is its twin): type to search, ↑↓ to select, ↵ to confirm, esc to
//! cancel. The state and the drawing are plain functions (`step`, `render_picker`), tested without a terminal;
//! `run_picker` drives them with crossterm (raw mode on stdin, the picker redrawn in place on stdout).
//!
//! Used by `nexus models pick`; it is generic, so other choices (an organization) can use it too.

use crate::style::{Group, Row, Style, col, grouped_list, key_hints, legend, meta, provider_mark, stack, truncate};

#[derive(Debug, Clone, Default)]
pub struct PickItem {
    pub id: String,
    pub label: String,
    /// Dim text beside the label (a model's id).
    pub detail: Option<String>,
    /// Items with the same group follow each other under its title.
    pub group: Option<String>,
    /// A model provider: the group title's coloured mark.
    pub provider: Option<String>,
    /// What typing matches, besides the label and detail.
    pub search: Option<String>,
    /// A short meter beside the item (relative price), already drawn.
    pub meter: Option<String>,
    /// The item in use now: marked, and selected first.
    pub current: bool,
    /// Shown for the selected item as dim labels over bright values (its prices).
    pub facts: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct PickerSpec {
    pub title: String,
    pub meta: Vec<Option<String>>,
    pub items: Vec<PickItem>,
    /// What ↵ does: "set default".
    pub action: String,
    pub legend: Vec<(String, &'static str)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Home,
    End,
    Enter,
    Escape,
    Cancel,
    Backspace,
    Clear,
    Char(char),
    Ignore,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PickerState {
    pub query: String,
    pub index: usize,
    /// `Some(true)`: picked; `Some(false)`: cancelled.
    pub done: Option<bool>,
}

/// The items every word of the query is part of (label, detail, search text or group), in order.
pub fn filter_items<'a>(items: &'a [PickItem], query: &str) -> Vec<&'a PickItem> {
    let lower = query.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    items
        .iter()
        .filter(|item| {
            let text = format!(
                "{} {} {} {}",
                item.label,
                item.detail.as_deref().unwrap_or(""),
                item.search.as_deref().unwrap_or(""),
                item.group.as_deref().unwrap_or("")
            )
            .to_lowercase();
            words.iter().all(|word| text.contains(word))
        })
        .collect()
}

pub fn initial_state(items: &[PickItem]) -> PickerState {
    PickerState {
        query: String::new(),
        index: items.iter().position(|item| item.current).unwrap_or(0),
        done: None,
    }
}

pub fn step(state: &PickerState, key: &Key, items: &[PickItem]) -> PickerState {
    let count = filter_items(items, &state.query).len();
    let mut next = state.clone();
    match key {
        Key::Up => next.index = state.index.saturating_sub(1),
        Key::Down => next.index = (state.index + 1).min(count.saturating_sub(1)),
        Key::Home => next.index = 0,
        Key::End => next.index = count.saturating_sub(1),
        Key::Enter => {
            if count > 0 {
                next.done = Some(true);
            }
        }
        Key::Escape | Key::Cancel => next.done = Some(false),
        Key::Backspace => {
            next.query.pop();
            next.index = 0;
        }
        Key::Clear => {
            next.query.clear();
            next.index = 0;
        }
        Key::Char(c) => {
            next.query.push(*c);
            next.index = 0;
        }
        Key::Ignore => {}
    }
    next
}

/// The picker's lines for a terminal `width` × `height`: header, search, the visible items, the selection's facts,
/// hints.
pub fn render_picker(
    style: &Style,
    spec: &PickerSpec,
    state: &PickerState,
    width: usize,
    height: usize,
) -> Vec<String> {
    let g = style.g;
    let shown = filter_items(&spec.items, &state.query);
    let mut lines = vec![
        format!("{}  {}", style.bold(&spec.title), meta(style, &spec.meta)),
        String::new(),
        format!(
            "  {}  {}{}{}",
            style.dim("Search"),
            state.query,
            style.accent(g.cursor),
            if state.query.is_empty() {
                style.faint(" type to filter")
            } else {
                String::new()
            }
        ),
        String::new(),
    ];
    if shown.is_empty() {
        lines.push(format!(
            "  {}",
            style.dim(&format!("Nothing matches {}.", crate::style::quote(&state.query)))
        ));
    } else {
        // Room for the rows: the screen less the header, search, facts and hints, and two lines (a blank, a title) per
        // group.
        let mut groups: Vec<Option<&String>> = spec.items.iter().map(|item| item.group.as_ref()).collect();
        groups.sort();
        groups.dedup();
        let reserved = 12 + usize::from(!spec.legend.is_empty()) + 2 * groups.len();
        let room = height.saturating_sub(reserved).max(3);
        let index = state.index as i64;
        let start = (index - (room as i64) / 2).min(shown.len() as i64 - room as i64).max(0) as usize;
        let visible = &shown[start..(start + room).min(shown.len())];
        if start > 0 {
            lines.push(format!(
                "  {}",
                style.faint(&format!("{} {start} more", if style.unicode { "↑" } else { "^" }))
            ));
        }
        let mut groups: Vec<Group> = Vec::new();
        for (i, item) in visible.iter().enumerate() {
            let row = Row {
                cells: vec![
                    item.label.clone(),
                    item.detail
                        .as_deref()
                        .map(|detail| style.dim(detail))
                        .unwrap_or_default(),
                    item.meter.clone().unwrap_or_default(),
                    if item.current {
                        format!("{} {}", style.ok, style.dim("current"))
                    } else {
                        String::new()
                    },
                ],
                selected: start + i == state.index,
                ..Row::default()
            };
            let same = i > 0 && visible[i - 1].group == item.group;
            match groups.last_mut() {
                Some(last) if same => last.rows.push(row),
                _ => groups.push(Group {
                    title: item.group.as_ref().map(|group| {
                        let mark = item
                            .provider
                            .as_deref()
                            .map(|provider| format!("{} ", provider_mark(style, provider)))
                            .unwrap_or_default();
                        format!("{mark}{}", style.bold(group))
                    }),
                    rows: vec![row],
                }),
            }
        }
        lines.extend(grouped_list(
            style,
            &[col(""), col(""), col(""), col("")],
            &groups,
            None,
        ));
        let after = shown.len() - start - visible.len();
        if after > 0 {
            lines.push(format!(
                "  {}",
                style.faint(&format!("{} {after} more", if style.unicode { "↓" } else { "v" }))
            ));
        }
        if let Some(item) = shown.get(state.index)
            && !item.facts.is_empty()
        {
            lines.push(String::new());
            lines.extend(stack(style, &item.facts));
        }
    }
    lines.push(String::new());
    if !spec.legend.is_empty() {
        let items: Vec<(String, &str)> = spec
            .legend
            .iter()
            .map(|(glyph, label)| (glyph.clone(), *label))
            .collect();
        lines.push(format!("  {}", legend(style, &items)));
    }
    lines.push(format!(
        "  {}",
        key_hints(
            style,
            &[
                (g.updown, "select"),
                ("type", "search"),
                (g.enter, &spec.action),
                ("esc", "cancel"),
            ]
        )
    ));
    lines
        .iter()
        .map(|line| truncate(line, width.saturating_sub(1).max(10), g.ellipsis))
        .collect()
}

/// A key crossterm read, as the picker sees it: arrows, ↵, esc, backspace, ctrl-c/d/u/n/p, tab, and text.
fn key_of(event: &crossterm::event::KeyEvent) -> Option<Key> {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
    if event.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);
    Some(match event.code {
        KeyCode::Up => Key::Up,
        KeyCode::Down | KeyCode::Tab => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Char('c' | 'd') if ctrl => Key::Cancel,
        KeyCode::Char('u') if ctrl => Key::Clear,
        KeyCode::Char('n') if ctrl => Key::Down,
        KeyCode::Char('p') if ctrl => Key::Up,
        KeyCode::Char(_) if ctrl => Key::Ignore,
        KeyCode::Char(c) => Key::Char(c),
        _ => Key::Ignore,
    })
}

/// Run a picker on the terminal: draw, read keys, redraw in place; the chosen item's id, or `None` when cancelled.
pub fn run_picker(style: &Style, spec: &PickerSpec) -> std::io::Result<Option<String>> {
    use crossterm::event::{Event, read};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
    use std::io::Write as _;

    let mut state = initial_state(&spec.items);
    let mut drawn = 0usize;
    let up = |drawn: usize| {
        if drawn > 1 {
            format!("\r\x1b[{}A", drawn - 1)
        } else {
            "\r".to_string()
        }
    };
    let mut stdout = std::io::stdout();
    enable_raw_mode()?;
    let result = (|| -> std::io::Result<()> {
        loop {
            // A terminal that does not say its size (0 × 0) is drawn as 80 × 24, as the TypeScript picker does.
            let (columns, rows) = size().unwrap_or((0, 0));
            let columns = if columns == 0 { 80 } else { usize::from(columns) };
            let rows = if rows == 0 { 24 } else { usize::from(rows) };
            let lines = render_picker(style, spec, &state, columns, rows);
            let prefix = if drawn > 0 { up(drawn) } else { "\x1b[?25l".to_string() };
            write!(stdout, "{prefix}\x1b[J{}", lines.join("\r\n"))?;
            stdout.flush()?;
            drawn = lines.len();
            // Wait for a key that means something; any other event (a resize) draws again.
            while let Event::Key(event) = read()? {
                if let Some(key) = key_of(&event) {
                    state = step(&state, &key, &spec.items);
                    break;
                }
            }
            if state.done.is_some() {
                return Ok(());
            }
        }
    })();
    let _ = write!(stdout, "{}\x1b[J\x1b[?25h", up(drawn));
    let _ = stdout.flush();
    let _ = disable_raw_mode();
    result?;
    Ok(if state.done == Some(true) {
        filter_items(&spec.items, &state.query)
            .get(state.index)
            .map(|item| item.id.clone())
    } else {
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<PickItem> {
        let item = |id: &str, label: &str, group: &str| PickItem {
            id: id.into(),
            label: label.into(),
            detail: Some(id.into()),
            group: Some(group.into()),
            provider: Some(group.to_lowercase()),
            facts: vec![("Input".into(), "$1.00 / 1M tokens".into())],
            ..PickItem::default()
        };
        vec![
            PickItem {
                current: true,
                ..item("nexus/auto", "Auto", "Nexus")
            },
            item("anthropic/claude-haiku-5-5", "Claude Haiku 5.5", "Anthropic"),
            item("openai/gpt-6-mini", "GPT-6 mini", "OpenAI"),
        ]
    }

    #[test]
    fn filters_moves_and_picks() {
        let items = items();
        let mut state = initial_state(&items);
        assert_eq!(state.index, 0);
        state = step(&state, &Key::Down, &items);
        state = step(&state, &Key::Down, &items);
        state = step(&state, &Key::Down, &items);
        assert_eq!(state.index, 2);
        for c in "haiku".chars() {
            state = step(&state, &Key::Char(c), &items);
        }
        assert_eq!(filter_items(&items, &state.query).len(), 1);
        state = step(&state, &Key::Enter, &items);
        assert_eq!(state.done, Some(true));
        let none = step(
            &PickerState {
                query: "zzz".into(),
                ..PickerState::default()
            },
            &Key::Enter,
            &items,
        );
        assert_eq!(none.done, None);
        assert_eq!(step(&state, &Key::Escape, &items).done, Some(false));
    }

    #[test]
    fn draws_groups_the_selection_and_its_prices() {
        let style = Style::new(false);
        let spec = PickerSpec {
            title: "Default model for nexus chat".into(),
            meta: vec![Some("acme".into()), Some("2 models you may use".into())],
            items: items(),
            action: "set default".into(),
            legend: vec![(style.ok.clone(), "current default")],
        };
        let state = step(&initial_state(&spec.items), &Key::Down, &spec.items);
        let lines = render_picker(&style, &spec, &state, 100, 30);
        assert_eq!(
            lines,
            vec![
                "Default model for nexus chat  acme · 2 models you may use",
                "",
                "  Search  ▏ type to filter",
                "",
                "  ● Nexus",
                "· Auto               nexus/auto                      ✓ current",
                "",
                "  ● Anthropic",
                "❯ Claude Haiku 5.5   anthropic/claude-haiku-5-5",
                "",
                "  ● OpenAI",
                "· GPT-6 mini         openai/gpt-6-mini",
                "",
                "  Input",
                "  $1.00 / 1M tokens",
                "",
                "  ✓ current default",
                "  ↑↓ select · type search · ↵ set default · esc cancel",
            ]
        );
    }
}
