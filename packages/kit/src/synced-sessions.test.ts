import { deviceFingerprint } from "./pairing";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { E2eDevice, ThreadKey } from "./api";
import { confirmDeviceApprover, memoryDeviceTrust, type DeviceTrustStore } from "./device-trust";
import { approvalMessage, b64u, fromB64u, generateDeviceKey, importHolderSecret, lockContentKey, lockTeamKey, openEvent, sign, teamThreadKeyMessage, threadKeyMessage } from "./e2e";
import { FIXTURE, fixtureDevice, fixtureDeviceKey, fixtureKeyCopy, lockFixtureKey, sealFixtureEvent } from "./synced-fixture";
import { gatewayTeamKeys, openSyncedEvent, readPlaintext, sealedBytes, SyncedKeyError, ThreadKeys, type PrivateThread } from "./synced-sessions";
import { syncedTimeline } from "./synced-timeline";
import { rememberTeamTrust, type TeamTrustMemory, type TeamTrustStore } from "./team-trust";

const vectors = JSON.parse(readFileSync(fileURLToPath(new URL("../../../crates/nexus-client/tests/e2e-vectors.json", import.meta.url)), "utf8")) as {
  events: { content_key: string; epoch: number; thread_id: string; seq: number; sealed: string }[];
  devices: { secret: string; encryption_key_b64u: string; signing_key_b64u: string }[];
  thread_keys: { thread_id: string; key_version: number; device_id: string; locked: string; signature: string }[];
  signer_secret: string;
};

const hex = (text: string) => Uint8Array.from(text.match(/../g) ?? [], (pair) => parseInt(pair, 16));
const THREAD: PrivateThread = { localId: "t_local_1", title: "Fix the flaky retry test", projectName: "nexus", projectPath: "/Users/ana/code/nexus" };

async function keysFor(devices: E2eDevice[], copies: Record<number, ThreadKey | undefined>, threadId: string = FIXTURE.threadId) {
  const holder = await fixtureDeviceKey();
  return new ThreadKeys(threadId, { deviceId: FIXTURE.deviceId, holder }, devices, async (version) => copies[version]);
}

