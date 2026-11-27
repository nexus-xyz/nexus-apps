//! Extra plan accounts, so one computer can hold more than one Claude or ChatGPT plan: each is a config folder of the
//! agent's own under the app's data folder (`<root>/<agent>/<id>`), handed to it as `CLAUDE_CONFIG_DIR` or `CODEX_HOME`.
//! The agent keeps each folder's login apart (Claude Code, on macOS, in a keychain item of its own; Codex in the
//! folder's `auth.json`). The person's settings, instructions, skills and plugins are links to the agent's own folder,
//! so the agent works the same on every account; Claude Code's conversations are shared too, so a session resumes on
//! any. The agent's own login (no variable set) is the default account and has no id here.
//!
//! A linked account is a profile the person made outside Nexus (`~/.claude-work`, or the folder a `CLAUDE_CONFIG_DIR`
//! names): `<root>/<agent>/<id>.link` holds its path, as written. Nexus hands the agent that path unchanged, since
//! Claude Code on macOS names its keychain item after it, and never writes into the folder. [`discover`] finds them,
//! so a computer with a work and a personal login lists both.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::event::{SessionAgent, SessionKind};
use crate::harness::harness;

pub type Result<T> = std::result::Result<T, String>;

/// Where new plan sessions of one agent start, as Settings chose it: an extra account (`account`), else the agent's
/// own login, and whether they move to the account with the most left near a limit (`rotate`). The app applies it to
/// the workbench that runs its sessions ([`crate::Workbench::apply_account_choices`]). While the background service
/// runs them, the sign-in carries the choices to it, and [`SERVICE_CALL`] carries each later change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub agent: SessionAgent,
    #[serde(default)]
    pub account: Option<String>,
    pub rotate: bool,
}

/// The service's call that takes the [`Choice`] list again, after Settings saved it.
pub const SERVICE_CALL: &str = "host_accounts";

/// The variable that hands an account's folder to the agent: a coding agent's catalog `profile.var`, and
/// `NEXUS_CONFIG_DIR` for Nexus's own agent. `None` for an agent no variable moves (agy): set nothing for it.
pub fn account_variable(agent: SessionAgent) -> Option<&'static str> {
    match agent.kind() {
        SessionKind::Coding(coding) => coding.entry().profile.var,
        SessionKind::Nexus => Some("NEXUS_CONFIG_DIR"),
    }
}

/// [`account_variable`], with `ANTIGRAVITY_ACCOUNT_DIR` for an agent that has none (agy, which does not read it).
/// Kept only for the previous desktop app's callers; it goes with that app.
pub fn variable(agent: SessionAgent) -> &'static str {
    account_variable(agent).unwrap_or("ANTIGRAVITY_ACCOUNT_DIR")
}

/// The agent's profile, using the catalog's supported override and the caller's home-folder fallback.
pub fn profile_dir(agent: SessionAgent, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    match agent.kind() {
        SessionKind::Coding(coding) => nexus_local_agents::profile_dir(coding, &|name| {
            if name == "HOME" {
                home.to_str().map(String::from)
            } else {
                env(name)
            }
        })
        .unwrap_or_else(|| nexus_local_agents::profile_under(coding, home)),
        SessionKind::Nexus => account_variable(agent)
            .and_then(env)
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".nexus")),
    }
}

pub(crate) fn plan(agent: SessionAgent) -> &'static str {
    agent.coding().map_or("Nexus", |coding| coding.entry().identity.plan)
}

/// The data folder of the Desktop app and its background service.
pub fn desktop_data_dir(home: Option<&Path>, env: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    let nonempty = |name| env(name).filter(|path| !path.as_os_str().is_empty());
    let base = if cfg!(target_os = "macos") {
        home.map(|home| home.join("Library/Application Support"))
    } else if cfg!(windows) {
        nonempty("APPDATA")
    } else {
        nonempty("XDG_DATA_HOME").or_else(|| home.map(|home| home.join(".local/share")))
    };
    base.map(|base| base.join("xyz.nexus.desktop"))
}

