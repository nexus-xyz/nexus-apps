/**
 * Settings navigation and search share this registry. Only entries available to this person enter the index.
 *
 * Brand v5's settings map: two headed parts. **Global** is the app itself, for every
 * account (Preferences, and Computer on desktop), with no group label. Under it, the current account (Personal or a
 * company) under its logo, name and your role, holding Account › People › Controls › Tools › Activity › Admin. Every
 * page is one row with a one-word label; nothing folds and nothing counts.
 *
 * Only pages that exist today: the spec's proposed pages (Accounts, Profile, Models, Security, Webhooks, Browser)
 * are left out. Assistant shows only while `NEXUS_UI_ASSISTANT_SETTINGS=on` (`assistant`). Overview
 * and Usage live under Activity (the web's sidebar had them, Frost puts them in Settings); spending caps live under
 * Controls.
 */
export type SettingsIcon =
  | "preferences" | "computer" | "privacy" | "subscriptions" | "api-keys" | "referrals" | "members" | "teams" | "model-policy" | "budgets" | "overview" | "usage"
  | "providers" | "integrations" | "skills" | "repositories" | "audit" | "general" | "billing" | "data" | "devices" | "design-systems"
  | "assistant" | "developer";

export type SettingsScope = "you" | "device" | "org";

export interface SettingsField {
  label: string;
  description: string;
  /** A stable control or section ID on the destination page. */
  anchor: string;
  keywords?: string[];
  scope?: SettingsScope;
}

export interface SettingsEntry {
  page: string;
  label: string;
  icon: SettingsIcon;
  description: string;
  scope: SettingsScope;
  keywords?: string[];
  fields?: SettingsField[];
  /** Other addresses that light this row (an old URL, a page's children). */
  also?: string[];
}

export type SettingsGroupId = "global" | "account" | "people" | "controls" | "tools" | "activity" | "admin";

export interface SettingsGroup {
  id: SettingsGroupId;
  /** The small, unclickable label over the rows; Global has none. */
  label: string;
  /** Global (the app, every account) or the current account's part. */
  part: "global" | "account";
  entries: SettingsEntry[];
}

export interface SettingsNavOptions {
  /** The desktop app: Computer, Subscriptions and its own Integrations page. */
  desktop: boolean;
  managedGateway?: boolean;
  /** Pages that need Nexus UI's server (the sign-in provider, the GitHub App): Members and Repositories. */
  serverPages: boolean;
  billing: boolean;
  usage?: boolean;
  referrals?: boolean;
  providers?: boolean;
  /** Slack: owners and admins, who hold `chat_apps.manage`. Its page is under Integrations. */
  slack?: boolean;
  /** A personal account: just you, so nothing that manages other people (Members, Teams). */
  personal?: boolean;
  /** Account › Assistant: Nexus UI's server has `NEXUS_UI_ASSISTANT_SETTINGS=on`. */
  assistant?: boolean;
  /** Account › Developer apps: the Gateway answers `/v1/developer/clients` (`NEXUS_OAUTH_CLIENT_TIERS=on`). */
  developer?: boolean;
}

const entry = (
  slug: string, label: string, icon: SettingsIcon, description: string, scope: SettingsScope,
  extra: Partial<Pick<SettingsEntry, "also" | "keywords" | "fields">> = {},
): SettingsEntry => ({ page: "/settings/" + slug, label, icon, description, scope, ...extra });

/** The Nexus Slack app is a connector: its row is on Integrations, and its page under it. */
export const SLACK_APP_PAGE = "/settings/connectors/slack";
const SLACK_APP_FIELD: SettingsField = {
  label: "Slack app", description: "Slack workspaces where Nexus answers as @nexus.", anchor: "slack-app", scope: "org",
  keywords: ["slack", "workspace", "install", "add to slack", "disconnect", "chat app", "bot"],
};

