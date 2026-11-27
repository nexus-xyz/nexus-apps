//! What a server has, and signing its host in to Nexus, for the app's server
//! screens.
//!
//! - [`probe`]: one SSH round trip that says what the server is (`uname`), whether this app's host can run there,
//!   whether the host is signed in to Nexus, and whether Claude Code and Codex are installed and signed in, from their
//!   own status commands (`claude auth status --json`, `codex login status`), with the variables that would bill
//!   something else unset. The app reads the answers with `nexus_client::local_agents`, as it does for this computer.
//!   It works before the host is installed, so "Test connection" uses it too. Claude's check gets the one variable the
//!   host itself would give Claude from `$root/agent.env` (`CLAUDE_CODE_OAUTH_TOKEN`, a cloud machine's setup token),
//!   so a machine signed in only through that file reads as signed in, as its sessions are.
//! - [`login`]: `nexus-host login` on the server (its own device token, never the laptop's), streaming the code to
//!   approve, then restarting the running host so it uses the sign-in.

use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::Result;
use crate::install::{COMMON, Installer, parse};
use crate::ssh::{Target, ssh_error};

/// `probe <budget> <step> <agents>`: the system, the host's sign-in, the agents (space-separated, [`AGENTS`]). Agents
/// are looked for on `PATH`, in the folders their installers use, and on the login shell's `PATH` (a non-interactive
/// SSH command gets a short one). The slow steps share `budget` seconds, each at most `step`: one that does not fit
/// is cut (`timeout` exits 124) and says so (`<name>_slow=yes`), so the script still ends in time with what it read.
const PROBE: &str = r#"
end=$(( $(date +%s) + ${1:-20} )); step="${2:-10}"
limit() {
  left=$((end - $(date +%s)))
  [ "$left" -gt "$step" ] && left="$step"
  [ "$left" -gt 0 ] || return 124
  if command -v timeout >/dev/null 2>&1; then timeout "$left" "$@"; else "$@"; fi
}
# A step `limit` cut: GNU timeout exits 124, BusyBox's (Alpine) 143 from the TERM it sends.
was_cut() { [ "$1" -eq 124 ] || [ "$1" -eq 143 ]; }
os="$(uname -s)"; arch="$(uname -m)"
case "$arch" in x86_64|amd64) arch=x86_64 ;; aarch64|arm64) arch=aarch64 ;; esac
echo "os=$os"; echo "arch=$arch"
if [ -f "$root/credentials" ]; then echo "nexus=yes"; else echo "nexus=no"; fi
# Whether the host starts on boot: the unit `nexus-host service install` named in `$root/service` is enabled.
unit="$(cat "$root/service" 2>/dev/null)"
if [ "$os" = Linux ] && command -v systemctl >/dev/null 2>&1; then
  if [ -n "$unit" ] && systemctl --user is-enabled --quiet "$unit" 2>/dev/null; then echo "boot=yes"; else echo "boot=no"; fi
elif [ "$os" = Darwin ]; then
  if [ -n "$unit" ] && [ -f "$HOME/Library/LaunchAgents/$unit.plist" ]; then echo "boot=yes"; else echo "boot=no"; fi
fi
PATH="$PATH:$HOME/.local/bin:$HOME/.claude/local:$HOME/.npm-global/bin:$HOME/.bun/bin:$HOME/.volta/bin:/usr/local/bin"
if [ -n "${SHELL:-}" ] && [ -x "$SHELL" ]; then
  login_path="$(limit "$SHELL" -lc 'printf "%s" "$PATH"' 2>/dev/null </dev/null)"
  was_cut $? && echo "path_slow=yes"
  [ -n "$login_path" ] && PATH="$PATH:$login_path"
