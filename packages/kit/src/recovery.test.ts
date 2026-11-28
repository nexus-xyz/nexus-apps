import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { ApproveDeviceRequest, E2eDevice, PutThreadKeysRequest, RecoverDeviceRequest, RegisterDeviceRequest, ThreadKey } from "./api";
import { CoreError } from "./core-client";
import {
  b64u,
  E2eError,
  formatRecoveryCode,
  importHolderSecret,
  fromB64u,
  generateDeviceKey,
  generateRecoveryCode,
  holderApprovalMessage,
  lockContentKey,
  threadKeyMessage,
  unlockContentKeyBytes,
  verify,
} from "./e2e";
import { type PairingSigner, verifyApprovals, webCryptoSigner } from "./pairing";
import { addRecoveryKey, findRecoveryKey, recoverWith, recoveryCodeHolder, RecoveryError, unlockHistory } from "./recovery";

// The shared vectors' recovery challenge: Gateway's sealed bytes for a known holder.
const vectors = JSON.parse(readFileSync(fileURLToPath(new URL("../../../crates/nexus-client/tests/e2e-vectors.json", import.meta.url)), "utf8")) as {
  recover_challenges: { holder_secret_key: string; person_id: string; holder_id: string; challenge: string; sealed: string }[];
};
const VECTOR = vectors.recover_challenges[0]!;
const fromHex = (text: string) => Uint8Array.from(text.match(/../g)!, (pair) => parseInt(pair, 16));

const PERSON = "user_ada";
const THREAD = "0192f3a4-5b6c-7d8e-9fa0-b1c2d3e4f506";
const OTHER = "0192f3a4-0000-7000-8000-00000000ffff";
const AT = "2026-11-05T12:00:00.000Z";

function row(id: string, extra: Partial<E2eDevice>): E2eDevice {
  return {
    id,
    name: id,
    kind: "device",
    encryptionKey: "",
    signingKey: null,
    state: "trusted",
    approvedBy: null,
    approvalSignature: null,
    createdAt: AT,
    approvedAt: AT,
    revokedAt: null,
    ...extra,
  };
}

async function deviceRow(id: string, signer: PairingSigner, extra: Partial<E2eDevice> = {}): Promise<E2eDevice> {
  return row(id, { encryptionKey: b64u(signer.encryptionKey), signingKey: b64u(signer.signingKey), ...extra });
}

/**
 * Device A (the root), its recovery code approved by A, and one copy of a content key locked for the code by A. With
 * `vector`, the code is the shared vectors' recovery-challenge holder.
 */
async function setup(vector = false) {
  const a = webCryptoSigner(await generateDeviceKey());
  const text = formatRecoveryCode(generateRecoveryCode());
  const holder = vector ? await importHolderSecret(fromHex(VECTOR.holder_secret_key)) : await recoveryCodeHolder(text, PERSON);
  const codeId = vector ? VECTOR.holder_id : "dev_code";
  const approval = await a.signHolderApproval(codeId, "recovery_code", holder.encryptionKey);
  const code = row(codeId, {
    kind: "recovery_code",
    encryptionKey: b64u(holder.encryptionKey),
    approvedBy: "dev_a",
    approvalSignature: b64u(approval),
  });
  const devices = [await deviceRow("dev_a", a), code];
  const raw = globalThis.crypto.getRandomValues(new Uint8Array(32));
  const copyFor = async (threadId: string, signer: PairingSigner, sender: string): Promise<ThreadKey> => {
    const locked = await lockContentKey(raw, threadId, 0, codeId, holder.encryptionKey);
    return {
      threadId,
      keyVersion: 1,
      deviceId: codeId,
      lockedKey: b64u(locked),
      senderDeviceId: sender,
      signature: b64u(await signer.signThreadKey(threadId, 0, codeId, locked)),
      createdAt: AT,
    };
  };
  return { a, text, holder, code, devices, raw: new Uint8Array(raw), copyFor };
}

