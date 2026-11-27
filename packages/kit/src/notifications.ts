/**
 * The notification inbox (Rust gateway only, `notifications.use`): openapi.json's `NotificationItem`,
 * `NotificationPage`, `NotificationKind`, `NotificationKinds`, `NotificationSettings` and `SetNotificationSettings`. An item holds ids only: show its kind's fixed
 * `subjectText` and link its sources to their own pages, which check their own permission. The operations are on
 * `createArtifactsClient`; the parsers here check each answer at that boundary.
 */

export const NOTIFICATION_GROUPS = ["my_sessions", "usage_and_billing", "requests_and_approvals", "automations_and_integrations", "account_and_security"] as const;
export type NotificationGroup = (typeof NOTIFICATION_GROUPS)[number];

export interface NotificationSubject {
  type: "organization" | "team" | "user" | "artifact";
  id: string;
}

/** One event an item reports, as ids: a refused call's spend record, an access request or report on an artifact, a
 * billing change's audit row, a billing month whose payment failed, a person who joined, a skill promotion, a
 * security change's audit action, a rejected key's provider, the answer to the person's own access request, a GitHub
 * App installation that lost repositories, a closed month's usage report (`YYYY-MM`), a spend limit change's audit
 * action, a referral by a hash of the referred person's id, a state of the organization's money, or a member's question
 * to the owners about a billing block, or a Policy approval record that waits on the person,
 * or a cloud session whose state moved at its event `seq`. */
export type NotificationSource =
  | { type: "app_hosting"; appId: string; notice: "half" | "near" | "reached" | "payment_due" }
  | { type: "spend"; spendId: string }
  | { type: "artifact_access"; artifactId: string; requestId: string }
  | { type: "artifact_access_answer"; artifactId: string; requestId: string; outcome: "granted" | "denied" }
  | { type: "artifact_report"; artifactId: string; reportId: string }
  | { type: "member"; userId: string }
  | { type: "skill_promotion"; promotionId: string }
  | { type: "billing_change"; auditId: string }
  | { type: "billing_period"; periodId: string }
  | { type: "security_change"; action: string }
  | { type: "provider_key"; provider: string }
  | { type: "github_installation"; installationId: string }
  | { type: "usage_month"; month: string }
  | { type: "limit_change"; action: string }
  | { type: "referral"; referral: string }
  | { type: "billing_state"; state: BillingState }
  | { type: "owner_ask"; userId: string; block: BillingBlock }
  | { type: "approval"; approvalId: string }
  | { type: "session"; machineId: string; threadId: string; seq: number };

/** A state of the organization's money that its owners hear of (`billing_state` sources). */
export const BILLING_STATES = ["usage_limit_reached", "credits_low", "credits_out", "free_start_near", "free_start_used", "free_start_expiring", "free_start_expired", "auto_topup_off", "usage_limit_near", "pricing_change", "contract_invoice_due", "card_expiring", "plan_ai_near", "plan_ai_used", "plan_cloud_near", "plan_cloud_used"] as const;
export type BillingState = (typeof BILLING_STATES)[number];

/**
 * What stops a member, which only an owner fixes: the block they ask the owners about (`POST /v1/notifications/ask-owner`).
 * `billing` is a question about the plan and its usage, from the Billing page.
 */
export const BILLING_BLOCKS = ["usage_limit", "credits_out", "free_start_used", "payment_failed", "past_due", "billing"] as const;
export type BillingBlock = (typeof BILLING_BLOCKS)[number];

/** `sent`: each owner got an inbox item now. `already_asked`: you asked about this block today (UTC); nobody is told again. */
export interface AskOwnersResponse {
  status: "sent" | "already_asked";
}

/** The answer to a question to the owners; throws `MalformedNotification` on any other shape. */
export function parseAskOwners(value: unknown): AskOwnersResponse {
  if (isObject(value) && (value.status === "sent" || value.status === "already_asked")) return { status: value.status };
  throw new MalformedNotification("a question to the owners answered no status");
}

