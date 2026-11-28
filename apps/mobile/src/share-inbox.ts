import { AGENT_MAX_CHARS, CHAT_IMAGE_LIMITS } from "@nexus/kit";

/**
 * Text, links and photos shared to Nexus from another app. The Share extension (`targets/share/`) shows them,
 * and Save keeps them on the phone in the App Group's inbox: no upload and no model call. The app reads the inbox when
 * it opens (`modules/share-inbox`), asks where the import goes, and hands it to that composer as an unsent draft.
 *
 * The parts with no React Native in them, so the repository's vitest runs them. The extension writes `import.json` in
 * Swift; everything here treats it as untrusted input.
 */

/** The App Group of the app and its Share extension. `app.json` and both Swift sides name it too; a test checks that. */
export const APP_GROUP = "group.xyz.nexus.ios";

const MB = 1024 * 1024;

/** What the inbox holds at most. `targets/share/ShareInbox.swift` has the same numbers; a test checks that. */
export const INBOX_LIMITS = {
  /** Imports waiting at once. The extension refuses another until the app takes or discards one. */
  imports: 5,
  /** Photos in one import: as many as one message takes. */
  images: CHAT_IMAGE_LIMITS.count,
  /** One photo as shared, before the app draws it again within Gateway's limits. */
  imageBytes: 25 * MB,
  /** The shared text; longer text is cut, and the extension says so. */
  textCharacters: 20_000,
  urls: 10,
  /** After this, an import that was never opened is removed. */
  days: 7,
} as const;

const DAY_MS = 24 * 60 * 60 * 1000;

/** Why the extension left an item out. */
export type SkipReason = "unsupported" | "too-large" | "too-many" | "unreadable";

export interface SharedImage {
  /** Its file in the import's folder: `image-<n>.<ext>`. */
  file: string;
  /** Its name as shared ("IMG_0001.HEIC"). */
  name: string;
  /** Its MIME type, when the extension knew it. */
  type?: string;
  bytes: number;
}

export interface SharedImport {
  id: string;
  /** When Save was tapped, in ms. */
  createdAt: number;
  /** The account signed in when it was shared (`accountKey`), or null when none was. */
  account: string | null;
  text: string;
  urls: string[];
  images: SharedImage[];
  skipped: { name: string; reason: SkipReason }[];
}

const ID = /^[A-Za-z0-9-]{1,64}$/;
const IMAGE_FILE = /^image-\d{1,2}\.[A-Za-z0-9]{1,8}$/;
const REASONS: readonly SkipReason[] = ["unsupported", "too-large", "too-many", "unreadable"];

const str = (value: unknown, max: number) => (typeof value === "string" ? value.slice(0, max) : "");

/** A web link only: a shared URL passes its address, never a file on the phone. */
function webLink(value: unknown): string | undefined {
  if (typeof value !== "string" || value.length > 2048) return undefined;
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:" ? url.href : undefined;
  } catch {
    return undefined;
  }
}

/** One `import.json` as the extension wrote it, or null when it is not one. */
export function parseImport(id: string, raw: string): SharedImport | null {
  if (!ID.test(id)) return null;
  let data: Record<string, unknown>;
  try {
    data = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!data || typeof data !== "object" || data.version !== 1 || data.id !== id) return null;
  const createdAt = typeof data.createdAt === "string" ? Date.parse(data.createdAt) : NaN;
  if (!Number.isFinite(createdAt)) return null;
  const images = (Array.isArray(data.images) ? data.images : [])
    .filter((image): image is Record<string, unknown> => !!image && typeof image === "object")
    .filter((image) => typeof image.file === "string" && IMAGE_FILE.test(image.file) && typeof image.bytes === "number" && image.bytes > 0 && image.bytes <= INBOX_LIMITS.imageBytes)
    .slice(0, INBOX_LIMITS.images)
    .map((image) => ({ file: image.file as string, name: str(image.name, 200) || "Photo", type: str(image.type, 100) || undefined, bytes: image.bytes as number }));
  const urls = [...new Set((Array.isArray(data.urls) ? data.urls : []).map(webLink).filter((url): url is string => !!url))].slice(0, INBOX_LIMITS.urls);
  const skipped = (Array.isArray(data.skipped) ? data.skipped : [])
    .filter((item): item is Record<string, unknown> => !!item && typeof item === "object" && REASONS.includes(item.reason as SkipReason))
    .slice(0, 50)
    .map((item) => ({ name: str(item.name, 200) || "An item", reason: item.reason as SkipReason }));
  const text = str(data.text, INBOX_LIMITS.textCharacters);
  if (!text.trim() && !urls.length && !images.length && !skipped.length) return null;
  return { id, createdAt, account: typeof data.account === "string" && data.account ? data.account : null, text, urls, images, skipped };
}

