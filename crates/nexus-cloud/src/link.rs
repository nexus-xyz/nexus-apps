//! A cloud machine as a `nexus_remote::machine::Link`:
//! the relay (`nexus_remote::machine::Remote`) opens it on every connect and reconnect, exactly like a server's
//! [`SshLink`], and it does three things first.
//!
//! 1. **Wake.** A sleeping machine is woken and waited for; the app hears "Waking", then "Starting" ([`Progress`]).
//! 2. **Reach.** The machine's public key is registered again and its host key pinned again: boat starts a new machine
//!    on every wake, with a new address and a new host key. The key goes in the app's own known_hosts file under the
//!    machine's stable alias (`nexus-cloud-<id>`), so ssh checks it strictly and never asks.
//! 3. **Prepare** ([`provision`], one SSH round trip, every time): the agent's environment file (`agent.env`, the
//!    Claude token) and its MCP servers (`mcp.json`, the Playwright browser, for Claude Code and Codex), both written from standard input, never
//!    on a command line, with the Playwright MCP package installed once ([`PLAYWRIGHT_MCP`]); git's name and email, linger (so the host
//!    started under systemd survives the SSH session), each agent's first-run screen marked done
//!    (`CloudAgent::first_run`), boat's pre-stop hook (stop the host cleanly before the disk is saved), and the person's GitHub token for `gh` and git. Gateway decides
//!    when the machine sleeps (from the host's heartbeat), so the idle timer an older app installed is removed.
//!
//! Then it is a server like any other: [`SshLink`] over [`Target::Direct`] installs the app's own `nexus-host`,
//! starts it, and opens `nexus-host proxy`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use futures::future::BoxFuture;
use nexus_remote::install::{Fetch, Installer, Source, parse};
use nexus_remote::machine::{Link, Opened, SshLink};
use nexus_remote::ssh::{Target, ssh_error, write_known_hosts};
use nexus_workbench::cloud_agent::{FirstRun, InstructionsFile, first_run_files, instructions_files};
use nexus_workbench::provision::{Github, HostProvision};
use serde::Serialize;

use crate::keys::KeyPair;
use crate::{Error, Machine, Provider, Result, State, wait_ready, wait_stopped};

/// What the app is doing before the connection, for the machine's status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Progress {
    /// Asked the provider to wake the machine.
    Waking,
    /// The machine is booting.
    Starting,
    /// Up: registering the key, pinning the host key and preparing the machine over SSH.
    Preparing,
}

impl Progress {
    pub fn label(&self) -> &'static str {
        match self {
            Progress::Waking => "Waking",
            Progress::Starting => "Starting",
            Progress::Preparing => "Preparing",
        }
    }
}

/// What the app puts on the machine at every connect, read fresh each time (so a token replaced in Settings takes
/// effect on the next connect).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Provision {
    /// `KEY=VALUE` lines for the agents (`CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat…`), written to `$root/agent.env` (mode
    /// 0600). None removes the file, unless [`Provision::keep_agent_env`].
    pub agent_env: Option<String>,
    /// With no [`Provision::agent_env`]: leave the machine's `agent.env` as it is. The same machine shows on every
    /// device the person signs in on, and a device without the Claude token must not remove the one another device
    /// wrote there.
    pub keep_agent_env: bool,
    /// The agents' MCP servers, written to `$root/mcp.json` (mode 0600; the host refuses it otherwise). None removes
    /// the file. A config that names `@playwright/mcp` gets [`PLAYWRIGHT_MCP`] installed on the machine once.
    pub mcp_json: Option<String>,
    /// `git config --global user.name` / `user.email`, when given.
    pub git_name: Option<String>,
    pub git_email: Option<String>,
    /// The person's GitHub token (`gho_…`), kept in the app's keychain: the machine's `gh` signs in with it and git
    /// uses it for github.com. Applied only when the machine's own differs; None leaves the machine's `gh` as it is.
    pub github_token: Option<String>,
    /// What the agents should know about the machine, as Markdown: kept in its own marked block of each agent's
    /// instructions file (`CloudAgent::instructions`: `~/.claude/CLAUDE.md`, Codex's `~/.codex/AGENTS.md` or
    /// `$CODEX_HOME`), [`CLAUDE_MD_BEGIN`] to [`CLAUDE_MD_END`], which each agent reads in every session. The rest of
    /// each file is the person's and stays as it is. None removes the block from each.
    pub claude_md: Option<String>,
}

