import { CHAT_IMAGE_LIMITS, type ChatAttachmentRef, type ChatImageInput, type ChatImageType } from "@nexus/kit";

/**
 * Images in chat, as Nexus UI's `images.tsx` prepares them: PNG, JPEG, GIF or WebP; at most `SEND_SIDE` on a side;
 * within Gateway's limits (`CHAT_IMAGE_LIMITS`), with the web's wording when one is left out. The parts with no React
 * Native in them, so `eng/factory`'s vitest runs them; `images.tsx` picks, draws and opens the images.
 */

/** An image in the composer, ready to send: its bytes and what the tray and the viewer show. */
export interface ComposerImage {
  id: string;
  name: string;
  mediaType: ChatImageType;
  /** Its `data:` URL, built once: what the tray draws and what is sent. */
  uri: string;
  bytes: number;
  width?: number;
  height?: number;
}

/** An image's bytes as Gateway returns them (`chatAttachment`, `artifactAttachment`); a data file's come the same way. */
export type ImageData = { mediaType: string; data: string };

export const IMAGE_TYPES: readonly ChatImageType[] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
/** Longer than this on a side, an image is scaled down before it is sent: models read about 1,600 px at most. */
export const SEND_SIDE = 2048;

const MB = 1024 * 1024;
const EXTENSIONS: Record<ChatImageType, string> = { "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp" };

export function sizeLabel(bytes: number): string {
  return bytes >= MB ? `${(bytes / MB).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** The bytes a base64 string holds. */
export function base64Bytes(data: string): number {
  const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
  return Math.floor((data.length * 3) / 4) - padding;
}

/** "1024 × 768 · 240 KB", as the web's viewer says it. */
export function detailOf(image: { width?: number; height?: number; bytes: number }): string {
  return [image.width && image.height ? `${image.width} × ${image.height}` : "", sizeLabel(image.bytes)].filter(Boolean).join(" · ");
}

/** Within a `maxWidth` × `maxHeight` box, keeping its shape: how one image alone shows above a question. */
export function within(width: number, height: number, maxWidth: number, maxHeight: number): { width: number; height: number } {
  if (!width || !height) return { width: maxWidth, height: Math.round(maxWidth * 0.75) };
  const scale = Math.min(1, maxWidth / width, maxHeight / height);
  return { width: Math.round(width * scale), height: Math.round(height * scale) };
}

export const dataUrl = (image: ImageData) => `data:${image.mediaType};base64,${image.data}`;

/** What a question's `images` sends: the data URL (Gateway reads the type from the bytes) and the name. */
export function toInput(image: ComposerImage): ChatImageInput {
  return { data: image.uri, name: image.name };
}

/**
 * The type an image is drawn again in before it is sent. PNG and WebP go as PNG, which keeps transparency (the phone
 * cannot write WebP everywhere); everything else as JPEG, an iPhone's HEIC photos too, which Gateway does not take. A
 * GIF is not drawn again: it goes as it is, because it may move.
 */
export function sendType(mimeType?: string | null): ChatImageType {
  if (mimeType === "image/gif") return "image/gif";
  return mimeType === "image/png" || mimeType === "image/webp" ? "image/png" : "image/jpeg";
}

/** Its own name with the extension of the type it is sent as: "IMG_0001.HEIC" goes as "IMG_0001.jpg". */
export function sendName(fileName: string | null | undefined, mediaType: ChatImageType): string {
  const base = (fileName ?? "").replace(/\.[^./]*$/, "").trim() || "image";
  return `${base}.${EXTENSIONS[mediaType]}`;
}

export const tooLarge = (name: string) => `${name} is too large: an image may be at most ${CHAT_IMAGE_LIMITS.bytes / MB} MB.`;
export const TOO_MANY = `A message takes at most ${CHAT_IMAGE_LIMITS.count} images.`;

/** How many more images the composer takes. */
export const room = (current: ComposerImage[]) => Math.max(0, CHAT_IMAGE_LIMITS.count - current.length);

/** Prepared images (or why each failed) added to `current` within Gateway's limits; what was left out and why, in one line. */
export function fit(current: ComposerImage[], prepared: (ComposerImage | Error)[]): { images: ComposerImage[]; problem?: string } {
  const images = [...current];
  const problems: string[] = [];
  for (const image of prepared) {
    if (images.length >= CHAT_IMAGE_LIMITS.count) {
      problems.push(TOO_MANY);
      break;
    }
    if (image instanceof Error) {
      problems.push(image.message);
      continue;
    }
    if (images.reduce((sum, item) => sum + item.bytes, 0) + image.bytes > CHAT_IMAGE_LIMITS.totalBytes) {
      problems.push(`A message's images may total at most ${CHAT_IMAGE_LIMITS.totalBytes / MB} MB.`);
      break;
    }
    images.push(image);
  }
  const problem = [...new Set(problems)].join(" ");
  return { images, ...(problem ? { problem } : {}) };
}

