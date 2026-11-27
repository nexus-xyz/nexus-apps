//! Getting `nexus-host` onto a machine and running, in as few SSH round
//! trips as possible: one when the host is already there and running, four the first time a server cannot download.
//!
//! The host lives in `~/.nexus/host/<version>/nexus-host`, one folder per version, so the app always asks for exactly
//! its own version and an update installs beside the running one. Where it comes from ([`Source`]):
//!
//! - **A release** (the default): the version's own manifest on the downloads host,
//!   `desktop/<channel>/<semver>/latest.json`, lists `linux-x86_64-nexus-host` and `linux-aarch64-nexus-host`
//!   (scripts/updater-manifest.mjs): the static host, gzipped, with the `.gz`'s SHA-256 and its minisign
//!   signature. The app ignores the entry's `url`: the file is always the signed name beside the manifest, so a changed
//!   manifest cannot point a server's download at another origin. A server cannot check a signature, so an app that carries the updater's key first downloads the file
//!   and verifies it here, and refuses a host with a missing or bad signature. The server then downloads the file and
//!   compares it with that verified SHA-256; when it cannot (no internet, no curl or wget), the app copies its verified
//!   bytes over the same SSH. A private downloads host may require a credential, and a server
//!   never receives one, so there the copy over SSH is the usual path.
//!   Only a keyless build (a local or branch build, with no updater key) skips the signature.
//! - **A local build** (development, `NEXUS_HOST_BINARY`): copied over as is, into `dev-<its sha256>`.
//!
//! When the app's own version is not published (a local build of the next release, before it ships) or its manifest
//! cannot be read, a host already on the machine serves instead: the running one, else the
//! newest installed (a cloud image's baked host). With none, the channel's latest release is installed
//! (`desktop/<channel>/latest.json`). The hello's protocol check still refuses a host that does not fit, and such a host
//! is never silently updated ([`Ready::exact`]): there is no version of the app's own to update it to.
//!
//! Starting: through the host's own service when `nexus-host service install` put one there: `current`
//! is pointed at the version and the unit restarts, so the host also starts on boot. Where the person's user manager
//! outlives their logins (linger) and no service is there, the host installs one first. A host too old for that gets
//! `systemd-run --user`, as before. Without linger (a user unit is stopped at logout), `setsid nohup`, which survives
//! the SSH session closing. The host's token is read back each time (memory only, never saved on the laptop).
//!
//! "Update now" ([`Installer::update`]): stop the running host as quitting the app does (`nexus-host stop`), copy its
//! database aside, start the new version; if that does not start, put the database back and start the old binary
//! again. Then keep the two newest versions.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use nexus_desktop_updates::verify::verify_signed_file;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::Result;
use crate::ssh::{Output, Target, ssh_error};

/// Downloads a URL on this computer (the app's own HTTP client), refusing a body above the given size: the manifest,
/// and a host the server could not fetch. [`CappedBody`] holds the bytes as they arrive.
pub type Fetch = Arc<dyn Fn(String, u64) -> BoxFuture<'static, Result<Vec<u8>>> + Send + Sync>;

/// How long this computer may take to download a host, as long as the server gets for its own attempt.
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(300);
/// How long this computer may take to read a manifest.
const MANIFEST_LIMIT: Duration = Duration::from_secs(30);
/// The most a manifest may hold. A published one is about 6 KB.
const MANIFEST_MAX: u64 = 1024 * 1024;
/// The most a host's `.gz` may hold. A published one is about 4.5 MB. It is a fixed ceiling, because the manifest's
/// `size` is not signed: whoever can change the manifest can change that too.
const HOST_MAX: u64 = 64 * 1024 * 1024;

/// A download's bytes as they arrive, refused as soon as they pass the most it may hold.
#[derive(Debug)]
pub struct CappedBody {
    url: String,
    max: u64,
    bytes: Vec<u8>,
}

impl CappedBody {
    /// Starts a body for `url`. `declared` is the size the server announces, which already refuses it when too large.
    pub fn new(url: &str, max: u64, declared: Option<u64>) -> Result<CappedBody> {
        let body = CappedBody {
            url: url.to_string(),
            max,
            bytes: Vec::new(),
        };
        match declared {
            Some(size) if size > max => Err(body.too_large()),
            _ => Ok(body),
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<()> {
        if self.bytes.len() as u64 + chunk.len() as u64 > self.max {
            return Err(self.too_large());
        }
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }

    fn too_large(&self) -> String {
        format!("{} is larger than the {} bytes it may hold.", self.url, self.max)
    }
}

/// Where the host's file comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// The release of `version` (the app's own, `0.9.0-internal.42+abc1234`) whose manifest is at `manifest`. `pubkey`
    /// is the updater's public key compiled into the app; `None` only in a keyless build, which skips the signature.
    Release {
        version: String,
        manifest: String,
        pubkey: Option<String>,
    },
    /// A build on this computer (`NEXUS_HOST_BINARY`), for development before releases are published.
    Local { path: PathBuf },
}

/// Where release builds are, when `NEXUS_DOWNLOADS_URL` does not say.
pub const DOWNLOADS: &str = "https://downloads.nexus.inc";

impl Source {
    /// Where an app of `version` takes its host from: `NEXUS_HOST_BINARY` (a local build, for development before
    /// releases are published), else the release of `version` on the downloads host. `NEXUS_DOWNLOADS_URL`
    /// moves the downloads host, as scripts/install.sh reads it. Both are read in debug builds only
    /// ([`development_setting`](crate::development_setting)): a release build ignores them. `pubkey` is the app's
    /// updater key, `None` in a keyless build.
    pub fn app(version: &str, pubkey: Option<&str>) -> Source {
        match crate::development_setting("NEXUS_HOST_BINARY") {
            Some(path) => Source::Local {
                path: PathBuf::from(path),
            },
            None => {
                let downloads = crate::development_setting("NEXUS_DOWNLOADS_URL")
                    .map(|url| url.to_string_lossy().into_owned())
                    .unwrap_or_else(|| DOWNLOADS.to_string());
                Source::release(&downloads, version, pubkey)
            }
        }
    }

    /// The release of the app's `version` on `downloads` (by default [`DOWNLOADS`]): its channel is `internal`
    /// for `-internal.` builds, else `stable`; the manifest is that version's own copy, so the host is exactly the
    /// app's build.
    pub fn release(downloads: &str, version: &str, pubkey: Option<&str>) -> Source {
        let semver = semver(version);
        let channel = if semver.contains("-internal.") {
            "internal"
        } else {
            "stable"
        };
        Source::Release {
            version: version.to_string(),
            manifest: format!(
                "{}/desktop/{channel}/{semver}/latest.json",
                downloads.trim_end_matches('/')
            ),
            pubkey: pubkey.map(str::to_string),
        }
    }
}

/// The channel's own manifest (`…/desktop/<channel>/latest.json`) for a version's copy of it
/// (`…/desktop/<channel>/<semver>/latest.json`): what is installed when the version is not published.
pub fn channel_manifest(manifest: &str) -> Option<String> {
    let folder = manifest.strip_suffix("/latest.json")?;
    let (channel, _) = folder.rsplit_once('/')?;
    Some(format!("{channel}/latest.json"))
}

/// A host's file in a manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    /// The file beside the manifest ([`host_download`]), never the URL the manifest lists.
    pub url: String,
    pub sha256: String,
    /// The `.gz`'s minisign signature, in the updater's format.
    pub signature: Option<String>,
}

/// A version without its build (`0.9.0-internal.42+abc1234` is `0.9.0-internal.42`): what a manifest's path and a
/// signature name.
fn semver(version: &str) -> &str {
    version.split('+').next().unwrap_or(version)
}

/// The file name a host's signature names (scripts/updater-manifest.mjs lists no other).
fn host_file(version: &str, arch: &str) -> String {
    format!("nexus-host_{}_linux-{arch}.gz", semver(version))
}

/// Refuses a host the updater's key did not sign for this version and architecture.
fn check_signature(download: &Download, bytes: &[u8], pubkey: &str, version: &str, arch: &str) -> Result<()> {
    let signature = download.signature.as_deref().unwrap_or_default();
    if verify_signed_file(pubkey, signature, semver(version), &host_file(version, arch), bytes) {
        Ok(())
    } else {
        Err(format!(
            "The Nexus host downloaded from {} failed its signature check, so it was not installed.",
            download.url
        ))
    }
}

