/** Wire types of the public Nexus API (`/v1`). The API's OpenAPI document describes the same. */
import type { ArtifactSharingSettings, ChatSessionState } from "./artifacts";
import type { ModuleManifest } from "./manifest";
import type { DataFileType, PageFormat, PagePeer } from "./pages";
export type * from "./computer-types";

export interface ApiError {
  error: { message: string; type: string; code?: string };
}

export interface CreateTenantResponse {
  tenant: { id: string; name: string; budgetUsd: number };
  user: { id: string; email?: string };
  apiKey: string;
}

/** an internal decision roles: one owner per org, admins, members. */
export type Role = "owner" | "admin" | "member";

/** `GET /v1/tenants/me`: the calling key's tenant (org), and who is calling with which role. */
export interface CurrentTenantResponse {
  /**
   * `kind` (Rust gateway): `personal` for a person's own organization (one member), else `company`. `imageUrl` (Rust
   * gateway): the organization's logo, as Clerk serves it; unset when it has none. `mark` (Rust gateway),
   * on the tenant and the user: set only on Nexus's own accounts.
   */
  tenant: { id: string; name: string; createdAt: string; slug?: string; clerkOrgId?: string; kind?: TenantKind; imageUrl?: string; mark?: AccountMark };
  user?: { id: string; email?: string; name?: string; mark?: AccountMark };
  role?: Role;
  /** What the role allows, e.g. `members.manage`. */
  permissions?: string[];
  /** The calling credential: a token id, or `ui` for Nexus UI's server. */
  keyId?: string;
  /** an internal decision: the teams the person is in, in this org. */
  teams?: { id: string; slug: string; name: string; role: TeamRole }[];
  /** an internal decision: their default team, where spend goes when a call names none. */
  team?: { id: string; slug: string; name: string };
  /** Rust gateway: the products this Gateway has on. Unset reads as all off (`wikiOn` in `features.ts`). */
  features?: GatewayFeatures;
}

/** The products a Gateway can turn off. Clients hide what is off. */
export interface GatewayFeatures {
  /** Nexus Wiki (`NEXUS_WIKI_ENABLED`): the Wiki place, its runs, and Ask your code, which reads it. */
  wiki: boolean;
  /** Nexus Apps (`NEXUS_APPS_ENABLED`): `nexus apps` and `/v1/apps`. An older Gateway leaves it out: off. */
  apps?: boolean;
  /** Coding agent sessions write `agent_session.*` audit events (`NEXUS_AUDIT_AGENT_SESSIONS`). An older Gateway leaves it out: off. */
  agentSessionAudit?: boolean;
}

/** `GET /v1/auth/whoami`'s `kind`: which credential called. */
export type CredentialKind = "person" | "agent_link" | "service_account" | "module" | "sandbox" | "operator";

/**
 * `GET /v1/auth/whoami` (Rust gateway): who the calling credential is, for any valid credential. `userId` is
 * the caller's own person, an agent link's person or the operator. `email` comes only for a person's own credential,
 * `grant` only for an agent link. `role` is the person's org role, for a person and for an agent link.
 */
export interface Whoami {
  kind: CredentialKind;
  tenantId: string;
  userId?: string;
  email?: string;
  grant?: string[];
  role?: Role;
  /** What this Gateway has on, as `GET /v1/tenants/me` says it. Any credential reads it, an agent link too. */
  features?: GatewayFeatures;
}

/** An organization's kind (Rust gateway): a company's, or a person's own personal organization (exactly one each). */
export type TenantKind = "company" | "personal";

/** Why an organization or a person is Nexus's own. Figures that leave an organization count it apart. */
export type AccountMark = "staff" | "test" | "demo";

/** `GET /v1/orgs` (Rust gateway): one of your organizations; Personal comes first. */
export interface OrgSummary {
  id: string;
  name: string;
  slug?: string;
  /** Its Clerk organization's id: Nexus UI's URL segment when it has no slug. */
  clerkOrgId?: string;
  kind: TenantKind;
  role?: Role;
  /** The organization this request is in. */
  current?: boolean;
  /** Its logo, as the tenant's `imageUrl`; unset when it has none. */
  imageUrl?: string;
}

/** Pending Gateway approvals and waiting cloud sessions, limited to this person’s organizations. */
export interface OrgWaitingCounts {
  counts: Record<string, number>;
  checkedAt: number;
}

/** `POST /v1/orgs/switch` (Rust gateway): a token for another of your organizations, and who you are there. */
export interface SwitchedOrg extends CurrentTenantResponse {
  token: string;
  tokenKind: "cli" | "ui";
  expiresAt?: string;
}

/** an internal decision: a person's place in a team. `lead` is reserved for the planned team-lead role and grants nothing extra yet. */
export type TeamRole = "lead" | "member";

/** `GET /v1/teams` (each team), `GET /v1/teams/{slug}` (with `memberList`). */
export interface TeamSummary {
  id: string;
  slug: string;
  name: string;
  description?: string;
  /** an internal decision: the parent team's id and slug; absent for a top-level team. */
  parentId?: string;
  parent?: string;
  /** an internal decision: the organization's own label for the team's level, e.g. `department`. */
  level?: string;
  /** an internal decision: 1 for a top-level team. */
  depth: number;
  createdAt: string;
  /** How many people are in it (not counting pending ones). */
  members: number;
  /** an internal decision: people added by email who have not joined the organization yet. */
  pending: number;
  /** an internal decision: how many child teams it has. */
  childCount: number;
  /** The caller's role in it, when they are in it. */
  myRole?: TeamRole;
  /** Whether it is the caller's default team. */
  isDefault?: boolean;
}

export interface TeamMember {
  userId: string;
  email?: string;
  name?: string;
  role: TeamRole;
  /** Whether this team is the person's default. */
  isDefault: boolean;
  joinedAt: string;
}

/** an internal decision: someone added to a team by an email not in the organization yet. */
export interface PendingTeamMember {
  id: string;
  teamId: string;
  /** The team's slug. */
  team: string;
  email: string;
  role: TeamRole;
  /** Becomes their default team when they join. */
  home: boolean;
  addedBy?: string;
  createdAt: string;
}

/** an internal decision: a team as a link (in a path, or a child). */
export interface TeamLink {
  id: string;
  slug: string;
  name: string;
  level?: string;
}

export interface TeamDetail extends TeamSummary {
  memberList: TeamMember[];
  /** an internal decision */
  pendingList: PendingTeamMember[];
  /** an internal decision: its ancestors, top level first (the last is its parent). */
  path: TeamLink[];
  /** an internal decision: its child teams. */
  children: (TeamLink & { members: number; pending: number; childCount: number })[];
}

/** an internal decision: `GET /v1/teams?tree=1`, one team and its child teams. */
export interface TeamNode extends Omit<TeamSummary, "childCount"> {
  leads: { userId?: string; email?: string; name?: string; pending?: boolean }[];
  /** Distinct people in it and every team under it, pending ones included. */
  headcount: number;
  children: TeamNode[];
}

export interface TeamTreeResponse {
  tree: TeamNode[];
  default: string | null;
}

/** an internal decision: one person in an import file. `email: null` keeps a seat for someone whose email is not known yet. */
export interface TeamImportPerson {
  email?: string | null;
  name?: string;
  home?: boolean;
}

/** an internal decision: one team in an import file, matched by slug. */
export interface TeamImportUnit {
  slug: string;
  name?: string;
  level?: string | null;
  /** The parent's slug; absent or null for a top-level team. */
  parent?: string | null;
  description?: string | null;
  leads?: (string | TeamImportPerson)[];
  members?: (string | TeamImportPerson)[];
}

/** an internal decision: `POST /v1/teams/import`. */
export interface TeamImport {
  units: TeamImportUnit[];
  dryRun?: boolean;
  prune?: boolean;
  /** A dry run's `plan`: apply only if the result is still exactly that (else 409 `plan_changed`). */
  plan?: string;
}

/** an internal decision: what an import changed (or, with `dryRun`, would change). */
export interface TeamImportDiff {
  dryRun: boolean;
  prune: boolean;
  created: string[];
  moved: { slug: string; from: string | null; to: string | null }[];
  renamed: { slug: string; from: string; to: string }[];
  relevelled: { slug: string; from: string | null; to: string | null }[];
  membershipsAdded: { team: string; email: string; role: TeamRole }[];
  rolesChanged: { team: string; email: string; from: TeamRole; to: TeamRole }[];
  membershipsRemoved: { team: string; email: string; role: TeamRole }[];
  pending: { team: string; email: string; role: TeamRole }[];
  pendingRemoved: { team: string; email: string }[];
  defaults: { email: string; team: string; pending: boolean }[];
  deleted: string[];
  warnings: string[];
  totals: { teams: number; memberships: number; pending: number };
  /** A short hash of this diff; pass it back as `plan` to apply exactly what a dry run showed. */
  plan: string;
}

/** `POST /v1/teams/{slug}/members`: added now (201), or pending until they join the organization (202). */
export type AddTeamMemberResponse = { pending: false; team: string; member: TeamMember } | { pending: true; team: string; entry: PendingTeamMember };

export interface TeamsResponse {
  teams: TeamSummary[];
  /** The caller's default team's slug, or null. */
  default: string | null;
}

/** `GET /v1/analytics/usage`: how usage is grouped. `hour`: one group per UTC hour with calls, keyed `YYYY-MM-DDTHH`. */
export type UsageGroupBy = "team" | "person" | "model" | "provider" | "key" | "product" | "day" | "hour";

/** `GET /v1/analytics/usage`: calls by status (eng-architecture §4.4). */
export interface UsageCallsByStatus {
  settled: number;
  failed: number;
  refused: number;
  released: number;
}

export interface UsageFigures {
  spentUsd: number;
  /** Rust gateway: `spentUsd` as an exact decimal string (`"12.245000"`); `null` with no call at all (rule 9's empty day). Prefer it; `spentUsd` is deprecated. */
  spentUsdDecimal?: string | null;
  calls: number;
  inputTokens: number;
  outputTokens: number;
  /** Calls by status, refused calls included; `calls` counts settled, failed and released only (Rust gateway only). */
  callsByStatus: UsageCallsByStatus;
  /** `(failed + refused) / (settled + failed + refused)`, 0–1; `null` when there are no such calls. Released calls count in neither part (Rust gateway only). */
  errorRate: number | null;
}

/** `GET /v1/analytics/usage`: refused calls a spend limit stopped, and what they asked to hold. */
export interface UsageOverspendFigures {
  calls: number;
  /** What the calls asked to hold, not what they would have cost: the spend file's `overspendAmountUsd`, summed. */
  amountUsd: number;
  /** Rust gateway: the exact decimal string of the number beside it (`"12.245000"`). Prefer it; the number is deprecated (root rule 9). */
  amountUsdDecimal?: string;
}

/** The overspend prevented over the report's days: in all, and by the level whose limit refused the calls. */
export interface UsageOverspendPrevented extends UsageOverspendFigures {
  /** `product` is a product's monthly cap. A refusal at any other level counts nowhere. */
  byLevel: Record<"organization" | "team" | "person" | "product", UsageOverspendFigures>;
}

export interface UsageTotals extends UsageFigures {
  /** Zeros when no limit stopped a call. Absent from the TypeScript gateway (Rust gateway only). */
  overspendPrevented?: UsageOverspendPrevented;
}

export interface UsageGroup extends UsageFigures {
  /** A team id, user id, model, provider, `product/capability`, key id, day or UTC hour (`YYYY-MM-DDTHH`); `ui:<user id>` for a person's Nexus UI sessions; `unassigned` for spend with no team (or no person, or key); `others` for a member's teammates (or their keys). */
  key: string;
  label: string;
  /** Share of the range's spend, 0–1. */
  share: number;
  slug?: string;
  /** Keys: the key's name, when it has one (Rust gateway only). */
  name?: string;
  /** Keys: the first characters of the token, to recognise it. Never the token (Rust gateway only). */
  prefix?: string;
  /** A team deleted since, or a key that no store holds. A team's spend keeps its old slug. A key shows its id. */
  deleted?: boolean;
  /** Keys: the key is revoked. It keeps its name and prefix, and its label ends with `, revoked` (Rust gateway only). */
  revoked?: boolean;
  /** an internal decision, teams: how deep the team sits (1 = top level) and its level label. */
  depth?: number;
  level?: string;
  /** an internal decision, rollups: the team has child teams (drill in with `under`). */
  hasChildren?: boolean;
  /** an internal decision, rollups: spend attributed to the team itself, not to the teams under it. */
  direct?: boolean;
  /** By team, `product:<product>/<capability>`: a module's own calls, with no team and no person (Rust gateway only). */
  product?: string;
  capability?: string;
}

export interface UsageDay extends UsageFigures {
  day: string;
  /** Spend per group key that day (absent for `groupBy=day`). */
  groups?: Record<string, number>;
  /** Rust gateway: `groups` with each spend as an exact decimal string. Prefer it; `groups` is deprecated. */
  groupsDecimal?: Record<string, string>;
}

/** `GET /v1/analytics/usage`. */
export interface UsageReport {
  from: string;
  to: string;
  groupBy: UsageGroupBy;
  /**
   * `org`: the whole org (owners and admins); `member`: your own usage and your teams'; `own`: only your own calls
   * (an agent link, or `person=me`).
   */
  scope: "org" | "member" | "own";
  team?: { id: string; slug: string; name: string };
  /** an internal decision, rollups: the depth shown, from the top level. */
  depth?: number;
  /** an internal decision, rollups: the team whose child teams are shown, with its path. */
  under?: { id: string; slug: string; name: string; level?: string; path: TeamLink[] };
  totals: UsageTotals;
  groups: UsageGroup[];
  series: UsageDay[];
}

export interface UsageQuery {
  /** `YYYY-MM-DD`, UTC, inclusive; default: 29 days before `to`. */
  from?: string;
  /** `YYYY-MM-DD`, UTC, inclusive; default: today. */
  to?: string;
  groupBy?: UsageGroupBy;
  /** Only this team's usage and its child teams' (slug). */
  team?: string;
  /** an internal decision, groupBy team: roll up to the teams at this depth (1 = top level; with `under`, counted below it). */
  depth?: string;
  /** an internal decision, groupBy team: roll up to the child teams of this team (slug). */
  under?: string;
  /** `me`: only your own calls, under every groupBy (Rust gateway). */
  person?: "me";
}

/** `GET /v1/analytics/agents`: one person whose agent calls (`nexus claude`, `nexus codex`) went through Nexus Gateway that day. */
export interface AgentAdoptionPerson {
  userId: string;
  email: string | null;
  /** Calls through Nexus Gateway that day; refused calls are left out. */
  calls: number;
}

/** `GET /v1/analytics/agents`: one person with only `subscription` sessions that day (their own Claude or ChatGPT plan). */
export interface AgentAdoptionSubscriber {
  userId: string;
  email: string | null;
  /** Sessions that reported a model call that day. */
  sessions: number;
}

/** One product's active engineers on one day. */
export interface AgentAdoptionProduct {
  /** The product slug: `core`, `agents` or `chat`. */
  product: string;
  /** People with a non-refused call in this product that day; for `agents`, subscription-only people too. */
  engineers: number;
}

/** One UTC day of agent adoption, split by route. A person on both routes that day is in `people` only. */
export interface AgentAdoptionDay {
  day: string;
  /** Through Nexus Gateway. */
  people: AgentAdoptionPerson[];
  /** On their own subscription, with no call through Nexus Gateway that day. */
  subscription: AgentAdoptionSubscriber[];
  /** Distinct active engineers that day over every product; 0 on a day with no calls. */
  engineers: number;
  /** Active engineers by product, sorted, only products with someone. They can add up to more than `engineers`. */
  products: AgentAdoptionProduct[];
}

/**
 * `GET /v1/analytics/agents`: agent adoption per day, by route, and active
 * engineers by product. Rust gateway only.
 */
export interface AgentAdoption {
  from: string;
  to: string;
  /** `org`: the whole organization (owners and admins); `member`: only the caller. */
  scope: "org" | "member";
  /** Every day from `from` to `to`, oldest first. */
  days: AgentAdoptionDay[];
}

export interface AgentAdoptionQuery {
  /** `YYYY-MM-DD`, UTC, inclusive; default: 29 days before `to`. */
  from?: string;
  /** `YYYY-MM-DD`, UTC, inclusive; default: today. At most 366 days. */
  to?: string;
}

/**
 * `GET /v1/teams/{slug}/budget` (Rust gateway). The Ledger shows a caller only what remains for them, and
 * nothing from a team above theirs: `own` is a member's view, `cap` an owner's or admin's view of that one cap.
 * Person budgets are `PersonBudget`; team model policies are `ModelPolicyView`.
 */
export interface TeamBudget {
  team: { slug: string; name: string };
  scope: "cap" | "own";
  /** The cap on the team (`cap`), or the cap that binds along its path (`own`); null when none applies. */
  budget: {
    /** What is left for the period, never below 0. */
    remainingUsd: number;
    /** Rust gateway: the exact decimal string of the number beside it (`"12.245000"`). Prefer it; the number is deprecated (root rule 9). */
    remainingUsdDecimal?: string;
    period: "day" | "week" | "month";
    resetsAt: string;
    /** The rest is for owners and admins (`scope: "cap"`), each with its decimal twin. */
    limitUsd?: number;
    limitUsdDecimal?: string;
    mode?: "hard_stop";
    spentUsd?: number;
    spentUsdDecimal?: string;
    reservedUsd?: number;
    reservedUsdDecimal?: string;
    updatedAt?: string;
  } | null;
  /** Whether the caller may set the cap (owners and admins). */
  canManage: boolean;
}

/** What `PUT /v1/teams/{slug}/budget` takes. Only `hard_stop` is available: `warn`, `approval` and `warnAtPercent` answer 400. */
export interface BudgetSetting {
  /** The cap: a number (deprecated, root rule 9) or a decimal string such as `"12.245"`. Send it or `amountUsdDecimal`. */
  amountUsd?: number | string;
  /** Rust gateway: the cap as a decimal string, at most 6 decimals, read exactly. With `amountUsd` too, both must name one amount. */
  amountUsdDecimal?: string;
  period: "day" | "week" | "month";
  mode?: "hard_stop";
  /** Check it and answer what would change; change nothing. */
  dryRun?: boolean;
}

/** A team's cap as `PUT /v1/teams/{slug}/budget` answers it, before and after. */
export interface TeamBudgetChange {
  dryRun: boolean;
  team: { slug: string; name: string };
  /** Each cap's `limitUsd` comes with `limitUsdDecimal`, its exact decimal string (Rust gateway); prefer it. */
  before: { limitUsd: number; limitUsdDecimal?: string; period: "day" | "week" | "month"; mode: "hard_stop" } | null;
  after: { limitUsd: number; limitUsdDecimal?: string; period: "day" | "week" | "month"; mode: "hard_stop" };
}

/** A module capability's budget and its UTC period so far (`GET /v1/budgets/products/{product}/{capability}`). Amounts are decimal strings. */
export interface ProductBudget {
  product: string;
  capability: string;
  /** Null without a budget. */
  cap: {
    /** The budget's amount for its period. Absent from a Gateway older than it: read `monthlyUsd`. */
    limitUsd?: string;
    /** `day`, `week` or `month`, in UTC. Absent from a Gateway older than it: a month. */
    every?: BudgetEvery;
    /** The amount whatever its period; its name is from before budgets had a period (`every`). */
    monthlyUsd: string;
    /** The period: `YYYY-MM-DD`, `YYYY-Www` or `YYYY-MM`. */
    period: string;
    spentUsd: string;
    /** The product's own fees this period, part of `spentUsd`. */
    feesUsd: string;
    reservedUsd: string;
    remainingUsd: string;
    resetsAt: string;
    updatedAt: string;
  } | null;
  canManage: boolean;
}

/** A product budget's period. */
export type BudgetEvery = "day" | "week" | "month";

/** One side of a product budget's change: `limitUsd` null is no budget. `limitUsd` and `every` are absent from an older Gateway. */
export interface ProductBudgetSide {
  limitUsd?: string | null;
  every?: BudgetEvery | null;
  monthlyUsd: string | null;
}

/** A module capability's budget before and after a `PUT /v1/budgets/products/{product}/{capability}`. */
export interface ProductBudgetChange {
  product: string;
  capability: string;
  before: ProductBudgetSide;
  after: ProductBudgetSide;
}

/** The organization's budget as `POST /v1/orgs/budget` answers it: a cap per `period` (Rust gateway, Ledger M1.4). */
export interface OrgBudget {
  budgetUsd: number;
  /** Rust gateway: the exact decimal string of the number beside it (`"12.245000"`). Prefer it; the number is deprecated (root rule 9). */
  budgetUsdDecimal?: string;
  /** The Rust gateway's; the TypeScript gateway's budget has no period. */
  period?: "day" | "week" | "month";
}

/** A dry run of `POST /v1/orgs/budget` (Rust gateway): the budget before and after, each with its exact decimal twin; nothing changed. */
export interface OrgBudgetChange {
  dryRun: true;
  before: { budgetUsd: number; budgetUsdDecimal?: string; period: "day" | "week" | "month" };
  after: { budgetUsd: number; budgetUsdDecimal?: string; period: "day" | "week" | "month" };
}

/**
 * The details (`error.nexus`) of a 402 `budget_exceeded` or `subsidy_exhausted`. Each money figure comes as a number
 * (deprecated, root rule 9) and as its exact decimal twin, which the Rust gateway sends. A refusal below
 * the person level names only what is left and what was asked.
 */