impl Provision {
    /// The same setup as `host_provision`'s arguments, for a machine reached through Gateway: the host applies it
    /// (nexus-host's src/provision.rs) as PREPARE does over SSH.
    pub fn host_provision(&self) -> HostProvision {
        let agent_env = match (&self.agent_env, self.keep_agent_env) {
            (Some(text), _) => Some(text.clone()),
            (None, true) => None,
            // An empty text removes the file.
            (None, false) => Some(String::new()),
        };
        let any = self.github_token.is_some() || self.git_name.is_some() || self.git_email.is_some();
        HostProvision {
            github: any.then(|| Github {
                token: self.github_token.clone(),
                name: self.git_name.clone(),
                email: self.git_email.clone(),
            }),
            agent_env,
            mcp: self.mcp_json.clone(),
            instructions: self.claude_md.clone(),
        }
    }
}

/// The first line of the app's block in `~/.claude/CLAUDE.md` ([`Provision::claude_md`]). The host writes the same.
pub const CLAUDE_MD_BEGIN: &str = nexus_workbench::provision::BLOCK_BEGIN;
/// Its last line.
pub const CLAUDE_MD_END: &str = nexus_workbench::provision::BLOCK_END;

/// The Playwright MCP server the provisioning installs globally when `mcp.json` names it, so a session does not
/// download it as it starts. The app's config should run this same version. Latest on npm on 2026-11-01.
pub const PLAYWRIGHT_MCP: &str = "@playwright/mcp@0.0.84";

/// How long a wake may take before the connect fails (boat says seconds; a cold create takes longer).
pub const WAKE_DEADLINE: Duration = Duration::from_secs(5 * 60);

/// How long to keep asking for the SSH key to be registered while boat is still securing a fresh machine.
const AUTHORIZE_DEADLINE: Duration = Duration::from_secs(90);

/// A cloud machine's link.
pub struct CloudLink {
    pub provider: Arc<dyn Provider>,
    /// The provider's id for the machine (`bx_23456789`).
    pub id: String,
    /// The app's key for this machine ([`crate::keys::load_or_generate`]).
    pub key: KeyPair,
    /// The app's known_hosts file for this machine, rewritten on every connect.
    pub known_hosts: PathBuf,
    /// The host to install: the app's own release, or a local build in development.
    pub source: Source,
    /// Downloads on this computer, for a machine that cannot download the host itself.
    pub fetch: Option<Fetch>,
    /// What to put on the machine ([`Provision`]), asked at every connect.
    pub provision: Arc<dyn Fn() -> Provision + Send + Sync>,
    /// Hears [`Progress`] while connecting.
    pub progress: Arc<dyn Fn(Progress) + Send + Sync>,
    pub wake_deadline: Duration,
}

impl CloudLink {
    /// A link with nothing to provision and no one listening; the app sets [`CloudLink::provision`],
    /// [`CloudLink::progress`] and [`CloudLink::fetch`].
    pub fn new(
        provider: Arc<dyn Provider>,
        id: impl Into<String>,
        key: KeyPair,
        known_hosts: PathBuf,
        source: Source,
    ) -> CloudLink {
        CloudLink {
            provider,
            id: id.into(),
            key,
            known_hosts,
            source,
            fetch: None,
            provision: Arc::new(Provision::default),
            progress: Arc::new(|_| {}),
            wake_deadline: WAKE_DEADLINE,
        }
    }

    /// The name the machine's host key is pinned under.
    pub fn alias(&self) -> String {
        alias(&self.id)
    }

    /// Wakes the machine when it sleeps, waits for it, registers the key, pins the host key, and returns how to reach
    /// it. Also what the app uses for a port forward ([`Target::forward`]) or a terminal.
    pub async fn reach(&self) -> Result<Target> {
        let provider = &*self.provider;
        let id = self.id.as_str();
        let progress = &self.progress;
        wake_up(provider, id, self.wake_deadline, &**progress).await?;
        progress(Progress::Preparing);
        let started = Instant::now();
        let access = loop {
            match provider.authorize_ssh(id, &self.key.public).await {
                Ok(access) => break access,
                Err(error) if error.is_retryable() && started.elapsed() < AUTHORIZE_DEADLINE => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(error) => return Err(error),
            }
        };
        let alias = self.alias();
        write_known_hosts(&self.known_hosts, &alias, 22, &access.host_key).map_err(Error::Other)?;
        Ok(Target::Direct {
            host: access.host,
            port: access.port,
            user: access.user,
            identity: self.key.private.clone(),
            known_hosts: self.known_hosts.clone(),
            alias,
        })
    }

