/**
 * What the Assistant screen tells the drawer, per account, without React Native: whether an answer streams now, and a
 * count that moves each time the screen shows new replies, so the drawer reads its row again. In memory only: what was
 * seen is kept on the device by the screen (`assistant-row.ts`'s `Seen`).
 */

export interface Activity {
  streaming: boolean;
  /** Moves each time the screen marks replies seen. */
  seen: number;
}

const IDLE: Activity = { streaming: false, seen: 0 };
const byAccount = new Map<string, Activity>();
const listeners = new Set<() => void>();

const put = (account: string, next: Activity) => {
  byAccount.set(account, next);
  for (const listener of listeners) listener();
};

/** The account's activity. The same object until it changes, as `useSyncExternalStore` needs. */
export const activityOf = (account: string | undefined): Activity => (account ? byAccount.get(account) ?? IDLE : IDLE);

/** An answer starts or stops streaming in the account's Assistant. */
export function setStreaming(account: string | undefined, streaming: boolean) {
  if (!account) return;
  const now = activityOf(account);
  if (now.streaming !== streaming) put(account, { ...now, streaming });
}

/** The account's Assistant showed its replies. */
export function markSeen(account: string | undefined) {
  if (!account) return;
  const now = activityOf(account);
  put(account, { ...now, seen: now.seen + 1 });
}

export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
