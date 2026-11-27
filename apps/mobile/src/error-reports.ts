import type { DiagnosticReport } from "@nexus/kit";
import { enqueue } from "./diagnostics";

/**
 * The error and crash reports the phone app keeps until it can send them, and the switch that stops
 * them. Reports are on until the person turns off "Send crash and error reports" in Settings. When it is off, the
 * app keeps no report, sends none, and deletes the reports it kept. `reporting.tsx` sends what this keeps.
 */

/** The switch: off when it holds `false`, on when it is missing. Not `pref:`, so a sign-out removes it (`forgetAll`). */
export const SWITCH = "consent:error-reports";
/** The reports not yet sent, as JSON. Not `cache:`, `draft:` or `place:`: they are the device's, not a screen's. */
export const PENDING = "diag:pending";

/** expo-sqlite's key-value store, synchronous: a fatal error ends the app right after the report is kept. */
export interface SyncStorage {
  getItemSync(key: string): string | null;
  setItemSync(key: string, value: string): void;
  removeItemSync(key: string): void;
}

export function reportStore(storage: SyncStorage) {
  // Off for this run when storage refused to save the off, so the app sends nothing until it restarts.
  let offUntilRestart = false;

  const on = () => {
    if (offUntilRestart) return false;
    try {
      return storage.getItemSync(SWITCH) !== "false";
    } catch {
      return false;
    }
  };

  const pending = (): DiagnosticReport[] => {
    if (!on()) return [];
    try {
      const raw = storage.getItemSync(PENDING);
      return raw ? (JSON.parse(raw) as DiagnosticReport[]) : [];
    } catch {
      return [];
    }
  };

  const keep = (reports: DiagnosticReport[]) => {
    try {
      if (reports.length) storage.setItemSync(PENDING, JSON.stringify(reports));
      else storage.removeItemSync(PENDING);
    } catch {
      // Storage unavailable: the report is lost, and the app goes on.
    }
  };

  return {
    on,
    pending,

    /**
     * Turns reports on or off. Off also deletes the kept reports. False when storage did not save the change: an off
     * then holds only until the app restarts.
     */
    set(value: boolean): boolean {
      offUntilRestart = false;
      try {
        if (value) storage.removeItemSync(SWITCH);
        else storage.setItemSync(SWITCH, "false");
      } catch {
        // Checked below.
      }
      if (!value) keep([]);
      const saved = on() === value;
      if (!value && !saved) offUntilRestart = true;
      return saved;
    },

    /** Keeps `report` to send. False when reports are off: nothing is kept. */
    record(report: DiagnosticReport): boolean {
      if (!on()) return false;
      keep(enqueue(pending(), report));
      return true;
    },

    /** Removes the reports in `batch`, after the Gateway took them or refused them for good. */
    sent(batch: DiagnosticReport[]) {
      const done = new Set(batch.map((report) => JSON.stringify(report)));
      keep(pending().filter((report) => !done.has(JSON.stringify(report))));
    },
  };
}
