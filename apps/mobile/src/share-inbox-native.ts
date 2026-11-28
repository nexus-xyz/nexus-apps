import { requireOptionalNativeModule } from "expo";
import * as Crypto from "expo-crypto";

/**
 * The app's own module (`modules/share-inbox`), iOS only. Expo Go and Android lack it, and then the inbox is empty.
 * Every call fails quietly: an inbox that can't be read must not stop the app.
 */
const native = requireOptionalNativeModule<{
  list(): Promise<{ id: string; json: string; folder: string }[]>;
  copyImages(id: string, files: string[]): Promise<string[]>;
  remove(id: string): Promise<void>;
  clear(): Promise<void>;
  setAccount(key: string | null): Promise<void>;
}>("NexusShareInbox");

export const shareInboxAvailable = native !== null;

export const shareInbox = {
  list: () => native?.list().catch(() => []) ?? Promise.resolve([]),
  /** The import's photos, copied into the app's caches; throws when one can't be copied. */
  copyImages: (id: string, files: string[]) => (native && files.length ? native.copyImages(id, files) : Promise.resolve([])),
  remove: (id: string) => native?.remove(id).catch(() => undefined) ?? Promise.resolve(),
  clear: () => native?.clear().catch(() => undefined) ?? Promise.resolve(),
  setAccount: (key: string | null) => native?.setAccount(key).catch(() => undefined) ?? Promise.resolve(),
};

/**
 * The key Save records with an import: the person's Gateway and user, hashed, so the inbox holds no account id. An
 * organization switch keeps it; another person's sign-in changes it.
 */
export function accountKey(gateway: string, user: string): Promise<string> {
  return Crypto.digestStringAsync(Crypto.CryptoDigestAlgorithm.SHA256, `${gateway}\n${user}`);
}
