import { deviceFingerprint } from "./pairing";
import { describe, expect, it, vi } from "vitest";
import type { E2eDevice } from "./api";
import type { CodeAction, CodeControl } from "./code-control-types";
import { createCoreClient } from "./core-client";
import { confirmDeviceApprover, memoryDeviceTrust } from "./device-trust";
import { approvalMessage, b64u, type DeviceKey, fromB64u, generateDeviceKey, importDeviceSecret, openEvent, sign, unlockContentKey, verify } from "./e2e";
import { CODE_COMMAND_WAIT_MS, codeCommandCancelMessage, codeCommandMessage, codeControlMessage, controlAvailability, controlNow, controlWaitable, pendingCodeApprovals, recordClockOffset, sealCodeCommand, signCodeCommandCancel, trustedRunner } from "./remote-control";

const threadId = "0192f3a4-0000-7000-8000-000000000001";
const now = 1_793_959_200_000;
const control: CodeControl = { runnerDeviceId: "dev_runner", enabled: true, expiresAt: now + 30_000 };

async function fiveKeys(): Promise<[DeviceKey, DeviceKey, DeviceKey, DeviceKey, DeviceKey]> {
  return [await generateDeviceKey(), await generateDeviceKey(), await generateDeviceKey(), await generateDeviceKey(), await generateDeviceKey()];
}

/** A listed device, approved by `approver` over its keys, or a root with none. */
async function row(id: string, key: DeviceKey, approver?: { id: string; key: DeviceKey }): Promise<E2eDevice> {
  const signature = approver && (await sign(approver.key, approvalMessage(id, key.encryptionKey, key.signingKey)));
  return {
    id, name: id, kind: "device", state: "trusted", encryptionKey: b64u(key.encryptionKey), signingKey: b64u(key.signingKey),
    approvedBy: approver?.id ?? null, approvalSignature: signature ? b64u(signature) : null,
    createdAt: "2026-11-01T00:00:00Z", approvedAt: approver ? "2026-11-01T00:01:00Z" : null, revokedAt: null,
  };
}

async function paired() {
  const phone = await importDeviceSecret(Uint8Array.from({ length: 64 }, (_, i) => i));
  const runner = await importDeviceSecret(Uint8Array.from({ length: 64 }, (_, i) => i + 64));
  const base = { kind: "device", state: "trusted", createdAt: "2026-11-01T00:00:00Z", approvedAt: "2026-11-01T00:01:00Z", revokedAt: null } satisfies Partial<E2eDevice>;
  const devices: E2eDevice[] = [
    { ...base, id: "dev_phone", name: "Phone", encryptionKey: b64u(phone.encryptionKey), signingKey: b64u(phone.signingKey), approvedBy: null, approvalSignature: null },
    { ...base, id: "dev_runner", name: "Computer", encryptionKey: b64u(runner.encryptionKey), signingKey: b64u(runner.signingKey), approvedBy: "dev_phone", approvalSignature: b64u(await sign(phone, approvalMessage("dev_runner", runner.encryptionKey, runner.signingKey))) },
  ];
  return { phone, runner, devices, reader: { deviceId: "dev_phone", holder: phone } };
}

