/**
 * Passwords and payments (Assistant A8), for every app: the agent API's `/agent/vault` routes, read
 * strictly, and the words each row shows.
 *
 * The rule: Nexus never sees, stores or repeats a password or a card number. The person's password manager fills a
 * login in, and the card's own page takes a payment. The agent API sends names, sites, masked accounts and grants only.
 * Each reader here refuses a body that carries a secret's shape (`holdsSecret`), so a bug upstream shows nothing
 * rather than a secret.
 *
 * - `GET /agent/vault`: the linked password manager, the logins Nexus may ask to use and their grants, the card and its
 *   limits per merchant, and the grants that wait for the person (`asks`).
 * - `PUT /agent/vault/logins/{id}` with `{ grant }`: each use, until a date, or `none` (Revoke).
 * - `PUT /agent/vault/card` with `{ frozen }`: Freeze, or unfreeze.
 * - `POST /agent/vault/asks/{id}/approve` with `{ for: "once" | "until" }`, and `…/decline`: the grant sheet (A8·b).
 * - `GET /agent/vault/activity`: every use and every payment, who approved it and where (A8·e).
 * - `PUT /agent/vault/index`: the person's desktop sends its vault's index, with the person's own key: the
 *   sources linked there, and each login's name, site, masked account, source, grant and last use. Never a secret: the
 *   logins stay in the Mac's keychain or in 1Password. The answer is the vault as the agent API now serves it.
 * - `POST /agent/vault/asks` with `{ login, task, until? }`: Nexus asks to use one login for one task. The person
 *   approves it on the desktop, with Touch ID. With the person's own credential the answer is the vault as it is now;
 *   with an agent link, only `{ ask }`, the ask it made.
 *
 * The API answers 503 `vault_off` while it has no vault source, and an older API answers 404: both read as `off`.
 */

export const VAULT_PATH = "/agent/vault";
export const VAULT_ACTIVITY_PATH = "/agent/vault/activity";
export const VAULT_CARD_PATH = "/agent/vault/card";
export const VAULT_LOGIN_PREFIX = "/agent/vault/logins/";
export const VAULT_ASK_PREFIX = "/agent/vault/asks/";
export const VAULT_INDEX_PATH = "/agent/vault/index";
export const VAULT_ASKS_PATH = "/agent/vault/asks";
/** The most logins one index holds. */
export const VAULT_INDEX_MAX_LOGINS = 2000;
/** The most activity lines one index holds. */
export const VAULT_INDEX_MAX_ACTIVITY = 200;
/** How long an ask waits for the person. */
export const VAULT_ASK_TTL_MS = 10 * 60_000;
/** A login's, a merchant's or an ask's id. */
export const VAULT_ID = /^[A-Za-z0-9_-]{1,64}$/;

/** The password managers Nexus can ask. `nexus` is the Nexus vault (A8·B). */
export type VaultManagerKind = "1password" | "bitwarden" | "nexus";
export const VAULT_MANAGER_KINDS: readonly VaultManagerKind[] = ["1password", "bitwarden", "nexus"];
export const VAULT_MANAGER_NAMES: Record<VaultManagerKind, string> = { "1password": "1Password", bitwarden: "Bitwarden", nexus: "Nexus vault" };

export interface VaultManager {
  kind: VaultManagerKind;
  /** The vault's name, such as "Maya’s Personal vault". */
  vault: string;
  /** How many logins it holds, when it says (the Nexus vault does). */
  total?: number;
}

/** What one login lets Nexus do: ask at each use, or use it until a date. Each use takes Face ID. */
export type VaultGrant = { kind: "each_use" } | { kind: "until"; until: string };

export interface VaultLogin {
  id: string;
  name: string;
  site: string;
  /** Null: Nexus may not use it (the Nexus vault lists those too). */
  grant: VaultGrant | null;
  /** ISO 8601, or null when never used. */
  lastUsed: string | null;
  /** A weak or reused password, noted quietly (A8·B·e). */
  flag?: "weak" | "reused";
  /** Masked, such as "m•••@gmail.com": four digits at most. Absent when the source gives none. */
  account?: string;
  /** Where the login lives: the Nexus vault on the person's Mac, or their own 1Password. */
  source?: VaultManagerKind;
}

