use super::{Navigation, Target};
use core::prelude::v1::test;
use std::time::{Duration, Instant};
fn later() -> Instant {
    Instant::now() + Duration::from_secs(1)
}
#[test]
fn follows_exact_latest_ready_target() {
    let mut n = Navigation::new(false);
    n.offer(Target::Browser("tab-1".into()), true, 0);
    n.offer(Target::Terminal("exec-2".into()), true, 0);
    assert_eq!(n.take(false, true, later()), Some(Target::Terminal("exec-2".into())));
}
#[test]
fn human_input_cancels_pending_move_and_follow_resumes_latest() {
    let mut n = Navigation::new(false);
    n.offer(Target::Browser("one".into()), true, 0);
    n.interact();
    assert_eq!(n.take(false, true, later()), None);
    n.follow();
    assert_eq!(n.take(false, true, later()), Some(Target::Browser("one".into())));
}
#[test]
fn hidden_and_inactive_windows_never_navigate() {
    for (hidden, active) in [(true, true), (false, false)] {
        let mut n = Navigation::new(false);
        n.offer(Target::Changes("src/a.rs".into()), true, 0);
        assert_eq!(n.take(hidden, active, later()), None);
        assert_eq!(n.unread.len(), 1);
    }
}
#[test]
fn permission_requests_do_not_become_ready_targets() {
    let mut n = Navigation::new(false);
    n.offer(Target::Browser("ready".into()), true, 0);
    n.offer(Target::Terminal("awaitingUser".into()), false, 0);
    assert_eq!(n.take(false, true, later()), Some(Target::Browser("ready".into())));
}
#[test]
fn stale_readiness_cannot_undo_human_input() {
    let mut n = Navigation::new(false);
    n.interact();
    n.follow();
    n.offer(Target::Browser("old".into()), true, 0);
    assert_eq!(n.take(false, true, later()), None);
}
#[test]
fn manual_records_background_activity_without_navigation() {
    let mut n = Navigation::new(true);
    n.offer(Target::Browser("a".into()), true, 0);
    assert!(n.manual);
    assert_eq!(n.take(false, true, later()), None);
    assert_eq!(n.unread.len(), 1);
}
#[test]
fn interact_and_follow_switch_the_mode() {
    let mut n = Navigation::new(true);
    n.interact();
    assert!(n.manual);
    n.follow();
    assert!(!n.manual);
    n.interact();
    assert!(n.manual);
    n.sync_mode(false);
    assert!(!n.manual);
}
fn tool(id: &str, output: Option<&str>) -> crate::code::Block {
    crate::code::Block::Tool {
        call_id: id.into(),
        name: "functions.exec".into(),
        summary: String::new(),
        error: None,
        edit: None,
        images: vec![],
        call: Box::new(crate::code::ToolCall {
            id: id.into(),
            input: serde_json::json!({}),
            output: output.map(|text| crate::code::ToolOutput {
                text: text.into(),
                error: false,
                truncated: false,
            }),
        }),
    }
}
#[test]
fn native_browser_identity_and_primary_presentation_are_both_required() {
    let mut n = Navigation::new(false);
    let blocks = vec![tool("call", Some("tab-exact"))];
    assert!(n.collect(&blocks).is_empty());
    n.native.insert("tab-exact".into(), 0);
    assert_eq!(n.collect(&blocks), vec![(Target::Browser("tab-exact".into()), 0)]);
    assert!(n.collect(&blocks).is_empty());
}
#[test]
fn reads_console_reads_and_screenshots_follow_their_tab() {
    // A read's page, a console read's logs and a screenshot's image (`[image]`, then its tab as text), as the agents'
    // tool results show them.
    let results = [
        r#"{"title":"Home","text":"Hi","tab":"tab-a-1"}"#,
        r#"{"tab":"tab-a-1","logs":[]}"#,
        "[image]\n{\"tab\":\"tab-a-1\"}",
    ];
    for (index, output) in results.into_iter().enumerate() {
        let mut n = Navigation::new(false);
        n.native.insert("tab-a-1".into(), 0);
        let blocks = vec![tool(&format!("call-{index}"), Some(output))];
        assert_eq!(
            n.collect(&blocks),
            vec![(Target::Browser("tab-a-1".into()), 0)],
            "{output}"
        );
    }
}
#[test]
fn a_tab_id_never_matches_a_longer_one() {
    let mut n = Navigation::new(false);
    n.native.insert("tab-a-1".into(), 0);
    let blocks = vec![
        tool("call", Some(r#"{"tab":"tab-a-12"}"#)),
        crate::code::Block::Text("Opened tab-a-12.".into()),
    ];
    assert!(n.collect(&blocks).is_empty());
}
#[test]
fn restored_history_cannot_replay_navigation() {
    let mut n = Navigation::new(false);
    let blocks = vec![
        tool("old", Some("tab-exact")),
        crate::code::Block::Text("[File](src/a.rs:19)".into()),
    ];
    super::seed(&blocks, &mut n.seen);
    n.native.insert("tab-exact".into(), 0);
    assert!(n.collect(&blocks).is_empty());
}
#[test]
fn subagent_results_never_replace_the_primary_view() {
    let mut n = Navigation::new(false);
    n.native.insert("tab-child".into(), 0);
    let child = crate::code::Block::Inside {
        path: vec!["child".into()],
        block: Box::new(tool("child-call", Some("tab-child"))),
    };
    assert!(n.collect(&[child]).is_empty());
    assert_eq!(
        n.collect(&[tool("lead", Some("tab-child"))]),
        vec![(Target::Browser("tab-child".into()), 0)]
    );
}
#[test]
fn explicit_file_links_keep_exact_lines_and_streaming_does_not_replay_them() {
    let mut n = Navigation::new(false);
    let link = crate::code::Block::Text("[File](src/a.rs:19)".into());
    assert_eq!(n.collect(&[link]), vec![(Target::File("src/a.rs".into(), Some(19)), 0)]);
    assert!(
        n.collect(&[crate::code::Block::Text("[File](src/a.rs:19) More prose".into())])
            .is_empty()
    );
}
#[test]
fn late_result_retains_the_revision_from_before_human_input() {
    let mut n = Navigation::new(false);
    assert!(n.collect(&[tool("run", None)]).is_empty());
    n.interact();
    n.follow();
    let offers = n.collect(&[tool("run", Some("execution 12345678-1234-1234-1234-123456789abc"))]);
    assert_eq!(
        offers,
        vec![(Target::Terminal("12345678-1234-1234-1234-123456789abc".into()), 0)]
    );
    for (target, revision) in offers {
        n.offer(target, true, revision);
    }
    assert_eq!(n.take(false, true, later()), None);
}
#[test]
fn edits_cannot_target_another_folder() {
    // Full paths on this system: `/repo` is not one on Windows.
    let folder = std::env::temp_dir().join("repo");
    let inside = folder.join("src").join("a.rs");
    let relative = std::path::Path::new("src").join("a.rs");
    assert_eq!(
        super::scoped_path(&inside.to_string_lossy(), &folder),
        Some(relative.to_string_lossy().into_owned())
    );
    let sibling = folder.with_file_name("repo-two").join("a.rs");
    for path in [&*sibling.to_string_lossy(), "../private/a.rs", "src/../../private"] {
        assert_eq!(super::scoped_path(path, &folder), None);
    }
}
#[test]
fn switching_sessions_discards_pending_navigation() {
    let mut n = Navigation::new(false);
    n.offer(Target::Browser("old-session".into()), true, 0);
    n.reset_history();
    assert_eq!(n.take(false, true, later()), None);
    assert_eq!(n.baseline, None);
}
#[test]
fn codex_patch_paths_are_explicit_resources_including_orchestrated_calls() {
    let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-old\n+new\n*** Add File: src/b.rs\n+x\n*** End Patch";
    assert_eq!(
        super::patch_paths("functions.apply_patch", &serde_json::json!({"input":patch})),
        vec!["src/a.rs", "src/b.rs"]
    );
    let code = format!(
        "text(await tools.apply_patch({}));",
        serde_json::to_string(patch).unwrap()
    );
    assert_eq!(
        super::patch_paths("functions.exec", &serde_json::json!({"code":code})),
        vec!["src/a.rs", "src/b.rs"]
    );
    assert!(super::patch_paths("exec_command", &serde_json::json!({"command":"echo success"})).is_empty());
}
#[test]
fn explicit_follow_can_resume_a_ready_item_filtered_from_automatic_navigation() {
    let mut n = Navigation::new(false);
    n.offer_filtered(Target::Changes("a.rs".into()), true, 0, false);
    assert_eq!(n.take(false, true, later()), None);
    n.follow();
    assert_eq!(n.take(false, true, later()), Some(Target::Changes("a.rs".into())));
}

#[test]
fn restored_text_and_its_streamed_suffix_cannot_replay_old_resource_references() {
    let mut n = Navigation::new(false);
    let text = "Browser tab-old and execution 12345678-1234-1234-1234-123456789abc";
    super::seed(&[crate::code::Block::Text(text.into())], &mut n.seen);
    n.native.insert("tab-old".into(), 0);
    assert!(
        n.collect(&[crate::code::Block::Text(format!("{text} More prose"))])
            .is_empty()
    );
}
