//! The parser on the transcripts recorded on 2026-11-17, and the runner on fake logins: shell scripts that print what
//! the real ones print. No network, and no real configuration folder.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use nexus_workbench::event::SessionAgent;

use super::{Event, Exit, Mode, Options, Parser, Runner, args, port_error, strip_ansi, vetted};

const CLAUDE_LINK: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c&response_type=code\
                           &redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&state=s1";
const CODEX_LINK: &str = "https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_EMo\
                          &redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&state=s2";

fn claude_transcript() -> String {
    format!(
        "Opening browser to sign in…\nIf the browser didn't open, visit: {CLAUDE_LINK}\nPaste code here if prompted > "
    )
}

fn codex_browser_transcript() -> String {
    format!(
        "WARNING: proceeding, even though we could not update PATH: Refusing to create helper binaries under \
         temporary dir \"/tmp\"\nStarting local login server on http://localhost:1455.\nIf your browser did not \
         open, navigate to this URL to authenticate:\n\n{CODEX_LINK}\n\nOn a remote or headless machine? Use `codex \
         login --device-auth` instead."
    )
}

/// As piped: the colors stay.
const CODEX_DEVICE: &str = "\n\x1b[1mWelcome to Codex [v\x1b[0m\x1b[90m0.169.0\x1b[0m\x1b[1m]\x1b[0m\n\n1. Open this \
                            link in your browser and sign in to your account\n   \
                            \x1b[94mhttps://auth.openai.com/codex/device\x1b[0m\n\n2. Enter this one-time code \
                            \x1b[90m(expires in 15 minutes)\x1b[0m\n   \x1b[94mNJSZ-6I2YS\x1b[0m\n";

fn feed(transcript: &str, now: Instant) -> Vec<Event> {
    let mut parser = Parser::default();
    transcript.lines().flat_map(|line| parser.feed(line, now)).collect()
}

#[test]
fn reads_claudes_link_and_nothing_else() {
    assert_eq!(
        feed(&claude_transcript(), Instant::now()),
        [Event::Link(vetted(CLAUDE_LINK).expect("a Claude link"))]
    );
}

#[test]
fn reads_codexs_browser_link_and_drops_its_localhost_server() {
    assert_eq!(
        feed(&codex_browser_transcript(), Instant::now()),
        [Event::Link(vetted(CODEX_LINK).expect("an OpenAI link"))]
    );
}

#[test]
fn reads_codexs_device_code_and_expiry_through_its_colors() {
    let now = Instant::now();
    assert_eq!(
        feed(CODEX_DEVICE, now),
        [
            Event::Link("https://auth.openai.com/codex/device".into()),
            Event::Code {
                code: "NJSZ-6I2YS".into(),
                expires_at: now + Duration::from_secs(15 * 60),
            },
        ]
    );
    assert_eq!(strip_ansi("\x1b[94mNJSZ-6I2YS\x1b[0m\r\n"), "NJSZ-6I2YS");
    assert_eq!(strip_ansi("\x1b]0;title\x07spinner\rdone"), "done");
}

#[test]
fn keeps_an_unknown_line_and_the_error_after_claudes_prompt() {
    let mut parser = Parser::default();
    assert_eq!(
        parser.feed(
            "Paste code here if prompted > OAuth error: Invalid code",
            Instant::now()
        ),
        [Event::Line("OAuth error: Invalid code".into())]
    );
    assert!(port_error(
        "Error: failed to bind 127.0.0.1:1455: Address already in use (os error 98)"
    ));
    assert!(port_error("Port 1455 is already in use"));
    assert!(!port_error("Successfully logged in"));
}

#[test]
fn offers_only_https_links_on_the_sign_in_hosts() {
    for good in [
        "https://claude.com/cai/oauth/authorize?x=1",
        "https://claude.ai/oauth",
        "https://console.claude.ai/x",
        "https://auth.openai.com/codex/device",
        "https://eu.auth.openai.com/x",
    ] {
        assert!(vetted(good).is_some(), "{good}");
    }
    for bad in [
        "http://claude.com/cai/oauth",
        "https://claude.com.evil.test/oauth",
        "https://evilclaude.com/oauth",
        "https://openai.com/x",
        "https://chatgpt.com/x",
        "https://user@claude.com/x",
        "https://claude.com:8443/x",
        "https://127.0.0.1/x",
        "javascript:alert(1)",
    ] {
        assert_eq!(vetted(bad), None, "{bad}");
    }
    let mut parser = Parser::default();
    assert!(
        parser
            .feed("visit: https://evil.test/cai/oauth", Instant::now())
            .is_empty()
    );
}

