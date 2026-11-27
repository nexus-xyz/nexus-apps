//! On a cloud machine, a repository's `.agents/setup` runs as its own unprivileged OS user, [`USER`], not
//! as the host's: repository code at boot must not reach the host's folder (`~/.nexus/host`: its token, `cloud.json`,
//! `agent.env`, the setup's state), the person's credentials, or anything it could leave running to catch them later.
//!
//! The machine's user may use `sudo` without a password (the image installs with it). Around each run, the host:
//! 1. makes the user when the machine has none ([`ENSURE`]: a system user, no login shell, its own home [`HOME`], no
//!    cron or at jobs);
//! 2. closes its own home to other users except to pass through it (`o=x`: no listing, no reading of what is private,
//!    which is everything the host and the machine's preparation write: they run with umask 077) and makes `~/.nexus`
//!    0700;
//! 3. hands the clone to the user (`chown -hR`), so the setup can install into it;
//! 4. runs the script through `sudo setpriv` as the user, with no new privileges (no sudo, no setuid program), its own
//!    home and a short list of variables ([`command`]);
//! 5. ends every process of the user ([`end_all`]): a `setsid`, a double fork or a dropped mark escapes a process group,
//!    not a user;
//! 6. gives the clone back to the host's user ([`take_back`]), before git reads its config as the host.
//!
//! The host's state (`setup.json`) and the files the preparation waits on stay in `~/.nexus/host`, which the user can
//! neither read nor write.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::{git, process};

/// The OS user `.agents/setup` runs as.
pub const USER: &str = "nexus-setup";

/// Its home folder: its caches and tools. Readable by all (0755), so what the setup made point there (a virtual
/// environment's Python) works for the agent too.
pub const HOME: &str = "/var/lib/nexus-setup";

/// The `PATH` `sudo` and the commands it runs are found on.
const SYSTEM_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Makes the user once, and its home, as root (`sh -c ENSURE sh <user> <home>`). It may run on every setup: a user
/// that exists is kept, and one made meanwhile by another run is taken.
const ENSURE: &str = r#"set -eu
user="$1"; home="$2"
if ! id -u "$user" >/dev/null 2>&1; then
  useradd --system --user-group --home-dir "$home" --shell /usr/sbin/nologin "$user" || id -u "$user" >/dev/null
fi
mkdir -p "$home"
chown "$user:$user" "$home"
chmod 755 "$home"
for deny in /etc/cron.deny /etc/at.deny; do
  grep -qx "$user" "$deny" 2>/dev/null || echo "$user" >> "$deny"
done
"#;

