//! A new terminal window that runs one command: resuming an agent's conversation from this computer (Settings ›
//! History) and an agent's own plan login. Nexus GPUI's History page uses it.
//!
//! The command is written to a private script first, then the OS opens the script in a terminal: macOS with `open`
//! (Terminal, or the app the person chose for `.command` files), Linux with the person's available terminal,
//! Windows with `cmd /k` in a console of its own (Windows Terminal when the person made it the default).
//! A conversation is named only by its agent and id: the folder and the command come from the transcript on disk
//! (`nexus_client::local_sessions::find`), and the id is letters, digits and dashes, so nothing a page sends reaches a
//! shell.

use std::path::{Path, PathBuf};

use nexus_client::agent_entries::Agent as ClientAgent;
use nexus_client::agent_setup::{agent_entry, route_of};
use nexus_client::agents::Route;
use nexus_client::local_sessions::{self, LocalSession, shell_quote};

mod linux;

/// The operating systems a terminal opens on, named so each one's script and command test on any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    Linux,
    /// Any other: no terminal opens.
    Other,
}

impl Os {
    /// This computer's.
    pub const CURRENT: Os = if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else {
        Os::Other
    };

    /// The script files' extension: what a double click (macOS) or `cmd /k` (Windows) runs.
    pub fn script_extension(self) -> &'static str {
        if self == Os::Windows { "cmd" } else { "command" }
    }
}

/// The process that opens a script in a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opener {
    pub program: String,
    /// The arguments. On Windows one argument, passed as written (`raw_arg`): cmd does not read a C runtime's quoting.
    pub args: Vec<String>,
    /// Wait for it and check its exit status (`open` returns once the terminal has the file). Otherwise a started
    /// process counts as opened.
    pub wait: bool,
}

/// How `os` opens `script` in a terminal; `None` where Nexus knows no way.
pub fn opener(os: Os, script: &Path) -> Option<Opener> {
    let path = script.display().to_string();
    match os {
        Os::Mac => Some(Opener {
            program: "open".into(),
            args: vec![path],
            wait: true,
        }),
        Os::Linux => Some(Opener {
            program: "x-terminal-emulator".into(),
            args: vec!["-e".into(), path],
            wait: false,
        }),
        // With `/s` cmd drops the outer pair of quotes and runs `"<path>"` whole, so a path with spaces, `&`, `(`,
        // `)`, `@` or `^` stays one path. A Windows path has no `"`. `/k` keeps the console open after the script, so
        // the person sees how it went.
        Os::Windows => Some(Opener {
            program: "cmd.exe".into(),
            args: vec![format!("/s /k \"\"{path}\"\"")],
            wait: false,
        }),
        Os::Other => None,
    }
}

/// The agent's own login for its plan, from the catalog; `None` for an agent the catalog does not know, or for a
/// name that is not letters, digits and dashes, since the name also names the script file.
pub fn login_command(agent: &str) -> Option<String> {
    if !agent.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    route_of(&agent_entry(None, ClientAgent::parse(agent)?), Route::Subscription)?["login"]
        .as_str()
        .map(String::from)
}

/// The script a terminal runs on `os`: into `cwd` when there is one, then `command` with `env` set. A login-shell
/// script on macOS and Linux, a batch file on Windows.
pub fn terminal_script(os: Os, cwd: Option<&str>, env: Option<(&str, &str)>, command: &str) -> Result<String, String> {
    if os == Os::Windows {
        return in_cmd(cwd, env, command);
    }
    Ok(match env {
        Some((name, value)) => in_login_shell(cwd, &format!("{name}={} {command}", shell_quote(value))),
        None => in_login_shell(cwd, command),
    })
}

/// A batch file that runs `command` in `cmd`, after moving into `cwd` and setting `env`. Quoted `cd` and `set`
/// keep spaces and `&` literal; `%` is doubled, since a batch file expands it even inside quotes. `cwd` comes from a
/// transcript on disk, so one with `"` or a line break, which no Windows path has, is refused; `env` holds a folder the
/// app named. The file is UTF-8 and cmd reads a batch file in the console's code page, so `chcp 65001` comes first: a
/// folder named `José` stays `José`.
fn in_cmd(cwd: Option<&str>, env: Option<(&str, &str)>, command: &str) -> Result<String, String> {
    let literal = |text: &str| text.replace('%', "%%");
    let mut script = String::from("@echo off\r\nchcp 65001>nul\r\n");
    if let Some(cwd) = cwd {
        if cwd.contains(['"', '\r', '\n']) {
            return Err("That conversation's folder is not a Windows path.".into());
        }
        script.push_str(&format!("cd /d \"{}\" || exit /b 1\r\n", literal(cwd)));
    }
    if let Some((name, value)) = env {
        script.push_str(&format!("set \"{name}={}\"\r\n", literal(value)));
    }
    // `call`: an agent installed by npm is a batch file itself, and without it the script would end there.
    script.push_str(&format!("call {command}\r\n"));
    Ok(script)
}