/** The virtual card. Amounts are decimal strings in US dollars (rule 9). */
export interface VaultCard {
  last4: string;
  state: "active" | "frozen";
  spent: string;
  limit: string;
  period: "month";
}

export type VaultApproval = "asks_first" | "face_id" | "off";
export type VaultPer = "order" | "ride" | "booking";

/** One merchant's limit: an amount per order, ride or booking, or none; and how a payment there is approved. */
export interface VaultMerchant {
  id: string;
  merchant: string;
  /** A decimal string, or null for no amount. */
  amount: string | null;
  per: VaultPer;
  approval: VaultApproval;
}

/** A grant that waits for the person (A8·b): one login, one site, one task. */
export interface VaultAsk {
  id: string;
  login: {
    id: string;
    name: string;
    /** Masked, such as "MileagePlus ••• 772": four digits at most. */
    account: string;
  };
  manager: VaultManagerKind;
  site: string;
  task: string;
  /** The "Until" choice the sheet offers, ISO 8601; null offers "This once" alone. */
  until: string | null;
  expiresAt: string;
}

/** `GET /agent/vault`'s answer. `manager: null` is "no password manager is linked"; `card: null`, no card. */
export interface AgentVaultView {
  /** The desktop that sent the index, such as "Maya’s MacBook Pro". Absent before one did. */
  device?: string;
  /** Every source linked on that desktop. `manager` is the first of them, for older readers. */
  sources?: VaultManager[];
  manager: VaultManager | null;
  logins: VaultLogin[];
  card: VaultCard | null;
  merchants: VaultMerchant[];
  applePay: boolean;
  asks: VaultAsk[];
}

/** A write's answer: the vault as it is now. `stopped`: errands a revoke stopped. `decided`: what an ask came to. */
export interface AgentVaultChanged {
  vault: AgentVaultView;
  stopped?: number;
  /** `confirm_on_device`: the approval waits for Face ID on the person's phone. */
  decided?: "approved" | "declined" | "confirm_on_device";
}

/** One line of the activity log (A8·e). `with` and `approved` are null for "—". */
export interface VaultActivityEntry {
  at: string;
  what: string;
  with: string | null;
  approved: string | null;
  result: string;
  outcome: "done" | "approved" | "refused";
}

/** `PUT /agent/vault/logins/{id}`'s body. */
export type VaultLoginChange = { grant: "each_use" | "none" | { until: string } };
/** `POST /agent/vault/asks/{id}/approve`'s body. */
export type VaultAskApproval = { for: "once" | "until" };

/** How a read ended. `off`: the agent API has no vault here. */
export type AgentVaultRead = { kind: "view"; view: AgentVaultView } | { kind: "off" } | { kind: "failed"; status: number };

const SECRET_KEYS = /^(password|passwd|passcode|passphrase|secret|pin|otp|totp|cvc|cvv|pan|card_?number|number)$/i;

/**
 * Whether a body holds a secret's shape: a key named like one (`password`, `cvc`, `cardNumber`), or a run of twelve or
 * more digits, as a card number is, even with spaces or dashes in it. Masked values ("••4021") have four at most.
 */
export function holdsSecret(value: unknown, depth = 0): boolean {
  if (depth > 8) return true;
  if (typeof value === "string") return /\d(?:[ -]?\d){11,}/.test(value);
  if (Array.isArray(value)) return value.some((each) => holdsSecret(each, depth + 1));
  if (value && typeof value === "object") {
    return Object.entries(value).some(([key, each]) => SECRET_KEYS.test(key) || holdsSecret(each, depth + 1));
  }
  return false;
}

