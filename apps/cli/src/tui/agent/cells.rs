//! How agent work looks, in Codex's way: a `•` line for each tool that ran (`Ran`, `Read`, `Edited`…), the start and
//! end of a command's output under `└`, an edit's diff in colour, and the approval asked before an edit or a command.

use crate::style::truncate;

use super::super::view::Theme;
use super::Task;
use super::tools::{Action, Ask};

/// At most this many diff lines show for an edit, and this many output lines for a command.
const DIFF_LINES: usize = 16;
const OUTPUT_LINES: usize = 6;
/// The person's own commands (`!`) show more of what they printed.
const SHELL_LINES: usize = 40;

/// The lines for what a tool did.
pub fn tool_cell(theme: &Theme, action: &Action, width: usize) -> Vec<String> {
    let style = &theme.style;
    let bullet = style.dim(if style.unicode { "• " } else { "* " });
    let fit = |line: String| truncate(&line, width.saturating_sub(1), style.g.ellipsis);
    let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut lines = vec![String::new()];
    match action {
        Action::Read { path, lines: read } => {
            lines.push(fit(format!(
                "{bullet}{} {path} {}",
                style.bold("Read"),
                style.dim(&format!("({})", count(*read, "line", "lines")))
            )));
        }
        Action::Listed { path, entries } => {
            lines.push(fit(format!(
                "{bullet}{} {path} {}",
                style.bold("Listed"),
                style.dim(&format!("({})", count(*entries, "entry", "entries")))
            )));
        }
        Action::Searched { pattern, matches } => {
            lines.push(fit(format!(
                "{bullet}{} {} {}",
                style.bold("Searched for"),
                style.accent(pattern),
                style.dim(&format!("({})", count(*matches, "match", "matches")))
            )));
        }
        Action::Edited {
            path,
            added,
            removed,
            diff,
            created,
        } => {
            let verb = if *created { "Created" } else { "Edited" };
            let counts = format!(
                "{}{} {}{}",
                style.dim("("),
                style.green(&format!("+{added}")),
                style.red(&format!("-{removed}")),
                style.dim(")")
            );
            lines.push(fit(format!("{bullet}{} {path} {counts}", style.bold(verb))));
            lines.extend(diff_lines(theme, diff, width));
        }
        Action::Ran { command, exit, output } => {
            let status = match exit {
                Some(0) => String::new(),
                Some(code) => format!(" {}", style.red(&format!("(exit {code})"))),
                None => format!(" {}", style.red("(timed out)")),
            };
            lines.push(fit(format!(
                "{bullet}{} {}{status}",
                style.bold("Ran"),
                style.accent(command.lines().next().unwrap_or(""))
            )));
            lines.extend(output_lines(theme, output, width));
        }
        Action::Planned { steps } => {
            lines.push(fit(format!("{bullet}{}", style.bold("Updated plan"))));
            let corner = if style.unicode { "└" } else { "`" };
            for (index, (step, status)) in steps.iter().enumerate() {
                let prefix = style.dim(&if index == 0 {
                    format!("  {corner} ")
                } else {
                    "    ".into()
                });
                let line = match status.as_str() {
                    "completed" => format!(
                        "{} {}",
                        style.green(if style.unicode { "✓" } else { "x" }),
                        style.dim(step)
                    ),
                    "in_progress" => format!(
                        "{} {}",
                        style.accent(if style.unicode { "◐" } else { ">" }),
                        style.bold(step)
                    ),
                    _ => format!("{} {step}", style.dim(if style.unicode { "□" } else { "-" })),
                };
                lines.push(fit(format!("{prefix}{line}")));
            }
        }
        // One line: the person asked about Nexus, not for its docs.
        Action::Docs { topic, text } => {
            lines.push(fit(format!(
                "{bullet}{} {topic} {}",
                style.bold("Read Nexus docs:"),
                style.dim(&format!("({})", count(text.lines().count(), "line", "lines")))
            )));
        }
        Action::Called {
            name,
            input,
            output,
            error,
        } => {
            let status = if *error {
                format!(" {}", style.red("(failed)"))
            } else {
                String::new()
            };
            lines.push(fit(format!(
                "{bullet}{} {} {}{status}",
                style.bold("Called"),
                style.accent(name),
                style.dim(input)
            )));
            lines.extend(output_lines(theme, output, width));
        }
        Action::Failed { what, error } => {
            lines.push(fit(format!(
                "{} {} {}",
                style.fail,
                style.bold(&capitalize(what)),
                style.dim(&format!("· {error}"))
            )));
        }
    }
    lines
}

