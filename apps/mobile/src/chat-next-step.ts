import type { ChatSessionMessage, ChatSuggestion } from "@nexus/kit";

type Answer = Pick<ChatSessionMessage, "id" | "role" | "status" | "mode" | "model"> & { stopped?: boolean };

export function nextStepModel(last: Answer | undefined): string | undefined {
  if (last?.role !== "assistant" || last.status !== "complete" || last.stopped || last.mode === "ask" || last.id === "answer") return;
  return last.model && last.model !== "nexus/auto" ? last.model : undefined;
}

export function addSuggestionToDraft(draft: string, suggestion: string): string {
  if (!draft.trim()) return suggestion;
  const separator = draft.endsWith("\n\n") ? "" : draft.endsWith("\n") ? "\n" : "\n\n";
  return draft + separator + suggestion;
}

export async function requestNextStep(
  consent: () => Promise<boolean>,
  load: () => Promise<ChatSuggestion>,
  signal: AbortSignal,
): Promise<{ kind: "cancelled" } | { kind: "absent" } | { kind: "ready"; text: string }> {
  if (signal.aborted || !(await consent()) || signal.aborted) return { kind: "cancelled" };
  const result = await load();
  if (signal.aborted) return { kind: "cancelled" };
  if (result.suggestion === null) return { kind: "absent" };
  const text = typeof result.suggestion === "string" ? result.suggestion.trim() : "";
  if (!text || /[\r\n]/.test(text) || [...text].length > 120) throw new Error("Gateway returned an invalid suggestion.");
  return { kind: "ready", text };
}
