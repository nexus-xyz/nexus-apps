//! The CLI's config file (`config.ts`), shared with the TypeScript CLI so existing sign-ins keep working:
//! `~/.nexus/factory/config.json` (or `NEXUS_CONFIG`), `{ url, apiKey?, kind?, chatSession?, gateways? }`, written
//! `0600` in a `0700` folder. Not `~/.nexus/config.json`: that file belongs to another Nexus CLI.
//!
//! The top-level fields are the default Gateway's sign-in. `gateways` keeps the sign-in of every other Gateway, by URL
//! (`{ "<url>": { apiKey, kind, org, … } }`), and `NEXUS_GATEWAY=<url>` picks one of them for a single command.
//!
//! Also the managed config ([`ManagedConfig`]): a file an administrator (or device management) puts on the machine
//! to preset the Gateway and the organization for every native client.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Production Nexus Gateway.
pub const GATEWAY_URL: &str = "https://api.nexus.inc";
/// The Gateway when none is saved and `NEXUS_URL` is unset: production's.
pub const DEFAULT_URL: &str = GATEWAY_URL;
/// The API Gateway in local development.
pub const LOCAL_URL: &str = "http://localhost:3100";
/// Nexus UI in local development: the landing app's `/app`.
pub const DEFAULT_UI_URL: &str = "http://localhost:3000/app";

/// One of Nexus's own Gateways, for `nexus auth login --env`.
pub struct Environment {
    pub name: &'static str,
    pub url: &'static str,
    /// Its Nexus UI, for links; `None`: `/app` on the Gateway URL.
    pub ui_url: Option<&'static str>,
}

/// The dev Gateway and its Nexus UI, named at build time (`NEXUS_DEV_GATEWAY_URL`, `NEXUS_DEV_UI_URL`). A build
/// without `NEXUS_DEV_GATEWAY_URL` (the desktop app's, for one) has no `dev` environment.
pub const DEV_GATEWAY_URL: Option<&str> = option_env!("NEXUS_DEV_GATEWAY_URL");
const DEV_UI_URL: Option<&str> = option_env!("NEXUS_DEV_UI_URL");

const PROD: Environment = Environment {
    name: "prod",
    url: GATEWAY_URL,
    ui_url: Some("https://nexus.inc/app"),
};

/// Nexus's own Gateways.
pub const ENVIRONMENTS: &[Environment] = match DEV_GATEWAY_URL {
    Some(url) => &[
        PROD,
        Environment {
            name: "dev",
            url,
            ui_url: DEV_UI_URL,
        },
    ],
    None => &[PROD],
};

/// The environment `--env` names: `prod` or `dev`; `prd` and `production` work too.
pub fn environment(name: &str) -> Option<&'static Environment> {
    let name = match name.trim().to_ascii_lowercase().as_str() {
        "prd" | "production" => "prod".to_string(),
        other => other.to_string(),
    };
    ENVIRONMENTS.iter().find(|env| env.name == name)
}

fn environment_at(url: &str) -> Option<&'static Environment> {
    ENVIRONMENTS.iter().find(|env| same_gateway(env.url, url))
}

/// A saved Gateway URL on a retired host (a `*.nexus.xyz` name outside Nexus's own environments under
/// `*.factory.nexus.xyz`): production's. Only for a URL a person saved; an env var, a managed URL or a flag is used as
/// given. The environments are matched by name, not by `ENVIRONMENTS`, so a build without the dev Gateway (the desktop
/// app's) never moves a dev sign-in to production.
pub fn moved_gateway(url: &str) -> Option<&'static str> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let environment = host.ends_with(".factory.nexus.xyz");
    let retired = host.ends_with(".nexus.xyz") && !environment;
    retired.then_some(GATEWAY_URL)
}

/// The saved config. Fields the CLI does not know are kept as they are when it saves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    pub url: String,
    pub api_key: Option<String>,
    /// `cli`: a personal token from `nexus auth login` (revoked on logout); `api`: an API key (only forgotten).
    pub kind: Option<String>,
    /// The chat session `nexus chat` continues.
    pub chat_session: Option<String>,
    /// Nexus UI's base URL for links; set at runtime, never saved.
    pub ui_url: Option<String>,
    /// Fields this CLI does not know, kept when it saves (as the TypeScript CLI's object spread does).
    pub extra: Map<String, Value>,
}

impl Config {
    pub fn with_url(url: &str) -> Config {
        Config {
            url: url.to_string(),
            ..Config::default()
        }
    }

