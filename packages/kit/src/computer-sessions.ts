import type { E2eDevice } from "./api";
import type { ComputerBranchList, ComputerOperation, ComputerPresence, ComputerProjectChoice, ComputerReplyEnvelope, ComputerRequestEnvelope, ComputerResult, ComputerScope, ExecutionOrigin } from "./computer-types";
import { b64u, type DeviceKey, sign, verify } from "./e2e";
import type { DeviceTrustMode } from "./device-trust";
import { type DeviceReader, trustedRunner } from "./remote-control";
import { isComputerAgent, readComputerAgents, readComputerModelSettings, readComputerPayer, readComputerSelection, readComputerSessionSettings } from "./computer-settings";
import { binary, COMPUTER_LEASE_MS, computerName, computerPresenceMessage, computerReplyMessage, computerRequestHash, computerRequestMessage, deviceId, integer, MAX_COMPUTER_PROMPT_BYTES, MAX_COMPUTER_REPLY_BYTES, MAX_COMPUTER_REQUEST_BYTES, object, openObject, protocolError, randomUuid, readScope, sameScope, sealObject, text, uuid } from "./computer-protocol";

export { computerClaimMessage, computerPresenceMessage, computerReplyMessage, computerRequestHash, computerRequestMessage } from "./computer-protocol";
export type { ComputerOperation, ComputerPresence, ComputerProjectChoice, ComputerRequestEnvelope, ComputerResult, ComputerScope, ExecutionOrigin } from "./computer-types";
export { isComputerAgent, isComputerEffort, readComputerModelSettings, readComputerPayer, readComputerSelection, readComputerSessionSettings } from "./computer-settings";

/** `trust` is the devices list's mode: with `gateway`, a computer Gateway lists as trusted is trusted. */
type Sender = DeviceReader;

function version(value: unknown): 1 {
  if (value !== 1) throw protocolError("Update Nexus to connect to this computer.");
  return 1;
}

export function parseComputerRequestEnvelope(value: unknown): ComputerRequestEnvelope {
  const fields = object(value);
  binary(fields.signature, 64);
  binary(fields.lockedKey);
  binary(fields.sealed);
  const request: ComputerRequestEnvelope = {
    version: version(fields.version), scope: readScope(fields.scope), computerId: uuid(fields.computerId), id: uuid(fields.id),
    senderDeviceId: deviceId(fields.senderDeviceId), runnerDeviceId: deviceId(fields.runnerDeviceId), generation: integer(fields.generation),
    issuedAt: integer(fields.issuedAt), expiresAt: integer(fields.expiresAt), lockedKey: text(fields.lockedKey, 1024),
    sealed: text(fields.sealed, Math.ceil((MAX_COMPUTER_REQUEST_BYTES + 64) * 4 / 3)), signature: text(fields.signature, 128),
  };
  if (request.expiresAt <= request.issuedAt || request.expiresAt > request.issuedAt + 60_000) throw protocolError();
  return request;
}

const MAX_COMPUTER_CAPABILITIES = 16;

/** A known capability, or a newer computer's one kept as text. The presence signature joins them with commas. */
function capability(value: unknown): ComputerPresence["capabilities"][number] {
  const name = text(value, 64);
  if (name.includes(",")) throw protocolError();
  return name;
}

export function parseComputerPresence(value: unknown): ComputerPresence {
  const fields = object(value);
  if (typeof fields.enabled !== "boolean" || !Array.isArray(fields.capabilities) || fields.capabilities.length > MAX_COMPUTER_CAPABILITIES) throw protocolError();
  const capabilities: ComputerPresence["capabilities"] = fields.capabilities.map(capability);
  const presence: ComputerPresence = {
    version: version(fields.version), scope: readScope(fields.scope), computerId: uuid(fields.computerId), runnerDeviceId: deviceId(fields.runnerDeviceId),
    name: computerName(fields.name), generation: integer(fields.generation), issuedAt: integer(fields.issuedAt), expiresAt: integer(fields.expiresAt),
    enabled: fields.enabled, capabilities, signature: text(fields.signature, 128),
  };
  binary(presence.signature, 64);
  if (new Set(capabilities).size !== capabilities.length || presence.expiresAt <= presence.issuedAt || presence.expiresAt > presence.issuedAt + COMPUTER_LEASE_MS) throw protocolError();
  return presence;
}

export async function verifyComputerPresence({ presence: value, scope, reader, devices }: {
  presence: ComputerPresence; scope: ComputerScope; reader: Sender; devices: E2eDevice[];
}): Promise<ComputerPresence> {
  const presence = parseComputerPresence(value);
  if (!sameScope(presence.scope, scope)) throw protocolError();
  const runner = await trustedRunner(devices, reader, presence.runnerDeviceId);
  if (!await verify(binary(runner.signingKey, 32), computerPresenceMessage(presence), binary(presence.signature, 64))) throw protocolError("This computer's identity could not be verified. Connect it again.");
  return presence;
}

