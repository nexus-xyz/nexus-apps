//! Installed editor discovery and folder/file launch rules shared by desktop shells.

use crate::{
    open::{self, Spot},
    process,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Debug)]
enum How {
    /// The folder as the only argument.
    Arg,
    /// `flag` then the folder, as two arguments.
    Flag(&'static str),
    /// One argument, `prefix` then the folder (`--working-directory=/x`).
    Joined(&'static str),
    /// No argument: the app starts in the folder (a terminal started from `current_dir`).
    Cwd,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Editor,
    Terminal,
    /// An editor that runs inside a terminal (Vim, Neovim): found on `PATH` everywhere, opened in a terminal window.
    Console,
}

struct Entry {
    id: &'static str,
    name: &'static str,
    kind: Kind,
    /// The macOS app (`/Applications/<app>.app`); empty: none there.
    app: &'static str,
    /// The commands elsewhere, first found wins. Empty: none there.
    commands: &'static [&'static str],
    how: How,
}

const fn editor(id: &'static str, name: &'static str, app: &'static str, commands: &'static [&'static str]) -> Entry {
    Entry {
        id,
        name,
        kind: Kind::Editor,
        app,
        commands,
        how: How::Arg,
    }
}

const fn console(id: &'static str, name: &'static str, commands: &'static [&'static str]) -> Entry {
    Entry {
        id,
        name,
        kind: Kind::Console,
        app: "",
        commands,
        how: How::Arg,
    }
}

const fn terminal(
    id: &'static str,
    name: &'static str,
    app: &'static str,
    commands: &'static [&'static str],
    how: How,
) -> Entry {
    Entry {
        id,
        name,
        kind: Kind::Terminal,
        app,
        commands,
        how,
    }
}

/// What a session's folder opens in, in the order shown (the first installed editor is the default until the user
/// picks one). The file manager is always there (`files`). Ids are what the window remembers, so never rename one.
/// The editors are T3 Code's list (its `editor.ts`) minus the ones we couldn't find a stable launch for.
const EDITORS: &[Entry] = &[
    editor("cursor", "Cursor", "Cursor", &["cursor"]),
    editor("vscode", "VS Code", "Visual Studio Code", &["code"]),
    editor(
        "vscode-insiders",
        "VS Code Insiders",
        "Visual Studio Code - Insiders",
        &["code-insiders"],
    ),
    editor("vscodium", "VSCodium", "VSCodium", &["codium"]),
    editor("windsurf", "Windsurf", "Windsurf", &["windsurf"]),
    editor("zed", "Zed", "Zed", &["zed", "zeditor"]),
    editor("idea", "IntelliJ IDEA", "IntelliJ IDEA", &["idea"]),
    editor("webstorm", "WebStorm", "WebStorm", &["webstorm"]),
    editor("pycharm", "PyCharm", "PyCharm", &["pycharm"]),
    editor("goland", "GoLand", "GoLand", &["goland"]),
    editor("rustrover", "RustRover", "RustRover", &["rustrover"]),
    editor("clion", "CLion", "CLion", &["clion"]),
    editor("phpstorm", "PhpStorm", "PhpStorm", &["phpstorm"]),
    editor("rubymine", "RubyMine", "RubyMine", &["rubymine"]),
    editor("rider", "Rider", "Rider", &["rider"]),
    editor("datagrip", "DataGrip", "DataGrip", &["datagrip"]),
    editor("sublime", "Sublime Text", "Sublime Text", &["subl"]),
    console("nvim", "Neovim", &["nvim"]),
    console("vim", "Vim", &["vim"]),
    terminal("terminal-app", "Terminal", "Utilities/Terminal", &[], How::Arg),
    terminal("iterm", "iTerm", "iTerm", &[], How::Arg),
    terminal(
        "ghostty",
        "Ghostty",
        "Ghostty",
        &["ghostty"],
        How::Joined("--working-directory="),
    ),
    terminal("x-terminal", "Terminal", "", &["x-terminal-emulator"], How::Cwd),
    terminal(
        "gnome-terminal",
        "GNOME Terminal",
        "",
        &["gnome-terminal"],
        How::Joined("--working-directory="),
    ),
    terminal("konsole", "Konsole", "", &["konsole"], How::Flag("--workdir")),
    terminal("wt", "Windows Terminal", "", &["wt"], How::Flag("-d")),
    terminal("cmd", "Command Prompt", "", &["cmd"], How::Cwd),
];