#[test]
fn builds_each_agents_login() {
    assert_eq!(
        args(SessionAgent::Claude, None, Mode::DeviceCode).unwrap(),
        ["auth", "login", "--claudeai"]
    );
    assert_eq!(
        args(SessionAgent::Claude, Some("ana@acme.dev"), Mode::Browser).unwrap(),
        ["auth", "login", "--claudeai", "--email", "ana@acme.dev"]
    );
    assert_eq!(args(SessionAgent::Codex, None, Mode::Browser).unwrap(), ["login"]);
    assert_eq!(
        args(SessionAgent::Codex, None, Mode::DeviceCode).unwrap(),
        ["login", "--device-auth"]
    );
    assert!(args(SessionAgent::Antigravity, None, Mode::Browser).is_err());
}

fn claude_login(account: Option<&str>, email: &str) -> nexus_workbench::plans::PlanUsage {
    let init = serde_json::json!({"account": {"email": email, "subscriptionType": "Claude Max"}});
    nexus_workbench::plans::read_claude(account.map(String::from), &init, None)
}

fn codex_login(account: Option<&str>, email: &str, plan: Option<&str>) -> nexus_workbench::plans::PlanUsage {
    let answer = serde_json::json!({"account": {"type": "chatgpt", "email": email, "planType": plan}});
    nexus_workbench::plans::read_codex(account.map(String::from), &answer, None)
}

#[test]
fn a_new_account_on_a_login_already_here_is_a_duplicate() {
    use super::{Finish, finish};
    let own = claude_login(None, "ana@acme.dev");
    let new = claude_login(Some("a_2"), "Ana@Acme.dev");
    assert_eq!(
        finish(&new, std::slice::from_ref(&own), true, false),
        Finish::Duplicate("ana@acme.dev".into())
    );
    // Sign in again on an account already here keeps it: the page flags copies on its own.
    assert_eq!(finish(&new, std::slice::from_ref(&own), false, false), Finish::Done);
    let other = claude_login(Some("a_3"), "bo@acme.dev");
    assert_eq!(finish(&other, std::slice::from_ref(&own), true, false), Finish::Done);
    // Another agent's login with the same email is no copy.
    let codex = codex_login(None, "ana@acme.dev", Some("plus"));
    assert_eq!(finish(&codex, &[own], true, false), Finish::Done);
}

#[test]
fn a_login_with_no_plan_is_kept_and_said() {
    use super::{Finish, finish};
    let free = codex_login(Some("a_4"), "cy@acme.dev", None);
    assert_eq!(finish(&free, &[], true, false), Finish::NoPlan("cy@acme.dev".into()));
    // An API key reads as signed out: after the login ended, that is no plan; before, not yet.
    let key = nexus_workbench::plans::read_codex(
        Some("a_5".into()),
        &serde_json::json!({"account": {"type": "apiKey"}}),
        None,
    );
    assert_eq!(finish(&key, &[], true, true), Finish::NoPlan("This account".into()));
    assert_eq!(finish(&key, &[], true, false), Finish::NotYet);
}

// ---------------------------------------------------------------- the runner, on fake logins

#[cfg(unix)]
mod runner {
    use super::*;

    /// A temporary folder with a fake login in it, deleted at the end.
    pub(super) struct Fake {
        pub dir: PathBuf,
        script: PathBuf,
    }

    impl Fake {
        pub fn new(name: &str, script: &str) -> Fake {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default();
            let dir = std::env::temp_dir().join(format!("nexus-login-{name}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(dir.join("account")).expect("temporary folder");
            let path = dir.join("login.sh");
            std::fs::write(&path, script).expect("fake login");
            Fake { dir, script: path }
        }

        pub fn options(&self, agent: SessionAgent, mode: Mode, deadline: Duration) -> Options {
            Options {
                agent,
                // Run through the shell, so the script is never executed while a test thread holds it open.
                program: "/bin/sh".into(),
                program_args: vec![self.script.to_string_lossy().into_owned()],
                env: [
                    ("PATH".to_string(), "/usr/bin:/bin".to_string()),
                    ("OUT".to_string(), self.dir.to_string_lossy().into_owned()),
                ]
                .into(),
                dir: Some(self.dir.join("account")),
                cwd: self.dir.clone(),
                email: None,
                mode,
                deadline,
            }
        }

        pub fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.dir.join(name)).unwrap_or_default()
        }

