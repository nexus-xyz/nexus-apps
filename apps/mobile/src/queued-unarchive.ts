import { CoreError, type ChatSessionSummary, type CoreClient } from "@nexus/kit";

export type QueuedUnarchive = Pick<ChatSessionSummary, "id" | "updatedAt">;

/**
 * Read again before honoring an offline action. The versioned client uses that read's ETag for the PATCH. If the
 * session changed on another device before this read or before the write, the newer change wins.
 */
export async function restoreQueuedChat(
  core: Pick<CoreClient, "chatSession" | "updateChatSession">,
  queued: QueuedUnarchive,
  signal: AbortSignal,
): Promise<"restored" | "changed"> {
  try {
    const current = await core.chatSession(queued.id, signal);
    signal.throwIfAborted();
    if (!current.archived) return "restored";
    if (current.updatedAt !== queued.updatedAt) return "changed";
    await core.updateChatSession(queued.id, { archived: false }, signal);
    return "restored";
  } catch (error) {
    if (error instanceof CoreError && error.status === 404) return "restored";
    if (error instanceof CoreError && error.status === 412) return "changed";
    throw error;
  }
}
