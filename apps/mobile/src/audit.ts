import type { AuditEvent, AuditScope } from "@nexus/kit";

// Copies of Nexus UI's Audit copy and helpers (`audit/page.tsx`); move them into @nexus/kit if a third
// client needs them.

/** Time ranges Gateway understands (`since=7d`); empty means every event. */
export const RANGES = [
  { id: "24h", label: "24 hours" },
  { id: "7d", label: "7 days" },
  { id: "30d", label: "30 days" },
  { id: "90d", label: "90 days" },
  { id: "", label: "Everything" },
] as const;

/** Areas of the audit log, as action prefixes (Gateway's `AUDIT_ACTIONS`). */
export const AREAS = [
  { id: "", label: "Every change" },
  { id: "team.*", label: "Teams" },
  { id: "member.*,invitation.*", label: "Members" },
  { id: "key.*,auth.*,provider_key.*,agent.*", label: "Keys and sign-ins" },
  { id: "org.*", label: "Organization" },
  { id: "policy.*", label: "Model policy" },
  { id: "github.*", label: "Repositories" },
] as const;

export const SCOPE: Record<AuditScope, string> = {
  all: "Every change in your organization.",
  team: "Your changes and those to the teams you lead.",
  own: "The changes you made.",
};
export const SURFACE: Record<AuditEvent["surface"], string> = { api: "API", cli: "CLI", mcp: "MCP", ui: "Nexus UI", chat: "Chat", webhook: "Webhook", system: "Nexus Gateway" };
export const KIND: Record<AuditEvent["actor"]["kind"], string> = {
  person: "signed in",
  api_key: "API key",
  cli: "CLI sign-in",
  ui: "Nexus UI session",
  module: "module key",
  service: "service account",
  sandbox: "sandbox session",
  operator: "Nexus operator",
  agent: "agent link",
  // The agents platform's own calls, as Nexus UI names them.
  platform: "Nexus service",
  system: "system",
};

export function actorName(event: AuditEvent): string {
  if (event.actor.email) return event.actor.email;
  if (event.actor.userId) return event.actor.userId;
  if (event.actor.keyId === "github") return "GitHub";
  if (event.actor.keyId === "clerk") return "Clerk";
  return "Nexus Gateway";
}

export function targetName(event: AuditEvent): string {
  const record = { ...(event.before as Record<string, unknown> | undefined), ...(event.after as Record<string, unknown> | undefined) };
  if (event.target.type === "member") return String(record.email ?? event.metadata?.email ?? event.target.id);
  if (event.target.type === "org") return event.target.slug ?? "This organization";
  if (event.target.type === "team" && typeof record.email === "string") return `${event.target.slug ?? event.target.id} · ${record.email}`;
  return event.target.slug ?? event.target.id;
}

const asRecord = (value: unknown): Record<string, unknown> | undefined =>
  value === undefined || value === null ? undefined : typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : { value };

/** A recorded value as text: `–` for absent, `none` for null. */
export const show = (value: unknown) => (value === undefined ? "–" : value === null ? "none" : typeof value === "string" ? value : JSON.stringify(value));

/** Before and after, field by field; with both, only the fields that changed. */
export function changeRows(event: Pick<AuditEvent, "before" | "after">) {
  const before = asRecord(event.before);
  const after = asRecord(event.after);
  const fields = [...new Set([...Object.keys(before ?? {}), ...Object.keys(after ?? {})])];
  return {
    hasBefore: !!before,
    hasAfter: !!after,
    rows: fields.map((field) => ({ field, from: before?.[field], to: after?.[field] })).filter((row) => !before || !after || JSON.stringify(row.from) !== JSON.stringify(row.to)),
  };
}

/** An action as words (brand v5's P29·E): `member.role_changed` → "Member role changed". */
export function actionText(action: string): string {
  const words = action.replace(/[._]/g, " ").trim();
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : action;
}

/** A day's heading in the log, in the viewer's time zone: "Today", "Yesterday", then "Mon 2 Nov". */
export function dayTitle(iso: string, now = new Date()): string {
  const day = new Date(iso);
  const start = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const back = Math.round((start(now) - start(day)) / 86_400_000);
  if (back === 0) return "Today";
  if (back === 1) return "Yesterday";
  return day.toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short", ...(day.getFullYear() !== now.getFullYear() ? { year: "numeric" } : {}) });
}

/** The events, newest first as Gateway sends them, under their days' headings. */
export function byDay<E extends Pick<AuditEvent, "at">>(events: E[], now = new Date()): { title: string; events: E[] }[] {
  const days: { title: string; events: E[] }[] = [];
  for (const event of events) {
    const title = dayTitle(event.at, now);
    const last = days[days.length - 1];
    if (last?.title === title) last.events.push(event);
    else days.push({ title, events: [event] });
  }
  return days;
}

// The events the log has shown, so an event's own page (`audit-event/[id]`) opens with no second request: Gateway has
// no call for one event.
const shown = new Map<string, AuditEvent>();
export const keepEvents = (events: AuditEvent[]) => events.forEach((event) => shown.set(event.id, event));
export const keptEvent = (id: string) => shown.get(id);
