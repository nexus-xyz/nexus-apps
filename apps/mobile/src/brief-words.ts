import { fewerLikeThis, KIND_NAMES, turnOff, type AgentBrief, type AgentProactivityView, type ProactiveKind, type ProactivityChange } from "@nexus/kit";
import { clock } from "./assistant-thread";
import { levelLabel } from "./proactivity-words";

/**
 * The morning brief on the phone (A1, A2·A), without React Native: today's brief is the first message of
 * the thread, the one Slack's DM got that morning (`GET /agent/brief/today`, the kit's `briefRead`). Under it, "Why am
 * I seeing this?" (A10·b) offers Fewer like this and Turn off, as on the web (`agent-proactivity.tsx` there).
 */

/** The time in the brief's "Nexus · 08:02" header, or none when the agent API sent no valid time. */
export function briefTime(brief: Pick<AgentBrief, "at">): string | undefined {
  const at = Date.parse(brief.at);
  return Number.isNaN(at) ? undefined : clock(at);
}

/** What VoiceOver reads for the brief: the head, each line, then the errands. */
export function briefLabel(brief: AgentBrief): string {
  return [brief.head, ...brief.lines, brief.errands].filter(Boolean).join(" ");
}

/** The two ways on from "Why am I seeing this?": the change each makes, and what it says once done. */
export function whyActions(kind: ProactiveKind): { label: string; change: (view: AgentProactivityView) => ProactivityChange; done: (saved: AgentProactivityView) => string }[] {
  return [
    {
      label: "Fewer like this",
      change: (view) => fewerLikeThis(view, kind),
      done: (saved) => (saved.muted.includes(kind) ? `Done: no more ${KIND_NAMES[kind]}.` : `Done: Proactive is set to ${levelLabel(saved.level)}.`),
    },
    { label: `Turn off ${KIND_NAMES[kind]}`, change: (view) => turnOff(view, kind), done: () => `Done: ${KIND_NAMES[kind]} off.` },
  ];
}