describe("synced sessions: unlock and open", () => {
  it("uses the Rust core's vector device, copy and signature as they are", () => {
    const device = vectors.devices[0]!;
    const copy = vectors.thread_keys[0]!;
    expect(FIXTURE.deviceSecret).toBe(device.secret);
    expect(vectors.signer_secret).toBe(device.secret);
    expect(FIXTURE.encryptionKey).toBe(device.encryption_key_b64u);
    expect(FIXTURE.signingKey).toBe(device.signing_key_b64u);
    expect([FIXTURE.threadId, FIXTURE.deviceId, FIXTURE.locked, FIXTURE.signature]).toEqual([copy.thread_id, copy.device_id, copy.locked, copy.signature]);
    expect(copy.key_version).toBe(1);
  });

  it("unlocks the Rust-locked copy and opens a Rust-sealed event with it", async () => {
    const keys = await keysFor([fixtureDevice()], { 1: fixtureKeyCopy() });
    const key = await keys.at(1);
    expect(key.epoch).toBe(0);
    const sealed = vectors.events.find((event) => event.thread_id === FIXTURE.threadId && event.epoch === 0)!;
    expect(sealed.content_key).toBe(FIXTURE.contentKey);
    // The vector's body is no `Plaintext`, so it reads as unreadable: it opens, then is refused by its format.
    await expect(openEvent(key, FIXTURE.threadId, sealed.seq, hex(sealed.sealed))).resolves.toBeInstanceOf(Uint8Array);
    const event = { seq: sealed.seq, at: 1, agent: "claude", keyVersion: 1, sealed: b64u(hex(sealed.sealed)) };
    expect((await openSyncedEvent(keys, event)).state).toBe("unreadable");
  });

  it("opens a runner's Plaintext: the thread's title and project, and the stored event", async () => {
    const keys = await keysFor([fixtureDevice()], { 1: fixtureKeyCopy() });
    const located: PrivateThread = { ...THREAD, origin: { kind: "computer", computerId: "0192f3a4-0000-7000-8000-000000000001", name: "Eduardo’s MacBook" } };
    const event = await sealFixtureEvent(FIXTURE.threadId, 0, 1_700_000_000_000, located, { kind: "user", text: "Why does retry flake?" });
    expect(await openSyncedEvent(keys, event)).toEqual({
      state: "open",
      seq: 0,
      at: 1_700_000_000_000,
      thread: located,
      event: { kind: "user", text: "Why does retry flake?" },
    });
  });

  it("opens each event with the key at its own version: epoch = keyVersion − 1", async () => {
    const rotated = new Uint8Array(32).fill(7);
    const keys = await keysFor([fixtureDevice()], { 1: fixtureKeyCopy(), 2: await lockFixtureKey(FIXTURE.threadId, 1, rotated) });
    const before = await sealFixtureEvent(FIXTURE.threadId, 0, 1, THREAD, { kind: "notice", text: "old key" });
    const after = await sealFixtureEvent(FIXTURE.threadId, 1, 2, THREAD, { kind: "notice", text: "new key" }, { epoch: 1, contentKey: rotated });
    expect((await openSyncedEvent(keys, before)).state).toBe("open");
    expect((await openSyncedEvent(keys, after)).state).toBe("open");
    // The same body claimed at the other version does not open.
    expect((await openSyncedEvent(keys, { ...after, keyVersion: 1 })).state).toBe("unreadable");
  });

  it("refuses an event moved to another number or thread", async () => {
    const keys = await keysFor([fixtureDevice()], { 1: fixtureKeyCopy() });
    const event = await sealFixtureEvent(FIXTURE.threadId, 3, 1, THREAD, { kind: "text", id: "a", text: "hi" });
    expect((await openSyncedEvent(keys, { ...event, seq: 4 })).state).toBe("unreadable");
    const elsewhere = await keysFor([fixtureDevice()], { 1: await lockFixtureKey("0192f3a4-0000-7000-8000-000000000001", 0, hex(FIXTURE.contentKey)) }, "0192f3a4-0000-7000-8000-000000000001");
    expect((await openSyncedEvent(elsewhere, event)).state).toBe("unreadable");
  });

  it("says no-key when this device holds no copy, and local when the content never leaves the machine", async () => {
    const keys = await keysFor([fixtureDevice()], {});
    const event = await sealFixtureEvent(FIXTURE.threadId, 0, 1, THREAD, { kind: "user", text: "x" });
    expect((await openSyncedEvent(keys, event)).state).toBe("no-key");
    await expect(keys.at(1)).rejects.toSatisfy((error) => error instanceof SyncedKeyError && error.kind === "none");
    expect(await openSyncedEvent(keys, { seq: 1, at: 2, agent: "claude", keyVersion: null, sealed: null })).toEqual({ state: "local", seq: 1, at: 2 });
  });

  it("says unreadable, not local, for a sealed body with no key version", async () => {
    const keys = await keysFor([fixtureDevice()], {});
    const event = await sealFixtureEvent(FIXTURE.threadId, 0, 1, THREAD, { kind: "user", text: "x" });
    expect(await openSyncedEvent(keys, { ...event, keyVersion: null })).toEqual({ state: "unreadable", seq: 0, at: event.at });
    expect(await openSyncedEvent(keys, { ...event, keyVersion: undefined })).toEqual({ state: "unreadable", seq: 0, at: event.at });
    expect(await openSyncedEvent(keys, { ...event, sealed: null })).toEqual({ state: "local", seq: 0, at: event.at });
  });

  it("uses no copy that a trusted device of yours did not sign", async () => {
    const copy = fixtureKeyCopy();
    const signature = fromB64u(copy.signature)!;
    signature[0]! ^= 1;
    const forged = { ...copy, signature: b64u(signature) };
    const untrusted = (error: unknown) => error instanceof SyncedKeyError && error.kind === "untrusted";
    await expect((await keysFor([fixtureDevice()], { 1: forged })).at(1)).rejects.toSatisfy(untrusted);
    // A signer Gateway lists but this device does not trust: pending, or unknown.
    await expect((await keysFor([fixtureDevice({ state: "pending" })], { 1: copy })).at(1)).rejects.toSatisfy(untrusted);
    await expect((await keysFor([], { 1: copy })).at(1)).rejects.toSatisfy(untrusted);
    // A copy its signer stored after it was revoked.
    const revoked = fixtureDevice({ state: "revoked", approvedAt: "2026-11-01T09:00:00Z", revokedAt: "2026-11-01T09:01:00Z" });
    await expect((await keysFor([revoked], { 1: copy })).at(1)).rejects.toSatisfy(untrusted);
    const event = await sealFixtureEvent(FIXTURE.threadId, 0, 1, THREAD, { kind: "user", text: "x" });
    expect((await openSyncedEvent(await keysFor([fixtureDevice()], { 1: forged }), event)).state).toBe("unreadable");
  });

  it("refuses a copy Gateway gives for another device, thread or version", async () => {
    const copy = fixtureKeyCopy();
    for (const wrong of [{ ...copy, deviceId: "dev_other" }, { ...copy, threadId: "0192f3a4-0000-7000-8000-000000000001" }, { ...copy, keyVersion: 2 }]) {
      await expect((await keysFor([fixtureDevice()], { 1: wrong })).at(1)).rejects.toSatisfy((error) => error instanceof SyncedKeyError && error.kind === "none");
    }
  });

  it("reads base64url, and standard base64 from today's runner engine", () => {
    const bytes = Uint8Array.of(251, 255, 191, 0, 1);
    expect(sealedBytes(b64u(bytes))).toEqual(bytes);
    expect(sealedBytes(Buffer.from(bytes).toString("base64"))).toEqual(bytes);
    expect(sealedBytes("not base64!")).toBeUndefined();
  });

  it("reads only Plaintext version 1 with a stored event that names its kind", () => {
    const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value));
    const thread = { local_id: "l", title: "t", project_name: "p", project_path: "/p" };
    expect(readPlaintext(encode({ version: 1, at: 5, thread, event_json: '{"kind":"notice","text":"x"}' }))?.event).toEqual({ kind: "notice", text: "x" });
    expect(readPlaintext(encode({ version: 2, at: 5, thread, event_json: '{"kind":"notice"}' }))).toBeUndefined();
    expect(readPlaintext(encode({ version: 1, at: 5, thread, event_json: '{"text":"no kind"}' }))).toBeUndefined();
    expect(readPlaintext(encode({ version: 1, at: 5, thread, event_json: "not json" }))).toBeUndefined();
    expect(readPlaintext(Uint8Array.of(0xff, 0xfe))).toBeUndefined();
  });

  it("asks Gateway again for a copy it refused, once the person may have paired the device", async () => {
    let copy: ThreadKey | undefined;
    const keys = new ThreadKeys(FIXTURE.threadId, { deviceId: FIXTURE.deviceId, holder: await fixtureDeviceKey() }, [fixtureDevice()], async () => copy);
    await expect(keys.at(1)).rejects.toBeInstanceOf(SyncedKeyError);
    copy = fixtureKeyCopy();
    await expect(keys.at(1)).resolves.toMatchObject({ epoch: 0 });
  });
});