/// The manifest's `downloads` entry for a Linux `arch` (`x86_64`, `aarch64`) host (a `.gz`). `manifest_url` is where
/// that manifest came from, and `version` the app's: they give the file's URL, whatever the entry's own `url` says.
pub fn host_download(manifest: &Value, manifest_url: &str, version: &str, arch: &str) -> Option<Download> {
    let entry = manifest.get("downloads")?.get(format!("linux-{arch}-nexus-host"))?;
    let (folder, _) = manifest_url.rsplit_once('/')?;
    Some(Download {
        url: format!("{folder}/{}", host_file(version, arch)),
        sha256: entry.get("sha256")?.as_str()?.to_lowercase(),
        signature: entry.get("signature").and_then(Value::as_str).map(str::to_string),
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A running host, as the bootstrap left it.
#[derive(Debug, Clone, PartialEq)]
pub struct Ready {
    /// `uname -s` and the architecture (`x86_64`, `aarch64`).
    pub os: String,
    pub arch: String,
    /// The folder under `~/.nexus/host` of the host that serves: the version this app installs (its version, or
    /// `dev-…`), or, when that is not published, the host already on the machine or the channel's latest.
    pub dir: String,
    /// That binary's own version (`nexus-host version`): what a host of this app's version says in its hello.
    pub version: String,
    /// Whether `dir` is this app's own version. False when another host stands in for an unpublished one: then
    /// nothing should replace the running host, since this app's own cannot be installed.
    pub exact: bool,
    /// The token for the hello.
    pub token: String,
    /// How it was started now (`systemd`, `nohup`), or `already` when it was running.
    pub started: String,
}

/// The prelude every script shares: the host's folder, the architecture, and `start`, which starts `$1` detached and
/// waits until it answers.
pub(crate) const COMMON: &str = r#"set -u
root="${NEXUS_HOST_DIR:-$HOME/.nexus/host}"
umask 077
running() { "$1" status --json 2>/dev/null | grep -q '"running":true'; }
listening() { [ -S "$root/run/host.sock" ] || [ -f "$root/run/host.port" ]; }
# Whether the `status --json` on standard input says a host runs and answered. `status` says `"running":true` once
# a host holds its lock, and adds a top-level `"error"` key when that host did not answer (a dead host's socket, or
# one not bound yet). Every nexus-host version adds `error` this way. One older than 2026-10-30 has no 5 s limit on
# that answer, so its `status` can hang; only the caller's 300 s limit ends that.
answered() {
  awk '{ s = s $0 "\n" }
  END {
    # Scan the JSON once: q = in a string, e = after a backslash, t = the last string read, d = object and array
    # depth. Only keys at depth 1 count, so an error key inside a session never does.
    n = length(s)
    for (i = 1; i <= n; i++) {
      c = substr(s, i, 1)
      if (q) { if (e) e = 0; else if (c == "\\") e = 1; else if (c == "\"") q = 0; else t = t c; continue }
      if (c == "\"") { q = 1; t = "" }
      else if (c == "{" || c == "[") d++
      else if (c == "}" || c == "]") d--
      else if (c == ":" && d == 1) {
        if (t == "error") err = 1
        if (t == "running") { v = substr(s, i + 1); sub(/^[ \t\r\n]*/, "", v); run = (substr(v, 1, 4) == "true") }
      }
    }
    exit !(run && !err)
  }'
}
answering() { "$1" status --json 2>/dev/null | answered; }
nap() { sleep 0.2 2>/dev/null || sleep 1; }
lingers() { [ "$(loginctl show-user "$(id -un)" --property=Linger --value 2>/dev/null)" = yes ]; }
# The unit `nexus-host service install` wrote (its name is in `$root/service`), when systemd still has it enabled.
service_unit() {
  u="$(cat "$root/service" 2>/dev/null)"
  [ -n "$u" ] && command -v systemctl >/dev/null 2>&1 && systemctl --user is-enabled --quiet "$u" 2>/dev/null \
    && echo "$u"
}
# Never remove the socket or the port file here: a host started meanwhile (another install) may own them. `serve`
# removes a dead host's files itself, once it holds the lock.
# With the service installed, `$1` runs through it: `current` names `$1`'s folder, and the unit restarts. With none, and
# where a user unit outlives logins (linger), `$1` installs the service, so the host starts on boot. A host
# too old for `service`, or a failed install, leaves no enabled unit and falls through to the paths below.
# `service install` refuses a HOME that is not the account's own (a test's throwaway home), so tests enable no unit.
start() {
  mkdir -p "$root/run"
  log="$root/run/host.log"
  how=""
  name="$(basename "$(dirname "$1")")"
  if [ "$1" = "$root/$name/nexus-host" ] && [ "$name" != current ]; then
    if unit="$(service_unit)"; then
      rm -f "$root/current" && ln -s "$name" "$root/current" \
        && systemctl --user restart "$unit" >/dev/null 2>&1 && how=service
    elif command -v systemctl >/dev/null 2>&1 && lingers \
      && "$1" service install >/dev/null 2>&1 && service_unit >/dev/null; then
      how=service
    fi
  fi
  if [ -n "$how" ]; then
    :
  elif command -v systemd-run >/dev/null 2>&1 && lingers \
    && systemd-run --user --collect --quiet --unit="nexus-host-$(date +%s)-$$" \
      --setenv=HOME="$HOME" --setenv=NEXUS_HOST_DIR="$root" "$1" serve >/dev/null 2>&1; then
    how=systemd
  elif command -v setsid >/dev/null 2>&1; then
    setsid nohup "$1" serve >>"$log" 2>&1 </dev/null &
    how=nohup
  else
    nohup "$1" serve >>"$log" 2>&1 </dev/null &
    how=nohup
  fi
  # Wait by the clock, not by tries: a host that holds its lock but never answers makes each `status` take 5 s.
  # `date +%s` counts whole seconds, so the wait is 15 to 16 s: the old budget (75 naps of 0.2 s plus a `status` each).
  # It fits three such tries.
  end=$(($(date +%s) + 16))
  while [ "$(date +%s)" -lt "$end" ]; do
    if answering "$1" && listening; then echo "started=$how"; return 0; fi
    nap
  done
  echo "started=failed"
  echo "log=$(tail -n 3 "$log" 2>/dev/null | tr '\n' ' ')"
  # A host that is only slow still holds its database; stop it before anyone restores files under it.
  "$1" stop >/dev/null 2>&1
  return 1
}
"#;

/// `ensure <want> <dir> <url x86_64> <sha x86_64> <url aarch64> <sha aarch64> <most bytes> <reuse>`: install when missing
/// (download and check), start when not running, print the folder that serves and the token. `<want>` is this app's own
/// folder and serves when installed. `<dir>` is where a download goes: `<want>`, or the channel's latest when `<want>`
/// is not published. `<reuse>` 1: `<want>` is not published, so a host already there serves (the running one, else the
/// newest installed) before anything is downloaded. The download stops at `<most bytes>`, for a curl or wget that does
/// not stop by itself: before curl 8.4, `--max-filesize` works only when the server announces the size. A cut file fails
/// the SHA-256 check. Three curl attempts of 90 s fit inside the 300 s that `ensure_once` allows; a server too slow for
/// that gets this computer's bytes over SSH.
const ENSURE: &str = r#"
dir="$2"
os="$(uname -s)"; arch="$(uname -m)"
case "$arch" in x86_64|amd64) arch=x86_64 ;; aarch64|arm64) arch=aarch64 ;; esac
echo "os=$os"; echo "arch=$arch"
present() {
  pid="$(cat "$root/run/host.lock" 2>/dev/null)"
  exe=""
  [ -n "$pid" ] && exe="$(readlink "/proc/$pid/exe" 2>/dev/null)"
  # A local build (`dev-…`) never stands in for a release.
  case "$exe" in "$root"/dev-*/nexus-host) ;; "$root"/*/nexus-host) [ -x "$exe" ] && { basename "$(dirname "$exe")"; return 0; } ;; esac
  for name in $(ls -1t "$root" 2>/dev/null); do
    # `current` is the service's link to a version, never a version of its own.
    case "$name" in dev-*|current) continue ;; esac
    [ -x "$root/$name/nexus-host" ] && { echo "$name"; return 0; }
  done
  return 1
}
if [ -x "$root/$1/nexus-host" ]; then
  dir="$1"
elif [ "$8" = 1 ] && found="$(present)"; then
  dir="$found"
  echo "reused=yes"
fi
echo "dir=$dir"
mkdir -p "$root/$dir" || { echo "error=Could not create $root/$dir."; exit 0; }
chmod 700 "$root" "$root/$dir" 2>/dev/null
bin="$root/$dir/nexus-host"
if [ ! -x "$bin" ]; then
  case "$arch" in x86_64) url="$3"; want="$4" ;; aarch64) url="$5"; want="$6" ;; *) url=""; want="" ;; esac
  if [ -n "$url" ]; then
    part="$bin.part.$$"
    rm -f "$part"
    if command -v curl >/dev/null 2>&1; then curl -fsSL --retry 2 --connect-timeout 10 --speed-limit 1024 --speed-time 30 --max-time 90 --max-filesize "$7" "$url" 2>/dev/null
    elif command -v wget >/dev/null 2>&1; then wget -q -t 2 -T 10 -O - "$url" 2>/dev/null
    else false; fi | head -c "$7" > "$part"
    if command -v sha256sum >/dev/null 2>&1; then got="$(sha256sum "$part" 2>/dev/null | cut -d ' ' -f 1)"
    else got="$(shasum -a 256 "$part" 2>/dev/null | cut -d ' ' -f 1)"; fi
    if [ "$got" = "$want" ] && gzip -dc "$part" > "$bin.new.$$" 2>/dev/null; then
      chmod 700 "$bin.new.$$" && mv -f "$bin.new.$$" "$bin" && echo "downloaded=yes"
    else
      rm -f "$bin.new.$$"
      echo "downloaded=no"
    fi
    rm -f "$part"
  fi
fi
if [ ! -x "$bin" ]; then echo "installed=no"; exit 0; fi
echo "installed=yes"
touch "$root/$dir"
echo "binary=$("$bin" version 2>/dev/null | sed 's/^nexus-host //')"
if running "$bin"; then
  # A host still starting may sit on a dead host's socket or port file: wait for its answer, not for the file.
  # 9 to 10 s in whole seconds, the old 50 naps. A host that never answers is still reported "already", as before.
  end=$(($(date +%s) + 10))
  while [ "$(date +%s)" -lt "$end" ] && ! { answering "$bin" && listening; }; do nap; done
  echo "started=already"
else
  start "$bin" || exit 0
fi
echo "token=$(cat "$root/token" 2>/dev/null)"
"#;

/// `upload <dir> <gz>`: the file on standard input (gzipped when `<gz>` is 1) becomes `<dir>/nexus-host`.
const UPLOAD: &str = r#"
mkdir -p "$root/$1" && chmod 700 "$root" "$root/$1" 2>/dev/null
part="$root/$1/nexus-host.part.$$"
if [ "$2" = 1 ]; then gzip -dc > "$part"; else cat > "$part"; fi \
  && chmod 700 "$part" && mv -f "$part" "$root/$1/nexus-host" && echo "uploaded=yes"
rm -f "$part"
"#;

/// `update <dir>`: stop the running host, copy its database aside, start `<dir>`'s; roll back when it does not start;
/// keep the two newest versions.
const UPDATE: &str = r#"
dir="$1"
bin="$root/$dir/nexus-host"
[ -x "$bin" ] || { echo "error=The new Nexus host is not installed."; exit 0; }
pid="$(cat "$root/run/host.lock" 2>/dev/null)"
old=""
[ -n "$pid" ] && old="$(readlink "/proc/$pid/exe" 2>/dev/null)"
"$bin" stop >/dev/null 2>&1 || { echo "error=Could not stop the running Nexus host."; exit 0; }
rm -f "$root"/workbench.sqlite3*.before-*
for f in workbench.sqlite3 workbench.sqlite3-wal workbench.sqlite3-shm; do
  if [ -f "$root/$f" ]; then cp -p "$root/$f" "$root/$f.before-$dir"; fi
done
if start "$bin"; then
  echo "updated=yes"
else
  for f in workbench.sqlite3 workbench.sqlite3-wal workbench.sqlite3-shm; do
    if [ -f "$root/$f.before-$dir" ]; then cp -p "$root/$f.before-$dir" "$root/$f"; else rm -f "$root/$f"; fi
  done
  if [ -n "$old" ] && [ -x "$old" ]; then start "$old" >/dev/null; fi
  echo "updated=rolled-back"
  exit 0
fi
kept=0
for name in $(ls -1t "$root"); do
  [ -x "$root/$name/nexus-host" ] || continue
  [ "$name" = "$dir" ] || [ "$name" = current ] && continue
  kept=$((kept + 1))
  [ "$kept" -le 1 ] && continue
  rm -rf "${root:?}/$name"
done
echo "token=$(cat "$root/token" 2>/dev/null)"
"#;

/// `proxy <dir>`: the host's socket on standard input and output.
const PROXY: &str = r#"exec "$root/$1/nexus-host" proxy
"#;

/// The script's `key=value` lines (the last of each key wins).
pub fn parse(stdout: &str) -> HashMap<String, String> {
    stdout
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Whether a download's error says the file is not there (HTTP 404), as against a failure to read it.
fn not_published(error: &str) -> bool {
    error.contains("(404 ")
}

/// Whether `host` is an older version than `latest`, by semver precedence (build metadata after `+` ignored). A
/// version that does not read as one is older: nothing says it is fit to serve.
fn older(host: &str, latest: &str) -> bool {
    fn parse(version: &str) -> Option<([u64; 3], Vec<String>)> {
        let version = version.trim().split('+').next()?;
        let (core, pre) = match version.split_once('-') {
            Some((core, pre)) => (core, pre.split('.').map(str::to_string).collect()),
            None => (version, Vec::new()),
        };
        let mut numbers = core.split('.').map(|n| n.parse::<u64>().ok());
        let found = [numbers.next()??, numbers.next()??, numbers.next()??];
        numbers.next().is_none().then_some((found, pre))
    }
    let (Some((host, host_pre)), Some((latest, latest_pre))) = (parse(host), parse(latest)) else {
        return true;
    };
    if host != latest {
        return host < latest;
    }
    // Same numbers: a release outranks its pre-releases; pre-release parts compare one by one.
    match (host_pre.is_empty(), latest_pre.is_empty()) {
        (true, _) => false,
        (false, true) => true,
        (false, false) => {
            for (a, b) in host_pre.iter().zip(&latest_pre) {
                let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                    (Ok(a), Ok(b)) => a.cmp(&b),
                    (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                    (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                    (Err(_), Err(_)) => a.cmp(b),
                };
                if order != std::cmp::Ordering::Equal {
                    return order == std::cmp::Ordering::Less;
                }
            }
            host_pre.len() < latest_pre.len()
        }
    }
}

/// One download on this computer, with a deadline and a size limit, even for a [`Fetch`] that ignores its limit.
async fn fetch_within(fetch: &Fetch, url: &str, max: u64, limit: Duration) -> Result<Vec<u8>> {
    let bytes = tokio::time::timeout(limit, fetch(url.to_string(), max))
        .await
        .map_err(|_| format!("Downloading {url} took too long."))??;
    let mut body = CappedBody::new(url, max, None)?;
    body.push(&bytes)?;
    Ok(body.finish())
}

/// Installs, starts, updates and reaches the host on one machine.
#[derive(Clone)]
pub struct Installer {
    pub target: Target,
    pub source: Source,
    /// Downloads on this computer; None: a release can only be downloaded by the server itself.
    pub fetch: Option<Fetch>,
}

impl Installer {
    fn script(body: &str) -> String {
        format!("{COMMON}{body}")
    }

    async fn run(&self, body: &str, args: &[&str], input: Option<Vec<u8>>, limit: Duration) -> Result<Output> {
        let output = self.target.run(&Installer::script(body), args, input, limit).await?;
        if !output.ok {
            let reason = ssh_error(&output.stderr).unwrap_or_else(|| "it closed the connection".into());
            return Err(format!("Could not connect to {}: {reason}", self.target.name()));
        }
        Ok(output)
    }

    /// The manifest, from this computer.
    async fn manifest(&self, url: &str) -> Result<Value> {
        let fetch = self
            .fetch
            .as_ref()
            .ok_or_else(|| "No way to download on this computer.".to_string())?;
        let bytes = fetch_within(fetch, url, MANIFEST_MAX, MANIFEST_LIMIT).await?;
        serde_json::from_slice(&bytes).map_err(|e| format!("The release manifest at {url} is not JSON: {e}"))
    }

    /// The folder a local build goes in: `dev-` and the start of its SHA-256.
    async fn local_dir(path: &PathBuf) -> Result<(String, Vec<u8>)> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| format!("Could not read the local Nexus host {}: {e}", path.display()))?;
        let dir = format!("dev-{}", &sha256_hex(&bytes)[..16]);
        Ok((dir, bytes))
    }

    /// Copies `bytes` over as `<dir>/nexus-host`; a release's file is gzipped (`gz`), a local build is not.
    async fn upload(&self, dir: &str, bytes: Vec<u8>, gz: bool) -> Result<()> {
        let gz = if gz { "1" } else { "0" };
        let output = self
            .run(UPLOAD, &[dir, gz], Some(bytes), Duration::from_secs(600))
            .await?;
        if parse(&output.stdout).get("uploaded").map(String::as_str) != Some("yes") {
            return Err(format!(
                "Could not copy the Nexus host to {}: {}",
                self.target.name(),
                ssh_error(&output.stderr).unwrap_or_else(|| "no room or no permission?".into())
            ));
        }
        Ok(())
    }

    async fn ensure_once(
        &self,
        want: &str,
        dir: &str,
        downloads: &[Option<Download>; 2],
        reuse: bool,
    ) -> Result<HashMap<String, String>> {
        let field = |i: usize, f: fn(&Download) -> &str| downloads[i].as_ref().map(f).unwrap_or("").to_string();
        let args = [
            want.to_string(),
            dir.to_string(),
            field(0, |d| &d.url),
            field(0, |d| &d.sha256),
            field(1, |d| &d.url),
            field(1, |d| &d.sha256),
            HOST_MAX.to_string(),
            if reuse { "1" } else { "0" }.to_string(),
        ];
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.run(ENSURE, &args, None, Duration::from_secs(300)).await?;
        let fields = parse(&output.stdout);
        if let Some(error) = fields.get("error") {
            return Err(error.clone());
        }
        Ok(fields)
    }

    /// The folder under `~/.nexus/host` this app's host goes in (its version, or `dev-…` for a local build).
    pub async fn dir(&self) -> Result<String> {
        match &self.source {
            Source::Local { path } => Installer::local_dir(path).await.map(|(dir, _)| dir),
            Source::Release { version, .. } => Ok(version.clone()),
        }
    }

    /// A release's file for `arch`, and its bytes when this computer already downloaded them to verify them.
    async fn release_file(
        &self,
        dir: &str,
        arch: &str,
        (version, manifest, pubkey): (&str, &str, Option<&str>),
    ) -> Result<(Download, Option<Vec<u8>>)> {
        let json = self
            .manifest(manifest)
            .await
            .map_err(|problem| format!("The Nexus host {dir} is not available: {problem}"))?;
        let download = host_download(&json, manifest, version, arch)
            .ok_or_else(|| format!("The Nexus host {dir} has no Linux {arch} build."))?;
        let Some(pubkey) = pubkey else {
            return Ok((download, None));
        };
        if download.signature.is_none() {
            return Err(format!(
                "The Nexus host {dir} has no signature in its manifest, so it was not installed."
            ));
        }
        let bytes = self.download_here(&download).await?;
        check_signature(&download, &bytes, pubkey, version, arch)?;
        Ok((download, Some(bytes)))
    }

    /// This app's release, as [`Installer::release_file`] reads it; `None` for a local build.
    fn own_release(&self) -> Option<(&str, &str, Option<&str>)> {
        match &self.source {
            Source::Release {
                version,
                manifest,
                pubkey,
            } => Some((version.as_str(), manifest.as_str(), pubkey.as_deref())),
            Source::Local { .. } => None,
        }
    }

    /// Makes sure `dir` holds a host and starts it when no host runs. Nothing is downloaded when it is already there.
    /// `want` is this app's own folder: it serves when installed. `release` is where `dir`'s file comes from (a local
    /// build's is the source's own). `reuse`: a host already there may serve in place of `want`.
    async fn install(
        &self,
        want: &str,
        dir: &str,
        release: Option<(&str, &str, Option<&str>)>,
        reuse: bool,
    ) -> Result<HashMap<String, String>> {
        let installed = |fields: &HashMap<String, String>| fields.get("installed").map(String::as_str) == Some("yes");
        let fields = self.ensure_once(want, dir, &[None, None], reuse).await?;
        if installed(&fields) {
            return Ok(fields);
        }
        let (bytes, gz) = match (&self.source, release) {
            (Source::Local { path }, _) => (Installer::local_dir(path).await?.1, false),
            (Source::Release { .. }, Some(release)) => {
                let os = fields.get("os").cloned().unwrap_or_default();
                let arch = fields.get("arch").cloned().unwrap_or_default();
                if os != "Linux" || !["x86_64", "aarch64"].contains(&arch.as_str()) {
                    return Err(format!(
                        "{} runs {os} on {arch}: the Nexus host runs on Linux (x86-64 or arm64) for now.",
                        self.target.name()
                    ));
                }
                let (download, verified) = self.release_file(dir, &arch, release).await?;
                let mut downloads = [None, None];
                downloads[usize::from(arch == "aarch64")] = Some(download.clone());
                let fields = self.ensure_once(want, dir, &downloads, reuse).await?;
                if installed(&fields) {
                    return Ok(fields);
                }
                let bytes = match verified {
                    Some(bytes) => bytes,
                    None => self.download_here(&download).await?,
                };
                (bytes, true)
            }
            (Source::Release { .. }, None) => return Err(format!("The Nexus host {dir} has no release to install.")),
        };
        self.upload(dir, bytes, gz).await?;
        self.ensure_once(want, dir, &[None, None], reuse).await
    }

    /// The channel's latest release: its version, and that version's own manifest.
    async fn latest(&self) -> Result<(String, String)> {
        let Source::Release { manifest, .. } = &self.source else {
            return Err("a local build has no channel.".to_string());
        };
        let url = channel_manifest(manifest).ok_or_else(|| format!("{manifest} is not a version's manifest."))?;
        let json = self.manifest(&url).await?;
        let version = match json.get("version").and_then(Value::as_str) {
            // A folder name: never empty, a path or a dot file.
            Some(v) if !v.is_empty() && !v.contains('/') && !v.starts_with('.') => v.to_string(),
            _ => return Err(format!("{url} names no version.")),
        };
        let folder = url.strip_suffix("/latest.json").unwrap_or(&url);
        let own = format!("{folder}/{}/latest.json", semver(&version));
        Ok((version, own))
    }

    /// Installs the channel's latest release, signature checked as any release is, when `want` is not published.
    /// `problem` is why `want` cannot be installed.
    async fn install_latest(&self, want: &str, problem: &str) -> Result<(String, HashMap<String, String>)> {
        let name = self.target.name();
        let problem = problem
            .strip_prefix(&format!("The Nexus host {want} is not available: "))
            .unwrap_or(problem);
        let pubkey = self.own_release().and_then(|(_, _, pubkey)| pubkey);
        let (version, manifest) = self.latest().await.map_err(|error| {
            format!("The Nexus host {want} is not available, and {name} has no other Nexus host: {problem}; the channel's latest release: {error}")
        })?;
        let fields = self
            .install(want, &version, Some((&version, &manifest, pubkey)), false)
            .await
            .map_err(|error| {
                format!("The Nexus host {want} is not available, and {name} has no other Nexus host: {problem}; the channel's latest release: {error}")
            })?;
        Ok((version, fields))
    }

    /// Installs this app's own host: the one of its version, else the channel's latest when that is not published.
    /// Never a host already there. Its folder, and the script's fields.
    async fn install_own_or_latest(&self) -> Result<(String, HashMap<String, String>)> {
        let want = self.dir().await?;
        match self.install(&want, &want, self.own_release(), false).await {
            Ok(fields) => Ok((want, fields)),
            Err(error) if self.own_release().is_some() && not_published(&error) => {
                self.install_latest(&want, &error).await
            }
            Err(error) => Err(error),
        }
    }

    /// Makes sure a host is installed and running there, and reads its token: this app's own version, or, when that
    /// is not published, a host already there or the channel's latest release.
    pub async fn ensure(&self) -> Result<Ready> {
        let want = self.dir().await?;
        let problem = match self.install(&want, &want, self.own_release(), false).await {
            Ok(fields) => return Installer::ready(&want, &want, &fields, self.target.name()),
            // Only "not published" lets another host stand in. A network failure or an error page may be a published
            // version that could not be read: that connection fails, as it did before reuse existed.
            Err(error) if self.own_release().is_some() && not_published(&error) => error,
            Err(error) => return Err(error),
        };
        let fields = self.ensure_once(&want, &want, &[None, None], true).await?;
        if fields.get("installed").map(String::as_str) != Some("yes") {
            let (dir, fields) = self.install_latest(&want, &problem).await?;
            return Installer::ready(&want, &dir, &fields, self.target.name());
        }
        if fields.get("reused").map(String::as_str) == Some("yes") {
            let version = fields.get("binary").map(String::as_str).unwrap_or("unknown");
            let dir = fields.get("dir").map(String::as_str).unwrap_or("");
            // The floor: a host older than the channel's latest does not serve. Unknown when the channel's manifest
            // cannot be read: nothing to compare.
            if let Ok((latest, _)) = self.latest().await
                && older(version, &latest)
            {
                eprintln!(
                    "nexus-remote: {}: the host {dir} ({version}) on the machine is older than the channel's latest {latest}; replacing it",
                    self.target.name()
                );
                return self.update().await;
            }
            eprintln!(
                "nexus-remote: {}: this app's host {want} is not published; the host {dir} ({version}) on the machine serves",
                self.target.name()
            );
        }
        let dir = fields.get("dir").cloned().unwrap_or_else(|| want.clone());
        Installer::ready(&want, &dir, &fields, self.target.name())
    }

    fn ready(want: &str, dir: &str, fields: &HashMap<String, String>, name: &str) -> Result<Ready> {
        let get = |key: &str| fields.get(key).cloned().unwrap_or_default();
        if get("installed") != "yes" {
            return Err(format!("Could not install the Nexus host on {name}."));
        }
        if get("started") == "failed" {
            return Err(format!("The Nexus host on {name} did not start: {}", get("log")));
        }
        let token = get("token");
        if token.len() < 32 {
            return Err(format!("Could not read the Nexus host's token on {name}."));
        }
        let dir = fields.get("dir").map(String::as_str).unwrap_or(dir);
        Ok(Ready {
            os: get("os"),
            arch: get("arch"),
            exact: dir == want,
            dir: dir.to_string(),
            version: get("binary"),
            token,
            started: get("started"),
        })
    }

    /// This computer downloads the file and checks it against its manifest's SHA-256.
    async fn download_here(&self, download: &Download) -> Result<Vec<u8>> {
        let fetch = self
            .fetch
            .as_ref()
            .ok_or_else(|| "No way to download the Nexus host on this computer.".to_string())?;
        let bytes = fetch_within(fetch, &download.url, HOST_MAX, DOWNLOAD_LIMIT).await?;
        let got = sha256_hex(&bytes);
        if got != download.sha256 {
            return Err(format!(
                "The Nexus host downloaded from {} does not match its manifest (sha256 {got}).",
                download.url
            ));
        }
        Ok(bytes)
    }

    /// "Update now": replaces whatever host runs with this app's (installing it first if needed). Running sessions
    /// stop as when the app quits. Returns the new host's token.
    pub async fn update(&self) -> Result<Ready> {
        let want = self.dir().await?;
        // Installed (and, since a host already runs, not started): ensure only installs here. Never the host already
        // there: an update installs this app's version, or the channel's latest when that is not published.
        let (dir, mut fields) = self.install_own_or_latest().await?;
        let output = self.run(UPDATE, &[&dir], None, Duration::from_secs(120)).await?;
        let updated = parse(&output.stdout);
        if let Some(error) = updated.get("error") {
            return Err(error.clone());
        }
        match updated.get("updated").map(String::as_str) {
            Some("yes") => {}
            Some("rolled-back") => {
                return Err(format!(
                    "The new Nexus host did not start on {}; the previous one runs again.",
                    self.target.name()
                ));
            }
            _ => return Err(format!("Could not update the Nexus host on {}.", self.target.name())),
        }
        fields.insert("token".into(), updated.get("token").cloned().unwrap_or_default());
        fields.insert("started".into(), "updated".into());
        Installer::ready(&want, &dir, &fields, self.target.name())
    }

    /// `ssh … nexus-host proxy`: the running host's socket as this child's standard input and output.
    pub fn proxy(&self, dir: &str) -> Result<tokio::process::Child> {
        let mut command = self.target.command(&Installer::script(PROXY), &[dir])?;
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        command
            .spawn()
            .map_err(|e| format!("Could not run ssh: {e}. Is OpenSSH installed?"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Writes the script `body` at `path` with `mode`. Written by a child `sh`, so this process never holds the file
    /// open while another test's child runs it (ETXTBSY), as the probe tests' fake agents are.
    #[cfg(unix)]
    fn write_script(path: &std::path::Path, body: &str, mode: &str) {
        let written = nexus_workbench::process::command("sh")
            .arg("-c")
            .arg(r#"printf '%s' "$2" > "$1" && chmod "$3" "$1""#)
            .arg("sh")
            .arg(path)
            .arg(body)
            .arg(mode)
            .status();
        assert!(written.is_ok_and(|status| status.success()), "{}", path.display());
    }

    #[test]
    fn a_release_reads_its_own_versions_manifest() {
        assert_eq!(
            Source::release("https://downloads.example.test/", "0.9.0-internal.42+abc1234", None),
            Source::Release {
                version: "0.9.0-internal.42+abc1234".into(),
                manifest: "https://downloads.example.test/desktop/internal/0.9.0-internal.42/latest.json".into(),
                pubkey: None,
            }
        );
        assert!(matches!(
            Source::release("https://d.test", "1.0.0+abc", None),
            Source::Release { manifest, .. } if manifest == "https://d.test/desktop/stable/1.0.0/latest.json"
        ));
    }

    #[test]
    fn finds_the_hosts_in_the_manifest() {
        let manifest = json!({ "downloads": {
            "linux-x86_64-nexus-host": { "url": "https://d.test/x.gz", "sha256": "AB12", "size": 3, "signature": "c2ln" },
            "linux-x86_64-deb": { "url": "https://d.test/deb", "sha256": "cd", "size": 3 },
        }});
        assert_eq!(
            host_download(&manifest, MANIFEST, "1.0.0+abc", "x86_64"),
            Some(Download {
                url: "https://d.test/desktop/stable/1.0.0/nexus-host_1.0.0_linux-x86_64.gz".into(),
                sha256: "ab12".into(),
                signature: Some("c2ln".into()),
            })
        );
        assert_eq!(host_download(&manifest, MANIFEST, "1.0.0+abc", "aarch64"), None);
        assert_eq!(host_download(&json!({}), MANIFEST, "1.0.0+abc", "x86_64"), None);
    }

    #[test]
    fn older_follows_semver_and_ignores_the_build() {
        assert!(older("0.8.0+abc", "0.9.0"));
        assert!(older("0.9.0-internal.3", "0.9.0"));
        assert!(older("0.9.0-internal.3+abc", "0.9.0-internal.12"));
        assert!(older("garbage", "0.9.0"));
        assert!(!older("0.9.0+abc", "0.9.0"));
        assert!(!older("0.10.0-internal.388+abc1234", "0.9.0"));
        assert!(!older("1.0.0", "0.9.9"));
    }

    #[test]
    fn the_channels_manifest_is_beside_the_versions() {
        assert_eq!(
            channel_manifest("https://d.test/desktop/stable/0.10.0/latest.json").as_deref(),
            Some("https://d.test/desktop/stable/latest.json")
        );
        assert_eq!(channel_manifest("https://d.test/x.json"), None);
    }

    /// A published manifest of `1.0.0-internal.1`: the URL the app computes is the one
    /// the release lists, for each architecture.
    #[test]
    fn a_published_release_lists_the_url_the_app_computes() {
        let manifest_url = "https://downloads.example.test/desktop/internal/1.0.0-internal.1/latest.json";
        for arch in ["x86_64", "aarch64"] {
            let listed = format!(
                "https://downloads.example.test/desktop/internal/1.0.0-internal.1/nexus-host_1.0.0-internal.1_linux-{arch}.gz"
            );
            let manifest = json!({ "downloads": {
                format!("linux-{arch}-nexus-host"): { "url": listed, "sha256": "ab" },
            }});
            let download = host_download(&manifest, manifest_url, "1.0.0-internal.1+abc1234", arch);
            assert_eq!(download.map(|d| d.url), Some(listed));
        }
    }

    #[test]
    fn a_body_stops_at_its_limit() {
        assert!(CappedBody::new("u", 4, Some(5)).is_err_and(|e| e.contains("larger")));
        let mut body = CappedBody::new("u", 4, Some(4)).expect("a size at the limit");
        assert_eq!(body.push(b"abc"), Ok(()));
        assert_eq!(body.push(b"d"), Ok(()));
        assert_eq!(body.finish(), b"abcd");
        // A server that announces nothing, or too little, is stopped by what it sends.
        let mut body = CappedBody::new("u", 4, None).expect("no size");
        assert!(body.push(b"abcde").is_err_and(|e| e.contains("larger")));
        let mut body = CappedBody::new("u", 4, Some(1)).expect("a small size");
        assert_eq!(body.push(b"abc"), Ok(()));
        assert!(body.push(b"de").is_err());
    }

    /// An installer whose `Fetch` answers each URL from `answer`, noting the limit it was given.
    fn fetching(
        pubkey: Option<&str>,
        answer: impl Fn(&str, u64) -> BoxFuture<'static, Result<Vec<u8>>> + Send + Sync + 'static,
    ) -> (Installer, Arc<std::sync::Mutex<Vec<u64>>>) {
        let limits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = limits.clone();
        let fetch: Fetch = Arc::new(move |url: String, max| {
            log.lock().unwrap_or_else(|p| p.into_inner()).push(max);
            answer(&url, max)
        });
        let installer = Installer {
            target: no_target(),
            source: Source::release(DOWNLOADS, "1.0.0+abc", pubkey),
            fetch: Some(fetch),
        };
        (installer, limits)
    }

    fn signed_manifest() -> Vec<u8> {
        json!({ "downloads": { "linux-x86_64-nexus-host": entry(FILE, Some(SIGNATURE.trim())) } })
            .to_string()
            .into_bytes()
    }

    #[tokio::test]
    async fn each_download_asks_for_its_own_limit() {
        let (installer, limits) = fetching(Some(KEY), |url, _| {
            let answer = if url.ends_with("/latest.json") {
                signed_manifest()
            } else {
                HOST.to_vec()
            };
            Box::pin(async move { Ok(answer) })
        });
        file_for(&installer).await.expect("the signed host");
        assert_eq!(
            *limits.lock().unwrap_or_else(|p| p.into_inner()),
            vec![MANIFEST_MAX, HOST_MAX]
        );
    }

    #[tokio::test]
    async fn a_body_past_its_limit_is_refused_whatever_the_fetch_does() {
        let (installer, _) = fetching(None, |_, max| {
            let answer = vec![0; max as usize + 1];
            Box::pin(async move { Ok(answer) })
        });
        assert!(file_for(&installer).await.is_err_and(|e| e.contains("larger")));
        let (installer, _) = fetching(Some(KEY), |url, max| {
            let answer = if url.ends_with("/latest.json") {
                signed_manifest()
            } else {
                vec![0; max as usize + 1]
            };
            Box::pin(async move { Ok(answer) })
        });
        assert!(file_for(&installer).await.is_err_and(|e| e.contains("larger")));
    }

    /// Fails, rather than hangs, when a download's own deadline is gone. The clock is paused, so this costs no real time.
    async fn within_outer_limit<T>(work: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(DOWNLOAD_LIMIT + MANIFEST_LIMIT + Duration::from_secs(1), work)
            .await
            .expect("the download's own deadline should have ended it first")
    }

    #[tokio::test(start_paused = true)]
    async fn a_manifest_that_never_comes_runs_out_of_time() {
        let (installer, _) = fetching(Some(KEY), |_, _| Box::pin(std::future::pending()));
        let started = tokio::time::Instant::now();
        let result = within_outer_limit(file_for(&installer)).await;
        assert!(result.is_err_and(|e| e.contains("took too long")));
        assert_eq!(started.elapsed(), MANIFEST_LIMIT);
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_that_never_comes_runs_out_of_time() {
        let (installer, _) = fetching(Some(KEY), |url, _| {
            if url.ends_with("/latest.json") {
                let manifest = signed_manifest();
                Box::pin(async move { Ok(manifest) })
            } else {
                Box::pin(std::future::pending())
            }
        });
        let started = tokio::time::Instant::now();
        let result = within_outer_limit(file_for(&installer)).await;
        assert!(result.is_err_and(|e| e.contains("took too long")));
        assert_eq!(started.elapsed(), DOWNLOAD_LIMIT);
    }

    #[tokio::test]
    async fn a_url_in_the_manifest_is_never_asked_for() {
        let elsewhere = "https://another-origin.test/host.gz";
        let (installer, asked) = installer(no_target(), Some(KEY), entry(elsewhere, Some(SIGNATURE.trim())), HOST);
        let (download, _) = file_for(&installer).await.expect("the signed host");
        assert_eq!(download.url, SIGNED_FILE);
        assert_eq!(
            *asked.lock().unwrap_or_else(|p| p.into_inner()),
            vec![MANIFEST.to_string(), SIGNED_FILE.to_string()]
        );
    }

    #[test]
    fn parses_the_scripts_lines() {
        let fields = parse("os=Linux\nnoise\ntoken=abc=def\nstarted=already\n");
        assert_eq!(fields.get("os").map(String::as_str), Some("Linux"));
        assert_eq!(fields.get("token").map(String::as_str), Some("abc=def"));
        assert_eq!(fields.get("noise"), None);
        assert_eq!(sha256_hex(b"abc").len(), 64);
    }

    // A stand-in host signed with a disposable key; its private key was not kept. minisign wrote both
    // `.sig` files the way the release pipeline does, over the same bytes. Each trusted comment names version 1.0.0 and
    // its own file, so the aarch64 one differs only in the file it names.
    const HOST: &[u8] = include_bytes!("fixtures/nexus-host_1.0.0_linux-x86_64.gz");
    const SIGNATURE: &str = include_str!("fixtures/nexus-host_1.0.0_linux-x86_64.gz.sig");
    const SIGNATURE_AARCH64: &str = include_str!("fixtures/nexus-host_1.0.0_linux-aarch64.gz.sig");
    const KEY: &str = include_str!("fixtures/test.pub");
    /// The updater's test key: a real key that did not sign this host.
    const OTHER_KEY: &str = include_str!("../../nexus-desktop-updates/fixtures/test.pub");
    const OTHER_SIGNATURE: &str = include_str!("../../nexus-desktop-updates/fixtures/update.txt.sig");
    const DOWNLOADS: &str = "https://d.test";
    /// A downloads host no server can reach: the discard port on this computer.
    #[cfg(unix)]
    const NOBODY: &str = "http://127.0.0.1:9";
    const MANIFEST: &str = "https://d.test/desktop/stable/1.0.0/latest.json";
    const SIGNED_FILE: &str = "https://d.test/desktop/stable/1.0.0/nexus-host_1.0.0_linux-x86_64.gz";
    const FILE: &str = "https://d.test/host.gz";

    fn entry(url: &str, signature: Option<&str>) -> Value {
        let mut entry = json!({ "url": url, "sha256": sha256_hex(HOST) });
        if let Some(signature) = signature {
            entry["signature"] = json!(signature);
        }
        entry
    }

    /// A release whose manifest lists `entry` for x86_64, with `bytes` at every other URL. The list holds each URL
    /// asked for.
    fn installer(
        target: Target,
        pubkey: Option<&str>,
        entry: Value,
        bytes: &'static [u8],
    ) -> (Installer, Arc<std::sync::Mutex<Vec<String>>>) {
        installer_on(DOWNLOADS, target, pubkey, entry, bytes)
    }

    /// The same, on another downloads host (a `file://` folder when the server should download from it).
    fn installer_on(
        downloads: &str,
        target: Target,
        pubkey: Option<&str>,
        entry: Value,
        bytes: &'static [u8],
    ) -> (Installer, Arc<std::sync::Mutex<Vec<String>>>) {
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let manifest = json!({ "downloads": { "linux-x86_64-nexus-host": entry } }).to_string();
        let log = asked.clone();
        let fetch: Fetch = Arc::new(move |url: String, _max| {
            log.lock().unwrap_or_else(|p| p.into_inner()).push(url.clone());
            let answer = if url.ends_with("/latest.json") {
                manifest.clone().into_bytes()
            } else {
                bytes.to_vec()
            };
            Box::pin(async move { Ok(answer) })
        });
        let installer = Installer {
            target,
            source: Source::release(downloads, "1.0.0+abc", pubkey),
            fetch: Some(fetch),
        };
        (installer, asked)
    }

    fn no_target() -> Target {
        Target::Local { env: Vec::new() }
    }

    async fn file_for(installer: &Installer) -> Result<(Download, Option<Vec<u8>>)> {
        let release = (
            "1.0.0+abc",
            MANIFEST,
            match &installer.source {
                Source::Release { pubkey, .. } => pubkey.as_deref(),
                Source::Local { .. } => None,
            },
        );
        installer.release_file("1.0.0+abc", "x86_64", release).await
    }

    #[tokio::test]
    async fn a_good_signature_passes_and_keeps_the_verified_bytes() {
        let entry = entry(FILE, Some(SIGNATURE.trim()));
        let (installer, _) = installer(no_target(), Some(KEY), entry, HOST);
        let (download, verified) = file_for(&installer).await.expect("a signed host");
        assert_eq!(download.sha256, sha256_hex(HOST));
        assert_eq!(verified.as_deref(), Some(HOST));
    }

    #[tokio::test]
    async fn a_missing_signature_refuses_before_any_download() {
        let (installer, asked) = installer(no_target(), Some(KEY), entry(FILE, None), HOST);
        let refused = file_for(&installer).await;
        assert!(refused.is_err_and(|e| e.contains("no signature")));
        assert_eq!(
            *asked.lock().unwrap_or_else(|p| p.into_inner()),
            vec![MANIFEST.to_string()]
        );
    }

    #[tokio::test]
    async fn a_bad_signature_refuses() {
        for signature in ["", "not base64", OTHER_SIGNATURE.trim(), SIGNATURE_AARCH64.trim()] {
            let (installer, _) = installer(no_target(), Some(KEY), entry(FILE, Some(signature)), HOST);
            let refused = file_for(&installer).await;
            assert!(refused.is_err_and(|e| e.contains("signature check")), "{signature:?}");
        }
    }

    #[tokio::test]
    async fn a_good_signature_from_another_key_refuses() {
        let (installer, _) = installer(no_target(), Some(OTHER_KEY), entry(FILE, Some(SIGNATURE.trim())), HOST);
        assert!(file_for(&installer).await.is_err_and(|e| e.contains("signature check")));
    }

    #[tokio::test]
    async fn a_key_the_app_cannot_read_refuses_rather_than_skips() {
        let (installer, _) = installer(
            no_target(),
            Some("not a key"),
            entry(FILE, Some(SIGNATURE.trim())),
            HOST,
        );
        assert!(file_for(&installer).await.is_err_and(|e| e.contains("signature check")));
    }

    #[tokio::test]
    async fn only_a_keyless_build_skips_the_signature_and_the_download() {
        let (installer, asked) = installer(no_target(), None, entry(FILE, None), HOST);
        let (_, verified) = file_for(&installer).await.expect("a keyless build");
        assert_eq!(verified, None);
        assert_eq!(asked.lock().unwrap_or_else(|p| p.into_inner()).len(), 1);
    }

    #[test]
    fn a_signature_binds_the_version_and_the_architecture() {
        let download = Download {
            url: FILE.into(),
            sha256: sha256_hex(HOST),
            signature: Some(SIGNATURE.trim().into()),
        };
        assert!(check_signature(&download, HOST, KEY, "1.0.0+abc", "x86_64").is_ok());
        assert!(check_signature(&download, HOST, KEY, "1.0.1+abc", "x86_64").is_err());
        assert!(check_signature(&download, HOST, KEY, "1.0.0+abc", "aarch64").is_err());
        assert!(check_signature(&download, b"another file", KEY, "1.0.0+abc", "x86_64").is_err());
    }

    // The whole flow on this computer: `uname` says Linux, so the script runs the same way on a Mac and on CI. The
    // stand-in host answers `version` and `status`, which is all the script asks of a host that already runs.
    #[cfg(unix)]
    mod flow {
        use super::*;
        use std::path::Path;
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// A throwaway home, removed when dropped.
        struct Home(PathBuf);

        impl Home {
            fn new() -> Home {
                static NEXT: AtomicUsize = AtomicUsize::new(0);
                let home = std::env::temp_dir().join(format!(
                    "nexus-remote-install-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::SeqCst)
                ));
                std::fs::create_dir_all(home.join("bin")).expect("a throwaway home");
                write_script(
                    &home.join("bin/uname"),
                    "#!/bin/sh\nif [ \"$1\" = -s ]; then echo Linux; else echo x86_64; fi\n",
                    "755",
                );
                Home(home)
            }

            fn target(&self) -> Target {
                let path = format!(
                    "{}:{}",
                    self.0.join("bin").display(),
                    std::env::var("PATH").unwrap_or_default()
                );
                Target::Local {
                    env: vec![
                        ("HOME".into(), self.0.to_string_lossy().into_owned()),
                        ("PATH".into(), path),
                    ],
                }
            }

            fn host(&self) -> PathBuf {
                self.0.join(".nexus/host/1.0.0+abc/nexus-host")
            }

            /// A downloads host the server can reach, serving `bytes` as version 1.0.0's x86_64 host.
            fn downloads(&self, bytes: &[u8]) -> String {
                let folder = self.0.join("downloads/desktop/stable/1.0.0");
                std::fs::create_dir_all(&folder).expect("the served folder");
                std::fs::write(folder.join("nexus-host_1.0.0_linux-x86_64.gz"), bytes).expect("the served file");
                file_url(&self.0.join("downloads"))
            }

            /// A `curl` that notes its arguments in `curl.log`, then runs the real one.
            fn log_curl(&self) -> PathBuf {
                let log = self.0.join("curl.log");
                let curl = self.0.join("bin/curl");
                let script = format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nPATH=\"${{PATH#*:}}\" exec curl \"$@\"\n",
                    log.display()
                );
                write_script(&curl, &script, "755");
                log
            }
        }

        impl Drop for Home {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        fn file_url(path: &Path) -> String {
            format!("file://{}", path.display())
        }

        #[tokio::test]
        async fn a_signed_host_is_downloaded_by_the_server_after_this_computer_verifies_it() {
            let home = Home::new();
            let downloads = home.downloads(HOST);
            // The manifest's own URL names nothing the server can read: only the computed one works.
            let entry = entry(FILE, Some(SIGNATURE.trim()));
            let curl = home.log_curl();
            let (installer, asked) = installer_on(&downloads, home.target(), Some(KEY), entry, HOST);
            let ready = installer.ensure().await.expect("installed");
            assert_eq!(ready.version, "1.0.0");
            assert!(home.host().exists());
            assert_eq!(asked.lock().unwrap_or_else(|p| p.into_inner()).len(), 2);
            let curl = std::fs::read_to_string(curl).expect("the server used curl");
            assert!(
                curl.contains(&format!(
                    "{downloads}/desktop/stable/1.0.0/nexus-host_1.0.0_linux-x86_64.gz"
                )),
                "{curl}"
            );
            assert!(!curl.contains(FILE), "{curl}");
            assert!(curl.contains(&format!("--max-filesize {HOST_MAX}")), "{curl}");
            assert!(curl.contains("--max-time 90"), "{curl}");
        }

        /// Runs the install script for an x86_64 server on `url`, allowed `limit` bytes.
        async fn ensure_script(home: &Home, url: &str, sha256: &str, limit: &str) -> HashMap<String, String> {
            let installer = Installer {
                target: home.target(),
                source: Source::Local { path: home.host() },
                fetch: None,
            };
            let output = installer
                .run(
                    ENSURE,
                    &["1.0.0+abc", "1.0.0+abc", url, sha256, "", "", limit, "0"],
                    None,
                    Duration::from_secs(60),
                )
                .await
                .expect("the script ran");
            parse(&output.stdout)
        }

        #[tokio::test]
        async fn the_server_refuses_a_file_above_the_limit() {
            let home = Home::new();
            let downloads = home.downloads(HOST);
            let url = format!("{downloads}/desktop/stable/1.0.0/nexus-host_1.0.0_linux-x86_64.gz");
            let at_limit = ensure_script(&home, &url, &sha256_hex(HOST), &HOST.len().to_string()).await;
            assert_eq!(at_limit.get("downloaded").map(String::as_str), Some("yes"));
            std::fs::remove_file(home.host()).expect("the host it installed");

            let above = ensure_script(&home, &url, &sha256_hex(HOST), &(HOST.len() - 1).to_string()).await;
            assert_eq!(above.get("downloaded").map(String::as_str), Some("no"));
            assert!(!home.host().exists());
        }

        #[tokio::test]
        async fn a_download_that_ignores_the_limit_is_cut_off_there() {
            // An older curl does not stop when the server announces no size: this one writes the whole file anyway.
            let home = Home::new();
            let served = home.0.join("served");
            std::fs::write(&served, HOST).expect("the served file");
            write_script(
                &home.0.join("bin/curl"),
                &format!("#!/bin/sh\ncat '{}'\n", served.display()),
                "755",
            );
            let limit = (HOST.len() - 1).to_string();
            let fields = ensure_script(&home, "https://d.test/x", &sha256_hex(HOST), &limit).await;
            assert_eq!(fields.get("downloaded").map(String::as_str), Some("no"));
            assert!(!home.host().exists());
        }

        #[tokio::test]
        async fn a_server_that_cannot_download_gets_the_verified_bytes() {
            let home = Home::new();
            let entry = entry(FILE, Some(SIGNATURE.trim()));
            let (installer, asked) = installer_on(NOBODY, home.target(), Some(KEY), entry, HOST);
            installer.ensure().await.expect("installed");
            assert!(home.host().exists());
            // Downloaded once, to verify it: the copy over SSH reuses those bytes.
            assert_eq!(asked.lock().unwrap_or_else(|p| p.into_inner()).len(), 2);
        }

        #[tokio::test]
        async fn a_host_already_there_needs_no_manifest_and_no_signature() {
            let home = Home::new();
            let entry = entry(FILE, Some(SIGNATURE.trim()));
            let (first, _) = installer_on(NOBODY, home.target(), Some(KEY), entry.clone(), HOST);
            first.ensure().await.expect("installed");
            let (second, asked) = installer_on(NOBODY, home.target(), Some(KEY), entry, HOST);
            second.ensure().await.expect("already there");
            let asked = asked.lock().unwrap_or_else(|p| p.into_inner());
            assert!(asked.is_empty(), "{asked:?}");
        }

        #[tokio::test]
        async fn a_refused_host_never_reaches_the_server() {
            // A rewritten manifest: other bytes, with their own SHA-256 beside the real signature.
            let tampered: &[u8] = b"not the signed host";
            let cases = [
                ("missing", entry(FILE, None), HOST),
                ("wrong file", entry(FILE, Some(SIGNATURE_AARCH64.trim())), HOST),
                (
                    "tampered",
                    json!({ "url": FILE, "sha256": sha256_hex(tampered), "signature": SIGNATURE.trim() }),
                    tampered,
                ),
            ];
            for (case, mut entry, bytes) in cases {
                let home = Home::new();
                let downloads = home.downloads(bytes);
                entry["url"] = json!(FILE);
                let (installer, _) = installer_on(&downloads, home.target(), Some(KEY), entry, bytes);
                let refused = installer.ensure().await;
                assert!(refused.is_err_and(|e| e.contains("signature")), "{case}");
                assert!(!home.host().exists(), "{case}");
            }
        }

        #[tokio::test]
        async fn update_refuses_an_unsigned_host_too() {
            let home = Home::new();
            let (installer, _) = installer(home.target(), Some(KEY), entry(FILE, None), HOST);
            let updated = installer.update().await;
            assert!(
                updated.as_ref().is_err_and(|e| e.contains("no signature")),
                "{updated:?}"
            );
            assert!(!home.host().exists());
        }
    }

    /// The ENSURE script run on this computer against a fake `nexus-host`, as over ssh.
    #[cfg(unix)]
    mod on_a_machine {
        use super::*;
        use std::path::Path;
        use std::sync::Mutex;

        const UNPUBLISHED: &str = "0.10.0+d4c3b3c";
        const BAKED: &str = "0.10.0-internal.388+abc1234";

        /// A home whose `~/.nexus/host` has a host listening and its token, as a cloud machine has after boot.
        fn home(name: &str) -> PathBuf {
            let home = std::env::temp_dir().join(format!("nexus-remote-install-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&home);
            let run = home.join(".nexus/host/run");
            assert!(std::fs::create_dir_all(&run).is_ok());
            assert!(std::fs::write(run.join("host.port"), "1").is_ok());
            assert!(std::fs::write(home.join(".nexus/host/token"), "t".repeat(40)).is_ok());
            home
        }

        /// A fake `nexus-host` of `version` that says it runs.
        fn fake(version: &str) -> String {
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  version) echo 'nexus-host {version}' ;;\n  status) echo '{{\"running\":true}}' ;;\n  serve) r=\"${{NEXUS_HOST_DIR:-$HOME/.nexus/host}}\"; mkdir -p \"$r/run\"; echo 1 > \"$r/run/host.port\" ;;\nesac\n"
            )
        }

        fn install(home: &Path, dir: &str, version: &str) {
            let bin = home.join(".nexus/host").join(dir).join("nexus-host");
            assert!(std::fs::create_dir_all(bin.parent().unwrap_or(home)).is_ok());
            write_script(&bin, &fake(version), "700");
        }

        /// What a published release of `version` answers: its own manifest and its file beside it.
        fn published(home: &Path, version: &str) -> Vec<(String, Vec<u8>)> {
            let semver = version.split('+').next().unwrap_or(version);
            let plain = home.join(format!("host-{version}"));
            let gz = home.join(format!("host-{version}.gz"));
            assert!(std::fs::write(&plain, fake(version)).is_ok());
            let status = nexus_workbench::process::command("sh")
                .arg("-c")
                .arg("gzip -c \"$1\" > \"$2\"")
                .arg("sh")
                .arg(&plain)
                .arg(&gz)
                .status();
            assert!(status.is_ok_and(|s| s.success()));
            let bytes = std::fs::read(&gz).unwrap_or_default();
            let sha = sha256_hex(&bytes);
            let folder = format!("{DEAD}/desktop/stable/{semver}");
            let manifest = json!({ "version": semver, "downloads": {
                "linux-x86_64-nexus-host": { "url": "https://elsewhere.test/x.gz", "sha256": sha },
                "linux-aarch64-nexus-host": { "url": "https://elsewhere.test/a.gz", "sha256": sha },
            }});
            vec![
                (format!("{folder}/latest.json"), manifest.to_string().into_bytes()),
                (format!("{folder}/nexus-host_{semver}_linux-x86_64.gz"), bytes.clone()),
                (format!("{folder}/nexus-host_{semver}_linux-aarch64.gz"), bytes),
            ]
        }

        /// The channel's manifest, which names its latest version.
        fn channel(version: &str) -> (String, Vec<u8>) {
            (
                LATEST.to_string(),
                json!({ "version": version }).to_string().into_bytes(),
            )
        }

        /// An installer for the app of version [`UNPUBLISHED`] whose downloads answer `answers` (else a 404), and the
        /// URLs it asked for.
        fn app(home: &Path, answers: Vec<(String, Vec<u8>)>) -> (Installer, Arc<Mutex<Vec<String>>>) {
            let asked = Arc::new(Mutex::new(Vec::new()));
            let fetch: Fetch = {
                let asked = asked.clone();
                Arc::new(move |url: String, _max| {
                    if let Ok(mut asked) = asked.lock() {
                        asked.push(url.clone());
                    }
                    let answer = answers
                        .iter()
                        .find(|(known, _)| *known == url)
                        .map(|(_, bytes)| bytes.clone())
                        .ok_or_else(|| format!("Could not download {url}: HTTP status client error (404 Not Found)"));
                    Box::pin(async move { answer })
                })
            };
            let installer = Installer {
                target: Target::Local {
                    env: vec![("HOME".into(), home.to_string_lossy().into_owned())],
                },
                source: Source::release(DEAD, UNPUBLISHED, None),
                fetch: Some(fetch),
            };
            (installer, asked)
        }

        const OWN: &str = "http://127.0.0.1:9/desktop/stable/0.10.0/latest.json";
        const LATEST: &str = "http://127.0.0.1:9/desktop/stable/latest.json";
        /// A downloads host nothing answers, so the server cannot download and this computer's bytes are copied over.
        const DEAD: &str = "http://127.0.0.1:9";

        #[tokio::test]
        async fn an_unpublished_version_reuses_the_host_on_the_machine() {
            let home = home("reuse");
            install(&home, BAKED, BAKED);
            let (installer, asked) = app(&home, vec![]);
            let ready = installer.ensure().await.unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(ready.dir, BAKED);
            assert_eq!(ready.version, BAKED);
            assert!(!ready.exact);
            assert_eq!(ready.started, "already");
            assert!(!home.join(".nexus/host").join(UNPUBLISHED).join("nexus-host").exists());
            assert_eq!(asked.lock().map(|a| a.clone()).unwrap_or_default(), vec![OWN, LATEST]);
            // Nothing is downloaded for it either when the channel's latest is published: the machine's host serves.
            let mut answers = published(&home, "0.9.0+5576d15");
            answers.push(channel("0.9.0"));
            let (installer, _) = app(&home, answers);
            let ready = installer.ensure().await.unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(ready.dir, BAKED);
            assert!(!home.join(".nexus/host/0.9.0/nexus-host").exists());
            let _ = std::fs::remove_dir_all(&home);
        }

        #[tokio::test]
        async fn a_machines_host_older_than_the_channels_latest_is_replaced() {
            let home = home("floor");
            install(&home, "0.8.0", "0.8.0+aaaaaaa");
            let mut answers = published(&home, "0.9.0+5576d15");
            answers.push(channel("0.9.0"));
            let (installer, _) = app(&home, answers);
            let ready = installer.ensure().await.unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(ready.dir, "0.9.0");
            assert_eq!(ready.version, "0.9.0+5576d15");
            assert!(!ready.exact);
            let _ = std::fs::remove_dir_all(&home);
        }

        #[tokio::test]
        async fn a_local_build_never_stands_in_for_a_release() {
            let home = home("dev");
            install(&home, "dev-0123456789abcdef", "0.0.0-dev");
            let (installer, _) = app(&home, vec![]);
            let error = installer.ensure().await.err().unwrap_or_default();
            assert!(error.contains("has no other Nexus host"), "{error}");
            let _ = std::fs::remove_dir_all(&home);
        }

        #[tokio::test]
        async fn a_manifest_that_cannot_be_read_does_not_let_another_host_stand_in() {
            let home = home("blip");
            install(&home, BAKED, BAKED);
            let fetch: Fetch = Arc::new(|url: String, _max| {
                Box::pin(async move { Err(format!("Could not download {url}: error sending request")) })
            });
            let installer = Installer {
                target: Target::Local {
                    env: vec![("HOME".into(), home.to_string_lossy().into_owned())],
                },
                source: Source::release(DEAD, UNPUBLISHED, None),
                fetch: Some(fetch),
            };
            let error = installer.ensure().await.err().unwrap_or_default();
            assert!(error.contains("error sending request"), "{error}");
            let _ = std::fs::remove_dir_all(&home);
        }

        #[tokio::test]
        async fn a_published_version_is_still_installed_beside_another_host() {
            let home = home("exact");
            install(&home, BAKED, BAKED);
            let (installer, _) = app(&home, published(&home, UNPUBLISHED));
            let ready = installer.ensure().await.unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(ready.dir, UNPUBLISHED);
            assert_eq!(ready.version, UNPUBLISHED);
            assert!(ready.exact);
            let _ = std::fs::remove_dir_all(&home);
        }

        #[tokio::test]
        async fn with_no_host_on_the_machine_the_channels_latest_is_installed() {
            let home = home("latest");
            let mut answers = published(&home, "0.9.0+5576d15");
            answers.push(channel("0.9.0"));
            let (installer, _) = app(&home, answers);
            let ready = installer.ensure().await.unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(ready.dir, "0.9.0");
            assert_eq!(ready.version, "0.9.0+5576d15");
            assert!(!ready.exact);
            let _ = std::fs::remove_dir_all(&home);
        }

        #[tokio::test]
        async fn with_no_host_anywhere_the_error_says_why() {
            let home = home("none");
            let (installer, _) = app(&home, vec![]);
            let error = installer.ensure().await.err().unwrap_or_default();
            assert!(
                error.starts_with(&format!("The Nexus host {UNPUBLISHED} is not available, and this computer has no other Nexus host: Could not download {OWN}")),
                "{error}"
            );
            assert!(error.contains(LATEST), "{error}");
            let _ = std::fs::remove_dir_all(&home);
        }
    }

    /// `answered`, the parser behind `answering`, on what `status --json` prints.
    #[cfg(unix)]
    #[test]
    fn answered_reads_only_the_top_level_keys() {
        let cases = [
            (
                r#"{"running":true,"sessions":[{"id":"a","error":"It failed."}],"runningTurns":0}"#,
                true,
            ),
            (
                r#"{"running":true,"sessions":[],"error":"The running host did not answer in time."}"#,
                false,
            ),
            (r#"{"running":false,"sessions":[]}"#, false),
            (
                r#"{"version":"0.5.0","signedIn":true,"running":true,"sessions":[]}"#,
                true,
            ),
            (r#"{"error":"x","running":true}"#, false),
            (r#"{"org":"a \"error\": b","running" : true,"sessions":[]}"#, true),
            (r#"{"running":false,"sessions":[{"running":true}]}"#, false),
            ("", false),
        ];
        for (status, want) in cases {
            let result = nexus_workbench::process::command("sh")
                .arg("-c")
                .arg(format!("{COMMON}printf '%s' \"$1\" | answered"))
                .arg("sh")
                .arg(status)
                .status();
            assert_eq!(result.map(|s| s.success()).ok(), Some(want), "{status}");
        }
    }

    /// The ENSURE script run on this computer against a fake `nexus-host` that holds a real lock (`flock(1)`), as the
    /// real host does: `serve` takes the lock, then removes a dead host's port file and writes its own; `status` says
    /// running while the lock is held, and adds `"error"` when the port file is not the lock holder's.
    #[cfg(target_os = "linux")]
    mod with_a_lock {
        use super::*;

        const VERSION: &str = "0.9.0";

        /// The fake host. Every wait is on a file, bounded at 10 s.
        /// - `FAKE_ROLE=first`: `serve` takes the lock only after the second install found no host; a `status`
        ///   that finds the lock held waits until a second `serve` lost it.
        /// - `FAKE_ROLE=second`: the first `status` that finds no host waits, its answer already decided, until a
        ///   host wrote its port file.
        /// - `FAKE_STALE=1`: `serve` writes its port file only after a `status` found it not answering.
        ///
        /// Verbs: `version` (must match `VERSION`), `serve`, `status`, `stop`, and `held` (whether a host holds the
        /// lock, without a `status`).
        const FAKE: &str = r#"#!/bin/sh
r="${NEXUS_HOST_DIR:-$HOME/.nexus/host}"; run="$r/run"; lock="$run/host.lock"
held() { [ -f "$lock" ] && ! flock -n -s "$lock" true; }
wait_for() { i=0; while [ ! -e "$1" ] && [ "$i" -lt 200 ]; do sleep 0.05; i=$((i + 1)); done; }
case "$1" in
  version) echo 'nexus-host 0.9.0' ;;
  serve)
    mkdir -p "$run"
    [ "${FAKE_ROLE:-}" = first ] && wait_for "$run/gated"
    exec 9<>"$lock"
    if ! flock -w 1 9; then echo 'A Nexus host is already running for this user.' >&2; : > "$run/lost"; exit 1; fi
    echo $$ > "$lock"
    [ "${FAKE_STALE:-}" = 1 ] && wait_for "$run/unanswered"
    rm -f "$run/host.sock" "$run/host.port"
    echo $$ > "$run/host.port"
    exec sleep 600
    ;;
  status)
    if ! held; then
      if [ "${FAKE_ROLE:-}" = second ] && [ ! -e "$run/gated" ]; then : > "$run/gated"; wait_for "$run/host.port"; fi
      echo '{"running":false,"sessions":[]}'
      exit 0
    fi
    [ "${FAKE_ROLE:-}" = first ] && wait_for "$run/lost"
    pid="$(cat "$lock" 2>/dev/null)"
    # A live holder's own port file: a dead host's lock and port files may name the same pid.
    if [ -n "$pid" ] && [ "$pid" = "$(cat "$run/host.port" 2>/dev/null)" ] && kill -0 "$pid" 2>/dev/null; then
      : > "$run/answered"
      echo '{"running":true,"sessions":[]}'
    else
      : > "$run/unanswered"
      echo '{"running":true,"sessions":[],"error":"The running host did not answer in time."}'
    fi
    ;;
  held) held ;;
  stop)
    if held; then kill "$(cat "$lock")"; fi
    flock -w 10 -s "$lock" true
    ;;
esac
"#;

        /// A throwaway home with the fake installed as [`VERSION`] and a token, and no host running. Its host is
        /// stopped when it drops.
        struct Machine {
            home: PathBuf,
            path: String,
        }

        impl Machine {
            fn new(name: &str) -> Machine {
                let home = std::env::temp_dir().join(format!("nexus-remote-lock-{name}-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&home);
                let host = home.join(".nexus/host");
                let bin = host.join(VERSION).join("nexus-host");
                let stubs = home.join("stubs");
                for dir in [host.join("run"), host.join(VERSION), stubs.clone()] {
                    assert!(std::fs::create_dir_all(dir).is_ok());
                }
                assert!(std::fs::write(host.join("token"), "t".repeat(40)).is_ok());
                // No systemd here: the host starts with `setsid nohup`, as a child the test can stop.
                let no = "#!/bin/sh\nexit 1\n";
                for (file, body) in [
                    (&bin, FAKE),
                    (&stubs.join("systemd-run"), no),
                    (&stubs.join("systemctl"), no),
                ] {
                    write_script(file, body, "700");
                }
                let path = format!("{}:{}", stubs.display(), std::env::var("PATH").unwrap_or_default());
                Machine { home, path }
            }

            fn run_dir(&self) -> PathBuf {
                self.home.join(".nexus/host/run")
            }

            fn host(&self, verb: &str) -> String {
                // Through `sh`, never exec'd from here: a file written moments ago can still be open in a child
                // another test forked (ETXTBSY).
                let output = nexus_workbench::process::command("sh")
                    .arg(self.home.join(".nexus/host").join(VERSION).join("nexus-host"))
                    .arg(verb)
                    .arg("--json")
                    .env("HOME", &self.home)
                    .output();
                output
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                    .unwrap_or_default()
            }

            /// ENSURE with the fake already installed, its variables set (`FAKE_ROLE`, `FAKE_STALE`).
            async fn ensure(&self, vars: &[(&str, &str)]) -> HashMap<String, String> {
                let mut env = vec![
                    ("HOME".to_string(), self.home.to_string_lossy().into_owned()),
                    ("PATH".to_string(), self.path.clone()),
                ];
                env.extend(vars.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())));
                let target = Target::Local { env };
                let args = [VERSION, VERSION, "", "", "", "", "0", "0"];
                let output = target
                    .run(&Installer::script(ENSURE), &args, None, Duration::from_secs(60))
                    .await
                    .unwrap_or_else(|e| panic!("{e}"));
                parse(&output.stdout)
            }
        }

        impl Drop for Machine {
            fn drop(&mut self) {
                let _ = self.host("stop");
                let _ = std::fs::remove_dir_all(&self.home);
            }
        }

        #[tokio::test]
        async fn two_overlapping_installs_keep_the_host_that_won() {
            let machine = Machine::new("race");
            // Both see no host. The second starts its own once the first's host wrote its port file, and the first
            // looks again only once the second's host lost the lock.
            let (first, second) = tokio::join!(
                machine.ensure(&[("FAKE_ROLE", "first")]),
                machine.ensure(&[("FAKE_ROLE", "second")]),
            );
            assert!(machine.run_dir().join("lost").exists(), "the second host never ran");
            for (name, fields) in [("first", &first), ("second", &second)] {
                let started = fields.get("started").map(String::as_str);
                assert!(matches!(started, Some("nohup" | "already")), "{name}: {fields:?}");
                assert_eq!(fields.get("token").map(String::len), Some(40), "{name}: {fields:?}");
            }
            let status = machine.host("status");
            assert!(
                status.contains("\"running\":true") && !status.contains("error"),
                "{status}"
            );
        }

        #[tokio::test]
        async fn a_host_still_starting_on_a_dead_hosts_port_file_is_waited_for() {
            let machine = Machine::new("already");
            let run = machine.run_dir();
            assert!(std::fs::write(run.join("host.lock"), "999999999\n").is_ok());
            assert!(std::fs::write(run.join("host.port"), "999999999\n").is_ok());
            // Another install's host: it holds the lock, and writes its own port file only once a `status` found it
            // not answering.
            let bin = machine.home.join(".nexus/host").join(VERSION).join("nexus-host");
            // Through `sh`, as in `Machine::host`.
            let serve = nexus_workbench::process::command("sh")
                .arg(&bin)
                .arg("serve")
                .env("HOME", &machine.home)
                .env("FAKE_STALE", "1")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            assert!(serve.is_ok());
            let held = || {
                nexus_workbench::process::command("sh")
                    .arg(&bin)
                    .arg("held")
                    .env("HOME", &machine.home)
                    .status()
                    .is_ok_and(|s| s.success())
            };
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !held() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the other host never took the lock"
                );
                std::thread::yield_now();
            }
            let fields = machine.ensure(&[]).await;
            assert_eq!(fields.get("started").map(String::as_str), Some("already"), "{fields:?}");
            assert!(
                run.join("answered").exists(),
                "already before the host answered: {fields:?}"
            );
            assert_eq!(fields.get("token").map(String::len), Some(40), "{fields:?}");
            if let Ok(mut serve) = serve {
                let _ = machine.host("stop");
                let _ = serve.wait();
            }
        }

        #[tokio::test]
        async fn a_dead_hosts_port_file_is_not_a_started_host() {
            let machine = Machine::new("stale");
            let run = machine.run_dir();
            // A host killed with SIGKILL: its lock file names it, its port file stays, nothing holds the lock.
            assert!(std::fs::write(run.join("host.lock"), "999999999\n").is_ok());
            assert!(std::fs::write(run.join("host.port"), "999999999\n").is_ok());
            let fields = machine.ensure(&[("FAKE_STALE", "1")]).await;
            assert_eq!(fields.get("started").map(String::as_str), Some("nohup"), "{fields:?}");
            assert!(
                run.join("answered").exists(),
                "started before the new host answered: {fields:?}"
            );
            let status = machine.host("status");
            assert!(
                status.contains("\"running\":true") && !status.contains("error"),
                "{status}"
            );
        }

        /// A `systemctl` that knows one enabled unit, `nexus-host.service`, and restarts it as systemd would: the
        /// binary `current` names, detached. Each call goes in `systemctl.log`.
        fn systemctl(machine: &Machine) -> PathBuf {
            let log = machine.home.join("systemctl.log");
            let script = format!(
                r#"#!/bin/sh
printf '%s\n' "$*" >> '{}'
case "$2" in
  is-enabled) [ "$4" = nexus-host.service ] ;;
  restart) setsid nohup "$HOME/.nexus/host/current/nexus-host" serve >/dev/null 2>&1 </dev/null & ;;
  *) exit 1 ;;
esac
"#,
                log.display()
            );
            write_script(&machine.home.join("stubs/systemctl"), &script, "700");
            log
        }

        #[tokio::test]
        async fn an_installed_service_starts_the_host_through_current() {
            let machine = Machine::new("service");
            let root = machine.home.join(".nexus/host");
            let log = systemctl(&machine);
            assert!(std::fs::write(root.join("service"), "nexus-host.service\n").is_ok());
            // A link left at an older version: the start points it at the one it starts.
            assert!(std::os::unix::fs::symlink("0.8.0", root.join("current")).is_ok());
            let fields = machine.ensure(&[]).await;
            assert_eq!(fields.get("started").map(String::as_str), Some("service"), "{fields:?}");
            assert_eq!(fields.get("token").map(String::len), Some(40), "{fields:?}");
            assert_eq!(
                std::fs::read_link(root.join("current")).ok(),
                Some(PathBuf::from(VERSION))
            );
            let calls = std::fs::read_to_string(&log).unwrap_or_default();
            assert!(calls.contains("--user restart nexus-host.service"), "{calls}");
            // No second host beside the service's: no `systemd-run`, no `nohup` log.
            assert!(!machine.run_dir().join("host.log").exists());
            let run = machine.run_dir();
            assert!(
                run.join("answered").exists(),
                "started before the new host answered: {fields:?}"
            );
            let status = machine.host("status");
            assert!(
                status.contains("\"running\":true") && !status.contains("error"),
                "{status}"
            );
        }
    }
}
