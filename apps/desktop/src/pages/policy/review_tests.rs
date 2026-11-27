use super::{Draft, review_ready};
#[test]
fn edits_after_review_require_another_review() {
    let mut reviewed = None;
    let mut draft = Draft {
        allow: vec!["anthropic/*".into()],
        deny: vec![],
        default_model: String::new(),
        only: true,
    };
    assert!(!review_ready(&mut reviewed, &draft));
    assert!(review_ready(&mut reviewed, &draft));
    draft.deny.push("anthropic/legacy".into());
    assert!(!review_ready(&mut reviewed, &draft));
    assert!(review_ready(&mut reviewed, &draft));
    reviewed = None;
    assert!(!review_ready(&mut reviewed, &draft));
}
