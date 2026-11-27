//! Codex on this machine: `codex login status` read, and its launch, through Gateway or on the plan.
//!
//! Through Gateway: a `nexus` model provider given with `-c` overrides for this run only (base URL
//! `<core>/v1/openai/v1`, the Responses API, the key from `NEXUS_API_KEY`); `OPENAI_API_KEY` is removed. On the plan
//! it keeps its ChatGPT login and reports each call's usage (not its content) to the session over OpenTelemetry.

use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncReadExt;

use nexus_agents::{Agent, AgentT, Codex};
use nexus_providers::{Credential, ProviderError};
use serde_json::Value;

use super::LocalAgentT;
use crate::Login;
pub use crate::codex_rollouts::codex_conversation;
use crate::launch::{
    CredentialEnv, LaunchOptions, Route, SESSION_HEADER, TEAM_HEADER, after, credential_of, toml, usage_url,
    with_overrides,
};

/// The variable Codex reads its Nexus key from (the `nexus` provider's `env_key`).
pub const CODEX_KEY_ENV: &str = "NEXUS_API_KEY";

/// Codex's shell snapshot writes its environment to a plain file in ~/.codex. Every launch that puts a key or token in
/// that environment turns it off.
const NO_SHELL_SNAPSHOT: &str = "features.shell_snapshot=false";

/// Codex's `login status`, on stdout or stderr. Only Codex saying so is signed out: a check that fails otherwise (an
/// older Codex, a broken config) is unknown.
pub fn read_codex_status(output: &str) -> Login {
    let text = output.to_lowercase();
    if text.contains("logged in using chatgpt") {
        Login::Plan
    } else if text.contains("api key") {
        Login::ApiKey
    } else if text.contains("not logged in") {
        Login::SignedOut
    } else {
        Login::Unknown
    }
}

