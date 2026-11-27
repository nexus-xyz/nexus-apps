use std::process::{Command, Output};

use serde_json::Value;

#[test]
fn shared_memory_management_is_local_and_requires_opt_in() {
    let fixture = std::env::temp_dir().join(format!("nexus-memory-cli-{}", uuid::Uuid::new_v4()));
    let data = if cfg!(target_os = "macos") {
        fixture.join("Library/Application Support")
    } else {
        fixture.join("data")
    };
    let profile = fixture.join("codex");
    let extra = data.join("xyz.nexus.desktop/accounts/codex/a_1");
    for folder in [&profile, &extra] {
        std::fs::create_dir_all(folder).expect("profile folder");
    }
    std::fs::write(profile.join("AGENTS.md"), "Default Codex account note").expect("default profile note");
    std::fs::write(extra.join("AGENTS.md"), "Extra Codex account note").expect("extra profile note");

    let run = |words: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_nexus"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH with git"))
            .env("HOME", &fixture)
            .env("USERPROFILE", &fixture)
            .env("APPDATA", &data)
            .env("LOCALAPPDATA", &data)
            .env("XDG_DATA_HOME", &data)
            .env("CODEX_HOME", &profile)
            .current_dir(&fixture)
            .args(["memory", "--json"])
            .args(words)
            .output()
            .expect("run nexus memory")
    };
    assert_eq!(ok(run(&["status"]))["enabled"], false);
    assert!(!data.join("nexus/memory").exists());
    assert!(!run(&["import", "codex"]).status.success());
    assert_eq!(ok(run(&["enable"]))["enabled"], true);
    assert_eq!(ok(run(&["import", "codex"]))["imported"], 2);
    assert_eq!(ok(run(&["import", "codex"]))["imported"], 0);
    let entries = ok(run(&["list"]));
    let id = entries[0]["id"].as_str().expect("entry ID");
    assert_eq!(ok(run(&["edit", id, "Updated note"]))["edited"], true);
    assert_eq!(ok(run(&["list"]))[0]["text"], "Updated note");
    assert!(!run(&["disable"]).status.success());
    assert_eq!(ok(run(&["status"]))["enabled"], true);
    assert_eq!(ok(run(&["disable", "--export", "memory.md"]))["enabled"], false);
    let markdown = std::fs::read_to_string(fixture.join("memory.md")).expect("Markdown export");
    assert!(markdown.contains("Updated note"));
    assert_eq!(ok(run(&["enable"]))["enabled"], true);
    assert_eq!(ok(run(&["forget", id]))["deleted"], true);
    assert_eq!(ok(run(&["list"])).as_array().expect("entry list").len(), 1);
    let personal = ok(run(&["add", "Person wrote this"]));
    let store = data.join("nexus/memory");
    std::fs::write(store.join("personal/malformed.md"), "broken").expect("malformed fixture");
    let list = run(&["list"]);
    assert!(String::from_utf8_lossy(&list.stderr).contains("Skipped 1 unreadable shared memory file"));
    let entries = ok(list);
    assert!(
        entries
            .as_array()
            .expect("entry list")
            .iter()
            .any(|entry| entry["id"] == personal["id"] && entry["text"] == "Person wrote this")
    );
}

fn ok(output: Output) -> Value {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).expect("JSON from nexus memory")
}

#[test]
fn disable_catalog_describes_its_two_export_choices() {
    let catalog = nexus_cli::catalog::command_catalog(nexus_cli::SEMVER);
    let instructions = nexus_cli::catalog::find_command(
        catalog["commands"].as_array().expect("command list"),
        &["memory".into(), "disable".into()],
    )
    .expect("memory disable command");
    let find = |name: &str| {
        instructions["flags"]
            .as_array()
            .expect("disable flags")
            .iter()
            .find(|flag| flag["name"] == name)
            .expect("flag in the catalog")
    };
    let export = find("--export");
    let skip = find("--skip-export");
    assert_eq!(export["value"], "<path>");
    assert!(skip.get("value").is_none());
    assert!(export["description"].as_str().is_some_and(|text| !text.is_empty()));
    assert!(skip["description"].as_str().is_some_and(|text| !text.is_empty()));
}
