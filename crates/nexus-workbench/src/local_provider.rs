//! Launch preparation for a key or server saved in the desktop provider settings.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use nexus_providers::{Credential, InstanceId, ProviderKind, ProviderSettings};

use crate::{
    Launch, LaunchRequest, SessionAgent,
    agent_launch::{allows_local_launch, strip_gateway_env},
    app_socket::Launchables,
    computer::BillingPayer,
    mcp_servers::{self, McpServer, Values},
    process::Spawn,
    provider::{
        LaunchContext, LaunchRoute, LocalProviders, MachineFacts, Prepared, ProviderSet, saved_provider_refusal,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub id: InstanceId,
    pub kind: ProviderKind,
    pub native_mcp: NativeMcp,
}

impl Run {
    pub(crate) fn check_payer(&self, payer: Option<BillingPayer>) -> Result<(), String> {
        match (self.kind, payer) {
            (ProviderKind::ApiKey, None | Some(BillingPayer::ApiKey)) | (ProviderKind::OpenAiCompatible, None) => {
                Ok(())
            }
            _ => Err("This session requires a different payer.".into()),
        }
    }

    pub(crate) fn mcp_warning(&self) -> Option<String> {
        let mut lines = Vec::new();
        if !self.native_mcp.names.is_empty() {
            lines.push(format!(
                "These MCP servers may receive the provider key: {}.",
                self.native_mcp.names.join(", "),
            ));
        }
        if self.native_mcp.uncertain {
            lines.push(
                "Nexus could not check all active MCP configuration. Other MCP servers may receive the provider key."
                    .into(),
            );
        }
        (!lines.is_empty()).then(|| lines.join(" "))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeMcp {
    names: Vec<String>,
    uncertain: bool,
}

impl NativeMcp {
    /// Names only. Never put native commands, arguments or values in a notice.
    pub fn include(&mut self, name: &str) {
        if !name.is_empty()
            && name.len() <= 96
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            self.names.push(name.into());
        } else {
            self.uncertain = true;
        }
    }

    pub fn unresolved(&mut self) {
        self.uncertain = true;
    }
}

pub struct Mcp<'a> {
    pub program: Option<&'a Path>,
    pub servers: &'a [McpServer],
    pub values: &'a Values,
    pub native: NativeMcp,
}

pub struct Source<'a> {
    pub settings: &'a ProviderSettings,
    pub local: &'a LocalProviders,
    pub check_owner: &'a (dyn Fn() -> Result<(), String> + Sync),
}

