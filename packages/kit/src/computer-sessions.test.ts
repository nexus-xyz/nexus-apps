import { confirmDeviceApprover, memoryDeviceTrust, setDeviceTrustStore } from "./device-trust";
import { deviceFingerprint } from "./pairing";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { E2eDevice } from "./api";
import { approvalMessage, b64u, importDeviceSecret, openEvent, sign, unlockContentKey, verify } from "./e2e";
import { computerPresenceMessage, computerReplyMessage, computerRequestHash, computerRequestMessage, openComputerReply, parseComputerPresence, parseComputerReplyEnvelope, parseComputerRequestEnvelope, readComputerModelSettings, readComputerResult, readComputerSelection, sealComputerRequest, verifyComputerPresence } from "./computer-sessions";
import { binary, integer, object, sealObject, text } from "./computer-protocol";
import type { ComputerOperation, ComputerPresence, ComputerSessionSettings } from "./computer-types";
import { parsePhoneInvitation, phoneInvitationLink, phonePairingMessage, sealPhonePairing, verifyPhoneInvitation } from "./phone-link";

const fixture = object(JSON.parse(readFileSync(new URL("./fixtures/computer-v1.json", import.meta.url), "utf8")));
const presence = parseComputerPresence(fixture.presence);
const request = parseComputerRequestEnvelope(fixture.request);
const reply = parseComputerReplyEnvelope(fixture.reply);
const invitation = parsePhoneInvitation(text(fixture.invitationLink, 16 * 1024));
const now = integer(fixture.now);

async function paired() {
  const phone = await importDeviceSecret(binary(fixture.phoneSecret, 64));
  const runner = await importDeviceSecret(binary(fixture.runnerSecret, 64));
  const base = { kind: "device", state: "trusted", createdAt: "2026-11-01T00:00:00Z", approvedAt: "2026-11-01T00:01:00Z", revokedAt: null } satisfies Partial<E2eDevice>;
  const devices: E2eDevice[] = [
    { ...base, id: "dev_runner", name: "Computer", encryptionKey: b64u(runner.encryptionKey), signingKey: b64u(runner.signingKey), approvedBy: null, approvalSignature: null },
    { ...base, id: "dev_phone", name: "Phone", encryptionKey: b64u(phone.encryptionKey), signingKey: b64u(phone.signingKey), approvedBy: "dev_runner", approvalSignature: b64u(await sign(runner, approvalMessage("dev_phone", phone.encryptionKey, phone.signingKey))) },
  ];
  setDeviceTrustStore(memoryDeviceTrust());
  await confirmDeviceApprover(devices, { deviceId: "dev_phone", ...phone, holder: phone }, (await deviceFingerprint(devices[0]!)).code);
  return { phone, runner, devices, sender: { deviceId: "dev_phone", holder: phone } };
}

