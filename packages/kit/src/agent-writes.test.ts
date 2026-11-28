import { describe, expect, it } from "vitest";
import { createAgentClient } from "./agent-chat";
import {
  writeAmountText,
  writeChoices,
  writeConsentOf,
  writeDecidedOf,
  writeDecisionBody,
  writeDecisionPath,
  writeDecisionProblem,
  writeEditAddresses,
  writeEditBody,
  writeEditChanges,
  writeEditedOf,
  writeEditFields,
  writeEditPath,
  writeEditProblem,
  writeEditValues,
  writeExpiryText,
  writeTextParts,
  writeTierLabel,
  writeToEntries,
  writeToText,
  writeVoices,
  writeWhyText,
} from "./agent-writes";

describe("the exact text in its parts (A7·a)", () => {
  it("draws the assistant's signature apart, and an email's subject first", () => {
    const text = "Hi Priya, Maya asked me to follow up on the SOW.\n\nSent by Nexus, Maya's assistant";
    expect(writeTextParts(text, { subject: " SOW v3, two questions ", to: "priya@lumen.example" })).toEqual({
      subject: "SOW v3, two questions",
      body: "Hi Priya, Maya asked me to follow up on the SOW.",
      end: "Sent by Nexus, Maya's assistant",
    });
  });

  it("reads each connector's Sent using Nexus as the recipient sees it", () => {
    expect(writeTextParts("Running late.\n\n_Sent using <https://nexus.inc|Nexus>_")).toEqual({ body: "Running late.", end: "Sent using Nexus" });
    expect(writeTextParts("Shipped it — Sent using Nexus")).toEqual({ body: "Shipped it", end: "Sent using Nexus" });
    expect(writeTextParts("Notes attached.\n\nSent using Nexus (https://nexus.inc)\n")).toEqual({ body: "Notes attached.", end: "Sent using Nexus" });
  });

  it("keeps a text with no end whole, and never takes a lone line for a signature", () => {
    expect(writeTextParts("Dinner Friday 7:30 at Nopa.")).toEqual({ body: "Dinner Friday 7:30 at Nopa." });
    expect(writeTextParts("Sent by Nexus, Maya's assistant")).toEqual({ body: "Sent by Nexus, Maya's assistant" });
    expect(writeTextParts("Hi.\n\nSent by courier\nTomorrow")).toEqual({ body: "Hi.\n\nSent by courier\nTomorrow" });
    expect(writeTextParts("Hi.", { subject: 3 })).toEqual({ body: "Hi." });
  });
});

describe("the two voices (A7·b)", () => {
  it("names the person in each sample, and plain words without a name", () => {
    expect(writeVoices("Maya", "Maya Chen").map((voice) => [voice.title, voice.sample])).toEqual([
      ["As your assistant", "Sent by Nexus, Maya's assistant"],
      ["As you", "Maya Chen"],
    ]);
    expect(writeVoices().map((voice) => voice.sample)).toEqual(["Sent by Nexus, your assistant", "Your name"]);
    // The name the person gave their Assistant signs instead of Nexus.
    expect(writeVoices("Maya", "Maya Chen", "Ada")[0]?.sample).toBe("Sent by Ada, Maya's assistant");
  });
});

// The A9 card's words and the agent API's answer to a decision.

const CARD = `card_${"a".repeat(32)}`;