export function parseComputerReplyEnvelope(value: unknown): ComputerReplyEnvelope {
  const fields = object(value);
  binary(fields.signature, 64);
  binary(fields.requestHash, 32);
  binary(fields.lockedKey);
  binary(fields.sealed);
  return { version: version(fields.version), scope: readScope(fields.scope), computerId: uuid(fields.computerId),
    requestId: uuid(fields.requestId), requestHash: text(fields.requestHash, 64), runnerDeviceId: deviceId(fields.runnerDeviceId),
    recipientDeviceId: deviceId(fields.recipientDeviceId), lockedKey: text(fields.lockedKey, 1024),
    sealed: text(fields.sealed, Math.ceil((MAX_COMPUTER_REPLY_BYTES + 64) * 4 / 3)), signature: text(fields.signature, 128) };
}

function baseBranch(value: unknown) {
  const branch = text(value, 256);
  if (branch.startsWith("-") || /\s/.test(branch)) throw protocolError("Choose a valid branch.");
  return branch;
}

function operation(value: ComputerOperation): ComputerOperation {
  if (value.kind === "describeProjects") return { kind: "describeProjects" };
  if (value.kind === "describeBranches") return { kind: "describeBranches", projectId: text(value.projectId, 128), revision: text(value.revision, 128) };
  if (value.kind === "describeSession" || value.kind === "readSessionChanges" || value.kind === "describeHandoff" || value.kind === "compactSession") return { kind: value.kind, threadId: uuid(value.threadId) };
  if (value.kind === "updateSession") {
    return { kind: "updateSession", threadId: uuid(value.threadId), revision: text(value.revision, 128), settings: readComputerModelSettings(value.settings) };
  }
  if (value.kind !== "startSession" || !value.text.trim()) throw protocolError("Write a message first.");
  text(value.text, MAX_COMPUTER_PROMPT_BYTES);
  if (value.handoff && (!isComputerAgent(value.handoff.agent) || !value.selection || value.baseBranch)) throw protocolError("Choose an available handoff agent and payment route.");
  return { kind: "startSession", projectId: text(value.projectId, 128), revision: text(value.revision, 128), text: value.text,
    ...(value.selection !== undefined ? { selection: readComputerSelection(value.selection) } : {}),
    ...(value.baseBranch !== undefined ? { baseBranch: baseBranch(value.baseBranch) } : {}),
    ...(value.handoff ? { handoff: { agent: value.handoff.agent, sourceThreadId: uuid(value.handoff.sourceThreadId) } } : {}) };
}

export async function sealComputerRequest({ scope, computer, operation: requested, sender, devices, id = randomUuid(), now = Date.now(), expiresAt }: {
  scope: ComputerScope; computer: ComputerPresence; operation: ComputerOperation; sender: Sender; devices: E2eDevice[];
  id?: string; now?: number; expiresAt?: number;
}): Promise<ComputerRequestEnvelope> {
  await verifyComputerPresence({ presence: computer, scope, reader: sender, devices });
  if (!computer.enabled || computer.expiresAt <= now || computer.issuedAt > now + 5_000) throw protocolError("This computer is offline or phone control is turned off.");
  const selected = operation(requested);
  const capability = selected.kind === "startSession" ? "start" : (selected.kind === "describeProjects" || selected.kind === "describeBranches") ? "projects" : selected.kind === "readSessionChanges" ? "changes" : "settings";
  if (!computer.capabilities.includes(capability)) {
    throw protocolError(capability === "changes" ? "Update Nexus on the computer to view its recorded changes." : "Update Nexus on this computer to choose remote models and billing.");
  }
  if (selected.kind === "startSession" && selected.selection !== undefined && !computer.capabilities.includes("settings")) throw protocolError("Update Nexus on this computer to choose remote models and billing.");
  const runner = await trustedRunner(devices, sender, computer.runnerDeviceId);
  const admission = selected.kind === "startSession" ? 15_000 : 30_000;
  const route = {
    version: 1 as const, scope: readScope(scope), computerId: computer.computerId, id: uuid(id), senderDeviceId: sender.deviceId,
    runnerDeviceId: runner.id, generation: computer.generation, issuedAt: integer(now), expiresAt: expiresAt ?? now + admission,
  };
  if (route.expiresAt <= now || route.expiresAt > now + admission) throw protocolError("The request has expired. Try again.");
  const box = await sealObject(route.id, runner.id, binary(runner.encryptionKey, 32), { ...route, kind: "computerRequest", operation: selected }, MAX_COMPUTER_REQUEST_BYTES);
  const envelope = { ...route, ...box };
  return { ...envelope, signature: b64u(await sign(sender.holder, computerRequestMessage(envelope))) };
}

function nullableText(value: unknown, maximum = 256): string | null {
  return value === null ? null : text(value, maximum);
}

export function readExecutionOrigin(value: unknown): ExecutionOrigin {
  const fields = object(value);
  if (fields.kind === "unknown") return { kind: "unknown" };
  if (fields.kind === "computer") return { kind: "computer", computerId: uuid(fields.computerId), name: computerName(fields.name) };
  if (fields.kind === "cloud") {
    const machineId = text(fields.machineId, 128);
    return fields.name === undefined ? { kind: "cloud", machineId } : { kind: "cloud", machineId, name: text(fields.name, 160) };
  }
  // A newer computer's origin kind: the session still started, so read it as unknown.
  if (typeof fields.kind !== "string" || fields.kind.length > 64) throw protocolError();
  return { kind: "unknown" };
}

