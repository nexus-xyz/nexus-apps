//! The outside access socket: a Unix socket, 0600 in a 0700 folder, so only this person's processes can connect. One
//! JSON line in and one out per connection ([`super::Outside::handle`]).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::Value;

use super::{Host, Outside};

/// The largest request line.
const LIMIT: u64 = 256 * 1024;
/// Connections served at once.
const MAX_CONNECTIONS: usize = 8;

/// The socket's path: in the person's runtime folder when the system has one (Linux), else in the app's data folder.
/// The background service's socket lives beside it. The app's identifier keeps a development build apart
/// from the released app.
pub fn path(identifier: &str, data: &Path) -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute() && dir.is_dir());
    match runtime {
        Some(dir) => dir.join(identifier).join("sessions.sock"),
        None => data.join("run").join("sessions.sock"),
    }
}

/// A listening socket. Dropping it stops the server and removes the socket file.
pub struct Listening {
    path: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Binds `path` (its folder made 0700, the socket 0600) and serves it on its own thread.
pub fn listen(path: &Path, outside: Arc<Outside>, host: Arc<dyn Host>) -> Result<Listening, String> {
    let dir = path.parent().ok_or("The socket has no folder.")?;
    std::fs::create_dir_all(dir).map_err(|error| format!("Could not create {}: {error}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Could not make {} private: {error}", dir.display()))?;
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err("Another Nexus already offers outside access on this computer.".into());
        }
        std::fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    let listener =
        UnixListener::bind(path).map_err(|error| format!("Could not listen on {}: {error}", path.display()))?;
    // The folder is 0700 already, so nobody else reached the socket before this.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| error.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::spawn(move || serve(listener, outside, host, stopped));
    Ok(Listening {
        path: path.to_path_buf(),
        stop,
    })
}

fn serve(listener: UnixListener, outside: Arc<Outside>, host: Arc<dyn Host>, stop: Arc<AtomicBool>) {
    let active = Arc::new(AtomicUsize::new(0));
    for connection in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = connection else {
            // Out of file descriptors, say: accept fails at once again, so wait instead of spinning a core.
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let (outside, host, active) = (outside.clone(), host.clone(), active.clone());
        std::thread::spawn(move || {
            let _ = answer(&outside, &*host, stream);
            active.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

fn answer(outside: &Outside, host: &dyn Host, mut stream: UnixStream) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(|error| error.to_string())?;
    let mut line = String::new();
    let count = BufReader::new((&stream).take(LIMIT + 1))
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    if count as u64 > LIMIT || !line.ends_with('\n') {
        return Err("Request too large.".into());
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(request) => outside.handle(host, &request),
        Err(_) => super::error("bad_request", "Send one JSON object on one line."),
    };
    writeln!(stream, "{reply}").map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::grants::{Grants, Scope};
    use super::super::tests::{Fake, call};
    use super::*;
    use serde_json::json;

    fn send(path: &Path, request: &Value) -> Value {
        let mut stream = UnixStream::connect(path).expect("connect");
        writeln!(stream, "{request}").expect("write");
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).expect("read");
        serde_json::from_str(&line).expect("json")
    }

    #[test]
    fn the_socket_is_private_answers_lines_and_goes_away_when_dropped() {
        let dir = std::env::temp_dir().join(format!("nexus-outside-socket-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("run").join("sessions.sock");
        let mut grants = Grants::default();
        let token = grants.add("test", Scope::Read).expect("add");
        let outside = Arc::new(Outside::new(grants));
        let fake = Arc::new(Fake::default());
        let listening = listen(&path, outside, fake.clone()).expect("listen");
        let mode = |path: &Path| std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().expect("dir")), 0o700);

        assert_eq!(send(&path, &call("", tools_list()))["error"]["code"], -32001);
        assert_eq!(send(&path, &call(&token, tools_list()))["result"]["isError"], false);
        assert_eq!(send(&path, &json!("not an object"))["error"]["code"], -32001);
        // A second app cannot take the socket over.
        assert!(listen(&path, Arc::new(Outside::new(Grants::default())), fake).is_err());

        drop(listening);
        assert!(!path.exists());
        assert!(UnixStream::connect(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tools_list() -> &'static str {
        super::super::tools::LIST
    }
}