describe("a decision on a write", () => {
  it("names only a card id of the agent API's shape", () => {
    expect(writeDecisionPath(CARD, "approve")).toBe(`/agent/writes/${CARD}/approve`);
    expect(writeDecisionPath(CARD, "decline")).toBe(`/agent/writes/${CARD}/decline`);
    expect(writeDecisionPath("card_x/../../agent/memory", "approve")).toBeUndefined();
  });

  it("sends always only on an approve", () => {
    expect(writeDecisionBody("approve", true)).toBe('{"always":true}');
    expect(writeDecisionBody("approve")).toBe("{}");
    expect(writeDecisionBody("decline", true)).toBe("{}");
  });

  it("reads the answer strictly", () => {
    expect(writeDecidedOf({ card: CARD, decision: "approve", outcome: "written", text: "Done.", result: { url: "https://x.test/1", extra: 1 }, always: true })).toEqual({
      card: CARD,
      decision: "approve",
      outcome: "written",
      text: "Done.",
      result: { url: "https://x.test/1" },
      always: true,
    });
    expect(writeDecidedOf({ card: CARD, decision: "approve", outcome: "maybe", text: "?" })).toBeUndefined();
    expect(writeDecidedOf(null)).toBeUndefined();
  });

  // Gateway's `error.nexus` for 403 `write_consent_required`, carried on a failed write's answer.
  const CONSENT = {
    connector: "microsoft-todo",
    account: "ada@acme.test",
    scopes: ["Tasks.ReadWrite"],
    consent: { method: "POST", path: "/v1/connections/microsoft-todo/oauth", body: { scope: "user", access: "write" } },
  };

  it("reads a write's consent request strictly, and only the call the contract names", () => {
    expect(writeConsentOf(CONSENT)).toEqual(CONSENT);
    expect(writeConsentOf({ ...CONSENT, consent: { ...CONSENT.consent, path: "/v1/connections/gmail/oauth" } })).toBeUndefined();
    expect(writeConsentOf({ ...CONSENT, consent: { ...CONSENT.consent, path: "https://evil.test/v1/connections/microsoft-todo/oauth" } })).toBeUndefined();
    expect(writeConsentOf({ ...CONSENT, consent: { ...CONSENT.consent, method: "DELETE" } })).toBeUndefined();
    expect(writeConsentOf({ ...CONSENT, consent: { ...CONSENT.consent, body: { scope: "user", access: "read" } } })).toBeUndefined();
    expect(writeConsentOf({ ...CONSENT, connector: "../admin" })).toBeUndefined();
    expect(writeConsentOf({ ...CONSENT, scopes: [1] })).toBeUndefined();
    expect(writeConsentOf(undefined)).toBeUndefined();
  });

  it("keeps the consent request only on a failed write", () => {
    const failed = { card: CARD, decision: "approve", outcome: "failed", text: "Nothing was written.", consent: CONSENT };
    expect(writeDecidedOf(failed)?.consent).toEqual(CONSENT);
    expect(writeDecidedOf({ ...failed, outcome: "written" })?.consent).toBeUndefined();
    expect(writeDecidedOf({ ...failed, consent: { ...CONSENT, consent: null } })).toEqual({ card: CARD, decision: "approve", outcome: "failed", text: "Nothing was written." });
  });

  it("says why a decision was refused", () => {
    expect(writeDecisionProblem(410, { error: "expired" })).toContain("expired");
    expect(writeDecisionProblem(409, { error: "already_decided" })).toContain("already approved or declined");
    expect(writeDecisionProblem(404, { error: "not_found" })).toContain("no longer waits");
  });

  it("goes through the client with the person's key", async () => {
    const seen: { url: string; body: string; authorization: string | null }[] = [];
    const client = createAgentClient({
      baseUrl: "https://agents.test",
      apiKey: "nxk_ann",
      fetch: async (input, init) => {
        seen.push({ url: String(input), body: String(init?.body), authorization: new Headers(init?.headers).get("authorization") });
        return Response.json({ card: CARD, decision: "approve", outcome: "written", text: "Done: Comment on an issue." });
      },
    });
    expect(await client.decideWrite(CARD, "approve", { always: true })).toEqual({ kind: "decided", decided: { card: CARD, decision: "approve", outcome: "written", text: "Done: Comment on an issue." } });
    expect(seen).toEqual([{ url: `https://agents.test/agent/writes/${CARD}/approve`, body: '{"always":true}', authorization: "Bearer nxk_ann" }]);
    expect(await client.decideWrite("bad", "approve")).toMatchObject({ kind: "failed", failure: { status: 400 } });
  });
});

describe("what the card offers", () => {
  it("approves only a write that asks first, and always allows only where the card offers it", () => {
    expect(writeChoices({ inChat: "approve-or-decline", asks: { always: true } })).toEqual({ approve: true, decline: true, always: true });
    expect(writeChoices({ inChat: "decline-only", asks: { always: true } })).toEqual({ approve: false, decline: true, always: false });
    expect(writeChoices({})).toEqual({ approve: false, decline: false, always: false });
  });
});

describe("the A9 card's words", () => {
  it("names the tier, the expiry, Why and the amount", () => {
    expect(writeTierLabel({ tier: "asks-first" })).toBe("Asks first");
    expect(writeTierLabel({ tier: "always-asks" })).toBe("Always asks");
    const now = Date.parse("2026-11-09T12:00:00.000Z");
    expect(writeExpiryText("2026-11-09T12:30:00.000Z", now)).toBe("Expires in 30 min");
    expect(writeExpiryText("2026-11-09T11:59:00.000Z", now)).toBe("Expired");
    expect(writeExpiryText(undefined, now)).toBeUndefined();
    expect(writeWhyText({ said: "tell them", at: "2026-11-09T08:24:00.000Z" }, () => "08:24")).toBe("You said “tell them” at 08:24.");
    expect(writeAmountText("31.4")).toBe("$31.40");
    expect(writeAmountText("50")).toBe("$50.00");
    expect(writeAmountText("nope")).toBeUndefined();
  });
});