/** The settings sidebar, top to bottom: Global, then the account's groups. Empty groups drop out. */
export function settingsNav({ desktop, managedGateway = false, serverPages, billing, usage = false, referrals = false, providers = false, personal = false, slack = false, assistant = false, developer = false }: SettingsNavOptions): SettingsGroup[] {
  const groups: SettingsGroup[] = [
    {
      id: "global", label: "", part: "global", entries: [
        entry("preferences", "Preferences", "preferences", "How Nexus looks, alerts you and answers your keys on this device.", "device", {
          also: ["/settings"], keywords: ["appearance", "accessibility", "shortcuts", "notifications"],
          fields: [
            { label: "Theme", description: "Follow your system, or choose light or dark mode.", anchor: "theme", keywords: ["appearance", "color", "colour"] },
            ...(desktop ? [{ label: "Text size", description: "Scale text in the app, code, and terminal.", anchor: "text-size", keywords: ["font", "zoom", "larger", "smaller", "scale"] }] : []),
            { label: "Reduce motion", description: "Reduce animated effects in the app.", anchor: "reduce-motion", keywords: ["animations", "movement", "accessibility"] },
            { label: "Notifications", description: "Get an alert when a session needs your attention.", anchor: "notifications", keywords: ["alerts", "sound", "banner", "finished", "completion"] },
            { label: "Keyboard shortcuts", description: "Find keyboard commands for moving around Nexus.", anchor: "shortcuts", keywords: ["hotkeys", "keybindings", "keys"] },
            ...(personal ? [] : [{ label: "Show when I'm online", description: "Let teammates see when you have Nexus open, and what you are doing.", anchor: "show-online", keywords: ["presence", "online", "status", "team", "privacy", "green dot", "activity"] }]),
          ],
        }),
        ...(desktop ? [entry("computer", "Computer", "computer", "Gateways, worktrees, servers, the app and its history, on this computer only.", "device", {
          also: ["/settings/this-computer", "/settings/gateways", "/settings/worktrees", "/settings/servers", "/settings/cloud", "/settings/local-sessions"],
          keywords: ["this computer", "this mac", "machine", "desktop app"],
          fields: [
            { label: "Gateways", description: managedGateway ? "The gateway address set by your organization." : "Add or switch the gateway Nexus connects to.", anchor: "gateways", keywords: ["connection", "endpoint", "environment", "server url"] },
            { label: "Gateway address", description: "The endpoint for your current gateway.", anchor: "gateway-address", keywords: ["base url", "server address", "connection"] },
            ...(!managedGateway ? [{ label: "Add a Gateway", description: "Connect to another gateway address.", anchor: "add-gateway", keywords: ["endpoint", "connection"] }] : []),
            { label: "Worktree location", description: "Where new session checkouts are stored.", anchor: "worktrees", keywords: ["folder", "directory", "path", "worktree", "git", "checkouts"] },
            { label: "Worktree cleanup", description: "Remove session checkouts by rule, when nothing would be lost.", anchor: "worktree-cleanup", keywords: ["delete", "disk", "merged", "archived", "idle", "worktree"] },
            { label: "Branch names", description: "What new worktree branches start with.", anchor: "branch-prefix", keywords: ["prefix", "branch", "naming", "git"] },
            { label: "Hooks", description: "Run a command when a coding session finishes, needs you, merges or fails a check.", anchor: "hooks", keywords: ["hook", "script", "command", "automation", "event", "shell"] },
            { label: "Project setup scripts", description: "Commands to run before a coding session starts.", anchor: "setup-scripts", scope: "you" as const, keywords: ["install", "initialization", "setup"] },
            { label: "Servers", description: "Run coding sessions on your own Linux machines.", anchor: "servers", scope: "you" as const, keywords: ["ssh", "host", "remote"] },
            { label: "Cloud", description: "Run sessions on a cloud machine that keeps going when your laptop sleeps.", anchor: "cloud", scope: "you" as const, keywords: ["cloud machine", "sandbox", "remote", "sleep", "github"] },
            { label: "Global shortcut", description: "Open Nexus from any app.", anchor: "global-shortcut", keywords: ["hotkey", "option space", "keyboard"] },
            { label: "Screen recording shortcut", description: "Record your screen and voice for an agent.", anchor: "record-shortcut", keywords: ["capture", "recording"] },
            { label: "Open at login", description: "Start Nexus when you sign in to this computer.", anchor: "open-at-login", keywords: ["startup", "launch", "boot", "automatic"] },
            { label: "Menu bar only", description: "Keep Nexus in the menu bar without a Dock icon.", anchor: "menu-bar-only", keywords: ["tray", "dock", "icon"] },
            { label: "Follow the agent's edits", description: "Changes opens on each file the agent edits.", anchor: "follow-edits", keywords: ["changes", "diff", "auto open", "side pane", "follow"] },
            { label: "Archive merged sessions", description: "A session whose pull request merged goes to Archived, not Settled.", anchor: "archive-merged", keywords: ["pull request", "merged", "archive", "settled", "auto archive", "sidebar"] },
            { label: "Desktop updates", description: "Check your app version and available updates.", anchor: "desktop-updates", keywords: ["update", "upgrade", "version", "channel", "release"] },
            { label: "History", description: "Find conversations from coding agents on this computer.", anchor: "history", keywords: ["local sessions", "terminal", "resume", "local conversations"] },
          ],
        }), entry("privacy", "Privacy", "privacy", "Local conversation discovery, outside access and account privacy controls.", "device", {
          keywords: ["private", "isolation", "separate", "permissions"],
          fields: [
            { label: "Show conversations from other apps", description: "Choose whether Nexus reads local agent conversation titles and folders. Off by default.", anchor: "local-session-history", keywords: ["history", "discover", "scan", "local sessions", "isolation", "separate", "claude", "codex"] },
            { label: "Let other apps use Code sessions", description: "Review access for scripts, the nexus CLI and other agents.", anchor: "outside-access", keywords: ["outside access", "revoke", "consent", "permissions"] },
          ],
        })] : []),
      ],
    },
    {
      id: "account", label: "Account", part: "account", entries: [
        ...(assistant ? [entry("assistant", "Assistant", "assistant", "Your Nexus: how it sounds, what it knows and what it does without asking.", "you", {
          keywords: ["agent", "nexus", "personality", "tone", "voice", "your agent"],
          fields: [
            { label: "Personality", description: "Calm, Warm, Direct or Lively, and how much it says.", anchor: "personality", keywords: ["preset", "tone", "calm", "warm", "direct", "lively"] },
            { label: "Length", description: "How much Nexus says, from brief to detailed.", anchor: "length", keywords: ["brief", "detailed", "short", "long"] },
            { label: "Formality", description: "How formal Nexus is, from casual to formal.", anchor: "formality", keywords: ["casual", "formal", "polite"] },
            { label: "Humour", description: "How much humour Nexus uses.", anchor: "humour", keywords: ["humor", "jokes", "funny"] },
            { label: "Emoji", description: "Whether Nexus uses emoji.", anchor: "emoji", keywords: ["emojis", "icons"] },
            { label: "Shorter texts", description: "Shorter answers when you text Nexus, as on Telegram.", anchor: "short-texts", keywords: ["sms", "texts", "telegram"] },
          ],
        })] : []),
        ...(desktop ? [entry("subscriptions", "Subscriptions", "subscriptions", "Your Claude and ChatGPT plans, and how each coding agent reaches its models.", "you", {
          also: ["/settings/plan-usage", "/settings/agents"], keywords: ["plan usage", "subscription", "quota", "limits", "routing", "coding agents"],
          fields: [
            { label: "Plan usage", description: "Usage reported by your connected AI plans.", anchor: "plan-usage", keywords: ["quota", "limits", "claude max", "chatgpt"] },
            { label: "Codex routing", description: "Choose Gateway or your ChatGPT plan for Codex.", anchor: "agent-codex", keywords: ["openai", "route", "gateway"] },
            { label: "Claude Code routing", description: "Choose Gateway or your Claude plan for Claude Code.", anchor: "agent-claude", keywords: ["anthropic", "route", "gateway"] },
          ],
        })] : []),
        entry("api-keys", "Keys", "api-keys", "API keys that authenticate your tools and scripts against Gateway.", "you", { also: ["/keys"], keywords: ["api keys", "token", "credentials", "authentication", "secret"] }),
        ...(developer ? [entry("developer", "Developer apps", "developer", "Register a fork of a Nexus app and get its client ID.", "you", {
          keywords: ["oauth", "client id", "client_id", "fork", "app", "developer", "register", "scopes", "revoke"],
        })] : []),
        entry("devices", "Devices", "devices", "The devices that can read your synced sessions, and pairing a new one.", "you", { keywords: ["encryption", "end-to-end", "pairing", "trusted", "qr code", "revoke", "browser", "fingerprint"] }),
        ...(referrals ? [{ ...entry("referrals", "Referrals", "referrals", "Earn credit for your Personal account and track referral rewards.", "you", { keywords: ["invite", "friend"] }), page: "/referrals" }] : []),
      ],
    },
    {
      id: "people", label: "People", part: "account", entries: personal ? [] : [
        ...(serverPages ? [entry("members", "Members", "members", "People and their roles.", "org", { also: ["/members"], keywords: ["users", "invite", "permissions", "access"] })] : []),
        entry("teams", "Teams", "teams", "Team membership and organization units.", "org", { also: ["/teams"], keywords: ["groups", "departments", "members"] }),
      ],
    },
    {
      id: "controls", label: "Controls", part: "account", entries: [
        entry("budgets", "Budgets", "budgets", "How much this account and its people may spend. Every limit lives here.", "org", { also: ["/budgets"], keywords: ["limit", "cap", "spending", "default", "person"] }),
        entry("model-policy", "Policy", "model-policy", personal ? "Which models and tools you can use." : "Which models and tools teams can use.", "org", {
          keywords: ["model policy", "rules", "allowlist", "blocklist", "permissions", "access", "allowed", "denied"],
          fields: [
            { label: "Chat default model", description: "The default model for new chats.", anchor: "default-model-chat" },
            { label: "Claude Code default model", description: "The default model for Claude Code.", anchor: "default-model-claude" },
            { label: "Codex default model", description: "The default model for Codex.", anchor: "default-model-codex" },
            { label: "Web search", description: "Let people turn on web search in a chat.", anchor: "web-search", keywords: ["internet", "browse", "online", "sources"] },
            { label: "Model that searches", description: "Pick the model that runs every chat's web search.", anchor: "web-search-model-row", keywords: ["search model", "cheapest", "pin"] },
            { label: "Agents on personal plans", description: "Let people use their own Claude or ChatGPT plan.", anchor: "personal-agent-plans", keywords: ["subscriptions", "licenses", "allow personal"] },
          ],
        }),
        ...(providers ? [entry("providers", "Providers", "providers", "Provider API keys and organization model access.", "org", { also: ["/settings/provider-keys"], keywords: ["byok", "bring your own key", "anthropic", "openai", "credentials"] })] : []),
      ],
    },
    {
      id: "tools", label: "Tools", part: "account", entries: [
        desktop
          ? entry("integrations", "Integrations", "integrations", "Connectors your agents can use through Nexus, connected accounts, and additional MCP servers.", "you", {
              also: ["/settings/mcp", "/settings/connectors"], keywords: ["mcp", "model context protocol", "connectors", "connections", "tools"],
              fields: [
                { label: "Connectors", description: "Tools available through Nexus, their accounts, and who can use them.", anchor: "connectors", keywords: ["integrations", "connections", "connected accounts", "oauth", "agent writes", "linear", "github", "slack", "jira", "notion"] },
                { label: "MCP servers", description: "The built-in Nexus server and additional servers for your coding agents.", anchor: "mcp-servers", keywords: ["model context protocol", "tools"] },
              ],
            })
          : entry("connectors", "Integrations", "integrations", "Tools available through Nexus, their accounts, and who can use them.", "you", {
              also: ["/settings/integrations", "/settings/slack"], keywords: ["connectors", "connections", "connected accounts", "oauth", "linear", "github", "slack", "jira", "notion", "agent writes"],
              ...(slack ? { fields: [SLACK_APP_FIELD] } : {}),
            }),
        { ...entry("skills", "Skills", "skills", "Your skills, and the ones your organization shares.", "you", { keywords: ["skill md", "library", "slash", "promotions"] }), page: "/skills" },
        ...(serverPages ? [entry("repositories", "Repositories", "repositories", "Repositories connected through the Nexus GitHub App.", "org", { also: ["/repositories", "/settings/github"], keywords: ["repo", "github", "gitlab", "indexing"] })] : []),
      ],
    },
    {
      id: "activity", label: "Activity", part: "account", entries: [
        ...(usage ? [{ ...entry("overview", "Overview", "overview", "What the gateway is doing now: calls, errors, routes and agent sessions.", "org", { also: ["/connect"], keywords: ["gateway", "dashboard", "live routes", "setup", "agents"] }), page: "/overview" }, { ...entry("usage", "Usage", "usage", "What was spent, and on what. Limits are set in Budgets.", "org", { keywords: ["costs", "calls", "reports", "consumption"] }), page: "/usage" }] : []),
        entry("audit", "Audit", "audit", "Review changes to settings and access.", "org", { also: ["/audit"], keywords: ["audit log", "history", "activity", "events"] }),
      ],
    },
    {
      id: "admin", label: "Admin", part: "account", entries: [
        entry("general", "General", "general", personal ? "Your account's name, logo and address." : "Your organization's name, logo and address.", "org", {
          keywords: ["organization", "workspace", "company"],
          fields: [
            { label: "Organization logo", description: "The image shown in the account switcher and shared pages.", anchor: "org-logo", keywords: ["picture", "icon", "avatar", "branding"] },
            { label: "Organization name", description: "The name shown to organization members.", anchor: "org-name", keywords: ["rename", "company", "workspace"] },
            { label: "Organization address", description: "The address for your organization's pages.", anchor: "org-address", keywords: ["slug", "url"] },
            { label: "Approvals and requests for changes", description: "Let automated reviewers approve or block pull requests.", anchor: "review-events", keywords: ["code review", "nexus review", "approve", "request changes", "pull request"] },
            { label: "Synced coding sessions", description: "Choose what leaves a person's computer when coding sessions sync.", anchor: "synced-sessions", keywords: ["sync", "encryption", "recovery key", "content policy", "privacy", "devices"] },
          ],
        }),
        entry("design-systems", "Design", "design-systems", "The tokens and notes Nexus Design makes and checks designs with.", "org", {
          keywords: ["design", "tokens", "brand", "css variables", "tailwind", "default", "nexus design"],
        }),
        ...(billing ? [entry("billing", "Billing", "billing", "Credits, payment and receipts.", "org", { also: ["/billing"], keywords: ["invoice", "payment", "credits", "buy", "top up", "referral rewards"] })] : []),
        entry("data", "Data", "data", "How long chats are kept, and who can open what you share.", "org", {
          keywords: ["retention", "sharing", "privacy"],
          fields: [
            { label: "Sharing and pages", description: "Public links, email domains, expiry, and shared code sources.", anchor: "sharing-settings", keywords: ["external", "artifacts", "share", "expiration", "public pages", "public links", "readers", "continue chat"] },
            { label: "Chat retention", description: "How long chats are kept.", anchor: "retention-days", keywords: ["delete", "retention", "days", "privacy"] },
          ],
        }),
      ],
    },
  ];
  return groups.filter((group) => group.entries.length > 0);
}

