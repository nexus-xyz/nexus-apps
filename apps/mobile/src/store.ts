import DefaultStorage, { SQLiteStorage } from "expo-sqlite/kv-store";
import { DEVELOPMENT_GATEWAY } from "./development";

export const Storage = DEVELOPMENT_GATEWAY ? new SQLiteStorage("nexus-local-code.sqlite") : DefaultStorage;

/**
 * What the app keeps on the device (SQLite's key-value store), as JSON: `cache:<org>:<name>`, the last answer of a
 * screen, so it opens at once and then refreshes; `draft:<org>:<chat>`, an unsent message; `place:<org>:<chat>`, where
 * you were reading; `choice:<org>:<key>`, a reader's Allow or Deny for one page's bridge (`page-view.tsx`);
 * `consent:ai-providers:v2`, the AI providers the person agreed to send messages and attachments to (`ai-consent.ts`; Settings
 * withdraws it); `consent:error-reports`, `false` when the person turned off error and crash reports, and
 * `diag:pending`, the reports not sent yet (`error-reports.ts`); `recent:orgs`, the organizations you were in last
 * (`use-switch-org.ts`); `device-trust:<scope>`, the devices this phone verified and the revocations it saw (`device-trust.ts`). Reads are
 * synchronous, for a screen's first frame;
 * writes are not, so typing never waits on them.
 * The caches go on an organization switch and everything but the device's `pref:` keys on sign-out (`forgetCaches`, `forgetAll`). `useStored` (auth.tsx) is one value of the signed-in organization.
 */
export function load<T>(key: string): T | undefined {
  try {
    const raw = Storage.getItemSync(key);
    return raw == null ? undefined : (JSON.parse(raw) as T);
  } catch {
    return undefined;
  }
}

export function save(key: string, value: unknown) {
  void Storage.setItemAsync(key, JSON.stringify(value)).catch(() => undefined);
}

export function remove(key: string) {
  void Storage.removeItemAsync(key).catch(() => undefined);
}

/** Removes `key` before it returns, and throws when storage refuses: for a removal the person is told of. */
export function removeNow(key: string) {
  Storage.removeItemSync(key);
}

export function forgetCaches() {
  try {
    for (const key of Storage.getAllKeysSync()) if (key.startsWith("cache:")) Storage.removeItemSync(key);
  } catch {
    // A cache that stays is refreshed on the next load; it is the organization's own.
  }
}

/**
 * Everything but the device's own preferences (`pref:`, such as the appearance), which aren't the person's data, and
 * its device key's trust memory (`device-trust:`, `device-trust.ts`), which goes with that key.
 */
export function forgetAll() {
  try {
    for (const key of Storage.getAllKeysSync()) if (!key.startsWith("pref:") && !key.startsWith("device-trust:")) Storage.removeItemSync(key);
  } catch {
    // Nothing kept here is a secret: the token is in the Keychain, removed separately.
  }
}

/**
 * The shape of what `cache:` keys hold. Raise it when a cached answer's type changes: the app then drops the caches it
 * kept before (drafts and places stay), so `load` never hands a screen last version's shape as this one's.
 */
const CACHE_FORMAT = "1";
try {
  if (Storage.getItemSync("cache-format") !== CACHE_FORMAT) {
    forgetCaches();
    Storage.setItemSync("cache-format", CACHE_FORMAT);
  }
} catch {
  // Storage unavailable: nothing was kept to be stale.
}