/// Runs `args` as root through `sudo -n`, and answers why it failed.
fn sudo(args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let mut command = process::command("sudo");
    command
        .arg("-n")
        .args(args)
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .stdin(std::process::Stdio::null());
    let out = command
        .output()
        .map_err(|error| format!("could not run sudo: {error}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let said = String::from_utf8_lossy(&out.stderr);
        Err(format!(
            "sudo {} failed: {}",
            args.first().map(|a| a.to_string_lossy()).unwrap_or_default(),
            said.trim().chars().take(200).collect::<String>()
        ))
    }
}

/// Steps 1 to 3: the user ready, the host's home closed to it, and `dest` (the clone, inside `home`) handed to it.
pub(crate) fn hand_over(home: &Path, dest: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    sudo(&[
        "sh".as_ref(),
        "-c".as_ref(),
        ENSURE.as_ref(),
        "sh".as_ref(),
        USER.as_ref(),
        HOME.as_ref(),
    ])?;
    let mode = std::fs::metadata(home)
        .map_err(|error| format!("could not read {}: {error}", home.display()))?
        .permissions()
        .mode();
    let closed = (mode & 0o7770) | 0o001;
    if mode & 0o7777 != closed {
        std::fs::set_permissions(home, std::fs::Permissions::from_mode(closed))
            .map_err(|error| format!("could not close {} to other users: {error}", home.display()))?;
    }
    let nexus = home.join(".nexus");
    if std::fs::symlink_metadata(&nexus).is_ok_and(|meta| meta.is_dir()) {
        std::fs::set_permissions(&nexus, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("could not close {}: {error}", nexus.display()))?;
    }
    let owner = format!("{USER}:{USER}");
    sudo(&[
        "chown".as_ref(),
        "-hR".as_ref(),
        owner.as_ref(),
        "--".as_ref(),
        dest.as_os_str(),
    ])
}

/// Step 4: `script` run from `dest` as the user, with `path` after the user's own `~/.local/bin`, its home, and the
/// run's mark. Nothing else of the host's environment.
fn command(script: &Path, dest: &Path, path: &str, run_var: &str, run_id: &str) -> std::process::Command {
    let mut command = process::command("sudo");
    command
        .current_dir(dest)
        .env_clear()
        .env("PATH", SYSTEM_PATH)
        .args(["-n", "setpriv"])
        .arg(format!("--reuid={USER}"))
        .arg(format!("--regid={USER}"))
        .args(["--clear-groups", "--no-new-privs", "--", "env", "-i"])
        .arg(format!("PATH={HOME}/.local/bin:{path}"))
        .arg(format!("HOME={HOME}"))
        .arg(format!("USER={USER}"))
        .arg(format!("LOGNAME={USER}"))
        .arg("LANG=C.UTF-8")
        .arg(format!("{run_var}={run_id}"))
        .arg(script);
    command
}

/// Runs `script` as the user for at most `limit` (steps 4 and 5): None when it was stopped at the limit. The host may
/// not signal the user's processes, so at the limit they all end through [`end_all`], and every run ends with it.
pub(crate) fn run(
    script: &Path,
    dest: &Path,
    path: &str,
    run_var: &str,
    run_id: &str,
    limit: Duration,
) -> std::io::Result<Option<git::Ran>> {
    let stopped = Arc::new(AtomicBool::new(false));
    let (finished, wait) = std::sync::mpsc::channel::<()>();
    let watchdog = {
        let stopped = stopped.clone();
        std::thread::spawn(move || {
            if wait.recv_timeout(limit) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                stopped.store(true, Ordering::SeqCst);
                if let Err(why) = end_all() {
                    eprintln!("nexus-workbench: could not stop .agents/setup: {why}");
                }
            }
        })
    };
    // The backstop only matters when the user's processes could not be ended.
    let ran = git::output_ending_all(
        command(script, dest, path, run_var, run_id),
        limit + Duration::from_secs(60),
    );
    drop(finished);
    let _ = watchdog.join();
    let ended = end_all();
    let ran = ran?;
    ended.map_err(std::io::Error::other)?;
    Ok(if stopped.load(Ordering::SeqCst) { None } else { ran })
}