describe("synced sessions: whose copies this device trusts (S2)", () => {
  type Key = Awaited<ReturnType<typeof generateDeviceKey>>;
  const untrusted = (error: unknown) => error instanceof SyncedKeyError && error.kind === "untrusted";

  /** A listed device, approved by `approver` over its keys, or a root with none. */
  async function row(id: string, key: Key, approver?: { id: string; key: Key }, extra: Partial<E2eDevice> = {}): Promise<E2eDevice> {
    const signature = approver && (await sign(approver.key, approvalMessage(id, key.encryptionKey, key.signingKey)));
    return fixtureDevice({ id, name: id, encryptionKey: b64u(key.encryptionKey), signingKey: b64u(key.signingKey), approvedBy: approver?.id ?? null, approvalSignature: signature ? b64u(signature) : null, approvedAt: approver ? "2026-11-01T09:00:00Z" : null, ...extra });
  }

  /** The fixture's content key, locked to the fixture device and signed by `signer`. */
  async function copyBy(sender: string, signer: Key, createdAt = "2026-11-01T09:05:00Z"): Promise<ThreadKey> {
    const locked = await lockContentKey(hex(FIXTURE.contentKey), FIXTURE.threadId, 0, FIXTURE.deviceId, fromB64u(FIXTURE.encryptionKey)!);
    const signature = await sign(signer, threadKeyMessage(FIXTURE.threadId, 0, FIXTURE.deviceId, locked));
    return { threadId: FIXTURE.threadId, keyVersion: 1, deviceId: FIXTURE.deviceId, lockedKey: b64u(locked), senderDeviceId: sender, signature: b64u(signature), createdAt };
  }

  async function keysWith(devices: E2eDevice[], copy: ThreadKey, store: DeviceTrustStore = memoryDeviceTrust()) {
    return new ThreadKeys(FIXTURE.threadId, { deviceId: FIXTURE.deviceId, holder: await fixtureDeviceKey() }, devices, async () => copy, undefined, store);
  }

  it("refuses a copy from a second root Gateway made up, and takes one from this device's own root", async () => {
    const [desk, made] = [await generateDeviceKey(), await generateDeviceKey()];
    const reader = await fixtureDeviceKey();
    const devices = [await row("dev_desk", desk), await row(FIXTURE.deviceId, reader, { id: "dev_desk", key: desk }), await row("dev_made", made)];
    const store = memoryDeviceTrust();
    await confirmDeviceApprover(devices, { deviceId: FIXTURE.deviceId, ...reader, holder: reader }, (await deviceFingerprint(devices[0]!)).code, store);
    await expect((await keysWith(devices, await copyBy("dev_made", made), store)).at(1)).rejects.toSatisfy(untrusted);
    await expect((await keysWith(devices, await copyBy("dev_desk", desk), store)).at(1)).resolves.toMatchObject({ epoch: 0 });
  });

  it("trusts the confirmed approver and its children, with no automatic ancestor trust", async () => {
    const [root, a, c] = [await generateDeviceKey(), await generateDeviceKey(), await generateDeviceKey()];
    const reader = await fixtureDeviceKey();
    const devices = [
      await row("dev_root", root),
      await row("dev_a", a, { id: "dev_root", key: root }),
      await row(FIXTURE.deviceId, reader, { id: "dev_a", key: a }),
      await row("dev_c", c, { id: "dev_a", key: a }),
    ];
    const store = memoryDeviceTrust();
    await confirmDeviceApprover(devices, { deviceId: FIXTURE.deviceId, ...reader, holder: reader }, (await deviceFingerprint(devices[1]!)).code, store);
    await expect((await keysWith(devices, await copyBy("dev_root", root), store)).at(1)).rejects.toSatisfy(untrusted);
    await expect((await keysWith(devices, await copyBy("dev_c", c), store)).at(1)).resolves.toMatchObject({ epoch: 0 });
  });

  it("refuses every copy when this device's own approver is a recovery key", async () => {
    const [root, other] = [await generateDeviceKey(), await generateDeviceKey()];
    const reader = await fixtureDeviceKey();
    const code = fixtureDevice({ id: "dev_code", kind: "recovery_code", encryptionKey: b64u(other.encryptionKey), signingKey: null, approvedBy: "dev_root", approvalSignature: null });
    // Gateway names the recovery key as the approver, with a signature that verifies under another key.
    const self = { ...(await row(FIXTURE.deviceId, reader, { id: "dev_root", key: root })), approvedBy: "dev_code" };
    await expect((await keysWith([await row("dev_root", root), code, self], await copyBy("dev_root", root))).at(1)).rejects.toSatisfy(untrusted);
  });

  it("remembers a revocation: a device Gateway lists as trusted again stays refused", async () => {
    const [desk, laptop] = [await generateDeviceKey(), await generateDeviceKey()];
    const reader = await fixtureDeviceKey();
    const store = memoryDeviceTrust();
    const base = [await row("dev_desk", desk), await row(FIXTURE.deviceId, reader, { id: "dev_desk", key: desk })];
    await confirmDeviceApprover(base, { deviceId: FIXTURE.deviceId, ...reader, holder: reader }, (await deviceFingerprint(base[0]!)).code, store);
    const trusted = await row("dev_laptop", laptop, { id: "dev_desk", key: desk });
    const old = await copyBy("dev_laptop", laptop, "2026-11-01T09:05:00Z");
    await expect((await keysWith([...base, trusted], old, store)).at(1)).resolves.toMatchObject({ epoch: 0 });
    // Revoked: a copy made before the revocation still opens, one made after does not.
    const revoked = { ...trusted, state: "revoked" as const, revokedAt: "2026-11-01T10:00:00Z" };
    await expect((await keysWith([...base, revoked], old, store)).at(1)).resolves.toMatchObject({ epoch: 0 });
    await expect((await keysWith([...base, revoked], await copyBy("dev_laptop", laptop, "2026-11-01T10:05:00Z"), store)).at(1)).rejects.toSatisfy(untrusted);
    // Gateway lists it as trusted again: this device remembers.
    await expect((await keysWith([...base, trusted], old, store)).at(1)).rejects.toSatisfy(untrusted);
  });
});

