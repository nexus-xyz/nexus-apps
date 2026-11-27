use super::*;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nexus-transcripts-{name}-{}", crate::short_id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn names_a_folder_as_claude_code_does() {
    assert_eq!(folder_name(Path::new("/home/me/work/nexus")), "-home-me-work-nexus");
    assert_eq!(folder_name(Path::new("/Users/me/my repo.v2")), "-Users-me-my-repo-v2");
    assert_eq!(folder_name(Path::new(r"C:\Users\me\repo")), "C--Users-me-repo");
}

#[test]
fn a_long_folder_name_is_found_by_its_start_only_when_one_folder_has_it() {
    let root = temp("long");
    let cwd = root.join("x".repeat(220));
    std::fs::create_dir_all(&cwd).unwrap_or_else(|e| panic!("{e}"));
    let profile = root.join("profile");
    assert_eq!(expected(&profile, &cwd, "id"), None);
    let name = folder_name(&real(&cwd));
    let hashed = profile.join("projects").join(format!("{}-abc123", &name[..MAX_NAME]));
    std::fs::create_dir_all(&hashed).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(expected(&profile, &cwd, "id"), Some(hashed.join("id.jsonl")));
    std::fs::create_dir_all(profile.join("projects").join(format!("{}-zzz", &name[..MAX_NAME])))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(expected(&profile, &cwd, "id"), None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn lists_the_launch_profile_first_then_the_others_once() {
    let home = temp("home");
    for dir in [".claude", ".claude-work", ".config"] {
        std::fs::create_dir_all(home.join(dir)).unwrap_or_else(|e| panic!("{e}"));
    }
    write(&home.join(".claude.json"), "{}");
    let accounts = home.join("accounts");
    std::fs::create_dir_all(accounts.join("claude/a_1")).unwrap_or_else(|e| panic!("{e}"));
    let all = profiles(&home.join(".claude-work"), Some(&home), Some(&accounts));
    assert_eq!(
        all,
        vec![
            home.join(".claude-work"),
            home.join(".claude"),
            accounts.join("claude/a_1")
        ]
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn restores_the_fullest_transcript_found_and_leaves_one_in_place() {
    let root = temp("restore");
    let cwd = root.join("repo");
    std::fs::create_dir_all(&cwd).unwrap_or_else(|e| panic!("{e}"));
    let (a, b) = (root.join(".claude-a"), root.join(".claude-b"));
    let copies = root.join("copies");
    let want = expected(&b, &cwd, "s1").unwrap_or_else(|| panic!("a place"));
    // Under the other profile, in the folder of an old working folder; and a shorter copy of Nexus's.
    write(&a.join("projects/-old-place/s1.jsonl"), "1\n2\n3\n");
    write(&copy_path(&copies, "t_1", "s1"), "1\n2\n");
    let profiles = vec![b.clone(), a.clone()];
    let from = restore(&want, &profiles, Some(&copies), "s1").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(from, Some(a.join("projects/-old-place/s1.jsonl")));
    assert_eq!(std::fs::read_to_string(&want).ok().as_deref(), Some("1\n2\n3\n"));
    // There now: nothing more to do, and the original stays.
    assert_eq!(restore(&want, &profiles, Some(&copies), "s1").ok(), Some(None));
    assert!(a.join("projects/-old-place/s1.jsonl").is_file());
    // Found nowhere.
    let other = expected(&b, &cwd, "s2").unwrap_or_else(|| panic!("a place"));
    assert_eq!(restore(&other, &profiles, Some(&copies), "s2").ok(), Some(None));
    assert!(!other.exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn keeps_a_copy_only_when_the_transcript_changed_and_drops_older_conversations() {
    let root = temp("keep");
    let cwd = root.join("repo");
    std::fs::create_dir_all(&cwd).unwrap_or_else(|e| panic!("{e}"));
    let profile = root.join(".claude");
    let copies = root.join("copies");
    let source = expected(&profile, &cwd, "s2").unwrap_or_else(|| panic!("a place"));
    write(&source, "1\n");
    write(&copy_path(&copies, "t_1", "s1"), "old conversation\n");
    let copy = copy_path(&copies, "t_1", "s2");
    let profiles = vec![profile.clone()];
    assert_eq!(keep(&profiles, Some(&source), "s2", &copy).ok(), Some(true));
    assert_eq!(std::fs::read_to_string(&copy).ok().as_deref(), Some("1\n"));
    assert!(!copy_path(&copies, "t_1", "s1").exists());
    assert_eq!(keep(&profiles, Some(&source), "s2", &copy).ok(), Some(false));
    write(&source, "1\n2\n");
    assert_eq!(keep(&profiles, Some(&source), "s2", &copy).ok(), Some(true));
    assert_eq!(std::fs::read_to_string(&copy).ok().as_deref(), Some("1\n2\n"));
    // Not where it was expected: found in another project folder of the profile.
    std::fs::remove_file(&source).unwrap_or_else(|e| panic!("{e}"));
    write(&profile.join("projects/-moved/s2.jsonl"), "1\n2\n3\n");
    assert_eq!(keep(&profiles, Some(&source), "s2", &copy).ok(), Some(true));
    assert_eq!(std::fs::read_to_string(&copy).ok().as_deref(), Some("1\n2\n3\n"));
    forget(&copies, "t_1");
    assert!(!copies.join("t_1").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_id_that_is_not_plain_names_no_file_and_copies_nothing() {
    for id in ["", "../s1", "a/b", "..", "s1.jsonl", "s 1"] {
        assert!(!plain_id(id), "{id}");
    }
    assert!(plain_id("0b6f1d2e-7c1a-4a8e-9f00-1234567890ab"));
    let root = temp("plain");
    let cwd = root.join("repo");
    std::fs::create_dir_all(&cwd).unwrap_or_else(|e| panic!("{e}"));
    let profile = root.join(".claude");
    assert_eq!(expected(&profile, &cwd, "../s1"), None);
    let outside = root.join("outside.jsonl");
    write(&outside, "keep me\n");
    let copy = root.join("copies/t_1/x.jsonl");
    assert_eq!(
        keep(std::slice::from_ref(&profile), None, "../../outside", &copy).ok(),
        Some(false)
    );
    assert!(outside.exists());
    assert_eq!(restore(&copy, &[profile], None, "../../outside").ok(), Some(None));
    assert!(!copy.exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn a_kept_copy_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp("private");
    let source = root.join("s2.jsonl");
    write(&source, "1\n");
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap_or_else(|e| panic!("{e}"));
    let copy = copy_path(&root.join("copies"), "t_1", "s2");
    assert_eq!(keep(&[], Some(&source), "s2", &copy).ok(), Some(true));
    let mode = |path: &Path| {
        std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o777)
            .ok()
    };
    assert_eq!(mode(&copy), Some(0o600));
    assert_eq!(mode(copy.parent().unwrap_or(Path::new("."))), Some(0o700));
    let _ = std::fs::remove_dir_all(&root);
}