describe("recovery keys", () => {
  it("a device verifies a recovery key's v2 approval, and a recovery key approves nothing", async () => {
    const { a, code, devices } = await setup();
    expect(await verifyApprovals(devices)).toEqual(new Map([["dev_a", "root"], ["dev_code", "approved"]]));
    // Approved by the recovery key itself, or with no approver: never trusted.
    const b = webCryptoSigner(await generateDeviceKey());
    const byCode = await deviceRow("dev_b", b, { approvedBy: "dev_code", approvalSignature: b64u(new Uint8Array(64)) });
    const rootCode = { ...code, id: "dev_code2", approvedBy: null, approvalSignature: null };
    const trust = await verifyApprovals([...devices, byCode, rootCode]);
    expect(trust.get("dev_b")).toBe("unverified");
    expect(trust.get("dev_code2")).toBe("unverified");
    expect(findRecoveryKey(devices, { encryptionKey: fromB64u(code.encryptionKey)! })).toBe(code);
    expect(findRecoveryKey(devices, { encryptionKey: a.encryptionKey })).toBeUndefined();
  });

  it("setup registers the holder and approves it with this device's signature", async () => {
    const a = webCryptoSigner(await generateDeviceKey());
    const holder = await recoveryCodeHolder(formatRecoveryCode(generateRecoveryCode()), PERSON);
    const calls: unknown[] = [];
    const core = {
      registerE2eDevice: async (body: RegisterDeviceRequest) => (calls.push(body), row("dev_new", { ...body, state: "pending", approvedAt: null } as Partial<E2eDevice>)),
      approveE2eDevice: async (id: string, body: ApproveDeviceRequest) => (calls.push({ id, ...body }), row(id, {})),
    };
    await addRecoveryKey(core, "dev_a", a, "recovery_code", holder, "Recovery code");
    expect(calls[0]).toEqual({ name: "Recovery code", kind: "recovery_code", encryptionKey: b64u(holder.encryptionKey) });
    const approval = calls[1] as ApproveDeviceRequest & { id: string };
    expect(approval.id).toBe("dev_new");
    const message = holderApprovalMessage("dev_new", "recovery_code", holder.encryptionKey);
    expect(await verify(a.signingKey, message, fromB64u(approval.signature)!)).toBe(true);
  });

  it("a typo in the code is refused before any call", async () => {
    const { text } = await setup();
    const typo = (text[0] === "1" ? "2" : "1") + text.slice(1);
    await expect(recoveryCodeHolder(typo, PERSON)).rejects.toBeInstanceOf(E2eError);
  });

  it("unlocks the copies a trusted device signed, and skips any other", async () => {
    const { holder, code, devices, raw, copyFor, a } = await setup();
    const mallory = webCryptoSigner(await generateDeviceKey());
    const forged = await copyFor(OTHER, mallory, "dev_a");
    const pages: Record<string, { keys: ThreadKey[]; nextAfter: string | null }> = {
      start: { keys: [await copyFor(THREAD, a, "dev_a")], nextAfter: "p2" },
      p2: { keys: [forged], nextAfter: null },
    };
    const core = { deviceThreadKeys: async (id: string, query: { after?: string } = {}) => (expect(id).toBe("dev_code"), pages[query.after ?? "start"]!) };
    const { keys, skipped } = await unlockHistory(core, devices, code, holder);
    expect(skipped).toBe(1);
    expect(keys.map((key) => [key.threadId, key.keyVersion])).toEqual([[THREAD, 1]]);
    expect(b64u(keys[0]!.raw)).toBe(b64u(raw));
    // Nothing that opens: refused, so the person knows before any device is revoked.
    const empty = { deviceThreadKeys: async () => ({ keys: [forged], nextAfter: null }) };
    await expect(unlockHistory(empty, devices, code, holder)).rejects.toEqual(new RecoveryError("empty"));
  });

  it("skips a copy from a root Gateway made up: senders chain to the recovery key's own approver", async () => {
    const { holder, code, devices, copyFor, a } = await setup();
    const made = webCryptoSigner(await generateDeviceKey());
    const listed = [...devices, await deviceRow("dev_made", made)];
    const core = { deviceThreadKeys: async () => ({ keys: [await copyFor(THREAD, a, "dev_a"), await copyFor(OTHER, made, "dev_made")], nextAfter: null }) };
    const { keys, skipped } = await unlockHistory(core, listed, code, holder);
    expect(skipped).toBe(1);
    expect(keys.map((key) => key.threadId)).toEqual([THREAD]);
  });

  it("the way back: proves the recovery key, recovers, approves it again and locks each key for this device", async () => {
    const { holder, code, devices, raw, copyFor, a } = await setup(true);
    const { keys } = await unlockHistory({ deviceThreadKeys: async () => ({ keys: [await copyFor(THREAD, a, "dev_a"), await copyFor(OTHER, a, "dev_a")], nextAfter: null }) }, devices, code, holder);
    const bKey = await generateDeviceKey();
    const b = webCryptoSigner(bKey);
    const approvals: (ApproveDeviceRequest & { id: string })[] = [];
    const puts: (PutThreadKeysRequest & { threadId: string })[] = [];
    const recovered: RecoverDeviceRequest[] = [];
    const core = {
      recoverE2eChallenge: async () => ({
        challenges: [{ holderId: VECTOR.holder_id, kind: "recovery_code" as const, sealed: b64u(fromHex(VECTOR.sealed)) }],
        expiresAt: AT,
      }),
      recoverE2eDevice: async (body: RecoverDeviceRequest) => (recovered.push(body), row("dev_b", { name: body.name, encryptionKey: body.encryptionKey, signingKey: body.signingKey })),
      approveE2eDevice: async (id: string, body: ApproveDeviceRequest) => (approvals.push({ id, ...body }), row(id, {})),
      putThreadKeys: async (threadId: string, body: PutThreadKeysRequest) => {
        // The second thread was deleted meanwhile: skipped, not an error.
        if (threadId === OTHER) throw new CoreError(410, "this thread was deleted", "thread_deleted");
        puts.push({ threadId, ...body });
        return { threadId, keyVersion: body.keyVersion, stored: ["dev_b"], alreadyPresent: [] };
      },
    };
    const { device, stored } = await recoverWith(core, b, VECTOR.person_id, "new laptop", [{ device: code, holder }], keys);
    expect(device.id).toBe("dev_b");
    // The proof: the challenge Gateway sealed to the code, opened, for that recovery key.
    expect(recovered).toHaveLength(1);
    expect(recovered[0]).toMatchObject({ holderId: VECTOR.holder_id, challenge: b64u(fromHex(VECTOR.challenge)) });
    expect(stored).toBe(1);
    expect(approvals).toHaveLength(1);
    expect(approvals[0]!.approverDeviceId).toBe("dev_b");
    const message = holderApprovalMessage(VECTOR.holder_id, "recovery_code", holder.encryptionKey);
    expect(await verify(b.signingKey, message, fromB64u(approvals[0]!.signature)!)).toBe(true);
    const put = puts[0]!;
    expect(put).toMatchObject({ threadId: THREAD, keyVersion: 1, senderDeviceId: "dev_b" });
    const locked = fromB64u(put.keys[0]!.lockedKey)!;
    expect(await verify(b.signingKey, threadKeyMessage(THREAD, 0, "dev_b", locked), fromB64u(put.keys[0]!.signature)!)).toBe(true);
    expect(b64u(await unlockContentKeyBytes(bKey, THREAD, 0, "dev_b", locked))).toBe(b64u(raw));
    expect(keys.every((key) => key.raw.every((byte) => byte === 0))).toBe(true);
  });
});