/** The page header's kicker: "Global", "Personal" or the organization's name. */
export function settingsKicker(group: SettingsGroup | undefined, account: { personal: boolean; name?: string }): string | undefined {
  if (!group) return undefined;
  if (group.part === "global") return "Global";
  return account.personal ? "Personal" : (account.name ?? "Organization");
}

const under = (pathname: string, page: string) => pathname === page || pathname.startsWith(page + "/");

export function settingsCurrent(item: SettingsEntry, pathname: string): boolean {
  if (item.page === "/settings/preferences" && pathname === "/settings") return true;
  return under(pathname, item.page) || (item.also ?? []).some((path) => path !== "/settings" && under(pathname, path));
}

/** The group whose row lights for this path, if any. */
export function settingsGroupOf(groups: SettingsGroup[], pathname: string): SettingsGroup | undefined {
  return groups.find((group) => group.entries.some((item) => settingsCurrent(item, pathname)));
}

export function isSettingsPage(pathname: string): boolean {
  return under(pathname, "/settings") || ["/overview", "/connect", "/usage", "/members", "/repositories", "/keys", "/audit", "/billing", "/budgets", "/teams", "/referrals", "/skills"].some((page) => under(pathname, page));
}

export interface SettingsSearchResult {
  page: string;
  label: string;
  description: string;
  icon: SettingsIcon;
  scope: SettingsScope;
  breadcrumb: string[];
}