/// A script that runs `command` in the person's login shell, so their `PATH` (where `nexus` and the agent live)
/// applies, after moving into `cwd` when there is one.
fn in_login_shell(cwd: Option<&str>, command: &str) -> String {
    let cd = cwd
        .map(|cwd| format!("cd {} || exit 1\n", shell_quote(cwd)))
        .unwrap_or_default();
    format!(
        "#!/bin/sh\n{cd}exec \"${{SHELL:-/bin/sh}}\" -l -i -c {}\n",
        shell_quote(command)
    )
}

/// The script that resumes `session` on `os`: into its folder, then the agent's `--resume` under `nexus claude` /
/// `nexus codex`, on the person's saved route. An error for an agent or an id Nexus cannot resume, or for a folder a
/// batch file cannot hold.
pub fn resume_script(os: Os, session: &LocalSession) -> Result<String, String> {
    let resume = ClientAgent::parse(&session.agent)
        .and_then(|agent| local_sessions::resume_command(agent, &session.id, None))
        .ok_or("Nexus cannot resume that conversation's agent.")?;
    terminal_script(os, session.cwd.as_deref(), None, &resume)
}

/// Writes the resume script as `<dir>/resume-<id>.command` (`.cmd` on Windows; executable, only the person may read
/// it).
pub fn write_resume_script(dir: &Path, session: &LocalSession) -> Result<PathBuf, String> {
    if !local_sessions::valid_id(&session.id) {
        return Err("That conversation's id cannot be resumed.".into());
    }
    let script = resume_script(Os::CURRENT, session)?;
    let name = format!("resume-{}.{}", session.id, Os::CURRENT.script_extension());
    write_private(dir, &name, &script)
}

/// Writes the login script for `agent` as `<dir>/login-<agent>.command` (`.cmd` on Windows), with `env` set.
pub fn write_login_script(dir: &Path, agent: &str, env: Option<(&str, &str)>, login: &str) -> Result<PathBuf, String> {
    let script = terminal_script(Os::CURRENT, None, env, login)?;
    let name = format!("login-{agent}.{}", Os::CURRENT.script_extension());
    write_private(dir, &name, &script)
}

/// Writes `<dir>/<name>` (executable, only the person may read it).
fn write_private(dir: &Path, name: &str, script: &str) -> Result<PathBuf, String> {
    nexus_client::config::create_private_dir(dir).map_err(|error| error.to_string())?;
    let path = dir.join(name);
    std::fs::write(&path, script).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())?;
    }
    Ok(path)
}

/// Resume the conversation `agent`/`id` in a new terminal window, its script in `dir`. Blocking: it reads the
/// transcripts (`env` finds their folders) and, on macOS, waits for `open`.
pub fn resume(dir: &Path, env: &dyn Fn(&str) -> Option<String>, agent: &str, id: &str) -> Result<(), String> {
    let session = ClientAgent::parse(agent)
        .and_then(|agent| local_sessions::find(env, agent, id))
        .ok_or_else(|| "That conversation is no longer on this computer.".to_string())?;
    let script = write_resume_script(dir, &session)?;
    open(&script)
}

/// Opens `script` in a new terminal window ([`opener`]). When none opens, the error names the script, for the person to
/// run themselves.
pub fn open(script: &Path) -> Result<(), String> {
    let opened = if Os::CURRENT == Os::Linux {
        let terminal = std::env::var("TERMINAL").ok();
        let path = std::env::var_os("PATH");
        linux::open(script, terminal.as_deref(), path.as_deref(), start)
    } else {
        match opener(Os::CURRENT, script) {
            Some(opener) => start(&opener),
            None => Err(std::io::ErrorKind::Unsupported.into()),
        }
    };
    match opened {
        Ok(true) => Ok(()),
        _ => Err(format!(
            "Could not open a terminal. Run {} in one instead.",
            script.display()
        )),
    }
}

