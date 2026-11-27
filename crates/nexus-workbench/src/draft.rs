//! "Write it" in the git panel's commit form and the pull request form: the text drafted from the change by a model
//! through Nexus Gateway. What is read from the repository ([`Context`], by `Workbench::draft_context`,
//! on the machine that holds it), the request sent ([`request`]) and the answer read back ([`read`]). No I/O here: the
//! app makes the call ([`crate::draft_write`]), with the person's token.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The most of the diff sent, in bytes: about 10,000 tokens, enough to tell what a change does.
pub const DIFF_CAP: usize = 40_000;
/// The most of the pull request template sent, in bytes.
pub const TEMPLATE_CAP: usize = 8_000;
/// How many recent commit subjects show the repository's style.
pub const STYLE_SUBJECTS: usize = 20;
/// How many of the branch's own commits a pull request's draft reads.
pub const BRANCH_COMMITS: usize = 50;
/// How many untracked files are named.
pub const UNTRACKED_NAMES: usize = 100;
/// The longest title or commit message the forms take (their inputs' `maxLength`).
pub const TITLE_MAX: usize = 200;

/// What is drafted: the commit form's message, or the pull request form's title and description.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Commit,
    PullRequest,
}

/// What the draft is written from, as read from the repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub kind: Kind,
    /// The branch checked out; None on a detached HEAD.
    pub branch: Option<String>,
    /// A pull request's: the branch it merges into (`origin/main`).
    pub base: Option<String>,
    /// Recent commit subjects, newest first: the repository's style. A pull request's are its base's.
    pub subjects: Vec<String>,
    /// A pull request's: the branch's own commits, oldest first.
    pub commits: Vec<String>,
    /// A pull request's: the repository's template, cut at [`TEMPLATE_CAP`].
    pub template: Option<String>,
    /// The change as a unified diff, cut at [`DIFF_CAP`]: what isn't committed (a commit), or everything since the
    /// branch left its base (a pull request, which commits what isn't committed yet).
    pub diff: String,
    /// The diff was cut.
    pub cut: bool,
    /// New files git does not track yet, by path: a commit takes them too. At most [`UNTRACKED_NAMES`].
    pub untracked: Vec<String>,
}

/// The drafted text. A commit's is all in `title`, `body` empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub title: String,
    pub body: String,
}

/// The `POST /v1/chat/completions` body for `context`, on `model`.
pub fn request(model: &str, context: &Context) -> Value {
    json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system(context.kind) },
            { "role": "user", "content": prompt(context) },
        ],
        // Room for a reasoning model's thinking before the text; the text itself is a line or a short description.
        "max_tokens": match context.kind {
            Kind::Commit => 1_024,
            Kind::PullRequest => 4_096,
        },
    })
}

fn system(kind: Kind) -> &'static str {
    match kind {
        Kind::Commit => {
            "You write git commit messages from a diff. Answer with the commit message only: no preamble, no quotes, \
             no code fence."
        }
        Kind::PullRequest => {
            "You write pull request titles and descriptions from a branch's commits and diff. Answer with the title \
             and the description only: no preamble, no code fence."
        }
    }
}

