import type { CoreClient } from "@nexus/kit";

/** Holds a system link just until the connection screen reads it. */
let pending: string | undefined;
const listeners = new Set<() => void>();
export function keepConnectionIntent(value: string): void {
  pending = value;
  for (const listener of listeners) listener();
}
export function connectionIntent(): string | undefined { return pending; }
export function forgetConnectionIntent(): void {
  if (!pending) return;
  pending = undefined;
  for (const listener of listeners) listener();
}
export function watchConnectionIntent(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/** Unsent text crosses transport changes in memory, never through URL parameters. */
const drafts = new WeakMap<CoreClient, string>();
export function transferDraft(core: CoreClient, value: string): void { drafts.set(core, value); }
export function takeTransferredDraft(core: CoreClient): string | undefined {
  const value = drafts.get(core);
  drafts.delete(core);
  return value;
}

/** Leave the current composer only when the person opens the newly connected computer. */
const runOnContinuations = new WeakMap<CoreClient, () => void>();
export function keepRunOnContinuation(core: CoreClient, run: () => void): void {
  runOnContinuations.set(core, run);
}
export function consumeRunOnContinuation(core: CoreClient): (() => void) | undefined {
  const run = runOnContinuations.get(core);
  runOnContinuations.delete(core);
  return run;
}

const identities = new WeakMap<CoreClient, number>();
let nextIdentity = 0;
export function clientIdentity(core: CoreClient): number {
  const held = identities.get(core);
  if (held !== undefined) return held;
  const identity = ++nextIdentity;
  identities.set(core, identity);
  return identity;
}

/** Project names come from encrypted session history and stay in this process. */
const projectStarts = new WeakMap<CoreClient, { route: string; computerId: string; name: string }>();
let nextProjectStart = 0;
export function keepProjectStart(core: CoreClient, computerId: string, name: string): string {
  const route = String(++nextProjectStart);
  projectStarts.set(core, { route, computerId, name });
  return route;
}
export function projectStart(core: CoreClient, computerId: string, route: string | undefined): string | undefined {
  const start = projectStarts.get(core);
  return start && start.route === route && start.computerId === computerId ? start.name : undefined;
}

/** A Cloud launch can prefill text held in another composer without putting it in the route. */
const cloudDrafts = new WeakMap<CoreClient, { text: string; started?: () => void }>();
export function keepCloudDraft(core: CoreClient, text: string, started?: () => void): void {
  cloudDrafts.set(core, { text, started });
}
export function cloudDraft(core: CoreClient) { return cloudDrafts.get(core); }
export function forgetCloudDraft(core: CoreClient, draft: ReturnType<typeof cloudDraft>): void {
  if (cloudDrafts.get(core) === draft) cloudDrafts.delete(core);
}
