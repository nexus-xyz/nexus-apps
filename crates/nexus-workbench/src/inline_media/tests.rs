//! The Nexus app's `inline-media.test.ts` and the `fileReference` cases it relies on, ported.
use std::cell::RefCell;

use super::*;

fn chunk(path: &str, size: u64, offset: u64, bytes: &[u8], status: &str) -> MediaChunk {
    MediaChunk {
        path: path.into(),
        media_type: if path.ends_with(".png") {
            "image/png"
        } else {
            "video/webm"
        }
        .into(),
        size,
        offset,
        data: (status == "ready").then(|| STANDARD.encode(bytes)),
        status: status.into(),
    }
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    futures::executor::block_on(future)
}

#[test]
fn names_media_by_extension() {
    for (path, kind) in [
        (".nexus/artifacts/login.png", Some(MediaKind::Image)),
        ("shot.JPG", Some(MediaKind::Image)),
        ("a.jpeg", Some(MediaKind::Image)),
        ("a.gif", Some(MediaKind::Image)),
        ("a.webp", Some(MediaKind::Image)),
        (".nexus/artifacts/flow.webm", Some(MediaKind::Video)),
        ("clip.mp4", Some(MediaKind::Video)),
        ("clip.MOV", Some(MediaKind::Video)),
        ("diagram.svg", None),
        ("notes.md", None),
        ("video.webm.txt", None),
    ] {
        assert_eq!(media_kind(path), kind, "{path}");
    }
}

#[test]
fn shows_links_and_images_and_leaves_code_web_links_and_line_links() {
    let image = |path: &str| Some((path.to_string(), MediaKind::Image));
    let video = |path: &str| Some((path.to_string(), MediaKind::Video));
    assert_eq!(
        media_reference(".nexus/artifacts/login.png", Reference::Image),
        image(".nexus/artifacts/login.png")
    );
    assert_eq!(
        media_reference("./.nexus/artifacts/flow.webm", Reference::Link),
        video("./.nexus/artifacts/flow.webm")
    );
    assert_eq!(
        media_reference("file:///home/user/app/.nexus/artifacts/flow.mp4", Reference::Link),
        video("/home/user/app/.nexus/artifacts/flow.mp4")
    );
    assert_eq!(media_reference(".nexus/artifacts/login.png", Reference::Code), None);
    assert_eq!(media_reference("https://example.com/flow.mp4", Reference::Link), None);
    assert_eq!(media_reference("data:image/png;base64,AA==", Reference::Image), None);
    assert_eq!(media_reference("notes.md", Reference::Link), None);
    assert_eq!(media_reference("shot.png#L3", Reference::Link), None);
}

#[test]
fn file_links_skip_examples_in_code_and_web_links() {
    let text = "Saved [the flow](.nexus/artifacts/flow.webm) and ![shot](shot.png).\n\
                Link it as `![Login page](.nexus/artifacts/login.png)`.\n\
                ```md\n[Login flow](.nexus/artifacts/login.webm)\n```\n\
                See [docs](https://example.com/a.md) and [line](src/a.rs:19).";
    let paths: Vec<(String, Option<u32>)> = file_links(text)
        .into_iter()
        .map(|file| (file.path, file.line))
        .collect();
    assert_eq!(
        paths,
        [
            (".nexus/artifacts/flow.webm".to_string(), None),
            ("shot.png".to_string(), None),
            ("src/a.rs".to_string(), Some(19)),
        ]
    );
}

#[test]
fn a_short_link_with_a_wide_character_is_no_file_url() {
    // `…` spans bytes 4..7: slicing the first 5 bytes panicked at startup.
    assert_eq!(
        file_reference("src/…"),
        Some(FileReference {
            path: "src/…".into(),
            line: None,
        })
    );
    assert_eq!(
        file_reference("ab…"),
        Some(FileReference {
            path: "ab…".into(),
            line: None
        })
    );
}

#[test]
fn reads_file_references_as_the_nexus_app_does() {
    let file = |path: &str, line: Option<u32>| {
        Some(FileReference {
            path: path.into(),
            line,
        })
    };
    assert_eq!(file_reference("src/main.rs:12"), file("src/main.rs", Some(12)));
    assert_eq!(file_reference("src/main.rs:12:4"), file("src/main.rs", Some(12)));
    assert_eq!(file_reference("src/main.rs#L12-L20"), file("src/main.rs", Some(12)));
    assert_eq!(file_reference("src/main.rs#intro"), file("src/main.rs", None));
    assert_eq!(file_reference("a%20b.png"), file("a b.png", None));
    assert_eq!(file_reference("C:\\work\\shot.png"), file("C:/work/shot.png", None));
    assert_eq!(
        file_reference("file:///C:/work/shot.png"),
        file("C:/work/shot.png", None)
    );
    assert_eq!(file_reference("file://localhost/tmp/a.png"), file("/tmp/a.png", None));
    assert_eq!(file_reference("sandbox:/mnt/data/a.png"), file("/mnt/data/a.png", None));
    assert_eq!(file_reference("file://other/tmp/a.png"), None);
    assert_eq!(file_reference("mailto:someone@example.com"), None);
    assert_eq!(file_reference("#heading"), None);
    assert_eq!(file_reference("//host/share/a.png"), None);
    assert_eq!(file_reference("a%ZZ.png"), None);
    assert_eq!(file_reference("a.png?x=1"), None);
}

