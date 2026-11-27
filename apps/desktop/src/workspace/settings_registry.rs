//! Settings' map: the Nexus app's registry (`ui/src/components/nexus-ui/settings-navigation.ts`, `settingsNav`), in
//! Rust. Navigation, search, history and old links read it. Each destination and field has the Nexus app's stable ID:
//! its slug (`computer`) and its anchor (`branch-prefix`). A test reads the TypeScript file and fails when the two
//! maps differ, so a change there needs one here.
//!
//! Global holds Preferences and Computer, for every account. The current account's groups follow: Account, People,
//! Controls, Tools, Activity and Admin. A destination this build does not draw opens in the browser, by name.
//!
//! A destination is drawn from [`SettingsSection`]s. Preferences, Computer, Subscriptions and Integrations stack
//! several native views on one page, with a section index. Old sections (Appearance, Gateways, MCP servers…) stay valid
//! places: each opens its destination at its own section.

use super::SettingsSection;
use crate::pages::PageId;

/// Where a setting is kept, as the Nexus app labels it (`SettingsScope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scope {
    /// This computer only.
    Device,
    /// The person, on every computer.
    You,
    /// The organization.
    Org,
}

/// A group of the nav (`SettingsGroupId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Group {
    Global,
    Code,
    Account,
    People,
    Controls,
    Tools,
    Activity,
    Admin,
}

impl Group {
    pub const ALL: [Group; 8] = [
        Group::Global,
        Group::Code,
        Group::Account,
        Group::People,
        Group::Controls,
        Group::Tools,
        Group::Activity,
        Group::Admin,
    ];

    /// The small label over the rows; Global has none.
    pub fn label(self) -> &'static str {
        match self {
            Group::Global => "",
            Group::Code => "Code",
            Group::Account => "Account",
            Group::People => "People",
            Group::Controls => "Controls",
            Group::Tools => "Tools",
            Group::Activity => "Activity",
            Group::Admin => "Admin",
        }
    }
}

/// A destination of Settings: one row of the nav.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Dest {
    Preferences,
    Computer,
    Subsessions,
    LocalProviders,
    Privacy,
    Subscriptions,
    ApiKeys,
    Developer,
    Devices,
    Referrals,
    Members,
    Teams,
    Budgets,
    Policy,
    Providers,
    Slack,
    /// The person's own connector accounts and this computer's MCP servers. This build titles it Connections.
    Integrations,
    /// This build's own: the organization's connector rules, for owners and admins. Nexus UI keeps them on
    /// its one connectors page, so the parity tests leave this row out.
    ConnectorRules,
    Skills,
    Repositories,
    Usage,
    Audit,
    General,
    Design,
    Billing,
    Data,
    /// This build's own: who is signed in, and Sign out. Opened from the account's name, not a row.
    Account,
    /// Account › Assistant (brand v5 A4): the Assistant's settings, shown only with an agent API. It has no section
    /// here: the row opens the Assistant's own What Nexus knows, with Approvals beside it (`open_dest`), and this
    /// address only without the Assistant.
    Assistant,
}

/// What decides which rows show (`SettingsNavOptions`). This build is the desktop app, and draws Members and
/// Repositories itself, so `desktop` and `serverPages` are always on.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Visibility {
    pub managed_gateway: bool,
    pub local_providers: bool,
    pub signed_out: bool,
    pub billing: bool,
    pub usage: bool,
    pub referrals: bool,
    pub providers: bool,
    pub slack: bool,
    pub personal: bool,
    /// The app has an agent API, so the Assistant shows (`nexus_desktop_core::agent::configured`).
    pub assistant: bool,
    /// Account › Developer apps: Gateway has `/v1/developer/clients` (`NEXUS_OAUTH_CLIENT_TIERS=on`).
    pub developer: bool,
    /// An owner or admin (`policy.manage`): the organization's connector rules show.
    pub connector_rules: bool,
}

impl Visibility {
    /// From `GET /v1/tenants/me`'s permissions, as `settings-search.tsx` reads them.
    pub fn from_account(account: Option<&crate::gateway::Account>, managed_gateway: bool) -> Visibility {
        let can = |permission: &str| account.is_some_and(|account| account.permissions.iter().any(|p| p == permission));
        Visibility {
            managed_gateway,
            local_providers: false,
            signed_out: false,
            billing: can("org.billing"),
            usage: can("spend.read_all"),
            referrals: can("referrals.read"),
            providers: can("provider_keys.manage"),
            slack: can("chat_apps.manage"),
            personal: account.is_some_and(|account| account.personal),
            assistant: false,
            developer: false,
            connector_rules: can("policy.manage"),
        }
    }

    pub fn signed_out(managed_gateway: bool) -> Visibility {
        Visibility {
            managed_gateway,
            local_providers: true,
            signed_out: true,
            ..Visibility::all()
        }
    }

    /// With the Assistant's row, when the app has an agent API.
    pub fn with_assistant(self, assistant: bool) -> Visibility {
        Visibility { assistant, ..self }
    }

    /// Every shared row, for address resolution and the Nexus app's test fixtures.
    pub fn all() -> Visibility {
        Visibility {
            managed_gateway: false,
            local_providers: false,
            signed_out: false,
            billing: true,
            usage: true,
            referrals: true,
            providers: true,
            slack: true,
            personal: false,
            assistant: true,
            developer: true,
            connector_rules: true,
        }
    }
}

/// A setting on a destination (`SettingsField`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Field {
    pub account: Option<crate::settings_fields::AccountField>,
    pub label: &'static str,
    pub description: &'static str,
    pub anchor: &'static str,
    pub keywords: &'static [&'static str],
    pub scope: Option<Scope>,
    /// The section here that draws it; None for a browser destination.
    pub section: Option<SettingsSection>,
}

const fn field(
    label: &'static str,
    description: &'static str,
    anchor: &'static str,
    keywords: &'static [&'static str],
    section: SettingsSection,
) -> Field {
    Field {
        account: None,
        label,
        description,
        anchor,
        keywords,
        scope: None,
        section: Some(section),
    }
}

const fn account_field(
    label: &'static str,
    description: &'static str,
    control: crate::settings_fields::AccountField,
    keywords: &'static [&'static str],
    section: SettingsSection,
) -> Field {
    Field {
        account: Some(control),
        ..field(label, description, control.0, keywords, section)
    }
}

const fn yours(field: Field) -> Field {
    Field {
        scope: Some(Scope::You),
        ..field
    }
}

use SettingsSection as S;

const PREFERENCES_SECTIONS: &[SettingsSection] = &[S::Appearance, S::Notifications, S::Shortcuts];
const COMPUTER_SECTIONS: &[SettingsSection] = &[
    S::Page(PageId::Gateways),
    S::Page(PageId::Worktrees),
    S::Page(PageId::Hooks),
    S::Page(PageId::Servers),
    S::Page(PageId::Cloud),
    S::Page(PageId::DesktopApp),
    S::Page(PageId::LocalSessions),
    S::Page(PageId::SharedMemory),
];
const SUBSCRIPTIONS_SECTIONS: &[SettingsSection] = &[S::Page(PageId::Agents)];
const INTEGRATIONS_SECTIONS: &[SettingsSection] = &[S::Page(PageId::Connectors), S::Page(PageId::Mcp)];

impl Dest {
    /// Every destination, in the nav's order.
    pub const ALL: [Dest; 28] = [
        Dest::Preferences,
        Dest::Computer,
        Dest::Subsessions,
        Dest::LocalProviders,
        Dest::Privacy,
        Dest::Subscriptions,
        Dest::ApiKeys,
        Dest::Developer,
        Dest::Devices,
        Dest::Referrals,
        Dest::Members,
        Dest::Teams,
        Dest::Budgets,
        Dest::Policy,
        Dest::Providers,
        Dest::Slack,
        Dest::Integrations,
        Dest::ConnectorRules,
        Dest::Skills,
        Dest::Repositories,
        Dest::Usage,
        Dest::Audit,
        Dest::General,
        Dest::Design,
        Dest::Billing,
        Dest::Data,
        Dest::Account,
        Dest::Assistant,
    ];

