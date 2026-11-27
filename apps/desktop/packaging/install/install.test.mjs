// Tests of the public one-line installers (install.sh and install.ps1), with fake downloads and fake system tools.
// The real script runs in a shell. Only the tools it calls (curl, uname, hdiutil and so on) and the paths are faked.
// Run: node --test apps/desktop/packaging/install/install.test.mjs
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const shScript = fileURLToPath(new URL("./install.sh", import.meta.url));
const psScript = fileURLToPath(new URL("./install.ps1", import.meta.url));
const BASE = "https://downloads.nexus.inc/desktop/stable";
const MANIFEST_URL = `${BASE}/latest.json`;
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

// One fake for every tool. It logs each call to $FIXTURE/calls and acts by its own name.
const fake = `#!/bin/sh
tool="$(basename "$0")"
printf '%s %s\\n' "$tool" "$*" >> "$FIXTURE/calls"
case "$tool" in
  uname) if [ "\${1:-}" = -m ]; then echo "$FAKE_ARCH"; else echo "$FAKE_OS"; fi ;;
  sysctl) echo "$FAKE_APPLE_SILICON" ;;
  curl)
    out=""; url=""
    while [ "$#" -gt 0 ]; do
      if [ "$1" = -o ]; then out="$2"; shift 2; else url="$1"; shift; fi
    done
    case "$url" in
      */latest.json) cp "$FIXTURE/manifest.json" "$out" ;;
      *)
        [ -f "$FIXTURE/files/$(basename "$url")" ] || exit 22
        if [ -n "\${FAKE_CHANGE_INSTALL_DIR:-}" ]; then rm "$FAKE_CHANGE_INSTALL_DIR/nexus-desktop-gpui"; fi
        cp "$FIXTURE/files/$(basename "$url")" "$out"
        ;;
    esac
    ;;
  hdiutil)
    if [ "$1" = attach ]; then
      while [ "$1" != -mountpoint ]; do shift; done
      mkdir -p "$2/Nexus.app"
      printf 'new app' > "$2/Nexus.app/marker"
    fi
    ;;
  ditto) cp -R "$1" "$2" ;;
  codesign) [ "$FAKE_SIGNATURE" = pass ] ;;
  spctl) printf '%s\\n' "$FAKE_ASSESSMENT" >&2; [ "$FAKE_GATEKEEPER" = pass ] ;;
  pgrep) exit 1 ;;
  osascript | sleep | open) exit 0 ;;
  mv)
    real_mv=/usr/bin/mv; [ -x "$real_mv" ] || real_mv=/bin/mv
    if [ "$1" = -T ]; then
      shift
      [ ! -e "$2" ] || exit 1
    fi
    case "$1" in
      */.nexus-desktop-install.*/new/*)
        [ -z "\${FAKE_MV_FAIL:-}" ] || exit 1
        ;;
      */.nexus-desktop-install.*/previous)
        [ "\${FAKE_MV_FAIL:-}" != restore ] || exit 1
        ;;
    esac
    "$real_mv" "$@" || exit
    case "$2" in
      */.nexus-desktop-install.*/previous | */.Nexus.app.old.*)
        [ -z "\${FAKE_MV_SIGNAL:-}" ] || kill -s "$FAKE_MV_SIGNAL" "$PPID"
        ;;
    esac
    ;;
  *) exit 99 ;;
esac
`;