    fn ssh_link(&self, target: Target) -> SshLink {
        SshLink {
            installer: Installer {
                target,
                source: self.source.clone(),
                fetch: self.fetch.clone(),
            },
        }
    }
}

impl Link for CloudLink {
    fn open(&self) -> BoxFuture<'_, nexus_remote::Result<Opened>> {
        Box::pin(async move {
            let target = self.reach().await?;
            provision(&target, &(self.provision)()).await?;
            self.ssh_link(target).open().await
        })
    }

    fn update(&self) -> BoxFuture<'_, nexus_remote::Result<()>> {
        Box::pin(async move {
            let target = self.reach().await?;
            self.ssh_link(target).update().await
        })
    }
}

/// Wakes machine `id` when it sleeps (or waits while it goes to sleep, then wakes it) and waits until it runs, saying
/// [`Progress`]: [`CloudLink::reach`]'s first part, and all of it for a machine reached through Gateway, which needs
/// no SSH key.
pub async fn wake_up(
    provider: &dyn Provider,
    id: &str,
    deadline: Duration,
    progress: &(dyn Fn(Progress) + Send + Sync),
) -> Result<Machine> {
    let mut machine = provider.get(id).await?;
    if machine.state == State::Stopping {
        progress(Progress::Waking);
        machine = wait_stopped(provider, id, deadline).await?;
    }
    let mut said_starting = false;
    let mut starting = |state: &State| {
        if matches!(state, State::Creating | State::Starting) && !said_starting {
            said_starting = true;
            progress(Progress::Starting);
        }
    };
    match &machine.state {
        State::Ready => Ok(machine),
        State::Sleeping if machine.data_expired_at.is_some() => Err(Error::DataExpired),
        State::Sleeping => {
            progress(Progress::Waking);
            provider.wake(id, None).await?;
            wait_ready(provider, id, deadline, &mut starting).await
        }
        State::Creating | State::Starting => wait_ready(provider, id, deadline, &mut starting).await,
        State::Stopping => Err(Error::Other("The cloud machine is still going to sleep.".into())),
        State::Failed { message } => Err(Error::Other(format!("The cloud machine failed: {message}"))),
        State::Gone => Err(Error::NotFound),
    }
}

/// The host-key alias for machine `id`.
pub fn alias(id: &str) -> String {
    format!("nexus-cloud-{id}")
}

/// `prepare <git name> <git email> <playwright package> <block begin> <block end>`, standard input four lines, each
/// base64 or `-` for none: the GitHub token, agent.env's content (or `=`: keep the file as it is), mcp.json's content,
/// the instructions block's content (in each agent's instructions file, its `CloudAgent::instructions`). Prints
/// `key=value` lines and `provisioned=yes` last. Every step is best effort except the two files: a machine without
/// its Claude token would start sessions that cannot sign in.
/// Public for the tests, which run it with input [`provision`] never sends.
#[doc(hidden)]
pub fn prepare() -> &'static str {
    static SCRIPT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let first_runs: String = first_run_files().iter().map(first_run_block).collect();
        let agents: String = instructions_files().iter().map(agent_md_line).collect();
        format!("{PREPARE_HEAD}{first_runs}{PREPARE_MIDDLE}{agents}{PREPARE_TAIL}")
    });
    &SCRIPT
}

/// PREPARE's block for one agent's first-run file ([`FIRST_RUN_BLOCK`] filled in): it sets the marks in
/// `~/<file>` and reports `<report>=set`, `already`, `unreadable`, `failed` or `skipped`. The shell variable is the
/// file's name without its leading dot, `.` and `-` as `_` (`.claude.json`: `claude_json`).
fn first_run_block(first_run: &FirstRun) -> String {
    let var: String = first_run
        .file
        .trim_start_matches('.')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let python = |value: bool| if value { "True" } else { "False" };
    let marks = first_run.marks;
    let py_marked: Vec<String> = marks
        .iter()
        .map(|(field, value)| format!("data.get(\"{field}\") is {}", python(*value)))
        .collect();
    let py_mark: String = marks
        .iter()
        .map(|(field, value)| format!("data[\"{field}\"] = {}\n", python(*value)))
        .collect();
    let js_marked: Vec<String> = marks
        .iter()
        .map(|(field, value)| format!("d.{field} === {value}"))
        .collect();
    let js_mark: Vec<String> = marks
        .iter()
        .map(|(field, value)| format!("d.{field} = {value};"))
        .collect();
    FIRST_RUN_BLOCK
        .replace("@PY_MARKED@", &py_marked.join(" and "))
        .replace("@PY_MARK@", &py_mark)
        .replace("@JS_MARKED@", &js_marked.join(" && "))
        .replace("@JS_MARK@", &js_mark.join(" "))
        .replace("@VAR@", &var)
        .replace("@FILE@", first_run.file)
        .replace("@REPORT@", first_run.report)
}

