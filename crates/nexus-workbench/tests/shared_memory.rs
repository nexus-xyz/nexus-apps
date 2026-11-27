mod support;

use nexus_agent_memory::{Scope, Source, Store};
use nexus_workbench::{Limits, driver, event::SessionAgent};
use support::{new_root, root, setup_with};

#[tokio::test]
async fn claude_reads_shared_memory_only_when_enabled() {
    launch_with_and_without_memory(SessionAgent::Claude).await;
}

#[tokio::test]
async fn codex_reads_shared_memory_only_when_enabled() {
    launch_with_and_without_memory(SessionAgent::Codex).await;
}

async fn launch_with_and_without_memory(agent: SessionAgent) {
    let s = setup_with(agent.as_str(), Limits::default(), true, |config| {
        let fixture = config
            .worktrees
            .as_deref()
            .and_then(|worktrees| worktrees.parent())
            .expect("fixture folder");
        nexus_workbench::Config {
            memory: Some(Store::new(fixture.join("shared-memory"))),
            instructions: Some(fixture.join("instructions")),
            ..config
        }
    });
    let memory = Store::new(s.root.join("shared-memory"));
    let capture = s.root.join("prompt");
    s.host
        .extra
        .lock()
        .expect("host environment")
        .insert("FAKE_PROMPT_CAPTURE".into(), capture.to_string_lossy().into_owned());
    let launch = || {
        let mut request = new_root(&s.project, "hello");
        request.agent = agent;
        root(&s, request)
    };
    memory.set_enabled(true).expect("enable");
    memory
        .remember(Scope::Personal, Source::Person, "shared memory launch fixture")
        .expect("remember");
    memory.set_enabled(false).expect("disable");
    launch().await;
    assert_eq!(
        std::fs::read_to_string(&capture).expect("captured prompt"),
        driver::instructions()
    );

    memory.set_enabled(true).expect("enable");
    launch().await;
    let prompt = std::fs::read_to_string(&capture).expect("captured prompt");
    assert!(prompt.starts_with(&driver::instructions()));
    let (instruction, notes) = prompt.split_once("<nexus-shared-memory>").expect("notes block");
    assert!(instruction.contains("When the person asks to remember something, save a memory, or add a memory entry"));
    assert!(instruction.contains("Use the `nexus` MCP server by default"));
    assert!(notes.contains("shared memory launch fixture"));
    assert!(!notes.contains("MCP server by default"));
    if agent == SessionAgent::Claude {
        assert!(instruction.contains("select:mcp__nexus__remember"));
        assert!(instruction.contains("select:mcp__nexus__recall"));
        assert!(instruction.contains("select:mcp__nexus__forget"));
    } else {
        assert!(!instruction.contains("ToolSearch"));
    }
    #[cfg(unix)]
    if agent == SessionAgent::Claude {
        use std::os::unix::fs::PermissionsExt;
        for file in std::fs::read_dir(memory.root().join("prompts")).expect("prompt files") {
            let mode = file
                .expect("prompt file")
                .metadata()
                .expect("prompt permissions")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
    memory.set_enabled(false).expect("disable");
    assert!(!memory.root().join("prompts").exists());
}
