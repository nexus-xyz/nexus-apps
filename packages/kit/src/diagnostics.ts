import type { DiagnosticPlatform, DiagnosticReport } from "./api";

/**
 * The apps' error and crash reports, built here with no content: no prompt, answer, code or
 * token. A report keeps the error's name, the first line of its message and its top frames, each redacted as the
 * Gateway redacts a log line (`eng/factory/rs/crates/nexus-base/src/redact.rs`, whose rules `redact` repeats), and the
 * app's and OS's versions. The phone app (`mobile/src/reporting.tsx`), the desktop app and Nexus UI
 * (`ui/src/components/nexus-ui/reporting.tsx`) keep them and send them to `POST /v1/diagnostics/reports`, which writes
 * each one to the Gateway's log, and so to the observability store. Exported from `@nexus/kit` as `diagnostics`.
 */

/** Where a report comes from, read once at start. */
export interface Device {
  appVersion: string;
  appBuild?: string | null;
  platform: DiagnosticPlatform;
  osVersion: string;
}

/**
 * A native crash seen at the next start: MetricKit's on iOS and `ApplicationExitInfo`'s on Android
 * (`mobile/modules/crash-reports`), a panic on the desktop.
 */
export interface NativeCrash {
  /** The exception and signal: `EXC_CRASH (SIGABRT)`, or an Objective-C exception's name. */
  name: string;
  /** The exception's reason or the termination reason. */
  message: string;
  /** The crashed thread's top frames: a binary and its offset. */
  stack: string[];
  occurredAt: string;
  appBuild?: string | null;
  osVersion?: string | null;
}

/**
 * The most reports the device keeps until the Gateway takes them: the API's batch (`MAX_DIAGNOSTIC_REPORTS`). The
 * newest stay. `CrashStore.limit` in `modules/crash-reports/ios/CrashReportsModule.swift` keeps as many crashes.
 */
export const MAX_PENDING = 20;
/** The API's bounds (`nexus_types::api::diagnostics`). */
const MAX_FRAMES = 64;
const MAX_FRAME_CHARS = 300;
const MAX_NAME_CHARS = 120;
const MAX_VERSION_CHARS = 40;

/** The redacted text's most characters, as the Gateway's. */
const MAX_CHARS = 200;
/** A word this long that mixes letters and digits is read as a token. */
const TOKEN_CHARS = 20;
const REDACTED = "[redacted]";
/** A file's path names the person's folders and projects (`/Users/ana/acme-merger/…`): stricter than the Gateway. */
const PATH = "[path]";
const QUOTED = "[quoted]";
const QUOTES = ["'", '"', "`"];
const EDGES = "()[]{}<>,.;:!?'\"`";
/** Key starts the shapes do not cover (shorter keys, other providers), lowercased. */
const KEY_STARTS = ["sk-", "sk_", "pk_", "rk_", "nxs_", "nxsa_", "nxr_", "nxo_"];
/** The characters after which a quote opens inside a word: `content='…'`, `content:"…"`. */
const QUOTE_AFTER = ["=", ":", "("];
/** Words that carry a secret in themselves, lowercased. */
const SECRET_PARTS = ["key=", "token=", "secret=", "password=", "signature="];
/** Nexus tokens, provider and Stripe keys, GitHub tokens, PEM blocks, JWTs. */
const SECRET_SHAPES = [
  /\bnx(?:sa|[sdro])?_[A-Za-z0-9_.-]{16,}/g,
  /\bsk-[A-Za-z0-9_-]{12,}/g,
  /\bgh[pousr]_[A-Za-z0-9]{16,}/g,
  /github_pat_[A-Za-z0-9_]{22,}/g,
  /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/g,
  /\b[rs]k_(?:live|test)_[A-Za-z0-9]{8,}/g,
  /\bwhsec_[A-Za-z0-9+/=_-]{8,}/g,
  /\b(?:pi|seti|cs)_(?:(?:live|test)_)?[A-Za-z0-9]+_secret_[A-Za-z0-9]+/g,
];
/** Control characters, which the API refuses (Rust's `char::is_control`). */
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/g;
const TIME = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,9})?(Z|[+-]\d{2}:\d{2})$/;
/** The most of a message read before redaction, which cuts it to 200 characters. */
const MAX_SOURCE_CHARS = 2_000;
/**
 * The two shapes a frame may take, and the Gateway's (`handlers/diagnostics.rs`): a JavaScript frame,
 * `at <function> (<file>:<line>:<column>)`, `at <function> (native)` or `at <file>:<line>:<column>`, and a native frame,
 * `<binary> +0x<offset>`. A message line that starts with `at ` is not a frame.
 */
export const JS_FRAME = /^at (?:(?:new |async )?[^\s()]+ \((?:[^\s()]+:\d+:\d+|native)\)|[^\s()]+:\d+:\d+)$/;
export const NATIVE_FRAME = /^[^()]{1,120} \+0x[0-9a-f]{1,16}$/;

