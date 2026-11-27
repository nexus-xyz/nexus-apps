use super::*;
// The glob above brings GPUI's own `test` macro; this is the built-in one.
use core::prelude::v1::test;

#[test]
fn finds_the_teams_a_person_leads() {
    let me = sample(r#"{"teams":[{"slug":"core","role":"lead"},{"slug":"docs","role":"member"}]}"#);
    assert_eq!(led_teams(&me), vec!["core".to_string()]);
    assert!(led_teams(&sample("{}")).is_empty());
}

#[test]
fn reads_a_typed_end_time_in_local_time() {
    use chrono::{Local, NaiveDate, TimeZone};
    let naive = NaiveDate::from_ymd_opt(2030, 1, 2)
        .unwrap()
        .and_hms_opt(18, 30, 0)
        .unwrap();
    let at = Local.from_local_datetime(&naive).earliest().unwrap().timestamp();
    let now = at - 3600;
    assert_eq!(custom_end("2030-01-02 18:30", now), Some(at));
    assert_eq!(
        custom_end(" 2030-01-02T18:30 ", now),
        Some(at),
        "the web's datetime-local form"
    );
    assert_eq!(custom_end("2030-01-02 18:30", at), None, "not in the future");
    assert_eq!(custom_end("tomorrow", now), None);
    assert_eq!(control_end(AT_A_TIME, "", now), None);
    assert_eq!(
        control_end(3600, "", now),
        Some(now + 3600),
        "a preset ignores the field"
    );
    assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
}

/// A sample answer, parsed into the shape Gateway sends.
fn sample(text: &str) -> Value {
    serde_json::from_str(text).expect("sample is JSON")
}

fn view() -> PolicyView {
    parse_view(&sample(
        r#"{"level": "organization", "subject": "organization", "rule": {"deny": ["openai/gpt-6-astra", "deepseek/*"], "defaults": {"chat": "anthropic/claude-opus-5-5", "codex": "openai/gpt-6"}, "maxPrice": {"input": 20, "output": 80}, "providers": {"deny": ["mistral"]}}, "custom": true, "updatedAt": "2026-11-01T10:00:00Z", "inherited": [], "effective": {"allowed": ["anthropic/claude-opus-5-5", "anthropic/claude-haiku-5", "openai/gpt-6"], "defaults": {"chat": {"model": "anthropic/claude-opus-5-5", "from": {"level": "organization", "list": "defaults"}}, "claude": {"model": "anthropic/claude-opus-5-5", "from": {"level": "catalog"}, "note": "Nexus's default"}, "codex": {"model": null}}}}"#,
    ))
}

#[test]
fn reads_a_policy_view() {
    let view = view();
    assert!(view.custom);
    assert!(!view.migrated);
    assert_eq!(view.deny, ["openai/gpt-6-astra", "deepseek/*"]);
    assert!(view.allow.is_empty());
    assert_eq!(view.allowed.len(), 3);
    assert_eq!(view.defaults["codex"], "openai/gpt-6");
    assert_eq!(view.effective_defaults["claude"].1.as_deref(), Some("Nexus's default"));
    assert_eq!(view.effective_defaults["codex"].0, None);
}

#[test]
fn picks_the_mode_a_chip_can_show() {
    let known = |id: &str| id == "openai/gpt-6-astra";
    let view = view();
    assert_eq!(mode_of(&view.allow, &view.deny, &known), Mode::Except);
    assert_eq!(mode_of(&[], &[], &known), Mode::All);
    assert_eq!(mode_of(&["openai/gpt-6-*".into()], &[], &known), Mode::Advanced);
    assert_eq!(
        mode_of(&["openai/gpt-6-astra".into()], &["x".into()], &known),
        Mode::Advanced
    );
    assert!(is_provider_wildcard("deepseek/*"));
    assert!(!is_provider_wildcard("openai/gpt-*"));
    assert_eq!(lines("a/*,\n b/c ,,"), ["a/*", "b/c"]);
}

#[test]
fn a_save_keeps_what_the_page_does_not_edit() {
    let view = view();
    let draft = Draft {
        allow: vec!["anthropic/claude-opus-5-5".into()],
        deny: Vec::new(),
        default_model: String::new(),
        only: true,
    };
    let rule = draft.rule(&view, Kind::Chat);
    assert_eq!(rule["allow"], json!(["anthropic/claude-opus-5-5"]));
    assert!(rule.get("deny").is_none());
    assert_eq!(rule["defaults"], json!({ "codex": "openai/gpt-6" }));
    assert_eq!(rule["maxPrice"]["input"], 20);
    assert_eq!(rule["providers"]["deny"], json!(["mistral"]));
    assert!(draft.differs(&view, Kind::Chat));
    let same = Draft {
        allow: Vec::new(),
        deny: vec!["deepseek/*".into(), "openai/gpt-6-astra".into()],
        default_model: "anthropic/claude-opus-5-5".into(),
        only: false,
    };
    assert!(!same.differs(&view, Kind::Chat));
    let empty_only = Draft {
        allow: Vec::new(),
        deny: Vec::new(),
        default_model: String::new(),
        only: true,
    };
    assert_eq!(empty_only.rule(&view, Kind::Claude)["allow"], json!([]));
}

#[test]
fn reads_a_decision_and_controls() {
    let blocked = parse_decision(&sample(
        r#"{"allowed": false, "requested": "openai/gpt-6-astra", "model": null, "surface": "openai_responses", "reason": "openai/gpt-6-astra is blocked by the organization's policy", "rule": {"level": "organization", "list": "deny", "pattern": "openai/gpt-6-astra"}, "code": "model_not_allowed", "agent": "codex", "allowedModels": [], "warnings": ["Claude Code needs a Haiku model"]}"#,
    ));
    assert!(!blocked.allowed);
    assert_eq!(blocked.rule, "organization · deny list · openai/gpt-6-astra");
    assert_eq!(blocked.warnings.len(), 1);
    assert_eq!(rule_ref(&json!({ "level": "team", "subject": "infra" })), "team infra");
    assert_eq!(rule_ref(&json!({ "level": "none" })), "no rule restricts it");

    let controls = parse_controls(&sample(
        r#"{"controls": [{"id": "ctl_1", "tenantId": "t1", "kind": "limit", "subjectLevel": "team", "subject": "team_9", "models": ["anthropic/claude-haiku-5"], "reason": "Cost spike", "createdBy": "user_1", "createdAt": "2026-11-05T10:00:00Z", "expiresAt": "2026-11-06T10:00:00.000Z"}, {"kind": "pause"}]}"#,
    ));
    assert_eq!(controls.len(), 1);
    assert_eq!(controls[0].level, "team");
    assert_eq!(controls[0].models, ["anthropic/claude-haiku-5"]);

    let members = parse_members(&sample(
        r#"{"members": [{"userId": "user_1", "email": "grace@acme.dev", "role": "admin", "joinedAt": "2026-01-01"}, {"userId": "user_2", "name": "Ada", "email": "ada@acme.dev", "role": "member", "joinedAt": "2026-01-01"}]}"#,
    ));
    assert_eq!(members[0].label, "grace@acme.dev");
    assert_eq!(members[1].label, "Ada");
    let teams = parse_teams(&json!({ "teams": [{ "id": "team_9", "slug": "infra", "name": "Infra" }] }));
    assert_eq!(teams[0].value, "infra");
    assert_eq!(teams[0].id, "team_9");
    let catalog = parse_catalog(&sample(
        r#"{"object": "list", "data": [{"id": "anthropic/claude-haiku-5", "name": "Claude Haiku 5", "surfaces": ["chat", "anthropic_messages"]}]}"#,
    ));
    assert_eq!(catalog[0].surfaces, ["chat", "anthropic_messages"]);
}

#[test]
fn times_read_and_write_in_utc() {
    assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(iso_utc(1_793_000_000), "2026-10-26T07:33:20Z");
    assert_eq!(parse_iso("2026-10-26T07:33:20.000Z"), Some(1_793_000_000));
    let now = parse_iso("2026-11-05T10:00:00Z").unwrap();
    assert_eq!(time_left("2026-11-05T10:30:00Z", now), "expires in 30m");
    assert_eq!(time_left("2026-11-06T10:00:00Z", now), "expires in 24h");
    assert_eq!(time_left("2026-11-12T10:00:00Z", now), "expires in 7d");
    assert_eq!(time_left("2026-11-05T09:00:00Z", now), "expired");
    assert_eq!(
        refusal("only the owner or admins can change the model policy"),
        "Only organization admins can change this."
    );
}
