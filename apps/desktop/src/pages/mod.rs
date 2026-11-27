//! The pages drawn natively for parity with Nexus UI (`ui/src/app/app/[org]/...`): the organization's settings, the
//! person's other settings, and the Gateway places (Overview, Usage). Each page is its own GPUI view in its own
//! file. It reads Gateway through `ctx.gateway.query(..)` and [`read`], or `request(..)` and [`fetch`] for writes, so a new page touches no shared file.
//!
//! A page's `build` returns `None` while it is not drawn here; the app then opens the page in the browser.

use std::rc::Rc;

use gpui_kit::*;
use serde_json::Value;

pub use crate::gateway::Verb;
use crate::palette::Palette;

pub mod ui;

pub(crate) mod agent;
/// The Assistant's views beside its chat: Errands, Discover, What Nexus knows and Approvals.
pub(crate) mod assistant_views;
/// A Code session's pull request page: not a page of Nexus UI, so it has no [`PageId`].
pub mod pull_request;

mod agents;
pub(crate) use agents::{CLAUDE_ROUTING, CODEX_ROUTING};
mod api_explorer;
mod audit;
mod billing;
mod cloud;
mod connectors;
mod design_systems;
/// Settings › App, and the panel's Settings (`workspace/panel_view.rs`).
pub mod desktop_app;
mod devices;
mod download;
mod gateways;
mod general;
mod hooks;
pub(crate) mod local_providers;
mod local_sessions;
mod mcp;
mod members;
pub mod members_presence;
mod outside_access;
mod overview;
mod policy;
mod privacy;
mod project_editor;
pub(crate) mod project_host;
/// Settings › Providers, whose rows a setup line opens (`anchor`, `key_added`).
pub(crate) mod providers;
mod referrals;
mod repositories;
mod row;
mod schedules;
mod servers;
mod shared_memory;
pub(crate) mod skills;
pub(crate) mod subscriptions;
mod subsessions;
mod teams;
pub(crate) mod usage;
mod usage_caps;
mod usage_export;
mod worktrees;

pub(crate) use agents::connect_on_open;
pub(crate) use cloud::GITHUB_ANCHOR as CLOUD_GITHUB_ANCHOR;
pub(crate) use connectors::{authorize_url, oauth_request, row_anchor, usable_accounts};
#[cfg_attr(test, allow(unused_imports))] // The workspace calls it outside tests.
pub(crate) use devices::register_this_computer;

/// A page of Nexus UI that the app can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageId {
    // Places, in the sidebar.
    Overview,
    Usage,
    /// The Assistant: only with an agent API (`agent.rs`).
    Agent,
    /// The Assistant's other views (`assistant_views.rs`): Errands, Discover, What Nexus knows, Approvals,
    /// Passwords and payments and Privacy. Only with an agent API, as the Assistant.
    AgentErrands,
    AgentDiscover,
    AgentMemory,
    AgentApprovals,
    AgentVault,
    AgentPrivacy,
    /// Watches: what Nexus watches for the person.
    AgentWatches,
    // You: account and access.
    Referrals,
    Subscriptions,
    Agents,
    Devices,
    ApiExplorer,
    // You: customize.
    /// The person's own connector accounts: Settings › Connections.
    Connectors,
    Mcp,
    Skills,
    // You: this computer.
    Gateways,
    Worktrees,
    Subsessions,
    Hooks,
    Servers,
    Cloud,
    DesktopApp,
    LocalSessions,
    SharedMemory,
    Privacy,
    // The organization.
    /// Who in the organization can use each connector, and its own accounts: Settings › Integrations.
    ConnectorRules,
    Members,
    Teams,
    Policy,
    Providers,
    Audit,
    General,
    DesignSystems,
    Repositories,
    Billing,
}

