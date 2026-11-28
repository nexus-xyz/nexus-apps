import { router, useFocusEffect, usePathname } from "expo-router";
import { useCallback, useEffect, useRef } from "react";
import { AppState } from "react-native";
import { useAuth } from "./auth";
import { takeHandoff, watchHandoff, type Handoff } from "./share-handoff";
import { sortOut, type Destination, type SharedImport } from "./share-inbox";
import { shareInbox, shareInboxAvailable } from "./share-inbox-native";

/** An import waiting in the inbox, with its folder (for its photos' previews). */
export type Waiting = SharedImport & { folder: string };

/** Imports removed because another account was signed in when they were shared, since the review last said so. */
let removedForOthers = 0;

/** How many imports were removed for another account since the review last said so. */
export const removedForOthersCount = () => removedForOthers;

/** The review said so. */
export function clearRemovedForOthers() {
  removedForOthers = 0;
}

/**
 * The imports waiting for `account`, oldest first. Unreadable and expired ones, and those of another account, are
 * removed on the way: they never show.
 */
export async function readInbox(account: string): Promise<Waiting[]> {
  const entries = await shareInbox.list();
  const { pending, drop } = sortOut(entries, account, Date.now());
  for (const { id, why } of drop) {
    if (why === "other-account") removedForOthers += 1;
    await shareInbox.remove(id);
  }
  const folders = new Map(entries.map((entry) => [entry.id, entry.folder]));
  return pending.map((shared) => ({ ...shared, folder: folders.get(shared.id) ?? "" }));
}

/**
 * Opens the review (`/shared`) when the app comes to the front with something in the inbox: an import, or one removed
 * because it was another account's. Each import opens it once each time the app comes to the front, until the person
 * continues with it or discards it, or it expires.
 */
export function useShareInboxPrompt() {
  const { shareAccount, org } = useAuth();
  const pathname = usePathname();
  const here = useRef(pathname);
  here.current = pathname;
  const opened = useRef(new Set<string>());
  const checking = useRef(false);

  const check = useCallback(async () => {
    if (!shareInboxAvailable || !shareAccount || !org || checking.current) return;
    checking.current = true;
    try {
      const waiting = await readInbox(shareAccount);
      const fresh = waiting.filter((shared) => !opened.current.has(shared.id));
      for (const shared of fresh) opened.current.add(shared.id);
      if ((fresh.length || removedForOthers) && here.current !== "/shared") router.push("/shared");
    } finally {
      checking.current = false;
    }
  }, [shareAccount, org]);

  useEffect(() => {
    void check();
    const subscription = AppState.addEventListener("change", (state) => {
      if (state === "active") void check();
      else if (state === "background") opened.current.clear();
    });
    return () => subscription.remove();
  }, [check]);
}

/**
 * Hands an import to this composer (`to`) while it is on screen: the review sends it, and the composer adds it to its
 * draft. A composer behind another screen takes it when it comes back to the front.
 */
export function useSharedHandoff(to: Destination, enabled: boolean, receive: (handoff: Handoff) => void) {
  const { org } = useAuth();
  const latest = useRef(receive);
  latest.current = receive;
  useFocusEffect(
    useCallback(() => {
      if (!enabled) return;
      const take = () => {
        const handoff = takeHandoff(to, org);
        if (handoff) latest.current(handoff);
      };
      take();
      return watchHandoff(take);
    }, [to, org, enabled]),
  );
}
