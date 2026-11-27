//! `nexus-host service install | uninstall | status`: the host starts when the server boots.
//!
//! On Linux it is a systemd user unit in `~/.config/systemd/user/`, with linger on, so the person's user manager
//! starts it at boot and keeps it after they log out. On macOS it is a launchd LaunchAgent in
//! `~/Library/LaunchAgents/`. The default root (`~/.nexus/host`) uses `nexus-host.service` (`inc.nexus.host`); any other
//! `NEXUS_HOST_DIR` gets a name with a short hash of that root, so two roots never share a unit.
//!
//! The unit runs `<root>/current/nexus-host serve`. `current` is a symlink to a version's folder, and the app's
//! bootstrap (nexus-remote's install.rs) switches it before it restarts the unit, so an update never rewrites the unit.
//! `<root>/service` names the unit, so that bootstrap knows the service is there without asking this binary.

use std::path::{Path, PathBuf};
use std::time::Duration;

use nexus_workbench::process::{command, stdout_within};
use serde_json::{Value, json};

/// The symlink in the root that names the version the service runs.
pub const CURRENT: &str = "current";
/// The file in the root that names the installed unit (or launchd label).
pub const MARKER: &str = "service";
/// The first line of every unit this writes. A unit without it is someone else's (a cloud image's own
/// `nexus-host.service`, from the cloud image's install script), and `install` and `uninstall` leave it alone.
const HEADER: &str = "# Written by `nexus-host service install`. `nexus-host service uninstall` removes it.";
/// How long one `systemctl`, `loginctl` or `launchctl` call may take.
const CALL_LIMIT: Duration = Duration::from_secs(30);

/// One root's service.
#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    /// The host's root (`NEXUS_HOST_DIR`, else `~/.nexus/host`).
    pub root: PathBuf,
    pub home: PathBuf,
    /// The `PATH` the service's host and its agents get: the login shell's, as a host started over SSH has.
    pub path: String,
}

impl Service {
    fn is_default(&self) -> bool {
        self.root == self.home.join(".nexus").join("host")
    }

    /// `nexus-host.service` for the default root, else `nexus-host-<hash>.service`.
    pub fn unit_name(&self) -> String {
        if self.is_default() {
            "nexus-host.service".into()
        } else {
            format!("nexus-host-{}.service", short_hash(&self.root.to_string_lossy()))
        }
    }

    /// The launchd label: `inc.nexus.host` for the default root, else `inc.nexus.host.<hash>`.
    pub fn label(&self) -> String {
        if self.is_default() {
            "inc.nexus.host".into()
        } else {
            format!("inc.nexus.host.{}", short_hash(&self.root.to_string_lossy()))
        }
    }

    /// The binary the service runs: through `current`, never a version's own folder.
    pub fn program(&self) -> PathBuf {
        self.root.join(CURRENT).join("nexus-host")
    }

    /// The systemd user unit.
    pub fn systemd_unit(&self) -> String {
        let root = self.root.to_string_lossy();
        // `%` starts a specifier in every unit setting, the description too.
        let described = root.replace('%', "%%");
        format!(
            "{HEADER}\n\
             [Unit]\n\
             Description=Nexus host ({described})\n\
             StartLimitIntervalSec=60\n\
             StartLimitBurst=5\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={} serve\n\
             Environment={}\n\
             Environment={}\n\
             Environment={}\n\
             Restart=on-failure\n\
             RestartSec=2\n\
             KillMode=mixed\n\
             TimeoutStopSec=60\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            systemd_word(&self.program().to_string_lossy(), true),
            systemd_word(&format!("HOME={}", self.home.to_string_lossy()), false),
            systemd_word(&format!("NEXUS_HOST_DIR={root}"), false),
            systemd_word(&format!("PATH={}", self.path), false),
        )
    }