/// PREPARE's line for one agent's instructions file ([`InstructionsFile`], from its `CloudAgent`):
/// `agent_md_in "${CODEX_HOME:-$HOME/.codex}" AGENTS.md codexMd`.
fn agent_md_line(file: &InstructionsFile) -> String {
    let folder = match file.var {
        Some(var) => format!("${{{var}:-$HOME/{}}}", file.dir),
        None => format!("$HOME/{}", file.dir),
    };
    format!("agent_md_in \"{folder}\" {} {}\n", file.name, file.report)
}

/// PREPARE up to the agents' first-run files: one [`first_run_block`] per agent follows.
const PREPARE_HEAD: &str = r##"set -u
umask 077
root="${NEXUS_HOST_DIR:-$HOME/.nexus/host}"
cloud="$HOME/.nexus/cloud"
mkdir -p "$root" "$cloud" && chmod 700 "$root" "$cloud" 2>/dev/null
q() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
unpack() { [ "$1" = - ] && return 0; printf '%s' "$1" | base64 -d; }

IFS= read -r github64 || github64=-
IFS= read -r env64 || env64=-
IFS= read -r mcp64 || mcp64=-
IFS= read -r md64 || md64=-
github="$(unpack "$github64")" || github=""
# While the machine sets its repository up itself, the repository's `.agents/setup` runs as this user. Nothing
# secret is written until that setup is ready, failed or withdrawn (`provisioned=wait`: the app asks again). A machine
# whose host never saved a state (an older host) holds it only while `cloud.json` is new.
if [ -f "$root/cloud.json" ] && grep -q '"setup"' "$root/cloud.json" 2>/dev/null; then
  st="$(tr -d ' \n' < "$root/setup.json" 2>/dev/null | sed -n 's/.*"state":"\([a-z]*\)".*/\1/p')"
  case "$st" in
    ready|failed|withdrawn) ;;
    "") if [ -n "$(find "$root/cloud.json" -mmin -5 2>/dev/null)" ]; then echo "provisioned=wait"; exit 0; fi ;;
    *) echo "provisioned=wait"; exit 0 ;;
  esac
fi
# `write <name> <base64, - or =>`: $root/<name> from its content, 0600, removed for `-`, left as it is for `=`.
write() {
  if [ "$2" = = ]; then if [ -f "$root/$1" ]; then echo "$1=kept"; else echo "$1=none"; fi; return 0; fi
  if [ "$2" = - ]; then rm -f "$root/$1"; echo "$1=none"; return 0; fi
  if unpack "$2" > "$root/$1.part" && chmod 600 "$root/$1.part" && mv -f "$root/$1.part" "$root/$1"; then
    echo "$1=written"
  else
    rm -f "$root/$1.part"; echo "$1=failed"; echo "failed=$1"; exit 0
  fi
}
write agent.env "$env64"
write mcp.json "$mcp64"

if [ -f "$root/mcp.json" ] && grep -q '@playwright/mcp' "$root/mcp.json" 2>/dev/null; then
  if npm ls -g "$3" >/dev/null 2>&1; then echo "playwright=already"
  elif ! command -v npm >/dev/null 2>&1; then echo "playwright=no-npm"
  else
    log="$cloud/playwright-install.log"
    lim=""; command -v timeout >/dev/null 2>&1 && lim="timeout 90"
    if $lim npm install -g "$3" >>"$log" 2>&1 || $lim sudo -n npm install -g "$3" >>"$log" 2>&1; then
      echo "playwright=installed"
    else
      echo "playwright=failed"
    fi
  fi
fi