const text = (value: unknown, max = 200): value is string => typeof value === "string" && value.trim().length > 0 && value.length <= max;
const isId = (value: unknown): value is string => typeof value === "string" && VAULT_ID.test(value);
const isTime = (value: unknown): value is string => typeof value === "string" && value.length <= 40 && !Number.isNaN(Date.parse(value));
const DECIMAL = /^\d{1,9}(\.\d{1,2})?$/;
const isDecimal = (value: unknown): value is string => typeof value === "string" && DECIMAL.test(value);
/** A masked account: four digits at most. */
const isMasked = (value: unknown): value is string => text(value, 80) && (value.match(/\d/g) ?? []).length <= 4;

function managerOf(value: unknown): VaultManager | null | undefined {
  if (value === null) return null;
  if (!value || typeof value !== "object") return undefined;
  const { kind, vault, total } = value as Record<string, unknown>;
  if (!VAULT_MANAGER_KINDS.includes(kind as VaultManagerKind) || !text(vault, 120)) return undefined;
  if (total !== undefined && (typeof total !== "number" || !Number.isInteger(total) || total < 0)) return undefined;
  return { kind: kind as VaultManagerKind, vault, ...(total !== undefined ? { total } : {}) };
}

function grantOf(value: unknown): VaultGrant | null | undefined {
  if (value === null) return null;
  if (!value || typeof value !== "object") return undefined;
  const { kind, until } = value as Record<string, unknown>;
  if (kind === "each_use") return { kind };
  if (kind === "until" && isTime(until)) return { kind, until };
  return undefined;
}

function loginOf(value: unknown): VaultLogin | undefined {
  if (!value || typeof value !== "object") return undefined;
  const { id, name, site, grant, lastUsed, flag, account, source } = value as Record<string, unknown>;
  const kept = grantOf(grant);
  if (!isId(id) || !text(name, 120) || !text(site, 253) || kept === undefined) return undefined;
  if (lastUsed !== null && !isTime(lastUsed)) return undefined;
  if (flag !== undefined && flag !== "weak" && flag !== "reused") return undefined;
  if (account !== undefined && !isMasked(account)) return undefined;
  if (source !== undefined && !VAULT_MANAGER_KINDS.includes(source as VaultManagerKind)) return undefined;
  return {
    id,
    name,
    site,
    grant: kept,
    lastUsed: lastUsed as string | null,
    ...(flag ? { flag } : {}),
    ...(account !== undefined ? { account } : {}),
    ...(source !== undefined ? { source: source as VaultManagerKind } : {}),
  };
}

function cardOf(value: unknown): VaultCard | null | undefined {
  if (value === null) return null;
  if (!value || typeof value !== "object") return undefined;
  const { last4, state, spent, limit, period } = value as Record<string, unknown>;
  if (typeof last4 !== "string" || !/^\d{4}$/.test(last4) || (state !== "active" && state !== "frozen")) return undefined;
  if (!isDecimal(spent) || !isDecimal(limit) || period !== "month") return undefined;
  return { last4, state, spent, limit, period };
}

function merchantOf(value: unknown): VaultMerchant | undefined {
  if (!value || typeof value !== "object") return undefined;
  const { id, merchant, amount, per, approval } = value as Record<string, unknown>;
  if (!isId(id) || !text(merchant, 120) || (amount !== null && !isDecimal(amount))) return undefined;
  if (per !== "order" && per !== "ride" && per !== "booking") return undefined;
  if (approval !== "asks_first" && approval !== "face_id" && approval !== "off") return undefined;
  return { id, merchant, amount: amount as string | null, per, approval };
}

function askOf(value: unknown): VaultAsk | undefined {
  if (!value || typeof value !== "object") return undefined;
  const { id, login, manager, site, task, until, expiresAt } = value as Record<string, unknown>;
  if (!isId(id) || !login || typeof login !== "object") return undefined;
  const { id: loginId, name, account } = login as Record<string, unknown>;
  if (!isId(loginId) || !text(name, 120) || !isMasked(account)) return undefined;
  if (!VAULT_MANAGER_KINDS.includes(manager as VaultManagerKind) || !text(site, 253) || !text(task, 400)) return undefined;
  if ((until !== null && !isTime(until)) || !isTime(expiresAt)) return undefined;
  return { id, login: { id: loginId, name, account }, manager: manager as VaultManagerKind, site, task, until: until as string | null, expiresAt };
}

