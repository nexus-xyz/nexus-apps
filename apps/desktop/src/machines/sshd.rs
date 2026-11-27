//! [`Machines`] over real SSH to a throwaway `sshd` on 127.0.0.1: its own folder, host key and authorized key, made
//! for the test, and no person's ssh config or known hosts. It installs a local `nexus-host` build there, starts it,
//! connects through `nexus-host proxy`, reads the overview, survives a dropped connection, updates the host, and
//! closes. The host's files go in the test's folder (`NEXUS_HOST_DIR`, set by the sshd).
//!
//! Ignored by default: it needs OpenSSH's `sshd` and a built host. Run it with
//!
//!   NEXUS_HOST_BIN=<nexus-host> cargo test --bin nexus-desktop-gpui machines::sshd -- --ignored

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use nexus_remote::install::Installer;
use nexus_remote::machine::{Backoff, Link, SshLink, Status};
use nexus_remote::ssh::Target;

use super::*;

/// The test's sshd, stopped (by its pid) when dropped.
struct Sshd {
    child: Child,
    dir: PathBuf,
    port: u16,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn keygen(path: &Path) {
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(path)
        .status()
        .expect("ssh-keygen runs");
    assert!(made.success(), "ssh-keygen {}", path.display());
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .expect("a free port")
}

fn start_sshd(dir: &Path) -> Sshd {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir.join("host")).expect("folder");
    // The session's home (`SetEnv HOME` below): nothing there reads or writes the person's own.
    std::fs::create_dir_all(dir.join("home")).expect("home");
    keygen(&dir.join("host_key"));
    keygen(&dir.join("client_key"));
    std::fs::copy(dir.join("client_key.pub"), dir.join("authorized_keys")).expect("authorized key");
    let port = free_port();
    let user = std::env::var("USER").expect("USER");
    let config = format!(
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {dir}/host_key\nAuthorizedKeysFile {dir}/authorized_keys\n\
         PidFile {dir}/sshd.pid\nStrictModes no\nUsePAM no\nPasswordAuthentication no\n\
         KbdInteractiveAuthentication no\nPubkeyAuthentication yes\nAllowUsers {user}\n\
         SetEnv NEXUS_HOST_DIR={dir}/host HOME={dir}/home\n",
        dir = dir.display()
    );
    std::fs::write(dir.join("sshd_config"), config).expect("config");
    let sshd = ["/usr/sbin/sshd", "/usr/bin/sshd"]
        .into_iter()
        .find(|path| Path::new(path).exists())
        .expect("OpenSSH's sshd");
    let child = Command::new(sshd)
        .args(["-D", "-e", "-f"])
        .arg(dir.join("sshd_config"))
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(dir.join("sshd.log")).expect("log"))
        .spawn()
        .expect("sshd starts");
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Sshd {
        child,
        dir: dir.to_path_buf(),
        port,
    }
}

impl Sshd {
    /// How the app reaches it: an explicit address, key and known host, nothing from the person's ssh config.
    fn installer(&self, host: &Path) -> Installer {
        let key = std::fs::read_to_string(self.dir.join("host_key.pub")).expect("host key");
        let key = key.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        let known_hosts = self.dir.join("known_hosts");
        // ssh looks an alias up without a port, as nexus-cloud pins it.
        nexus_remote::ssh::write_known_hosts(&known_hosts, "nexus-test-sshd", 22, &key).expect("known hosts");
        Installer {
            target: Target::Direct {
                host: "127.0.0.1".into(),
                port: self.port,
                user: std::env::var("USER").expect("USER"),
                identity: self.dir.join("client_key"),
                known_hosts,
                alias: "nexus-test-sshd".into(),
            },
            source: nexus_remote::install::Source::Local {
                path: host.to_path_buf(),
            },
            fetch: None,
        }
    }
}