export interface NotificationItem {
  /** `ntf_…` */
  id: string;
  /** The kind's stable name, as `GET /v1/notifications/kinds` lists it. */
  kind: string;
  group: NotificationGroup;
  /** The kind's fixed text. No event text reaches it. */
  subjectText: string;
  /** A month report's summary, from its figures only: `October 2026 usage: $60.00`. */
  title?: string;
  /** The summary's second line: `6 model calls. Open Usage for teams and models.` */
  detail?: string;
  subject: NotificationSubject;
  /** At most 20; `count` can be higher. Sources of a type this client does not know are left out. */
  sources: NotificationSource[];
  count: number;
  readAt?: string;
  doneAt?: string;
  createdAt: string;
  updatedAt: string;
}

/** The caller's items that are not done, newest first; `nextCursor` while older ones match. */
export interface NotificationPage {
  items: NotificationItem[];
  nextCursor?: string;
}

export interface NotificationKind {
  name: string;
  group: NotificationGroup;
  subjectText: string;
  /** A required kind can move between channels but never be muted. */
  required: boolean;
  defaults: NotificationChannel[];
}

export interface NotificationsQuery {
  cursor?: string;
  /** Only items not yet read. */
  unread?: boolean;
  group?: NotificationGroup;
}

/** The answer did not have the shape openapi.json promises. */
export class MalformedNotification extends Error {
  constructor(what: string) {
    super(`malformed notification answer: ${what}`);
    this.name = "MalformedNotification";
  }
}

type Json = Record<string, unknown>;
const isObject = (value: unknown): value is Json => typeof value === "object" && value !== null && !Array.isArray(value);
const isString = (value: unknown): value is string => typeof value === "string";
const isGroup = (value: unknown): value is NotificationGroup => NOTIFICATION_GROUPS.includes(value as NotificationGroup);
const SUBJECTS: readonly string[] = ["organization", "team", "user", "artifact"];

function source(value: unknown): NotificationSource | undefined {
  if (!isObject(value)) return undefined;
  if (value.type === "spend" && isString(value.spendId)) return { type: "spend", spendId: value.spendId };
  if (value.type === "artifact_access" && isString(value.artifactId) && isString(value.requestId)) {
    return { type: "artifact_access", artifactId: value.artifactId, requestId: value.requestId };
  }
  if (value.type === "artifact_access_answer" && isString(value.artifactId) && isString(value.requestId) && (value.outcome === "granted" || value.outcome === "denied")) {
    return { type: "artifact_access_answer", artifactId: value.artifactId, requestId: value.requestId, outcome: value.outcome };
  }
  if (value.type === "artifact_report" && isString(value.artifactId) && isString(value.reportId)) {
    return { type: "artifact_report", artifactId: value.artifactId, reportId: value.reportId };
  }
  if (value.type === "billing_change" && isString(value.auditId)) return { type: "billing_change", auditId: value.auditId };
  if (value.type === "billing_period" && isString(value.periodId)) return { type: "billing_period", periodId: value.periodId };
  if (value.type === "member" && isString(value.userId)) return { type: "member", userId: value.userId };
  if (value.type === "skill_promotion" && isString(value.promotionId)) return { type: "skill_promotion", promotionId: value.promotionId };
  if (value.type === "security_change" && isString(value.action)) return { type: "security_change", action: value.action };
  if (value.type === "provider_key" && isString(value.provider)) return { type: "provider_key", provider: value.provider };
  if (value.type === "github_installation" && isString(value.installationId)) return { type: "github_installation", installationId: value.installationId };
  if (value.type === "usage_month" && isString(value.month) && /^\d{4}-\d{2}$/.test(value.month)) return { type: "usage_month", month: value.month };
  if (value.type === "limit_change" && isString(value.action)) return { type: "limit_change", action: value.action };
  if (value.type === "referral" && isString(value.referral)) return { type: "referral", referral: value.referral };
  if (value.type === "app_hosting" && typeof value.appId === "string" && ["half", "near", "reached", "payment_due"].includes(value.notice as string)) return { type: "app_hosting", appId: value.appId, notice: value.notice as "half" | "near" | "reached" | "payment_due" };
  if (value.type === "billing_state" && BILLING_STATES.includes(value.state as BillingState)) return { type: "billing_state", state: value.state as BillingState };
  if (value.type === "owner_ask" && isString(value.userId) && BILLING_BLOCKS.includes(value.block as BillingBlock)) {
    return { type: "owner_ask", userId: value.userId, block: value.block as BillingBlock };
  }
  if (value.type === "approval" && isString(value.approvalId)) return { type: "approval", approvalId: value.approvalId };
  if (value.type === "session" && isString(value.machineId) && isString(value.threadId) && Number.isSafeInteger(value.seq)) {
    return { type: "session", machineId: value.machineId, threadId: value.threadId, seq: value.seq as number };
  }
  return undefined;
}

