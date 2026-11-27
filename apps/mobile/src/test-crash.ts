/**
 * The forced test crash: `nexus://diagnostics/test-crash?kind=js|native` crashes the app on purpose, after
 * the person agrees, so a store or TestFlight build can show that its crashes reach the observability store. `js`
 * throws an uncaught JavaScript error; `native` stops the app natively (Swift on iOS, SIGABRT on Android), and the crash
 * module reports it at the next start.
 */

export type TestCrashKind = "js" | "native";

/** The test error's message: plain words, so redaction keeps all of it and the log query can match it. */
export const TEST_ERROR = "Forced test error";

/** The link's `kind`, `js` when it names none; `null` for a kind the app does not know. */
export function testCrashKind(value: string | string[] | undefined): TestCrashKind | null {
  const kind = Array.isArray(value) ? value[0] : value;
  if (kind === undefined || kind === "js") return "js";
  return kind === "native" ? "native" : null;
}
