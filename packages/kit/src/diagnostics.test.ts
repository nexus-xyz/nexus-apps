import type { DiagnosticReport } from "./api";
import { describe, expect, it } from "vitest";
import { enqueue, fromCrash, fromError, frames, MAX_PENDING, redact, redactShapes, type Device } from "./diagnostics";

const device: Device = { appVersion: "0.1.0", appBuild: "7", platform: "ios", osVersion: "26.1" };
const now = new Date("2026-10-27T18:04:05.123Z");

// What a careless error could carry: the question, the answer, code from the person's repository, their token, and
// their email.
const prompt = "summarize the merger memo for Globex";
const answer = "Globex pays 40 million in stock";
const code = "fn transfer(amount: u64) -> Result<Receipt>";
const token = "nx_s3cr3tT0k3nV4lu3abcdefgh";
const email = "ana@acme.test";
const CONTENT = [prompt, answer, code, token, email, "Globex", "merger", "transfer", "Receipt", "s3cr3t", "ana@"];

/** The API's contract (`openapi.json`'s `DiagnosticReport`): these fields, each within its bound. */
function expectWithinContract(report: DiagnosticReport) {
  expect(Object.keys(report).sort()).toEqual(
    ["appBuild", "appVersion", "fatal", "kind", "message", "name", "occurredAt", "osVersion", "platform", "stack"].sort(),
  );
  const control = /[\u0000-\u001f\u007f-\u009f]/;
  for (const [text, min, max] of [
    [report.name, 1, 120],
    [report.message, 0, 500],
    [report.appVersion, 1, 40],
    [report.osVersion, 1, 40],
    ...report.stack.map((frame) => [frame, 1, 300] as const),
  ] as const) {
    expect(Array.from(text).length).toBeGreaterThanOrEqual(min);
    expect(Array.from(text).length).toBeLessThanOrEqual(max);
    expect(text).not.toMatch(control);
  }
  expect(report.stack.length).toBeLessThanOrEqual(64);
  expect(report.occurredAt).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z$/);
}

describe("fromError", () => {
  it("carries no prompt, answer, code, token or email, wherever the error put them", () => {
    const leaky = (message: string) => {
      const error = new Error(message);
      error.stack = `Error: ${message}\n    at send (http://127.0.0.1:8081/index.bundle//&platform=ios&token=${token}:12:34)`;
      return error;
    };
    const thrown: unknown[] = [
      leaky(`Cannot send "${prompt}"`),
      new SyntaxError(`JSON Parse error: Unexpected token in '${answer}'`),
      new TypeError(`undefined is not an object (evaluating \`${code}\`)`),
      leaky(`fetch https://gateway.acme.test/v1/chat?key=${token} failed`),
      leaky(`Authorization: Bearer ${token}`),
      leaky(`the token ${token} was refused for ${email}`),
      new Error(`Parse error on line 2:\n${code}\n${answer}`),
      // A message line that starts with `at ` is not a frame.
      new Error(`Cannot parse:\nat the ${prompt}\n    at ${answer} (memo:1:2)`),
      `a string thrown with "${prompt}"`,
      { prompt, answer },
    ];
    for (const error of thrown) {
      const report = fromError(error, true, device, now);
      expectWithinContract(report);
      const sent = JSON.stringify(report);
      for (const content of CONTENT) expect(sent, `${content} in ${sent}`).not.toContain(content);
    }
  });

  it("keeps what finds the bug: the name, the message's own words, the frames and the versions", () => {
    const error = new TypeError("undefined is not a function");
    error.stack = [
      "TypeError: undefined is not a function",
      "    at render (address at /private/var/containers/Bundle/Application/0F1E/Nexus.app/main.jsbundle:1:2345)",
      "    at call (native)",
      "    at http://127.0.0.1:8081/index.bundle//&platform=ios&dev=true:20:7",
    ].join("\n");
    expect(fromError(error, false, device, now)).toEqual({
      kind: "error",
      fatal: false,
      name: "TypeError",
      message: "undefined is not a function",
      stack: ["at render (main.jsbundle:1:2345)", "at call (native)", "at index.bundle:20:7"],
      occurredAt: "2026-10-27T18:04:05.123Z",
      appVersion: "0.1.0",
      appBuild: "7",
      platform: "ios",
      osVersion: "26.1",
    });
  });

  it("keeps no control character, which would make the Gateway refuse the whole batch", () => {
    const error = new Error("bad \u001b[31mcolour\u0085 and \u0000 nul");
    error.name = "Odd\u0007Error";
    error.stack = "Odd: bad\n    at f\u0085 (main.jsbundle:1:2)";
    const report = fromError(error, false, device, now);
    expectWithinContract(report);
    expect(report.message).toBe("bad [31mcolour and nul");
    expect(report.name).toBe("Odd Error");
  });

  it("takes a message that is not a string without throwing", () => {
    const error = new Error("x");
    (error as unknown as { message: unknown }).message = { prompt };
    (error as unknown as { stack: unknown }).stack = 42;
    const report = fromError(error, true, device, now);
    expectWithinContract(report);
    expect(JSON.stringify(report)).not.toContain(prompt);
  });

  it("stays within the API's bounds for an outsized error", () => {
    const error = new Error("x".repeat(5000));
    error.name = `Very${"Long".repeat(100)}Error\u0007`;
    error.stack = Array.from({ length: 200 }, (_, i) => `    at f${i} (main.jsbundle:1:${i})`).join("\n");
    const report = fromError(error, true, { ...device, osVersion: "", appVersion: "9".repeat(80), appBuild: "" }, now);
    expectWithinContract(report);
    expect(report.stack).toHaveLength(64);
    expect(report.osVersion).toBe("unknown");
    expect(report.appBuild).toBeNull();
  });
});