impl PageId {
    pub const ALL: [PageId; 38] = [
        PageId::Overview,
        PageId::Usage,
        PageId::Agent,
        PageId::AgentErrands,
        PageId::AgentDiscover,
        PageId::AgentMemory,
        PageId::AgentApprovals,
        PageId::AgentVault,
        PageId::AgentPrivacy,
        PageId::AgentWatches,
        PageId::Referrals,
        PageId::Subscriptions,
        PageId::Agents,
        PageId::Devices,
        PageId::ApiExplorer,
        PageId::Connectors,
        PageId::Mcp,
        PageId::Skills,
        PageId::Gateways,
        PageId::Worktrees,
        PageId::Subsessions,
        PageId::Hooks,
        PageId::Servers,
        PageId::Cloud,
        PageId::DesktopApp,
        PageId::LocalSessions,
        PageId::SharedMemory,
        PageId::Privacy,
        PageId::ConnectorRules,
        PageId::Members,
        PageId::Teams,
        PageId::Policy,
        PageId::Providers,
        PageId::Audit,
        PageId::General,
        PageId::DesignSystems,
        PageId::Repositories,
        PageId::Billing,
    ];

    /// The page Nexus UI draws at `path` (`settings/members`), for `NEXUS_OPEN_PAGE`.
    pub fn from_web_path(path: &str) -> Option<PageId> {
        let path = path.trim_matches('/');
        PageId::ALL.into_iter().find(|id| id.web_path() == path)
    }

    /// A place has the main area to itself; any other page is a section of Settings.
    pub fn is_place(self) -> bool {
        matches!(self, PageId::Overview | PageId::Usage | PageId::Agent) || self.is_assistant_view()
    }

    /// One of the Assistant's views beside its chat.
    pub fn is_assistant_view(self) -> bool {
        matches!(
            self,
            PageId::AgentErrands
                | PageId::AgentDiscover
                | PageId::AgentMemory
                | PageId::AgentApprovals
                | PageId::AgentVault
                | PageId::AgentPrivacy
                | PageId::AgentWatches
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            PageId::Overview => "Overview",
            PageId::Usage => "Usage",
            PageId::Agent => "Assistant",
            PageId::AgentErrands => "Errands",
            PageId::AgentDiscover => "Discover",
            // Nexus UI's name for the view; "What Nexus knows" before the design named it.
            PageId::AgentMemory => "Memory",
            PageId::AgentApprovals => "Approvals",
            PageId::AgentVault => "Passwords and payments",
            PageId::AgentPrivacy => "Privacy",
            PageId::AgentWatches => "Watches",
            PageId::Referrals => "Referrals",
            PageId::Subscriptions => "Subscriptions",
            PageId::Agents => "Coding agents",
            PageId::Devices => "Devices",
            PageId::ApiExplorer => "API Explorer",
            PageId::Connectors => "Connections",
            PageId::ConnectorRules => "Integrations",
            PageId::Mcp => "MCP servers",
            PageId::Skills => "Skills",
            PageId::Gateways => "Gateways",
            PageId::Worktrees => "Projects & worktrees",
            PageId::Subsessions => "Subsessions",
            PageId::Hooks => "Hooks",
            PageId::Servers => "Remote servers",
            PageId::Cloud => "Cloud",
            PageId::DesktopApp => "Desktop app",
            PageId::LocalSessions => "Local sessions",
            PageId::SharedMemory => "Shared memory",
            PageId::Privacy => "Privacy",
            PageId::Members => "Members",
            PageId::Teams => "Teams",
            PageId::Policy => "Model policy",
            PageId::Providers => "Providers",
            PageId::Audit => "Audit log",
            PageId::General => "General",
            PageId::DesignSystems => "Design systems",
            PageId::Repositories => "Repositories",
            PageId::Billing => "Billing",
        }
    }

