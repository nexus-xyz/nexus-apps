//! The pull requests a session's agent opened from its shell. A finished `gh pr create` (or `glab mr create`) prints
//! the new pull request's URL on a line of its own, and the session saves that URL at once
//! (`Workbench::link_created_pull_request`). The pull request watch then reads that exact pull request. It does not
//! guess from the folder's branch, and it does not wait for the turn to end. A session in its project's folder gets
//! its link this way too. The rules follow T3 Code's `changeRequestsCreatedBy` (MIT).
//!
//! A URL counts only when the command surely created it:
//! - a create command runs at the start of a command, not as text (`echo "gh pr create"`, a quoted string, a heredoc
//!   body or a comment);
//! - the URL is alone on its line, and it is a request of the host whose create command ran;
//! - no other command in the script names its number (`gh pr view 12` names one that existed before);
//! - no other command lists requests (`gh pr list`, `gh api`), and there are no more URLs than create commands.

use std::collections::HashMap;

use serde_json::Value;

use crate::event::WbEvent;

/// The tool every agent's shell command is named by (Claude Code's name; codex.rs and acp.rs use it too).
const SHELL_TOOL: &str = "Bash";

/// A host's CLI, the words that open a request with it, and the path of a request's URL.
struct Create {
    program: &'static str,
    words: [&'static str; 2],
    path: &'static str,
}

const CREATES: [Create; 2] = [
    Create {
        program: "gh",
        words: ["pr", "create"],
        path: "/pull/",
    },
    Create {
        program: "glab",
        words: ["mr", "create"],
        path: "/-/merge_requests/",
    },
];

/// Commands that print other requests' URLs: a URL beside them proves nothing.
const LISTS: [&[&str]; 6] = [
    &["gh", "pr", "list"],
    &["gh", "pr", "status"],
    &["gh", "search", "prs"],
    &["gh", "api"],
    &["glab", "mr", "list"],
    &["glab", "api"],
];

/// The shell commands of a running session that may open a request, by tool call, until their results come back.
#[derive(Debug, Default)]
pub(crate) struct Commands(HashMap<String, String>);

impl Commands {
    /// Takes in one of the session's events. When a create command's result comes, returns the URLs it created.
    pub(crate) fn event(&mut self, event: &WbEvent) -> Vec<String> {
        match event {
            WbEvent::Tool { id, name, input, .. } if name == SHELL_TOOL => {
                if let Some(command) = input.get("command").and_then(Value::as_str)
                    && mentions_create(command)
                {
                    self.0.insert(id.clone(), command.to_string());
                }
                Vec::new()
            }
            WbEvent::ToolResult { id, output, .. } => self
                .0
                .remove(id)
                .map(|command| created_by(&command, output))
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

/// A cheap first look: whether the command may run a create command at all.
fn mentions_create(command: &str) -> bool {
    command.contains("create") && (command.contains("gh") || command.contains("glab"))
}

/// The URLs of the requests that `command` created, from its `output`. Empty when it is not sure.
pub fn created_by(command: &str, output: &str) -> Vec<String> {
    if !mentions_create(command) {
        return Vec::new();
    }
    let script = strip_heredocs(&unwrap_shell(command));
    let mut creators: Vec<usize> = Vec::new();
    let mut creates = 0;
    let mut others: Vec<&str> = Vec::new();
    for segment in segments(&script) {
        let code = code(segment);
        let mut runs = 0;
        // `$(…)`, a backtick, a subshell or a group starts a command of its own.
        for part in code.split(['(', ')', '`', '{', '}']) {
            let words: Vec<&str> = part.split_whitespace().collect();
            if lists(&words) {
                return Vec::new();
            }
            if let Some(create) = create_of(&words) {
                runs += 1;
                if !creators.contains(&create) {
                    creators.push(create);
                }
            }
        }
        if runs == 0 {
            others.push(segment);
        }
        creates += runs;
    }
    if creates == 0 {
        return Vec::new();
    }
    let mut created: Vec<String> = Vec::new();
    for line in output.lines() {
        let url = line.trim();
        let Some((create, number)) = request_url(url) else {
            continue;
        };
        if !creators.contains(&create) || others.iter().any(|other| names(other, number)) {
            continue;
        }
        if !created.iter().any(|known| known == url) {
            created.push(url.to_string());
        }
    }
    if created.len() > creates {
        return Vec::new();
    }
    created
}

/// The command's words past `NAME=value` assignments and wrappers such as `env`, `time` and `timeout 60`.
fn command_words<'a>(words: &'a [&'a str]) -> &'a [&'a str] {
    let mut rest = words;
    loop {
        match rest {
            [word, more @ ..]
                if is_assignment(word)
                    || matches!(
                        *word,
                        "env" | "time" | "nohup" | "command" | "exec" | "do" | "then" | "else" | "!"
                    ) =>
            {
                rest = more;
            }
            ["timeout", _, more @ ..] => rest = more,
            _ => return rest,
        }
    }
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=')
        .is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// The program a command runs, without its folder: `/usr/local/bin/gh` is `gh`.
fn program(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// Which host's create command these words run, if any.
fn create_of(words: &[&str]) -> Option<usize> {
    let words = command_words(words);
    let (first, rest) = words.split_first()?;
    CREATES
        .iter()
        .position(|create| program(first) == create.program && rest.starts_with(&create.words))
}

/// Whether these words run a command that prints other requests' URLs, anywhere in them (`xargs gh pr list`).
fn lists(words: &[&str]) -> bool {
    (0..words.len()).any(|start| {
        LISTS.iter().any(|list| {
            words.len() - start >= list.len()
                && program(words[start]) == list[0]
                && words[start + 1..start + list.len()] == list[1..]
        })
    })
}

/// Unwraps Codex's `/bin/zsh -lc '<script>'` into the script it runs; any other input is the script.
fn unwrap_shell(input: &str) -> String {
    let trimmed = input.trim_start();
    let mut parts = trimmed.splitn(3, char::is_whitespace);
    let (Some(shell), Some(flag), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        return input.to_string();
    };
    let is_shell = matches!(program(shell), "sh" | "bash" | "zsh" | "dash" | "ksh");
    let rest = rest.trim_start();
    if !is_shell || !matches!(flag, "-c" | "-lc") || !rest.starts_with(['"', '\'']) {
        return input.to_string();
    }
    // One shell word, quoted in pieces such as `"a "'b'" c"`.
    let mut script = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => script.extend(chars.by_ref().take_while(|&c| c != '\'')),
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some(next @ ('"' | '\\' | '$' | '`')) => script.push(next),
                            Some(next) => {
                                script.push('\\');
                                script.push(next);
                            }
                            None => {}
                        },
                        _ => script.push(c),
                    }
                }
            }
            '\\' => script.extend(chars.next()),
            c if c.is_whitespace() => break,
            c => script.push(c),
        }
    }
    script
}

/// The script without its heredoc bodies: text a command reads, not commands it runs. The `<<` stays.
fn strip_heredocs(script: &str) -> String {
    let mut out = String::new();
    let mut rest = script;
    while let Some(at) = rest.find("<<") {
        let after = &rest[at + 2..];
        if let Some(word) = after.strip_prefix('<') {
            // `<<<` is a here-string: one word, no body.
            out.push_str(&rest[..at + 3]);
            rest = word;
            continue;
        }
        let marker = after.strip_prefix('-').unwrap_or(after).trim_start_matches([' ', '\t']);
        let marker = marker.trim_start_matches(['\'', '"']);
        let word: String = marker
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        // A delimiter is a word, such as `EOF`: `1<<2` is a shift.
        if !word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            out.push_str(&rest[..at + 2]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..at + 2]);
        // The rest of the marker's line stays: it holds the command's other words.
        let Some(line_end) = after.find('\n') else {
            out.push_str(after);
            return out;
        };
        out.push_str(&after[..=line_end]);
        let mut body = &after[line_end + 1..];
        rest = "";
        while !body.is_empty() {
            let (line, next) = body.split_once('\n').unwrap_or((body, ""));
            body = next;
            if line.trim() == word {
                rest = body;
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The script split at `&&`, `||`, `;`, `|` and new lines that sit outside quotes.
fn segments(script: &str) -> Vec<&str> {
    let bytes = script.as_bytes();
    let mut segments = Vec::new();
    let (mut start, mut index) = (0, 0);
    let mut quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if byte == b'\\' && open == b'"' {
                index += 1;
            } else if byte == open {
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'\\' => index += 1,
            b'&' | b'|' if bytes.get(index + 1) == Some(&byte) => {
                segments.push(&script[start..index]);
                index += 2;
                start = index;
                continue;
            }
            b';' | b'|' | b'\n' => {
                segments.push(&script[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    segments.push(&script[start.min(script.len())..]);
    segments
}

/// The commands a segment runs, without the text it carries: each quoted string becomes `''`, and comments go.
fn code(segment: &str) -> String {
    let mut out = String::new();
    let mut chars = segment.chars().peekable();
    let mut word_start = true;
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                }
                out.push_str("''");
            }
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
                out.push_str("''");
            }
            '\\' => {
                out.push(c);
                out.extend(chars.next());
            }
            '#' if word_start => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            c => out.push(c),
        }
        word_start = out.ends_with(|c: char| c.is_whitespace() || "();&|".contains(c));
    }
    out
}

/// A request URL alone on its line: which host's, and its number.
fn request_url(url: &str) -> Option<(usize, u64)> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    if rest.contains(char::is_whitespace) {
        return None;
    }
    CREATES.iter().enumerate().find_map(|(index, create)| {
        let (repository, number) = rest.split_once(create.path)?;
        // A host, then at least an owner and a repository.
        if repository.split('/').filter(|part| !part.is_empty()).count() < 3 {
            return None;
        }
        let number = number.strip_suffix('/').unwrap_or(number);
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some((index, number.parse().ok()?))
    })
}

/// Whether `text` names `number` as a word of its own (`gh pr view 12`, `#12`, `…/pull/12`), not inside another.
fn names(text: &str, number: u64) -> bool {
    let number = number.to_string();
    text.match_indices(&number).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + number.len()..].chars().next();
        before.is_none_or(|c| c.is_whitespace() || "#/=(\"'".contains(c))
            && after.is_none_or(|c| c.is_whitespace() || ");&|/\"'".contains(c))
    })
}

#[cfg(test)]
#[path = "created_pulls_tests.rs"]
mod tests;
