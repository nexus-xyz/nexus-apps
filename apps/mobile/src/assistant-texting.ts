import type { AgentTurn } from "@nexus/kit";

/**
 * The Assistant's thread laid out as a session transcript (ASSISTANT.md, "A thread that never ends"), without React
 * Native. A day marker starts the thread and each new day: Today, Yesterday, the weekday within the last week, then the
 * date. Messages from one side in a row form a group and sit closer than a change of speaker; a day marker ends the
 * group above it.
 */

/** One message of the thread: whose it is and when it was sent, when the screen knows. */
export type ThreadItem = { id: string; side: "person" | "nexus"; at?: number };

/** Where one message sits. `first` and `last`: its place in its group. `day`: the day marker above it. */
export type ThreadPlace = { id: string; first: boolean; last: boolean; day?: string };

const sameDay = (a: Date, b: Date) => a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();

const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** The day marker's words: "Today", "Yesterday", "Monday", then "4 Nov", with the year when it is not this one's. */
export function dayLabel(at: number, now: number): string {
  const date = new Date(at);
  const today = new Date(now);
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  const weekAgo = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 6);
  if (sameDay(date, today)) return "Today";
  if (sameDay(date, yesterday)) return "Yesterday";
  if (date >= weekAgo && date < today) return WEEKDAYS[date.getDay()];
  const year = date.getFullYear() === today.getFullYear() ? "" : ` ${date.getFullYear()}`;
  return `${date.getDate()} ${MONTHS[date.getMonth()]}${year}`;
}

/**
 * Each message's place in the thread. A message with no time (one the screen cannot date) never starts a day, and the
 * next dated message is compared with the last dated one before it.
 */
export function layoutThread(items: ThreadItem[], now: number): ThreadPlace[] {
  let lastAt: number | undefined;
  const places: ThreadPlace[] = items.map((item) => {
    let day: string | undefined;
    if (item.at !== undefined) {
      if (lastAt === undefined || !sameDay(new Date(item.at), new Date(lastAt))) day = dayLabel(item.at, now);
      lastAt = item.at;
    }
    return { id: item.id, first: true, last: true, day };
  });
  for (let i = 1; i < items.length; i += 1) {
    if (items[i].side === items[i - 1].side && !places[i].day) {
      places[i].first = false;
      places[i - 1].last = false;
    }
  }
  return places;
}

/** The times of turns read back, by the ids `entriesOf` gives them (`h<index>`). A turn with no valid time is left out. */
export function timesOf(turns: AgentTurn[]): Record<string, number> {
  const times: Record<string, number> = {};
  turns.forEach((turn, i) => {
    const at = Date.parse(turn.at);
    if (Number.isFinite(at)) times[`h${i}`] = at;
  });
  return times;
}
