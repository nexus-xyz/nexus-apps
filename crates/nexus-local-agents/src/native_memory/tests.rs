use std::path::PathBuf;
use std::process::Command;

use nexus_agent_memory::{Scope, Source};

use super::*;

#[test]
fn claude_project_slugs_match_the_installed_folder_format() {
    assert_eq!(
        crate::agents::claude_project_name(Path::new("/home/annie/acme/web-app")),
        "-home-annie-acme-web-app"
    );
    let long = format!("/home/annie/{}", "a".repeat(205));
    let expected = format!("-home-annie-{}-yemkaa", "a".repeat(188));
    assert_eq!(crate::agents::claude_project_name(Path::new(&long)), expected);
}

#[test]
fn claude_import_reads_worktrees_and_profiles_and_exports_to_the_primary_checkout() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let repo = repository(&root, "repo")?;
    let worktree = root.join("second checkout");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            worktree.to_str().expect("UTF-8 checkout"),
        ],
    );
    fs::write(
        repo.join("CLAUDE.md"),
        "Repository instructions already loaded by Claude",
    )?;

    let profiles = [root.join("primary"), root.join("extra")];
    // The fixture root varies. The known checkout slugs have literal suffixes.
    let prefix = root
        .to_string_lossy()
        .replace(|ch: char| !ch.is_ascii_alphanumeric(), "-");
    let fixture = |home: &Path, suffix: &str, text: &str| -> io::Result<PathBuf> {
        let folder = home.join("projects").join(format!("{prefix}{suffix}")).join("memory");
        fs::create_dir_all(&folder)?;
        fs::write(
            folder.join("note.md"),
            format!("---\nname: 'Imported note'\ndescription: >-\n  A note from\n  a Claude profile\n---\n{text}\n"),
        )?;
        Ok(folder)
    };
    fixture(&profiles[0], "-repo", "A note from the main checkout")?;
    fixture(&profiles[0], "-second-checkout", "A note from the second checkout")?;
    let selected = fixture(&profiles[1], "-repo", "A note from another account")?;
    fs::write(selected.join("MEMORY.md"), "A handwritten index note\n")?;

    let store = Store::new(root.join("store"));
    store.set_enabled(true)?;
    assert_eq!(import(&store, Agent::Claude, &profiles, &repo)?, 4);
    assert_eq!(import(&store, Agent::Claude, &profiles, &worktree)?, 0);
    let repository = Repository::discover(&worktree).expect("worktree repository");
    let imported = store.entries(Some(&repository))?;
    let note = imported
        .iter()
        .find(|entry| entry.text == "A note from the main checkout")
        .expect("imported note");
    assert_eq!(note.name.as_deref(), Some("Imported note"));
    assert_eq!(note.description.as_deref(), Some("A note from a Claude profile"));
    let entry = store.remember(repository.scope(), Source::Person, "An independent note")?;
    assert_eq!(export(&store, Agent::Claude, &profiles[1], &worktree)?, 1);
    let topic = selected.join(format!("nexus-shared-{}.md", entry.id));
    let index = fs::read_to_string(selected.join("MEMORY.md"))?;
    assert_eq!(
        index,
        format!(
            "A handwritten index note\n\n<!-- nexus-shared-memory:begin -->\n- [nexus-shared-{id}](nexus-shared-{id}.md) - An independent note\n<!-- nexus-shared-memory:end -->\n",
            id = entry.id,
        )
    );
    assert!(topic.is_file());
    assert_eq!(import(&store, Agent::Claude, &profiles, &repo)?, 0);
    let own = selected.join(format!("nexus-shared-{}.md", uuid::Uuid::new_v4()));
    fs::write(&own, "A person's file")?;
    assert!(store.forget(Some(&repository), entry.id)?);
    assert_eq!(export(&store, Agent::Claude, &profiles[1], &worktree)?, 0);
    assert!(!topic.exists());
    assert_eq!(fs::read_to_string(own)?, "A person's file");
    assert_eq!(
        fs::read_to_string(selected.join("MEMORY.md"))?,
        "A handwritten index note\n"
    );
    Ok(())
}

