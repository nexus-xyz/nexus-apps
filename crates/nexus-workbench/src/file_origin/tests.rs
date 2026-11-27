use serde_json::json;

use super::*;

#[test]
fn a_file_tool_names_its_absolute_path() {
    assert_eq!(touched_paths(&json!({"file_path": "/w/b/src/a.rs"})), ["/w/b/src/a.rs"]);
    assert_eq!(
        touched_paths(&json!({"notebook_path": "/w/b/n.ipynb"})),
        ["/w/b/n.ipynb"]
    );
    assert_eq!(touched_paths(&json!({"pattern": "x", "path": "/w/b"})), ["/w/b"]);
    // A relative path has no folder to read it from.
    assert!(touched_paths(&json!({"file_path": "src/a.rs"})).is_empty());
    assert!(touched_paths(&json!({"file_path": 3})).is_empty());
}

#[test]
fn a_command_names_each_absolute_path_in_it() {
    let input = json!({"command": "cd /w/b && sed -n '1,80p' '/w/b/src/a.rs' | rg x \"src\" /w/b/e.rs:12,"});
    assert_eq!(touched_paths(&input), ["/w/b", "/w/b/src/a.rs", "/w/b/e.rs"]);
    for command in ["ls src", "rg -n x //host/share", "cat /w/../etc/passwd", "echo /"] {
        assert!(touched_paths(&json!({ "command": command })).is_empty(), "{command}");
    }
}

#[test]
fn the_other_checkouts_come_newest_first_without_the_session_s_own() {
    let dir = std::env::temp_dir().join(format!("nexus-origin-test-{}", uuid::Uuid::new_v4()));
    for tree in ["a", "b", "c", "plain"] {
        std::fs::create_dir_all(dir.join(tree).join("src")).expect("make the folder");
        std::fs::write(dir.join(tree).join("src/x.rs"), "x").expect("write a file");
        if tree != "plain" {
            std::fs::write(dir.join(tree).join(".git"), "gitdir: /r").expect("mark the worktree");
        }
    }
    let dir = dir.canonicalize().expect("the folder");
    let path = |tree: &str| dir.join(tree).join("src/x.rs").to_string_lossy().into_owned();
    let touched = [
        path("c"),
        path("a"),
        path("plain"),
        path("b"),
        path("c"),
        "/nowhere/x.rs".into(),
    ];
    assert_eq!(roots(&dir.join("a"), &touched), [dir.join("c"), dir.join("b")]);
    // A session in a subfolder of a checkout never lists that checkout.
    assert_eq!(roots(&dir.join("a/src"), &touched), [dir.join("c"), dir.join("b")]);
    std::fs::remove_dir_all(&dir).expect("clean up");
}