    /// The stable ID: the Nexus app's page slug under `/settings/`.
    pub fn slug(self) -> &'static str {
        match self {
            Dest::Preferences => "preferences",
            Dest::Computer => "computer",
            Dest::Subsessions => "subsessions",
            Dest::Privacy => "privacy",
            Dest::Subscriptions => "subscriptions",
            Dest::ApiKeys => "api-keys",
            Dest::Developer => "developer",
            Dest::Devices => "devices",
            Dest::Referrals => "referrals",
            Dest::Members => "members",
            Dest::Teams => "teams",
            Dest::Budgets => "budgets",
            Dest::Policy => "model-policy",
            Dest::Providers => "providers",
            Dest::LocalProviders => "local-providers",
            Dest::Slack => "slack",
            Dest::Integrations => "integrations",
            Dest::ConnectorRules => "connector-rules",
            Dest::Skills => "skills",
            Dest::Repositories => "repositories",
            Dest::Usage => "usage",
            Dest::Audit => "audit",
            Dest::General => "general",
            Dest::Design => "design-systems",
            Dest::Billing => "billing",
            Dest::Data => "data",
            Dest::Account => "account",
            Dest::Assistant => "assistant",
        }
    }

    /// Nexus UI's path under `/app/<org>/`, for a browser handoff.
    pub fn web_path(self) -> String {
        match self {
            Dest::Referrals => "referrals".into(),
            Dest::Skills => "skills".into(),
            Dest::Usage => "usage".into(),
            Dest::Assistant => "agent?view=memory".into(),
            // Its page under Integrations. The old `settings/slack` redirects there.
            Dest::Slack => "settings/connectors/slack".into(),
            other => format!("settings/{}", other.slug()),
        }
    }

    /// The row's one-word label.
    pub fn label(self) -> &'static str {
        match self {
            Dest::Preferences => "Preferences",
            Dest::Computer => "Computer",
            Dest::Subsessions => "Subsessions",
            Dest::Privacy => "Privacy",
            Dest::Subscriptions => "Subscriptions",
            Dest::ApiKeys => "Keys",
            Dest::Developer => "Developer apps",
            Dest::Devices => "Devices",
            Dest::Referrals => "Referrals",
            Dest::Members => "Members",
            Dest::Teams => "Teams",
            Dest::Budgets => "Budgets",
            Dest::Policy => "Policy",
            Dest::Providers => "Providers",
            Dest::LocalProviders => "Local providers",
            Dest::Slack => "Slack",
            Dest::Integrations => "Integrations",
            Dest::ConnectorRules => "Integrations",
            Dest::Skills => "Skills",
            Dest::Repositories => "Repositories",
            Dest::Usage => "Usage",
            Dest::Audit => "Audit",
            Dest::General => "General",
            Dest::Design => "Design",
            Dest::Billing => "Billing",
            Dest::Data => "Data",
            Dest::Account => "Account",
            Dest::Assistant => "Assistant",
        }
    }

    pub fn group(self) -> Group {
        match self {
            Dest::Preferences | Dest::Computer | Dest::Privacy | Dest::LocalProviders => Group::Global,
            Dest::Subsessions => Group::Code,
            Dest::Assistant
            | Dest::Subscriptions
            | Dest::ApiKeys
            | Dest::Developer
            | Dest::Devices
            | Dest::Referrals
            | Dest::Account => Group::Account,
            Dest::Members | Dest::Teams => Group::People,
            Dest::Budgets | Dest::Policy | Dest::Providers => Group::Controls,
            // Slack is a connector: the Nexus app shows it on Integrations, under Tools.
            Dest::Integrations | Dest::ConnectorRules | Dest::Skills | Dest::Repositories | Dest::Slack => Group::Tools,
            Dest::Usage | Dest::Audit => Group::Activity,
            Dest::General | Dest::Design | Dest::Billing | Dest::Data => Group::Admin,
        }
    }

    pub fn scope(self) -> Scope {
        match self {
            Dest::Preferences | Dest::Computer | Dest::Privacy | Dest::LocalProviders | Dest::Subsessions => {
                Scope::Device
            }
            Dest::Subscriptions
            | Dest::ApiKeys
            | Dest::Developer
            | Dest::Devices
            | Dest::Referrals
            | Dest::Integrations
            | Dest::Skills
            | Dest::Account
            | Dest::Assistant => Scope::You,
            _ => Scope::Org,
        }
    }

    pub fn description(self, visibility: &Visibility) -> &'static str {
        match self {
            Dest::Preferences => "How Nexus looks, alerts you and answers your keys on this device.",
            Dest::Subsessions => "Choose how agents start subsessions and set limits.",
            Dest::Computer => "Gateways, worktrees, servers, the app and its history, on this computer only.",
            Dest::Privacy => "Local conversation discovery, outside access and account privacy controls.",
            Dest::Subscriptions => {
                "Choose how each coding agent pays, and manage the Claude and ChatGPT accounts on this computer."
            }
            Dest::ApiKeys => "API keys that authenticate your tools and scripts against Gateway.",
            Dest::Developer => "Register a fork of a Nexus app and get its client ID.",
            Dest::Devices => "The devices that can read your synced sessions, and pairing a new one.",
            Dest::Referrals => "Earn credit for your Personal account and track referral rewards.",
            Dest::Members => "People and their roles.",
            Dest::Teams => "Team membership and organization units.",
            Dest::Budgets => {
                "Organization budget, default and individual person limits, card usage limit and team caps. Setting a cap does not buy credits."
            }
            Dest::Policy if visibility.personal => "Which models and tools you can use.",
            Dest::Policy => "Which models and tools teams can use.",
            Dest::Providers => "Provider API keys and organization model access.",
            Dest::LocalProviders => "Manage API keys and model servers saved on this computer.",
            Dest::Slack => "Slack workspaces bound to this organization.",
            Dest::Integrations => "Your own accounts on other services, and the MCP servers on this computer.",
            Dest::ConnectorRules => {
                "Who in your organization can use each connector, its shared account, and what agents may change."
            }
            Dest::Skills => "Your skills, and the ones your organization shares.",
            Dest::Repositories => "Repositories connected through the Nexus GitHub App.",
            Dest::Usage => "Activity and model costs. Spending limits are set in Budgets.",
            Dest::Audit => "Review changes to settings and access.",
            Dest::General if visibility.personal => "Your account's name, logo and address.",
            Dest::General => "Your organization's name, logo and address.",
            Dest::Design => "The tokens and notes Nexus Design makes and checks designs with.",
            Dest::Billing => "Buy credits, check credit balances, and manage payments and receipts.",
            Dest::Data => "How long chats are kept, and who can open what you share.",
            Dest::Account => "Who you are signed in as, and signing out.",
            Dest::Assistant => "Your Nexus: what it knows and what it does without asking.",
        }
    }

    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            Dest::Preferences => &["appearance", "accessibility", "shortcuts", "notifications"],
            Dest::Subsessions => &["subsession", "orchestration", "delegation", "subagent"],
            Dest::Computer => &["this computer", "this mac", "machine", "desktop app"],
            Dest::Privacy => &["private", "isolation", "separate", "permissions"],
            Dest::Subscriptions => &[
                "plan usage",
                "subscription",
                "quota",
                "limits",
                "routing",
                "coding agents",
            ],
            Dest::ApiKeys => &["api keys", "token", "credentials", "authentication", "secret"],
            Dest::Developer => &[
                "oauth",
                "client id",
                "client_id",
                "fork",
                "app",
                "developer",
                "register",
                "scopes",
                "revoke",
            ],
            Dest::Devices => &[
                "encryption",
                "end-to-end",
                "pairing",
                "trusted",
                "qr code",
                "revoke",
                "browser",
                "fingerprint",
            ],
            Dest::Referrals => &["invite", "friend"],
            Dest::Members => &["users", "invite", "permissions", "access"],
            Dest::Teams => &["groups", "departments", "members"],
            Dest::Budgets => &["limit", "cap", "spending", "default", "person"],
            Dest::Policy => &[
                "model policy",
                "rules",
                "allowlist",
                "blocklist",
                "permissions",
                "access",
                "allowed",
                "denied",
            ],
            Dest::LocalProviders => &[
                "api key",
                "byok",
                "bring your own key",
                "anthropic",
                "openai",
                "model server",
            ],
            Dest::Providers => &["byok", "bring your own key", "anthropic", "openai", "credentials"],
            Dest::Slack => &["chat", "workspace", "install", "app"],
            Dest::Integrations => &["mcp", "model context protocol", "connectors", "connections", "tools"],
            Dest::ConnectorRules => &[
                "connectors",
                "who can use",
                "availability",
                "organization account",
                "agent writes",
                "write grants",
            ],
            Dest::Skills => &["skill md", "library", "slash", "promotions"],
            Dest::Repositories => &["repo", "github", "gitlab", "indexing"],
            Dest::Usage => &["costs", "calls", "reports", "consumption"],
            Dest::Audit => &["audit log", "history", "activity", "events"],
            Dest::General => &["organization", "workspace", "company"],
            Dest::Design => &[
                "design",
                "tokens",
                "brand",
                "css variables",
                "tailwind",
                "default",
                "nexus design",
            ],
            Dest::Billing => &["invoice", "payment", "credits", "buy", "top up", "referral rewards"],
            Dest::Data => &["retention", "sharing", "privacy"],
            // This build's words for its own page.
            Dest::Account => &["profile", "sign out", "email"],
            Dest::Assistant => &["agent", "nexus", "memory", "approvals", "your agent"],
        }
    }

    /// Old addresses that open this destination (`also`), beside `/settings/<slug>`.
    pub fn also(self) -> &'static [&'static str] {
        match self {
            Dest::Preferences => &["/settings"],
            Dest::Computer => &[
                "/settings/this-computer",
                "/settings/gateways",
                "/settings/worktrees",
                "/settings/servers",
                "/settings/cloud",
                "/settings/local-sessions",
            ],
            Dest::Subscriptions => &["/settings/plan-usage", "/settings/agents"],
            Dest::ApiKeys => &["/keys"],
            Dest::Members => &["/members"],
            Dest::Teams => &["/teams"],
            Dest::Budgets => &["/budgets"],
            Dest::Providers => &["/settings/provider-keys"],
            Dest::Integrations => &["/settings/mcp", "/settings/connectors"],
            Dest::Repositories => &["/repositories", "/settings/github"],
            Dest::Audit => &["/audit"],
            Dest::Billing => &["/billing"],
            _ => &[],
        }
    }

    /// Whether this account sees the row (`settingsNav`'s filters). Account is never a row.
    pub fn visible(self, visibility: &Visibility) -> bool {
        match self {
            Dest::LocalProviders => visibility.local_providers,
            Dest::Referrals => visibility.referrals,
            Dest::Developer => visibility.developer,
            Dest::Members | Dest::Teams => !visibility.personal,
            Dest::Providers => visibility.providers,
            Dest::Slack => visibility.slack,
            Dest::Billing => visibility.billing,
            Dest::Usage => visibility.usage,
            Dest::Account => false,
            Dest::Assistant => visibility.assistant,
            Dest::ConnectorRules => visibility.connector_rules,
            _ => true,
        }
    }

    /// The sections drawn on its page, top to bottom. Empty: this build does not draw it, and it opens in the browser.
    pub fn sections(self) -> &'static [SettingsSection] {
        match self {
            Dest::Preferences => PREFERENCES_SECTIONS,
            Dest::Computer => COMPUTER_SECTIONS,
            Dest::Subsessions => &[S::Page(PageId::Subsessions)],
            Dest::Privacy => &[S::Page(PageId::Privacy)],
            Dest::Subscriptions => SUBSCRIPTIONS_SECTIONS,
            Dest::Integrations => INTEGRATIONS_SECTIONS,
            Dest::ConnectorRules => &[S::Page(PageId::ConnectorRules)],
            Dest::ApiKeys => &[S::ApiKeys],
            Dest::Account => &[S::Account],
            Dest::Devices => &[S::Page(PageId::Devices)],
            Dest::Referrals => &[S::Page(PageId::Referrals)],
            Dest::Members => &[S::Page(PageId::Members)],
            Dest::Teams => &[S::Page(PageId::Teams)],
            Dest::Policy => &[S::Page(PageId::Policy)],
            Dest::Providers => &[S::Page(PageId::Providers)],
            Dest::LocalProviders => &[S::LocalProviders],
            Dest::Repositories => &[S::Page(PageId::Repositories)],
            Dest::Audit => &[S::Page(PageId::Audit)],
            Dest::General | Dest::Data => &[S::Page(PageId::General)],
            Dest::Design => &[S::Page(PageId::DesignSystems)],
            Dest::Billing => &[S::Page(PageId::Billing)],
            Dest::Usage | Dest::Budgets => &[S::Page(PageId::Usage)],
            Dest::Skills => &[S::Page(PageId::Skills)],
            Dest::Developer | Dest::Slack | Dest::Assistant => &[],
        }
    }

    /// The section a nav row opens: the page's first.
    pub fn target(self) -> Option<&'static str> {
        (self == Dest::Budgets).then_some("usage-limits")
    }

    pub fn first(self) -> Option<SettingsSection> {
        self.sections().first().copied()
    }

    pub fn first_for(self, visibility: &Visibility) -> Option<SettingsSection> {
        if visibility.signed_out {
            self.sections()
                .iter()
                .copied()
                .find(|section| !section.needs_account())
                .or_else(|| self.first())
        } else {
            self.first()
        }
    }

    /// Its settings, for search (`fields`). Visibility removes the ones this account cannot use.
    pub fn fields(self, visibility: &Visibility) -> Vec<Field> {
        let gateways = S::Page(PageId::Gateways);
        let worktrees = S::Page(PageId::Worktrees);
        let app = S::Page(PageId::DesktopApp);
        match self {
            Dest::Subsessions => vec![field(
                "Subsessions",
                "Let agents start subsessions.",
                "subsessions",
                &[],
                S::Page(PageId::Subsessions),
            )],
            Dest::Preferences => {
                let mut fields = vec![
                    field(
                        "Theme",
                        "Follow your system, or choose light or dark mode.",
                        "theme",
                        &["appearance", "color", "colour"],
                        S::Appearance,
                    ),
                    field(
                        "Language",
                        "Show the app in English, Spanish or Portuguese, or follow your computer.",
                        "language",
                        &[
                            "idioma",
                            "locale",
                            "translation",
                            "español",
                            "português",
                            "spanish",
                            "portuguese",
                        ],
                        S::Appearance,
                    ),
                    field(
                        "Transparency",
                        "Let your desktop show through the window: opaque, frost or glass, or fine-tune it.",
                        "transparency",
                        &[
                            "frost",
                            "glass",
                            "opaque",
                            "blur",
                            "fine-tune",
                            "slider",
                            "custom",
                            "translucent",
                            "see-through",
                            "vibrancy",
                        ],
                        S::Appearance,
                    ),
                    field(
                        "Text size",
                        "Scale text in the app, code, and terminal.",
                        "text-size",
                        &["font", "zoom", "larger", "smaller", "scale"],
                        S::Appearance,
                    ),
                    field(
                        "Reduce motion",
                        "Reduce animated effects in the app.",
                        "reduce-motion",
                        &["animations", "movement", "accessibility"],
                        S::Appearance,
                    ),
                    field(
                        "Notifications",
                        "Get an alert when a session needs your attention.",
                        "notifications",
                        &["alerts", "sound", "banner", "finished", "completion"],
                        S::Notifications,
                    ),
                    field(
                        "Keyboard shortcuts",
                        "Find keyboard commands for moving around Nexus.",
                        "shortcuts",
                        &["hotkeys", "keybindings", "keys"],
                        S::Shortcuts,
                    ),
                ];
                if !visibility.personal {
                    fields.push(account_field(
                        "Show when I'm online",
                        "Let teammates see when you have Nexus open, and what you are doing.",
                        super::team_presence::SHOW_ONLINE,
                        &[
                            "presence",
                            "online",
                            "status",
                            "team",
                            "privacy",
                            "green dot",
                            "activity",
                        ],
                        S::Appearance,
                    ));
                }
                fields
            }
            Dest::Computer => {
                let mut fields = vec![
                    field(
                        "Gateways",
                        if visibility.managed_gateway {
                            "The gateway address set by your organization."
                        } else {
                            "Add or switch the gateway Nexus connects to."
                        },
                        "gateways",
                        &["connection", "endpoint", "environment", "server url"],
                        gateways,
                    ),
                    field(
                        "Gateway address",
                        "The endpoint for your current gateway.",
                        "gateway-address",
                        &["base url", "server address", "connection"],
                        gateways,
                    ),
                ];
                if !visibility.managed_gateway {
                    fields.push(field(
                        "Add a Gateway",
                        "Connect to another gateway address.",
                        "add-gateway",
                        &["endpoint", "connection"],
                        gateways,
                    ));
                }
                fields.extend([
                    field(
                        "Worktree location",
                        "Where new session checkouts are stored.",
                        "worktrees",
                        &["folder", "directory", "path", "worktree", "git", "checkouts"],
                        worktrees,
                    ),
                    field(
                        "Worktree cleanup",
                        "Remove session checkouts by rule, when nothing would be lost.",
                        "worktree-cleanup",
                        &["delete", "disk", "merged", "archived", "idle", "worktree"],
                        worktrees,
                    ),
                    field(
                        "Branch names",
                        "What new worktree branches start with.",
                        "branch-prefix",
                        &["prefix", "branch", "naming", "git"],
                        worktrees,
                    ),
                    field(
                        "Hooks",
                        "Run a command when a coding session finishes, needs you, merges or fails a check.",
                        "hooks",
                        &["hook", "script", "command", "automation", "event", "shell"],
                        S::Page(PageId::Hooks),
                    ),
                    yours(field(
                        "Project setup scripts",
                        "Commands to run before a coding session starts.",
                        "setup-scripts",
                        &["install", "initialization", "setup"],
                        worktrees,
                    )),
                    yours(field(
                        "Servers",
                        "Run coding sessions on your own Linux machines.",
                        "servers",
                        &["ssh", "host", "remote"],
                        S::Page(PageId::Servers),
                    )),
                    yours(field(
                        "Cloud",
                        "Run sessions on a cloud machine that keeps going when your laptop sleeps.",
                        "cloud",
                        &["cloud machine", "sandbox", "remote", "sleep", "github"],
                        S::Page(PageId::Cloud),
                    )),
                    field(
                        "Global shortcut",
                        "Open Nexus from any app.",
                        "global-shortcut",
                        &["hotkey", "option space", "keyboard"],
                        app,
                    ),
                    field(
                        "Screen recording shortcut",
                        "Record your screen and voice for an agent.",
                        "record-shortcut",
                        &["capture", "recording"],
                        app,
                    ),
                    field(
                        "Open at login",
                        "Start Nexus when you sign in to this computer.",
                        "open-at-login",
                        &["startup", "launch", "boot", "automatic"],
                        app,
                    ),
                    field(
                        "Menu bar only",
                        "Keep Nexus in the menu bar without a Dock icon.",
                        "menu-bar-only",
                        &["tray", "dock", "icon"],
                        app,
                    ),
                    field(
                        "Follow the agent's edits",
                        "Changes opens on each file the agent edits.",
                        "follow-edits",
                        &["changes", "diff", "auto open", "side pane", "follow"],
                        app,
                    ),
                    field(
                        "Archive merged sessions",
                        "A session whose pull request merged goes to Archived, not Settled.",
                        "archive-merged",
                        &[
                            "pull request",
                            "merged",
                            "archive",
                            "settled",
                            "auto archive",
                            "sidebar",
                        ],
                        app,
                    ),
                    field(
                        "Desktop updates",
                        "Check your app version and available updates.",
                        "desktop-updates",
                        &["update", "upgrade", "version", "channel", "release"],
                        app,
                    ),
                    field(
                        "Shared memory",
                        "Share local memories between coding agents and worktrees.",
                        "shared-memory",
                        &["memory", "remember", "recall", "import", "export"],
                        S::Page(PageId::SharedMemory),
                    ),
                    field(
                        "History",
                        "Find conversations from coding agents on this computer.",
                        "history",
                        &["local sessions", "terminal", "resume", "local conversations"],
                        S::Page(PageId::LocalSessions),
                    ),
                ]);
                fields
            }
            Dest::Privacy => vec![
                field(
                    "Show conversations from other apps",
                    "Choose whether Nexus reads local agent conversation titles and folders. Off by default.",
                    "local-session-history",
                    &[
                        "history",
                        "discover",
                        "scan",
                        "local sessions",
                        "isolation",
                        "separate",
                        "claude",
                        "codex",
                    ],
                    S::Page(PageId::Privacy),
                ),
                field(
                    "Let other apps use Code sessions",
                    "Review access for scripts, the nexus CLI and other agents.",
                    "outside-access",
                    &["outside access", "revoke", "consent", "permissions"],
                    S::Page(PageId::Privacy),
                ),
            ],
            Dest::Subscriptions => vec![
                field(
                    "Plan usage",
                    "Your Claude and ChatGPT accounts on this computer, and what is left of each plan.",
                    "plan-usage",
                    &["quota", "limits", "claude max", "chatgpt"],
                    S::Page(PageId::Agents),
                ),
                account_field(
                    "Codex routing",
                    "Choose whether Nexus or your ChatGPT plan pays for Codex.",
                    crate::pages::CODEX_ROUTING,
                    &["openai", "route", "gateway"],
                    S::Page(PageId::Agents),
                ),
                account_field(
                    "Claude Code routing",
                    "Choose whether Nexus or your Claude plan pays for Claude Code.",
                    crate::pages::CLAUDE_ROUTING,
                    &["anthropic", "route", "gateway"],
                    S::Page(PageId::Agents),
                ),
            ],
            Dest::Policy => {
                let policy = S::Page(PageId::Policy);
                vec![
                    field(
                        "Chat default model",
                        "The default model for new chats.",
                        "default-model-chat",
                        &[],
                        policy,
                    ),
                    field(
                        "Claude Code default model",
                        "The default model for Claude Code.",
                        "default-model-claude",
                        &[],
                        policy,
                    ),
                    field(
                        "Codex default model",
                        "The default model for Codex.",
                        "default-model-codex",
                        &[],
                        policy,
                    ),
                    field(
                        "Web search",
                        "Let people turn on web search in a chat.",
                        "web-search",
                        &["internet", "browse", "online", "sources"],
                        policy,
                    ),
                    field(
                        "Model that searches",
                        "Pick the model that runs every chat's web search.",
                        "web-search-model-row",
                        &["search model", "cheapest", "pin"],
                        policy,
                    ),
                    field(
                        "Agents on personal plans",
                        "Let people use their own Claude or ChatGPT plan.",
                        "personal-agent-plans",
                        &["subscriptions", "licenses", "allow personal"],
                        policy,
                    ),
                ]
            }
            Dest::Integrations => vec![
                field(
                    "Connectors",
                    "Tools available through Nexus, their accounts, and who can use them.",
                    "connectors",
                    &[
                        "integrations",
                        "connections",
                        "connected accounts",
                        "oauth",
                        "agent writes",
                        "linear",
                        "github",
                        "slack",
                        "jira",
                        "notion",
                    ],
                    S::Page(PageId::Connectors),
                ),
                field(
                    "MCP servers",
                    "The built-in Nexus server and additional servers for your coding agents.",
                    "mcp-servers",
                    &["model context protocol", "tools"],
                    S::Page(PageId::Mcp),
                ),
            ],
            Dest::General => {
                let general = S::Page(PageId::General);
                vec![
                    field(
                        "Organization logo",
                        "The image shown in the account switcher and shared pages.",
                        "org-logo",
                        &["picture", "icon", "avatar", "branding"],
                        general,
                    ),
                    field(
                        "Organization name",
                        "The name shown to organization members.",
                        "org-name",
                        &["rename", "company", "workspace"],
                        general,
                    ),
                    field(
                        "Organization address",
                        "The address for your organization's pages.",
                        "org-address",
                        &["slug", "url"],
                        general,
                    ),
                    field(
                        "Approvals and requests for changes",
                        "Let automated reviewers approve or block pull requests.",
                        "review-events",
                        &[
                            "code review",
                            "nexus review",
                            "approve",
                            "request changes",
                            "pull request",
                        ],
                        general,
                    ),
                    field(
                        "Synced coding sessions",
                        "Choose what leaves a person's computer when coding sessions sync.",
                        "synced-sessions",
                        &[
                            "sync",
                            "encryption",
                            "recovery key",
                            "content policy",
                            "privacy",
                            "devices",
                        ],
                        general,
                    ),
                ]
            }
            Dest::Data => {
                vec![
                    field(
                        "Sharing and pages",
                        "Public links, email domains, expiry, and shared code sources.",
                        "sharing-settings",
                        &[
                            "external",
                            "artifacts",
                            "share",
                            "expiration",
                            "public pages",
                            "public links",
                            "readers",
                            "continue chat",
                        ],
                        S::Page(PageId::General),
                    ),
                    field(
                        "Chat retention",
                        "How long chats are kept.",
                        "retention-days",
                        &["delete", "retention", "days", "privacy"],
                        S::Page(PageId::General),
                    ),
                ]
            }
            Dest::ApiKeys if visibility.signed_out => vec![field(
                "API Explorer",
                "Call Gateway's endpoints with your access, and see each request and answer.",
                "api-explorer",
                &["api explorer", "request"],
                S::Page(PageId::ApiExplorer),
            )],
            // The Assistant's own views (A8, A11): search finds them, and `open_hit` opens each here.
            Dest::Assistant => ASSISTANT_FIELDS.to_vec(),
            Dest::ConnectorRules => {
                let rules = S::Page(PageId::ConnectorRules);
                vec![
                    field(
                        "Who can use each connector",
                        "Turn a connector off, let people connect their own account, or lend the organization's.",
                        "who-can-use",
                        &["availability", "organization account", "teams", "turn off"],
                        rules,
                    ),
                    field(
                        "Changes from chat",
                        "Let agents in chat change things through connectors, and see each module's write grants.",
                        "agent-writes",
                        &["agent writes", "write grants", "chat writes"],
                        rules,
                    ),
                ]
            }
            _ => Vec::new(),
        }
    }
}

