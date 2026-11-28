import { createCoreClient, type CodeAction, type CodeCommand, type CodeCommandEnvelope } from "@nexus/kit";
import { generateDeviceKey } from "@nexus/kit/e2e";
import { fixtureDevice, fixtureDeviceKey, fixtureKeyCopy, FIXTURE } from "@nexus/kit/synced-fixture";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { codeControlSession, deliveryPending, deliveryText, POLL_MS, SLOW_POLL_MS } from "./code-control";
import type { CodeWait, CodeWaitStore } from "./code-draft";
import { commandFeed } from "./command-feed";

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const missing = () => json({ error: { message: "Not found", code: "not_found" } }, 404);

async function setup({ lose = "none", locked = false, supported = true, delayPost = false, refuseRetry = false, asleep = false, gateway }: { lose?: "none" | "stored" | "missing"; locked?: boolean; supported?: boolean; delayPost?: boolean; refuseRetry?: boolean; asleep?: boolean; gateway?: Map<string, CodeCommand> } = {}) {
  // Gateway's receipts, by command id: a message and a Stop can be outstanding together. `gateway` keeps them across a
  // simulated app restart.
  const stored = gateway ?? new Map<string, CodeCommand>();
  const posts: CodeCommandEnvelope[] = [];
  const cancels: string[] = [];
  let lost = false;
  let releasePost: (() => void) | undefined;
  const sleeping = { value: asleep };
  /**
   * `loseCancelAnswer`: Gateway applies the cancel, and its answer is lost. `cancelDeletes`: Gateway keeps no tombstone,
   * so a later read is a 404 (a Gateway that lost the command).
   */
  const faults = { loseCancelAnswer: false, cancelDeletes: false, offline: false };
  const core = createCoreClient({
    baseUrl: "https://gateway.test", apiKey: "test", timeoutMs: 10_000,
    fetch: async (input, init) => {
      if (faults.offline) throw new TypeError("Connection lost");
      const url = new URL(String(input));
      if (url.pathname.endsWith("/control")) return supported ? json({ runnerDeviceId: FIXTURE.deviceId, enabled: true, expiresAt: Date.now() + (sleeping.value ? -1_000 : 60_000) }) : missing();
      if (url.pathname.endsWith("/cancel") && init?.method === "POST") {
        const id = decodeURIComponent(url.pathname.split("/").at(-2)!);
        cancels.push(id);
        const command = stored.get(id);
        if (!command) return json({ error: { message: "no such command", code: "code_command_not_found" } }, 404);
        if (command.state !== "queued" && command.state !== "cancelled") return json({ error: { message: "claimed", code: "code_command_not_queued" } }, 409);
        // Gateway keeps a cancelled command as a tombstone: a read, a repeated cancel and a resend all answer `cancelled`.
        if (faults.cancelDeletes) stored.delete(id);
        else stored.set(id, { ...command, state: "cancelled" });
        if (faults.loseCancelAnswer) throw new TypeError("Connection lost");
        return json({ ...command, state: "cancelled" });
      }
      if (url.pathname === "/v1/devices") return json({ devices: [fixtureDevice()] });
      if (url.pathname.endsWith("/keys")) return locked ? missing() : json(fixtureKeyCopy());
      if (url.pathname === `/v1/threads/${FIXTURE.threadId}`) return json({ threadId: FIXTURE.threadId, keyVersion: 1 });
      if (url.pathname.endsWith("/commands") && init?.method === "POST") {
        const envelope: CodeCommandEnvelope = JSON.parse(String(init.body));
        posts.push(envelope);
        if (refuseRetry && posts.length > 1) return json({ error: { message: "This device is no longer allowed to send", code: "device_not_trusted" } }, 403);
        const command: CodeCommand = { ...envelope, threadId: FIXTURE.threadId, state: stored.get(envelope.id)?.state ?? "queued" };
        if (!lost && lose !== "none") {
          lost = true;
          if (lose === "stored") stored.set(envelope.id, command);
          throw new TypeError("Connection lost");
        }
        stored.set(envelope.id, command);
        if (delayPost) return new Promise<Response>((resolve) => { releasePost = () => resolve(json({ ...command, state: "queued" })); });
        return json(command);
      }
      if (url.pathname.includes("/commands/")) {
        const command = stored.get(decodeURIComponent(url.pathname.split("/").at(-1)!));
        return command ? json(command) : missing();
      }
      throw new Error(`Unexpected request ${init?.method} ${url.pathname}`);
    },
  });
  const reader = { deviceId: FIXTURE.deviceId, holder: await fixtureDeviceKey() };
  const session = codeControlSession(core, FIXTURE.threadId, reader);
  session.start();
  await vi.waitFor(() => expect(session.snapshot().control.kind).toBe(!supported ? "unsupported" : asleep ? "offline" : "ready"));
  /** Moves one command's receipt (`id`), or every stored one. */
  const state = (next: CodeCommand["state"], id?: string) => {
    for (const [key, command] of stored) if (!id || key === id) stored.set(key, { ...command, state: next });
  };
  return { session, core, posts, cancels, sleeping, stored, faults, releasePost: () => releasePost?.(), state };
}