fi
export PATH
# The host passes agent.env's variables to Claude (nexus-host's agent.env); the check sees the same token. Read as
# `KEY=VALUE` text, never sourced: the file is data, not a script.
oauth=""
if [ -f "$root/agent.env" ]; then
  oauth="$(sed -n 's/^[[:space:]]*CLAUDE_CODE_OAUTH_TOKEN=//p' "$root/agent.env" 2>/dev/null | tail -n 1 | tr -d '\r')"
  case "$oauth" in \"*\") oauth="${oauth#\"}"; oauth="${oauth%\"}" ;; \'*\') oauth="${oauth#\'}"; oauth="${oauth%\'}" ;; esac
fi
unset ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL ANTHROPIC_CUSTOM_HEADERS CLAUDE_CODE_USE_BEDROCK \
  CLAUDE_CODE_USE_VERTEX CLAUDE_CODE_USE_FOUNDRY OPENAI_API_KEY OPENAI_BASE_URL OPENAI_ORG_ID OPENAI_ORGANIZATION \
  OPENAI_PROJECT_ID
for agent in $3; do
  bin="$(command -v "$agent" 2>/dev/null)"
  [ -n "$bin" ] || continue
  echo "${agent}_path=$bin"
  version="$(limit "$bin" --version 2>/dev/null </dev/null)"
  was_cut $? && echo "${agent}_slow=yes"
  echo "${agent}_version=$(printf '%s\n' "$version" | head -n 1)"
  if [ "$agent" = claude ] && [ -n "$oauth" ]; then
    # In the command substitution's own subshell: the token is set for this check alone.
    status="$(export CLAUDE_CODE_OAUTH_TOKEN="$oauth"; limit "$bin" auth status --json 2>/dev/null </dev/null)"
  elif [ "$agent" = claude ]; then
    status="$(limit "$bin" auth status --json 2>/dev/null </dev/null)"
  else
    status="$(limit "$bin" login status 2>&1 </dev/null)"
  fi
  was_cut $? && echo "${agent}_slow=yes"
  echo "${agent}_status=$(printf '%s' "$status" | tr '\n' ' ')"
done
echo "probed=yes"
"#;

/// `login <dir> <gateway>`: `nexus-host login` with that version's binary, then the running host is restarted (with
/// the binary it runs) so the sign-in takes effect. Sessions cannot run on a host that is not signed in, so a first
/// sign-in stops nothing; signing in again stops running sessions as an update does.
const LOGIN: &str = r#"
bin="$root/$1/nexus-host"
[ -x "$bin" ] || { echo "error=The Nexus host is not installed on this server yet."; exit 0; }
if [ -n "$2" ]; then "$bin" login --gateway "$2"; else "$bin" login; fi || { echo "failed=yes"; exit 0; }
pid="$(cat "$root/run/host.lock" 2>/dev/null)"
old=""
[ -n "$pid" ] && old="$(readlink "/proc/$pid/exe" 2>/dev/null)"
[ -n "$old" ] && [ -x "$old" ] || old="$bin"
if running "$old"; then
  "$old" stop >/dev/null 2>&1
  start "$old" >/dev/null || { echo "error=Signed in, but the Nexus host did not start again."; exit 0; }
fi
echo "restarted=yes"
"#;

/// The agents [`probe`] looks for, in the order it lists them.
pub const AGENTS: [&str; 2] = ["claude", "codex"];

/// What [`probe`] found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Probe {
    /// `uname -s` (`Linux`) and the architecture (`x86_64`, `aarch64`).
    pub os: String,
    pub arch: String,
    /// The host there is signed in to Nexus (its credentials file exists).
    pub nexus: bool,
    /// The host starts when the server boots (`nexus-host service install`); None where the server has
    /// no service manager the host uses.
    pub boot: Option<bool>,
    /// Per agent (`claude`, `codex`) found there: its path, its `--version` line, and its status command's output.
    pub agents: Vec<AgentProbe>,
    /// Some checks did not finish in time: why, for the person. What was read is still here; the rest reads as not
    /// found or unknown.
    pub incomplete: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentProbe {
    pub agent: String,
    pub path: String,
    pub version: String,
    pub status: String,
}

impl Probe {
    /// Whether this app's host runs there (Linux on x86-64 or arm64).
    pub fn supported(&self) -> bool {
        self.os == "Linux" && (self.arch == "x86_64" || self.arch == "aarch64")
    }

    pub fn from_fields(fields: &HashMap<String, String>) -> Probe {
        let get = |key: &str| fields.get(key).cloned().unwrap_or_default();
        let agents = AGENTS
            .into_iter()
            .filter(|agent| !get(&format!("{agent}_path")).is_empty())
            .map(|agent| AgentProbe {
                agent: agent.to_string(),
                path: get(&format!("{agent}_path")),
                version: get(&format!("{agent}_version")),
                status: get(&format!("{agent}_status")),
            })
            .collect();
        Probe {
            os: get("os"),
            arch: get("arch"),
            nexus: get("nexus") == "yes",
            boot: match get("boot").as_str() {
                "yes" => Some(true),
                "no" => Some(false),
                _ => None,
            },
            agents,
            incomplete: None,
        }
    }
}

/// How long [`probe`] takes at most: the whole run, connecting included, and the seconds the server's slow steps share
/// (each at most `step`). ssh may spend its 15 s `ConnectTimeout` first, so `budget` leaves room for it.
#[derive(Debug, Clone, Copy)]
struct Limits {
    total: Duration,
    budget: u64,
    step: u64,
}

const LIMITS: Limits = Limits {
    total: Duration::from_secs(40),
    budget: 20,
    step: 10,
};

const SLOW: &str =
    "was slow to answer, so some checks did not finish. What it read is shown; check again for the rest.";

/// Reaches `target` and reads what it has. The error is ssh's own reason ("Permission denied (publickey).").
/// A server that answers but is slow still gives what it read, and [`Probe::incomplete`] says some is missing.
pub async fn probe(target: &Target) -> Result<Probe> {
    probe_within(target, LIMITS, &AGENTS).await
}

/// [`probe`], looking for `agents` only. The tests look for their fake `claude` alone, so a runner's own agents
/// (say, a `codex` in `/usr/local/bin`) are never found or run.
async fn probe_within(target: &Target, limits: Limits, agents: &[&str]) -> Result<Probe> {
    let (budget, step) = (limits.budget.to_string(), limits.step.to_string());
    let agents = agents.join(" ");
    let mut command = target.command(&format!("{COMMON}{PROBE}"), &[&budget, &step, &agents])?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not run ssh: {e}. Is OpenSSH installed?"))?;
    let stdout = child.stdout.take().ok_or("Could not talk to ssh.")?;
    let mut stderr = child.stderr.take().ok_or("Could not talk to ssh.")?;
    // Read line by line, so what came before a time-out is kept.
    let mut text = String::new();
    let work = async {
        let read_err = async {
            let mut errors = String::new();
            let _ = stderr.read_to_string(&mut errors).await;
            errors
        };
        let read_out = async {
            // By bytes, not `lines()`: one byte that is not UTF-8 (a CLI's status output) must not end the read.
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            while reader.read_until(b'\n', &mut line).await.is_ok_and(|read| read > 0) {
                text.push_str(String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']));
                text.push('\n');
                line.clear();
            }
        };
        let (errors, ()) = tokio::join!(read_err, read_out);
        let ok = child.wait().await.is_ok_and(|status| status.success());
        (ok, errors)
    };
    let finished = tokio::time::timeout(limits.total, work).await;
    // The ssh child is killed as it drops (`kill_on_drop`).
    drop(child);
    let fields = parse(&text);
    let name = target.name();
    let Ok((ok, errors)) = finished else {
        if fields.get("os").is_some_and(|os| !os.is_empty()) {
            let mut probe = Probe::from_fields(&fields);
            probe.incomplete = Some(format!("{name} {SLOW}"));
            return Ok(probe);
        }
        return Err(format!(
            "{name} did not answer within {} seconds.",
            limits.total.as_secs()
        ));
    };
    if !ok || fields.get("probed").map(String::as_str) != Some("yes") {
        let reason = ssh_error(&errors).unwrap_or_else(|| "it closed the connection".into());
        return Err(format!("Could not connect to {name}: {reason}"));
    }
    let mut probe = Probe::from_fields(&fields);
    if fields.keys().any(|key| key.ends_with("_slow")) {
        probe.incomplete = Some(format!("{name} {SLOW}"));
    }
    Ok(probe)
}

/// A step of [`login`], as it happens.
#[derive(Debug, Clone, PartialEq)]
pub enum LoginStep {
    /// Open `link` and approve `code`.
    Code { link: String, code: String },
    /// Approved: signed in to `org` (when the Gateway named it).
    SignedIn { org: Option<String> },
}

/// Reads `nexus-host login`'s lines: the link (an indented URL), then "and approve the code ABCD-EFGH.", then
/// "Signed in to <org>.".
#[derive(Default)]
pub struct LoginReader {
    link: Option<String>,
}

impl LoginReader {
    pub fn line(&mut self, line: &str) -> Option<LoginStep> {
        let line = line.trim();
        if line.starts_with("https://") || line.starts_with("http://") {
            self.link = Some(line.to_string());
            return None;
        }
        if let Some(rest) = line.strip_prefix("and approve the code ") {
            let code = rest.trim_end_matches('.').trim().to_string();
            return Some(LoginStep::Code {
                link: self.link.clone().unwrap_or_default(),
                code,
            });
        }
        if let Some(rest) = line.strip_prefix("Signed in to ") {
            let org = rest.trim_end_matches('.').trim();
            return Some(LoginStep::SignedIn {
                org: (!org.is_empty() && org != "your organization").then(|| org.to_string()),
            });
        }
        None
    }
}

/// Signs the host on `installer`'s server in to Nexus at `gateway` (the app's own), reporting each step to `step`, and
/// restarts it. Ends when approved and restarted, or with the reason it did not. Dropping the future stops it (the
/// ssh child is killed, and with it the waiting `nexus-host login`).
pub async fn login(installer: &Installer, gateway: Option<&str>, step: impl Fn(LoginStep)) -> Result<()> {
    let dir = installer.dir().await?;
    let script = format!("{COMMON}{LOGIN}");
    let mut command = installer.target.command(&script, &[&dir, gateway.unwrap_or("")])?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not run ssh: {e}. Is OpenSSH installed?"))?;
    let stdout = child.stdout.take().ok_or("Could not talk to ssh.")?;
    let mut stderr = child.stderr.take().ok_or("Could not talk to ssh.")?;
    let errors = tokio::spawn(async move {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text).await;
        text
    });
    let mut reader = LoginReader::default();
    let mut lines = BufReader::new(stdout).lines();
    let mut fields = HashMap::new();
    let mut signed_in = false;
    // The person has ten minutes to approve, as the device code does; the restart after takes a few seconds.
    let read = async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some((key, value)) = line.split_once('=')
                && matches!(key, "error" | "failed" | "restarted")
            {
                fields.insert(key.to_string(), value.to_string());
                continue;
            }
            if let Some(found) = reader.line(&line) {
                signed_in |= matches!(found, LoginStep::SignedIn { .. });
                step(found);
            }
        }
    };
    let finished = tokio::time::timeout(Duration::from_secs(16 * 60), read).await;
    if finished.is_err() {
        return Err(if signed_in {
            SIGNED_IN_UNCONFIRMED.to_string()
        } else {
            "The sign-in took too long. Try again.".to_string()
        });
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    let stderr = errors.await.unwrap_or_default();
    login_outcome(&fields, status.success(), &stderr, signed_in, installer.target.name())
}

