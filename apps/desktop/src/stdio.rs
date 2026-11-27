//! stdout and stderr that never fail. Rust ignores SIGPIPE, so a print to a closed pipe returns an error,
//! and `println!`/`eprintln!` panic on it: "failed printing to stderr: Broken pipe". A closed terminal gives EIO and the
//! same panic. The app loses its stderr while it runs when whatever started it goes away: a terminal that closes, a
//! `| tee` that ends, or the copy that relaunched it after an update (`updates/install.rs` inherits its stdio).
//!
//! [`guard`] points fds 1 and 2 at pipes this process reads itself. A print then always has a reader, so it never
//! fails, at any of the app's print sites or its dependencies'. A thread forwards each pipe to the real stream, and only
//! that thread sees the real stream's errors.

use std::fs::File;
use std::io::{PipeReader, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};

/// In the app's own process only, after the subprocess dispatch in `main`: Chromium's helpers, the web host and the
/// MCP server use stdout as a protocol and keep their own. A stream that cannot be guarded is left as it was.
pub fn guard() {
    guard_one(std::io::stdout().as_fd(), "stdout");
    guard_one(std::io::stderr().as_fd(), "stderr");
}

fn guard_one(stream: BorrowedFd<'_>, name: &str) {
    let Ok(real) = stream.try_clone_to_owned() else { return };
    let Ok((reader, writer)) = std::io::pipe() else { return };
    // The reader first: a stream pointed at a pipe nobody reads would fail every print.
    let forward = std::thread::Builder::new()
        .name(format!("{name}-forward"))
        .spawn(move || drain(reader, File::from(real)));
    if forward.is_err() {
        return;
    }
    // SAFETY: dup2 on this process's own descriptors. Both stay open: `writer` until after the call, `stream` for good.
    // On failure the stream is unchanged, and dropping `writer` ends the thread.
    unsafe { libc::dup2(writer.as_raw_fd(), stream.as_raw_fd()) };
}

/// Copies what the app prints to the real stream, until the app exits. Never panics and never stops reading: a reader
/// that stopped would bring the broken pipe back to every print.
fn drain(from: PipeReader, to: File) {
    forward(from, to, std::thread::sleep);
}

/// The longest wait between two reads after a read error.
const MAX_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

/// [`drain`]'s loop, over any reader and writer. Only the end of the pipe (a read of 0) stops it. Any other read error
/// is retried after a pause that doubles up to [`MAX_RETRY`], so a pipe that keeps failing costs at most one read a
/// second, and the app's prints still have a reader.
fn forward(mut from: impl Read, mut to: impl Write, mut pause: impl FnMut(std::time::Duration)) {
    // Once the real stream fails it stays gone: later output is read and dropped, with no write per print.
    let mut open = true;
    let mut retry = std::time::Duration::from_millis(10);
    let mut buf = [0_u8; 8192];
    loop {
        match from.read(&mut buf) {
            Ok(0) => return,
            Ok(read) => {
                open = open && to.write_all(&buf[..read]).is_ok();
                retry = std::time::Duration::from_millis(10);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                pause(retry);
                retry = (retry * 2).min(MAX_RETRY);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    const CHILD: &str = "NEXUS_STDIO_TEST_CHILD";

    /// The child half: run by [`a_closed_stderr_does_not_panic`] as its own process. It prints on after its reader
    /// is gone. `NEXUS_STDIO_TEST_CHILD=guarded` calls [`super::guard`] first; `bare` does not.
    #[test]
    #[ignore = "run as a child process by a_closed_stderr_does_not_panic"]
    fn child_prints_after_its_reader_is_gone() {
        let Ok(mode) = std::env::var(CHILD) else { return };
        if mode == "guarded" {
            super::guard();
        }
        for n in 0..500 {
            eprintln!("line {n}");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Runs the child with stderr on a pipe, reads one line, then closes the read end.
    fn run_child(mode: &str) -> bool {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "stdio::tests::child_prints_after_its_reader_is_gone",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, mode)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut first = String::new();
        BufReader::new(child.stderr.take().unwrap())
            .read_line(&mut first)
            .unwrap();
        child.wait().unwrap().success()
    }

    /// Reads `steps` in order: `Some(bytes)` returns them, `None` fails with an error that is not `Interrupted`. Then
    /// the end of the pipe.
    struct Script(std::collections::VecDeque<Option<&'static [u8]>>);

    impl std::io::Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.pop_front() {
                None => Ok(0),
                Some(None) => Err(std::io::Error::other("read failed")),
                Some(Some(bytes)) => {
                    buf[..bytes.len()].copy_from_slice(bytes);
                    Ok(bytes.len())
                }
            }
        }
    }

    #[test]
    fn a_read_error_does_not_stop_the_forwarder() {
        let steps = [Some(&b"one "[..]), None, None, None, Some(&b"two"[..])];
        let mut out = Vec::new();
        let mut pauses = Vec::new();
        super::forward(Script(steps.into_iter().collect()), &mut out, |wait| pauses.push(wait));
        assert_eq!(out, b"one two", "the output after the errors was lost");
        let ms = |n| std::time::Duration::from_millis(n);
        assert_eq!(
            pauses,
            [ms(10), ms(20), ms(40)],
            "each retry waits twice as long as the last"
        );
    }

    #[test]
    fn retries_wait_at_most_a_second() {
        let steps: Vec<_> = std::iter::repeat_n(None, 12).collect();
        let mut pauses = Vec::new();
        super::forward(Script(steps.into_iter().collect()), std::io::sink(), |wait| {
            pauses.push(wait)
        });
        assert_eq!(pauses.len(), 12);
        assert_eq!(pauses.iter().max(), Some(&super::MAX_RETRY));
    }

    #[test]
    fn a_closed_stderr_does_not_panic() {
        // The control first: without the guard the child must die, or this test proves nothing.
        assert!(
            !run_child("bare"),
            "the bare child survived a closed stderr: the test does not reach the bug"
        );
        assert!(run_child("guarded"), "the guarded child panicked on a closed stderr");
    }
}
