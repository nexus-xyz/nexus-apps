import { AGENT_ERRAND_PROJECT_MAX, errandName, type AgentErrand, type AgentErrandAction, type AgentErrandActed, type AgentErrandEdit, type AgentErrandList, type AgentErrandView, type AgentMessage } from "@nexus/kit";

/**
 * Errands on the phone's Assistant (A12), without React Native: which errands sit on top of the chat, each
 * chip's words, where a card or a question waits, and what each read of the agent API makes of the strip. Each errand
 * is its own thread: its entries, its long-press menu (A2·B), the History and the main chat's overview.
 * The words are Nexus UI's (`agent-errands.ts` there), so the web and the phone say the same thing.
 */

/** The build shows errands with `EXPO_PUBLIC_NEXUS_AGENT_ERRANDS=on`. */
export const errandsOn = (flag: string | undefined) => flag?.trim().toLowerCase() === "on";

/** The strip on top of the chat: nothing yet, nothing to show (off, failed, or no open errand), or its errands. */
export type Strip = { kind: "unknown" } | { kind: "hidden" } | { kind: "on"; errands: AgentErrand[] };

/**
 * What a read makes of the strip: the open errands that are not archived, in the API's order, a pinned one first. A
 * failure hides it.
 */
export function stripAfter(read: AgentErrandList | "unreachable"): Strip {
  if (read === "unreachable" || read.kind !== "list") return { kind: "hidden" };
  const open = read.errands.filter((errand) => (errand.status === "needs-you" || errand.status === "running") && !errand.archived);
  const shown = [...open.filter((errand) => errand.pinned), ...open.filter((errand) => !errand.pinned)];
  return shown.length ? { kind: "on", errands: shown } : { kind: "hidden" };
}

/** What an errand needs, or how far it went, in words: what VoiceOver reads after a chip's goal. */
export function chipLine(errand: AgentErrand): string {
  if (errand.need === "confirm") return "Confirm it";
  if (errand.need === "approval") return "Needs your approval";
  if (errand.need === "question") return "Asked you";
  return errand.steps ? `${errand.steps} ${errand.steps === 1 ? "step" : "steps"}` : "Started";
}

/**
 * The quiet word after a chip's goal (A2·A, "Gift for Ana · 3 ideas"): only a running errand's progress. An errand that
 * needs you says so by its filled dot alone.
 */
export const chipDetail = (errand: AgentErrand): string | undefined => (errand.status === "running" && errand.steps ? chipLine(errand) : undefined);

/** A chip as VoiceOver and TalkBack read it: "Dinner Friday, errand, needs your approval." */
export const chipLabel = (errand: AgentErrand): string => `${errandName(errand)}, errand, ${chipLine(errand).toLowerCase()}`;

const SURFACES: Record<string, string> = { slack: "Slack", telegram: "Telegram", api: "Nexus" };

/** Where an errand started, as the person reads it. */
export const surfaceName = (slug: string): string => SURFACES[slug] ?? slug;

/** Where the person answers what it needs, when the answer is not on the phone; undefined for a draft or nothing. */
export function whereToAnswer(view: Pick<AgentErrandView, "errand" | "surface">): string | undefined {
  const where = surfaceName(view.surface);
  if (view.errand.need === "approval") return view.surface === "api" ? "The change waits for your approval in Nexus." : `The change waits for your approval in ${where}. Approve or cancel it there.`;
  if (view.errand.need === "question") return view.surface === "api" ? "Reply in the chat where you asked." : `Reply in its ${where} thread.`;
  return undefined;
}

/** Each action's button. */
export const ACTION_LABEL: Record<AgentErrandAction, string> = { confirm: "Confirm", done: "Mark done", cancel: "Cancel errand" };

/** What the screen says once an action is done. */
export function actedText(acted: AgentErrandActed): string {
  if (acted.state === "confirmed") return "Confirmed. Nexus works on it now.";
  if (acted.state === "done") return "Marked done.";
  if (acted.state === "cancelled") return "Cancelled. Nexus stopped working on it.";
  return "The draft is deleted.";
}

