//! The person's own 1Password, through the 1Password CLI (`op`) with app integration.
//!
//! No service account, no admin and no install: Nexus uses the `op` the person already has, and 1Password's own app
//! asks for Touch ID. Nexus lists items' metadata only (title, site, username hint, vault). It reads a password only
//! through [`read_secret`], which needs an [`Authorized`] and is called for an approved grant, on this device.
//! Nothing here installs `op` or changes 1Password's settings: when `op` is missing or app integration is off, the
//! person reads [`STEPS`].

use std::io::Read as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::agent_vault::ManagerKind;
use crate::agent_vault::vault_id;
use crate::vault_local::{Authorized, LoginMeta, Secret, hint, op_login_id, site_of};

/// The steps the person takes when 1Password is not connected.
pub const STEPS: [&str; 3] = [
    "Install the 1Password CLI (`op`) from 1password.com/downloads/command-line.",
    "In the 1Password app, open Settings, then Developer, and turn on “Integrate with 1Password CLI”.",
    "Come back here and choose Check again. Nexus never installs anything and never asks for your 1Password password.",
];

/// How `op` stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpState {
    /// No `op` found.
    NotInstalled,
    /// `op` is there, and the 1Password app's CLI integration is off or has no account.
    IntegrationOff,
    /// `op` answers for this many accounts.
    Ready { accounts: usize },
    /// `op` is there and failed in some other way. The text is `op`'s own message, never an item.
    Failed(String),
}

/// What one `op` call gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Out {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `op`, behind a trait so a test supplies the answers and no process starts.
pub trait Run: Send + Sync {
    /// Run `op` with `args`. `Err` when it could not start or did not end in time.
    fn run(&self, args: &[&str]) -> Result<Out, String>;
}

/// The real `op`: found on `PATH` and in the places a Mac puts it, since a Mac app's `PATH` is short.
pub struct OsOp {
    path: PathBuf,
}

impl OsOp {
    pub fn find() -> Option<OsOp> {
        let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).map(|dir| dir.join("op")).collect())
            .unwrap_or_default();
        candidates.extend(["/opt/homebrew/bin/op", "/usr/local/bin/op", "/usr/bin/op"].map(PathBuf::from));
        candidates
            .into_iter()
            .find(|path| path.is_file())
            .map(|path| OsOp { path })
    }
}

impl Run for OsOp {
    fn run(&self, args: &[&str]) -> Result<Out, String> {
        let mut child = nexus_workbench::process::command(&self.path)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("op did not start: {}", error.kind()))?;
        // 1Password's own app asks the person for Touch ID, so the wait is as long as a person takes.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let (mut stdout, mut stderr) = (String::new(), String::new());
                    if let Some(mut pipe) = child.stdout.take() {
                        pipe.read_to_string(&mut stdout).ok();
                    }
                    if let Some(mut pipe) = child.stderr.take() {
                        pipe.read_to_string(&mut stderr).ok();
                    }
                    return Ok(Out {
                        ok: status.success(),
                        stdout,
                        stderr,
                    });
                }
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(40)),
                Ok(None) => {
                    child.kill().ok();
                    child.wait().ok();
                    return Err("op did not answer in time".into());
                }
                Err(error) => return Err(format!("op failed: {}", error.kind())),
            }
        }
    }
}

/// Whether `op` is installed and 1Password's app integration is on.
pub fn detect(run: Option<&dyn Run>) -> OpState {
    let Some(run) = run else { return OpState::NotInstalled };
    if !matches!(run.run(&["--version"]), Ok(out) if out.ok) {
        return OpState::NotInstalled;
    }
    match run.run(&["account", "list", "--format", "json"]) {
        Ok(out) if out.ok => match serde_json::from_str::<Value>(&out.stdout) {
            Ok(Value::Array(accounts)) if !accounts.is_empty() => OpState::Ready {
                accounts: accounts.len(),
            },
            _ => OpState::IntegrationOff,
        },
        Ok(out) if out.stderr.to_ascii_lowercase().contains("no accounts") => OpState::IntegrationOff,
        Ok(out) => OpState::Failed(first_line(&out.stderr)),
        Err(error) => OpState::Failed(error),
    }
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("op failed")
        .trim();
    line.chars().take(160).collect()
}

