//! `nexus recordings` (`recordings.ts`): the screen recordings Nexus for Mac makes for agents (⌥⇧S;
//! `desktop/README.md`, "Screen recordings for agents"), read from this computer's folder. Nothing goes to Gateway.
//! `list`, `show` (its summary.md, or with --json its capture.json), `path` and `frames` (more frames, drawn by Nexus for
//! Mac's binary). `nexus claude --recording` and `nexus codex --recording` start the agent on one ([`prompt`]).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Context;
use crate::args::Args;
use crate::error::{CliError, Result, usage};
use crate::json::pretty;
use crate::style::{Style, pad_end};

pub const RECORDINGS_COMMANDS: &[(&str, &str)] = &[
    (
        "list [--json]",
        "Your screen recordings from Nexus for Mac (⌥⇧S), newest first: when, how long, keyframes and the first words said",
    ),
    (
        "show [<id>|latest] [--json]",
        "A recording's summary.md, what an agent reads first (--json: its capture.json)",
    ),
    ("path [<id>|latest]", "A recording's folder, for scripts and agents"),
    (
        "frames [<id>|latest] --from <time> --to <time> [--count N]",
        "More frames from its video, at full resolution, for a closer look (Nexus for Mac draws them)",
    ),
];

const NONE_HINT: &str =
    "Record one with Nexus for Mac: ⌥⇧S starts and stops it (hold ⇧ and move to draw on the screen).";

/// Where Nexus for Mac keeps its recordings, or none on this computer.
pub fn folder(ctx: &Context) -> Option<PathBuf> {
    if let Some(dir) = ctx.var("NEXUS_RECORDINGS_DIR").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = ctx.var("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/xyz.nexus.desktop/recordings"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub id: String,
    pub path: PathBuf,
    pub created_at: Option<String>,
    pub duration: f64,
    pub frames: usize,
    pub narrated: bool,
    pub said: Option<String>,
}

impl Recording {
    /// As `nexus recordings --json` lists it (the TypeScript CLI's field order).
    fn to_json(&self) -> Value {
        let mut value = json!({ "id": self.id, "path": self.path.display().to_string() });
        if let Some(created_at) = &self.created_at {
            value["createdAt"] = json!(created_at);
        }
        value["duration"] = json!(self.duration);
        value["frames"] = json!(self.frames);
        value["narrated"] = json!(self.narrated);
        if let Some(said) = &self.said {
            value["said"] = json!(said);
        }
        value
    }
}

fn read_capture(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path.join("capture.json")).ok()?;
    let capture: Value = serde_json::from_str(&text).ok()?;
    (capture.get("kind").and_then(Value::as_str) == Some("nexus.screen-recording")).then_some(capture)
}

/// Every finished recording in `dir`, newest first (a folder's name is its time).
pub fn list(dir: &Path) -> Vec<Recording> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name != "latest" && !name.starts_with('.'))
        .collect();
    names.sort();
    names.reverse();
    names
        .into_iter()
        .filter_map(|id| {
            let path = dir.join(&id);
            let capture = read_capture(&path)?;
            let said = capture
                .pointer("/transcript/phrases/0/text")
                .and_then(Value::as_str)
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty());
            Some(Recording {
                id,
                created_at: capture.get("createdAt").and_then(Value::as_str).map(String::from),
                duration: capture.get("duration").and_then(Value::as_f64).unwrap_or(0.0),
                frames: capture.get("frames").and_then(Value::as_array).map_or(0, Vec::len),
                narrated: capture.get("narration").and_then(Value::as_bool).unwrap_or(false),
                said,
                path,
            })
        })
        .collect()
}

/// `0:04`, `12:30`, `1:02:03`.
pub fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (hours, minutes, rest) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{rest:02}")
    } else {
        format!("{minutes}:{rest:02}")
    }
}

