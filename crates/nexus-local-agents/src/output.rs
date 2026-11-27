//! Bounded output collection for local commands.

use std::process::Stdio;
use std::time::Duration;

/// After a command exited, how long [`stdout_within`] still waits for the end of its output.
const STDOUT_DRAIN: Duration = Duration::from_millis(500);

/// Runs `command` with nothing on its input and its errors dropped, for at most `limit`: how it exited and what it
/// printed, or None when it ran over (it is killed then). Its output is read as it comes, so a long one cannot fill
/// the pipe and stall it. Once it exited, the output is taken as it is after at most 500 milliseconds: a process it
/// left running with the output open (a daemon a shell profile starts, say) must not hold the caller for good. So the
/// output may include text a leftover process printed within that window: read what you need from it (a prefix, the
/// first line), not all of it.
pub fn stdout_within(
    mut command: std::process::Command,
    limit: Duration,
) -> Option<(std::process::ExitStatus, String)> {
    use std::io::Read;
    use std::sync::{Arc, Mutex};
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let printed = Arc::new(Mutex::new(Vec::new()));
    let (ended, end) = std::sync::mpsc::channel::<()>();
    {
        let printed = printed.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8 * 1024];
            while let Ok(read) = stdout.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                // Once the caller took the output, what a process left behind prints is read (so it never meets a
                // closed pipe) but not kept.
                if Arc::strong_count(&printed) > 1
                    && let Ok(mut printed) = printed.lock()
                {
                    printed.extend_from_slice(&buffer[..read]);
                }
            }
            let _ = ended.send(());
        });
    }
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let _ = end.recv_timeout(STDOUT_DRAIN);
    let printed = printed.lock().map(|printed| printed.clone()).unwrap_or_default();
    Some((status, String::from_utf8_lossy(&printed).into_owned()))
}

pub(crate) fn async_command(program: &std::path::Path) -> tokio::process::Command {
    hide_console(tokio::process::Command::new(program))
}

/// The Node-aware agent command for a model-catalog probe.
pub(crate) fn async_agent_command(program: &std::path::Path) -> tokio::process::Command {
    hide_console(tokio::process::Command::from(crate::command(program)))
}

fn hide_console(command: tokio::process::Command) -> tokio::process::Command {
    #[cfg(windows)]
    let command = {
        let mut command = command;
        command.creation_flags(0x0800_0000);
        command
    };
    command
}

/// Test helper. Writes an executable script (mode 755) from a child shell, so this process never holds the file open
/// for writing. Written here, a test thread that forks (any spawn) between the write and the close hands the open file
/// to its child until that child execs, and running the script then fails with "Text file busy" (ETXTBSY).
#[cfg(all(unix, test))]
#[doc(hidden)]
pub fn write_fake_script(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    let written = std::process::Command::new("/bin/sh")
        .args(["-c", r#"printf '%s' "$2" > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .arg(body)
        .status()
        .unwrap_or_else(|e| panic!("could not run sh to write {}: {e}", path.display()));
    assert!(written.success(), "could not write {}", path.display());
    let mode = std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("could not read {} back: {e}", path.display()))
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "{} is not mode 755", path.display());
}
