import type { AgentProfile } from "@nexus/kit";

/**
 * The Assistant's name on the phone, without React Native: the name the person chose for their own agent
 * profile on Gateway (`GET /v1/agents/profiles`, the profile whose owner is the person), Nexus by default. Nexus UI
 * reads it the same way (`lib/assistant-name.ts` there). "Nexus" stays where it names the product, not the agent:
 * "Sent using Nexus", "Review in Nexus", "Nexus UI".
 *
 * The name is kept per account in memory, so every part of the Assistant reads the same one, and the device keeps the
 * last one read so the screen does not open as Nexus first (`use-assistant-name.ts`).
 */

/** The name when the person chose none, or the Gateway keeps no profiles. */
export const DEFAULT_ASSISTANT_NAME = "Nexus";

/** The person's own Assistant's name: their personal profile's, trimmed; Nexus without one. */
export function assistantNameOf(profiles: readonly AgentProfile[] | null | undefined): string {
  const name = profiles?.find((profile) => profile.owner === "person")?.name.trim();
  return name || DEFAULT_ASSISTANT_NAME;
}

const byAccount = new Map<string, string>();
const listeners = new Set<() => void>();

/** The account's Assistant's name, or Nexus until one is read. */
export const nameOf = (account: string | undefined): string => (account ? byAccount.get(account) ?? DEFAULT_ASSISTANT_NAME : DEFAULT_ASSISTANT_NAME);

/** Whether the account's name was read or kept in this run. */
export const known = (account: string | undefined): boolean => !!account && byAccount.has(account);

/** Keeps the account's name and tells every reader. */
export function setName(account: string | undefined, name: string) {
  if (!account || byAccount.get(account) === name) return;
  byAccount.set(account, name);
  for (const listener of listeners) listener();
}

export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