/** A stored image: an attachment that is not a data file, in a type the composer sends. */
export type StoredImage = ChatAttachmentRef & { mediaType: ChatImageType };

export function isImage(attachment: ChatAttachmentRef): attachment is StoredImage {
  return attachment.type === "image" && IMAGE_TYPES.some((type) => type === attachment.mediaType);
}

/** A stored image as the composer holds one, to send it again (Retry on a chat opened from the list). */
export function fromStored(attachment: StoredImage, image: ImageData): ComposerImage {
  const { id, mediaType, bytes, width, height } = attachment;
  return { id, name: attachment.name ?? sendName(undefined, mediaType), mediaType, uri: dataUrl(image), bytes, width, height };
}

/**
 * How much of the images read the app keeps in memory, in base64 characters: a chat's images open again without a
 * download. The latest read is kept whatever its size; the oldest go first.
 */
export const KEPT_CHARS = 48 * MB;
const kept = new Map<string, { image: Promise<ImageData>; chars: number }>();

/**
 * One stored image's bytes, read once (a stored image never changes) and kept in memory, never on the device. `key`
 * names where it is read: `session:<id>:<attachment>` or `artifact:<id>:<attachment>`.
 */
export function readImage(key: string, load: () => Promise<ImageData>): Promise<ImageData> {
  const known = kept.get(key);
  if (known) {
    // The latest read last, so the oldest goes first.
    kept.delete(key);
    kept.set(key, known);
    return known.image;
  }
  const read = load();
  keep(key, read);
  // A failed read is not kept: the next look tries again.
  read.catch(() => kept.get(key)?.image === read && kept.delete(key));
  return read;
}

function keep(key: string, image: Promise<ImageData>) {
  const entry = { image, chars: 0 };
  kept.set(key, entry);
  image.then(
    (read) => {
      entry.chars = read.data.length;
      let total = [...kept.values()].reduce((sum, e) => sum + e.chars, 0);
      for (const [oldest, e] of kept) {
        if (total <= KEPT_CHARS || oldest === key) break;
        kept.delete(oldest);
        total -= e.chars;
      }
    },
    () => undefined,
  );
}

/** A question just sent: what was attached, kept under Gateway's ids, so the chat shows it without reading it back. */
export function keepSent(sessionId: string, attachments: ChatAttachmentRef[] | undefined, sent: ComposerImage[]) {
  attachments?.filter((attachment) => attachment.type !== "file").forEach((attachment, index) => {
    const image = sent[index];
    if (image) keep(`session:${sessionId}:${attachment.id}`, Promise.resolve({ mediaType: image.mediaType, data: image.uri.slice(image.uri.indexOf(",") + 1) }));
  });
}

/** On sign-out and on an organization switch: the images were the last organization's. */
export function forgetImages() {
  kept.clear();
}