export interface BudgetRefusal {
  level?: "organization" | "team" | "person" | "product";
  period?: string;
  resetsAt?: string;
  /** Person level only: `override` when set for them, `default` for the organization's default per person. */
  source?: "override" | "default";
  /** Person level only. */
  limitUsd?: number;
  limitUsdDecimal?: string;
  spentUsd?: number;
  spentUsdDecimal?: string;
  reservedUsd?: number;
  reservedUsdDecimal?: string;
  /** Every level (the person level included), and `subsidy_exhausted`: what is left, never below 0. */
  remainingUsd?: number;
  remainingUsdDecimal?: string;
  /** What the call may cost at most. */
  requestedUsd?: number;
  requestedUsdDecimal?: string;
  /** True once Gateway has told the refused person and the organization's owners and admins. */
  notified?: boolean;
  fundedBy?: string;
  next?: string[];
}

/**
 * What `PUT /v1/budgets/person-default` and `PUT /v1/people/{userId}/budget` take: `monthlyUsd` or `monthlyUsdDecimal`.
 * `null` clears the limit. With both, they must name one amount (400 `amount_mismatch`).
 */
export interface PersonLimitSetting {
  /** The limit: a number (deprecated, root rule 9), a decimal string such as `"12.245"`, or `null`. */
  monthlyUsd?: number | string | null;
  /** Rust gateway: the limit as a decimal string, at most 6 decimals, read exactly; or `null`. */
  monthlyUsdDecimal?: string | null;
}

/** an internal decision person budgets: one person's spend limit for the calendar month (UTC). `GET /v1/people/{userId}/budget`. */
export interface PersonBudget {
  userId: string;
  email?: string;
  name?: string;
  role: Role;
  /** Their limit this month; null: no limit of their own (only the organization's budget applies). */
  limitUsd: number | null;
  /** Rust gateway: the exact decimal string of the number beside it (`"12.245000"`). Prefer it; the number is deprecated (root rule 9). */
  limitUsdDecimal?: string | null;
  /** `override`: set for them; `default`: the organization's default per person; `none`: no limit. */
  source: "override" | "default" | "none";
  /** Their override, when they have one. */
  overrideUsd: number | null;
  overrideUsdDecimal?: string | null;
  /** Settled since the 1st, 00:00 UTC, across every team they spend through. */
  spentUsd: number;
  spentUsdDecimal?: string;
  /** Held by their calls in flight. */
  reservedUsd: number;
  reservedUsdDecimal?: string;
  remainingUsd: number | null;
  /** Rust gateway: each `*Usd` number above has a `*UsdDecimal` twin, its exact decimal string. Prefer the twins. */
  remainingUsdDecimal?: string | null;
  /** Spent and reserved reach the limit: their next call is refused (402 `budget_exceeded`, level `person`). */
  atLimit: boolean;
  /** Calls a spend limit refused this month. */
  overspendsPrevented: number;
  /** An admin an owner allowed to set person spend limits (Rust gateway only). Owners always may; others never. */
  limitManager?: boolean;
}

/** `GET /v1/budgets`: this month's spend limits. */
export interface BudgetsResponse {
  /** The calendar month (UTC), `YYYY-MM`. */
  period: string;
  periodStart: string;
  /** The 1st of next month, 00:00 UTC: when limits reset. */
  resetsAt: string;
  /** The organization's default monthly limit per person; null: none. */
  personDefault: { monthlyUsd: number | null; monthlyUsdDecimal?: string | null; updatedAt?: string; updatedBy?: string };
  /** `org`: everyone (owners and admins); `own`: only you. */
  scope: "org" | "own";
  people: PersonBudget[];
  /** The caller may set a person's limit: an owner, or an admin an owner allowed. */
  canManage: boolean;
  /** The caller may set the default per person and allow admins: an owner (Rust gateway only). */
  canGrant?: boolean;
}

/** `PUT` and `DELETE /v1/budgets/limit-managers/{userId}` (Rust gateway): whether the admin may set person limits now. */
export interface LimitManagerChange {
  userId: string;
  limitManager: boolean;
}

/** `POST /v1/auth/device`: start `nexus auth login`. */
export interface DeviceCodeResponse {
  device_code: string;
  user_code: string;
  /** Nexus UI's approval page, when Core knows where Nexus UI is (`NEXUS_UI_URL`). */
  verification_uri?: string;
  verification_uri_complete?: string;
  expires_in: number;
  interval: number;
}

/** `POST /v1/auth/device` with `agent` and `scope`: the approval issues an agent link, not a person's token. */
export interface DeviceScope {
  /** The agent's slug, such as `slack`. */
  agent: string;
  /** Matrix actions, space-separated: `models.use`, `spend.read_own`, `connectors.use`, `chat.ask`, `chat.web`, `agents.memory`, `agents.profiles`, `routines.use`, `cloud.runs.use`, `sessions.read`, `sessions.start`, `sessions.steer`. */
  scope: string;
}

/** `POST /v1/auth/token` once the code is approved. */
export interface DeviceTokenResponse {
  access_token: string;
  token_type: "Bearer";
  expires_at?: string;
  tenant: { id: string; name: string; slug?: string };
  user: { id: string; email?: string };
  role: Role;
  /** The actions an agent link holds, space-separated; only for a sign-in that asked for a scope. */
  scope?: string;
  /** Nexus UI's address (`NEXUS_UI_URL`), when Gateway knows it: where a client's links into Nexus UI go. */
  ui_url?: string;
}

/** `POST /v1/auth/authorize` (Rust only): the phone's sign-in, asked by Nexus UI's server for the signed-in person. */
export interface AppAuthorizationRequest {
  /** The app's label, kept on its token: "Nexus for iPhone". */
  client?: string;
  /** BASE64URL(SHA-256(verifier)), unpadded (RFC 7636). */
  codeChallenge: string;
  codeChallengeMethod: "S256";
  /** The app's own: `nexus://auth`. */
  redirectUri: string;
}

/** The code Nexus UI sends to the app's redirect URI: single-use, for `expiresIn` seconds. */
export interface AppAuthorization {
  code: string;
  expiresIn: number;
}

export interface TokenSummary {
  id: string;
  userId: string;
  kind: "api" | "cli" | "ui";
  prefix: string;
  label?: string;
  createdAt: string;
  expiresAt?: string;
  current?: boolean;
}

/** What a key reaches, which is its account's. Read from the account, never stored on the key. */
export interface ServiceAccountScope {
  teamId: string;
}

/** A service account's key (Rust gateway only). Never the token, except in the answer that issues or rotates it. */
export interface ServiceAccountKey {
  /** `sak_…` */
  id: string;
  serviceAccountId: string;
  /** The member who answers for its account. */
  ownerUserId: string;
  scope: ServiceAccountScope;
  /** The first characters of the token, to recognise it in a list. */
  prefix: string;
  label?: string;
  /** The person who issued it. */
  createdBy: string;
  createdAt: string;
  expiresAt: string;
  revokedAt?: string;
  /** Written at most once an hour. */
  lastUsedAt?: string;
  /** The key this one replaced in a rotation. */
  rotatedFrom?: string;
}

/** A caller for CI and scripts (Rust gateway only). It is no member and holds no seat. */
export interface ServiceAccount {
  /** `sva_…` */
  id: string;
  name: string;
  /** The member who answers for it. Its keys stop when they leave the organization. */
  ownerUserId: string;
  /** The team that owns it. Its keys stop when the team is deleted. */
  teamId: string;
  /** The person who made it. */
  createdBy: string;
  createdAt: string;
  expiresAt: string;
  revokedAt?: string;
  /** Every key it held, newest first. */
  keys: ServiceAccountKey[];
}

/** `GET /v1/service-accounts` */
export interface ServiceAccountList {
  serviceAccounts: ServiceAccount[];
  /** The next page's cursor, while more items follow. Only when `limit` or `cursor` was sent. */
  nextCursor?: string;
}

/** `POST /v1/service-accounts` */
export interface CreateServiceAccountRequest {
  /** 1 to 64 characters. */
  name: string;
  /** The member who answers for it. The caller by default. */
  ownerUserId?: string;
  /** The team that owns it: its slug or id. */
  team: string;
  /** In the future, at most 365 days from now. */
  expiresAt: string;
  /** Check everything and write nothing. */
  dryRun?: boolean;
}

/** `POST /v1/service-accounts`: the account as it was made or, with `dryRun`, as it would be made. */
export interface CreatedServiceAccount extends ServiceAccount {
  /** A dry run's id is never used. */
  dryRun: boolean;
}

/** `POST /v1/service-accounts/{id}/keys` and `.../keys/{keyId}/rotate` */
export interface ServiceAccountKeyRequest {
  /** 1 to 80 characters. Required to issue a key. A rotation keeps the old label when it names none. */
  label?: string;
  /** In the future, within the account's life. 90 days from now by default; a rotation keeps the old lifetime. */
  expiresAt?: string;
}

/** The answer that issues or rotates a key. */
export interface CreatedServiceAccountKey extends ServiceAccountKey {
  /** `nxsa_…`: in this answer, and in a replay of it (same `Idempotency-Key`) within 24 hours. */
  token: string;
}

/** `DELETE /v1/service-accounts/{id}` */
export interface ServiceAccountRevoked {
  id: string;
  /** It was revoked before this request. */
  already: boolean;
  dryRun: boolean;
  revokedAt: string | null;
  /** The live keys it stopped (with `dryRun`, the ones it would stop). */
  keysRevoked: number;
}

/** `POST /v1/slack/workspaces/{id}/channels` (Rust gateway only). */
export interface BindChatChannelRequest {
  /** Slack's channel id (`C…`, or `G…` for an older private channel). */
  channelId: string;
  /** The team the channel's calls are attributed to, by slug or id. */
  team: string;
}

/** A Slack channel bound to a team (Rust gateway only). Ids only. */
export interface ChatChannelBinding {
  /** The workspace binding (`chw_…`). */
  workspaceId: string;
  channelId: string;
  teamId: string;
  /** Absent when the team was deleted since. */
  teamSlug?: string;
  /** The person who bound it. */
  createdBy: string;
  createdAt: string;
  updatedAt: string;
}

/** `GET /v1/slack/workspaces/{id}/channels`: owners and admins see every one, a lead those of the teams they lead. */
export interface ChatChannelList {
  channels: ChatChannelBinding[];
}

/** `DELETE /v1/slack/workspaces/{id}/channels/{channel}` */
export interface ChatChannelUnbound {
  workspaceId: string;
  channelId: string;
  /** The team it was bound to. */
  teamId: string;
  removed: boolean;
}

/** The level that set a value: `channel`, `team` (named by `subject`), `organization`, `deployment` or `catalog`. */
export interface SettingSource {
  level: string;
  /** The team's slug at the `team` level. */
  subject?: string;
}

/** `GET /v1/slack/workspaces/{id}/channels/{channel}/settings`: a bound channel's effective settings (no an internal decision tree yet). */
export interface ChatChannelSettings {
  workspaceId: string;
  channelId: string;
  team: { id: string; slug: string; source: SettingSource };
  /** `caps` for an owner or admin; `own` for a team lead, who sees only `spendLeft`. */
  spendScope: "caps" | "own";
  /** The open team caps on the bound team's path, top level first: owners and admins only, empty for a lead. */
  spendCaps: { limitUsd: string; period: string; mode: string; source: SettingSource }[];
  /** What those caps leave: the tightest cap's room and period, naming no cap. Absent when none caps it. */
  spendLeft?: { remainingUsd: string; period: string; resetsAt: string };
  modelPolicy: {
    /** Every level that holds a model rule for the bound team, widest first. */
    levels: SettingSource[];
    defaultChatModel?: string;
    defaultChatModelSource: SettingSource;
  };
}

/** A job's state: `done`, `failed` and `cancelled` are final. */
export type JobState = "accepted" | "running" | "waiting_approval" | "done" | "failed" | "cancelled";

/** `POST /v1/jobs` (Rust gateway only): a module's verb to run in the background, with its input. */
export interface SubmitJobRequest {
  /** `<product>/<capability>`, such as `intelligence/wiki`: the path of the verb's own route. */
  module: string;
  verb: string;
  /** A JSON object; `{}` when left out. */
  input?: Record<string, unknown>;
  /** The team the job's model calls go to, by slug or id; the caller's default team when left out. */
  team?: string;
}

/** A job, as `POST /v1/jobs`, `GET /v1/jobs/{id}` and its cancel answer it. */
export interface Job {
  /** `job_…`, derived from the submit's `Idempotency-Key`. */
  id: string;
  module: string;
  verb: string;
  state: JobState;
  teamId?: string;
  createdAt: string;
  updatedAt: string;
  deadlineAt?: string;
  startedAt?: string;
  finishedAt?: string;
  /** Why it failed. */
  failureKind?: string;
}

/** `GET /v1/jobs/{id}/result`: the job's state, and its result once it is `done` or `failed`. */
export interface JobResult {
  id: string;
  state: JobState;
  result?: unknown;
  failureKind?: string;
}

/** Vercel's state of an app's last deployment. */
export type AppDeploymentState = "QUEUED" | "INITIALIZING" | "BUILDING" | "READY" | "ERROR" | "CANCELED" | "BLOCKED";

/**
 * An app (Nexus Apps): a folder deployed to Vercel on Nexus's own team, one Vercel project per app. The
 * costs are what Vercel billed Nexus for it, USD decimal strings, null until a cost pull saw a charge.
 */
export interface App {
  /** `app_…`. */
  id: string;
  name: string;
  /** The member who deployed it first. */
  userId: string;
  /** Its production URL once Vercel assigned it, else its last deployment's own; null before the first. */
  url: string | null;
  state: AppDeploymentState | null;
  framework: string | null;
  /** The Vercel project's name on Nexus's team. */
  projectName: string;
  deployedAt: string | null;
  /** When a member last asked for a claim link. */
  claimRequestedAt: string | null;
  createdAt: string;
  updatedAt: string;
  costToDate: string | null;
  cost30d: string | null;
  costUpdatedAt: string | null;
  billingError: string | null;
  monthlyCapUsd: string;
  monthSpendUsd: string;
  pauseAt: string | null;
  pausedReason: string | null;
}

/** `GET /v1/apps`. */
export interface AppList {
  apps: App[];
}

/** `POST /v1/apps`: a folder's files, each path relative to it and its bytes as standard base64. */
export interface DeployAppRequest {
  name: string;
  /** A Vercel framework preset, such as `nextjs`; Vercel detects it when left out. */
  framework?: string;
  files: { path: string; data: string }[];
}

/** `DELETE /v1/apps/{id}`. */
export interface AppDeleted {
  id: string;
  deleted: boolean;
}

/** `POST /v1/apps/{id}/claim`: a link that moves the app's Vercel project to the opener's own Vercel account. */
export interface AppClaim {
  id: string;
  claimUrl: string;
  expiresAt: string;
}

/** A day of the week, as a routine's trigger names it. */
export type RoutineDay = "mon" | "tue" | "wed" | "thu" | "fri" | "sat" | "sun";

/**
 * What a `watch` trigger checks: a pull request merged or closed, an issue that changes state (`target` is
 * `owner/repo#N`), or the owner's own AI spend today over `overUsd` (a decimal string, root rule 9). Today is the UTC
 * day, whatever the routine's `timezone`.
 */
export interface RoutineWatch {
  source: "github_pull" | "github_issue" | "spend_today";
  /** With `github_pull` and `github_issue`. */
  target?: string;
  /** With `spend_today`. */
  overUsd?: string;
}

/**
 * When a routine runs (W4.1): `everySeconds` (600 or more), or `at` (`HH:MM`) on `days`, in `timezone`. A
 * `watch` checks `watch` every `everySeconds`.
 */
export interface RoutineTrigger {
  kind: "schedule" | "watch";
  everySeconds?: number;
  watch?: RoutineWatch;
  at?: string;
  /** `mon` … `sun`, in week order; every day when left out. */
  days?: RoutineDay[];
  /** An IANA name; `UTC` when left out. */
  timezone?: string;
  catchUp?: "latest" | "none";
}

/** What a routine does: `brief`, or a watch's `notify`. Both read only. */
export interface RoutineAction {
  kind: "brief" | "notify";
}

/** When a routine's run speaks up, and what it may write: never. */
export interface RoutineProactivity {
  notify?: "always" | "on_change" | "digest";
  writes?: "never";
}

/** A routine's own limits. */
export interface RoutineCaps {
  /** Runs a day, in the trigger's time zone. */
  maxRunsPerDay?: number;
}

/** `POST /v1/routines` (Rust gateway only): a routine for yourself. */
export interface CreateRoutineRequest {
  title?: string;
  trigger: RoutineTrigger;
  action: RoutineAction;
  proactivity?: RoutineProactivity;
  caps?: RoutineCaps;
  reportTo?: "slack";
  /** The team its model calls go to, by slug or id; your default team when left out. */
  team?: string;
}

/** A routine, as `/v1/routines` answers it. It runs as its owner only, on its Runtime schedule. */
export interface Routine {
  /** `rtn_…` */
  id: string;
  title: string;
  ownerId: string;
  teamId?: string;
  executor: "gateway";
  trigger: RoutineTrigger;
  action: RoutineAction;
  proactivity: RoutineProactivity;
  caps: RoutineCaps;
  reportTo: "slack";
  state: "active" | "paused";
  pausedReason?: "owner" | "admin";
  scheduleId?: string;
  /** Its schedule's next slot; absent while paused. */
  nextRunAt?: string;
  lastRunAt?: string;
  /** Whether Gateway can deliver its action now; else `routine_brief_unavailable` or `routine_watch_unavailable`. */
  delivery: { available: boolean; code?: string; reason?: string };
  createdVia: "api" | "cli" | "ui" | "slack" | "desktop" | "agent";
  version: number;
  createdAt: string;
  updatedAt: string;
}

/** `GET /v1/routines`. */
export interface RoutineList {
  routines: Routine[];
}

/** `DELETE /v1/routines/{id}`. */
export interface RoutineDeleted {
  id: string;
  deleted: boolean;
}

/** A cloud machine's state: the desktop's `CloudState`. */
export type CloudMachineState =
  | "queued"
  | "creating"
  | "starting"
  | "ready"
  | "sleeping"
  | "stopping"
  | "failed"
  | "gone";

/** A cloud machine's size, priced per second at the provider's rate. */
export type CloudMachineSize = "small" | "default" | "large" | "xlarge";

/**
 * Why Gateway put a machine to sleep: its compute was refused, it ran its longest run, its organization was turned off,
 * nobody used it (or its host never connected), or it restarts bigger.
 */
export type CloudStopReason = "budget" | "max_run" | "not_enabled" | "idle" | "resize";

/** A cloud machine (Rust gateway only): the machine of one desktop session, on Nexus's provider account. */
export interface CloudMachine {
  /** `cm_…`, derived from the create's `Idempotency-Key`. */
  id: string;
  /** The desktop session it is for; null on a machine made before sessions. */
  sessionId: string | null;
  /** The session's title; null when the create sent none. */
  title: string | null;
  /** The title, cut to 120 characters, or `Cloud`. */
  name: string;
  provider: "boat";
  /** Nexus chooses it (`NEXUS_CLOUD_DEFAULT_SIZE`). */
  size: CloudMachineSize;
  state: CloudMachineState;
  error: string | null;
  createdAt: string;
  sleepingSince: string | null;
  /** Null unless Gateway put it to sleep; a wake clears it. */
  stopReason: CloudStopReason | null;
  /** While `queued`: the earliest time Gateway tries to start it. Null otherwise. */
  queuedUntil: string | null;
  /** The machine's host is connected to Gateway, so `GET /v1/cloud/machines/{id}/connect` can open a stream. */
  hostOnline: boolean;
}

/** `GET /v1/cloud/machines`: the caller's live machines, one for each session at most. */
export interface CloudMachineList {
  machines: CloudMachine[];
}

/** `POST /v1/cloud/machines`: a session that has a live machine gets that machine back (200); a machine that waits in the queue is 202. */
export interface CreateCloudMachine {
  /** The desktop session (`t_<uuid>`): 1 to 128 letters, digits, `_` and `-`. */
  sessionId?: string;
  /** The session's title; Gateway keeps its first line. */
  title?: string;
  /** @deprecated Read and ignored: the name is the title. */
  name?: string;
  /** @deprecated Read and ignored: Nexus sizes every machine. */
  size?: CloudMachineSize;
  /** The repository the session clones (a URL or GitHub `owner/name`): a GitHub one may start the machine from the
   * organization's saved copy of it, when the `x-nexus-github-token` header proves the person may read it. */
  repo?: string;
}

/** `DELETE /v1/cloud/machines/{id}`. */
export interface CloudMachineDeleted {
  id: string;
  deleted: boolean;
}

/** `POST /v1/cloud/machines/{id}/access`: this device's SSH public key. */
export interface CloudAccessRequest {
  publicKey: string;
}

/** Where to reach a cloud machine over SSH, and the host key to pin. */
export interface CloudAccess {
  host: string;
  port: number;
  user: string;
  hostKey: string;
}

/** A cloud agent session's lifecycle state (`GET /v1/sessions`). */
export type SessionState = "starting" | "running" | "stopped" | "failed";

/** Who started a cloud agent session (`POST /v1/sessions`). */
export interface SessionStartedBy {
  kind: "person" | "assistant_conversation" | "room" | "session";
  id: string;
}

/** The runner that currently holds a cloud agent session's lease. */
export interface SessionRunner {
  runnerId: string;
  fencingToken: number;
  leaseExpiresAt: string;
}

/** `POST /v1/sessions`: start a cloud coding agent session on a GitHub repository. */
export interface CreateSession {
  repo?: string;
  projectId?: string;
  /** A coding agent id. Absent, empty or `auto`: the model picks it (older Gateways require it). */
  agent?: string;
  /** A catalog model or the agent's own model name. Required when `agent` is absent. */
  model?: string;
  prompt: string;
  title?: string;
  idempotencyKey?: string;
  startedBy?: SessionStartedBy;
}