describe("encrypted Code commands", () => {
  it("delivers a signed command only the approved runner can decrypt, with the URL's thread inside", async () => {
    const { phone, runner, devices, reader } = await paired();
    const action: CodeAction = { kind: "send", text: "Fix the retry race in decoder.py" };
    const envelope = await sealCodeCommand({ threadId, action, reader, devices, control, now });
    expect(envelope.expiresAt).toBe(now + 60_000);
    expect(envelope.id).toMatch(/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/);
    expect(JSON.stringify(envelope)).not.toContain(action.text);
    const signature = fromB64u(envelope.signature)!;
    expect(await verify(phone.signingKey, codeCommandMessage(threadId, envelope), signature)).toBe(true);
    expect(await verify(phone.signingKey, codeCommandMessage("another-thread", envelope), signature)).toBe(false);
    expect(await verify(phone.signingKey, codeCommandMessage(threadId, { ...envelope, expiresAt: envelope.expiresAt + 1 }), signature)).toBe(false);
    const locked = fromB64u(envelope.lockedKey)!;
    const key = await unlockContentKey(runner, envelope.id, 0, "dev_runner", locked);
    const bytes = fromB64u(envelope.sealed)!;
    expect(JSON.parse(new TextDecoder().decode(await openEvent(key, envelope.id, 0, bytes)))).toEqual({ version: 1, threadId, action });
    await expect(unlockContentKey(phone, envelope.id, 0, "dev_runner", locked)).rejects.toThrow();
    await expect(openEvent(key, "another-command", 0, bytes)).rejects.toThrow();
    const next = await sealCodeCommand({ threadId, action, reader, devices, control, now });
    expect(next.id).not.toBe(envelope.id);
    expect(next.lockedKey).not.toBe(envelope.lockedKey);
    expect(next.sealed).not.toBe(envelope.sealed);
  });

  it("refuses unpaired or revoked devices, forged approvals and unrelated roots", async () => {
    const { devices, reader } = await paired();
    const seal = (changed: E2eDevice[]) => sealCodeCommand({ threadId, action: { kind: "stop" }, reader, devices: changed, control, now });
    await expect(seal([])).rejects.toThrow(/Pair/);
    for (const id of ["dev_phone", "dev_runner"]) {
      await expect(seal(devices.map((device) => device.id === id ? { ...device, state: "pending" } : device))).rejects.toThrow();
      await expect(seal(devices.map((device) => device.id === id ? { ...device, state: "revoked", revokedAt: "2026-11-02T00:00:00Z" } : device))).rejects.toThrow();
    }
    await expect(seal(devices.map((device) => device.id === "dev_runner" ? { ...device, approvalSignature: b64u(new Uint8Array(64)) } : device))).rejects.toThrow(/trusted/);
    await expect(seal(devices.map((device) => device.id === "dev_runner" ? { ...device, approvedBy: null, approvalSignature: null } : device))).rejects.toThrow(/trusted/);
  });

  it("refuses a runner that a device Gateway named as this phone's approver approved (S3)", async () => {
    const [desk, phone, runner, forger, fake] = await fiveKeys();
    const reader = { deviceId: "dev_phone", holder: phone };
    const store = memoryDeviceTrust();
    const honest = [await row("dev_desk", desk), await row("dev_phone", phone, { id: "dev_desk", key: desk }), await row("dev_runner", runner, { id: "dev_desk", key: desk })];
    await expect(trustedRunner(honest, reader, "dev_runner", store)).rejects.toThrow(/trusted/);
    await confirmDeviceApprover(honest, { deviceId: reader.deviceId, encryptionKey: phone.encryptionKey, signingKey: phone.signingKey, holder: phone }, (await deviceFingerprint(honest[0]!)).code, store);
    expect((await trustedRunner(honest, reader, "dev_runner", store)).id).toBe("dev_runner");
    // Gateway holds F's key: F's signature over the phone's keys verifies, and F approves a runner of Gateway's own.
    const rewritten = [
      honest[0]!,
      await row("dev_phone", phone, { id: "dev_f", key: forger }),
      honest[2]!,
      await row("dev_f", forger),
      await row("dev_fake", fake, { id: "dev_f", key: forger }),
    ];
    await expect(trustedRunner(rewritten, reader, "dev_fake", store)).rejects.toThrow(/trusted/);
    await expect(trustedRunner(rewritten, reader, "dev_f", store)).rejects.toThrow(/trusted/);
    expect((await trustedRunner(rewritten, reader, "dev_runner", store)).id).toBe("dev_runner");
  });

  it("trusts an honest chain, root to A to this phone, and refuses a runner a recovery key approved", async () => {
    const [root, a, phone, runner, other] = await fiveKeys();
    const reader = { deviceId: "dev_phone", holder: phone };
    const code: E2eDevice = { ...(await row("dev_code", other, { id: "dev_root", key: root })), kind: "recovery_code", signingKey: null };
    const devices = [
      await row("dev_root", root),
      await row("dev_a", a, { id: "dev_root", key: root }),
      await row("dev_phone", phone, { id: "dev_a", key: a }),
      await row("dev_runner", runner, { id: "dev_a", key: a }),
      code,
      { ...(await row("dev_by_code", other, { id: "dev_root", key: root })), approvedBy: "dev_code" },
    ];
    const store = memoryDeviceTrust();
    await confirmDeviceApprover(devices, { deviceId: reader.deviceId, encryptionKey: phone.encryptionKey, signingKey: phone.signingKey, holder: phone }, (await deviceFingerprint(devices[1]!)).code, store);
    expect((await trustedRunner(devices, reader, "dev_runner", store)).id).toBe("dev_runner");
    await expect(trustedRunner(devices, reader, "dev_by_code", store)).rejects.toThrow(/trusted/);
  });

  it("accepts a runner Gateway lists as trusted when trust is gateway, and refuses it in approvals mode", async () => {
    const [phone, runner, other] = await fiveKeys();
    const vouched = async (id: string, key: DeviceKey) => ({ ...(await row(id, key)), approvedAt: "2026-11-01T00:01:00Z" });
    const devices = [await vouched("dev_desk", other), await vouched("dev_phone", phone), await vouched("dev_runner", runner)];
    const reader = { deviceId: "dev_phone", holder: phone };
    expect((await trustedRunner(devices, { ...reader, trust: "gateway" }, "dev_runner", memoryDeviceTrust())).id).toBe("dev_runner");
    await expect(trustedRunner(devices, reader, "dev_runner", memoryDeviceTrust())).rejects.toThrow(/trusted/);
    const envelope = await sealCodeCommand({ threadId, action: { kind: "stop" }, reader: { ...reader, trust: "gateway" }, devices, control, now });
    expect(envelope.runnerDeviceId).toBe("dev_runner");
    // A recovery key is never a runner, and a runner this phone saw revoked stays refused.
    const code: E2eDevice = { ...(await vouched("dev_code", other)), kind: "recovery_code", signingKey: null };
    await expect(trustedRunner([...devices, code], { ...reader, trust: "gateway" }, "dev_code", memoryDeviceTrust())).rejects.toThrow(/trusted/);
    const store = memoryDeviceTrust();
    await expect(trustedRunner(devices.map((device) => device.id === "dev_runner" ? { ...device, state: "revoked" as const, revokedAt: "2026-11-02T00:00:00Z" } : device), { ...reader, trust: "gateway" }, "dev_runner", store)).rejects.toThrow(/trusted/);
    await expect(trustedRunner(devices, { ...reader, trust: "gateway" }, "dev_runner", store)).rejects.toThrow(/trusted/);
  });

  it("refuses a runner this phone saw revoked, when Gateway lists it trusted again", async () => {
    const { devices, reader } = await paired();
    const store = memoryDeviceTrust();
    expect((await trustedRunner(devices, reader, "dev_runner", store)).id).toBe("dev_runner");
    const revoked = devices.map((device) => (device.id === "dev_runner" ? { ...device, state: "revoked" as const, revokedAt: "2026-11-02T00:00:00Z" } : device));
    await expect(trustedRunner(revoked, reader, "dev_runner", store)).rejects.toThrow(/trusted/);
    await expect(trustedRunner(devices, reader, "dev_runner", store)).rejects.toThrow(/trusted/);
  });

  it("keeps typed answers encrypted and refuses offline, disabled, empty or oversized commands", async () => {
    const { runner, devices, reader } = await paired();
    const action: CodeAction = { kind: "approve", requestId: "req_1", decision: "allow", message: null, answers: { target: ["staging"], "Which branch?": "main" } };
    const envelope = await sealCodeCommand({ threadId, action, reader, devices, control, now });
    const key = await unlockContentKey(runner, envelope.id, 0, "dev_runner", fromB64u(envelope.lockedKey)!);
    expect(JSON.parse(new TextDecoder().decode(await openEvent(key, envelope.id, 0, fromB64u(envelope.sealed)!)))).toEqual({ version: 1, threadId, action });
    for (const unavailable of [{ ...control, enabled: false }, { ...control, expiresAt: now }, { ...control, runnerDeviceId: null }]) {
      await expect(sealCodeCommand({ threadId, action, reader, devices, control: unavailable, now })).rejects.toThrow(/offline/);
    }
    for (const text of ["   ", "x".repeat(65_536)]) await expect(sealCodeCommand({ threadId, action: { kind: "send", text }, reader, devices, control, now })).rejects.toThrow();
  });

  it("lets only a message wait a day for a sleeping computer, signed as v2 with its sending time", async () => {
    const { phone, runner, devices, reader } = await paired();
    const asleep: CodeControl = { ...control, expiresAt: now - 1 };
    expect(controlWaitable(asleep, now)).toBe(true);
    expect(controlWaitable({ ...asleep, enabled: false }, now)).toBe(false);
    expect(controlWaitable(control, now)).toBe(false);
    const action: CodeAction = { kind: "send", text: "Run the slow suite" };
    await expect(sealCodeCommand({ threadId, action, reader, devices, control: asleep, now })).rejects.toThrow(/offline/);
    const envelope = await sealCodeCommand({ threadId, action, reader, devices, control: asleep, now, waits: true });
    expect(envelope).toMatchObject({ waits: true, expiresAt: now + CODE_COMMAND_WAIT_MS });
    const signature = fromB64u(envelope.signature)!;
    expect(codeCommandMessage(threadId, envelope).split("\n")[0]).toBe("nexus-code-command-v2");
    expect(await verify(phone.signingKey, codeCommandMessage(threadId, envelope), signature)).toBe(true);
    expect(await verify(phone.signingKey, codeCommandMessage(threadId, { ...envelope, waits: false }), signature)).toBe(false);
    const key = await unlockContentKey(runner, envelope.id, 0, "dev_runner", fromB64u(envelope.lockedKey)!);
    expect(JSON.parse(new TextDecoder().decode(await openEvent(key, envelope.id, 0, fromB64u(envelope.sealed)!)))).toEqual({ version: 2, threadId, sentAt: now, action });
    for (const late of [{ kind: "stop" } as const, { kind: "approve", requestId: "req_1", decision: "allow", message: null, answers: null } as const]) {
      await expect(sealCodeCommand({ threadId, action: late, reader, devices, control: asleep, now, waits: true })).rejects.toThrow(/Only a message/);
    }
    await expect(sealCodeCommand({ threadId, action, reader, devices, control: { ...asleep, enabled: false }, now, waits: true })).rejects.toThrow(/offline/);
    const cancel = await signCodeCommandCancel(threadId, envelope.id, reader);
    expect(cancel.senderDeviceId).toBe("dev_phone");
    expect(await verify(phone.signingKey, codeCommandCancelMessage(threadId, envelope.id, "dev_phone"), fromB64u(cancel.signature)!)).toBe(true);
    expect(codeCommandCancelMessage("t", "c", "p")).toBe("nexus-code-command-cancel-v1\nt\nc\np");
  });

  it("uses the versioned newline signature contract without serializing an action", () => {
    expect(codeControlMessage("thread", { runnerDeviceId: "runner", enabled: true, expiresAt: 42 })).toBe("nexus-code-control-v1\nthread\nrunner\ntrue\n42");
    expect(codeCommandMessage("thread", { id: "command", senderDeviceId: "phone", runnerDeviceId: "runner", expiresAt: 42, lockedKey: "locked", sealed: "sealed" })).toBe("nexus-code-command-v1\nthread\ncommand\nphone\nrunner\n42\nlocked\nsealed");
    expect(controlAvailability(undefined, now)).toBe("loading");
    expect(controlAvailability({ runnerDeviceId: null, enabled: false, expiresAt: null }, now)).toBe("disabled");
    expect(controlAvailability({ ...control, expiresAt: now }, now)).toBe("offline");
  });

  it("seals by Gateway's clock when this phone's clock runs ahead", async () => {
    const { devices, reader } = await paired();
    const ahead = now + 120_000;
    const answered = recordClockOffset({ ...control, serverTime: now }, ahead);
    expect(answered).toEqual({ ...control, serverTime: now });
    expect(controlNow(answered, ahead + 5_000)).toBe(now + 5_000);
    // A copy, or an older Gateway's answer, has no offset: this phone's own clock.
    expect(controlNow({ ...answered }, ahead)).toBe(ahead);
    expect(controlNow(recordClockOffset(control, ahead), ahead)).toBe(ahead);
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(ahead + 1_000);
      // By this phone's clock the lease ended; by Gateway's it has 29 seconds left.
      expect(controlAvailability(control)).toBe("offline");
      expect(controlAvailability(answered)).toBe("ready");
      const envelope = await sealCodeCommand({ threadId, action: { kind: "stop" }, reader, devices, control: answered });
      expect(envelope.expiresAt).toBe(now + 1_000 + 60_000);
    } finally {
      vi.useRealTimers();
    }
  });

  it("records Gateway's clock offset on a control answer, and reads an older Gateway's answer by this phone's clock", async () => {
    const client = (body: object) => createCoreClient({
      baseUrl: "http://gateway.invalid",
      fetch: async () => new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } }),
    });
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(now + 120_000);
      const current = await client({ ...control, serverTime: now }).codeControl(threadId);
      expect(current).toEqual({ ...control, serverTime: now });
      expect(controlNow(current)).toBe(now);
      const older = await client(control).codeControl(threadId);
      expect(older).toEqual(control);
      expect(controlNow(older)).toBe(now + 120_000);
    } finally {
      vi.useRealTimers();
    }
  });

  it("offers only unresolved approvals from the latest turn, including a sub-agent's request", () => {
    const old = { kind: "approval", requestId: "old", tool: "Bash", input: { command: "pwd" } } as const;
    const first = { kind: "approval", requestId: "first", tool: "Bash", input: { command: "ls" } } as const;
    const question = { kind: "approval", requestId: "question", tool: "AskUserQuestion", parent: "task_1", input: { questions: [{ question: "Where?" }] } } as const;
    expect(pendingCodeApprovals([old, { kind: "turnEnd", status: "completed" }, { kind: "turnStart" }, first, question, { kind: "user", text: "A message queued while you wait" }, { kind: "approvalResolved", requestId: "first", decision: "deny" }])).toEqual([question]);
    expect(pendingCodeApprovals([question, { kind: "turnEnd", status: "interrupted" }])).toEqual([]);
  });
});