/** A deadline (`YYYY-MM-DD`, UTC) as the person reads it, against `now`. */
export function deadlineText(day: string, now: number = Date.now()): string {
  const today = new Date(now).toISOString().slice(0, 10);
  if (day < today) return "Past its deadline";
  if (day === today) return "Due today";
  if (day === new Date(now + 86_400_000).toISOString().slice(0, 10)) return "Due tomorrow";
  return `Due ${new Date(`${day}T00:00:00Z`).toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" })}`;
}

/** What the errand needs, or how it went, in a few words under its name: the overview's and the History's line. */
export function errandLine(errand: AgentErrand): string {
  if (errand.need === "confirm") return "Confirm it to start";
  if (errand.need === "approval") return errand.detail ? `Approve: ${errand.detail}` : "A change waits for your approval";
  if (errand.need === "question") return "It asked you something";
  if (errand.status === "running") return errand.steps ? `${errand.steps} ${errand.steps === 1 ? "step" : "steps"} so far` : "Started";
  if (errand.status === "failed") return "It did not finish";
  return errand.steps ? `Done in ${errand.steps} ${errand.steps === 1 ? "step" : "steps"}` : "Done";
}

/** How a closed errand ended, from the record's own word. */
export function outcomeText(status: string): string {
  if (status === "partly_done") return "Partly done";
  if (status === "failed") return "It did not finish";
  if (status === "cancelled") return "Cancelled";
  return "Done";
}

/** The errand's state in a few words, beside its name in the thread's bar. */
export function errandStateText(view: Pick<AgentErrandView, "errand" | "outcome">): string {
  if (view.outcome) return outcomeText(view.outcome.status);
  if (view.errand.archived) return "Archived";
  if (view.errand.status === "needs-you") return "Needs you";
  if (view.errand.status === "running") return "Working on it";
  return outcomeText(view.errand.status);
}

/** The composer's placeholder: "Reply about <the errand's name>", cut to fit the phone's composer. */
export function replyPlaceholder(errand: Pick<AgentErrand, "goal" | "title">): string {
  const name = errandName(errand);
  return `Reply about ${name.length > 32 ? `${name.slice(0, 31)}…` : name}`;
}

/**
 * One entry of an errand's thread, as the phone draws it: the person's goal as their first message, a step the agent
 * took as a quiet note, a reply of the person's, or the agent's messages. `at` is in milliseconds.
 */
export type ErrandEntry =
  | { kind: "you"; key: string; text: string; at: number }
  | { kind: "step"; key: string; text: string; urls: string[]; at: number }
  | { kind: "nexus"; key: string; messages: AgentMessage[]; at: number };

const msOf = (iso: string): number => {
  const at = Date.parse(iso);
  return Number.isNaN(at) ? 0 : at;
};

/** The thread of an errand: its goal, its steps and its turns, oldest first. A turn with no messages shows its text. */
export function errandEntries(view: Pick<AgentErrandView, "errand" | "createdAt" | "steps" | "thread">): ErrandEntry[] {
  const entries: ErrandEntry[] = [
    { kind: "you", key: "goal", text: view.errand.goal, at: msOf(view.createdAt) },
    ...view.steps.map((step): ErrandEntry => ({ kind: "step", key: `s-${step.id}`, text: step.text, urls: step.urls, at: msOf(step.at) })),
    ...view.thread.map((turn, i): ErrandEntry =>
      turn.role === "user"
        ? { kind: "you", key: `t${i}`, text: turn.text, at: msOf(turn.at) }
        : { kind: "nexus", key: `t${i}`, messages: turn.messages ?? [{ kind: "answer", text: turn.text, model: "", costUsd: Number.NaN }], at: msOf(turn.at) },
    ),
  ];
  // A stable sort: entries at the same time keep the order above.
  return entries
    .map((entry, i) => ({ entry, i }))
    .sort((a, b) => a.entry.at - b.entry.at || a.i - b.i)
    .map(({ entry }) => entry);
}