/** `GET /v1/sessions/{id}`, `POST /v1/sessions`, `POST /v1/sessions/{id}/messages`, `POST /v1/sessions/{id}/stop`. */
export interface Session {
  id: string;
  repo: string;
  agent: string;
  model: string | null;
  title: string | null;
  state: SessionState;
  threadId: string | null;
  error: string | null;
  startedBy: SessionStartedBy;
  createdAt: string;
  updatedAt: string;
  stoppedAt: string | null;
  machine: CloudMachine | null;
  fencingToken: number;
  runner: SessionRunner | null;
}

/** `GET /v1/sessions`: the caller's cloud agent sessions, newest first. */
export interface SessionList {
  sessions: Session[];
}

/** `POST /v1/sessions/{id}/messages`: a follow-up message sent to a running or sleeping cloud agent session. */
export interface SessionMessage {
  text: string;
}

/** One workbench event stored in Gateway's cloud machine history (`GET /v1/cloud/machines/{id}/history/{threadId}/events`). */
export interface CloudHistoryEvent {
  seq: number;
  at: number;
  body: string | null;
  omittedBytes?: number;
}

/** A page of stored workbench events from a cloud machine's history. */
export interface CloudHistoryEventPage {
  machineId: string;
  threadId: string;
  enabled: boolean;
  firstSeq: number | null;
  lastSeq: number | null;
  events: CloudHistoryEvent[];
}

/** `POST /v1/cloud/machines/{id}/links`: a port on the machine, or its desktop stream. */
export interface CloudLinkRequest {
  kind: "port" | "desktop";
  port?: number;
}

/** A link to a cloud machine; `expiresAt` is null when it lasts until the machine sleeps. */
export interface CloudLink {
  url: string;
  expiresAt: string | null;
}

/** `POST /v1/cloud/machines/{id}/sleep-key`: a provider key that may only read and stop this machine (null when refused). */
export interface CloudSleepKey {
  key: string | null;
  apiUrl: string;
  providerId: string;
}

/** An agent link (Rust gateway only): the credential of an agent that acts for one person. Never the token. */
export interface AgentLink {
  /** `agl_…` */
  id: string;
  /** The person the agent acts for. */
  userId: string;
  /** What the agent is, as a slug (`slack`). */
  agent: string;
  label: string;
  /** What the person granted. The agent holds it cut down to the person's role now. */
  grant: string[];
  /** The first characters of the token, to recognise it in a list. */
  prefix: string;
  createdAt: string;
  expiresAt: string;
  revokedAt?: string;
  /** Written at most once an hour. */
  lastUsedAt?: string;
}

/** `GET /v1/agent-links` */
export interface AgentLinkList {
  agentLinks: AgentLink[];
  /** The next page's cursor, while more items follow. Only when `limit` or `cursor` was sent. */
  nextCursor?: string;
}

/** `POST /v1/agent-links` */
export interface CreateAgentLinkRequest {
  /** A slug of 1 to 40 characters: lowercase letters, digits and hyphens. */
  agent: string;
  /** 1 to 80 characters. */
  label: string;
  /** `models.use` and `spend.read_own` can be granted. `models.use` by default. */
  grant?: string[];
  /** In the future, at most 365 days from now. 90 days from now by default. */
  expiresAt?: string;
  /** Check everything and write nothing. */
  dryRun?: boolean;
}

/** `POST /v1/agent-links`: the link as it was made or, with `dryRun`, as it would be made. */
export interface CreatedAgentLink extends AgentLink {
  /** `nxo_…`, in this answer only. None with `dryRun`. */
  token?: string;
  dryRun: boolean;
}

/** Rust gateway only, with `NEXUS_OAUTH_CLIENT_TIERS=on`: an OAuth client as its author sees it. */
export interface DeveloperClient {
  /** `nxc_…`: the `client_id` a fork sends when it signs a person in. */
  id: string;
  name: string;
  tier: "official" | "registered_fork" | "unregistered";
  /** What a token of this client may hold: its own list, or every third-party scope when it has none. */
  allowedScopes: string[];
  /** Absent: the tier's default limit. */
  rateLimitPerMinute?: number;
  /** Absent: the tier's default limit. */
  maxConcurrentPerUser?: number;
  createdAt: string;
  updatedAt: string;
  revokedAt?: string;
  revokedReason?: string;
}

/** `GET /v1/developer/clients` */
export interface DeveloperClientList {
  /** Your forks, revoked ones included, oldest first. */
  clients: DeveloperClient[];
  /** The scopes a fork may ask for, in their canonical order. */
  grantableScopes: string[];
}

/** `POST /v1/developer/clients` */
export interface RegisterDeveloperClientRequest {
  /** 1 to 100 characters. */
  name: string;
  /** Each one of `grantableScopes`. Absent: all of them. */
  allowedScopes?: string[];
}

/** `POST /v1/developer/clients/{id}/revoke` */
export interface RevokeDeveloperClientRequest {
  /** 1 to 500 characters. Absent: `revoked by owner`. */
  reason?: string;
}

/** W4.3's proactivity levels: whether the agent may start something the person did not ask for. */
export type AgentProactivity = "always" | "ask_first" | "never";

/** A tone preset; `custom` leaves the tone to the instructions. */
export type AgentTone = "calm" | "warm" | "direct" | "lively" | "custom";

/**
 * `GET /v1/agents/profiles` (Rust gateway only): one agent's profile. `owner: "person"` is a personal agent;
 * `owner: "team"` is a company agent, on that team's budget and policy. `tools: null` means every tool the link holds.
 */
export interface AgentProfile {
  id: string;
  owner: "person" | "team";
  ownerId: string;
  teamSlug?: string;
  name: string;
  avatar: string | null;
  tone: AgentTone | null;
  greeting: string | null;
  language: string | null;
  labels: string[];
  instructions: string;
  tools: string[] | null;
  proactivity: AgentProactivity;
  /**
   * A17's controls, each 0 to 100: how much it says (brief to detailed), how formal it is (casual to
   * formal), and how much humour it uses (none to some). `null`: the tone's own (`personality.ts`). An older Gateway
   * leaves them out.
   */
  length?: number | null;
  formality?: number | null;
  humour?: number | null;
  /** Emoji in its answers. `null`: off. */
  emoji?: boolean | null;
  /** Shorter by text and push than in the app. `null`: on. */
  shortTexts?: boolean | null;
  createdBy: string;
  createdAt: string;
  updatedAt: string;
}

export interface AgentProfileList {
  data: AgentProfile[];
}

/**
 * `PUT /v1/agents/profiles/{id}`: replaces every field but the owner, so each of W8's is required. The personality
 * fields came later: one left out keeps its value, so an older client never resets it.
 */
export type SetAgentProfileRequest = Pick<
  AgentProfile,
  "name" | "avatar" | "tone" | "greeting" | "language" | "labels" | "instructions" | "tools" | "proactivity" | "length" | "formality" | "humour" | "emoji" | "shortTexts"
>;

/** `POST /v1/agents/profiles`: without `team`, the caller's personal agent. An omitted field takes its default. */
export type CreateAgentProfileRequest = Pick<SetAgentProfileRequest, "name"> & Partial<Omit<SetAgentProfileRequest, "name">> & { team?: string };

/** Who answered, as a surface shows it: the profile's name, and its emoji or https image when it has one. */
export interface AgentIdentity {
  name: string;
  avatar?: string;
}

/** `POST /v1/slack/installs` (Rust gateway, owners and admins): open `authorizeUrl` and approve the app in Slack. */
export interface SlackInstallStarted {
  /** `chw_…`: the binding's id once the install completes. */
  installId: string;
  authorizeUrl: string;
  /** When the install link stops working (an hour). */
  expiresAt: string;
}

/** A Slack workspace bound to the organization. Never the bot token. */
export interface ChatWorkspace {
  /** `chw_…`, also the install's id. */
  id: string;
  platform: "slack";
  /** Slack's workspace id (`T0…`). */
  teamId: string;
  teamName?: string;
  enterpriseId?: string;
  botUserId?: string;
  /** The Slack user who approved the app. */
  approvedBySlackUser?: string;
  scopes: string[];
  /** `pending` until the person who started the install confirms it. */
  state: "pending" | "active" | "revoked";
  installedBy: string;
  confirmedBy?: string;
  createdAt: string;
  /** A pending binding confirms only before this. */
  expiresAt: string;
  confirmedAt?: string;
  revokedAt?: string;
}

/** `POST /v1/slack/installs/{id}/complete`: the query of Slack's redirect, as Nexus UI's install page received it. */
export interface CompleteSlackInstallRequest {
  code: string;
  state: string;
}

/** `GET /v1/slack/installs/{id}`, `POST …/complete` and `POST …/confirm`: the binding (a 404 before the install completes). */
export interface SlackInstall {
  installId: string;
  status: "pending" | "active" | "revoked";
  workspace?: ChatWorkspace;
}

/** `GET /v1/slack/workspaces`: the organization's pending and active bindings. */
export interface ChatWorkspaceList {
  workspaces: ChatWorkspace[];
}

/** `DELETE /v1/slack/workspaces/{id}` */
export interface ChatWorkspaceRemoved {
  removed: boolean;
  /** Whether Gateway uninstalled the app from the workspace (Slack's `apps.uninstall`). */
  uninstalled: boolean;
  /** Whether the bot token left the vault. */
  credentialRevoked: boolean;
}

/**
 * `GET /v1/agents/memory/{collection}/{key}` (Rust gateway only): one row of the caller's agent memory. The
 * value is the agent's sealed text; Gateway never opens it. A later write names `version` in `If-Match`.
 */
export interface AgentMemoryRow {
  collection: string;
  key: string;
  value: string;
  /** It never repeats, also after a delete. */
  version: number;
  updatedAt: string;
}

/** `PUT /v1/agents/memory/{collection}/{key}`, with `If-Match: <version>` or `If-None-Match: *`. */
export interface SetAgentMemoryRequest {
  /** The sealed text, at most 256 KiB. */
  value: string;
}

/** `PUT /v1/agents/memory/{collection}/{key}`: the row's new version. A lost race is a 409 instead. */
export interface AgentMemoryWritten {
  collection: string;
  key: string;
  version: number;
  /** True when the write made the row. */
  created: boolean;
  updatedAt: string;
}

/** `DELETE /v1/agents/memory/{collection}/{key}` */
export interface AgentMemoryDeleted {
  collection: string;
  key: string;
  /** The version the row had. */
  version: number;
}

/** One key of `GET /v1/agents/memory/{collection}`, without its value. */
export interface AgentMemoryEntry {
  key: string;
  version: number;
  updatedAt: string;
}

/** `GET /v1/agents/memory/{collection}?after=&limit=`: one page of keys, in byte order. */
export interface AgentMemoryKeys {
  collection: string;
  keys: AgentMemoryEntry[];
  /** The `after` of the next page; null on the last. */
  next: string | null;
}

/** `DELETE /v1/agents/memory`: every row of the caller in the organization. */
export interface AgentMemoryCleared {
  deleted: number;
}

export interface MemberSummary {
  userId: string;
  clerkUserId?: string;
  email?: string;
  name?: string;
  role: Role;
  joinedAt: string;
  /** Settled spend attributed to the person; shown to owners and admins, and to each person for themselves. */
  spentUsd?: number;
  calls?: number;
}

/** `GET /v1/orgs/members` */
export interface MembersResponse {
  members: MemberSummary[];
}

/** `GET` and `PUT /v1/presence/settings`, and the answer of `POST /v1/presence`. */
export interface PresenceSettings {
  /** Whether others see when you are online. On by default. */
  showOnline: boolean;
}

/** One member in `GET /v1/presence`. */
export interface PresenceMember {
  userId: string;
  name: string | null;
  email: string | null;
  role: Role;
  /** Seen within the online window. You are always online to yourself. */
  online: boolean;
  /** The last heartbeat, only while it is within the online window. */
  lastSeenAt: string | null;
  /** What they are doing, while online; null otherwise. */
  activity: PresenceActivity | null;
}

/**
 * What an online member is doing, as you may see it. `chatting`: in a chat, never which one. `page`: on a
 * Nexus Page, with `pageId` and `title` only when you may open it. `agents`: `count` of their own running agent
 * sessions, which Gateway reads. `idle`: nothing to say.
 */
export interface PresenceActivity {
  kind: "chatting" | "page" | "agents" | "idle";
  pageId?: string | null;
  title?: string | null;
  count?: number;
}

/** What Nexus UI reports with a heartbeat: no chat title, no path, no repository. Gateway counts agents itself. */
export interface ReportedActivity {
  kind: "chatting" | "page" | "idle";
  /** With `page`. */
  pageId?: string;
}

/** `POST /v1/presence`'s body. */
export interface PresenceHeartbeat {
  activity?: ReportedActivity;
}

/** `GET /v1/presence`: every member, online ones first, and your own setting. */
export interface PresenceList {
  members: PresenceMember[];
  /** How long after a heartbeat a member still counts as online. */
  onlineWindowSeconds: number;
  showOnline: boolean;
}

/** A removed member (ADR 0006): an owner or admin recovers them until `recoverableUntil`; after it the removal is final. */
export interface RemovedMember {
  userId: string;
  email?: string;
  name?: string;
  role: Role;
  removedAt: string;
  /** The user id who removed them; absent when Clerk did. */
  removedBy?: string;
  recoverableUntil: string;
  recoverable: boolean;
}

/** `GET /v1/orgs/members/removed` (Rust only) */
export interface RemovedMembersResponse {
  members: RemovedMember[];
}

/** `POST /v1/orgs/members/{userId}/recover` (Rust only): the member as they come back, and their teams' slugs. */
export interface MemberRecovered {
  userId: string;
  email?: string;
  role: Role;
  teams: string[];
  dryRun: boolean;
  recovered: boolean;
}

/**
 * `POST /v1/auth/account/delete` (Rust only): what deleting your own account did, or on a dry run would do. Your
 * personal organization closes with it, and you leave every company organization you are in (`left`).
 */
export interface AccountDeleted {
  userId: string;
  dryRun: boolean;
  /** False on a dry run; true once the account is deleted. */
  deleted: boolean;
  /** Your personal organization's id; absent when you have none. */
  personalOrg?: string;
  left: { id: string; name: string }[];
  /** Whether your personal organization's plan and cards at the payment provider end with it. */
  planEnded: boolean;
  /** Whether you must sign in again first: only a sign-in from the last ten minutes deletes your account. */
  signInNeeded: boolean;
  /**
   * Your personal organization's usage that its card has not paid, as a decimal dollar string. On a dry run, what the
   * deletion charges to the card on file first. Once deleted, what that charge did not collect, which Nexus writes off.
   * Absent when nothing is unpaid.
   */
  unpaidUsageUsd?: string;
  /** Your personal organization's Nexus credits that the deletion loses, as a decimal dollar string. Nothing refunds them. */
  creditsLostUsd?: string;
}

/**
 * `GET /v1/teams/pending?by=person`: someone in teams by email who has not joined the organization yet, with every
 * team they are pending in (leads first) and the home team they get when they join.
 */
export interface PendingPerson {
  email: string;
  teams: { id: string; slug: string; name: string; level?: string; role: TeamRole; home: boolean }[];
  /** The slug of the team that becomes their default when they join. */
  home?: string;
  /** When they were first added to a team. */
  since: string;
}

export interface PendingPeopleResponse {
  people: PendingPerson[];
  /** Whether this Core can send invitations (`CLERK_SECRET_KEY` set on Core). */
  invitations: "available" | "not_configured";
}

/** `POST /v1/orgs/invitations`: explicit emails, or everyone pending in a team (`pending: true`). */
export interface InvitationsRequest {
  emails?: string[];
  pending?: boolean;
  /** Default `member`. */
  role?: "member" | "admin";
  /** Report who would be invited; send nothing. */
  dryRun?: boolean;
  /** Required to send more than one: the number of invitations you confirmed (409 `count_changed` if it differs). */
  expect?: number;
}

/** `not_signed_in`: in the Clerk organization with no Gateway user yet; they need to sign in once, not an invitation. */
/** `removed`: removed from the organization inside the recovery window; an owner or admin recovers them instead. */
export type InvitationStatus = "would_invite" | "sent" | "already_member" | "not_signed_in" | "already_invited" | "removed" | "failed";

export interface InvitationResult {
  email: string;
  status: InvitationStatus;
  invitationId?: string;
  /** Clerk's message when it failed, or why it was skipped. */
  message?: string;
}

export interface InvitationsResponse {
  dryRun: boolean;
  role: "member" | "admin";
  /** False on a dry run without `CLERK_SECRET_KEY` on Core: Clerk's members and invitations were not checked. */
  checkedClerk: boolean;
  /** Invitations sent (or, in a dry run, that would be). */
  toSend: number;
  results: InvitationResult[];
  counts: Record<InvitationStatus, number>;
}

/** How people with a verified email on a domain join. `enterprise_sso` is set in Clerk, never through the API. */
export type EnrollmentMode = "automatic_invitation" | "automatic_suggestion" | "manual_invitation" | "enterprise_sso";

/** One verified email domain of the organization. Clerk keeps it (Enhanced B2B add-on); the Rust gateway only. */
export interface OrgDomain {
  /** Clerk's domain id. */
  id: string;
  domain: string;
  enrollmentMode: EnrollmentMode;
  verified: boolean;
  /** Invitations Clerk sent on this domain that nobody accepted yet. */
  pendingInvitations: number;
}

/** `GET /v1/orgs/domains` */
export interface DomainList {
  domains: OrgDomain[];
}

/**
 * `GET /v1/orgs/domain-suggestions` (`NEXUS_ORG_OWNER=core`): a company organization the signed-in person
 * may join, because a Nexus operator assigned `domain` to it and their Google sign-in verified an email there.
 */
export interface DomainSuggestion {
  /** The domain, lower-case. */
  domain: string;
  /** The organization; no `role` or `current`, since the person is not a member yet. Join it with `joinByDomain(org.id)`. */
  org: OrgSummary;
}

/** `GET /v1/orgs/domain-suggestions` */
export interface DomainSuggestionList {
  suggestions: DomainSuggestion[];
}

/** `GET /v1/orgs/invitations` (Rust gateway): one pending invitation Core holds (`NEXUS_ORG_OWNER=core`). */
export interface PendingInvitation {
  /** Core's invitation id. */
  id: string;
  email: string;
  role: "member" | "admin";
  /** The person who invited them, when a person did. */
  invitedBy?: string | null;
  createdAt: string;
  expiresAt: string;
  /** The last send. */
  sentAt?: string | null;
  /** Past `expiresAt`: its link no longer works; resend it. */
  expired: boolean;
}

/** `GET /v1/orgs/invitations` */
export interface PendingInvitationList {
  invitations: PendingInvitation[];
}

/** `GET /v1/orgs/invitations/mine`: one of the signed-in person's own pending invitations, sent to an email their sign-in verified. */
export interface ReceivedInvitation {
  /** Accept it with `acceptInvitationById`. */
  id: string;
  /** Their email it was sent to. */
  email: string;
  orgName: string;
  orgSlug?: string | null;
  role: "member" | "admin";
  /** The inviter's name, else their email. */
  invitedBy?: string | null;
  expiresAt: string;
}

/** `GET /v1/orgs/invitations/mine` */
export interface ReceivedInvitationList {
  invitations: ReceivedInvitation[];
}

/** `POST /v1/orgs/invitations/{id}/resend`: the new link's send, as one invitation's result. */
export interface InvitationResent {
  email: string;
  status: string;
  invitationId?: string | null;
  message?: string | null;
}

/** `POST /v1/orgs` (Rust gateway): a company organization Core alone holds; the caller is its owner. */
export interface CreateOrgRequest {
  /** 1 to 120 characters after trimming. */
  name: string;
  /** A lowercase DNS label that is not a Nexus UI page; Gateway picks one from the name when unset. */
  slug?: string;
}

/** `PATCH /v1/orgs/profile` (Rust gateway): the caller's company organization. `imageUrl: null` removes the logo. */
export interface OrgProfileChange {
  name?: string;
  slug?: string;
  imageUrl?: null;
}

/** `POST /v1/orgs/domains`: a domain the caller has a verified email on. Default `automatic_invitation`. */
export interface AddDomainRequest {
  domain: string;
  enrollmentMode?: Exclude<EnrollmentMode, "enterprise_sso">;
}

/**
 * `POST /v1/ui/session`'s body. The provider-neutral body names the person by `user.provider` and
 * `user.subject`, and Core's organization by `org.id` or `org.slug`, or `personal: true`. The Clerk fields are the Clerk
 * body's, until the cut-over. One body is one or the other: fields of both are 400 `mixed_identity_fields`.
 */
export interface UiSessionRequest {
  user: {
    provider?: string;
    subject?: string;
    email?: string;
    /** The neutral body keeps `email` only when this is `true`. */
    emailVerified?: boolean;
    name?: string;
    /** The provider's organization claim, such as Google's `hd`. */
    idpOrg?: string;
    /** @deprecated The Clerk body only. */
    clerkUserId?: string;
  };
  org?: {
    /** Core's tenant id. */
    id?: string;
    slug?: string;
    /** @deprecated The Clerk body only. */
    clerkOrgId?: string;
    /** @deprecated The Clerk body only. */
    name?: string;
    /** @deprecated The Clerk body only. */
    imageUrl?: string;
  };
  personal?: boolean;
  /** @deprecated The Clerk body only: the role is Core's membership. */
  clerkRole?: string;
  /** @deprecated The Clerk body only. */
  creator?: { clerkUserId?: string; email?: string; name?: string };
  /** `false`: sync only, no token. */
  token?: boolean;
  referral?: string;
}