if [ -n "$1" ]; then git config --global user.name "$1" 2>/dev/null && echo "gitName=set" || echo "gitName=failed"; fi
if [ -n "$2" ]; then git config --global user.email "$2" 2>/dev/null && echo "gitEmail=set" || echo "gitEmail=failed"; fi

# The person's GitHub token from the app's keychain: `gh` signs in with it (from standard input, never the command
# line), and git uses `gh` for github.com. Only when the machine's own token differs.
if [ -n "$github" ]; then
  export GH_NO_UPDATE_NOTIFIER=1 GH_PROMPT_DISABLED=1
  if ! command -v gh >/dev/null 2>&1; then echo "github=no-gh"
  elif [ "$(gh auth token --hostname github.com 2>/dev/null)" = "$github" ]; then echo "github=already"
  elif printf '%s\n' "$github" | gh auth login --hostname github.com --git-protocol https --with-token >/dev/null 2>&1; then
    gh auth setup-git --hostname github.com >/dev/null 2>&1 || echo "setupGit=failed"
    echo "github=set"
  else
    echo "github=failed"
  fi
fi

me="$(id -un)"
if [ "$(loginctl show-user "$me" --property=Linger --value 2>/dev/null)" = yes ]; then echo "linger=already"
elif sudo -n loginctl enable-linger "$me" >/dev/null 2>&1; then echo "linger=yes"
else echo "linger=no"; fi

"##;

/// PREPARE's block for one agent's first-run file ([`FirstRun`], from its `CloudAgent`), [`first_run_block`] fills
/// it in: `@VAR@` is the shell variable that holds the file's path, `@FILE@` the file under the home folder,
/// `@REPORT@` the report field, `@PY_MARKED@` and `@JS_MARKED@` the test that every mark is set, and `@PY_MARK@` and
/// `@JS_MARK@` the statements that set them.
const FIRST_RUN_BLOCK: &str = r##"@VAR@="$HOME/@FILE@"
if command -v python3 >/dev/null 2>&1; then
  python3 - "$@VAR@" <<'NEXUS_PY' || echo "@REPORT@=failed"
import json, os, sys
path = os.path.realpath(sys.argv[1])
try:
    with open(path) as f:
        data = json.load(f)
except FileNotFoundError:
    data = {}
except Exception:
    print("@REPORT@=unreadable")
    sys.exit(0)
if not isinstance(data, dict):
    print("@REPORT@=unreadable")
    sys.exit(0)
if @PY_MARKED@:
    print("@REPORT@=already")
    sys.exit(0)
@PY_MARK@part = "%s.nexus-%d" % (path, os.getpid())
with open(part, "w") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
try:
    os.chmod(part, os.stat(path).st_mode & 0o7777)
except FileNotFoundError:
    os.chmod(part, 0o600)
os.replace(part, path)
print("@REPORT@=set")
NEXUS_PY
elif command -v node >/dev/null 2>&1; then
  node -e '
const fs = require("fs"); let p = process.argv[1]; try { p = fs.realpathSync(p); } catch (e) {} let d = {};
try { d = JSON.parse(fs.readFileSync(p, "utf8")); } catch (e) { if (e.code !== "ENOENT") { console.log("@REPORT@=unreadable"); process.exit(0); } }
if (!d || typeof d !== "object" || Array.isArray(d)) { console.log("@REPORT@=unreadable"); process.exit(0); }
if (@JS_MARKED@) { console.log("@REPORT@=already"); process.exit(0); }
@JS_MARK@
const part = p + ".nexus-" + process.pid; let mode = 0o600; try { mode = fs.statSync(p).mode & 0o7777; } catch (e) {}
fs.writeFileSync(part, JSON.stringify(d, null, 2) + "\n", { mode }); fs.chmodSync(part, mode); fs.renameSync(part, p);
console.log("@REPORT@=set");' "$@VAR@" || echo "@REPORT@=failed"
else
  echo "@REPORT@=skipped"
fi

"##;