function listOf<T>(value: unknown, read: (item: unknown) => T | undefined, max = 500): T[] | undefined {
  if (!Array.isArray(value) || value.length > max) return undefined;
  const items = value.map(read);
  return items.every((item) => item !== undefined) ? (items as T[]) : undefined;
}

/** `GET /agent/vault`'s body, read strictly; undefined when it is not one, or when it holds a secret's shape. */
export function vaultViewOf(body: unknown): AgentVaultView | undefined {
  if (!body || typeof body !== "object" || holdsSecret(body)) return undefined;
  const fields = body as Record<string, unknown>;
  const manager = managerOf(fields.manager);
  const logins = listOf(fields.logins, loginOf, VAULT_INDEX_MAX_LOGINS);
  const card = cardOf(fields.card);
  const merchants = listOf(fields.merchants, merchantOf, 50);
  const asks = listOf(fields.asks, askOf, 20);
  if (manager === undefined || !logins || card === undefined || !merchants || !asks || typeof fields.applePay !== "boolean") return undefined;
  if (fields.device !== undefined && !text(fields.device, 120)) return undefined;
  const sources = fields.sources === undefined ? undefined : listOf(fields.sources, (each) => managerOf(each) ?? undefined, 4);
  if (fields.sources !== undefined && !sources) return undefined;
  return {
    ...(fields.device !== undefined ? { device: fields.device as string } : {}),
    ...(sources ? { sources } : {}),
    manager,
    logins,
    card,
    merchants,
    applePay: fields.applePay,
    asks,
  };
}

/** A write's answer, read strictly. */
export function vaultChangedOf(body: unknown): AgentVaultChanged | undefined {
  if (!body || typeof body !== "object") return undefined;
  const { vault, stopped, decided } = body as Record<string, unknown>;
  const view = vaultViewOf(vault);
  if (!view) return undefined;
  if (stopped !== undefined && (typeof stopped !== "number" || !Number.isInteger(stopped) || stopped < 0)) return undefined;
  if (decided !== undefined && decided !== "approved" && decided !== "declined" && decided !== "confirm_on_device") return undefined;
  return { vault: view, ...(stopped !== undefined ? { stopped } : {}), ...(decided ? { decided } : {}) };
}

function entryOf(value: unknown): VaultActivityEntry | undefined {
  if (!value || typeof value !== "object") return undefined;
  const { at, what, with: using, approved, result, outcome } = value as Record<string, unknown>;
  if (!isTime(at) || !text(what, 200) || !text(result, 120)) return undefined;
  if ((using !== null && !text(using, 120)) || (approved !== null && !text(approved, 120))) return undefined;
  if (outcome !== "done" && outcome !== "approved" && outcome !== "refused") return undefined;
  return { at, what, with: using as string | null, approved: approved as string | null, result, outcome };
}

/** `GET /agent/vault/activity`'s body, `{ entries }`, read strictly. */
export function vaultActivityOf(body: unknown): VaultActivityEntry[] | undefined {
  if (!body || typeof body !== "object" || holdsSecret(body)) return undefined;
  return listOf((body as { entries?: unknown }).entries, entryOf, 1000);
}

/** Whether a status and body say the vault is off here: a 404, or a 503 `vault_off`. */
export function vaultOff(status: number, body: unknown): boolean {
  const error = body && typeof body === "object" ? (body as { error?: unknown }).error : undefined;
  return status === 404 || (status === 503 && error === "vault_off");
}

/** A read from its status and body. */
export function vaultRead(status: number, body: unknown): AgentVaultRead {
  if (vaultOff(status, body)) return { kind: "off" };
  if (status < 200 || status >= 300) return { kind: "failed", status };
  const view = vaultViewOf(body);
  return view ? { kind: "view", view } : { kind: "failed", status: 502 };
}

