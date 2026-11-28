//! What the app remembers that is not secret (`settings.json` in the app's config folder): the Gateway, the
//! organization last signed in to, the kind of credential, the global shortcuts (the panel's, and the screen
//! recording's), launch at login, the menu-bar-only mode, the MCP servers, the panel's size and the keys a person set for Code's shortcuts. An administrator's managed config (src/policy.rs: a configuration profile, Group Policy or the managed JSON
//! file) presets the Gateway and the organization and can lock the Gateway; it always wins over what a person set.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nexus_client::config::{ManagedConfig, load_managed_config, managed_config_path};

use crate::chat_width::ChatWidth;
use crate::policy::{self, Policy};
use nexus_workbench::session_choice::Preference;
use nexus_workbench::{Limits, Subsessions};
use serde::{Deserialize, Serialize};

/// Where a fresh install points before anyone changes it: production Nexus Gateway.
pub const DEFAULT_GATEWAY: &str = "https://api.nexus.inc";
/// The global shortcut that opens the panel: ⌥Space (Alt+Space on Windows and Linux). Kept over the Swift app's ⌥⌘N:
/// one key chord people already use for the desktop app and for other assistants' panels, and easy to reach with one
/// hand; changeable in the panel's settings, and turned off by `GlobalShortcutEnabled`.
pub const DEFAULT_SHORTCUT: &str = "Alt+Space";
/// The shortcut that starts and stops a screen recording for an agent: ⌥⇧S, as Scrawl's (the tool this follows), one
/// hand on the left. macOS only (crates/nexus-desktop-record).
pub const DEFAULT_RECORD_SHORTCUT: &str = "Alt+Shift+S";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_url: Option<String>,
    /// The organization's segment in Nexus UI's URLs (its slug, else its id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    /// `cli`: a personal token from the device sign-in (revoked on sign-out); `api`: an API key (only forgotten).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
    /// The shortcut that starts and stops a screen recording for an agent (macOS; default ⌥⇧S, crates/nexus-desktop-record).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_shortcut: Option<String>,
    /// Nexus UI's base URL for links, when it is not `/app` on the Gateway (local development).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_url: Option<String>,
    /// macOS: no Dock icon and no window at launch, only the menu-bar item and its panel (the retired Swift app's
    /// way). Windows and Linux: no taskbar window at launch, only the tray. Default false: a normal app with a window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu_bar_only: Option<bool>,
    /// The person's choice to open Nexus at login (unless the managed `LaunchAtLogin` decides).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_at_login: Option<bool>,
    /// The extra Claude account new sessions on the plan start on (Plan usage); None: Claude Code's own login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_account: Option<String>,
    /// Whether sessions move to the Claude account with the most left near a limit. Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate_accounts: Option<bool>,
    /// The extra ChatGPT account new Codex sessions on the plan start on; None: Codex's own login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_account: Option<String>,
    /// Whether a new Codex session starts on the ChatGPT account with the most left when the chosen one is near a
    /// limit. A running Codex session keeps its account. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate_codex_accounts: Option<bool>,
    /// The agents (`claude`, `codex`) whose plan account the person chose themselves, in Settings or in "Choose your
    /// Claude account". With more than one login of an agent and no choice here, New session asks first: Nexus never
    /// picks one of several by itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chosen_accounts: Vec<String>,
    /// Each Nexus organization's plan accounts, by the organization's key: agent (`claude`, `codex`) to the account its
    /// new sessions start on there ([`OWN_LOGIN`] for the agent's own login). Entering an organization applies its own
    /// ([`Settings::enter_org`]), so a personal organization never takes over a work login.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub org_accounts: BTreeMap<String, BTreeMap<String, String>>,
    /// Antigravity's Sandbox switch (Settings › Agents › Antigravity): agy starts with `--sandbox`, its terminal
    /// sandbox, in every mode. Read as agy starts. Default false ([`Settings::antigravity_sandboxed`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antigravity_sandbox: Option<bool>,
    /// Whether Code sessions show a system notification when they finish or need the person. Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_notifications: Option<bool>,
    /// Whether this computer shows the organization's notifications ([`crate::path_notices`]). Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_notifications: Option<bool>,
    /// Whether a Code session's Browser opens on its own when the agent starts a local dev server
    /// (`auto-preview`). Default true ([`Settings::opens_browser_on_server`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_browser_on_server: Option<bool>,
    /// Whether a Code session whose pull request merged goes to Archived instead of Settled (Settings › Code). Default
    /// false ([`Settings::archives_merged`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_merged: Option<bool>,
    /// Whether scripts, the `nexus` CLI and other agents may ask for access to Code sessions (src/outside/). Default
    /// false: no socket listens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outside_access: Option<bool>,
    /// Read conversations created outside Nexus for Computer > History. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_session_history: Option<bool>,
    /// Where new Code sessions' worktrees go; None: `~/nexus-worktrees` (`workbench::worktree_roots`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktrees_dir: Option<String>,
    /// Where Code sessions that review a pull request check it out; None: `~/nexus-reviews`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviews_dir: Option<String>,
    /// The rules that remove Code sessions' worktrees by themselves (src/worktree_cleanup.rs); None: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_cleanup: Option<nexus_workbench::cleanup::Rules>,
    /// What new worktrees' branches start with; None: `nexus/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_prefix: Option<String>,
    /// The MCP servers every Code session gets (src/mcp.rs); their secrets' values are in the keychain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,
    /// The person's hooks (Settings › Hooks, [`crate::hooks`]): a command run on a Code session event. Only this file
    /// sets them; an entry that does not read is dropped, not the whole file ([`crate::hooks::lenient`]).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::hooks::lenient"
    )]
    pub hooks: Vec<crate::hooks::Hook>,
    /// "Keep agents running when Nexus quits" (src/service.rs): this computer's Code sessions in a background service.
    /// macOS and Linux; default false. Takes effect when the app starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_agents_running: Option<bool>,
    /// Keep this computer and display awake during agent work in GPUI. Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_awake: Option<bool>,
    /// The Code workbench's remote machines (src/machines.rs): servers reached over SSH, and cloud machines the app
    /// started (src/cloud.rs). "This computer" is not listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machines: Vec<MachineSetting>,
    /// The cloud setup on this computer (src/cloud.rs): when the provider key and the Claude token were saved, and a
    /// hint of each. The secrets themselves are in the keychain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<CloudAccountSetting>,
    /// The panel's size as the person last left it (width, height in points; `panel::size`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_size: Option<[f64; 2]>,
    /// The Mac chat bar (GPUI app, macOS): the panel's shortcut opens the small bar over any app instead of the panel.
    /// On unless the person turned it off (Settings › App, `opens_chat_bar`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_bar: Option<bool>,
    /// The limits on subsessions ([`OrchestrationLimits`]): off unless the block says `limitsOn`. A block that does not
    /// read is no block, so no limit ([`lenient_limits`]).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_limits"
    )]
    pub orchestration: Option<OrchestrationLimits>,
    /// Whether new sessions, schedules' runs and starts from another device get subsessions: their agent may start
    /// other sessions through `nexus_app`. A session's own switch changes one session. Default true
    /// ([`Settings::subsessions_on`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subsessions: Option<bool>,
    /// The default first-start choice for projects without a saved preference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subsessions_first_start: Option<Preference>,
    /// "Disable cross-agent subsessions": a subsession runs only its parent's agent, on every route. Off by default,
    /// and written only when on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_cross_agent_subsessions: bool,
    /// The Gateways signed in to before, other than the current one (`gateway_url`, whose organization and kind are
    /// `org` and `kind`). Settings lists them to switch back without signing in again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gateways: Vec<KnownGateway>,
    /// Code sessions whose agent is sent its pull request's failed checks and requested changes on its own
    /// (src/pr_watch.rs), by session id, with how many times it was.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pull_request_wakes: BTreeMap<String, AutoWake>,
    /// The agent API's base URL for the agent pane (src/agent.rs); `NEXUS_AGENTS_API_URL` wins. None: the pane says
    /// the agent is not available here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents_api_url: Option<String>,
    /// How wide the conversation and the composer are (Settings › Appearance; [`crate::chat_width`]). None: the
    /// default, Standard. A block that does not read is the default too ([`lenient_chat_width`]).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_chat_width"
    )]
    pub chat_width: Option<ChatWidth>,
    /// The interface's language as a locale code (`es`, `pt-BR`; Settings › Appearance, the GPUI app's `i18n.rs`).
    /// None: the computer's language when the app has it, else English.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// The keys the person set for Code's shortcuts (Settings › Keyboard shortcuts; [`crate::key_bindings`]), by
    /// shortcut id: only the changes, null for a cleared key. An entry that does not read
    /// keeps its shortcut's default.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::key_bindings::lenient"
    )]
    pub shortcuts: crate::key_bindings::Bindings,
    /// Whether the side pane's Changes follows the agent's edits: each file it edits opens there
    /// (`follow-edits`). Default true ([`Settings::follows_edits`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_edits: Option<bool>,
}

