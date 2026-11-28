import { describe, expect, it, vi } from "vitest";
import { CHIME_GAP_MS, createChime, soundsOn, type Chime } from "./assistant-chime";
import { menuRows, pickRow, rowDisabled } from "./assistant-menu";
import { dayLabel, layoutThread, timesOf } from "./assistant-texting";

// The Assistant's thread as a session transcript (the Assistant canvas): day markers, groups, sounds and the menu.

const now = new Date(2026, 10, 18, 9, 41).getTime(); // Wednesday 18 November 2026, 09:41.
const at = (day: number, hour: number, minute: number) => new Date(2026, 10, day, hour, minute).getTime();

describe("the day marker", () => {
  it("says today, yesterday, the weekday within a week, then the date", () => {
    expect(dayLabel(at(18, 7, 58), now)).toBe("Today");
    expect(dayLabel(at(17, 16, 2), now)).toBe("Yesterday");
    expect(dayLabel(at(16, 16, 2), now)).toBe("Monday");
    expect(dayLabel(at(12, 9, 0), now)).toBe("Thursday");
    expect(dayLabel(at(11, 9, 0), now)).toBe("11 Nov");
    expect(dayLabel(new Date(2025, 0, 3, 8, 5).getTime(), now)).toBe("3 Jan 2025");
  });
});

describe("the thread's groups", () => {
  it("groups one side's messages in a row, and starts the thread with its day", () => {
    const places = layoutThread(
      [
        { id: "a", side: "person", at: at(18, 8, 0) },
        { id: "b", side: "person", at: at(18, 8, 1) },
        { id: "c", side: "nexus", at: at(18, 8, 1) },
        { id: "d", side: "nexus", at: at(18, 8, 2) },
        { id: "e", side: "person", at: at(18, 8, 3) },
      ],
      now,
    );
    expect(places.map(({ first, last }) => [first, last])).toEqual([
      [true, false],
      [false, true],
      [true, false],
      [false, true],
      [true, true],
    ]);
    expect(places.map((place) => place.day)).toEqual(["Today", undefined, undefined, undefined, undefined]);
  });

  it("marks each new day only, however long the gap inside a day", () => {
    const places = layoutThread(
      [
        { id: "a", side: "person", at: at(17, 23, 50) },
        { id: "b", side: "person", at: at(18, 0, 1) },
        { id: "c", side: "person", at: at(18, 6, 0) },
      ],
      now,
    );
    expect(places.map((place) => place.day)).toEqual(["Yesterday", "Today", undefined]);
    expect(places.map(({ first, last }) => [first, last])).toEqual([
      [true, true],
      [true, false],
      [false, true],
    ]);
  });

  it("never starts a day at a message it cannot date, and compares the next with the last dated one", () => {
    const places = layoutThread(
      [
        { id: "a", side: "nexus" },
        { id: "b", side: "nexus", at: at(18, 8, 0) },
        { id: "c", side: "nexus" },
        { id: "d", side: "nexus", at: at(18, 8, 5) },
      ],
      now,
    );
    expect(places.map((place) => place.day)).toEqual([undefined, "Today", undefined, undefined]);
    expect(places.map((place) => place.last)).toEqual([true, false, false, true]);
  });

  it("dates turns read back by their ids, and leaves out a turn without a valid time", () => {
    expect(
      timesOf([
        { role: "user", text: "Hi", at: "2026-11-18T08:00:00.000Z" },
        { role: "assistant", text: "Hello", at: "not a time" },
      ]),
    ).toEqual({ h0: Date.parse("2026-11-18T08:00:00.000Z") });
  });
});

describe("the sounds", () => {
  const setup = () => {
    const state = { enabled: true, active: true, at: 0 };
    const play = vi.fn(async (_chime: Chime) => undefined);
    const chime = createChime({ enabled: () => state.enabled, active: () => state.active, now: () => state.at, play });
    return { state, play, chime };
  };

  it("stay quiet while muted or in the background", () => {
    const { state, play, chime } = setup();
    state.enabled = false;
    chime("send");
    state.enabled = true;
    state.active = false;
    state.at += 1000;
    chime("receive");
    expect(play).not.toHaveBeenCalled();
    state.active = true;
    state.at += 1000;
    chime("receive");
    expect(play).toHaveBeenCalledWith("receive");
  });

  it("play once for two at the same moment, and never throw", () => {
    const { state, play, chime } = setup();
    chime("send");
    state.at += CHIME_GAP_MS - 1;
    chime("receive");
    expect(play).toHaveBeenCalledTimes(1);
    const broken = createChime({ enabled: () => true, active: () => true, now: () => 0, play: () => { throw new Error("no audio"); } });
    expect(() => broken("send")).not.toThrow();
  });

  it("are on unless the switch was turned off", () => {
    expect(soundsOn(undefined)).toBe(true);
    expect(soundsOn(true)).toBe(true);
    expect(soundsOn(false)).toBe(false);
  });
});

describe("the menu", () => {
  const where = (groups: ReturnType<typeof menuRows>) => groups.map((rows) => rows.map((row) => row.action ?? row.href));

  it("lists the thread's actions, the Assistant's pages, then its settings", () => {
    expect(where(menuRows({ history: true, conversation: true, errands: true }))).toEqual([
      ["history", "new"],
      ["/errands", "/assistant-projects"],
      ["/assistant-settings"],
    ]);
  });

  it("leaves out History without shared conversations, New on an empty thread, and the pages without errands", () => {
    expect(where(menuRows({ history: false, conversation: false, errands: false }))).toEqual([["/assistant-settings"]]);
    expect(where(menuRows({ history: false, conversation: true, errands: false }))).toEqual([["new"], ["/assistant-settings"]]);
  });

  it("runs the thread's actions, and opens a page's row", () => {
    const [[history, fresh], [errands]] = menuRows({ history: true, conversation: true, errands: true });
    const onAction = vi.fn();
    const open = vi.fn();
    pickRow(fresh, onAction, open);
    pickRow(history, onAction, open);
    pickRow(errands, onAction, open);
    expect(onAction.mock.calls).toEqual([["new"], ["history"]]);
    expect(open.mock.calls).toEqual([["/errands"]]);
  });

  it("dims History and New conversation while an answer runs, never a page", () => {
    const [[history, fresh], [errands], [settings]] = menuRows({ history: true, conversation: true, errands: true });
    expect([history, fresh, errands, settings].map((row) => rowDisabled(row, true))).toEqual([true, true, false, false]);
    expect([history, fresh].map((row) => rowDisabled(row, false))).toEqual([false, false]);
  });
});
