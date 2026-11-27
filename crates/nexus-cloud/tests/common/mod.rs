//! What the tests share: throwaway folders, stand-ins for the system tools the provisioning calls (`sudo`,
//! `loginctl`, `systemctl`, `npm`), and a throwaway user-level sshd.
#![allow(dead_code)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

/// A fresh folder under the temp dir, removed when dropped.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Scratch {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("nexus-cloud-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        Scratch(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes an executable script with a child `sh`, so this process never holds it open while it runs (ETXTBSY).
pub fn script(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let written = nexus_workbench::process::command("sh")
        .arg("-c")
        .arg(r#"printf '#!/bin/sh\n%s\n' "$2" > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .arg(body)
        .status();
    assert!(written.is_ok_and(|s| s.success()), "could not write {}", path.display());
}

/// `bin/` with logging stand-ins for the tools that would touch this computer for real: `sudo` (only the
/// `loginctl enable-linger` the provisioning asks for), `loginctl` (linger off), `systemctl` (user units), `npm`
/// (nothing installed until `install -g`). Each appends its arguments to `<dir>/calls.log`.
pub fn stubs(dir: &Path) -> PathBuf {
    let bin = dir.join("bin");
    let log = dir.join("calls.log");
    let log = log.display();
    script(&bin.join("sudo"), &format!("echo \"sudo $*\" >> '{log}'"));
    script(
        &bin.join("loginctl"),
        &format!("echo \"loginctl $*\" >> '{log}'; echo no"),
    );
    script(&bin.join("systemctl"), &format!("echo \"systemctl $*\" >> '{log}'"));
    script(
        &bin.join("npm"),
        &format!(
            "echo \"npm $*\" >> '{log}'\ncase \"$1\" in ls) [ -f '{dir}/npm-installed' ] ;; install) touch '{dir}/npm-installed' ;; esac",
            dir = dir.display()
        ),
    );
    // `gh`: its token in `gh-token` (written by `auth login --with-token` from standard input).
    script(
        &bin.join("gh"),
        &format!(
            "echo \"gh $*\" >> '{log}'\ncase \"$1 $2\" in \"auth token\") cat '{dir}/gh-token' 2>/dev/null | tr -d '\\n' ;; \"auth login\") cat > '{dir}/gh-token' ;; esac",
            dir = dir.display()
        ),
    );
    bin
}

pub fn calls(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default()
}

/// A free port on 127.0.0.1 (free a moment ago).
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or_else(|e| panic!("{e}"))
}

/// A user-level sshd on 127.0.0.1, its own host key, logins with the keys in `authorized_keys` only, the session's
/// `HOME` and `PATH` set to the test's (sshd's `SetEnv`), so nothing touches the real home. Killed when dropped.
pub struct Sshd {
    pub port: u16,
    pub host_key: String,
    pub authorized_keys: PathBuf,
    pub user: String,
    child: Child,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub const SSHD: &str = "/usr/sbin/sshd";

/// None (with the reason printed) when this computer has no sshd or ssh-keygen to run one.
pub fn sshd(dir: &Path, home: &Path, path: &str) -> Option<Sshd> {
    if !Path::new(SSHD).exists() {
        eprintln!("skipped: {SSHD} is not installed");
        return None;
    }
    let host = dir.join("host_ed25519");
    let made = nexus_workbench::process::command("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "test-host", "-f"])
        .arg(&host)
        .stdin(Stdio::null())
        .status();
    if !made.is_ok_and(|s| s.success()) {
        eprintln!("skipped: ssh-keygen could not make a host key");
        return None;
    }
    let host_key = std::fs::read_to_string(dir.join("host_ed25519.pub")).unwrap_or_default();
    let authorized_keys = dir.join("authorized_keys");
    let _ = std::fs::write(&authorized_keys, "");
    let port = free_port();
    let config = dir.join("sshd_config");
    let _ = std::fs::write(
        &config,
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {host}\nPidFile {pid}\nAuthorizedKeysFile {keys}\n\
             StrictModes no\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n\
             PubkeyAuthentication yes\nSetEnv HOME={home} PATH={path}\n",
            host = host.display(),
            pid = dir.join("sshd.pid").display(),
            keys = authorized_keys.display(),
            home = home.display(),
        ),
    );
    let log = std::fs::File::create(dir.join("sshd.log")).ok()?;
    let child = nexus_workbench::process::command(SSHD)
        .arg("-D")
        .arg("-e")
        .arg("-f")
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(e) => {
            eprintln!("skipped: could not start sshd: {e}");
            return None;
        }
    };
    let user = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| {
            nexus_workbench::process::command("id")
                .arg("-un")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .unwrap_or_default();
    let sshd = Sshd {
        port,
        host_key: host_key.trim().to_string(),
        authorized_keys,
        user,
        child,
    };
    let started = Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        if started.elapsed() > Duration::from_secs(5) {
            eprintln!(
                "skipped: sshd did not listen: {}",
                std::fs::read_to_string(dir.join("sshd.log")).unwrap_or_default()
            );
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Some(sshd)
}