/// Account › Assistant's views that search finds by name: Passwords and payments (A8) and Privacy (A11). Each opens the
/// Assistant's own view (`assistant_view`), not a Settings section.
const ASSISTANT_FIELDS: [Field; 2] = [
    Field {
        account: None,
        label: "Passwords and payments",
        description: "The logins and the card Nexus may use, and every use of them.",
        anchor: "vault",
        keywords: &[
            "passwords",
            "payments",
            "vault",
            "logins",
            "card",
            "1password",
            "bitwarden",
            "freeze",
        ],
        scope: Some(Scope::You),
        section: None,
    },
    Field {
        account: None,
        label: "Privacy",
        description: "Pause the Assistant, revoke all its access, and see what it can read.",
        anchor: "privacy",
        keywords: &["pause", "revoke", "trust", "what it can see", "training", "disconnect"],
        scope: Some(Scope::You),
        section: None,
    },
];

/// The Assistant's view an Account › Assistant hit opens: a field's view, else What Nexus knows.
pub(super) fn assistant_view(anchor: Option<&str>) -> PageId {
    match anchor {
        Some("vault") => PageId::AgentVault,
        Some("privacy") => PageId::AgentPrivacy,
        _ => PageId::AgentMemory,
    }
}

/// How wide a settings section's content is. Every form shares one width, so the title and the left edge stay put
/// between sections. Only a section that draws a table of fixed columns is wider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsWidth {
    Form,
    Table,
}

