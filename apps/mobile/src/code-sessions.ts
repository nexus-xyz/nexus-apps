/**
 * Code on the phone, its pure parts: where this phone stands as a reader of your synced coding sessions,
 * and what a session's row says. Ported from Nexus UI's synced viewer (`ui/…/synced/model.ts`, `reader.ts`), with the
 * phone's own states for pairing. No React Native, so vitest runs it.
 */
import { amountOrNull, type E2eDevice, type SyncedStatus, type SyncedThread, usdText } from "@nexus/kit";
import type { ExecutionOrigin } from "@nexus/kit/computer-types";
import type { DeviceTrustMode } from "@nexus/kit/device-trust";
import type { DeviceKey } from "@nexus/kit/e2e";
import { deviceStatus, thisDevice } from "@nexus/kit/pairing";
import type { OpenedEvent, SyncedReader } from "@nexus/kit/synced-sessions";
import { needsGatewayRegistration } from "./gateway-device";

/**
 * Where this phone stands. `unpaired`: no key here, or a key Gateway does not list. `pending`: it shows its code until
 * a trusted device approves it. `expired`: it waited past 15 minutes. `revoked`: it gets no keys. `ready`: it opens
 * what was locked for it.
 */
export type ReaderState =
  | { kind: "loading" }
  | { kind: "unpaired" }
  | { kind: "pending" | "expired" | "revoked"; self: E2eDevice; devices: E2eDevice[] }
  | { kind: "ready"; reader: SyncedReader & { holder: DeviceKey }; devices: E2eDevice[] };

/**
 * `key` is `null` while the Keychain is read, `undefined` when this phone has none. `trust` is the devices list's
 * mode. With `gateway`, a phone with no trusted row is `loading` while it registers, and is trusted at once
 * (`gateway-device.ts`); it never waits for an approval. Only a revoked phone stops.
 */
export function readerState(key: DeviceKey | undefined | null, devices: E2eDevice[] | undefined, now = Date.now(), trust: DeviceTrustMode = "approvals"): ReaderState {
  if (key === null || !devices) return { kind: "loading" };
  const self = key ? thisDevice(devices, key) : undefined;
  if (needsGatewayRegistration(trust, self)) return { kind: "loading" };
  if (!key || !self) return { kind: "unpaired" };
  const status = deviceStatus(self, now, trust);
  if (status === "trusted") return { kind: "ready", reader: { deviceId: self.id, holder: key, ...(trust === "gateway" ? { trust } : {}) }, devices };
  return { kind: status, self, devices };
}

/**
 * What this phone can read of a thread. `empty`: no event yet. `local`: its organization keeps content on the machine.
 * `no-key`: this phone holds no copy of its key. `sealed`: there is a copy to try.
 */
export type Access = "empty" | "local" | "no-key" | "sealed";

/** `held`: the threads this phone holds a key copy for; `undefined` until read. */
export function accessOf(thread: Pick<SyncedThread, "threadId" | "lastSeq" | "keyVersion">, held: ReadonlySet<string> | undefined): Access {
  if (thread.lastSeq === null) return "empty";
  if (thread.keyVersion === null) return "local";
  return held?.has(thread.threadId) ? "sealed" : "no-key";
}

/** A thread's title once this phone opened it. */
export interface Title {
  title: string;
  projectName: string;
  origin?: ExecutionOrigin;
}

/**
 * A title as the list shows it: opened, or why not. `failed`: the read failed, and the next refresh tries again.
 * `waiting`: the event it opened from asks you something (`waitsOnYou`).
 */
export type TitleState = { state: "open"; title: Title; waiting?: boolean } | { state: "local" | "no-key" | "unreadable" | "failed" };

/**
 * The thread's latest event asks you something: a tool's approval or the agent's question, not yet answered (an
 * answer is a later `approvalResolved`). The list reads only that one event, so a call that runs on after the request
 * (a sub-agent's, say) shows the thread as working instead.
 */
export function waitsOnYou(latest: OpenedEvent | undefined): boolean {
  return latest?.state === "open" && latest.event.kind === "approval";
}

/** The newest title the opened events carry: every event holds it, so a rename shows on the next. */
export function latestTitle(events: readonly OpenedEvent[]): Title | undefined {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (event.state === "open") return { title: event.thread.title, projectName: event.thread.projectName, ...(event.thread.origin ? { origin: event.thread.origin } : {}) };
  }
  return undefined;
}

