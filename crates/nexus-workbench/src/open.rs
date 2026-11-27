//! Opening one file of a session in the person's editor, at a line: which files may open (only those inside the
//! session's folder) and the arguments each editor takes for a line and column. Nothing is started here; the app
//! builds the process from these.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Where in a file to open, 1-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spot {
    pub line: u32,
    pub col: Option<u32>,
}

/// How an editor's command takes a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goto {
    /// `-g path:line:col`: VS Code and its forks.
    Flag,
    /// `path:line:col`: Zed, Sublime Text.
    Suffix,
    /// `--line N --column C path`: JetBrains IDEs.
    Line,
    /// `+N path`: Vim and Neovim (no column).
    Plus,
}

/// How the editor with this id (`EDITORS` in the app) takes a line; `None`: it doesn't (a terminal, the file manager).
pub fn goto_for(id: &str) -> Option<Goto> {
    match id {
        "cursor" | "vscode" | "vscode-insiders" | "vscodium" | "windsurf" => Some(Goto::Flag),
        "zed" | "sublime" => Some(Goto::Suffix),
        "idea" | "webstorm" | "pycharm" | "goland" | "rustrover" | "clion" | "phpstorm" | "rubymine" | "rider"
        | "datagrip" => Some(Goto::Line),
        "vim" | "nvim" => Some(Goto::Plus),
        _ => None,
    }
}

/// The command-line tool inside the editor's macOS app bundle (`command`: its usual name), relative to the bundle.
/// On macOS the app is found by its bundle, and `open -a` can't pass a line, so the line goes through this tool.
pub fn mac_cli(id: &str, command: &str) -> Option<String> {
    match goto_for(id)? {
        Goto::Flag => Some(format!("Contents/Resources/app/bin/{command}")),
        Goto::Suffix if id == "zed" => Some("Contents/MacOS/cli".into()),
        Goto::Suffix => Some("Contents/SharedSupport/bin/subl".into()),
        Goto::Line => Some(format!("Contents/MacOS/{command}")),
        Goto::Plus => None,
    }
}

/// The arguments that open `file` at `spot` (or at its top, `None`) in an editor taking lines as `goto`.
pub fn goto_args(goto: Goto, file: &Path, spot: Option<Spot>) -> Vec<OsString> {
    let Some(Spot { line, col }) = spot else {
        return vec![file.into()];
    };
    let at = || {
        let mut at = OsString::from(file);
        at.push(format!(":{line}"));
        if let Some(col) = col {
            at.push(format!(":{col}"));
        }
        at
    };
    match goto {
        Goto::Flag => vec!["-g".into(), at()],
        Goto::Suffix => vec![at()],
        Goto::Line => {
            let mut args: Vec<OsString> = vec!["--line".into(), line.to_string().into()];
            if let Some(col) = col {
                args.extend(["--column".into(), col.to_string().into()]);
            }
            args.push(file.into());
            args
        }
        Goto::Plus => vec![format!("+{line}").into(), file.into()],
    }
}

/// `path` (absolute, or relative to `root`) as a file inside `root`, with links followed; an error for a path with
/// `..`, one that doesn't exist, or one that ends up outside (a link out of the folder included).
pub fn file_inside(root: &Path, path: &str) -> Result<PathBuf, String> {
    let given = Path::new(path);
    if path.is_empty() || given.components().any(|part| part == Component::ParentDir) {
        return Err(format!(
            "Can't open {path}: only files inside the session's folder open."
        ));
    }
    let joined = if given.is_absolute() {
        given.to_path_buf()
    } else {
        root.join(given)
    };
    let file = joined
        .canonicalize()
        .map_err(|error| format!("Can't open {path}: {error}"))?;
    let root = root
        .canonicalize()
        .map_err(|error| format!("The session's folder can't be read: {error}"))?;
    if !file.starts_with(&root) {
        return Err(format!(
            "Can't open {path}: only files inside the session's folder open."
        ));
    }
    Ok(plain(file))
}