describe("edit a write that waits (A24·b)", () => {
  const NEW = `card_${"b".repeat(32)}`;
  const EMAIL = {
    card: CARD,
    inChat: "approve-or-decline" as const,
    write: { arguments: { to: ["legal@lumen.example"], subject: "SOW redline", body: "Term back to 12 months.\n\nSent by Nexus, Maya's assistant" } },
    edit: [
      { name: "to", label: "To", kind: "addresses", required: true },
      { name: "cc", label: "Cc", kind: "addresses" },
      { name: "subject", label: "Subject", kind: "line", required: true },
      { name: "body", label: "Message", kind: "lines", signed: true },
    ],
  };
  const ACTION = { kind: "pending_action", card: NEW, write: { connector: "outlook", operation: "send", summary: "Send", effect: "reversible", arguments: {} }, approveUrl: "https://ui.test/a", detail: "Nexus asks", edited: true };

  it("reads the fields strictly, and only on a card decided in the chat", () => {
    expect(writeEditFields(EMAIL).map((field) => field.name)).toEqual(["to", "cc", "subject", "body"]);
    expect(writeEditFields({ ...EMAIL, inChat: undefined })).toEqual([]);
    expect(writeEditFields({ inChat: "decline-only", edit: [{ name: "x y", label: "X", kind: "line" }, { name: "x", label: "X", kind: "html" }, { name: "x", label: "", kind: "line" }, null] })).toEqual([]);
    expect(writeEditFields({ inChat: "decline-only", edit: "body" })).toEqual([]);
  });

  it("starts the inputs from the write, a signed text without its signature, and sends only what changed", () => {
    expect(writeEditValues(EMAIL)).toEqual({ to: "legal@lumen.example", cc: "", subject: "SOW redline", body: "Term back to 12 months." });
    expect(writeEditChanges(EMAIL, writeEditValues(EMAIL))).toEqual({});
    expect(writeEditChanges(EMAIL, { ...writeEditValues(EMAIL), cc: "maya@acme.example; priya@acme.example", subject: " SOW redline ", body: "Term back to 12 months, please." })).toEqual({
      cc: ["maya@acme.example", "priya@acme.example"],
      body: "Term back to 12 months, please.",
    });
    expect(writeEditAddresses(" a@x.io,\nb@x.io ,, ")).toEqual(["a@x.io", "b@x.io"]);
  });

  it("reads the answer strictly, and says why an edit was refused", () => {
    expect(writeEditedOf({ replaces: CARD, action: ACTION })).toEqual({ replaces: CARD, action: ACTION });
    expect(writeEditedOf({ replaces: CARD, action: { ...ACTION, card: CARD } })).toBeUndefined();
    expect(writeEditedOf({ replaces: CARD, action: { ...ACTION, kind: "answer" } })).toBeUndefined();
    expect(writeEditedOf({ replaces: "x", action: ACTION })).toBeUndefined();
    expect(writeEditedOf({ replaces: CARD, action: { ...ACTION, write: { ...ACTION.write, arguments: [] } } })).toBeUndefined();
    expect(writeEditProblem(409, { error: "not_editable" })).toContain("cannot be edited");
    expect(writeEditProblem(409, { error: "already_decided" })).toContain("no longer waits");
    expect(writeEditProblem(410)).toContain("expired");
    expect(writeEditProblem(400, { error: "bad_request", detail: "to needs one address at least." })).toBe("to needs one address at least.");
    expect(writeEditPath(CARD)).toBe(`/agent/writes/${CARD}`);
    expect(writeEditPath("card_x")).toBeUndefined();
    expect(writeEditBody({ body: "x" })).toBe('{"arguments":{"body":"x"}}');
  });

  it("goes through the client as a PATCH with the person's key", async () => {
    const seen: { url: string; method?: string; body: string }[] = [];
    const client = createAgentClient({
      baseUrl: "https://agents.test",
      apiKey: "nxk_ann",
      fetch: async (input, init) => {
        seen.push({ url: String(input), method: init?.method, body: String(init?.body) });
        return seen.length === 1 ? Response.json({ replaces: CARD, action: ACTION }) : Response.json({ error: "not_editable", detail: "No." }, { status: 409 });
      },
    });
    expect(await client.editWrite(CARD, { body: "x" })).toEqual({ kind: "edited", replaces: CARD, action: ACTION });
    expect(seen).toEqual([{ url: `https://agents.test/agent/writes/${CARD}`, method: "PATCH", body: '{"arguments":{"body":"x"}}' }]);
    expect(await client.editWrite(CARD, { body: "x" })).toMatchObject({ kind: "failed", failure: { status: 409, error: "not_editable" } });
    expect(await client.editWrite("bad", {})).toMatchObject({ kind: "failed", failure: { status: 400 } });
  });

  it("marks a recipient new to the errand on the To line, as A24·b draws it", () => {
    expect(writeToText({ to: ["legal@lumen.example"], newTo: ["legal@lumen.example"] })).toBe("legal@lumen.example · new to this errand");
    expect(writeToText({ to: ["legal@lumen.example"] })).toBe("legal@lumen.example");
    expect(writeToText({ to: ["priya@acme.example", "legal@lumen.example"], newTo: ["legal@lumen.example"] })).toBe("priya@acme.example · legal@lumen.example (new to this errand)");
    expect(writeToText({ to: [] })).toBeUndefined();
    expect(writeToEntries({ to: ["a@x.io"], newTo: ["a@x.io"] })).toEqual([{ to: "a@x.io", isNew: true }]);
  });
});
