import type { NotificationGroup, NotificationItem, NotificationSetting } from "@nexus/kit";

/**
 * The inbox as the phone shows it (brand v5's P35): each item's group, the line under its words, and the screen a tap
 * opens. An item holds ids only, so the screen is the source's own page, which checks its own permission.
 */

/**
 * Each group's name and what lands in it, from the sources the Gateway sends (`NotificationSource`). Your chats has
 * no source yet, so no line says what lands there.
 */
export const GROUPS: Record<NotificationGroup, { label: string; detail?: string }> = {
  my_sessions: { label: "Your chats" },
  usage_and_billing: { label: "Usage and billing", detail: "Refused calls, credits, the free start, payments, and members who ask about billing" },
  requests_and_approvals: { label: "Requests and approvals", detail: "Access to what you shared, its reports, and skill reviews" },
  automations_and_integrations: { label: "Automations and integrations", detail: "Rejected provider keys and the GitHub App" },
  account_and_security: { label: "Account and security", detail: "Who joined, and security changes" },
};

/** A symbol the inbox draws, by its name in `icon.tsx`. */
export type KindIcon = "chats" | "wallet" | "person" | "lock" | "flag" | "billing" | "members" | "skill" | "shield" | "keys" | "repositories" | "connector" | "mail";

/**
 * How a kind looks in the inbox (brand v5's P35·D): its symbol, where it happened (the row's second line), and whether
 * it asks for attention, which tints its symbol in the mark's amber: a limit reached, a payment or a key refused, a
 * connection lost. Kinds the phone does not know take their group's look.
 */
export type KindLook = { icon: KindIcon; where: string; attention?: boolean };
const KINDS: Record<string, KindLook> = {
  "usage.hard_stop_reached": { icon: "wallet", where: "Usage", attention: true },
  "usage.budget_near": { icon: "wallet", where: "Usage", attention: true },
  "requests.artifact_access_requested": { icon: "person", where: "Artifact" },
  "requests.artifact_access_answered": { icon: "lock", where: "Artifact" },
  "requests.artifact_reported": { icon: "flag", where: "Artifact" },
  "billing.payment_method_changed": { icon: "billing", where: "Billing" },
  "billing.plan_changed": { icon: "billing", where: "Billing" },
  "billing.payment_failed": { icon: "billing", where: "Billing", attention: true },
  "requests.member_joined": { icon: "members", where: "Members" },
  "requests.skill_promotion_requested": { icon: "skill", where: "Skills" },
  "requests.skill_promotion_decided": { icon: "skill", where: "Skills" },
  // Nexus waits for you to confirm or deny a write: Nexus UI's approval page decides it.
  "requests.approval_waiting": { icon: "shield", where: "Approvals", attention: true },
  "security.changed": { icon: "shield", where: "Security" },
  "integrations.provider_key_rejected": { icon: "keys", where: "Provider keys", attention: true },
  "integrations.connector_disconnected": { icon: "repositories", where: "Repositories", attention: true },
  "usage.referral_joined": { icon: "person", where: "Referrals" },
  "usage.referral_rewarded": { icon: "wallet", where: "Referrals" },
  // Brand v5's amber mark means someone must act (P73·H): a question, low credits, 80% used, ends soon. A limit
  // reached as set (used up, out, reached, ended) has no mark.
  "billing.owner_asked": { icon: "person", where: "Billing", attention: true },
  "billing.usage_limit_reached": { icon: "billing", where: "Billing" },
  "billing.credits_low": { icon: "wallet", where: "Billing", attention: true },
  "billing.credits_out": { icon: "wallet", where: "Billing" },
  "usage.free_start_near": { icon: "wallet", where: "Billing", attention: true },
  "usage.free_start_used": { icon: "wallet", where: "Billing" },
  "usage.free_start_expiring": { icon: "wallet", where: "Billing", attention: true },
  "usage.free_start_expired": { icon: "wallet", where: "Billing" },
  // Gateway turned auto top-up off: an owner fixes the payment and turns it on again.
  "billing.auto_topup_off": { icon: "wallet", where: "Billing", attention: true },
  // 80% of the month's usage limit: a warning before calls stop, so it carries the mark.
  "billing.usage_limit_near": { icon: "billing", where: "Billing", attention: true },
  // Pricing and the Customer Terms change on a date: an owner reads what changes before then.
  "billing.pricing_change": { icon: "billing", where: "Billing", attention: true },
  // A contract's invoice due in 7 days or less, and the card that pays expiring: an owner acts before the date.
  "billing.contract_invoice_due": { icon: "billing", where: "Billing", attention: true },
  "billing.card_expiring": { icon: "wallet", where: "Billing", attention: true },
};
const GROUP_LOOK: Record<NotificationGroup, KindLook> = {
  my_sessions: { icon: "chats", where: "Your chats" },
  usage_and_billing: { icon: "wallet", where: "Usage and billing" },
  requests_and_approvals: { icon: "mail", where: "Requests and approvals" },
  automations_and_integrations: { icon: "connector", where: "Integrations" },
  account_and_security: { icon: "shield", where: "Account and security" },
};
export const kindLook = (item: Pick<NotificationItem, "kind" | "group">): KindLook => KINDS[item.kind] ?? GROUP_LOOK[item.group];

/** The line under an item's words: where it happened, and how many events it holds when more than one. */
export const itemLine = (item: Pick<NotificationItem, "kind" | "group" | "count">) =>
  [kindLook(item).where, item.count > 1 ? `${item.count} times` : undefined].filter(Boolean).join(" · ");

/** A group's channels as its row says them (P97): "Push · Email · Inbox". The inbox always gets everything. */
export function channelsLine(settings: Pick<NotificationSetting, "group" | "channel" | "setting" | "available">[], group: NotificationGroup): string {
  const on = (channel: NotificationSetting["channel"]) => settings.some((s) => s.group === group && s.channel === channel && s.available && s.setting === "on");
  return [on("push") ? "Push" : undefined, on("email") ? "Email" : undefined, "Inbox"].filter(Boolean).join(" · ");
}

/** Where a tap on an item goes: its newest source's page, or nowhere when the phone has none for it. A `web` target is
 * a Nexus UI page the phone has no screen for, opened in the browser: an approval the person confirms there. */
export type Target =
  | { pathname: "/artifact/[id]"; params: { id: string } }
  | "/spend"
  | "/members"
  | "/billing"
  | "/audit"
  | "/keys"
  | "/repositories"
  | { web: string };

export function itemTarget(item: Pick<NotificationItem, "sources">): Target | undefined {
  const source = item.sources[0];
  switch (source?.type) {
    case "artifact_access":
    case "artifact_access_answer":
    case "artifact_report":
      return { pathname: "/artifact/[id]", params: { id: source.artifactId } };
    case "spend":
    case "limit_change":
      return "/spend";
    case "member":
      return "/members";
    case "billing_change":
    case "billing_period":
    case "billing_state":
    case "owner_ask":
      return "/billing";
    case "security_change":
      return "/audit";
    case "provider_key":
      return "/keys";
    case "github_installation":
      return "/repositories";
    case "approval":
      return { web: `/app/approvals/${encodeURIComponent(source.approvalId)}` };
    default:
      return undefined;
  }
}

/** The drawer's count: what is unread, "20+" past a page. */
export const unreadBadge = (unread: number, more: boolean) => (more ? `${unread}+` : unread ? String(unread) : undefined);
