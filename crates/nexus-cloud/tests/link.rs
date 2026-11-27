//! The provisioning step, run by `sh` on this computer in a throwaway home (`Target::Local`, the same script the app
//! runs over ssh), with stand-ins for `sudo`, `loginctl`, `systemctl`, `npm` and `gh`.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::{Scratch, calls, stubs};
use nexus_cloud::link::{CLAUDE_MD_BEGIN, CLAUDE_MD_END, PLAYWRIGHT_MCP, Provision, prepare, provision};
use nexus_remote::ssh::Target;
use serde_json::{Value, json};

fn local(home: &Path, bin: &Path) -> Target {
    Target::Local {
        env: vec![
            ("HOME".into(), home.display().to_string()),
            ("NEXUS_HOST_DIR".into(), home.join(".nexus/host").display().to_string()),
            ("XDG_CONFIG_HOME".into(), home.join(".config").display().to_string()),
            ("CODEX_HOME".into(), home.join(".codex").display().to_string()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("PATH".into(), format!("{}:/usr/bin:/bin", bin.display())),
        ],
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(0)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[tokio::test]
async fn prepares_the_machine_and_keeps_secrets_off_the_command_line() {
    let dir = Scratch::new("provision");
    let home = dir.path().join("home");
    let _ = std::fs::create_dir_all(&home);
    let bin = stubs(dir.path());
    let target = local(&home, &bin);
    // Claude's own settings are kept; ours are merged in.
    let _ = std::fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"computer":{}},"theme":"dark"}"#,
    );
    // The person's own CLAUDE.md is kept around the app's block.
    let claude_md = home.join(".claude/CLAUDE.md");
    let _ = std::fs::create_dir_all(home.join(".claude"));
    let mine = "# Mine\n\nAlways use tabs.\n";
    let _ = std::fs::write(&claude_md, mine);
    let env = "CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-secret\nOTHER=it's \"quoted\" $(not run)\n";
    let mcp = r#"{"mcpServers":{"browser":{"command":"npx","args":["@playwright/mcp@0.0.84"]}}}"#;
    let first = Provision {
        agent_env: Some(env.into()),
        mcp_json: Some(mcp.into()),
        git_name: Some("Zoë O'Neil".into()),
        git_email: Some("e@example.com".into()),
        github_token: Some("gho_secret".into()),
        claude_md: Some("# On a cloud machine\n\nSay the port.\n".into()),
        keep_agent_env: false,
    };
    let fields = provision(&target, &first).await.unwrap_or_else(|e| panic!("{e}"));
    let root = home.join(".nexus/host");
    let cloud = home.join(".nexus/cloud");
    assert_eq!(fields.get("provisioned").map(String::as_str), Some("yes"), "{fields:?}");
    assert_eq!(read(&root.join("agent.env")), env);
    assert_eq!(mode(&root.join("agent.env")), 0o600);
    assert_eq!(read(&root.join("mcp.json")), mcp);
    assert_eq!(mode(&root.join("mcp.json")), 0o600);
    assert_eq!(fields.get("playwright").map(String::as_str), Some("installed"));
    assert!(calls(dir.path()).contains(&format!("npm install -g {PLAYWRIGHT_MCP}")));
    let block = format!("{CLAUDE_MD_BEGIN}\n# On a cloud machine\n\nSay the port.\n{CLAUDE_MD_END}\n");
    assert_eq!(read(&claude_md), format!("{mine}\n{block}"));
    assert_eq!(fields.get("claudeMd").map(String::as_str), Some("written"));
    // Codex is told the same, in its own AGENTS.md.
    let agents_md = home.join(".codex/AGENTS.md");
    assert_eq!(read(&agents_md), block);
    assert_eq!(fields.get("codexMd").map(String::as_str), Some("written"));

    let git = read(&home.join(".gitconfig"));
    assert!(git.contains("Zoë O'Neil") && git.contains("e@example.com"), "{git}");
    assert!(calls(dir.path()).contains("sudo -n loginctl enable-linger"));

    let claude: Value = serde_json::from_str(&read(&home.join(".claude.json"))).unwrap_or_default();
    assert_eq!(
        claude,
        json!({ "mcpServers": { "computer": {} }, "theme": "dark", "hasCompletedOnboarding": true, "autoUpdates": false })
    );

    let pre_stop = home.join(".boat/pre-stop");
    assert_eq!(mode(&pre_stop), 0o700);
    assert!(read(&pre_stop).contains("\"$bin\" stop"));
    // The GitHub token reached `gh` on standard input only, and git uses it.
    assert!(calls(dir.path()).contains("gh auth login --hostname github.com --git-protocol https --with-token"));
    assert!(!calls(dir.path()).contains("gho_secret"), "no secret on a command line");
    assert_eq!(read(&dir.path().join("gh-token")), "gho_secret\n");
    assert!(calls(dir.path()).contains("gh auth setup-git"));
    assert_eq!(fields.get("github").map(String::as_str), Some("set"));
    // An older app's idle timer is gone.
    assert!(!cloud.join("stop-key.header").exists());
    assert!(!home.join(".config/systemd/user/nexus-cloud-idle.timer").exists());
    assert_eq!(fields.get("idle"), None);
    // The same token again: `gh` is not asked to sign in twice.
    let fields = ok_provision(&target, &first).await;
    assert_eq!(fields.get("github").map(String::as_str), Some("already"));
    assert_eq!(calls(dir.path()).matches("--with-token").count(), 1);

    // Another device without the Claude token keeps the one this device wrote.
    let keep = Provision {
        keep_agent_env: true,
        ..Provision::default()
    };
    let fields = ok_provision(&target, &keep).await;
    assert_eq!(fields.get("agent.env").map(String::as_str), Some("kept"));
    assert_eq!(read(&root.join("agent.env")), env);
    assert_eq!(mode(&root.join("agent.env")), 0o600);

    // Again with the same block, after the person wrote below it: one block, last, and their text kept.
    let _ = std::fs::write(&claude_md, format!("{mine}\n{block}\nMore of mine.\n\n"));
    let again = Provision {
        claude_md: first.claude_md.clone(),
        ..Provision::default()
    };
    ok_provision(&target, &again).await;
    let settled = format!("{mine}\nMore of mine.\n\n{block}");
    assert_eq!(read(&claude_md), settled);
    ok_provision(&target, &again).await;
    assert_eq!(read(&claude_md), settled, "a second connect changes nothing");

    // Again, with nothing: the files go, Claude's settings stay, an older app's timer goes, npm is not asked again.
    let units = home.join(".config/systemd/user");
    let _ = std::fs::create_dir_all(&units);
    let _ = std::fs::write(units.join("nexus-cloud-idle.timer"), "[Timer]\n");
    let _ = std::fs::write(cloud.join("stop-key.header"), "Authorization: Bearer old\n");
    let fields = provision(&target, &Provision::default())
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(fields.get("provisioned").map(String::as_str), Some("yes"));
    assert!(!root.join("agent.env").exists());
    assert!(!root.join("mcp.json").exists());
    assert!(!cloud.join("stop-key.header").exists());
    assert!(!units.join("nexus-cloud-idle.timer").exists());
    assert_eq!(fields.get("claude").map(String::as_str), Some("already"));
    assert_eq!(
        fields.get("github"),
        None,
        "no token: the machine's gh is left as it is"
    );
    assert!(calls(dir.path()).contains("systemctl --user disable --now nexus-cloud-idle.timer"));
    assert_eq!(calls(dir.path()).matches("npm install").count(), 1);
    assert_eq!(
        read(&claude_md),
        format!("{mine}\nMore of mine.\n"),
        "the block goes, the person's text stays"
    );
    assert_eq!(fields.get("claudeMd").map(String::as_str), Some("written"));
    assert!(!agents_md.exists(), "Codex's file held only the block");
    assert_eq!(fields.get("codexMd").map(String::as_str), Some("removed"));

    // A file that held only the block goes with it; one without a block is never touched.
    let _ = std::fs::write(&claude_md, &block);
    ok_provision(&target, &Provision::default()).await;
    assert!(!claude_md.exists());
    let _ = std::fs::write(&claude_md, "No block here.\n\n\n");
    let fields = ok_provision(&target, &Provision::default()).await;
    assert_eq!(fields.get("claudeMd").map(String::as_str), Some("none"));
    assert_eq!(read(&claude_md), "No block here.\n\n\n");
    // A begin line without its end is the person's text.
    let broken = format!("{CLAUDE_MD_BEGIN}\nhalf\n");
    let _ = std::fs::write(&claude_md, &broken);
    ok_provision(&target, &again).await;
    assert_eq!(read(&claude_md), format!("{broken}\n{block}"));
    ok_provision(&target, &again).await;
    assert_eq!(read(&claude_md), format!("{broken}\n{block}"));
}