pub fn memory_profiles(
    root: Option<&Path>,
    agent: SessionAgent,
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<PathBuf> {
    let default = profile_dir(agent, home, env);
    let mut profiles = vec![default];
    if let Some(root) = root
        && crate::harness::harness(agent).accounts().is_some()
    {
        profiles.extend(list(root, agent).iter().filter_map(|id| dir(root, agent, id).ok()));
    }
    profiles
}

/// An id `add` makes: `a_` and the time it was made, in milliseconds, so the folders' names sort oldest first.
pub fn valid_id(id: &str) -> bool {
    id.strip_prefix("a_")
        .is_some_and(|ms| !ms.is_empty() && ms.len() <= 20 && ms.bytes().all(|b| b.is_ascii_digit()))
}

/// The agent's accounts in `root`, its own folders and its linked profiles, oldest first.
pub fn list(root: &Path, agent: SessionAgent) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join(agent.as_str())) else {
        return vec![];
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = if entry.path().is_dir() {
                name
            } else {
                name.strip_suffix(LINK)?.to_string()
            };
            valid_id(&id).then_some(id)
        })
        .collect();
    ids.sort_by_key(|id| id[2..].parse::<u64>().unwrap_or(0));
    ids.dedup();
    ids
}

/// The suffix of a linked account's file.
const LINK: &str = ".link";

fn link_file(root: &Path, agent: SessionAgent, id: &str) -> PathBuf {
    root.join(agent.as_str()).join(format!("{id}{LINK}"))
}

/// The profile a linked account names, as written; None for an account of the app's own.
pub fn linked(root: &Path, agent: SessionAgent, id: &str) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    let text = std::fs::read_to_string(link_file(root, agent, id)).ok()?;
    let path = text.trim_end_matches(['\r', '\n']);
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Links the profile at `profile` as an account of `agent`, unless one already names it (by its real path); its id.
pub fn link_profile(root: &Path, agent: SessionAgent, profile: &Path) -> Result<String> {
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let target = real(profile);
    if let Some(id) = list(root, agent)
        .into_iter()
        .find(|id| dir(root, agent, id).is_ok_and(|dir| real(&dir) == target))
    {
        return Ok(id);
    }
    std::fs::create_dir_all(root.join(agent.as_str()))
        .map_err(|error| format!("Could not keep the account: {error}"))?;
    // A new id per link: two profiles linked in the same millisecond take the next free one, never the same file.
    let mut ms = crate::now_ms();
    loop {
        let id = format!("a_{ms}");
        let taken = root.join(agent.as_str()).join(&id).exists();
        let created = if taken {
            Err(std::io::ErrorKind::AlreadyExists.into())
        } else {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(link_file(root, agent, &id))
        };
        match created {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(profile.to_string_lossy().as_bytes())
                    .map_err(|error| format!("Could not keep the account: {error}"))?;
                return Ok(id);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => ms += 1,
            Err(error) => return Err(format!("Could not keep the account: {error}")),
        }
    }
}

/// The file that keeps the real paths of the linked profiles the person removed, one per line.
const FORGOTTEN: &str = "forgotten";

fn real_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether the person removed a linked account that named `profile` (by its real path).
pub fn forgotten(root: &Path, agent: SessionAgent, profile: &Path) -> bool {
    let target = real_path(profile);
    std::fs::read_to_string(root.join(agent.as_str()).join(FORGOTTEN))
        .is_ok_and(|text| text.lines().any(|line| !line.is_empty() && Path::new(line) == target))
}

/// Links a profile [`discover`] found, unless the person removed it before: Remove on a linked account sticks, and
/// the next plan check does not list it again. Its id, or None for a forgotten profile.
pub fn adopt(root: &Path, agent: SessionAgent, profile: &Path) -> Result<Option<String>> {
    if forgotten(root, agent, profile) {
        return Ok(None);
    }
    link_profile(root, agent, profile).map(Some)
}

