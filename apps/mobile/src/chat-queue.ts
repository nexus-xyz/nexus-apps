import type { ChatMode } from "@nexus/kit";
import type { ResentFile } from "@/chat-files";
import type { ComposerImage } from "@/chat-images";

export type QueueRequest = { mode: ChatMode; model: string; team?: string };
type QueueItem = { id: string; content: string; attempt?: QueueRequest; delivery?: "retry" | "unconfirmed" };
export type Queued = QueueItem & { images?: ComposerImage[]; files?: ResentFile[]; now?: boolean; missingAttachments?: boolean };
type StoredItem = QueueItem & { hadAttachments?: boolean };
export type StoredQueue = { items: StoredItem[]; held?: "failed" | "stopped" };

export function restoreQueue(stored: StoredQueue | string[] | undefined, nextId: () => string): Queued[] {
  if (!stored) return [];
  if (Array.isArray(stored)) {
    return stored.filter((content) => content.trim()).map((content) => ({ id: nextId(), content }));
  }
  return stored.items.map(({ hadAttachments, ...item }) => ({ ...item, missingAttachments: hadAttachments }));
}

export function storeQueue(queue: Queued[], outcome: "done" | "failed" | "stopped" | null): StoredQueue {
  return {
    items: queue.map(({ id, content, attempt, delivery, files, images, missingAttachments }) => ({
      id, content, attempt, delivery,
      hadAttachments: !!(missingAttachments || files?.length || images?.length),
    })),
    ...(outcome === "failed" || outcome === "stopped" ? { held: outcome } : {}),
  };
}

export function cannotSendQueued(item: Queued): boolean {
  return item.delivery === "unconfirmed" || !!item.missingAttachments;
}

export function queueFailure(code?: string): "accepted" | "retry" | "unconfirmed" {
  if (code === "idempotency_already_sent" || code === "idempotency_stream_replay") return "accepted";
  return code === "idempotency_interrupted" ? "unconfirmed" : "retry";
}
