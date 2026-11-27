//! The CLI with no Nexus account and `NEXUS_BYO_PROVIDERS` on: the coding agents run on the person's own plan or key,
//! with nothing sent to Gateway; every command that needs an account says how to sign in or create one; chat says it
//! needs an account or an agent. With the switch off, every command says what it said before.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::Value;

const SWITCH: (&str, &str) = ("NEXUS_BYO_PROVIDERS", "on");

struct Run {
    code: i32,
    out: String,
    err: String,
    /// Connections the stand-in Gateway accepted: none may be made without an account.
    gateway_calls: usize,
    /// What the stand-in agent wrote, if it ran.
    agent: Option<String>,
}

/// A port that counts every connection made to it, and answers none.
fn counting_gateway() -> (String, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    (url, calls)
}

/// A stand-in agent in `dir`: writes its arguments and two variables to `report`, then exits 3.
fn stand_in(dir: &Path, name: &str, report: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join(name);
        let script = format!(
            "#!/bin/sh\n{{ echo \"ARGS: $*\"; echo \"KEY: $ANTHROPIC_API_KEY\"; echo \"NEXUS_URL: $NEXUS_URL\"; }} > '{}'\nexit 3\n",
            report.display()
        );
        std::fs::write(&bin, script).expect("stand-in");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        bin
    }
    #[cfg(windows)]
    {
        let bin = dir.join(format!("{name}.cmd"));
        let report = report.display();
        let script = format!(
            "@echo off\r\n> \"{report}\" echo ARGS: %*\r\n>> \"{report}\" echo KEY: %ANTHROPIC_API_KEY%\r\n>> \"{report}\" echo NEXUS_URL: %NEXUS_URL%\r\nexit /b 3\r\n"
        );
        std::fs::write(&bin, script).expect("stand-in");
        bin
    }
}

/// `nexus <argv>` with no sign-in: no config file, Gateway at a counting port unless `env` names another.
fn run(argv: &[&str], env: &[(&str, &str)]) -> Run {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "nexus-no-account-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let (url, calls) = counting_gateway();
    let report = dir.join("agent-report.txt");
    let mut vars: HashMap<String, String> = HashMap::from([
        ("NEXUS_URL".into(), url),
        (
            "NEXUS_CLAUDE_BIN".into(),
            stand_in(&dir, "claude", &report).display().to_string(),
        ),
        (
            "NEXUS_CODEX_BIN".into(),
            stand_in(&dir, "codex", &report).display().to_string(),
        ),
        ("ANTHROPIC_API_KEY".into(), "sk-own-key".into()),
    ]);
    for (name, value) in env {
        if value.is_empty() {
            vars.remove(*name);
        } else {
            vars.insert((*name).into(), (*value).into());
        }
    }
    let capture = Capture(std::rc::Rc::new(Captured::default()));
    let ctx = Context {
        config_path: dir.join("config.json"),
        io: Box::new(capture.clone()),
        color: false,
        env: vars,
        poll_ms: Some(0),
        hostname: None,
        cwd: dir.clone(),
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    };
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    let agent = std::fs::read_to_string(&report).ok();
    let _ = std::fs::remove_dir_all(&dir);
    Run {
        code,
        out: capture.0.out.borrow().join("\n"),
        err: capture.0.err.borrow().join("\n"),
        gateway_calls: calls.load(Ordering::SeqCst),
        agent,
    }
}

/// The commands that need an account, each with the words its message names.
const ACCOUNT_ONLY: &[(&[&str], &str)] = &[
    (&["spend"], "`nexus spend`"),
    (&["models"], "`nexus models`"),
    (&["keys"], "`nexus keys`"),
    (&["team"], "`nexus team`"),
    (&["analytics"], "`nexus analytics`"),
    (&["budget"], "`nexus budget`"),
    (&["budgets"], "`nexus budgets`"),
    (&["billing"], "`nexus billing`"),
    (&["artifacts"], "`nexus artifacts`"),
    (&["pages"], "`nexus pages`"),
    (&["design-system"], "`nexus design-system`"),
    (&["skills"], "`nexus skills`"),
    (&["org"], "`nexus org`"),
    (&["policy"], "`nexus policy`"),
    (&["members"], "`nexus members`"),
    (&["audit"], "`nexus audit`"),
    (&["search", "notes"], "`nexus search`"),
    (&["service-accounts"], "`nexus service-accounts`"),
    (&["slack"], "`nexus slack`"),
    (&["modules"], "`nexus modules`"),
    (&["connectors"], "`nexus connectors`"),
    (&["connections"], "`nexus connections`"),
    (&["connect", "notion"], "`nexus connect`"),
    (&["machines"], "`nexus machines`"),
    (&["notifications"], "`nexus notifications`"),
    (&["jobs"], "`nexus jobs`"),
    (&["routines"], "`nexus routines`"),
    (&["apps"], "`nexus apps`"),
    (&["devices"], "`nexus devices`"),
    (&["mcp"], "`nexus mcp`"),
    (&["auth", "tokens"], "`nexus auth tokens`"),
    (&["agents", "share"], "`nexus agents share`"),
];