/** `POST /v1/ui/session`: Nexus UI's server, for a signed-in person. */
export interface UiSessionResponse extends CurrentTenantResponse {
  /** Unset when the call asked for `token: false` (sync only). */
  token?: string;
  expiresAt?: string;
  created: boolean;
}

/** `GET /v1/github/app` */
export interface GithubAppStatus {
  configured: boolean;
  slug?: string;
  /** For owners and admins: the GitHub install link, tied to this org for an hour. */
  installUrl?: string;
}

export interface LinkedRepositorySummary {
  id: string;
  installationId: string;
  fullName: string;
  url: string;
  defaultBranch?: string;
  private: boolean;
  linkedAt: string;
  lastWikiRun?: { id: string; at: string; trigger: "link" | "push" | "manual" | "cadence"; commit?: string; error?: string };
}

/** `GET /v1/github/repositories` */
export interface RepositoriesResponse {
  installations: { id: string; account: string; accountType?: string; repositorySelection?: "all" | "selected"; createdAt: string }[];
  repositories: LinkedRepositorySummary[];
}

/** Who holds a provider key: `team` (a slug or id) or `user` (`me` or a member's id); neither, the organization. */
export interface ProviderKeyHolder {
  team?: string;
  user?: string;
}

export interface ProviderKeySummary {
  provider: string;
  /** A team's or a person's key; absent for the organization's own. */
  scope?: "team" | "user";
  /** The team's or the person's id. */
  scopeId?: string;
  last4: string;
  createdAt: string;
  /** No call uses the key: its provider is retired (OpenRouter) or an admin disabled it; `notice` says which. */
  unused?: true;
  notice?: string;
  /** What the provider answered when the key was checked on add, an admin's disable or remove, or a provider's refusal on a call; absent for a key stored before. */
  state?: ProviderKeyState;
  checkedAt?: string;
  /** The UTC day (`YYYY-MM-DD`) a call last used the key. */
  lastUsedOn?: string;
  /** Each key stored for the provider, oldest first; only on `GET /v1/keys/providers/{provider}`. */
  versions?: ProviderKeyVersion[];
}

/** A provider key's check, or an admin's disable or remove: `rejected` keys are never stored by an add. */
export type ProviderKeyState = "valid" | "rejected" | "unreachable" | "disabled" | "removed";

/** One key stored for a provider: who added it, when, and its last four characters, never the value. */
export interface ProviderKeyVersion {
  version: number;
  last4: string;
  addedAt: string;
  addedBy?: string;
}

/** A provider key's replace, disable, enable or remove; `changed: false` when it was already so. */
export interface ProviderKeyChange {
  dryRun: boolean;
  changed: boolean;
  providerKey: ProviderKeySummary;
}

/** `GET /v1/providers`: one provider the Gateway calls, from its catalog entry. */
export interface ProviderView {
  /** Its id in stored keys, spend records and the policy's `providers` lists (`anthropic`). */
  id: string;
  name: string;
  /** ISO 3166-1 alpha-2: where it is headquartered. */
  country: string;
  /** Whether an organization may bring its own key for it. */
  byok: boolean;
  status: "active" | "inactive";
  /** How many listed models it hosts. */
  models: number;
  /** What a model on it costs beyond its input and output prices. Absent from an older Gateway. */
  rates?: ProviderRates;
  /** The host of the provider's public API (`api.anthropic.com`). Absent from an older Gateway. */
  apiHost?: string;
  /** Its models' API, or typed decisions only (`POST /v1/decisions`). Absent from an older Gateway. */
  serving?: "model_api" | "decisions";
  /** With a key, on a BYOK provider only: the organization's own key. `none`: none stored, or disabled. */
  key?: "none" | "valid" | "rejected";
  /** With a key: whether the organization's provider policy lets calls use it. Absent from an older Gateway. */
  enabled?: boolean;
}

/** A provider's rates, where a model states no price of its own. `null` is a rate it does not offer. */
export interface ProviderRates {
  /** A cache read, as a share of the model's input price; `null` where each model has its own. */
  cacheReadShare: number | null;
  /** Multiples of the input price by how long the cache entry lives, or the model's own cache-write price. */
  cacheWrite: { rule: "multipliers"; fiveMinutes: number; oneHour: number } | { rule: "model_rate" };
  /** One hosted web search, in dollars, as a decimal string; `null` where the Gateway runs none. */
  webSearchUsd: string | null;
}

export interface ProviderList {
  object: "list";
  data: ProviderView[];
}

export interface ChatMessage {
  role: "system" | "user" | "assistant";
  content: string;
  /** In an answer: the tools the model asks to call (OpenAI's `tool_calls`, functions only); `content` may then be empty. */
  tool_calls?: ChatToolCall[];
}

/** One tool call the model asks for. `arguments` is JSON text, as the model wrote it: parse it before you trust it. */
export interface ChatToolCall {
  id: string;
  type: "function";
  function: { name: string; arguments: string };
}

/** A tool offered to the model (OpenAI's `tools`, functions only). `parameters` is a JSON Schema object. */
export interface ChatTool {
  type: "function";
  function: { name: string; description?: string; parameters: Record<string, unknown> };
}

/**
 * A message of a call that offers tools: a plain message, the model's turn that called tools, or a `tool` turn with the
 * result of one call. The Rust gateway translates them per provider (`nexus-types::api::completions`).
 */
export type ToolChatMessage =
  | ChatMessage
  | { role: "assistant"; content: string; tool_calls: ChatToolCall[] }
  | { role: "tool"; tool_call_id: string; content: string };

/** `POST /v1/chat/completions` with tools (Rust gateway only; the frozen TypeScript gateway refuses `tool` turns). */
export interface ToolChatCompletionRequest extends Omit<ChatCompletionRequest, "messages"> {
  messages: ToolChatMessage[];
  tools?: ChatTool[];
  tool_choice?: "auto" | "none" | "required";
  parallel_tool_calls?: boolean;
}

export interface ChatCompletionRequest {
  model: string;
  messages: ChatMessage[];
  max_tokens?: number;
  temperature?: number;
}

export interface ChatCompletionResponse {
  id: string;
  object: "chat.completion";
  created: number;
  model: string;
  /** `finish_reason`: `stop`, `length`, or (Rust gateway) `refusal` when the provider declined to answer. */
  choices: { index: number; message: ChatMessage; finish_reason: string }[];
  usage: { prompt_tokens: number; completion_tokens: number; total_tokens: number };
  /**
   * Nexus extension: what the call cost and which product it was attributed to; an internal decision: the model asked for, and for
   * `nexus/auto` / `nexus/default` what it resolved from and why (`model` is always the model that ran). `refusal`
   * (Rust gateway): the provider's words when `finish_reason` is `refusal`.
   */
  nexus: { cost_usd: number; product: string; capability?: string; provider: string; requested?: string; resolved_from?: "nexus/auto" | "nexus/default"; notes?: string[]; funded_by?: FundedBy; refusal?: string; router?: RouterSummary };
}

/**
 * One `chat.completion.chunk` of a streamed `POST /v1/chat/completions` (Rust gateway): the first names the
 * model, each later one carries a piece of the text, and the last has no choices, only `usage` and `nexus`.
 */
export interface ChatCompletionChunk {
  id: string;
  object: "chat.completion.chunk";
  created: number;
  model: string | null;
  choices: { index: number; delta: { role?: "assistant"; content?: string; tool_calls?: ChatToolCallDelta[] }; finish_reason: string | null }[];
  usage?: ChatCompletionResponse["usage"];
  nexus?: ChatCompletionResponse["nexus"];
}

/**
 * A piece of a tool call in a streamed answer (`nexus-routing/.../chunks.rs` `delta`): the first piece of call `index`
 * carries its `id`, `type` and `function.name`; later pieces carry more of `function.arguments`.
 */
export interface ChatToolCallDelta {
  index: number;
  id?: string;
  type?: "function";
  function?: { name?: string; arguments?: string };
}

/**
 * Who paid the provider for a model call: the organization's own key, Nexus's own key (Nexus's own organizations,
 * an internal decision), a Nexus subsidy on a Nexus-supplied key, or the organization's prepaid Nexus credits on a
 * Nexus-supplied key.
 */
export type FundedBy = "own_key" | "nexus_key" | "nexus_subsidy" | "nexus_credit" | "nexus_invoiced";

/**
 * Rust gateway: the auto router's pick, on an answer it routed or a chat session held. The answer also
 * carries `x-nexus-router` with the version. Its cost estimates stay on the spend record.
 */
export interface RouterSummary {
  version: string;
  class?: "light" | "standard" | "heavy";
  reason?: "cheapest" | "below_floor" | "catalog_order";
  held: boolean;
  default_model?: string;
}

export interface SpendEntry {
  id: string;
  /** The person the call is attributed to. */
  userId?: string;
  /** The `nexus claude` / `nexus codex` session an agent gateway call belongs to. */
  agentSession?: string;
  /** an internal decision: the team the call is attributed to, and its slug at the time. */
  teamId?: string;
  teamSlug?: string;
  /** Product slug, or `core` for direct API calls. */
  product: string;
  capability?: string;
  /**
   * Rust gateway: the caller's key by id, never its secret. A token id (`key_…`), a module's scoped key
   * (`scoped:<product>/<capability>/smk_…`, as `nexus keys revoke` takes it), `ui` or a system caller's name. Unset
   * on calls from before it was recorded: an unknown key.
   */
  keyId?: string;
  /** Rust gateway: the kind of key `keyId` names, as the audit log's actor names it. */
  keyKind?: AuditActorKind;
  model: string;
  provider: string;
  status: "settled" | "failed" | "refused" | "released";
  promptTokens: number;
  completionTokens: number;
  costUsd: number;
  /** Rust gateway: the exact decimal string of the number beside it (`"12.245000"`). Prefer it; the number is deprecated (root rule 9). */
  costUsdDecimal?: string;
  reason?: string;
  /** A call a spend limit refused: the level (`organization`, `team` or `person`), its limit and how far over it would have gone. The limit of a team cap or of the organization is never shown (it may sit above the caller). Each amount has its decimal twin. */
  overspendPrevented?: {
    level: "organization" | "team" | "person" | "product";
    limitUsd?: number;
    limitUsdDecimal?: string;
    overUsd: number;
    overUsdDecimal?: string;
    period?: string;
  };
  /** Rust gateway: who paid the provider. Unset on a refusal, a typed decision and calls from before it was recorded. */
  fundedBy?: FundedBy;
  /** an internal decision: the organization whose stored provider key paid a subsidized call (`NEXUS_SUBSIDY_ORG`). */
  fundingOrg?: string;
  /** Rust gateway: Nexus's rate in basis points (500 is 5%) on a call Nexus paid the provider for (`nexus_key`, `nexus_subsidy`, `nexus_credit` or `nexus_invoiced`), as it was when the call settled. `costUsd` stays the provider's cost; credits are charged the cost plus the rate. */
  markupBps?: number;
  /** Rust gateway: the rate a Nexus subsidy was charged at for this call; the subsidy spent `costUsd` plus it. Unset on calls the subsidy did not pay, and on subsidized calls from an older Gateway, which spent `costUsd` alone. */
  subsidyMarkupBps?: number;
  /** Rust gateway: where an Anthropic call ran, as the provider reported it. Unset on other providers and on calls from before it was recorded. */
  inferenceGeo?: "us" | "global";
  at: string;
}

/** an internal decision (Rust gateway): an organization's Nexus subsidy (a personal organization's starting credit), Nexus's money. */
export interface SubsidyView {
  grantedUsd: number;
  /** What calls cost the subsidy: each call's provider cost plus Nexus's rate, as for credits. */
  spentUsd: number;
  reservedUsd: number;
  remainingUsd: number;
  /** Rust gateway: the same amounts as exact decimal strings. Prefer them; the numbers are deprecated. */
  grantedUsdDecimal?: string;
  spentUsdDecimal?: string;
  reservedUsdDecimal?: string;
  remainingUsdDecimal?: string;
  /** What was left of the free start when it expired, 30 days after its grant (pricing v2.0). Absent before. */
  expiredUsd?: number;
  expiredUsdDecimal?: string;
  fundedBy: "nexus";
  /** Whether this Gateway pays model calls of an organization with no key of its own from the subsidy. */
  usableForModelCalls: boolean;
}

/** Rust gateway: the organization's prepaid Nexus credits, its own money, never expiring. Decimal dollar strings. */
export interface CreditsView {
  purchasedUsd: string;
  /** What calls cost the credits: each call's provider cost plus `markupBps`. */
  spentUsd: string;
  reservedUsd: string;
  /** Bought, less spent, held and reversed: below zero after a refund or a lost dispute took back credit already spent. */
  remainingUsd: string;
  /** What refunds, lost disputes and operators took back; only once something was. */
  reversedUsd?: string;
  markupBps: number;
}

export interface SpendResponse {
  /** Owners and admins: the organization's budget, spent and reserved. The Rust gateway sends a member none of them. */
  budgetUsd?: number;
  spentUsd?: number;
  reservedUsd?: number;
  /** Owners and admins (Rust gateway, Ledger M1.4): the budget's period, and the spend it counts in the current one. */
  budgetPeriod?: "day" | "week" | "month";
  periodSpentUsd?: number;
  /** The organization's budget left this period (owners and admins), or what is left for you (members): the next 402's figure. */
  remainingUsd: number;
  /** Rust gateway: each money number of this answer has a `<name>Decimal` twin, its exact decimal string. Prefer the twins; the numbers are deprecated (root rule 9). */
  budgetUsdDecimal?: string;
  spentUsdDecimal?: string;
  reservedUsdDecimal?: string;
  periodSpentUsdDecimal?: string;
  remainingUsdDecimal?: string;
  /** Members: the period of the cap that binds what is left for you (`2026-10-17`, `2026-W43` or `2026-10`). */
  period?: string;
  /** When that cap's period ends; for owners and admins, when the budget's period ends (Rust gateway). */
  resetsAt?: string;
  byProduct: { product: string; capability?: string; costUsd: number; costUsdDecimal?: string; calls: number }[];
  /** an internal decision: spend per person, for owners and admins. */
  byPerson?: { userId: string; email?: string; costUsd: number; costUsdDecimal?: string; calls: number }[];
  /** `org`: the whole org's calls; `own`: only yours (members). */
  scope?: "org" | "own";
  recent: SpendEntry[];
  /** an internal decision (Rust gateway): the organization's Nexus subsidy, when it has one. */
  subsidy?: SubsidyView;
  /** an internal decision: what calls the subsidy paid cost, in your scope. */
  subsidizedUsd?: number;
  subsidizedUsdDecimal?: string;
  /** an internal decision, the funding organization (owners and admins): what its stored keys paid for others; never its own spend. */
  subsidiesPaid?: { usd: number; usdDecimal?: string; calls: number; organizations: number };
  /** Owners and admins: the organization's Nexus credits, once it bought some. */
  credits?: CreditsView;
  /** Pricing v2.0 (owners and admins): paying after the fact is on. It says nothing of a failed charge or a reached limit: read `nexusPays`. */
  payAfterTheFact?: boolean;
  /** Every member: whether Nexus can pay the next call, and why not. Absent where Nexus pays for no calls, or from an older Gateway. */
  nexusPays?: NexusPays;
}

/** Why Nexus cannot pay the organization's next call (`GET /v1/spend`'s `nexusPays.reason`). */
export type NexusPaysReason =
  | "free_start_used_up"
  | "free_start_expired"
  | "credits_out"
  | "usage_limit_reached"
  | "charge_failed"
  | "card_needs_confirming"
  | "card_disputed"
  | "past_due"
  | "billing_off"
  | "charge_processing";

/**
 * Whether Nexus can pay the organization's next small call to a model the free start pays for: who pays,
 * or why nobody does, and when that changes by itself (the free start's deadline, or the next month for the usage
 * limit). A pricier model or a larger call can still be refused.
 */
export interface NexusPays {
  pays: boolean;
  payer: "contract" | "free_start" | "subsidy" | "credits" | "card" | null;
  reason: NexusPaysReason | null;
  changesAt: string | null;
  /** While the card pays: how much of this UTC month's usage limit it paid, in whole percent. Absent from an older Gateway. */
  limitUsedPercent?: number | null;
}

/** A card on file as Stripe describes it (Rust gateway): never the card number. */
export interface CardSummary {
  brand: string;
  last4: string;
  expMonth: number;
  expYear: number;
}

/**
 * A payment method of any type as Stripe describes it (Rust gateway): its type and what that type shows, never a full
 * number. A field the type does not have is left out: a card has a brand, its last 4 and an expiry; a bank account a
 * bank name and its last 4; Link nothing more.
 */
export interface PaymentMethodSummary {
  /** Stripe's type, such as `card`, `us_bank_account` or `link`. Other types can appear. */
  type: string;
  brand?: string;
  bankName?: string;
  last4?: string;
  expMonth?: number;
  expYear?: number;
}

/** The oldest month Stripe could not collect, and when model calls stop for it. */
export interface PastDue {
  /** ISO time the month first failed. */
  since: string;
  /** `YYYY-MM`. */
  period: string;
  /** ISO time: `since` plus the grace. From then until an owner pays, model calls answer 402 `payment_past_due`. Null: no gate. */
  blockedFrom: string | null;
  /** Stripe's page that pays the month (its hosted invoice); null when Stripe gave none. Absent from an older Gateway. */
  invoiceUrl?: string | null;
}

/** `GET /v1/billing` (Rust gateway, owners): the payment method on file, how the organization pays (pricing v2.0) and the Stripe mode. Never Stripe's customer id. */
export interface BillingResponse {
  /** This Gateway accepts Stripe shared payment tokens for purchases, without a saved card. Currently test mode only. */
  sharedPaymentTokens?: boolean;
  /** The mode of Gateway's Stripe key. */
  mode: "test" | "live";
  /** `null` until the first card setup starts; `creating` until it finishes. */
  customer: { state: "creating" | "ready" } | null;
  /** The payment method on file when it is a card; `null` for none or for another type. */
  paymentMethod: CardSummary | null;
  /** The payment method on file, of any type. Absent from a Gateway older than that. */
  defaultPaymentMethod?: PaymentMethodSummary | null;
  /** The organization's plan; `null` when it holds none (pricing v2.0 sells plans only where `NEXUS_PLANS` is set). */
  plan?: BillingPlan | null;
  /** This month's plan allowances, when available. Absent on older Gateways. */
  planAllowance?: PlanAllowance;
  /** Pricing v2.0: the free start Nexus gave once, and what is left; `null` when it gave none. */
  freeStart?: FreeStart | null;
  /** Available referral and promotional credit, excluding the welcome free start. Absent on older Gateways. */
  promotionalCreditUsd?: string;
  /** The organization's prepaid Nexus credits; `null` before it buys any. Absent from an older Gateway. */
  credits?: CreditsView | null;
  /** The organization's auto top-up of Nexus credits; `null` before an owner sets it. Absent from an older Gateway. */
  autoTopup?: AutoTopup | null;
  /** Pricing v2.0: paying after the fact; `null` before an owner turns it on. */
  afterTheFact?: AfterTheFact | null;
  /** Pricing v2.0: the numbers a billing page shows, so no client writes a price of its own. */
  pricing?: BillingPricing;
  /** `null` while no month of this mode is unpaid after Stripe tried to collect it. */
  pastDue: PastDue | null;
  /** A change to Nexus pricing and the Customer Terms, until 30 days past its day. Absent from an older Gateway. */
  pricingChange?: PricingChange | null;
}

/** The day a change to Nexus pricing and the Customer Terms takes effect, `YYYY-MM-DD` at 00:00 UTC. */
export interface PricingChange {
  effectiveOn: string;
}

export interface PlanAllowance {
  /** The next usage month starts at this UTC timestamp. */
  resetAt: string;
  /** Decimal USD. Null until the allowance is granted. */
  ai: { includedUsd: string; usedUsd: string; leftUsd: string } | null;
  /** Default-size hours. Null when plan cloud metering is unavailable. */
  cloud: { includedHours: number; usedHours: number; leftHours: number } | null;
}

/** Pricing v2.0: what Nexus gave once, and what is left. Decimal dollar strings. */
export interface FreeStart {
  grantedUsd: string;
  remainingUsd: string;
  /** When the free start expires, or expired: 30 days after its grant. `null` when there is no free start; absent from an older Gateway. */
  expiresAt?: string | null;
  /** What was left of the free start when it expired; `null` before. Absent from an older Gateway. */
  expiredUsd?: string | null;
}

/** Pricing v2.0's numbers, from Gateway. Decimal dollar strings. */
export interface BillingPricing {
  /** The minimum final charge when paying after the fact turns off. Older Gateways omit it. */
  minFinalChargeUsd?: string;
  /** Nexus's rate on AI it runs past the free start and a plan's allowance: 500 is provider price + 5%. */
  markupBps: number;
  minPurchaseUsd: string;
  /** Paying after the fact, the card is charged each time the unpaid usage reaches this. */
  thresholdUsd: string;
  /** The monthly usage limit a new after-the-fact setting starts at. */
  defaultLimitUsd: string;
  /** The most a usage limit may be now for this organization: $100 in its first week. Absent from an older Gateway. */
  maxLimitUsd?: string;
  /** The free start pays only for a model whose cheapest host charges at most this per million output tokens. Absent from an older Gateway. */
  freeStartMaxOutputUsd?: string;
}