/// The user message: the instructions, then what was read.
pub fn prompt(context: &Context) -> String {
    let mut text = String::new();
    match context.kind {
        Kind::Commit => text.push_str(
            "Write the subject line of a commit message for these changes.\n\
             - One line, at most 72 characters, in the imperative mood.\n\
             - Follow the style of the repository's recent subjects below: their prefix (such as `feat(scope):`), \
             their casing, and where an issue reference goes. Use an issue reference only when the branch name \
             carries one; never copy a number from another subject.\n\
             - Say what the change does, not how.\n",
        ),
        Kind::PullRequest => {
            text.push_str(
                "Write the title and the description of a pull request for this branch.\n\
                 Answer in this shape: the title on the first line, an empty line, then the description in Markdown.\n\
                 - The title: one line, at most 72 characters, in the style of the recent subjects below. Use an \
                 issue reference only when the branch name carries one; never copy a number from another subject.\n\
                 - The description: what changes and why, short and plain, for a reviewer.\n\
                 - State only what the commits and the diff show: do not claim tests were run or link issues they do \
                 not name.\n",
            );
            if context.template.is_some() {
                text.push_str(
                    "- Fill in the repository's pull request template below: keep its headings and their order, drop \
                     its HTML comments, and leave a section out when it does not apply.\n",
                );
            }
        }
    }
    if let Some(branch) = &context.branch {
        text.push_str(&format!("\nBranch: {branch}\n"));
    }
    if let Some(base) = &context.base {
        text.push_str(&format!("Merges into: {base}\n"));
    }
    if !context.commits.is_empty() {
        text.push_str("\nThe branch's commits, oldest first:\n");
        list(&mut text, &context.commits);
    }
    if !context.subjects.is_empty() {
        text.push_str(match context.kind {
            Kind::Commit => "\nRecent commit subjects in this repository:\n",
            Kind::PullRequest => "\nRecent commit subjects on the branch it merges into:\n",
        });
        list(&mut text, &context.subjects);
    }
    if let Some(template) = &context.template {
        text.push_str(&format!("\nThe pull request template:\n{template}\n"));
    }
    if !context.untracked.is_empty() {
        text.push_str("\nNew files, also part of the change:\n");
        list(&mut text, &context.untracked);
    }
    text.push_str(&format!(
        "\nThe diff{}:\n{}\n",
        if context.cut { ", cut short" } else { "" },
        context.diff
    ));
    text
}

fn list(text: &mut String, items: &[String]) {
    for item in items {
        text.push_str(&format!("- {item}\n"));
    }
}

/// The answer's text: the first choice's message, as Chat Completions answers it.
pub fn answer_text(body: &Value) -> Option<String> {
    let content = body.pointer("/choices/0/message/content")?;
    match content {
        Value::String(text) => Some(text.clone()),
        // Some providers answer in parts.
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<String>(),
        ),
        _ => None,
    }
}

/// The draft in the model's `answer`, tidied: a code fence, quotes and labels such as `Title:` dropped, the title cut
/// at [`TITLE_MAX`]. An error when there is no text.
pub fn read(kind: Kind, answer: &str) -> Result<Draft, String> {
    let text = unfenced(answer);
    let mut lines = text.lines();
    let title = lines
        .by_ref()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| clean_title(line, kind))
        .unwrap_or_default();
    if title.is_empty() {
        return Err("The model answered with no text. Try again, or write it yourself.".into());
    }
    let body = match kind {
        Kind::Commit => String::new(),
        Kind::PullRequest => {
            let rest = lines.collect::<Vec<_>>().join("\n");
            let rest = rest.trim();
            strip_label(rest, &["description:", "body:"]).trim().to_string()
        }
    };
    Ok(Draft { title, body })
}

/// `text` without a code fence around it.
fn unfenced(text: &str) -> &str {
    let text = text.trim();
    let Some(inner) = text.strip_prefix("```") else {
        return text;
    };
    // The fence's first line may name a language.
    let inner = inner.split_once('\n').map_or("", |(_, rest)| rest);
    inner.trim_end().strip_suffix("```").unwrap_or(inner).trim()
}

fn clean_title(line: &str, kind: Kind) -> String {
    let line = line.trim_start_matches('#').trim();
    let labels: &[&str] = match kind {
        Kind::Commit => &["commit message:", "subject:"],
        Kind::PullRequest => &["title:"],
    };
    let line = strip_label(line, labels).trim();
    let line = line
        .strip_prefix('`')
        .and_then(|inner| inner.strip_suffix('`'))
        .unwrap_or(line);
    let line = line.trim_matches(['"', '\'']).trim();
    line.chars().take(TITLE_MAX).collect::<String>().trim_end().to_string()
}