/** One item; throws `MalformedNotification` when a required field is missing or of the wrong type. */
export function parseNotificationItem(value: unknown): NotificationItem {
  if (!isObject(value)) throw new MalformedNotification("an item is not an object");
  const { id, kind, group, subjectText, title, detail, subject, sources, count, readAt, doneAt, createdAt, updatedAt } = value;
  if (!isString(id) || !isString(kind) || !isString(subjectText) || !isString(createdAt) || !isString(updatedAt)) {
    throw new MalformedNotification(`item ${isString(id) ? id : "?"} lacks a required text field`);
  }
  if (!isGroup(group)) throw new MalformedNotification(`item ${id} has an unknown group ${JSON.stringify(group)}`);
  if (!isObject(subject) || !SUBJECTS.includes(subject.type as string) || !isString(subject.id)) throw new MalformedNotification(`item ${id} has no subject`);
  if (!Array.isArray(sources)) throw new MalformedNotification(`item ${id} has no sources`);
  if (typeof count !== "number" || !Number.isInteger(count) || count < 1) throw new MalformedNotification(`item ${id} has a bad count`);
  return {
    id,
    kind,
    group,
    subjectText,
    ...(isString(title) && title !== "" ? { title } : {}),
    ...(isString(detail) && detail !== "" ? { detail } : {}),
    subject: { type: subject.type as NotificationSubject["type"], id: subject.id },
    sources: sources.map(source).filter((entry): entry is NotificationSource => entry !== undefined),
    count,
    ...(isString(readAt) ? { readAt } : {}),
    ...(isString(doneAt) ? { doneAt } : {}),
    createdAt,
    updatedAt,
  };
}

export function parseNotificationPage(value: unknown): NotificationPage {
  if (!isObject(value) || !Array.isArray(value.items)) throw new MalformedNotification("the page has no items");
  return { items: value.items.map(parseNotificationItem), ...(isString(value.nextCursor) ? { nextCursor: value.nextCursor } : {}) };
}

export function parseNotificationKinds(value: unknown): NotificationKind[] {
  if (!isObject(value) || !Array.isArray(value.kinds)) throw new MalformedNotification("the catalogue has no kinds");
  return value.kinds.map((entry) => {
    if (!isObject(entry) || !isString(entry.name) || !isGroup(entry.group) || !isString(entry.subjectText) || typeof entry.required !== "boolean") {
      throw new MalformedNotification("a kind lacks its name, group, text or required flag");
    }
    const defaults = Array.isArray(entry.defaults) ? entry.defaults.filter(isChannel) : [];
    return { name: entry.name, group: entry.group, subjectText: entry.subjectText, required: entry.required, defaults };
  });
}

export type NotificationChannel = "inbox" | "email" | "push";
export type NotificationOnOff = "on" | "off";

/** The caller's effective setting for one group and channel. */
export interface NotificationSetting {
  group: NotificationGroup;
  channel: NotificationChannel;
  /** The person's choice, else the default. */
  setting: NotificationOnOff;
  default: NotificationOnOff;
  /** The inbox: always on, no setting changes it. */
  locked: boolean;
  /** The group holds a required kind, which `off` does not mute. */
  required: boolean;
  /** The channel can reach the person. Gateway sends `true`: push reaches a desktop app even without Web Push keys. */
  available: boolean;
}

/** One change for `PUT /v1/notifications/settings`. The inbox has no setting, so it is not a channel here. */
export interface NotificationSettingChange {
  group: NotificationGroup;
  channel: "email" | "push";
  setting: NotificationOnOff;
}

const isOnOff = (value: unknown): value is NotificationOnOff => value === "on" || value === "off";
const isChannel = (value: unknown): value is NotificationChannel => value === "inbox" || value === "email" || value === "push";

