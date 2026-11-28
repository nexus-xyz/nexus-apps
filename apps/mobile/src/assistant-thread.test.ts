import { readFileSync } from "node:fs";
import { AGENT_MAX_CHARS } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { askNexusHref, askNexusLabel, handedWords } from "./assistant-handoff";
import { clock, greetingOf, NOT_SENT, nextQueued, QUEUED, receiptAfter, receiptAfterFailure, receiptLine, SENDING, type Receipt } from "./assistant-thread";

// The thread's send states: Not sent · Retry, and what waits while offline. A session transcript shows no Delivered
// or Read (ASSISTANT.md, "How the thread looks").

describe("a message's receipt", () => {
  it("is delivered once the agent has it, and a failure stays one", () => {
    expect(receiptAfter(SENDING)).toEqual({ kind: "delivered" });
    expect(receiptAfter({ kind: "delivered" })).toEqual({ kind: "delivered" });
    expect(receiptAfter(QUEUED)).toEqual(QUEUED);
  });

  it("is not sent when the agent never had it: queued for the network, waiting for Retry otherwise", () => {
    expect(receiptAfterFailure(SENDING, true)).toEqual(QUEUED);
    expect(receiptAfterFailure(SENDING, false)).toEqual(NOT_SENT);
    expect(receiptAfterFailure({ kind: "delivered" }, true)).toEqual({ kind: "delivered" });
  });

  it("writes a line only when the message was not sent", () => {
    expect(receiptLine(NOT_SENT)).toBe("Not sent · Retry");
    expect(receiptLine(QUEUED)).toBe("Not sent · Retry");
    expect(receiptLine({ kind: "delivered" })).toBeUndefined();
    expect(receiptLine(SENDING)).toBeUndefined();
    expect(receiptLine(undefined)).toBeUndefined();
  });

  it("writes the time in 24 hours", () => {
    expect(clock(new Date(2026, 10, 16, 17, 41).getTime())).toBe("17:41");
  });
});

describe("Nexus's first message", () => {
  it("greets by first name, with the Assistant's own name", () => {
    expect(greetingOf("Maya", "Nexus")).toBe("Hi Maya. I’m Nexus. Ask me for anything.");
    expect(greetingOf(undefined, "Juno")).toBe("Hi. I’m Juno. Ask me for anything.");
  });
});

describe("what waits for the connection", () => {
  it("goes out in the thread's order, and a message waiting for Retry does not go by itself", () => {
    const receipts: Record<string, Receipt> = { e1: { kind: "delivered" }, e2: NOT_SENT, e3: QUEUED, e4: QUEUED };
    expect(nextQueued(["e1", "e2", "e3", "e4"], receipts)).toBe("e3");
    expect(nextQueued(["e1", "e2"], receipts)).toBeUndefined();
  });
});

describe("Ask Nexus in Search (A21·B e)", () => {
  it("names the typed words and opens the Assistant with them", () => {
    expect(askNexusLabel("  dinner friday ")).toBe("Ask Nexus “dinner friday”");
    expect(askNexusLabel("   ")).toBeUndefined();
    expect(askNexusHref(" dinner ")).toEqual({ pathname: "/agent", params: { ask: "dinner" } });
    expect(askNexusHref("x".repeat(AGENT_MAX_CHARS + 5)).params.ask).toHaveLength(AGENT_MAX_CHARS);
  });

  it("reads the handed words from the screen's parameter", () => {
    expect(handedWords(" dinner ")).toBe("dinner");
    expect(handedWords(["dinner", "again"])).toBe("dinner");
    expect(handedWords("  ")).toBeUndefined();
    expect(handedWords(undefined)).toBeUndefined();
  });

  it("is Search's first row, only with words typed and the Assistant in this build", () => {
    const panel = readFileSync(new URL("./search-panel.tsx", import.meta.url), "utf8");
    expect(panel).toContain("const label = agent ? askNexusLabel(query) : undefined;");
    expect(panel).toContain("return [ask, ...sessions, native, settings, ...rest, places]");
    expect(panel).toContain('if (item.kind === "ask") return onNavigate(askNexusHref(item.query));');
  });
});
