//! The golden image build (scripts/cloud-image.sh) against the fake boat: the script creates a builder, runs
//! the install over `/commands` (on this computer, in a throwaway home, with stand-ins for `npm`, `sudo`, `loginctl`
//! and the uv installer), checks that no secret is in, saves the snapshots, keeps the newest three and deletes the
//! builder. Needs `curl` and `jq`.
#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Scratch, script, stubs};
use nexus_cloud::fake::{Config, FakeBoat};
use nexus_cloud::link::PLAYWRIGHT_MCP;
use serde_json::{Value, json};

fn script_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/cloud-image.sh")
}

/// A fake boat whose commands run in `home`, with `bin` first on their PATH.
async fn boat(dir: &Path) -> (FakeBoat, PathBuf) {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap_or_else(|e| panic!("{e}"));
    let bin = stubs(dir);
    // The real build needs these; here they only leave the marks the script checks.
    let h = home.display();
    script(
        &bin.join("npm"),
        &format!(
            r#"case "$1 $2" in
"install -g")
  if [ "$3" = --prefix ]; then
    pkg="$5"; ver="${{pkg##*@}}"; mkdir -p "$4/bin"
    case "$pkg" in
      @openai/codex*) printf '#!/bin/sh\necho "codex-cli %s"\n' "$ver" > "$4/bin/codex"; chmod 755 "$4/bin/codex" ;;
      @opencode/cli*) printf '#!/bin/sh\necho "%s"\n' "$ver" > "$4/bin/opencode"; chmod 755 "$4/bin/opencode" ;;
      *) printf '#!/bin/sh\necho "%s (Claude Code)"\n' "$ver" > "$4/bin/claude"; chmod 755 "$4/bin/claude" ;;
    esac
  else
    echo "$3" >> '{h}/.npm-global'
    mkdir -p '{h}/.npm-root/@playwright/mcp'
    echo '{{"dependencies":{{"playwright":"1.65.0-test"}}}}' > '{h}/.npm-root/@playwright/mcp/package.json'
  fi ;;
"ls -g") grep -qx "$3" '{h}/.npm-global' 2>/dev/null ;;
"root -g") echo '{h}/.npm-root' ;;
esac"#
        ),
    );
    script(
        &bin.join("npx"),
        &format!(
            "echo \"$*\" >> '{h}/.npx'\ncase \"$*\" in *' install chromium') mkdir -p \"$PLAYWRIGHT_BROWSERS_PATH/chromium-1234\" ;; esac"
        ),
    );
    script(
        &bin.join("apt-get"),
        &format!(
            "case \"$*\" in *install*ffmpeg*) cp '{b}/ffmpeg-real' '{b}/ffmpeg' ;; esac",
            b = bin.display()
        ),
    );
    script(&bin.join("ffmpeg-real"), "echo ffmpeg");
    script(&bin.join("sudo"), "[ \"$1\" = -n ] && shift; exec \"$@\"");
    script(
        &bin.join("loginctl"),
        &format!(
            "case \"$1\" in show-user) [ -f '{h}/.linger' ] && echo yes || echo no ;; enable-linger) touch '{h}/.linger' ;; esac",
            h = home.display()
        ),
    );
    script(
        &bin.join("curl"),
        "case \"$*\" in *astral.sh*) echo 'mkdir -p \"$UV_UNMANAGED_INSTALL\"; printf \"#!/bin/sh\\necho uv 0.13.7\\n\" > \"$UV_UNMANAGED_INSTALL/uv\"; chmod 755 \"$UV_UNMANAGED_INSTALL/uv\"' ;; *) exec /usr/bin/curl \"$@\" ;; esac",
    );
    let fake = FakeBoat::start(Config {
        ready_after: Duration::from_millis(100),
        stop_after: Duration::from_millis(100),
        exec_cwd: Some(home.clone()),
        exec_env: vec![
            ("HOME".into(), home.display().to_string()),
            ("PATH".into(), format!("{}:/usr/bin:/bin", bin.display())),
        ],
        ..Config::default()
    })
    .await
    .unwrap_or_else(|e| panic!("{e}"));
    (fake, home)
}