    /// Whether the app draws it; else it opens in the browser.
    pub fn is_native(self) -> bool {
        match self {
            PageId::Overview => overview::NATIVE,
            PageId::Usage => usage::NATIVE,
            PageId::Agent => agent::NATIVE,
            PageId::AgentErrands
            | PageId::AgentDiscover
            | PageId::AgentMemory
            | PageId::AgentApprovals
            | PageId::AgentVault
            | PageId::AgentPrivacy
            | PageId::AgentWatches => true,
            PageId::Referrals => referrals::NATIVE,
            PageId::Subscriptions => subscriptions::NATIVE,
            PageId::Agents => agents::NATIVE,
            PageId::Devices => devices::NATIVE,
            PageId::ApiExplorer => api_explorer::NATIVE,
            PageId::Connectors | PageId::ConnectorRules => connectors::NATIVE,
            PageId::Mcp => mcp::NATIVE,
            PageId::Skills => skills::NATIVE,
            PageId::Gateways => gateways::NATIVE,
            PageId::Worktrees => worktrees::NATIVE,
            PageId::Subsessions => true,
            PageId::Hooks => hooks::NATIVE,
            PageId::Servers => servers::NATIVE,
            PageId::Cloud => cloud::NATIVE,
            PageId::DesktopApp => desktop_app::NATIVE,
            PageId::LocalSessions => local_sessions::NATIVE,
            PageId::SharedMemory => true,
            PageId::Privacy => privacy::NATIVE,
            PageId::Members => members::NATIVE,
            PageId::Teams => teams::NATIVE,
            PageId::Policy => policy::NATIVE,
            PageId::Providers => providers::NATIVE,
            PageId::Audit => audit::NATIVE,
            PageId::General => general::NATIVE,
            PageId::DesignSystems => design_systems::NATIVE,
            PageId::Repositories => repositories::NATIVE,
            PageId::Billing => billing::NATIVE,
        }
    }

    /// Where Nexus UI draws it, under `/app/<org>/`.
    pub fn web_path(self) -> &'static str {
        match self {
            PageId::Overview => "overview",
            PageId::Usage => "usage",
            PageId::Agent => "agent",
            // Nexus UI's views of the Assistant (`?view=`).
            PageId::AgentErrands => "agent?view=errands",
            PageId::AgentDiscover => "agent?view=discover",
            PageId::AgentMemory => "agent?view=memory",
            PageId::AgentApprovals => "agent?view=approvals",
            PageId::AgentVault => "agent?view=vault",
            PageId::AgentPrivacy => "agent?view=privacy",
            PageId::AgentWatches => "agent?view=watches",
            PageId::Referrals => "referrals",
            PageId::Subscriptions => "settings/plan-usage",
            PageId::Agents => "settings/agents",
            PageId::Devices => "settings/devices",
            PageId::ApiExplorer => "settings/api-explorer",
            PageId::Connectors => "settings/connectors",
            // Desktop only: Nexus UI's connectors page holds both sides; this build draws the organization's apart.
            PageId::ConnectorRules => "settings/connector-rules",
            PageId::Mcp => "settings/mcp",
            PageId::Skills => "skills",
            PageId::Gateways => "settings/gateways",
            PageId::Worktrees => "settings/worktrees",
            PageId::Subsessions => "settings/subsessions",
            // Desktop only: the Nexus app's Computer page holds it (`#hooks`); this build draws it.
            PageId::Hooks => "settings/hooks",
            PageId::Servers => "settings/servers",
            PageId::Cloud => "settings/cloud",
            PageId::DesktopApp => "settings/this-computer",
            PageId::LocalSessions => "settings/local-sessions",
            PageId::SharedMemory => "settings/shared-memory",
            PageId::Privacy => "settings/privacy",
            PageId::Members => "settings/members",
            PageId::Teams => "settings/teams",
            PageId::Policy => "settings/model-policy",
            PageId::Providers => "settings/providers",
            PageId::Audit => "settings/audit",
            PageId::General => "settings/general",
            PageId::DesignSystems => "settings/design-systems",
            PageId::Repositories => "settings/repositories",
            PageId::Billing => "settings/billing",
        }
    }
}

/// The Settings addresses of a person's connectors page and the organization's, for the registry's tests.
#[cfg(test)]
pub fn connectors_addresses() -> (&'static str, &'static str) {
    (connectors::MINE_ADDRESS, connectors::ORG_ADDRESS)
}