    /// The model `nexus chat` uses when no --model is given (`nexus models pick`); `None`: nexus/auto.
    pub fn chat_model(&self) -> Option<&str> {
        self.extra.get("chatModel").and_then(Value::as_str)
    }

    fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("url".into(), Value::String(self.url.clone()));
        if let Some(key) = &self.api_key {
            map.insert("apiKey".into(), Value::String(key.clone()));
        }
        if let Some(kind) = &self.kind {
            map.insert("kind".into(), Value::String(kind.clone()));
        }
        if let Some(session) = &self.chat_session {
            map.insert("chatSession".into(), Value::String(session.clone()));
        }
        for (key, value) in &self.extra {
            map.entry(key.clone()).or_insert_with(|| value.clone());
        }
        Value::Object(map)
    }
}

/// `NEXUS_CONFIG`, else `~/.nexus/factory/config.json`.
pub fn default_config_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(path) = env("NEXUS_CONFIG") {
        return PathBuf::from(path);
    }
    home_dir(env).join(".nexus").join("factory").join("config.json")
}

pub fn home_dir(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("HOME")
        .or_else(|| env("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The saved config; with none (or one that is not ours), Gateway is `NEXUS_URL` or local. `NEXUS_GATEWAY` picks
/// another Gateway for this command: its sign-in from `gateways`, if any, and the default stays as it is. A saved
/// default on a retired host moves to production's, and the file is saved with it (best effort); `NEXUS_GATEWAY` is
/// used as given.
pub fn load_config(path: &Path, env: &dyn Fn(&str) -> Option<String>) -> Config {
    let chosen = chosen_gateway(env);
    let Some(saved) = read_saved(path) else {
        let fallback = chosen
            .or_else(|| env("NEXUS_URL").filter(|url| !url.is_empty()))
            .unwrap_or_else(|| DEFAULT_URL.to_string());
        return Config::with_url(&fallback);
    };
    let url = saved_url(&saved);
    match chosen {
        Some(chosen) if !same_gateway(&url, &chosen) => {
            let entry = saved
                .get("gateways")
                .and_then(|gateways| gateways.get(&chosen))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            from_saved(chosen, &entry)
        }
        Some(_) => from_saved(url, &saved),
        None => match moved_gateway(&url) {
            Some(moved) => {
                let mut saved = saved;
                saved.insert("url".into(), Value::String(moved.to_string()));
                let gateways = take_gateways(&mut saved);
                let config = from_saved(moved.to_string(), &saved);
                let _ = write_config(path, saved, gateways);
                config
            }
            None => from_saved(url, &saved),
        },
    }
}

/// `NEXUS_GATEWAY`, without a trailing slash; `None` when unset or empty.
fn chosen_gateway(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    env("NEXUS_GATEWAY")
        .map(|url| trim_url(url.trim()).to_string())
        .filter(|url| !url.is_empty())
}

fn trim_url(url: &str) -> &str {
    url.strip_suffix('/').unwrap_or(url)
}

fn same_gateway(a: &str, b: &str) -> bool {
    trim_url(a) == trim_url(b)
}

/// The config file as a JSON object, when it is ours (it has a `url`).
fn read_saved(path: &Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(saved) if saved.get("url").is_some_and(Value::is_string) => Some(saved),
        _ => None,
    }
}

fn saved_url(saved: &Map<String, Value>) -> String {
    saved.get("url").and_then(Value::as_str).unwrap_or_default().to_string()
}

fn from_saved(url: String, saved: &Map<String, Value>) -> Config {
    let text = |key: &str| saved.get(key).and_then(Value::as_str).map(String::from);
    let extra = saved
        .iter()
        .filter(|(key, _)| !["url", "apiKey", "kind", "chatSession", "uiUrl", "gateways"].contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Config {
        url,
        api_key: text("apiKey"),
        kind: text("kind"),
        chat_session: text("chatSession"),
        ui_url: None,
        extra,
    }
}

/// Save `config`'s sign-in: as the default when it is the saved Gateway (or none is saved), else under `gateways`,
/// leaving the default as it is. A Gateway left with no key is dropped from `gateways`.
pub fn save_config(path: &Path, config: &Config) -> std::io::Result<()> {
    match read_saved(path) {
        Some(mut saved) if !same_gateway(&saved_url(&saved), &config.url) => {
            let mut gateways = take_gateways(&mut saved);
            let mut entry = config.to_json();
            if let Value::Object(entry) = &mut entry {
                entry.shift_remove("url");
            }
            let url = trim_url(&config.url).to_string();
            if config.api_key.is_some() {
                gateways.insert(url, entry);
            } else {
                gateways.shift_remove(&url);
            }
            write_config(path, saved, gateways)
        }
        saved => {
            let gateways = saved.map(|mut saved| take_gateways(&mut saved)).unwrap_or_default();
            write_config(path, object(config.to_json()), gateways)
        }
    }
}

/// Save `config`'s sign-in and make its Gateway the default (`nexus auth login --url`). The previous default's sign-in
/// moves to `gateways`, so `NEXUS_GATEWAY` still reaches it.
pub fn save_default_config(path: &Path, config: &Config) -> std::io::Result<()> {
    let mut gateways = Map::new();
    if let Some(mut saved) = read_saved(path) {
        gateways = take_gateways(&mut saved);
        let url = saved_url(&saved);
        if !same_gateway(&url, &config.url) && saved.get("apiKey").is_some() {
            saved.shift_remove("url");
            gateways.insert(trim_url(&url).to_string(), Value::Object(saved));
        }
    }
    gateways.shift_remove(trim_url(&config.url));
    write_config(path, object(config.to_json()), gateways)
}

fn take_gateways(saved: &mut Map<String, Value>) -> Map<String, Value> {
    match saved.shift_remove("gateways") {
        Some(Value::Object(gateways)) => gateways,
        _ => Map::new(),
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn write_config(path: &Path, mut top: Map<String, Value>, gateways: Map<String, Value>) -> std::io::Result<()> {
    top.shift_remove("gateways");
    if !gateways.is_empty() {
        top.insert("gateways".into(), Value::Object(gateways));
    }
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        create_private_dir(dir)?;
    }
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(top)).unwrap_or_default()
    );
    write_private(path, text.as_bytes())
}

/// `mkdir -p` with mode 0700 for the folders it creates.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Write a file readable by its owner only (a new file is created 0600; an existing one keeps its mode, as Node's
/// `writeFileSync` with `mode` does).
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options.open(path)?.write_all(bytes)
}

/// Nexus UI's base URL: `NEXUS_UI_URL`; else the UI of one of Nexus's own Gateways; else, for the API Gateway on its
/// local default port, the landing app's dev server; else `/app` on the Gateway URL itself. The installer's wrapper
/// sets `NEXUS_UI_URL` with `NEXUS_URL`, so `NEXUS_UI_URL` counts only when `NEXUS_URL` is unset or is this Gateway:
/// signed in to another Gateway, the links open that Gateway's UI.
pub fn ui_url(core_url: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    let paired = env("NEXUS_URL")
        .filter(|url| !url.is_empty())
        .is_none_or(|url| same_gateway(url.trim(), core_url));
    if let Some(configured) = env("NEXUS_UI_URL").filter(|url| paired && !url.is_empty()) {
        return configured.strip_suffix('/').unwrap_or(&configured).to_string();
    }
    if let Some(ui) = environment_at(core_url).and_then(|env| env.ui_url) {
        return ui.to_string();
    }
    let gateway_only = match reqwest::Url::parse(core_url) {
        Ok(url) => {
            let host = url.host_str().unwrap_or("");
            ["localhost", "127.0.0.1", "[::1]"].contains(&host) && url.port() == Some(3100)
        }
        Err(_) => true,
    };
    if gateway_only {
        DEFAULT_UI_URL.to_string()
    } else {
        format!("{}/app", core_url.strip_suffix('/').unwrap_or(core_url))
    }
}

/// What an administrator presets for every native client on a machine (enterprise deployment through device
/// management): the Gateway URL, the organization people sign in to, Nexus UI's URL for links, and whether people may
/// point the app elsewhere. A JSON file at [`managed_config_path`]; absent or unreadable, nothing is preset.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedConfig {
    /// Nexus Gateway's URL (`https://nexus.example.com`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The organization to sign in to and open: its slug (`acme`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// Nexus UI's base URL, for links that open in the browser (default: `/app` on the Gateway URL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_url: Option<String>,
    /// When true, people cannot change the Gateway URL in the app.
    #[serde(default)]
    pub lock_url: bool,
}