        /// The fake's process, which wrote `$$` to `pid`.
        pub fn alive(&self) -> bool {
            let pid: i32 = self.read("pid").trim().parse().expect("the fake wrote its pid");
            // SAFETY: signal 0 only asks whether the process exists.
            unsafe { libc::kill(pid, 0) == 0 }
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The events up to the first that `stop` matches (included), within ten seconds.
    pub(super) fn until(events: &mut UnboundedReceiver<Event>, stop: impl Fn(&Event) -> bool) -> Vec<Event> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let mut seen = vec![];
            loop {
                match tokio::time::timeout(Duration::from_secs(10), events.next()).await {
                    Ok(Some(event)) => {
                        let done = stop(&event);
                        seen.push(event);
                        if done {
                            return seen;
                        }
                    }
                    other => panic!("no matching event ({other:?}) after {seen:?}"),
                }
            }
        })
    }

    fn exited(event: &Event) -> bool {
        matches!(event, Event::Exited(_))
    }

    const DEVICE_SCRIPT: &str = r#"
printf '\n\033[1mWelcome to Codex [v\033[0m\033[90m0.169.0\033[0m\033[1m]\033[0m\n'
printf '1. Open this link in your browser and sign in to your account\n'
printf '   \033[94mhttps://auth.openai.com/codex/device\033[0m\n'
printf '2. Enter this one-time code \033[90m(expires in 15 minutes)\033[0m\n'
printf '   \033[94mNJSZ-6I2YS\033[0m\n'
"#;

    #[test]
    fn claude_waits_with_its_link_and_a_pasted_code_reaches_stdin_once() {
        let fake = Fake::new(
            "claude",
            r#"
printf '%s\n' "$@" > "$OUT/args"
printf '%s' "$CLAUDE_CONFIG_DIR" > "$OUT/env"
echo 'Opening browser to sign in…'
echo "If the browser didn't open, visit: https://claude.com/cai/oauth/authorize?state=1"
printf 'Paste code here if prompted > '
read code
printf '%s\n' "$code" >> "$OUT/stdin"
echo 'Login successful.'
"#,
        );
        let mut options = fake.options(SessionAgent::Claude, Mode::Browser, Duration::from_secs(30));
        options.email = Some("ana@acme.dev".into());
        let (runner, mut events) = Runner::start(options);
        let seen = until(&mut events, |event| matches!(event, Event::Link(_)));
        assert_eq!(
            seen,
            [
                Event::Started(Mode::Browser),
                Event::Link("https://claude.com/cai/oauth/authorize?state=1".into())
            ]
        );
        // It waits for the code: nothing ends it but the person.
        std::thread::sleep(Duration::from_millis(300));
        assert!(events.try_recv().is_err(), "the login is still waiting");
        runner.submit_code("abc#123".into());
        let seen = until(&mut events, exited);
        assert_eq!(seen.last(), Some(&Event::Exited(Exit::Status(Some(0)))));
        assert_eq!(fake.read("stdin"), "abc#123\n", "the code reached the login once");
        assert_eq!(fake.read("args"), "auth\nlogin\n--claudeai\n--email\nana@acme.dev\n");
        assert_eq!(fake.read("env"), fake.dir.join("account").to_string_lossy());
    }

    #[test]
    fn codex_device_code_and_its_expiry_read_through_colors() {
        let fake = Fake::new(
            "codex-device",
            &format!("printf '%s\\n' \"$@\" > \"$OUT/args\"\necho $$ > \"$OUT/pid\"\n{DEVICE_SCRIPT}\nsleep 30\n"),
        );
        let started = Instant::now();
        let (runner, mut events) =
            Runner::start(fake.options(SessionAgent::Codex, Mode::DeviceCode, Duration::from_secs(30)));
        let seen = until(&mut events, |event| matches!(event, Event::Code { .. }));
        assert_eq!(seen[0], Event::Started(Mode::DeviceCode));
        assert_eq!(seen[1], Event::Link("https://auth.openai.com/codex/device".into()));
        let Event::Code { code, expires_at } = &seen[2] else {
            panic!("{seen:?}")
        };
        assert_eq!(code, "NJSZ-6I2YS");
        let lasts = expires_at.duration_since(started);
        assert!(
            lasts <= Duration::from_secs(15 * 60) + Duration::from_secs(5) && lasts >= Duration::from_secs(14 * 60)
        );
        assert_eq!(fake.read("args"), "login\n--device-auth\n");
        // Cancel kills it.
        runner.cancel();
        assert_eq!(until(&mut events, exited).last(), Some(&Event::Exited(Exit::Cancelled)));
        assert!(!fake.alive());
    }

    #[test]
    fn codex_browser_that_cannot_bind_restarts_with_the_device_code() {
        let fake = Fake::new(
            "codex-port",
            &format!(
                "case \"$2\" in\n--device-auth) {DEVICE_SCRIPT} ;;\n*) echo 'Error: failed to bind 127.0.0.1:1455: \
                 Address already in use (os error 98)' >&2; sleep 30 ;;\nesac\n"
            ),
        );
        let (_runner, mut events) =
            Runner::start(fake.options(SessionAgent::Codex, Mode::Browser, Duration::from_secs(30)));
        let seen = until(&mut events, exited);
        let started: Vec<&Event> = seen.iter().filter(|event| matches!(event, Event::Started(_))).collect();
        assert_eq!(
            started,
            [&Event::Started(Mode::Browser), &Event::Started(Mode::DeviceCode)]
        );
        assert!(
            seen.iter()
                .any(|event| matches!(event, Event::Code { code, .. } if code == "NJSZ-6I2YS"))
        );
        assert_eq!(seen.last(), Some(&Event::Exited(Exit::Status(Some(0)))));
    }

    #[test]
    fn codex_browser_that_fails_early_restarts_and_a_code_can_be_asked_for() {
        let fake = Fake::new(
            "codex-early",
            &format!("case \"$2\" in\n--device-auth) {DEVICE_SCRIPT} ;;\n*) exit 3 ;;\nesac\n"),
        );
        let (_runner, mut events) =
            Runner::start(fake.options(SessionAgent::Codex, Mode::Browser, Duration::from_secs(30)));
        let seen = until(&mut events, exited);
        assert!(seen.contains(&Event::Started(Mode::DeviceCode)));
        assert_eq!(seen.last(), Some(&Event::Exited(Exit::Status(Some(0)))));

        // "Use a code instead" while the browser login waits.
        let fake = Fake::new(
            "codex-ask",
            &format!(
                "case \"$2\" in\n--device-auth) {DEVICE_SCRIPT} ;;\n*) echo $$ > \"$OUT/pid\"; echo \
                 'https://auth.openai.com/oauth/authorize?x=1'; sleep 30 ;;\nesac\n"
            ),
        );
        let (runner, mut events) =
            Runner::start(fake.options(SessionAgent::Codex, Mode::Browser, Duration::from_secs(30)));
        until(&mut events, |event| matches!(event, Event::Link(_)));
        runner.device_code();
        let seen = until(&mut events, exited);
        assert!(seen.contains(&Event::Started(Mode::DeviceCode)));
        assert!(!fake.alive(), "the browser login was stopped");
    }

    #[test]
    fn a_failure_keeps_the_last_line_said() {
        let fake = Fake::new(
            "claude-fail",
            "echo 'Opening browser to sign in…'\necho 'Error: OAuth token exchange failed (400)' >&2\nexit 1\n",
        );
        let (_runner, mut events) =
            Runner::start(fake.options(SessionAgent::Claude, Mode::Browser, Duration::from_secs(30)));
        let seen = until(&mut events, exited);
        assert!(seen.contains(&Event::Line("Error: OAuth token exchange failed (400)".into())));
        assert_eq!(seen.last(), Some(&Event::Exited(Exit::Status(Some(1)))));
        // A Claude login that fails is not retried: only Codex has a second way.
        assert_eq!(
            seen.iter().filter(|event| matches!(event, Event::Started(_))).count(),
            1
        );
    }

    #[test]
    fn the_deadline_and_dropping_the_runner_kill_the_login() {
        let script = "echo $$ > \"$OUT/pid\"\necho 'https://claude.com/x'\nsleep 30\n";
        let fake = Fake::new("deadline", script);
        let (_runner, mut events) =
            Runner::start(fake.options(SessionAgent::Claude, Mode::Browser, Duration::from_millis(400)));
        assert_eq!(until(&mut events, exited).last(), Some(&Event::Exited(Exit::TimedOut)));
        assert!(!fake.alive());

        let fake = Fake::new("dropped", script);
        let (runner, mut events) =
            Runner::start(fake.options(SessionAgent::Claude, Mode::Browser, Duration::from_secs(30)));
        until(&mut events, |event| matches!(event, Event::Link(_)));
        drop(runner);
        assert_eq!(until(&mut events, exited).last(), Some(&Event::Exited(Exit::Cancelled)));
        assert!(!fake.alive());
    }

    #[test]
    fn a_missing_program_does_not_start() {
        let fake = Fake::new("missing", "");
        let mut options = fake.options(SessionAgent::Claude, Mode::Browser, Duration::from_secs(5));
        options.program = Path::new("/nonexistent/claude").into();
        let (_runner, mut events) = Runner::start(options);
        assert!(matches!(
            until(&mut events, exited)[..],
            [Event::Exited(Exit::NotStarted(_))]
        ));
    }
}

