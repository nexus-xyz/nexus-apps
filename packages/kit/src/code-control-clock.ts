/**
 * Gateway's clock for phone control. A phone signs a command's expiry, and Gateway checks it by its own
 * clock. `CoreClient.codeControl` records the offset between the two clocks on the answer it returns, so a phone with a
 * skewed clock still seals an expiry that Gateway accepts.
 */
import type { CodeControl } from "./code-control-types";

/** How far apart two clocks may be when one checks an expiry the other signed. Gateway allows the same. */
export const CLOCK_SKEW_MS = 30_000;

/** Gateway's clock minus this device's clock, per answer. Not a property: the wire type stays as the contract says. */
const offsets = new WeakMap<CodeControl, number>();

/**
 * Records Gateway's clock offset for this control answer, and returns the same object. Keep that object: a copy has no
 * offset, and `controlNow` then reads this device's clock. An answer without a valid `serverTime` records nothing.
 */
export function recordClockOffset(control: CodeControl, receivedAt = Date.now()): CodeControl {
  const { serverTime } = control;
  if (typeof serverTime === "number" && Number.isSafeInteger(serverTime)) offsets.set(control, serverTime - receivedAt);
  return control;
}

/** Gateway's clock now, estimated from a recorded `control`. This device's clock when nothing was recorded. */
export function controlNow(control: CodeControl | undefined, local = Date.now()): number {
  const offset = control && offsets.get(control);
  return offset === undefined ? local : local + offset;
}