/** The phone's journal of waiting messages, as the disk keeps it across a restart (the real one is ciphertext). */
function waitJournal() {
  let saved: CodeWait | undefined;
  const store: CodeWaitStore = {
    load: async () => saved && structuredClone(saved),
    save: async (value) => { saved = structuredClone(value); },
    remove: async () => { saved = undefined; },
  };
  return { store, saved: () => saved };
}

describe("a waiting message after an app restart", () => {
  /** Sends a waiting message, then "restarts": a new client and session with the same Gateway and journal. */
  async function restarted() {
    const journal = waitJournal();
    const first = await setup({ asleep: true });
    first.session.attachWaits(journal.store);
    await first.session.submit({ kind: "send", text: "Run the slow suite" }, { waits: true });
    await vi.waitFor(() => expect(journal.saved()?.envelope.id).toBe(first.posts[0]!.id));
    expect(journal.saved()).toMatchObject({ text: "Run the slow suite", envelope: { waits: true } });
    first.session.pause();
    const second = await setup({ asleep: true, gateway: first.stored });
    return { journal, first, second };
  }

  it("comes back as its receipt, sends nothing again, and clears when the computer completes it", async () => {
    const { journal, first, second } = await restarted();
    const { session, posts, sleeping, state } = second;
    try {
      session.attachWaits(journal.store);
      await vi.waitFor(() => expect(session.snapshot().delivery).toMatchObject({ kind: "queued", id: first.posts[0]!.id, waitsFor: fixtureDevice().name }));
      expect(session.canCancel()).toBe(true);
      // The composer's slot is taken: a resend from the kept draft sends nothing.
      await session.submit({ kind: "send", text: "Run the slow suite" }, { waits: true });
      expect(posts).toHaveLength(0);
      sleeping.value = false;
      state("completed");
      await vi.advanceTimersByTimeAsync(POLL_MS);
      expect(session.snapshot().delivery).toMatchObject({ kind: "completed", action: { kind: "send", text: "Run the slow suite" } });
      await vi.waitFor(() => expect(journal.saved()).toBeUndefined());
    } finally {
      session.pause();
    }
  });

  it("can be cancelled after the restart", async () => {
    const { journal, first, second } = await restarted();
    const { session, cancels } = second;
    try {
      session.attachWaits(journal.store);
      await vi.waitFor(() => expect(session.canCancel()).toBe(true));
      await session.cancel();
      expect(cancels).toEqual([first.posts[0]!.id]);
      expect(session.snapshot().delivery.kind).toBe("cancelled");
      await vi.waitFor(() => expect(journal.saved()).toBeUndefined());
    } finally {
      session.pause();
    }
  });

  it("reads as cancelled when the app closed after a cancel and before it forgot the message", async () => {
    const journal = waitJournal();
    const first = await setup({ asleep: true });
    first.session.attachWaits(journal.store);
    await first.session.submit({ kind: "send", text: "Run the slow suite" }, { waits: true });
    await vi.waitFor(() => expect(journal.saved()?.envelope.id).toBe(first.posts[0]!.id));
    const saved = journal.saved()!;
    await first.session.cancel();
    expect(first.session.snapshot().delivery.kind).toBe("cancelled");
    first.session.pause();
    // The cancel landed on Gateway, but the app closed before it removed its journal entry.
    await journal.store.save(saved);
    const { session, posts } = await setup({ asleep: true, gateway: first.stored });
    try {
      session.attachWaits(journal.store);
      await vi.waitFor(() => expect(session.snapshot().delivery.kind).toBe("cancelled"));
      expect(deliveryText(session.snapshot().delivery)).toBe("Cancelled. Your message is still here.");
      expect(session.snapshot().delivery).toMatchObject({ action: { kind: "send", text: "Run the slow suite" } });
      await session.retry();
      expect(posts).toHaveLength(0);
      await vi.waitFor(() => expect(journal.saved()).toBeUndefined());
    } finally {
      session.pause();
    }
  });

  it("never offers a resend after a restart mid-cancel when Gateway has no record of it", async () => {
    const { journal, first, second } = await restarted();
    const { session, posts, cancels, faults } = second;
    try {
      // The app closes while the cancel is in flight. The journal already says the person cancelled.
      faults.cancelDeletes = true;
      session.attachWaits(journal.store);
      await vi.waitFor(() => expect(session.canCancel()).toBe(true));
      faults.loseCancelAnswer = true;
      await session.cancel();
      expect(cancels).toEqual([first.posts[0]!.id]);
      session.pause();
      expect(journal.saved()?.cancelling).toBe(true);
      const third = await setup({ asleep: true, gateway: second.stored });
      try {
        third.session.attachWaits(journal.store);
        await vi.waitFor(() => expect(third.session.snapshot().delivery.kind).toBe("recheck"));
        expect(deliveryText(third.session.snapshot().delivery)).toBe("Couldn't confirm the cancel. Check again.");
        expect(third.session.canCancel()).toBe(false);
        await third.session.retry();
        expect([...posts, ...third.posts]).toHaveLength(0);
        expect(third.session.snapshot().delivery.kind).toBe("recheck");
      } finally {
        third.session.pause();
      }
    } finally {
      session.pause();
    }
  });

  it("reads as expired when it expired while the app was closed, and is forgotten", async () => {
    const journal = waitJournal();
    const first = await setup({ asleep: true });
    first.session.attachWaits(journal.store);
    await first.session.submit({ kind: "send", text: "Too late" }, { waits: true });
    first.session.pause();
    first.state("expired");
    const { session, posts } = await setup({ asleep: true, gateway: first.stored });
    try {
      session.attachWaits(journal.store);
      await vi.waitFor(() => expect(session.snapshot().delivery.kind).toBe("expired"));
      expect(deliveryPending(session.snapshot().delivery)).toBe(false);
      expect(posts).toHaveLength(0);
      await vi.waitFor(() => expect(journal.saved()).toBeUndefined());
    } finally {
      session.pause();
    }
  });
});

