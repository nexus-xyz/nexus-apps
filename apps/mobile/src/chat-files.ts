import { CHAT_FILE_LIMITS, type ChatAttachmentRef, type ChatFileInput } from "@nexus/kit";
import { dataUrl, type ImageData } from "./chat-images";

/**
 * Data files in chat: CSV, TSV, JSON, TXT, Markdown or XLSX, attached to a question in Nexus UI. The app
 * attaches none of its own. A question lists its files by name and size, as Nexus UI's `data-files.tsx` does, and
 * Retry sends them again with the question. The parts with no React Native in them, so the repository's vitest runs them.
 */

export const isDataFile = (attachment: ChatAttachmentRef) => attachment.type === "file";

/** A question's data file, read back to send again: Gateway's ref, which the chat shows meanwhile, and what `files` takes. */
export interface ResentFile {
  ref: ChatAttachmentRef;
  input: ChatFileInput;
}

/** A stored data file as `files` sends it again: its data URL, and its name with its extension, which a page reads it by. */
export function resend(ref: ChatAttachmentRef, file: ImageData): ResentFile {
  const extension = Object.entries(CHAT_FILE_LIMITS.types).find(([, type]) => type === ref.mediaType)?.[0] ?? "txt";
  return { ref, input: { data: dataUrl(file), name: ref.name ?? `file.${extension}` } };
}