impl SettingsWidth {
    /// The content width in points, without the gutters that the settings shell adds.
    pub(super) fn points(self) -> f32 {
        use crate::pages::ui;
        match self {
            SettingsWidth::Form => ui::SETTINGS_COLUMN,
            SettingsWidth::Table => ui::PLACE_WIDTH - 2. * ui::PAGE_GUTTER,
        }
    }
}

impl SettingsSection {
    /// Members, Teams, Audit log and Repositories draw tables whose fixed columns need more than a form's width.
    pub(super) fn width(self) -> SettingsWidth {
        match self {
            S::Page(PageId::Members | PageId::Teams | PageId::Audit | PageId::Repositories) => SettingsWidth::Table,
            _ => SettingsWidth::Form,
        }
    }

    pub(super) fn content_width(self) -> f32 {
        self.width().points()
    }

    /// The destination whose page draws it. API Explorer sits under Keys: this build's own page, linked from there.
    pub(super) fn dest(self) -> Dest {
        match self {
            S::GatewayLink(dest) => dest,
            S::Page(PageId::ApiExplorer) => Dest::ApiKeys,
            // Plan usage is part of the Agents section now.
            S::Page(PageId::Subscriptions) => Dest::Subscriptions,
            S::Page(PageId::Usage) => Dest::Usage,
            section => Dest::ALL
                .into_iter()
                .find(|dest| dest.sections().contains(&section))
                .unwrap_or(Dest::Preferences),
        }
    }

