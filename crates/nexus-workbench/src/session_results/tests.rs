//! Reading linked sessions' results back, and a round trip through the core's own frame.

use std::collections::HashMap;

use super::{ReadResult, is_result_batch, own_results, session_results};
use crate::event::SessionAgent;
use crate::linked::results::{RESULTS_HEADER, ResultStatus, SessionResult};

fn batch() -> String {
    format!(
        "{RESULTS_HEADER}\n<nexus-session-result session=\"child/one\" title=\"Builder &lt;one&gt;\" agent=\"codex\" status=\"completed\" seq=\"7\">Use &lt;script&gt; as text &amp; keep &amp;lt; escaped.</nexus-session-result>\n<nexus-session-result session=\"reviewer\" title=\"Reviewer\" agent=\"claude\" status=\"failed\" seq=\"9\">The review failed.</nexus-session-result>"
    )
}

#[test]
fn reads_the_cores_escaped_batched_frames_as_separate_reports() {
    let text = batch();
    assert!(is_result_batch(&text));
    let results = session_results(&text).expect("a batch");
    assert_eq!(
        results[0],
        ReadResult {
            session_id: "child/one".into(),
            title: "Builder <one>".into(),
            agent: SessionAgent::Codex,
            status: ResultStatus::Completed,
            text: "Use <script> as text & keep &lt; escaped.".into(),
        }
    );
    assert_eq!(results[1].session_id, "reviewer");
    assert_eq!(results[1].status, ResultStatus::Failed);
    assert_eq!(results[1].status_label(), "Failed");
}

#[test]
fn refuses_a_partial_or_malformed_batch() {
    let text = batch();
    assert_eq!(session_results(&format!("{text}\n<malformed>")), None);
    assert_eq!(session_results(&text[..text.len() - 5]), None);
    assert_eq!(session_results(RESULTS_HEADER), None);
    assert_eq!(session_results(&format!("{RESULTS_HEADER}\n")), None);
    assert_eq!(session_results("Plain message"), None);
    let zero = text.replace("seq=\"7\"", "seq=\"07\"");
    assert_eq!(session_results(&zero), None);
    let unknown = text.replace("agent=\"codex\"", "agent=\"gemini\"");
    assert_eq!(session_results(&unknown), None);
}

/// A batch with a Nexus child reads as its cards: the orchestra starts Nexus children, and the core writes
/// their frames.
#[test]
fn reads_a_nexus_childs_frame() {
    let result = SessionResult {
        session_id: "t_nexus".into(),
        title: "Nexus child".into(),
        agent: SessionAgent::Nexus,
        status: ResultStatus::Completed,
        last_message: "Done".into(),
        cost_usd: 0.0,
        branch: None,
        seq: 3,
    };
    let text = format!("{RESULTS_HEADER}\n{}", result.frame());
    let read = session_results(&text).expect("a batch");
    assert_eq!(read[0].agent, SessionAgent::Nexus);
    assert_eq!(read[0].text, "Done");
}

#[test]
fn reads_what_the_core_writes() {
    let result = SessionResult {
        session_id: "t_child".into(),
        title: "Fix \"quotes\" & <tags>".into(),
        agent: SessionAgent::Claude,
        status: ResultStatus::Stopped,
        last_message: "Stopped at <b> & \"done\"".into(),
        cost_usd: 0.0,
        branch: None,
        seq: 12,
    };
    let text = format!("{RESULTS_HEADER}\n{}", result.frame());
    let read = session_results(&text).expect("a batch");
    assert_eq!(read[0].title, result.title);
    assert_eq!(read[0].text, result.last_message);
    assert_eq!(read[0].status_label(), "Stopped");
}

#[test]
fn a_batch_shows_as_results_only_in_its_parent_from_its_own_children() {
    let parents: HashMap<&str, Option<&str>> = HashMap::from([
        ("lead", None),
        ("child/one", Some("lead")),
        ("reviewer", Some("lead")),
        ("stranger", Some("elsewhere")),
    ]);
    let parent_of = |id: &str| parents.get(id).copied();
    let parsed = || session_results(&batch());
    assert_eq!(
        own_results(parsed(), "child/one", Some("lead"), parent_of).map(|own| own.len()),
        Some(2)
    );
    // In another session, or with nothing on screen, it is a plain message.
    assert_eq!(own_results(parsed(), "child/one", Some("other"), parent_of), None);
    assert_eq!(own_results(parsed(), "child/one", None, parent_of), None);
    // From a known session that is not a child here: a plain message with all its text.
    assert_eq!(own_results(parsed(), "stranger", Some("lead"), parent_of), None);
    // A deleted sender still counts, so a batch stays a batch after its child goes.
    assert_eq!(
        own_results(parsed(), "gone", Some("lead"), parent_of).map(|own| own.len()),
        Some(2)
    );
}

#[test]
fn drops_the_frames_of_a_known_session_that_is_not_a_child() {
    let parents: HashMap<&str, Option<&str>> =
        HashMap::from([("child/one", Some("lead")), ("reviewer", Some("elsewhere"))]);
    let own = own_results(session_results(&batch()), "child/one", Some("lead"), |id| {
        parents.get(id).copied()
    });
    assert_eq!(
        own.map(|own| own.into_iter().map(|result| result.session_id).collect::<Vec<_>>()),
        Some(vec!["child/one".to_string()])
    );
}