/// What a page needs from the app.
/// Local page services. Gateway information is display-only here.
#[derive(Clone)]
pub struct DeviceCtx {
    /// The window elected to work without an account. Missing account data alone does not imply this state.
    pub(crate) local_workspace: bool,
    pub settings_fields: crate::settings_fields::SettingsFields,
    pub gateway_url: String,
    /// Open another page of the app.
    pub navigate: Rc<dyn Fn(PageId, &mut Window, &mut App)>,
    /// Open an existing settings address natively, or in the browser when it has no native view.
    pub navigate_settings: Rc<dyn Fn(String, &mut Window, &mut App)>,
    /// Settings' Gateway switch: the app moves to the Gateway at this address with no restart, or says why not.
    pub switch_gateway: Rc<dyn Fn(String, &mut Window, &mut App) -> Result<(), String>>,
    /// The Nexus app's settings were saved here: the running Code sessions use the new folders and branch prefix.
    pub settings_saved: Rc<dyn Fn(&nexus_desktop_core::settings::Settings, &mut App)>,
    /// Settings › App's Restart to update (`Workspace::restart_for_update`): an error says why it did not start.
    pub restart_for_update: Rc<dyn Fn(&mut App) -> Result<(), String>>,
    /// Settings › App's Restart now for Keep agents running (`Workspace::restart_for_service`).
    pub restart_for_service: Rc<dyn Fn(&mut App) -> Result<(), String>>,
    /// The Code sessions as they stand now: asked again at each use, because this build may take the store over.
    pub code: Rc<dyn Fn(&App) -> CodeAccess>,
    /// The Overview first step opens Code with the current sign-in.
    pub new_code_session: Rc<dyn Fn(&mut Window, &mut App)>,
}

/// A page that uses the signed-in account.
#[derive(Clone)]
pub struct Ctx {
    pub device: DeviceCtx,
    pub(crate) gateway: crate::workspace::gateway_access::AccountGateway,
    /// The organization's segment in Nexus UI's URLs: its slug, else its id.
    pub org: String,
    pub org_name: String,
    pub email: String,
    /// Settings › Cloud's "Start a session": New session with Cloud picked.
    pub new_cloud_session: Rc<dyn Fn(&mut Window, &mut App)>,
    /// The sidebar's presence poll as it stands now, for Settings › Members.
    pub team: Rc<dyn Fn(&App) -> members_presence::Team>,
    /// Open a teammate's shared sessions, by user id.
    pub watch: Rc<dyn Fn(String, &mut Window, &mut App)>,
    /// Runs a setup line's fix in this window (`crate::setup`).
    pub fixer: crate::setup::Fixer,
}

impl std::ops::Deref for Ctx {
    type Target = DeviceCtx;

    fn deref(&self) -> &DeviceCtx {
        &self.device
    }
}

/// What a page may do with the Code sessions store (`owner.rs`).
#[derive(Clone, Default)]
pub struct CodeAccess {
    /// The store's scope (`<gateway>|<org>`); None before the organization is known.
    pub scope: Option<String>,
    /// The schedules' commands, only while this build owns the store.
    pub schedules: Option<crate::code_run::Schedules>,
    /// The projects' commands (setup, commands, pin, session-start choice, Clean up now), only while this build owns the
    /// store.
    pub projects: Option<crate::code_run::Projects>,
    /// Why the store is read-only here, when it is.
    pub note: Option<String>,
    /// The organization lets sessions run on a person's own plan.
    pub plans: bool,
    /// Outside access, only while this build owns the store.
    pub outside: Option<std::sync::Arc<crate::outside::OutsideAccess>>,
    /// The background service's connection, while it runs the sessions (`service.rs`).
    pub service: Option<nexus_remote::machine::Status>,
    /// Settings › Devices made or replaced the device key: session sync takes it up
    /// (`CodeRunner::device_key_changed`). None without a runner.
    pub device_key_changed: Option<Rc<dyn Fn()>>,
}

impl Ctx {
    /// Open a Nexus UI page of the organization (`/app/<org>/<path>`) in the browser: for what is not drawn here.
    pub fn open_web(&self, path: &str, cx: &mut App) {
        crate::external_link::open(&org_url(&self.gateway.site_url(), &self.org, path), cx);
    }
}

