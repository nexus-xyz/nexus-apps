//! A cloud machine's heartbeat. On a cloud machine, Gateway writes `~/.nexus/host/cloud.json`
//! (`{ "machineId", "gatewayUrl" }`) and the host's own Nexus key (`credentials`) when it creates the machine. Then
//! every minute the host tells Gateway what its sessions do, and Gateway decides when the machine sleeps, and when it
//! restarts on a bigger size after the system ran out of memory:
//!
//! `POST /v1/cloud/machines/{machineId}/activity` with
//! `{ "busy", "agentIdleSince", "interactedAt", "oomKills" }` (times in RFC 3339, null when there is none).
//!
//! On a person's own server there is no `cloud.json`, and the host sends nothing.
//!
//! The machine's own setup. When Gateway made the machine for a session on a GitHub repository, for a
//! person GitHub lets read it, `cloud.json` also has `"setup": { "repo": "<owner/name>", "grant": "<sealed>" }`. The
//! host then clones the repository and runs its `.agents/setup` at boot, with no app connected (nexus-workbench
//! `cloud_setup`). Its GitHub token comes from Gateway ([`GatewayToken`]):
//!
//! `POST /v1/cloud/machines/{machineId}/setup-token` with `{ "grant" }` and the machine's own Nexus token, answered
//! `{ "repo", "token" }`: a GitHub App token that may only read that repository, for an hour at most.
//!
//! The grant is no credential by itself: Gateway sealed it for this machine, and it works only with the machine's
//! own Nexus token. The token is kept in memory only, and never written or printed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use nexus_client::Client;
use nexus_workbench::cloud_setup::{TokenError, TokenSource};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::files::Credentials;
use crate::server::{Heartbeat, Server};

/// How often the host sends its heartbeat.
pub const HEARTBEAT: Duration = Duration::from_secs(60);

/// The cgroup v2 memory events of the host's own group, and the kernel's counters for the whole system.
pub const MEMORY_EVENTS: &str = "/sys/fs/cgroup/memory.events";
pub const VMSTAT: &str = "/proc/vmstat";

/// What Gateway wrote on the machine at create.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudFile {
    pub machine_id: String,
    /// The Gateway to tell. When it is missing, the one the host signed in to.
    #[serde(default)]
    pub gateway_url: Option<String>,
    /// The machine's own setup at boot, when Gateway asked for one.
    #[serde(default)]
    pub setup: Option<SetupFile>,
}

/// `cloud.json`'s `setup`: GitHub's `owner/name`, and Gateway's grant for the repository's token.
#[derive(Clone, PartialEq, Deserialize)]
pub struct SetupFile {
    pub repo: String,
    pub grant: String,
}

impl std::fmt::Debug for SetupFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupFile")
            .field("repo", &self.repo)
            .finish_non_exhaustive()
    }
}

impl CloudFile {
    /// None on a machine that is not a cloud machine, or with a file that cannot be read.
    pub fn load(path: &Path) -> Option<CloudFile> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<CloudFile>(&text)
            .ok()
            .filter(|file| !file.machine_id.trim().is_empty())
    }
}

/// Pairs this server with the person `client` signs in as (`POST /v1/cloud/machines/pair`) and writes `paired`
/// ([`crate::files::Paths::paired`]): the line out then dials Gateway as this machine. Answers its id.
pub async fn pair(client: &Client, gateway: &str, name: &str, paired: &Path) -> Result<String, String> {
    let answer = client
        .post("/v1/cloud/machines/pair", json!({ "name": name }))
        .await
        .map_err(|e| format!("Could not pair this server: {e}"))?;
    let id = answer["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or("Gateway paired this server but did not say its id")?;
    let file = json!({ "machineId": id, "gatewayUrl": gateway });
    crate::files::save_private(paired, &format!("{file}\n"))?;
    Ok(id.to_string())
}

/// The `oom_kill` count in `memory.events` or `/proc/vmstat` text (both write `oom_kill <n>` on a line of its own).
pub fn oom_kills_in(text: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        match (words.next(), words.next()) {
            (Some("oom_kill"), Some(count)) => count.parse().ok(),
            _ => None,
        }
    })
}

/// How many processes the kernel killed for want of memory: the host's cgroup first, else the whole system.
pub fn oom_kills(memory_events: &Path, vmstat: &Path) -> u64 {
    [memory_events, vmstat]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok().and_then(|text| oom_kills_in(&text)))
        .unwrap_or(0)
}

fn rfc3339(ms: Option<i64>) -> Value {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|time| Value::String(time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)))
        .unwrap_or(Value::Null)
}

/// The heartbeat's body.
pub fn body(beat: &Heartbeat, oom_kills: u64) -> Value {
    json!({
        "busy": beat.busy,
        "agentIdleSince": rfc3339(beat.agent_idle_since),
        "interactedAt": rfc3339(beat.interacted_at),
        "oomKills": oom_kills,
    })
}