    /// The stable ID of its section on the page (`/settings/computer#servers`).
    pub(super) fn anchor(self) -> &'static str {
        match self {
            S::Appearance => "appearance",
            S::LocalProviders => "local-providers",
            S::Notifications => "notifications",
            S::Shortcuts => "shortcuts",
            S::ApiKeys => "api-keys",
            S::Account => "account",
            S::GatewayLink(dest) => dest.slug(),
            S::Page(PageId::Gateways) => "gateways",
            S::Page(PageId::Worktrees) => "worktrees",
            S::Page(PageId::Hooks) => "hooks",
            S::Page(PageId::Servers) => "servers",
            S::Page(PageId::Cloud) => "cloud",
            S::Page(PageId::DesktopApp) => "app",
            S::Page(PageId::LocalSessions) => "history",
            S::Page(PageId::Subscriptions) => "plan-usage",
            S::Page(PageId::Agents) => "agents",
            S::Page(PageId::Mcp) => "mcp-servers",
            S::Page(PageId::Connectors) => "connectors",
            S::Page(PageId::ApiExplorer) => "api-explorer",
            S::Page(id) => id.web_path().rsplit('/').next().unwrap_or_default(),
        }
    }

    /// Its tab's name on its page. "Gateway" stays out of the words.
    pub(super) fn section_title(self) -> &'static str {
        match self {
            S::Appearance => "Appearance",
            S::Notifications => "Notifications",
            S::Shortcuts => "Shortcuts",
            S::Page(PageId::Gateways) => "Connection",
            S::Page(PageId::Worktrees) => "Projects",
            S::Page(PageId::Mcp) => "MCP servers",
            S::Page(PageId::Policy) => "Policy",
            S::Page(PageId::Providers) => "Provider keys",
            S::Page(PageId::Servers) => "Servers",
            S::Page(PageId::DesktopApp) => "App",
            S::Page(PageId::LocalSessions) => "History",
            S::Page(PageId::Subscriptions) => "Connected plans",
            S::Page(PageId::Connectors) => "Your accounts",
            S::Page(PageId::ConnectorRules) => "Integrations",
            S::Page(PageId::Agents) => "Coding agents",
            other => other.label(),
        }
    }

    /// Where it is kept, when that differs from its page (the Nexus app's field scopes).
    pub(super) fn scope(self) -> Scope {
        match self {
            S::Page(PageId::Servers) | S::Page(PageId::Cloud) => Scope::You,
            S::Page(PageId::Subscriptions) | S::Page(PageId::Mcp) => Scope::Device,
            other => other.dest().scope(),
        }
    }
}

/// The section an address opens: `/settings/computer#servers`, an old page (`/settings/mcp`) or a section's old
/// address (`/settings/appearance`). None for an address that is not a section drawn here.
pub(super) fn resolve(path: &str) -> Option<SettingsSection> {
    if subsessions_anchor(path) {
        return Some(S::Page(PageId::Subsessions));
    }
    let (page, anchor) = match path.split_once('#') {
        Some((page, anchor)) => (page, Some(anchor)),
        None => (path, None),
    };
    let page = format!("/{}", page.trim_matches('/'));
    // An old page drawn here as a section keeps its own section.
    if let Some(id) = PageId::from_web_path(&page).filter(|id| !id.is_place()) {
        // Plan usage is part of Agents and plans now.
        let id = if id == PageId::Subscriptions {
            PageId::Agents
        } else {
            id
        };
        let section = S::Page(id);
        if anchor.is_none() || section.dest().sections().len() < 2 {
            return Some(section);
        }
    }
    let legacy = match page.as_str() {
        "/settings/appearance" => Some(S::Appearance),
        "/settings/notifications" => Some(S::Notifications),
        "/settings/shortcuts" => Some(S::Shortcuts),
        "/settings/account" => Some(S::Account),
        _ => None,
    };
    let dest = Dest::ALL.into_iter().find(|dest| {
        page == format!("/settings/{}", dest.slug())
            || page == format!("/{}", dest.web_path())
            || dest.also().contains(&page.as_str())
    });
    let dest = match (legacy, dest) {
        (Some(section), _) => return Some(section),
        (None, Some(dest)) => dest,
        (None, None) => return None,
    };
    let sections = dest.sections();
    let by_anchor = anchor.and_then(|anchor| {
        sections
            .iter()
            .copied()
            .find(|section| section.anchor() == anchor)
            .or_else(|| {
                dest.fields(&Visibility::all())
                    .into_iter()
                    .find(|field| field.anchor == anchor)
                    .and_then(|field| field.section)
            })
    });
    // A place (Usage) opens in Settings from its row, but its address stays the place's own.
    by_anchor
        .or_else(|| sections.first().copied())
        .filter(|section| dest == Dest::Budgets || !matches!(section, S::Page(id) if id.is_place()))
}

/// The Settings links that normally open elsewhere have an inert page in the local workspace.
pub(super) fn resolve_local_gateway_link(path: &str) -> Option<SettingsSection> {
    let page = path.split(['?', '#']).next()?.trim_matches('/');
    match page {
        "settings/slack" | "settings/connectors/slack" => Some(S::GatewayLink(Dest::Slack)),
        "settings/assistant" => Some(S::GatewayLink(Dest::Assistant)),
        _ => None,
    }
}

/// A part of this build's nav: "You" is about the person and this computer; the
/// second part is the organization's, labelled with its name (or "Personal").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Part {
    You,
    Code,
    Org,
}

/// The rows of each part, in order. Some shared destinations sit under another row (`Dest::row`).
const YOU_ROWS: [Dest; 11] = [
    Dest::Account,
    Dest::Assistant,
    Dest::Preferences,
    Dest::Computer,
    Dest::LocalProviders,
    Dest::Privacy,
    Dest::Subscriptions,
    Dest::ApiKeys,
    Dest::Devices,
    Dest::Integrations,
    Dest::Referrals,
];
const ORG_ROWS: [Dest; 10] = [
    Dest::General,
    Dest::Members,
    Dest::Policy,
    Dest::Usage,
    Dest::Billing,
    Dest::Repositories,
    Dest::Audit,
    Dest::Skills,
    Dest::ConnectorRules,
    Dest::Slack,
];

impl Dest {
    /// Its name in this build's nav, page titles and search, where it differs from the shared label (which the
    /// parity tests keep equal to the Nexus app's).
    pub fn title(self) -> &'static str {
        match self {
            Dest::Computer => "This computer",
            Dest::Subscriptions => "Agents and plans",
            Dest::ApiKeys => "API keys",
            Dest::Members => "People",
            Dest::Policy => "Model access",
            Dest::Audit => "Audit log",
            // The person's own accounts; the organization's rules are Integrations.
            Dest::Integrations => "Connections",
            other => other.label(),
        }
    }

    /// The nav row that holds it: People holds Teams, Model access holds Providers, Usage holds Budgets (its
    /// Limits tab), and General holds Data (sharing and retention) and Design (its Branding row).
    pub fn row(self) -> Dest {
        match self {
            Dest::Teams => Dest::Members,
            Dest::Providers => Dest::Policy,
            Dest::Budgets => Dest::Usage,
            Dest::Data | Dest::Design => Dest::General,
            other => other,
        }
    }

    /// The destinations whose sections this row holds, in order. Their sections are its sub-rows in the nav.
    pub fn tabs(self, visibility: &Visibility) -> Vec<Dest> {
        let tabs: &[Dest] = match self {
            Dest::Members => &[Dest::Members, Dest::Teams],
            Dest::Policy => &[Dest::Policy, Dest::Providers],
            Dest::General => &[Dest::General, Dest::Design],
            _ => return vec![self],
        };
        tabs.iter()
            .copied()
            .filter(|dest| *dest == self || dest.visible(visibility))
            .collect()
    }

    /// The sections of this row's destinations, in order. With more than one, each is a sub-row under the row in
    /// the nav (Preferences: Appearance, Notifications, Shortcuts).
    pub fn nav_sections(self, visibility: &Visibility) -> Vec<SettingsSection> {
        self.tabs(visibility)
            .into_iter()
            .flat_map(|dest| dest.sections().iter().copied())
            .collect()
    }
}

/// A place the nav's keyboard cursor can stand: a row, or a sub-row of the open row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NavItem {
    Row(Dest),
    Section(SettingsSection),
}