/// The items `op` lists: metadata, never a password.
pub struct Listed {
    pub logins: Vec<LoginMeta>,
    /// The first vault's name, for the source line.
    pub vault: Option<String>,
}

/// List the person's Login items. Grants and last uses are the device's (`LocalVault::states`), joined by the caller.
pub fn list_items(run: &dyn Run) -> Result<Listed, String> {
    let out = run.run(&["item", "list", "--categories", "Login", "--format", "json"])?;
    if !out.ok {
        return Err(first_line(&out.stderr));
    }
    let items = serde_json::from_str::<Value>(&out.stdout).map_err(|_| "op's list was not JSON".to_string())?;
    let items = items.as_array().ok_or_else(|| "op's list was not a list".to_string())?;
    let mut logins = Vec::new();
    let mut vault = None;
    for item in items {
        let Some(id) = item.get("id").and_then(Value::as_str).and_then(op_login_id) else {
            continue;
        };
        let Some(title) = item
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
        else {
            continue;
        };
        let url = item
            .get("urls")
            .and_then(Value::as_array)
            .and_then(|urls| {
                urls.iter()
                    .find(|u| u.get("primary").and_then(Value::as_bool) == Some(true))
                    .or_else(|| urls.first())
            })
            .and_then(|u| u.get("href"))
            .and_then(Value::as_str);
        // An item with no site is named by its title when that reads as a host.
        let Some(site) = url
            .and_then(site_of)
            .or_else(|| site_of(title).filter(|s| s.contains('.')))
        else {
            continue;
        };
        let username = item
            .get("additional_information")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if vault.is_none() {
            vault = item.pointer("/vault/name").and_then(Value::as_str).map(String::from);
        }
        logins.push(LoginMeta {
            id,
            name: title.chars().take(120).collect(),
            site,
            hint: hint(&username),
            username,
            source: ManagerKind::OnePassword,
            grant: None,
            last_used: None,
            flag: None,
        });
    }
    Ok(Listed { logins, vault })
}