/** One item of an errand's long-press menu. */
export type ErrandMenuItem = { id: "pin" | "rename" | "move" | "done" | "archive"; label: string };

/**
 * An errand's long-press menu, in A2·B's order: Pin, Rename, Move to project (A14), Mark done, then Archive.
 * Mark done shows only while the errand is open. A2's Mute for a day waits for its backend.
 */
export function errandMenu(errand: Pick<AgentErrand, "status" | "pinned" | "archived">): ErrandMenuItem[] {
  const open = (errand.status === "needs-you" || errand.status === "running") && !errand.archived;
  return [
    { id: "pin", label: errand.pinned ? "Unpin" : "Pin" },
    { id: "rename", label: "Rename" },
    { id: "move", label: "Move to project" },
    ...(open ? [{ id: "done" as const, label: "Mark done" }] : []),
    { id: "archive", label: errand.archived ? "Move out of the archive" : "Archive" },
  ];
}

/** The edit that a menu item makes, for each item that needs no more input. */
export function menuEdit(id: Exclude<ErrandMenuItem["id"], "rename" | "move">, errand: Pick<AgentErrand, "pinned" | "archived">): AgentErrandEdit {
  if (id === "pin") return { pinned: !errand.pinned };
  if (id === "done") return { done: true };
  return { archived: !errand.archived };
}

/** The edit that Rename makes: the new name, or `null` to drop it when the field is empty. */
export function renameEdit(title: string): AgentErrandEdit {
  const name = title.trim();
  return { title: name ? name : null };
}

/** The edit that Move to project makes: into a project by its name (cut to the API's limit), or out of one (`null`). */
export function moveEdit(project: string | null): AgentErrandEdit {
  const name = project?.trim().slice(0, AGENT_ERRAND_PROJECT_MAX);
  return { project: name ? name : null };
}

/** Where a filed errand is, under its name: "in Fundraise". */
export function inProject(errand: Pick<AgentErrand, "project">): string | undefined {
  return errand.project ? `in ${errand.project}` : undefined;
}

/** What the screen says once an errand moved: into a project, or out of one (Nexus UI's words). */
export function movedText(project: string | null | undefined, errand?: Pick<AgentErrand, "goal" | "title">): string {
  const name = errand ? errandName(errand) : "The errand";
  return project ? `${name} is in ${project} now.` : `${name} is loose again, under the Assistant.`;
}

/** What the screen says once an edit is done. */
export function editedText(edit: AgentErrandEdit): string {
  if (edit.project !== undefined) return movedText(edit.project);
  if (edit.done) return "Marked done.";
  if (edit.archived === true) return "Archived. It is in the history now.";
  if (edit.archived === false) return "Back in your errands.";
  if (edit.pinned === true) return "Pinned.";
  if (edit.pinned === false) return "Unpinned.";
  return edit.title === null ? "The name is gone." : "Renamed.";
}

/** The Errands page's groups, each in the API's order: Needs you, Running, then Done this week. */
export function errandGroups(errands: readonly AgentErrand[]): { title: string; errands: AgentErrand[] }[] {
  const groups = [
    { title: "Needs you", errands: errands.filter((errand) => errand.status === "needs-you") },
    { title: "Running", errands: errands.filter((errand) => errand.status === "running") },
    { title: "Done this week", errands: errands.filter((errand) => errand.status === "done" || errand.status === "failed") },
  ];
  return groups.filter((group) => group.errands.length > 0);
}

/** The main chat's overview card (A12·c): "2 errands need you", or undefined when none does. */
export function overviewTitle(count: number): string | undefined {
  if (count <= 0) return undefined;
  return `${count} ${count === 1 ? "errand needs" : "errands need"} you`;
}

/** The most errands that the overview card names. The Errands page has the rest. */
export const OVERVIEW_ROWS = 3;

/** A History row's line: "Archived · Done in 3 steps", or how it went, after where it is filed. */
export function historyLine(errand: AgentErrand): string {
  return [inProject(errand), errand.archived ? `Archived · ${errandLine(errand)}` : errandLine(errand)].filter(Boolean).join(" · ");
}