impl Settings {
    /// "Follow the agent's edits" (Settings › App): on unless the person turned it off.
    pub fn follows_edits(&self) -> bool {
        self.follow_edits.unwrap_or(true)
    }
}

/// [`Settings::org_accounts`]' name for an agent's own login.
pub const OWN_LOGIN: &str = "own";

/// Updates the selected account for an agent.
pub type AgentAccountSetter = fn(&mut Settings, Option<String>);

impl Settings {
    /// The account `org` chose for `agent`: `Some(None)` for its own login, `None` when it chose none yet.
    pub fn org_account(&self, org: &str, agent: &str) -> Option<Option<String>> {
        let pick = self.org_accounts.get(org)?.get(agent)?;
        Some((pick != OWN_LOGIN).then(|| pick.clone()))
    }

    /// `org` now uses `account` (None: the agent's own login) for `agent`.
    pub fn remember_org_account(&mut self, org: &str, agent: &str, account: Option<&str>) {
        self.org_accounts
            .entry(org.to_string())
            .or_default()
            .insert(agent.to_string(), account.unwrap_or(OWN_LOGIN).to_string());
    }

    /// The window entered `org`: each agent's new sessions start on the account `org` chose, and `chosen_accounts`
    /// lists exactly the agents it chose for. An agent it chose nothing for keeps no choice from the organization left
    /// behind, so New session asks before it starts there. `agents` are the agents whose accounts the app keeps, with
    /// a function that sets each one's account. True when anything changed.
    ///
    /// An agent with no pick keeps its `*_account` value from before: New session's "Choose your … account" is the
    /// guard that asks first, so a caller that starts a plan session without that check must not rely on the value.
    pub fn enter_org(&mut self, org: &str, agents: &[(&str, AgentAccountSetter)]) -> bool {
        let before = self.clone();
        // Settings from before organizations kept picks: the first organization entered keeps the choice made then,
        // so the person is not asked again.
        if self.org_accounts.is_empty() {
            for agent in self.chosen_accounts.clone() {
                let account = match agent.as_str() {
                    "claude" => self.claude_account.clone(),
                    "codex" => self.codex_account.clone(),
                    _ => continue,
                };
                self.remember_org_account(org, &agent, account.as_deref());
            }
        }
        self.chosen_accounts.clear();
        for (agent, set) in agents {
            if let Some(account) = self.org_account(org, agent) {
                set(self, account);
                self.chosen_accounts.push((*agent).to_string());
            }
        }
        *self != before
    }
}

/// A Gateway this computer signed in to before: the organization and the credential's kind it had, restored when the
/// person switches back. Its token stays in the keychain (src/secrets.rs: one entry per Gateway).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownGateway {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl Settings {
    /// "Open the browser when the agent starts a server" (Settings › App): on unless the person turned it off.
    pub fn opens_browser_on_server(&self) -> bool {
        self.open_browser_on_server.unwrap_or(true)
    }

    /// The Mac chat bar (Settings › App): on unless the person turned it off.
    pub fn opens_chat_bar(&self) -> bool {
        self.chat_bar.unwrap_or(true)
    }

    /// "Archive merged sessions" (Settings › Code): off unless the person turned it on.
    pub fn archives_merged(&self) -> bool {
        self.archive_merged.unwrap_or(false)
    }

    /// Antigravity's Sandbox switch: off unless the person turned it on.
    pub fn antigravity_sandboxed(&self) -> bool {
        self.antigravity_sandbox.unwrap_or(false)
    }

    /// Make `to` the current Gateway. `from`, the current one, is kept in `gateways` when it is signed in, and `to`'s
    /// organization and kind come back from there (none for a Gateway not signed in to before).
    pub fn switch_gateway(&mut self, from: &str, signed_in: bool, to: &str) {
        if from != to {
            self.gateways.retain(|known| known.url != from);
            let org = self.org.take();
            let kind = self.kind.take();
            if signed_in {
                self.gateways.push(KnownGateway {
                    url: from.to_string(),
                    org,
                    kind,
                });
            }
            if let Some(at) = self.gateways.iter().position(|known| known.url == to) {
                let back = self.gateways.remove(at);
                self.org = back.org;
                self.kind = back.kind;
            }
        }
        self.gateway_url = Some(to.to_string());
    }

    /// The Gateways Settings offers, as (URL, organization): `current` first, then those signed in to before, then
    /// production when it is not one of them.
    pub fn known_gateways(&self, current: &str) -> Vec<(String, Option<String>)> {
        let mut known = vec![(current.to_string(), self.org.clone())];
        for gateway in &self.gateways {
            if known.iter().all(|(url, _)| *url != gateway.url) {
                known.push((gateway.url.clone(), gateway.org.clone()));
            }
        }
        if known.iter().all(|(url, _)| url != DEFAULT_GATEWAY) {
            known.push((DEFAULT_GATEWAY.to_string(), None));
        }
        known
    }
}

