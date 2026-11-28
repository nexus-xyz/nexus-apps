import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { ApproveDeviceRequest, E2eDevice, RevokeDeviceRequest } from "./api";
import { approvalMessage, b64u, fromB64u, generateDeviceKey, handoverMessage, importDeviceSecret, openHandover, revocationMessage, verify } from "./e2e";
import { readHandoverPayload } from "./handover";
import {
  approvePending,
  deviceFingerprint,
  deviceStatus,
  findPending,
  formatCode,
  normalizeCode,
  PAIRING_TTL_MS,
  PairingError,
  pairingQrText,
  readPairingInput,
  revokeWith,
  thisDevice,
  verifyApprovals,
  webCryptoSigner,
} from "./pairing";

// The Rust core's device vectors: the same keys give the same fingerprint and code on both sides.
const vectorsPath = fileURLToPath(new URL("../../../crates/nexus-client/tests/e2e-vectors.json", import.meta.url));
type Vector = { secret: string; encryption_key_b64u: string; signing_key_b64u: string; fingerprint: string; short_code: string };
const vectors = JSON.parse(readFileSync(vectorsPath, "utf8")) as { devices: [Vector, Vector] };
const [first, second] = vectors.devices;

const NOW = Date.parse("2026-11-04T12:00:00Z");
const ago = (ms: number) => new Date(NOW - ms).toISOString();

function device(id: string, keys: { encryption_key_b64u: string; signing_key_b64u: string }, extra: Partial<E2eDevice> = {}): E2eDevice {
  return {
    id,
    name: id,
    encryptionKey: keys.encryption_key_b64u,
    signingKey: keys.signing_key_b64u,
    state: "pending",
    approvedBy: null,
    approvalSignature: null,
    createdAt: ago(60_000),
    approvedAt: null,
    revokedAt: null,
    ...extra,
  };
}

const hex = (text: string) => Uint8Array.from(text.match(/../g)!, (pair) => parseInt(pair, 16));
const signerOf = async (secret: string) => webCryptoSigner(await importDeviceSecret(hex(secret)));

describe("the code and the QR code", () => {
  it("takes the fingerprint and code the Rust core computes", async () => {
    for (const vector of vectors.devices) {
      const { fingerprint, code } = await deviceFingerprint(device("dev_x", vector));
      expect(b64u(fingerprint)).toBe(b64u(hex(vector.fingerprint)));
      expect(code).toBe(vector.short_code);
    }
  });

  it("reads a code as people type it", () => {
    expect(normalizeCode("s632 4wbk-gocm 3tnw")).toBe("S6324WBKGOCM3TNW");
    expect(normalizeCode("S6324WBKGOCM3TN")).toBeUndefined();
    expect(normalizeCode("S6324WBKGOCM3TN1")).toBeUndefined(); // 1 is not base32
    expect(formatCode("S6324WBKGOCM3TNW")).toBe("S632 4WBK GOCM 3TNW");
    expect(() => readPairingInput("hello")).toThrow(PairingError);
  });

  it("reads a QR code's id and whole fingerprint", () => {
    const fingerprint = hex(first.fingerprint);
    const input = readPairingInput(pairingQrText("dev_abcdefghijkl", fingerprint));
    expect(input).toEqual({ code: first.short_code, deviceId: "dev_abcdefghijkl", fingerprint });
    expect(() => readPairingInput("nexus-pair:v1:dev_a:AAAA")).toThrow(PairingError);
  });
});

