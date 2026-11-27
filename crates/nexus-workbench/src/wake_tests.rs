use serde_json::json;

use super::*;

fn parse(name: &str, input: &serde_json::Value) -> Option<Call> {
    super::parse(name, input, Timing::default())
}

#[test]
fn a_call_sets_a_wake_up_with_its_delay_clamped_as_claude_code_clamps_it() {
    let set = |input| parse(TOOL, &input);
    assert_eq!(
        set(json!({"delaySeconds": 270, "reason": "watching CI", "prompt": "/loop"})),
        Some(Call::Set {
            delay_ms: 270_000,
            reason: "watching CI".into()
        })
    );
    assert_eq!(
        set(json!({"delaySeconds": 5})),
        Some(Call::Set {
            delay_ms: 60_000,
            reason: String::new()
        })
    );
    assert_eq!(
        set(json!({"delaySeconds": 99_999})),
        Some(Call::Set {
            delay_ms: 3_600_000,
            reason: String::new()
        })
    );
}

#[test]
fn stop_clears_it_and_a_call_without_a_delay_or_another_tool_sets_nothing() {
    assert_eq!(parse(TOOL, &json!({"stop": true})), Some(Call::Stop));
    assert_eq!(
        parse(TOOL, &json!({"stop": true, "delaySeconds": 60})),
        Some(Call::Stop)
    );
    assert_eq!(parse(TOOL, &json!({"reason": "no delay"})), None);
    assert_eq!(parse(TOOL, &json!({"delaySeconds": "soon"})), None);
    assert_eq!(parse("Bash", &json!({"delaySeconds": 60})), None);
    assert_eq!(parse("mcp__x__ScheduleWakeup", &json!({"delaySeconds": 60})), None);
}

#[test]
fn a_reason_keeps_its_first_line_and_is_cut_short() {
    assert_eq!(
        clean_reason("  watching   the\tdeploy \nsecond line"),
        "watching the deploy"
    );
    let long = "a".repeat(REASON_MAX_CHARS + 10);
    let cut = clean_reason(&long);
    assert_eq!(cut.chars().count(), REASON_MAX_CHARS + 1);
    assert!(cut.ends_with('…'));
}

#[test]
fn a_turn_near_or_past_the_time_is_the_wake_up_and_an_early_one_is_not() {
    let at = 1_000_000;
    assert!(fired(at, at));
    assert!(fired(at, at + 60_000));
    assert!(fired(at, at - EARLY));
    assert!(!fired(at, at - EARLY - 1));
}

#[test]
fn it_may_still_come_until_its_grace_has_passed() {
    let at = 1_000_000;
    let grace = Timing::default().grace;
    let ms = grace.as_millis() as i64;
    assert!(may_come(at, at - 1, grace));
    assert!(may_come(at, at + ms - 1, grace));
    assert!(!may_come(at, at + ms, grace));
}

#[test]
fn a_lost_wake_up_says_when_it_was_due_and_why_it_will_not_come() {
    let clock = |_| "14:32".to_string();
    assert_eq!(
        lost_notice(0, Lost::Missed, clock),
        "The wake-up the agent set for 14:32 did not come. Send a message to continue."
    );
    let ended = |stop| lost_notice(0, Lost::Ended(stop), clock);
    assert_eq!(
        ended(None),
        "The wake-up the agent set for 14:32 will not come: the agent's process ended. Send a message to continue."
    );
    assert!(ended(Some(StopReason::Stop)).contains(": the agent was stopped."));
    assert!(ended(Some(StopReason::Restart)).contains(": the agent was stopped."));
    assert!(ended(Some(StopReason::Quit)).contains(": Nexus quit."));
    assert!(ended(Some(StopReason::Idle)).contains(": Nexus stopped the idle agent."));
    assert!(ended(Some(StopReason::Move)).contains(": the session moved to a new agent process."));
}

/// Readers find some notices by their text (`Workbench::steer_notice`): a wake-up's notice must not read as one.
#[test]
fn a_lost_wake_up_notice_reads_as_no_other_notice() {
    for lost in [Lost::Missed, Lost::Ended(None), Lost::Ended(Some(StopReason::Move))] {
        let text = lost_notice(0, lost, |_| "09:05".into());
        assert!(!text.contains("compacted the conversation"), "{text}");
        assert!(!text.starts_with(crate::moving::MOVED), "{text}");
        assert!(!text.starts_with("What the agent left running"), "{text}");
    }
}