/// The recording `which` names (an id, or `latest` / nothing for the newest), or an error that says what exists.
pub fn find(ctx: &Context, which: Option<&str>) -> Result<Recording> {
    let Some(dir) = folder(ctx) else {
        return usage(
            "Screen recordings for agents are made by Nexus for Mac, on macOS.",
            Some("Set NEXUS_RECORDINGS_DIR to read recordings copied from a Mac."),
        );
    };
    let recordings = list(&dir);
    let Some(newest) = recordings.first() else {
        return Err(CliError::hint(format!("No screen recordings in {}.", dir.display()), NONE_HINT).into());
    };
    match which {
        None | Some("latest") => Ok(newest.clone()),
        Some(which) => recordings
            .iter()
            .find(|recording| recording.id == which || recording.path == Path::new(which))
            .cloned()
            .ok_or_else(|| {
                CliError::hint(
                    format!("No recording {which}."),
                    "`nexus recordings` lists them, newest first.",
                )
                .into()
            }),
    }
}

/// The agent's first message about a recording (`recordingPrompt` in recordings.ts): where it is, what to read in
/// which order, what the marks mean and how to look closer.
pub fn prompt(recording: &Recording) -> String {
    [
        format!(
            "I recorded my screen to show you something (Nexus for Mac, {}{}). It is in {}",
            clock(recording.duration),
            if recording.narrated { ", narrated" } else { "" },
            recording.path.display()
        ),
        String::new(),
        format!(
            "Read summary.md there first: a timeline with one section per keyframe, with {}the red ink and numbered pins I drew, my clicks (places are pixels in that keyframe) and the app in front with a browser tab's address. Then open contact-sheet.png (every keyframe, numbered with its time) and the frames/NN.png it points at. capture.json has the same as data.",
            if recording.narrated { "what I said around it (quoted), " } else { "" }
        ),
        String::new(),
        format!(
            "The red ink and the pins are mine: they point at what I mean.{} For a closer look at a moment between keyframes: `nexus recordings frames {} --from 0:10 --to 0:14` (times as in summary.md).",
            if recording.narrated { " My narration is the request." } else { "" },
            recording.id
        ),
        String::new(),
        "Before you change anything, tell me in a sentence or two what you understood: what I showed, what is wrong or wanted, and where.".to_string(),
    ]
    .join("\n")
}

/// The desktop app's binary that draws frames from a recording's video (`--frames`): Nexus's (the GPUI app), else the
/// older Tauri app's.
fn desktop_binary(ctx: &Context) -> Option<PathBuf> {
    desktop_binary_in(std::env::consts::OS, Path::new("/"), &ctx.env_fn())
}

/// The binaries [`desktop_binary`] tries on `os`, first found wins: `NEXUS_DESKTOP_BIN`, then the installed apps where
/// their installers put them (`desktop/scripts/install.sh`, the Windows installer). On macOS `Nexus.app` is the GPUI
/// app (`nexus-desktop-gpui`) or the older Tauri app (`nexus-desktop`); GPUI builds before the rename are
/// `Nexus GPUI.app`, and on Windows `Programs\Nexus GPUI`. `root` is the file system's root, a temp folder in tests.
fn desktop_binary_in(os: &str, root: &Path, var: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|value| !value.is_empty());
    let mut candidates: Vec<PathBuf> = set("NEXUS_DESKTOP_BIN").map(PathBuf::from).into_iter().collect();
    let home = set("HOME").or_else(|| set("USERPROFILE")).map(PathBuf::from);
    match os {
        "macos" => {
            let apps = [
                Some(root.join("Applications")),
                home.as_ref().map(|home| home.join("Applications")),
            ];
            for (bundle, bin) in [
                ("Nexus.app", "nexus-desktop-gpui"),
                ("Nexus.app", "nexus-desktop"),
                ("Nexus GPUI.app", "nexus-desktop-gpui"),
            ] {
                for apps in apps.iter().flatten() {
                    candidates.push(apps.join(bundle).join("Contents/MacOS").join(bin));
                }
            }
        }
        "windows" => {
            if let Some(local) = set("LOCALAPPDATA") {
                for folder in ["Nexus", "Nexus GPUI"] {
                    candidates.push(
                        Path::new(&local)
                            .join("Programs")
                            .join(folder)
                            .join("nexus-desktop-gpui.exe"),
                    );
                }
            }
        }
        _ => {
            let data = set("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
            if let Some(data) = data {
                candidates.push(data.join("nexus-desktop-gpui/nexus-desktop-gpui"));
            }
            if let Some(home) = &home {
                candidates.push(home.join(".local/bin/nexus-desktop-gpui"));
            }
        }
    }
    candidates.into_iter().find(|path| path.exists())
}