#[test]
fn claude_import_preserves_metadata_from_malformed_frontmatter() -> io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let checkout = repository(tmp.path(), "repo")?.canonicalize()?;
    let profile = tmp.path().join("profile");
    let folder = profile
        .join("projects")
        .join(crate::agents::claude_project_name(&checkout))
        .join("memory");
    fs::create_dir_all(&folder)?;
    let cases = [
        ("eod", r#""EOD memory entry": the user's personal EOD routine"#),
        ("commands", "`nexus memory list` shows local notes"),
        ("two-phrases", r#""One phrase": "a separate phrase""#),
    ];
    for (name, description) in cases {
        let header =
            format!("nested:\n  name: ignored\n  description: ignored\nname: '{name}'\ndescription: {description}");
        assert!(yaml_serde::from_str::<NoteMetadata>(&header).is_err());
        fs::write(
            folder.join(format!("{name}.md")),
            format!("---\n{header}\n---\nBody for {name}.\n"),
        )?;
    }

    let store = Store::new(tmp.path().join("store"));
    store.set_enabled(true)?;
    assert_eq!(import(&store, Agent::Claude, &[profile], &checkout)?, cases.len());
    let repo = Repository::discover(&checkout).expect("repository");
    let entries = store.entries(Some(&repo))?;
    for (name, description) in cases {
        let note = entries
            .iter()
            .find(|note| note.name.as_deref() == Some(name))
            .expect("top-level name");
        assert_eq!(note.description.as_deref(), Some(description));
        assert_eq!(note.text, format!("Body for {name}."));
    }
    Ok(())
}

#[test]
fn codex_import_reads_the_handbook_and_updates_profile_instructions() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let profile = dir.path().join("profile");
    fs::create_dir_all(profile.join("memories"))?;
    fs::create_dir_all(profile.join("memories_v2"))?;
    fs::write(profile.join("AGENTS.md"), "My profile instructions\n")?;
    fs::write(profile.join("memories/MEMORY.md"), "Old handbook\n")?;
    fs::write(profile.join("memories_v2/MEMORY.md"), "Current handbook\n")?;
    fs::write(profile.join("memories_v2/memory_summary.md"), "Generated summary\n")?;
    fs::write(dir.path().join("AGENTS.md"), "Repository instructions")?;
    let store = Store::new(dir.path().join("store"));
    store.set_enabled(true)?;
    let profiles = [&profile];
    assert_eq!(import(&store, Agent::Codex, &profiles, dir.path())?, 2);
    let entries = store.entries(None)?;
    assert!(entries.iter().any(|entry| entry.text == "Current handbook"));
    let original_id = entries
        .iter()
        .find(|entry| entry.text == "My profile instructions")
        .expect("profile instructions")
        .id;
    assert_eq!(import(&store, Agent::Codex, &profiles, dir.path())?, 0);
    fs::write(profile.join("AGENTS.md"), "Edited profile instructions\n")?;
    assert_eq!(import(&store, Agent::Codex, &profiles, dir.path())?, 1);
    let entries = store.entries(None)?;
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.id == original_id)
            .expect("updated entry")
            .text,
        "Edited profile instructions"
    );
    Ok(())
}