async fn ok_provision(target: &Target, provision_with: &Provision) -> std::collections::HashMap<String, String> {
    let fields = provision(target, provision_with)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(fields.get("provisioned").map(String::as_str), Some("yes"), "{fields:?}");
    fields
}

fn block(text: &str) -> String {
    format!("{CLAUDE_MD_BEGIN}\n{text}{CLAUDE_MD_END}\n")
}

fn with_md(text: &str) -> Provision {
    Provision {
        claude_md: Some(text.into()),
        ..Provision::default()
    }
}

/// Two laptops connect to the same machine at once: the person's text survives every interleaving.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_connects_at_once_keep_the_persons_claude_md() {
    let dir = Scratch::new("md-race");
    let home = dir.path().join("home");
    let _ = std::fs::create_dir_all(home.join(".claude"));
    let bin = stubs(dir.path());
    let target = local(&home, &bin);
    let md = home.join(".claude/CLAUDE.md");
    let mine = "# Mine\n\nAlways use tabs.\n";
    let ours = "Say the port.\n";
    let (add, remove) = (with_md(ours), Provision::default());
    for round in 0..10 {
        let _ = std::fs::write(&md, format!("{mine}\n{}", block(ours)));
        let (a, b) = tokio::join!(provision(&target, &add), provision(&target, &add));
        assert!(a.is_ok() && b.is_ok(), "round {round}: {a:?} {b:?}");
        assert_eq!(read(&md), format!("{mine}\n{}", block(ours)), "round {round}");
        // One adds the block while the other removes it: either way, the person's text stays.
        let (a, b) = tokio::join!(provision(&target, &add), provision(&target, &remove));
        assert!(a.is_ok() && b.is_ok(), "round {round}: {a:?} {b:?}");
        assert!(read(&md).starts_with(mine), "round {round}: {:?}", read(&md));
    }
    let left: Vec<String> = std::fs::read_dir(home.join(".claude"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        left.iter().all(|f| !f.contains("nexus-") || f == ".nexus-md.lock"),
        "{left:?}"
    );
}

