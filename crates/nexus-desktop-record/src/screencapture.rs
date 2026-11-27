//! The video and the narration, recorded by macOS's own `screencapture -v` (ScreenCaptureKit underneath): the display,
//! the clicks drawn in (`-k`) and the default microphone (`-g`), into `recording.mov`. It records until it is
//! interrupted (SIGINT, as ctrl-c), then writes the file and exits. Its prompt says that a character typed on its input
//! stops it too, but a newline through a pipe does not: it keeps recording.
//! macOS asks the person once to let Nexus record the screen and use the microphone (Info.plist's reasons).

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct Recording {
    child: Child,
    pub narration: bool,
}

/// How long `screencapture` takes to fail when it cannot record (no permission, no such display).
const SETTLE: Duration = Duration::from_millis(1200);

/// How long `screencapture` may take to write the file once interrupted (a tenth of a second after a short recording).
const WRITE: Duration = Duration::from_secs(10);

fn spawn(display: u32, path: &Path, narration: bool) -> std::io::Result<Child> {
    let mut command = Command::new("/usr/sbin/screencapture");
    command.args(["-v", "-k", "-x", "-D", &display.to_string()]);
    if narration {
        command.arg("-g");
    }
    command
        .arg(path)
        // Never written, as it does not stop for its input; a pipe, as when that was tried.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
}

/// What a `screencapture` that stopped early said.
fn failure(child: &mut Child) -> String {
    let mut text = String::new();
    if let Some(stderr) = child.stderr.as_mut() {
        let _ = stderr.read_to_string(&mut text);
    }
    let text = text.trim();
    if text.is_empty() {
        "screencapture stopped before recording".into()
    } else {
        text.lines().last().unwrap_or(text).to_string()
    }
}

/// Wait up to `limit` for `child` to exit; true when it has.
fn exited_within(child: &mut Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return true,
            Ok(None) if Instant::now() >= deadline => return false,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

impl Recording {
    /// Start recording display `display` (1 is the main one) into `path`, with the microphone; without it when that
    /// fails (no microphone, or not allowed). Blocks for about a second, to see that it really started.
    pub fn start(display: u32, path: &Path) -> Result<Recording, String> {
        let mut last = String::new();
        for narration in [true, false] {
            let mut child = spawn(display, path, narration).map_err(|error| format!("screencapture: {error}"))?;
            if !exited_within(&mut child, SETTLE) {
                return Ok(Recording { child, narration });
            }
            last = failure(&mut child);
            let _ = std::fs::remove_file(path);
        }
        Err(last)
    }

    /// Whether it is still recording (it stops by itself when the display goes away or the permission is revoked).
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Interrupt it, which stops the recording at once, and wait up to `WRITE` for it to write the file and exit;
    /// killed when it has not. True when it exited by itself.
    fn interrupt(&mut self) -> bool {
        let _ = Command::new("/bin/kill")
            .args(["-INT", &self.child.id().to_string()])
            .status();
        if exited_within(&mut self.child, WRITE) {
            return true;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        false
    }

    /// Stop and let it write the file. Returns when it is written, or an error.
    pub fn stop(mut self) -> Result<(), String> {
        if self.interrupt() {
            Ok(())
        } else {
            Err("screencapture did not stop".into())
        }
    }

    /// Stop without keeping anything (the caller deletes the file).
    pub fn cancel(mut self) {
        self.interrupt();
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};

    use super::*;

    /// Stands in for `screencapture`, which would record the screen: ignores its input, and writes `file` and exits
    /// when interrupted. Returns once it is ready for the interrupt.
    fn stand_in(file: &Path) -> Recording {
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                r#"trap 'echo movie > "$0"; kill $!; exit 0' INT; sleep 60 & echo ready; wait"#,
            ])
            .arg(file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("sh");
        let mut ready = String::new();
        BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut ready)
            .expect("ready");
        Recording {
            child,
            narration: false,
        }
    }

    #[test]
    fn stop_interrupts_at_once_and_returns_when_the_file_is_written() {
        let file = std::env::temp_dir().join(format!("nexus-desktop-screencapture-{}.mov", std::process::id()));
        let recording = stand_in(&file);
        let asked = Instant::now();
        recording.stop().expect("stop");
        assert!(
            asked.elapsed() < Duration::from_secs(2),
            "stopped after {:?}",
            asked.elapsed()
        );
        assert_eq!(std::fs::read_to_string(&file).expect("file"), "movie\n");
        std::fs::remove_file(&file).ok();
    }
}