function fixture(t) {
  const dir = mkdtempSync(join(tmpdir(), "nexus-public-install-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const bin = join(dir, "bin");
  for (const sub of ["bin", "home", "tmp", "files", "apps"]) mkdirSync(join(dir, sub));
  for (const name of ["curl", "uname", "sysctl", "hdiutil", "ditto", "codesign", "spctl", "pgrep", "osascript", "sleep", "open", "mv"]) {
    writeFileSync(join(bin, name), fake, { mode: 0o755 });
  }
  return dir;
}

// A Linux tarball with the folder layout the packager makes: one folder, the binary and its own install.sh.
function tarball(dir, name = "nexus-desktop-9.9.9-linux-x86_64") {
  const root = join(dir, "pack", name);
  mkdirSync(root, { recursive: true });
  writeFileSync(join(root, "nexus-desktop-gpui"), `#!/bin/sh\nprintf launched > "$HOME/launched"\n`, { mode: 0o755 });
  writeFileSync(join(root, "install.sh"), `#!/bin/sh\nmkdir -p "$HOME/.local/share/applications"\nprintf menu > "$HOME/.local/share/applications/nexus-desktop-gpui.desktop"\n`, { mode: 0o755 });
  const file = join(dir, "files", `${name}.tar.xz`);
  // Relative paths: GNU tar (Git for Windows') reads the `C:` of a Windows path as a remote host.
  const made = spawnSync("tar", ["-C", "pack", "-cJf", join("files", `${name}.tar.xz`), name], { cwd: dir });
  assert.equal(made.status, 0, String(made.stderr));
  return { file: `${name}.tar.xz`, bytes: readFileSync(file) };
}

function writeManifest(dir, downloads, { multiline = false } = {}) {
  const entries = Object.entries(downloads).map(([key, d]) => {
    const body = `"url":"${d.url}","sha256":"${d.sha256}","size":${d.size ?? 1234}`;
    return multiline ? `    "${key}": {\n      ${body.replaceAll(",", ",\n      ")}\n    }` : `    "${key}": {${body}}`;
  });
  writeFileSync(join(dir, "manifest.json"), `{\n  "version": "9.9.9",\n  "platforms": {"darwin-aarch64-dmg": {"url": "${BASE}/9.9.9/x.dmg", "signature": "abc"}},\n  "downloads": {\n${entries.join(",\n")}\n  }\n}\n`);
}

function macFixture(t, { sha256, url = `${BASE}/9.9.9/nexus-desktop-9.9.9-macos-arm64.dmg`, multiline = false } = {}) {
  const dir = fixture(t);
  const dmg = "fake disk image";
  writeFileSync(join(dir, "files", "nexus-desktop-9.9.9-macos-arm64.dmg"), dmg);
  writeManifest(dir, { "macos-arm64-dmg": { url, sha256: sha256 ?? sha(dmg) } }, { multiline });
  const apps = join(dir, "apps", "Applications");
  mkdirSync(join(apps, "Nexus.app"), { recursive: true });
  writeFileSync(join(apps, "Nexus.app", "marker"), "old app");
  return { dir, apps };
}

function linuxFixture(t, { corrupt = false } = {}) {
  const dir = fixture(t);
  const pkg = tarball(dir);
  writeManifest(dir, { "linux-x86_64-tarball": { url: `${BASE}/9.9.9/${pkg.file}`, sha256: corrupt ? "0".repeat(64) : sha(pkg.bytes) } });
  return dir;
}

// The environment without its PATH. On Windows the name is `Path` and names ignore case, so a second `PATH` would be
// a second variable, and the child could read either.
function withoutPath(env) {
  return Object.fromEntries(Object.entries(env).filter(([name]) => name.toUpperCase() !== "PATH"));
}

// A path as the script's shell names it. On Windows the shell is Git for Windows' sh, which names `C:\a\b` `/c/a/b`.
// Elsewhere a path is already that.
const posix = (value) =>
  process.platform === "win32" && /^[A-Za-z]:[\\/]/.test(value)
    ? `/${value[0].toLowerCase()}${value.slice(2).replaceAll("\\", "/")}`
    : value;

// The script's environment, with `bin` first on its PATH. Git for Windows' sh turns PATH (with `;`) into its own form,
// but not the other variables, so a path in one is given as the shell names it. Its `ln -s` copies unless MSYS asks
// for real symbolic links, and the Linux install links its launcher.
function shEnv(env, bin) {
  const out = Object.fromEntries(Object.entries(env).map(([name, value]) => [name, posix(value)]));
  const msys = process.platform === "win32" ? { MSYS: "winsymlinks:nativestrict" } : {};
  return { ...out, ...msys, PATH: `${bin}${delimiter}${process.env.PATH}` };
}

function run(dir, { args = [], env = {}, cwd } = {}) {
  const result = spawnSync("sh", [shScript, ...args], {
    encoding: "utf8",
    cwd,
    timeout: 20_000,
    env: shEnv({
      ...withoutPath(process.env),
      HOME: join(dir, "home"),
      TMPDIR: join(dir, "tmp"),
      FIXTURE: dir,
      FAKE_OS: "Darwin",
      FAKE_ARCH: "arm64",
      FAKE_APPLE_SILICON: "1",
      FAKE_SIGNATURE: "pass",
      FAKE_GATEKEEPER: "pass",
      FAKE_ASSESSMENT: "Nexus.app: accepted\nsource=Notarized Developer ID",
      NEXUS_APP_DIR: join(dir, "apps", "Applications"),
      NEXUS_NO_LAUNCH: "",
      DISPLAY: "",
      WAYLAND_DISPLAY: "",
      ...env,
    }, join(dir, "bin")),
  });
  assert.equal(result.error, undefined);
  const calls = existsSync(join(dir, "calls")) ? readFileSync(join(dir, "calls"), "utf8") : "";
  return { ...result, calls, leftovers: readdirSync(join(dir, "tmp")) };
}

const marker = (apps) => readFileSync(join(apps, "Nexus.app", "marker"), "utf8");

test("macOS: installs the app, replaces the old one, opens it and cleans up", (t) => {
  const { dir, apps } = macFixture(t);
  const r = run(dir);
  assert.equal(r.status, 0, r.stderr);
  assert.equal(marker(apps), "new app");
  assert.match(r.stdout, /Installing Nexus Desktop 9\.9\.9/);
  assert.match(r.stdout, /Installed .*Nexus\.app/);
  assert.match(r.calls, new RegExp(`^curl .* ${MANIFEST_URL.replaceAll(".", "\\.")}$`, "m"));
  assert.match(r.calls, /^open .*Applications\/Nexus\.app$/m);
  assert.doesNotMatch(r.calls, /\?/, "no query string on any URL");
  assert.deepEqual(r.leftovers, [], "the temporary folder is removed");
  assert.deepEqual(readdirSync(apps), ["Nexus.app"], "no staging folder is left beside the app");
});

test("macOS: reads a manifest whose entries span several lines", (t) => {
  const { dir, apps } = macFixture(t, { multiline: true });
  const r = run(dir);
  assert.equal(r.status, 0, r.stderr);
  assert.equal(marker(apps), "new app");
});

test("macOS: --no-launch and NEXUS_NO_LAUNCH=1 do not open the app", (t) => {
  for (const options of [{ args: ["--no-launch"] }, { env: { NEXUS_NO_LAUNCH: "1" } }]) {
    const { dir, apps } = macFixture(t);
    const r = run(dir, options);
    assert.equal(r.status, 0, r.stderr);
    assert.equal(marker(apps), "new app");
    assert.doesNotMatch(r.calls, /^open /m);
  }
});

test("macOS: a checksum mismatch fails, installs nothing and cleans up", (t) => {
  const { dir, apps } = macFixture(t, { sha256: "f".repeat(64) });
  const r = run(dir);
  assert.equal(r.status, 1);
  assert.match(r.stderr, /checksum mismatch/);
  assert.equal(marker(apps), "old app");
  assert.doesNotMatch(r.calls, /^(hdiutil|open) /m);
  assert.deepEqual(r.leftovers, []);
});

test("macOS: an app Apple has not notarized is refused and the old app stays", (t) => {
  const { dir, apps } = macFixture(t);
  const r = run(dir, { env: { FAKE_GATEKEEPER: "fail", FAKE_ASSESSMENT: "Nexus.app: rejected" } });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /notarized/);
  assert.equal(marker(apps), "old app");
  assert.deepEqual(readdirSync(apps), ["Nexus.app"]);
  assert.doesNotMatch(r.calls, /^open /m);
  assert.deepEqual(r.leftovers, []);
});

test("macOS: a failed Gatekeeper assessment preserves the old app", (t) => {
  const { dir, apps } = macFixture(t);
  const result = run(dir, { env: { FAKE_GATEKEEPER: "fail" } });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Gatekeeper did not accept/);
  assert.equal(marker(apps), "old app");
});