/// The nav's rows in order, with the sub-rows of the row that holds `open` under it.
pub(super) fn nav_items(visibility: &Visibility, open: Dest) -> Vec<NavItem> {
    let mut items = Vec::new();
    for dest in layout(visibility).into_iter().flat_map(|(_, rows)| rows) {
        items.push(NavItem::Row(dest));
        let sections = dest.nav_sections(visibility);
        if open.row() == dest && sections.len() > 1 {
            items.extend(sections.into_iter().map(NavItem::Section));
        }
    }
    items
}

/// This build's nav: its two parts and the rows this account sees in each.
pub(super) fn layout(visibility: &Visibility) -> Vec<(Part, Vec<Dest>)> {
    let shown = |rows: &[Dest]| -> Vec<Dest> {
        rows.iter()
            .copied()
            .filter(|dest| *dest == Dest::Account || dest.visible(visibility))
            .collect()
    };
    [
        (Part::You, shown(&YOU_ROWS)),
        (Part::Code, shown(&[Dest::Subsessions])),
        (Part::Org, shown(&ORG_ROWS)),
    ]
    .into_iter()
    .filter(|(_, rows)| !rows.is_empty())
    .collect()
}

/// The rows of the nav this account sees, group by group; empty groups drop out.
pub(super) fn nav(visibility: &Visibility) -> Vec<(Group, Vec<Dest>)> {
    Group::ALL
        .into_iter()
        .map(|group| {
            let rows: Vec<Dest> = Dest::ALL
                .into_iter()
                .filter(|dest| {
                    dest.group() == group
                        && (dest.visible(visibility) || (visibility.signed_out && *dest == Dest::Account))
                })
                .collect();
            (group, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows this build draws that the Nexus app's nav does not list as its own entries.
    fn only_here(dest: Dest) -> bool {
        matches!(
            dest,
            Dest::Account
                | Dest::Assistant
                | Dest::Slack
                | Dest::ConnectorRules
                | Dest::LocalProviders
                | Dest::Developer
                | Dest::Subsessions
        )
    }

    /// The Nexus web app's settings registry, as written: a copy of its `settings-navigation.ts`
    /// (`fixtures/settings-navigation.ts`). Refresh it when the web's registry changes.
    fn registry_source() -> String {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/settings-navigation.ts");
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {path}: {error}"))
    }

    /// Every quoted string right after `marker` in `source`, in order.
    fn quoted_after(source: &str, marker: &str) -> Vec<String> {
        source
            .match_indices(marker)
            .filter_map(|(at, _)| {
                let rest = source[at + marker.len()..].strip_prefix('"')?;
                rest.split_once('"').map(|(value, _)| value.to_string())
            })
            .collect()
    }

    /// The `settingsNav` function's body, without the search code after it.
    fn nav_source() -> String {
        let source = registry_source();
        let start = source.find("export function settingsNav").expect("settingsNav");
        let end = source[start..]
            .find("export function settingsKicker")
            .expect("settingsKicker")
            + start;
        source[start..end].to_string()
    }

    #[test]
    fn destinations_match_the_nexus_apps_registry_in_order() {
        let source = nav_source();
        // `entry("slug", "Label", …`: each destination, in the nav's order.
        let theirs: Vec<(String, String)> = source
            .match_indices("entry(\"")
            .map(|(at, _)| {
                let rest = &source[at + 7..];
                let (slug, after) = rest.split_once("\", \"").expect("a label after the slug");
                (slug.to_string(), after.split_once('"').expect("label").0.to_string())
            })
            // The web's `connectors` row is Nexus UI's browser twin of `integrations`, not shown on desktop. Its
            // `overview` row is the Gateway home the web sidebar dropped; desktop keeps Overview
            // as a place of its own (`pages/`). Its `assistant` row is Nexus UI's Personality page, shown
            // only with `NEXUS_UI_ASSISTANT_SETTINGS`; this build's Assistant row opens the Assistant's own views.
            // Developer apps belongs to Nexus UI, which discovers support through its developer-clients query.
            .filter(|(slug, _)| !matches!(slug.as_str(), "connectors" | "overview" | "assistant" | "developer"))
            .collect();
        // The two Assistant rows open different pages, so their places and fields differ by design.
        let ours: Vec<(String, String)> = Dest::ALL
            .into_iter()
            // Slack is a nav row here, and a row on the Nexus app's Integrations page there. The
            // organization's connector rules are a row here and part of the Nexus app's connectors page.
            .filter(|dest| !only_here(*dest))
            .map(|dest| (dest.slug().to_string(), dest.label().to_string()))
            .collect();
        assert_eq!(ours, theirs);
    }

    #[test]
    fn groups_match_the_nexus_apps_registry() {
        let source = nav_source();
        let ids = quoted_after(&source, "id: ");
        let ours: Vec<String> = Group::ALL
            .iter()
            .filter(|group| **group != Group::Code)
            .map(|group| format!("{group:?}").to_lowercase())
            .collect();
        assert_eq!(ours, ids);
        for group in Group::ALL
            .into_iter()
            .filter(|group| !matches!(group, Group::Global | Group::Code))
        {
            assert!(
                source.contains(&format!("label: \"{}\", part: \"account\"", group.label())),
                "{group:?}"
            );
        }
        // Each destination sits in the group that holds it there.
        for dest in Dest::ALL.into_iter().filter(|dest| !only_here(*dest)) {
            let at = source.find(&format!("entry(\"{}\"", dest.slug())).expect("entry");
            let group_at = source[..at].rfind("id: \"").expect("group");
            let id = source[group_at + 5..].split('"').next().unwrap_or_default();
            assert_eq!(id, format!("{:?}", dest.group()).to_lowercase(), "{dest:?}");
        }
    }

    /// Fields managed by this app.
    const ONLY_HERE: &[&str] = &["transparency", "language", "shared-memory"];

    #[test]
    fn fields_aliases_and_keywords_match_the_nexus_apps_registry() {
        let source = nav_source();
        // This build's Assistant fields are its own views; the web row's are its Personality page's.
        for dest in Dest::ALL.into_iter().filter(|dest| *dest != Dest::Assistant) {
            let Some(start) = source.find(&format!("entry(\"{}\"", dest.slug())) else {
                continue;
            };
            let end = Dest::ALL
                .iter()
                .filter_map(|other| source.find(&format!("entry(\"{}\"", other.slug())))
                .filter(|at| *at > start)
                .min()
                .unwrap_or(source.len());
            let body = &source[start..end];
            let fields: Vec<Field> = dest
                .fields(&Visibility::all())
                .into_iter()
                .filter(|field| !ONLY_HERE.contains(&field.anchor))
                .collect();
            let ours: Vec<String> = fields.iter().map(|field| field.anchor.to_string()).collect();
            assert_eq!(ours, quoted_after(body, "anchor: "), "{dest:?}'s fields");
            for field in fields {
                assert!(
                    body.contains(&format!("label: \"{}\"", field.label)),
                    "{dest:?}'s {} label",
                    field.anchor
                );
                for keyword in field.keywords {
                    assert!(
                        body.contains(&format!("\"{keyword}\"")),
                        "{dest:?} {} {keyword}",
                        field.anchor
                    );
                }
            }
            for also in dest.also() {
                assert!(body.contains(&format!("\"{also}\"")), "{dest:?} also {also}");
            }
            for keyword in dest.keywords() {
                assert!(body.contains(&format!("\"{keyword}\"")), "{dest:?} keyword {keyword}");
            }
        }
    }

    #[test]
    fn visibility_follows_the_account_as_the_nexus_app_does() {
        let rows =
            |visibility: &Visibility| -> Vec<Dest> { nav(visibility).into_iter().flat_map(|(_, rows)| rows).collect() };
        let all = rows(&Visibility::all());
        assert!(all.contains(&Dest::Members) && all.contains(&Dest::Billing) && all.contains(&Dest::Slack));
        assert!(!all.contains(&Dest::Account));

        // A personal account has no People. A member without the permissions sees no Providers, Slack, Referrals or
        // Billing.
        let personal = Visibility {
            personal: true,
            ..Visibility::default()
        };
        let groups: Vec<Group> = nav(&personal).into_iter().map(|(group, _)| group).collect();
        assert!(!groups.contains(&Group::People));
        let member = rows(&Visibility::default());
        for hidden in [
            Dest::Providers,
            Dest::Slack,
            Dest::Referrals,
            Dest::Billing,
            Dest::Usage,
            Dest::ConnectorRules,
        ] {
            assert!(!member.contains(&hidden), "{hidden:?}");
        }
        // Everyone keeps their own Connections; only owners and admins get the organization's Integrations, in the
        // organization's part of this build's nav.
        assert!(member.contains(&Dest::Integrations));
        assert!(all.contains(&Dest::ConnectorRules));
        let parts = layout(&Visibility::all());
        let part_of = |dest: Dest| {
            parts
                .iter()
                .find(|(_, rows)| rows.contains(&dest))
                .map(|(part, _)| *part)
        };
        assert_eq!(part_of(Dest::Integrations), Some(Part::You));
        assert_eq!(part_of(Dest::ConnectorRules), Some(Part::Org));
        assert_eq!(Dest::Integrations.title(), "Connections");
        assert_eq!(Dest::ConnectorRules.title(), "Integrations");
        assert!(
            Dest::Preferences
                .fields(&personal)
                .iter()
                .all(|field| field.anchor != "show-online")
        );

        // A locked Gateway offers no "Add a Gateway".
        let managed = Visibility {
            managed_gateway: true,
            ..Visibility::all()
        };
        assert!(
            Dest::Computer
                .fields(&managed)
                .iter()
                .all(|field| field.anchor != "add-gateway")
        );
    }

    #[test]
    fn gateway_permissions_drive_visibility_instead_of_role_names() {
        for role in ["owner", "admin", "member"] {
            let mut me = serde_json::json!({"tenant":{"id":"test", "kind":"company"}, "role":role});
            let account = crate::gateway::Account::from_json(&me).unwrap();
            assert!(!Visibility::from_account(Some(&account), false).billing);
            assert!(!Visibility::from_account(Some(&account), false).usage);
            me["permissions"] = serde_json::json!([
                "org.billing",
                "spend.read_all",
                "provider_keys.manage",
                "referrals.read",
                "chat_apps.manage"
            ]);
            let account = crate::gateway::Account::from_json(&me).unwrap();
            let visibility = Visibility::from_account(Some(&account), false);
            assert!(
                visibility.billing
                    && visibility.usage
                    && visibility.providers
                    && visibility.referrals
                    && visibility.slack
            );
            assert_eq!(account.role, role);
        }
    }

    #[test]
    fn every_native_settings_page_keeps_a_destination() {
        for id in PageId::ALL.into_iter().filter(|id| !id.is_place()) {
            let section = S::Page(id);
            assert!(
                section.dest().sections().contains(&section)
                    || matches!(id, PageId::ApiExplorer | PageId::Subscriptions),
                "{id:?} lost its destination"
            );
        }
        for section in [S::Appearance, S::Notifications, S::Shortcuts, S::ApiKeys, S::Account] {
            assert!(section.dest().sections().contains(&section), "{section:?}");
        }
    }

    #[test]
    fn subsessions_anchors_open_the_same_field_at_launch_and_when_restored() {
        let cx = gpui_kit::TestAppContext::single();
        cx.update(|_| {
            let section = S::Page(PageId::Subsessions);
            for path in [
                "subsessions",
                "session-starts",
                "crew-spend-cap",
                "settings/subsessions#subsessions",
                "/settings#session-starts",
                "/settings/computer#session-starts",
                "/settings/worktrees#crew-spend-cap",
            ] {
                assert_eq!(resolve(path), Some(section), "{path}");
                assert_eq!(resolve_field(path), Some((section, "subsessions")), "{path}");
                assert!(
                    matches!(
                        super::super::last_place::place_of(path, None),
                        Some(super::super::Place::SettingsField(target, "subsessions")) if target == section
                    ),
                    "{path}"
                );
            }
            for term in ["subsession", "orchestration", "delegation", "subagent"] {
                assert!(
                    super::super::settings_search::search(term, &Visibility::all())
                        .iter()
                        .any(|hit| hit.dest == Dest::Subsessions),
                    "{term}"
                );
            }
        });
    }

    #[test]
    fn budgets_aliases_target_limits_without_changing_canonical_usage() {
        let usage = S::Page(PageId::Usage);
        assert_eq!(usage.dest(), Dest::Usage);
        assert_eq!(Dest::Usage.target(), None);
        assert_eq!(Dest::Budgets.first(), Some(usage));
        for path in [
            "/budgets",
            "/settings/budgets",
            "/settings/budgets#usage-limit",
            "/budgets#usage-limits",
        ] {
            assert_eq!(resolve(path), Some(usage), "{path}");
            assert_eq!(resolve_field(path), Some((usage, "usage-limits")), "{path}");
        }
        assert_eq!(resolve("/usage"), None);
        assert_eq!(resolve_field("/usage"), None);
    }

    #[test]
    fn old_addresses_open_their_section() {
        for (path, section) in [
            ("/settings", S::Appearance),
            ("/settings/preferences", S::Appearance),
            ("/settings/preferences#shortcuts", S::Shortcuts),
            ("/settings/appearance", S::Appearance),
            ("/settings/notifications", S::Notifications),
            ("/settings/computer", S::Page(PageId::Gateways)),
            ("/settings/privacy", S::Page(PageId::Privacy)),
            ("/settings/privacy#local-session-history", S::Page(PageId::Privacy)),
            ("/settings/privacy#outside-access", S::Page(PageId::Privacy)),
            ("/settings/computer#servers", S::Page(PageId::Servers)),
            ("/settings/computer#branch-prefix", S::Page(PageId::Worktrees)),
            ("/settings/this-computer", S::Page(PageId::DesktopApp)),
            ("/settings/local-sessions", S::Page(PageId::LocalSessions)),
            ("/settings/shared-memory", S::Page(PageId::SharedMemory)),
            ("/settings/computer#shared-memory", S::Page(PageId::SharedMemory)),
            ("/settings/plan-usage", S::Page(PageId::Agents)),
            ("/settings/agents", S::Page(PageId::Agents)),
            ("/settings/subscriptions#agent-codex", S::Page(PageId::Agents)),
            ("/settings/mcp", S::Page(PageId::Mcp)),
            ("/settings/integrations", S::Page(PageId::Connectors)),
            ("/settings/connectors", S::Page(PageId::Connectors)),
            ("/settings/integrations#mcp-servers", S::Page(PageId::Mcp)),
            ("/settings/skills", S::Page(PageId::Skills)),
            ("/skills", S::Page(PageId::Skills)),
            ("/settings/integrations#connectors", S::Page(PageId::Connectors)),
            ("/settings/api-keys", S::ApiKeys),
            ("/keys", S::ApiKeys),
            ("/settings/model-policy#web-search", S::Page(PageId::Policy)),
            ("/members", S::Page(PageId::Members)),
            ("/settings/api-explorer", S::Page(PageId::ApiExplorer)),
            ("/settings/connector-rules", S::Page(PageId::ConnectorRules)),
            ("/settings/connector-rules#who-can-use", S::Page(PageId::ConnectorRules)),
        ] {
            assert_eq!(resolve(path), Some(section), "{path}");
        }
        // Each connectors page links to the other by an address that opens it.
        assert_eq!(
            resolve(crate::pages::connectors_addresses().0),
            Some(S::Page(PageId::Connectors))
        );
        assert_eq!(
            resolve(crate::pages::connectors_addresses().1),
            Some(S::Page(PageId::ConnectorRules))
        );
        assert!(PageId::Skills.is_native());
        assert_eq!(S::Page(PageId::Skills).dest(), Dest::Skills);
        assert_eq!(resolve("/usage"), None);
        assert!(
            Dest::Data
                .fields(&Visibility::all())
                .iter()
                .all(|field| field.section == Some(S::Page(PageId::General)))
        );
        for dest in Dest::ALL {
            for field in dest.fields(&Visibility::all()) {
                if let Some(section) = field.section {
                    assert_eq!(resolve(&format!("{}#{}", dest.web_path(), field.anchor)), Some(section));
                }
            }
        }
    }

    #[test]
    fn data_addresses_open_general_without_changing_its_canonical_destination() {
        let general = S::Page(PageId::General);
        assert_eq!(Dest::Data.first(), Some(general));
        assert_eq!(general.dest(), Dest::General);
        for path in [
            "/settings/data",
            "/settings/data#sharing-settings",
            "/settings/data#retention-days",
        ] {
            assert_eq!(resolve(path), Some(general), "{path}");
        }
        assert_eq!(Dest::Data.row(), Dest::General);
        assert_eq!(Dest::Budgets.sections(), &[S::Page(PageId::Usage)]);
    }

    #[test]
    fn the_nav_layout_rows_and_tabs() {
        let all = Visibility::all();
        let plain = Visibility::default();
        for visibility in [&all, &plain] {
            let parts = layout(visibility);
            assert_eq!(parts[0].0, Part::You);
            assert_eq!(parts[0].1[0], Dest::Account, "Account is first");
            assert_eq!(parts[1], (Part::Code, vec![Dest::Subsessions]));
            assert_eq!(parts[2].0, Part::Org);
        }
        let parts = layout(&all);
        assert_eq!(
            parts[0].1,
            YOU_ROWS
                .iter()
                .copied()
                .filter(|d| *d == Dest::Account || d.visible(&all))
                .collect::<Vec<_>>()
        );
        assert_eq!(parts[0].1.len(), 10);
        assert_eq!(parts[2].1.first(), Some(&Dest::General));
        assert!(parts[2].1.contains(&Dest::Members) && parts[2].1.contains(&Dest::Policy));
        // Hidden-by-permission rows drop out of the default layout, Account stays.
        let hidden = layout(&plain);
        assert!(hidden[0].1.contains(&Dest::Account));
        assert!(!hidden[0].1.contains(&Dest::Referrals));
        // Tabs live under their rows, never as rows.
        for (_, rows) in &parts {
            assert!(!rows.contains(&Dest::Teams) && !rows.contains(&Dest::Providers));
            assert!(!rows.contains(&Dest::Budgets) && !rows.contains(&Dest::Data) && !rows.contains(&Dest::Design));
        }
        assert_eq!(Dest::Teams.row(), Dest::Members);
        assert_eq!(Dest::Providers.row(), Dest::Policy);
        assert_eq!(Dest::Budgets.row(), Dest::Usage);
        assert_eq!(Dest::Data.row(), Dest::General);
        assert_eq!(Dest::Design.row(), Dest::General);
        assert_eq!(Dest::Skills.row(), Dest::Skills);
        assert_eq!(Dest::Members.tabs(&all), vec![Dest::Members, Dest::Teams]);
        assert_eq!(Dest::Policy.tabs(&all), vec![Dest::Policy, Dest::Providers]);
        assert_eq!(Dest::Usage.tabs(&all), vec![Dest::Usage]);
    }

    /// A4: Account › Assistant shows only with an agent API, under Account. It has no section: the row opens the
    /// Assistant's own What Nexus knows (`open_dest`, `assistant_views_tests.rs`), and its web address is the fallback.
    #[test]
    fn assistant_shows_with_an_agent_api_and_opens_the_assistants_own_views() {
        assert_eq!(Dest::Assistant.group(), Group::Account);
        assert_eq!(Dest::Assistant.scope(), Scope::You);
        assert!(Dest::Assistant.sections().is_empty(), "no Settings section");
        assert_eq!(Dest::Assistant.web_path(), "agent?view=memory");
        let anchors: Vec<_> = Dest::Assistant
            .fields(&Visibility::all())
            .into_iter()
            .map(|field| (field.anchor, field.section, assistant_view(Some(field.anchor))))
            .collect();
        assert_eq!(
            anchors,
            [
                ("vault", None, PageId::AgentVault),
                ("privacy", None, PageId::AgentPrivacy)
            ],
            "Passwords and payments, and Privacy, open the Assistant's own views"
        );
        let without = Visibility::from_account(None, false);
        assert!(!Dest::Assistant.visible(&without));
        assert!(!layout(&without)[0].1.contains(&Dest::Assistant));
        let with = without.with_assistant(true);
        assert!(Dest::Assistant.visible(&with));
        assert_eq!(layout(&with)[0].1[..2], [Dest::Account, Dest::Assistant]);
        assert!(
            nav(&with)
                .iter()
                .any(|(group, rows)| *group == Group::Account && rows.contains(&Dest::Assistant))
        );
        // No section is drawn here, so its address is not a native one.
        assert_eq!(resolve("/settings/assistant"), None);
    }

    /// A22·d: the Assistant's contact card opens Accounts natively, and its Settings where Account › Assistant goes
    /// (`pages/agent/contact_card.rs`).
    #[test]
    fn the_assistant_card_opens_the_homes_settings_has() {
        let accounts = crate::pages::agent::CARD_ACCOUNTS;
        assert!(
            resolve_field(accounts).is_some() || resolve(accounts).is_some(),
            "{accounts}"
        );
        assert_eq!(crate::pages::agent::CARD_SETTINGS, Dest::Assistant.web_path());
    }

    #[test]
    fn design_systems_opens_natively_under_the_existing_general_row() {
        assert_eq!(
            resolve("/settings/design-systems"),
            Some(S::Page(PageId::DesignSystems))
        );
        assert!(PageId::DesignSystems.is_native());
        assert_eq!(S::Page(PageId::DesignSystems).dest(), Dest::Design);
        assert_eq!(Dest::Design.row(), Dest::General);
        assert_eq!(
            Dest::General.nav_sections(&Visibility::all()),
            vec![S::Page(PageId::General), S::Page(PageId::DesignSystems)]
        );
    }

    #[test]
    fn the_open_row_shows_its_sections_as_sub_rows() {
        let all = Visibility::all();
        assert_eq!(
            Dest::Preferences.nav_sections(&all),
            vec![S::Appearance, S::Notifications, S::Shortcuts]
        );
        assert_eq!(
            Dest::Members.nav_sections(&all),
            vec![S::Page(PageId::Members), S::Page(PageId::Teams)]
        );
        assert_eq!(Dest::Privacy.nav_sections(&all), vec![S::Page(PageId::Privacy)]);
        // Only the open row unfolds; a row with one section never does.
        let items = nav_items(&all, Dest::Teams);
        let people = items
            .iter()
            .position(|item| *item == NavItem::Row(Dest::Members))
            .unwrap();
        assert_eq!(items[people + 1], NavItem::Section(S::Page(PageId::Members)));
        assert_eq!(items[people + 2], NavItem::Section(S::Page(PageId::Teams)));
        assert!(!items.contains(&NavItem::Section(S::Appearance)));
        let privacy = nav_items(&all, Dest::Privacy);
        assert!(privacy.iter().all(|item| matches!(item, NavItem::Row(_))));
    }

    #[test]
    fn only_table_sections_are_wider_than_a_form() {
        let tables = [PageId::Members, PageId::Teams, PageId::Audit, PageId::Repositories];
        let all = Visibility::all();
        for dest in Dest::ALL {
            for section in dest.nav_sections(&all) {
                let table = matches!(section, S::Page(id) if tables.contains(&id));
                let want = if table {
                    SettingsWidth::Table
                } else {
                    SettingsWidth::Form
                };
                assert_eq!(section.width(), want, "{section:?} under {dest:?}");
            }
        }
        assert!(SettingsWidth::Table.points() > SettingsWidth::Form.points());
    }

    #[test]
    fn model_access_and_this_computer_keep_one_width() {
        let all = Visibility::all();
        for row in [Dest::Policy, Dest::Computer, Dest::Preferences, Dest::Integrations] {
            let widths: Vec<_> = row
                .nav_sections(&all)
                .into_iter()
                .map(|section| section.width())
                .collect();
            assert!(
                widths.iter().all(|width| *width == SettingsWidth::Form),
                "{row:?} mixes widths: {widths:?}"
            );
        }
    }
}

/// These anchors moved from Projects & worktrees to the Subsessions page.
fn subsessions_anchor(path: &str) -> bool {
    let path = path.trim_matches('/');
    let anchor = match path.split_once('#') {
        Some(("settings" | "settings/computer" | "settings/worktrees" | "settings/subsessions", anchor)) => anchor,
        Some(_) => return false,
        None => path,
    };
    matches!(anchor, "subsessions" | "session-starts" | "crew-spend-cap")
}

/// Resolve a known field independently of the caller's permissions. Visibility controls discovery, not routing.
pub(super) fn resolve_field(path: &str) -> Option<(SettingsSection, &'static str)> {
    if subsessions_anchor(path) {
        return Some((S::Page(PageId::Subsessions), "subsessions"));
    }
    let (page, anchor) = path.split_once('#').unwrap_or((path, ""));
    let dest = Dest::ALL.into_iter().find(|dest| {
        page.trim_matches('/') == format!("settings/{}", dest.slug())
            || dest
                .also()
                .iter()
                .any(|alias| alias.trim_matches('/') == page.trim_matches('/'))
    })?;
    if dest == Dest::Budgets && matches!(anchor, "" | "usage-limits" | "usage-limit") {
        return Some((S::Page(PageId::Usage), "usage-limits"));
    }
    let anchor = if anchor.is_empty() { dest.target()? } else { anchor };
    dest.fields(&Visibility::all())
        .into_iter()
        .find(|field| field.anchor == anchor)
        .and_then(|field| field.section.map(|section| (section, field.anchor)))
}

#[cfg(test)]
mod local_provider_tests {
    use super::*;

    #[test]
    fn local_provider_route_and_search_are_independent_of_organization_providers() {
        assert_eq!(resolve("/settings/local-providers"), Some(S::LocalProviders));
        assert_eq!(resolve("/settings/providers"), Some(S::Page(PageId::Providers)));
        assert!(!Dest::LocalProviders.visible(&Visibility::default()));

        let local = Visibility::signed_out(false);
        assert!(Dest::LocalProviders.visible(&local));
        assert!(Dest::Providers.visible(&local));
        for query in ["BYOK", "bring your own key", "model server"] {
            let hits = crate::workspace::settings_search::search(query, &local);
            assert!(hits.iter().any(|hit| hit.section == Some(S::LocalProviders)), "{query}");
        }

        let member = Visibility {
            local_providers: true,
            ..Visibility::default()
        };
        assert!(Dest::LocalProviders.visible(&member));
        assert!(!Dest::Providers.visible(&member));
        assert!(Dest::Providers.visible(&Visibility::all()));
    }

    #[test]
    fn keyboard_nav_keeps_local_providers_after_computer_in_you() {
        let signed_in = Visibility {
            local_providers: true,
            ..Visibility::all()
        };
        let parts = layout(&signed_in);
        assert_eq!(parts[0].0, Part::You);
        assert!(parts[0].1.contains(&Dest::Integrations), "personal Connections");
        assert_eq!(parts[1], (Part::Code, vec![Dest::Subsessions]));
        assert_eq!(parts[2].0, Part::Org);
        assert!(parts[2].1.contains(&Dest::ConnectorRules), "organization Integrations");
        assert!(!parts[2].1.contains(&Dest::LocalProviders));

        let rows = nav_items(&signed_in, Dest::LocalProviders);
        assert!(rows.windows(3).any(|rows| rows
            == [
                NavItem::Row(Dest::Computer),
                NavItem::Row(Dest::LocalProviders),
                NavItem::Row(Dest::Privacy),
            ]));
    }
}