/// Where JetBrains' Toolbox app puts the launch scripts it generates (`idea`, `webstorm`, ...), which are on no `PATH`.
/// Not on macOS: there an entry is found by its app bundle (`entry_at`).
fn toolbox_scripts() -> Option<PathBuf> {
    let home = || {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("JetBrains/Toolbox/scripts"))
    } else {
        home().map(|home| home.join(".local/share/JetBrains/Toolbox/scripts"))
    }
}

/// The bundle names an app can have on macOS: its own, then per edition, as JetBrains names them
/// (`IntelliJ IDEA CE.app`, `PyCharm Professional Edition.app`, Toolbox's `IntelliJ IDEA Ultimate.app`).
fn app_names(app: &str) -> impl Iterator<Item = String> + '_ {
    const EDITIONS: [&str; 7] = [
        "",
        " CE",
        " Ultimate",
        " Community",
        " Community Edition",
        " Professional",
        " Professional Edition",
    ];
    EDITIONS.into_iter().map(move |edition| format!("{app}{edition}"))
}

/// Where an entry is: its app on macOS, its command on the agents' `PATH` (and Toolbox's scripts) elsewhere.
fn entry_at(entry: &Entry, path: &str) -> Option<PathBuf> {
    if cfg!(target_os = "macos") && entry.kind != Kind::Console {
        if entry.app.is_empty() {
            return None;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let system = PathBuf::from("/System/Applications");
        let dirs = [PathBuf::from("/Applications"), home.join("Applications"), system];
        return app_names(entry.app)
            .flat_map(|name| dirs.iter().map(move |dir| dir.join(format!("{name}.app"))))
            .find(|path| path.exists());
    }
    let mut dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
    dirs.extend(toolbox_scripts());
    for command in entry.commands {
        let names: Vec<String> = if cfg!(windows) {
            vec![format!("{command}.cmd"), format!("{command}.exe")]
        } else {
            vec![command.to_string()]
        };
        let found = dirs
            .iter()
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .find(|path| path.is_file());
        if found.is_some() {
            return found;
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Editor {
    pub id: &'static str,
    pub name: &'static str,
    /// `editor`, `terminal` or `files`.
    pub kind: &'static str,
}

/// The command that opens `dir` in `entry`, found at `at`.
fn launch(entry: &Entry, at: &Path, dir: &Path, path: &str) -> Result<std::process::Command, String> {
    if entry.kind == Kind::Console {
        return in_terminal(at, vec![".".into()], dir, path);
    }
    if cfg!(target_os = "macos") {
        let mut open = process::command("open");
        open.arg("-a").arg(at).arg(dir);
        return Ok(open);
    }
    let mut command = process::command(at);
    if entry.id == "cmd" {
        // This `cmd` has no window (`process::command`): `start` gives the prompt a window of its own.
        command.args(["/c", "start", "", "cmd"]);
    }
    match entry.how {
        How::Arg => command.arg(dir),
        How::Flag(flag) => command.arg(flag).arg(dir),
        How::Joined(prefix) => {
            let mut joined = std::ffi::OsString::from(prefix);
            joined.push(dir);
            command.arg(joined)
        }
        How::Cwd => command.current_dir(dir),
    };
    Ok(command)
}

/// The command that runs `program` with `args` in a new terminal window, in `cwd`: Terminal on macOS, a console of
/// its own on Windows, the first of the system's terminal, GNOME Terminal and Konsole elsewhere.
fn in_terminal(
    program: &Path,
    args: Vec<std::ffi::OsString>,
    cwd: &Path,
    path: &str,
) -> Result<std::process::Command, String> {
    if cfg!(target_os = "macos") {
        let words: Vec<String> = std::iter::once(program.as_os_str())
            .chain(args.iter().map(|arg| arg.as_os_str()))
            .map(|word| open::shell_quote(&word.to_string_lossy()))
            .collect();
        let line = format!(
            "cd {} && {}",
            open::shell_quote(&cwd.to_string_lossy()),
            words.join(" ")
        );
        let mut command = process::command("osascript");
        command
            .arg("-e")
            .arg(format!(
                "tell application \"Terminal\" to do script {}",
                open::applescript_string(&line)
            ))
            .arg("-e")
            .arg("tell application \"Terminal\" to activate");
        return Ok(command);
    }
    // On Windows the program itself, in a console of its own. Going through `cmd /c start` would make cmd.exe parse
    // the file path, so a name like `x&calc.exe` could run something.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x10;
        // A console window is the point here, so not `process::command` (clippy.toml).
        #[allow(clippy::disallowed_methods)]
        let mut command = std::process::Command::new(program);
        command.args(args).current_dir(cwd).creation_flags(CREATE_NEW_CONSOLE);
        return Ok(command);
    }
    #[cfg(not(windows))]
    {
        let (at, flag) = [("x-terminal", "-e"), ("gnome-terminal", "--"), ("konsole", "-e")]
            .into_iter()
            .find_map(|(id, flag)| {
                let entry = EDITORS.iter().find(|entry| entry.id == id)?;
                Some((entry_at(entry, path)?, flag))
            })
            .ok_or_else(|| format!("No terminal found to run {} in.", program.display()))?;
        let mut command = process::command(at);
        command.arg(flag).arg(program).args(args).current_dir(cwd);
        Ok(command)
    }
}

/// The command that opens `path` in the system's default app for it (a folder: the file manager).
fn system_open(path: &Path) -> std::process::Command {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let mut command = process::command(opener);
    command.arg(path);
    command
}

/// The command that opens `file` at `spot` in `entry` (found at `at`), which takes lines as `goto`. On macOS through
/// the command-line tool in the app's bundle, since `open -a` can't pass a line; without one, at the file's top.
fn open_at(
    entry: &Entry,
    goto: open::Goto,
    at: &Path,
    file: &Path,
    spot: Option<Spot>,
    cwd: &Path,
    path: &str,
) -> Result<std::process::Command, String> {
    let args = open::goto_args(goto, file, spot);
    if entry.kind == Kind::Console {
        return in_terminal(at, args, cwd, path);
    }
    if cfg!(target_os = "macos") {
        // The bundle's tool is usually named like the shell command. VS Code Insiders may ship it as plain `code`
        // (`code-insiders` being a symlink to it), so that is tried too.
        let extra: &[&str] = if entry.id == "vscode-insiders" { &["code"] } else { &[] };
        let cli = entry
            .commands
            .iter()
            .chain(extra)
            .filter_map(|command| open::mac_cli(entry.id, command))
            .map(|inside| at.join(inside))
            .find(|cli| cli.is_file());
        let Some(cli) = cli else {
            let mut command = process::command("open");
            command.arg("-a").arg(at).arg(file);
            return Ok(command);
        };
        let mut command = process::command(cli);
        command.args(args);
        return Ok(command);
    }
    let mut command = process::command(at);
    command.args(args).current_dir(cwd);
    Ok(command)
}

/// Starts `command` without waiting and without leaving a zombie: nothing else collects an exited child's status, so a
/// thread waits on it (the launcher usually exits at once; an editor started without a helper keeps it a while).
fn detach(command: std::process::Command, what: &str) -> Result<(), String> {
    spawn_detached(command, what).map(|_| ())
}

/// `detach`, giving back the child's pid.
fn spawn_detached(command: std::process::Command, what: &str) -> Result<u32, String> {
    spawn_detached_guarded(command, what, &|| true)
}
fn spawn_detached_guarded(
    mut command: std::process::Command,
    what: &str,
    valid: &(dyn Fn() -> bool + Send + Sync),
) -> Result<u32, String> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so a Ctrl+C or closing the app doesn't take the editor along.
        command.process_group(0);
    }
    if !valid() {
        return Err("This file view is no longer active.".into());
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not open {what}: {error}"))?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

pub fn installed(path: &str) -> Vec<Editor> {
    let files = if cfg!(target_os = "macos") {
        "Finder"
    } else if cfg!(windows) {
        "Explorer"
    } else {
        "Files"
    };
    let mut found: Vec<Editor> = EDITORS
        .iter()
        .filter(|entry| entry_at(entry, path).is_some())
        .map(|entry| Editor {
            id: entry.id,
            name: entry.name,
            kind: match entry.kind {
                Kind::Editor => "editor",
                Kind::Terminal => "terminal",
                Kind::Console => "console",
            },
        })
        .collect();
    found.push(Editor {
        id: "files",
        name: files,
        kind: "files",
    });
    found
}

pub fn open_folder(dir: &Path, editor: &str, path: &str) -> Result<(), String> {
    let command = if editor == "files" {
        system_open(dir)
    } else {
        let entry = EDITORS
            .iter()
            .find(|entry| entry.id == editor)
            .ok_or_else(|| format!("Unknown editor {editor}."))?;
        let at = entry_at(entry, path).ok_or_else(|| format!("{} isn't installed.", entry.name))?;
        launch(entry, &at, dir, path)?
    };
    detach(command, &dir.display().to_string())
}

pub fn open_file(
    root: &Path,
    path: &str,
    line: Option<u32>,
    col: Option<u32>,
    editor: Option<&str>,
    search_path: &str,
) -> Result<(), String> {
    open_file_guarded(root, path, line, col, editor, search_path, &|| true)
}

/// Recheck a local presentation's frozen authority at the actual process dispatch boundary.
/// Ordinary callers keep the same launcher and path/argument validation through `open_file`.
pub fn open_file_guarded(
    root: &Path,
    path: &str,
    line: Option<u32>,
    col: Option<u32>,
    editor: Option<&str>,
    search_path: &str,
    valid: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(), String> {
    launch_file(
        FileRequest {
            root,
            path,
            line,
            col,
            editor,
            search_path,
        },
        valid,
        |command, what| spawn_detached_guarded(command, what, valid).map(|_| ()),
    )
}
struct FileRequest<'a> {
    root: &'a Path,
    path: &'a str,
    line: Option<u32>,
    col: Option<u32>,
    editor: Option<&'a str>,
    search_path: &'a str,
}
fn launch_file(
    request: FileRequest<'_>,
    valid: &(dyn Fn() -> bool + Send + Sync),
    launch: impl FnOnce(std::process::Command, &str) -> Result<(), String>,
) -> Result<(), String> {
    if !valid() {
        return Err("This file view is no longer active.".into());
    }
    let FileRequest {
        root,
        path,
        line,
        col,
        editor,
        search_path,
    } = request;
    let file = open::file_inside(root, path)?;
    let spot = line.filter(|line| *line > 0).map(|line| Spot {
        line,
        col: col.filter(|col| *col > 0),
    });
    let entry = editor.and_then(|id| EDITORS.iter().find(|entry| entry.id == id));
    let found = entry.and_then(|entry| Some((entry, open::goto_for(entry.id)?, entry_at(entry, search_path)?)));
    let command = match found {
        Some((entry, goto, at)) => open_at(entry, goto, &at, &file, spot, root, search_path)?,
        None => system_open(&file),
    };
    // Discovery and filesystem validation can take time. There is no await between this check and spawn.
    if !valid() {
        return Err("This file view is no longer active.".into());
    }
    launch(command, &file.display().to_string())
}

/// Prefer the saved choice, then an editor, terminal, or console. Files is only an explicit choice.
pub fn preferred<'a>(installed: &'a [Editor], saved: Option<&str>) -> Option<&'a Editor> {
    installed
        .iter()
        .find(|entry| Some(entry.id) == saved && entry.kind != "files")
        .or_else(|| installed.iter().find(|entry| entry.kind == "editor"))
        .or_else(|| installed.iter().find(|entry| entry.kind == "terminal"))
        .or_else(|| installed.iter().find(|entry| entry.kind != "files"))
}

pub fn open_default(path: &Path) -> Result<(), String> {
    detach(system_open(path), &path.display().to_string())
}

#[cfg(test)]
mod open_in_tests {
    use super::*;

    #[test]
    fn a_saved_choice_and_fallbacks_match_the_desktop_picker() {
        let files = Editor {
            id: "files",
            name: "Files",
            kind: "files",
        };
        let terminal = Editor {
            id: "terminal",
            name: "Terminal",
            kind: "terminal",
        };
        let editor = Editor {
            id: "code",
            name: "Code",
            kind: "editor",
        };
        let found = [files.clone(), terminal.clone(), editor.clone()];
        assert_eq!(preferred(&found, Some("terminal")), Some(&terminal));
        assert_eq!(preferred(&found, Some("files")), Some(&editor));
        assert_eq!(preferred(&found, Some("uninstalled")), Some(&editor));
        assert_eq!(preferred(&[files.clone(), terminal.clone()], None), Some(&terminal));
        assert_eq!(preferred(&[files], None), None);
    }

    #[test]
    fn ids_are_unique() {
        let mut ids: Vec<_> = EDITORS.iter().map(|entry| entry.id).collect();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn launch_builds_each_style() {
        let dir = Path::new("/work/app");
        let at = Path::new("/usr/bin/tool");
        let build = |id: &str| {
            let entry = EDITORS.iter().find(|entry| entry.id == id).expect("a known editor id");
            launch(entry, at, dir, "").unwrap_or_else(|e| panic!("{e}"))
        };
        let args = |command: &std::process::Command| -> Vec<String> {
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(args(&build("zed")), ["/work/app"]);
        assert_eq!(args(&build("konsole")), ["--workdir", "/work/app"]);
        assert_eq!(args(&build("gnome-terminal")), ["--working-directory=/work/app"]);
        let plain = build("x-terminal");
        assert!(args(&plain).is_empty());
        assert_eq!(plain.get_current_dir(), Some(dir));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn open_at_passes_the_line_each_editor_takes() {
        let args = |id: &str| -> Vec<String> {
            let entry = EDITORS
                .iter()
                .find(|entry| entry.id == id)
                .unwrap_or_else(|| panic!("no editor {id}"));
            let goto = open::goto_for(id).unwrap_or_else(|| panic!("no goto for {id}"));
            let spot = Some(Spot { line: 7, col: Some(2) });
            let command = open_at(
                entry,
                goto,
                Path::new("/usr/bin/tool"),
                Path::new("/w/a.rs"),
                spot,
                Path::new("/w"),
                "",
            )
            .unwrap_or_else(|e| panic!("{e}"));
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(args("windsurf"), ["-g", "/w/a.rs:7:2"]);
        assert_eq!(args("zed"), ["/w/a.rs:7:2"]);
        assert_eq!(args("goland"), ["--line", "7", "--column", "2", "/w/a.rs"]);
        assert_eq!(args("sublime"), ["/w/a.rs:7:2"]);
    }

    /// B7: a launcher that exits must not stay a zombie of ours. Watches only the child it started, so other tests'
    /// children in the same binary can't make it flaky.
    #[cfg(target_os = "linux")]
    #[test]
    fn detach_reaps_the_child() {
        use std::time::{Duration, Instant};
        let pid = spawn_detached(process::command("true"), "true").unwrap_or_else(|e| panic!("{e}"));
        // Reaped: gone from /proc. Not reaped: there (as a zombie) until someone collects it.
        let proc = PathBuf::from(format!("/proc/{pid}"));
        let start = Instant::now();
        while proc.exists() && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!proc.exists(), "pid {pid} was never reaped");
    }

    #[test]
    fn looks_for_each_jetbrains_edition_bundle() {
        let names: Vec<String> = app_names("PyCharm").collect();
        assert_eq!(names[0], "PyCharm");
        assert!(names.contains(&"PyCharm CE".to_string()));
        assert!(names.contains(&"PyCharm Professional Edition".to_string()));
        assert!(
            !names.iter().any(|name| name.contains(" - ")),
            "never a prefix match: {names:?}"
        );
    }
}

#[cfg(test)]
mod guarded_file_tests;
