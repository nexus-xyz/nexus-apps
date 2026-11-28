/**
 * A connector's name as a person reads it, for every app and the agents service. Gateway's catalogue names connectors
 * by id (`GET /v1/connectors` has no display name), so the names live here: Nexus UI's Integrations and the agent's
 * Connect card (A6·e) say them alike.
 */
const NAMES: Record<string, string> = {
  asana: "Asana",
  bitbucket: "Bitbucket",
  cloudflare: "Cloudflare",
  confluence: "Confluence",
  datadog: "Datadog",
  dropbox: "Dropbox",
  figma: "Figma",
  github: "GitHub",
  gitlab: "GitLab",
  gmail: "Gmail",
  "google-calendar": "Google Calendar",
  "google-drive": "Google Drive",
  hubspot: "HubSpot",
  intercom: "Intercom",
  jira: "Jira",
  linear: "Linear",
  "microsoft-todo": "Microsoft To Do",
  notion: "Notion",
  okta: "Okta",
  pagerduty: "PagerDuty",
  postgres: "Postgres",
  quickbooks: "QuickBooks",
  salesforce: "Salesforce",
  sentry: "Sentry",
  shopify: "Shopify",
  slack: "Slack",
  snowflake: "Snowflake",
  stripe: "Stripe",
  teams: "Microsoft Teams",
  x: "X",
  zendesk: "Zendesk",
  zoom: "Zoom",
};

/** `github` is GitHub; an id the map lacks is capitalised by word. */
export function connectorName(id: string): string {
  return (
    NAMES[id] ??
    id
      .split(/[-_]/)
      .filter(Boolean)
      .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
      .join(" ")
  );
}

/**
 * What Nexus can do with a connector, in plain words (A6·e): the Connect card's one line on web, phone and
 * desktop. Each line says what Nexus reads, then that a change asks first, as the board's "It reads your reservations;
 * booking asks first." It is never the operation's own description: that text is for the model. GPUI mirrors this map
 * in `nexus-desktop-core`'s `agent_cards.rs`, and a test in the kit keeps the two alike.
 */
export const CONNECTOR_PURPOSES: Readonly<Record<string, string>> = {
  asana: "It reads your tasks and projects; each change asks first.",
  bitbucket: "It reads your repositories and pull requests; each change asks first.",
  cloudflare: "It reads your zones and settings; each change asks first.",
  confluence: "It reads your pages and spaces; each edit asks first.",
  datadog: "It reads your monitors, logs and metrics; each change asks first.",
  dropbox: "It finds and reads your files; each change asks first.",
  figma: "It reads your files and comments; each change asks first.",
  github: "It reads your repositories, issues and pull requests; each change asks first.",
  gitlab: "It reads your projects, issues and merge requests; each change asks first.",
  gmail: "It reads your mail; sending asks first.",
  "google-calendar": "It reads your calendar and finds time; booking asks first.",
  "google-drive": "It finds and reads your files; each change asks first.",
  hubspot: "It reads your contacts and deals; each change asks first.",
  intercom: "It reads your conversations; each reply asks first.",
  jira: "It reads your issues and projects; each change asks first.",
  linear: "It reads your issues and projects; each change asks first.",
  "microsoft-todo": "It reads your tasks; each change asks first.",
  notion: "It reads your pages and databases; each edit asks first.",
  okta: "It reads your users and groups; each change asks first.",
  pagerduty: "It reads your incidents and who is on call; each change asks first.",
  postgres: "It reads your database; each change asks first.",
  quickbooks: "It reads your invoices and accounts; each change asks first.",
  salesforce: "It reads your accounts and opportunities; each change asks first.",
  sentry: "It reads your issues and errors; each change asks first.",
  shopify: "It reads your orders and products; each change asks first.",
  slack: "It reads your messages; each reply asks first.",
  snowflake: "It reads your data; each change asks first.",
  stripe: "It reads your payments and customers; each change asks first.",
  teams: "It reads your messages; each reply asks first.",
  x: "It reads your mentions; each post asks first.",
  zendesk: "It reads your tickets; each reply asks first.",
  zoom: "It reads your meetings; each booking asks first.",
};

/**
 * The Connect card's line for `id`: its plain words, else Integrations' own line (A6·d), "Nexus can read it, and each
 * write asks you first." `assistant`: the Assistant's name, which the person may change.
 */
export function connectorPurpose(id: string, assistant = "Nexus"): string {
  return (Object.hasOwn(CONNECTOR_PURPOSES, id) ? CONNECTOR_PURPOSES[id] : undefined) ?? `${assistant} can read it, and each write asks you first.`;
}