#[test]
fn finds_media_links_outside_code_in_order_once() {
    let markdown = "Here is the page:\n\n![Login page](.nexus/artifacts/login.png)\n\n\
        And [the flow](<.nexus/artifacts/flow one.webm> \"Flow\"), again [here](.nexus/artifacts/login.png).\n\n\
        Not `![code](.nexus/artifacts/code.png)` nor [a doc](notes.md) nor [web](https://x.dev/a.png).\n\n\
        ```md\n![fenced](.nexus/artifacts/fenced.png)\n```\n\n[![thumb](a/thumb.png)](a/full.png)\n";
    let links = media_links(markdown);
    let paths: Vec<_> = links
        .iter()
        .map(|link| (link.label.as_str(), link.path.as_str(), link.kind))
        .collect();
    assert_eq!(
        paths,
        vec![
            ("Login page", ".nexus/artifacts/login.png", MediaKind::Image),
            ("the flow", ".nexus/artifacts/flow one.webm", MediaKind::Video),
            ("thumb", "a/thumb.png", MediaKind::Image),
            ("![thumb](a/thumb.png)", "a/full.png", MediaKind::Image),
        ]
    );
    assert!(
        media_links("```\n![open](a.png)\n").is_empty(),
        "an unclosed fence is code to the end"
    );
    assert!(media_links("\\[not](a.png)").is_empty(), "an escaped bracket is text");
}

#[test]
fn joins_the_pieces_into_one_file() {
    let offsets = RefCell::new(Vec::new());
    let loaded = block_on(read_all("flow.webm", |offset| {
        offsets.borrow_mut().push(offset);
        async move {
            Ok(if offset == 0 {
                chunk("flow.webm", 5, 0, &[1, 2, 3], "ready")
            } else {
                chunk("flow.webm", 5, 3, &[4, 5], "ready")
            })
        }
    }));
    assert_eq!(
        loaded,
        Ok(Loaded::Ready {
            bytes: vec![1, 2, 3, 4, 5],
            media_type: "video/webm".into(),
            size: 5
        })
    );
    assert_eq!(*offsets.borrow(), vec![0, 3]);
}

#[test]
fn stops_at_the_cap_of_its_kind() {
    let over = block_on(read_all("big.mp4", |_| async {
        Ok(chunk("big.mp4", MAX_VIDEO + 1, 0, &[], "tooLarge"))
    }));
    assert_eq!(over, Ok(Loaded::TooLarge { size: MAX_VIDEO + 1 }));
    // An image over 10 MB stays a card even if a machine sends it.
    let image = block_on(read_all("big.png", |_| async {
        Ok(chunk("big.png", MAX_IMAGE + 1, 0, &[1], "ready"))
    }));
    assert_eq!(image, Ok(Loaded::TooLarge { size: MAX_IMAGE + 1 }));
}

#[test]
fn fails_when_the_file_changes_while_it_loads() {
    let changed = block_on(read_all("flow.webm", |offset| async move {
        Ok(if offset == 0 {
            chunk("flow.webm", 5, 0, &[1, 2, 3], "ready")
        } else {
            chunk("flow.webm", 9, 3, &[4, 5], "ready")
        })
    }));
    assert!(
        changed
            .expect_err("a changed size fails")
            .contains("changed while it loaded")
    );
    let short = block_on(read_all("flow.webm", |offset| async move {
        Ok(if offset == 0 {
            chunk("flow.webm", 5, 0, &[1, 2, 3], "ready")
        } else {
            chunk("flow.webm", 5, 3, &[], "ready")
        })
    }));
    assert!(short.expect_err("an early end fails").contains("ended early"));
}

#[test]
fn passes_the_machines_refusal_on() {
    let refused = block_on(read_all("../secret.png", |_| async {
        Err::<MediaChunk, _>("Only files inside the project show inline.".to_string())
    }));
    assert!(refused.expect_err("refused").contains("inside the project"));
}

#[test]
fn says_sizes_and_caps_as_the_card_does() {
    assert_eq!(size_label(40_000), "39 KB");
    assert_eq!(size_label(10), "1 KB");
    assert_eq!(size_label(2_400_000), "2.3 MB");
    assert_eq!(
        too_large(MediaKind::Video, 72 * 1024 * 1024),
        "This video is 72.0 MB. The reply shows videos up to 50 MB."
    );
}