/** Why the app removes an import without asking: it is not readable, too old, or another account's. */
export type DropReason = "unreadable" | "expired" | "other-account";

/**
 * The inbox's imports, oldest first, for the account signed in now (`account`); and those to remove. An import made
 * while no one was signed in stays, and the review asks about it. One made for another account never shows.
 */
export function sortOut(entries: { id: string; json: string }[], account: string, now: number): { pending: SharedImport[]; drop: { id: string; why: DropReason }[] } {
  const pending: SharedImport[] = [];
  const drop: { id: string; why: DropReason }[] = [];
  for (const entry of entries) {
    const shared = parseImport(entry.id, entry.json);
    if (!shared) drop.push({ id: entry.id, why: "unreadable" });
    else if (now - shared.createdAt > INBOX_LIMITS.days * DAY_MS) drop.push({ id: entry.id, why: "expired" });
    else if (shared.account !== null && shared.account !== account) drop.push({ id: entry.id, why: "other-account" });
    else pending.push(shared);
  }
  pending.sort((a, b) => a.createdAt - b.createdAt);
  return { pending, drop };
}

/** The import's words for a composer: its text, then each link the text does not already hold. */
export function draftOf(shared: Pick<SharedImport, "text" | "urls">): string {
  const text = shared.text.trim();
  const links = shared.urls.filter((url) => !text.includes(url));
  return [text, links.join("\n")].filter(Boolean).join("\n\n");
}

/** A composer's draft with an import added: an unsent draft is kept, and the import goes after it, once. */
export function withExisting(existing: string, imported: string): string {
  if (!imported) return existing;
  if (!existing.trim()) return imported;
  if (existing.includes(imported)) return existing;
  return `${existing.trimEnd()}\n\n${imported}`;
}

const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** What an import holds, in a few words: "Text, 2 links and 3 photos". */
export function contentsOf(shared: Pick<SharedImport, "text" | "urls" | "images">): string {
  const parts = [shared.text.trim() ? "Text" : "", shared.urls.length ? count(shared.urls.length, "link") : "", shared.images.length ? count(shared.images.length, "photo") : ""].filter(Boolean);
  if (!parts.length) return "Nothing Nexus can use";
  return parts.length === 1 ? parts[0] : `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

const WHY: Record<SkipReason, string> = {
  unsupported: "Nexus takes text, links and photos only",
  "too-large": `larger than ${INBOX_LIMITS.imageBytes / MB} MB`,
  "too-many": `a message takes at most ${INBOX_LIMITS.images} photos`,
  unreadable: "it couldn't be read",
};

/** The items the extension left out, and why, in one line; undefined when it kept everything. */
export function skippedLine(skipped: SharedImport["skipped"]): string | undefined {
  if (!skipped.length) return undefined;
  const shown = skipped.slice(0, 3).map((item) => `${item.name} (${WHY[item.reason]})`);
  const more = skipped.length > 3 ? ` and ${count(skipped.length - 3, "more item")}` : "";
  return `Not saved: ${shown.join("; ")}${more}.`;
}

export type Destination = "chat" | "assistant";

/** Where an import may go: a new chat always; the Assistant only for words it takes, in a build that has it. */
export function destinationsFor(shared: Pick<SharedImport, "text" | "urls" | "images">, assistant: boolean): { to: Destination; label: string; why?: string }[] {
  const assistantWhy = !assistant
    ? "This build has no Assistant."
    : shared.images.length
      ? "The Assistant doesn't take photos. Use a new chat."
      : draftOf(shared).length > AGENT_MAX_CHARS
        ? `The Assistant takes at most ${AGENT_MAX_CHARS.toLocaleString("en-US")} characters. Use a new chat.`
        : undefined;
  return [
    { to: "chat", label: "New chat" },
    { to: "assistant", label: "Assistant", why: assistantWhy },
  ];
}