/** `text` with every match of the secret shapes replaced: for a stack frame, a code location and not prose. */
export function redactShapes(text: string): string {
  let shaped = redactPem(text);
  for (const shape of SECRET_SHAPES) shaped = shaped.replace(shape, REDACTED);
  return shaped;
}

const PEM_BEGIN = /-----BEGIN [A-Z ]*PRIVATE KEY-----/g;
const PEM_END = /-----END [A-Z ]*PRIVATE KEY-----/g;

/**
 * `text` with each PEM private key block, from its BEGIN line to the first END line after it, replaced. One pass: a
 * single lazy regex rescans to the end of the text from every BEGIN that has no END.
 */
function redactPem(text: string): string {
  let out = "";
  let from = 0;
  PEM_BEGIN.lastIndex = 0;
  for (let begin = PEM_BEGIN.exec(text); begin; begin = PEM_BEGIN.exec(text)) {
    PEM_END.lastIndex = PEM_BEGIN.lastIndex;
    if (!PEM_END.exec(text)) break;
    out += text.slice(from, begin.index) + REDACTED;
    from = PEM_BEGIN.lastIndex = PEM_END.lastIndex;
  }
  return out + text.slice(from);
}

/** `text` with every key, token, URL path and query, email, quoted span and file path replaced, cut at 200 characters. */
export function redact(text: string): string {
  const shaped = redactShapes(text);
  const out: string[] = [];
  let quote: string | null = null;
  let afterBearer = false;
  for (const word of shaped.split(/\s+/)) {
    if (!word) continue;
    let safe: string;
    if (quote) {
      if (closes(word, quote)) quote = null;
      continue;
    }
    const opening = openingQuote(word);
    if (opening) {
      const [at, open] = opening;
      if (!closes(word.slice(at + 1), open)) quote = open;
      safe = `${word.slice(0, at)}${QUOTED}`;
    } else if (afterBearer) {
      safe = REDACTED;
    } else if (word.includes("://")) {
      safe = origin(word, word.indexOf("://"));
    } else if (secret(word)) {
      safe = REDACTED;
    } else if (filePath(word)) {
      safe = PATH;
    } else {
      safe = word;
    }
    afterBearer = word.toLowerCase() === "bearer";
    out.push(safe);
  }
  return cut(out.join(" "), MAX_CHARS, "…");
}

/** Where a quote opens in `word`, and which: at its start, or right after `=`, `:` or `(`. */
function openingQuote(word: string): [number, string] | null {
  let previous: string | null = null;
  for (let at = 0; at < word.length; at++) {
    const c = word.charAt(at);
    if (QUOTES.includes(c) && (previous === null || QUOTE_AFTER.includes(previous))) return [at, c];
    previous = c;
  }
  return null;
}

/** Whether `word` ends the quote `open` began. */
function closes(word: string, open: string): boolean {
  return word.replace(/[,.;:!?)\]}]+$/, "").endsWith(open);
}

/**
 * Whether `word` looks like a file's path: from the root, the home folder or a drive (`/Users/…`, `~/…`, `C:\\…`), with
 * a backslash, or two folders deep (`a/b/c`). "and/or" is not one.
 */