describe("fromCrash", () => {
  it("keeps the crash's own build and OS, and redacts its reason", () => {
    const report = fromCrash(
      {
        name: "NSInvalidArgumentException",
        message: `Unhandled JS Exception: Cannot send "${prompt}" with ${token}`,
        stack: ["Nexus +0x1a2b3c", `the ${answer}`, "React +0x00ff10"],
        occurredAt: "2026-10-27T17:00:00Z",
        appBuild: "6",
        osVersion: "iPhone OS 26.0 (23A341)",
      },
      device,
      now,
    );
    expectWithinContract(report);
    expect(report).toMatchObject({
      kind: "crash",
      fatal: true,
      name: "NSInvalidArgumentException",
      message: "Unhandled JS Exception: Cannot send [quoted] with [redacted]",
      stack: ["Nexus +0x1a2b3c", "React +0x00ff10"],
      occurredAt: "2026-10-27T17:00:00Z",
      appVersion: "0.1.0",
      appBuild: "6",
      osVersion: "iPhone OS 26.0 (23A341)",
    });
  });

  it("takes the start's time when the OS gave no usable one", () => {
    const report = fromCrash({ name: "", message: "", stack: [], occurredAt: "today" }, device, now);
    expect(report.occurredAt).toBe("2026-10-27T18:04:05.123Z");
    expect(report.name).toBe("Crash");
  });
});

describe("redact", () => {
  it("does what the Gateway's Redacted does", () => {
    expect(redact("Invalid value 'the secret plan for Q4' for owner ana@example.com, \"x\" too")).toBe(
      "Invalid value [quoted] for owner [redacted] [quoted] too",
    );
    expect(redact("error sending request for url (https://user:pw@api.openai.com/v1/responses?key=AIzaSy0)")).toBe(
      "error sending request for url (https://api.openai.com",
    );
    expect(redact("x-api-key:nx_abcdefghijklmnopqrst used")).toBe("x-api-key:[redacted] used");
    expect(redact("an 'unclosed quote runs to the end")).toBe("an [quoted]");
    expect(redact("it doesn't keep session 4f9c2a7e1b3d5f60718293a4b5c6d7e8")).toBe("it doesn't keep session [redacted]");
  });

  it("redacts each PEM private key block, and stays fast on BEGIN lines without an END", () => {
    // Assembled from parts so secret scanners do not read the fixtures as a real key.
    const label = (kind: string) => `${kind}PRIVATE ${"KEY"}-----`;
    const begin = (kind = "") => `-----BEGIN ${label(kind)}`;
    const pem = (body: string) => `${begin("RSA ")}\n${body}\n-----END ${label("RSA ")}`;
    expect(redactShapes(`a ${pem("MIIE")} b ${pem("MIIF")} c`)).toBe("a [redacted] b [redacted] c");
    expect(redactShapes(`${begin()} no end`)).toBe(`${begin()} no end`);
    const many = begin().repeat(20_000);
    const started = performance.now();
    expect(redactShapes(many)).toBe(many);
    expect(performance.now() - started).toBeLessThan(1_000);
  });
});

describe("frames", () => {
  it("has none without a stack", () => {
    expect(frames(undefined)).toEqual([]);
  });
});

describe("enqueue", () => {
  it("keeps a repeated error once, so it cannot push out a waiting crash", () => {
    const crash = fromCrash({ name: "EXC_CRASH (SIGABRT)", message: "", stack: [], occurredAt: "2026-10-27T17:00:00Z" }, device, now);
    let pending = [crash];
    for (let i = 0; i < 100; i++) pending = enqueue(pending, fromError(new Error("onScroll threw"), false, device, new Date(now.getTime() + i)));
    expect(pending).toHaveLength(2);
    expect(pending[0]).toBe(crash);
  });

  it("keeps the newest reports up to the API's batch", () => {
    let pending: DiagnosticReport[] = [];
    for (let i = 0; i < MAX_PENDING + 5; i++) pending = enqueue(pending, fromError(new Error(`e${i}`), false, device, now));
    expect(pending).toHaveLength(MAX_PENDING);
    expect(pending[0]?.message).toBe("e5");
  });
});

describe("a WebKit stack (the desktop's webview on macOS, Safari)", () => {
  it("reads as V8's frames, with the file's name and no path or query", () => {
    const stack = [
      "render@tauri://localhost/assets/index-a1b2.js?v=3:12:345",
      "global code@https://nexus.test/app/_next/static/chunks/main.js:1:2",
      "@tauri://localhost/assets/vendor.js:7:8",
      "[native code]",
    ].join("\n");
    expect(frames(stack)).toEqual([
      "at render (index-a1b2.js:12:345)",
      "at global_code (main.js:1:2)",
      "at <anonymous> (vendor.js:7:8)",
    ]);
  });

  it("keeps a token out of a frame", () => {
    expect(frames(`load@https://nexus.test/app.js?key=${token}:1:2`)).toEqual(["at load (app.js:1:2)"]);
  });
});

describe("redact, stricter than the Gateway", () => {
  it("takes out a file's path, which names the person's folders", () => {
    expect(redact("No such file or directory (os error 2): /Users/ana/acme-merger/notes.md")).toBe("No such file or directory (os error 2): [path]");
    expect(redact("cannot open ~/Projects/globex/.env")).toBe("cannot open [path]");
    expect(redact("cannot open C:\\Users\\ana\\deal.docx")).toBe("cannot open [path]");
    expect(redact("cannot read src/deals/globex.ts, try again")).toBe("cannot read [path] try again");
    expect(redact("use and/or retry; ratio 1/2 holds")).toBe("use and/or retry; ratio 1/2 holds");
  });
});