/// Use the native test executable as the provider, including on Windows where the shell fixtures cannot run.
#[test]
fn browser_login_starts_both_native_providers_with_the_selected_profile() {
    use std::collections::HashMap;

    const PROVIDER: &str = "NEXUS_TEST_BROWSER_LOGIN_PROVIDER";
    if let Ok(agent) = std::env::var(PROVIDER) {
        let args: Vec<_> = std::env::args().skip_while(|arg| arg != "--").skip(1).collect();
        let (expected, variable, transcript) = if agent == "claude" {
            (
                vec!["auth", "login", "--claudeai"],
                "CLAUDE_CONFIG_DIR",
                claude_transcript(),
            )
        } else {
            (vec!["login"], "CODEX_HOME", codex_browser_transcript())
        };
        assert_eq!(args, expected);
        assert_eq!(
            PathBuf::from(std::env::var_os(variable).unwrap())
                .canonicalize()
                .unwrap(),
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
        println!("\n{transcript}");
        return;
    }

    for (agent, url) in [(SessionAgent::Claude, CLAUDE_LINK), (SessionAgent::Codex, CODEX_LINK)] {
        let home = tempfile::tempdir().unwrap();
        let profile = home.path().join("Work account");
        std::fs::create_dir(&profile).unwrap();
        let mut env: HashMap<String, String> = ["SystemRoot", "PATH", "DYLD_FALLBACK_LIBRARY_PATH", "LD_LIBRARY_PATH"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok().map(|value| (name.into(), value)))
            .collect();
        let home_path = home.path().to_string_lossy().into_owned();
        for name in ["HOME", "USERPROFILE", "CFFIXED_USER_HOME", "TMPDIR", "TMP", "TEMP"] {
            env.insert(name.into(), home_path.clone());
        }
        env.insert(PROVIDER.into(), agent.as_str().into());
        let test = std::thread::current().name().unwrap().to_string();
        let options = Options {
            agent,
            program: std::env::current_exe().unwrap(),
            program_args: vec!["--exact".into(), test, "--nocapture".into(), "--".into()],
            env,
            dir: Some(profile.clone()),
            cwd: profile,
            email: None,
            mode: Mode::Browser,
            deadline: Duration::from_secs(20),
        };
        let (_runner, events) = Runner::start(options);
        let seen = futures::executor::block_on(events.collect::<Vec<_>>());
        assert_eq!(seen.first(), Some(&Event::Started(Mode::Browser)), "{seen:?}");
        assert!(seen.contains(&Event::Link(vetted(url).unwrap())), "{seen:?}");
        assert_eq!(seen.last(), Some(&Event::Exited(Exit::Status(Some(0)))), "{seen:?}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn a_missing_default_store_stops_only_claude() {
    // Scope the metadata probe to a fresh temporary home. The parent's active keychain must never enter this test.
    const CHILD: &str = "NEXUS_TEST_MISSING_LOGIN_STORE";
    if std::env::var_os(CHILD).is_some() {
        assert!(super::check_login_storage(SessionAgent::Claude).is_err());
        assert!(super::check_login_storage(SessionAgent::Codex).is_ok());
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let thread = std::thread::current();
    let output = nexus_workbench::process::command(std::env::current_exe().unwrap())
        .args(["--exact", thread.name().unwrap()])
        .env(CHILD, "1")
        .env("HOME", home.path())
        .env("CFFIXED_USER_HOME", home.path())
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && report.contains("1 passed"), "{report}");
}