/// Step 5: ends every process of the user, and answers once none is left.
pub(crate) fn end_all() -> Result<(), String> {
    let Some(uid) = uid_of(USER) else {
        return Ok(());
    };
    for _ in 0..20 {
        // `kill -9 -1` as the user reaches each of its processes but the shell that sends it.
        sudo(&[
            "setpriv".as_ref(),
            format!("--reuid={USER}").as_ref(),
            format!("--regid={USER}").as_ref(),
            "--clear-groups".as_ref(),
            "--".as_ref(),
            "sh".as_ref(),
            "-c".as_ref(),
            "kill -9 -1 2>/dev/null; exit 0".as_ref(),
        ])?;
        if processes_of(uid) == 0 {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!("processes of {USER} are still running"))
}

/// Step 6: gives `dest` back to the host's user, links included (never what they point at).
pub(crate) fn take_back(dest: &Path) -> Result<(), String> {
    let owner = format!("{}:{}", nix::unistd::getuid(), nix::unistd::getgid());
    sudo(&[
        "chown".as_ref(),
        "-hR".as_ref(),
        owner.as_ref(),
        "--".as_ref(),
        dest.as_os_str(),
    ])
}

fn uid_of(user: &str) -> Option<u32> {
    nix::unistd::User::from_name(user)
        .ok()
        .flatten()
        .map(|user| user.uid.as_raw())
}

/// How many live processes (not zombies) run as `uid`.
pub(crate) fn processes_of(uid: u32) -> usize {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.parse::<u32>().is_ok())
        })
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("status")).ok())
        .filter(|status| {
            let field = |name: &str| status.lines().find_map(|line| line.strip_prefix(name)).map(str::trim);
            let live = field("State:").is_some_and(|state| !state.starts_with('Z') && !state.starts_with('X'));
            let owned = field("Uid:")
                .and_then(|ids| ids.split_whitespace().next())
                .is_some_and(|real| real.parse::<u32>().ok() == Some(uid));
            live && owned
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    /// Whether this machine lets the tests be root without a password (CI, a root container): only then can they make
    /// the user. Elsewhere they say so and pass.
    fn can_sudo() -> bool {
        let ok = process::command("sudo")
            .args(["-n", "true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !ok {
            eprintln!("skipped: no passwordless sudo to make {USER}");
        }
        ok
    }

    fn write(path: &Path, text: &str, mode: u32) {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).unwrap_or_else(|e| panic!("{e}"));
        }
        std::fs::write(path, text).unwrap_or_else(|e| panic!("{e}"));
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap_or_else(|e| panic!("{e}"));
    }

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap_or_else(|e| panic!("{e}"));
    }

    /// One test, so no other run of the user's ends while it ends them all.
    #[test]
    fn agents_setup_runs_as_its_own_user_and_reaches_no_secret_of_the_host() {
        if !can_sudo() {
            return;
        }
        let root = std::env::temp_dir().join(format!("nexus-setup-user-{}", uuid::Uuid::new_v4().simple()));
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap_or_else(|e| panic!("{e}"));
        chmod(&root, 0o755);
        // A home open to all, and a `~/.nexus` too, as a machine may have them: the run closes both.
        chmod(&home, 0o755);
        write(
            &home.join(".nexus/host/agent.env"),
            "CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat-secret\n",
            0o600,
        );
        write(
            &home.join(".nexus/host/cloud.json"),
            "{\"token\":\"nx-machine-secret\"}\n",
            0o600,
        );
        write(&home.join(".nexus/host/setup.json"), "{\"state\":\"setup\"}\n", 0o600);
        write(&home.join(".nexus/agents-setup.log"), "log-secret\n", 0o644);
        chmod(&home.join(".nexus"), 0o755);
        chmod(&home.join(".nexus/host"), 0o700);
        write(&home.join(".config/gh/hosts.yml"), "oauth_token: gho_secret\n", 0o600);
        chmod(&home.join(".config/gh"), 0o700);
        write(
            &home.join(".git-credentials"),
            "https://x:ghp_secret@github.com\n",
            0o600,
        );
        let repo = home.join("web");
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
        let init = process::command("git")
            .args(["init", "--quiet"])
            .current_dir(&repo)
            .status()
            .is_ok_and(|status| status.success());
        assert!(init, "git init");
        let h = home.display();
        // It reads what it can of the host's, writes where it should not, installs into the clone, and leaves a
        // `setsid` child that keeps reading `agent.env` (the person's tokens arrive after the setup).
        let script = format!(
            r#"#!/bin/sh
H='{h}'
{{ id -un; cat "$H/.nexus/host/agent.env"; cat "$H/.nexus/host/cloud.json"; cat "$H/.nexus/agents-setup.log"; ls "$H/.nexus"; ls "$H"; cat "$H/.config/gh/hosts.yml"; cat "$H/.git-credentials"; echo ENV; env; }} > seen.txt 2>&1
echo planted > "$H/.nexus/host/setup.json" 2>/dev/null && echo wrote-state >> seen.txt
echo planted >> "$H/.profile" 2>/dev/null && echo wrote-profile >> seen.txt
mkdir -p node_modules/dep && echo x > node_modules/dep/index.js
export H
setsid sh -c 'while :; do cat "$H/.nexus/host/agent.env" >> leak.txt 2>&1; sleep 0.1; done' >/dev/null 2>&1 &
env -u NEXUS_SETUP_RUN sh -c 'sleep 33' >/dev/null 2>&1 &
"#
        );
        write(&repo.join(".agents/setup"), &script, 0o755);
        let path = "/usr/local/bin:/usr/bin:/bin";
        crate::clone::run_agents_setup_as_user(&repo, &home, path, Duration::from_secs(30))
            .unwrap_or_else(|e| panic!("{e}"));

        let seen = std::fs::read_to_string(repo.join("seen.txt")).unwrap_or_default();
        assert_eq!(seen.lines().next(), Some(USER), "{seen}");
        for secret in [
            "sk-ant-oat-secret",
            "nx-machine-secret",
            "gho_secret",
            "ghp_secret",
            "log-secret",
        ] {
            assert!(!seen.contains(secret), "the setup read {secret}: {seen}");
        }
        assert!(!seen.contains("wrote-"), "the setup wrote into the host's home: {seen}");
        let env: Vec<&str> = seen.split("ENV\n").nth(1).unwrap_or_default().lines().collect();
        let names: Vec<&str> = env
            .iter()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            .collect();
        for name in &names {
            assert!(
                [
                    "PATH",
                    "HOME",
                    "USER",
                    "LOGNAME",
                    "LANG",
                    "NEXUS_SETUP_RUN",
                    "PWD",
                    "SHLVL",
                    "_",
                    "OLDPWD"
                ]
                .contains(name),
                "{name} reached the setup: {env:?}"
            );
        }
        assert!(env.contains(&format!("HOME={HOME}").as_str()), "{env:?}");
        assert_eq!(
            std::fs::read_to_string(home.join(".nexus/host/setup.json")).unwrap_or_default(),
            "{\"state\":\"setup\"}\n"
        );
        assert!(!home.join(".profile").exists());

        // Nothing of the user's runs on: not the `setsid` loop, not the child that dropped the run's mark.
        let uid = uid_of(USER).unwrap_or_else(|| panic!("no {USER}"));
        assert_eq!(processes_of(uid), 0, "a process of {USER} outlived the setup");
        // The person's tokens arrive: nothing is left to read them.
        write(
            &home.join(".nexus/host/agent.env"),
            "CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat-later\n",
            0o600,
        );
        std::thread::sleep(Duration::from_millis(500));
        let leak = std::fs::read_to_string(repo.join("leak.txt")).unwrap_or_default();
        assert!(!leak.contains("sk-ant-oat"), "{leak}");

        // The clone, and what the setup put in it, is the host's again; the host's home is closed to others.
        let me = nix::unistd::getuid().as_raw();
        for file in ["", ".git", "seen.txt", "node_modules/dep/index.js", ".agents/setup"] {
            let meta = std::fs::symlink_metadata(repo.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert_eq!(meta.uid(), me, "{file} is not the host's");
        }
        let mode = |path: &Path| {
            std::fs::metadata(path)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0)
        };
        assert_eq!(mode(&home), 0o751);
        assert_eq!(mode(&home.join(".nexus")), 0o700);
        assert_eq!(mode(Path::new(HOME)), 0o755);

        // A slow one is stopped at its limit, with everything it started.
        write(
            &repo.join(".agents/setup"),
            "#!/bin/sh\nsetsid sleep 34 &\nsleep 30\n",
            0o755,
        );
        let started = std::time::Instant::now();
        let why = crate::clone::run_agents_setup_as_user(&repo, &home, path, Duration::from_secs(1))
            .err()
            .unwrap_or_default();
        assert!(why.contains("did not finish"), "{why}");
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
        assert_eq!(processes_of(uid), 0, "a process of {USER} outlived the limit");
        assert_eq!(
            std::fs::symlink_metadata(&repo).map(|m| m.uid()).ok(),
            Some(me),
            "the clone is the host's again"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
