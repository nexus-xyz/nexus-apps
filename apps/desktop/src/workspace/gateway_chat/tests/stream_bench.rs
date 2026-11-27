//! What a streamed token costs in a long Gateway chat: the Owner's receive, the projection and the native text.
//!
//! Run it alone, in an optimized test binary, from the crate's directory:
//! `cargo test --locked --release stream_history_benchmark -- --ignored --nocapture --test-threads=1`.
//! It uses only what main has too, so the same file measures main for comparison.
use super::{Probe, ask, delta, fixture, named};
use crate::model::Meta;
use crate::workspace::Entry;
use gpui_kit::TestAppContext;
use std::time::Instant;

/// Stored messages before the question: half questions, half answers.
const HISTORY: usize = 200;
const DELTAS: usize = 2_000;
const SAMPLES: usize = 7;

/// About 3 KB of Markdown, different for each answer.
fn stored_answer(n: usize) -> String {
    let mut text = format!("## Answer {n}\n\n");
    while text.len() < 3_000 {
        text.push_str(&format!(
            "A **stored** paragraph of answer {n}, with `code`, a [link](https://example.test/{n}) and héllo.\n\n"
        ));
    }
    text
}

#[gpui_kit::test]
#[ignore = "performance measurement: run alone in an optimized test binary"]
fn stream_history_benchmark(cx: &mut TestAppContext) {
    assert!(!cfg!(debug_assertions), "use cargo test --release");
    let _agents = crate::code_run::fixture_agents(vec![]);
    let probe = Probe::new();
    let chunks: Vec<String> = (0..DELTAS).map(|n| format!("w{n} ")).collect();
    let final_text = chunks.concat();
    let mut samples = Vec::new();
    // Round 0 warms allocations and is left out.
    for round in 0..=SAMPLES {
        let (workspace, cx) = fixture(cx, &probe);
        workspace.update(cx, |workspace, cx| {
            workspace.entries = (1..=HISTORY)
                .map(|id| {
                    if id % 2 == 1 {
                        Entry::user(id, format!("Question {id}"))
                    } else {
                        let mut entry = Entry::answer(id, &stored_answer(id), false, cx);
                        entry.meta = Meta {
                            model: Some("anthropic/claude-sonnet-5.5".into()),
                            auto: true,
                            cost_usd: Some(0.0123),
                            tokens: Some(1_234),
                            created_at: Some("2026-11-13T12:00:00Z".into()),
                        };
                        entry
                    }
                })
                .collect();
            workspace.next_entry = HISTORY;
        });
        // Parse the stored answers before the clock starts.
        cx.run_until_parked();
        let asked = ask(&workspace, cx, &probe, "Question");
        named(&probe, &asked, &format!("bench-{round}"));
        cx.run_until_parked();
        let start = Instant::now();
        for chunk in &chunks {
            delta(&probe, &asked, chunk);
            // Each token is received, projected and parsed before the next, as when tokens arrive apart.
            cx.run_until_parked();
        }
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.;
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.entries.len(), HISTORY + 2);
            assert_eq!(workspace.entries.last().expect("answer").streamed, final_text);
            for entry in workspace.entries[..HISTORY].iter().filter(|entry| entry.id % 2 == 0) {
                assert_eq!(entry.streamed, stored_answer(entry.id));
            }
        });
        workspace.update_in(cx, |_, window, _| window.remove_window());
        cx.run_until_parked();
        if round > 0 {
            samples.push(elapsed_ms);
        }
    }
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    println!(
        "stream_history_benchmark={}",
        serde_json::json!({
            "history_messages": HISTORY,
            "deltas": DELTAS,
            "final_bytes": final_text.len(),
            "samples_ms": samples,
            "median_ms": sorted[sorted.len() / 2],
            "min_ms": sorted[0],
            "max_ms": sorted[sorted.len() - 1],
        })
    );
}
