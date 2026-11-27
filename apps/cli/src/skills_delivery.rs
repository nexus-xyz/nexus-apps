//! Skills at launch (`skills-delivery.ts`): before `nexus claude` or `nexus codex` starts the agent, it writes
//! the person's skills into the agent's own skills folder, so the agent finds them with no setup step.
//!
//! **The library is delivered in the same pass**. The skills of the person's organization and teams come from
//! `GET /v1/skill-library`, each at its approved version and under its own `skl_` id. The manifest, the lock and the
//! removal rules are the same, and the manifest keeps its version 1 format. `merge` applies the name rules: a name never
//! shadows. Gateway marks a library skill whose name collides (`duplicateName`), because neither list carries a SHA-256. A
//! marked name is delivered by neither skill, and delivery reports it as `duplicate_name` with the scope of each. A library
//! skill that is not marked is served, and a personal skill of its name is dropped without a word: Gateway leaves it
//! unmarked only when it is the one library skill of its name and the person's skill, if any, has its bytes. Two library
//! skills of one name collide whatever the marker says. A Gateway without the library route (404), and a key that may not
//! hold skills (403), have no library.
//!
//! **Nexus owns what it wrote, and nothing else.** A record on this machine (the manifest) lists each folder Nexus wrote
//! for a person: the skill, its version and the SHA-256 of the SKILL.md. Delivery writes and replaces the SKILL.md of a
//! real folder that a manifest lists, and never writes through a link. A folder that no manifest lists, and a link or
//! file where a listed folder was, are never touched, and a skill with that name is not delivered (`duplicate_name`).
//!
//! **A skill that is gone takes only what Nexus wrote.** Delivery removes a SKILL.md only while it is the file Nexus
//! wrote (its SHA-256 matches), and the folder only when nothing else is in it. What it leaves is reported (`left`) and
//! is the person's from then on. The record forgets it. If the skill comes back, the folder keeps it out
//! (`duplicate_name`), and the delivery line says to move or delete the folder.
//!
//! **A launch that dies strands nothing.** The manifest lists a folder before Nexus writes it. The next launch finds a
//! folder that the dead launch was writing, and replaces or removes it.
//!
//! **One person at a time on a folder.** When a second person launches on the same agent folder, the files Nexus wrote
//! for the first person go, and their manifest with them.
//!
//! **One launch at a time on a folder.** A lock file in Nexus's own config folder serves each agent folder, and names
//! the launch that holds it. A second launch waits for it up to ten seconds, then stops with `delivery_locked`. The next
//! launch takes over a lock when the launch that held it is gone, or when nobody touched it for 330 seconds.
//!
//! **Fail fast.** When Gateway cannot answer, answers a list it cannot read, or a file cannot be written, the launch
//! stops before the agent starts, and `--no-skills` starts the agent without delivery. A Gateway with no skills routes
//! (404) and a key that may not hold skills (403: a key that belongs to no person, `person_required`) deliver nothing,
//! and the agent starts as it did before skills.
//!
//! **Gateway's rate limit is waited out**. Each skill that is not current is one export, and Gateway allows a
//! key 120 requests a minute, so a first launch with a large library meets a 429 `rate_limited`. Delivery then waits the
//! `Retry-After` Gateway sends and asks for the same export again, and says so once. A wait longer than a minute, a 429
//! without a `Retry-After`, or more than three minutes of waits in one launch stops the launch with the 429, as before.
//! Only exports wait: the lists, `tenants/me` and the records do not.
//!
//! Delivery says what it put in the folder, never that the agent used it: Gateway records each delivery as `delivered`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use nexus_client::agent_entries::Agent;
use nexus_client::base::sha256_hex;
use nexus_client::blocking::Download;
use serde_json::{Value, json};

use crate::client::{Client, enc};
use crate::config::create_private_dir;
use crate::error::{CliError, Error, Result};
use crate::json::s;
use crate::skills::valid_name;
use crate::style::Style;
use crate::{Context, Io};

/// How long a second launch waits for the lock before it stops with `delivery_locked`.
const LOCK_WAIT: Duration = Duration::from_secs(10);
/// A lock nobody touched for this long belongs to a launch that died. It is longer than a request may take (300 s).
const LOCK_STALE: Duration = Duration::from_secs(330);
const LOCK_POLL: Duration = Duration::from_millis(50);
/// The skills named on the delivery line, before "and N more".
const SHOWN: usize = 8;
const NO_SKILLS: &str = "Or start the agent without skills: add --no-skills.";
/// The most skills one `POST /v1/skill-loads` takes (Gateway's `MAX_LOADS_A_CALL`). A larger delivery goes in several
/// calls, each all or none.
const MAX_LOADS_A_CALL: usize = 900;
/// The longest `Retry-After` delivery waits for one export. A longer one stops the launch.
const RATE_WAIT_MAX: Duration = Duration::from_secs(60);
/// The most a launch waits for Gateway's rate limit in all. The lock goes stale only after [`LOCK_STALE`], and delivery
/// touches it before each export.
const RATE_WAIT_TOTAL: Duration = Duration::from_secs(180);

/// One folder Nexus wrote: the skill, the version, and the SHA-256 of the SKILL.md it wrote.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    skill_id: String,
    version: i64,
    sha256: String,
}

/// Where a skill comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    /// One of the person's own skills.
    Personal,
    /// The organization's library.
    Org,
    /// A team's library, by the team's id (the library list gives no name).
    Team(String),
}

impl Scope {
    /// As the delivery line says it.
    fn words(&self) -> String {
        match self {
            Scope::Personal => "personal".into(),
            Scope::Org => "organization".into(),
            Scope::Team(id) => format!("team {id}"),
        }
    }

    /// As `--json` says it: the wire's `scope` and, for a team, its `teamId`.
    fn json(&self) -> Value {
        match self {
            Scope::Personal => json!({ "scope": "personal" }),
            Scope::Org => json!({ "scope": "org" }),
            Scope::Team(id) => json!({ "scope": "team", "teamId": id }),
        }
    }
}

/// A skill Gateway listed, with what delivery needs of it. A name that is not a skill name carries nothing else.
#[derive(Debug, Clone, PartialEq)]
struct Listed {
    name: String,
    id: String,
    /// The version to deliver: a personal skill's latest, a library skill's approved version.
    version: i64,
    scope: Scope,
}

/// A library skill, and whether Gateway marked its name (`duplicateName`).
#[derive(Debug, Clone, PartialEq)]
struct Shared {
    skill: Listed,
    marked: bool,
}

/// What the two lists come to for one name.
#[derive(Debug, Clone, PartialEq)]
enum Plan {
    /// Deliver this skill.
    Deliver(Listed),
    /// Other skills of this name keep each other out: none is delivered. `scopes` is where each comes from.
    Collide { name: String, scopes: Vec<Scope> },
}

/// A skill that is in the agent's folder when the agent starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Present {
    pub name: String,
    pub skill_id: String,
    pub version: i64,
    pub scope: Scope,
}

/// A skill that was not delivered, with the code.
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub name: String,
    /// `duplicate_name` (a folder of that name is not Nexus's, or another skill has the name) or `invalid_name`.
    pub code: String,
    /// For a name that other skills have, where each of them comes from. Empty for any other refusal.
    pub scopes: Vec<Scope>,
}