/**
 * Pricing v2.0: the card on file pays for what Nexus runs after it ran, up to a monthly usage limit. Gateway charges it
 * each time the unpaid usage reaches `thresholdUsd`; less carries over, and what is left is charged once it is turned
 * off. Decimal dollar strings.
 */
export interface AfterTheFact {
  /** Where the card's charge lifecycle is: read it instead of deriving a state from the other fields. Absent from an older Gateway. */
  phase?: AfterTheFactPhase;
  /** The amount of the charge that runs now; `null` when none runs. Absent from an older Gateway. */
  chargeUsd?: string | null;
  /** The usage the card has not paid, a running charge included: the amount to name when a charge failed or runs. Absent from an older Gateway. */
  unpaidUsd?: string;
  enabled: boolean;
  limitUsd: string;
  /** What the card paid for this UTC month so far. */
  monthSpentUsd: string;
  reservedUsd: string;
  /** What new calls may still spend on the card this month: 0 while off or after a failed charge. */
  leftUsd: string;
  /** The latest hold Gateway refused on this month's limit (402 `usage_limit_reached`), or null: a call that size is refused while `leftUsd` is less. */
  refusedUsd?: string | null;
  /** Usage not charged yet, less a running charge: about zero during a retry. Prefer `unpaidUsd`. */
  owedUsd: string;
  thresholdUsd: string;
  /** A charge is being taken now. */
  pending: boolean;
  /** The last charge failed: calls on the card stop until one goes through. */
  failed: { at: string; reason: "declined" | "authentication_required" | "payment_failed" | "no_payment_method" | "disputed" } | null;
}

/**
 * The card's charge phase, first that holds: `closed`, `disputed`, `needs_confirmation` (3-D Secure), `retrying` or
 * `failed`, `charging`, `on`, then `off_owing` or `off`. Calls on the card stop while failed, retrying, needs
 * confirmation or disputed.
 */
export type AfterTheFactPhase =
  | "off"
  | "off_owing"
  | "on"
  | "charging"
  | "failed"
  | "retrying"
  | "needs_confirmation"
  | "disputed"
  | "closed";

/** `PUT /v1/billing/after-the-fact` (Rust gateway, owners): the setting after; `null` after turning off what was never on with no `limitUsd`. */
export interface SetAfterTheFactResponse {
  afterTheFact: AfterTheFact | null;
}

/** One payment in `GET /v1/billing/receipts` (pricing v2.0). */
export interface Receipt {
  /** The payment (`pi_…`), or `month:<YYYY-MM>` for an invoiced month. */
  id: string;
  at: string;
  kind: "credits" | "auto_topup" | "usage" | "invoice";
  description: string;
  amountUsd: string;
  status: "paid" | "open" | "failed" | "void";
  /** Stripe's page of an invoiced month; `null` for a payment. */
  url: string | null;
  /** Stripe has a receipt page: `GET /v1/billing/receipts/{id}` answers its link. */
  receipt: boolean;
}

export interface ReceiptsResponse {
  receipts: Receipt[];
}

/** A plan's subscription as Stripe keeps it. `active` and `trialing` get the included usage; other statuses do not. */
export interface BillingPlan {
  /** A plan in `NEXUS_PLANS`, such as `plus`, `pro`, `teams_plus` or `teams_pro` (growth/PRICING.md v3.0). */
  plan: string;
  interval: "month" | "year";
  status: string;
  seats: number;
  currentPeriodEnd: string;
  /** The plan ends instead of renewing when this period ends. */
  cancelAtPeriodEnd: boolean;
  /**
   * When a cancelled plan ends (ISO 8601): the period's end, a day before it, or a day in a later period. `null` while
   * it renews. While it is set, the plan cannot change (409 `plan_ending`). Absent from an older Gateway.
   */
  cancelAt?: string | null;
  /** This month's included usage, as a decimal string (`"30.000000"`): it resets each month and does not roll over. */
  includedUsd: string;
  /**
   * Pricing v3.0: this month's cloud hours, in default-size hours, pooled across the seats of a per-seat
   * plan. Absent from an older Gateway.
   */
  includedHours?: number;
  /** The price of a default-size hour past the hours, a decimal string; `null` when the plan sets none. */
  hourlyUsd?: string | null;
  /** The machine sizes the plan allows, smallest first. */
  sizes?: CloudMachineSize[];
  /** A change for the period's end; `null` without one. Absent from an older Gateway. */
  pendingChange?: { plan: string; interval: "month" | "year"; seats: number; at: string } | null;
}

/** `POST /v1/billing/plan/change` (Rust gateway, owners): what a plan change does, or would do with `dryRun`. */
export interface PlanChange {
  when: "now" | "period_end";
  /** When it applies (ISO 8601). */
  at: string;
  /** What is invoiced now: a dry run's preview, or "0" for a change at the period's end; `null` after a change applied now. */
  amountDueUsd: string | null;
  dryRun: boolean;
  plan: BillingPlan | null;
}

/** `GET /v1/billing/plans` (Rust gateway, owners): the plans an organization of this kind can take, priced by Stripe. */
export interface PlanCatalogue {
  audience: "personal" | "company";
  /** Cheapest a month first. */
  plans: CataloguePlan[];
  /** The plan the organization's subscription holds; `null` on the Free plan. */
  current: { plan: string; interval: "month" | "year"; seats: number } | null;
}

export interface CataloguePlan {
  /** The plan's key in `NEXUS_PLANS`: `plus`, `pro`, `teams_plus`, `teams_pro`… */
  plan: string;
  /** The Stripe Product's name; the key when Stripe gave none. */
  name: string;
  perSeat: boolean;
  minSeats: number;
  /** Pricing v3.0: cloud hours each month (per seat), in default-size hours. Absent from an older Gateway. */
  includedHours?: number;
  /** The price of a default-size hour past the hours, a decimal string; `null` when the plan sets none. */
  hourlyUsd?: string | null;
  /** The machine sizes the plan allows, smallest first. */
  sizes?: CloudMachineSize[];
  intervals: {
    interval: "month" | "year";
    /** The price for the interval (per seat for a per-seat plan), a decimal string; `null` with `error`. */
    amountUsd: string | null;
    /** Included usage each month (per seat), a decimal string. */
    includedUsd: string;
    error: "stripe_error" | "stripe_timeout" | "inactive" | "not_usd" | "interval_mismatch" | "no_fixed_amount" | null;
  }[];
  current: boolean;
}

/** What `POST /v1/billing/plan` takes. `seats` is for a per-seat plan only (its minimum when absent). */
export interface PlanChoice {
  plan: string;
  interval: "month" | "year";
  seats?: number;
}

/** The organization's auto top-up of Nexus credits: below `belowUsd` left, Gateway buys `addUsd` with the payment method on file. */
export interface AutoTopup {
  enabled: boolean;
  belowUsd: string;
  addUsd: string;
  /** Why Gateway turned it off; `null` while it is on, or after an owner turned it off. */
  offReason: "closed" | "declined" | "authentication_required" | "payment_failed" | "no_payment_method" | "disputed" | "refunded" | null;
  /** Top-ups charged this UTC day (at most 3), and what they bought (at most $500). */
  todayCount: number;
  todayUsd: string;
  /** A top-up is being charged, or Stripe is still taking its payment. */
  pending: boolean;
}

/** `PUT /v1/billing/auto-topup` (Rust gateway, owners): the setting after; `null` after turning off what was never set. */
export interface SetAutoTopupResponse {
  autoTopup: AutoTopup | null;
}

/** `POST /v1/billing/credits` (Rust gateway, owners): a credit purchase, credited now, on its way, or on a Checkout page. */
export type BuyCreditsResponse =
  | { status: "credited"; amountUsd: string; credits: CreditsView }
  | { status: "processing"; amountUsd: string; paymentId: string }
  | { status: "checkout"; amountUsd: string; url: string; expiresAt: string; checkoutSessionId?: string };

/** `GET /v1/billing/operations/{operationId}`: the outcome of this organization's exact setup or purchase. */
/** `superseded`: a newer card setup set its card first, so this one ended without setting the default. */
export interface BillingOperation { kind: "card" | "credits"; status: "pending" | "complete" | "expired" | "failed" | "superseded" }

/** `POST /v1/billing/payment-method` (Rust gateway, owners): a card page hosted by Stripe Checkout. */
export interface PaymentMethodSetupResponse {
  checkoutSessionId?: string;
  url: string;
  /** When Stripe stops accepting `url`. */
  expiresAt: string;
}

/**
 * `POST /v1/billing/payment-method` with `{"embedded": true}`, when Gateway has `STRIPE_PUBLISHABLE_KEY`: the
 * card form inside Nexus UI's billing page (`clientSecret` and `publishableKey` for Stripe.js, no `url`). Stripe sends
 * the page to `?checkout=done` once the card is saved. Without the key, Gateway answers Stripe's own page.
 */
export interface EmbeddedCardSetup {
  checkoutSessionId: string;
  clientSecret: string;
  publishableKey: string;
  /** When Stripe stops accepting the form. */
  expiresAt: string;
}

/** Whether Gateway answered the card form to show in the page rather than Stripe's own page. */
export function isEmbeddedCardSetup(answer: PaymentMethodSetupResponse | EmbeddedCardSetup): answer is EmbeddedCardSetup {
  return "clientSecret" in answer && typeof answer.clientSecret === "string" && typeof answer.publishableKey === "string";
}

/**
 * `POST /v1/billing/confirm-payment` (Rust gateway, owners): the charge the bank wants the cardholder to confirm (3-D
 * Secure), confirmed again on session. `redirect`: open `url`, the bank's page. `succeeded` or `processing`: Stripe took
 * it, or a bank debit is on its way.
 */
export interface PaymentConfirmationResponse {
  status: "redirect" | "succeeded" | "processing";
  url: string | null;
}

/** `POST /v1/billing/portal` (Rust gateway, owners): Stripe's customer portal; open `url` at once, it expires within minutes. */
export interface BillingPortalResponse {
  url: string;
}

/** One charge of a statement. A `call` line charges a price per call; a `usd` line charges basis points of a dollar amount. */
export interface BillingStatementLine {
  code: "gateway.calls" | "gateway.provider_cost" | "gateway.nexus_usage";
  unit: "call" | "usd";
  /** `call`: a whole number of calls. `usd`: a decimal dollar amount. */
  quantity: string;
  /** `call` only: the price of one call, as a decimal dollar amount. */
  rateUsd?: string;
  /** `usd` only: basis points of `quantity`. */
  rateBps?: number;
  amountUsd: string;
}

/** The Stripe invoice of a stored month. */
export interface BillingInvoice {
  /** `invoicing`: Gateway is making it and nothing is charged yet. The others are where Stripe stands. */
  state: "invoicing" | "invoiced" | "paid" | "failed" | "void" | "uncollectible";
  /** Stripe's page for the invoice; null until Stripe has finalized it. */
  hostedInvoiceUrl: string | null;
}

/** `GET /v1/billing/statement` (Rust gateway, owners): one UTC month's bill. Money is a decimal string in US dollars. */
export interface BillingStatementResponse {
  /** `YYYY-MM`. */
  period: string;
  /** Both lines, with amount 0 until a price is set. */
  lines: BillingStatementLine[];
  /** What the providers charged for the calls on the organization's own keys. They bill it, not Nexus. */
  providerCostUsd: string;
  /** The rest of the month before: the part of its total that no invoice took. */
  carriedInUsd: string;
  totalUsd: string;
  /** What to charge: `totalUsd` in whole cents, rounded down. A whole number as a string. Under 50 cents is not invoiced. */
  amountCents: string;
  /** What moves to the next month: the part of `totalUsd` below one cent, or all of it for a month under 50 cents. */
  carriedOutUsd: string;
  /** The Stripe invoice of a stored month; null for a month priced now, and for a stored month that was not invoiced. */
  invoice: BillingInvoice | null;
}

/** One stored month in `GET /v1/billing/statements` (Rust gateway, owners). */
export interface BillingStatementSummary {
  /** `YYYY-MM`. */
  period: string;
  totalUsd: string;
  /** What the statement priced, in whole cents, as a string. A `closed` month invoices none of it. */
  amountCents: string;
  /** `closed`: under 50 cents, so nothing was invoiced and the total carries to the next month. The rest are the invoice's states. */
  state: "closed" | BillingInvoice["state"];
  /** Null for a `closed` month, and until Stripe has finalized the invoice. */
  hostedInvoiceUrl: string | null;
}

/** `GET /v1/billing/statements`: the stored months of Gateway's Stripe mode, newest first, at most 24. */
export interface BillingStatementsResponse {
  statements: BillingStatementSummary[];
}

export interface ModulesResponse {
  modules: ModuleManifest[];
  /** Registered modules that did not answer `GET /manifest`. */
  unavailable: { url: string; error: string }[];
}

export interface ConnectorSummary {
  name: string;
  description: string;
  operations: { name: string; summary: string }[];
  /** How far it is along its release: production offers only `released`. Absent from an older Gateway. */
  stage?: ConnectorStage;
  /** Whether this Gateway offers it; no one uses one it does not offer. Absent (offered) from an older Gateway. */
  offered?: boolean;
  /** Rust gateway: whether the organization made it available to the caller; null when that could not be read. */
  usable?: boolean | null;
  /** Rust gateway: whose connections it runs with; empty when it resolves its own credential (the GitHub App). */
  connects?: ConnectionScope[];
  /** Rust gateway: the fields a key connection takes, as a JSON Schema; null when it connects with OAuth or not at all. */
  form?: ConnectorForm | null;
  /** Whether it connects with OAuth on this Gateway. */
  oauth?: boolean;
  /** Offline only (`NEXUS_CONNECTORS=mock`): the form's input its test data accepts; absent live. */
  offline_input?: Record<string, string>;
}

/**
 * `GET /v1/connectors/{name}` (Rust gateway): one connector, with each operation's `effect` (`read` or `mutate`) and
 * its `request` JSON Schema.
 */
export interface ConnectorDetail extends Omit<ConnectorSummary, "operations"> {
  operations: { name: string; summary: string; effect: "read" | "mutate" | (string & {}); request: Record<string, unknown> }[];
}

/** A connector's release stage. */
export type ConnectorStage = "preview" | "released";

/** A connector's key form: `x-nexus-secret` marks a field never shown again. */
export interface ConnectorForm {
  type?: "object";
  required?: string[];
  properties: Record<string, { type?: string; description?: string; examples?: string[]; minLength?: number; maxLength?: number; "x-nexus-secret"?: boolean }>;
}

/** `GET|PUT /v1/policies/connectors…`: who in the organization may use a connector. */
export type ConnectorAvailabilityLevel = "disabled" | "available" | "enabled_for_all";

export interface ConnectorAvailability {
  name: string;
  availability: ConnectorAvailabilityLevel;
  /** Team slugs; null for the whole organization. */
  teams: string[] | null;
  /** The connector's default: the organization never chose. */
  default: boolean;
  /** Whether this Gateway offers it: one it does not offer may only be set to `disabled`. Absent from an older Gateway. */
  offered?: boolean;
  changedBy?: string;
  changedAt?: string;
}

export interface SetConnectorAvailability {
  availability: ConnectorAvailabilityLevel;
  /** Leave it out for the whole organization. */
  teams?: string[];
}

/** Where Policy's approval record for a chat write is (PO1): pending, then confirmed and used, or denied. */
export type ApprovalStatus = "pending" | "confirmed" | "denied" | "used" | "expired";

/**
 * `POST /v1/approvals` and the routes under it (PO1): Policy's record bound to one exact connector call by
 * a chat agent's link. Only the agent link creates it and uses it; only its person confirms or denies it, in Nexus.
 * `argumentsHash` is the SHA-256 of the arguments' canonical JSON, and `arguments` is that JSON, as stored.
 */
export interface Approval {
  id: string;
  connector: string;
  operation: string;
  arguments: Record<string, unknown>;
  argumentsHash: string;
  status: ApprovalStatus;
  /** Its expiry passed: it can no longer be confirmed or used. */
  expired: boolean;
  createdAt: string;
  expiresAt: string;
  decidedAt?: string;
  usedAt?: string;
  /** Who decided it: `person` (confirm or deny in Nexus) or `agent` (the link cancelled it). */
  decidedBy?: "person" | "agent";
  /** Nexus UI's page where the person confirms or denies it, when Gateway knows Nexus UI's URL. */
  confirmUrl?: string;
}

/** `POST /v1/approvals`: the exact call, as it will be sent. */
export interface CreateApproval {
  connector: string;
  operation: string;
  arguments: Record<string, unknown>;
}

/** One connector operation that chat writes may run in the organization. */
export interface ChatWriteOperation {
  connector: string;
  operation: string;
}

/**
 * `GET /v1/policies/chat-writes`: whether a person-confirmed chat write may run, and which operations it
 * may run. On with every registered write by default; saved choices take precedence.
 */
export interface ChatWriteSetting {
  enabled: boolean;
  /** The organization never chose: on with every registered write. */
  default: boolean;
  /** The allowlist. A write of any other operation is refused (403 `operation_not_allowed`). */
  operations: ChatWriteOperation[];
  changedBy?: string;
  changedAt?: string;
}

/**
 * `GET` and `PUT /v1/policies/agent-writes`: whether the person lets their agent links make the organization's
 * allowed writes with no confirmation in Nexus per write. On by default. Only the person, never an agent, changes it.
 */
export interface AgentWriteSetting {
  enabled: boolean;
}

/** `PUT /v1/policies/chat-writes`: `operations`, when given, replaces the allowlist; left out, it is kept. */
export interface SetChatWrites {
  enabled: boolean;
  operations?: ChatWriteOperation[];
}

/** A person's own connection (`user`) or the organization's (`org`). */
export type ConnectionScope = "user" | "org";

/** `GET /v1/connections`: one account at a connector's vendor, never its secret. */
export interface Connection {
  connector: string;
  scope: ConnectionScope;
  scopeId: string;
  /** The vendor's stable id for the account. */
  account: string;
  /** The account's name, as a call's `account` may name it. */
  name: string;
  data?: Record<string, unknown>;
  /** Who the vendor says the holder is there; members do not see it on the organization's. */
  identity?: string;
  status: "active" | "needs_reconnect";
  /** Its connector is in preview and this Gateway does not offer it: kept, unused, still disconnectable. */
  suspended?: boolean;
  kind: "api_key" | "basic" | "oauth2" | "installation";
  last4?: string;
  scopes?: string[];
  expiresAt?: string;
  createdAt: string;
}

export interface ConnectionsResponse {
  connections: Connection[];
}

/** `DELETE /v1/connections/{connector}/{account}`. */
export interface DisconnectedResponse {
  disconnected: { connector: string; account: string; name: string };
}

/** `POST /v1/connections/oauth/finish`. */
export interface OAuthFinishedResponse {
  connector: string;
  connections: Connection[];
}

/**
 * `{ state, code }` from the vendor's return, or `{ state, error }` when the person declined. `realmId` is the company
 * QuickBooks names on its return; Gateway checks it with the new token.
 */
export type OAuthFinishRequest = { state: string; code: string; realmId?: string } | { state: string; error: string };

/**
 * `github` connector: `POST /v1/connectors/github/repo`. This and the module types below (`RunSummary`, `Page`,
 * `RunReport`) have Rust twins in `nexus-types::api::{connectors, modules}` (Nexus's Rust services); change both together.
 */
export interface RepoInfo {
  owner: string;
  name: string;
  ref: string;
  description?: string;
  /** Every file path (Rust gateway; the frozen TypeScript one bounds them); `truncated` says GitHub cut the tree. */
  paths: string[];
  truncated: boolean;
  /** How many paths (Rust gateway only). */
  total?: number;
  /** The folder asked for with `path`, when only it was read (Rust gateway only). */
  path?: string;
}

/**
 * `github` connector: `POST /v1/connectors/github/files`. Within a byte budget for the call (Rust gateway: 100 files,
 * 2 MB in all): a file is cut where it runs out, and the ones after it come back empty and `truncated`.
 */
export interface RepoFiles {
  files: { path: string; content: string; truncated: boolean }[];
  missing: string[];
}

/** What a module returns for `run` and `report/:id`. */
export interface RunSummary {
  id: string;
  status: "running" | "succeeded" | "failed";
  /** The module's own word for the state, shown in place of the status (`Posted`, `Shadow`, `Paused`, …). */
  label?: string;
  /**
   * How the state looks; absent: from the status. `running` status with another tone is a waiting (paused) run:
   * no spinner, not polled.
   */
  tone?: RunTone;
  input: Record<string, string>;
  startedAt: string;
  finishedAt?: string;
  error?: string;
  costUsd: number;
  /** The commit of the repository the run read, for a module that reads one and when it was given one. */
  commit?: string;
  /** Coverage of the codebase, from 0 to 1: the share of the repository's files in scope that the run's pages cite. */
  coverage?: number;
}

/** One module write grant (`GET /v1/policies/grants`): `module` may run `connector/action`. */
export interface WriteGrant {
  module: string;
  connector: string;
  action: string;
  grantedBy?: string | null;
  at: string;
}

/** `GET /v1/policies/grants`: the organization's grants, and the scopes each connector can grant. */
export interface WriteGrantList {
  grants: WriteGrant[];
  grantable: Record<string, string[]>;
}

/** `PUT /v1/policies/grants/{product}/{capability}`: what the module now holds for the connector, and what changed. */
export interface WriteGrantsSet {
  module: string;
  connector: string;
  actions: string[];
  granted: string[];
  revoked: string[];
}

/**
 * Which review events a module may post on the organization's pull requests (`GET /v1/policies/review-events`):
 * `comment` (COMMENT only), `request_changes` (and REQUEST_CHANGES) or `full` (and APPROVE). `full` by default.
 */
export type ReviewEventLevel = "comment" | "request_changes" | "full";

