import { connectorName, type AgentPauseView, type AgentPrivacyAccount, type AgentPrivacyCall } from "@nexus/kit";

/**
 * The Assistant's Privacy on the phone (A11), without React Native: an account's row, its lists in one line,
 * and what the page says after each change. The wire types, the strict readers and the dialogs' words are the kit's
 * (`agent-privacy.ts`); the rest are Nexus UI's words (`lib/agent-privacy.ts` there).
 */

/** An account's row head: `Gmail · ann@gmail.test`, and whose it is. */
export function accountHead(account: Pick<AgentPrivacyAccount, "connector" | "account" | "scope">): { name: string; whose: string } {
  return { name: `${connectorName(account.connector)} · ${account.account}`, whose: account.scope === "user" ? "Yours" : "Your organization's" };
}

/** At most `max` items in one line, then how many more. */
export function shortList(items: readonly string[], max = 3): string {
  if (items.length <= max) return items.join("; ");
  return `${items.slice(0, max).join("; ")}; ${items.length - max} more`;
}

/** The account's Never line: the writes the organization does not allow, else that every write asks first. */
export function neverLine(account: Pick<AgentPrivacyAccount, "writes" | "never">): string {
  if (account.never.length) return shortList(account.never);
  return account.writes.length ? "Any write without your approval" : "Any write";
}

/** What the page says once the pause changed. `assistant`: the Assistant's name. */
export const pauseText = (pause: AgentPauseView, assistant = "Nexus"): string => (pause.paused ? `${assistant} is paused. Nothing runs until you resume.` : `${assistant} runs again.`);

/** A time as the paused bar writes it: 24 hours, "14:02", on the phone's clock. */
export const clock = (at: Date): string => `${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;

/** What a failed call says, in Nexus UI's words for the Privacy view. */
export function privacyFailed(call: Exclude<AgentPrivacyCall<unknown>, { kind: "ok" }>): string {
  if (call.kind === "off") return "This agent service has no Privacy settings yet.";
  if (call.failure.status === 502) return call.failure.detail || "Nexus Gateway did not answer. Nothing changed. Try again in a moment.";
  if (call.failure.status === 503) return call.failure.detail || "Your privacy settings are not available now. Try again in a moment.";
  return call.failure.detail || "Nexus could not reach the Assistant. Try again in a moment.";
}
