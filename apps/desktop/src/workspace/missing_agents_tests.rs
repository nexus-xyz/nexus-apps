use super::{MissingRow, fold_label, install_label, missing_rows, terminal_command};
use nexus_workbench::event::SessionAgent;

const BOTH: [SessionAgent; 2] = [SessionAgent::Antigravity, SessionAgent::Cursor];

#[test]
fn the_agents_fold_into_one_row_that_opens_to_one_row_each() {
    assert_eq!(missing_rows(&[], true, ""), Vec::new(), "no row when every agent runs");
    assert_eq!(missing_rows(&BOTH, false, ""), vec![MissingRow::Fold(2)]);
    assert_eq!(
        missing_rows(&BOTH, true, "  "),
        vec![
            MissingRow::Fold(2),
            MissingRow::Agent(SessionAgent::Antigravity),
            MissingRow::Agent(SessionAgent::Cursor),
        ]
    );
}

#[test]
fn a_search_finds_a_folded_agent_by_its_name() {
    assert_eq!(
        missing_rows(&BOTH, false, "CURS"),
        vec![MissingRow::Agent(SessionAgent::Cursor)],
        "a match lists the agent with no fold, even while the fold is closed"
    );
    assert_eq!(missing_rows(&BOTH, true, "opus"), Vec::new());
}

#[test]
fn row_keys_are_stable_while_the_count_changes() {
    assert_eq!(MissingRow::Fold(2).key(), MissingRow::Fold(3).key());
    assert_ne!(
        MissingRow::Agent(SessionAgent::Cursor).key(),
        MissingRow::Agent(SessionAgent::Antigravity).key()
    );
}

#[test]
fn the_fold_names_the_place_and_fits_both_readiness_states() {
    assert_eq!(fold_label(2, false, None), "2 agents not set up here");
    assert_eq!(fold_label(1, false, None), "1 agent not set up here");
    assert_eq!(fold_label(2, true, None), "Not set up here");
    assert_eq!(
        fold_label(1, false, Some("Build server")),
        "1 agent not set up on Build server"
    );
    // An installed agent that can't run sessions sits under the fold too, so it never says "not installed".
    for open in [true, false] {
        assert!(!fold_label(2, open, None).to_lowercase().contains("installed"));
    }
}

#[test]
fn the_button_updates_an_installed_agent_and_names_a_server() {
    assert_eq!(install_label(false, None), "Install in Terminal");
    assert_eq!(install_label(true, None), "Update in Terminal");
    assert_eq!(install_label(false, Some("Build server")), "Install on Build server");
}

#[test]
fn a_terminal_on_this_computer_reaches_the_server_over_ssh_and_one_on_it_runs_bare() {
    let server = Some(("box", "dev@example"));
    assert_eq!(
        terminal_command(SessionAgent::Codex, "https://gateway", server, None),
        "ssh -t 'dev@example' 'npm install -g @openai/codex'"
    );
    assert_eq!(
        terminal_command(SessionAgent::Codex, "https://gateway", server, Some("box")),
        "npm install -g @openai/codex"
    );
    assert_eq!(
        terminal_command(SessionAgent::Codex, "https://gateway", server, Some("another")),
        "ssh -t 'dev@example' 'npm install -g @openai/codex'",
        "a Terminal on another server still reaches this one over ssh"
    );
    assert_eq!(
        terminal_command(SessionAgent::Codex, "https://gateway", None, None),
        "npm install -g @openai/codex"
    );
}