/// Which of the two worktree folders: new work's (a branch to open a pull request from), or reviews'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeRoot {
    Worktrees,
    Reviews,
}

/// The most idle days a cleanup rule may wait.
pub const MAX_IDLE_DAYS: u32 = 365;

/// The edits Settings makes, each checked first: a refused one changes nothing. The shell saves the file after.
impl Settings {
    /// Where new sessions' worktrees (or reviews) go; None: back to the default. Sessions already made keep theirs.
    pub fn set_worktree_root(&mut self, root: WorktreeRoot, path: Option<String>) -> Result<(), String> {
        let path = path.filter(|path| !path.trim().is_empty());
        if let Some(path) = &path
            && !Path::new(path).is_absolute()
        {
            return Err(format!("{path} is not a full path."));
        }
        match root {
            WorktreeRoot::Worktrees => self.worktrees_dir = path,
            WorktreeRoot::Reviews => self.reviews_dir = path,
        }
        Ok(())
    }

    /// The rules that remove worktrees by themselves. 0 idle days is off; none on keeps no rules at all.
    pub fn set_cleanup_rules(&mut self, mut rules: nexus_workbench::cleanup::Rules) -> Result<(), String> {
        rules.idle_days = rules.idle_days.filter(|days| *days > 0);
        if rules.idle_days.is_some_and(|days| days > MAX_IDLE_DAYS) {
            return Err(format!("Pick at most {MAX_IDLE_DAYS} days."));
        }
        self.worktree_cleanup = rules.any().then_some(rules);
        Ok(())
    }

    /// What new worktrees' branches start with; None: back to `nexus/`. Answers the prefix in effect, which the shell
    /// gives its running workbench (`Workbench::set_branch_prefix`).
    pub fn set_branch_prefix(&mut self, prefix: Option<String>) -> Result<String, String> {
        let prefix = prefix.map(|prefix| prefix.trim().to_string());
        let effective = prefix
            .clone()
            .unwrap_or_else(|| nexus_workbench::git::BRANCH_PREFIX.to_string());
        nexus_workbench::git::check_branch_prefix(&effective)?;
        self.branch_prefix = prefix.filter(|prefix| prefix != nexus_workbench::git::BRANCH_PREFIX);
        Ok(effective)
    }

    /// The branch prefix in effect: the person's, else `nexus/`.
    pub fn branch_prefix_or_default(&self) -> String {
        self.branch_prefix
            .clone()
            .unwrap_or_else(|| nexus_workbench::git::BRANCH_PREFIX.to_string())
    }

    /// Add a server reached over SSH (`ssh`: a host from `~/.ssh/config`, `user@server`, or a Tailscale name), named
    /// `name`, else by its destination. Answers the machine as saved.
    pub fn add_ssh_machine(&mut self, name: &str, ssh: &str) -> Result<MachineSetting, String> {
        let ssh = ssh.trim().to_string();
        if !nexus_remote::ssh::valid_destination(&ssh) {
            return Err(format!("{ssh:?} is not an SSH host."));
        }
        let name = Some(name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| ssh.clone());
        let machine = MachineSetting {
            id: new_machine_id(&self.machines),
            name,
            kind: "ssh".into(),
            ssh,
            cloud: None,
        };
        self.machines.push(machine.clone());
        Ok(machine)
    }
}

/// A new machine's id: unique on this computer, stable once saved.
pub fn new_machine_id(taken: &[MachineSetting]) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut id = format!("m{nanos:x}");
    while taken.iter().any(|m| m.id == id) {
        id.push('0');
    }
    id
}

/// A Gateway address without a trailing `/`. HTTP is permitted only for a local development Gateway.
pub fn gateway_address(address: &str) -> Result<String, String> {
    let url = url::Url::parse(address.trim()).map_err(|_| "Enter a valid Gateway address.".to_string())?;
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("The Gateway address starts with https://.".into());
    }
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use the Gateway's address without a username, password, query or fragment.".into());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// An MCP server every Code session gets (`Settings::mcp_servers`): `nexus_workbench::mcp_servers`'s, which formats it
/// for the agents.
pub use nexus_workbench::mcp_servers::McpServer;

/// A remote machine as settings.json keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineSetting {
    pub id: String,
    pub name: String,
    /// `ssh` (a server the person reaches) or `cloud` (a machine the app starts at a provider, src/cloud.rs).
    pub kind: String,
    /// The `ssh` destination: a host from `~/.ssh/config`, `user@server`, or a Tailscale name. Empty for a cloud
    /// machine, whose address changes on every wake: then left out.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ssh: String,
    /// A cloud machine's provider side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<CloudSetting>,
}

/// A cloud machine's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CloudState {
    /// Nexus has no room for it yet: Gateway makes it when there is.
    Queued,
    #[default]
    Creating,
    Starting,
    Ready,
    Sleeping,
    Stopping,
    Failed,
    Gone,
}

/// A cloud machine as settings.json keeps it (`MachineSetting::cloud`). No secret. Its machine id is Gateway's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CloudSetting {
    /// The machine provider, as Gateway names it.
    pub provider: String,
    pub size: String,
    /// The app's SSH key for the machine (`<app data>/cloud/<machine id>/id_ed25519`).
    pub key: PathBuf,
    /// The app's known_hosts for it, rewritten on every connect.
    pub known_hosts: PathBuf,
    /// The provider's state as last seen.
    #[serde(default)]
    pub state: CloudState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The desktop session it is for, and its title, as Gateway says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The repository its session clones, as the person picked it on this device. The sidebar files the session under
    /// that project before the machine is read. None on another device, and on a machine made before this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// While queued: when Nexus expects room for it (unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_until: Option<i64>,
    /// Why it stopped (`idle`, `resize`), while it is stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// When it fell asleep (unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleeping_since: Option<i64>,
    /// When it was created (unix ms).
    #[serde(default)]
    pub created_at: i64,
}