test("macOS: a bad signature is refused", (t) => {
  const { dir, apps } = macFixture(t);
  const r = run(dir, { env: { FAKE_SIGNATURE: "fail" } });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /not validly signed/);
  assert.equal(marker(apps), "old app");
});

for (const signal of ["TERM", "INT"]) {
  test(`macOS: ${signal} after moving the old app aside restores it`, (t) => {
    const { dir, apps } = macFixture(t);
    const r = run(dir, { env: { FAKE_MV_SIGNAL: signal } });
    assert.equal(r.status, 130, r.stderr);
    assert.equal(marker(apps), "old app");
    assert.deepEqual(readdirSync(apps), ["Nexus.app"], "no staging or backup folder is left");
    assert.deepEqual(r.leftovers, []);
  });
}

test("macOS: an Intel Mac is refused before any download", (t) => {
  const { dir } = macFixture(t);
  const r = run(dir, { env: { FAKE_APPLE_SILICON: "0" } });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /Apple silicon/);
  assert.doesNotMatch(r.calls, /dmg/);
});

test("a download address outside downloads.nexus.inc/desktop/stable is refused", (t) => {
  const { dir, apps } = macFixture(t, { url: "https://evil.test/desktop/stable/Nexus.dmg" });
  const r = run(dir);
  assert.equal(r.status, 1);
  assert.match(r.stderr, /outside/);
  assert.doesNotMatch(r.calls, /evil\.test/);
  assert.equal(marker(apps), "old app");
});