/** A pull request review's event, as GitHub names it. */
export type ReviewEventName = "COMMENT" | "REQUEST_CHANGES" | "APPROVE";

/** `GET` and `PUT /v1/policies/review-events`: the organization's level; `default` while nobody chose. */
export interface ReviewEventSetting {
  level: ReviewEventLevel;
  default: boolean;
  changedBy?: string;
  changedAt?: string;
}

/** A starter rule Nexus Review drafted from a repository's conventions (S4.1); the owner accepts or rejects it. */
export interface ReviewRuleDraft {
  /** `.review-bot/rules/<name>.md`: where the file goes when the team commits it. */
  path: string;
  /** The whole rule file, header included. */
  text: string;
  state: "proposed" | "accepted" | "rejected";
}

/** Nexus Review's `connect` answer: the stored config and, when the repository has any, its starter rule drafts. */
export interface ReviewConnectResult {
  ok: boolean;
  repo: string;
  config: Record<string, unknown>;
  drafts?: ReviewRuleDraft[];
  /** Why `draft_rules` drafted nothing (the repository has rule files, or no convention file). */
  drafts_skipped?: string;
  /** Why drafting failed; the config is stored all the same. */
  drafts_error?: string;
}

/*
 * Nexus Review's status contract (`GET /v1/robustness/review/status?repo=&pr=`, `products.use`; `POST …/status` with a
 * draft `config` evaluates it without saving, owners only). The Review module computes every field
 * (`nexus-review/src/core/status.rs`); clients render it and never re-derive it. A client reads `contract` and ignores
 * fields it does not know. A 404 means a backend older than the contract: show the reduced view.
 */

export type ReviewMode = "shadow" | "comment" | "comment_and_check";
export type ReviewEffort = "quick" | "standard" | "deep";
export type ReviewWriteAction = "pull_review" | "check_run" | "issue_comment" | "reaction" | "resolve_thread";
export type ReviewRole = "owner" | "admin" | "member";

/** A stable reason code. A client that does not know a code shows `message`. */
export type ReviewReasonCode =
  | "publish_off"
  | "review_disabled"
  | "sandbox_off"
  | "sandbox_probe_failed"
  | "write_not_granted"
  | "connector_disabled"
  | "grants_unknown"
  | "repo_not_watched"
  | "repo_not_connected"
  | "backend_unsupported"
  | "shadow_mode"
  | "dry_run"
  | "no_review_model"
  | "run_overrides_off"
  | "author_excluded"
  | "author_not_listed"
  | "author_not_opted_in"
  | "author_opted_out"
  | "bot_author"
  | "model_override_full_review"
  | "review_events_unknown"
  | "review_events_limited"
  | "role";

/** Why something is off or falls back: a stable `code`, a sentence for people, and who can change it and how. */
export interface ReviewReason {
  code: ReviewReasonCode | (string & {});
  message: string;
  fix?: { who: "operator" | "owner" | "owner_or_admin" | "author"; action: string; command?: string };
}

/** One deployment or organization capability, as the running module sees it. `source` names who sets it. */
export interface ReviewCapability {
  available: boolean;
  source: string;
  detail?: string;
  reason?: ReviewReason;
}

/**
 * Whether one option of a setting can run here. `supported`: the backend knows it. `executable`: it can run now;
 * `null` when that depends on grants the Gateway did not report (`grants_unknown` in `reasons`).
 */
export interface ReviewOptionState<T extends string> {
  value: T;
  label: string;
  hint: string;
  supported: boolean;
  executable: boolean | null;
  needs: ReviewWriteAction[];
  reasons: ReviewReason[];
}

/** One behaviour as it will actually run, and when its dependency is checked. */
export interface ReviewEffective {
  state: "on" | "off" | "fallback" | "blocked";
  summary: string;
  reasons: ReviewReason[];
  checked: "before_spend" | "at_publish" | "not_applicable";
}

/** The repository's stored layer-C config: what the owner asked for. Never rewritten when a capability is missing. */
export type ReviewIntent = Record<string, unknown> & {
  enabled: boolean;
  mode: ReviewMode;
  effort?: ReviewEffort;
};

export interface ReviewStatus {
  contract: 1;
  backend: { version: string; features: string[] };
  viewer: { role: ReviewRole; can: { configure: boolean; grant: boolean; run: boolean; apply: boolean } };
  deployment: {
    review: ReviewCapability;
    publish: ReviewCapability;
    sandbox: ReviewCapability & { runtime: "bwrap" | null };
    run_overrides: ReviewCapability;
    poll_seconds: number | null;
    app_handle: string;
  };
  organization: {
    connector: ReviewCapability;
    /** False when the Gateway sent no organization facts (an older Gateway): grants are then unknown, not missing. */
    grants_known: boolean;
    /** `granted` is `null` when `grants_known` is false: unknown, not missing. */
    grants: Array<{ action: ReviewWriteAction; granted: boolean | null; needed_by: string[] }>;
    budget: { team: string | null; per_pr_usd: string; per_hour_usd: string };
    /** The review event level the Gateway reported; absent from an older backend, `null` when the Gateway sent none. */
    review_events?: ReviewEventLevel | null;
  };
  repository: {
    repo: string;
    connected: boolean;
    intent: ReviewIntent;
    updated_at: string | null;
    updated_by: string | null;
  };
  options: { mode: ReviewOptionState<ReviewMode>[]; effort: ReviewOptionState<ReviewEffort>[] };
  effective: {
    readiness: "ready" | "limited" | "blocked" | "not_connected";
    headline: string;
    automatic: ReviewEffective;
    manual: ReviewEffective;
    posting: ReviewEffective & { intended: ReviewMode; actual: ReviewMode | "none" };
    comments: ReviewEffective;
    check_run: ReviewEffective;
    replies_reactions: ReviewEffective;
    resolve_threads: ReviewEffective;
    /**
     * Which review event each verdict of a fully read run posts, at the level Review acts on (`comment` while the
     * level is unknown). A partial run always posts a COMMENT. Absent from a backend without the `review_events` feature.
     */
    review_events?: ReviewEffective & {
      level: ReviewEventLevel;
      posts: { failure: ReviewEventName | null; success: ReviewEventName | null; neutral: ReviewEventName | null };
    };
    effort: ReviewEffective & { intended: ReviewEffort; actual: ReviewEffort };
    apply: ReviewEffective;
  };
  /** What a manual run (the run form, `nexus robustness run review`) will do. */
  run: {
    /** What "post to GitHub" does on a manual run: `actual` is what it posts; `none` runs dry and says why. */
    post: ReviewEffective & { actual: ReviewMode | "none" };
    /** The filters a manual run skips (watching, PR state, base branch, labels and title, drafts and paths). */
    skips: string[];
    /** The filters a manual run still applies (the author filter). */
    applies: string[];
    /** With `pr`: whether the author filter lets a manual run review it; `null`, with `error`, when it was not read. */
    pr?: { number: number; author: string | null; reviewed: boolean | null; reasons: ReviewReason[]; error?: string };
    /** The `outcome_filter_*` run overrides: available only where the deployment allows them. */
    overrides: ReviewCapability;
    /** A model override reviews every file again: incremental review is off for that run. */
    model_override: ReviewReason | null;
  };
  /** The models a run of this repository may use: the repository team's, not the caller's. */
  run_models: string[];
  /** Saved intent this deployment or organization cannot carry out now. Kept, not rewritten. */
  warnings: ReviewReason[];
}

/** What a Review run did about posting, beside its `RunSummary` (in `GET report` answers from Review). */
export interface ReviewRunPosting {
  posted: boolean;
  /** What it posted: `none` for shadow, dry and held runs. */
  mode: ReviewMode | "none";
  /**
   * Why a run that asked to post did not: `publish_off`, `write_not_granted`, `connector_disabled`, `dry_run`,
   * `shadow_mode`.
   */
  reason?: string | null;
  /** Set when the agent did not run and the single pass did: the effort asked and why it fell back. */
  effort_fallback?: { intended: ReviewEffort; actual: "quick"; reason: string } | null;
}

/** The look of a run's state: in progress, done, done quietly, needs a look or waits, failed. */
export type RunTone = "running" | "strong" | "muted" | "warn" | "danger";

/**
 * A run's stored status, as `GET report?status=` filters by it (the Review module's `RunStatus`). `RunSummary.status`
 * is coarser: the stored status shows as the run's `label` instead.
 */
export type RunStatusName =
  | "queued"
  | "running"
  | "paused"
  | "superseded"
  | "withheld"
  | "skipped"
  | "posted"
  | "failed"
  | "degraded";

/** `GET report` on a paged capability: one page of runs, newest first. `next_cursor` is the `after` of the next page. */
export interface RunsPage {
  runs: RunSummary[];
  next_cursor?: string | null;
}

/** What a paged `GET report` is asked for: the page size (1 to 200), the page after a cursor, and the statuses to keep. */
export interface RunsQuery {
  limit?: number;
  after?: string;
  status?: RunStatusName[];
}

/** `github` connector: `POST /v1/connectors/github/pulls_open` (`{ repo, page? }`), one page of open pull requests. */
export interface PullsOpen {
  pulls: OpenPull[];
  page: number;
  next_page?: number;
}

export interface OpenPull {
  number: number;
  /** PR title when the connector supplies it; older gateways may omit it. */
  title?: string;
  head_sha: string;
  base_ref: string;
  draft: boolean;
  author: string;
  updated_at: string;
}

export interface Page {
  slug: string;
  title: string;
  markdown: string;
  /** The code the page cites as `[n]`; absent when it cites none. A `[n]` without an entry here is text (`arr[1]`). */
  citations?: PageCitation[];
  /** The repository paths the page was written from, cited or not; absent for pages written before they were kept. */
  files?: string[];
}

/** Lines of the repository a page cites as `[n]`. */
export interface PageCitation {
  n: number;
  path: string;
  /** First and last line, from 1. */
  from: number;
  to: number;
  /** The lines on the repository's host, at the commit the run read; absent for runs that did not record it. */
  href?: string;
}

export interface RunReport extends RunSummary {
  pages: Page[];
}

/**
 * `GET report?repo=<url>` (Nexus Wiki, G6): one repository's runs as versions, newest first, and the pages of one of
 * them: `&run=<id>`, else the newest that succeeded. `run` is absent when no version has pages to show.
 */
export interface RunVersions {
  repo: string;
  versions: RunSummary[];
  run?: string;
  pages: Page[];
}

/** Coding agents in the Gateway catalog, by their API id. */
export type AgentName = "claude" | "codex" | "antigravity" | "cursor" | "opencode" | "grok";

/** an internal decision's agent policy (deprecated alias of the agent's an internal decision rule): its allow list and its default. */
export interface AgentModelPolicy {
  allowedModels: string[];
  defaultModel: string;
}

/** `GET /v1/agents/policy` */
export interface AgentPolicyResponse {
  agents: Record<AgentName, AgentModelPolicy>;
  /** Agents whose policy the organization set itself; the others show Core's defaults. */
  custom?: Partial<Record<AgentName, boolean>>;
}

/**
 * How an agent reaches its models: `gateway` (through Nexus Gateway, every call metered and checked; paid with the
 * organization's own key or a key Nexus pays for) or `subscription` (the person's own plan, signed in with the agent's
 * own login on their machine; calls straight to the vendor).
 */
export type AgentRoute = "gateway" | "subscription";

/** Who pays an agent's model calls: the organization's own key (BYOK), a key Nexus pays for, or the person's plan. */
export type AgentAccessKind = "byok" | "nexus" | "subscription";

/** One route of an agent in `GET /v1/agents/access`. */
export interface AgentRouteView {
  route: AgentRoute;
  /** Who pays on it; `null` on `gateway` when no key would pay (Gateway refuses the calls). */
  access: AgentAccessKind | null;
  /** Whether it can run now; `reason` says why not, with the command that fixes it. */
  available: boolean;
  reason?: string;
  /** `gateway`: the provider Gateway calls. */
  provider?: string;
  /** `subscription`: the plan, its vendor, and how to sign the agent in to it (once) and check that sign-in. */
  plan?: string;
  vendor?: string;
  login?: string;
  loginStatus?: string;
}

/** One agent in `GET /v1/agents/access`. */
export interface AgentAccessEntry {
  agent: AgentName;
  name: string;
  /** How to install it (the vendor's own package; Nexus never installs it). */
  install: string;
  /** The route it takes when the person names none, and why. */
  route: AgentRoute;
  from: "preference" | "default" | "organization";
  /** The person's saved route, if any. */
  preference?: AgentRoute;
  routes: AgentRouteView[];
}

/** `GET` and `PUT /v1/agents/access` (Rust gateway only): the caller's coding agents' routes. */
export interface AgentAccessResponse {
  /** Whether the organization allows subscription routes. */
  agentSubscriptions: boolean;
  agents: AgentAccessEntry[];
}

/** `POST /v1/agents/check`: may `agent` launch on `model` (the default when none is named)? an internal decision's decision. */
export interface AgentCheckResponse extends ModelDecisionView {
  agent: AgentName;
  allowedModels: string[];
  defaultModel: string;
  defaultFrom?: PolicyRuleRef;
}

// ---------------------------------------------------------------- models and policies

export type ModelStatus = "supported" | "coming_soon";
export type ModelSurface = "chat" | "anthropic_messages" | "openai_responses";

/** One model of `GET /v1/models` (OpenAI-compatible, with Nexus's fields). */
export interface ModelEntry {
  /** Canonical: `provider/model`. */
  id: string;
  object: "model";
  created: number;
  /** The maker: who made the model, the first part of its id. */
  owned_by: string;
  /** Rust gateway only, supported models: the host that serves it, whose rules price it. */
  provider?: string;
  name: string;
  family: string;
  status: ModelStatus;
  /** Whether the caller may use it under the policies (always false for coming-soon models). */
  allowed: boolean;
  surfaces: ModelSurface[];
  /** The provider's own id (`claude-opus-5-5`). */
  provider_model?: string;
  pricing?: { input: number; output: number; unit: string };
  default_for?: ("chat" | "claude" | "codex")[];
  note?: string;
  /** Catalog classification. True means open weights. Null means unclassified. */
  openWeights?: boolean | null;
  /** Rust gateway only, supported models: what it reads; `image` when a chat question with images may use it. */
  input?: ("text" | "image")[];
  /** Rust gateway only, supported models: input and output tokens together, at most. */
  context_window?: number;
  /** Rust gateway only, supported models: the longest output. */
  max_output_tokens?: number;
  /** Rust gateway only: one hosted web search's list price in dollars, when the model can run a chat's search. */
  web_search_usd?: number;
  /** Rust gateway only, supported models: how it reasons (`nexus-catalog`'s spec). */
  reasoning?: {
    mode: "effort" | "budget" | "none";
    /** Lowest first; empty unless `mode` is `effort`. */
    efforts: string[];
    default_effort: string | null;
    can_disable: boolean;
    disable_max_effort: string | null;
  };
}

export interface ModelListResponse {
  object: "list";
  data: ModelEntry[];
  /** Models the Gateway knows of but cannot serve yet, each with why. They are not in `data` and cannot be called. */
  unavailable?: { id: string; reason: string }[];
  team?: string;
}

/** A model access rule: patterns are exact models, `provider/*` or `provider/family-*`. */
export interface ModelRule {
  allow?: string[];
  deny?: string[];
  defaults?: { chat?: string; claude?: string; codex?: string };
  /** USD per million tokens. A model priced above either limit is refused; a price equal to the limit passes. */
  maxPrice?: { input?: number; output?: number };
  /** ISO 3166-1 alpha-2 codes. A model whose maker country or provider country is listed is refused. A model with no catalog entry is refused when a side is listed. */
  denyCountries?: { maker?: string[]; provider?: string[] };
  /** Provider ids: a provider in `deny` is off; with `allow`, only the providers it lists are on. */
  providers?: { allow?: string[]; deny?: string[] };
}

export interface PolicyRuleRef {
  level: "deployment" | "organization" | "team" | "agent" | "key" | "catalog" | "none";
  subject?: string;
  list?: "allow" | "deny" | "defaults" | "maxPrice" | "denyCountries";
  pattern?: string;
}

/** `GET|PUT /v1/policies/organization`, `…/teams/{slug}`, `…/agents/{agent}`. */
export interface ModelPolicyView {
  level: "organization" | "team" | "agent";
  subject: string;
  team?: { id: string; slug: string; name: string };
  rule: ModelRule;
  custom: boolean;
  migrated?: boolean;
  updatedAt?: string;
  inherited: { level: string; subject: string; rule: ModelRule }[];
  effective: { allowed: string[]; defaults: Record<"chat" | "claude" | "codex", { model?: string; from: PolicyRuleRef; note?: string }> };
  /** After a PUT: accepted, but worth knowing. */
  notes?: string[];
}

export interface ModelDecisionView {
  allowed: boolean;
  requested: string;
  model: string | null;
  providerModel?: string;
  surface: ModelSurface;
  reason: string;
  rule: PolicyRuleRef;
  code?: "model_not_allowed" | "model_not_found" | "model_not_supported";
  resolvedFrom?: "nexus/auto" | "nexus/default";
  defaultFrom?: PolicyRuleRef;
  notes: string[];
}

/** `POST /v1/policies/test` */
export interface PolicyTestRequest {
  /** An email, a user id or a key id; default: you. Someone else needs owner or admin. */
  as?: string;
  team?: string;
  agent?: AgentName;
  model?: string;
  surface?: ModelSurface;
}

export interface PolicyTestResponse extends ModelDecisionView {
  as: { userId?: string; email?: string; keyId?: string };
  team?: string;
  agent?: AgentName;
  allowedModels: string[];
  warnings: string[];
}

export type OperatorControlKind = "pause" | "limit" | "override";
export type OperatorControlLevel = "organization" | "team" | "person" | "key";

/** A time-boxed pause, limit or override on one level. `subject` is a team id, a user id, a key id, or the tenant id for the organization. */
export interface OperatorControl {
  id: string;
  tenantId: string;
  kind: OperatorControlKind;
  subjectLevel: OperatorControlLevel;
  subject: string;
  models?: string[];
  reason: string;
  createdBy?: string;
  createdAt: string;
  expiresAt: string;
}

/** `subject` is a team slug, a user id or a key id, and the organization ignores it. `models` is for limit and override only. */
export interface SetOperatorControlBody {
  kind: OperatorControlKind;
  level: OperatorControlLevel;
  subject?: string;
  models?: string[];
  reason: string;
  expiresAt: string;
}

export interface OperatorControlsResponse {
  controls: OperatorControl[];
}

export interface AgentSessionSummary {
  id: string;
  agent: AgentName;
  model: string;
  /**
   * `nexus`: the agent's calls go through Core's gateway, each one reserved, metered and attributed to the session
   * . `subscription`: the person's own Claude or ChatGPT login, calls straight to the provider; `usage` is what the agent
   * reported. `ori`: sessions from an internal decision, when the traffic went to the user's own OpenRouter account.
   */
  via: "ori" | "nexus" | "subscription";
  person: string;
  cwd?: string;
  startedAt: string;
  endedAt?: string;
  durationMs?: number;
  exitCode?: number;
  signal?: string;
  /** The agent's own conversation id: `nexus claude --resume <id>` / `nexus codex resume <id>` picks it up again. */
  agentSessionId?: string;
  status: "running" | "succeeded" | "failed" | "interrupted" | "lost";
  /** an internal decision: the team (slug) the session's spend goes to. */
  team?: string;
  /**
   * Who pays its model calls, as Gateway classified the session when it opened (Rust gateway only). Absent for `ori`,
   * for older sessions, and when no key would pay.
   */
  access?: AgentAccessKind;
  /** `subscription` sessions, once Claude Code has reported: its calls summed. The cost is its list-price estimate, never charged. */
  usage?: {
    calls: number;
    inputTokens: number;
    outputTokens: number;
    cacheReadTokens: number;
    cacheCreationTokens: number;
    estimatedCostUsd: number;
    /** Rust gateway: calls with no estimate, because the agent stated no cost or the call names no model the catalog lists. */
    unpricedCalls?: number;
    models: string[];
  };
}

/** `POST /v1/agents/sessions` */
export interface OpenAgentSessionRequest {
  agent: AgentName;
  /** `nexus`: the agent runs through Core's gateway; the model is checked on every call, not here. `subscription`: on the person's own login. */
  via?: "ori" | "nexus" | "subscription";
  model?: string;
  /** Who is running it, e.g. the OS user name. */
  person?: string;
  /** Basename of the working directory; Core keeps only the last path segment. */
  cwd?: string;
  /** an internal decision: a team slug you are in (`nexus claude --team`); default: your default team. */
  team?: string;
}

/** `POST /v1/agents/sessions/{id}/end` */
export interface EndAgentSessionRequest {
  exitCode: number;
  durationMs?: number;
  /** Set when the agent was ended by a signal, e.g. `SIGINT`. */
  signal?: string;
  /** The agent's own conversation id (a hyphenated UUID), so the session can be resumed. */
  agentSessionId?: string;
}

/** an internal decision: who made a change, through which credential. */
export type AuditActorKind = "person" | "api_key" | "cli" | "ui" | "module" | "service" | "sandbox" | "operator" | "agent" | "platform" | "system";
/** `chat`: a chat agent acting for a person (`x-nexus-client: agent-<platform>/<version>`). */
export type AuditSurface = "api" | "cli" | "mcp" | "ui" | "chat" | "webhook" | "system";