impl LocalAgentT for Codex {
    fn model_catalog<'a>(
        &self,
        program: &'a std::path::Path,
        env: &'a HashMap<String, String>,
        dir: &'a std::path::Path,
    ) -> super::CatalogFuture<'a> {
        Box::pin(codex_catalog(program, env, dir))
    }

    fn models<'a>(
        &self,
        program: &'a std::path::Path,
        path: &'a str,
        _version: Option<&'a str>,
        env: Option<&'a HashMap<String, String>>,
    ) -> super::ModelFuture<'a> {
        Box::pin(crate::codex_models::discover_with_env(program, path, env))
    }

    /// No file settles Codex's sign-in: its check reads it.
    fn settled_login(&self, _home: Option<&str>) -> Option<Login> {
        None
    }

    fn read_status(&self, stdout: &str, stderr: &str) -> (Login, Option<String>) {
        (
            read_codex_status(&format!("{}\n{}", stdout.trim(), stderr.trim())),
            None,
        )
    }

    /// Codex's `login status` names no account.
    fn read_account(&self, _stdout: &str) -> Option<String> {
        None
    }

    fn launch(&self, options: LaunchOptions) -> (Vec<String>, HashMap<String, String>) {
        let credential = credential_of(&options);
        self.launch_with_credential(options, credential)
    }

    fn launch_with_credential(
        &self,
        options: LaunchOptions,
        credential: Credential,
    ) -> (Vec<String>, HashMap<String, String>) {
        let mut env = options.env;
        let first = super::route_env(Agent::Codex, &credential).apply(&mut env);
        let mut overrides = Vec::new();
        if options.route == Route::Gateway {
            // Codex sends a hosted search tool on every request unless told not to. Gateway refuses it while the
            // organization's search is off, so Codex searches only when it may; a later `-c` of the person's wins.
            if !options.web_search {
                overrides.push(format!("web_search={}", toml("disabled")));
            }
            if let Some(catalog) = options.model_catalog {
                overrides.push(format!("model_catalog_json={}", toml(&catalog.to_string_lossy())));
            }
        }
        if let Some(model) = options.model.filter(|m| !m.is_empty()) {
            overrides.push(format!("model={}", toml(model)));
        }
        (after(first, with_overrides(overrides, options.args)), env)
    }

    fn credential_env<'c>(&self, credential: &'c Credential) -> Result<CredentialEnv<'c>, ProviderError> {
        let remove = Codex.entry().billing.plan_overrides.to_vec();
        match credential {
            Credential::VendorKey {
                vendor: nexus_providers::KeyVendor::Openai,
                secret,
            } => {
                let overrides = vec![
                    "model_provider=\"nexus_byo\"".into(),
                    "model_providers.nexus_byo={name=\"OpenAI\",base_url=\"https://api.openai.com/v1\",env_key=\"NEXUS_API_KEY\",wire_api=\"responses\"}".into(),
                    "forced_login_method=\"api\"".into(),
                    "otel.exporter=\"none\"".into(),
                    NO_SHELL_SNAPSHOT.into(),
                ];
                Ok(CredentialEnv {
                    set: vec![(CODEX_KEY_ENV, Cow::Borrowed(secret.expose()))],
                    remove,
                    args: with_overrides(overrides, Vec::new()),
                })
            }
            Credential::Gateway {
                core_url,
                token,
                session_id,
                team,
            } => {
                let mut headers = vec![format!("{} = {}", toml(SESSION_HEADER), toml(session_id))];
                if let Some(team) = team.as_deref().filter(|t| !t.is_empty()) {
                    headers.push(format!("{} = {}", toml(TEAM_HEADER), toml(team)));
                }
                let overrides = vec![
                    format!("model_provider={}", toml("nexus")),
                    format!("model_providers.nexus.name={}", toml("Nexus")),
                    format!(
                        "model_providers.nexus.base_url={}",
                        toml(&format!("{core_url}/v1/openai/v1"))
                    ),
                    format!("model_providers.nexus.env_key={}", toml(CODEX_KEY_ENV)),
                    format!("model_providers.nexus.wire_api={}", toml("responses")),
                    format!("model_providers.nexus.http_headers={{ {} }}", headers.join(", ")),
                    NO_SHELL_SNAPSHOT.into(),
                ];
                Ok(CredentialEnv {
                    set: vec![(CODEX_KEY_ENV, Cow::Borrowed(token.expose()))],
                    remove,
                    args: with_overrides(overrides, Vec::new()),
                })
            }
            Credential::AgentLogin { usage, .. } => {
                let mut set: Vec<(&'static str, Cow<'c, str>)> =
                    super::account_dir_env(Agent::Codex, credential).into_iter().collect();
                let mut overrides = vec![format!("forced_login_method={}", toml("chatgpt"))];
                if let Some(usage) = usage {
                    set.push((CODEX_KEY_ENV, Cow::Borrowed(usage.token.expose())));
                    // Codex does not expand variables in -c, so the key goes in the OTLP headers variable, never in
                    // argv.
                    set.push((
                        "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
                        Cow::Owned(format!("authorization=Bearer {}", usage.token.expose())),
                    ));
                    overrides.extend([
                        "otel.log_user_prompt=false".to_string(),
                        format!(
                            "otel.exporter={{ otlp-http = {{ endpoint = {}, protocol = \"json\" }} }}",
                            toml(&usage_url(usage))
                        ),
                        NO_SHELL_SNAPSHOT.into(),
                    ]);
                }
                Ok(CredentialEnv {
                    set,
                    remove,
                    args: with_overrides(overrides, Vec::new()),
                })
            }
            other => Err(ProviderError::NotOffered {
                agent: Agent::Codex,
                kind: other.kind(),
            }),
        }
    }

    /// Codex's `inherit = "core"` drops variables such as `GIT_CONFIG_PARAMETERS` from its command shells. This sets
    /// one value explicitly, without widening inheritance or replacing the person's other
    /// `shell_environment_policy.set` entries.
    fn shell_variable(&self, name: &str, value: &str) -> Option<[String; 2]> {
        Some([
            "-c".into(),
            format!("shell_environment_policy.set.{name}={}", toml(value)),
        ])
    }

    fn takes_short_model_flags(&self) -> bool {
        true
    }

    fn provider_model_id(&self, id: &str) -> String {
        id.strip_prefix("openai/").unwrap_or(id).to_string()
    }

    fn takes_instructions(&self) -> bool {
        true
    }

    /// Codex gets `text` as `developer_instructions`; a later `-c` the user gave still wins.
    fn with_instructions(&self, args: &[String], text: &str) -> Vec<String> {
        let mut out = vec![
            "-c".to_string(),
            format!("developer_instructions={}", Value::from(text)),
        ];
        out.extend(args.iter().cloned());
        out
    }

    fn takes_recording(&self) -> bool {
        true
    }

    /// `codex "<prompt>"` starts an interactive conversation on the prompt.
    fn takes_prompt_argument(&self) -> bool {
        true
    }

    /// Codex gets the contact sheet attached, when it exists. With nothing else to say, `prompt` is its first prompt.
    fn with_recording(&self, args: &[String], prompt: &str, sheet: &Path) -> Vec<String> {
        // Codex takes `-i` after its subcommand (`codex exec -i …`); before one it is the interactive session's.
        let image: Vec<String> = if sheet.is_file() {
            vec!["-i".to_string(), sheet.display().to_string()]
        } else {
            Vec::new()
        };
        let mut out = Vec::new();
        if let Some(first) = args.first().filter(|first| *first == "exec" || *first == "e") {
            out.push(first.clone());
            out.extend(image);
            out.extend(args[1..].iter().cloned());
            return out;
        }
        out.extend(image);
        out.extend(args.iter().cloned());
        if args.is_empty() {
            out.push(prompt.to_string());
        }
        out
    }

    fn memory(&self, home: &Path, repo: &Path) -> std::io::Result<Vec<nexus_agent_memory::NativeEntry>> {
        crate::native_memory::codex::read(home, repo)
    }

    fn write_memory(&self, home: &Path, repo: &Path, entries: &[nexus_agent_memory::Entry]) -> std::io::Result<()> {
        crate::native_memory::codex::write(home, repo, entries)
    }

    fn keeps_transcript(&self) -> bool {
        true
    }

    fn newest_conversation(&self, home: &Path, cwd: &Path, since: SystemTime) -> Option<String> {
        codex_conversation(home, cwd, since)
    }

    fn transcript(&self, home: &Path, _cwd: &Path, id: &str) -> Option<PathBuf> {
        crate::codex_rollouts::rollout(home, id)
    }
}