pub async fn prepare(
    request: &LaunchRequest,
    program: PathBuf,
    mut env: HashMap<String, String>,
    source: Source<'_>,
    mcp: Mcp<'_>,
    grant: impl FnOnce(&str, Launchables) -> Option<(String, String)>,
) -> Result<Launch, String> {
    if !allows_local_launch(request, &crate::session_scope::SessionScope::Local) {
        return Err(saved_provider_refusal(false));
    }
    let id = request.provider.as_ref().ok_or_else(|| saved_provider_refusal(false))?;
    (source.check_owner)()?;
    let providers = ProviderSet::from_settings(source.settings, &MachineFacts::default(), None, true);
    let provider = providers
        .get(id)
        .ok_or("This provider was removed. Choose a provider in Settings.")?;
    let mut run = Run {
        id: id.clone(),
        kind: provider.kind(),
        native_mcp: NativeMcp::default(),
    };
    run.check_payer(request.required_payer)?;
    let coding = request.agent.coding().ok_or_else(|| saved_provider_refusal(false))?;
    let variable = match coding {
        nexus_agents::Agent::Claude => "ANTHROPIC_API_KEY",
        nexus_agents::Agent::Codex => nexus_local_agents::agents::CODEX_KEY_ENV,
        nexus_agents::Agent::Opencode => nexus_local_agents::agents::OPENCODE_KEY_ENV,
        _ => return Err(saved_provider_refusal(false)),
    };
    let cx = LaunchContext {
        request,
        route: LaunchRoute::SavedProvider {
            keys: &source.local.keys,
        },
        program: &program,
        env: &env,
        home: env.get("HOME").or_else(|| env.get("USERPROFILE")).map(String::as_str),
    };
    let prepared = provider
        .prepare(request.agent, &cx)
        .await
        .map_err(|err| err.to_string())?;
    if !matches!(prepared, Prepared::Local { .. }) {
        return Err(saved_provider_refusal(false));
    }
    let keyed = match run.kind {
        ProviderKind::ApiKey => true,
        ProviderKind::OpenAiCompatible => source
            .settings
            .servers
            .iter()
            .any(|setting| &setting.id == id && setting.key.is_some()),
        _ => return Err(saved_provider_refusal(false)),
    };
    if keyed {
        mcp_servers::check_credential_name(mcp.servers, variable)?;
        let agent = request.agent;
        let cwd = request.cwd.clone();
        let config_env = env.clone();
        let mut native = mcp.native;
        run.native_mcp = tokio::task::spawn_blocking(move || {
            inspect_native(agent, &cwd, &config_env, variable, &mut native)?;
            native.names.sort_unstable();
            native.names.dedup();
            Ok::<_, String>(native)
        })
        .await
        .map_err(|_| "Nexus could not check native MCP configuration.".to_string())??;
    }
    (source.check_owner)()?;
    let credential = provider
        .credential(request.agent, &prepared, &cx)
        .await
        .map_err(|err| err.to_string())?;
    let Prepared::Local { model, args } = prepared else {
        unreachable!()
    };
    strip_gateway_env(&mut env);
    env.retain(|name, _| !crate::credential_env::stripped(name) && !same_env_name(name, "OPENAI_API_KEY"));
    if let Credential::Server { base_url, .. } = &credential {
        bypass_proxy_for_loopback(base_url, &mut env);
    }
    let (args, env) = nexus_client::agents::subscription_launch(coding, credential, model.as_deref(), args, env);
    let mut launch = Launch {
        attribution: None,
        spawn: Spawn {
            program,
            args,
            env,
            cwd: request.cwd.clone(),
        },
        nexus_session: None,
        local_run: Some(format!("local_{}", uuid::Uuid::new_v4())),
        model,
        local_provider: Some(run),
        session_mcp: Default::default(),
    };
    (source.check_owner)()?;
    mcp_servers::configure_local(&mut launch, request.agent, mcp.program, mcp.servers, mcp.values, grant);
    Ok(launch)
}

fn bypass_proxy_for_loopback(base_url: &str, env: &mut HashMap<String, String>) {
    let Ok(url) = url::Url::parse(base_url) else { return };
    let Some(host) = url.host_str().filter(|_| crate::provider::is_loopback(&url)) else {
        return;
    };
    let mut exclusions = env
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("NO_PROXY"))
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>();
    exclusions.push(host);
    let value = exclusions.join(",");
    env.retain(|name, _| !name.eq_ignore_ascii_case("NO_PROXY"));
    env.insert("NO_PROXY".into(), value.clone());
    if !cfg!(windows) {
        env.insert("no_proxy".into(), value);
    }
}

pub(crate) fn while_allowed<T>(host: &dyn crate::Host, operation: impl FnOnce() -> T) -> std::io::Result<T> {
    let mut operation = Some(operation);
    let mut result = None;
    host.while_saved_provider_allowed(&mut || {
        result = operation.take().map(|operation| operation());
    })
    .map_err(|reason| std::io::Error::new(std::io::ErrorKind::PermissionDenied, reason))?;
    result.ok_or_else(|| std::io::ErrorKind::PermissionDenied.into())
}

pub(crate) async fn write_input(
    host: &dyn crate::Host,
    stdin: &mut tokio::process::ChildStdin,
    mut line: &[u8],
) -> std::io::Result<()> {
    use std::{future::poll_fn, pin::Pin, task::Poll};
    use tokio::io::AsyncWrite;

    while !line.is_empty() {
        // Authorize every poll. A full pipe must wait without holding the sign-in lock.
        let written = poll_fn(|cx| {
            while_allowed(host, || Pin::new(&mut *stdin).poll_write(cx, line))
                .unwrap_or_else(|error| Poll::Ready(Err(error)))
        })
        .await?;
        if written == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        line = &line[written..];
    }
    Ok(())
}

