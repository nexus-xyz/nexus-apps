import type { ChatSessionMessage } from "@nexus/kit";

// Copies of Nexus UI's `messageMetaParts` (chat/message-meta.ts) and `usd` (shared.ts), which import a
// Next-only module beside them; move both into @nexus/kit if a third client needs them.

export const AUTO = "nexus/auto";

/** Dollars at the precision the amount needs: budgets in cents, model calls to the micro-dollar. */
export function usd(value: number): string {
  const abs = Math.abs(value);
  if (abs === 0) return "$0.00";
  if (abs >= 100 || Math.abs(value * 100 - Math.round(value * 100)) < 1e-9) return `$${value.toFixed(2)}`;
  if (abs >= 0.01) return `$${value.toFixed(4)}`;
  return `$${value.toFixed(6)}`;
}

/** The line under an answer: the model that ran, "auto" when Nexus chose it, the cost. */
export function messageMetaParts(message: Partial<Pick<ChatSessionMessage, "model" | "mode" | "costUsd" | "requestedModel">>, name: (id: string) => string): string[] {
  return [
    message.model ? name(message.model) : message.mode === "ask" && !message.costUsd ? "no model called" : undefined,
    message.requestedModel === AUTO && message.model ? "auto" : undefined,
    message.costUsd !== undefined ? usd(message.costUsd) : undefined,
  ].filter((part): part is string => Boolean(part));
}