const normalize = (value: string) => value.normalize("NFKD").replace(/[\u0300-\u036f]/g, "").toLowerCase().replace(/[^a-z0-9]+/g, " ").trim();

/** Bound the edit distance. Short queries use exact text matches to avoid unrelated results. */
function near(a: string, b: string): boolean {
  if (a.length < 4 || a.length > 32) return false;
  const limit = a.length < 7 ? 1 : 2;
  if (Math.abs(a.length - b.length) > limit) return false;
  const row = Array.from({ length: b.length + 1 }, (_, i) => i);
  for (let i = 1; i <= a.length; i++) {
    let diagonal = row[0];
    row[0] = i;
    for (let j = 1; j <= b.length; j++) {
      const previous = row[j];
      row[j] = Math.min(row[j] + 1, row[j - 1] + 1, diagonal + Number(a[i - 1] !== b[j - 1]));
      diagonal = previous;
    }
    if (Math.min(...row) > limit) return false;
  }
  return row[b.length] <= limit;
}

/** Search page names, individual controls, descriptions, and aliases across the available groups. */
export function searchSettings(groups: SettingsGroup[], query: string): SettingsSearchResult[] {
  const normalized = normalize(query.slice(0, 200));
  if (!normalized) return [];
  const terms = normalized.split(" ");
  const matches: { result: SettingsSearchResult; score: number; fuzzy: boolean }[] = [];
  for (const group of groups) {
    for (const item of group.entries) {
      for (const field of [undefined, ...(item.fields ?? [])]) {
        const label = normalize(field?.label ?? item.label);
        const keywords = normalize((field?.keywords ?? item.keywords ?? []).join(" "));
        const description = normalize(field?.description ?? item.description);
        const context = normalize((group.label || "Global") + " " + item.label);
        const tokens = (label + " " + keywords + " " + description + " " + context).split(" ");
        let score = label === normalized ? 100 : 0;
        let fuzzy = false;
        const found = terms.every((term) => {
          if (label.includes(term)) { score += 40; return true; }
          if (keywords.includes(term)) { score += 25; return true; }
          if (description.includes(term) || context.includes(term)) { score += 10; return true; }
          if (tokens.some((token) => near(term, token))) { score += 1; fuzzy = true; return true; }
          return false;
        });
        if (found) matches.push({
          score, fuzzy,
          result: {
            page: item.page + (field ? "#" + field.anchor : ""),
            label: field?.label ?? item.label,
            description: field?.description ?? item.description,
            icon: item.icon,
            scope: field?.scope ?? item.scope,
            breadcrumb: [group.label || "Global", item.label, ...(field ? [field.label] : [])],
          },
        });
      }
    }
  }
  // Prefer direct matches. Use spelling tolerance when direct matching finds nothing.
  const direct = matches.filter((match) => !match.fuzzy);
  return (direct.length ? direct : matches).sort((a, b) => b.score - a.score).map((match) => match.result);
}