/// `settings.json`'s `cloud`: when the Claude token was saved, and its hint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CloudAccountSetting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_saved_at: Option<i64>,
    /// The person removed the token on this device: the next connect removes it from the machines too. Without
    /// this, a device with no token leaves the one another device wrote.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub claude_cleared: bool,
    /// Gateway keeps the token saved here: the machines get it from Gateway, not from this device. With
    /// `claude_cleared`, a removal Gateway has not confirmed yet: the next connect asks again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub claude_on_gateway: bool,
    /// The GitHub login the saved token belongs to, and when it was saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_saved_at: Option<i64>,
    /// The Nexus sign-in the secrets above belong to, as a one-way tag of its Gateway and token. Another sign-in, or
    /// none, gets neither secret. Nexus GPUI keeps the tag it finds. None: saved before any tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// A session's switch that sends the agent the message on its own (Settings' `pullRequestWakes`), how many times
/// it did, and the watch its agent armed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoWake {
    #[serde(default)]
    pub on: bool,
    #[serde(default)]
    pub wakes: u32,
    /// The agent asked to be told (`watch_pull_request`): every change wakes it, with the switch off too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent: bool,
    /// The pull request the agent named, by URL or number; None: its branch's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// `orchestration` with a value that does not read (a wrong type, a negative number) is no block, so no limit, rather
/// than lose the whole file, and with it the Gateway and the organization.
fn lenient_limits<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<OrchestrationLimits>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok().flatten())
}

fn lenient_chat_width<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<ChatWidth>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok().flatten())
}

/// `settings.json`'s `orchestration`: the limits on subsessions, applied at once to every tree.
///
/// `limitsOn` turns the limits on. While it is on, each field is one limit: absent or `null` is no limit, and a number
/// is the limit as set, with no ceiling (depth stops at `nexus_workbench::linked::MAX_DEPTH` either way). False, the
/// default, Nexus enforces none, `crewSpendUsd` included, and the fields stay in the file for when limits are on
/// again. A file written before `limitsOn` has no such key: it reads with limits on when it sets any limit, so a cap
/// the person saved then still applies ([`RawOrchestrationLimits`]). `limitsOn` is always written, so a block the
/// person turned off stays off.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "RawOrchestrationLimits")]
pub struct OrchestrationLimits {
    pub limits_on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_live_children: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_live_linked: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_child_worktrees: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_starts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_auto_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_idle_waits: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crew_spend_usd: Option<f64>,
}

/// `orchestration` as the file holds it: `limitsOn` absent in a file written before the switch existed.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOrchestrationLimits {
    #[serde(default)]
    limits_on: Option<bool>,
    #[serde(default)]
    max_depth: Option<u32>,
    #[serde(default)]
    max_live_children: Option<u32>,
    #[serde(default)]
    max_live_linked: Option<u32>,
    #[serde(default)]
    max_child_worktrees: Option<u32>,
    #[serde(default)]
    max_starts: Option<u32>,
    #[serde(default)]
    max_auto_turns: Option<u32>,
    #[serde(default)]
    max_idle_waits: Option<u32>,
    #[serde(default)]
    crew_spend_usd: Option<f64>,
}

impl From<RawOrchestrationLimits> for OrchestrationLimits {
    fn from(raw: RawOrchestrationLimits) -> OrchestrationLimits {
        let any_set = [
            raw.max_depth,
            raw.max_live_children,
            raw.max_live_linked,
            raw.max_child_worktrees,
            raw.max_starts,
            raw.max_auto_turns,
            raw.max_idle_waits,
        ]
        .iter()
        .any(Option::is_some)
            || raw.crew_spend_usd.is_some();
        OrchestrationLimits {
            limits_on: raw.limits_on.unwrap_or(any_set),
            max_depth: raw.max_depth,
            max_live_children: raw.max_live_children,
            max_live_linked: raw.max_live_linked,
            max_child_worktrees: raw.max_child_worktrees,
            max_starts: raw.max_starts,
            max_auto_turns: raw.max_auto_turns,
            max_idle_waits: raw.max_idle_waits,
            crew_spend_usd: raw.crew_spend_usd,
        }
    }
}

impl Settings {
    /// The limits Nexus enforces: none unless the person turned limits on ([`OrchestrationLimits::limits`]).
    pub fn linked_limits(&self) -> Limits {
        self.orchestration
            .as_ref()
            .map(OrchestrationLimits::limits)
            .unwrap_or_default()
    }

    /// Subsessions for new sessions: on unless the person turned them off.
    pub fn subsessions_on(&self) -> bool {
        self.subsessions.unwrap_or(true)
    }

    /// The switch and the limits, as the engine applies them (`Workbench::set_subsessions`).
    pub fn subsessions(&self) -> Subsessions {
        Subsessions {
            on: self.subsessions_on(),
            first_start: self.subsessions_first_start.unwrap_or_default(),
            limits: self.linked_limits(),
            cross_agent_off: self.disable_cross_agent_subsessions,
        }
    }
}

impl OrchestrationLimits {
    /// The limits these set: none while limits are off, else each one set, as set.
    pub fn limits(&self) -> Limits {
        if !self.limits_on {
            return Limits::default();
        }
        Limits {
            max_depth: self.max_depth,
            max_live_children: self.max_live_children,
            max_live_linked: self.max_live_linked,
            max_child_worktrees: self.max_child_worktrees,
            max_starts: self.max_starts,
            max_auto_turns: self.max_auto_turns,
            max_idle_waits: self.max_idle_waits,
            crew_spend_usd: self.crew_spend_usd,
        }
        .sanitized()
    }
}

/// The app's identifier: the name of its config folder.
pub const APP_IDENTIFIER: &str = "xyz.nexus.desktop";
/// The settings' file name, in the config folder.
pub const FILE: &str = "settings.json";

/// The app's config folder: the OS's config folder, then [`APP_IDENTIFIER`]. None when the OS has no config folder.
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(APP_IDENTIFIER))
}

/// `settings.json` in `config_dir`.
pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

/// The saved settings. A Gateway saved on a retired host (`nexus_client::config::moved_gateway`) moves to
/// production's, and the file is saved with it (best effort).
pub fn load(path: &Path) -> Settings {
    let mut settings: Settings = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    if let Some(moved) = settings
        .gateway_url
        .as_deref()
        .and_then(nexus_client::config::moved_gateway)
    {
        settings.gateway_url = Some(moved.to_string());
        let _ = save(path, &settings);
    }
    settings
}

pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        nexus_client::config::create_private_dir(dir)?;
    }
    let text = serde_json::to_string_pretty(settings).unwrap_or_default();
    nexus_client::config::write_private(path, format!("{text}\n").as_bytes())
}

/// This computer's policy: the OS's managed preferences merged with the managed JSON file (`NEXUS_MANAGED_CONFIG`,
/// else the OS's path).
pub fn managed() -> (Policy, PathBuf) {
    let env = |name: &str| std::env::var(name).ok();
    let path = managed_config_path(&env);
    let file = load_managed_config(&path);
    let source = policy::system();
    (Policy::merge(source.as_ref(), file, &path.display().to_string()), path)
}

/// The Gateway to use: a locked managed URL, else the person's, else the managed one, else `NEXUS_URL`, else
/// production. Unsafe persisted, managed or environment addresses fall back to production; its own vault entry
/// supplies the credential, so a credential is never moved from the rejected address.
pub fn gateway_url(settings: &Settings, managed: Option<&ManagedConfig>) -> String {
    gateway_url_with_env(settings, managed, std::env::var("NEXUS_URL").ok())
}

fn gateway_url_with_env(settings: &Settings, managed: Option<&ManagedConfig>, environment: Option<String>) -> String {
    let managed_url = managed.and_then(|managed| managed.url.clone());
    if let Some(url) = managed_url.clone().filter(|_| managed.is_some_and(|m| m.lock_url)) {
        return gateway_address(&url).unwrap_or_else(|_| DEFAULT_GATEWAY.to_string());
    }
    let address = settings
        .gateway_url
        .clone()
        .or(managed_url)
        .or_else(|| environment.filter(|url| !url.is_empty()))
        .unwrap_or_else(|| DEFAULT_GATEWAY.to_string())
        .trim_end_matches('/')
        .to_string();
    gateway_address(&address).unwrap_or_else(|_| DEFAULT_GATEWAY.to_string())
}

