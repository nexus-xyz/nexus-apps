// @nexus/kit times its calls out with AbortSignal.timeout, which Hermes may not have. The reason is an Error named
// TimeoutError, as the standard's DOMException is named, since React Native may not define DOMException.
if (!AbortSignal.timeout) {
  AbortSignal.timeout = (ms: number) => {
    const controller = new AbortController();
    setTimeout(() => controller.abort(Object.assign(new Error("The operation timed out."), { name: "TimeoutError" })), ms);
    return controller.signal;
  };
}

// The kit's Search joins its own signal with a timeout through AbortSignal.any, which Hermes may not have either.
if (!AbortSignal.any) {
  AbortSignal.any = (signals: Iterable<AbortSignal>) => {
    const controller = new AbortController();
    for (const signal of signals) {
      if (signal.aborted) {
        controller.abort(signal.reason);
        break;
      }
      signal.addEventListener("abort", () => controller.abort(signal.reason), { once: true });
    }
    return controller.signal;
  };
}

// The kit's end-to-end encryption reaches crypto.subtle, which Hermes lacks: synced sessions use the
// app's own subset, on noble.
import { getRandomBytes } from "expo-crypto";
import { installSubtle } from "./src/subtle";
installSubtle(getRandomBytes);

import "expo-router/entry";