#[test]
fn switch_off_keeps_todays_sign_in_messages() {
    let spend = run(&["spend"], &[]);
    assert_eq!(spend.code, 1);
    assert!(spend.err.contains("You are not logged in."), "{}", spend.err);
    let chat = run(&["chat", "hi"], &[]);
    assert!(chat.err.contains("You are not logged in."), "{}", chat.err);
    let claude = run(&["claude", "-p", "hi"], &[]);
    assert_eq!(claude.code, 1);
    assert!(claude.err.contains("You are not signed in to Nexus."), "{}", claude.err);
    assert!(claude.agent.is_none(), "the agent started");
    let saved = run(&["agents", "use", "claude", "subscription"], &[]);
    assert!(saved.err.contains("You are not signed in to Nexus."), "{}", saved.err);
    let agents = run(&["agents"], &[]);
    assert!(agents.out.contains("Signed out: this machine only."), "{}", agents.out);
    assert!(
        agents.out.contains("Sign in to Nexus: nexus auth login"),
        "{}",
        agents.out
    );
}

#[test]
fn signed_out_an_agent_runs_on_its_own_sign_in_and_nothing_reaches_gateway() {
    for (argv, args) in [
        (&["claude", "-p", "hi"][..], "ARGS: -p hi"),
        (&["claude", "--subscription", "-p", "hi"][..], "ARGS: -p hi"),
        (&["codex", "exec", "hi"][..], "ARGS: exec hi"),
    ] {
        let ran = run(argv, &[SWITCH]);
        // The agent's own exit code.
        assert_eq!(ran.code, 3, "{argv:?}: {}", ran.err);
        let report = ran.agent.expect("the agent ran");
        assert!(report.contains(args), "{argv:?}: {report}");
        // The person's own key reaches the agent; the Gateway the CLI came from does not.
        assert!(report.contains("KEY: sk-own-key"), "{report}");
        assert!(!report.contains("127.0.0.1"), "{report}");
        assert!(
            ran.err.contains("not signed in to Nexus, so nothing is recorded"),
            "{}",
            ran.err
        );
        assert_eq!(ran.gateway_calls, 0, "{argv:?}");
    }
    let json = run(&["--json", "claude", "-p", "hi"], &[SWITCH]);
    let summary: Value = serde_json::from_str(&json.out).expect("json");
    assert_eq!(summary["signedIn"], false);
    assert_eq!(summary["exitCode"], 3);
    assert_eq!(summary["route"]["route"], "subscription");
}

#[test]
fn signed_out_gateway_and_auto_need_an_account() {
    for (argv, env) in [
        (&["claude", "--api", "-p", "hi"][..], None),
        (&["claude", "-p", "hi"][..], Some(("NEXUS_CLAUDE_AUTH", "gateway"))),
        (&["claude", "--model", "auto"][..], None),
    ] {
        let mut vars = vec![SWITCH];
        vars.extend(env);
        let refused = run(argv, &vars);
        assert_eq!(refused.code, 1, "{argv:?}");
        assert!(
            refused.err.contains("needs a Nexus account."),
            "{argv:?}: {}",
            refused.err
        );
        assert!(refused.err.contains("nexus auth login"), "{}", refused.err);
        assert!(refused.agent.is_none(), "{argv:?}: the agent started");
        assert_eq!(refused.gateway_calls, 0);
    }
}

#[test]
fn every_account_only_command_says_how_to_sign_in_or_create_an_account() {
    for (argv, named) in ACCOUNT_ONLY {
        let refused = run(argv, &[SWITCH]);
        assert_eq!(refused.code, 1, "{argv:?}: {}", refused.err);
        assert!(
            refused.err.contains(&format!("{named} needs a Nexus account.")),
            "{argv:?}: {}",
            refused.err
        );
        assert!(
            refused
                .err
                .contains("Sign in with `nexus auth login`, or create an account at http://127.0.0.1:")
        );
        assert_eq!(refused.gateway_calls, 0, "{argv:?}");
        assert!(refused.out.is_empty(), "{argv:?}: {}", refused.out);
    }
    let json = run(&["spend", "--json"], &[SWITCH]);
    let error: Value = serde_json::from_str(&json.err).expect("json");
    assert_eq!(error["error"]["code"], "account_required");
    // Production's Gateway: the link is Nexus UI's.
    let production = run(&["spend"], &[SWITCH, ("NEXUS_URL", "")]);
    assert!(
        production.err.contains("create an account at https://nexus.inc/app."),
        "{}",
        production.err
    );
}

#[test]
fn chat_says_it_needs_an_account_or_an_agent() {
    for argv in [&["chat", "hi"][..], &["agent", "ask", "hi"][..]] {
        let refused = run(argv, &[SWITCH]);
        assert_eq!(refused.code, 1);
        assert!(
            refused
                .err
                .contains("Chat needs a Nexus account, or a coding agent on your own plan or key."),
            "{}",
            refused.err
        );
        assert!(
            refused.err.contains("`nexus agents` shows what this computer has"),
            "{}",
            refused.err
        );
        assert_eq!(refused.gateway_calls, 0);
    }
}

#[test]
fn signed_out_agents_lists_no_nexus_sign_in_step_and_use_needs_an_account() {
    let listed = run(&["agents", "--json"], &[SWITCH]);
    assert_eq!(listed.code, 0, "{}", listed.err);
    let answer: Value = serde_json::from_str(&listed.out).expect("json");
    assert_eq!(answer["signedIn"], false);
    let claude = answer["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .find(|agent| agent["agent"] == "claude")
        .expect("claude");
    let steps: Vec<&str> = claude["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .filter_map(|step| step["id"].as_str())
        .collect();
    assert!(!steps.contains(&"nexus_login"), "{steps:?}");
    assert_eq!(listed.gateway_calls, 0);

    let saved = run(&["agents", "use", "claude", "subscription"], &[SWITCH]);
    assert_eq!(saved.code, 1);
    assert!(
        saved.err.contains("Saving a route needs a Nexus account."),
        "{}",
        saved.err
    );
    assert_eq!(saved.gateway_calls, 0);
}
