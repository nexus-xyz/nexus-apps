/**
 * Settings' map, organized as the desktop's (GPUI `settings_registry.rs` `layout`, designer E's revamp): You (your
 * account, preferences and privacy), then the organization by its name (or Personal), then the app. Pages that belong
 * together sit under one row that opens a list of them: People holds Members and Teams, Model access holds Policy and
 * Providers, Usage holds Budgets, General holds Data. So the list stays short, and every page is still one tap from
 * its row. Search finds a page here by its words, with where it lives ("Acme › People"). No React Native, so vitest
 * runs it.
 */

/** The `Icon` names the map uses, so a row can pass its page's straight to `Row`. */
export type SettingsIcon =
  | "sliders"
  | "org"
  | "gift"
  | "person"
  | "bell"
  | "agents"
  | "keys"
  | "members"
  | "models"
  | "privacy"
  | "shield"
  | "gauge"
  | "gateway"
  | "connector"
  | "repositories"
  | "skill"
  | "spend"
  | "analytics"
  | "audit"
  | "inbox"
  | "billing"
  | "providers"
  | "pulse"
  | "info"
  | "data"
  | "archive"
  | "docs"
  | "feedback"
  | "comment"
  | "brain";

export type PartKey = "you" | "org" | "app";

export type SettingsPage = {
  label: string;
  icon: SettingsIcon;
  href: string;
  /** One line on what it is for, on the list of a row that holds several pages. */
  about?: string;
  /** Other words people search it by. */
  words?: string[];
  /** Manages other people: hidden in Personal, which has no one else (P48·F). */
  company?: boolean;
  /**
   * The permissions it needs, all of them (`GET /v1/tenants/me`): without them the row is not shown, as the desktop
   * hides it (GPUI `settings_registry.rs` `Visibility`). Pages a member can read stay, with their changes locked.
   */
  needs?: string[];
};

/** A row of Settings: a page, or several under one name (`hub`), which opens the list of them. */
export type SettingsRow = SettingsPage & { hub?: string; pages?: SettingsPage[] };

export type SettingsPart = { key: PartKey; rows: SettingsRow[] };

/** A row that holds `pages`. `about` says what they cover, under the row's name ("Members and teams"). */
const hub = (key: string, label: string, icon: SettingsIcon, about: string, pages: SettingsPage[], words: string[] = []): SettingsRow => ({
  label,
  icon,
  href: `/settings-group/${key}`,
  hub: key,
  about,
  pages,
  words,
});

export const SETTINGS: SettingsPart[] = [
  {
    key: "you",
    rows: [
      { label: "Organizations", icon: "org", href: "/accounts", words: ["switch", "accounts", "sign out", "personal", "team", "company"] },
      { label: "Profile", icon: "person", href: "/profile", words: ["name", "photo"] },
      hub("preferences", "Preferences", "sliders", "Appearance and notifications", [
        { label: "Appearance", icon: "sliders", href: "/appearance", about: "Theme, text size, motion and haptics.", words: ["theme", "dark", "light", "text size", "motion", "haptic", "vibration"] },
        { label: "Notifications", icon: "bell", href: "/notification-settings", about: "What reaches you, and where.", words: ["push", "email"] },
      ]),
      // Account › Assistant (A4·B): Nexus, the person's own agent. Hidden in a build without the agent API (`settings.tsx`).
      { label: "Assistant", icon: "comment", href: "/assistant-settings", words: ["nexus", "approvals", "always allow", "memory", "what nexus knows", "learning", "sounds", "mute"] },
      { label: "Agents and plans", icon: "agents", href: "/agents", words: ["coding agents", "claude", "codex", "subscriptions", "who pays"] },
      { label: "API keys", icon: "keys", href: "/keys", words: ["keys", "tokens"] },
      hub("connections", "Connections", "connector", "Computers and integrations", [
        { label: "Computers", icon: "agents", href: "/computers", about: "Connect a computer to follow and control its coding sessions.", words: ["pair", "qr", "scan", "desktop", "devices", "phone control"] },
        { label: "Integrations", icon: "connector", href: "/integrations", about: "Your connected accounts, tools and MCP servers.", words: ["connectors", "mcp", "accounts"] },
      ]),
      { label: "Privacy", icon: "shield", href: "/privacy", words: ["ai providers", "consent", "crash", "error reports"] },
      { label: "Referrals", icon: "gift", href: "/referrals", words: ["invite", "credit", "link"], needs: ["referrals.read"] },
    ],
  },
  {
    key: "org",
    rows: [
      hub("general", "General", "info", "Name, domains and data", [
        { label: "General", icon: "info", href: "/general", about: "The organization's name, address, domains and owner.", words: ["name", "address", "domains", "owner"] },
        { label: "Data", icon: "data", href: "/data", about: "What is kept, for how long, and web search.", words: ["retention", "privacy", "web search"] },
      ]),
      hub("people", "People", "members", "Members and teams", [
        { label: "Members", icon: "person", href: "/members", about: "Who is in the organization, and their roles.", words: ["people", "invite"], company: true },
        { label: "Teams", icon: "members", href: "/teams", about: "Groups of members, with their own budgets.", words: ["people", "groups"], company: true },
      ]),
      hub("models", "Model access", "models", "Models, policy and provider keys", [
        { label: "Models", icon: "models", href: "/models", about: "The models the organization can use.", words: ["catalog"] },
        { label: "Policy", icon: "privacy", href: "/policy", about: "Rules that allow or deny models and tools.", words: ["rules", "allow", "deny"] },
        { label: "Providers", icon: "providers", href: "/providers", about: "The organization's provider keys.", words: ["provider keys", "byok", "anthropic", "openai"], needs: ["provider_keys.manage"] },
        { label: "Gateway", icon: "gateway", href: "/gateway", about: "Nexus Gateway and its products.", words: ["products"] },
      ], ["model policy"]),
      hub("usage", "Usage", "spend", "Spend and budgets", [
        { label: "Usage", icon: "spend", href: "/spend", about: "What was spent, by whom and on what.", words: ["spend", "cost"], needs: ["spend.read_all"] },
        { label: "Budgets", icon: "gauge", href: "/budgets", about: "Spending limits for people and teams.", words: ["limits", "spend"] },
        { label: "Analytics", icon: "analytics", href: "/analytics", about: "How the organization adopts AI.", words: ["adoption"] },
        { label: "Observability", icon: "pulse", href: "/observability", about: "Runs and model calls, as they happen.", words: ["runs", "calls", "adoption", "agents"], needs: ["spend.read_all", "spend.export"] },
      ]),
      { label: "Billing", icon: "billing", href: "/billing", words: ["plan", "payment", "card", "invoice", "credits"], needs: ["org.billing"] },
      { label: "Repositories", icon: "repositories", href: "/repositories", words: ["github", "code"] },
      { label: "Skills", icon: "skill", href: "/skills" },
      { label: "Audit log", icon: "audit", href: "/audit", words: ["log", "events", "changes"] },
    ],
  },
  {
    key: "app",
    rows: [
      { label: "Archived sessions", icon: "archive", href: "/archived", words: ["archive", "chats"] },
      { label: "Docs", icon: "docs", href: "/docs", words: ["help", "guide"] },
      { label: "Send feedback", icon: "feedback", href: "/feedback", words: ["bug", "idea"] },
      { label: "About", icon: "info", href: "/about", words: ["version", "support", "terms", "privacy policy"] },
    ],
  },
];