/// Codex's catalog with Responses Lite turned off, as the desktop app writes it.
fn without_responses_lite(text: &str) -> Option<String> {
    let mut catalog: Value = serde_json::from_str(text).ok()?;
    let models = catalog.get_mut("models")?.as_array_mut()?;
    if !models.iter().all(Value::is_object) || !models.iter().any(|m| m["use_responses_lite"] == Value::Bool(true)) {
        return None;
    }
    for model in models.iter_mut() {
        model["use_responses_lite"] = Value::Bool(false);
    }
    serde_json::to_string(&catalog).ok()
}

async fn codex_catalog(program: &Path, env: &HashMap<String, String>, dir: &Path) -> Option<PathBuf> {
    catalog_with_timeout(program, env, dir, Duration::from_secs(10)).await
}

async fn catalog_with_timeout(
    program: &Path,
    env: &HashMap<String, String>,
    dir: &Path,
    deadline: Duration,
) -> Option<PathBuf> {
    let mut child = crate::output::async_agent_command(program)
        .args(["debug", "models", "--bundled"])
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let printed = tokio::time::timeout(deadline, async {
        let mut stdout = child.stdout.take()?;
        let mut printed = Vec::new();
        stdout.read_to_end(&mut printed).await.ok()?;
        child.wait().await.ok()?.success().then_some(printed)
    })
    .await
    .ok()
    .flatten();
    if printed.is_none() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    let catalog = without_responses_lite(std::str::from_utf8(&printed?).ok()?)?;
    write_catalog(&catalog, dir)
}

/// Content-addressed catalogs can be shared by simultaneous launches once the complete file is published.
fn write_catalog(catalog: &str, dir: &Path) -> Option<PathBuf> {
    let digest = Sha256::digest(catalog.as_bytes());
    let hash: String = digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect();
    let name = format!("codex-models-{hash}.json");
    let path = dir.join(&name);
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == catalog) {
        return Some(path);
    }
    let mut directory = std::fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory.create(dir).ok()?;
    let unique = uuid::Uuid::new_v4().simple();
    let partial = dir.join(format!("{name}.{unique}.tmp"));
    let written = (|| -> std::io::Result<PathBuf> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&partial)?;
        file.write_all(catalog.as_bytes())?;
        drop(file);
        match std::fs::rename(&partial, &path) {
            Ok(()) => Ok(path.clone()),
            Err(_) if std::fs::read_to_string(&path).is_ok_and(|existing| existing == catalog) => Ok(path.clone()),
            Err(_) => {
                // A running Codex can hold a corrupt Windows cache file open. Publish beside it.
                let alternate = dir.join(format!("codex-models-{hash}-{unique}.json"));
                std::fs::rename(&partial, &alternate)?;
                Ok(alternate)
            }
        }
    })();
    let _ = std::fs::remove_file(partial);
    written.ok()
}

