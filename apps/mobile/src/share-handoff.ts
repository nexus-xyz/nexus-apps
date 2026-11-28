import type { Destination } from "./share-inbox";

/** A photo of an import, copied into the app's caches for the composer to prepare. */
export type HandedImage = { uri: string; name: string; mimeType?: string };

/** An import on its way to a composer: its words, its photos, and what the extension left out. */
export type Handoff = { to: Destination; org: string; text: string; images: HandedImage[]; skipped?: string };

/**
 * The review hands an import to a composer in memory, never through URL parameters (as `connection-intent.ts` does
 * with drafts). The composer of `to` takes it when it is focused, in the organization it was meant for.
 */
let pending: Handoff | undefined;
const listeners = new Set<() => void>();

export function handOff(handoff: Handoff): void {
  pending = handoff;
  for (const listener of listeners) listener();
}

export function takeHandoff(to: Destination, org: string | undefined): Handoff | undefined {
  if (!pending || pending.to !== to || pending.org !== org) return undefined;
  const taken = pending;
  pending = undefined;
  return taken;
}

export function watchHandoff(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