/// Stops the host the test started there, by the pid its lock holds.
fn stop_host(dir: &Path) {
    let lock = dir.join("host").join("run").join("host.lock");
    if let Some(pid) = std::fs::read_to_string(lock)
        .ok()
        .and_then(|pid| pid.trim().parse::<i32>().ok())
    {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
}

async fn until(what: &str, seconds: u64, done: impl Fn() -> bool) {
    for _ in 0..seconds * 20 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

fn status(machines: &Machines) -> Status {
    machines
        .servers()
        .first()
        .map(|server| server.status.clone())
        .unwrap_or(Status::Closed)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs OpenSSH's sshd and NEXUS_HOST_BIN"]
async fn installs_reaches_and_updates_a_host_over_a_local_sshd() {
    let host = PathBuf::from(std::env::var("NEXUS_HOST_BIN").expect("NEXUS_HOST_BIN: a built nexus-host"));
    let dir = std::env::var_os("NEXUS_SSHD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("nexus-gpui-sshd-{}", std::process::id())));
    let sshd = start_sshd(&dir);
    let installer = sshd.installer(&host);
    let link: Arc<dyn Link> = Arc::new(SshLink {
        installer: installer.clone(),
    });
    let links: Links = Arc::new(move |_: &MachineSetting| link.clone());
    let backoff = Backoff {
        first: Duration::from_millis(200),
        max: Duration::from_secs(1),
        healthy: Duration::from_secs(60),
    };
    let machines = Machines::new(links, backoff, tokio::runtime::Handle::current());
    let settings = Settings {
        machines: vec![MachineSetting {
            id: "m1".into(),
            name: "sshd".into(),
            kind: "ssh".into(),
            // The links above reach it; the destination is only what the settings keep.
            ssh: "test-sshd".into(),
            cloud: None,
        }],
        ..Settings::default()
    };
    machines.sync(&settings);
    until("connected over ssh", 90, || {
        let now = status(&machines);
        if let Status::Unreachable { error, .. } = &now {
            eprintln!("unreachable: {error}");
        }
        matches!(now, Status::Connected { .. })
    })
    .await;
    let Status::Connected { host: info, update } = status(&machines) else {
        unreachable!()
    };
    println!(
        "connected: {} protocol {} on {}",
        info.version, info.protocol, info.machine
    );
    assert_eq!(update, None, "the host it installed is the version it asked for");
    let overview = machines.call("m1", "wb_overview", json!({})).await;
    assert!(overview.is_ok_and(|answer| answer.is_ok_and(|overview| overview.get("projects").is_some())));
    let status_call = machines.call("m1", "host_status", json!(null)).await;
    assert_eq!(
        status_call.ok(),
        Some(Err("host_status is not a workbench command.".to_string())),
        "only workbench calls go to a server"
    );

    // A folder there becomes one of its projects (`server_folder.rs`).
    let folder = sshd.dir.join("work").join("app");
    std::fs::create_dir_all(&folder).expect("a folder on the server");
    let added = machines
        .call("m1", "wb_project_add", json!({ "path": folder.to_string_lossy() }))
        .await;
    let project = added.ok().and_then(Result::ok).expect("the server adds the folder");
    assert_eq!(project["name"], "app");
    let id = project["id"].as_str().unwrap_or_default().to_string();
    assert_eq!(machines.machine_of(&id).as_deref(), Some("m1"));

    // A repository cloned there joins its projects (`server_folder.rs`): from a bare repository in the test's folder,
    // into the session's home (the sshd's `HOME`, in the test's folder too).
    let origin = sshd.dir.join("origin.git");
    let git = |args: &[&str], dir: &Path| {
        let ran = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(
            ran.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&ran.stderr)
        );
    };
    let seed = sshd.dir.join("seed");
    std::fs::create_dir_all(&seed).expect("seed folder");
    git(&["init", "-q", "-b", "main"], &seed);
    std::fs::write(seed.join("README.md"), "hello\n").expect("a file");
    git(&["add", "README.md"], &seed);
    git(
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "-m",
            "first",
        ],
        &seed,
    );
    git(
        &[
            "clone",
            "-q",
            "--bare",
            &seed.to_string_lossy(),
            &origin.to_string_lossy(),
        ],
        &sshd.dir,
    );
    let url = format!("file://{}", origin.display());
    let cloned = machines.clone_project("m1", &url, "~/code", "clone-1").await;
    let project = cloned.expect("the server clones the repository");
    assert_eq!(machines.machine_of(&project).as_deref(), Some("m1"));
    let landed = machines.projects_on("m1").into_iter().find(|known| known.id == project);
    let path = landed.map(|known| known.path).unwrap_or_default();
    assert_eq!(Path::new(&path), sshd.dir.join("home").join("code").join("origin"));
    assert!(
        sshd.dir.join("home/code/origin/README.md").exists(),
        "the clone is on the server"
    );
    // The same folder again: the server says why, in words.
    let again = machines.clone_project("m1", &url, "~/code", "clone-2").await;
    assert!(again.is_err(), "{again:?}");
    println!("cloned: {path}; again: {}", again.unwrap_err());

    // The worktree sweep: the host sweeps its own sessions and answers its report (`nexus_remote::cleanup`).
    let connected = machines.connected_now();
    assert_eq!(connected.len(), 1);
    let rules = nexus_workbench::cleanup::Rules::default();
    let swept = nexus_remote::cleanup::sweep("sshd", &connected[0].1, &rules).await;
    assert!(swept.is_ok_and(|report| report.removed.is_empty()));

    // A server's session's tools read there (`side/server.rs`): a git folder as a project, its files, then a session's
    // diff, a file and a commit.
    let repo = sshd.dir.join("work").join("tools");
    std::fs::create_dir_all(&repo).expect("a folder on the server");
    let git = |args: &[&str]| {
        Command::new("git")
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args(args)
            .current_dir(&repo)
            .env("HOME", &sshd.dir)
            .output()
            .expect("git runs")
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "hello\n").expect("a file");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "first"]);
    let project = machines
        .call("m1", "wb_project_add", json!({ "path": repo.to_string_lossy() }))
        .await
        .ok()
        .and_then(Result::ok)
        .expect("the server adds the folder");
    let files = machines
        .call("m1", "wb_files", json!({ "projectId": project["id"] }))
        .await
        .ok()
        .and_then(Result::ok)
        .expect("the server lists the files");
    assert_eq!(
        crate::side::server::files_of(&files).map(|(files, _)| files),
        Ok(vec!["README.md".to_string()])
    );
    let created = machines
        .call(
            "m1",
            "wb_thread_create",
            json!({ "projectId": project["id"], "agent": "claude", "text": "hi", "permissionMode": "default", "via": "nexus" }),
        )
        .await
        .ok()
        .and_then(Result::ok);
    // A session starts only where the host can make one (it is not signed in here); its tools are checked when it did.
    if let Some(thread) = created {
        let thread = thread["id"].clone();
        std::fs::write(repo.join("README.md"), "hello again\n").expect("a change");
        let diff = machines
            .call(
                "m1",
                "wb_diff",
                json!({ "threadId": thread, "source": { "kind": "uncommitted" } }),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .expect("the server's diff");
        let (patch, _) = crate::side::server::diff_of(&diff);
        assert_eq!(crate::side::server::status_of_diff(&patch).get("README.md"), Some(&'M'));
        let file = machines
            .call("m1", "wb_file_read", json!({ "threadId": thread, "path": "README.md" }))
            .await
            .ok()
            .and_then(Result::ok)
            .expect("the server reads the file");
        let file: nexus_workbench::project_files::FileText = serde_json::from_value(file).expect("a file");
        assert_eq!(file.text.as_deref(), Some("hello again\n"));
        let committed = machines
            .call(
                "m1",
                "wb_commit",
                json!({ "threadId": thread, "repo": null, "message": "second", "push": false }),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .map(serde_json::from_value::<nexus_workbench::GitState>);
        assert!(matches!(committed, Some(Ok(_))), "{committed:?}");
        println!("session tools checked on {thread}");
    } else {
        println!("no session: the host is not signed in");
    }

    // A lost network: Retry opens a new SSH connection.
    if let Ok(remote) = machines.remote("m1") {
        remote.drop_connection();
    }
    until("offline", 10, || {
        matches!(status(&machines), Status::Unreachable { .. })
    })
    .await;
    machines.reconnect("m1").expect("retry");
    until("reconnected", 60, || {
        matches!(status(&machines), Status::Connected { .. })
    })
    .await;

    // "Update now": the host stops, its database is copied aside, and the same build starts again.
    let updated = machines.update("m1").await;
    assert_eq!(updated.ok(), Some(Ok(())));
    until("connected after the update", 90, || {
        matches!(status(&machines), Status::Connected { .. })
    })
    .await;

    machines.shutdown();
    until("closed", 10, || status(&machines) == Status::Closed).await;
    stop_host(&sshd.dir);
    drop(sshd);
}
