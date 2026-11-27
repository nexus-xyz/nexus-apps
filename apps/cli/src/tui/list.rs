//! A list to choose from, drawn in place of the composer as Codex draws its pickers: a title, a search line, the
//! items under their groups with the chosen one marked, and the keys. `/model`, `/resume` and `/approvals` open one.
//! Typing narrows it, ↑ ↓ move, ⏎ chooses and esc goes back. Older items (a model a newer one of its line replaces)
//! stay out of the way until tab shows them or a search finds them.

use crate::style::truncate;

use super::app::Key;
use super::view::{Theme, display_width};

/// What the list chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Model,
    Session,
    Approvals,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub label: String,
    /// Dim text after the label: a model's price, when a session was last used.
    pub detail: String,
    /// The one in use now: marked, shown even when older, and chosen first.
    pub current: bool,
    /// The heading it goes under (a model's provider); items of one group follow each other.
    pub group: Option<String>,
    /// Replaced by a newer item: hidden until tab or a search.
    pub older: bool,
}

/// What a key did to the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Open,
    Chosen(String),
    Closed,
    /// The sessions' list, not filtering: the person started typing, so a new session starts with this text.
    Typed(String),
}

/// At most this many rows of items and headings show; the rest scroll with the choice.
const ROWS: usize = 14;
/// The sessions' list: at most this many rows of items (two lines each, a blank between) and headings.
const SESSION_ROWS: usize = 14;

#[derive(Debug, Clone)]
pub struct List {
    pub purpose: Purpose,
    title: String,
    items: Vec<Item>,
    query: String,
    selected: usize,
    /// Tab: older items too.
    all: bool,
    /// The sessions' list filters only after ⌃f; before, typing starts a new session.
    filtering: bool,
}

impl List {
    pub fn new(purpose: Purpose, title: &str, items: Vec<Item>) -> List {
        let mut list = List {
            purpose,
            title: title.to_string(),
            items,
            query: String::new(),
            selected: 0,
            all: false,
            filtering: false,
        };
        list.selected = list.visible().iter().position(|item| item.current).unwrap_or(0);
        list
    }

    /// The sessions' list after ⌃f: typing filters it.
    pub fn filtering(&self) -> bool {
        self.filtering
    }

    fn visible(&self) -> Vec<&Item> {
        let query = self.query.to_lowercase();
        self.items
            .iter()
            .filter(|item| {
                if !query.is_empty() {
                    return [&item.label, &item.detail, &item.id]
                        .iter()
                        .any(|text| text.to_lowercase().contains(&query));
                }
                self.all || !item.older || item.current
            })
            .collect()
    }

    /// How many older items are out of sight now.
    fn hidden(&self) -> usize {
        if self.all || !self.query.is_empty() {
            return 0;
        }
        self.items.iter().filter(|item| item.older && !item.current).count()
    }

    pub fn key(&mut self, key: &Key) -> Outcome {
        if self.purpose == Purpose::Session && !self.filtering {
            match key {
                Key::Char(c) => return Outcome::Typed(c.to_string()),
                Key::Paste(text) => return Outcome::Typed(text.clone()),
                Key::CtrlF => {
                    self.filtering = true;
                    return Outcome::Open;
                }
                _ => {}
            }
        }
        let count = self.visible().len();
        match key {
            Key::Esc | Key::CtrlC => return Outcome::Closed,
            Key::Up => self.selected = self.selected.checked_sub(1).unwrap_or(count.saturating_sub(1)),
            Key::Down => {
                self.selected = if self.selected + 1 >= count {
                    0
                } else {
                    self.selected + 1
                }
            }
            Key::Tab if self.items.iter().any(|item| item.older) => {
                // Keep the same item chosen as the list grows or shrinks.
                let chosen = self.visible().get(self.selected).map(|item| item.id.clone());
                self.all = !self.all;
                self.selected = chosen
                    .and_then(|id| self.visible().iter().position(|item| item.id == id))
                    .unwrap_or(0);
            }
            Key::Enter => {
                return match self.visible().get(self.selected) {
                    Some(item) => Outcome::Chosen(item.id.clone()),
                    None => Outcome::Open,
                };
            }
            Key::Char(c) => {
                self.query.push(*c);
                self.selected = 0;
            }
            Key::Paste(text) => {
                self.query.push_str(text.trim());
                self.selected = 0;
            }
            Key::Backspace => {
                self.query.pop();
                self.selected = 0;
            }
            Key::CtrlU => {
                self.query.clear();
                self.selected = 0;
            }
            _ => {}
        }
        Outcome::Open
    }