/** A login change's body, read strictly. */
export function vaultLoginChangeOf(value: unknown): VaultLoginChange | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const keys = Object.keys(value);
  if (keys.length !== 1 || keys[0] !== "grant") return undefined;
  const { grant } = value as { grant: unknown };
  if (grant === "each_use" || grant === "none") return { grant };
  if (grant && typeof grant === "object" && !Array.isArray(grant)) {
    const { until, ...rest } = grant as Record<string, unknown>;
    if (!Object.keys(rest).length && isTime(until)) return { grant: { until } };
  }
  return undefined;
}

/** A card change's body, `{ frozen }`, read strictly. */
export function vaultCardChangeOf(value: unknown): { frozen: boolean } | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const keys = Object.keys(value);
  const { frozen } = value as { frozen?: unknown };
  return keys.length === 1 && typeof frozen === "boolean" ? { frozen } : undefined;
}

/** An approval's body, `{ for }`, read strictly. */
export function vaultApprovalOf(value: unknown): VaultAskApproval | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const keys = Object.keys(value);
  const chosen = (value as { for?: unknown }).for;
  return keys.length === 1 && (chosen === "once" || chosen === "until") ? { for: chosen } : undefined;
}

/** `PUT /agent/vault/index`'s body: what the person's desktop holds, without a secret. */
export interface VaultIndex {
  /** The desktop's name, such as "Maya’s MacBook Pro". */
  device: string;
  /** The sources linked on it: the Nexus vault, 1Password. Empty after Delete all. */
  sources: VaultManager[];
  /** Each login: name, site, masked account, source, grant and last use. Never a secret. */
  logins: (VaultLogin & { source: VaultManagerKind })[];
  /** The desktop's own activity lines, newest first. */
  activity: VaultActivityEntry[];
}

const only = (value: object, allowed: readonly string[]) => Object.keys(value).every((key) => allowed.includes(key));

/**
 * `PUT /agent/vault/index`'s body, read strictly: undefined when it is not one, when it has a field it does not know,
 * or when it holds a secret's shape anywhere. A login's ids are unique.
 */
export function vaultIndexOf(body: unknown): VaultIndex | undefined {
  if (!body || typeof body !== "object" || Array.isArray(body) || holdsSecret(body)) return undefined;
  if (!only(body, ["device", "sources", "logins", "activity"])) return undefined;
  const fields = body as Record<string, unknown>;
  if (!text(fields.device, 120)) return undefined;
  const sources = listOf(fields.sources, (each) => (each && typeof each === "object" && only(each, ["kind", "vault", "total"]) ? (managerOf(each) ?? undefined) : undefined), 4);
  const logins = listOf(
    fields.logins,
    (each) => {
      if (!each || typeof each !== "object" || !only(each, ["id", "name", "site", "account", "source", "grant", "lastUsed", "flag"])) return undefined;
      const login = loginOf(each);
      return login?.source ? (login as VaultIndex["logins"][number]) : undefined;
    },
    VAULT_INDEX_MAX_LOGINS,
  );
  const activity = listOf(fields.activity, (each) => (each && typeof each === "object" && only(each, ["at", "what", "with", "approved", "result", "outcome"]) ? entryOf(each) : undefined), VAULT_INDEX_MAX_ACTIVITY);
  if (!sources || !logins || !activity) return undefined;
  if (new Set(logins.map((login) => login.id)).size !== logins.length) return undefined;
  return { device: fields.device, sources, logins, activity };
}

/** `POST /agent/vault/asks`'s body: one login, one task, and the date an "Until" choice offers. */
export interface VaultAskRequest {
  login: string;
  task: string;
  until?: string;
}

/** An ask's body, read strictly. */
export function vaultAskRequestOf(value: unknown): VaultAskRequest | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value) || holdsSecret(value)) return undefined;
  if (!only(value, ["login", "task", "until"])) return undefined;
  const { login, task, until } = value as Record<string, unknown>;
  if (!isId(login) || !text(task, 400) || (until !== undefined && !isTime(until))) return undefined;
  return { login, task, ...(until !== undefined ? { until: until as string } : {}) };
}