describe("computer request and reply interoperability", () => {
  it("opens the persisted cross-runtime vector and verifies its exact signed request hash", async () => {
    const { phone, runner, devices } = await paired();
    expect(await computerRequestHash(request)).toBe(fixture.requestHash);
    expect(await verify(phone.signingKey, computerRequestMessage(request), binary(request.signature, 64))).toBe(true);
    const key = await unlockContentKey(runner, request.id, 0, request.runnerDeviceId, binary(request.lockedKey));
    const payload = object(JSON.parse(new TextDecoder().decode(await openEvent(key, request.id, 0, binary(request.sealed)))));
    expect(payload).toMatchObject({ kind: "computerRequest", scope: presence.scope, operation: { kind: "startSession", projectId: "p_public_fixture" } });
    expect(JSON.stringify(request)).not.toContain("PHONE_START_READY");
    expect(await openComputerReply({ request, reply, holder: phone, devices })).toMatchObject({ kind: "created", threadId: "0192f3a4-0000-7000-8000-000000000003", origin: { name: presence.name }, launch: "running" });
  });

  it("does not accept a valid runner signature for another request or account", async () => {
    const { phone, runner, devices } = await paired();
    const swapped = { ...reply, requestHash: b64u(new Uint8Array(32)) };
    swapped.signature = b64u(await sign(runner, computerReplyMessage(swapped)));
    await expect(openComputerReply({ request, reply: swapped, holder: phone, devices })).rejects.toThrow();
    const otherOrg = { ...reply, scope: { ...reply.scope, tenantId: "ten_other" } };
    otherOrg.signature = b64u(await sign(runner, computerReplyMessage(otherOrg)));
    await expect(openComputerReply({ request, reply: otherOrg, holder: phone, devices })).rejects.toThrow();
  });

  it("verifies a computer Gateway vouches for when trust is gateway, with no approval", async () => {
    const { sender, devices } = await paired();
    setDeviceTrustStore(memoryDeviceTrust());
    const vouched = devices.map((device) => ({ ...device, approvedBy: null, approvalSignature: null }));
    await expect(verifyComputerPresence({ presence, scope: presence.scope, reader: sender, devices: vouched })).rejects.toThrow("trusted");
    expect(await verifyComputerPresence({ presence, scope: presence.scope, reader: { ...sender, trust: "gateway" }, devices: vouched })).toEqual(presence);
    await expect(openComputerReply({ request, reply, holder: sender.holder, devices: vouched })).rejects.toThrow("trusted");
    expect(await openComputerReply({ request, reply, holder: sender.holder, devices: vouched, trust: "gateway" })).toMatchObject({ kind: "created" });
  });

  it("requires the anchored live computer before sealing a new operation", async () => {
    const { sender, devices, runner } = await paired();
    expect(await verifyComputerPresence({ presence, scope: presence.scope, reader: sender, devices })).toEqual(presence);
    const disabled = { ...presence, enabled: false };
    disabled.signature = b64u(await sign(runner, computerPresenceMessage(disabled)));
    await expect(sealComputerRequest({ scope: presence.scope, computer: disabled, operation: { kind: "describeProjects" }, sender, devices, now })).rejects.toThrow("turned off");
    await expect(sealComputerRequest({ scope: presence.scope, computer: presence, operation: { kind: "describeProjects" }, sender, devices, now: presence.expiresAt + 1 })).rejects.toThrow("offline");
    const revoked = devices.map((device) => device.id === "dev_runner" ? { ...device, state: "revoked" as const } : device);
    await expect(sealComputerRequest({ scope: presence.scope, computer: presence, operation: { kind: "describeProjects" }, sender, devices: revoked, now })).rejects.toThrow("trusted");
  });

  it("keeps start admission short and the requested project inside recipient-only ciphertext", async () => {
    const { sender, devices, runner } = await paired();
    const sealed = await sealComputerRequest({ scope: presence.scope, computer: presence,
      operation: { kind: "startSession", projectId: "p_secret_project", revision: "r1", text: "Fix private-project scheduling" }, sender, devices, now });
    expect(sealed.expiresAt).toBe(now + 15_000);
    expect(JSON.stringify(sealed)).not.toContain("private-project");
    expect(JSON.stringify(sealed)).not.toContain("p_secret_project");
    const key = await unlockContentKey(runner, sealed.id, 0, sealed.runnerDeviceId, binary(sealed.lockedKey));
    const plaintext = JSON.parse(new TextDecoder().decode(await openEvent(key, sealed.id, 0, binary(sealed.sealed))));
    expect(plaintext.operation.projectId).toBe("p_secret_project");
    await expect(unlockContentKey(sender.holder, sealed.id, 0, sealed.runnerDeviceId, binary(sealed.lockedKey))).rejects.toThrow();
  });

  it("keeps worktree branches in the signed and encrypted project request", async () => {
    const { sender, devices, runner } = await paired();
    const operations: ComputerOperation[] = [
      { kind: "describeBranches", projectId: "p_secret_project", revision: "r1" },
      { kind: "startSession", projectId: "p_secret_project", revision: "r1", text: "Continue", baseBranch: "origin/private-branch" },
    ];
    for (const operation of operations) {
      const sealed = await sealComputerRequest({ scope: presence.scope, computer: presence, operation, sender, devices, now });
      expect(JSON.stringify(sealed)).not.toContain("p_secret_project");
      expect(JSON.stringify(sealed)).not.toContain("private-branch");
      const key = await unlockContentKey(runner, sealed.id, 0, sealed.runnerDeviceId, binary(sealed.lockedKey));
      const payload = JSON.parse(new TextDecoder().decode(await openEvent(key, sealed.id, 0, binary(sealed.sealed))));
      expect(payload.operation).toEqual(operation);
    }
    expect(() => readComputerResult({ kind: "branches", projectId: "p1", revision: "r1", branches: ["--force"] })).toThrow("valid branch");
  });

  it("requires advertised settings support before encrypting a model or explicit payer choice", async () => {
    const { sender, runner, devices } = await paired();
    const operations: ComputerOperation[] = [
      { kind: "describeSession", threadId: request.id },
      { kind: "compactSession", threadId: request.id },
      { kind: "describeHandoff", threadId: request.id },
      { kind: "startSession", projectId: "private-project", revision: "r1", text: "Reviewed private context", handoff: { sourceThreadId: request.id, agent: "codex" }, selection: { via: "nexus", payer: "gateway", model: null, effort: null } },
      { kind: "updateSession", threadId: request.id, revision: "r1", settings: { model: "private-model", effort: "high" } },
      { kind: "startSession", projectId: "private-project", revision: "r1", text: "Hello", selection: { via: "subscription", payer: "plan", model: "private-model", effort: "high" } },
    ];
    const current: ComputerPresence = { ...presence, capabilities: ["projects", "start", "settings"] };
    current.signature = b64u(await sign(runner, computerPresenceMessage(current)));
    for (const operation of operations) {
      await expect(sealComputerRequest({ scope: presence.scope, computer: presence, operation, sender, devices, now })).rejects.toThrow("Update Nexus");
      const sealed = await sealComputerRequest({ scope: presence.scope, computer: current, operation, sender, devices, now });
      expect(JSON.stringify(sealed)).not.toContain("private-model");
      const key = await unlockContentKey(runner, sealed.id, 0, sealed.runnerDeviceId, binary(sealed.lockedKey));
      const opened = JSON.parse(new TextDecoder().decode(await openEvent(key, sealed.id, 0, binary(sealed.sealed))));
      expect(opened.operation).toEqual(operation);
      expect(sealed.expiresAt).toBe(now + (operation.kind === "startSession" ? 15_000 : 30_000));
    }
  });

  it("authenticates effective settings separately from a requested change and preserves the actual API-key payer", async () => {
    const { phone, runner, sender, devices } = await paired();
    const computer: ComputerPresence = { ...presence, capabilities: ["projects", "start", "settings"] };
    computer.signature = b64u(await sign(runner, computerPresenceMessage(computer)));
    const sent = await sealComputerRequest({ scope: presence.scope, computer, operation: { kind: "describeSession", threadId: request.id }, sender, devices, now });
    const session: ComputerSessionSettings = { threadId: request.id, revision: "current", agent: "claude", via: "subscription", model: "effective-model", effort: "medium",
      payer: { kind: "apiKey", label: "Provider API key on the computer", detail: null }, pending: { model: "requested-model", effort: null },
      models: [{ id: "requested-model", name: "Requested model", efforts: ["medium", "high"] }], editable: true, reason: null };
    const routing = { version: 1 as const, scope: presence.scope, computerId: computer.computerId, requestId: sent.id, requestHash: await computerRequestHash(sent), runnerDeviceId: sent.runnerDeviceId, recipientDeviceId: sent.senderDeviceId };
    const box = await sealObject(sent.id, sent.senderDeviceId, phone.encryptionKey, { ...routing, kind: "computerReply", result: { kind: "sessionSettings", session } }, 256 * 1024);
    const unsigned = { ...routing, ...box };
    const result = await openComputerReply({ request: sent, reply: { ...unsigned, signature: b64u(await sign(runner, computerReplyMessage(unsigned))) }, holder: phone, devices });
    expect(result).toEqual({ kind: "sessionSettings", session });
    expect(() => readComputerResult({ kind: "sessionSettings", session: { ...session, via: "nexus" } })).toThrow();
  });

  it("decodes a bounded Desktop patch and refuses oversized replies", () => {
    const result = { kind: "sessionChanges", threadId: request.id, files: ["changes with spaces.txt"], patch: "diff --git a/changes.txt b/changes.txt\n+Recorded", truncated: false };
    expect(readComputerResult(result)).toEqual(result);
    expect(() => readComputerResult({ ...result, patch: "x".repeat(24 * 1024 + 1) })).toThrow();
    expect(() => readComputerResult({ ...result, files: Array(33).fill("file") })).toThrow();
  });

  it("keeps legacy project replies valid and rejects ambiguous route/model choices", () => {
    expect(readComputerResult({ kind: "projects", projects: [] })).toEqual({ kind: "projects", projects: [] });
    const route = { via: "subscription", payer: "plan", label: "Claude subscription", detail: null, state: "usable", defaultModel: null, models: [{ id: "one", name: "One", efforts: ["high"] }] };
    const choices = { kind: "projects", projects: [], agents: [{ agent: "claude", name: "Claude Code", routes: [route] }] };
    expect(readComputerResult(choices)).toEqual(choices);
    expect(() => readComputerResult({ ...choices, agents: [{ ...choices.agents[0], routes: [route, route] }] })).toThrow();
    expect(() => readComputerResult({ ...choices, agents: [{ ...choices.agents[0], routes: [{ ...route, models: [route.models[0], route.models[0]] }] }] })).toThrow();
  });

  it("leaves out agents and effort levels from a newer computer and keeps the rest", () => {
    const route = { via: "subscription", payer: "plan", label: "Claude subscription", detail: null, state: "usable", defaultModel: null, models: [{ id: "one", name: "One", efforts: ["future-level", "high"] }] };
    const claude = { agent: "claude", name: "Claude Code", routes: [route] };
    const future = { agent: "future-agent", name: "Future", routes: [{ ...route, via: "future-route" }] };
    const project = { id: "p1", name: "Project", locationHint: "~/p1", revision: "r1", settings: { agent: "future-agent", model: null, via: "future-route", permissionMode: "default", effort: "future-level", workspace: "folder" } };
    const read = readComputerResult({ kind: "projects", projects: [project], agents: [future, claude, future, future, future] });
    expect(read).toEqual({ kind: "projects", projects: [project], agents: [{ ...claude, routes: [{ ...route, models: [{ id: "one", name: "One", efforts: ["high"] }] }] }] });
    const choices = (agents: unknown[]) => ({ kind: "choicesChanged", projects: [], agents });
    expect(readComputerResult(choices(Array.from({ length: 32 }, () => future)))).toEqual({ kind: "choicesChanged", projects: [], agents: [] });
    expect(() => readComputerResult(choices(Array.from({ length: 33 }, () => future)))).toThrow();
    expect(() => readComputerResult(choices([claude, future, claude]))).toThrow();
    expect(() => readComputerResult(choices([{ ...claude, agent: 3 }]))).toThrow();
    expect(() => readComputerResult(choices([{ ...claude, agent: "" }]))).toThrow();
    expect(() => readComputerResult(choices(["claude"]))).toThrow();
    const withEfforts = (efforts: unknown[]) => choices([{ ...claude, routes: [{ ...route, models: [{ id: "one", name: "One", efforts }] }] }]);
    expect(() => readComputerResult(withEfforts(["high", "future-level", "high"]))).toThrow();
    expect(() => readComputerResult(withEfforts(["high", 3]))).toThrow();
    expect(() => readComputerResult(withEfforts([null]))).toThrow();
    expect(() => readComputerResult(withEfforts(Array.from({ length: 33 }, (_, n) => `level-${n}`)))).toThrow();
  });

  it("leaves out routes from a newer computer and keeps the agent's other routes", () => {
    const route = { via: "subscription", payer: "plan", label: "Claude subscription", detail: null, state: "usable", defaultModel: "one", models: [{ id: "one", name: "One", efforts: ["high"] }] };
    const ori = { ...route, via: "ori", label: "OpenRouter", defaultModel: "elsewhere" };
    const choices = (routes: unknown[]) => ({ kind: "projects", projects: [], agents: [{ agent: "claude", name: "Claude Code", routes }] });
    expect(readComputerResult(choices([ori, route, { ...route, payer: "future-payer" }, { ...route, state: "future-state" }]))).toEqual(choices([route]));
    expect(readComputerResult(choices(Array.from({ length: 32 }, () => ori)))).toEqual(choices([]));
    expect(() => readComputerResult(choices(Array.from({ length: 33 }, () => ori)))).toThrow();
    expect(() => readComputerResult(choices([route, ori, route]))).toThrow();
    expect(() => readComputerResult(choices([{ ...route, via: "nexus" }]))).toThrow();
    expect(() => readComputerResult(choices([{ ...route, via: 3 }]))).toThrow();
    expect(() => readComputerResult(choices([{ ...route, state: null }]))).toThrow();
  });

  it("reads session of agent this phone does not know as read-only and unknown levels as no level", () => {
    const session = { threadId: request.id, revision: "current", agent: "future-agent", via: "subscription", model: "future-model", effort: "future-level",
      payer: { kind: "plan", label: "Future plan", detail: null }, pending: { model: "next-model", effort: "future-level" },
      models: [{ id: "next-model", name: "Next", efforts: ["future-level", "medium"] }], editable: true, reason: null };
    expect(readComputerResult({ kind: "sessionChanged", session })).toEqual({ kind: "sessionChanged", session: { ...session, agent: "unknown", effort: null, effortUnknown: true,
      pending: { model: "next-model", effortUnknown: true }, models: [{ id: "next-model", name: "Next", efforts: ["medium"] }], editable: false,
      reason: "Update Nexus on this phone to change this session's settings." } });
    expect(readComputerResult({ kind: "sessionSettings", session: { ...session, reason: "Busy" } })).toMatchObject({ session: { editable: false, reason: "Update Nexus on this phone to change this session's settings." } });
    expect(readComputerResult({ kind: "sessionSettings", session: { ...session, agent: "codex" } })).toMatchObject({ session: { agent: "codex", effort: null, effortUnknown: true, editable: true, reason: null } });
    const known = readComputerResult({ kind: "sessionSettings", session: { ...session, agent: "codex", effort: "medium", pending: { effort: null } } });
    expect(known).toMatchObject({ session: { effort: "medium", pending: { effort: null } } });
    expect(known.kind === "sessionSettings" && "effortUnknown" in known.session).toBe(false);
    expect(known).toEqual({ kind: "sessionSettings", session: expect.objectContaining({ pending: { effort: null } }) });
    expect(readComputerResult({ kind: "sessionSettings", session: { ...session, agent: "codex", effort: "medium", pending: { model: "next-model" } } }))
      .toEqual({ kind: "sessionSettings", session: expect.objectContaining({ pending: { model: "next-model" } }) });
    for (const broken of [{ agent: 3 }, { agent: "" }, { editable: "yes" }, { effort: 3 }, { pending: { effort: 3 } }, { via: "nexus" }, { models: [{ id: "next-model", name: "Next", efforts: ["medium", "medium"] }] }]) {
      expect(() => readComputerResult({ kind: "sessionSettings", session: { ...session, ...broken } })).toThrow();
    }
  });

  it("reads session on route this phone does not know as read-only", () => {
    const session = { threadId: request.id, revision: "current", agent: "claude", via: "ori", model: "one", effort: "high",
      payer: { kind: "gateway", label: "OpenRouter", detail: null }, pending: null, models: [{ id: "one", name: "One", efforts: ["high"] }], editable: true, reason: "Busy" };
    expect(readComputerResult({ kind: "sessionSettings", session })).toEqual({ kind: "sessionSettings", session: { ...session, via: "unknown", editable: false,
      reason: "Update Nexus on this phone to change this session's settings." } });
    expect(readComputerResult({ kind: "sessionSettings", session: { ...session, payer: { kind: "plan", label: "Plan", detail: null } } })).toMatchObject({ session: { via: "unknown", editable: false } });
    for (const broken of [{ via: 3 }, { via: "" }, { via: "subscription" }]) {
      expect(() => readComputerResult({ kind: "sessionSettings", session: { ...session, ...broken } })).toThrow();
    }
    expect(() => readComputerSelection({ via: "ori", payer: "gateway", model: "one", effort: "high" })).toThrow();
  });

  it("still refuses unknown effort level in a choice this phone sends or saves", () => {
    expect(() => readComputerSelection({ via: "subscription", payer: "plan", model: "one", effort: "future-level" })).toThrow();
    expect(() => readComputerModelSettings({ model: "one", effort: "future-level" })).toThrow();
  });

  it("shows a session started by a newer computer whose launch state or origin kind this phone does not know", async () => {
    const { phone, runner, devices } = await paired();
    const created = { kind: "created", threadId: request.id, origin: { kind: "computer", computerId: presence.computerId, name: presence.name }, launch: "running" };
    expect(readComputerResult({ ...created, launch: "future-state" })).toEqual({ ...created, launch: "unknown" });
    expect(readComputerResult({ ...created, origin: { kind: "future-place", region: "x" } })).toEqual({ ...created, origin: { kind: "unknown" } });
    const routing = { version: 1 as const, scope: presence.scope, computerId: request.computerId, requestId: request.id, requestHash: await computerRequestHash(request), runnerDeviceId: request.runnerDeviceId, recipientDeviceId: request.senderDeviceId };
    const box = await sealObject(request.id, request.senderDeviceId, phone.encryptionKey, { ...routing, kind: "computerReply", result: { ...created, launch: "queued", origin: { kind: "edge" } } }, 256 * 1024);
    const unsigned = { ...routing, ...box };
    expect(await openComputerReply({ request, reply: { ...unsigned, signature: b64u(await sign(runner, computerReplyMessage(unsigned))) }, holder: phone, devices }))
      .toEqual({ ...created, launch: "unknown", origin: { kind: "unknown" } });
    for (const launch of [undefined, null, 1, "x".repeat(65)]) expect(() => readComputerResult({ ...created, launch })).toThrow();
    for (const kind of [undefined, null, 1, "x".repeat(65)]) expect(() => readComputerResult({ ...created, origin: { kind } })).toThrow();
    expect(() => readComputerResult({ ...created, origin: { kind: "computer", computerId: "not-a-uuid", name: "Laptop" } })).toThrow();
  });

  it("shows a newer computer with a capability this phone does not know, keeps its known ones working and still verifies it", async () => {
    const { sender, runner, devices } = await paired();
    const wire = { ...presence, capabilities: ["projects", "future-capability", "start"] };
    wire.signature = b64u(await sign(runner, computerPresenceMessage(wire)));
    const computer = parseComputerPresence(wire);
    expect(computer).toEqual(wire);
    expect(await verifyComputerPresence({ presence: computer, scope: presence.scope, reader: sender, devices })).toEqual(wire);
    await expect(sealComputerRequest({ scope: presence.scope, computer, operation: { kind: "describeProjects" }, sender, devices, now })).resolves.toMatchObject({ computerId: presence.computerId });
    await expect(sealComputerRequest({ scope: presence.scope, computer, operation: { kind: "describeSession", threadId: request.id }, sender, devices, now })).rejects.toThrow("Update Nexus");
    const dropped = { ...computer, capabilities: ["projects", "start"] };
    await expect(verifyComputerPresence({ presence: dropped, scope: presence.scope, reader: sender, devices })).rejects.toThrow("identity");
    for (const capabilities of [[1], [""], ["x".repeat(65)], ["a,b"], ["future", "future"], Array.from({ length: 17 }, (_, i) => `c${i}`), "start"])
      expect(() => parseComputerPresence({ ...wire, capabilities })).toThrow();
  });
});