test("a manifest without this platform's entry fails with a clear message", (t) => {
  const dir = linuxFixture(t);
  const r = run(dir, { env: { FAKE_OS: "Darwin" } });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /has no macos-arm64-dmg download/);
});

test("Linux: unpacks the folder, links the binary, adds the menu entry and cleans up", (t) => {
  const dir = linuxFixture(t);
  const install = join(dir, "home", "app");
  const r = run(dir, { args: ["--no-launch"], env: { FAKE_OS: "Linux", FAKE_ARCH: "x86_64", NEXUS_INSTALL_DIR: install } });
  assert.equal(r.status, 0, r.stderr);
  assert.ok(existsSync(join(install, "nexus-desktop-gpui")));
  assert.ok(existsSync(join(dir, "home", ".local/share/applications/nexus-desktop-gpui.desktop")), "the package's install.sh ran");
  assert.ok(lstatSync(join(dir, "home", ".local/bin/nexus-desktop-gpui")).isSymbolicLink());
  assert.equal(existsSync(join(dir, "home", "launched")), false);
  assert.deepEqual(r.leftovers, []);
  assert.deepEqual(readdirSync(join(dir, "home")).sort(), [".local", "app"], "no staging folder is left");
});

test("Linux: opens the app when there is a display, and defaults to ~/.local/share/nexus-desktop-gpui", (t) => {
  const dir = linuxFixture(t);
  const r = run(dir, { env: { FAKE_OS: "Linux", FAKE_ARCH: "x86_64", DISPLAY: ":99" } });
  assert.equal(r.status, 0, r.stderr);
  assert.ok(existsSync(join(dir, "home", ".local/share/nexus-desktop-gpui/nexus-desktop-gpui")));
  for (let i = 0; i < 40 && !existsSync(join(dir, "home", "launched")); i++) sleep(50);
  assert.equal(readFileSync(join(dir, "home", "launched"), "utf8"), "launched");
});

test("Linux: with no display, it says so instead of failing", (t) => {
  const dir = linuxFixture(t);
  const r = run(dir, { env: { FAKE_OS: "Linux", FAKE_ARCH: "x86_64" } });
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /No display found/);
});

test("Linux: a checksum mismatch installs nothing", (t) => {
  const dir = linuxFixture(t, { corrupt: true });
  const install = join(dir, "home", "app");
  const r = run(dir, { env: { FAKE_OS: "Linux", FAKE_ARCH: "x86_64", NEXUS_INSTALL_DIR: install } });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /checksum mismatch/);
  assert.equal(existsSync(install), false);
  assert.deepEqual(r.leftovers, []);
});

function installedLinux(t) {
  const dir = linuxFixture(t);
  const home = join(dir, "home");
  const install = join(home, "app");
  mkdirSync(install, { recursive: true });
  writeFileSync(join(install, "nexus-desktop-gpui"), "#!/bin/sh", { mode: 0o755 });
  writeFileSync(join(install, "install.sh"), "#!/bin/sh", { mode: 0o755 });
  writeFileSync(join(install, "marker"), "old contents");
  return { dir, home, install, env: { FAKE_OS: "Linux", FAKE_ARCH: "x86_64", NEXUS_INSTALL_DIR: install } };
}

