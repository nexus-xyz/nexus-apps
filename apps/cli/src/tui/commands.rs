//! The session's slash commands, as Codex has them: `/` at the start of the composer opens a list under it, typing
//! narrows it, ↑ ↓ choose, tab completes and ⏎ runs. Plain data and functions; `app.rs` runs what is chosen.

use super::view::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slash {
    Model,
    New,
    Resume,
    Status,
    Spend,
    Suggestions,
    Approvals,
    Diff,
    Compact,
    Cost,
    Btw,
    Help,
    Quit,
}

/// Every command, in the order the list shows them, with what it does.
pub const COMMANDS: [(Slash, &str, &str); 13] = [
    (Slash::Model, "model", "choose the model for this session"),
    (Slash::New, "new", "start a new chat session"),
    (Slash::Resume, "resume", "continue an earlier chat session"),
    (Slash::Status, "status", "your organization, model, session and budget"),
    (Slash::Spend, "spend", "what is left of your budget"),
    (
        Slash::Suggestions,
        "suggestions",
        "turn suggested next prompts on or off",
    ),
    (Slash::Approvals, "approvals", "what the agent may do without asking"),
    (Slash::Diff, "diff", "what changed in this folder (git)"),
    (
        Slash::Compact,
        "compact",
        "summarize the agent's conversation, so each step costs less",
    ),
    (Slash::Cost, "cost", "what this session has spent, turn by turn"),
    (
        Slash::Btw,
        "btw",
        "ask a quick side question without interrupting the conversation",
    ),
    (Slash::Help, "help", "commands and keys"),
    (Slash::Quit, "quit", "leave nexus"),
];

/// Other names a command answers to.
const ALIASES: [(&str, Slash); 2] = [("exit", Slash::Quit), ("clear", Slash::New)];

/// The commands whose name starts with what follows the `/`, while the composer holds only `/word` (no space, one
/// line). `None` when the composer is not a command being typed.
pub fn matching(text: &str) -> Option<Vec<(Slash, &'static str, &'static str)>> {
    let word = text.strip_prefix('/')?;
    if word.contains(char::is_whitespace) {
        return None;
    }
    let word = word.to_lowercase();
    Some(
        COMMANDS
            .into_iter()
            .filter(|(_, name, _)| name.starts_with(&word))
            .collect(),
    )
}

/// `/name args` as the command it names and its arguments (trimmed). `None` when the text is not a slash command;
/// `Some(Err(name))` when it names no command.
pub fn parse(text: &str) -> Option<Result<(Slash, String), String>> {
    let rest = text.trim().strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let name = name.to_lowercase();
    let slash = COMMANDS
        .iter()
        .find(|(_, command, _)| *command == name)
        .map(|(slash, _, _)| *slash)
        .or_else(|| {
            ALIASES
                .iter()
                .find(|(alias, _)| *alias == name)
                .map(|(_, slash)| *slash)
        });
    Some(slash.map(|slash| (slash, args.trim().to_string())).ok_or(name))
}

/// The list under the composer: each command and what it does, the chosen one marked.
pub fn popup(theme: &Theme, matches: &[(Slash, &str, &str)], selected: usize) -> Vec<String> {
    let style = &theme.style;
    if matches.is_empty() {
        return vec![format!("  {}", style.dim("no command matches"))];
    }
    let column = matches.iter().map(|(_, name, _)| name.len()).max().unwrap_or(0) + 4;
    matches
        .iter()
        .enumerate()
        .map(|(index, (_, name, about))| {
            let command = format!("/{name}{}", " ".repeat(column - name.len() - 1));
            if index == selected {
                theme.selected(&format!(
                    "{} {}{}",
                    style.accent(style.g.select),
                    style.accent(&command),
                    about
                ))
            } else {
                format!("  {command}{}", style.dim(about))
            }
        })
        .collect()
}

/// `/help`: the commands, then where the keys are.
pub fn help(theme: &Theme) -> Vec<String> {
    let style = &theme.style;
    let mut lines = vec![String::new(), format!("  {}", style.bold("Commands"))];
    for (_, name, about) in COMMANDS {
        lines.push(format!(
            "  {}{}",
            style.accent(&format!("/{name:<13}")),
            style.dim(about)
        ));
    }
    lines.push(format!("  {}", style.dim("? on an empty composer shows the keys.")));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;

    #[test]
    fn narrows_while_a_command_is_typed() {
        let names = |text| matching(text).map(|found| found.iter().map(|(_, name, _)| *name).collect::<Vec<_>>());
        assert_eq!(names("/").map(|found| found.len()), Some(COMMANDS.len()));
        assert_eq!(names("/s"), Some(vec!["status", "spend", "suggestions"]));
        assert_eq!(names("/MO"), Some(vec!["model"]));
        assert_eq!(names("/model gpt"), None, "arguments: the list closes");
        assert_eq!(names("hello"), None);
    }

    #[test]
    fn parses_names_aliases_and_arguments() {
        assert_eq!(
            parse("/model  openai/gpt-6 "),
            Some(Ok((Slash::Model, "openai/gpt-6".into())))
        );
        assert_eq!(parse("/exit"), Some(Ok((Slash::Quit, String::new()))));
        assert_eq!(parse("/nope"), Some(Err("nope".into())));
        assert_eq!(parse("not a command"), None);
    }

    #[test]
    fn marks_the_chosen_command() {
        let theme = Theme::new(Style::new(false), None);
        let found = matching("/s").unwrap_or_default();
        assert_eq!(
            popup(&theme, &found, 1),
            [
                "  /status        your organization, model, session and budget",
                "❯ /spend         what is left of your budget",
                "  /suggestions   turn suggested next prompts on or off",
            ]
        );
    }
}