describe("synced sessions: the timeline from stored events", () => {
  const open = (seq: number, event: Record<string, unknown>) => ({ state: "open" as const, seq, at: 1000 + seq, thread: THREAD, event: event as { kind: string } });

  it("reads the desktop's stored events into the shared timeline's events", () => {
    const timeline = syncedTimeline([
      open(0, { kind: "session", nexusSessionId: null, nativeId: "n", model: "claude-fable-5-1", via: "subscription", resumed: false }),
      open(1, { kind: "user", text: "Find the flake", images: [{ mediaType: "image/png", data: "iVBORw0KGgo=", name: "shot.png" }] }),
      open(2, { kind: "turnStart", turnId: "t1" }),
      open(3, { kind: "tool", id: "c1", name: "Bash", input: { command: "npm test" }, parent: null }),
      open(4, { kind: "toolResult", id: "c1", output: "1 failed", isError: true, truncated: false }),
      open(5, { kind: "text", id: "m1", text: "The retry test races the clock.", parent: null }),
      open(6, { kind: "turnEnd", turnId: "t1", status: "completed", costUsd: 0.42, durationMs: 9000, error: null }),
      { state: "no-key", seq: 7, at: 1007 },
      open(8, { kind: "commands", commands: ["review"] }),
      open(9, { kind: "somethingNew", text: "skipped" }),
    ]);
    expect(timeline.model).toBe("claude-fable-5-1");
    expect(timeline.images).toEqual([{ id: "sync_1_0", mediaType: "image/png", data: "iVBORw0KGgo=", name: "shot.png" }]);
    expect(timeline.events).toEqual([
      { kind: "user", text: "Find the flake", images: ["sync_1_0"], at: 1001 },
      { kind: "turnStart", turnId: "t1", at: 1002 },
      { kind: "tool", id: "c1", name: "Bash", input: { command: "npm test" }, at: 1003 },
      { kind: "toolResult", id: "c1", output: "1 failed", isError: true, truncated: false, at: 1004 },
      { kind: "text", id: "m1", text: "The retry test races the clock.", at: 1005 },
      { kind: "turnEnd", turnId: "t1", status: "completed", costUsd: 0.42, durationMs: 9000, at: 1006 },
    ]);
  });

  it("shows queued prompts as edited, leaves out those taken back and a linked session's messages", () => {
    // The queue names a message by its local number: the synced number plus one.
    const timeline = syncedTimeline([
      open(0, { kind: "user", text: "first" }),
      open(1, { kind: "user", text: "taken back" }),
      open(2, { kind: "user", text: "draft" }),
      open(3, { kind: "queueCancel", seq: 2 }),
      open(4, { kind: "queueEdit", seq: 3, text: "edited" }),
      open(5, { kind: "user", text: "from a linked session", from: { threadId: "x", title: "y" } }),
    ]);
    expect(timeline.events.map((event) => (event.kind === "user" ? event.text : event.kind))).toEqual(["first", "edited"]);
  });

  it("leaves out a field of the wrong type and an event missing what it needs", () => {
    const timeline = syncedTimeline([
      open(0, { kind: "text", id: 5, text: "kept, without its id" }),
      open(1, { kind: "tool", name: "Bash" }),
      open(2, { kind: "turnEnd", status: "weird" }),
      open(3, { kind: "usage", model: "m", inputTokens: "1" }),
    ]);
    expect(timeline.events).toEqual([{ kind: "text", text: "kept, without its id", at: 1000 }]);
  });
});