/// Where the heartbeat reads from.
#[derive(Debug, Clone)]
pub struct Sources {
    pub cloud: PathBuf,
    pub credentials: PathBuf,
    pub memory_events: PathBuf,
    pub vmstat: PathBuf,
}

/// Sends one heartbeat, if this is a cloud machine with a Nexus key. The files are read each time: Gateway may write
/// them after the host started. An error is only logged: the next minute tries again.
pub async fn beat(server: &Server, sources: &Sources) {
    let Some(file) = CloudFile::load(&sources.cloud) else {
        return;
    };
    let Some(credentials) = Credentials::load(&sources.credentials) else {
        eprintln!("nexus-host: a cloud machine without its Nexus key; no heartbeat");
        return;
    };
    let gateway = file
        .gateway_url
        .clone()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| credentials.gateway.clone());
    let beat = match server.heartbeat() {
        Ok(beat) => beat,
        Err(error) => {
            eprintln!("nexus-host: heartbeat: {error}");
            return;
        }
    };
    let client = Client::new(crate::gateway::client_options(
        &gateway,
        Some(credentials.token.clone()),
    ));
    let body = body(&beat, oom_kills(&sources.memory_events, &sources.vmstat));
    // Inline, so scripts/api-requirement.mjs reads the operation (`POST /v1/cloud/machines/{}/activity`).
    if let Err(error) = client
        .post(&format!("/v1/cloud/machines/{}/activity", file.machine_id), body)
        .await
    {
        eprintln!("nexus-host: heartbeat: {error}");
    }
}

/// How often the host looks for `cloud.json` before it starts the machine's own setup: the cloud provider may write it after the
/// host started.
const SETUP_LOOK: Duration = Duration::from_secs(2);

/// Starts the machine's own setup once `cloud.json` is there and names one ([`SetupFile`]); returns at once when it
/// names none. It also gives the repository's `.agents/resume` its environment ([`HeadlessHost::resume_env`]). The state is saved in `state` (`~/.nexus/host/setup.json`).
pub async fn begin_setup(
    server: Arc<Server>,
    host: Arc<crate::gateway::HeadlessHost>,
    sources: Sources,
    state: PathBuf,
) {
    let file = loop {
        if let Some(file) = CloudFile::load(&sources.cloud) {
            break file;
        }
        tokio::time::sleep(SETUP_LOOK).await;
    };
    let Some(setup) = file.setup.clone().filter(|setup| valid_repo(&setup.repo)) else {
        return;
    };
    let token = GatewayToken {
        sources,
        grant: setup.grant,
    };
    // `.agents/resume` gets the agent's own environment, once the person's credentials are on the machine.
    server.api.set_cloud_resume_env(Arc::new(move || host.resume_env()));
    server.api.begin_cloud_setup(setup.repo, state, Arc::new(token));
}

/// GitHub's `owner/name`: two names of letters, digits, `-`, `_` and `.`.
fn valid_repo(repo: &str) -> bool {
    let name = |part: &str| {
        !part.is_empty()
            && !part.starts_with(['-', '.'])
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    matches!(repo.split_once('/'), Some((owner, name_part)) if name(owner) && name(name_part))
}

/// The setup's GitHub token, asked of Gateway with the machine's own Nexus token and the grant each time: a retry
/// gets a new one.
pub struct GatewayToken {
    sources: Sources,
    grant: String,
}

impl TokenSource for GatewayToken {
    fn done(&self) -> BoxFuture<'static, ()> {
        let (sources, grant) = (self.sources.clone(), self.grant.clone());
        Box::pin(async move {
            let Some(file) = CloudFile::load(&sources.cloud) else {
                return;
            };
            let Some(credentials) = Credentials::load(&sources.credentials) else {
                return;
            };
            let gateway = file
                .gateway_url
                .clone()
                .filter(|url| !url.trim().is_empty())
                .unwrap_or_else(|| credentials.gateway.clone());
            let client = Client::new(crate::gateway::client_options(
                &gateway,
                Some(credentials.token.clone()),
            ));
            // Inline, so scripts/api-requirement.mjs reads the operation (`POST /v1/cloud/machines/{}/setup-done`).
            let ended = client
                .post(
                    &format!("/v1/cloud/machines/{}/setup-done", file.machine_id),
                    json!({ "grant": grant }),
                )
                .await;
            if let Err(error) = ended {
                eprintln!("nexus-host: could not end the setup's grant: {error}");
            }
        })
    }

    fn token(&self) -> BoxFuture<'static, Result<String, TokenError>> {
        let (sources, grant) = (self.sources.clone(), self.grant.clone());
        Box::pin(async move {
            let file =
                CloudFile::load(&sources.cloud).ok_or_else(|| TokenError::Failed("cloud.json is gone".into()))?;
            let credentials = Credentials::load(&sources.credentials)
                .ok_or_else(|| TokenError::Failed("the machine has no Nexus key".into()))?;
            let gateway = file
                .gateway_url
                .clone()
                .filter(|url| !url.trim().is_empty())
                .unwrap_or_else(|| credentials.gateway.clone());
            let client = Client::new(crate::gateway::client_options(
                &gateway,
                Some(credentials.token.clone()),
            ));
            // Inline, so scripts/api-requirement.mjs reads the operation (`POST /v1/cloud/machines/{}/setup-token`).
            let answer = client
                .post(
                    &format!("/v1/cloud/machines/{}/setup-token", file.machine_id),
                    json!({ "grant": grant }),
                )
                .await
                .map_err(token_error)?;
            answer
                .get("token")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .map(str::to_string)
                .ok_or_else(|| TokenError::Withdrawn("Gateway gave no token for the repository".into()))
        })
    }
}