function projects(value: unknown): ComputerProjectChoice[] {
  if (!Array.isArray(value) || value.length > 256) throw protocolError();
  return value.map((entry: unknown) => {
    const fields = object(entry);
    const settings = object(fields.settings);
    if (settings.workspace !== "folder" && settings.workspace !== "worktree") throw protocolError();
    return {
      id: text(fields.id, 128), name: text(fields.name, 512), locationHint: text(fields.locationHint, 2048, true), revision: text(fields.revision, 128),
      ...(fields.payer !== undefined ? { payer: readComputerPayer(fields.payer) } : {}),
      settings: { agent: text(settings.agent, 64), model: nullableText(settings.model), via: text(settings.via, 64),
        permissionMode: text(settings.permissionMode, 64), effort: nullableText(settings.effort, 64), workspace: settings.workspace },
    };
  });
}

export function readComputerBranchList(value: unknown): ComputerBranchList {
  const fields = object(value);
  if (!Array.isArray(fields.branches) || fields.branches.length > 300) throw protocolError();
  return { projectId: text(fields.projectId, 128), revision: text(fields.revision, 128), branches: fields.branches.map(baseBranch) };
}

export function readComputerResult(value: unknown): ComputerResult {
  const fields = object(value);
  if (fields.kind === "projects" || fields.kind === "choicesChanged") return { kind: fields.kind, projects: projects(fields.projects), ...(fields.agents !== undefined ? { agents: readComputerAgents(fields.agents) } : {}) };
  if (fields.kind === "branches") return { kind: "branches", ...readComputerBranchList(fields) };
  if (fields.kind === "handoffChoices") {
    const [project] = projects([fields.project]);
    if (!project) throw protocolError();
    return { kind: "handoffChoices", threadId: uuid(fields.threadId), project, agents: readComputerAgents(fields.agents) };
  }
  if (fields.kind === "sessionSettings" || fields.kind === "sessionUpdated" || fields.kind === "sessionChanged") return { kind: fields.kind, session: readComputerSessionSettings(fields.session) };
  if (fields.kind === "sessionChanges") {
    if (!Array.isArray(fields.files) || fields.files.length > 32 || typeof fields.truncated !== "boolean") throw protocolError();
    return { kind: "sessionChanges", threadId: uuid(fields.threadId), files: fields.files.map((file) => text(file, 512)),
      patch: text(fields.patch, 24 * 1024, true), truncated: fields.truncated };
  }
  if (fields.kind === "refused") return { kind: "refused", code: text(fields.code, 128), message: text(fields.message, 2048) };
  if (fields.kind === "compacted") return { kind: "compacted", threadId: uuid(fields.threadId) };
  if (fields.kind === "uncertain") return { kind: "uncertain", threadId: fields.threadId === null ? null : uuid(fields.threadId) };
  if (fields.kind === "created") {
    const launch = fields.launch;
    if (typeof launch !== "string" || launch.length > 64) throw protocolError();
    return { kind: "created", threadId: uuid(fields.threadId), origin: readExecutionOrigin(fields.origin),
      launch: launch === "starting" || launch === "running" || launch === "idle" || launch === "error" || launch === "interrupted" ? launch : "unknown" };
  }
  throw protocolError();
}

export async function openComputerReply({ request: original, reply: received, holder, devices, trust }: {
  request: ComputerRequestEnvelope; reply: ComputerReplyEnvelope; holder: DeviceKey; devices: E2eDevice[]; trust?: DeviceTrustMode;
}): Promise<ComputerResult> {
  const request = parseComputerRequestEnvelope(original);
  const reply = parseComputerReplyEnvelope(received);
  const runner = await trustedRunner(devices, { deviceId: request.senderDeviceId, holder, ...(trust ? { trust } : {}) }, request.runnerDeviceId);
  if (!await verify(holder.signingKey, computerRequestMessage(request), binary(request.signature, 64))
    || reply.version !== 1 || !sameScope(readScope(reply.scope), request.scope) || reply.computerId !== request.computerId
    || reply.requestId !== request.id || reply.requestHash !== await computerRequestHash(request)
    || reply.runnerDeviceId !== request.runnerDeviceId || reply.recipientDeviceId !== request.senderDeviceId
    || !await verify(binary(runner.signingKey, 32), computerReplyMessage(reply), binary(reply.signature, 64))) throw protocolError();
  const payload = await openObject(request.id, request.senderDeviceId, holder, reply.lockedKey, reply.sealed, MAX_COMPUTER_REPLY_BYTES);
  if (payload.version !== 1 || payload.kind !== "computerReply" || !sameScope(readScope(payload.scope), request.scope)
    || payload.computerId !== request.computerId || payload.requestId !== request.id || payload.requestHash !== reply.requestHash
    || payload.runnerDeviceId !== reply.runnerDeviceId || payload.recipientDeviceId !== reply.recipientDeviceId) throw protocolError();
  return readComputerResult(payload.result);
}
