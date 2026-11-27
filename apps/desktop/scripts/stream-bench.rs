//! Standalone benchmark of the production stream reducer, without GPUI or CEF.
//!
//! From desktop-gpui: rustc --edition=2024 -O scripts/stream-bench.rs -o /tmp/nexus-stream-bench
//! Run the binary with an optional repeat count (default 5). JSON lines report ingestion only.
//! To compare a revision, copy this file and that revision's src/stream.rs into the same directory layout.

#![allow(dead_code)]

extern crate self as nexus_workbench;

// Only the event shapes consumed by stream.rs. The measured reducer is the production source below.
pub mod event {
    #[derive(Clone)]
    pub struct Delta {
        pub thread_id: String,
        pub id: String,
        pub parent: Option<String>,
        pub text: String,
        pub offset: Option<u64>,
    }

    #[derive(PartialEq)]
    pub enum TaskStatus {
        Running,
        Completed,
    }

    pub enum WbEvent {
        Text { id: String, parent: Option<String> },
        TurnEnd {},
        Task { status: TaskStatus, tool_use_id: Option<String> },
        Session {},
        Error {},
        Other,
    }
}

#[path = "../src/stream.rs"]
mod stream;

use std::hint::black_box;
use std::time::Instant;

fn main() {
    let repeats = std::env::args()
        .nth(1)
        .map(|value| value.parse::<usize>().expect("repeat count"))
        .unwrap_or(5);
    assert!(repeats > 0);
    for (label, chunk) in [("ascii", "A short streaming response. 12345"), ("unicode", "A🦀e\u{301} 中文 streaming response. ")] {
        for (chunks, sessions) in [(1024, 1), (8192, 1), (32768, 1), (8192, 8)] {
            let mut deltas = Vec::with_capacity(chunks * sessions);
            let units = chunk.encode_utf16().count() as u64;
            for part in 0..chunks {
                for session in 0..sessions {
                    deltas.push(event::Delta {
                        thread_id: format!("session-{session}"),
                        id: "message".into(),
                        parent: None,
                        text: chunk.into(),
                        offset: Some(part as u64 * units),
                    });
                }
            }
            let expected = chunk.repeat(chunks);
            // Warm allocations and the reducer before collecting samples.
            let mut warm = stream::Streams::default();
            for delta in &deltas {
                black_box(warm.delta(black_box(delta)));
            }
            assert_eq!(warm.text("session-0").as_deref(), Some(expected.as_str()));
            drop(warm);
            for sample in 0..repeats {
                let mut streams = stream::Streams::default();
                let start = Instant::now();
                for delta in &deltas {
                    black_box(streams.delta(black_box(delta)));
                }
                let elapsed = start.elapsed();
                for session in 0..sessions {
                    assert_eq!(streams.text(&format!("session-{session}")).as_deref(), Some(expected.as_str()));
                }
                println!(
                    "{{\"encoding\":\"{label}\",\"chunks\":{chunks},\"sessions\":{sessions},\"bytes_per_session\":{},\"deltas\":{},\"sample\":{sample},\"ingest_ms\":{:.6}}}",
                    expected.len(),
                    deltas.len(),
                    elapsed.as_secs_f64() * 1000.0
                );
            }
        }
    }
}
