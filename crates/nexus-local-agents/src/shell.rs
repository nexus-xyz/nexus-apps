//! What a shell says about an agent: its binary, from `NEXUS_<AGENT>_BIN` or the folders it is installed in, its
//! profile folder, and the variable that sets its route. The launch ([`find_agent`]) and the readiness check
//! (`find_in`, `detect_in`) use the one [`find_entry`]. Given the same environment, they find the same binary.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nexus_agents::{Agent, AgentEntry};

/// `NEXUS_<AGENT>_BIN` names a file that does not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingBinary {
    pub variable: String,
    pub path: PathBuf,
}

/// `NEXUS_<ID>_<KIND>` for `entry`, keyed by its id.
fn entry_variable(entry: &AgentEntry, kind: &str) -> String {
    format!("NEXUS_{}_{kind}", entry.identity.id.to_uppercase())
}

/// The variable that names an agent's binary: `NEXUS_CLAUDE_BIN`, `NEXUS_ANTIGRAVITY_BIN`.
pub fn bin_variable(agent: Agent) -> String {
    entry_variable(agent.entry(), "BIN")
}

/// The variable that sets an agent's route for a shell: `NEXUS_CLAUDE_AUTH`, `NEXUS_CODEX_AUTH`.
pub fn route_variable(agent: Agent) -> String {
    entry_variable(agent.entry(), "AUTH")
}

/// The agent's profile folder: its `profile.var` when set, else its `profile.dir` under the home folder (`HOME`).
/// `None` without either.
pub fn profile_dir(agent: Agent, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let entry = agent.entry();
    entry
        .profile
        .var
        .and_then(env)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| profile_under(agent, Path::new(&home)))
        })
}

/// The agent's `profile.dir` under the home folder `home`.
pub fn profile_under(agent: Agent, home: &Path) -> PathBuf {
    home.join(agent.entry().profile.dir)
}

/// The binary a launch runs: `NEXUS_<AGENT>_BIN` if set (a relative one under `cwd`), else the first of the agent's
/// `executable` on `PATH` in `env`, then in the common install folders ([`search_path`](crate::search_path)). The
/// readiness check searches the same folders.
pub fn find_agent(
    agent: Agent,
    env: &HashMap<String, String>,
    windows: bool,
    cwd: &Path,
) -> Result<Option<PathBuf>, MissingBinary> {
    let lookup = |name: &str| env.get(name).cloned();
    let dirs = crate::search_path(&lookup);
    find_entry(agent.entry(), &dirs, &lookup, windows, cwd)
}