/** A part's title: "You", the organization's name (or "Personal"), "App". */
export function partTitle(key: PartKey, account: string, personal: boolean): string {
  if (key === "you") return "You";
  if (key === "app") return "App";
  return personal ? "Personal" : account || "Organization";
}

/**
 * The parts as this account shows them. Personal has no one else, so pages that manage people are left out; so is a
 * page that needs a permission you do not have (`permissions`, from `GET /v1/tenants/me`; none while it is not known),
 * a page in `hidden`, a row whose pages are all left out, and a part with no rows. A row left with one page links
 * straight to it.
 */
export function settingsFor({ personal, hidden = [], permissions = [] }: { personal: boolean; hidden?: string[]; permissions?: readonly string[] }): SettingsPart[] {
  const allowed = (page: SettingsPage) => (page.needs ?? []).every((need) => permissions.includes(need));
  const shown = (page: SettingsPage) => !(personal && page.company) && !hidden.includes(page.href) && allowed(page);
  return SETTINGS.map((part) => ({
    ...part,
    rows: part.rows.flatMap((row): SettingsRow[] => {
      if (!row.pages) return shown(row) ? [row] : [];
      const pages = row.pages.filter(shown);
      if (!pages.length) return [];
      if (pages.length === 1) return [{ ...pages[0]!, label: row.label, words: [...(row.words ?? []), ...(pages[0]!.words ?? []), pages[0]!.label.toLowerCase()] }];
      return [{ ...row, pages }];
    }),
  })).filter((part) => part.rows.length > 0);
}

/** The pages under the row `key`, as this account shows them; null when there is no such row. */
export function hubFor(key: string, parts: SettingsPart[]): SettingsRow | null {
  for (const part of parts) for (const row of part.rows) if (row.hub === key) return row;
  return null;
}

export type SettingsMatch = SettingsPage & { where: string };

/** The pages whose label or words have every word of `query`, labels first, each with where it lives. */
export function findSettings(query: string, parts: SettingsPart[], account: string, personal = false): SettingsMatch[] {
  const entries = parts.flatMap((part) => {
    const where = partTitle(part.key, account, personal);
    return part.rows.flatMap((row) => row.pages
      ? row.pages.map((page) => ({ ...page, where: `${where} › ${row.label}` }))
      : [{ ...row, where }]);
  });
  return matchSettings(query, entries);
}

export function matchSettings(query: string, entries: SettingsMatch[]): SettingsMatch[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const hits: { page: SettingsMatch; byLabel: boolean }[] = [];
  for (const page of entries) {
    const label = page.label.toLowerCase();
    const all = [label, ...(page.words ?? [])].join(" ");
    if (words.every((w) => all.includes(w))) hits.push({ page, byLabel: words.every((w) => label.includes(w)) });
  }
  return hits.sort((a, b) => Number(b.byLabel) - Number(a.byLabel)).map((h) => h.page);
}