/// Keeps `profile`'s real path in the forgotten list, so [`adopt`] skips it.
fn forget(root: &Path, agent: SessionAgent, profile: &Path) -> Result<()> {
    if forgotten(root, agent, profile) {
        return Ok(());
    }
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(agent.as_str()).join(FORGOTTEN))
        .and_then(|mut file| writeln!(file, "{}", real_path(profile).to_string_lossy()))
        .map_err(|error| format!("Could not forget the account: {error}"))
}

/// The agent's logins on this computer that Nexus did not make and that are not `own` (the folder its own login
/// reads): each folder in `named` (the `CLAUDE_CONFIG_DIR` values of the app and of the person's shell), then the
/// agent's default folder and its siblings named like it (`~/.claude-work` beside `~/.claude`), when they hold a login.
/// One folder reached two ways (a symlink) counts once. On macOS Claude Code's default folder is left out: its login
/// is the keychain item of no folder, which a path cannot name.
pub fn discover(agent: SessionAgent, home: &Path, own: &Path, named: &[String]) -> Vec<PathBuf> {
    let Some(coding) = agent.coding() else {
        return vec![];
    };
    let default = nexus_local_agents::profile_under(coding, home);
    let mut candidates: Vec<PathBuf> = named.iter().filter(|dir| !dir.is_empty()).map(PathBuf::from).collect();
    let default_is_a_path = !(cfg!(target_os = "macos") && agent == SessionAgent::Claude);
    if default_is_a_path {
        candidates.push(default.clone());
    }
    if let (Some(parent), Some(name)) = (default.parent(), default.file_name().and_then(|name| name.to_str()))
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        let prefix = format!("{name}-");
        let mut siblings: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.file_name().to_str().is_some_and(|file| file.starts_with(&prefix)))
            .map(|entry| entry.path())
            .collect();
        siblings.sort();
        candidates.extend(siblings);
    }
    let real = |path: &Path| std::fs::canonicalize(path).ok();
    let mut seen: Vec<PathBuf> = real(own).into_iter().collect();
    let mut found = vec![];
    for candidate in candidates {
        let Some(target) = real(&candidate) else { continue };
        if seen.contains(&target) || !holds_login(agent, &candidate) {
            continue;
        }
        seen.push(target);
        found.push(candidate);
    }
    found
}

/// Whether `dir` holds a login of `agent`: Claude Code's state or credentials file, Codex's `auth.json`.
fn holds_login(agent: SessionAgent, dir: &Path) -> bool {
    let files: &[&str] = match agent {
        SessionAgent::Claude => &[".claude.json", ".credentials.json"],
        SessionAgent::Codex => &["auth.json"],
        _ => &[],
    };
    dir.is_dir() && files.iter().any(|file| dir.join(file).is_file())
}

/// A new account of `agent` in `root`, sharing what `home` (the agent's own folder) has. It is signed out until the
/// agent's login runs with its folder.
pub fn add(root: &Path, agent: SessionAgent, home: &Path) -> Result<String> {
    let id = format!("a_{}", crate::now_ms());
    let parent = root.join(agent.as_str());
    let dir = parent.join(&id);
    // Only the person may read them: an account's folder holds its sign-in (Codex's `auth.json`, Claude's config).
    crate::private_fs::create_dir_all(&parent)
        .and_then(|()| crate::private_fs::restrict_dir(&parent))
        .and_then(|()| crate::private_fs::create_dir(&dir))
        .map_err(|error| format!("Could not make a folder for the account: {error}"))?;
    share(&dir, agent, home);
    Ok(id)
}

/// The account's folder, while it is there: its own, or the profile it links to.
pub fn dir(root: &Path, agent: SessionAgent, id: &str) -> Result<PathBuf> {
    let dir = root.join(agent.as_str()).join(id);
    if valid_id(id) && dir.is_dir() {
        Ok(dir)
    } else if let Some(profile) = linked(root, agent, id).filter(|profile| profile.is_dir()) {
        Ok(profile)
    } else {
        Err(format!(
            "That {} account is no longer on this computer. Start a new session on another.",
            plan(agent)
        ))
    }
}

