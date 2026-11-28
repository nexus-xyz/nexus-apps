import type { CodeCommandEnvelope } from "@nexus/kit";
import type { ComputerScope } from "@nexus/kit/computer-types";
import type { DeviceKey } from "@nexus/kit/e2e";
import { computerJournal } from "./computer-journal";

/** A Code session's unsent message, saved per thread as ciphertext locked to this phone's key. */
export interface CodeDraft {
  text: string;
}

export interface CodeDraftStore {
  load(): Promise<CodeDraft | undefined>;
  save(value: CodeDraft): Promise<void>;
  remove(): Promise<void>;
}

function parseDraft(value: unknown): CodeDraft | undefined {
  if (!value || typeof value !== "object" || !("text" in value) || typeof value.text !== "string") return;
  return { text: value.text };
}

/**
 * `scope` is the Gateway, organization and person, as `ComputerSession` uses them: another person or organization
 * signed in on this phone never reads the draft.
 */
export function codeDraftStore(scope: ComputerScope, threadId: string, reader: { deviceId: string; holder: DeviceKey }): CodeDraftStore {
  const account = JSON.stringify([scope.gateway.replace(/\/+$/, ""), scope.tenantId, scope.personId]);
  return computerJournal(`code-draft:${account}:${threadId}`, reader.deviceId, reader.holder, parseDraft);
}

/**
 * A message that waits for a sleeping computer, kept through an app restart so the phone tracks it by id and
 * a resend never queues it twice. `envelope` is the signed ciphertext Gateway holds. `text` is the message, as the draft
 * keeps it: the phone clears the draft when the computer confirms this text. The journal stores it all as ciphertext
 * locked to this phone's key, never a key. `cancelling`: the person tapped Cancel, so the phone never sends it again.
 */
export interface CodeWait {
  envelope: CodeCommandEnvelope;
  text: string;
  waitsFor: string;
  cancelling?: true;
}

export interface CodeWaitStore {
  load(): Promise<CodeWait | undefined>;
  save(value: CodeWait): Promise<void>;
  remove(): Promise<void>;
}

const text = (value: unknown, max: number): value is string => typeof value === "string" && value.length > 0 && value.length <= max;

function parseWait(value: unknown): CodeWait | undefined {
  if (!value || typeof value !== "object" || !("envelope" in value) || !("text" in value) || !("waitsFor" in value)) return;
  const { envelope, text: message, waitsFor, cancelling } = value as { envelope: unknown; text: unknown; waitsFor: unknown; cancelling?: unknown };
  if (!envelope || typeof envelope !== "object" || typeof message !== "string" || !text(waitsFor, 256)) return;
  const e = envelope as Record<string, unknown>;
  if (!text(e.id, 64) || !text(e.senderDeviceId, 128) || !text(e.runnerDeviceId, 128) || !Number.isSafeInteger(e.expiresAt)
    || !text(e.lockedKey, 512) || !text(e.sealed, 100_000) || !text(e.signature, 256) || e.waits !== true) return;
  return {
    envelope: { id: e.id, senderDeviceId: e.senderDeviceId, runnerDeviceId: e.runnerDeviceId, expiresAt: e.expiresAt as number, lockedKey: e.lockedKey, sealed: e.sealed, signature: e.signature, waits: true },
    text: message,
    waitsFor,
    ...(cancelling === true ? { cancelling: true as const } : {}),
  };
}

/** Scoped as `codeDraftStore`: one waiting message per person, organization and thread. */
export function codeWaitStore(scope: ComputerScope, threadId: string, reader: { deviceId: string; holder: DeviceKey }): CodeWaitStore {
  const account = JSON.stringify([scope.gateway.replace(/\/+$/, ""), scope.tenantId, scope.personId]);
  return computerJournal(`code-wait:${account}:${threadId}`, reader.deviceId, reader.holder, parseWait);
}

/**
 * The draft to show after a load: the saved one, unless the person already typed, or the saved text is the message
 * the computer already confirmed.
 */
export function restoredDraft(saved: CodeDraft | undefined, typed: string, delivered: string | undefined): string | undefined {
  if (!saved?.text.trim() || typed) return;
  if (delivered !== undefined && saved.text.trim() === delivered.trim()) return;
  return saved.text;
}