// The words each row shows, as A8 draws them. Times read in `timeZone` (the person's), English only for now.

const parts = (iso: string, timeZone: string) => {
  const at = new Date(iso);
  const get = (options: Intl.DateTimeFormatOptions) => new Intl.DateTimeFormat("en-GB", { timeZone, ...options }).format(at);
  return { weekday: get({ weekday: "short" }), day: get({ day: "numeric" }), month: get({ month: "short" }), clock: get({ hour: "2-digit", minute: "2-digit", hourCycle: "h23" }), date: get({ year: "numeric", month: "2-digit", day: "2-digit" }) };
};

/** "Sun 8 Nov". */
export function vaultDay(iso: string, timeZone = "UTC"): string {
  const { weekday, day, month } = parts(iso, timeZone);
  return `${weekday} ${day} ${month}`;
}

/** A grant as the table says it: "Each use · Face ID", "Until Sun 8 Nov", or "Off". */
export function vaultGrantLabel(grant: VaultGrant | null, timeZone = "UTC"): string {
  if (!grant) return "Off";
  return grant.kind === "each_use" ? "Each use · Face ID" : `Until ${vaultDay(grant.until, timeZone)}`;
}

const DAY_MS = 86_400_000;

/** "Today 07:41", "Tue" within the week, "12 Oct" before it, "never". */
export function vaultWhen(iso: string | null, now: number, timeZone = "UTC"): string {
  if (!iso) return "never";
  const at = parts(iso, timeZone);
  const today = parts(new Date(now).toISOString(), timeZone);
  if (at.date === today.date) return `Today ${at.clock}`;
  const age = now - Date.parse(iso);
  return age >= 0 && age < 6 * DAY_MS ? at.weekday : `${at.day} ${at.month}`;
}

/** The activity log's When: "07:41" today, "Tue 19:02" within the week, "12 Oct 08:30" before it. */
export function vaultLogWhen(iso: string, now: number, timeZone = "UTC"): string {
  const at = parts(iso, timeZone);
  const today = parts(new Date(now).toISOString(), timeZone);
  if (at.date === today.date) return at.clock;
  const age = now - Date.parse(iso);
  return age >= 0 && age < 6 * DAY_MS ? `${at.weekday} ${at.clock}` : `${at.day} ${at.month} ${at.clock}`;
}

/** "$142", "$38.60". */
export function vaultMoney(amount: string): string {
  const [whole = "0", cents] = amount.split(".");
  const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return cents && Number(cents) !== 0 ? `$${grouped}.${cents.padEnd(2, "0")}` : `$${grouped}`;
}

/** "$142 of $500 this month". */
export function vaultSpent(card: VaultCard): string {
  return `${vaultMoney(card.spent)} of ${vaultMoney(card.limit)} this month`;
}

/** How far the month's spend is, 0 to 1. */
export function vaultSpentShare(card: VaultCard): number {
  const limit = Number(card.limit);
  return limit > 0 ? Math.min(1, Math.max(0, Number(card.spent) / limit)) : 0;
}

const PER_WORDS: Record<VaultPer, string> = { order: "an order", ride: "a ride", booking: "a booking" };

/** A merchant's Limit: "$60 an order", "Each booking", or "Off". */
export function vaultLimit(merchant: VaultMerchant): string {
  if (merchant.approval === "off") return "Off";
  return merchant.amount ? `${vaultMoney(merchant.amount)} ${PER_WORDS[merchant.per]}` : `Each ${merchant.per}`;
}

/** A merchant's Approval: "Asks first", "Face ID", or "Can’t pay". */
export function vaultApprovalLabel(approval: VaultApproval): string {
  return approval === "asks_first" ? "Asks first" : approval === "face_id" ? "Face ID" : "Can’t pay";
}