/** an internal decision: one change in the organization, from `GET /v1/audit`. Append-only, kept indefinitely. */
export interface AuditEvent {
  id: string;
  at: string;
  /** `email` is filled in when the log is read; `erased` marks a person erased under ADR 0006, shown with no email. */
  actor: { userId?: string; email?: string; keyId: string; kind: AuditActorKind; erased?: boolean };
  surface: AuditSurface;
  /** Stable and dotted: `team.created`, `member.role_changed`, `key.revoked`, … */
  action: string;
  target: { type: string; id: string; slug?: string };
  before?: unknown;
  after?: unknown;
  requestId?: string;
  metadata?: Record<string, unknown>;
}

/** `all`: owners and admins; `team`: a lead (their teams and those under them, and their own actions); `own`: your own actions. */
export type AuditScope = "all" | "team" | "own";

export interface AuditResponse {
  scope: AuditScope;
  events: AuditEvent[];
  /** Pass as `cursor` for the next, older page. */
  nextCursor?: string;
}

export interface AuditQuery {
  /** ISO time or relative (`24h`, `7d`). */
  since?: string;
  until?: string;
  /** An action, a comma-separated list, or a prefix with `*` (`team.*`). */
  action?: string;
  /** A user id or email. */
  actor?: string;
  /** A target's id or slug. */
  target?: string;
  /** A team slug: that team and every team under it. */
  team?: string;
  limit?: number;
  cursor?: string;
}

/**
 * Nexus Observability: the spend list and the files (Rust only; owners and admins). Every row is flat and
 * has every field: an unknown value is `null`. Money is a decimal string in US dollars with six decimals. People and
 * teams are ids; their names are in the directory file.
 */
export type SpendCallStatus = "settled" | "failed" | "refused" | "released";

/** One model call, as the ledger recorded it (`GET /v1/spend/calls`, and a row of its files). */
export interface SpendCall {
  id: string;
  at: string;
  status: SpendCallStatus;
  /** `null` for a released call, whose model the ledger did not record. */
  model: string | null;
  /** `null` for a refused or released call. */
  provider: string | null;
  product: string;
  capability: string | null;
  userId: string | null;
  teamId: string | null;
  agentSessionId: string | null;
  chatSessionId: string | null;
  /** `null` for a released call, whose tokens nobody reported. */
  inputTokens: number | null;
  outputTokens: number | null;
  costUsd: string;
  reservedUsd: string | null;
  /** The ledger's note, with any secret redacted: why it was refused, failed or released, or what it charged past its hold. */
  reason: string | null;
  overspendLevel: "organization" | "team" | "person" | "product" | null;
  /** A person's limit only. */
  overspendLimitUsd: string | null;
  overspendAmountUsd: string | null;
  overspendOverUsd: string | null;
  overspendPeriod: string | null;
  fundedBy: string | null;
}

export interface SpendCallPage {
  from: string;
  to: string;
  calls: SpendCall[];
  /** Pass as `cursor` for the next, older page; `null` on the last page. */
  nextCursor: string | null;
}

/** The calls of one UTC day, team, person, model, provider and product, summed (refused calls left out). */
export interface UsageRow {
  day: string;
  teamId: string | null;
  userId: string | null;
  /** `null` for released calls. */
  model: string | null;
  /** `null` for released calls. */
  provider: string | null;
  product: string;
  capability: string | null;
  calls: number;
  inputTokens: number;
  outputTokens: number;
  costUsd: string;
}

/** One coding-agent session. The `reported…` fields are a `subscription` session's, as the agent reported them. */
export interface AgentSessionRow {
  id: string;
  agent: AgentName;
  model: string;
  via: "nexus" | "subscription" | "ori";
  access: "subscription" | "byok" | "nexus" | null;
  userId: string | null;
  teamId: string | null;
  status: "running" | "succeeded" | "failed" | "interrupted" | "lost";
  startedAt: string;
  endedAt: string | null;
  durationMs: number | null;
  exitCode: number | null;
  signal: string | null;
  calls: number;
  costUsd: string;
  reportedCalls: number | null;
  reportedInputTokens: number | null;
  reportedOutputTokens: number | null;
  reportedCacheReadTokens: number | null;
  reportedCacheCreationTokens: number | null;
  estimatedCostUsd: string | null;
}

/** One person or team with its name: the directory file, the one that maps ids to names. */
export interface DirectoryEntry {
  kind: "person" | "team";
  id: string;
  name: string | null;
  slug: string | null;
  parentId: string | null;
  erased: boolean;
}

export type ExportFamily = "spend_calls" | "usage" | "agent_sessions" | "directory";
export type ExportFormat = "csv" | "jsonl";

/**
 * What one file holds. A JSON Lines file ends with `{"manifest": …}`; a CSV file's is `metadata.manifest` on its
 * audit event (`audit({ target: exportId })`).
 */
export interface ExportManifest {
  exportId: string;
  family: ExportFamily;
  format: ExportFormat;
  schemaVersion: number;
  scope: { userId: string | null; keyId: string; filters: Record<string, string> };
  /** `null` for the directory file. */
  range: { from: string; to: string } | null;
  rows: number;
  /** `sha256:<hex>` */
  checksum: string;
  generatedAt: string;
  watermark: string | null;
  /** `false` when the row limit cut the file: pass `nextCursor` as `cursor` for the rest. */
  complete: boolean;
  nextCursor: string | null;
  policyVersion: string | null;
  integrity: "not_verifiable";
}

/** A downloaded file: its text, and the export's id (`x-nexus-export-id`) that finds its audit event. */
export interface ExportFile {
  body: string;
  exportId: string | null;
}

export interface SpendCallFilters {
  /** `YYYY-MM-DD`, UTC; default 29 days before `to`. */
  from?: string;
  /** `YYYY-MM-DD`, UTC, included; default today. At most 366 days. */
  to?: string;
  /** A team slug or id, with every team under it; `unassigned` for calls with no team. */
  team?: string;
  /** A user id. */
  person?: string;
  /** A model, or `-` for the calls with none (released calls). */
  model?: string;
  /** A provider, or `-` for the calls with none (refused and released calls). */
  provider?: string;
  /** `product` or `product/capability`. */
  product?: string;
  /** A comma-separated set of statuses. */
  status?: string;
  /** An agent session id or a chat session id. */
  session?: string;
  /** A page's `nextCursor`, or a cut file's `manifest.nextCursor`. */
  cursor?: string;
}

export interface SpendCallsQuery extends SpendCallFilters {
  /** 1 to 500; default 50. */
  limit?: number;
}

export type UsageFilters = Omit<SpendCallFilters, "status" | "session">;

export interface AgentSessionFilters {
  /** The first UTC day a session started. */
  from?: string;
  to?: string;
  team?: string;
  person?: string;
  agent?: AgentName;
  via?: "nexus" | "subscription" | "ori";
  /** A comma-separated set of `running`, `succeeded`, `failed` and `interrupted`. */
  status?: string;
  cursor?: string;
}

/** an internal decision: a chat session's mode: `chat` with any allowed model, `ask` (Ask your code) grounded in Nexus Wiki. */
export type ChatMode = "chat" | "ask";

/** an internal decision: a Wiki section an Ask your code answer cites as `[n]`. */
export interface ChatCitation {
  n: number;
  /** `owner/name` */
  repo: string;
  page: string;
  slug: string;
  section?: string;
  /** `<slug>#<anchor>` in the repository's wiki. */
  path: string;
  /** Repository files the section names. */
  files?: string[];
  /** The Nexus Wiki run that wrote the page. */
  runId: string;
}

/** `GET /v1/chat/sessions` (one row), `POST`/`PATCH /v1/chat/sessions…`: a session, private to its author. */
/** an internal decision sessions; the state fields (`ChatSessionState`: visibility, sharing, artifacts, author, forkedFrom) come from the Rust gateway only. */
export interface ChatSessionSummary extends ChatSessionState {
  id: string;
  title: string;
  mode: ChatMode;
  /** The model asked for (`nexus/auto` by default). */
  model: string;
  /** The model that ran last. */
  lastModel?: string;
  /** The team its spend is attributed to. */
  team?: string;
  archived: boolean;
  /** Rust gateway only: whether the model may search the web in this session. It works only while the organization's `webSearch.engine` is on. */
  webSearch?: boolean;
  messageCount: number;
  costUsd: number;
  createdAt: string;
  updatedAt: string;
}

/** What a stored chat 402 keeps of its details: short codes and ways forward, and amounts as decimal strings. */
export interface ChatStoredErrorDetails {
  /** Why it was refused, such as `authentication_required` or `disputed` for `payment_failed`. */
  reason?: string;
  level?: string;
  freeStart?: string;
  next?: string[];
  notified?: boolean;
  owedUsd?: string;
  unpaidUsd?: string;
  remainingUsd?: string;
  remainingUsdDecimal?: string;
  requestedUsd?: string;
  limitUsd?: string;
  chargeUsd?: string;
  capUsd?: string;
}

export interface ChatSuggestion {
  suggestion: string | null;
  model: string | null;
  costUsd: number;
}

export interface ChatSessionMessage {
  id: string;
  sessionId: string;
  role: "user" | "assistant";
  content: string;
  /** `refused`: the policy or the budget refused the call (`error` says why); `failed`: it broke mid-way. */
  status: "complete" | "failed" | "refused";
  /** Retained for the author, but excluded from future model context after a rewind. */
  rewound?: boolean;
  mode?: ChatMode;
  requestedModel?: string;
  model?: string;
  costUsd?: number;
  usage?: { promptTokens: number; completionTokens: number };
  citations?: ChatCitation[];
  /**
   * `notified`: a budget refusal told the organization's owners and admins. `remainingUsdDecimal`: what a budget
   * refusal left, so Nexus UI can say whether the call may cost more than that. `requestedUsd` and
   * `remainingUsd`: a usage-limit refusal's hold and what the month's limit had left, as decimal strings.
   * `details`: a 402's details as the live error's `error.nexus` had them, cut to an allow-list; absent on older answers.
   */
  error?: {
    code: string;
    message: string;
    status: number;
    notified?: boolean;
    remainingUsdDecimal?: string;
    requestedUsd?: string;
    remainingUsd?: string;
    details?: ChatStoredErrorDetails;
  };
  createdAt: string;
  /** Rust gateway only: the images attached to a question (`getChatAttachment` has their bytes). */
  attachments?: ChatAttachmentRef[];
  /** Rust gateway only: a page this answer created or revised through `write_page`. */
  page?: ChatPageBlock;
  /** Rust gateway only: the skills this answer loaded through `load_skill`, in the order it loaded them. */
  skills?: ChatSkillBlock[];
  /** Rust gateway only: the connector reads this answer ran through `read_connector`, in order; never what they returned. */
  connectors?: ChatConnectorBlock[];
  /** Rust gateway only: the web pages this answer's searches gave the model, one per URL. Only the author reads them. */
  sources?: ChatWebSource[];
}

/** A web page a chat answer's `web_search` read, and the first query that found it. No snippet is kept. */
export interface ChatWebSource {
  title: string;
  url: string;
  /** As the engine gave it, cut as the model read it. */
  date?: string;
  query: string;
}

/** A page block in a chat answer. The source stays in the Pages store. */
export interface ChatPageBlock {
  pageId: string;
  version: number;
  title: string;
  format: PageFormat;
}

/** Where a skill a chat answer loaded came from: the person's own skills, a team's library, or the organization's. */
export type ChatSkillScope = "personal" | "team" | "org";

/**
 * A skill a chat answer loaded through `load_skill`: which one, which version, and where it came from. The file stays in the
 * skills store. For a library skill, `skillId` is the library skill's id (`skill` and `skillVersion` take it) and `version`
 * the approved version that was served.
 */
export interface ChatSkillBlock {
  skillId: string;
  name: string;
  version: number;
  /** Where it came from. Left out on an answer stored before C2: read it as `personal`. */
  scope?: ChatSkillScope;
  /** The team (`team_…`), for scope `team`. Map it to the team's name with `teams()`. */
  teamId?: string;
}

/** A connector read a chat answer ran through `read_connector`: which connector, which operation, and its request. */
export interface ChatConnectorBlock {
  connector: string;
  operation: string;
  input: Record<string, unknown>;
  /** The connection the read ran with: whose it is and the account's name. Absent while the read streams, and on reads kept before it was recorded. */
  account?: { scope: "org" | "team" | "user"; name: string };
}

/** An image attached to a question, without its bytes (`getChatAttachment`; in a shared artifact, `getArtifactAttachment`). */
export interface ChatAttachmentRef {
  id: string;
  /** `file`: a data file, which a page written in the chat keeps a copy of. */
  type: "image" | "file";
  /** An image's from the bytes, not as it was named; a data file's from its extension, checked against the bytes. */
  mediaType: ChatImageType | DataFileType;
  bytes: number;
  width?: number;
  height?: number;
  name?: string;
}

export type ChatImageType = "image/png" | "image/jpeg" | "image/gif" | "image/webp";

/** `GET /v1/chat/sessions/{id}/attachments/{attachmentId}` (Rust gateway only): one image with its bytes. */
export interface ChatAttachment extends Omit<ChatAttachmentRef, "type"> {
  sessionId: string;
  messageId: string;
  /** Standard base64: `data:${mediaType};base64,${data}` shows it. */
  data: string;
  createdAt: string;
}

/** An image to attach to a question: base64 or a `data:` URL, and its file name. */
export interface ChatImageInput {
  data: string;
  name?: string;
}

/** A data file to attach to a question: base64 or a `data:` URL, and its name with its extension. */
export interface ChatFileInput {
  data: string;
  name: string;
}

/**
 * The data files Gateway takes in one question (Rust gateway only), by extension. Their bytes count against
 * `CHAT_IMAGE_LIMITS.totalBytes` with the question's images.
 */
export const CHAT_FILE_LIMITS = {
  count: 8,
  bytes: 5 * 1024 * 1024,
  types: {
    csv: "text/csv",
    tsv: "text/tab-separated-values",
    json: "application/json",
    txt: "text/plain",
    md: "text/markdown",
    markdown: "text/markdown",
    xlsx: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  },
} as const satisfies { count: number; bytes: number; types: Record<string, DataFileType> };

/** What Gateway takes in one question (Rust gateway only): `images` items, bytes each, bytes in all. */
export const CHAT_IMAGE_LIMITS = { count: 8, bytes: 5 * 1024 * 1024, totalBytes: 20 * 1024 * 1024, side: 8000 } as const;

/** `GET /v1/chat/sessions/{id}` */
export interface ChatSessionDetail extends ChatSessionSummary {
  messages: ChatSessionMessage[];
}

export interface ChatSessionsResponse {
  sessions: ChatSessionSummary[];
  nextCursor?: string;
}

export interface ChatSessionsQuery {
  limit?: number;
  cursor?: string;
  /** Words in the title or the messages. */
  q?: string;
  archived?: "exclude" | "include" | "only";
}

export interface CreateChatSessionRequest {
  title?: string;
  mode?: ChatMode;
  model?: string;
  team?: string;
  /** Rust gateway only: let the model search the web in this session (default `true`). */
  webSearch?: boolean;
}

export interface SendChatMessageRequest {
  /** May be empty when `images` or `files` has at least one. */
  content: string;
  /** Rust gateway only: up to `CHAT_IMAGE_LIMITS.count` images, PNG, JPEG, GIF or WebP. */
  images?: ChatImageInput[];
  /** Rust gateway only: up to `CHAT_FILE_LIMITS.count` data files, CSV, TSV, JSON, TXT, Markdown or XLSX. */
  files?: ChatFileInput[];
  /** For this message and the next ones. */
  model?: string;
  mode?: ChatMode;
  team?: string;
}

/**
 * `POST /v1/chat/ask` (Rust gateway): Ask your code alone, with the `chat.ask` permission. Without `session`
 * Gateway starts a new ask-mode session of the person; an agent link may continue only a session it started.
 */
export interface AskCodeRequest {
  content: string;
  session?: string;
  model?: string;
}

/**
 * `POST /v1/chat/web` (Rust gateway): web search alone, with the `chat.web` permission. The turn offers the
 * `web_search` tool and no other. Without `session` Gateway starts a new chat session of the person with `webSearch`
 * on; an agent link may continue only a session it started. While the organization's search is off it is a 403
 * `web_search_off`.
 */
export interface AskWebRequest {
  content: string;
  session?: string;
  model?: string;
}

/** `POST /v1/chat/web` with `stream: false`: the question and the answer, and the pages the answer read. */
export interface ChatWebResult {
  session: ChatSessionSummary;
  /** The question, then the answer. */
  messages: [ChatSessionMessage, ChatSessionMessage];
  /** The answer's own `sources`; absent when the turn ran no search. */
  sources?: ChatWebSource[];
}

/** `POST /v1/chat/sessions/{id}/messages` with `stream: false`. */
export interface ChatMessageResult {
  session: ChatSessionSummary;
  /** The question, then the answer. */
  messages: [ChatSessionMessage, ChatSessionMessage];
  citations?: ChatCitation[];
}

/**
 * One server-sent event of `POST /v1/chat/sessions/{id}/messages` (OpenAI's `chat.completion.chunk`): the first
 * carries `nexus.session` and `nexus.question`, the middle ones `delta.content`, the last `finish_reason`, `usage`
 * and `nexus.message` (the stored answer). A failure mid-way arrives as `{ error }`; `data: [DONE]` ends the stream.
 */
export interface ChatStreamChunk {
  id: string;
  object: "chat.completion.chunk";
  created: number;
  model: string | null;
  choices: { index: number; delta: { role?: "assistant"; content?: string }; finish_reason: string | null }[];
  usage?: { prompt_tokens: number; completion_tokens: number; total_tokens: number };
  nexus?: {
    session?: ChatSessionSummary;
    question?: ChatSessionMessage;
    message?: ChatSessionMessage;
    mode?: ChatMode;
    requested?: string;
    citations?: ChatCitation[];
    /** Ask your code: the organization has no Nexus Wiki yet (nothing was charged). */
    no_wiki?: boolean;
    /** Present while chat is executing a tool: `write_page` or `edit_page`, `load_skill` for a person with skills, a connector tool, or `web_search` in a session that searches the web. */
    tool?: { name: "write_page" | "edit_page" | "load_skill" | "describe_connector" | "read_connector" | "web_search"; status: "running" };
    /** Rust gateway only: a skill `load_skill` has just read. */
    skill?: ChatSkillBlock;
    /** Rust gateway only: a connector read `read_connector` is about to run. */
    connector?: ChatConnectorBlock;
    cost_usd?: number;
  };
}

/**
 * `GET /v1/guest/chat` (Rust gateway): guest chat for a signed-out visitor. `enabled` says whether this
 * guest id and its address may send a round now: the id has rounds and output tokens left, and the address has rounds
 * left today. A spent guest budget or an empty spend bucket shows only on the next round (429 `guest_chat_paused`).
 * `model` is the display name of the model every guest round runs on.
 */
export interface GuestChatStatus {
  enabled: boolean;
  rounds_used: number;
  rounds_limit: number;
  model: string;
}

/** A turn of the conversation the browser holds for a guest. Gateway stores none of it. */
export interface GuestChatMessage {
  role: "user" | "assistant";
  content: string;
}

/**
 * `POST /v1/guest/chat/messages` (Rust gateway): the whole conversation, its last message the guest's, at
 * most 20 messages and 32,000 characters; and a Cloudflare Turnstile token. The model is always the guest model, which `GuestChatStatus.model` names.
 */
export interface GuestChatRequest {
  messages: GuestChatMessage[];
  turnstile: string;
}

/** The organization's web search settings (Rust gateway). Web search is off until an owner or admin sets an engine. */
export interface WebSearchSettings {
  /** Gates hosted web search in chat and in Code (`nexus claude --api`, `nexus codex --api`). */
  "webSearch.engine": "off" | "borrowed";
  /** The model that runs a chat's search: null for the default (the chat's own model, the cheapest on auto), `cheapest`, or a model's catalog id. Chat only. */
  "webSearch.model": string | null;
  /** Domains a search result may (allow, when set) or may not (deny) come from. While any is set, no web search runs, in chat or in Code, until the engine applies them. */
  "webSearch.domains": { allow: string[]; deny: string[] };
  /** Most searches in one chat turn: 1 to 10 (3 by default). Gateway-run engines (chat) only. */
  "webSearch.maxPerTurn": number;
  /** Whether only engines with zero data retention may run (false by default). While set, no web search runs, in chat or in Code, until the engine applies it. */
  "webSearch.requireZdr": boolean;
}

/**
 * What leaves a person's machine when coding sessions sync, end-to-end encrypted: `never` (only usage
 * numbers), `user` (only the person's own devices read it) or `user_and_org` (also the org's recovery key, which the org
 * holds). `user_and_org` reads as `user`, and a write of it is refused, until the org registers its recovery key
 * (`OrgRecoveryKey`).
 */
export type SessionContent = "never" | "user" | "user_and_org";

/**
 * The organization's recovery key (Rust gateway): the public half of a key pair its vault holds. Runners lock
 * thread content keys to it under `user_and_org`. `fingerprint` is lower-case hex SHA-256 of `nexus-holder-fingerprint-v1`
 * and the key's 32 bytes; members confirm it from a source that is not Gateway before their clients lock for the key.
 */
export interface OrgRecoveryKey {
  id: string;
  encryptionKey: string;
  fingerprint: string;
  registeredBy: string;
  createdAt: string;
}

/** `GET /v1/orgs/recovery-key`: `key` is null until the organization registers one. */
export interface OrgRecoveryKeyState {
  key: OrgRecoveryKey | null;
}

export type MobilePlace = "assistant" | "artifacts" | "customize";

/**
 * `GET|PUT /v1/orgs/settings`. The Rust gateway adds the `artifacts.external*` settings (Nexus Artifacts
 * outside the organization, an internal decision limits): see `ArtifactSharingSettings`. It adds the `webSearch.*` settings too.
 */