/// A subagent's lines: its `Agent` card as it starts, each tool it runs indented under it, and how it ended.
pub fn task_lines(theme: &Theme, task: &Task, width: usize) -> Vec<String> {
    let style = &theme.style;
    let fit = |line: String| truncate(&line, width.saturating_sub(1), style.g.ellipsis);
    match task {
        Task::Started { description, .. } => vec![
            String::new(),
            fit(format!(
                "{}{} {}",
                style.dim(if style.unicode { "• " } else { "* " }),
                style.bold("Agent"),
                style.accent(description)
            )),
        ],
        Task::Tool(action) => tool_cell(theme, action, width.saturating_sub(4))
            .into_iter()
            .skip(1)
            .map(|line| format!("    {line}"))
            .collect(),
        Task::Finished { result, error, .. } => {
            let corner = if style.unicode { "└" } else { "`" };
            let first = result.lines().find(|line| !line.trim().is_empty()).unwrap_or("");
            let status = if *error {
                style.red("failed:")
            } else {
                style.dim("done:")
            };
            vec![fit(format!("  {} {status} {}", style.dim(corner), style.dim(first)))]
        }
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// A diff, its lines coloured: added in green, removed in red, the rest dim. Long ones end with how many more.
pub fn diff_lines(theme: &Theme, diff: &[String], width: usize) -> Vec<String> {
    let style = &theme.style;
    let mut lines: Vec<String> = diff
        .iter()
        .take(DIFF_LINES)
        .map(|line| {
            let line = truncate(line, width.saturating_sub(5), style.g.ellipsis);
            let shown = if line.starts_with('+') {
                style.green(&line)
            } else if line.starts_with('-') {
                style.red(&line)
            } else {
                style.dim(&line)
            };
            format!("    {shown}")
        })
        .collect();
    if diff.len() > DIFF_LINES {
        lines.push(format!(
            "    {}",
            style.dim(&format!("… {} more lines", diff.len() - DIFF_LINES))
        ));
    }
    lines
}

/// A whole diff for `/diff`: the same colours, up to 400 lines.
pub fn long_diff(theme: &Theme, diff: &[String], width: usize) -> Vec<String> {
    let style = &theme.style;
    let mut lines: Vec<String> = diff
        .iter()
        .take(400)
        .map(|line| {
            let line = truncate(line, width.saturating_sub(5), style.g.ellipsis);
            let shown = if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ") {
                style.bold(&line)
            } else if line.starts_with('+') {
                style.green(&line)
            } else if line.starts_with('-') {
                style.red(&line)
            } else {
                style.dim(&line)
            };
            format!("    {shown}")
        })
        .collect();
    if diff.len() > 400 {
        lines.push(format!(
            "    {}",
            style.dim(&format!("… {} more lines (git diff shows them all)", diff.len() - 400))
        ));
    }
    lines
}

/// A command's output, dim under `└`: its first and last lines when it is long.
fn output_lines(theme: &Theme, output: &str, width: usize) -> Vec<String> {
    output_lines_up_to(theme, output, width, OUTPUT_LINES)
}

/// A command the person ran with `!`: the command after `!`, then up to 40 lines of what it printed.
pub fn shell_cell(theme: &Theme, action: &Action, width: usize) -> Vec<String> {
    let Action::Ran { command, exit, output } = action else {
        return tool_cell(theme, action, width);
    };
    let style = &theme.style;
    let status = match exit {
        Some(0) => String::new(),
        Some(code) => format!(" {}", style.red(&format!("(exit {code})"))),
        None => format!(" {}", style.red("(timed out)")),
    };
    let mut lines = vec![
        String::new(),
        truncate(
            &format!(
                "{} {}{status}",
                style.yellow(&style.bold("!")),
                command.lines().next().unwrap_or("")
            ),
            width.saturating_sub(1),
            style.g.ellipsis,
        ),
    ];
    lines.extend(output_lines_up_to(theme, output, width, SHELL_LINES));
    lines
}

fn output_lines_up_to(theme: &Theme, output: &str, width: usize, most: usize) -> Vec<String> {
    let style = &theme.style;
    let all: Vec<&str> = output.lines().filter(|line| !line.trim().is_empty()).collect();
    if all.is_empty() {
        return Vec::new();
    }
    let half = most / 2;
    let (shown, left_out): (Vec<&str>, usize) = if all.len() <= most {
        (all.clone(), 0)
    } else {
        ([&all[..half], &all[all.len() - half..]].concat(), all.len() - most)
    };
    let corner = if style.unicode { "└" } else { "`" };
    let fit = |line: &str| truncate(line, width.saturating_sub(5), style.g.ellipsis);
    let mut lines = Vec::new();
    for (index, line) in shown.iter().enumerate() {
        if left_out > 0 && index == half {
            lines.push(format!("    {}", style.dim(&format!("… {left_out} more lines"))));
        }
        let prefix = if index == 0 {
            format!("  {corner} ")
        } else {
            "    ".into()
        };
        lines.push(format!("{}{}", style.dim(&prefix), style.dim(&fit(line))));
    }
    lines
}

/// The question before an edit or a command, in place of the composer: what will happen, then the three answers.
pub fn approval_lines(theme: &Theme, ask: &Ask, selected: usize, width: usize) -> Vec<String> {
    let style = &theme.style;
    let mut lines = Vec::new();
    let kind = match ask {
        Ask::Edit { path, diff } => {
            lines.push(format!("  {}", style.bold(&format!("Allow this edit to {path}?"))));
            lines.extend(diff_lines(theme, diff, width));
            "edits"
        }
        Ask::Run { command } => {
            lines.push(format!("  {}", style.bold("Allow this command?")));
            for line in command.lines().take(6) {
                lines.push(format!(
                    "  {} {}",
                    style.dim("$"),
                    style.accent(&truncate(line, width.saturating_sub(6), style.g.ellipsis))
                ));
            }
            "commands"
        }
        Ask::Tool { name, input } => {
            lines.push(format!("  {}", style.bold(&format!("Allow {name}?"))));
            lines.push(format!(
                "  {}",
                style.dim(&truncate(input, width.saturating_sub(4), style.g.ellipsis))
            ));
            "this tool"
        }
    };
    lines.push(String::new());
    let answers = [
        ("y", "Yes".to_string()),
        ("a", format!("Yes, and don't ask again for {kind} in this session")),
        ("n", "No, and tell the agent what to do instead".to_string()),
    ];
    for (index, (key, answer)) in answers.iter().enumerate() {
        let label = format!("{}. {answer} {}", index + 1, style.dim(&format!("({key})")));
        lines.push(if index == selected {
            format!("{} {}", style.accent(style.g.select), style.accent(&label))
        } else {
            format!("  {label}")
        });
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;

    fn plain() -> Theme {
        Theme::new(Style::new(false), None)
    }

    #[test]
    fn a_command_shows_its_output_ends() {
        let output = (1..=10).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        let action = Action::Ran {
            command: "cargo test".into(),
            exit: Some(101),
            output,
        };
        let lines = tool_cell(&plain(), &action, 80);
        assert_eq!(lines[1], "• Ran cargo test (exit 101)");
        assert_eq!(lines[2], "  └ line 1");
        assert_eq!(lines[5], "    … 4 more lines");
        assert_eq!(lines.last().map(String::as_str), Some("    line 10"));
    }

    #[test]
    fn the_persons_command_shows_more_of_its_output() {
        let output = (1..=30).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        let action = Action::Ran {
            command: "ls".into(),
            exit: Some(0),
            output,
        };
        let lines = shell_cell(&plain(), &action, 80);
        assert_eq!(lines[1], "! ls");
        assert_eq!(
            lines.len(),
            2 + 30,
            "all 30 lines: under the 40 a person's command shows"
        );
    }

    #[test]
    fn an_edit_shows_its_counts_and_diff() {
        let action = Action::Edited {
            path: "src/main.rs".into(),
            added: 1,
            removed: 1,
            diff: vec!["@@ -1 +1 @@".into(), "-old".into(), "+new".into()],
            created: false,
        };
        assert_eq!(
            tool_cell(&plain(), &action, 80),
            [
                "",
                "• Edited src/main.rs (+1 -1)",
                "    @@ -1 +1 @@",
                "    -old",
                "    +new"
            ]
        );
    }

    #[test]
    fn a_plan_marks_each_step() {
        let steps = vec![
            ("Read the tests".into(), "completed".into()),
            ("Fix the parser".into(), "in_progress".into()),
            ("Run the suite".into(), "pending".into()),
        ];
        let lines = tool_cell(&plain(), &Action::Planned { steps }, 80);
        assert_eq!(
            lines,
            [
                "",
                "• Updated plan",
                "  └ ✓ Read the tests",
                "    ◐ Fix the parser",
                "    □ Run the suite"
            ]
        );
    }

    #[test]
    fn a_read_of_the_docs_is_one_line() {
        let action = Action::Docs {
            topic: "pages".into(),
            text: "# Pages\n\nA page is a file.\n".into(),
        };
        assert_eq!(
            tool_cell(&plain(), &action, 80),
            ["", "• Read Nexus docs: pages (3 lines)"]
        );
    }

    #[test]
    fn the_approval_names_the_command_and_marks_the_choice() {
        let lines = approval_lines(
            &plain(),
            &Ask::Run {
                command: "cargo test".into(),
            },
            1,
            80,
        );
        assert_eq!(lines[0], "  Allow this command?");
        assert_eq!(lines[1], "  $ cargo test");
        assert_eq!(
            lines[4],
            "❯ 2. Yes, and don't ask again for commands in this session (a)"
        );
    }

    #[test]
    fn a_subagent_shows_as_an_agent_card_with_its_tools_under_it() {
        let theme = plain();
        let started = task_lines(
            &theme,
            &Task::Started {
                description: "Read hello".into(),
                prompt: "Read hello.py.".into(),
                call: "call_1".into(),
            },
            80,
        );
        assert_eq!(started, ["", "• Agent Read hello"]);
        let read = task_lines(
            &theme,
            &Task::Tool(Action::Read {
                path: "hello.py".into(),
                lines: 1,
            }),
            80,
        );
        assert!(read[0].starts_with("    • Read hello.py"), "{read:?}");
        let finished = task_lines(
            &theme,
            &Task::Finished {
                result: "\nhello.py prints hi.\nMore.".into(),
                error: false,
                messages: Vec::new(),
            },
            80,
        );
        assert_eq!(finished, ["  └ done: hello.py prints hi."]);
    }
}