/// Gateway's refusal: a 403 or 404 means the grant is not good any more, so the app clones instead; anything else is
/// a failure Try again may get past. Only Gateway's words: they never hold a token.
fn token_error(error: nexus_client::Error) -> TokenError {
    match error {
        nexus_client::Error::Core(core) if matches!(core.status, 403 | 404 | 409) => {
            TokenError::Withdrawn(core.message.clone())
        }
        nexus_client::Error::Core(core) => TokenError::Failed(core.message.clone()),
        error @ nexus_client::Error::Transport(_) => TokenError::Failed(format!("Gateway did not answer: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_oom_kill_count_from_either_file() {
        let events = "low 0\nhigh 0\nmax 12\noom 3\noom_kill 2\noom_group_kill 0\n";
        assert_eq!(oom_kills_in(events), Some(2));
        let vmstat = "pgfault 1234\noom_kill 7\nnr_free_pages 5\n";
        assert_eq!(oom_kills_in(vmstat), Some(7));
        // `oom_group_kill` and `oom` are other counters.
        assert_eq!(oom_kills_in("oom 3\noom_group_kill 4\n"), None);
    }

    #[test]
    fn the_cgroup_comes_first_and_the_system_is_the_fallback() {
        let dir = std::env::temp_dir().join(format!("nh-oom-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let events = dir.join("memory.events");
        let vmstat = dir.join("vmstat");
        std::fs::write(&vmstat, "oom_kill 9\n").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(oom_kills(&events, &vmstat), 9);
        std::fs::write(&events, "oom 1\noom_kill 1\n").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(oom_kills(&events, &vmstat), 1);
        assert_eq!(oom_kills(&dir.join("none"), &dir.join("none")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_body_is_the_contracts() {
        let beat = Heartbeat {
            busy: false,
            agent_idle_since: Some(1_700_000_000_000),
            interacted_at: None,
        };
        assert_eq!(
            body(&beat, 2),
            json!({
                "busy": false,
                "agentIdleSince": "2023-11-14T22:13:20.000Z",
                "interactedAt": null,
                "oomKills": 2,
            })
        );
    }

    #[test]
    fn only_a_github_repository_is_set_up() {
        assert!(valid_repo("acme/web"));
        assert!(valid_repo("acme-inc/web.app_2"));
        for bad in [
            "acme",
            "acme/",
            "/web",
            "acme/web/x",
            "-x/web",
            "acme/..",
            "acme/w b",
            "../web",
        ] {
            assert!(!valid_repo(bad), "{bad}");
        }
    }

    #[test]
    fn a_refused_grant_leaves_the_clone_to_the_app() {
        let refused = |status: u16| {
            nexus_client::Error::Core(Box::new(nexus_client::CoreError {
                status,
                message: "no".into(),
                code: None,
                interval: None,
                details: None,
            }))
        };
        assert_eq!(token_error(refused(403)), TokenError::Withdrawn("no".into()));
        assert_eq!(token_error(refused(404)), TokenError::Withdrawn("no".into()));
        assert_eq!(token_error(refused(502)), TokenError::Failed("no".into()));
    }

    #[test]
    fn a_cloud_file_needs_a_machine() {
        let dir = std::env::temp_dir().join(format!("nh-cloudfile-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("cloud.json");
        assert_eq!(CloudFile::load(&path), None);
        std::fs::write(&path, r#"{"machineId":"m1","gatewayUrl":"https://gw"}"#).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            CloudFile::load(&path),
            Some(CloudFile {
                machine_id: "m1".into(),
                gateway_url: Some("https://gw".into()),
                setup: None,
            })
        );
        // A machine Gateway sets up itself names its repository and the grant for its token.
        std::fs::write(
            &path,
            r#"{"machineId":"m1","setup":{"repo":"acme/web","grant":"sealed"}}"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let setup = CloudFile::load(&path).and_then(|file| file.setup);
        assert_eq!(setup.as_ref().map(|s| s.repo.as_str()), Some("acme/web"));
        assert!(!format!("{setup:?}").contains("sealed"));
        std::fs::write(&path, r#"{"machineId":""}"#).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(CloudFile::load(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