export interface OrgSettings extends Partial<ArtifactSharingSettings>, Partial<WebSearchSettings> {
  /** Places an owner hides in members’ mobile drawers. */
  hiddenMobilePlaces?: MobilePlace[];
  /** Days chat messages are kept: 1 to 3650 (90 by default). */
  chatRetentionDays: number;
  /** Whether `nexus claude|codex --subscription` may run (people's own Claude and ChatGPT logins, outside Gateway's checks). */
  agentSubscriptions: boolean;
  /** Whether the Gateway records the organization's growth events in its own store (Rust gateway; on by default). */
  growthEvents?: boolean;
  /** Whether chat sessions suggest the person's next message after an answer, a call the organization pays for (Rust gateway; on by default). */
  chatSuggestions?: boolean;
  /** The content policy for synced sessions that runners follow (Rust gateway). Every member reads it; changing it needs `synced_sessions.manage_policy`. */
  sessionContent?: SessionContent;
  /** Whether members read each other's team-visible synced sessions (Rust gateway, ADR 0011; off by default, always off in a personal one). */
  multiplayerSessions?: boolean;
  /** Whether the Gateway stores each cloud session's history so it opens on any device (Rust gateway, ADR 0013; on by default). Off, it stores nothing and erases what it stored. */
  cloudHistory?: boolean;
  /**
   * Whether Nexus UI in a browser opens the organization's synced sessions (Rust gateway; on by default). Off,
   * Gateway refuses Nexus UI their sealed events, their key copies and a new device: 403 `web_session_viewer_off`.
   * Changing it needs `synced_sessions.manage_policy`.
   */
  webSessionViewer?: boolean;
  defaults: { chatRetentionDays: number; agentSubscriptions: boolean; growthEvents?: boolean; chatSuggestions?: boolean; hiddenMobilePlaces?: MobilePlace[]; sessionContent?: SessionContent; multiplayerSessions?: boolean; cloudHistory?: boolean; webSessionViewer?: boolean } & Partial<ArtifactSharingSettings> &
    Partial<WebSearchSettings>;
  updatedAt?: string;
}

interface ChatUsageFigures {
  sessions: number;
  messages: number;
  costUsd: number;
  /** Rust gateway: the exact decimal string of `costUsd` (`"12.245000"`). Prefer it; `costUsd` is deprecated (root rule 9). */
  costUsdDecimal?: string;
}

/** `GET /v1/analytics/chat`: counts only, never content or titles. */
export interface ChatUsageReport {
  scope: "org" | "own";
  days: number;
  since: string;
  retentionDays: number;
  totals: ChatUsageFigures & { people: number };
  byMode: Record<ChatMode, ChatUsageFigures>;
  byTeam: (ChatUsageFigures & { team: string | null })[];
  byPerson?: (ChatUsageFigures & { userId: string; email?: string })[];
}

/**
 * `POST /v1/chat/voice-token` (Rust gateway): what a client needs to dictate. Open `url` (the token is in it) within
 * `expiresInSeconds` and send the microphone as binary frames of 16-bit little-endian mono PCM at `sampleRate`. A
 * session runs at most `maxSessionSeconds`. AssemblyAI's `Turn` messages carry the words.
 */
export interface VoiceToken {
  token: string;
  url: string;
  sampleRate: number;
  expiresInSeconds: number;
  maxSessionSeconds: number;
}

/** The longest text one `POST /v1/chat/voice-speech` says, in characters. Split a longer answer. */
export const VOICE_SPEECH_MAX_CHARS = 500;

/**
 * `POST /v1/chat/voice-speech`: a piece of a voice-mode answer for the Gateway to say with ElevenLabs. The answer is
 * `audio/mpeg`. A 503 `voice_speech_not_configured` means the Gateway has no key: keep the device's own speech engine.
 */
export interface VoiceSpeechRequest {
  text: string;
}

/**
 * `POST /v1/diagnostics/reports` (Rust gateway): one error or crash from the app, with no content. The fields
 * are all there is, each one bounded (`openapi.json`), and the free text is redacted before it is sent.
 */
export interface DiagnosticReport {
  /** `error`: in the app's JavaScript; `crash`: a native crash (the OS's, or a desktop panic) seen at the next start. */
  kind: "error" | "crash";
  /** Whether it ended the app. */
  fatal: boolean;
  name: string;
  message: string;
  /** The top frames, at most 64. */
  stack: string[];
  occurredAt: string;
  appVersion: string;
  appBuild?: string | null;
  platform: DiagnosticPlatform;
  osVersion: string;
}

/** Where a report or feedback comes from: the phone app, the desktop app or Nexus UI in a browser. */
export type DiagnosticPlatform = "ios" | "android" | "macos" | "windows" | "linux" | "web";

/** `POST /v1/diagnostics/frame-failures` and its public counterpart. */
export interface FrameFailure {
  surface: "api" | "share_link" | "subdomain";
  kind: "error" | "timeout" | "blocked" | "csp" | "http";
  app: "web" | "desktop-gpui";
  appVersion: string;
  browser?: "chrome" | "safari" | "firefox" | "edge" | "webkit" | "other";
  browserVersion?: string;
  artifactId?: string;
  host?: string;
  status?: number;
  occurredAt?: string;
}

export interface FrameFailures {
  failures: FrameFailure[];
}

export interface FrameFailuresAccepted {
  accepted: number;
}

/**
 * What feedback is. `crash` carries the crash the person chose to send, and only it does; `report` carries the model's
 * answer the person reported (App Review 1.2), and only it does. `triage` is a defect report that
 * `nexus triage`'s agent wrote, sent only by Nexus's own people.
 */
export type FeedbackCategory = "bug" | "idea" | "other" | "crash" | "report" | "triage";

/** Why a person reported an answer: offensive, abusive or harmful; wrong or misleading; or something else. */
export type AnswerReportReason = "offensive" | "wrong" | "other";

/** A model's answer that a person reported, as the app showed it. */
export interface ReportedAnswer {
  reason: AnswerReportReason;
  /** The answer's text: 1 to 12000 characters (`MAX_REPORTED_ANSWER_CHARS`), not only white space. */
  text: string;
  /** The model that wrote it, when the app knows it: at most 200 characters, on one line. */
  model?: string | null;
}

/** The Gateway's bound on a reported answer's text, in characters. */
export const MAX_REPORTED_ANSWER_CHARS = 12_000;

/**
 * `POST /v1/diagnostics/feedback` (Rust gateway): what a person chose to send to Nexus's team. The Gateway
 * posts it to the feedback channel in Slack, with the person's name, email and organization, and keeps no copy.
 */
export interface Feedback {
  category: FeedbackCategory;
  /** What the person wrote: 1 to 4000 characters, or empty with a crash or a report. */
  message: string;
  /** The screen the person was on, at most 120 characters. */
  screen?: string | null;
  appVersion: string;
  appBuild?: string | null;
  platform: DiagnosticPlatform;
  osVersion: string;
  /** With the category `crash`: the crash, built like any other report and logged as one too. */
  crash?: DiagnosticReport | null;
  /** With the category `report`: the answer the person reported. */
  answer?: ReportedAnswer | null;
}

/** One person who joined through your referral link (`GET /v1/referrals`). */
export interface ReferredPerson {
  /** Whether this referral earned you a reward. Absent or null when the Gateway does not report it. */
  rewarded?: boolean | null;
  /** This referral's reward. Absent on older Gateways. */
  reward?: ReferralReward;
  /** ISO time the person signed up. */
  joinedAt: string;
  /** The email with its name part masked ("b***@example.com"). Null once the person is erased, or while Nexus has none for them. */
  email: string | null;
  /**
   * Whether the person erased their account. A null email on a person who is not erased means Nexus has no
   * email for them yet, and their reward needs one. Absent on older Gateways: read a null email as erased.
   */
  erased?: boolean;
}

/** `GET /v1/referrals` (Rust gateway): your referral code and a page of the people who joined through it. */
export interface ReferralList {
  /** False while the referral program is stopped. Absent on older Gateways. */
  enabled?: boolean;
  code: string;
  referred: ReferredPerson[];
  /**
   * Lifetime referral rewards for the person's personal organization, across all pages, against its limit. Absent for a
   * staff member, whose account has no limit, and on older Gateways.
   */
  rewardProgress?: ReferralRewardProgress;
  /** How many referral rewards the person's personal organization got in all, limit or not. Absent on older Gateways. */
  rewardsGranted?: number;
  /** Pass it back as `cursor` for the next page. Absent on the last page. */
  nextCursor?: string;
  /** While sign-up is invite-only: how many more people the code admits. Absent while sign-up is open. */
  invitesLeft?: number;
  /**
   * While sign-up is invite-only: how many people Nexus approved the code to admit in all, used or not. 0:
   * not approved. Absent while sign-up is open, and on older Gateways.
   */
  invitesAllowed?: number;
  /** What one rewarded referral gives you, six decimals (`"1.000000"`); `"0.000000"` while none is set. */
  rewardUsdDecimal?: string;
  /**
   * Whether your referrals earn rewards now: Nexus pays them, and you are below your lifetime limit. Absent
   * on older Gateways: read it as false, and promise no money.
   */
  rewardsActive?: boolean;
}

/**
 * A referral's reward. `pending`: none yet, and one may come (a payment is never disclosed before the
 * reward). `rewarded`: you got it. `not_rewarded`: final, it never will: its reward was voided (no reason is given), you
 * reached the lifetime limit, or you have no personal organization.
 */
export type ReferralReward = "pending" | "rewarded" | "not_rewarded";

/** Lifetime referral rewards for the person's personal organization, including credit already spent or expired. */
export interface ReferralRewardProgress {
  granted: number;
  limit: number;
}

/**
 * `GET /v1/ui/referrals/{code}` (Rust gateway, Nexus UI's server with its shared secret): who invited a person whose
 * sign-up carries the code, for the sign-up page. `inviter` is absent for a campaign's code or a person with no name. A
 * code that would not admit and record the sign-up is a 404 `referral_not_found`. Never the person's email or id.
 */
export interface ReferralInvite {
  inviter?: { name: string };
}

/** A successful employee download check. Refusals use HTTP 401 or 403. */
export interface DesktopDownloadAccess { allowed: true }

/**
 * A key holder's kind: a device, or one of your recovery keys. A `passkey` (WebAuthn PRF) or a printable
 * `recovery_code` has an X25519 key and no Ed25519 key: it receives locked copies and never signs.
 */
export type HolderKind = "device" | "passkey" | "recovery_code";

/**
 * One of your devices or recovery keys in the key directory (`GET /v1/devices`): its public keys, its state,
 * and the approval a client checks itself. Not the CLI's device sign-in (`DeviceCodeResponse`). The contract names it
 * `Device`.
 */
export interface E2eDevice {
  /** `dev_` and 12 characters, minted by Gateway. */
  id: string;
  name: string;
  /** Absent from an older Gateway: a device. */
  kind?: HolderKind;
  /** X25519 public key, base64url. */
  encryptionKey: string;
  /** Ed25519 public key, base64url. Null for a recovery key. */
  signingKey: string | null;
  state: "pending" | "trusted" | "revoked";
  /** The device that approved it; null for a device trusted without an approval and for one never approved. */
  approvedBy: string | null;
  approvalSignature: string | null;
  createdAt: string;
  approvedAt: string | null;
  revokedAt: string | null;
  /**
   * The approver's trusted list, signed and sealed to this device. Null for a root, a recovery key and an
   * approval sent without one; absent from an older Gateway. Only this device opens it.
   */
  handover?: string | null;
}

/** `GET /v1/devices`: your devices, oldest first, revoked ones included. */
export interface DeviceList {
  devices: E2eDevice[];
  /**
   * How this Gateway trusts your later devices (`NEXUS_DEVICE_TRUST`). `gateway`: each device you register is trusted
   * at once, with no approver. Absent from an older Gateway: read that as `approvals`.
   */
  trust?: "approvals" | "gateway";
}

/** `POST /v1/devices`: your first device is trusted at once; anything later is pending until a trusted device approves it. */
export interface RegisterDeviceRequest {
  name: string;
  /** `device` when absent. */
  kind?: HolderKind;
  encryptionKey: string;
  /** Required for a device; absent for a recovery key. */
  signingKey?: string;
}

/**
 * `POST /v1/devices/recover`, the way back: the new device's keys and the proof of one of your recovery keys,
 * the challenge Gateway sealed to it, opened. Without the proof Gateway answers 403 `recovery_proof_required`.
 */
export interface RecoverDeviceRequest {
  name: string;
  encryptionKey: string;
  signingKey: string;
  holderId: string;
  /** The opened challenge: 32 bytes, base64url. */
  challenge: string;
}

/** `POST /v1/devices/recover/challenge`: one challenge per trusted recovery key, each sealed to it. Single use. */
export interface RecoveryChallenge {
  challenges: { holderId: string; kind: Exclude<HolderKind, "device">; sealed: string }[];
  expiresAt: string;
}

/** One thread's locked copy, at a key version you hold, for the device being approved. */
export interface DeviceThreadKey {
  threadId: string;
  keyVersion: number;
  lockedKey: string;
  /** The approver's signature over `nexus-thread-key:v1:{threadId}:{keyVersion}:{deviceId}:{lockedKey}`. */
  signature: string;
}

/** `POST /v1/devices/{id}/approve`. */
export interface ApproveDeviceRequest {
  approverDeviceId: string;
  /**
   * Ed25519 over `nexus-device-approval:v1:{deviceId}:{encryptionKey}:{signingKey}` for a device, or
   * `nexus-device-approval:v2:{holderId}:{kind}:{encryptionKey}:` for a recovery key, base64url.
   */
  signature: string;
  keys?: DeviceThreadKey[];
  /**
   * For a device only: the approver's trusted holders and remembered revoked ids, signed by it and HPKE-sealed to the
   * new device, base64url. Gateway stores it unread.
   */
  handover?: string;
}

/** `POST /v1/devices/{id}/revoke`. */
export interface RevokeDeviceRequest {
  revokerDeviceId: string;
  /** Ed25519 over `nexus-device-revoke:v1:{deviceId}`, base64url. */
  signature: string;
}

/**
 * One device's locked copy of a thread's content key at one version (`GET /v1/threads/{threadId}/keys`,
 * `GET /v1/devices/{id}/keys`). Check `signature` with the sender's `signingKey` before you use it.
 */
export interface ThreadKey {
  threadId: string;
  /** From 1: the content key's epoch plus one. */
  keyVersion: number;
  deviceId: string;
  /** HPKE-sealed to the device's `encryptionKey`, base64url. */
  lockedKey: string;
  senderDeviceId: string;
  /** The sender's Ed25519 signature over `nexus-thread-key:v1:{threadId}:{keyVersion}:{deviceId}:{lockedKey}`. */
  signature: string;
  createdAt: string;
}

/** One epoch of the organization's team key (ADR 0011, M5). */
export interface TeamKey {
  id: string;
  epoch: number;
  encryptionKey: string;
  createdBy: string;
  createdAt: string;
  retiredAt: string | null;
  retiredReason: string | null;
}

/** One holder's copy of one team-key epoch's private half. */
export interface TeamKeyCopy {
  teamKeyId: string;
  epoch: number;
  holderId: string;
  lockedKey: string;
  senderDeviceId: string;
  signature: string;
  createdAt: string;
}

/** `GET /v1/orgs/team-key?holder=`: the newest epoch, and the holder's copies of every epoch. */
export interface TeamKeyState {
  key: TeamKey | null;
  copies: TeamKeyCopy[];
}

/** A thread's content key at one version, locked to one team-key epoch (M6). */
export interface TeamThreadKey {
  threadId: string;
  keyVersion: number;
  teamKeyId: string;
  /** The thread's person. */
  personId: string;
  lockedKey: string;
  senderDeviceId: string;
  signature: string;
  createdAt: string;
}

/** `GET /v1/threads/{threadId}/team-keys`. */
export interface TeamThreadKeyList {
  threadId: string;
  keys: TeamThreadKey[];
}

/** A member's trusted holder, never its name (M7). */
export interface TeamKeyHolder {
  id: string;
  /** Missing on older Gateways. Observed revocations remain final on this client. */
  state?: "trusted" | "revoked";
  kind: HolderKind;
  encryptionKey: string;
  signingKey: string | null;
  approvedBy: string | null;
  approvalSignature: string | null;
  approvedAt: string | null;
  hasCurrentTeamKey: boolean;
}

/** `GET /v1/orgs/members/{userId}/holders`. */
export interface TeamKeyHolderList {
  personId: string;
  teamKeyId: string | null;
  epoch: number | null;
  holders: TeamKeyHolder[];
}

/** `PUT /v1/threads/{threadId}/keys`: copies of one key version, each for a trusted device or recovery key. */
export interface PutThreadKeysRequest {
  keyVersion: number;
  senderDeviceId: string;
  keys: { deviceId: string; lockedKey: string; signature: string }[];
}

/** `PUT /v1/threads/{threadId}/keys`'s answer. */
export interface ThreadKeysStored {
  threadId: string;
  keyVersion: number;
  stored: string[];
  alreadyPresent: string[];
}

/** `GET /v1/devices/{id}/keys`: a device's copies, in thread and key version order. */
export interface ThreadKeyList {
  keys: ThreadKey[];
  /** The `after` of the next page (`{threadId}:{keyVersion}`); null on the last. */
  nextAfter: string | null;
}

/** The session store: a thread's state, as an event reports it. */
export type SyncedStatus = "running" | "completed" | "interrupted" | "error";

/** One model call's usage, readable beside the sealed body. Gateway bills nothing from it. */
export interface SyncedUsage {
  model: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  /** A decimal string. */
  costUsd: string;
}

/** One event of a synced thread. Its body is sealed with the thread's content key at `keyVersion`. */
export interface SyncedEvent {
  /** 0 first, one more each time. */
  seq: number;
  /** The runner's time, in milliseconds since 1970. */
  at: number;
  /** The coding agent, as the runner's workbench names it: `claude`, `codex` or `nexus`. */
  agent: string;
  usage?: SyncedUsage | null;
  status?: SyncedStatus | null;
  keyVersion?: number | null;
  /** base64url without padding. Null when the organization's content never leaves the machine. */
  sealed?: string | null;
}

export type RatingAudience = "personal" | "organization";
export type AnswerRating = "good" | "bad" | "none";
export type AnswerRatingReason = "wrong" | "instructions" | "slow" | "other";

export interface SetThreadAnswerRating {
  audience: RatingAudience;
  rating: AnswerRating;
  reason?: AnswerRatingReason | null;
  detail?: string | null;
  expectedVersion: number;
}

export interface ThreadAnswerRating {
  answerSeq: number;
  rating: AnswerRating;
  reason: AnswerRatingReason | null;
  detail: string | null;
  version: number;
}

export interface ThreadAnswerRatings {
  audience: RatingAudience;
  ratings: ThreadAnswerRating[];
  nextBefore: number | null;
}

/** A synced thread's readable summary (`GET /v1/threads`). Its title and repository are inside the sealed events. */
export interface SyncedThread {
  threadId: string;
  /** The thread's person (`users.id`): you, unless a teammate or an auditor reads it. */
  personId: string;
  /** The latest event's agent; null before the first event. */
  agent: string | null;
  model: string | null;
  status: SyncedStatus | null;
  totals: { inputTokens: number; outputTokens: number; cacheReadTokens: number; cacheWriteTokens: number; costUsd: string };
  /** The highest event number stored; null before the first event. */
  lastSeq: number | null;
  /** The newest content key version of a sealed event; null before one. */
  keyVersion: number | null;
  /** Whether its owner keeps it from their teammates (ADR 0011); otherwise team-visible. */
  private?: boolean;
  /** Who follows it: members' ids, in the order they followed it. Absent from a Gateway before it. */
  followers?: string[];
  lastEventAt: string | null;
  createdAt: string;
  updatedAt: string;
}

/** `GET /v1/threads`: newest first. */
export interface SyncedThreadList {
  threads: SyncedThread[];
  nextBefore: string | null;
}

/** `GET /v1/threads/{threadId}/events`: in number order. */
export interface SyncedEventPage {
  threadId: string;
  lastSeq: number | null;
  events: SyncedEvent[];
  /** More events follow in the direction read: after the last, or before the first with `before`. */
  more: boolean;
}

export interface SyncedThreadDeleted {
  threadId: string;
  deleted: boolean;
  eventsDeleted: number;
  keysDeleted: number;
}

export type SessionPinSource = "chat" | "synced";
export interface SessionPin {
  source: SessionPinSource;
  sessionId: string;
  pinned: boolean;
  version: number;
  updatedAt: string | null;
}
export interface SessionPinList {
  pins: SessionPin[];
  chats: ChatSessionSummary[];
  threads: SyncedThread[];
}
export interface SetSessionPin {
  pinned: boolean;
  expectedVersion: number;
}

/** `thread` on `GET /v1/threads/{threadId}/stream`: the thread now holds every event up to `lastSeq`. */
export interface SyncedThreadAdvanced {
  threadId: string;
  lastSeq: number;
}

/** `presence` on `GET /v1/threads/{threadId}/stream` (ADR 0011, M11): one watcher's place on the thread, never stored. */
export interface SyncedThreadPresence {
  threadId: string;
  /** Gateway's name for the watcher, never the client's. */
  from: PagePeer;
  /** The watcher's window or device. */
  client: string;
  /** The event number on the watcher's screen; null before any shows, or when they left. */
  seq: number | null;
  leave: boolean;
}

/** `PUT /v1/apps/{id}/cap`: the app owner's monthly hosting cap, in whole cents. */
export interface SetAppCapRequest { monthlyCapUsd: string }