describe("finding the pending device", () => {
  const pending = device("dev_new000000001", second);

  it("finds the device whose fingerprint has the code", async () => {
    const devices = [device("dev_root00000001", first, { state: "trusted", approvedAt: ago(10 * 60_000) }), pending];
    expect(await findPending(devices, readPairingInput(second.short_code), NOW)).toBe(pending);
  });

  it("refuses a code no pending device has, and names an expired one", async () => {
    await expect(findPending([pending], readPairingInput(first.short_code), NOW)).rejects.toMatchObject({ kind: "none" });
    const old = device("dev_new000000001", second, { createdAt: ago(PAIRING_TTL_MS) });
    expect(deviceStatus(old, NOW)).toBe("expired");
    await expect(findPending([old], readPairingInput(second.short_code), NOW)).rejects.toMatchObject({ kind: "expired" });
  });

  it("refuses a QR code whose id or fingerprint is another device's", async () => {
    const otherId = readPairingInput(pairingQrText("dev_someone00001", hex(second.fingerprint)));
    await expect(findPending([pending], otherId, NOW)).rejects.toMatchObject({ kind: "none" });
    const otherFingerprint = readPairingInput(pairingQrText(pending.id, hex(first.fingerprint)));
    await expect(findPending([pending], otherFingerprint, NOW)).rejects.toMatchObject({ kind: "none" });
  });

  it("takes the newest row of one device that registered twice", async () => {
    const again = device("dev_new000000002", second, { createdAt: ago(1000) });
    expect(await findPending([pending, again], readPairingInput(second.short_code), NOW)).toBe(again);
  });

  it("finds this device by its keys", async () => {
    const signer = await signerOf(second.secret);
    expect(thisDevice([device("dev_root00000001", first), pending], signer)).toBe(pending);
    expect(thisDevice([device("dev_root00000001", first)], signer)).toBeUndefined();
  });
});

describe("approving and revoking", () => {
  it("signs exactly the approval Gateway verifies, with no copies yet", async () => {
    const approver = await signerOf(first.secret);
    const pending = device("dev_new000000001", second);
    let sent: ApproveDeviceRequest | undefined;
    const core = {
      approveE2eDevice: async (id: string, body: ApproveDeviceRequest) => {
        sent = body;
        return { ...pending, id, state: "trusted" as const };
      },
    };
    await approvePending(core, "dev_root00000001", approver, pending, readPairingInput(second.short_code));
    expect(sent?.approverDeviceId).toBe("dev_root00000001");
    expect(sent?.keys).toEqual([]);
    const message = approvalMessage(pending.id, fromB64u(second.encryption_key_b64u)!, fromB64u(second.signing_key_b64u)!);
    expect(message).toBe(`nexus-device-approval:v1:${pending.id}:${second.encryption_key_b64u}:${second.signing_key_b64u}`);
    expect(await verify(approver.signingKey, message, fromB64u(sent!.signature)!)).toBe(true);
  });

  it("refuses to sign for a device whose keys do not have the code", async () => {
    const approver = await signerOf(first.secret);
    // Gateway lists a device of its own under the code the person typed: its keys are not the new device's.
    const swapped = device("dev_new000000001", { encryption_key_b64u: b64u((await generateDeviceKey()).encryptionKey), signing_key_b64u: second.signing_key_b64u });
    const core = { approveE2eDevice: async () => { throw new Error("must not be called"); } };
    await expect(approvePending(core, "dev_root00000001", approver, swapped, readPairingInput(second.short_code))).rejects.toMatchObject({ kind: "mismatch" });
    await expect(approver.signApproval(swapped, second.short_code)).rejects.toMatchObject({ kind: "mismatch" });
  });

  it("passes the copies the approver locks for the new device", async () => {
    const approver = await signerOf(first.secret);
    const pending = device("dev_new000000001", second);
    const copy = { threadId: "0190f0a0-0000-7000-8000-000000000001", keyVersion: 1, lockedKey: "AAAA", signature: "s".repeat(86) };
    let sent: ApproveDeviceRequest | undefined;
    const core = { approveE2eDevice: async (_id: string, body: ApproveDeviceRequest) => ((sent = body), pending) };
    await approvePending(core, "dev_root00000001", approver, pending, readPairingInput(second.short_code), async (d) => [{ ...copy, threadId: copy.threadId + d.id.slice(-1) }]);
    expect(sent?.keys).toEqual([{ ...copy, threadId: copy.threadId + "1" }]);
  });

  it("seals the approver's trusted list to the new device, signed by the approver", async () => {
    const approver = await signerOf(first.secret);
    const pending = device("dev_new000000001", second);
    const handover = { holders: [{ id: "dev_root00000001", encryptionKey: first.encryption_key_b64u, signingKey: first.signing_key_b64u }], revoked: ["dev_gone00000001"] };
    let sent: ApproveDeviceRequest | undefined;
    const core = { approveE2eDevice: async (_id: string, body: ApproveDeviceRequest) => ((sent = body), pending) };
    await approvePending(core, "dev_root00000001", approver, pending, readPairingInput(second.short_code), undefined, handover);
    const opened = await openHandover(await importDeviceSecret(hex(second.secret)), pending.id, "dev_root00000001", fromB64u(sent!.handover!)!);
    expect(readHandoverPayload(opened.payload)).toEqual(handover);
    expect(await verify(approver.signingKey, handoverMessage(pending.id, "dev_root00000001", opened.payload), opened.signature)).toBe(true);
    // Without a list, or with a signer that cannot sign one (the discontinued Tauri app), the approval carries none.
    await approvePending(core, "dev_root00000001", approver, pending, readPairingInput(second.short_code));
    expect(sent?.handover).toBeUndefined();
    await approvePending(core, "dev_root00000001", { ...approver, signHandover: undefined }, pending, readPairingInput(second.short_code), undefined, handover);
    expect(sent?.handover).toBeUndefined();
  });

  it("signs the revocation text", async () => {
    const revoker = await signerOf(first.secret);
    let sent: RevokeDeviceRequest | undefined;
    const core = { revokeE2eDevice: async (_id: string, body: RevokeDeviceRequest) => ((sent = body), device("dev_new000000001", second, { state: "revoked" })) };
    await revokeWith(core, "dev_root00000001", revoker, "dev_new000000001");
    expect(await verify(revoker.signingKey, revocationMessage("dev_new000000001"), fromB64u(sent!.signature)!)).toBe(true);
  });
});

