import type { TimelineItem } from "./code-timeline";

export const HANDOFF_LIMIT = 12_000;

export function handoffPrompt(items: readonly TimelineItem[]): string {
  const recent = items.flatMap((item) => {
    if (item.kind !== "user" && item.kind !== "text") return [];
    const content = item.text.trim();
    if (!content) return [];
    return [`${item.kind === "user" ? "I asked" : "The agent replied"}\n${content.slice(0, 1_750)}${content.length > 1_750 ? "…" : ""}`];
  }).slice(-6);
  if (!recent.length) return "";
  return ["Continue this work using the context below. Ask me for any missing details.", ...recent].join("\n\n");
}