/// Nexus UI's page `path` of the organization `org`: `<site>/app/<org>/<path>`. `ui` may be the site or Nexus UI's URL,
/// with or without a trailing `/`; `/app` is never doubled.
pub fn org_url(ui: &str, org: &str, path: &str) -> String {
    format!(
        "{}/{}",
        nexus_desktop_core::web_url::org_base(ui, org),
        path.trim_start_matches('/')
    )
}

type Build<C> = fn(PageId, C, &mut Window, &mut App) -> Option<AnyView>;
type BuildLocked = fn(PageId, &mut Window, &mut App) -> Option<AnyView>;
type BuildMixed = fn(PageId, DeviceCtx, Option<Ctx>, &mut Window, &mut App) -> Option<AnyView>;

/// The constructor's input determines whether the page is available without an account.
pub(crate) enum Builder {
    Device(Build<DeviceCtx>),
    Account(Build<Ctx>, Option<BuildLocked>),
    Mixed(BuildMixed),
}

impl Builder {
    fn account(live: Build<Ctx>) -> Self {
        Self::Account(live, None)
    }

    /// Both presentations belong to the same account page. The locked view has no account or device actions.
    #[allow(dead_code, reason = "Settings page migrations supply the locked builders")]
    pub(crate) fn account_with_locked(live: Build<Ctx>, locked: BuildLocked) -> Self {
        Self::Account(live, Some(locked))
    }

