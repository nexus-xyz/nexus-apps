//! Servo as the page engine, to compare with CEF: `NEXUS_WEB_ENGINE=servo`. Servo cannot be linked into this binary
//! (its pinned ICU and SQLite clash with GPUI's and nexus-workbench's), so it runs in its own process,
//! `nexus-servo-host` (`servo-host/`). That process renders off screen and writes each frame into a memory file. This
//! side starts it, sends it the page's input as JSON lines, and reads its events into the page's [`Shared`] state,
//! the same state CEF's handlers fill.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

use serde_json::{Value, json};

use super::SharedRef;
use super::{Cursor, Frame};

/// Whether pages use Servo instead of CEF.
pub(super) fn chosen() -> bool {
    std::env::var("NEXUS_WEB_ENGINE").is_ok_and(|engine| engine == "servo")
}

/// One page in a `nexus-servo-host` process.
pub(super) struct ServoPage {
    child: Child,
    input: Mutex<ChildStdin>,
}

impl ServoPage {
    pub(super) fn start(url: &str, size: (i32, i32), scale: f32, shared: SharedRef) -> Result<ServoPage, String> {
        let host = host_binary().ok_or("nexus-servo-host is not built (cargo build --release in servo-host/)")?;
        let frames = frames_path();
        let mut child = Command::new(&host)
            .args([
                url.to_string(),
                frames.to_string_lossy().into_owned(),
                size.0.to_string(),
                size.1.to_string(),
                scale.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("{}: {error}", host.display()))?;
        let input = child.stdin.take().expect("piped stdin");
        let output = child.stdout.take().expect("piped stdout");
        std::thread::Builder::new()
            .name("servo-events".into())
            .spawn(move || read_events(output, shared))
            .map_err(|error| error.to_string())?;
        Ok(ServoPage {
            child,
            input: Mutex::new(input),
        })
    }

    pub(super) fn send(&self, command: Value) {
        let mut input = self.input.lock().expect("servo input");
        let _ = writeln!(input, "{command}");
        let _ = input.flush();
    }

    pub(super) fn resize(&self, size: (i32, i32), scale: f32) {
        self.send(json!({ "cmd": "resize", "width": size.0, "height": size.1, "scale": scale }));
    }
}

impl Drop for ServoPage {
    fn drop(&mut self) {
        // Closing its input ends the process; kill it if it hangs on.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Frame events become the page's frame, title and cursor.
fn read_events(output: std::process::ChildStdout, shared: SharedRef) {
    for line in BufReader::new(output).lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match event["event"].as_str() {
            Some("frame") => {
                let (Some(path), Some(width), Some(height)) = (
                    event["path"].as_str(),
                    event["width"].as_u64(),
                    event["height"].as_u64(),
                ) else {
                    continue;
                };
                let Ok(pixels) = std::fs::read(path) else {
                    continue;
                };
                if pixels.len() != (width * height * 4) as usize {
                    continue;
                }
                let mut shared = shared.lock().expect("web view state");
                shared.frame = Some(Frame {
                    pixels,
                    width: width as u32,
                    height: height as u32,
                    seq: 0,
                    changed: None,
                });
                shared.changed();
            }
            Some("title") => {
                let mut shared = shared.lock().expect("web view state");
                shared.title = event["title"]
                    .as_str()
                    .map(str::to_string)
                    .filter(|title| !title.is_empty());
                shared.changed();
            }
            Some("cursor") => {
                let mut shared = shared.lock().expect("web view state");
                shared.cursor = match event["cursor"].as_str() {
                    Some("pointer") => Cursor::Hand,
                    Some("text") => Cursor::Text,
                    _ => Cursor::Arrow,
                };
                shared.changed();
            }
            _ => {}
        }
    }
}

/// `nexus-servo-host`: `NEXUS_SERVO_HOST`, else beside this binary, else the development build in `servo-host/`.
fn host_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NEXUS_SERVO_HOST") {
        return Some(PathBuf::from(path));
    }
    let beside = std::env::current_exe().ok()?.parent()?.join("nexus-servo-host");
    if beside.exists() {
        return Some(beside);
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("servo-host/target/release/nexus-servo-host");
    dev.exists().then_some(dev)
}

/// Where the host writes frames: in memory (`/dev/shm`) where the system has it.
fn frames_path() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = PathBuf::from("/dev/shm");
    let dir = if dir.is_dir() { dir } else { std::env::temp_dir() };
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dir.join(format!("nexus-servo-{}-{n}", std::process::id()))
}