test("Linux: a failed move restores the previous install and cleans the stage", (t) => {
  const { dir, home, install, env } = installedLinux(t);
  const r = run(dir, { args: ["--no-launch"], env: { ...env, FAKE_MV_FAIL: "install" } });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /could not move the new version/);
  assert.equal(readFileSync(join(install, "marker"), "utf8"), "old contents");
  assert.deepEqual(readdirSync(home).sort(), [".local", "app"]);
  assert.deepEqual(r.leftovers, []);
});

for (const signal of ["TERM", "INT"]) {
  test(`Linux: ${signal} after moving the old app aside restores it`, (t) => {
    const { dir, home, install, env } = installedLinux(t);
    const r = run(dir, { args: ["--no-launch"], env: { ...env, FAKE_MV_SIGNAL: signal } });
    assert.equal(r.status, 130, r.stderr);
    assert.equal(readFileSync(join(install, "marker"), "utf8"), "old contents");
    assert.deepEqual(readdirSync(home).sort(), [".local", "app"]);
    assert.deepEqual(r.leftovers, []);
  });
}

test("Linux: a failed restore leaves the previous install at the reported path", (t) => {
  const { dir, home, install, env } = installedLinux(t);
  const r = run(dir, { args: ["--no-launch"], env: { ...env, FAKE_MV_FAIL: "restore" } });
  assert.equal(r.status, 1);
  assert.equal(existsSync(install), false);
  const [stage] = readdirSync(home).filter((name) => name.startsWith(".nexus-desktop-install."));
  const backup = join(home, stage, "previous");
  assert.ok(r.stderr.includes(posix(backup)), r.stderr);
  assert.equal(readFileSync(join(backup, "marker"), "utf8"), "old contents");
  assert.deepEqual(r.leftovers, []);
});

test("Linux: a NEXUS_INSTALL_DIR that holds something else is refused before any download, and left whole", (t) => {
  const dir = linuxFixture(t);
  const install = join(dir, "home", "documents");
  mkdirSync(install, { recursive: true });
  writeFileSync(join(install, "thesis.txt"), "keep me");
  const r = run(dir, { env: { FAKE_OS: "Linux", FAKE_ARCH: "x86_64", NEXUS_INSTALL_DIR: install } });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /is not a Nexus install/);
  assert.equal(readFileSync(join(install, "thesis.txt"), "utf8"), "keep me");
  const calls = existsSync(join(dir, "calls")) ? readFileSync(join(dir, "calls"), "utf8") : "";
  assert.doesNotMatch(calls, /\.tar\.xz/, "nothing was downloaded");
});

test("Linux: a directory named like the binary does not make a folder a Nexus install", (t) => {
  const dir = linuxFixture(t);
  const install = join(dir, "home", "documents");
  mkdirSync(join(install, "nexus-desktop-gpui"), { recursive: true });
  writeFileSync(join(install, "thesis.txt"), "keep me");
  const r = run(dir, { env: { FAKE_OS: "Linux", FAKE_ARCH: "x86_64", NEXUS_INSTALL_DIR: install } });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /is not a Nexus install/);
  assert.equal(readFileSync(join(install, "thesis.txt"), "utf8"), "keep me");
});

test("Linux: a relative NEXUS_INSTALL_DIR installs where it points and the launcher link is absolute", (t) => {
  const dir = linuxFixture(t);
  const home = join(dir, "home");
  const r = run(dir, { args: ["--no-launch"], cwd: home, env: { FAKE_OS: "Linux", FAKE_ARCH: "x86_64", NEXUS_INSTALL_DIR: "rel-app" } });
  assert.equal(r.status, 0, r.stderr);
  const link = join(home, ".local", "bin", "nexus-desktop-gpui");
  assert.ok(lstatSync(link).isSymbolicLink());
  assert.ok(existsSync(link), "the link resolves");
  assert.ok(existsSync(join(home, "rel-app", "nexus-desktop-gpui")));
});