/// The nexus-host the build installs: a script that says its version, gzipped as the release file is.
fn host_file(dir: &Path) -> PathBuf {
    let plain = dir.join("nexus-host");
    script(&plain, "[ \"$1\" = version ] && echo 'nexus-host 0.1.0-test'");
    let gz = dir.join("nexus-host.gz");
    let done = nexus_workbench::process::command("sh")
        .arg("-c")
        .arg("gzip -9 --no-name --stdout \"$1\" > \"$2\"")
        .arg("sh")
        .arg(&plain)
        .arg(&gz)
        .status();
    assert!(done.is_ok_and(|s| s.success()));
    gz
}

async fn build(fake: &FakeBoat, host: &Path, version: &str) -> std::process::Output {
    build_with(fake, host, version, &[]).await
}

async fn build_with(fake: &FakeBoat, host: &Path, version: &str, more: &[(&str, &str)]) -> std::process::Output {
    nexus_workbench::process::async_command("sh")
        .arg(script_path())
        .env("BOAT_API_KEY", "boat_fake")
        .env("BOAT_API_BASE", fake.url())
        .env("NEXUS_HOST_PATH", host)
        .env("VERSION", version)
        .env("IMAGE_HOST_VERSION", version)
        .env("CLOUD_IMAGE_POLL_S", "0.2")
        .env("CLOUD_IMAGE_TIMEOUT_S", "60")
        .envs(more.iter().copied())
        .output()
        .await
        .unwrap_or_else(|e| panic!("{e}"))
}

fn text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// One call to the fake with `curl` (the script's own client): the status and the body.
async fn call(fake: &FakeBoat, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut command = nexus_workbench::process::async_command("curl");
    command
        .args([
            "-sS",
            "-X",
            method,
            "-H",
            "Authorization: Bearer boat_fake",
            "-w",
            "\n%{http_code}",
        ])
        .arg(format!("{}{path}", fake.url()));
    if let Some(body) = body {
        command.args(["-H", "Content-Type: application/json", "-d", &body.to_string()]);
    }
    let output = command.output().await.unwrap_or_else(|e| panic!("{e}"));
    let out = String::from_utf8_lossy(&output.stdout).into_owned();
    let (body, status) = out.rsplit_once('\n').unwrap_or(("", "0"));
    (
        status.trim().parse().unwrap_or(0),
        serde_json::from_str(body).unwrap_or_default(),
    )
}