const AGENTS: Record<string, string> = { claude: "Claude Code", codex: "Codex", nexus: "Nexus" };

export function agentName(agent: string | null): string | null {
  return agent ? (AGENTS[agent] ?? agent) : null;
}

/** A thread's state in words, as its row and VoiceOver say it. */
export const STATUS_WORDS: Record<SyncedStatus, string> = { running: "Running", completed: "Done", interrupted: "Stopped", error: "Failed" };

/** The phone's status mark for a synced thread (`status-mark.tsx`). */
export function markOf(status: SyncedStatus | null): "running" | "succeeded" | "failed" | "interrupted" {
  return status === "running" ? "running" : status === "error" ? "failed" : status === "interrupted" ? "interrupted" : "succeeded";
}

/** A decimal string of dollars as the phone shows money, or null when it is none or not a number. */
export function costText(decimal: string | null | undefined): string | null {
  const micros = amountOrNull(decimal, null);
  return micros === null || micros === 0 ? null : usdText(micros);
}

/** The title a row shows: the opened one, or what stands for it. */
export function titleText(access: Access, title: TitleState | undefined): string {
  if (title?.state === "open") return title.title.title.trim() || "Untitled session";
  if (access === "empty") return "New session";
  if (access === "local") return "Kept on its computer";
  return "Locked session";
}

/**
 * A row's second line: the state, the agent, the model and the cost, in plain text (Gateway reads all four). The
 * repository comes first once the title opened. `model` names a model id for people.
 */
export function rowDetail(thread: Pick<SyncedThread, "status" | "agent" | "model" | "totals">, title: TitleState | undefined, model: (id: string) => string): string {
  return [
    title?.state === "open" ? title.title.projectName || null : null,
    thread.status ? STATUS_WORDS[thread.status] : null,
    agentName(thread.agent),
    thread.model ? model(thread.model) : null,
    costText(thread.totals.costUsd),
  ]
    .filter(Boolean)
    .join(" · ");
}

/** When the thread last moved. */
export function movedAt(thread: Pick<SyncedThread, "lastEventAt" | "updatedAt">): string {
  return thread.lastEventAt ?? thread.updatedAt;
}

/** The list's two groups, as P18 draws them: Today, then Earlier, each newest first. Empty groups are left out. */
export function groupThreads<T extends Pick<SyncedThread, "lastEventAt" | "updatedAt">>(threads: readonly T[], now = Date.now()): { title: "Today" | "Earlier"; threads: T[] }[] {
  const midnight = new Date(now);
  midnight.setHours(0, 0, 0, 0);
  const sorted = [...threads].sort((a, b) => Date.parse(movedAt(b)) - Date.parse(movedAt(a)));
  const today = sorted.filter((thread) => Date.parse(movedAt(thread)) >= midnight.getTime());
  const earlier = sorted.filter((thread) => Date.parse(movedAt(thread)) < midnight.getTime());
  return [
    { title: "Today" as const, threads: today },
    { title: "Earlier" as const, threads: earlier },
  ].filter((group) => group.threads.length);
}

/** `held` and `fresh` as one list in number order, each number once: a later copy of a number replaces the earlier. */
export function mergeEvents(held: readonly OpenedEvent[], fresh: readonly OpenedEvent[]): OpenedEvent[] {
  const bySeq = new Map(held.map((event) => [event.seq, event]));
  for (const event of fresh) bySeq.set(event.seq, event);
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
}

/** How many held events this phone could not show, by why. */
export function unreadCounts(events: readonly OpenedEvent[]): { local: number; noKey: number; unreadable: number } {
  const count = (state: OpenedEvent["state"]) => events.filter((event) => event.state === state).length;
  return { local: count("local"), noKey: count("no-key"), unreadable: count("unreadable") };
}

/** The line under the pairing code: how long it may still be approved. */
export function waitText(createdAt: string, now: number, ttlMs: number): string {
  const minutes = Math.ceil((Date.parse(createdAt) + ttlMs - now) / 60_000);
  return minutes <= 1 ? "Under a minute left" : `${minutes} minutes left`;
}