/// The one lookup. `NEXUS_<AGENT>_BIN` (keyed by `entry`'s id) wins when set; a relative one resolves under `cwd`.
/// When it names no file, the lookup returns [`MissingBinary`] and tries no other binary. Without it, the lookup
/// returns the first of `entry`'s `executable` in `dirs`, with each `PATHEXT` extension on Windows.
pub(crate) fn find_entry(
    entry: &AgentEntry,
    dirs: &[PathBuf],
    env: &dyn Fn(&str) -> Option<String>,
    windows: bool,
    cwd: &Path,
) -> Result<Option<PathBuf>, MissingBinary> {
    let variable = entry_variable(entry, "BIN");
    if let Some(explicit) = env(&variable).filter(|value| !value.is_empty()) {
        let path = if Path::new(&explicit).is_absolute() {
            PathBuf::from(explicit)
        } else {
            cwd.join(explicit)
        };
        if !path.is_file() {
            return Err(MissingBinary { variable, path });
        }
        return Ok(Some(path));
    }
    let extensions: Vec<String> = if windows {
        env("PATHEXT")
            .unwrap_or_else(|| ".EXE;.CMD;.BAT".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(String::from)
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in dirs.iter().filter(|dir| !dir.as_os_str().is_empty()) {
        for extension in &extensions {
            let candidate = dir.join(format!("{}{extension}", entry.identity.executable));
            if candidate.is_file() && (windows || is_executable(&candidate)) {
                return Ok(Some(candidate));
            }
        }
    }
    Ok(None)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(items: &[(&str, &str)]) -> HashMap<String, String> {
        items.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn finds_the_agent_on_path_or_from_its_variable() {
        let dir = std::env::temp_dir().join(format!("nexus-client-agents-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let bin = dir.join("claude");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap_or_default();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755));
        }
        let path = format!("/nonexistent:{}", dir.display());
        let found = find_agent(Agent::Claude, &map(&[("PATH", &path)]), false, Path::new("/"));
        assert_eq!(found, Ok(Some(bin.clone())));
        assert_eq!(
            find_agent(Agent::Codex, &map(&[("PATH", &path)]), false, Path::new("/")),
            Ok(None)
        );
        let missing = find_agent(Agent::Claude, &map(&[("NEXUS_CLAUDE_BIN", "nope")]), false, &dir);
        assert_eq!(
            missing,
            Err(MissingBinary {
                variable: "NEXUS_CLAUDE_BIN".into(),
                path: dir.join("nope"),
            })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_an_entry_by_its_executable_and_its_variable_by_its_id() {
        let dir = std::env::temp_dir().join(format!("nexus-client-executable-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap_or_default();
        // The id's name on PATH is not the agent: only the executable is.
        for name in ["claude", "agy"] {
            std::fs::write(dir.join(name), "#!/bin/sh\n").unwrap_or_default();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o755));
            }
        }
        let claude = Agent::Claude.entry();
        let entry = AgentEntry {
            identity: nexus_agents::Identity {
                executable: "agy",
                ..claude.identity
            },
            ..*claude
        };
        let dirs = [dir.clone()];
        assert_eq!(
            find_entry(&entry, &dirs, &|_| None, false, Path::new("/")),
            Ok(Some(dir.join("agy")))
        );
        let named = |name: &str| (name == "NEXUS_CLAUDE_BIN").then(|| "claude".to_string());
        assert_eq!(
            find_entry(&entry, &dirs, &named, false, &dir),
            Ok(Some(dir.join("claude")))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An agent only in an install folder (`~/.local/bin`), not on `PATH`: the readiness check and the launch both
    /// find it. A `NEXUS_CLAUDE_BIN` that names no file stops both: neither takes the installed binary instead.
    #[cfg(unix)]
    #[test]
    fn launch_finds_what_readiness_finds_in_an_install_folder() {
        let home = std::env::temp_dir().join(format!("nexus-install-folder-{}", uuid::Uuid::new_v4()));
        let (local_bin, empty) = (home.join(".local/bin"), home.join("empty"));
        std::fs::create_dir_all(&local_bin).expect("dir");
        std::fs::create_dir_all(&empty).expect("dir");
        let claude = local_bin.join("claude");
        std::fs::write(&claude, "#!/bin/sh\n").expect("write");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let mut env = map(&[
            ("PATH", &empty.display().to_string()),
            ("HOME", &home.display().to_string()),
        ]);
        let lookup = |env: &HashMap<String, String>| {
            let env = env.clone();
            move |name: &str| env.get(name).cloned()
        };
        // Only this test's folders: the common ones also include /usr/local/bin, which may hold a real agent.
        let dirs: Vec<PathBuf> = crate::search_path(&lookup(&env))
            .into_iter()
            .filter(|dir| dir.starts_with(&home))
            .collect();
        assert_eq!(
            crate::find_in(Agent::Claude, &dirs, &lookup(&env)),
            Some(claude.clone())
        );
        assert_eq!(
            find_agent(Agent::Claude, &env, false, Path::new("/")),
            Ok(Some(claude.clone()))
        );

        env.insert(bin_variable(Agent::Claude), "gone".into());
        assert_eq!(
            find_agent(Agent::Claude, &env, false, &home),
            Err(MissingBinary {
                variable: "NEXUS_CLAUDE_BIN".into(),
                path: home.join("gone"),
            })
        );
        assert_eq!(crate::find_in(Agent::Claude, &dirs, &lookup(&env)), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn variables_and_profile_folders_come_from_the_entry() {
        assert_eq!(bin_variable(Agent::Antigravity), "NEXUS_ANTIGRAVITY_BIN");
        assert_eq!(route_variable(Agent::Antigravity), "NEXUS_ANTIGRAVITY_AUTH");
        let env = |name: &str| match name {
            "HOME" => Some("/h".to_string()),
            "CODEX_HOME" => Some("/c".to_string()),
            _ => None,
        };
        assert_eq!(profile_dir(Agent::Claude, &env), Some(PathBuf::from("/h/.claude")));
        assert_eq!(profile_dir(Agent::Codex, &env), Some(PathBuf::from("/c")));
        assert_eq!(profile_dir(Agent::Antigravity, &|_| None), None);
    }
}