describe("synced sessions: a teammate's thread through the team key (ADR 0011)", () => {
  /** A teammate's thread at key version 1, locked to team-key epoch 1, which the fixture device holds. */
  async function teammate(signer?: Awaited<ReturnType<typeof generateDeviceKey>>) {
    const owner = await generateDeviceKey();
    const reader = await fixtureDeviceKey();
    const secret = crypto.getRandomValues(new Uint8Array(32));
    const team = await importHolderSecret(secret);
    const teamLocked = await lockTeamKey(secret, "ten_acme", 1, FIXTURE.deviceId, reader.encryptionKey);
    const locked = await lockContentKey(hex(FIXTURE.contentKey), FIXTURE.threadId, 0, "tmk_1", team.encryptionKey);
    const signature = await sign(signer ?? owner, teamThreadKeyMessage(FIXTURE.threadId, 0, "tmk_1", locked));
    const core = {
      whoami: async () => ({ kind: "person" as const, tenantId: "ten_acme" }),
      teamThreadKeys: async (threadId: string) => ({
        threadId,
        keys: [{ threadId, keyVersion: 1, teamKeyId: "tmk_1", personId: "usr_owner", lockedKey: b64u(locked), senderDeviceId: "dev_owner", signature: b64u(signature), createdAt: "" }],
      }),
      memberKeyHolders: async (personId: string) => ({
        personId,
        teamKeyId: "tmk_1",
        epoch: 1,
        holders: [{ id: "dev_owner", kind: "device" as const, encryptionKey: b64u(owner.encryptionKey), signingKey: b64u(owner.signingKey), approvedBy: null, approvalSignature: null, approvedAt: null, hasCurrentTeamKey: true }],
      }),
      teamKey: async (holderId: string) => ({
        key: null,
        copies: [{ teamKeyId: "tmk_1", epoch: 1, holderId, lockedKey: b64u(teamLocked), senderDeviceId: "dev_owner", signature: "", createdAt: "" }],
      }),
    };
    const holder = { deviceId: FIXTURE.deviceId, holder: reader };
    const memory = new Map<string, TeamTrustMemory>();
    const store: TeamTrustStore = { async observe(scope, person, root, revoked) {
      const key = JSON.stringify([scope, person]);
      const next = rememberTeamTrust(memory.get(key), root, revoked);
      memory.set(key, next);
      return next;
    } };
    return new ThreadKeys(FIXTURE.threadId, holder, [fixtureDevice()], async () => undefined, gatewayTeamKeys(core, FIXTURE.threadId, holder, store));
  }

  it("opens an event with the team key when this device holds no copy of its own", async () => {
    const event = await sealFixtureEvent(FIXTURE.threadId, 0, 1_700_000_000_000, THREAD, { kind: "user", text: "from a teammate" });
    expect(await openSyncedEvent(await teammate(), event)).toMatchObject({ state: "open", event: { text: "from a teammate" } });
  });

  it("waits for the key when the team copy's signer is not the thread's person's", async () => {
    const event = await sealFixtureEvent(FIXTURE.threadId, 0, 1_700_000_000_000, THREAD, { kind: "user", text: "forged" });
    expect((await openSyncedEvent(await teammate(await generateDeviceKey()), event)).state).toBe("no-key");
  });
});