test("Linux: preserves an unrelated directory that replaces the app during download", (t) => {
  const dir = linuxFixture(t);
  const app = join(dir, "home", "chosen-install");
  mkdirSync(app);
  // A script, so it is executable on Windows too, where Git for Windows' sh reads `#!` instead of a mode.
  writeFileSync(join(app, "nexus-desktop-gpui"), "#!/bin/sh\nprevious\n", { mode: 0o755 });
  writeFileSync(join(app, "install.sh"), "previous", { mode: 0o755 });
  writeFileSync(join(app, "keep"), "unrelated");
  const result = run(dir, { env: {
    FAKE_OS: "Linux",
    FAKE_ARCH: "x86_64",
    NEXUS_INSTALL_DIR: app,
    FAKE_CHANGE_INSTALL_DIR: app,
  }});
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /install directory changed/);
  assert.equal(readFileSync(join(app, "keep"), "utf8"), "unrelated");
  assert.equal(existsSync(join(app, "nexus-desktop-gpui")), false);
});

test("Linux: arm64 is refused", (t) => {
  const dir = linuxFixture(t);
  const r = run(dir, { env: { FAKE_OS: "Linux", FAKE_ARCH: "aarch64" } });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /only x86_64/);
});

test("Windows shells and unknown options point to the right command", (t) => {
  const dir = linuxFixture(t);
  const win = run(dir, { env: { FAKE_OS: "MINGW64_NT-10.0" } });
  assert.equal(win.status, 1);
  assert.match(win.stderr, /install\.ps1/);
  const bad = run(dir, { args: ["--bogus"] });
  assert.equal(bad.status, 1);
  assert.match(bad.stderr, /unknown option/);
});