#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn an_openai_key_launch_turns_off_the_shell_snapshot() {
        let credential = Credential::VendorKey {
            vendor: nexus_providers::KeyVendor::Openai,
            secret: nexus_providers::Secret::new("sk-test-key".into()),
        };
        let (args, env) = Codex.launch_with_credential(
            LaunchOptions {
                route: Route::Subscription,
                core_url: "",
                api_key: "",
                session_id: "",
                team: None,
                model: None,
                model_catalog: None,
                web_search: false,
                args: vec!["app-server".into()],
                env: HashMap::new(),
            },
            credential,
        );
        assert_snapshot_off(&args, &env, "sk-test-key");
    }

    #[test]
    fn a_gateway_launch_turns_off_the_shell_snapshot() {
        let (args, env) = Codex.launch(LaunchOptions {
            route: Route::Gateway,
            core_url: "https://core.example",
            api_key: "nx_test_token",
            session_id: "sess_1",
            team: None,
            model: None,
            model_catalog: None,
            web_search: false,
            args: vec!["app-server".into()],
            env: HashMap::new(),
        });
        assert_snapshot_off(&args, &env, "nx_test_token");
    }

    fn assert_snapshot_off(args: &[String], env: &HashMap<String, String>, secret: &str) {
        assert_eq!(env.get(CODEX_KEY_ENV).map(String::as_str), Some(secret));
        let at = args
            .iter()
            .position(|arg| arg == NO_SHELL_SNAPSHOT)
            .expect("the snapshot override");
        assert_eq!(args[at - 1], "-c");
        assert!(
            !args.iter().any(|arg| arg.contains(secret)),
            "the secret stays out of argv"
        );
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::without_responses_lite;
    use serde_json::{Value, json};

    #[test]
    fn turns_responses_lite_off_in_the_codex_catalog() {
        let catalog = json!({ "models": [
            { "slug": "gpt-6-sol", "use_responses_lite": true, "tool_mode": "code_mode_only" },
            { "slug": "gpt-5.5", "use_responses_lite": false },
        ]});
        let changed: Value =
            serde_json::from_str(&without_responses_lite(&catalog.to_string()).expect("a catalog")).expect("json");
        assert_eq!(
            changed,
            json!({ "models": [
                { "slug": "gpt-6-sol", "use_responses_lite": false, "tool_mode": "code_mode_only" },
                { "slug": "gpt-5.5", "use_responses_lite": false },
            ]})
        );
        // Nothing to change (an older Codex), or not a catalog: Codex gets none.
        assert_eq!(without_responses_lite(r#"{"models":[{"slug":"gpt-5.5"}]}"#), None);
        assert_eq!(without_responses_lite("args: debug models --bundled"), None);
        assert_eq!(without_responses_lite(r#"{"models":["gpt-6-sol"]}"#), None);
    }
}

#[cfg(test)]
mod storage_tests {
    use super::*;

    #[test]
    fn concurrent_catalog_writers_publish_complete_private_files() {
        let dir = std::env::temp_dir().join(format!("nexus-catalog-{}", uuid::Uuid::new_v4()));
        let catalog = serde_json::json!({"models": [{"base_instructions": "x".repeat(1_000_000)}]}).to_string();
        let barrier = std::sync::Barrier::new(9);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        write_catalog(&catalog, &dir)
                    })
                })
                .collect();
            barrier.wait();
            while handles.iter().any(|handle| !handle.is_finished()) {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        if entry.path().extension().is_some_and(|extension| extension == "json") {
                            assert_eq!(
                                std::fs::read_to_string(entry.path()).expect("published catalog"),
                                catalog
                            );
                        }
                    }
                }
                std::thread::yield_now();
            }
            for handle in handles {
                let path = handle.join().expect("writer finished").expect("published path");
                assert_eq!(std::fs::read_to_string(&path).expect("catalog"), catalog);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    assert_eq!(
                        std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
                        0o600
                    );
                }
            }
        });
        assert_eq!(std::fs::read_dir(&dir).expect("directory").count(), 1);
        std::fs::remove_dir_all(dir).expect("remove fixture");
    }

    #[test]
    fn corrupt_cached_catalog_is_replaced_atomically() {
        let dir = std::env::temp_dir().join(format!("nexus-catalog-repair-{}", uuid::Uuid::new_v4()));
        let catalog = r#"{"models":[{"slug":"m","use_responses_lite":false}]}"#;
        let path = write_catalog(catalog, &dir).expect("initial catalog");
        std::fs::write(&path, "{").expect("corrupt cached file");
        assert_eq!(write_catalog(catalog, &dir), Some(path.clone()));
        assert_eq!(std::fs::read_to_string(&path).expect("repaired catalog"), catalog);
        std::fs::remove_file(&path).expect("remove catalog");
        std::fs::create_dir(&path).expect("blocking directory");
        let alternate = write_catalog(catalog, &dir).expect("alternate catalog");
        assert_ne!(alternate, path);
        assert!(path.is_dir());
        assert_eq!(std::fs::read_to_string(alternate).expect("alternate contents"), catalog);
        std::fs::remove_dir_all(dir).expect("remove fixture");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn timed_out_catalog_probe_is_reaped_before_runtime_shutdown() {
        let dir = std::env::temp_dir().join(format!("nexus-catalog-reap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("directory");
        let program = dir.join("codex");
        let pid_file = dir.join("pid");
        crate::output::write_fake_script(
            &program,
            "#!/bin/sh\nprintf '%s' \"$$\" > \"$PROBE_PID\"\nexec /bin/sleep 30\n",
        );
        let env = HashMap::from([("PROBE_PID".into(), pid_file.to_string_lossy().into_owned())]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert_eq!(
            runtime.block_on(catalog_with_timeout(
                &program,
                &env,
                &dir.join("out"),
                Duration::from_secs(3)
            )),
            None
        );
        drop(runtime);
        let pid = std::fs::read_to_string(pid_file).expect("probe started");
        assert!(!Path::new("/proc").join(pid).exists(), "probe was reaped");
        std::fs::remove_dir_all(dir).expect("remove fixture");
    }

    #[cfg(windows)]
    #[test]
    fn open_corrupt_catalog_uses_an_alternate_complete_file() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = std::env::temp_dir().join(format!("nexus-catalog-open-{}", uuid::Uuid::new_v4()));
        let catalog = r#"{"models":[{"slug":"m","use_responses_lite":false}]}"#;
        let original = write_catalog(catalog, &dir).expect("initial catalog");
        std::fs::write(&original, "{").expect("corrupt catalog");
        // FILE_SHARE_READ permits reads while preventing replacement of this open file.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&original)
            .expect("held catalog");
        let alternate = write_catalog(catalog, &dir).expect("alternate catalog");
        assert_ne!(alternate, original);
        assert_eq!(std::fs::read_to_string(alternate).expect("catalog"), catalog);
        drop(held);
        std::fs::remove_dir_all(dir).expect("remove fixture");
    }

    #[tokio::test]
    async fn node_stand_ins_are_started_and_nonzero_exit_skips_the_catalog() {
        let dir = std::env::temp_dir().join(format!("nexus-node-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("directory");
        let program = dir.join("codex stand-in.mjs");
        std::fs::write(
            &program,
            r#"
if (process.argv.slice(2).join(" ") !== "debug models --bundled") process.exit(2);
if (process.env.CATALOG_FAIL === "1") process.exit(3);
if (process.env.CATALOG_INVALID_UTF8 === "1") {
    process.stdout.write(Buffer.concat([Buffer.from('{"models":[{"slug":"'), Buffer.from([255]), Buffer.from('","use_responses_lite":true}]}')]));
    process.exit(0);
}
console.log(JSON.stringify({models: [{slug: "m", use_responses_lite: true}]}));
"#,
        )
        .expect("Node stand-in");
        let mut env: HashMap<String, String> = std::env::vars().collect();
        let path = codex_catalog(&program, &env, &dir.join("out")).await.expect("catalog");
        assert_eq!(
            serde_json::from_str::<Value>(&std::fs::read_to_string(path).expect("read")).expect("json")["models"][0]["use_responses_lite"],
            false
        );
        env.insert("CATALOG_FAIL".into(), "1".into());
        assert_eq!(codex_catalog(&program, &env, &dir.join("failed")).await, None);
        assert!(!dir.join("failed").exists());
        env.remove("CATALOG_FAIL");
        env.insert("CATALOG_INVALID_UTF8".into(), "1".into());
        assert_eq!(codex_catalog(&program, &env, &dir.join("invalid-utf8")).await, None);
        assert!(!dir.join("invalid-utf8").exists());
        std::fs::remove_dir_all(dir).expect("remove fixture");
    }
}
