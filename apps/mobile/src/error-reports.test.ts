import { describe, expect, it } from "vitest";
import { fromError, type Device } from "./diagnostics";
import { PENDING, reportStore, SWITCH, type SyncStorage } from "./error-reports";

const DEVICE: Device = { appVersion: "1.0.0", appBuild: "1", platform: "ios", osVersion: "26.1" };
const report = (message: string) => fromError(new Error(message), false, DEVICE, new Date("2026-11-06T00:00:00Z"));

/** The device's key-value store, as a map; `refuse` makes every write throw, as a full disk does. */
function device(entries: Record<string, string> = {}) {
  const kept = new Map(Object.entries(entries));
  let refuse = false;
  const write = () => {
    if (refuse) throw new Error("disk full");
  };
  const storage: SyncStorage = {
    getItemSync: (key) => kept.get(key) ?? null,
    setItemSync: (key, value) => (write(), kept.set(key, value)),
    removeItemSync: (key) => (write(), kept.delete(key)),
  };
  return { storage, kept, refuseWrites: () => (refuse = true) };
}

describe("error-reports", () => {
  it("is on by default, and keeps each report to send", () => {
    const { storage } = device();
    const reports = reportStore(storage);
    expect(reports.on()).toBe(true);
    expect(reports.record(report("a"))).toBe(true);
    expect(reports.record(report("b"))).toBe(true);
    expect(reports.pending().map((r) => r.message)).toEqual(["a", "b"]);
    reports.sent(reports.pending().slice(0, 1));
    expect(reports.pending().map((r) => r.message)).toEqual(["b"]);
  });

  it("off deletes the kept reports, keeps no new one and sends none", () => {
    const { storage, kept } = device();
    const reports = reportStore(storage);
    reports.record(report("a"));
    expect(reports.set(false)).toBe(true);
    expect(kept.has(PENDING)).toBe(false);
    expect(kept.get(SWITCH)).toBe("false");
    expect(reports.record(report("b"))).toBe(false);
    expect(reports.pending()).toEqual([]);
    expect(kept.has(PENDING)).toBe(false);
  });

  it("stays off after a restart, and is on again when the switch is removed, as a sign-out does", () => {
    const { storage, kept } = device({ [SWITCH]: "false" });
    expect(reportStore(storage).on()).toBe(false);
    kept.delete(SWITCH);
    expect(reportStore(storage).on()).toBe(true);
  });

  it("sends nothing it kept before the switch went off on another run", () => {
    const { storage } = device({ [SWITCH]: "false", [PENDING]: JSON.stringify([report("old")]) });
    expect(reportStore(storage).pending()).toEqual([]);
  });

  it("on again removes the switch and keeps reports again", () => {
    const { storage, kept } = device({ [SWITCH]: "false" });
    const reports = reportStore(storage);
    expect(reports.set(true)).toBe(true);
    expect(kept.has(SWITCH)).toBe(false);
    expect(reports.record(report("a"))).toBe(true);
  });

  it("an off that storage refused holds until the app restarts, and says it was not saved", () => {
    const { storage, refuseWrites } = device();
    const reports = reportStore(storage);
    reports.record(report("a"));
    refuseWrites();
    expect(reports.set(false)).toBe(false);
    expect(reports.on()).toBe(false);
    expect(reports.record(report("b"))).toBe(false);
    expect(reports.pending()).toEqual([]);
    // The next start reads the stored switch, which is still on.
    expect(reportStore(storage).on()).toBe(true);
  });

  it("is off when the switch cannot be read, so nothing is sent that the person may have stopped", () => {
    const storage: SyncStorage = {
      getItemSync: () => {
        throw new Error("no storage");
      },
      setItemSync: () => undefined,
      removeItemSync: () => undefined,
    };
    expect(reportStore(storage).on()).toBe(false);
  });
});