    /// The lines, and the cursor's row and column (at the end of the search).
    pub fn lines(&self, theme: &Theme, width: usize) -> (Vec<String>, (usize, usize)) {
        if self.purpose == Purpose::Session {
            return self.session_lines(theme, width);
        }
        let style = &theme.style;
        let visible = self.visible();
        let search = if self.query.is_empty() {
            style.dim("type to search")
        } else {
            self.query.clone()
        };
        let mut lines = vec![
            format!("  {}", style.bold(&self.title)),
            format!("  {} {search}", style.dim(if style.unicode { "⌕" } else { "?" })),
        ];
        let cursor = (
            1,
            4 + if self.query.is_empty() {
                0
            } else {
                display_width(&self.query)
            },
        );
        if visible.is_empty() {
            lines.push(String::new());
            lines.push(format!("  {}", style.dim("nothing matches")));
        }

        // Each item's row, under its group's heading, so the window can scroll to the chosen one.
        let label_width = visible
            .iter()
            .map(|item| display_width(&item.label))
            .max()
            .unwrap_or(0)
            .min(32);
        let mut rows: Vec<(Option<usize>, String)> = Vec::new();
        let mut group: Option<&str> = None;
        for (index, item) in visible.iter().enumerate() {
            if item.group.is_some() && item.group.as_deref() != group {
                group = item.group.as_deref();
                rows.push((None, String::new()));
                rows.push((None, format!("  {}", style.dim(group.unwrap_or_default()))));
            } else if index == 0 {
                rows.push((None, String::new()));
            }
            let label = truncate(&item.label, label_width, style.g.ellipsis);
            let padding = " ".repeat(label_width.saturating_sub(display_width(&label)) + 3);
            let mark = if item.current {
                format!(" {}", style.ok)
            } else {
                String::new()
            };
            let older = if item.older {
                style.dim(" · older")
            } else {
                String::new()
            };
            let line = if index == self.selected {
                theme.selected(&format!(
                    "{} {}{padding}{}{older}{mark}",
                    style.accent(style.g.select),
                    style.accent(&label),
                    style.dim(&item.detail)
                ))
            } else {
                format!("  {label}{padding}{}{older}{mark}", style.dim(&item.detail))
            };
            rows.push((Some(index), truncate(&line, width.saturating_sub(1), style.g.ellipsis)));
        }
        let at = rows
            .iter()
            .position(|(item, _)| *item == Some(self.selected))
            .unwrap_or(0);
        let first = (at + 1).saturating_sub(ROWS);
        lines.extend(rows.iter().skip(first).take(ROWS).map(|(_, line)| line.clone()));
        if rows.len() > first + ROWS {
            lines.push(format!(
                "  {}",
                style.dim(&format!("↓ {} more", rows.len() - first - ROWS))
            ));
        }

        lines.push(String::new());
        let hidden = self.hidden();
        let mut keys = vec!["⏎ choose".to_string(), "↑ ↓ move".to_string()];
        if hidden > 0 {
            keys.push(format!("tab {hidden} older"));
        } else if self.all {
            keys.push("tab newest only".into());
        }
        keys.push("esc back".into());
        lines.push(format!("  {}", style.dim(&keys.join(" · "))));
        (lines, cursor)
    }