    /// The launchd LaunchAgent. `KeepAlive` restarts the host only when it fails, as `Restart=on-failure` does, so
    /// `nexus-host stop` (the first step of an update) stops it.
    pub fn launchd_plist(&self) -> String {
        let log = self.root.join("run").join("host.log");
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- {HEADER} -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>serve</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key>
    <string>{}</string>
    <key>NEXUS_HOST_DIR</key>
    <string>{}</string>
    <key>PATH</key>
    <string>{}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>10</integer>
  <key>StandardOutPath</key>
  <string>{}</string>
  <key>StandardErrorPath</key>
  <string>{}</string>
</dict>
</plist>
"#,
            xml(&self.label()),
            xml(&self.program().to_string_lossy()),
            xml(&self.home.to_string_lossy()),
            xml(&self.root.to_string_lossy()),
            xml(&self.path),
            xml(&log.to_string_lossy()),
            xml(&log.to_string_lossy()),
        )
    }

    /// The folder of the version `exe` belongs to, when `exe` is `<root>/<version>/nexus-host`; else the version
    /// `current` already names. An install from anywhere else has nothing stable to point at.
    pub fn version_dir(&self, exe: Option<&Path>) -> Result<String, String> {
        let root = self.root.canonicalize().ok();
        if let Some(folder) = exe
            .and_then(|exe| exe.canonicalize().ok())
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            && folder.parent() == root.as_deref()
            && let Some(name) = folder.file_name().and_then(|name| name.to_str())
            && name != CURRENT
        {
            return Ok(name.to_string());
        }
        if let Ok(target) = std::fs::read_link(self.root.join(CURRENT))
            && self.program().is_file()
        {
            return Ok(target.to_string_lossy().into_owned());
        }
        Err(format!(
            "Run `service install` with the nexus-host in {}/<version>/: the service needs a stable folder to start.",
            self.root.display()
        ))
    }

    fn manager(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            "launchd"
        } else {
            "systemd"
        }
    }

    /// Where the unit (or plist) is written.
    pub fn file(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{}.plist", self.label()))
        } else {
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| self.home.join(".config"));
            config.join("systemd").join("user").join(self.unit_name())
        }
    }

    /// Whether the service runs a host now.
    pub fn active(&self) -> bool {
        if cfg!(target_os = "macos") {
            launchd_running(&self.label())
        } else {
            call("systemctl", &["--user", "is-active", "--quiet", &self.unit_name()]).is_ok()
        }
    }

    /// Refuses a HOME that is not the account's own. The user manager reads units under the account's home: a unit
    /// written under another HOME (a test's throwaway one) would never start, and a test must not give the account's
    /// manager one either.
    pub fn check_home(&self) -> Result<(), String> {
        let own = user_home().and_then(|home| home.canonicalize().ok());
        if own.is_some() && own == self.home.canonicalize().ok() {
            return Ok(());
        }
        Err(format!(
            "HOME ({}) is not this account's home folder, so no service was installed.",
            self.home.display()
        ))
    }

    /// Refuses a unit file this did not write.
    pub fn check_ours(&self) -> Result<(), String> {
        let file = self.file();
        match std::fs::read_to_string(&file) {
            Ok(text) if !text.contains(HEADER) => Err(format!(
                "{} was not written by nexus-host, so it was left as it is.",
                file.display()
            )),
            _ => Ok(()),
        }
    }

    /// Writes the unit, points `current` at `version`, turns linger on, and starts the host through the service.
    /// The lines say what happened. The caller stops a host the service does not run first.
    pub fn install(&self, version: &str) -> Result<Vec<String>, String> {
        self.install_and(version, true)
    }

    /// [`Service::install`] without the start: the host running now (outside the service) keeps its sessions, and
    /// the service runs it from its next start, at boot or when the app starts it again.
    pub fn enable(&self, version: &str) -> Result<Vec<String>, String> {
        let mut lines = self.install_and(version, false)?;
        lines.push("The host running now keeps running; the service runs it from its next start.".into());
        Ok(lines)
    }

    fn install_and(&self, version: &str, start: bool) -> Result<Vec<String>, String> {
        self.check_home()?;
        self.check_ours()?;
        let mut lines = Vec::new();
        point_current(&self.root, version)?;
        let file = self.file();
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder).map_err(|e| format!("Could not create {}: {e}", folder.display()))?;
        }
        let (text, name) = if cfg!(target_os = "macos") {
            (self.launchd_plist(), self.label())
        } else {
            (self.systemd_unit(), self.unit_name())
        };
        std::fs::write(&file, text).map_err(|e| format!("Could not write {}: {e}", file.display()))?;
        std::fs::write(self.root.join(MARKER), format!("{name}\n"))
            .map_err(|e| format!("Could not write {}: {e}", self.root.join(MARKER).display()))?;
        if cfg!(target_os = "macos") {
            let domain = format!("gui/{}", nix::unistd::getuid().as_raw());
            // A plist already loaded keeps its old text until it is unloaded. RunAtLoad starts it as it loads, so
            // without a start it is only loaded at the next login, never now.
            if start {
                let _ = call("launchctl", &["bootout", &format!("{domain}/{name}")]);
                call("launchctl", &["bootstrap", &domain, &file.to_string_lossy()])?;
            }
        } else {
            call("systemctl", &["--user", "daemon-reload"])?;
            call("systemctl", &["--user", "enable", "--quiet", &name])?;
            if start {
                call("systemctl", &["--user", "restart", &name])?;
            }
            lines.extend(self.ensure_linger());
        }
        lines.insert(
            0,
            format!(
                "Installed {name}: the Nexus host in {} starts on boot.",
                self.root.display()
            ),
        );
        Ok(lines)
    }

    /// Turns linger on when it is off, so the host starts at boot and runs after logout.
    fn ensure_linger(&self) -> Vec<String> {
        let Some(user) = user_name() else {
            return vec!["Could not read this user's name, so linger was not checked.".into()];
        };
        if linger(&user) == Some(true) {
            return Vec::new();
        }
        match call("loginctl", &["enable-linger", &user]) {
            Ok(_) if linger(&user) == Some(true) => vec![format!("Turned on linger for {user}.")],
            _ => vec![format!(
                "Could not turn on linger for {user}: loginctl refused it. The host starts at boot only once {user} \
                 logs in, and stops at logout. An administrator can run `sudo loginctl enable-linger {user}`."
            )],
        }
    }

    /// Stops the service's host and removes the unit, the marker and `current`. The versions stay.
    pub fn uninstall(&self) -> Result<Vec<String>, String> {
        self.check_ours()?;
        let file = self.file();
        let name = if cfg!(target_os = "macos") {
            let name = self.label();
            let domain = format!("gui/{}", nix::unistd::getuid().as_raw());
            let _ = call("launchctl", &["bootout", &format!("{domain}/{name}")]);
            name
        } else {
            let name = self.unit_name();
            // Fails when the unit is not there: nothing to stop.
            let _ = call("systemctl", &["--user", "disable", "--now", "--quiet", &name]);
            name
        };
        let had = file.exists();
        remove(&file)?;
        if !cfg!(target_os = "macos") {
            call("systemctl", &["--user", "daemon-reload"])?;
            let _ = call("systemctl", &["--user", "reset-failed", &name]);
        }
        remove(&self.root.join(MARKER))?;
        remove(&self.root.join(CURRENT))?;
        Ok(vec![if had {
            format!(
                "Removed {name}: the Nexus host in {} no longer starts on boot.",
                self.root.display()
            )
        } else {
            format!("{name} was not installed.")
        }])
    }

    /// Installed, enabled, active and linger, as JSON.
    pub fn status(&self) -> Value {
        let installed = self.file().is_file();
        let current = std::fs::read_link(self.root.join(CURRENT))
            .ok()
            .map(|target| target.to_string_lossy().into_owned());
        let (unit, enabled, linger) = if cfg!(target_os = "macos") {
            (self.label(), installed, None)
        } else {
            let unit = self.unit_name();
            let enabled = call("systemctl", &["--user", "is-enabled", "--quiet", &unit]).is_ok();
            (unit, enabled, user_name().and_then(|user| linger(&user)))
        };
        json!({
            "manager": self.manager(),
            "unit": unit,
            "file": self.file().to_string_lossy(),
            "installed": installed,
            "enabled": enabled,
            "active": installed && self.active(),
            "linger": linger,
            "current": current,
        })
    }
}