describe("checking the directory", () => {
  it("marks the root, the approved, and a signature that does not verify", async () => {
    const approver = await signerOf(first.secret);
    const root = device("dev_root00000001", first, { state: "trusted", approvedAt: ago(3600_000) });
    const signed = await approver.signApproval(device("dev_new000000001", second), second.short_code);
    const approved = device("dev_new000000001", second, { state: "trusted", approvedBy: root.id, approvalSignature: b64u(signed), approvedAt: ago(1000) });
    const forged = { ...approved, id: "dev_new000000002" };
    const pending = device("dev_new000000003", second);
    const result = await verifyApprovals([root, approved, forged, pending]);
    expect(Object.fromEntries(result)).toEqual({ dev_root00000001: "root", dev_new000000001: "approved", dev_new000000002: "unverified" });
  });

  it("labels every device Gateway vouches for when trust is gateway, and a recovery key as before", async () => {
    const root = device("dev_root00000001", first, { state: "trusted", approvedAt: ago(3600_000) });
    const unapproved = device("dev_new000000001", second, { state: "trusted", approvedAt: ago(1000) });
    const forged = { ...unapproved, id: "dev_new000000002", approvedBy: root.id, approvalSignature: b64u(new Uint8Array(64)) };
    const recovery = device("dev_code00000001", { encryption_key_b64u: first.encryption_key_b64u, signing_key_b64u: "" }, { kind: "recovery_code", signingKey: null, state: "trusted", approvedAt: ago(1000) });
    const pending = device("dev_new000000003", second);
    const result = await verifyApprovals([root, unapproved, forged, recovery, pending], "gateway");
    expect(Object.fromEntries(result)).toEqual({ dev_root00000001: "gateway", dev_new000000001: "gateway", dev_new000000002: "gateway", dev_code00000001: "unverified" });
    expect((await verifyApprovals([root, unapproved])).get(unapproved.id)).toBe("root");
  });
});

describe("the device's status when trust is gateway", () => {
  it("reads a pending row as expired: nothing approves it, and a new registration is trusted at once", () => {
    const pending = device("dev_new000000001", second);
    expect(deviceStatus(pending, NOW)).toBe("pending");
    expect(deviceStatus(pending, NOW, "gateway")).toBe("expired");
    expect(deviceStatus({ ...pending, state: "trusted" }, NOW, "gateway")).toBe("trusted");
    expect(deviceStatus({ ...pending, state: "revoked" }, NOW, "gateway")).toBe("revoked");
  });
});