#[test]
fn codex_import_reads_ignored_project_notes_without_profile_instructions() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let checkout = repository(dir.path(), "repo")?;
    let repo = Repository::discover(&checkout).expect("repository");
    let folder = checkout.join(".codex/memory/project");
    fs::create_dir_all(&folder)?;
    fs::write(checkout.join(".gitignore"), ".codex/memory/\n")?;
    fs::write(folder.join("new note.md"), "---\nname: example\n---\nNew Codex note\n")?;
    fs::write(folder.join("duplicate.md"), "Existing Claude note")?;
    fs::write(folder.join("tracked.md"), "A tracked file is not a local note")?;
    git(&checkout, &["add", "-f", ".codex/memory/project/tracked.md"]);

    let store = Store::new(dir.path().join("store"));
    store.set_enabled(true)?;
    store.remember(repo.scope(), Source::Person, "Existing Claude note")?;
    let profile = dir.path().join("profile");
    let profiles = [&profile];
    assert_eq!(import(&store, Agent::Codex, &profiles, &checkout)?, 1);
    assert_eq!(import(&store, Agent::Codex, &profiles, &checkout)?, 0);
    let entries = store.entries(Some(&repo))?;
    assert!(store.entries(None)?.is_empty());
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|entry| entry.text == "New Codex note"));
    Ok(())
}

#[test]
fn codex_export_has_exact_content_and_preserves_other_repository_blocks() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let profile = dir.path().join("profile");
    fs::create_dir(&profile)?;
    let instructions = profile.join("AGENTS.md");
    fs::write(&instructions, "My profile instructions\n")?;
    let repo_a = repository(dir.path(), "a")?;
    let repo_b = repository(dir.path(), "b")?;
    let a = Repository::discover(&repo_a).expect("repository A");
    let b = Repository::discover(&repo_b).expect("repository B");
    let store = Store::new(dir.path().join("store"));
    store.set_enabled(true)?;
    assert_eq!(import(&store, Agent::Codex, &[&profile], &repo_a)?, 1);
    let personal = store.remember(Scope::Personal, Source::Person, "Shared note")?;
    let first = store.remember(a.scope(), Source::Person, "First repository")?;
    let second = store.remember(b.scope(), Source::Person, "Second repository")?;

    let personal_block = format!(
        "<!-- nexus-shared-memory:personal:begin -->\n## Personal memory ({})\n\nShared note\n\n<!-- nexus-shared-memory:personal:end -->\n",
        personal.id,
    );
    let repository_block = |repo: &Repository, entry: &nexus_agent_memory::Entry| {
        let key = repo.fingerprint();
        format!(
            "<!-- nexus-shared-memory:repository-{key}:begin -->\n## Repository memory ({})\n\n{}\n\n<!-- nexus-shared-memory:repository-{key}:end -->\n",
            entry.id, entry.text
        )
    };
    let a_block = repository_block(&a, &first);
    let b_block = repository_block(&b, &second);
    assert_eq!(export(&store, Agent::Codex, &profile, &repo_a)?, 2);
    assert_eq!(
        fs::read_to_string(&instructions)?,
        format!("My profile instructions\n\n{personal_block}\n{a_block}")
    );
    assert_eq!(export(&store, Agent::Codex, &profile, &repo_b)?, 2);
    assert_eq!(
        fs::read_to_string(&instructions)?,
        format!("My profile instructions\n\n{a_block}\n{personal_block}\n{b_block}")
    );
    assert!(store.forget(Some(&a), first.id)?);
    assert_eq!(export(&store, Agent::Codex, &profile, &repo_a)?, 1);
    assert_eq!(
        fs::read_to_string(&instructions)?,
        format!("My profile instructions\n\n{b_block}\n{personal_block}")
    );
    assert_eq!(import(&store, Agent::Codex, &[&profile], &repo_a)?, 0);
    let output = dir.path().join("export.md");
    assert_eq!(store.export_markdown(None, &output)?, 2);
    assert!(fs::read_to_string(output)?.contains("Shared note"));
    Ok(())
}

fn repository(root: &Path, name: &str) -> io::Result<PathBuf> {
    let path = root.join(name);
    fs::create_dir(&path)?;
    git(&path, &["init"]);
    git(&path, &["commit", "--allow-empty", "-m", "init"]);
    Ok(path)
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .expect("run git for the worktree fixture");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}
