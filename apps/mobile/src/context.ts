import type { ChatSessionMessage } from "@nexus/kit";

/**
 * How full the chat's context is (brand v5's P53): what the latest answer read and wrote, out of its model's window.
 * Context stays available while the Gateway reports both token counts and a model window.
 */
export const CONTEXT_WARNING_AT = 0.75;
export const CONTEXT_NEAR_FULL_AT = 0.9;

export function contextUse(messages: Partial<Pick<ChatSessionMessage, "role" | "usage" | "model">>[], windowOf: (model: string | undefined) => number | undefined) {
  const last = [...messages].reverse().find((m) => m.role === "assistant" && m.usage);
  const window = last ? windowOf(last.model) : undefined;
  if (!last?.usage || !window) return undefined;
  const input = last.usage.promptTokens;
  const output = last.usage.completionTokens;
  const used = input + output;
  return { used, window, share: Math.min(used / window, 1), input, output, model: last.model };
}

/** Tokens as the boards write them: "940", "8.2k", "472k", "1M", "1.2M". */
export function tokens(n: number): string {
  const short = (v: number) => (v < 10 ? String(Math.round(v * 10) / 10) : String(Math.round(v)));
  if (n >= 1_000_000) return `${short(n / 1_000_000)}M`;
  if (n >= 1000) return `${short(n / 1000)}k`;
  return String(n);
}