async fn names(fake: &FakeBoat) -> Vec<String> {
    let (_, answer) = call(fake, "GET", "/saved-snapshots", None).await;
    let mut all: Vec<String> = answer["snapshots"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    all.sort();
    all
}

fn builders(fake: &FakeBoat) -> Vec<String> {
    fake.calls()
        .iter()
        .filter(|c| c.method == "DELETE" && c.path.starts_with("/sandboxes/"))
        .map(|c| c.path.trim_start_matches("/sandboxes/").to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn builds_the_image_saves_it_twice_keeps_three_versions_and_deletes_every_builder() {
    let dir = Scratch::new("image");
    let (fake, home) = boat(dir.path()).await;
    let host = host_file(dir.path());
    for version in [
        "0.1.0-internal.1",
        "0.1.0-internal.2",
        "0.1.0-internal.3",
        "0.1.0-internal.4",
    ] {
        let output = build(&fake, &host, version).await;
        assert!(output.status.success(), "{version}: {}", text(&output));
    }

    // What the machine got.
    // The folder is the version the host reports (the fake says 0.1.0-test), not the snapshot's slug.
    let bin = home.join(".nexus/host").join("0.1.0-test").join("nexus-host");
    assert!(bin.is_file(), "nexus-host installed under the version's folder");
    let unit = std::fs::read_to_string(home.join(".config/systemd/user/nexus-host.service")).unwrap_or_default();
    assert!(unit.contains("serve") && unit.contains("WantedBy=default.target"));
    assert!(
        home.join(".config/systemd/user/default.target.wants/nexus-host.service")
            .is_symlink()
    );
    assert!(home.join(".linger").exists(), "linger enabled");
    assert!(!home.join(".nexus/host/token").exists(), "the host was never started");
    let claude = nexus_workbench::process::command(home.join(".local/bin/claude"))
        .arg("--version")
        .output();
    assert!(claude.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).starts_with("2.1.")));
    let state: Value = serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap_or_default())
        .unwrap_or_default();
    assert_eq!(state["hasCompletedOnboarding"], json!(true));
    assert_eq!(state["autoUpdates"], json!(false));
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".claude/settings.json")).unwrap_or_default())
            .unwrap_or_default();
    assert_eq!(settings["env"]["DISABLE_AUTOUPDATER"], json!("1"));
    assert!(
        std::fs::read_to_string(home.join(".npm-global"))
            .unwrap_or_default()
            .lines()
            .any(|l| l == PLAYWRIGHT_MCP),
        "the Playwright MCP package link.rs names"
    );
    assert!(home.join(".local/bin/uv").is_file());
    let codex = nexus_workbench::process::command(home.join(".local/bin/codex"))
        .arg("--version")
        .output();
    assert!(codex.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).starts_with("codex-cli 0.")));
    assert!(!home.join(".codex/auth.json").exists(), "no Codex sign-in");
    let opencode = nexus_workbench::process::command(home.join(".local/bin/opencode"))
        .arg("--version")
        .output();
    assert!(opencode.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).starts_with("2.3.")));
    assert!(
        !home.join(".local/share/opencode/auth.json").exists(),
        "no OpenCode sign-in"
    );
    assert!(
        home.join(".nexus/browsers/chromium-1234").is_dir(),
        "Chromium in the persisted folder"
    );
    let npx = std::fs::read_to_string(home.join(".npx")).unwrap_or_default();
    assert!(npx.contains("playwright@1.65.0-test install-deps chromium"), "{npx}");
    assert!(
        std::fs::read_to_string(home.join(".claude/settings.json"))
            .unwrap_or_default()
            .contains("PLAYWRIGHT_BROWSERS_PATH")
    );
    assert!(home.join(".nexus/cloud-image.json").is_file());
    assert!(dir.path().join("bin/ffmpeg").exists(), "ffmpeg installed through apt");
    assert!(home.join(".boat/pre-stop").is_file());
    assert!(
        std::fs::read_to_string(home.join(".boatignore"))
            .unwrap_or_default()
            .contains(".cache/")
    );

    // What boat was asked.
    let calls = fake.calls();
    let create = calls.iter().find(|c| c.method == "POST" && c.path == "/sandboxes");
    assert_eq!(create.map(|c| c.body["noEnv"].clone()), Some(json!(true)));
    assert_eq!(create.map(|c| c.body["type"].clone()), Some(json!("small")));
    let saved = names(&fake).await;
    assert_eq!(saved.len(), 4, "the alias and the newest three: {saved:?}");
    assert_eq!(saved[0], "nexus-cloud");
    for (name, n) in saved[1..].iter().zip(2..) {
        let hash = name.rsplit('-').next().unwrap_or("");
        assert!(
            name.starts_with(&format!("nexus-cloud-0-1-0-internal-{n}-")) && hash.len() == 8,
            "{name}"
        );
    }
    let ids = builders(&fake);
    assert_eq!(ids.len(), 4, "one builder per run, each deleted");
    assert!(ids.iter().all(|id| fake.state(id).is_none()));

    // A new machine can start from the alias; an unknown image is boat's 404.
    let create = |from: &'static str| {
        call(
            &fake,
            "POST",
            "/sandboxes",
            Some(json!({ "name": "person", "from": from, "noEnv": true })),
        )
    };
    assert_eq!(create("nexus-cloud").await.0, 202);
    assert_eq!(create("nexus-cloud-nope").await.0, 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_on_the_builder_stops_the_build_saves_nothing_and_still_deletes_it() {
    // Each is a way a credential could get into the image: a file that only a sign-in or a started host writes, or a
    // token inside a file of the agents' folders.
    let cases = [
        (".config/gh/hosts.yml", "github.com: {}\n"),
        (".codex/auth.json", "{}\n"),
        (".claude/.credentials.json", "{}\n"),
        (".local/share/opencode/auth.json", "{}\n"),
        (".nexus/host/agent.env", "CLAUDE_CODE_OAUTH_TOKEN=x\n"),
        (".nexus/host/token", "t\n"),
        (".claude/notes.txt", "key sk-ant-test\n"),
        (".codex/config.toml", "token = \"sk-ant-test\"\n"),
    ];
    for (file, content) in cases {
        let dir = Scratch::new("image-secret");
        let (fake, home) = boat(dir.path()).await;
        let host = host_file(dir.path());
        let planted = home.join(file);
        std::fs::create_dir_all(planted.parent().unwrap_or(&home)).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(&planted, content).unwrap_or_else(|e| panic!("{e}"));

        let output = build(&fake, &host, "0.1.0-internal.9").await;
        assert!(!output.status.success(), "{file}: the build went on");
        assert!(
            text(&output).contains("secret or state found"),
            "{file}: {}",
            text(&output)
        );
        assert!(names(&fake).await.is_empty(), "{file}: no snapshot saved");
        let ids = builders(&fake);
        assert_eq!(ids.len(), 1, "{file}");
        assert!(fake.state(&ids[0]).is_none(), "{file}: the builder is gone");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_internal_build_of_the_same_release_does_not_rebuild_the_image() {
    let dir = Scratch::new("image-release");
    let (fake, _home) = boat(dir.path()).await;
    let host = host_file(dir.path());
    let release = [("IMAGE_HOST_VERSION", "0.1.0")];
    let first = build_with(&fake, &host, "0.1.0-internal.1", &release).await;
    assert!(first.status.success(), "{}", text(&first));
    let second = build_with(&fake, &host, "0.1.0-internal.2", &release).await;
    assert!(
        second.status.success() && text(&second).contains("unchanged"),
        "{}",
        text(&second)
    );
    let creates = fake
        .calls()
        .iter()
        .filter(|c| c.method == "POST" && c.path == "/sandboxes")
        .count();
    assert_eq!(creates, 1, "no second builder");
}

/// A `curl` first on the script's own PATH that logs its arguments, then runs the real one.
fn logging_curl(dir: &Path) -> (String, PathBuf) {
    let bin = dir.join("script-bin");
    let log = dir.join("curl-args.log");
    script(
        &bin.join("curl"),
        &format!(
            "printf '%s\\n' \"$*\" >> '{}'\nexec /usr/bin/curl \"$@\"",
            log.display()
        ),
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    (path, log)
}

fn sandbox_creates(fake: &FakeBoat) -> usize {
    fake.calls()
        .iter()
        .filter(|c| c.method == "POST" && c.path == "/sandboxes")
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_api_key_and_the_upload_url_never_reach_curls_command_line() {
    let dir = Scratch::new("image-argv");
    let (fake, _home) = boat(dir.path()).await;
    let host = host_file(dir.path());
    let (path, log) = logging_curl(dir.path());
    let output = build_with(&fake, &host, "0.1.0-internal.1", &[("PATH", &path)]).await;
    assert!(output.status.success(), "{}", text(&output));
    let argv = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(argv.lines().count() > 5, "curl ran through the logger: {argv}");
    assert!(!argv.contains("boat_fake"), "the key is on a command line: {argv}");
    assert!(
        !argv.contains("/upload/"),
        "the pre-signed URL is on a command line: {argv}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_quote_in_a_value_stays_inside_it_on_the_builder() {
    let dir = Scratch::new("image-quote");
    let (fake, home) = boat(dir.path()).await;
    let host = host_file(dir.path());
    let url = format!("file://{}", host.display());
    let sha = "0000'; touch \"$HOME/injected\"; '";
    let output = build_with(
        &fake,
        &host,
        "0.1.0-internal.1",
        &[("NEXUS_HOST_URL", &url), ("NEXUS_HOST_SHA256", sha)],
    )
    .await;
    assert!(!output.status.success(), "a wrong checksum must stop the build");
    assert!(text(&output).contains("checksum is"), "{}", text(&output));
    assert!(
        !home.join("injected").exists(),
        "the value ran as a command on the builder"
    );
    assert!(names(&fake).await.is_empty(), "nothing saved");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_download_without_a_checksum_is_refused_before_any_builder() {
    let dir = Scratch::new("image-nosum");
    let (fake, _home) = boat(dir.path()).await;
    let host = host_file(dir.path());
    let url = format!("file://{}", host.display());
    let output = build_with(&fake, &host, "0.1.0-internal.1", &[("NEXUS_HOST_URL", &url)]).await;
    assert!(!output.status.success());
    assert!(
        text(&output).contains("NEXUS_HOST_SHA256 is required"),
        "{}",
        text(&output)
    );
    assert_eq!(sandbox_creates(&fake), 0, "no builder, so no cost");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_boat_base_without_https_is_refused_before_the_key_is_sent() {
    let dir = Scratch::new("image-http");
    let (fake, _home) = boat(dir.path()).await;
    let host = host_file(dir.path());
    let output = build_with(
        &fake,
        &host,
        "0.1.0-internal.1",
        &[("BOAT_API_BASE", "http://boat.example.test/api/v1")],
    )
    .await;
    assert!(!output.status.success());
    assert!(text(&output).contains("must be https"), "{}", text(&output));
    assert!(fake.calls().is_empty());
}

#[test]
fn the_install_refuses_a_download_without_its_checksum_and_a_host_dir_that_is_a_path() {
    let dir = Scratch::new("image-install");
    let install = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/cloud-image-install.sh");
    let run = |more: &[(&str, &str)]| {
        let output = nexus_workbench::process::command("sh")
            .arg(&install)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", dir.path())
            .env("STAGE", "host")
            .env("HOST_DIR", "slug")
            .env("CLAUDE_CODE_VERSION", "1")
            .env("CODEX_VERSION", "1")
            .env("OPENCODE_VERSION", "1")
            .env("PLAYWRIGHT_MCP", "@playwright/mcp@1")
            .env("UV_VERSION", "1")
            .envs(more.iter().copied())
            .output()
            .unwrap_or_else(|e| panic!("{e}"));
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let (ok, err) = run(&[("HOST_URL", "file:///nonexistent")]);
    assert!(!ok && err.contains("HOST_SHA256 is required"), "{err}");
    for bad in ["../escape", "a/b", ".hidden"] {
        let (ok, err) = run(&[("HOST_DIR", bad), ("HOST_FILE", "/nonexistent")]);
        assert!(!ok && err.contains("must be one folder name"), "{bad}: {err}");
    }
    assert!(!dir.path().join(".nexus").exists(), "nothing written");
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_built_when_no_input_changed_unless_forced() {
    let dir = Scratch::new("image-skip");
    let (fake, _home) = boat(dir.path()).await;
    let host = host_file(dir.path());
    let creates = |fake: &FakeBoat| {
        fake.calls()
            .iter()
            .filter(|c| c.method == "POST" && c.path == "/sandboxes")
            .count()
    };
    let first = build(&fake, &host, "0.1.0-internal.1").await;
    assert!(first.status.success(), "{}", text(&first));
    assert_eq!(creates(&fake), 1);

    // Same inputs: boat is only read.
    let before = fake.calls().len();
    let again = build(&fake, &host, "0.1.0-internal.1").await;
    assert!(
        again.status.success() && text(&again).contains("unchanged"),
        "{}",
        text(&again)
    );
    assert_eq!(creates(&fake), 1, "no builder, so no cost");
    assert!(fake.calls()[before..].iter().all(|c| c.method == "GET"), "only reads");

    // A new host release is a changed input; so is a new Claude Code; FORCE builds anyway.
    let next = build(&fake, &host, "0.2.0").await;
    assert!(next.status.success(), "{}", text(&next));
    assert_eq!(creates(&fake), 2);
    let claude = build_with(&fake, &host, "0.2.0", &[("CLAUDE_CODE_VERSION", "2.1.400")]).await;
    assert!(
        claude.status.success() && !text(&claude).contains("unchanged"),
        "{}",
        text(&claude)
    );
    assert_eq!(creates(&fake), 3);
    let forced = build_with(
        &fake,
        &host,
        "0.2.0",
        &[("CLAUDE_CODE_VERSION", "2.1.400"), ("FORCE", "1")],
    )
    .await;
    assert!(forced.status.success(), "{}", text(&forced));
    assert_eq!(creates(&fake), 4);
}