/// The status JSON as lines for a person.
pub fn describe(status: &Value) -> Vec<String> {
    let flag = |key: &str| status[key].as_bool().unwrap_or(false);
    let unit = status["unit"].as_str().unwrap_or("the service");
    let mut lines = vec![if flag("installed") {
        format!(
            "{unit} is installed ({}), {}, and {}.",
            status["file"].as_str().unwrap_or(""),
            if flag("enabled") { "enabled" } else { "disabled" },
            if flag("active") { "running" } else { "not running" }
        )
    } else {
        format!("{unit} is not installed: the host does not start on boot. Run `nexus-host service install`.")
    }];
    if let Some(current) = status["current"].as_str() {
        lines.push(format!("It starts the version in {current}."));
    }
    match status["linger"].as_bool() {
        Some(true) => lines.push("Linger is on: it starts at boot and runs after logout.".into()),
        Some(false) => lines.push("Linger is off: it runs only while you are logged in.".into()),
        None => {}
    }
    lines
}

/// Points `<root>/current` at `version` (a folder in the root) in one rename, so the service never sees no link.
pub fn point_current(root: &Path, version: &str) -> Result<(), String> {
    if version.is_empty() || version.contains('/') || version == CURRENT || version.starts_with('.') {
        return Err(format!("{version:?} is not a version's folder."));
    }
    if !root.join(version).join("nexus-host").is_file() {
        return Err(format!("{} has no nexus-host.", root.join(version).display()));
    }
    let link = root.join(CURRENT);
    let part = root.join(format!("{CURRENT}.new.{}", std::process::id()));
    let _ = std::fs::remove_file(&part);
    std::os::unix::fs::symlink(version, &part).map_err(|e| format!("Could not create {}: {e}", part.display()))?;
    std::fs::rename(&part, &link).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("Could not point {} at {version}: {e}", link.display())
    })
}

