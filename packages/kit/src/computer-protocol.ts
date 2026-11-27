import type { ComputerPresence, ComputerReplyEnvelope, ComputerRequestEnvelope, ComputerScope, PhonePairingEnvelope } from "./computer-types";
import { b64u, type DeviceKey, fromB64u, lockContentKey, openEvent, sealEvent, unlockContentKey } from "./e2e";

export const MAX_COMPUTER_REQUEST_BYTES = 64 * 1024;
export const MAX_COMPUTER_REPLY_BYTES = 256 * 1024;
export const MAX_PHONE_PAIRING_BYTES = 8 * 1024;
export const MAX_COMPUTER_PROMPT_BYTES = 48 * 1024;
export const COMPUTER_LEASE_MS = 45_000;

const utf8 = (text: string) => new TextEncoder().encode(text);
export const sized = (value: string): string => `${utf8(value).length}:${value}`;
export const frame = (domain: string, parts: readonly string[]): string => `${domain}\n${parts.map(sized).join("")}`;
export const scopeParts = (scope: ComputerScope): string[] => [scope.gateway, scope.tenantId, scope.personId];
export const sameScope = (a: ComputerScope, b: ComputerScope): boolean => scopeParts(a).every((value, index) => value === scopeParts(b)[index]);

export function computerPresenceMessage(presence: Omit<ComputerPresence, "signature">): string {
  return frame("nexus-computer-presence-v1", [...scopeParts(presence.scope), presence.computerId, presence.runnerDeviceId, presence.name,
    String(presence.generation), String(presence.issuedAt), String(presence.expiresAt), String(presence.enabled), [...presence.capabilities].sort().join(",")]);
}

export function computerRequestMessage(request: Omit<ComputerRequestEnvelope, "signature">): string {
  return frame("nexus-computer-request-v1", [...scopeParts(request.scope), request.computerId, request.id, request.senderDeviceId,
    request.runnerDeviceId, String(request.generation), String(request.issuedAt), String(request.expiresAt), request.lockedKey, request.sealed]);
}

export async function computerRequestHash(request: ComputerRequestEnvelope): Promise<string> {
  return sha256(computerRequestMessage(request) + sized(request.signature));
}

export function computerClaimMessage(scope: ComputerScope, computerId: string, requestId: string, requestHash: string, runnerDeviceId: string): string {
  return frame("nexus-computer-claim-v1", [...scopeParts(scope), computerId, requestId, requestHash, runnerDeviceId]);
}

export function computerReplyMessage(reply: Omit<ComputerReplyEnvelope, "signature">): string {
  return frame("nexus-computer-reply-v1", [...scopeParts(reply.scope), reply.computerId, reply.requestId, reply.requestHash,
    reply.runnerDeviceId, reply.recipientDeviceId, reply.lockedKey, reply.sealed]);
}

export function phonePairingMessage(request: Omit<PhonePairingEnvelope, "signature">): string {
  return frame("nexus-phone-pairing-v1", [...scopeParts(request.scope), request.id, request.inviteId, request.inviterDeviceId,
    request.senderDeviceId, String(request.expiresAt), request.lockedKey, request.sealed]);
}

export async function sha256(input: string | Uint8Array): Promise<string> {
  const bytes = typeof input === "string" ? utf8(input) : new Uint8Array(input);
  return b64u(new Uint8Array(await globalThis.crypto.subtle.digest("SHA-256", bytes)));
}

export function protocolError(message = "The computer sent an invalid response."): Error {
  return new Error(message);
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function object(value: unknown): Record<string, unknown> {
  if (!isRecord(value)) throw protocolError();
  return value;
}

export function text(value: unknown, maximum = 256, empty = false): string {
  if (typeof value !== "string" || (!empty && value.length === 0) || utf8(value).length > maximum) throw protocolError();
  return value;
}

export function computerName(value: unknown): string {
  const name = text(value, 160);
  if (Array.from(name).length > 64 || /[\u0000-\u001f\u007f]/.test(name)) throw protocolError();
  return name;
}

export function integer(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) throw protocolError();
  return value;
}

export function uuid(value: unknown): string {
  const id = text(value, 36);
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id)) throw protocolError();
  return id;
}

export function deviceId(value: unknown): string {
  const id = text(value, 132);
  if (!/^dev_[A-Za-z0-9_-]+$/.test(id)) throw protocolError();
  return id;
}

export function binary(value: unknown, length?: number): Uint8Array<ArrayBuffer> {
  const encoded = text(value, Math.ceil(MAX_COMPUTER_REPLY_BYTES * 4 / 3) + 512);
  const decoded = fromB64u(encoded);
  if (!decoded || b64u(decoded) !== encoded || (length !== undefined && decoded.length !== length)) throw protocolError();
  return decoded;
}

export function readScope(value: unknown): ComputerScope {
  const fields = object(value);
  const gateway = text(fields.gateway, 2048);
  const url = new URL(gateway);
  if (!["https:", "http:"].includes(url.protocol) || gateway.endsWith("/") || url.username || url.password || url.hash || url.search) throw protocolError();
  return { gateway, tenantId: text(fields.tenantId, 128), personId: text(fields.personId, 128) };
}

export function randomUuid(): string {
  const bytes = globalThis.crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6]! & 15) | 64;
  bytes[8] = (bytes[8]! & 63) | 128;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export async function sealObject(id: string, recipientDeviceId: string, recipientKey: Uint8Array, value: unknown, maximum: number): Promise<{ lockedKey: string; sealed: string }> {
  const plaintext = utf8(JSON.stringify(value));
  if (plaintext.length > maximum) throw protocolError("This request is too large. Shorten it and try again.");
  const raw = globalThis.crypto.getRandomValues(new Uint8Array(32));
  try {
    const [locked, sealed] = await Promise.all([lockContentKey(raw, id, 0, recipientDeviceId, recipientKey), sealEvent(raw, 0, id, 0, plaintext)]);
    return { lockedKey: b64u(locked), sealed: b64u(sealed) };
  } finally {
    raw.fill(0);
    plaintext.fill(0);
  }
}

export async function openObject(id: string, recipientDeviceId: string, holder: DeviceKey, lockedKey: string, sealed: string, maximum: number): Promise<Record<string, unknown>> {
  const cipher = binary(sealed);
  if (cipher.length > maximum + 64) throw protocolError();
  const locked = binary(lockedKey);
  if (locked.length > 512) throw protocolError();
  const content = await unlockContentKey(holder, id, 0, recipientDeviceId, locked);
  const plaintext = await openEvent(content, id, 0, cipher);
  try {
    if (plaintext.length > maximum) throw protocolError();
    return object(JSON.parse(new TextDecoder().decode(plaintext)));
  } finally {
    plaintext.fill(0);
  }
}