/// Links into `dir` what `home` has of what the agent shares and `dir` has nothing in place of. Run before each start
/// too, so what the person adds to their own folder later is shared as well. A link that cannot be made is skipped:
/// the account then works with the agent's defaults there.
pub fn share(dir: &Path, agent: SessionAgent, home: &Path) {
    let Some(folders) = harness(agent).accounts() else {
        return;
    };
    // Only a folder of the app's own (`<root>/<agent>/<id>`): a linked profile is the person's, and stays as it is.
    let own_folder = dir.file_name().and_then(|name| name.to_str()).is_some_and(valid_id)
        && dir
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == agent.as_str());
    if !own_folder {
        return;
    }
    for name in folders.shared {
        let (from, to) = (home.join(name), dir.join(name));
        if from.exists() && to.symlink_metadata().is_err() {
            let _ = link(&from, &to);
        }
    }
}

#[cfg(unix)]
fn link(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(from, to)
}

/// Windows makes links only in Developer Mode or as an administrator; elsewhere nothing is shared.
#[cfg(windows)]
fn link(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::os::windows::fs::symlink_dir(from, to)
    } else {
        std::os::windows::fs::symlink_file(from, to)
    }
}

/// Removes the account's folder: its links, never what they point to. A linked account loses only its link file:
/// the profile it names is the person's.
pub fn remove(root: &Path, agent: SessionAgent, id: &str) -> Result<()> {
    if let Some(profile) = linked(root, agent, id) {
        forget(root, agent, &profile)?;
        return std::fs::remove_file(link_file(root, agent, id))
            .map_err(|error| format!("Could not forget the account: {error}"));
    }
    std::fs::remove_dir_all(dir(root, agent, id)?)
        .map_err(|error| format!("Could not remove the account's folder: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-accounts-{name}-{}", crate::now_ms()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        dir
    }

    #[test]
    fn profile_resolution_uses_catalog_overrides_and_the_callers_home() {
        let home = Path::new("/users/person");
        for (agent, folder) in [
            (SessionAgent::Claude, ".claude"),
            (SessionAgent::Codex, ".codex"),
            (SessionAgent::Antigravity, ".gemini/antigravity-cli"),
            (SessionAgent::Nexus, ".nexus"),
        ] {
            assert_eq!(profile_dir(agent, home, &|_| None), home.join(folder));
            assert_eq!(profile_dir(agent, home, &|_| Some(String::new())), home.join(folder));
        }
        for agent in [SessionAgent::Claude, SessionAgent::Codex, SessionAgent::Nexus] {
            let env = |name: &str| (Some(name) == account_variable(agent)).then(|| "/accounts/other".to_string());
            assert_eq!(profile_dir(agent, home, &env), PathBuf::from("/accounts/other"));
        }
        let env = |name: &str| match name {
            "ANTIGRAVITY_ACCOUNT_DIR" => Some("/unsupported".into()),
            "HOME" => Some("/different-home".into()),
            _ => None,
        };
        assert_eq!(
            profile_dir(SessionAgent::Antigravity, home, &env),
            home.join(".gemini/antigravity-cli")
        );
        assert_eq!(
            profile_dir(SessionAgent::Claude, Path::new(""), &|_| None),
            PathBuf::from(".claude")
        );
    }

    #[test]
    fn account_variables_come_from_the_catalog_and_agy_has_none() {
        assert_eq!(account_variable(SessionAgent::Claude), Some("CLAUDE_CONFIG_DIR"));
        assert_eq!(account_variable(SessionAgent::Codex), Some("CODEX_HOME"));
        assert_eq!(account_variable(SessionAgent::Nexus), Some("NEXUS_CONFIG_DIR"));
        // agy's catalog entry has `profile.var: None`: its folder is always under the home folder.
        assert_eq!(account_variable(SessionAgent::Antigravity), None);
        // The previous desktop app's name for agy is unchanged.
        assert_eq!(variable(SessionAgent::Antigravity), "ANTIGRAVITY_ACCOUNT_DIR");
        assert_eq!(variable(SessionAgent::Claude), "CLAUDE_CONFIG_DIR");
    }

    #[test]
    fn checks_ids_before_they_name_a_folder() {
        assert!(valid_id("a_1792602091776"));
        for bad in ["", "a_", "a_../x", "a_12/3", "b_123", "a_123456789012345678901"] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn adds_shares_lists_and_removes_accounts() {
        let home = temp("home");
        std::fs::write(home.join("CLAUDE.md"), "# mine").unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(home.join("projects/p")).unwrap_or_else(|e| panic!("{e}"));
        let root = temp("root");
        assert_eq!(list(&root, SessionAgent::Claude), Vec::<String>::new());

        let first = add(&root, SessionAgent::Claude, &home).unwrap_or_else(|e| panic!("{e}"));
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = add(&root, SessionAgent::Claude, &home).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(list(&root, SessionAgent::Claude), vec![first.clone(), second.clone()]);
        assert_eq!(list(&root, SessionAgent::Codex), Vec::<String>::new());

        let dir = dir(&root, SessionAgent::Claude, &first).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            std::fs::read_to_string(dir.join("CLAUDE.md")).ok().as_deref(),
            Some("# mine")
        );
        assert!(dir.join("projects/p").is_dir());
        // Only what the person's folder has is linked.
        assert!(dir.join("skills").symlink_metadata().is_err());
        // Something added later is shared at the next start.
        std::fs::create_dir_all(home.join("skills")).unwrap_or_else(|e| panic!("{e}"));
        share(&dir, SessionAgent::Claude, &home);
        assert!(dir.join("skills").is_dir());

        remove(&root, SessionAgent::Claude, &first).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(list(&root, SessionAgent::Claude), vec![second]);
        // The links went, what they pointed to stays.
        assert!(home.join("CLAUDE.md").is_file() && home.join("projects/p").is_dir());
        assert!(super::dir(&root, SessionAgent::Claude, &first).is_err());
        assert!(remove(&root, SessionAgent::Claude, "a_../../home").is_err());
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A work and a personal profile made outside Nexus are both found, a folder reached through a symlink
    /// counts once, and a linked profile is listed, read and forgotten without Nexus writing into it.
    #[cfg(unix)]
    #[test]
    fn profiles_made_outside_nexus_are_found_linked_and_never_written() {
        let home = temp("found-home");
        let login = |dir: &Path, file: &str| {
            std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("{e}"));
            std::fs::write(dir.join(file), "{}").unwrap_or_else(|e| panic!("{e}"));
        };
        let (own, work, play) = (
            home.join(".claude"),
            home.join(".claude-work"),
            home.join("profiles/personal"),
        );
        login(&own, ".credentials.json");
        login(&work, ".claude.json");
        login(&play, ".credentials.json");
        // A folder with no login is not an account.
        std::fs::create_dir_all(home.join(".claude-empty")).unwrap_or_else(|e| panic!("{e}"));
        // The same work profile, reached through a symlink.
        std::os::unix::fs::symlink(&work, home.join(".claude-alias")).unwrap_or_else(|e| panic!("{e}"));
        let named = [play.to_string_lossy().into_owned(), String::new()];
        let found = discover(SessionAgent::Claude, &home, &own, &named);
        // The person's named folder first, then the siblings; the own login and the second way to work are left out.
        let alias_or_work = |path: &PathBuf| path == &work || path == &home.join(".claude-alias");
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0], play);
        assert!(alias_or_work(&found[1]), "{found:?}");
        // The own login is a profile too when it reads another folder.
        let found = discover(SessionAgent::Claude, &home, &work, &[]);
        assert_eq!(
            found.iter().any(|path| path == &own),
            !cfg!(target_os = "macos"),
            "{found:?}"
        );

        let root = temp("found-root");
        let id = link_profile(&root, SessionAgent::Claude, &work).unwrap_or_else(|e| panic!("{e}"));
        // Linking it again, or through its symlink, keeps one account.
        assert_eq!(link_profile(&root, SessionAgent::Claude, &work).ok(), Some(id.clone()));
        assert_eq!(
            link_profile(&root, SessionAgent::Claude, &home.join(".claude-alias")).ok(),
            Some(id.clone())
        );
        assert_eq!(list(&root, SessionAgent::Claude), vec![id.clone()]);
        // The path as written, so Claude Code finds the same keychain item.
        assert_eq!(dir(&root, SessionAgent::Claude, &id).ok(), Some(work.clone()));
        std::fs::write(own.join("CLAUDE.md"), "# mine").unwrap_or_else(|e| panic!("{e}"));
        share(&work, SessionAgent::Claude, &own);
        assert!(
            work.join("CLAUDE.md").symlink_metadata().is_err(),
            "nothing written into it"
        );
        remove(&root, SessionAgent::Claude, &id).unwrap_or_else(|e| panic!("{e}"));
        assert!(list(&root, SessionAgent::Claude).is_empty());
        assert!(work.join(".claude.json").is_file(), "the profile stays");
        // Remove sticks: the next plan check adopts what it finds again, and skips the forgotten profile, by either path.
        for profile in discover(SessionAgent::Claude, &home, &own, &named) {
            adopt(&root, SessionAgent::Claude, &profile).unwrap_or_else(|e| panic!("{e}"));
        }
        assert_eq!(
            adopt(&root, SessionAgent::Claude, &home.join(".claude-alias")).ok(),
            Some(None)
        );
        let left: Vec<PathBuf> = list(&root, SessionAgent::Claude)
            .iter()
            .filter_map(|id| linked(&root, SessionAgent::Claude, id))
            .collect();
        assert_eq!(left, vec![play.clone()], "only the profile not removed");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two profiles linked in the same millisecond keep two link files.
    #[test]
    fn profiles_linked_together_keep_their_own_ids() {
        let home = temp("together-home");
        let root = temp("together-root");
        let (one, two) = (home.join("one"), home.join("two"));
        std::fs::create_dir_all(&one).unwrap_or_else(|e| panic!("{e}"));
        std::fs::create_dir_all(&two).unwrap_or_else(|e| panic!("{e}"));
        // A link file at every id the next few milliseconds can give, so both links must step past them.
        std::fs::create_dir_all(root.join("claude")).unwrap_or_else(|e| panic!("{e}"));
        let ms = crate::now_ms();
        for at in ms..ms + 5 {
            std::fs::write(root.join("claude").join(format!("a_{at}.link")), "").unwrap_or_else(|e| panic!("{e}"));
        }
        let a = link_profile(&root, SessionAgent::Claude, &one).unwrap_or_else(|e| panic!("{e}"));
        let b = link_profile(&root, SessionAgent::Claude, &two).unwrap_or_else(|e| panic!("{e}"));
        assert_ne!(a, b);
        assert_eq!(linked(&root, SessionAgent::Claude, &a), Some(one));
        assert_eq!(linked(&root, SessionAgent::Claude, &b), Some(two));
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_codex_account_shares_its_config_but_never_its_login_or_state() {
        let home = temp("codex-home");
        for file in ["config.toml", "auth.json", "state_5.sqlite"] {
            std::fs::write(home.join(file), file).unwrap_or_else(|e| panic!("{e}"));
        }
        let root = temp("codex-root");
        let id = add(&root, SessionAgent::Codex, &home).unwrap_or_else(|e| panic!("{e}"));
        let dir = dir(&root, SessionAgent::Codex, &id).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(dir, root.join("codex").join(&id));
        assert!(dir.join("config.toml").is_file());
        assert!(dir.join("auth.json").symlink_metadata().is_err());
        assert!(dir.join("state_5.sqlite").symlink_metadata().is_err());
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&root);
    }
}
