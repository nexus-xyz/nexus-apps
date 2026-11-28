import { CoreError, type createCoreClient, type DiagnosticReport } from "@nexus/kit";
import { requireOptionalNativeModule } from "expo";
import * as Application from "expo-application";
import Constants from "expo-constants";
import { ErrorBoundary as RouterErrorBoundary, type ErrorBoundaryProps } from "expo-router";
import { Storage } from "./store";
import { useEffect } from "react";
import { AppState, Platform } from "react-native";
import { fromCrash, fromError, type Device, type NativeCrash } from "./diagnostics";
import { reportStore } from "./error-reports";

/**
 * Errors and crashes reach the observability store. A JavaScript error is written to the device at once,
 * synchronously, since a fatal one ends the app right after. A native crash is kept by the OS and handed over at the
 * next start (`modules/crash-reports`: MetricKit on iOS, `ApplicationExitInfo` on Android). Both are sent through the
 * signed-in Gateway (`POST /v1/diagnostics/reports`), which writes each one to its log. Reports wait on the device until a sign-in can
 * send them, and go with the sign-in like the rest of the store (`forgetAll`). `@nexus/kit`'s `diagnostics` builds them.
 * The person can turn them off in Settings (`error-reports.ts`): the app then keeps and sends none, and drops
 * the OS's crashes as it takes them.
 */

type Core = ReturnType<typeof createCoreClient>;

/** The OS's crashes, through the app's own module. Missing in Expo Go, where it takes nothing. */
const crashes = requireOptionalNativeModule<{ take(): Promise<NativeCrash[]> }>("NexusCrashReports");

/** The reports kept to send, and the switch that stops them. */
const reports = reportStore(Storage);
/** How long after a non-fatal error the app sends it, so a burst goes as one request. */
const SEND_AFTER_MS = 2_000;
/** MetricKit may deliver a crash a few seconds after the start's first send: look once more after this long. */
const LOOK_AGAIN_MS = 10_000;

/** Where a report or feedback comes from: this app, its build and the OS, read once at start. */
export const device: Device = {
  // app.json's version, which a store build carries too; Expo Go's own would be the native one.
  appVersion: Constants.expoConfig?.version ?? Application.nativeApplicationVersion ?? "unknown",
  appBuild: __DEV__ ? "dev" : Application.nativeBuildVersion,
  platform: Platform.OS === "android" ? "android" : "ios",
  osVersion:
    Platform.OS === "android" ? String((Platform.constants as { Release?: string }).Release ?? Platform.Version) : String(Platform.Version),
};

let client: Core | null = null;
let sending = false;
let timer: ReturnType<typeof setTimeout> | undefined;

/** The OS's new crashes. Missing module or a failed take: none. */
async function takeCrashes(): Promise<NativeCrash[]> {
  return (await crashes?.take().catch(() => [])) ?? [];
}

function record(report: DiagnosticReport) {
  if (!reports.record(report) || report.fatal || timer) return;
  timer = setTimeout(() => {
    timer = undefined;
    void send();
  }, SEND_AFTER_MS);
}

/** `error` reported, never thrown from: a reporter that throws would keep React Native's own handler from running. */
function report(error: unknown, fatal: boolean) {
  try {
    record(fromError(error, fatal, device));
  } catch {
    // Nothing is reported; the app goes on as it would without the reporter.
  }
}

/**
 * The kept reports and the OS's new crashes, sent; kept for the next try when the Gateway cannot take them now. With
 * reports off, the OS's crashes are taken and dropped, signed in or not, so none waits on the device.
 */
async function send() {
  if (sending) return;
  if (!reports.on()) {
    await takeCrashes();
    return;
  }
  if (!client) return;
  const core = client;
  sending = true;
  try {
    for (const crash of await takeCrashes()) record(fromCrash(crash, device));
    const batch = reports.pending();
    if (!batch.length) return;
    try {
      await core.sendDiagnosticReports(batch);
    } catch (error) {
      // A 400 is a report the Gateway will never take: drop it rather than send it forever. Anything else waits.
      if (!(error instanceof CoreError && error.status === 400)) return;
    }
    reports.sent(batch);
  } finally {
    sending = false;
  }
}

/** Whether the app sends error and crash reports: Settings' switch. */
export const errorReportsOn = () => reports.on();

/**
 * Turns error and crash reports on or off. Off deletes the kept reports and the OS's crashes not yet taken. False when
 * the device did not save the change; an off then holds until the app restarts.
 */
export function setErrorReports(on: boolean): boolean {
  const saved = reports.set(on);
  if (!on) {
    clearTimeout(timer);
    timer = undefined;
    void takeCrashes();
  }
  return saved;
}

/** Hermes's hook for promise rejections nothing handled. */
type Hermes = {
  enablePromiseRejectionTracker?(options: { allRejections: boolean; onUnhandled(id: number, error: unknown): void }): void;
};

/**
 * Every uncaught JavaScript error, fatal or not, is kept before React Native's own handler runs. So is every promise
 * rejection nothing handled, in a release build: React Native tracks them only in development, where they show as a
 * warning, and this would replace that.
 */
export function installReporting() {
  // On the global object, not this module: Fast Refresh runs both again, and a second wrapper keeps each error twice.
  const state = globalThis as { nexusReporting?: boolean; HermesInternal?: Hermes };
  if (state.nexusReporting) return;
  state.nexusReporting = true;
  const previous = ErrorUtils.getGlobalHandler();
  ErrorUtils.setGlobalHandler((error, fatal) => {
    report(error, !!fatal);
    previous(error, fatal);
  });
  if (!__DEV__) {
    state.HermesInternal?.enablePromiseRejectionTracker?.({ allRejections: true, onUnhandled: (_id, error) => report(error, false) });
  }
}

/** Sends what is kept when a sign-in is there, and again each time the app comes back to the front. */
export function useReporting(core: Core | null | undefined) {
  useEffect(() => {
    client = core ?? null;
    void send();
    const again = setTimeout(() => void send(), LOOK_AGAIN_MS);
    const listener = AppState.addEventListener("change", (state) => state === "active" && void send());
    return () => {
      clearTimeout(again);
      listener.remove();
    };
  }, [core]);
}

/** A render error shows Expo Router's error screen, with Retry, and is reported once. */
export function ErrorBoundary(props: ErrorBoundaryProps) {
  useEffect(() => report(props.error, false), [props.error]);
  return <RouterErrorBoundary {...props} />;
}
