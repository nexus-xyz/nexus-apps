//! The agents and the home folder a test sees: stand-in `claude` and `codex` scripts and a home folder,
//! both under the test root (`paths::test_root`). In a test, [`super::agent_env`] gives this environment and
//! [`super::home`] this home, so no test starts the person's own agents or writes under their home folder.
//!
//! The `PATH` is the stand-ins' folder and the system's own folders, never the person's `PATH` or the folders where
//! agents are usually installed (`process::agent_path` adds `/opt/homebrew/bin` and `~/.local/bin`). The stand-ins
//! answer `--version` and fail anything else, so model discovery and plan checks end at once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// What a stand-in prints for `--version`.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) const VERSION: &str = "0.0.0 (desktop test stand-in)";

/// The agents the stand-ins replace. Off Unix there are none: the stand-ins are shell scripts.
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) const STAND_INS: [&str; 2] = ["claude", "codex"];

/// The system's own folders, after the stand-ins: `sh`, `git` and the other tools the tests' commands run.
const SYSTEM_PATH: &[&str] = if cfg!(windows) {
    &[r"C:\Windows\System32", r"C:\Windows"]
} else {
    &["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
};

/// Variables of the person's environment that would point a test at their own agents or their own folders.
const PERSON_ONLY: &[&str] = &[
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "NEXUS_INSTALL_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "ZDOTDIR",
    "BASH_ENV",
    "ENV",
];

/// This process's folder for the stand-ins and the home folder.
fn root() -> PathBuf {
    crate::paths::test_root().join("agents")
}

/// The test home folder: `HOME` for the agents, the terminals and the default worktree folders.
pub(crate) fn home() -> PathBuf {
    let home = root().join("home");
    let _ = std::fs::create_dir_all(&home);
    home
}

/// The folder of stand-in agents, written once per process.
pub(crate) fn bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let bin = root().join("bin");
        std::fs::create_dir_all(&bin).expect("the stand-in agents' folder");
        #[cfg(unix)]
        for name in STAND_INS {
            write_stand_in(&bin, name);
        }
        bin
    })
    .clone()
}

/// One stand-in agent: written beside its place, then moved there, so no test runs a half-written script.
#[cfg(unix)]
fn write_stand_in(bin: &std::path::Path, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        "#!/bin/sh\n# {name}, as the desktop app's tests see it: a version, and nothing else.\n\
         if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi\nexit 1\n"
    );
    let partial = bin.join(format!(".{name}.partial"));
    std::fs::write(&partial, script).expect("a stand-in agent");
    std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755)).expect("a runnable stand-in");
    std::fs::rename(&partial, bin.join(name)).expect("the stand-in in place");
}

/// The agents' environment in a test: this process's, with the stand-ins first on a `PATH` of the system's own
/// folders, `HOME` the test home, the shell history in it, and none of [`PERSON_ONLY`] or `NEXUS_*_BIN`.
pub(crate) fn env() -> HashMap<String, String> {
    let home = home().to_string_lossy().into_owned();
    let mut env: HashMap<String, String> = std::env::vars()
        .filter(|(name, _)| !PERSON_ONLY.contains(&name.as_str()))
        .filter(|(name, _)| !(name.starts_with("NEXUS_") && name.ends_with("_BIN")))
        .collect();
    let separator = if cfg!(windows) { ";" } else { ":" };
    let path = std::iter::once(bin().to_string_lossy().into_owned())
        .chain(SYSTEM_PATH.iter().map(|dir| dir.to_string()))
        .collect::<Vec<_>>()
        .join(separator);
    env.remove("Path");
    env.insert("PATH".into(), path);
    env.insert(
        "HISTFILE".into(),
        format!("{home}{}.sh_history", std::path::MAIN_SEPARATOR),
    );
    env.insert("USERPROFILE".into(), home.clone());
    env.insert("HOME".into(), home);
    env
}
