use serde_json::json;

use super::*;

const PR: &str = "https://github.com/o/r/pull/12";
const MR: &str = "https://gitlab.com/g/p/-/merge_requests/7";

/// What `gh pr create` prints: a progress line on stderr, then the new pull request's URL.
fn created(url: &str) -> String {
    format!("\nCreating pull request for feat into main in o/r\n\n{url}\n")
}

#[test]
fn a_create_command_links_the_url_it_printed() {
    assert_eq!(created_by("gh pr create --fill", &created(PR)), [PR]);
    assert_eq!(created_by("glab mr create --fill --yes", &created(MR)), [MR]);
    assert_eq!(
        created_by(
            "git push -u origin HEAD && gh pr create --base main --fill",
            &created(PR)
        ),
        [PR],
        "after a push"
    );
    assert_eq!(
        created_by(
            "GH_PROMPT_DISABLED=1 timeout 60 /usr/local/bin/gh pr create --fill",
            &created(PR)
        ),
        [PR],
        "past assignments, wrappers and a folder"
    );
    assert_eq!(
        created_by("url=$(gh pr create --fill) && echo \"$url\"", &created(PR)),
        [PR],
        "inside a command substitution"
    );
}

#[test]
fn a_body_in_a_heredoc_is_text_not_commands() {
    let command = "gh pr create --title \"Fix\" --body \"$(cat <<'EOF'\nSee gh pr list and #12.\nEOF\n)\"";
    assert_eq!(created_by(command, &created(PR)), [PR]);
    let unquoted = "cat <<EOF > body.md\ngh pr list\nEOF\ngh pr create --body-file body.md";
    assert_eq!(
        created_by(unquoted, &created(PR)),
        [PR],
        "an unquoted heredoc's body too"
    );
}

#[test]
fn codex_wraps_the_script_in_a_shell() {
    let command = "/bin/zsh -lc 'git push && gh pr create --fill'";
    assert_eq!(created_by(command, &created(PR)), [PR]);
    let pieces = "/bin/zsh -lc \"gh pr create --title \"'\"Fix it\"'\" --fill\"";
    assert_eq!(created_by(pieces, &created(PR)), [PR], "quoted in pieces");
}

#[test]
fn a_create_command_as_text_creates_nothing() {
    assert!(created_by("echo \"gh pr create\"", &created(PR)).is_empty());
    assert!(created_by("echo 'run gh pr create'", &created(PR)).is_empty());
    assert!(created_by("# gh pr create\nls", &created(PR)).is_empty());
    assert!(
        created_by("gh pr create#x", &created(PR)).is_empty(),
        "another subcommand"
    );
    assert!(created_by("gh pr view 12", &created(PR)).is_empty());
}

#[test]
fn a_url_that_existed_before_is_not_created() {
    assert!(
        created_by("gh pr create --fill || gh pr view 12", &created(PR)).is_empty(),
        "the script names its number"
    );
    assert!(
        created_by("gh pr create --fill; gh pr list", &created(PR)).is_empty(),
        "a list prints other pull requests"
    );
    assert!(
        created_by("gh pr create --fill && gh api repos/o/r/pulls", &created(PR)).is_empty(),
        "so does the API"
    );
    let two = format!("{PR}\nhttps://github.com/o/r/pull/13\n");
    assert!(
        created_by("gh pr create --fill", &two).is_empty(),
        "more URLs than create commands"
    );
}

#[test]
fn only_a_request_alone_on_its_line_of_the_host_that_ran_counts() {
    assert!(created_by("gh pr create --fill", &format!("Opened {PR}")).is_empty());
    assert!(
        created_by("gh pr create --fill", &created(MR)).is_empty(),
        "glab did not run"
    );
    assert!(created_by("gh pr create --fill", "https://github.com/o/r/issues/12\n").is_empty());
    assert!(created_by("gh pr create --fill", "https://github.com/o/r/pull/12/files\n").is_empty());
    assert!(created_by("gh pr create --fill", "a pull request already exists").is_empty());
    assert_eq!(
        created_by(
            "gh pr create --fill",
            &format!("a pull request for branch \"feat\" into branch \"main\" already exists:\n{PR}\n")
        ),
        [PR],
        "the branch's own one, opened before: it is the session's too"
    );
}

#[test]
fn a_running_session_links_when_the_create_command_s_result_comes() {
    let mut commands = Commands::default();
    let tool = |id: &str, name: &str, command: &str| WbEvent::Tool {
        id: id.into(),
        name: name.into(),
        input: json!({ "command": command }),
        parent: None,
    };
    let result = |id: &str, output: &str| WbEvent::ToolResult {
        id: id.into(),
        output: output.into(),
        images: vec![],
        is_error: false,
        truncated: false,
        parent: None,
    };
    assert!(commands.event(&tool("t1", "Bash", "gh pr create --fill")).is_empty());
    assert!(commands.event(&tool("t2", "Bash", "ls")).is_empty());
    assert_eq!(commands.0.len(), 1, "only a command that may create one is kept");
    assert!(commands.event(&result("t2", PR)).is_empty(), "ls created nothing");
    assert_eq!(commands.event(&result("t1", &created(PR))), [PR]);
    assert!(commands.0.is_empty(), "a result ends its command");
    assert!(commands.event(&result("t1", &created(PR))).is_empty(), "told once");
    assert!(commands.event(&tool("t3", "Read", "gh pr create")).is_empty());
    assert!(commands.0.is_empty(), "only the shell runs commands");
}

#[test]
fn heredoc_stripping_keeps_a_shift_and_a_here_string() {
    assert_eq!(strip_heredocs("echo $((1<<2))"), "echo $((1<<2))");
    assert_eq!(strip_heredocs("cat <<< \"x\""), "cat <<< \"x\"");
    assert_eq!(strip_heredocs("cat <<EOF\nbody\nEOF\nls"), "cat <<EOF\nls");
}