/// PREPARE after the first-run files, up to the agents' instructions files: one [`agent_md_line`] per agent follows.
const PREPARE_MIDDLE: &str = r##"# The app's block in an agent's own instructions file (Claude Code's ~/.claude/CLAUDE.md, Codex's AGENTS.md), between
# the lines $4 and $5: the old block goes (a begin line without an end before the next begin or the end of the file is
# the person's text and stays), the new one goes last. A symlinked file (dotfiles) is written through to its target,
# which keeps its mode. Under a lock when `flock` exists, with a temporary file of its own, so two connects at once
# never lose the person's text. `agent_md <begin> <end> <file> <field>` reports as `<field>=…`.
agent_md() {
  link="$3"; key="$4"
  if [ "$md64" = - ] && ! grep -qF "$1" "$link" 2>/dev/null; then echo "$key=none"; return 0; fi
  block=""
  if [ "$md64" != - ]; then
    block="$(unpack "$md64")" && [ -n "$block" ] || { echo "$key=failed"; return 0; }
  fi
  md="$link"
  if [ -L "$link" ]; then md="$(readlink -f "$link")" && [ -n "$md" ] || { echo "$key=failed"; return 0; }; fi
  dir="$(dirname "$md")"
  mkdir -p "$dir" && part="$(mktemp "$dir/.$(basename "$md").nexus-XXXXXX")" || { echo "$key=failed"; return 0; }
  if { [ ! -f "$md" ] || awk -v b="$1" -v e="$2" '
      index($0, b) == 1 { for (i = 1; i <= held; i++) lines[++n] = hold[i]; inb = 1; held = 0; hold[++held] = $0; next }
      inb { hold[++held] = $0; if (index($0, e) == 1) { inb = 0; held = 0; gone = 1 } next }
      gone && $0 ~ /^[ \t]*$/ { gone = 0; next }
      { gone = 0; lines[++n] = $0 }
      END {
        if (inb) for (i = 1; i <= held; i++) lines[++n] = hold[i]
        while (n > 0 && lines[n] ~ /^[ \t]*$/) n--
        for (i = 1; i <= n; i++) print lines[i]
      }' "$md"; } > "$part" \
    && { [ -z "$block" ] || { if [ -s "$part" ]; then echo; fi; printf '%s\n%s\n%s\n' "$1" "$block" "$2"; } >> "$part"; }; then
    if [ -s "$part" ]; then
      mode="$(stat -c %a "$md" 2>/dev/null)" || mode="$(stat -f %Lp "$md" 2>/dev/null)" || mode=""
      [ -n "$mode" ] && chmod "$mode" "$part"
      mv -f "$part" "$md" && echo "$key=written" || { rm -f "$part"; echo "$key=failed"; }
    else
      # Nothing of the person's was left, and no block to add: the file only ever held ours. A symlink's target is
      # emptied, not deleted, so the link stays whole.
      rm -f "$part"
      if [ "$md" = "$link" ]; then rm -f "$md"; else : > "$md"; fi
      echo "$key=removed"
    fi
  else
    rm -f "$part"; echo "$key=failed"
  fi
}
# `agent_md_in <folder> <file name> <field>`: under the folder's lock.
agent_md_in() {
  if mkdir -p "$1"; then
    if command -v flock >/dev/null 2>&1; then
      ( flock -w 60 9 || { echo "$3=failed"; exit 0; }; agent_md "$begin" "$end" "$1/$2" "$3" ) 9>"$1/.nexus-md.lock"
    else
      agent_md "$begin" "$end" "$1/$2" "$3"
    fi
  else
    echo "$3=failed"
  fi
}
begin="$4"; end="$5"
"##;

/// PREPARE after the agents' instructions files.
const PREPARE_TAIL: &str = r##"
header() {
  echo '#!/bin/sh'
  echo "# Written by Nexus (nexus-cloud) on every connect; changes here are replaced."
  printf 'root=%s\n' "$(q "$root")"
  printf 'cloud=%s\n' "$(q "$cloud")"
  printf 'HOME=%s\n' "$(q "$HOME")"
  echo 'export HOME NEXUS_HOST_DIR="$root"'
  cat <<'NEXUS_EOF'
bin=""
pid="$(cat "$root/run/host.lock" 2>/dev/null)" || pid=""
if [ -n "$pid" ]; then bin="$(readlink "/proc/$pid/exe" 2>/dev/null)" || bin=""; fi
if [ -z "$bin" ] || [ ! -x "$bin" ]; then bin="$(ls -1t "$root"/*/nexus-host 2>/dev/null | head -n 1)" || bin=""; fi
NEXUS_EOF
}

mkdir -p "$HOME/.boat"
{
  header "$@"
  cat <<'NEXUS_EOF'
# boat runs this before it saves the disk to sleep: the host stops its agents as when the app quits, so its
# database is saved whole.
if [ -n "$bin" ] && [ -x "$bin" ]; then "$bin" stop >/dev/null 2>&1; fi
exit 0
NEXUS_EOF
} > "$HOME/.boat/pre-stop.part" && chmod 700 "$HOME/.boat/pre-stop.part" && mv -f "$HOME/.boat/pre-stop.part" "$HOME/.boat/pre-stop" \
  && echo "preStop=written" || echo "preStop=failed"

# Gateway decides when the machine sleeps now (the host's heartbeat): the idle timer an older app installed goes.
units="$HOME/.config/systemd/user"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
if [ -f "$units/nexus-cloud-idle.timer" ]; then
  systemctl --user disable --now nexus-cloud-idle.timer >/dev/null 2>&1
  rm -f "$units/nexus-cloud-idle.timer" "$units/nexus-cloud-idle.service"
  systemctl --user daemon-reload >/dev/null 2>&1
fi
rm -f "$cloud/stop-key.header" "$cloud/idle.sh"
echo "provisioned=yes"
"##;

/// How many times [`provision`] tries to get through (sshd may come up a moment after the provider says ready).
const PREPARE_TRIES: u32 = 8;

/// Prepares the machine at `target` ([`Provision`]; the script is [`prepare`]) and returns what it reported
/// (`agent.env`, `mcp.json`, `playwright`, `claudeMd`, `codexMd`, `gitName`, `gitEmail`, `github`, `linger`, `claude`,
/// `preStop`). Secrets travel on standard input only. Fails only when the machine cannot be reached or a file cannot be written.
pub async fn provision(target: &Target, provision: &Provision) -> Result<HashMap<String, String>> {
    let secret_line = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty() && !v.chars().any(|c| c.is_whitespace() || c.is_control()))
            .map(str::to_string)
    };
    let github_token = secret_line(&provision.github_token);
    let one_line = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.chars().any(char::is_control))
            .unwrap_or("")
            .to_string()
    };
    let git_name = one_line(&provision.git_name);
    let git_email = one_line(&provision.git_email);
    let line = |value: Option<&str>| match value {
        Some(value) => base64::engine::general_purpose::STANDARD.encode(value.as_bytes()),
        None => "-".to_string(),
    };
    let agent_env = match (&provision.agent_env, provision.keep_agent_env) {
        (None, true) => "=".to_string(),
        (value, _) => line(value.as_deref()),
    };
    let input = format!(
        "{}\n{}\n{}\n{}\n",
        line(github_token.as_deref()),
        agent_env,
        line(provision.mcp_json.as_deref()),
        line(provision.claude_md.as_deref())
    )
    .into_bytes();
    let args = [
        git_name.as_str(),
        git_email.as_str(),
        PLAYWRIGHT_MCP,
        CLAUDE_MD_BEGIN,
        CLAUDE_MD_END,
    ];
    let mut tries = 0;
    loop {
        tries += 1;
        let output = target
            .run(prepare(), &args, Some(input.clone()), Duration::from_secs(180))
            .await
            .map_err(Error::Other)?;
        let fields = parse(&output.stdout);
        if let Some(file) = fields.get("failed") {
            return Err(Error::Other(format!("Could not write {file} on {}.", target.name())));
        }
        if fields.get("provisioned").map(String::as_str) == Some("yes") {
            return Ok(fields);
        }
        if fields.get("provisioned").map(String::as_str) == Some("wait") {
            // The machine's own setup runs repository code: nothing secret is on it yet. Ask again until it ends.
            let target = target.clone();
            let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
            tokio::spawn(provision_after_setup(target, args, input));
            return Ok(fields);
        }
        if tries >= PREPARE_TRIES {
            let reason = ssh_error(&output.stderr).unwrap_or_else(|| "it closed the connection".into());
            return Err(Error::Other(format!(
                "Could not connect to {}: {reason}",
                target.name()
            )));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// How long the app holds the person's secrets back while the machine's own setup runs ([`prepare`] says `wait`).
const SETUP_HOLD: Duration = Duration::from_secs(20 * 60);

/// Runs [`prepare`] again every few seconds until it writes (the setup is ready, failed or withdrawn), or [`SETUP_HOLD`]
/// has passed: then the secrets stay off the machine, and the next connect provisions it.
async fn provision_after_setup(target: Target, args: Vec<String>, input: Vec<u8>) {
    let started = Instant::now();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    while started.elapsed() < SETUP_HOLD {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let Ok(output) = target
            .run(prepare(), &args, Some(input.clone()), Duration::from_secs(180))
            .await
        else {
            continue;
        };
        let fields = parse(&output.stdout);
        if fields.get("provisioned").map(String::as_str) != Some("wait") {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relays_setup_says_what_prepare_says() {
        let kept = Provision {
            keep_agent_env: true,
            ..Provision::default()
        }
        .host_provision();
        assert_eq!(kept, HostProvision::default(), "null keeps agent.env");
        assert_eq!(
            Provision::default().host_provision().agent_env.as_deref(),
            Some(""),
            "an empty text removes it"
        );
        let full = Provision {
            agent_env: Some("A=1\n".into()),
            keep_agent_env: true,
            mcp_json: Some("{}".into()),
            git_name: Some("Ada".into()),
            git_email: None,
            github_token: Some("gho_1".into()),
            claude_md: Some("# Hi".into()),
        }
        .host_provision();
        assert_eq!(
            serde_json::to_value(full).unwrap_or_default(),
            serde_json::json!({
                "github": { "token": "gho_1", "name": "Ada", "email": null },
                "agentEnv": "A=1\n",
                "mcp": "{}",
                "instructions": "# Hi",
            })
        );
    }

    #[test]
    fn progress_labels() {
        assert_eq!(Progress::Waking.label(), "Waking");
        assert_eq!(Progress::Starting.label(), "Starting");
        assert_eq!(
            serde_json::to_value(Progress::Preparing).unwrap_or_default(),
            serde_json::json!("preparing")
        );
        assert_eq!(alias("bx_23456789"), "nexus-cloud-bx_23456789");
    }

    /// PREPARE sets each agent's first-run marks where its `CloudAgent` says: Claude Code's, in `~/.claude.json`.
    #[test]
    fn prepare_sets_each_agents_first_run_marks() {
        let script = prepare();
        for line in [
            "claude_json=\"$HOME/.claude.json\"\n",
            "  python3 - \"$claude_json\" <<'NEXUS_PY' || echo \"claude=failed\"\n",
            "if data.get(\"hasCompletedOnboarding\") is True and data.get(\"autoUpdates\") is False:\n",
            "data[\"hasCompletedOnboarding\"] = True\ndata[\"autoUpdates\"] = False\npart = ",
            "if (d.hasCompletedOnboarding === true && d.autoUpdates === false) { console.log(\"claude=already\");",
            "\nd.hasCompletedOnboarding = true; d.autoUpdates = false;\nconst part = ",
            "console.log(\"claude=set\");' \"$claude_json\" || echo \"claude=failed\"\n",
            "  echo \"claude=skipped\"\nfi\n\n# The app's block",
        ] {
            assert_eq!(script.matches(line).count(), 1, "{line}");
        }
        for placeholder in ["@VAR@", "@FILE@", "@REPORT@", "@PY_MARK", "@JS_MARK"] {
            assert!(!script.contains(placeholder), "{placeholder} is left");
        }
        assert_eq!(script.matches("<<'NEXUS_PY'").count(), first_run_files().len());
    }

    /// Each first-run file's shell variable is a name of its own: PREPARE's other variables stay as they are.
    #[test]
    fn each_first_run_variable_is_its_own() {
        let taken = [
            "root", "cloud", "md", "md64", "env64", "mcp64", "github", "github64", "link", "key", "block", "dir",
            "part", "mode", "me", "st", "units", "log", "lim", "begin", "end", "bin", "pid", "HOME", "PATH",
        ];
        for first_run in first_run_files() {
            let block = first_run_block(&first_run);
            let var = block.split('=').next().unwrap_or_default();
            let plain = var.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            assert!(
                plain && !taken.contains(&var),
                "{} gives the variable {var}",
                first_run.file
            );
        }
    }

    /// PREPARE writes each agent's instructions file where its `CloudAgent` says, in the agents' order.
    #[test]
    fn prepare_writes_each_agents_instructions_file() {
        let lines = "begin=\"$4\"; end=\"$5\"\n\
                     agent_md_in \"$HOME/.claude\" CLAUDE.md claudeMd\n\
                     agent_md_in \"${CODEX_HOME:-$HOME/.codex}\" AGENTS.md codexMd\n\
                     \n\
                     header() {\n";
        assert_eq!(prepare().matches(lines).count(), 1, "{}", prepare());
        assert_eq!(prepare().matches("agent_md_in \"").count(), 2);
    }
}