/** `GET` and `PUT /v1/notifications/settings`; throws `MalformedNotification` on a row it cannot read. */
export function parseNotificationSettings(value: unknown): NotificationSetting[] {
  if (!isObject(value) || !Array.isArray(value.settings)) throw new MalformedNotification("the settings have no rows");
  return value.settings.map((row) => {
    if (!isObject(row) || !isGroup(row.group) || !isChannel(row.channel) || !isOnOff(row.setting) || !isOnOff(row.default) || typeof row.locked !== "boolean" || typeof row.required !== "boolean") {
      throw new MalformedNotification("a setting lacks its group, channel, setting, default or flags");
    }
    // A Gateway from before push sends no `available`: every channel it lists can send.
    const available = row.available !== false;
    return { group: row.group, channel: row.channel, setting: row.setting, default: row.default, locked: row.locked, required: row.required, available };
  });
}

/** `GET /v1/notifications/push-key`: the VAPID public key a browser passes to `pushManager.subscribe`. */
export function parseNotificationPushKey(value: unknown): string {
  if (!isObject(value) || !isString(value.publicKey) || value.publicKey.length === 0) throw new MalformedNotification("the answer has no public key");
  return value.publicKey;
}

export type NotificationDevicePlatform = "web" | "desktop" | "ios" | "android";
export type NotificationDeviceRevokeReason = "signed_out" | "lost" | "left" | "rejected" | "replaced";

/** One of the caller's devices that take push. It never holds the token. */
export interface NotificationDevice {
  id: string;
  platform: NotificationDevicePlatform;
  createdAt: string;
  lastSeenAt: string;
  revokedAt?: string;
  revokedReason?: NotificationDeviceRevokeReason;
}

const PLATFORMS: readonly string[] = ["web", "desktop", "ios", "android"];
const REVOKE_REASONS: readonly string[] = ["signed_out", "lost", "left", "rejected", "replaced"];
const isPlatform = (value: unknown): value is NotificationDevicePlatform => isString(value) && PLATFORMS.includes(value);
const isRevokeReason = (value: unknown): value is NotificationDeviceRevokeReason => isString(value) && REVOKE_REASONS.includes(value);

/** `POST` and `DELETE /v1/notifications/devices…`; throws `MalformedNotification` on an answer it cannot read. */
export function parseNotificationDevice(value: unknown): NotificationDevice {
  if (!isObject(value) || !isString(value.id) || !isPlatform(value.platform) || !isString(value.createdAt) || !isString(value.lastSeenAt)) {
    throw new MalformedNotification("a device lacks its id, platform or times");
  }
  return {
    id: value.id,
    platform: value.platform,
    createdAt: value.createdAt,
    lastSeenAt: value.lastSeenAt,
    ...(isString(value.revokedAt) ? { revokedAt: value.revokedAt } : {}),
    ...(isRevokeReason(value.revokedReason) ? { revokedReason: value.revokedReason } : {}),
  };
}

/** `GET /v1/notifications/devices`: the caller's live devices. */
export function parseNotificationDevices(value: unknown): NotificationDevice[] {
  if (!isObject(value) || !Array.isArray(value.devices)) throw new MalformedNotification("the answer has no devices");
  return value.devices.map(parseNotificationDevice);
}

/** What a lock screen may show: ids and fixed text only, the same fields Web Push sends. */
export interface CollectedNotification {
  deliveryId: string;
  kind: string;
  eventId: string;
  title: string;
  /** A month report's second line, from its figures only; absent for every other kind. */
  body?: string;
  path: string;
}

/** `POST /v1/notifications/devices/{id}/collect`: a desktop device's deliveries, each handed over once. */
export function parseCollectedNotifications(value: unknown): CollectedNotification[] {
  if (!isObject(value) || !Array.isArray(value.items)) throw new MalformedNotification("the answer has no items");
  return value.items.map((item: unknown) => {
    if (!isObject(item) || !isString(item.deliveryId) || !isString(item.kind) || !isString(item.eventId) || !isString(item.title) || !isString(item.path)) {
      throw new MalformedNotification("a collected notification lacks its delivery id, kind, event id, title or path");
    }
    if (item.body !== undefined && item.body !== null && !isString(item.body)) throw new MalformedNotification("a collected notification's body is not text");
    const collected: CollectedNotification = { deliveryId: item.deliveryId, kind: item.kind, eventId: item.eventId, title: item.title, path: item.path };
    if (isString(item.body)) collected.body = item.body;
    return collected;
  });
}