pub(crate) fn same_env_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn inspect_native(
    agent: SessionAgent,
    cwd: &Path,
    env: &HashMap<String, String>,
    variable: &str,
    native: &mut NativeMcp,
) -> Result<(), String> {
    let home = env.get("HOME").or_else(|| env.get("USERPROFILE")).map(Path::new);
    match agent.coding() {
        Some(nexus_agents::Agent::Claude) => {
            #[cfg(target_os = "linux")]
            inspect_config(
                Path::new("/etc/claude-code/managed-mcp.json"),
                &["mcpServers"],
                variable,
                native,
            )?;
            #[cfg(target_os = "macos")]
            inspect_config(
                Path::new("/Library/Application Support/ClaudeCode/managed-mcp.json"),
                &["mcpServers"],
                variable,
                native,
            )?;
            #[cfg(windows)]
            if let Some(root) = env
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("ProgramFiles"))
                .map(|(_, value)| value)
            {
                inspect_config(
                    &Path::new(root).join("ClaudeCode/managed-mcp.json"),
                    &["mcpServers"],
                    variable,
                    native,
                )?;
            } else {
                native.unresolved();
            }
            for dir in cwd.ancestors() {
                inspect_config(&dir.join(".mcp.json"), &["mcpServers"], variable, native)?;
            }
            if let Some(home) = home {
                let config = env
                    .get("CLAUDE_CONFIG_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".claude"));
                if !matches!(config.join("plugins/installed_plugins.json").try_exists(), Ok(false)) {
                    native.unresolved();
                }
                for path in [config.join(".claude.json"), home.join(".claude.json")] {
                    inspect_config(&path, &["mcpServers"], variable, native)?;
                    if let Some(project) = cwd.to_str() {
                        inspect_config(&path, &["projects", project, "mcpServers"], variable, native)?;
                    } else {
                        native.unresolved();
                    }
                }
            } else {
                native.unresolved();
            }
        }
        Some(nexus_agents::Agent::Codex) => {
            let profile = env
                .get("CODEX_HOME")
                .map(PathBuf::from)
                .or_else(|| home.map(|dir| dir.join(".codex")));
            if let Some(profile) = profile {
                inspect_config(&profile.join("config.toml"), &["mcp_servers"], variable, native)?;
            } else {
                native.unresolved();
            }
            for dir in cwd.ancestors() {
                inspect_config(&dir.join(".codex/config.toml"), &["mcp_servers"], variable, native)?;
            }
        }
        Some(nexus_agents::Agent::Opencode) => {
            #[cfg(target_os = "linux")]
            inspect_opencode(Path::new("/etc/opencode"), variable, native)?;
            #[cfg(target_os = "macos")]
            {
                inspect_opencode(Path::new("/Library/Application Support/opencode"), variable, native)?;
                let managed = Path::new("/Library/Managed Preferences");
                let file = "ai.opencode.managed.plist";
                if !matches!(managed.join(file).try_exists(), Ok(false))
                    || env
                        .get("USER")
                        .is_none_or(|user| !matches!(managed.join(user).join(file).try_exists(), Ok(false)))
                {
                    native.unresolved();
                }
            }
            #[cfg(windows)]
            if let Some(root) = env
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("ProgramData"))
                .map(|(_, value)| value)
            {
                inspect_opencode(&Path::new(root).join("opencode"), variable, native)?;
            } else {
                native.unresolved();
            }
            let config = env
                .get("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| home.map(|dir| dir.join(".config")));
            if let Some(config) = config {
                inspect_opencode(&config.join("opencode"), variable, native)?;
            } else {
                native.unresolved();
            }
            for dir in cwd.ancestors() {
                inspect_opencode(dir, variable, native)?;
                inspect_opencode(&dir.join(".opencode"), variable, native)?;
            }
            if let Some(dir) = env.get("OPENCODE_CONFIG_DIR") {
                inspect_opencode(Path::new(dir), variable, native)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn inspect_opencode(dir: &Path, variable: &str, native: &mut NativeMcp) -> Result<(), String> {
    for file in ["opencode.json", "opencode.jsonc"] {
        let path = dir.join(file);
        inspect_config(&path, &["mcp"], variable, native)?;
        inspect_config(&path, &["mcp", "servers"], variable, native)?;
    }
    Ok(())
}

fn read_native_config(path: &Path) -> std::io::Result<Option<String>> {
    use std::io::{ErrorKind, Read};
    const LIMIT: u64 = 2 * 1024 * 1024;
    let meta = match path.metadata() {
        Ok(meta) => meta,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !meta.is_file() || meta.len() > LIMIT {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut contents = String::new();
    std::fs::File::open(path)?
        .take(LIMIT + 1)
        .read_to_string(&mut contents)?;
    if contents.len() as u64 > LIMIT {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    Ok(Some(contents))
}

fn inspect_config(path: &Path, fields: &[&str], variable: &str, native: &mut NativeMcp) -> Result<(), String> {
    let raw = match read_native_config(path) {
        Ok(None) => return Ok(()),
        Ok(Some(raw)) => raw,
        Err(_) => {
            native.unresolved();
            return Ok(());
        }
    };
    let value = if path.extension().is_some_and(|extension| extension == "toml") {
        toml::from_str::<toml::Value>(&raw)
            .ok()
            .and_then(|value| serde_json::to_value(value).ok())
    } else if path.extension().is_some_and(|extension| extension == "jsonc") {
        jsonc(&raw)
    } else {
        serde_json::from_str(&raw).ok()
    };
    let Some(value) = value else {
        native.unresolved();
        return Ok(());
    };
    if let Some(servers) = fields
        .iter()
        .try_fold(&value, |value, field| value.get(*field))
        .and_then(serde_json::Value::as_object)
    {
        for (name, entry) in servers {
            if entry.get("command").is_none()
                || entry.get("enabled").and_then(serde_json::Value::as_bool) == Some(false)
                || entry.get("disabled").and_then(serde_json::Value::as_bool) == Some(true)
            {
                continue;
            }
            native.include(name);
            let declared = ["env", "environment"]
                .into_iter()
                .filter_map(|field| entry.get(field).and_then(serde_json::Value::as_object))
                .flat_map(|object| object.keys().map(String::as_str));
            let forwarded = entry
                .get("env_vars")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str);
            if declared.chain(forwarded).any(|name| same_env_name(name, variable)) {
                return Err(format!(
                    "A native MCP server declares {variable}. Rename that secret before starting this session."
                ));
            }
        }
    }
    Ok(())
}
/// JSONC permits comments and trailing commas. Leave all string bytes untouched.
fn jsonc(input: &str) -> Option<serde_json::Value> {
    let mut out = Vec::with_capacity(input.len());
    let mut input = input.bytes().peekable();
    let mut in_string = false;
    let mut comma = false;
    while let Some(byte) = input.next() {
        if in_string {
            out.push(byte);
            match byte {
                b'"' => in_string = false,
                b'\\' => out.push(input.next()?),
                _ => {}
            }
            continue;
        }
        match (byte, input.peek()) {
            (b'/', Some(b'/')) => {
                for byte in input.by_ref() {
                    if byte == b'\n' {
                        break;
                    }
                }
                out.push(b' ');
            }
            (b'/', Some(b'*')) => {
                input.next();
                loop {
                    if input.next()? == b'*' && input.peek() == Some(&b'/') {
                        input.next();
                        break;
                    }
                }
                out.push(b' ');
            }
            _ if byte.is_ascii_whitespace() => out.push(byte),
            _ => {
                if comma && !matches!(byte, b']' | b'}') {
                    out.push(b',');
                }
                comma = byte == b',';
                in_string = byte == b'"';
                if !comma {
                    out.push(byte);
                }
            }
        }
    }
    (!comma).then(|| serde_json::from_slice(&out).ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_native_servers_and_warns_when_the_file_cannot_be_parsed() {
        let temp = std::env::temp_dir().join(format!("nexus-mcp-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).expect("temporary folder");
        let file = temp.join("config.toml");
        std::fs::write(&file, "[mcp_servers.private-db]\ncommand = 'server'\n").expect("config");
        let mut native = NativeMcp::default();
        inspect_config(&file, &["mcp_servers"], "NEXUS_API_KEY", &mut native).expect("safe config");
        assert_eq!(native.names, ["private-db"]);
        assert!(!native.uncertain);

        std::fs::write(
            &file,
            "[mcp_servers.private-db]\ncommand = 'server'\nenv_vars = ['NEXUS_API_KEY']\n",
        )
        .expect("forwarded variable");
        assert!(inspect_config(&file, &["mcp_servers"], "NEXUS_API_KEY", &mut native).is_err());

        std::fs::write(&file, "[invalid").expect("invalid config");
        inspect_config(&file, &["mcp_servers"], "NEXUS_API_KEY", &mut native).expect("warn without refusing");
        assert!(native.uncertain);
        std::fs::remove_dir_all(temp).expect("remove fixture");
    }

    #[test]
    fn jsonc_keeps_secret_declarations_and_string_punctuation() {
        let doc = jsonc(
            r#"{
            // Keep quoted slashes and commas.
            "mcp": { "db": {
                "command": ["http://fixture/,]"],
                "environment": { "NEXUS_OPENCODE_API_KEY": /* declared */ "", },
            }, },
        }"#,
        )
        .expect("OpenCode JSONC");
        assert_eq!(doc["mcp"]["db"]["command"][0], "http://fixture/,]");
        assert_eq!(doc["mcp"]["db"]["environment"]["NEXUS_OPENCODE_API_KEY"], "");
        assert!(jsonc("{/*").is_none());
    }

    #[test]
    fn codex_servers_get_only_their_own_secret_via_the_launcher() {
        use std::cell::RefCell;
        let servers: Vec<McpServer> = serde_json::from_value(serde_json::json!([{
            "name": "db", "command": "start-db", "secrets": ["DB_TOKEN"]
        }]))
        .expect("MCP settings");
        let values = Values::from([(
            "db".into(),
            std::collections::BTreeMap::from([("DB_TOKEN".into(), "db-own".into())]),
        )]);
        let grant = RefCell::new(None);
        let mut launch = Launch {
            attribution: None,
            spawn: Spawn {
                program: "codex".into(),
                args: vec!["app-server".into()],
                cwd: ".".into(),
                env: HashMap::from([("NEXUS_API_KEY".into(), "credential".into())]),
            },
            nexus_session: None,
            local_run: Some("local-test".into()),
            model: None,
            local_provider: Some(Run {
                id: InstanceId::new("key:codex".into()),
                kind: ProviderKind::ApiKey,
                native_mcp: NativeMcp::default(),
            }),
            session_mcp: Default::default(),
        };
        mcp_servers::configure_local(
            &mut launch,
            SessionAgent::Codex,
            Some(Path::new("launcher")),
            &servers,
            &values,
            |_, launchable| {
                grant.replace(Some(launchable));
                Some(("socket".into(), "grant".into()))
            },
        );
        let configured = launch.spawn.args.join(" ");
        assert!(configured.contains("command=\"launcher\""));
        assert!(configured.contains("--mcp-launch=db"));
        assert!(!configured.contains("DB_TOKEN"));
        assert!(!launch.spawn.env.contains_key("DB_TOKEN"));
        let launchables = grant.into_inner().expect("launcher grant");
        let server = &launchables["db"];
        assert_eq!(
            server.env,
            std::collections::BTreeMap::from([("DB_TOKEN".into(), "db-own".into())])
        );
        let child = crate::mcp_launch::command(server, [("NEXUS_API_KEY".into(), "credential".into())]);
        assert!(
            child
                .get_envs()
                .any(|(name, value)| name == "NEXUS_API_KEY" && value.is_none())
        );
    }
}