/// Where the managed config lives: `NEXUS_MANAGED_CONFIG` when set, else, per OS,
/// macOS `/Library/Application Support/Nexus/managed.json`, Windows `%ProgramData%\Nexus\managed.json`
/// (`C:\ProgramData` when unset), Linux and the rest `/etc/nexus/managed.json`. Machine-wide and writable by
/// administrators only, so a person cannot change what their organization preset.
pub fn managed_config_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(path) = env("NEXUS_MANAGED_CONFIG").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/Nexus/managed.json")
    } else if cfg!(windows) {
        let data = env("ProgramData")
            .filter(|dir| !dir.is_empty())
            .unwrap_or_else(|| "C:\\ProgramData".into());
        PathBuf::from(data).join("Nexus").join("managed.json")
    } else {
        PathBuf::from("/etc/nexus/managed.json")
    }
}

/// The managed config at `path`; `None` when there is none or it is not a JSON object of ours.
pub fn load_managed_config(path: &Path) -> Option<ManagedConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut managed: ManagedConfig = serde_json::from_str(&text).ok()?;
    managed.url = managed
        .url
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty());
    managed.org = managed
        .org
        .map(|org| org.trim().to_string())
        .filter(|org| !org.is_empty());
    Some(managed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn a_saved_gateway_on_a_retired_host_moves_to_production() {
        assert_eq!(moved_gateway("https://api.nexus.xyz/"), Some(GATEWAY_URL));
        assert_eq!(moved_gateway("https://gateway.old.nexus.xyz"), Some(GATEWAY_URL));
        // Nexus's own environments stay where they are, whether or not this build names the dev Gateway.
        assert_eq!(moved_gateway("https://any.factory.nexus.xyz"), None);
        if let Some(dev) = DEV_GATEWAY_URL {
            assert_eq!(moved_gateway(dev), None);
        }
        assert_eq!(moved_gateway("https://nexus.acme.test"), None);
        assert_eq!(moved_gateway("http://localhost:3100"), None);

        let dir = std::env::temp_dir().join(format!("nexus-cfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"url":"https://api.nexus.xyz","apiKey":"nx_1","org":"acme"}"#).expect("write");
        let chosen = load_config(&path, &env(&[("NEXUS_GATEWAY", "https://api.nexus.xyz/")]));
        assert_eq!(chosen.url, "https://api.nexus.xyz", "NEXUS_GATEWAY is used as given");
        assert_eq!(chosen.api_key.as_deref(), Some("nx_1"));
        let loaded = load_config(&path, &env(&[("NEXUS_URL", "https://core.example")]));
        assert_eq!(loaded.url, GATEWAY_URL);
        assert_eq!(loaded.api_key.as_deref(), Some("nx_1"));
        assert_eq!(load_config(&path, &env(&[])), loaded, "saved with the new URL");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_its_own_file_apart_from_the_prover_cli() {
        assert!(default_config_path(&env(&[("HOME", "/h")])).ends_with(".nexus/factory/config.json"));
        let dir = std::env::temp_dir().join(format!("nexus-cfg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let foreign = dir.join("config.json");
        std::fs::write(&foreign, r#"{"environment":"beta","node_id":"1"}"#).expect("write");
        assert_eq!(
            load_config(&foreign, &env(&[("NEXUS_URL", "https://core.example")])),
            Config::with_url("https://core.example")
        );
        assert_eq!(
            load_config(&dir.join("missing.json"), &env(&[])),
            Config::with_url(DEFAULT_URL)
        );
        let saved = Config {
            url: "http://core.test".into(),
            api_key: Some("nx_1".into()),
            kind: Some("cli".into()),
            chat_session: Some("chs_1".into()),
            ui_url: Some("ignored".into()),
            extra: Map::new(),
        };
        save_config(&dir.join("sub/config.json"), &saved).expect("save");
        let text = std::fs::read_to_string(dir.join("sub/config.json")).expect("read");
        assert_eq!(
            text,
            "{\n  \"url\": \"http://core.test\",\n  \"apiKey\": \"nx_1\",\n  \"kind\": \"cli\",\n  \"chatSession\": \"chs_1\"\n}\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn keeps_one_sign_in_per_gateway() {
        let dir = std::env::temp_dir().join(format!("nexus-gw-{}", uuid::Uuid::new_v4()));
        let path = dir.join("config.json");
        let signed_in = |url: &str, key: &str| Config {
            api_key: Some(key.into()),
            kind: Some("cli".into()),
            ..Config::with_url(url)
        };
        let dev = env(&[("NEXUS_GATEWAY", "https://dev.test/")]);
        // With nothing saved, NEXUS_GATEWAY's Gateway is the one saved.
        assert_eq!(load_config(&path, &dev), Config::with_url("https://dev.test"));
        save_config(&path, &signed_in("https://prod.test", "nx_prod")).expect("save");
        // Another Gateway: no sign-in yet; saving it leaves the default as it is.
        assert_eq!(load_config(&path, &dev), Config::with_url("https://dev.test"));
        save_config(&path, &signed_in("https://dev.test", "nx_dev")).expect("save");
        assert_eq!(load_config(&path, &env(&[])), signed_in("https://prod.test", "nx_prod"));
        assert_eq!(load_config(&path, &dev), signed_in("https://dev.test", "nx_dev"));
        assert_eq!(
            load_config(&path, &env(&[("NEXUS_GATEWAY", "https://prod.test")])),
            signed_in("https://prod.test", "nx_prod")
        );
        // The default's own saves keep the other Gateways.
        save_config(&path, &Config::with_url("https://prod.test")).expect("logout");
        assert_eq!(load_config(&path, &dev), signed_in("https://dev.test", "nx_dev"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "{\n  \"url\": \"https://prod.test\",\n  \"gateways\": {\n    \"https://dev.test\": {\n      \"apiKey\": \"nx_dev\",\n      \"kind\": \"cli\"\n    }\n  }\n}\n"
        );
        // `nexus auth login --url`: the new default; the previous one's sign-in moves to `gateways`.
        save_config(&path, &signed_in("https://prod.test", "nx_prod")).expect("login");
        save_default_config(&path, &signed_in("https://dev.test", "nx_dev2")).expect("login --url");
        assert_eq!(load_config(&path, &env(&[])), signed_in("https://dev.test", "nx_dev2"));
        let prod = env(&[("NEXUS_GATEWAY", "https://prod.test")]);
        assert_eq!(load_config(&path, &prod), signed_in("https://prod.test", "nx_prod"));
        // Signing out of another Gateway drops it.
        save_config(&path, &Config::with_url("https://prod.test")).expect("logout");
        assert_eq!(load_config(&path, &prod), Config::with_url("https://prod.test"));
        assert!(!std::fs::read_to_string(&path).expect("read").contains("gateways"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ui_links_follow_the_core() {
        assert_eq!(ui_url(LOCAL_URL, &env(&[])), DEFAULT_UI_URL);
        assert_eq!(ui_url("https://core.example/", &env(&[])), "https://core.example/app");
        assert_eq!(ui_url("https://api.nexus.inc/", &env(&[])), "https://nexus.inc/app");
        assert_eq!(
            ui_url("http://x", &env(&[("NEXUS_UI_URL", "http://localhost:4000/app/")])),
            "http://localhost:4000/app"
        );
    }

    #[test]
    fn the_wrappers_ui_url_is_for_the_wrappers_gateway_only() {
        // The wrapper of an installer from the dev Gateway, then a sign-in to production.
        let wrapper = env(&[
            ("NEXUS_URL", "https://gateway.dev.test"),
            ("NEXUS_UI_URL", "https://preview.dev.test/app"),
        ]);
        assert_eq!(ui_url(GATEWAY_URL, &wrapper), "https://nexus.inc/app");
        assert_eq!(
            ui_url("https://gateway.dev.test/", &wrapper),
            "https://preview.dev.test/app"
        );
    }

    #[test]
    fn names_nexuss_own_gateways() {
        assert_eq!(environment("prod").map(|env| env.url), Some(GATEWAY_URL));
        assert_eq!(environment(" PRD ").map(|env| env.name), Some("prod"));
        assert_eq!(environment("stg").map(|env| env.name), None);
        assert_eq!(environment("dev").map(|env| env.url), DEV_GATEWAY_URL);
        assert!(environment("qa").is_none());
    }

    #[test]
    fn reads_the_managed_config() {
        let dir = std::env::temp_dir().join(format!("nexus-managed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("managed.json");
        assert_eq!(
            managed_config_path(&env(&[("NEXUS_MANAGED_CONFIG", "/x/m.json")])),
            PathBuf::from("/x/m.json")
        );
        assert!(managed_config_path(&env(&[])).ends_with("managed.json"));
        assert_eq!(load_managed_config(&file), None);
        std::fs::write(
            &file,
            r#"{"url":" https://nexus.acme.test/ ","org":"acme","lockUrl":true,"extra":1}"#,
        )
        .expect("write");
        assert_eq!(
            load_managed_config(&file),
            Some(ManagedConfig {
                url: Some("https://nexus.acme.test".into()),
                org: Some("acme".into()),
                ui_url: None,
                lock_url: true,
            })
        );
        std::fs::write(&file, "[1]").expect("write");
        assert_eq!(load_managed_config(&file), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
