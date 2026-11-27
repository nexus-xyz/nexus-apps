//! Linux terminal discovery. Commands and arguments stay separate; no environment value is evaluated by a shell.

use super::Opener;
use std::ffi::OsStr;
use std::path::Path;

const TERMINALS: &[&str] = &[
    "xdg-terminal-exec",
    "x-terminal-emulator",
    "kitty",
    "alacritty",
    "foot",
    "wezterm",
    "ghostty",
    "gnome-terminal",
    "kgx",
    "ptyxis",
    "konsole",
    "xfce4-terminal",
    "xterm",
];

fn candidates(script: &Path, terminal: Option<&str>) -> Vec<Opener> {
    let mut candidates = Vec::new();
    for program in terminal
        .into_iter()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .chain(TERMINALS.iter().copied())
    {
        if candidates.iter().any(|candidate: &Opener| candidate.program == program) {
            continue;
        }
        let name = Path::new(program)
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or(program);
        let flags: &[&str] = match name {
            "xdg-terminal-exec" => &[],
            "wezterm" => &["start", "--"],
            "gnome-terminal" | "kgx" | "ptyxis" => &["--"],
            "xfce4-terminal" => &["-x"],
            _ => &["-e"],
        };
        let mut args = flags.iter().map(|flag| (*flag).to_string()).collect::<Vec<_>>();
        args.push(script.display().to_string());
        candidates.push(Opener {
            program: program.into(),
            args,
            wait: false,
        });
    }
    candidates
}

fn executable(file: &Path) -> bool {
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn available(program: &str, path: Option<&OsStr>) -> bool {
    let program = Path::new(program);
    if program.components().count() > 1 {
        return executable(program);
    }
    path.is_some_and(|path| std::env::split_paths(path).any(|dir| executable(&dir.join(program))))
}

pub(super) fn open(
    script: &Path,
    terminal: Option<&str>,
    path: Option<&OsStr>,
    start: impl FnMut(&Opener) -> std::io::Result<bool>,
) -> std::io::Result<bool> {
    try_candidates(
        candidates(script, terminal)
            .into_iter()
            .filter(|candidate| available(&candidate.program, path)),
        start,
    )
}

fn try_candidates(
    candidates: impl IntoIterator<Item = Opener>,
    mut start: impl FnMut(&Opener) -> std::io::Result<bool>,
) -> std::io::Result<bool> {
    for candidate in candidates {
        if matches!(start(&candidate), Ok(true)) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_preference_is_first_and_each_terminal_receives_its_own_arguments() {
        let script = Path::new("/tmp/account with spaces.command");
        let candidates = candidates(script, Some("/opt/terminals/wezterm"));
        assert_eq!(candidates[0].program, "/opt/terminals/wezterm");
        assert_eq!(candidates[0].args, ["start", "--", "/tmp/account with spaces.command"]);
        assert_eq!(candidates[1].program, "xdg-terminal-exec");
        assert_eq!(candidates[1].args, ["/tmp/account with spaces.command"]);
        for (name, flags) in [
            ("kitty", vec!["-e"]),
            ("alacritty", vec!["-e"]),
            ("foot", vec!["-e"]),
            ("ghostty", vec!["-e"]),
            ("gnome-terminal", vec!["--"]),
            ("kgx", vec!["--"]),
            ("ptyxis", vec!["--"]),
            ("konsole", vec!["-e"]),
            ("xfce4-terminal", vec!["-x"]),
            ("xterm", vec!["-e"]),
        ] {
            let found = candidates
                .iter()
                .find(|candidate| candidate.program == name)
                .expect("known terminal");
            assert_eq!(&found.args[..found.args.len() - 1], flags);
            assert_eq!(
                found.args.last().map(String::as_str),
                Some("/tmp/account with spaces.command")
            );
        }
        let preferred = candidates.first().expect("preference");
        assert!(!preferred.wait);
    }

    #[test]
    fn a_missing_or_failed_preferred_terminal_does_not_prevent_fallback() {
        let all = candidates(Path::new("/tmp/login.command"), Some("kitty"));
        assert_eq!(all.iter().filter(|candidate| candidate.program == "kitty").count(), 1);
        let mut tried = Vec::new();
        let opened = try_candidates(
            all.into_iter()
                .filter(|candidate| ["kitty", "foot", "xterm"].contains(&candidate.program.as_str())),
            |candidate| {
                tried.push(candidate.program.clone());
                if candidate.program == "foot" {
                    Ok(true)
                } else {
                    Err(std::io::ErrorKind::NotFound.into())
                }
            },
        )
        .expect("fallback result");
        assert!(opened);
        assert_eq!(tried, ["kitty", "foot"]);
        assert!(!try_candidates([], |_| panic!("no terminal to start")).expect("no candidate"));
    }

    #[test]
    fn unavailable_preference_is_not_run_as_a_shell_command() {
        let mut calls = 0;
        assert!(
            !open(
                Path::new("/tmp/login.command"),
                Some("kitty; arbitrary-command"),
                None,
                |_| {
                    calls += 1;
                    Ok(true)
                }
            )
            .expect("no executable")
        );
        assert_eq!(calls, 0);
    }
}