/// Read one item's password. It needs the person's Touch ID ([`Authorized`]), and 1Password asks its own.
pub fn read_secret(run: &dyn Run, _: &Authorized, login_id: &str) -> Result<Secret, String> {
    let item = login_id
        .strip_prefix("op_")
        .filter(|item| vault_id(item) && !item.starts_with('-'))
        .ok_or("Not a 1Password login.")?;
    let out = run.run(&["item", "get", item, "--fields", "label=password", "--reveal"])?;
    if !out.ok {
        // `op`'s own message holds no item, and the secret is only on stdout of a success.
        return Err(first_line(&out.stderr));
    }
    let secret = out.stdout.trim_end_matches(['\n', '\r']).to_string();
    // A concealed field may come quoted by `op`.
    let secret = if secret.len() > 1 && secret.starts_with('"') && secret.ends_with('"') {
        secret[1..secret.len() - 1].to_string()
    } else {
        secret
    };
    if secret.is_empty() {
        Err("The item has no password.".into())
    } else {
        Ok(Secret::new(secret))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::vault_local::Auth;
    use std::sync::Mutex;

    /// Answers by the first words of the call, and records each call's arguments.
    struct Fake {
        answers: Vec<(&'static str, Result<Out, String>)>,
        calls: Mutex<Vec<String>>,
    }

    impl Run for Fake {
        fn run(&self, args: &[&str]) -> Result<Out, String> {
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            self.answers
                .iter()
                .find(|(start, _)| line.starts_with(start))
                .map(|(_, a)| a.clone())
                .unwrap_or(Err("none".into()))
        }
    }

    fn out(ok: bool, stdout: &str, stderr: &str) -> Result<Out, String> {
        Ok(Out {
            ok,
            stdout: stdout.into(),
            stderr: stderr.into(),
        })
    }

    #[test]
    fn detects_each_state_and_installs_nothing() {
        assert_eq!(detect(None), OpState::NotInstalled);
        let missing = Fake {
            answers: vec![("--version", Err("op did not start".into()))],
            calls: Default::default(),
        };
        assert_eq!(detect(Some(&missing)), OpState::NotInstalled);
        let off = Fake {
            answers: vec![
                ("--version", out(true, "2.30\n", "")),
                (
                    "account list",
                    out(false, "", "[ERROR] No accounts configured for use with 1Password CLI."),
                ),
            ],
            calls: Default::default(),
        };
        assert_eq!(detect(Some(&off)), OpState::IntegrationOff);
        let ready = Fake {
            answers: vec![
                ("--version", out(true, "2.30", "")),
                ("account list", out(true, "[{\"url\":\"my.1password.com\"}]", "")),
            ],
            calls: Default::default(),
        };
        assert_eq!(detect(Some(&ready)), OpState::Ready { accounts: 1 });
        assert!(
            ready
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| !call.contains("install") && !call.contains("signin"))
        );
    }

    #[test]
    fn lists_metadata_and_asks_for_no_secret() {
        let json = r#"[{"id":"abcd1234efgh5678ijkl9012mn","title":"Delta","category":"LOGIN","vault":{"id":"v","name":"Personal"},
            "urls":[{"label":"w","primary":true,"href":"https://www.delta.com/login"}],"additional_information":"maya@mail.com"},
            {"id":"zzzzzzzzzzzzzzzzzzzzzzzzzz","title":"No site","vault":{"name":"Personal"}}]"#;
        let fake = Fake {
            answers: vec![("item list", out(true, json, ""))],
            calls: Default::default(),
        };
        let listed = list_items(&fake).unwrap();
        assert_eq!(listed.logins.len(), 1);
        let login = &listed.logins[0];
        assert_eq!(
            (login.site.as_str(), login.hint.as_str(), login.source),
            ("delta.com", "m•••", ManagerKind::OnePassword)
        );
        assert_eq!(listed.vault.as_deref(), Some("Personal"));
        let calls = fake.calls.lock().unwrap();
        assert!(
            calls
                .iter()
                .all(|call| !call.contains("--reveal") && !call.contains("password")),
            "{calls:?}"
        );
    }

    #[test]
    fn a_secret_is_read_for_one_item_after_touch_id_only() {
        let fake = Fake {
            answers: vec![("item get", out(true, "s3cret-from-op\n", ""))],
            calls: Default::default(),
        };
        let auth = Authorized::from(&Auth::Approved).unwrap();
        let secret = read_secret(&fake, &auth, "op_abcd1234efgh5678ijkl9012mn").unwrap();
        assert_eq!(secret.expose(), "s3cret-from-op");
        assert_eq!(
            fake.calls.lock().unwrap()[0],
            "item get abcd1234efgh5678ijkl9012mn --fields label=password --reveal"
        );
        assert!(read_secret(&fake, &auth, "nx_local").is_err());
        assert!(read_secret(&fake, &auth, "op_--help").is_err());
    }

    #[test]
    fn a_failed_read_says_op_s_words_and_no_secret() {
        let fake = Fake {
            answers: vec![(
                "item get",
                out(false, "s3cret-leak", "[ERROR] authorization prompt dismissed\nmore"),
            )],
            calls: Default::default(),
        };
        let auth = Authorized::from(&Auth::Approved).unwrap();
        let error = read_secret(&fake, &auth, "op_abcd").unwrap_err();
        assert_eq!(error, "[ERROR] authorization prompt dismissed");
        assert!(!error.contains("s3cret-leak"));
    }

    #[test]
    fn steps_name_the_two_things_the_person_does() {
        assert!(STEPS[0].contains("op") && STEPS[1].contains("Integrate with 1Password CLI"));
    }
}