/// What a delivery did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Delivery {
    /// The agent's skills folder.
    pub folder: PathBuf,
    /// The skills in it when the agent starts, delivered now or current already, in name order.
    pub present: Vec<Present>,
    /// Of those, the names this run wrote or replaced.
    pub written: Vec<String>,
    /// Skills not delivered, in name order.
    pub refused: Vec<Refused>,
    /// Folders Nexus wrote for this person before, and this run removed, because the skill is gone.
    pub removed: Vec<String>,
    /// Names whose files Nexus did not remove, though the skill is gone, with why: `changed` (the SKILL.md is not the
    /// file Nexus wrote), `has_other_files` (the folder holds files Nexus did not write) or `not_a_folder` (a link or a
    /// file stands where the folder was).
    pub left: Vec<(String, String)>,
    /// Folders Nexus wrote for another person, and this run removed.
    pub others_removed: Vec<String>,
}

/// What removing a skill's folder did.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Removal {
    /// Nothing was there.
    Absent,
    /// The SKILL.md and the folder are gone.
    Removed,
    /// Something stays, with the code that says what.
    Left(&'static str),
}

/// `path` made absolute from `base`, with `.` and `..` resolved by name alone, as Node's `path.resolve` does.
fn absolute(base: &Path, path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in base.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// An integer as JavaScript's `Number.isInteger` sees it: `1`, `1.0` and `1e0` are all 1.
fn as_integer(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|number| number.fract() == 0.0 && number.abs() <= 9_007_199_254_740_991.0)
            .map(|number| number as i64)
    })
}

/// Where the agent keeps its skills: `<CLAUDE_CONFIG_DIR or ~/.claude>/skills`, `<CODEX_HOME or ~/.codex>/skills`.
fn agent_folder(ctx: &Context, agent: Agent) -> Result<PathBuf> {
    // An agent whose folder no variable moves keeps it under the home folder.
    let (variable, dot) = (agent.entry().profile.var.unwrap_or("HOME"), agent.entry().profile.dir);
    let set = |name: &str| ctx.var(name).filter(|value| !value.trim().is_empty());
    let folder = match agent.entry().profile.var.and_then(set) {
        Some(folder) => PathBuf::from(folder),
        None => {
            let Some(home) = set("HOME").or_else(|| set("USERPROFILE")) else {
                return Err(CliError::hint(
                    format!("Nexus cannot find where {agent} keeps its skills: neither {variable} nor HOME is set."),
                    format!("Set {variable}. {NO_SKILLS}"),
                )
                .into());
            };
            PathBuf::from(home).join(dot)
        }
    };
    Ok(absolute(&ctx.cwd, &folder))
}

/// Nexus's own folder for one agent folder: its lock and the manifests of the people who launched on it. The name is the
/// first 16 characters of the SHA-256 of the agent folder's path.
fn state_dir(ctx: &Context, agent_folder: &Path) -> PathBuf {
    let key = sha256_hex(&[agent_folder.to_string_lossy().as_bytes()]);
    ctx.config_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("skills-delivered")
        .join(&key[..16])
}

fn manifest_names(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = read
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .collect();
    paths.sort();
    paths
}

fn io_failed(action: &str, path: &Path, error: &std::io::Error) -> Error {
    CliError::hint(
        format!("Could not {action} {}: {error} (delivery_failed)", path.display()),
        format!("Check that you may write there. {NO_SKILLS}"),
    )
    .into()
}

/// A Gateway failure while getting skills, as the line the person reads, with the way to start without them.
fn stopped(error: Error) -> Error {
    match error {
        Error::Core(core) => {
            let code = core
                .code
                .as_deref()
                .map(|code| format!(" ({code})"))
                .unwrap_or_default();
            CliError::hint(
                format!("Could not get your skills from Nexus Gateway: {}{code}", core.message),
                NO_SKILLS,
            )
            .into()
        }
        Error::Cli(cli) => CliError::hint(cli.message, NO_SKILLS).into(),
        other => other,
    }
}

/// A Gateway failure while reporting what was delivered. A skill can change between the list and the report: an approver
/// withdraws a library version, or the person deletes a skill. Gateway then answers 404 for the whole call. The next
/// launch lists again, so the line says to run it again.
fn report_failed(error: Error) -> Error {
    match error {
        Error::Core(core) => {
            let code = core
                .code
                .as_deref()
                .map(|code| format!(" ({code})"))
                .unwrap_or_default();
            let changed = matches!(
                core.code.as_deref(),
                Some("skill_not_found" | "skill_version_not_found")
            );
            let hint = if changed {
                format!("A skill changed while the launch ran. Run the command again. {NO_SKILLS}")
            } else {
                NO_SKILLS.to_string()
            };
            CliError::hint(
                format!(
                    "Could not report the delivered skills to Nexus Gateway: {}{code}",
                    core.message
                ),
                hint,
            )
            .into()
        }
        other => stopped(other),
    }
}

/// Gateway's list is not the shape it documents. It is never read as "no skills": that would remove folders. `list`
/// says which one: "skills list" or "skill library".
fn malformed(list: &str, why: &str) -> Error {
    CliError::hint(
        format!("Nexus Gateway answered the {list} in a way this nexus does not understand: {why} (delivery_failed)"),
        NO_SKILLS,
    )
    .into()
}

// ---------------------------------------------------------------------------------------------------------- the lock

/// The lock of one agent folder: a file made with `create_new`, so one launch holds it. It names the launch (its process
/// and a token no other launch has), and goes when the launch ends, if it is still this launch's.
struct Lock {
    path: PathBuf,
    token: String,
}

/// The process a lock file names (its first line).
fn lock_pid(held: &str) -> Option<i32> {
    held.lines().next()?.parse().ok().filter(|pid| *pid > 0)
}

/// The token a lock file holds (its second line).
fn lock_token(held: &str) -> Option<&str> {
    held.lines().nth(1)
}

/// Whether a process exists. Signal 0 only asks; `EPERM` means it exists and belongs to someone else.
#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    !matches!(kill(Pid::from_raw(pid), None), Err(Errno::ESRCH))
}

/// Without Unix signals the age of the lock alone says that a launch died.
#[cfg(not(unix))]
fn process_alive(_pid: i32) -> bool {
    true
}

/// Removes the lock `judged` that a launch found stale; `true` when it did. The lock is moved aside first and the moved
/// file is read. If it is not the lock that was judged (another launch took the lock in between), it goes back.
fn take_over(dir: &Path, path: &Path, judged: &str) -> bool {
    let aside = dir.join(format!("lock.stale.{}", hex::encode(rand::random::<[u8; 8]>())));
    if std::fs::rename(path, &aside).is_err() {
        return false;
    }
    let moved = std::fs::read_to_string(&aside).unwrap_or_default();
    let stale = moved == judged;
    if !stale {
        // A hard link never replaces a file: if the place is taken again, the launch that lost its lock finds out when
        // it touches the lock.
        let _ = std::fs::hard_link(&aside, path);
    }
    let _ = std::fs::remove_file(&aside);
    stale
}