/// `text` without the first of `labels` it starts with, in any case.
fn strip_label<'a>(text: &'a str, labels: &[&str]) -> &'a str {
    labels
        .iter()
        .find_map(|label| {
            text.get(..label.len())
                .filter(|head| head.eq_ignore_ascii_case(label))
                .map(|_| &text[label.len()..])
        })
        .unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit() -> Context {
        Context {
            kind: Kind::Commit,
            branch: Some("ada/eng-12-login".into()),
            base: None,
            subjects: vec!["feat(app): sign in with a passkey".into(), "fix: a typo".into()],
            commits: vec![],
            template: None,
            diff: "diff --git a/a.txt b/a.txt\n-one\n+two".into(),
            cut: false,
            untracked: vec!["new.rs".into()],
        }
    }

    #[test]
    fn a_commit_prompt_carries_the_style_the_branch_and_the_change() {
        let text = prompt(&commit());
        assert!(text.contains("subject line of a commit message"));
        assert!(text.contains("Branch: ada/eng-12-login"));
        assert!(text.contains("- feat(app): sign in with a passkey\n- fix: a typo\n"));
        assert!(text.contains("New files, also part of the change:\n- new.rs\n"));
        assert!(text.ends_with("The diff:\ndiff --git a/a.txt b/a.txt\n-one\n+two\n"));
        assert!(!text.contains("template"));
        assert!(!text.contains("Merges into"));
    }

    #[test]
    fn a_pull_request_prompt_fills_in_the_template_and_says_a_cut_diff_is_cut() {
        let context = Context {
            kind: Kind::PullRequest,
            base: Some("origin/main".into()),
            commits: vec!["first".into(), "second".into()],
            template: Some("## Summary\n<!-- what -->".into()),
            cut: true,
            untracked: vec![],
            ..commit()
        };
        let text = prompt(&context);
        assert!(text.contains("the title on the first line, an empty line, then the description"));
        assert!(text.contains("Fill in the repository's pull request template"));
        assert!(text.contains("Merges into: origin/main\n"));
        assert!(text.contains("The branch's commits, oldest first:\n- first\n- second\n"));
        assert!(text.contains("Recent commit subjects on the branch it merges into:"));
        assert!(text.contains("The pull request template:\n## Summary\n<!-- what -->\n"));
        assert!(text.contains("The diff, cut short:\n"));
        assert!(!text.contains("New files"));
    }

    #[test]
    fn the_request_names_the_model_and_both_messages() {
        let body = request("nexus/auto", &commit());
        assert_eq!(body["model"], "nexus/auto");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], prompt(&commit()));
        assert_eq!(body["max_tokens"], 1_024);
    }

    #[test]
    fn reads_the_answer_text_as_a_string_or_in_parts() {
        let plain = json!({ "choices": [{ "message": { "content": "Add it" } }] });
        assert_eq!(answer_text(&plain).as_deref(), Some("Add it"));
        let parts = json!({ "choices": [{ "message": { "content": [{ "type": "text", "text": "Add " }, { "text": "it" }] } }] });
        assert_eq!(answer_text(&parts).as_deref(), Some("Add it"));
        assert_eq!(answer_text(&json!({ "choices": [] })), None);
    }

    #[test]
    fn a_commit_message_is_its_first_line_without_fence_label_or_quotes() {
        for answer in [
            "feat(app): add it",
            "  \n\"feat(app): add it\"\n",
            "```\nfeat(app): add it\n```",
            "```text\nCommit message: `feat(app): add it`\n```",
            "Subject: feat(app): add it\n\nA body the form has no room for.",
        ] {
            let draft = read(Kind::Commit, answer).unwrap_or_else(|e| panic!("{answer}: {e}"));
            assert_eq!(
                draft,
                Draft {
                    title: "feat(app): add it".into(),
                    body: String::new()
                },
                "{answer}"
            );
        }
    }

    #[test]
    fn a_pull_request_is_a_title_line_then_its_description() {
        let draft = read(
            Kind::PullRequest,
            "# Title: Sign in with a passkey\n\n## Summary\nAdds it.\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(draft.title, "Sign in with a passkey");
        assert_eq!(draft.body, "## Summary\nAdds it.");
        let labelled = read(Kind::PullRequest, "Title: X\nDescription: Why.").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            labelled,
            Draft {
                title: "X".into(),
                body: "Why.".into()
            }
        );
    }

    #[test]
    fn an_empty_answer_is_an_error_and_a_long_title_is_cut() {
        assert!(read(Kind::Commit, "  \n```\n```").is_err());
        assert!(read(Kind::PullRequest, "").is_err());
        let long = "é".repeat(TITLE_MAX + 50);
        let draft = read(Kind::Commit, &long).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(draft.title.chars().count(), TITLE_MAX);
    }
}