describe("a message that waits for a sleeping computer", () => {
  it("names the computer, waits a day, and keeps the draft until the computer runs it", async () => {
    const { session, posts, sleeping, state } = await setup({ asleep: true });
    try {
      const control = session.snapshot().control;
      expect(control).toMatchObject({ kind: "offline", waits: { computer: fixtureDevice().name } });
      await session.submit({ kind: "send", text: "Run the slow suite" });
      expect(posts).toHaveLength(0);
      await session.submit({ kind: "send", text: "Run the slow suite" }, { waits: true });
      expect(posts).toHaveLength(1);
      expect(posts[0]).toMatchObject({ waits: true });
      expect(posts[0]!.expiresAt - Date.now()).toBeGreaterThan(23 * 60 * 60 * 1000);
      const delivery = session.snapshot().delivery;
      expect(deliveryText(delivery)).toBe(`Waits for ${fixtureDevice().name}. Runs when it's back, within 24 hours.`);
      expect(deliveryPending(delivery)).toBe(true);
      expect(session.canCancel()).toBe(true);
      // Stop and answers never wait.
      await session.submit({ kind: "stop" }, { waits: true });
      expect(session.snapshot().stop.kind).toBe("failed");
      expect(posts).toHaveLength(1);
      sleeping.value = false;
      state("claimed");
      await vi.advanceTimersByTimeAsync(POLL_MS);
      expect(session.canCancel()).toBe(false);
      state("completed");
      await vi.advanceTimersByTimeAsync(POLL_MS);
      expect(session.snapshot().delivery).toMatchObject({ kind: "completed", action: { kind: "send", text: "Run the slow suite" } });
    } finally {
      session.pause();
    }
  });

  it("keeps the waiting message visible offline and checks its original receipt after reconnection", async () => {
    const { session, faults, posts, sleeping, state } = await setup({ asleep: true });
    try {
      await session.submit({ kind: "send", text: "Run the slow suite" }, { waits: true });
      faults.offline = true;
      await vi.advanceTimersByTimeAsync(POLL_MS);
      const offline = session.snapshot();
      expect(offline.control.kind).toBe("reconnecting");
      expect(offline.delivery).toMatchObject({ id: posts[0]!.id, waitsFor: fixtureDevice().name });
      expect(deliveryText(offline.delivery)).toBe("Connection lost. Checking your message when Nexus is reachable.");
      expect(deliveryPending(offline.delivery)).toBe(true);
      expect(session.canCancel()).toBe(false);

      sleeping.value = false;
      state("completed");
      faults.offline = false;
      await vi.advanceTimersByTimeAsync(POLL_MS);
      expect(session.snapshot().delivery).toMatchObject({ kind: "completed", id: posts[0]!.id });
      expect(posts).toHaveLength(1);
    } finally {
      session.pause();
    }
  });

  it("cancels before the computer receives it, and checks the receipt when it already has", async () => {
    const { session, posts, cancels, state } = await setup({ asleep: true });
    try {
      await session.submit({ kind: "send", text: "Never mind" }, { waits: true });
      await session.cancel();
      expect(cancels).toEqual([posts[0]!.id]);
      expect(session.snapshot().delivery).toMatchObject({ kind: "cancelled", action: { kind: "send", text: "Never mind" } });
      expect(deliveryPending(session.snapshot().delivery)).toBe(false);
      expect(deliveryText(session.snapshot().delivery)).toMatch(/Cancelled/);

      await session.submit({ kind: "send", text: "Too late to cancel" }, { waits: true });
      state("claimed", posts[1]!.id);
      await session.cancel();
      expect(session.snapshot().delivery).toMatchObject({ kind: "claimed", id: posts[1]!.id });
    } finally {
      session.pause();
    }
  });

  it("reads cancelled when the cancel's answer is lost, and never sends the message again", async () => {
    const journal = waitJournal();
    const { session, posts, cancels, faults } = await setup({ asleep: true });
    try {
      session.attachWaits(journal.store);
      await session.submit({ kind: "send", text: "Never mind" }, { waits: true });
      faults.loseCancelAnswer = true;
      await session.cancel();
      expect(cancels).toEqual([posts[0]!.id]);
      expect(session.snapshot().delivery).toMatchObject({ kind: "cancelled", action: { kind: "send", text: "Never mind" } });
      expect(deliveryText(session.snapshot().delivery)).toBe("Cancelled. Your message is still here.");
      expect(deliveryPending(session.snapshot().delivery)).toBe(false);
      await vi.waitFor(() => expect(journal.saved()).toBeUndefined());
      await session.retry();
      expect(posts).toHaveLength(1);
    } finally {
      session.pause();
    }
  });

  it("offers Check again, never Retry, when a cancel's receipt is missing", async () => {
    const { session, posts, faults } = await setup({ asleep: true });
    try {
      await session.submit({ kind: "send", text: "Do not run this" }, { waits: true });
      faults.cancelDeletes = true;
      faults.loseCancelAnswer = true;
      await session.cancel();
      expect(session.snapshot().delivery).toMatchObject({ kind: "recheck", message: "Couldn't confirm the cancel. Check again.", action: { kind: "send", text: "Do not run this" } });
      expect(deliveryPending(session.snapshot().delivery)).toBe(true);
      expect(session.canCancel()).toBe(false);
      // Retry and Check again only read the receipt; the poll does too.
      await session.retry();
      await session.recheck();
      await vi.advanceTimersByTimeAsync(POLL_MS * 2);
      expect(session.snapshot().delivery.kind).toBe("recheck");
      expect(posts).toHaveLength(1);
      // The person may stop waiting; the phone still sends nothing.
      expect(session.canStopChecking()).toBe(true);
      session.stopChecking();
      expect(session.snapshot().delivery.kind).toBe("unconfirmed");
      expect(posts).toHaveLength(1);
    } finally {
      session.pause();
    }
  });
});

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("a phone command's delivery", () => {
  it("keeps a delayed queued receipt pending when the runner claimed it before the receipt arrived", async () => {
    const { session, posts, state, releasePost } = await setup({ delayPost: true });
    try {
      const sending = session.submit({ kind: "send", text: "Run this once" });
      await vi.waitFor(() => expect(posts).toHaveLength(1));
      state("claimed");
      vi.setSystemTime(Date.now() + 60_500);
      releasePost();
      await sending;
      expect(session.snapshot().delivery.kind).toBe("queued");
      expect(deliveryPending(session.snapshot().delivery)).toBe(true);
      await session.submit({ kind: "send", text: "Run this once" });
      expect(posts).toHaveLength(1);
      await vi.advanceTimersByTimeAsync(2000);
      expect(session.snapshot().delivery.kind).toBe("claimed");
    } finally {
      session.pause();
    }
  });

  it("reacquires an unresolved command after leaving the screen, with a newly loaded copy of the same device key", async () => {
    const { session, core, posts } = await setup({ lose: "stored" });
    let reopened = session;
    try {
      await session.submit({ kind: "send", text: "Still waiting for this receipt" });
      const first = session.snapshot().delivery;
      expect(first.kind).toBe("unknown");
      session.pause();
      reopened = codeControlSession(core, FIXTURE.threadId, { deviceId: FIXTURE.deviceId, holder: await fixtureDeviceKey() });
      reopened.start();
      await vi.waitFor(() => expect(reopened.snapshot().delivery.kind).toBe("queued"));
      expect(reopened.snapshot().delivery).toMatchObject({ id: posts[0]!.id, action: { kind: "send", text: "Still waiting for this receipt" } });
      await reopened.submit({ kind: "send", text: "Still waiting for this receipt" });
      expect(posts).toHaveLength(1);
    } finally {
      session.pause();
      reopened.pause();
    }
  });

  it("does not expose a pending action under another account, thread, device or device key", async () => {
    const { session, core } = await setup({ lose: "stored" });
    try {
      await session.submit({ kind: "send", text: "Private to this account and device" });
      session.pause();
      const reader = { deviceId: FIXTURE.deviceId, holder: await fixtureDeviceKey() };
      const otherCore = createCoreClient({ baseUrl: "https://gateway.test", apiKey: "other-account" });
      const separated = [
        codeControlSession(otherCore, FIXTURE.threadId, reader),
        codeControlSession(core, "another-thread", reader),
        codeControlSession(core, FIXTURE.threadId, { ...reader, deviceId: "another-device" }),
        codeControlSession(core, FIXTURE.threadId, { ...reader, holder: await generateDeviceKey() }),
      ];
      expect(separated.map((controller) => controller.snapshot().delivery)).toEqual([
        { kind: "idle" }, { kind: "idle" }, { kind: "idle" }, { kind: "idle" },
      ]);
      expect(separated.map((controller) => controller.snapshot().stop)).toEqual([
        { kind: "idle" }, { kind: "idle" }, { kind: "idle" }, { kind: "idle" },
      ]);
    } finally {
      session.pause();
    }
  });

  it("keeps queued and claimed actions pending until the computer reports completion", async () => {
    const { session, posts, state } = await setup();
    try {
      await session.submit({ kind: "send", text: "Check the parser" });
      expect(session.snapshot().delivery.kind).toBe("queued");
      expect(deliveryPending(session.snapshot().delivery)).toBe(true);
      expect(deliveryText(session.snapshot().delivery)).toBe("Waiting for the computer to receive it…");
      expect(posts).toHaveLength(1);
      expect(JSON.stringify(posts[0])).not.toContain("Check the parser");
      state("claimed");
      await vi.advanceTimersByTimeAsync(2000);
      expect(session.snapshot().delivery.kind).toBe("claimed");
      expect(deliveryPending(session.snapshot().delivery)).toBe(true);
      state("completed");
      await vi.advanceTimersByTimeAsync(2000);
      expect(session.snapshot().delivery).toMatchObject({ kind: "completed", action: { kind: "send", text: "Check the parser" } });
      expect(deliveryPending(session.snapshot().delivery)).toBe(false);
    } finally {
      session.pause();
    }
  });

  it("checks a lost receipt on refocus without posting the action again", async () => {
    const { session, posts, state } = await setup({ lose: "stored" });
    try {
      await session.submit({ kind: "stop" });
      expect(session.snapshot().stop.kind).toBe("unknown");
      await session.submit({ kind: "stop" });
      session.pause();
      session.start();
      await vi.waitFor(() => expect(session.snapshot().stop.kind).toBe("queued"));
      expect(posts).toHaveLength(1);
      state("rejected");
      await vi.advanceTimersByTimeAsync(2000);
      expect(session.snapshot().stop.kind).toBe("rejected");
    } finally {
      session.pause();
    }
  });

  it("retries an unconfirmed delivery with the same encrypted command and id", async () => {
    const { session, posts } = await setup({ lose: "missing" });
    try {
      await session.submit({ kind: "send", text: "Keep this draft" });
      await vi.advanceTimersByTimeAsync(2000);
      expect(session.snapshot().delivery.kind).toBe("retry");
      await session.retry();
      expect(posts).toHaveLength(2);
      expect(posts[1]).toEqual(posts[0]);
      expect(session.snapshot().delivery.kind).toBe("queued");
    } finally {
      session.pause();
    }
  });

  it("does not infer expiry from a missing receipt or resend after the local deadline", async () => {
    const { session, posts } = await setup({ lose: "missing" });
    try {
      await session.submit({ kind: "send", text: "Delivery is uncertain" });
      vi.setSystemTime(Date.now() + 60_500);
      await vi.advanceTimersByTimeAsync(2000);
      expect(session.snapshot().delivery.kind).toBe("unknown");
      expect(deliveryPending(session.snapshot().delivery)).toBe(true);
      await session.retry();
      expect(session.snapshot().delivery.kind).toBe("unknown");
      await session.submit({ kind: "send", text: "Delivery is uncertain" });
      expect(posts).toHaveLength(1);
      session.stopChecking();
      expect(session.snapshot().delivery.kind).toBe("unconfirmed");
    } finally {
      session.pause();
    }
  });

  it("queries the original receipt when a retry is refused after an uncertain send", async () => {
    const { session, posts, state } = await setup({ lose: "stored", refuseRetry: true });
    try {
      await session.submit({ kind: "send", text: "The computer may already have this" });
      state("claimed");
      await session.retry();
      expect(posts).toHaveLength(2);
      expect(posts[1]).toEqual(posts[0]);
      expect(session.snapshot().delivery.kind).toBe("claimed");
      expect(deliveryPending(session.snapshot().delivery)).toBe(true);
      await session.submit({ kind: "send", text: "The computer may already have this" });
      expect(posts).toHaveLength(2);
    } finally {
      session.pause();
    }
  });

  it("sends nothing when the phone cannot open the session's current key", async () => {
    const { session, posts } = await setup({ locked: true });
    try {
      await session.submit({ kind: "send", text: "Cannot send without the key" });
      expect(session.snapshot().delivery.kind).toBe("failed");
      expect(posts).toHaveLength(0);
    } finally {
      session.pause();
    }
  });

  it("never calls a claimed action expired, and lets the person stop checking after its deadline", async () => {
    const { session, posts, state } = await setup();
    try {
      await session.submit({ kind: "send", text: "Do not submit this twice" });
      state("claimed");
      await vi.advanceTimersByTimeAsync(2000);
      expect(session.canStopChecking()).toBe(false);
      await vi.advanceTimersByTimeAsync(60_000);
      expect(session.snapshot().delivery.kind).toBe("claimed");
      expect(session.canStopChecking()).toBe(true);
      await session.submit({ kind: "send", text: "Do not submit this twice" });
      expect(posts).toHaveLength(1);
      session.stopChecking();
      expect(session.snapshot().delivery.kind).toBe("unconfirmed");
      expect(deliveryText(session.snapshot().delivery)).toContain("may already have run");
      expect(deliveryPending(session.snapshot().delivery)).toBe(false);
      expect(posts).toHaveLength(1);
    } finally {
      session.pause();
    }
  });

  it("sends Stop while a message is still queued, each with its own command and receipt", async () => {
    const { session, posts, state } = await setup();
    try {
      await session.submit({ kind: "send", text: "Refactor the parser" });
      expect(session.snapshot().delivery.kind).toBe("queued");
      await session.submit({ kind: "stop" });
      expect(posts).toHaveLength(2);
      expect(posts[1]!.id).not.toBe(posts[0]!.id);
      expect(session.snapshot().stop).toMatchObject({ kind: "queued", id: posts[1]!.id, action: { kind: "stop" } });
      expect(session.snapshot().delivery).toMatchObject({ kind: "queued", id: posts[0]!.id });
      // A second Stop while the first is outstanding sends nothing.
      await session.submit({ kind: "stop" });
      expect(posts).toHaveLength(2);
      state("completed", posts[1]!.id);
      await vi.advanceTimersByTimeAsync(2000);
      expect(session.snapshot().stop.kind).toBe("completed");
      expect(deliveryText(session.snapshot().stop)).toBe("The session stopped.");
      expect(session.snapshot().delivery.kind).toBe("queued");
      state("expired", posts[0]!.id);
      await vi.advanceTimersByTimeAsync(2000);
      expect(session.snapshot().delivery.kind).toBe("expired");
      expect(deliveryText(session.snapshot().delivery)).toContain("Your draft is still here");
    } finally {
      session.pause();
    }
  });

  it("retries an unconfirmed Stop with the same command, apart from a pending message", async () => {
    const { session, posts } = await setup({ lose: "missing" });
    try {
      await session.submit({ kind: "stop" });
      expect(session.snapshot().stop.kind).toBe("unknown");
      await session.submit({ kind: "send", text: "Then do this" });
      expect(session.snapshot().delivery.kind).toBe("queued");
      await vi.advanceTimersByTimeAsync(2000);
      expect(session.snapshot().stop.kind).toBe("retry");
      expect(session.snapshot().delivery.kind).toBe("queued");
      await session.retry("stop");
      expect(posts).toHaveLength(3);
      expect(posts[2]).toEqual(posts[0]);
      expect(session.snapshot().stop.kind).toBe("queued");
      expect(session.snapshot().delivery.kind).toBe("queued");
    } finally {
      session.pause();
    }
  });

  it("names the action when a command expires, and never says the computer did not receive it", () => {
    // Gateway also expires a claimed command after its deadline, so expiry does not prove the command never ran.
    const expired = (action: CodeAction) => deliveryText({ kind: "expired", id: "c", action })!;
    const send = expired({ kind: "send", text: "hi" });
    const stop = expired({ kind: "stop" });
    const answer = expired({ kind: "approve", requestId: "r", decision: "allow", message: null, answers: null });
    expect(send).toContain("The computer did not confirm this message in time. It may still have run. Check the session before sending it again.");
    expect(send).toContain("Your draft is still here");
    expect(stop).toContain("did not confirm Stop in time");
    expect(stop).not.toContain("draft");
    expect(answer).toContain("did not confirm your answer in time");
    for (const text of [send, stop, answer]) {
      expect(text).toContain("Check the session");
      expect(text).not.toMatch(/before the computer received/);
    }
  });

  it("reports an older Gateway as unsupported", async () => {
    const { session, posts } = await setup({ supported: false });
    try {
      expect(session.snapshot().control.kind).toBe("unsupported");
      expect(posts).toHaveLength(0);
    } finally {
      session.pause();
    }
  });
});

