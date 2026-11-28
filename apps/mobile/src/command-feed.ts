/**
 * Phone control's receipts on the session's own stream. The session screen follows the thread's stream
 * (`code-reader.ts`), and Gateway sends the thread's command states on it to the thread's person. This feed passes them
 * to the screen's `CodeControlSession`, so the phone opens no second connection. Memory only; no React Native import.
 */
import type { CodeCommandChanged } from "@nexus/kit";

export type FeedChange = { kind: "command"; command: CodeCommandChanged } | { kind: "live"; live: boolean };

export class CommandFeed {
  private holders = 0;
  private readonly listeners = new Set<(change: FeedChange) => void>();

  /** A stream is open, and Gateway announces this thread's commands on it. */
  get live(): boolean {
    return this.holders > 0;
  }

  subscribe(listener: (change: FeedChange) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** A stream that announces commands is open. Call the returned function once when it closes. */
  open(): () => void {
    let held = true;
    this.holders += 1;
    if (this.holders === 1) this.emit({ kind: "live", live: true });
    return () => {
      if (!held) return;
      held = false;
      this.holders -= 1;
      if (this.holders === 0) this.emit({ kind: "live", live: false });
    };
  }

  publish(command: CodeCommandChanged): void {
    this.emit({ kind: "command", command });
  }

  private emit(change: FeedChange): void {
    this.listeners.forEach((listener) => listener(change));
  }
}

// The account's CoreClient owns its feeds weakly, as it owns its control sessions.
const owners = new WeakMap<object, Map<string, CommandFeed>>();

/** The feed of `threadId` for the account that `owner` (its CoreClient) names. */
export function commandFeed(owner: object, threadId: string): CommandFeed {
  let feeds = owners.get(owner);
  if (!feeds) {
    feeds = new Map();
    owners.set(owner, feeds);
  }
  let feed = feeds.get(threadId);
  if (!feed) {
    feed = new CommandFeed();
    feeds.set(threadId, feed);
  }
  return feed;
}