impl Lock {
    fn take(dir: &Path, wait: Duration, folder: &Path) -> Result<Lock> {
        create_private_dir(dir).map_err(|error| io_failed("create", dir, &error))?;
        let path = dir.join("lock");
        let token = hex::encode(rand::random::<[u8; 16]>());
        let deadline = Instant::now() + wait;
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    // A launch that dies between these two calls leaves an empty lock, which goes stale by its age.
                    let text = format!("{}\n{token}\n", std::process::id());
                    file.write_all(text.as_bytes())
                        .map_err(|error| io_failed("write", &path, &error))?;
                    return Ok(Lock { path, token });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let held = std::fs::read_to_string(&path).unwrap_or_default();
                    let idle = std::fs::metadata(&path)
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|modified| SystemTime::now().duration_since(modified).ok());
                    let died = lock_pid(&held).is_some_and(|pid| !process_alive(pid));
                    if (died || idle.is_some_and(|idle| idle > LOCK_STALE)) && take_over(dir, &path, &held) {
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return Err(CliError::hint(
                            format!(
                                "Another nexus run is delivering skills to {} (delivery_locked).",
                                folder.display()
                            ),
                            format!(
                                "Wait for it, then run again. If none is running, delete {}.",
                                path.display()
                            ),
                        )
                        .into());
                    }
                    std::thread::sleep(LOCK_POLL);
                }
                Err(error) => return Err(io_failed("create", &path, &error)),
            }
        }
    }

    /// Says the launch is alive: a lock nobody touches goes stale. A lock that is no longer this launch's is an error,
    /// because another launch took it over.
    fn touch(&self) -> Result<()> {
        let held = std::fs::read_to_string(&self.path).unwrap_or_default();
        if lock_token(&held) != Some(self.token.as_str()) {
            return Err(CliError::hint(
                "Another nexus run took the delivery lock while this one was working (delivery_locked).",
                format!("Run again. {NO_SKILLS}"),
            )
            .into());
        }
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&self.path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Ok(())
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let ours = std::fs::read_to_string(&self.path).is_ok_and(|held| lock_token(&held) == Some(self.token.as_str()));
        if ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

// ------------------------------------------------------------------------------------------------------ the manifest

/// One person's record for one agent folder.
struct Manifest {
    path: PathBuf,
    user_id: String,
    folder: PathBuf,
    /// The agent's skills folder in `folder`.
    skills_dir: PathBuf,
}

impl Manifest {
    fn unreadable(&self, why: &str) -> Error {
        CliError::hint(
            format!(
                "The record of delivered skills {} is not readable: {why} (delivery_failed)",
                self.path.display()
            ),
            format!(
                "Delete it, and the folders in {} that have the names of your Nexus skills, then run again. {NO_SKILLS}",
                self.skills_dir.display()
            ),
        )
        .into()
    }

    /// The entries by skill name. A missing file is no entries.
    fn read(&self) -> Result<BTreeMap<String, Entry>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(error) => return Err(io_failed("read", &self.path, &error)),
        };
        let value: Value = serde_json::from_str(&text).map_err(|error| self.unreadable(&error.to_string()))?;
        if as_integer(value.get("version")) != Some(1) {
            return Err(self.unreadable("it is not a version 1 record"));
        }
        if value.get("userId").and_then(Value::as_str) != Some(self.user_id.as_str()) {
            return Err(self.unreadable("it names another person"));
        }
        let Some(skills) = value.get("skills").and_then(Value::as_object) else {
            return Err(self.unreadable("it has no `skills`"));
        };
        let mut entries = BTreeMap::new();
        for (name, entry) in skills {
            let skill_id = s(entry, "skillId");
            let version = as_integer(entry.get("version")).filter(|version| *version >= 1);
            let sha256 = s(entry, "sha256")
                .filter(|sha| sha.len() == 64 && sha.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));
            let (true, Some(skill_id), Some(version), Some(sha256)) = (valid_name(name), skill_id, version, sha256)
            else {
                return Err(self.unreadable(&format!("the entry for {name:?} is not a delivered skill")));
            };
            entries.insert(
                name.clone(),
                Entry {
                    skill_id: skill_id.to_string(),
                    version,
                    sha256: sha256.to_string(),
                },
            );
        }
        Ok(entries)
    }

    /// The manifest, written whole: a temporary file, flushed to disk, renamed into place. No entries: no file.
    fn write(&self, entries: &BTreeMap<String, Entry>) -> Result<()> {
        let path = &self.path;
        if entries.is_empty() {
            return match std::fs::remove_file(path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(io_failed("remove", path, &error)),
                _ => Ok(()),
            };
        }
        let skills: serde_json::Map<String, Value> = entries
            .iter()
            .map(|(name, entry)| {
                (
                    name.clone(),
                    json!({ "skillId": entry.skill_id, "version": entry.version, "sha256": entry.sha256 }),
                )
            })
            .collect();
        let text = serde_json::to_string_pretty(&json!({
            "version": 1,
            "userId": self.user_id,
            "agentFolder": self.folder.display().to_string(),
            "skills": skills,
        }))
        .expect("a manifest serializes");
        let partial = path.with_extension(format!("json.{}.tmp", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Flushed before the rename: after a power cut the manifest is the old one or the new one, never half of one.
        options
            .open(&partial)
            .and_then(|mut file| {
                file.write_all(format!("{text}\n").as_bytes())?;
                file.sync_all()
            })
            .and_then(|()| std::fs::rename(&partial, path))
            .map_err(|error| {
                let _ = std::fs::remove_file(&partial);
                io_failed("write", path, &error)
            })
    }
}

// ---------------------------------------------------------------------------------------------------------- the files

/// The SHA-256 of a skill's SKILL.md on disk, when it is there.
fn file_sha(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| sha256_hex(&[bytes.as_slice()]))
}

/// A file this module leaves in a skill's folder while it writes the SKILL.md.
fn is_partial(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with(".SKILL.md.") && name.ends_with(".tmp"))
}

/// `<skills>/<name>/SKILL.md`, written whole: a temporary file in the same folder, renamed into place, so an agent
/// never reads half a file. The folder is a real folder: a link there would send the write to its target.
fn write_skill(skills: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let dir = skills.join(name);
    let made = match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_dir() => false,
        Ok(_) => {
            return Err(CliError::hint(
                format!("{} is not a folder Nexus wrote (duplicate_name)", dir.display()),
                NO_SKILLS,
            )
            .into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The skills folder itself may be a link, as in a dotfiles repository: that is the person's choice.
            std::fs::create_dir_all(skills)
                .and_then(|()| std::fs::create_dir(&dir))
                .map_err(|error| io_failed("create", &dir, &error))?;
            true
        }
        Err(error) => return Err(io_failed("look at", &dir, &error)),
    };
    let partial = dir.join(format!(".SKILL.md.{}.tmp", std::process::id()));
    let file = dir.join("SKILL.md");
    let _ = std::fs::remove_file(&partial);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .and_then(|mut handle| handle.write_all(bytes))
        .and_then(|()| std::fs::rename(&partial, &file))
        .map_err(|error| {
            let _ = std::fs::remove_file(&partial);
            // A folder this run made and could not fill is not left behind: the next run lists it or finds it empty.
            if made {
                let _ = std::fs::remove_dir(&dir);
            }
            io_failed("write", &file, &error)
        })
}

