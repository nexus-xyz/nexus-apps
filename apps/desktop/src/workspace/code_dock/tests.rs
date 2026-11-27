// Not `super::*`: GPUI's prelude has a `test` attribute of its own.
use super::{
    PermissionSuggestion, Value, describe_all, next_step, plan_body, plan_title, question_answers, question_receipt,
    question_values, questions_of, receipt_rows,
};
use serde_json::json;

#[test]
fn private_answers_reach_the_agent_but_never_the_receipt() {
    let questions = questions_of(&json!({ "questions": [{ "id": "key", "question": "Key?", "isSecret": true }] }));
    assert!(questions[0].secret);
    let values = vec![vec!["private-test-value".to_string()]];
    assert_eq!(
        question_answers(&questions, &values),
        serde_json::Map::from_iter([("key".to_string(), json!(["private-test-value"]))])
    );
    assert_eq!(question_receipt(&questions, &values), "Private answer submitted");
}

#[test]
fn mixed_receipts_keep_ordinary_answers_and_redact_private_options_too() {
    let questions = questions_of(&json!({ "questions": [
        { "id": "name", "question": "Name?" },
        { "id": "key", "question": "Key?", "isSecret": true }
    ] }));
    let values = vec![vec!["Ada".into()], vec!["private-option".into(), "private-text".into()]];
    assert_eq!(
        question_receipt(&questions, &values),
        "Name?\nAda\n\nKey?\nPrivate answer submitted"
    );
    assert!(!questions[0].secret);
    let unflagged = questions_of(&json!({ "questions": [{ "question": "Name?", "isSecret": "true" }] }));
    assert!(!unflagged[0].secret);
}

/// A menu item's hover is `menu_hover`, which differs from the menu's own color in both themes: the Frost
/// board's selected-row ink, white at 8% in dark and black at 5% in light.
#[test]
fn a_menu_item_hover_shows_on_the_menu_in_both_themes() {
    use crate::palette::{Palette, ThemeChoice};
    use gpui_kit::{Hsla, StyleRefinement, WindowAppearance, rgba};

    for (choice, color) in [(ThemeChoice::Dark, 0xFFFFFF14), (ThemeChoice::Light, 0x0000000D)] {
        let palette = Palette::for_choice(choice, WindowAppearance::Light);
        let hover = super::menu_item_hover(StyleRefinement::default(), palette);
        let expected: Hsla = rgba(color).into();
        assert_eq!(hover.background, Some(expected.into()), "{choice:?}");
        assert_ne!(
            palette.menu, expected,
            "{choice:?}: the hover must differ from the menu"
        );
    }
}

/// JSON from text: `json!` with this much nesting reaches the macro recursion limit.
fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn next_goes_to_the_first_unanswered_question_then_the_review() {
    let answered = |picks: [bool; 3]| -> Vec<Vec<String>> {
        picks
            .iter()
            .map(|on| if *on { vec!["x".to_string()] } else { Vec::new() })
            .collect()
    };
    assert_eq!(next_step(&answered([true, false, false]), 0), 1);
    assert_eq!(next_step(&answered([true, true, false]), 0), 2);
    assert_eq!(next_step(&answered([true, true, true]), 0), 3);
    assert_eq!(next_step(&answered([false, false, false]), 2), 3);
}

#[test]
fn the_receipt_reads_back_as_one_row_per_question() {
    let questions = questions_of(&json!({ "questions": [
        { "question": "When?", "header": "Timing" }, { "question": "Where?" }
    ] }));
    assert_eq!(questions[0].header.as_deref(), Some("Timing"));
    let values = vec![vec!["Now".to_string()], vec!["Repo".to_string(), "Page".to_string()]];
    assert_eq!(
        receipt_rows(&questions, &question_receipt(&questions, &values)),
        Some(vec![
            ("When?".into(), "Now".into()),
            ("Where?".into(), "Repo, Page".into())
        ])
    );
    assert_eq!(
        receipt_rows(&questions[..1], "Now"),
        Some(vec![("When?".into(), "Now".into())])
    );
    assert_eq!(receipt_rows(&questions, "Something else"), None);
    assert_eq!(receipt_rows(&[], "Now"), None);
}

#[test]
fn answers_take_the_shape_each_agent_reads() {
    let input = parse(
        r#"{ "questions": [
                { "question": "Which?", "multiSelect": true, "options": [{ "label": "A" }, { "label": "B" }] },
                { "id": "q2", "question": "Name?", "options": [] }
            ]}"#,
    );
    let questions = questions_of(&input);
    assert_eq!(questions.len(), 2);
    let values = vec![
        question_values(&questions[0], &["A".into(), "B".into()], true, " C "),
        question_values(&questions[1], &[], true, "Ada"),
    ];
    let answers = question_answers(&questions, &values);
    assert_eq!(answers["Which?"], json!("A, B, C"));
    assert_eq!(answers["q2"], json!(["Ada"]));
    assert_eq!(question_receipt(&questions, &values), "Which?\nA, B, C\n\nName?\nAda");
}

#[test]
fn one_pick_with_your_own_words_drops_the_option() {
    let input = parse(r#"{ "questions": [{ "question": "Which?", "options": [{ "label": "A" }] }] }"#);
    let question = &questions_of(&input)[0];
    assert_eq!(
        question_values(question, &["A".into()], true, "Mine"),
        vec!["Mine".to_string()]
    );
    assert_eq!(
        question_values(question, &["A".into()], false, "Mine"),
        vec!["A".to_string()]
    );
}

#[test]
fn plans_read_by_their_heading() {
    assert_eq!(
        plan_title("\n# Plan: Add the dock\n\nSteps"),
        Some("Add the dock".into())
    );
    assert_eq!(plan_body("# Add the dock\n\n1. Read\n2. Write"), "1. Read\n2. Write");
    assert_eq!(plan_title(""), None);
}

#[test]
fn suggestions_read_in_plain_words() {
    let suggestion = |raw: Value| PermissionSuggestion {
        kind: "addRules".into(),
        label: "label".into(),
        raw,
    };
    let views = describe_all(&[
        suggestion(parse(
            r#"{ "type": "addRules", "destination": "session",
                    "rules": [{ "toolName": "Bash", "ruleContent": "npm test:*" }] }"#,
        )),
        suggestion(parse(
            r#"{ "type": "addRules", "destination": "session",
                    "rules": [{ "toolName": "Bash", "ruleContent": "npm test:*" }] }"#,
        )),
        suggestion(parse(r#"{ "type": "addRules", "behavior": "deny", "rules": [] }"#)),
        suggestion(parse(
            r#"{ "type": "addRules", "destination": "localSettings",
                    "rules": [{ "toolName": "Read", "ruleContent": "//home/ada/src/**" }] }"#,
        )),
    ]);
    assert_eq!(views.len(), 2);
    assert_eq!(views[0].title, "Commands starting with");
    assert_eq!(views[0].code.as_deref(), Some("npm test"));
    assert_eq!(views[0].scope, "For the rest of this session");
    assert_eq!(views[1].index, 3);
    assert_eq!(views[1].title, "Reading ~/src");
}