/// A CLAUDE.md kept in dotfiles and linked: the link stays, the target gets the block and keeps its mode.
#[tokio::test]
async fn a_symlinked_claude_md_is_written_through() {
    let dir = Scratch::new("md-link");
    let home = dir.path().join("home");
    let dotfiles = dir.path().join("dotfiles");
    let _ = std::fs::create_dir_all(home.join(".claude"));
    let _ = std::fs::create_dir_all(&dotfiles);
    let bin = stubs(dir.path());
    let target = local(&home, &bin);
    let real = dotfiles.join("claude.md");
    let link = home.join(".claude/CLAUDE.md");
    let _ = std::fs::write(&real, "Mine.\n");
    let _ = std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o644));
    let _ = std::os::unix::fs::symlink(&real, &link);
    let is_link = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());

    let fields = ok_provision(&target, &with_md("Say the port.\n")).await;
    assert_eq!(fields.get("claudeMd").map(String::as_str), Some("written"));
    assert!(is_link(&link), "the link was replaced by a file");
    assert_eq!(read(&real), format!("Mine.\n\n{}", block("Say the port.\n")));
    assert_eq!(mode(&real), 0o644);

    ok_provision(&target, &Provision::default()).await;
    assert!(is_link(&link));
    assert_eq!(read(&real), "Mine.\n");
    assert_eq!(mode(&real), 0o644);
}

/// Content that does not decode is a failure, never an empty block reported as written.
#[tokio::test]
async fn a_block_that_does_not_decode_writes_nothing() {
    let dir = Scratch::new("md-bad");
    let home = dir.path().join("home");
    let _ = std::fs::create_dir_all(home.join(".claude"));
    let bin = stubs(dir.path());
    let target = local(&home, &bin);
    let md = home.join(".claude/CLAUDE.md");
    let _ = std::fs::write(&md, "Mine.\n");
    let args = ["", "", PLAYWRIGHT_MCP, CLAUDE_MD_BEGIN, CLAUDE_MD_END];
    let output = target
        .run(
            prepare(),
            &args,
            Some(b"-\n-\n-\n%%%not base64%%%\n".to_vec()),
            std::time::Duration::from_secs(60),
        )
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(output.stdout.contains("claudeMd=failed"), "{}", output.stdout);
    assert!(output.stdout.contains("provisioned=yes"), "{}", output.stdout);
    assert_eq!(read(&md), "Mine.\n");
}
