//! Guard for AGENTS.md render rule "never an uncapped `repeat()`": a looping animation without
//! `with_max_fps` asks for a frame on every display refresh (60 to 120 a second) for as long as it is shown.

use std::path::Path;

/// Each `.repeat()` or `.repeat_synced()`, with its chain: from its `Animation::new` to the closure that draws it.
fn loops(source: &str) -> Vec<(usize, &str)> {
    let mut found = Vec::new();
    for pattern in [".repeat()", ".repeat_synced()"] {
        for (at, _) in source.match_indices(pattern) {
            let start = source[..at]
                .rfind("Animation::new")
                .filter(|s| at - s < 200)
                .unwrap_or(at);
            let end = draw_closure(&source[at..]).map_or(source.len(), |end| at + end);
            let line = source[..at].matches('\n').count() + 1;
            found.push((line, &source[start..end]));
        }
    }
    found
}

/// Where the closure after the animation starts: a `|` after `,` or `move`, not one inside `.with_easing(|t| ..)`.
/// At most 400 bytes on, cut on a char boundary.
fn draw_closure(rest: &str) -> Option<usize> {
    let limit = rest
        .char_indices()
        .map(|(i, _)| i)
        .find(|&i| i >= 400)
        .unwrap_or(rest.len());
    rest[..limit]
        .match_indices('|')
        .map(|(i, _)| i)
        .find(|&i| {
            let before = rest[..i].trim_end();
            before.ends_with(',') || before.ends_with("move")
        })
        .or(Some(limit))
}

fn uncapped(source: &str) -> Vec<usize> {
    loops(source)
        .into_iter()
        .filter(|(_, chain)| !chain.contains(".with_max_fps("))
        .map(|(line, _)| line)
        .collect()
}

fn visit(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("src is readable") {
        let path = entry.expect("src entry").path();
        if path.is_dir() {
            visit(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("motion/loops_capped.rs") {
            let source = std::fs::read_to_string(&path).expect("a source file is readable");
            for line in uncapped(&source) {
                out.push(format!("{}:{line}", path.strip_prefix(root).unwrap_or(&path).display()));
            }
        }
    }
}

#[test]
fn every_loop_has_a_frame_cap() {
    let mut found = Vec::new();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    visit(&src, &src, &mut found);
    assert!(
        found.is_empty(),
        "looping animations without `.with_max_fps(crate::motion::LOOP_FPS)`: {found:#?}"
    );
}

#[test]
fn the_check_finds_an_uncapped_loop() {
    let capped = "Animation::new(TURN)\n    .repeat()\n    .with_max_fps(LOOP_FPS),\n|icon, delta| icon";
    let bare = "Animation::new(TURN)\n    .repeat(),\n|icon, delta| icon";
    let text = r#"let s = "a".repeat(3);"#;
    assert_eq!(uncapped(capped), Vec::<usize>::new());
    assert_eq!(uncapped(bare), vec![2]);
    assert_eq!(uncapped(text), Vec::<usize>::new());
    let cap_first = "Animation::new(TURN).with_max_fps(LOOP_FPS).repeat(),\n|icon, delta| icon";
    let eased = "Animation::new(TURN)\n    .repeat()\n    .with_easing(|t| t * t)\n    .with_max_fps(LOOP_FPS),\nmove |icon, delta| icon";
    let wide = format!("Animation::new(TURN)\n    .repeat(){}", "é".repeat(300));
    assert_eq!(uncapped(cap_first), Vec::<usize>::new());
    assert_eq!(uncapped(eased), Vec::<usize>::new());
    assert_eq!(uncapped(&wide), vec![2]);
}