/// What Nexus wrote in a skill's folder, removed: the SKILL.md while its SHA-256 is `written_sha`, then the folder when
/// nothing else is in it. A link or a file where the folder was, a SKILL.md someone changed, and files someone added are
/// left, and the result says which.
fn remove_skill(skills: &Path, name: &str, written_sha: &str) -> Result<Removal> {
    let dir = skills.join(name);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(_) => return Ok(Removal::Left("not_a_folder")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Removal::Absent),
        Err(error) => return Err(io_failed("look at", &dir, &error)),
    }
    let file = dir.join("SKILL.md");
    match std::fs::symlink_metadata(&file) {
        Ok(meta) if meta.file_type().is_file() && file_sha(&file).as_deref() == Some(written_sha) => {
            std::fs::remove_file(&file).map_err(|error| io_failed("remove", &file, &error))?;
        }
        Ok(_) => return Ok(Removal::Left("changed")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_failed("look at", &file, &error)),
    }
    if let Ok(read) = std::fs::read_dir(&dir) {
        for entry in read.flatten().filter(|entry| is_partial(&entry.file_name())) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    match std::fs::remove_dir(&dir) {
        Ok(()) => Ok(Removal::Removed),
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(Removal::Left("has_other_files")),
        Err(error) => Err(io_failed("remove", &dir, &error)),
    }
}

/// The waits for Gateway's rate limit in one launch: how long they came to, and whether the launch said so.
struct RateWaits<'a> {
    io: &'a dyn Io,
    waited: Duration,
    said: bool,
    /// `NEXUS_SKILLS_RATE_WAIT_MS`: each wait sleeps this long instead of the `Retry-After` (tests). The bounds still
    /// count Gateway's seconds.
    sleep: Option<Duration>,
}

impl RateWaits<'_> {
    /// The wait for a refused export: a 429 `rate_limited` with a `Retry-After` within [`RATE_WAIT_MAX`], while the
    /// launch's waits stay within [`RATE_WAIT_TOTAL`]. `None`: the refusal stops the launch.
    fn allow(&self, error: &Error) -> Option<Duration> {
        let Error::Core(core) = error else { return None };
        if core.status != 429 || core.code.as_deref() != Some("rate_limited") {
            return None;
        }
        // A `Retry-After` of 0 counts as a second, so the waits always reach the bound.
        let wait = Duration::from_secs(core.interval? as u64).max(Duration::from_secs(1));
        (wait <= RATE_WAIT_MAX && self.waited + wait <= RATE_WAIT_TOTAL).then_some(wait)
    }

    fn wait(&mut self, wait: Duration) {
        if !self.said {
            self.io.err(&format!(
                "Skills: Nexus Gateway's rate limit makes the delivery wait {} s.",
                wait.as_secs()
            ));
            self.said = true;
        }
        self.waited += wait;
        std::thread::sleep(self.sleep.unwrap_or(wait));
    }
}

/// The file Gateway stored for a version, byte for byte. A 429 `rate_limited` waits and asks again ([`RateWaits`]). The
/// lock is touched before each try, so it stays this launch's while it waits.
fn export(core: &Client, lock: &Lock, waits: &mut RateWaits, id: &str, version: i64) -> Result<Vec<u8>> {
    loop {
        lock.touch()?;
        match core.download(&format!("/v1/skills/{}/versions/{version}/export", enc(id))) {
            Ok(Download::File(file)) => return Ok(file.body),
            Ok(Download::Cut(_) | Download::TimedOut) => {
                return Err(CliError::hint(
                    "A skill file was cut: Nexus Gateway did not send all of it.",
                    format!("Run the command again. {NO_SKILLS}"),
                )
                .into());
            }
            Err(error) => match waits.allow(&error) {
                Some(wait) => waits.wait(wait),
                None => return Err(stopped(error)),
            },
        }
    }
}

// --------------------------------------------------------------------------------------------------------- delivery

/// A refusal that names no scope.
fn refused(name: &str, code: &str) -> Refused {
    Refused {
        name: name.to_string(),
        code: code.to_string(),
        scopes: Vec::new(),
    }
}

/// The person's own skills of Gateway's list, in the order given. A list or a skill that is not the shape Gateway
/// documents is an error.
fn parse_list(list: &Value) -> Result<Vec<Listed>> {
    let Some(items) = list.get("skills").and_then(Value::as_array) else {
        return Err(malformed("skills list", "there is no `skills` list"));
    };
    let mut skills = Vec::with_capacity(items.len());
    for item in items {
        let Some(name) = s(item, "name") else {
            return Err(malformed("skills list", "a skill has no name"));
        };
        if !valid_name(name) {
            // Refused by name, so it needs nothing else.
            skills.push(Listed {
                name: name.to_string(),
                id: String::new(),
                version: 0,
                scope: Scope::Personal,
            });
            continue;
        }
        let (Some(id), Some(latest)) = (
            s(item, "id"),
            as_integer(item.get("latestVersion")).filter(|latest| *latest >= 1),
        ) else {
            return Err(malformed(
                "skills list",
                &format!("the skill {name} has no id or no latestVersion"),
            ));
        };
        skills.push(Listed {
            name: name.to_string(),
            id: id.to_string(),
            version: latest,
            scope: Scope::Personal,
        });
    }
    Ok(skills)
}

/// The skills of the library list, in the order given. A list or a skill that is not the shape Gateway documents is an
/// error: a library nobody could read is never taken for "no library", because that would remove folders.
fn parse_library(list: &Value) -> Result<Vec<Shared>> {
    let Some(items) = list.get("skills").and_then(Value::as_array) else {
        return Err(malformed("skill library", "there is no `skills` list"));
    };
    let mut skills = Vec::with_capacity(items.len());
    for item in items {
        let Some(name) = s(item, "name") else {
            return Err(malformed("skill library", "a skill has no name"));
        };
        if !valid_name(name) {
            // Refused by name, so it needs nothing else; its scope is never read.
            skills.push(Shared {
                skill: Listed {
                    name: name.to_string(),
                    id: String::new(),
                    version: 0,
                    scope: Scope::Org,
                },
                marked: false,
            });
            continue;
        }
        let version = as_integer(item.get("version")).filter(|version| *version >= 1);
        let scope = match (s(item, "scope"), s(item, "teamId").filter(|team| !team.is_empty())) {
            (Some("org"), _) => Some(Scope::Org),
            (Some("team"), Some(team)) => Some(Scope::Team(team.to_string())),
            _ => None,
        };
        let marked = match item.get("duplicateName") {
            None | Some(Value::Null) => Some(false),
            Some(Value::Bool(marked)) => Some(*marked),
            Some(_) => None,
        };
        let (Some(id), Some(version), Some(scope), Some(marked)) = (s(item, "id"), version, scope, marked) else {
            return Err(malformed(
                "skill library",
                &format!("the skill {name} has no id, version or scope, or a duplicateName that is not true or false"),
            ));
        };
        skills.push(Shared {
            skill: Listed {
                name: name.to_string(),
                id: id.to_string(),
                version,
                scope,
            },
            marked,
        });
    }
    Ok(skills)
}