describe("desktop-first phone invitation", () => {
  it("opens a real app/Expo fragment without putting the scan capability in an HTTP query", async () => {
    const { sender, devices } = await paired();
    const link = phoneInvitationLink(invitation, { appUrl: "exp://192.0.2.10:8081/--/connect" });
    expect(new URL(link).search).toBe("");
    expect(parsePhoneInvitation(link)).toEqual(invitation);
    expect((await verifyPhoneInvitation({ invitation: parsePhoneInvitation(link), scope: presence.scope, devices, reader: sender, now })).id).toBe("dev_runner");
  });

  it("binds scanning proof to the phone's keys while hiding the invitation secret from Gateway", async () => {
    const { sender, devices, runner } = await paired();
    const envelope = await sealPhonePairing({ invitation, sender, devices, now });
    expect(JSON.stringify(envelope)).not.toContain(invitation.secret);
    expect(await verify(sender.holder.signingKey, phonePairingMessage(envelope), binary(envelope.signature, 64))).toBe(true);
    const key = await unlockContentKey(runner, envelope.id, 0, envelope.inviterDeviceId, binary(envelope.lockedKey));
    const opened = JSON.parse(new TextDecoder().decode(await openEvent(key, envelope.id, 0, binary(envelope.sealed))));
    expect(opened).toMatchObject({ kind: "phonePairing", secret: invitation.secret, senderDeviceId: sender.deviceId,
      encryptionKey: b64u(sender.holder.encryptionKey), signingKey: b64u(sender.holder.signingKey) });
  });

  it("refuses swapped directory keys, another account, and an expired invitation", async () => {
    const { sender, devices } = await paired();
    const swapped = devices.map((device) => device.id === "dev_runner" ? { ...device, encryptionKey: b64u(sender.holder.encryptionKey) } : device);
    await expect(verifyPhoneInvitation({ invitation, scope: presence.scope, devices: swapped, reader: sender, now })).rejects.toThrow("keys");
    await expect(verifyPhoneInvitation({ invitation, scope: { ...presence.scope, personId: "usr_other" }, devices, reader: sender, now })).rejects.toThrow("same Nexus account");
    await expect(verifyPhoneInvitation({ invitation, scope: presence.scope, devices, reader: sender, now: invitation.expiresAt })).rejects.toThrow("expired");
  });

  it("accepts an invitation from a computer clock up to 30 seconds ahead of the phone's", async () => {
    const { sender, devices } = await paired();
    const earliest = invitation.expiresAt - 5 * 60_000 - 30_000;
    expect((await verifyPhoneInvitation({ invitation, scope: presence.scope, devices, reader: sender, now: earliest })).id).toBe("dev_runner");
    await expect(verifyPhoneInvitation({ invitation, scope: presence.scope, devices, reader: sender, now: earliest - 1 })).rejects.toThrow("expired");
  });
});