function filePath(word: string): boolean {
  const bare = trimEdges(word);
  return /^(~|\.{1,2})?\/[^/\s]/.test(bare) || /^[A-Za-z]:[\\/]/.test(bare) || bare.includes("\\") || (bare.match(/\//g) ?? []).length >= 2;
}

/** Whether `word` looks like a key, a token or an email. */
function secret(word: string): boolean {
  const bare = trimEdges(word);
  const lower = bare.toLowerCase();
  return (
    KEY_STARTS.some((start) => lower.startsWith(start)) ||
    SECRET_PARTS.some((part) => lower.includes(part)) ||
    (bare.includes("@") && bare.includes(".")) ||
    (Array.from(bare).length >= TOKEN_CHARS && /[0-9]/.test(bare) && /[A-Za-z]/.test(bare))
  );
}

function trimEdges(word: string): string {
  let start = 0;
  let end = word.length;
  while (start < end && EDGES.includes(word.charAt(start))) start++;
  while (end > start && EDGES.includes(word.charAt(end - 1))) end--;
  return word.slice(start, end);
}

/** A URL cut to its scheme and host: no user, path, query or fragment. */
function origin(word: string, at: number): string {
  const authority = word.slice(at + 3).split(/[/?#)\]>,;"']/)[0] ?? "";
  return `${word.slice(0, at)}://${authority.split("@").pop() ?? ""}`;
}

/** At most `max` characters of `text` (code points), with `more` after a cut. */
function cut(text: string, max: number, more = ""): string {
  const chars = Array.from(text);
  return chars.length > max ? chars.slice(0, max).join("") + more : text;
}

/** One line, no control characters, at most `max` characters. */
function plain(text: string, max: number): string {
  return cut(text.replace(CONTROL, " ").replace(/\s+/g, " ").trim(), max);
}

/**
 * A JavaScript stack's frames, as `at <function> (<file>:<line>:<column>)`: Hermes's
 * `at f (address at /…/Nexus.app/main.jsbundle:1:2345)` and Metro's `at f (http://…/index.bundle//&platform=ios…:12:3)`
 * both keep only the file's name. A line that is not a frame once reduced ({@link JS_FRAME}) is dropped: the error's
 * name and message, a message line that starts with `at `. A frame is a code location, so only the secret shapes are
 * taken out: the word rules would read `main.jsbundle:1:2345` as a token.
 */
export function frames(stack: unknown): string[] {
  if (typeof stack !== "string") return [];
  const out: string[] = [];
  for (const raw of stack.split("\n")) {
    const line = v8Shape(plain(raw, MAX_SOURCE_CHARS));
    if (!line.startsWith("at ")) continue;
    const call = /^at (.*?) \((?:address at )?(.*)\)$/.exec(line);
    const frame = cut(redactShapes(call ? `at ${call[1]} (${place(call[2] ?? "")})` : `at ${place(line.slice(3))}`), MAX_FRAME_CHARS);
    if (JS_FRAME.test(frame)) out.push(frame);
    if (out.length === MAX_FRAMES) break;
  }
  return out;
}

/**
 * A WebKit or Firefox frame (`render@https://…/app.js:12:3`, `global code@…`), the desktop's webview on macOS and
 * Safari's, as V8 writes it (`at render (https://…/app.js:12:3)`), so one parser reads all three. Any other line stays.
 */
function v8Shape(line: string): string {
  const frame = /^([^@\s]*(?: [^@\s]+)*)@(.+:\d+:\d+)$/.exec(line);
  if (!frame || line.startsWith("at ")) return line;
  const [, name = "", place = ""] = frame;
  return `at ${name.replace(/\s+/g, "_") || "<anonymous>"} (${place})`;
}

/** A code location reduced to the file's name and its line and column; `native` and the like stay. */
function place(location: string): string {
  const position = /:(\d+):(\d+)$/.exec(location);
  const path = ((position ? location.slice(0, position.index) : location).split(/[?&]/)[0] ?? "").replace(/\/+$/, "");
  const file = path.slice(path.lastIndexOf("/") + 1);
  return position ? `${file}:${position[1]}:${position[2]}` : file;
}

/** A thrown value as a report. Only an `Error` or a string has a message; any other value is named, not printed. */
export function fromError(error: unknown, fatal: boolean, device: Device, now = new Date()): DiagnosticReport {
  const known = error instanceof Error;
  const name = known ? error.name : typeof error === "string" ? "Error" : "NonError";
  const message = known ? error.message : typeof error === "string" ? error : "";
  return {
    kind: "error",
    fatal,
    name: plain(redact(firstLine(name)), MAX_NAME_CHARS) || "Error",
    message: redact(firstLine(message)),
    stack: known ? frames(error.stack) : [],
    occurredAt: now.toISOString(),
    ...versions(device),
  };
}

/** A native crash as a report. */
export function fromCrash(crash: NativeCrash, device: Device, now = new Date()): DiagnosticReport {
  return {
    kind: "crash",
    fatal: true,
    name: plain(redact(firstLine(crash.name)), MAX_NAME_CHARS) || "Crash",
    message: redact(firstLine(crash.message)),
    stack: (Array.isArray(crash.stack) ? crash.stack : [])
      .map((frame) => redactShapes(plain(String(frame), MAX_FRAME_CHARS)))
      .filter((frame) => NATIVE_FRAME.test(frame))
      .slice(0, MAX_FRAMES),
    occurredAt: TIME.test(crash.occurredAt) ? crash.occurredAt : now.toISOString(),
    ...versions({ ...device, appBuild: crash.appBuild ?? device.appBuild, osVersion: crash.osVersion || device.osVersion }),
  };
}

/**
 * Content follows a message's first line (a parser's excerpt, a diagram's source), so only the first line is kept, with
 * no control character, which the Gateway refuses. Any value: a message can be set to something that is not a string.
 */
function firstLine(text: unknown): string {
  return plain(String(text ?? "").split("\n", 1)[0] ?? "", MAX_SOURCE_CHARS);
}

function versions(device: Device): Pick<DiagnosticReport, "appVersion" | "appBuild" | "platform" | "osVersion"> {
  const appBuild = device.appBuild ? plain(device.appBuild, MAX_VERSION_CHARS) : "";
  return {
    appVersion: plain(device.appVersion, MAX_VERSION_CHARS) || "unknown",
    appBuild: appBuild || null,
    platform: device.platform,
    osVersion: plain(device.osVersion, MAX_VERSION_CHARS) || "unknown",
  };
}

/**
 * `pending` with `report` added, the oldest dropped past {@link MAX_PENDING}. A report that repeats one already kept
 * (the same kind, name, message and top frame) is not kept again, so an error that fires on every frame cannot push
 * out a crash still waiting to be sent.
 */
export function enqueue(pending: DiagnosticReport[], report: DiagnosticReport): DiagnosticReport[] {
  const same = (kept: DiagnosticReport) =>
    kept.kind === report.kind && kept.name === report.name && kept.message === report.message && kept.stack[0] === report.stack[0];
  return pending.some(same) ? pending : [...pending, report].slice(-MAX_PENDING);
}