/// What a sign-in that ended after the host's last line came to. Once the host said "Signed in", a dropped connection
/// or a missing "restarted=yes" has still signed the server in, so it says so instead of blaming the connection.
const SIGNED_IN_UNCONFIRMED: &str = "Signed in, but the restart was not confirmed. Check the server again.";

/// `boot <dir>`: `nexus-host service install --keep-running` with that version's binary. The running host and its
/// sessions go on; the service runs the host from its next start. A host from before the command says it is too old.
const BOOT: &str = r#"
bin="$root/$1/nexus-host"
[ -x "$bin" ] || { echo "error=The Nexus host is not installed on this server yet."; exit 0; }
if out="$("$bin" service install --keep-running 2>&1)"; then
  echo "boot=yes"
else
  case "$out" in
    *"Usage: nexus-host"*) echo "error=This server's Nexus host is too old to start on boot. Update it first." ;;
    *) echo "error=$(printf '%s\n' "$out" | tail -n 1)" ;;
  esac
fi
"#;

/// Makes the host on `installer`'s server start on boot (`nexus-host service install`), leaving the host
/// running now as it is.
pub async fn start_on_boot(installer: &Installer) -> Result<()> {
    let dir = installer.dir().await?;
    start_on_boot_in(&installer.target, &dir).await
}