pub fn recordings_help(style: &Style) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{}  nexus recordings [list|show|path|frames] [<id>|latest]",
            style.bold("Usage")
        ),
        String::new(),
        "Screen recordings Nexus for Mac makes for agents (⌥⇧S), on this computer. `nexus claude --recording latest` starts"
            .to_string(),
        "Claude Code on one; `nexus codex --recording latest`, Codex.".to_string(),
        String::new(),
    ];
    lines.extend(
        RECORDINGS_COMMANDS
            .iter()
            .map(|(name, text)| format!("  {}{text}", style.cyan(&pad_end(name, 58)))),
    );
    lines
}

pub fn recordings_command(rest: &[String], ctx: &Context, args: &Args, json: bool, style: &Style) -> Result<i32> {
    let out = |line: &str| ctx.io.out(line);
    let print = |value: &Value| {
        ctx.io.out(&pretty(value));
        Ok(0)
    };
    let action = rest.first().map(String::as_str).unwrap_or("list");
    let which = rest.get(1).map(String::as_str);
    match action {
        "help" => {
            if json {
                return print(&json!({
                    "usage": "nexus recordings [list|show|path|frames] [<id>|latest]",
                    "commands": ["list", "show", "path", "frames"],
                }));
            }
            for line in recordings_help(style) {
                out(&line);
            }
            Ok(0)
        }
        "list" => {
            let dir = folder(ctx);
            let recordings = dir.as_deref().map(list).unwrap_or_default();
            if json {
                return print(&json!({
                    "folder": dir.as_ref().map(|dir| dir.display().to_string()),
                    "recordings": recordings.iter().map(Recording::to_json).collect::<Vec<_>>(),
                }));
            }
            if recordings.is_empty() {
                match &dir {
                    Some(dir) => out(&format!("No screen recordings in {}.", dir.display())),
                    None => out("Screen recordings for agents are made by Nexus for Mac, on macOS."),
                }
                out(&style.dim(NONE_HINT));
                return Ok(0);
            }
            for recording in &recordings {
                let mut facts = vec![
                    clock(recording.duration),
                    format!(
                        "{} keyframe{}",
                        recording.frames,
                        if recording.frames == 1 { "" } else { "s" }
                    ),
                ];
                if recording.narrated {
                    facts.push("narrated".into());
                }
                let said = recording
                    .said
                    .as_ref()
                    .map(|said| {
                        let short: String = if said.chars().count() > 60 {
                            format!("{}…", said.chars().take(59).collect::<String>())
                        } else {
                            said.clone()
                        };
                        format!("  “{short}”")
                    })
                    .unwrap_or_default();
                out(&format!(
                    "{}  {}{said}",
                    style.bold(&recording.id),
                    style.dim(&facts.join(" · "))
                ));
            }
            out("");
            out(&style.dim(&format!(
                "{} · nexus recordings show latest · nexus claude --recording latest",
                dir.map(|dir| dir.display().to_string()).unwrap_or_default()
            )));
            Ok(0)
        }
        "show" | "path" => {
            let recording = find(ctx, which)?;
            if action == "path" {
                if json {
                    return print(&json!({ "id": recording.id, "path": recording.path.display().to_string() }));
                }
                out(&recording.path.display().to_string());
                return Ok(0);
            }
            if json {
                let capture = read_capture(&recording.path).unwrap_or(Value::Null);
                return print(&capture);
            }
            let Ok(summary) = std::fs::read_to_string(recording.path.join("summary.md")) else {
                return Err(CliError::hint(
                    format!("{} has no summary.md yet.", recording.id),
                    "Nexus for Mac writes it a few seconds after the recording stops.",
                )
                .into());
            };
            for line in summary.trim_end_matches('\n').split('\n') {
                out(line);
            }
            Ok(0)
        }
        "frames" => {
            let recording = find(ctx, which)?;
            let (Some(from), Some(to)) = (args.flag_set("from"), args.flag_set("to")) else {
                return usage(
                    "Usage: nexus recordings frames [<id>|latest] --from <time> --to <time> [--count N]",
                    Some("Times as summary.md writes them (0:12) or in seconds (12.5)."),
                );
            };
            let Some(bin) = desktop_binary(ctx) else {
                return Err(CliError::hint(
                    "Nexus for Mac draws the frames, and it is not installed here.",
                    "Install Nexus, or set NEXUS_DESKTOP_BIN to its binary (nexus-desktop-gpui, or nexus-desktop for the older app).",
                )
                .into());
            };
            let mut command = std::process::Command::new(bin);
            command
                .arg("--frames")
                .arg(&recording.path)
                .args(["--from", from, "--to", to]);
            if let Some(count) = args.flag_set("count") {
                command.args(["--count", count]);
            }
            let run = command
                .output()
                .map_err(|error| CliError::new(format!("Could not run Nexus for Mac: {error}")))?;
            if !run.status.success() {
                let text = String::from_utf8_lossy(&run.stderr).trim().to_string();
                let message = ["nexus-desktop-gpui: ", "nexus-desktop: "]
                    .iter()
                    .find_map(|prefix| text.strip_prefix(prefix))
                    .unwrap_or(&text)
                    .to_string();
                let message = if message.is_empty() {
                    "Nexus for Mac could not draw the frames.".to_string()
                } else {
                    message
                };
                return Err(CliError {
                    message,
                    hint: None,
                    exit_code: if run.status.code() == Some(2) { 2 } else { 1 },
                    code: None,
                }
                .into());
            }
            let stdout = String::from_utf8_lossy(&run.stdout).to_string();
            let paths: Vec<&str> = stdout.trim().split('\n').filter(|line| !line.is_empty()).collect();
            if json {
                return print(&json!({ "id": recording.id, "frames": paths }));
            }
            for path in paths {
                out(path);
            }
            Ok(0)
        }
        other => usage(
            format!("Unknown command: nexus recordings {other}"),
            Some("`nexus recordings help` lists them."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_newest_first_and_reads_what_an_agent_needs() {
        let dir = std::env::temp_dir().join(format!("nexus-cli-recordings-{}", std::process::id()));
        for (id, said) in [
            ("2026-10-19T14-03-22", "This button is cut off."),
            ("2026-10-20T09-00-00", ""),
        ] {
            let path = dir.join(id);
            std::fs::create_dir_all(&path).expect("dir");
            let capture = json!({
                "kind": "nexus.screen-recording", "version": 1, "createdAt": "2026-10-19T14:03:22-0600",
                "duration": 42.4, "narration": true, "frames": [{}, {}, {}],
                "transcript": { "status": "ok", "phrases": [{ "start": 0.5, "end": 2.0, "text": said }] },
            });
            std::fs::write(path.join("capture.json"), capture.to_string()).expect("write");
        }
        std::fs::create_dir_all(dir.join("not-a-recording")).expect("dir");
        let recordings = list(&dir);
        assert_eq!(
            recordings.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["2026-10-20T09-00-00", "2026-10-19T14-03-22"]
        );
        assert_eq!(
            (
                recordings[1].frames,
                recordings[1].narrated,
                recordings[1].said.as_deref()
            ),
            (3, true, Some("This button is cut off."))
        );
        assert_eq!(recordings[0].said, None);
        let text = prompt(&recordings[1]);
        assert!(
            text.starts_with("I recorded my screen to show you something (Nexus for Mac, 0:42, narrated). It is in ")
        );
        assert!(text.contains("`nexus recordings frames 2026-10-19T14-03-22 --from 0:10 --to 0:14`"));
        assert!(text.contains("with what I said around it (quoted), the red ink"));
        assert!(text.contains("they point at what I mean. My narration is the request."));
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(clock(3723.0), "1:02:03");
    }

    #[test]
    fn frames_find_nexus_app_first_then_nexus_gpui_and_the_override_wins() {
        let root = std::env::temp_dir().join(format!("nexus-cli-desktop-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home/ruy");
        let place = |path: PathBuf| {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(&path, "").expect("write");
            path
        };
        let env = |pairs: Vec<(&'static str, String)>| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.clone())
            }
        };
        let with_home = env(vec![("HOME", home.display().to_string())]);
        let find = |os: &str, var: &dyn Fn(&str) -> Option<String>| desktop_binary_in(os, &root, var);

        // Nothing installed.
        assert_eq!(find("macos", &with_home), None);
        assert_eq!(find("linux", &with_home), None);

        // macOS: a GPUI build from before the rename in ~/Applications, until the Tauri app's Nexus.app is installed,
        // and the renamed GPUI app's Nexus.app before both.
        let gpui_mac = place(home.join("Applications/Nexus GPUI.app/Contents/MacOS/nexus-desktop-gpui"));
        assert_eq!(find("macos", &with_home), Some(gpui_mac.clone()));
        let tauri_mac = place(root.join("Applications/Nexus.app/Contents/MacOS/nexus-desktop"));
        assert_eq!(find("macos", &with_home), Some(tauri_mac.clone()));
        let nexus_mac = place(home.join("Applications/Nexus.app/Contents/MacOS/nexus-desktop-gpui"));
        assert_eq!(find("macos", &with_home), Some(nexus_mac.clone()));
        std::fs::remove_file(&nexus_mac).expect("remove the Nexus.app binary");

        // Linux: the installed folder in the data folder (XDG_DATA_HOME when set), else the link in ~/.local/bin.
        let gpui_link = place(home.join(".local/bin/nexus-desktop-gpui"));
        assert_eq!(find("linux", &with_home), Some(gpui_link));
        let gpui_linux = place(home.join(".local/share/nexus-desktop-gpui/nexus-desktop-gpui"));
        assert_eq!(find("linux", &with_home), Some(gpui_linux));
        let xdg = root.join("xdg");
        let gpui_xdg = place(xdg.join("nexus-desktop-gpui/nexus-desktop-gpui"));
        let with_xdg = env(vec![
            ("HOME", home.display().to_string()),
            ("XDG_DATA_HOME", xdg.display().to_string()),
        ]);
        assert_eq!(find("linux", &with_xdg), Some(gpui_xdg));

        // Windows: the per-user installer's folder.
        let local = root.join("AppData/Local");
        let with_local = env(vec![("LOCALAPPDATA", local.display().to_string())]);
        assert_eq!(find("windows", &with_local), None);
        let gpui_windows = place(local.join("Programs/Nexus GPUI/nexus-desktop-gpui.exe"));
        assert_eq!(find("windows", &with_local), Some(gpui_windows));
        let nexus_windows = place(local.join("Programs/Nexus/nexus-desktop-gpui.exe"));
        assert_eq!(find("windows", &with_local), Some(nexus_windows));

        // NEXUS_DESKTOP_BIN wins over every install; an empty or missing one is skipped.
        let own = place(root.join("build/nexus-desktop-gpui"));
        let with_override = env(vec![
            ("HOME", home.display().to_string()),
            ("NEXUS_DESKTOP_BIN", own.display().to_string()),
        ]);
        assert_eq!(find("macos", &with_override), Some(own.clone()));
        assert_eq!(find("linux", &with_override), Some(own));
        let with_missing = env(vec![
            ("HOME", home.display().to_string()),
            ("NEXUS_DESKTOP_BIN", root.join("gone").display().to_string()),
        ]);
        assert_eq!(
            find("macos", &with_missing),
            Some(root.join("Applications/Nexus.app/Contents/MacOS/nexus-desktop"))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_recording_without_narration_leaves_the_talking_out_of_the_prompt() {
        let recording = Recording {
            id: "2026-10-19T14-03-22".into(),
            path: PathBuf::from("/tmp/rec/2026-10-19T14-03-22"),
            created_at: None,
            duration: 42.4,
            frames: 3,
            narrated: false,
            said: None,
        };
        // The TypeScript CLI's test (interfaces/src/cli.test.ts) holds the same text.
        assert_eq!(
            prompt(&recording),
            [
                "I recorded my screen to show you something (Nexus for Mac, 0:42). It is in /tmp/rec/2026-10-19T14-03-22",
                "",
                "Read summary.md there first: a timeline with one section per keyframe, with the red ink and numbered pins I drew, my clicks (places are pixels in that keyframe) and the app in front with a browser tab's address. Then open contact-sheet.png (every keyframe, numbered with its time) and the frames/NN.png it points at. capture.json has the same as data.",
                "",
                "The red ink and the pins are mine: they point at what I mean. For a closer look at a moment between keyframes: `nexus recordings frames 2026-10-19T14-03-22 --from 0:10 --to 0:14` (times as in summary.md).",
                "",
                "Before you change anything, tell me in a sentence or two what you understood: what I showed, what is wrong or wanted, and where.",
            ]
            .join("\n")
        );
    }
}
