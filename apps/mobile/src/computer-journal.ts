import { b64u, fromB64u, lockContentKey, openEvent, sealEvent, unlockContentKey, type DeviceKey } from "@nexus/kit/e2e";
import * as Crypto from "expo-crypto";
import { Storage } from "./store";

interface Ciphertext { id: string; locked: string; sealed: string }
function ciphertext(value: unknown): Ciphertext | undefined {
  if (!value || typeof value !== "object" || !("id" in value) || !("locked" in value) || !("sealed" in value)) return;
  if (typeof value.id !== "string" || typeof value.locked !== "string" || typeof value.sealed !== "string") return;
  return { id: value.id, locked: value.locked, sealed: value.sealed };
}

/** The only disk adapter for native start state. SQLite receives ciphertext locked to this phone's existing key. */
export function computerJournal<T>(scope: string, deviceId: string, holder: DeviceKey, parse: (value: unknown) => T | undefined) {
  const key = `native-journal:${scope}:${b64u(holder.encryptionKey)}`;
  let writing = Promise.resolve();
  return {
    async load(): Promise<T | undefined> {
      const raw = await Storage.getItemAsync(key);
      if (!raw) return;
      const stored = ciphertext(JSON.parse(raw));
      if (!stored) throw new Error("Saved computer request is invalid.");
      const locked = fromB64u(stored.locked);
      const sealed = fromB64u(stored.sealed);
      if (!locked || !sealed) throw new Error("Saved computer request is invalid.");
      const content = await unlockContentKey(holder, stored.id, 0, deviceId, locked);
      const opened = parse(JSON.parse(new TextDecoder().decode(await openEvent(content, stored.id, 0, sealed))));
      if (!opened) throw new Error("Saved computer request is invalid.");
      return opened;
    },
    save(value: T): Promise<void> {
      // Serial order prevents a slow old encryption from replacing a newer draft or receipt.
      const next = writing.catch(() => undefined).then(async () => {
        const id = Crypto.randomUUID();
        const content = Crypto.getRandomBytes(32);
        try {
          const sealed = await sealEvent(content, 0, id, 0, new TextEncoder().encode(JSON.stringify(value)));
          const locked = await lockContentKey(content, id, 0, deviceId, holder.encryptionKey);
          await Storage.setItemAsync(key, JSON.stringify({ id, locked: b64u(locked), sealed: b64u(sealed) }));
        } finally { content.fill(0); }
      });
      writing = next;
      return next;
    },
    remove(): Promise<void> {
      const next = writing.catch(() => undefined).then(async () => { await Storage.removeItemAsync(key); });
      writing = next;
      return next;
    },
  };
}
