/** Public teammate roots and observed revocations persist across reloads. No content or private key is stored. */
import type { E2eDevice } from "./api";
import { fromB64u } from "./e2e";
import { verifyApprovals } from "./pairing";

export interface TeamTrustMemory {
  root?: string;
  revoked: string[];
}

/** Updates one member atomically. Concurrent tabs cannot replace the first root or forget a revocation. */
export interface TeamTrustStore {
  observe(scope: string, person: string, root: string | undefined, revoked: string[]): Promise<TeamTrustMemory>;
}

export function rememberTeamTrust(previous: TeamTrustMemory | undefined, root: string | undefined, revoked: string[]): TeamTrustMemory {
  return { root: previous?.root ?? root, revoked: [...new Set([...(previous?.revoked ?? []), ...revoked])] };
}

export const browserTeamTrust: TeamTrustStore = {
  observe(scope, person, root, revoked) {
    return new Promise((resolve, reject) => {
      const request = globalThis.indexedDB.open("nexus-team-trust", 1);
      request.onupgradeneeded = () => request.result.createObjectStore("members");
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        const db = request.result;
        const tx = db.transaction("members", "readwrite");
        const store = tx.objectStore("members");
        const key = JSON.stringify([scope, person]);
        const read = store.get(key);
        let memory: TeamTrustMemory;
        read.onsuccess = () => {
          memory = rememberTeamTrust(read.result as TeamTrustMemory | undefined, root, revoked);
          store.put(memory, key);
        };
        tx.oncomplete = () => { db.close(); resolve(memory); };
        tx.onabort = () => { db.close(); reject(tx.error ?? new Error("The teammate trust store failed.")); };
      };
    });
  },
};

/** Only chains anchored in the unchanged pinned root count. A seen revocation always wins over Gateway's list. */
export async function trustedTeamHolders(store: TeamTrustStore, scope: string, person: string, holders: E2eDevice[]): Promise<Set<string>> {
  const roots = holders.filter((holder) => holder.kind === "device" && holder.state === "trusted"
    && holder.approvedBy === null && holder.approvalSignature === null
    && fromB64u(holder.encryptionKey)?.length === 32 && fromB64u(holder.signingKey ?? "")?.length === 32);
  const root = roots.length === 1 ? roots[0] : undefined;
  const fingerprint = root && JSON.stringify([root.id, root.encryptionKey, root.signingKey]);
  const memory = await store.observe(scope, person, fingerprint, holders.filter((holder) => holder.state === "revoked").map((holder) => holder.id));
  const revoked = new Set(memory.revoked);
  const trusted = new Set<string>();
  if (!root || memory.root !== fingerprint || revoked.has(root.id)) return trusted;
  trusted.add(root.id);
  const approvals = await verifyApprovals(holders);
  for (let changed = true; changed;) {
    changed = false;
    for (const holder of holders) {
      if (trusted.has(holder.id) || revoked.has(holder.id) || holder.state !== "trusted") continue;
      if (holder.approvedBy && trusted.has(holder.approvedBy) && approvals.get(holder.id) === "approved") {
        trusted.add(holder.id);
        changed = true;
      }
    }
  }
  return trusted;
}