/// A word for a unit file: quoted, with `%` (a specifier) doubled, and `$` doubled in a command line.
fn systemd_word(text: &str, command_line: bool) -> String {
    let mut escaped = text.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%");
    if command_line {
        escaped = escaped.replace('$', "$$");
    }
    format!("\"{escaped}\"")
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Eight hex digits of a root's FNV-1a hash: the same root always gives the same unit name.
fn short_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", hash >> 32)
}

fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Could not remove {}: {e}", path.display())),
    }
}

/// Runs `program args`, with a time limit. Its standard output, or why it failed.
fn call(program: &str, args: &[&str]) -> Result<String, String> {
    let mut runner = command(program);
    runner.args(args);
    match stdout_within(runner, CALL_LIMIT) {
        Some((status, stdout)) if status.success() => Ok(stdout),
        Some((status, _)) => Err(format!("`{program} {}` failed ({status}).", args.join(" "))),
        None => Err(format!(
            "`{program} {}` did not finish or could not run.",
            args.join(" ")
        )),
    }
}

fn user() -> Option<nix::unistd::User> {
    nix::unistd::User::from_uid(nix::unistd::getuid()).ok().flatten()
}

fn user_name() -> Option<String> {
    user().map(|user| user.name)
}

/// The account's home folder, as the user manager sees it (the password database), whatever HOME says.
fn user_home() -> Option<PathBuf> {
    user().map(|user| user.dir)
}

/// Whether `user`'s manager outlives their logins; `None` when loginctl cannot say.
fn linger(user: &str) -> Option<bool> {
    call("loginctl", &["show-user", user, "--property=Linger", "--value"])
        .ok()
        .map(|value| value.trim() == "yes")
}