/// Nexus UI's base URL for links: managed, else the person's, else `NEXUS_UI_URL` or `/app` on the Gateway (as the
/// CLI decides it).
pub fn ui_url(settings: &Settings, managed: Option<&ManagedConfig>, gateway: &str) -> String {
    if let Some(url) = managed
        .and_then(|managed| managed.ui_url.clone())
        .or(settings.ui_url.clone())
    {
        return url.trim_end_matches('/').to_string();
    }
    let env = |name: &str| std::env::var(name).ok();
    nexus_client::config::ui_url(gateway, &env)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A choice made before organizations kept picks moves to the first organization entered, once.
    #[test]
    fn a_choice_from_before_moves_to_the_first_organization() {
        let set_claude: fn(&mut Settings, Option<String>) = |settings, account| settings.claude_account = account;
        let agents = [("claude", set_claude)];
        let mut settings = Settings {
            claude_account: Some("a_7".into()),
            chosen_accounts: vec!["claude".into()],
            ..Settings::default()
        };
        settings.enter_org("acme", &agents);
        assert_eq!(settings.org_account("acme", "claude"), Some(Some("a_7".into())));
        assert_eq!(settings.chosen_accounts, ["claude"]);
        // A second organization starts with no pick.
        settings.enter_org("me", &agents);
        assert!(settings.chosen_accounts.is_empty());
        assert_eq!(settings.org_account("me", "claude"), None);
    }

    /// Each organization keeps its own agent accounts, and one that chose none takes over nothing.
    #[test]
    fn an_organization_switch_switches_the_agent_accounts_with_it() {
        let set_claude: fn(&mut Settings, Option<String>) = |settings, account| settings.claude_account = account;
        let set_codex: fn(&mut Settings, Option<String>) = |settings, account| settings.codex_account = account;
        let agents = [("claude", set_claude), ("codex", set_codex)];
        let mut settings = Settings::default();
        // At work: the work Claude account (an extra one) and Codex's own login.
        settings.remember_org_account("acme", "claude", Some("a_1"));
        settings.remember_org_account("acme", "codex", None);
        assert!(settings.enter_org("acme", &agents));
        assert_eq!(settings.claude_account.as_deref(), Some("a_1"));
        assert_eq!(settings.codex_account, None);
        assert_eq!(settings.chosen_accounts, ["claude", "codex"]);
        // The personal organization chose nothing yet: it is asked, and is not counted as having chosen work's.
        assert!(settings.enter_org("me", &agents));
        assert!(settings.chosen_accounts.is_empty());
        settings.remember_org_account("me", "claude", None);
        settings.enter_org("me", &agents);
        assert_eq!(settings.claude_account, None);
        assert_eq!(settings.chosen_accounts, ["claude"]);
        // Back at work, work's accounts again.
        settings.enter_org("acme", &agents);
        assert_eq!(settings.claude_account.as_deref(), Some("a_1"));
        assert!(!settings.enter_org("acme", &agents), "nothing changes twice");
        // Kept in the settings file.
        let text = serde_json::to_string(&settings).expect("settings write");
        let read: Settings = serde_json::from_str(&text).expect("settings read");
        assert_eq!(read.org_account("acme", "claude"), Some(Some("a_1".into())));
        assert_eq!(read.org_account("acme", "codex"), Some(None));
        assert_eq!(read.org_account("me", "codex"), None);
    }

    #[test]
    fn the_browser_opens_on_a_server_unless_turned_off() {
        let settings: Settings = serde_json::from_str("{}").expect("empty settings read");
        assert!(settings.opens_browser_on_server(), "on by default");
        let off: Settings = serde_json::from_str(r#"{"openBrowserOnServer":false}"#).expect("the setting reads");
        assert!(!off.opens_browser_on_server());
        assert_eq!(
            serde_json::to_value(&off).expect("the settings serialize"),
            serde_json::json!({ "openBrowserOnServer": false })
        );
    }

    #[test]
    fn merged_sessions_archive_only_when_turned_on() {
        let settings: Settings = serde_json::from_str("{}").expect("empty settings read");
        assert!(!settings.archives_merged(), "off by default");
        let on: Settings = serde_json::from_str(r#"{"archiveMerged":true}"#).expect("the setting reads");
        assert!(on.archives_merged());
    }

    /// The Sandbox switch is off by default, and a file written before it existed reads with it off.
    #[test]
    fn antigravity_sandbox_is_off_unless_turned_on() {
        let empty: Settings = serde_json::from_str("{}").expect("empty settings read");
        assert!(!empty.antigravity_sandboxed(), "off by default");
        let older: Settings = serde_json::from_str(r#"{"gatewayUrl":"https://api.nexus.inc","archiveMerged":true}"#)
            .expect("a file without the switch reads");
        assert!(!older.antigravity_sandboxed());
        assert_eq!(
            serde_json::to_value(&older).expect("the settings serialize"),
            serde_json::json!({ "gatewayUrl": "https://api.nexus.inc", "archiveMerged": true }),
            "nothing is written for a switch never touched"
        );
        let on: Settings = serde_json::from_str(r#"{"antigravitySandbox":true}"#).expect("the setting reads");
        assert!(on.antigravity_sandboxed());
        assert_eq!(
            serde_json::to_value(&on).expect("the settings serialize"),
            serde_json::json!({ "antigravitySandbox": true })
        );
    }

    #[test]
    fn switching_gateways_keeps_each_ones_organization() {
        const DEV: &str = "https://gateway.dev.test";
        let mut settings = Settings {
            gateway_url: Some(DEFAULT_GATEWAY.into()),
            org: Some("acme".into()),
            kind: Some("cli".into()),
            ..Settings::default()
        };
        assert_eq!(
            settings.known_gateways(DEFAULT_GATEWAY),
            vec![(DEFAULT_GATEWAY.to_string(), Some("acme".into()))]
        );
        // To a new Gateway: production is kept, and the new one has no organization yet.
        settings.switch_gateway(DEFAULT_GATEWAY, true, DEV);
        assert_eq!(settings.gateway_url.as_deref(), Some(DEV));
        assert_eq!((settings.org.clone(), settings.kind.clone()), (None, None));
        settings.org = Some("nexus-dev".into());
        settings.kind = Some("api".into());
        assert_eq!(
            settings.known_gateways(DEV),
            vec![
                (DEV.to_string(), Some("nexus-dev".into())),
                (DEFAULT_GATEWAY.to_string(), Some("acme".into())),
            ]
        );
        // And back: production's organization and kind return, and the dev Gateway is kept.
        settings.switch_gateway(DEV, true, DEFAULT_GATEWAY);
        assert_eq!(
            (settings.org.as_deref(), settings.kind.as_deref()),
            (Some("acme"), Some("cli"))
        );
        assert_eq!(
            settings.gateways,
            vec![KnownGateway {
                url: DEV.into(),
                org: Some("nexus-dev".into()),
                kind: Some("api".into()),
            }]
        );
        // A Gateway left signed out (a typo, say) is not kept.
        settings.switch_gateway(DEFAULT_GATEWAY, true, "https://typo.test");
        settings.switch_gateway("https://typo.test", false, DEV);
        assert_eq!(
            settings
                .known_gateways(DEV)
                .into_iter()
                .map(|(url, _)| url)
                .collect::<Vec<_>>(),
            vec![DEV.to_string(), DEFAULT_GATEWAY.to_string()]
        );
        // The same Gateway again changes nothing.
        let before = settings.clone();
        settings.switch_gateway(DEV, true, DEV);
        assert_eq!(settings, before);
    }

    #[test]
    fn a_locked_managed_gateway_wins() {
        let person = Settings {
            gateway_url: Some("http://127.0.0.1:4601/".into()),
            ..Settings::default()
        };
        let mut managed = ManagedConfig {
            url: Some("https://nexus.acme.test".into()),
            ..ManagedConfig::default()
        };
        assert_eq!(gateway_url(&person, Some(&managed)), "http://127.0.0.1:4601");
        assert_eq!(
            gateway_url(&Settings::default(), Some(&managed)),
            "https://nexus.acme.test"
        );
        managed.lock_url = true;
        assert_eq!(gateway_url(&person, Some(&managed)), "https://nexus.acme.test");
    }

    #[test]
    fn startup_gateway_addresses_are_validated_before_vault_selection() {
        let unsafe_url = "http://remote-gateway.example";
        let vault = crate::secrets::Vault::Memory(Default::default());
        vault.set(unsafe_url, "unsafe-origin-token").expect("memory vault");
        vault.set(DEFAULT_GATEWAY, "production-token").expect("memory vault");
        let saved = Settings {
            gateway_url: Some(unsafe_url.into()),
            ..Settings::default()
        };
        let managed = |lock_url| ManagedConfig {
            url: Some(unsafe_url.into()),
            lock_url,
            ..ManagedConfig::default()
        };
        let defaults = Settings::default();
        let managed_unlocked = managed(false);
        let managed_locked = managed(true);
        for resolved in [
            gateway_url_with_env(&saved, None, None),
            gateway_url_with_env(&defaults, Some(&managed_unlocked), None),
            gateway_url_with_env(&defaults, Some(&managed_locked), None),
            gateway_url_with_env(&defaults, None, Some(unsafe_url.into())),
        ] {
            assert_eq!(resolved, DEFAULT_GATEWAY);
            assert_eq!(vault.get(&resolved).as_deref(), Some("production-token"));
        }
        assert_eq!(vault.get(unsafe_url).as_deref(), Some("unsafe-origin-token"));
        assert_eq!(
            gateway_url_with_env(&defaults, None, Some("http://127.0.0.1:3100/".into())),
            "http://127.0.0.1:3100"
        );
    }

    /// Limits are off unless turned on; a file written before the switch is on only when it sets a limit. While on,
    /// each limit left out or null is no limit, and a set one is kept as set, with no ceiling. Every form survives a save and a load.
    #[test]
    fn limits_are_off_unless_turned_on_and_each_is_optional_with_no_ceiling() {
        let none = Limits::default();
        for (text, expected) in [
            ("{}", none),
            (r#"{"orchestration": {}}"#, none),
            (r#"{"orchestration": {"maxStarts": null}}"#, none),
            (
                r#"{"orchestration": {"maxDepth": 2, "maxStarts": 4}}"#,
                Limits {
                    max_depth: Some(2),
                    max_starts: Some(4),
                    ..none
                },
            ),
            (
                r#"{"orchestration": {"limitsOn": false, "maxStarts": 4, "crewSpendUsd": 20}}"#,
                none,
            ),
            (r#"{"orchestration": {"limitsOn": true}}"#, none),
            (
                r#"{"orchestration": {"limitsOn": true, "maxStarts": null, "maxAutoTurns": 12}}"#,
                Limits {
                    max_auto_turns: Some(12),
                    ..none
                },
            ),
            (
                r#"{"orchestration": {"limitsOn": true, "maxDepth": 9, "maxLiveChildren": 500, "maxStarts": 4000,
                    "crewSpendUsd": 5000}}"#,
                Limits {
                    max_depth: Some(9),
                    max_live_children: Some(500),
                    max_starts: Some(4000),
                    crew_spend_usd: Some(5000.0),
                    ..none
                },
            ),
        ] {
            let settings: Settings = serde_json::from_str(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(settings.linked_limits(), expected, "{text}");
            assert_eq!(settings.subsessions().limits, expected, "{text}");
            let saved = serde_json::to_string(&settings).expect("save");
            let again: Settings = serde_json::from_str(&saved).expect("load");
            assert_eq!(again, settings, "{text} round-trips");
            assert_eq!(again.linked_limits(), expected, "{text} after a save");
        }
        let off_with_values: Settings =
            serde_json::from_str(r#"{"orchestration": {"limitsOn": false, "maxStarts": 4}}"#).expect("json");
        assert_eq!(
            serde_json::to_value(&off_with_values).expect("json"),
            serde_json::json!({ "orchestration": { "limitsOn": false, "maxStarts": 4 } }),
            "limits turned off are written off, so they do not read as an old file's caps"
        );
    }

    /// "Disable cross-agent subsessions" is off in a file written before it, and reaches the engine when on.
    #[test]
    fn cross_agent_subsessions_are_allowed_unless_disabled() {
        let older: Settings = serde_json::from_str(r#"{"subsessions":true}"#).expect("a file without the switch reads");
        assert!(!older.subsessions().cross_agent_off);
        assert_eq!(
            serde_json::to_value(&older).expect("the settings serialize"),
            serde_json::json!({ "subsessions": true }),
            "nothing is written while the switch is off"
        );
        let disabled: Settings =
            serde_json::from_str(r#"{"disableCrossAgentSubsessions":true}"#).expect("the setting reads");
        assert!(disabled.subsessions().cross_agent_off);
        assert!(disabled.subsessions().on, "the switch does not turn subsessions off");
        assert_eq!(
            serde_json::to_value(&disabled).expect("the settings serialize"),
            serde_json::json!({ "disableCrossAgentSubsessions": true })
        );
    }

    /// Subsessions are on in a file written before the switch, and only a choice the person made is written.
    #[test]
    fn subsessions_are_on_unless_turned_off() {
        let older: Settings = serde_json::from_str(r#"{"org":"acme","orchestration":{"maxStarts":4}}"#)
            .expect("a file without the switch reads");
        assert!(older.subsessions_on());
        assert_eq!(
            older.subsessions(),
            Subsessions {
                on: true,
                first_start: Preference::Ask,
                limits: older.linked_limits(),
                cross_agent_off: false,
            }
        );
        assert_eq!(
            serde_json::to_value(&older).expect("the settings serialize"),
            serde_json::json!({ "org": "acme", "orchestration": { "limitsOn": true, "maxStarts": 4 } }),
            "nothing is written for the subsessions switch never touched; the old cap is written on"
        );
        let off: Settings = serde_json::from_str(r#"{"subsessions":false}"#).expect("the setting reads");
        assert!(!off.subsessions().on);
        assert_eq!(
            serde_json::to_value(&off).expect("the settings serialize"),
            serde_json::json!({ "subsessions": false })
        );
    }

    /// A file written before `limitsOn`, with a spend cap and other numbers in it, keeps enforcing them after the
    /// upgrade, and still does after a save. Turned off, the values stay in the file, dormant, and stay off.
    #[test]
    fn an_old_file_with_a_cap_and_no_limits_on_still_enforces_it() {
        let old = r#"{"org": "acme", "orchestration": {"maxDepth": 1, "maxStarts": 8, "crewSpendUsd": 20}}"#;
        let caps = Limits {
            max_depth: Some(1),
            max_starts: Some(8),
            crew_spend_usd: Some(20.0),
            ..Limits::default()
        };
        let settings: Settings = serde_json::from_str(old).expect("an old file reads");
        assert_eq!(settings.linked_limits(), caps);
        assert_eq!(settings.subsessions().limits, caps);
        let saved: Settings = serde_json::from_str(&serde_json::to_string(&settings).expect("save")).expect("load");
        assert_eq!(saved, settings, "the caps survive a save");
        let mut off = saved;
        off.orchestration.as_mut().expect("the block").limits_on = false;
        let off: Settings = serde_json::from_str(&serde_json::to_string(&off).expect("save")).expect("load");
        assert_eq!(off.linked_limits(), Limits::default(), "turned off stays off");
        assert_eq!(off.orchestration.expect("the block").crew_spend_usd, Some(20.0));
    }

    #[test]
    fn crew_spend_cap_round_trips_off_zero_and_positive_amounts() {
        for (fields, expected) in [
            (r#"{"limitsOn":true}"#, None),
            (r#"{"limitsOn":true,"crewSpendUsd":null}"#, None),
            (r#"{"limitsOn":true,"crewSpendUsd":0}"#, Some(0.0)),
            (r#"{"limitsOn":true,"crewSpendUsd":20.4}"#, Some(20.4)),
            (r#"{"crewSpendUsd":20.4}"#, Some(20.4)),
            (r#"{"crewSpendUsd":null}"#, None),
            (r#"{"limitsOn":false,"crewSpendUsd":20.4}"#, None),
        ] {
            let json = format!(r#"{{"orchestration":{fields}}}"#);
            let settings: Settings = serde_json::from_str(&json).expect("read crew spend settings");
            let saved = serde_json::to_string(&settings).expect("save crew spend settings");
            assert_eq!(
                serde_json::from_str::<Settings>(&saved)
                    .expect("reload crew spend settings")
                    .linked_limits()
                    .crew_spend_usd,
                expected
            );
        }
    }

    #[test]
    fn orchestration_limits_that_do_not_read_are_no_limit_and_keep_the_rest_of_the_file() {
        for block in [
            r#"{"limitsOn": true, "maxDepth": -1}"#,
            r#"{"limitsOn": "yes", "maxStarts": 4}"#,
            r#"{"maxDepth": "deep"}"#,
            r#"{"crewSpendUsd": [1]}"#,
            r#""many""#,
            "7",
            "null",
        ] {
            let text = format!(r#"{{"org": "acme", "orchestration": {block}}}"#);
            let settings: Settings = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{block}: {e}"));
            assert_eq!(settings.org.as_deref(), Some("acme"), "{block}");
            assert_eq!(settings.orchestration, None, "{block}");
        }
        let dir = std::env::temp_dir().join(format!("nexus-desktop-settings-lenient-{}", std::process::id()));
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(&path, r#"{"org": "acme", "orchestration": {"maxStarts": "lots"}}"#).expect("write");
        assert_eq!(load(&path).org.as_deref(), Some("acme"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reads_the_chat_width_and_a_block_that_does_not_read_is_the_default() {
        use crate::chat_width::Preset;
        let settings: Settings =
            serde_json::from_str(r#"{"chatWidth": {"preset": "custom", "share": 0.8, "maxCh": 140}}"#).expect("json");
        assert_eq!(settings.chat_width, Some(ChatWidth::custom(0.8, 140.)));
        for block in [r#"{"preset": "huge"}"#, r#""wide""#, "7", "null"] {
            let text = format!(r#"{{"org": "acme", "chatWidth": {block}}}"#);
            let settings: Settings = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{block}: {e}"));
            assert_eq!(settings.org.as_deref(), Some("acme"), "{block}");
            assert_eq!(settings.chat_width, None, "{block}");
        }
        let written = serde_json::to_value(Settings {
            chat_width: Some(ChatWidth::preset(Preset::Wide)),
            ..Settings::default()
        })
        .expect("json");
        assert_eq!(written, serde_json::json!({"chatWidth": {"preset": "wide"}}));
    }

    #[test]
    fn a_gateway_saved_on_a_retired_host_moves_to_production() {
        let dir = std::env::temp_dir().join(format!("nexus-desktop-moved-{}", std::process::id()));
        let path = dir.join("settings.json");
        for (saved, now) in [
            ("https://api.nexus.xyz", DEFAULT_GATEWAY),
            ("https://nexus.acme.test", "https://nexus.acme.test"),
        ] {
            let settings = Settings {
                gateway_url: Some(saved.into()),
                org: Some("acme".into()),
                ..Settings::default()
            };
            save(&path, &settings).expect("save");
            assert_eq!(load(&path).gateway_url.as_deref(), Some(now), "{saved}");
            let on_disk: Settings = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            assert_eq!(on_disk.gateway_url.as_deref(), Some(now), "{saved} saved");
            assert_eq!(on_disk.org.as_deref(), Some("acme"));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edits_are_checked_and_a_refused_one_changes_nothing() {
        let mut settings = Settings::default();
        assert!(
            settings
                .set_worktree_root(WorktreeRoot::Worktrees, Some("relative/dir".into()))
                .is_err()
        );
        settings
            .set_worktree_root(WorktreeRoot::Reviews, Some("/r".into()))
            .expect("a full path");
        assert_eq!(settings.reviews_dir.as_deref(), Some("/r"));
        settings
            .set_worktree_root(WorktreeRoot::Reviews, Some("  ".into()))
            .expect("blank resets");
        assert_eq!(settings.reviews_dir, None);

        let rules = |idle_days| nexus_workbench::cleanup::Rules {
            merged: false,
            archived: false,
            idle_days,
        };
        assert!(settings.set_cleanup_rules(rules(Some(400))).is_err());
        settings.set_cleanup_rules(rules(Some(14))).expect("14 days");
        assert_eq!(settings.worktree_cleanup, Some(rules(Some(14))));
        settings.set_cleanup_rules(rules(Some(0))).expect("0 is off");
        assert_eq!(settings.worktree_cleanup, None);

        assert!(settings.set_branch_prefix(Some("has space/".into())).is_err());
        assert_eq!(settings.set_branch_prefix(Some(" me/ ".into())), Ok("me/".into()));
        assert_eq!(settings.branch_prefix.as_deref(), Some("me/"));
        assert_eq!(settings.set_branch_prefix(Some("nexus/".into())), Ok("nexus/".into()));
        assert_eq!(settings.branch_prefix, None, "the default is not kept");
        assert_eq!(settings.set_branch_prefix(Some(String::new())), Ok(String::new()));
        assert_eq!(settings.branch_prefix_or_default(), "");

        assert!(settings.add_ssh_machine("Box", "-oProxyCommand=x").is_err());
        let added = settings.add_ssh_machine("  ", " dev@box ").expect("a host");
        assert_eq!(
            (added.name.as_str(), added.ssh.as_str(), added.kind.as_str()),
            ("dev@box", "dev@box", "ssh")
        );
        let second = settings.add_ssh_machine("Build", "build").expect("a host");
        assert_ne!(added.id, second.id);
        assert_eq!(settings.machines.len(), 2);

        assert_eq!(gateway_address(" https://gw.test/ "), Ok("https://gw.test".into()));
        assert!(gateway_address("gw.test").is_err());
    }

    #[test]
    fn the_chat_bar_is_on_unless_turned_off() {
        let settings: Settings = serde_json::from_str("{}").expect("empty settings read");
        assert!(settings.opens_chat_bar(), "on by default");
        let off: Settings = serde_json::from_str(r#"{"chatBar":false}"#).expect("the setting reads");
        assert!(!off.opens_chat_bar());
    }

    #[test]
    fn follows_edits_unless_turned_off() {
        let settings: Settings = serde_json::from_str("{}").expect("empty settings read");
        assert!(settings.follows_edits(), "on by default");
        let off: Settings = serde_json::from_str(r#"{"followEdits":false}"#).expect("the setting reads");
        assert!(!off.follows_edits());
        assert_eq!(
            serde_json::to_value(&off).expect("the settings serialize"),
            serde_json::json!({ "followEdits": false })
        );
    }

    #[test]
    fn gateway_addresses_keep_sign_ins_on_secure_unambiguous_destinations() {
        for address in [
            "http://gateway.example",
            "http://localhost.example",
            "http://127.0.0.1.example",
            "http://192.168.0.1",
            "https://gateway.example@attacker.example",
            "https://user:password@gateway.example",
            "https://gateway.example?next=attacker.example",
            "https://gateway.example/#secret",
            "https://",
            "file:///tmp/gateway",
        ] {
            assert!(gateway_address(address).is_err(), "{address}");
        }
        for (address, expected) in [
            ("https://GATEWAY.example/", "https://gateway.example"),
            ("https://gateway.example/nexus/", "https://gateway.example/nexus"),
            ("http://localhost:3100/", "http://localhost:3100"),
            ("http://127.0.0.1:3100/", "http://127.0.0.1:3100"),
            ("http://[::1]:3100/", "http://[::1]:3100"),
        ] {
            assert_eq!(gateway_address(address).as_deref(), Ok(expected), "{address}");
        }
    }

    #[test]
    fn round_trips() {
        let dir = std::env::temp_dir().join(format!("nexus-desktop-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let settings = Settings {
            org: Some("acme".into()),
            shortcut: Some("Alt+Space".into()),
            menu_bar_only: Some(true),
            panel_size: Some([520.0, 680.0]),
            ..Settings::default()
        };
        save(&path, &settings).expect("save");
        assert_eq!(load(&path), settings);
        assert_eq!(load(&dir.join("missing.json")), Settings::default());
        std::fs::remove_dir_all(&dir).ok();
    }
}