async fn start_on_boot_in(target: &Target, dir: &str) -> Result<()> {
    let output = target
        .run(&format!("{COMMON}{BOOT}"), &[dir], None, Duration::from_secs(90))
        .await?;
    let fields = parse(&output.stdout);
    if let Some(error) = fields.get("error") {
        return Err(error.clone());
    }
    if fields.get("boot").map(String::as_str) == Some("yes") {
        return Ok(());
    }
    let reason = ssh_error(&output.stderr).unwrap_or_else(|| "it closed the connection".into());
    Err(format!(
        "Could not set {} to start the host on boot: {reason}",
        target.name()
    ))
}

fn login_outcome(
    fields: &HashMap<String, String>,
    ssh_ok: bool,
    stderr: &str,
    signed_in: bool,
    name: impl std::fmt::Display,
) -> Result<()> {
    if let Some(error) = fields.get("error") {
        return Err(error.clone());
    }
    let restarted = fields.contains_key("restarted");
    if signed_in && !restarted {
        return Err(SIGNED_IN_UNCONFIRMED.into());
    }
    if fields.contains_key("failed") || !ssh_ok {
        let reason = ssh_error(stderr).unwrap_or_else(|| "it closed the connection".into());
        return Err(if ssh_ok {
            format!("The sign-in did not finish: {reason}")
        } else {
            format!("Could not connect to {name}: {reason}")
        });
    }
    if !signed_in || !restarted {
        return Err("The sign-in did not finish. Try again.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_login_lines() {
        let mut reader = LoginReader::default();
        assert_eq!(reader.line("To sign this machine in to Nexus, open"), None);
        assert_eq!(reader.line(""), None);
        assert_eq!(reader.line("  https://example.test/device?code=ABCD-EFGH"), None);
        assert_eq!(
            reader.line("and approve the code ABCD-EFGH."),
            Some(LoginStep::Code {
                link: "https://example.test/device?code=ABCD-EFGH".into(),
                code: "ABCD-EFGH".into()
            })
        );
        assert_eq!(
            reader.line("Signed in to acme."),
            Some(LoginStep::SignedIn {
                org: Some("acme".into())
            })
        );
        assert_eq!(
            reader.line("Signed in to your organization."),
            Some(LoginStep::SignedIn { org: None })
        );
    }

    #[test]
    fn a_connection_lost_after_sign_in_says_signed_in() {
        let none = HashMap::new();
        let restarted: HashMap<String, String> = [("restarted".to_string(), "yes".to_string())].into();
        // Dropped ssh (not ok) after the signed-in line, with and without the host's own "failed".
        let dropped = login_outcome(&none, false, "Connection reset by peer", true, "box");
        assert_eq!(dropped, Err(SIGNED_IN_UNCONFIRMED.to_string()));
        let failed: HashMap<String, String> = [("failed".to_string(), "1".to_string())].into();
        assert_eq!(
            login_outcome(&failed, true, "", true, "box"),
            Err(SIGNED_IN_UNCONFIRMED.to_string())
        );
        // Before the signed-in line it is still a plain connection failure.
        let Err(before) = login_outcome(&none, false, "Connection reset by peer", false, "box") else {
            panic!("a dropped connection before sign-in is an error");
        };
        assert!(before.starts_with("Could not connect to box"), "{before}");
        // The host's own error wins, and a confirmed restart is success.
        let error: HashMap<String, String> = [("error".to_string(), "No room".to_string())].into();
        assert_eq!(
            login_outcome(&error, false, "", true, "box"),
            Err("No room".to_string())
        );
        assert_eq!(login_outcome(&restarted, true, "", true, "box"), Ok(()));
    }

    #[test]
    fn a_probe_lists_only_the_agents_found() {
        let fields = parse(
            "os=Linux\narch=aarch64\nnexus=no\nclaude_path=/home/me/.local/bin/claude\nclaude_version=2.1.0 (Claude Code)\nclaude_status={\"loggedIn\": false} \nprobed=yes\n",
        );
        let probe = Probe::from_fields(&fields);
        assert!(probe.supported());
        assert!(!probe.nexus);
        assert_eq!(probe.agents.len(), 1);
        assert_eq!(probe.agents[0].agent, "claude");
        assert_eq!(probe.agents[0].status, "{\"loggedIn\": false}");
        assert!(!Probe::from_fields(&parse("os=Darwin\narch=aarch64\n")).supported());
        assert_eq!(probe.boot, None);
        assert_eq!(Probe::from_fields(&parse("boot=yes\n")).boot, Some(true));
        assert_eq!(Probe::from_fields(&parse("boot=no\n")).boot, Some(false));
    }

    /// "Start on boot" runs the version's `service install --keep-running`, and reads an older host's usage text as
    /// "too old".
    #[cfg(unix)]
    #[tokio::test]
    async fn start_on_boot_runs_the_versions_service_install() {
        let (home, target) = slow_home("boot", "exit 0");
        let root = home.join("host");
        for (version, body) in [
            (
                "new",
                r#"echo "$*" > "$NEXUS_HOST_DIR/asked"; echo "Installed nexus-host.service.""#,
            ),
            ("old", "echo 'Usage: nexus-host <command>' >&2; exit 1"),
            ("broken", "echo 'first'; echo 'Could not write the unit.' >&2; exit 1"),
        ] {
            let bin = root.join(version).join("nexus-host");
            let _ = std::fs::create_dir_all(root.join(version));
            let written = nexus_workbench::process::command("sh")
                .arg("-c")
                .arg(r#"printf '#!/bin/sh\n%s\n' "$2" > "$1" && chmod +x "$1""#)
                .arg("sh")
                .arg(&bin)
                .arg(body)
                .status();
            assert!(written.is_ok_and(|status| status.success()));
        }
        assert_eq!(start_on_boot_in(&target, "new").await, Ok(()));
        assert_eq!(
            std::fs::read_to_string(root.join("asked")).ok().as_deref(),
            Some("service install --keep-running\n")
        );
        let old = start_on_boot_in(&target, "old").await;
        assert!(old.as_ref().is_err_and(|e| e.contains("too old")), "{old:?}");
        let broken = start_on_boot_in(&target, "broken").await;
        assert_eq!(broken, Err("Could not write the unit.".to_string()));
        let missing = start_on_boot_in(&target, "none").await;
        assert!(
            missing.as_ref().is_err_and(|e| e.contains("not installed")),
            "{missing:?}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The agents a [`slow_home`] test looks for: its fake `claude` alone. The probe still adds the folders a real host
    /// needs (`/usr/local/bin`), so looking for `codex` too would find the runner's own.
    const FAKE: &[&str] = &["claude"];

    /// A throwaway home with a fake `claude` on `PATH` that runs `body`. Written by a child `sh`, so this process never
    /// holds the file open while another test runs it (ETXTBSY).
    #[cfg(unix)]
    fn slow_home(tag: &str, body: &str) -> (std::path::PathBuf, Target) {
        let home = std::env::temp_dir().join(format!("nexus-remote-probe-{tag}-{}", std::process::id()));
        let bin = home.join("bin");
        let _ = std::fs::create_dir_all(&bin);
        let fake = bin.join("claude");
        let written = nexus_workbench::process::command("sh")
            .arg("-c")
            .arg(r#"printf '#!/bin/sh\n%s\n' "$2" > "$1" && chmod +x "$1""#)
            .arg("sh")
            .arg(&fake)
            .arg(body)
            .status();
        assert!(written.is_ok_and(|status| status.success()));
        let target = Target::Local {
            env: vec![
                ("HOME".into(), home.display().to_string()),
                ("NEXUS_HOST_DIR".into(), home.join("host").display().to_string()),
                ("SHELL".into(), String::new()),
                ("PATH".into(), format!("{}:/usr/bin:/bin", bin.display())),
            ],
        };
        (home, target)
    }

    /// Steps past the shared budget are cut: the probe ends in time, with what it read and a note. The cut comes at
    /// the 3 s step, so ending within 10 s proves it: the slow step would take 30 s, and the whole limit is 20 s. A 1 s
    /// step also cut the fast `--version` on a loaded runner.
    #[cfg(unix)]
    #[tokio::test]
    async fn slow_steps_are_cut_and_the_rest_is_kept() {
        let (home, target) = slow_home(
            "cut",
            r#"case "$1" in --version) echo "2.1.0 (Claude Code)" ;; *) sleep 30 ;; esac"#,
        );
        let limits = Limits {
            total: Duration::from_secs(20),
            budget: 6,
            step: 3,
        };
        let started = std::time::Instant::now();
        let probe = probe_within(&target, limits, FAKE).await;
        let _ = std::fs::remove_dir_all(&home);
        let probe = probe.unwrap_or_else(|e| panic!("{e}"));
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        assert!(!probe.os.is_empty());
        assert_eq!(probe.agents.len(), 1);
        assert_eq!(probe.agents[0].version, "2.1.0 (Claude Code)");
        assert_eq!(probe.agents[0].status, "");
        assert!(probe.incomplete.as_deref().is_some_and(|note| note.contains("slow")));
    }

    /// A byte that is not UTF-8 in one line (an agent's version) does not end the read: the lines after it still count.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_line_that_is_not_utf8_keeps_the_rest() {
        let (home, target) = slow_home(
            "bytes",
            r#"case "$1" in --version) printf '2.1.0 \377\n' ;; *) echo '{"loggedIn": true}' ;; esac"#,
        );
        let probe = probe_within(&target, LIMITS, FAKE).await;
        let _ = std::fs::remove_dir_all(&home);
        let probe = probe.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(probe.agents.len(), 1);
        assert!(probe.agents[0].version.starts_with("2.1.0"));
        assert_eq!(probe.agents[0].status, "{\"loggedIn\": true}");
        assert!(probe.incomplete.is_none());
    }

    /// A run that outlasts the whole limit still gives what it printed before it, marked incomplete. The limit leaves
    /// the script time to print its first lines on a loaded runner, which 1 s did not. The fake agent outlasts it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_run_past_the_limit_keeps_what_it_read() {
        let (home, target) = slow_home("hung", "sleep 10");
        let limits = Limits {
            total: Duration::from_secs(5),
            budget: 30,
            step: 30,
        };
        let probe = probe_within(&target, limits, FAKE).await;
        let _ = std::fs::remove_dir_all(&home);
        let probe = probe.unwrap_or_else(|e| panic!("{e}"));
        assert!(!probe.os.is_empty());
        assert_eq!(probe.agents.len(), 1);
        assert!(probe.agents[0].path.ends_with("/bin/claude"));
        assert!(probe.incomplete.is_some());
    }

    /// A machine signed in to Claude only through agent.env's setup token (a cloud machine) reads as signed in: the
    /// check gets that one variable, from a file that is read, not run.
    #[cfg(unix)]
    #[tokio::test]
    async fn claudes_check_sees_the_agent_env_token() {
        let (home, target) = slow_home(
            "oauth",
            r#"case "$1" in --version) echo "2.1.0 (Claude Code)" ;; *) if [ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ]; then echo "{\"loggedIn\":true,\"authMethod\":\"oauth_token\",\"t\":\"$CLAUDE_CODE_OAUTH_TOKEN\"}"; else echo '{"loggedIn":false}'; fi ;; esac"#,
        );
        let before = probe_within(&target, LIMITS, FAKE).await.unwrap_or_default();
        let host = home.join("host");
        let _ = std::fs::create_dir_all(&host);
        let marker = home.join("ran");
        let env = format!(
            "# set by Nexus\nOTHER=1\nCLAUDE_CODE_OAUTH_TOKEN=\"sk-ant-oat01-abc\"\n$(touch {})\n",
            marker.display()
        );
        let _ = std::fs::write(host.join("agent.env"), env);
        let after = probe_within(&target, LIMITS, FAKE).await;
        let ran = marker.exists();
        let _ = std::fs::remove_dir_all(&home);
        let after = after.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            before.agents.first().map(|a| a.status.as_str()),
            Some("{\"loggedIn\":false}")
        );
        assert_eq!(
            after.agents.first().map(|a| a.status.as_str()),
            Some("{\"loggedIn\":true,\"authMethod\":\"oauth_token\",\"t\":\"sk-ant-oat01-abc\"}")
        );
        assert!(!ran, "agent.env must be read, never run");
    }

    #[tokio::test]
    async fn probes_a_machine() {
        let home = std::env::temp_dir().join(format!("nexus-remote-probe-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&home);
        let target = Target::Local {
            env: vec![
                ("HOME".into(), home.display().to_string()),
                ("NEXUS_HOST_DIR".into(), home.join("host").display().to_string()),
                // No login shell to ask.
                ("SHELL".into(), String::new()),
                ("PATH".into(), "/usr/bin:/bin".into()),
            ],
        };
        // No agents to look for: the runner's own would be found and run.
        let probe = probe_within(&target, LIMITS, &[]).await;
        let _ = std::fs::remove_dir_all(&home);
        let probe = probe.unwrap_or_default();
        assert!(!probe.os.is_empty());
        assert!(!probe.nexus);
    }
}