fn launchd_running(label: &str) -> bool {
    let target = format!("gui/{}/{label}", nix::unistd::getuid().as_raw());
    call("launchctl", &["print", &target]).is_ok_and(|text| text.contains("state = running"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(root: &str) -> Service {
        Service {
            root: PathBuf::from(root),
            home: PathBuf::from("/home/ana"),
            path: "/home/ana/.local/bin:/usr/bin:/bin".into(),
        }
    }

    #[test]
    fn the_default_root_has_the_plain_name_and_others_a_stable_hash() {
        assert_eq!(service("/home/ana/.nexus/host").unit_name(), "nexus-host.service");
        assert_eq!(service("/home/ana/.nexus/host/").unit_name(), "nexus-host.service");
        assert_eq!(service("/home/ana/.nexus/host").label(), "inc.nexus.host");
        let other = service("/home/ana/.nexus/host-e2e").unit_name();
        assert_eq!(other, service("/home/ana/.nexus/host-e2e").unit_name());
        assert_ne!(other, service("/home/ana/.nexus/host-e2f").unit_name());
        assert!(
            other.starts_with("nexus-host-") && other.ends_with(".service"),
            "{other}"
        );
        assert_eq!(other.len(), "nexus-host-".len() + 8 + ".service".len());
    }

    #[test]
    fn the_unit_runs_current_with_the_roots_environment() {
        let unit = service("/home/ana/.nexus/host").systemd_unit();
        assert!(unit.starts_with(HEADER), "{unit}");
        assert!(
            unit.contains("ExecStart=\"/home/ana/.nexus/host/current/nexus-host\" serve\n"),
            "{unit}"
        );
        assert!(unit.contains("Environment=\"HOME=/home/ana\"\n"));
        assert!(unit.contains("Environment=\"NEXUS_HOST_DIR=/home/ana/.nexus/host\"\n"));
        assert!(unit.contains("Environment=\"PATH=/home/ana/.local/bin:/usr/bin:/bin\"\n"));
        assert!(unit.contains("Restart=on-failure\n"));
        assert!(unit.contains("WantedBy=default.target\n"));
    }

    #[test]
    fn a_unit_quotes_what_systemd_would_read_otherwise() {
        let unit = service("/srv/a b/50%/$x\"y").systemd_unit();
        assert!(
            unit.contains(r#"ExecStart="/srv/a b/50%%/$$x\"y/current/nexus-host" serve"#),
            "{unit}"
        );
        assert!(
            unit.contains(r#"Environment="NEXUS_HOST_DIR=/srv/a b/50%%/$x\"y""#),
            "{unit}"
        );
    }

    #[test]
    fn the_plist_runs_current_and_restarts_only_on_failure() {
        let plist = service("/Users/ana/.nexus/host-<e2e>").launchd_plist();
        assert!(plist.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
        assert!(plist.contains(
            "<string>/Users/ana/.nexus/host-&lt;e2e&gt;/current/nexus-host</string>\n    <string>serve</string>"
        ));
        assert!(plist.contains("<key>NEXUS_HOST_DIR</key>\n    <string>/Users/ana/.nexus/host-&lt;e2e&gt;</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(plist.contains("<key>SuccessfulExit</key>\n    <false/>"));
        assert!(plist.contains("<string>/Users/ana/.nexus/host-&lt;e2e&gt;/run/host.log</string>"));
        let label = service("/Users/ana/.nexus/host-<e2e>").label();
        assert!(plist.contains(&format!("<key>Label</key>\n  <string>{label}</string>")));
    }

    #[test]
    fn current_switches_in_one_step_and_refuses_what_is_not_a_version() {
        let root = std::env::temp_dir().join(format!("nexus-service-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for version in ["1.0.0", "1.1.0"] {
            std::fs::create_dir_all(root.join(version)).expect("a version folder");
            std::fs::write(root.join(version).join("nexus-host"), "").expect("a binary");
        }
        point_current(&root, "1.0.0").expect("the first version");
        assert_eq!(
            std::fs::read_link(root.join(CURRENT)).ok(),
            Some(PathBuf::from("1.0.0"))
        );
        point_current(&root, "1.1.0").expect("the second version");
        assert_eq!(
            std::fs::read_link(root.join(CURRENT)).ok(),
            Some(PathBuf::from("1.1.0"))
        );
        for bad in ["", "../x", CURRENT, ".hidden", "2.0.0"] {
            assert!(point_current(&root, bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            std::fs::read_link(root.join(CURRENT)).ok(),
            Some(PathBuf::from("1.1.0"))
        );

        let mut service = service("/unused");
        service.root = root.clone();
        let exe = root.join("1.0.0").join("nexus-host");
        assert_eq!(service.version_dir(Some(exe.as_path())), Ok("1.0.0".into()));
        // Run through `current`, the folder it names; run from elsewhere, what `current` already names.
        let through = root.join(CURRENT).join("nexus-host");
        assert_eq!(service.version_dir(Some(through.as_path())), Ok("1.1.0".into()));
        assert_eq!(service.version_dir(Some(Path::new("/bin/sh"))), Ok("1.1.0".into()));
        std::fs::remove_file(root.join(CURRENT)).expect("no current");
        assert!(service.version_dir(Some(Path::new("/bin/sh"))).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn status_reads_as_a_sentence() {
        let off = json!({ "unit": "nexus-host.service", "installed": false, "linger": false });
        assert_eq!(
            describe(&off),
            vec![
                "nexus-host.service is not installed: the host does not start on boot. Run `nexus-host service install`."
                    .to_string(),
                "Linger is off: it runs only while you are logged in.".to_string(),
            ]
        );
        let on = json!({ "unit": "u", "file": "/f", "installed": true, "enabled": true, "active": true,
                         "current": "1.0.0", "linger": true });
        assert_eq!(describe(&on)[0], "u is installed (/f), enabled, and running.");
        assert_eq!(describe(&on)[1], "It starts the version in 1.0.0.");
    }
}