test("both scripts read only the stable manifest, add no query string and need no jq or python", () => {
  for (const file of [shScript, psScript]) {
    const text = readFileSync(file, "utf8");
    const code = text.split("\n").filter((line) => !line.trim().startsWith("#")).join("\n");
    assert.ok(text.includes(MANIFEST_URL), `${file} names the manifest`);
    assert.doesNotMatch(code, /downloads\.nexus\.inc[^'"\s]*\?/, "no query string on a URL");
    assert.doesNotMatch(code, /\b(jq|python3?)\b/, "no jq or python");
    assert.doesNotMatch(code, /gcloud/, "no gcloud");
  }
});

// install.ps1 runs in PowerShell. These run only where pwsh is installed (GitHub's Linux runners have it).
const pwsh = spawnSync("pwsh", ["-NoProfile", "-Command", "$PSVersionTable.PSVersion.Major"], { encoding: "utf8" });
const hasPwsh = pwsh.status === 0;

// The fake machine's environment. PROCESSOR_ARCHITEW6432 is left out, not set empty: Windows starts a 64-bit process
// with PROCESSOR_ARCHITECTURE taken from it whenever it is set, so an empty one would erase the architecture.
function psEnv(env) {
  const all = { ...process.env, ...env };
  delete all.PROCESSOR_ARCHITEW6432;
  return all;
}

// Runs the script's functions with its last line (the call) removed and Invoke-*, Start-Process stubbed.
function runPs(t, { manifest, body, env = {} }) {
  const dir = mkdtempSync(join(tmpdir(), "nexus-public-install-ps-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const lines = readFileSync(psScript, "utf8").trimEnd().split("\n");
  assert.equal(lines.pop(), "Install-Nexus", "the last line calls Install-Nexus");
  writeFileSync(join(dir, "lib.ps1"), lines.join("\n"));
  writeFileSync(join(dir, "manifest.json"), JSON.stringify(manifest));
  writeFileSync(
    join(dir, "run.ps1"),
    `. '${join(dir, "lib.ps1")}'
$script:log = @()
function Invoke-RestMethod { param($Uri, [switch]$UseBasicParsing) $script:log += "irm $Uri"; Get-Content -Raw '${join(dir, "manifest.json")}' }
function Invoke-WebRequest { param($Uri, $OutFile, [switch]$UseBasicParsing) $script:log += "iwr $Uri"; [IO.File]::WriteAllBytes($OutFile, [Text.Encoding]::ASCII.GetBytes('fake installer')) }
function Start-Process { param($FilePath, $ArgumentList, [switch]$Wait, [switch]$PassThru)
  $script:log += "start $([IO.Path]::GetFileName($FilePath)) $ArgumentList"
  if ($FilePath -like '*.exe' -and $ArgumentList -eq '/S') {
    $dest = Join-Path $env:LOCALAPPDATA 'Programs/Nexus'
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Set-Content -Path (Join-Path $dest 'nexus-desktop-gpui.exe') -Value 'app'
  }
  [pscustomobject]@{ ExitCode = [int]$env:FAKE_EXIT }
}
try { ${body}; Write-Host 'RESULT ok' } catch { Write-Host "RESULT error: $($_.Exception.Message)" }
$script:log | ForEach-Object { Write-Host "LOG $_" }
`,
  );
  const r = spawnSync("pwsh", ["-NoProfile", "-File", join(dir, "run.ps1")], {
    encoding: "utf8",
    timeout: 60_000,
    env: psEnv({ LOCALAPPDATA: join(dir, "local"), PROCESSOR_ARCHITECTURE: "AMD64", NEXUS_NO_LAUNCH: "", FAKE_EXIT: "0", ...env }),
  });
  return { ...r, dir };
}

const psManifest = (sha256, url = `${BASE}/9.9.9/nexus-desktop-9.9.9-windows-x86_64.exe`) => ({
  version: "9.9.9",
  downloads: { "windows-x86_64-nsis": { url, sha256, size: 14 } },
});

test("install.ps1: downloads, checks the hash, runs the installer silently, opens the app", { skip: !hasPwsh && "pwsh is not installed" }, (t) => {
  const r = runPs(t, { manifest: psManifest(sha("fake installer")), body: "Install-Nexus" });
  assert.match(r.stdout, /RESULT ok/, r.stdout + r.stderr);
  assert.match(r.stdout, /LOG irm https:\/\/downloads\.nexus\.inc\/desktop\/stable\/latest\.json/);
  assert.match(r.stdout, /LOG iwr https:\/\/downloads\.nexus\.inc\/desktop\/stable\/9\.9\.9\/nexus-desktop-9\.9\.9-windows-x86_64\.exe$/m);
  assert.match(r.stdout, /LOG start nexus-desktop-9\.9\.9-windows-x86_64\.exe \/S/);
  assert.match(r.stdout, /LOG start nexus-desktop-gpui\.exe\s*$/m, "it opens the installed app");
  assert.doesNotMatch(r.stdout, /LOG .*\?/, "no query string");
});

test("install.ps1: NEXUS_NO_LAUNCH=1 does not open the app", { skip: !hasPwsh && "pwsh is not installed" }, (t) => {
  const r = runPs(t, { manifest: psManifest(sha("fake installer")), body: "Install-Nexus", env: { NEXUS_NO_LAUNCH: "1" } });
  assert.match(r.stdout, /RESULT ok/, r.stdout + r.stderr);
  assert.doesNotMatch(r.stdout, /LOG start nexus-desktop-gpui\.exe\s*$/m);
});

test("install.ps1: a checksum mismatch throws before the installer runs", { skip: !hasPwsh && "pwsh is not installed" }, (t) => {
  const r = runPs(t, { manifest: psManifest("a".repeat(64)), body: "Install-Nexus" });
  assert.match(r.stdout, /RESULT error: Checksum mismatch/);
  assert.doesNotMatch(r.stdout, /LOG start/);
});

test("install.ps1: refuses an off-host address, a missing entry and a bad digest", { skip: !hasPwsh && "pwsh is not installed" }, (t) => {
  const off = runPs(t, { manifest: psManifest(sha("x"), "https://evil.test/desktop/stable/a.exe"), body: "Install-Nexus" });
  assert.match(off.stdout, /RESULT error: .*outside/);
  assert.doesNotMatch(off.stdout, /LOG iwr/);
  const none = runPs(t, { manifest: { version: "9.9.9", downloads: {} }, body: "Install-Nexus" });
  assert.match(none.stdout, /RESULT error: .*has no windows-x86_64-nsis download/);
  const bad = runPs(t, { manifest: psManifest("zz"), body: "Install-Nexus" });
  assert.match(bad.stdout, /RESULT error: .*64-character/);
});

test("install.ps1: a failing installer and a non-x64 machine throw", { skip: !hasPwsh && "pwsh is not installed" }, (t) => {
  const failed = runPs(t, { manifest: psManifest(sha("fake installer")), body: "Install-Nexus", env: { FAKE_EXIT: "2" } });
  assert.match(failed.stdout, /RESULT error: .*exit code 2/);
  const arm = runPs(t, { manifest: psManifest(sha("fake installer")), body: "Install-Nexus", env: { PROCESSOR_ARCHITECTURE: "ARM64" } });
  assert.match(arm.stdout, /RESULT error: .*64-bit x86/);
});