/// What the person's skills and the library come to, by name and in name order (a name never shadows).
/// - A name only the person has is delivered, as before the library.
/// - A name with one library skill that Gateway did not mark is delivered from the library. A personal skill of that
///   name is dropped without a word: Gateway leaves the library skill unmarked only when that skill has the library's
///   bytes.
/// - A name with a marked library skill, or with more than one library skill, is delivered by none. The person's skill of
///   that name is one of those kept out. The plan lists where each skill comes from, the library's in the order given and
///   the person's last.
///
/// Neither list carries a SHA-256, so the marker is all there is to tell identical copies from different ones.
fn merge(own: Vec<Listed>, library: Vec<Shared>) -> Vec<Plan> {
    let mut names: BTreeMap<String, (Vec<Shared>, Vec<Listed>)> = BTreeMap::new();
    for shared in library {
        names.entry(shared.skill.name.clone()).or_default().0.push(shared);
    }
    for skill in own {
        names.entry(skill.name.clone()).or_default().1.push(skill);
    }
    let mut plans = Vec::new();
    for (name, (library, own)) in names {
        if library.is_empty() || !valid_name(&name) {
            // Nothing in the library to collide with, or no name at all: each skill goes on to be delivered or refused.
            plans.extend(library.into_iter().map(|shared| Plan::Deliver(shared.skill)));
            plans.extend(own.into_iter().map(Plan::Deliver));
        } else if library.len() > 1 || library.iter().any(|shared| shared.marked) {
            let mut scopes: Vec<Scope> = library.into_iter().map(|shared| shared.skill.scope).collect();
            if !own.is_empty() {
                scopes.push(Scope::Personal);
            }
            plans.push(Plan::Collide { name, scopes });
        } else {
            plans.extend(library.into_iter().map(|shared| Plan::Deliver(shared.skill)));
        }
    }
    plans
}

/// Writes each skill that is not current and removes what Nexus wrote for the skills that are gone. A name that other
/// skills have is delivered by none, so what Nexus wrote under it goes too. It notes every change in `entries`, and saves
/// the manifest before it writes a folder.
#[allow(clippy::too_many_arguments)]
fn apply(
    core: &Client,
    lock: &Lock,
    waits: &mut RateWaits,
    skills_dir: &Path,
    plans: &[Plan],
    manifest: &Manifest,
    entries: &mut BTreeMap<String, Entry>,
    delivery: &mut Delivery,
) -> Result<()> {
    let mut kept: BTreeSet<&str> = BTreeSet::new();
    for plan in plans {
        let skill = match plan {
            Plan::Deliver(skill) => skill,
            Plan::Collide { name, scopes } => {
                delivery.refused.push(Refused {
                    name: name.clone(),
                    code: "duplicate_name".into(),
                    scopes: scopes.clone(),
                });
                continue;
            }
        };
        if !valid_name(&skill.name) {
            delivery.refused.push(refused(&skill.name, "invalid_name"));
            continue;
        }
        let target = skills_dir.join(&skill.name);
        let on_disk = std::fs::symlink_metadata(&target);
        let listed = entries.get(&skill.name).cloned();
        if listed.is_none() && on_disk.is_ok() {
            delivery.refused.push(refused(&skill.name, "duplicate_name"));
            continue;
        }
        if on_disk.as_ref().is_ok_and(|meta| !meta.file_type().is_dir()) {
            // A link or a file stands where Nexus wrote a folder. It is the person's now, and no write goes through it.
            entries.remove(&skill.name);
            delivery.refused.push(refused(&skill.name, "duplicate_name"));
            continue;
        }
        kept.insert(skill.name.as_str());
        let current = listed.as_ref().is_some_and(|entry| {
            entry.skill_id == skill.id
                && entry.version == skill.version
                && file_sha(&target.join("SKILL.md")).as_deref() == Some(entry.sha256.as_str())
        });
        if !current {
            let bytes = export(core, lock, waits, &skill.id, skill.version)?;
            // Listed first: a launch that dies while it writes the folder leaves a folder the next launch knows.
            entries.insert(
                skill.name.clone(),
                Entry {
                    skill_id: skill.id.clone(),
                    version: skill.version,
                    sha256: sha256_hex(&[bytes.as_slice()]),
                },
            );
            manifest.write(entries)?;
            write_skill(skills_dir, &skill.name, &bytes)?;
            delivery.written.push(skill.name.clone());
        }
        delivery.present.push(Present {
            name: skill.name.clone(),
            skill_id: skill.id.clone(),
            version: skill.version,
            scope: skill.scope.clone(),
        });
    }

    // What Nexus wrote for this person, whose skill is gone. A name leaves the manifest only after its files are dealt with.
    let gone: Vec<String> = entries
        .keys()
        .filter(|name| !kept.contains(name.as_str()))
        .cloned()
        .collect();
    for name in gone {
        let written_sha = entries[&name].sha256.clone();
        let removal = remove_skill(skills_dir, &name, &written_sha)?;
        entries.remove(&name);
        match removal {
            Removal::Removed => delivery.removed.push(name),
            Removal::Left(why) => delivery.left.push((name, why.into())),
            Removal::Absent => {}
        }
    }
    Ok(())
}