    pub(crate) fn build_locked(self, id: PageId, window: &mut Window, cx: &mut App) -> Option<AnyView> {
        match self {
            Self::Account(_, Some(locked)) => locked(id, window, cx),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn has_locked_view(&self) -> bool {
        matches!(self, Self::Account(_, Some(_)))
    }

    pub(crate) fn needs_account(&self) -> bool {
        matches!(self, Self::Account(..))
    }

    pub(crate) fn build(
        self,
        id: PageId,
        device: DeviceCtx,
        account: Option<Ctx>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyView> {
        match self {
            Self::Device(build) => build(id, device, window, cx),
            Self::Account(build, _) => build(id, account?, window, cx),
            Self::Mixed(build) => build(id, device, account, window, cx),
        }
    }
}

impl PageId {
    pub(crate) fn builder(self) -> Builder {
        use Builder::{Device, Mixed};
        match self {
            Self::Overview => Builder::account(|_, ctx, window, cx| overview::build(ctx, window, cx)),
            Self::Usage => Builder::account_with_locked(
                |_, ctx, window, cx| usage::build(ctx, window, cx),
                |_, _, cx| Some(usage::build_locked(cx)),
            ),
            // The Workspace keeps its Assistant and opens its subpages with the appropriate moves.
            Self::Agent => Builder::account(|_, ctx, window, cx| {
                let scope = ctx.org_name.clone().into();
                Some(
                    agent::build(
                        ctx,
                        None,
                        scope,
                        agent::Person::default(),
                        agent::Reactions::default(),
                        window,
                        cx,
                    )
                    .into(),
                )
            }),
            Self::AgentErrands
            | Self::AgentDiscover
            | Self::AgentMemory
            | Self::AgentApprovals
            | Self::AgentVault
            | Self::AgentPrivacy
            | Self::AgentWatches => Builder::account(|id, ctx, window, cx| {
                Some(assistant_views::build(
                    id,
                    ctx,
                    assistant_views::Moves::default(),
                    window,
                    cx,
                ))
            }),
            Self::Referrals => Builder::account_with_locked(
                |_, ctx, window, cx| referrals::build(ctx, window, cx),
                |_, _, cx| Some(referrals::build_locked(cx)),
            ),
            Self::Subscriptions | Self::Agents => {
                Mixed(|_, device, account, window, cx| agents::build(device, account, window, cx))
            }
            Self::Devices => Builder::account_with_locked(
                |_, ctx, window, cx| devices::build(ctx, window, cx),
                |_, _, cx| Some(devices::build_locked(cx)),
            ),
            Self::ApiExplorer => Builder::account_with_locked(
                |_, ctx, window, cx| api_explorer::build(ctx, window, cx),
                |_, _, cx| Some(api_explorer::build_locked(cx)),
            ),
            Self::Connectors => Builder::account_with_locked(
                |_, ctx, window, cx| connectors::build(ctx, connectors::Side::Mine, window, cx),
                |_, window, cx| Some(connectors::build_locked(window, cx)),
            ),
            Self::ConnectorRules => Builder::account_with_locked(
                |_, ctx, window, cx| connectors::build(ctx, connectors::Side::Org, window, cx),
                |_, window, cx| Some(connectors::build_locked(window, cx)),
            ),
            Self::Mcp => Device(|_, ctx, window, cx| mcp::build(ctx, window, cx)),
            Self::Skills => Builder::account_with_locked(
                |_, ctx, window, cx| skills::build(ctx, window, cx),
                |_, _, cx| Some(skills::build_locked(cx)),
            ),
            Self::Gateways => Device(|_, ctx, window, cx| gateways::build(ctx, window, cx)),
            Self::Worktrees => Mixed(|_, device, account, window, cx| worktrees::build(device, account, window, cx)),
            Self::Subsessions => Device(|_, ctx, window, cx| subsessions::build(ctx, window, cx)),
            Self::Hooks => Device(|_, ctx, window, cx| hooks::build(ctx, window, cx)),
            Self::Servers => Mixed(|_, device, account, window, cx| servers::build(device, account, window, cx)),
            Self::Cloud => Builder::account_with_locked(
                |_, ctx, window, cx| cloud::build(ctx, window, cx),
                |_, _, cx| Some(cloud::build_locked(cx)),
            ),
            Self::DesktopApp => Device(|_, ctx, window, cx| desktop_app::build(ctx, window, cx)),
            Self::LocalSessions => Device(|_, ctx, window, cx| local_sessions::build(ctx, window, cx)),
            Self::SharedMemory => Device(|_, _, _, cx| shared_memory::build(cx)),
            Self::Privacy => Mixed(|_, ctx, account, window, cx| privacy::build(ctx, account, window, cx)),
            Self::Members => Builder::account_with_locked(
                |_, ctx, window, cx| members::build(ctx, window, cx),
                |_, window, cx| Some(members::build_locked(window, cx)),
            ),
            Self::Teams => Builder::account_with_locked(
                |_, ctx, window, cx| teams::build(ctx, window, cx),
                |_, _, cx| Some(teams::build_locked(cx)),
            ),
            Self::Policy => Builder::account_with_locked(
                |_, ctx, window, cx| policy::build(ctx, window, cx),
                |_, _, cx| Some(policy::build_locked(cx)),
            ),
            Self::Providers => Builder::account_with_locked(
                |_, ctx, window, cx| providers::build(ctx, window, cx),
                |_, _, cx| Some(providers::build_locked(cx)),
            ),
            Self::Audit => Builder::account_with_locked(
                |_, ctx, window, cx| audit::build(ctx, window, cx),
                |_, _, cx| Some(audit::build_locked(cx)),
            ),
            Self::General => Builder::account_with_locked(
                |_, ctx, window, cx| general::build(ctx, window, cx),
                |_, window, cx| Some(general::build_locked(window, cx)),
            ),
            Self::DesignSystems => Builder::account_with_locked(
                |_, ctx, window, cx| design_systems::build(ctx, window, cx),
                |_, _, cx| Some(design_systems::build_locked(cx)),
            ),
            Self::Repositories => Builder::account_with_locked(
                |_, ctx, window, cx| repositories::build(ctx, window, cx),
                |_, _, cx| Some(repositories::build_locked(cx)),
            ),
            Self::Billing => Builder::account_with_locked(
                |_, ctx, window, cx| billing::build(ctx, window, cx),
                |_, _, cx| Some(billing::build_locked(cx)),
            ),
        }
    }
}

/// Build a page from an existing signed-in context.
pub fn build(id: PageId, ctx: Ctx, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    id.builder().build(id, ctx.device.clone(), Some(ctx), window, cx)
}

/// The colors the window draws with now. The workspace sets it on every frame; a page reads it with [`palette`].
pub struct Theme(pub Palette);

impl Global for Theme {}

pub fn palette(cx: &App) -> Palette {
    cx.global::<Theme>().0
}

/// A value a page reads from Gateway.
#[derive(Debug, Clone)]
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(SharedString),
}

/// Await a Gateway call and apply its answer to the page, then redraw. A page that closed first ignores it.
pub fn fetch<V: 'static, E: 'static>(
    cx: &mut Context<V>,
    request: impl std::future::Future<Output = Result<Value, E>> + 'static,
    apply: impl FnOnce(&mut V, Result<Value, E>, &mut Context<V>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        let answer = request.await;
        this.update(cx, |view, cx| {
            apply(view, answer, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

/// Render a saved read before its refresh. A closed page or changed account ignores the reply.
pub fn read<V: 'static>(
    cx: &mut Context<V>,
    request: crate::backend::Query,
    mut apply: impl FnMut(&mut V, Result<Value, String>, &mut Context<V>) + 'static,
) {
    let mut request = request;
    cx.spawn(async move |this, cx| {
        if !request.current() {
            return;
        }
        let cached = request.take_cached();
        let saved_value = cached.clone();
        let warmed = saved_value.is_some();
        if let Some(saved) = cached
            && this
                .update(cx, |view, cx| {
                    apply(view, Ok(saved), cx);
                    cx.notify();
                })
                .is_err()
        {
            return;
        }
        let generation = request.generation.clone();
        let answer = request.await;
        if generation
            .as_ref()
            .is_some_and(|(epoch, expected)| epoch.load(std::sync::atomic::Ordering::SeqCst) != *expected)
        {
            return;
        }
        // A refresh failure keeps the saved data, as a stale React Query answer does.
        if warmed && (answer.is_err() || answer.as_ref().ok() == saved_value.as_ref()) {
            return;
        }
        this.update(cx, |view, cx| {
            apply(view, answer, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

/// A saved read whose form fields need the window. Writes use the single-result helper.
pub fn read_in<V: 'static>(
    window: &mut Window,
    cx: &mut Context<V>,
    request: crate::backend::Query,
    mut apply: impl FnMut(&mut V, Result<Value, String>, &mut Window, &mut Context<V>) + 'static,
) {
    let mut request = request;
    cx.spawn_in(window, async move |this, cx| {
        if !request.current() {
            return;
        }
        let cached = request.take_cached();
        let saved_value = cached.clone();
        let warmed = saved_value.is_some();
        if let Some(saved) = cached
            && this
                .update_in(cx, |view, window, cx| {
                    apply(view, Ok(saved), window, cx);
                    cx.notify();
                })
                .is_err()
        {
            return;
        }
        let generation = request.generation.clone();
        let answer = request.await;
        if generation
            .as_ref()
            .is_some_and(|(epoch, expected)| epoch.load(std::sync::atomic::Ordering::SeqCst) != *expected)
        {
            return;
        }
        // A refresh failure keeps the saved data, as a stale React Query answer does.
        if warmed && (answer.is_err() || answer.as_ref().ok() == saved_value.as_ref()) {
            return;
        }
        this.update_in(cx, |view, window, cx| {
            apply(view, answer, window, cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

#[cfg(test)]
mod warm_read_tests {
    use super::read;
    use crate::backend::Query;
    use futures::channel::oneshot;
    use gpui_kit::{Context, IntoElement, Render, Styled, TestAppContext, Window, div, px};
    use serde_json::Value;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    struct Probe {
        seen: Vec<Value>,
    }
    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size(px(1.))
        }
    }
    fn request(cached: Value, epoch: Option<(Arc<AtomicU64>, u64)>) -> (Query, oneshot::Sender<Result<Value, String>>) {
        let (send, receive) = oneshot::channel();
        (
            Query {
                cached: Some(cached),
                cached_generation: None,
                refresh: Box::pin(async move { receive.await.map_err(|_| "cancelled".to_string())? }),
                generation: epoch,
            },
            send,
        )
    }
    #[gpui_kit::test]
    fn shows_saved_data_before_a_pending_refresh_and_preserves_it_on_error(cx: &mut TestAppContext) {
        let (query, send) = request(serde_json::json!("saved"), None);
        let (page, cx) = cx.add_window_view(|_, cx| {
            read(cx, query, |page: &mut Probe, result, _| {
                page.seen.push(result.expect("read succeeds"))
            });
            Probe { seen: vec![] }
        });
        cx.run_until_parked();
        assert_eq!(
            page.read_with(cx, |page, _| page.seen.clone()),
            vec![serde_json::json!("saved")]
        );
        send.send(Err("offline".into())).expect("refresh pending");
        cx.run_until_parked();
        assert_eq!(
            page.read_with(cx, |page, _| page.seen.clone()),
            vec![serde_json::json!("saved")]
        );
    }
    #[gpui_kit::test]
    fn fresh_data_replaces_saved_data_and_a_late_account_reply_is_ignored(cx: &mut TestAppContext) {
        let epoch = Arc::new(AtomicU64::new(0));
        let (query, send) = request(serde_json::json!("saved"), Some((epoch.clone(), 0)));
        let (page, cx) = cx.add_window_view(|_, cx| {
            read(cx, query, |page: &mut Probe, result, _| {
                page.seen.push(result.expect("read succeeds"))
            });
            Probe { seen: vec![] }
        });
        cx.run_until_parked();
        send.send(Ok(serde_json::json!("fresh"))).expect("refresh pending");
        cx.run_until_parked();
        assert_eq!(
            page.read_with(cx, |page, _| page.seen.clone()),
            vec![serde_json::json!("saved"), serde_json::json!("fresh")]
        );
        let (query, send) = request(serde_json::json!("current"), Some((epoch.clone(), 0)));
        page.update(cx, |_, cx| {
            read(cx, query, |page, result, _| {
                page.seen.push(result.expect("read succeeds"))
            })
        });
        cx.run_until_parked();
        epoch.store(1, Ordering::SeqCst);
        send.send(Ok(serde_json::json!("old-account")))
            .expect("refresh pending");
        cx.run_until_parked();
        assert_eq!(
            page.read_with(cx, |page, _| page.seen.last().cloned()),
            Some(serde_json::json!("current"))
        );
    }
    #[gpui_kit::test]
    fn an_unchanged_refresh_does_not_reapply_form_defaults(cx: &mut TestAppContext) {
        let (query, send) = request(serde_json::json!({"name":"saved"}), None);
        let (page, cx) = cx.add_window_view(|_, cx| {
            read(cx, query, |page: &mut Probe, result, _| {
                page.seen.push(result.expect("read succeeds"))
            });
            Probe { seen: vec![] }
        });
        cx.run_until_parked();
        page.update(cx, |page, _| page.seen.push(serde_json::json!({"name":"unsaved edit"})));
        send.send(Ok(serde_json::json!({"name":"saved"})))
            .expect("refresh pending");
        cx.run_until_parked();
        assert_eq!(
            page.read_with(cx, |page, _| page.seen.last().cloned()),
            Some(serde_json::json!({"name":"unsaved edit"}))
        );
    }
}

#[cfg(test)]
mod org_url_tests {
    use super::org_url;

    #[test]
    fn an_org_page_has_one_app_whatever_the_ui_url() {
        for ui in [
            "https://nexus.inc",
            "https://nexus.inc/",
            "https://nexus.inc/app",
            "https://nexus.inc/app/",
        ] {
            assert_eq!(
                org_url(ui, "nexus-xyz", "skills"),
                "https://nexus.inc/app/nexus-xyz/skills",
                "{ui}"
            );
            assert_eq!(
                org_url(ui, "nexus-xyz", "/settings/budgets"),
                "https://nexus.inc/app/nexus-xyz/settings/budgets",
                "{ui}"
            );
        }
    }
}
