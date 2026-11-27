import { diagnostics } from "@nexus/kit";

/** The phone app's names for the apps' shared report builder (`kit/src/diagnostics.ts`). */
export const { enqueue, fromCrash, fromError, frames, redact, redactShapes, JS_FRAME, MAX_PENDING, NATIVE_FRAME } = diagnostics;
export type Device = diagnostics.Device;
export type NativeCrash = diagnostics.NativeCrash;