/// Writes the person's skills and the library into the agent's skills folder. `None`: nothing to do (a Gateway with no
/// skills, a key that belongs to no person, or no skills and nothing delivered before).
pub fn deliver(ctx: &Context, core: &Client, agent: Agent) -> Result<Option<Delivery>> {
    // An agent whose entry names no skills folder (Antigravity) gets nothing written, and Gateway is not asked.
    let Some(skills_folder) = agent.entry().profile.skills else {
        return Ok(None);
    };
    let list = match core.get("/v1/skills") {
        Ok(list) => list,
        Err(Error::Core(refusal)) if refusal.status == 404 || refusal.status == 403 => return Ok(None),
        Err(error) => return Err(stopped(error)),
    };
    let own = parse_list(&list)?;
    // The library list is read as the first is: a Gateway without the route (404) and a key that may not hold skills (403)
    // have no library. Any other answer stops the launch, because a library nobody could read is not "no library".
    let library = match core.get("/v1/skill-library") {
        Ok(list) => parse_library(&list)?,
        Err(Error::Core(refusal)) if refusal.status == 404 || refusal.status == 403 => Vec::new(),
        Err(error) => return Err(stopped(error)),
    };
    let plans = merge(own, library);

    // With no skills there is nothing to write, and no folder to look in when there is none to name.
    let folder = match agent_folder(ctx, agent) {
        Ok(folder) => folder,
        Err(_) if plans.is_empty() => return Ok(None),
        Err(error) => return Err(error),
    };
    let state = state_dir(ctx, &folder);
    if plans.is_empty() && manifest_names(&state).is_empty() {
        return Ok(None);
    }
    let me = core.get("/v1/tenants/me").map_err(stopped)?;
    let user_id = me
        .pointer("/user/id")
        .and_then(Value::as_str)
        .filter(|id| {
            !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .ok_or_else(|| {
            Error::from(CliError::hint(
                "Nexus Gateway did not say who you are, so it cannot tell which skills to deliver.",
                NO_SKILLS,
            ))
        })?
        .to_string();

    let wait = ctx
        .var("NEXUS_SKILLS_LOCK_WAIT_MS")
        .and_then(|ms| ms.parse().ok())
        .map_or(LOCK_WAIT, Duration::from_millis);
    let lock = Lock::take(&state, wait, &folder)?;
    let skills_dir = folder.join(skills_folder);
    let mut delivery = Delivery {
        folder: skills_dir.clone(),
        ..Delivery::default()
    };

    // Other people's files first: a name they share with this person is theirs to clear before it is delivered.
    let own_manifest = state.join(format!("{user_id}.json"));
    for path in manifest_names(&state) {
        if path == own_manifest {
            continue;
        }
        let other = Manifest {
            user_id: path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
            folder: folder.clone(),
            skills_dir: skills_dir.clone(),
            path: path.clone(),
        };
        for (name, entry) in other.read()? {
            match remove_skill(&skills_dir, &name, &entry.sha256)? {
                Removal::Removed => delivery.others_removed.push(name),
                Removal::Left(why) => delivery.left.push((name, why.into())),
                Removal::Absent => {}
            }
        }
        std::fs::remove_file(&path).map_err(|error| io_failed("remove", &path, &error))?;
    }

    let manifest = Manifest {
        path: own_manifest,
        user_id,
        folder,
        skills_dir: skills_dir.clone(),
    };
    let mut entries = manifest.read()?;
    let before = entries.clone();
    let mut waits = RateWaits {
        io: ctx.io.as_ref(),
        waited: Duration::ZERO,
        said: false,
        sleep: ctx
            .var("NEXUS_SKILLS_RATE_WAIT_MS")
            .and_then(|ms| ms.parse().ok())
            .map(Duration::from_millis),
    };
    let applied = apply(
        core,
        &lock,
        &mut waits,
        &skills_dir,
        &plans,
        &manifest,
        &mut entries,
        &mut delivery,
    );
    // The manifest says what is in the folder, so it is written when a step failed too: the next run then knows the
    // folders this run made, and does not take them for someone else's.
    if entries != before {
        let saved = manifest.write(&entries);
        if applied.is_ok() {
            saved?;
        }
    }
    applied?;
    drop(lock);
    Ok(Some(delivery))
}

/// Tells Gateway what was delivered to the agent session, one record for each skill in the folder. A delivery of more than
/// [`MAX_LOADS_A_CALL`] skills goes in several calls.
pub fn record(core: &Client, agent: Agent, session_id: &str, delivery: &Delivery) -> Result<()> {
    for chunk in delivery.present.chunks(MAX_LOADS_A_CALL) {
        let skills: Vec<Value> = chunk
            .iter()
            .map(|skill| json!({ "skillId": skill.skill_id, "version": skill.version }))
            .collect();
        core.post(
            "/v1/skill-loads",
            json!({ "surface": format!("nexus_{agent}"), "sessionId": session_id, "skills": skills }),
        )
        .map_err(report_failed)?;
    }
    Ok(())
}

/// The one line before the agent starts: what is in the folder, with versions; what was not delivered, with codes. Once
/// the library takes part (a library skill is delivered, or a name collides with one), each skill names its scope, and a
/// name that other skills have names the scope of each. A launch without the library names no scope.
pub fn line(style: &Style, agent: Agent, delivery: &Delivery) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let scoped = delivery.present.iter().any(|skill| skill.scope != Scope::Personal)
        || delivery.refused.iter().any(|refused| !refused.scopes.is_empty());
    if !delivery.present.is_empty() {
        let shown: Vec<String> = delivery
            .present
            .iter()
            .take(SHOWN)
            .map(|skill| {
                if scoped {
                    format!("{} v{} ({})", skill.name, skill.version, skill.scope.words())
                } else {
                    format!("{} v{}", skill.name, skill.version)
                }
            })
            .collect();
        let more = delivery.present.len().saturating_sub(SHOWN);
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        let count = delivery.present.len();
        parts.push(format!(
            "{count} {} delivered: {}{tail}",
            if count == 1 { "skill" } else { "skills" },
            shown.join(", ")
        ));
    }
    if !delivery.refused.is_empty() {
        let refused: Vec<String> = delivery
            .refused
            .iter()
            .map(|refused| {
                if refused.scopes.is_empty() {
                    format!("{} ({})", refused.name, refused.code)
                } else {
                    let scopes: Vec<String> = refused.scopes.iter().map(Scope::words).collect();
                    format!("{} ({}: {})", refused.name, refused.code, scopes.join(" and "))
                }
            })
            .collect();
        parts.push(format!("not delivered: {}", refused.join(", ")));
        // A refusal with no scope is a folder (or a link, or a file) of that name that Nexus did not write. Nexus never
        // writes into it, so the line says what frees the name.
        let folders: Vec<&str> = delivery
            .refused
            .iter()
            .filter(|refused| refused.code == "duplicate_name" && refused.scopes.is_empty())
            .map(|refused| refused.name.as_str())
            .collect();
        if !folders.is_empty() {
            let more = folders.len().saturating_sub(SHOWN);
            let tail = if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            };
            parts.push(format!(
                "move or delete a folder in {} to receive its skill: {}{tail}",
                delivery.folder.display(),
                folders.iter().take(SHOWN).copied().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    if !delivery.removed.is_empty() {
        parts.push(format!("removed: {}", delivery.removed.join(", ")));
    }
    if !delivery.left.is_empty() {
        let left: Vec<String> = delivery
            .left
            .iter()
            .map(|(name, code)| format!("{name} ({code})"))
            .collect();
        parts.push(format!("left in place: {}", left.join(", ")));
    }
    if !delivery.others_removed.is_empty() {
        let variable = agent.entry().profile.var.unwrap_or("HOME");
        parts.push(format!(
            "removed {} from another person on this folder (give each person their own {variable})",
            delivery.others_removed.join(", ")
        ));
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!("Skills: {}", parts.join(&format!(" {} ", style.bullet))))
}

/// What `--json` adds to the summary. A delivered skill names its `scope` (`personal`, `org` or `team`, with its `teamId`),
/// and a refusal for a name that other skills have lists the `scopes` of each.
pub fn json_of(delivery: &Delivery) -> Value {
    json!({
        "folder": delivery.folder.display().to_string(),
        "delivered": delivery.present.iter().map(|skill| {
            let mut entry = json!({ "name": skill.name, "skillId": skill.skill_id, "version": skill.version });
            if let (Some(entry), Some(scope)) = (entry.as_object_mut(), skill.scope.json().as_object()) {
                entry.extend(scope.clone());
            }
            entry
        }).collect::<Vec<_>>(),
        "written": delivery.written,
        "refused": delivery.refused.iter().map(|refused| {
            let mut entry = json!({ "name": refused.name, "code": refused.code });
            if !refused.scopes.is_empty() {
                entry["scopes"] = Value::Array(refused.scopes.iter().map(Scope::json).collect());
            }
            entry
        }).collect::<Vec<_>>(),
        "removed": delivery.removed,
        "left": delivery.left.iter().map(|(name, code)| json!({ "name": name, "code": code })).collect::<Vec<_>>(),
        "othersRemoved": delivery.others_removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_made_absolute_by_name_alone() {
        let base = Path::new("/work/app");
        assert_eq!(
            absolute(base, Path::new("/home/ada/.claude")),
            PathBuf::from("/home/ada/.claude")
        );
        assert_eq!(
            absolute(base, Path::new("cfg/../.claude/")),
            PathBuf::from("/work/app/.claude")
        );
        assert_eq!(absolute(base, Path::new("./a/./b")), PathBuf::from("/work/app/a/b"));
        assert_eq!(absolute(base, Path::new("../../..")), PathBuf::from("/"));
    }

    #[test]
    fn an_integer_is_read_as_javascript_reads_it() {
        assert_eq!(as_integer(Some(&json!(3))), Some(3));
        assert_eq!(as_integer(Some(&json!(1.0))), Some(1));
        assert_eq!(as_integer(Some(&json!(1.5))), None);
        assert_eq!(as_integer(Some(&json!("1"))), None);
        assert_eq!(as_integer(Some(&json!(null))), None);
        assert_eq!(as_integer(None), None);
    }

    #[test]
    fn a_lock_file_names_its_process_and_token() {
        assert_eq!(lock_pid("4242\nabc\n"), Some(4242));
        assert_eq!(lock_token("4242\nabc\n"), Some("abc"));
        // A lock that is empty, half written or not a number names nobody, so only its age can make it stale.
        assert_eq!(lock_pid(""), None);
        assert_eq!(lock_pid("soon\n"), None);
        assert_eq!(lock_pid("0\nabc\n"), None);
        assert_eq!(lock_pid("-1\nabc\n"), None);
        assert_eq!(lock_token("4242\n"), None);
    }

    /// A folder of its own for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nexus-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn a_launch_releases_only_a_lock_that_is_still_its_own() {
        let dir = scratch("release");
        let path = dir.join("lock");
        let lock = Lock::take(&dir, Duration::ZERO, Path::new("/agent")).expect("lock");
        assert!(lock.touch().is_ok());
        assert_eq!(
            lock_token(&std::fs::read_to_string(&path).expect("lock file")),
            Some(lock.token.as_str())
        );
        drop(lock);
        assert!(!path.exists(), "a lock that is still ours goes");

        // Another launch took the lock over: this one finds out when it touches the lock, and leaves it alone.
        let lock = Lock::take(&dir, Duration::ZERO, Path::new("/agent")).expect("lock");
        std::fs::write(&path, "4242\nsomeone-else\n").expect("taken over");
        assert!(lock.touch().is_err());
        drop(lock);
        assert_eq!(std::fs::read_to_string(&path).expect("kept"), "4242\nsomeone-else\n");
        std::fs::remove_dir_all(&dir).expect("clean");
    }

    #[test]
    fn a_takeover_puts_back_a_lock_it_did_not_judge() {
        let dir = scratch("takeover");
        let path = dir.join("lock");
        // The lock that was judged stale was replaced by a fresh one before the file was moved.
        std::fs::write(&path, "4242\nfresh\n").expect("fresh");
        assert!(!take_over(&dir, &path, "99999\nold\n"));
        assert_eq!(std::fs::read_to_string(&path).expect("put back"), "4242\nfresh\n");
        // The lock that was judged goes.
        assert!(take_over(&dir, &path, "4242\nfresh\n"));
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(&dir).expect("dir").count(), 0, "no leftovers");
        // A lock that is not there is not taken over.
        assert!(!take_over(&dir, &path, ""));
        std::fs::remove_dir_all(&dir).expect("clean");
    }

    #[test]
    fn the_line_names_versions_and_codes_and_cuts_a_long_list() {
        let style = Style::new(false);
        let present: Vec<Present> = (1..=10)
            .map(|n| Present {
                name: format!("skill-{n}"),
                skill_id: format!("skl_{n}"),
                version: n,
                scope: Scope::Personal,
            })
            .collect();
        let delivery = Delivery {
            present,
            refused: vec![refused("pdf", "duplicate_name")],
            removed: vec!["old".into()],
            left: vec![("mine".into(), "changed".into())],
            ..Delivery::default()
        };
        let text = line(&style, Agent::Claude, &delivery).expect("a line");
        assert!(
            text.starts_with("Skills: 10 skills delivered: skill-1 v1, skill-2 v2,"),
            "{text}"
        );
        assert!(text.contains("skill-8 v8 and 2 more"), "{text}");
        assert!(text.contains("not delivered: pdf (duplicate_name)"), "{text}");
        assert!(text.ends_with("removed: old · left in place: mine (changed)"), "{text}");
        assert_eq!(line(&style, Agent::Codex, &Delivery::default()), None);
    }

    #[test]
    fn the_line_names_scopes_once_the_library_takes_part() {
        let style = Style::new(false);
        let present = |name: &str, scope: Scope| Present {
            name: name.into(),
            skill_id: format!("skl_{name}"),
            version: 2,
            scope,
        };
        // Only the person's own skills: the line of a launch before the library.
        let own = Delivery {
            present: vec![present("a", Scope::Personal), present("b", Scope::Personal)],
            ..Delivery::default()
        };
        assert_eq!(
            line(&style, Agent::Claude, &own).as_deref(),
            Some("Skills: 2 skills delivered: a v2, b v2")
        );
        // A library skill: each skill names its scope.
        let mixed = Delivery {
            present: vec![
                present("a", Scope::Personal),
                present("b", Scope::Org),
                present("c", Scope::Team("team_x".into())),
            ],
            ..Delivery::default()
        };
        assert_eq!(
            line(&style, Agent::Claude, &mixed).as_deref(),
            Some("Skills: 3 skills delivered: a v2 (personal), b v2 (organization), c v2 (team team_x)")
        );
        // A name that other skills have: the scope of each, and the person's own skills name theirs too.
        let collision = Delivery {
            present: vec![present("a", Scope::Personal)],
            refused: vec![
                Refused {
                    name: "deploy".into(),
                    code: "duplicate_name".into(),
                    scopes: vec![Scope::Org, Scope::Team("team_x".into()), Scope::Personal],
                },
                refused("pdf", "duplicate_name"),
            ],
            folder: PathBuf::from("/agent/skills"),
            ..Delivery::default()
        };
        // Only `pdf` is a folder that keeps a skill out, so only `pdf` gets the way to free the name.
        assert_eq!(
            line(&style, Agent::Claude, &collision).as_deref(),
            Some(
                "Skills: 1 skill delivered: a v2 (personal) · not delivered: deploy (duplicate_name: organization and team team_x and personal), pdf (duplicate_name) · move or delete a folder in /agent/skills to receive its skill: pdf"
            )
        );
    }

    #[test]
    fn a_folder_that_keeps_a_skill_out_says_how_to_free_the_name() {
        let style = Style::new(false);
        let delivery = |names: &[&str]| Delivery {
            folder: PathBuf::from("/agent/skills"),
            refused: names.iter().map(|name| refused(name, "duplicate_name")).collect(),
            ..Delivery::default()
        };
        assert_eq!(
            line(&style, Agent::Claude, &delivery(&["deploy"])).as_deref(),
            Some(
                "Skills: not delivered: deploy (duplicate_name) · move or delete a folder in /agent/skills to receive its skill: deploy"
            )
        );
        // The advice names the first eight, as the delivered list does.
        let many: Vec<String> = (1..=10).map(|n| format!("skill-{n}")).collect();
        let names: Vec<&str> = many.iter().map(String::as_str).collect();
        let text = line(&style, Agent::Claude, &delivery(&names)).expect("a line");
        assert!(
            text.ends_with("to receive its skill: skill-1, skill-2, skill-3, skill-4, skill-5, skill-6, skill-7, skill-8 and 2 more"),
            "{text}"
        );
        // A bad name is not a folder of that name, and gets no advice.
        let invalid = Delivery {
            refused: vec![refused("Bad Name", "invalid_name")],
            ..Delivery::default()
        };
        assert_eq!(
            line(&style, Agent::Claude, &invalid).as_deref(),
            Some("Skills: not delivered: Bad Name (invalid_name)")
        );
    }

    #[test]
    fn the_summary_names_scopes_and_leaves_the_rest_as_it_was() {
        let delivery = Delivery {
            present: vec![
                Present {
                    name: "a".into(),
                    skill_id: "skl_a".into(),
                    version: 1,
                    scope: Scope::Personal,
                },
                Present {
                    name: "b".into(),
                    skill_id: "skl_b".into(),
                    version: 3,
                    scope: Scope::Team("team_x".into()),
                },
            ],
            refused: vec![
                refused("pdf", "duplicate_name"),
                Refused {
                    name: "deploy".into(),
                    code: "duplicate_name".into(),
                    scopes: vec![Scope::Org, Scope::Personal],
                },
            ],
            ..Delivery::default()
        };
        let summary = json_of(&delivery);
        assert_eq!(
            summary["delivered"],
            json!([
                { "name": "a", "skillId": "skl_a", "version": 1, "scope": "personal" },
                { "name": "b", "skillId": "skl_b", "version": 3, "scope": "team", "teamId": "team_x" },
            ])
        );
        assert_eq!(
            summary["refused"],
            json!([
                { "name": "pdf", "code": "duplicate_name" },
                { "name": "deploy", "code": "duplicate_name", "scopes": [{ "scope": "org" }, { "scope": "personal" }] },
            ])
        );
    }

    fn mine(name: &str) -> Listed {
        Listed {
            name: name.into(),
            id: format!("skl_mine_{name}"),
            version: 1,
            scope: Scope::Personal,
        }
    }

    fn shared(name: &str, scope: Scope, marked: bool) -> Shared {
        let team = match &scope {
            Scope::Team(id) => id.clone(),
            _ => "org".into(),
        };
        Shared {
            skill: Listed {
                name: name.into(),
                id: format!("skl_{team}_{name}"),
                version: 4,
                scope,
            },
            marked,
        }
    }

    /// What `merge` delivers, as `id` and the scope's words, and what it keeps out, as `name:` and the scopes' words.
    fn outcome(plans: &[Plan]) -> Vec<String> {
        plans
            .iter()
            .map(|plan| match plan {
                Plan::Deliver(skill) => format!("{} {}", skill.id, skill.scope.words()),
                Plan::Collide { name, scopes } => {
                    let scopes: Vec<String> = scopes.iter().map(Scope::words).collect();
                    format!("{name}: {}", scopes.join(" + "))
                }
            })
            .collect()
    }

    #[test]
    fn the_person_alone_and_the_library_alone_are_delivered_in_name_order() {
        assert_eq!(
            outcome(&merge(vec![mine("b"), mine("a")], Vec::new())),
            ["skl_mine_a personal", "skl_mine_b personal"]
        );
        assert_eq!(
            outcome(&merge(
                Vec::new(),
                vec![
                    shared("z", Scope::Org, false),
                    shared("y", Scope::Team("t1".into()), false)
                ]
            )),
            ["skl_t1_y team t1", "skl_org_z organization"]
        );
        assert!(merge(Vec::new(), Vec::new()).is_empty());
    }

    #[test]
    fn skills_with_different_names_are_all_delivered() {
        let plans = merge(
            vec![mine("changelog"), mine("pdf")],
            vec![
                shared("deploy", Scope::Org, false),
                shared("review", Scope::Team("t1".into()), false),
            ],
        );
        assert_eq!(
            outcome(&plans),
            [
                "skl_mine_changelog personal",
                "skl_org_deploy organization",
                "skl_mine_pdf personal",
                "skl_t1_review team t1",
            ]
        );
    }

    #[test]
    fn a_marked_name_is_delivered_by_neither_skill() {
        // The library skill is marked and the person has a skill of the name: neither, both reported.
        let plans = merge(
            vec![mine("deploy"), mine("pdf")],
            vec![shared("deploy", Scope::Org, true)],
        );
        assert_eq!(
            outcome(&plans),
            ["deploy: organization + personal", "skl_mine_pdf personal"]
        );
        // Sibling teams: both marked, the person has none of the name.
        let plans = merge(
            Vec::new(),
            vec![
                shared("review", Scope::Team("t1".into()), true),
                shared("review", Scope::Team("t2".into()), true),
            ],
        );
        assert_eq!(outcome(&plans), ["review: team t1 + team t2"]);
        // A marked skill stays out when the person's skill of the name is gone from their list: Gateway answered the two
        // lists a moment apart, and the next launch delivers it.
        assert_eq!(
            outcome(&merge(Vec::new(), vec![shared("deploy", Scope::Org, true)])),
            ["deploy: organization"]
        );
    }

    #[test]
    fn an_unmarked_library_skill_is_served_and_the_personal_skill_of_its_name_is_dropped() {
        let plans = merge(vec![mine("deploy")], vec![shared("deploy", Scope::Org, false)]);
        assert_eq!(outcome(&plans), ["skl_org_deploy organization"]);
    }

    #[test]
    fn two_library_skills_of_one_name_collide_whatever_the_marker_says() {
        let plans = merge(
            vec![mine("review")],
            vec![
                shared("review", Scope::Org, false),
                shared("review", Scope::Team("t1".into()), false),
            ],
        );
        assert_eq!(outcome(&plans), ["review: organization + team t1 + personal"]);
    }

    #[test]
    fn a_name_that_is_not_a_skill_name_goes_on_to_be_refused() {
        let plans = merge(
            vec![mine("../evil")],
            vec![shared("../evil", Scope::Org, true), shared("ok", Scope::Org, false)],
        );
        // Both are handed on to be refused by name, and neither collides with the other.
        assert_eq!(
            outcome(&plans),
            [
                "skl_org_../evil organization",
                "skl_mine_../evil personal",
                "skl_org_ok organization"
            ]
        );
    }

    #[test]
    fn the_library_list_is_read_as_documented_or_not_at_all() {
        let list = json!({ "skills": [
            { "id": "skl_a", "name": "deploy", "description": "d", "scope": "org", "version": 3, "approvedAt": "x", "duplicateName": true },
            { "id": "skl_b", "name": "review", "scope": "team", "teamId": "team_x", "version": 1 },
            { "name": "../evil" },
        ] });
        let found = parse_library(&list).expect("a list");
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].skill.scope, Scope::Org);
        assert!(found[0].marked);
        assert_eq!(found[0].skill.version, 3);
        assert_eq!(found[1].skill.scope, Scope::Team("team_x".into()));
        assert!(!found[1].marked);
        assert_eq!(
            found[2].skill.id, "",
            "a name that is not a skill name carries nothing else"
        );
        let bad = |skill: Value| parse_library(&json!({ "skills": [skill] })).is_err();
        assert!(parse_library(&json!({})).is_err());
        assert!(parse_library(&json!({ "skills": {} })).is_err());
        assert!(bad(json!({ "id": "skl_a", "scope": "org", "version": 1 })), "no name");
        assert!(bad(json!({ "name": "a", "scope": "org", "version": 1 })), "no id");
        assert!(bad(json!({ "id": "skl_a", "name": "a", "scope": "org" })), "no version");
        assert!(
            bad(json!({ "id": "skl_a", "name": "a", "scope": "org", "version": 0 })),
            "version 0"
        );
        assert!(
            bad(json!({ "id": "skl_a", "name": "a", "scope": "org", "version": 1.5 })),
            "fractional"
        );
        assert!(bad(json!({ "id": "skl_a", "name": "a", "version": 1 })), "no scope");
        assert!(
            bad(json!({ "id": "skl_a", "name": "a", "scope": "all", "version": 1 })),
            "unknown scope"
        );
        assert!(
            bad(json!({ "id": "skl_a", "name": "a", "scope": "team", "version": 1 })),
            "team without id"
        );
        assert!(bad(
            json!({ "id": "skl_a", "name": "a", "scope": "team", "teamId": "", "version": 1 })
        ));
        assert!(bad(
            json!({ "id": "skl_a", "name": "a", "scope": "org", "version": 1, "duplicateName": "yes" })
        ));
        assert!(parse_library(&json!({ "skills": [{ "id": "skl_a", "name": "a", "scope": "org", "version": 1, "duplicateName": null }] })).is_ok());
    }
}
