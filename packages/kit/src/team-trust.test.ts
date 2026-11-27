import { expect, it } from "vitest";
import type { E2eDevice } from "./api";
import { approvalMessage, b64u, generateDeviceKey, sign } from "./e2e";
import { rememberTeamTrust, trustedTeamHolders, type TeamTrustMemory, type TeamTrustStore } from "./team-trust";

function memoryTeamTrust(): TeamTrustStore {
  const memory = new Map<string, TeamTrustMemory>();
  return { async observe(scope, person, root, revoked) {
    const key = JSON.stringify([scope, person]);
    const next = rememberTeamTrust(memory.get(key), root, revoked);
    memory.set(key, next);
    return next;
  } };
}
async function directory() {
  const root = await generateDeviceKey();
  const phone = await generateDeviceKey();
  const base = { kind: "device" as const, name: "", state: "trusted" as const, createdAt: "", approvedAt: "2026-11-12", revokedAt: null };
  const holders: E2eDevice[] = [
    { ...base, id: "root", encryptionKey: b64u(root.encryptionKey), signingKey: b64u(root.signingKey), approvedBy: null, approvalSignature: null },
    { ...base, id: "phone", encryptionKey: b64u(phone.encryptionKey), signingKey: b64u(phone.signingKey), approvedBy: "root", approvalSignature: b64u(await sign(root, approvalMessage("phone", phone.encryptionKey, phone.signingKey))) },
  ];
  return holders;
}
it("pins a root and refuses replacement roots after reopening the reader", async () => {
  const store = memoryTeamTrust(); const holders = await directory();
  expect(await trustedTeamHolders(store, "org/device", "person", holders)).toEqual(new Set(["root", "phone"]));
  const forged = await directory();
  expect(await trustedTeamHolders(store, "org/device", "person", forged)).toEqual(new Set());
  expect(await trustedTeamHolders(store, "other-org/device", "person", forged)).toEqual(new Set(["root", "phone"]));
});
it("remembers a revoked holder when Gateway restores its valid approval", async () => {
  const store = memoryTeamTrust(); const holders = await directory();
  await trustedTeamHolders(store, "org/device", "person", holders);
  expect(await trustedTeamHolders(store, "org/device", "person", [holders[0]!, { ...holders[1]!, state: "revoked" }])).toEqual(new Set(["root"]));
  expect(await trustedTeamHolders(store, "org/device", "person", holders)).toEqual(new Set(["root"]));
});
it("refuses disconnected approvals and ambiguous roots", async () => {
  const holders = await directory(); const other = await directory();
  expect(await trustedTeamHolders(memoryTeamTrust(), "scope", "person", [holders[0]!, other[1]!])).toEqual(new Set(["root"]));
  expect(await trustedTeamHolders(memoryTeamTrust(), "scope", "person", [holders[0]!, { ...other[0]!, id: "other" }])).toEqual(new Set());
});