    /// The sessions' list, drawn as Claude Code draws its `/resume`: a rule, the title with where the choice is, the
    /// search in a box, then each session in two lines (its title; when, how long and on what) under its group's
    /// heading, and the keys.
    fn session_lines(&self, theme: &Theme, width: usize) -> (Vec<String>, (usize, usize)) {
        let style = &theme.style;
        let visible = self.visible();
        let fit = |line: String| truncate(&line, width.saturating_sub(1), style.g.ellipsis);
        let (rule, corners, side) = if style.unicode {
            ("─", ["╭", "╮", "╰", "╯"], "│")
        } else {
            ("-", ["+", "+", "+", "+"], "|")
        };
        let inner = width.saturating_sub(7);
        let position = if visible.is_empty() {
            String::new()
        } else {
            format!(" ({} of {})", self.selected + 1, visible.len())
        };
        let placeholder = if self.filtering {
            "Filter…"
        } else {
            "Ctrl+F to filter"
        };
        let search = if self.query.is_empty() {
            style.dim(placeholder)
        } else {
            self.query.clone()
        };
        let typed = if self.query.is_empty() {
            0
        } else {
            display_width(&self.query)
        };
        let glass = if style.unicode { "⌕" } else { "?" };
        let mut lines = vec![
            style.accent(&rule.repeat(width.saturating_sub(1))),
            format!("  {}{}", style.accent(&style.bold(&self.title)), style.dim(&position)),
            format!("  {}{}{}", corners[0], rule.repeat(inner + 2), corners[1]),
            format!(
                "  {side} {} {search}{} {side}",
                style.dim(glass),
                " ".repeat(inner.saturating_sub(
                    2 + if self.query.is_empty() {
                        display_width(placeholder)
                    } else {
                        typed
                    }
                ))
            ),
            format!("  {}{}{}", corners[2], rule.repeat(inner + 2), corners[3]),
        ];
        let cursor = (3, 6 + typed);
        if visible.is_empty() {
            lines.push(String::new());
            lines.push(format!("    {}", style.dim("No session matches.")));
        }

        let mut rows: Vec<(Option<usize>, String)> = Vec::new();
        let mut group: Option<&str> = None;
        for (index, item) in visible.iter().enumerate() {
            if item.group.as_deref() != group {
                group = item.group.as_deref();
                if let Some(heading) = group {
                    if !rows.is_empty() {
                        rows.push((None, String::new()));
                    }
                    rows.push((None, fit(format!("    {}", style.dim(heading)))));
                }
            }
            rows.push((None, String::new()));
            let (mark, title) = if index == self.selected {
                (style.accent(style.g.select), style.accent(&item.label))
            } else {
                (" ".to_string(), item.label.clone())
            };
            let current = if item.current {
                format!(" {}", style.dim("(current)"))
            } else {
                String::new()
            };
            rows.push((Some(index), fit(format!("  {mark} {title}{current}"))));
            rows.push((Some(index), fit(format!("    {}", style.dim(&item.detail)))));
        }
        // The chosen session's second line in view.
        let at = rows
            .iter()
            .rposition(|(item, _)| *item == Some(self.selected))
            .unwrap_or(0);
        let first = (at + 1).saturating_sub(SESSION_ROWS);
        lines.extend(rows.iter().skip(first).take(SESSION_ROWS).map(|(_, line)| line.clone()));

        lines.push(String::new());
        let more = if rows.len() > first + SESSION_ROWS {
            format!("{} ", if style.unicode { "↓" } else { "v" })
        } else {
            "  ".to_string()
        };
        // The keys wrap at a separator, as Claude Code's do.
        let keys: &[&str] = if self.filtering {
            &["↑↓ to move", "Enter to resume", "Type to filter", "Esc to cancel"]
        } else {
            &["↑↓ to move", "Enter to resume", "Ctrl+F to filter", "Esc to cancel"]
        };
        let room = width.saturating_sub(6).max(16);
        let mut line = String::new();
        let mut first_line = true;
        for &key in keys {
            let next = if line.is_empty() {
                key.to_string()
            } else {
                format!("{line} · {key}")
            };
            if display_width(&next) > room && !line.is_empty() {
                let lead = if first_line { more.clone() } else { "  ".to_string() };
                lines.push(format!("  {lead}{}", style.dim(&line)));
                first_line = false;
                line = key.to_string();
            } else {
                line = next;
            }
        }
        let lead = if first_line { more } else { "  ".to_string() };
        lines.push(format!("  {lead}{}", style.dim(&line)));
        (lines, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;

    fn models() -> List {
        let item = |id: &str, label: &str, group: &str, older: bool, current: bool| Item {
            id: id.into(),
            label: label.into(),
            detail: String::new(),
            current,
            group: Some(group.into()),
            older,
        };
        List::new(
            Purpose::Model,
            "Choose a model",
            vec![
                item("nexus/auto", "Auto", "Nexus", false, false),
                item(
                    "anthropic/claude-opus-5.5",
                    "Claude Opus 5.5",
                    "Anthropic",
                    false,
                    false,
                ),
                item("anthropic/claude-opus-4.8", "Claude Opus 4.8", "Anthropic", true, true),
                item("anthropic/claude-opus-4.6", "Claude Opus 4.6", "Anthropic", true, false),
                item("openai/gpt-6-luna", "GPT-6 Luna", "OpenAI", false, false),
            ],
        )
    }

    fn text(list: &List) -> Vec<String> {
        list.lines(&Theme::new(Style::new(false), None), 80).0
    }

    #[test]
    fn older_models_wait_behind_tab_but_the_current_one_shows() {
        let mut list = models();
        let shown = text(&list);
        assert!(
            shown
                .iter()
                .any(|line| line.starts_with("❯ Claude Opus 4.8") && line.contains("older")),
            "{shown:#?}"
        );
        assert!(!shown.iter().any(|line| line.contains("Opus 4.6")));
        assert!(shown.iter().any(|line| line.contains("tab 1 older")));
        assert!(shown.contains(&"  Anthropic".to_string()) && shown.contains(&"  OpenAI".to_string()));
        list.key(&Key::Tab);
        assert!(text(&list).iter().any(|line| line.contains("Opus 4.6")));
        assert_eq!(
            list.key(&Key::Enter),
            Outcome::Chosen("anthropic/claude-opus-4.8".into()),
            "the choice stays"
        );
    }

    #[test]
    fn a_search_finds_older_models_too() {
        let mut list = models();
        for c in "4.6".chars() {
            list.key(&Key::Char(c));
        }
        assert_eq!(
            list.key(&Key::Enter),
            Outcome::Chosen("anthropic/claude-opus-4.6".into())
        );
    }

    #[test]
    fn moves_and_closes() {
        let mut list = models();
        list.key(&Key::Down);
        assert_eq!(list.key(&Key::Enter), Outcome::Chosen("openai/gpt-6-luna".into()));
        assert_eq!(list.key(&Key::Esc), Outcome::Closed);
    }

    #[test]
    fn sessions_draw_as_claude_codes_resume() {
        let item = |id: &str, label: &str, group: &str, current: bool| Item {
            id: id.into(),
            label: label.into(),
            detail: "3s ago · 4 messages".into(),
            current,
            group: Some(group.into()),
            older: false,
        };
        let list = List::new(
            Purpose::Session,
            "Resume session",
            vec![
                item("agent:t-1", "Fix the login", "project", true),
                item("c-1", "Plans", "Chats", false),
            ],
        );
        let (lines, cursor) = list.lines(&Theme::new(Style::new(false), None), 60);
        assert!(
            lines[0].chars().all(|c| c == '─') && lines[0].chars().count() == 59,
            "{lines:#?}"
        );
        assert_eq!(lines[1], "  Resume session (1 of 2)");
        assert!(
            lines[2].starts_with("  ╭") && lines[3].starts_with("  │ ⌕ Ctrl+F to filter") && lines[3].ends_with("│")
        );
        assert_eq!(display_width(&lines[3]), display_width(&lines[2]), "the box is square");
        assert_eq!(cursor, (3, 6));
        let body = &lines[5..];
        assert_eq!(body[0], "    project");
        assert_eq!(body[2], "  ❯ Fix the login (current)");
        assert_eq!(body[3], "    3s ago · 4 messages");
        assert_eq!((body[4].as_str(), body[5].as_str()), ("", "    Chats"));
        assert_eq!(body[7], "    Plans");
        assert!(lines.last().is_some_and(|keys| keys.contains("Esc to cancel")));
    }

    #[test]
    fn typing_in_the_sessions_starts_a_new_one_and_ctrl_f_filters() {
        let item = |id: &str, label: &str| Item {
            id: id.into(),
            label: label.into(),
            ..Item::default()
        };
        let mut list = List::new(
            Purpose::Session,
            "Resume session",
            vec![item("a", "Fix the login"), item("b", "Plans")],
        );
        assert_eq!(list.key(&Key::Char('h')), Outcome::Typed("h".into()));
        assert_eq!(list.key(&Key::Paste("hello".into())), Outcome::Typed("hello".into()));
        assert_eq!(list.key(&Key::CtrlF), Outcome::Open);
        for c in "plan".chars() {
            assert_eq!(list.key(&Key::Char(c)), Outcome::Open, "filtering now");
        }
        assert_eq!(list.key(&Key::Enter), Outcome::Chosen("b".into()));
        // Other lists search as they are typed in.
        let mut models = List::new(Purpose::Model, "Choose a model", vec![item("m", "Haiku")]);
        assert_eq!(models.key(&Key::Char('h')), Outcome::Open);
    }
}