describe("receipts on the session's stream", () => {
  it("reads a receipt the moment the stream announces it, and polls slowly while the stream is live", async () => {
    const { session, core, posts, state } = await setup();
    const feed = commandFeed(core, FIXTURE.threadId);
    const close = feed.open();
    try {
      await session.submit({ kind: "stop" });
      expect(session.snapshot().stop.kind).toBe("queued");
      state("completed");
      // Well inside the slow poll, nothing reads the receipt but an announcement.
      await vi.advanceTimersByTimeAsync(POLL_MS * 3);
      expect(session.snapshot().stop.kind).toBe("queued");
      feed.publish({ threadId: FIXTURE.threadId, id: posts[0]!.id, runnerDeviceId: FIXTURE.deviceId, state: "completed" });
      await vi.waitFor(() => expect(session.snapshot().stop.kind).toBe("completed"));
      // The slow poll is the backstop for an announcement that was lost.
      await session.submit({ kind: "send", text: "Lost announcement" });
      state("claimed", posts[1]!.id);
      await vi.advanceTimersByTimeAsync(SLOW_POLL_MS);
      expect(session.snapshot().delivery.kind).toBe("claimed");
    } finally {
      close();
      session.pause();
    }
  });

  it("polls every 2 s again once the stream drops, and ignores another thread's announcement", async () => {
    const { session, core, posts, state } = await setup();
    const feed = commandFeed(core, FIXTURE.threadId);
    const close = feed.open();
    try {
      await session.submit({ kind: "send", text: "Waiting" });
      state("claimed");
      feed.publish({ threadId: "another-thread", id: posts[0]!.id, runnerDeviceId: FIXTURE.deviceId, state: "claimed" });
      await vi.advanceTimersByTimeAsync(POLL_MS * 3);
      expect(session.snapshot().delivery.kind).toBe("queued");
      close();
      await vi.advanceTimersByTimeAsync(POLL_MS);
      expect(session.snapshot().delivery.kind).toBe("claimed");
    } finally {
      close();
      session.pause();
    }
  });
});
