/**
 * Public test keys only. From packages/: npx vite-node kit/scripts/computer-vectors.ts [--name <computer name>] [--out <file>]
 *
 * Without options it writes the kit's vector (kit/src/fixtures/computer-v1.json). nexus-workbench keeps its own copy,
 * with another computer name: npx vite-node kit/scripts/computer-vectors.ts --name "Ada’s laptop 台" \
 *   --out ../crates/nexus-workbench/tests/fixtures/computer-v1.json
 * The keys come from fixed seeds; the sealed request and reply take fresh randomness, so each run writes new bytes.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import type { ComputerPresence, ComputerReplyEnvelope, ComputerResult, PhoneInvitation } from "../src/computer-types";
import type { E2eDevice } from "../src/api";
import { approvalMessage, b64u, importDeviceSecret, lockContentKey, sealEvent, sign } from "../src/e2e";
import { confirmDeviceApprover, memoryDeviceTrust, setDeviceTrustStore } from "../src/device-trust";
import { deviceFingerprint } from "../src/pairing";
import { computerClaimMessage, computerPresenceMessage, computerReplyMessage, computerRequestHash, computerRequestMessage, sealComputerRequest } from "../src/computer-sessions";
import { phoneInvitationLink, phoneInvitationMessage, phonePairingMessage, sealPhonePairing } from "../src/phone-link";

const { values: options } = parseArgs({ options: { name: { type: "string" }, out: { type: "string" } } });
const phoneSecret = Uint8Array.from({ length: 64 }, (_, i) => i);
const runnerSecret = Uint8Array.from({ length: 64 }, (_, i) => i + 64);
const phone = await importDeviceSecret(phoneSecret);
const runner = await importDeviceSecret(runnerSecret);
const scope = { gateway: "https://api.nexus.example", tenantId: "ten_computer_vector", personId: "usr_computer_vector" };
const now = 1_800_000_000_000;
const computerId = "0192f3a4-0000-7000-8000-000000000001";
const base = { kind: "device", state: "trusted", createdAt: "2026-11-01T00:00:00Z", approvedAt: "2026-11-01T00:01:00Z", revokedAt: null } satisfies Partial<E2eDevice>;
const devices: E2eDevice[] = [
  { ...base, id: "dev_runner", name: "Computer", encryptionKey: b64u(runner.encryptionKey), signingKey: b64u(runner.signingKey), approvedBy: null, approvalSignature: null },
  { ...base, id: "dev_phone", name: "Phone", encryptionKey: b64u(phone.encryptionKey), signingKey: b64u(phone.signingKey), approvedBy: "dev_runner", approvalSignature: b64u(await sign(runner, approvalMessage("dev_phone", phone.encryptionKey, phone.signingKey))) },
];
// The phone trusts the runner as a paired phone does: it confirmed its approver's code on the runner's screen.
setDeviceTrustStore(memoryDeviceTrust());
await confirmDeviceApprover(devices, { deviceId: "dev_phone", ...phone, holder: phone }, (await deviceFingerprint(devices[0]!)).code);
const presence: ComputerPresence = {
  version: 1, scope, computerId, runnerDeviceId: "dev_runner", name: options.name ?? "Eduardo’s MacBook 台", generation: 7,
  issuedAt: now, expiresAt: now + 45_000, enabled: true, capabilities: ["projects", "start"], signature: "",
};
presence.signature = b64u(await sign(runner, computerPresenceMessage(presence)));
const operation = { kind: "startSession", projectId: "p_public_fixture", revision: "public-test-revision", text: "Reply PHONE_START_READY. Do not use tools." } as const;
const request = await sealComputerRequest({ scope, computer: presence, operation, sender: { deviceId: "dev_phone", holder: phone }, devices,
  id: "0192f3a4-0000-7000-8000-000000000002", now });
const requestHash = await computerRequestHash(request);
const result: ComputerResult = { kind: "created", threadId: "0192f3a4-0000-7000-8000-000000000003", origin: { kind: "computer", computerId, name: presence.name }, launch: "running" };
const replyRoute = { version: 1 as const, scope, computerId, requestId: request.id, requestHash, runnerDeviceId: "dev_runner", recipientDeviceId: "dev_phone" };
const replyKey = new Uint8Array(32).fill(201);
const reply: ComputerReplyEnvelope = {
  ...replyRoute,
  lockedKey: b64u(await lockContentKey(replyKey, request.id, 0, "dev_phone", phone.encryptionKey)),
  sealed: b64u(await sealEvent(replyKey, 0, request.id, 0, new TextEncoder().encode(JSON.stringify({ ...replyRoute, kind: "computerReply", result })))),
  signature: "",
};
reply.signature = b64u(await sign(runner, computerReplyMessage(reply)));
replyKey.fill(0);
const invitation: PhoneInvitation = {
  version: 1, scope, inviteId: "0192f3a4-0000-7000-8000-000000000004", computerId, computerName: presence.name,
  inviterDeviceId: "dev_runner", encryptionKey: b64u(runner.encryptionKey), signingKey: b64u(runner.signingKey),
  expiresAt: now + 300_000, secret: b64u(Uint8Array.from({ length: 32 }, (_, i) => i + 128)), signature: "",
};
invitation.signature = b64u(await sign(runner, await phoneInvitationMessage(invitation)));
const pairing = await sealPhonePairing({ invitation, sender: { deviceId: "dev_phone", holder: phone }, devices,
  id: "0192f3a4-0000-7000-8000-000000000005", now });
const fixture = {
  purpose: "Public cross-runtime test keys; never credentials for a real Gateway", now, scope,
  phoneSecret: b64u(phoneSecret), runnerSecret: b64u(runnerSecret), devices, presence, request, requestHash, reply, result,
  invitation, invitationLink: phoneInvitationLink(invitation), pairing,
  messages: { presence: computerPresenceMessage(presence), request: computerRequestMessage(request),
    claim: computerClaimMessage(scope, computerId, request.id, requestHash, "dev_runner"), reply: computerReplyMessage(reply),
    invitation: await phoneInvitationMessage(invitation), pairing: phonePairingMessage(pairing) },
};
const target = options.out ? resolve(options.out) : fileURLToPath(new URL("../src/fixtures/computer-v1.json", import.meta.url));
mkdirSync(dirname(target), { recursive: true });
writeFileSync(target, JSON.stringify(fixture, null, 2) + "\n");
process.stdout.write(`Wrote ${target}\n`);