/// Starts `opener`; true when it opened (or, when it is not waited for, started).
#[cfg(not(windows))]
fn start(opener: &Opener) -> std::io::Result<bool> {
    let mut command = nexus_workbench::process::command(&opener.program);
    command.args(&opener.args);
    if opener.wait {
        command.status().map(|status| status.success())
    } else {
        let mut child = command.spawn()?;
        #[cfg(target_os = "linux")]
        {
            // A resolver can exist yet fail to open its configured terminal. Catch that early exit without
            // waiting for the person's interactive terminal session to finish.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
            loop {
                if let Some(status) = child.try_wait()? {
                    return Ok(status.success());
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = &mut child;
        Ok(true)
    }
}

/// Windows: the console window of its own that the person types into.
#[cfg(windows)]
fn start(opener: &Opener) -> std::io::Result<bool> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    // The one process that must have a console: the person types into it.
    #[allow(clippy::disallowed_methods)]
    let mut command = std::process::Command::new(&opener.program);
    for arg in &opener.args {
        command.raw_arg(arg);
    }
    command.creation_flags(CREATE_NEW_CONSOLE).spawn().map(|_| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn a_terminal_resolver_that_exits_with_failure_falls_back_to_the_next_terminal() {
        let dir = std::env::temp_dir().join(format!("nexus-terminal-fallback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("terminal folder");
        nexus_workbench::process::write_fake_script(&dir.join("xdg-terminal-exec"), "#!/bin/sh\nexit 1\n");
        nexus_workbench::process::write_fake_script(&dir.join("kitty"), "#!/bin/sh\nexit 0\n");
        let mut tried = Vec::new();
        let opened = linux::open(Path::new("/tmp/login.command"), None, Some(dir.as_os_str()), |opener| {
            tried.push(opener.program.clone());
            let mut opener = opener.clone();
            opener.program = dir.join(&opener.program).to_string_lossy().into_owned();
            start(&opener)
        })
        .expect("terminal fallback");
        assert!(opened);
        assert_eq!(tried, ["xdg-terminal-exec", "kitty"]);
        std::fs::remove_dir_all(dir).expect("remove terminal folder");
    }

    fn session(agent: &str, cwd: Option<&str>) -> LocalSession {
        LocalSession {
            agent: agent.into(),
            id: "0b1c2d3e-4f50".into(),
            title: None,
            cwd: cwd.map(String::from),
            project: None,
            git_branch: None,
            model: None,
            entrypoint: None,
            started_at: None,
            updated_at: "2026-10-19T10:00:00.000Z".into(),
            pr_url: None,
            resume: String::new(),
        }
    }

    #[test]
    fn each_os_opens_the_script_with_its_own_terminal() {
        let script = Path::new("/home/ada/.cache/resume/resume-0b1c2d3e-4f50.command");
        assert_eq!(
            opener(Os::Mac, script),
            Some(Opener {
                program: "open".into(),
                args: vec![script.display().to_string()],
                wait: true,
            })
        );
        assert_eq!(
            opener(Os::Linux, script),
            Some(Opener {
                program: "x-terminal-emulator".into(),
                args: vec!["-e".into(), script.display().to_string()],
                wait: false,
            })
        );
        let windows = Path::new(r"C:\Users\Ada Lovelace\AppData\Local\resume\resume-0b1c2d3e-4f50.cmd");
        assert_eq!(
            opener(Os::Windows, windows),
            Some(Opener {
                program: "cmd.exe".into(),
                args: vec![r#"/s /k ""C:\Users\Ada Lovelace\AppData\Local\resume\resume-0b1c2d3e-4f50.cmd"""#.into()],
                wait: false,
            })
        );
        assert_eq!(opener(Os::Other, script), None);
        assert_eq!(Os::Windows.script_extension(), "cmd");
        assert_eq!(Os::Mac.script_extension(), "command");
        assert_eq!(Os::Linux.script_extension(), "command");
    }

    #[test]
    fn the_script_goes_to_the_folder_and_resumes_in_the_login_shell() {
        for os in [Os::Mac, Os::Linux] {
            assert_eq!(
                resume_script(os, &session("claude", Some("/Users/ada/my 'web'"))).as_deref(),
                Ok(
                    "#!/bin/sh\ncd '/Users/ada/my '\\''web'\\''' || exit 1\nexec \"${SHELL:-/bin/sh}\" -l -i -c 'nexus claude --resume 0b1c2d3e-4f50'\n"
                )
            );
            assert_eq!(
                resume_script(os, &session("codex", None)).as_deref(),
                Ok("#!/bin/sh\nexec \"${SHELL:-/bin/sh}\" -l -i -c 'nexus codex resume 0b1c2d3e-4f50'\n")
            );
            assert!(resume_script(os, &session("gemini", None)).is_err());
        }
    }

    #[test]
    fn on_windows_the_resume_script_is_a_batch_file() {
        assert_eq!(
            resume_script(Os::Windows, &session("claude", Some(r"C:\Users\Ada\web"))).as_deref(),
            Ok(
                "@echo off\r\nchcp 65001>nul\r\ncd /d \"C:\\Users\\Ada\\web\" || exit /b 1\r\ncall nexus claude --resume 0b1c2d3e-4f50\r\n"
            )
        );
    }

    #[test]
    fn connects_only_agents_the_catalog_knows_with_their_own_login() {
        assert_eq!(login_command("claude").as_deref(), Some("claude auth login"));
        assert_eq!(login_command("codex").as_deref(), Some("codex login"));
        assert_eq!(login_command("antigravity").as_deref(), Some("agy"));
        assert_eq!(login_command("x; rm -rf ~"), None);
        assert_eq!(login_command("../../etc/passwd"), None);
        assert_eq!(
            in_login_shell(None, "claude auth login"),
            "#!/bin/sh\nexec \"${SHELL:-/bin/sh}\" -l -i -c 'claude auth login'\n"
        );
        assert_eq!(
            terminal_script(
                Os::Linux,
                None,
                Some(("CLAUDE_CONFIG_DIR", "/home/ada/acct one")),
                "claude auth login"
            )
            .as_deref(),
            Ok(
                "#!/bin/sh\nexec \"${SHELL:-/bin/sh}\" -l -i -c 'CLAUDE_CONFIG_DIR='\\''/home/ada/acct one'\\'' claude auth login'\n"
            )
        );
    }

    #[test]
    fn on_windows_a_batch_file_goes_to_the_folder_sets_the_account_and_calls_the_command() {
        assert_eq!(
            in_cmd(None, None, "claude auth login").as_deref(),
            Ok("@echo off\r\nchcp 65001>nul\r\ncall claude auth login\r\n")
        );
        assert_eq!(
            in_cmd(
                Some(r"C:\Users\Ada Lovelace\100% & co"),
                Some(("CLAUDE_CONFIG_DIR", r"C:\Users\Ada Lovelace\AppData\Local\acct")),
                "nexus claude --resume 0b1c2d3e-4f50"
            )
            .as_deref(),
            Ok(
                "@echo off\r\nchcp 65001>nul\r\ncd /d \"C:\\Users\\Ada Lovelace\\100%% & co\" || exit /b 1\r\n\
                set \"CLAUDE_CONFIG_DIR=C:\\Users\\Ada Lovelace\\AppData\\Local\\acct\"\r\n\
                call nexus claude --resume 0b1c2d3e-4f50\r\n"
            )
        );
    }

    #[test]
    fn on_windows_a_batch_file_reads_utf8_and_refuses_a_folder_that_breaks_its_quotes() {
        assert_eq!(
            in_cmd(Some(r"C:\Users\José\código"), None, "claude auth login").as_deref(),
            Ok(
                "@echo off\r\nchcp 65001>nul\r\ncd /d \"C:\\Users\\José\\código\" || exit /b 1\r\ncall claude auth login\r\n"
            )
        );
        for cwd in [r#"C:\a" & calc & ""#, "C:\\a\r\ncalc", "C:\\a\ncalc"] {
            assert!(in_cmd(Some(cwd), None, "claude auth login").is_err(), "{cwd:?}");
        }
    }

    #[test]
    fn writes_a_private_executable_script_and_refuses_an_unsafe_id() {
        let dir = std::env::temp_dir().join(format!("nexus-resume-{}", std::process::id()));
        let path = write_resume_script(&dir, &session("claude", Some("/tmp"))).expect("script");
        assert!(path.ends_with(format!("resume-0b1c2d3e-4f50.{}", Os::CURRENT.script_extension())));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777,
                0o700
            );
        }
        let mut bad = session("claude", None);
        bad.id = "x; rm -rf ~".into();
        assert!(write_resume_script(&dir, &bad).is_err());
        let login = write_login_script(&dir, "claude", None, "claude auth login").expect("login");
        assert!(login.ends_with(format!("login-claude.{}", Os::CURRENT.script_extension())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_conversation_no_longer_on_this_computer_opens_nothing() {
        let home = std::env::temp_dir().join(format!("nexus-resume-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).expect("home");
        let root = home.to_string_lossy().into_owned();
        let env = move |name: &str| (name == "HOME" || name == "USERPROFILE").then(|| root.clone());
        let dir = home.join("resume");
        assert_eq!(
            resume(&dir, &env, "claude", "0b1c2d3e-4f50"),
            Err("That conversation is no longer on this computer.".into())
        );
        assert_eq!(
            resume(&dir, &env, "gemini", "0b1c2d3e-4f50"),
            Err("That conversation is no longer on this computer.".into())
        );
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(&home);
    }
}
