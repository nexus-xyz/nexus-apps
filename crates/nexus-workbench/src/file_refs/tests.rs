use super::*;

fn file(path: &str, line: Option<u32>) -> Option<FileReference> {
    Some(FileReference {
        path: path.into(),
        line,
    })
}

#[test]
fn a_named_file_is_found_where_it_is_or_left_as_written() {
    struct Root(std::path::PathBuf);
    impl Root {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = Root(std::env::temp_dir().join(format!("nexus-locate-test-{}", uuid::Uuid::new_v4())));
    let at = |path: &str| {
        let file = root.path().join(path);
        std::fs::create_dir_all(file.parent().expect("a parent")).expect("make its folder");
        std::fs::write(file, "x").expect("write the file");
    };
    at("src/workspace/side_pane.rs");
    at("src/side/files.rs");
    at("crates/a/files.rs");
    at(".nexus/artifacts/demo.gif");
    at("README.md");
    let list = |_: &std::path::Path| {
        [
            "README.md",
            "crates/a/files.rs",
            "src/side/files.rs",
            "src/workspace/side_pane.rs",
        ]
        .map(String::from)
        .to_vec()
    };
    let locate = |path: &str| locate(root.path(), path, &[], list);
    // As written, when it is there; the list is not read.
    assert_eq!(
        super::locate(root.path(), "README.md", &[], |_| panic!("not listed")),
        Some("README.md".into())
    );
    // A name alone: under `.nexus/artifacts/`, else the one file that ends with it.
    assert_eq!(locate("demo.gif"), Some(".nexus/artifacts/demo.gif".into()));
    assert_eq!(locate("./demo.gif"), Some(".nexus/artifacts/demo.gif".into()));
    assert_eq!(locate("side_pane.rs"), Some("src/workspace/side_pane.rs".into()));
    assert_eq!(
        locate("workspace/side_pane.rs"),
        Some("src/workspace/side_pane.rs".into())
    );
    // Two files end with it, a name only part of one, an absolute path, or a climb: left as written.
    assert_eq!(locate("files.rs"), None);
    assert_eq!(locate("pane.rs"), None);
    assert_eq!(locate("/tmp/nowhere/demo.gif"), None);
    assert_eq!(locate("../demo.gif"), None);
    assert_eq!(locate("missing.png"), None);
}

#[test]
fn a_code_span_with_a_path_names_a_file() {
    assert_eq!(code_reference("src/main.ts:7"), file("src/main.ts", Some(7)));
    assert_eq!(code_reference("README.md"), file("README.md", None));
    assert_eq!(code_reference("Cargo.toml"), file("Cargo.toml", None));
    assert_eq!(code_reference("side_pane.rs:828"), file("side_pane.rs", Some(828)));
    assert_eq!(code_reference("/tmp/report.xlsx"), file("/tmp/report.xlsx", None));
    assert_eq!(code_reference("./my notes/a b.txt"), file("./my notes/a b.txt", None));
    assert_eq!(code_reference("src/main.rs#L12-L20"), file("src/main.rs", Some(12)));
}

#[test]
fn a_code_span_that_is_code_or_a_command_stays_code() {
    // The Nexus app's cases (`output-references.test.ts`).
    for text in ["npm run build", "sed -n file.ts", "v1.2"] {
        assert_eq!(code_reference(text), None, "{text}");
    }
    for text in [
        "self.files",
        "foo.bar()",
        "cargo test.rs",
        "python3 run.py",
        "a.rs; rm -rf x",
        "$HOME/a.rs",
        "x = y.rs",
        "my notes.txt",
        "https://example.com/a.js",
        "src/",
        ".gitignore",
        "e.g.",
        "a.verylongextension",
    ] {
        assert_eq!(code_reference(text), None, "{text}");
    }
}

#[test]
fn a_link_to_a_local_path_names_a_file_and_a_web_link_does_not() {
    assert_eq!(link_reference("../report.pdf"), file("../report.pdf", None));
    assert_eq!(link_reference("src/a.rs#L12"), file("src/a.rs", Some(12)));
    assert_eq!(link_reference("file:///tmp/a.png"), file("/tmp/a.png", None));
    assert_eq!(link_reference("https://example.com/a.rs"), None);
    assert_eq!(link_reference("mailto:a@example.com"), None);
}

#[test]
fn office_documents_open_in_their_app_on_this_computer_and_the_rest_in_files() {
    assert_eq!(opens("reports/q4.xlsx", true), Opens::App);
    assert_eq!(opens("a.DOCX", true), Opens::App);
    assert_eq!(opens("hero.avif", true), Opens::App);
    // A server's file stays in Files, which says why it has no preview.
    assert_eq!(opens("reports/q4.xlsx", false), Opens::Files);
    for path in ["a.rs", "a.md", "a.png", "a.pdf", "a.mp4", "a.html", "Makefile", "a.zip"] {
        assert_eq!(opens(path, true), Opens::Files, "{path}");
    }
}

#[test]
fn a_preview_shows_the_lines_around_the_named_line() {
    let text: String = (1..=20).map(|n| format!("l{n}\n")).collect();
    let lines = |from: u32, to: u32| (from..=to).map(|n| format!("l{n}")).collect::<Vec<_>>();
    assert_eq!(peek_lines(&text, Some(10)), (8, lines(8, 14)));
    assert_eq!(peek_lines(&text, None), (1, lines(1, 7)));
    assert_eq!(peek_lines(&text, Some(1)), (1, lines(1, 7)));
    // Near or past the end, the last lines.
    assert_eq!(peek_lines(&text, Some(20)), (14, lines(14, 20)));
    assert_eq!(peek_lines(&text, Some(99)), (14, lines(14, 20)));
    assert_eq!(peek_lines("", Some(3)), (1, Vec::<String>::new()));
    assert_eq!(peek_lines("a\nb", Some(2)), (1, vec!["a".to_string(), "b".to_string()]));
}

#[test]
fn a_preview_line_is_cut_and_its_tabs_are_spaces() {
    let long = "é".repeat(PEEK_WIDTH + 10);
    let (_, lines) = peek_lines(&format!("\tx\n{long}"), None);
    assert_eq!(lines[0], "    x");
    assert_eq!(lines[1], format!("{}…", "é".repeat(PEEK_WIDTH)));
}

#[test]
fn a_chip_says_the_file_name_and_its_line() {
    assert_eq!(
        chip_label(&FileReference {
            path: "src/workspace/side_pane.rs".into(),
            line: Some(828)
        }),
        "side_pane.rs:828"
    );
    assert_eq!(
        chip_label(&FileReference {
            path: "README.md".into(),
            line: None
        }),
        "README.md"
    );
    assert_eq!(file_name("src/side/"), "side");
    assert_eq!(file_name("C:/work/a.png"), "a.png");
}
