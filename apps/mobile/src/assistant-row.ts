import type { AgentConversation, AgentErrandList } from "@nexus/kit";

/**
 * The Assistant's row at the top of the drawer (A21·c, A2·B), without React Native: an unread count, the mark thinking
 * while an answer streams, and a filled dot when something needs the person. Paused is not drawn: the agent API has no
 * paused state yet.
 *
 * Unread counts Nexus's replies in the kept conversation that the Assistant screen has not shown. The screen keeps
 * what it showed per account (`useStored`): the conversation and how many replies it had. Counting replies, not times,
 * keeps the phone's clock out of it. A reply written on another surface (Slack, Nexus UI) into that conversation counts.
 */

/** What the Assistant screen last showed: the kept conversation and how many of Nexus's replies were in it. */
export interface Seen {
  conversation: string;
  answers: number;
}

export interface RowState {
  unread: number;
  needsYou: boolean;
  /** No `Seen` was kept yet: keep this one, so an update does not count old replies as new. */
  baseline?: Seen;
}

/** How many of Nexus's replies a read of the conversation holds. */
export const answersIn = (read: AgentConversation): number => (read.kind === "turns" ? read.turns.filter((turn) => turn.role === "assistant").length : 0);

/**
 * Whether the conversation waits on the person: its last turn is Nexus's and asks for a write (a pending approval).
 * A later turn of the person's means they moved on.
 */
export function waitsOnYou(read: AgentConversation | undefined): boolean {
  if (read?.kind !== "turns") return false;
  const last = read.turns.at(-1);
  return last?.role === "assistant" && !!last.messages?.some((message) => message.kind === "pending_action");
}

/** Whether an errand needs the person (A12), from the errands list the strip reads. Off or failing, none does. */
export const errandNeedsYou = (list: AgentErrandList | undefined): boolean => list?.kind === "list" && list.errands.some((errand) => errand.status === "needs-you");

/**
 * The row's state from the reads the drawer makes when it opens: the kept conversation (`conversation`, its id) and the
 * errands. A read that failed or was not made counts nothing.
 */
export function rowState({ conversation, read, errands, seen }: { conversation?: string; read?: AgentConversation; errands?: AgentErrandList; seen?: Seen }): RowState {
  const needsYou = waitsOnYou(read) || errandNeedsYou(errands);
  if (!conversation || read?.kind !== "turns") return { unread: 0, needsYou };
  const answers = answersIn(read);
  if (!seen) return { unread: 0, needsYou, baseline: { conversation, answers } };
  // The screen moved to another conversation since: what it kept is about that one, so all of this one's are new.
  const unread = seen.conversation === conversation ? answers - seen.answers : answers;
  return { unread: Math.max(0, unread), needsYou };
}

/** The count in the row's pill, as the inbox's: none at zero, 99+ past 99. */
export const unreadPill = (unread: number): string | undefined => (unread > 99 ? "99+" : unread > 0 ? String(unread) : undefined);

/** The row as VoiceOver and TalkBack read it: "Assistant, thinking, needs you, 2 unread". */
export function rowLabel({ unread, needsYou, thinking }: { unread: number; needsYou: boolean; thinking: boolean }): string {
  const pill = unreadPill(unread);
  return ["Assistant", thinking ? "thinking" : undefined, needsYou ? "needs you" : undefined, pill ? `${pill} unread` : undefined].filter(Boolean).join(", ");
}