/// Windows' `\\?\C:\x` (what `canonicalize` gives) as `C:\x`, which editors take; any other path as it is.
fn plain(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

/// `arg` quoted for a POSIX shell: in single quotes, a single quote inside as `'\''`.
pub fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// `text` as an AppleScript string literal.
pub fn applescript_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', r"\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn each_editor_family_takes_the_line_its_own_way() {
        let file = Path::new("/w/src/main.rs");
        let spot = Some(Spot { line: 12, col: Some(5) });
        let line_only = Some(Spot { line: 12, col: None });
        assert_eq!(
            strings(goto_args(Goto::Flag, file, spot)),
            ["-g", "/w/src/main.rs:12:5"]
        );
        assert_eq!(
            strings(goto_args(Goto::Flag, file, line_only)),
            ["-g", "/w/src/main.rs:12"]
        );
        assert_eq!(strings(goto_args(Goto::Suffix, file, spot)), ["/w/src/main.rs:12:5"]);
        assert_eq!(
            strings(goto_args(Goto::Line, file, spot)),
            ["--line", "12", "--column", "5", "/w/src/main.rs"]
        );
        assert_eq!(
            strings(goto_args(Goto::Line, file, line_only)),
            ["--line", "12", "/w/src/main.rs"]
        );
        assert_eq!(strings(goto_args(Goto::Plus, file, spot)), ["+12", "/w/src/main.rs"]);
        for goto in [Goto::Flag, Goto::Suffix, Goto::Line, Goto::Plus] {
            assert_eq!(
                strings(goto_args(goto, file, None)),
                ["/w/src/main.rs"],
                "{goto:?} without a line"
            );
        }
    }

    #[test]
    fn editors_map_to_their_family_and_mac_tool() {
        assert_eq!(goto_for("cursor"), Some(Goto::Flag));
        assert_eq!(goto_for("windsurf"), Some(Goto::Flag));
        assert_eq!(goto_for("zed"), Some(Goto::Suffix));
        assert_eq!(goto_for("sublime"), Some(Goto::Suffix));
        assert_eq!(goto_for("rustrover"), Some(Goto::Line));
        assert_eq!(goto_for("nvim"), Some(Goto::Plus));
        assert_eq!(goto_for("ghostty"), None);
        assert_eq!(goto_for("files"), None);
        assert_eq!(
            mac_cli("vscode", "code").as_deref(),
            Some("Contents/Resources/app/bin/code")
        );
        assert_eq!(mac_cli("zed", "zed").as_deref(), Some("Contents/MacOS/cli"));
        assert_eq!(
            mac_cli("sublime", "subl").as_deref(),
            Some("Contents/SharedSupport/bin/subl")
        );
        assert_eq!(mac_cli("idea", "idea").as_deref(), Some("Contents/MacOS/idea"));
        assert_eq!(mac_cli("nvim", "nvim"), None);
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-open-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("root/src")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.join("root/src/a.rs"), "").unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(dir.join("secret"), "").unwrap_or_else(|e| panic!("{e}"));
        dir
    }

    #[test]
    fn only_files_inside_the_folder_open() {
        let dir = folder("inside");
        let root = dir.join("root");
        let real = root.canonicalize().unwrap_or_else(|e| panic!("{e}")).join("src/a.rs");
        assert_eq!(file_inside(&root, "src/a.rs"), Ok(real.clone()));
        assert_eq!(file_inside(&root, &root.join("src/a.rs").to_string_lossy()), Ok(real));
        assert!(file_inside(&root, "../secret").is_err(), "`..` is refused");
        assert!(
            file_inside(&root, "src/../../secret").is_err(),
            "`..` anywhere is refused"
        );
        assert!(
            file_inside(&root, &dir.join("secret").to_string_lossy()).is_err(),
            "an absolute path outside"
        );
        assert!(file_inside(&root, "src/missing.rs").is_err(), "a file that isn't there");
        assert!(file_inside(&root, "").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_folder_is_refused() {
        let dir = folder("link");
        let root = dir.join("root");
        std::os::unix::fs::symlink(dir.join("secret"), root.join("out")).unwrap_or_else(|e| panic!("{e}"));
        assert!(file_inside(&root, "out").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn windows_verbatim_prefix_is_dropped() {
        assert_eq!(plain(PathBuf::from(r"\\?\C:\w\a.rs")), PathBuf::from(r"C:\w\a.rs"));
        assert_eq!(
            plain(PathBuf::from(r"\\?\UNC\srv\a.rs")),
            PathBuf::from(r"\\?\UNC\srv\a.rs")
        );
        assert_eq!(plain(PathBuf::from("/w/a.rs")), PathBuf::from("/w/a.rs"));
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(applescript_string(r#"say "hi" \o/"#), r#""say \"hi\" \\o/""#);
    }
}
