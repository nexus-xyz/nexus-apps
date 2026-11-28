import type { AgentMessage } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { afterEdit, approvalCard, connectCards, EDIT_UNSENT, editChanges, editStart, endedView, errandCard, lastSuggestions, ON_DESKTOP, OPENED_ERRAND, phaseAfter, sessionCards, UNSENT, type PendingMessage } from "./assistant-cards";

// The cards under Nexus's bubble on the phone: Connect (A6·e) and the approval card (A9).

const CARD = `card_${"a".repeat(32)}`;
const NOW = Date.parse("2026-11-17T08:30:00Z");

const answer = (connect: unknown): AgentMessage => ({ kind: "answer", text: "I need Linear.", model: "anthropic/claude-sonnet-5", costUsd: 0.01, connect } as AgentMessage);

const pending = (more: Partial<PendingMessage> = {}): PendingMessage => ({
  kind: "pending_action",
  card: CARD,
  write: { connector: "github", operation: "create_issue", summary: "Open an issue in acme/api", effect: "irreversible", arguments: { title: "Bug" } },
  approveUrl: "https://nexus.inc/app/acme/approve/a1",
  detail: "Nothing runs until you approve it.",
  ...more,
});

describe("the Connect card", () => {
  it("draws one card per connector the answer needed, with its letter, title and line", () => {
    const cards = connectCards(answer([{ connector: "linear", label: "Linear", url: "https://nexus.inc/app/acme/settings/connectors#linear", summary: "Create an issue." }]));
    expect(cards).toEqual([
      { connector: "linear", label: "Linear", title: "Connect Linear", initial: "L", line: "Create an issue.", url: "https://nexus.inc/app/acme/settings/connectors#linear" },
    ]);
  });

  it("says the kit's plain words when the hint has no summary, and names a connector without a label", () => {
    const [card] = connectCards(answer([{ connector: "linear", url: "https://nexus.inc/c" }]));
    expect(card?.label).toBe("Linear");
    expect(card?.line).toBe("It reads your issues and projects; each change asks first.");
    // A connector with no plain words of its own names the Assistant, as the person named it.
    expect(connectCards(answer([{ connector: "resy", url: "https://nexus.inc/c" }]), "Juno")[0]?.line).toBe("Juno can read it, and each write asks you first.");
  });

  it("leaves out a hint that is not a web link, and every message that is not an answer", () => {
    expect(connectCards(answer([{ connector: "linear", label: "Linear", url: "javascript:alert(1)" }]))).toEqual([]);
    expect(connectCards(answer(undefined))).toEqual([]);
    expect(connectCards(pending())).toEqual([]);
  });
});

describe("the approval card", () => {
  const asks = {
    tier: "asks-first" as const,
    kind: "send",
    to: ["Ana R.", "Dev L."],
    text: "Dinner Friday 7:30 at Nopa.",
    amountUsd: "31.4",
    why: { said: "tell them", at: "2026-11-17T08:24:00Z" },
    voice: "assistant" as const,
    always: true,
  };

  it("shows the tier, what, To, the amount, Why, the exact text and the expiry, and offers every choice", () => {
    const card = approvalCard(pending({ asks, inChat: "approve-or-decline", expiresAt: "2026-11-17T09:00:00Z" }), NOW);
    expect(card).toMatchObject({
      tier: "asks-first",
      tierLabel: "Asks first",
      summary: "Open an issue in acme/api",
      to: "Ana R. · Dev L.",
      amount: "$31.40",
      text: "Dinner Friday 7:30 at Nopa.",
      expiry: "Expires in 30 min",
      expired: false,
      approve: true,
      decline: true,
      always: true,
      review: "https://nexus.inc/app/acme/approve/a1",
    });
    expect(card.why).toMatch(/^You said “tell them” at \d\d:\d\d\.$/);
  });

  it("offers only Decline and Review in Nexus for a write that always asks", () => {
    const card = approvalCard(pending({ asks: { ...asks, tier: "always-asks" }, inChat: "decline-only" }), NOW);
    expect(card).toMatchObject({ tier: "always-asks", tierLabel: "Always asks", approve: false, decline: true, always: false });
  });

  it("offers only the link from an agent API without the routes, or without a client", () => {
    expect(approvalCard(pending(), NOW)).toMatchObject({ approve: false, decline: false, review: "https://nexus.inc/app/acme/approve/a1" });
    expect(approvalCard(pending({ asks, inChat: "approve-or-decline" }), NOW, false)).toMatchObject({ approve: false, decline: false, always: false });
  });

  it("shows the write's operation when there is no exact text, and no link that is not a web link", () => {
    const card = approvalCard(pending({ approveUrl: "nexus://approve/a1" }), NOW);
    expect(card.text).toBeUndefined();
    expect(card.technical).toBe("github.create_issue · cannot be undone");
    expect(card.review).toBeUndefined();
    expect(approvalCard(pending({ write: { ...pending().write, effect: "reversible" } }), NOW).technical).toBe("github.create_issue · can be undone");
  });

  it("says new to this errand on the To line, and offers Edit only with an edit and a write the chat approves (A24·b)", () => {
    const email = pending({
      inChat: "approve-or-decline",
      write: { connector: "outlook", operation: "send", summary: "Send the redline", effect: "reversible", arguments: { to: ["legal@lumen.example"], subject: "Redline", body: "Term at 12 months." } },
      asks: { ...asks, to: ["legal@lumen.example"], newTo: ["legal@lumen.example"], text: "Term at 12 months." },
      edit: [
        { name: "to", label: "To", kind: "addresses", required: true },
        { name: "body", label: "Message", kind: "lines", signed: true },
      ],
    });
    expect(approvalCard(email, NOW, true, true)).toMatchObject({ to: "legal@lumen.example · new to this errand", edited: false, edit: [{ name: "to" }, { name: "body" }] });
    expect(approvalCard(email, NOW, true).edit).toEqual([]);
    expect(approvalCard({ ...email, inChat: "decline-only" }, NOW, true, true).edit).toEqual([]);
    expect(approvalCard({ ...email, edited: true }, NOW, true, true).edited).toBe(true);
    expect(editStart(email)).toEqual({ to: "legal@lumen.example", body: "Term at 12 months." });
    expect(editChanges(email, { to: "legal@lumen.example", body: "Term at 12 months." })).toEqual({});
    expect(editChanges(email, { to: "a@x.io; b@x.io", body: "Twelve months." })).toEqual({ to: ["a@x.io", "b@x.io"], body: "Twelve months." });
    expect(afterEdit({ kind: "edited", replaces: email.card, action: { ...email, card: `card_${"c".repeat(32)}`, edited: true } })).toMatchObject({ kind: "edited", card: { edited: true } });
    expect(afterEdit({ kind: "failed", failure: { status: 409, error: "not_editable" } })).toEqual({ kind: "refused", text: "This write cannot be edited. Decline it and ask again." });
    expect(EDIT_UNSENT).toMatch(/could not save your edit/);
  });

  it("locks once it expired", () => {
    expect(approvalCard(pending({ inChat: "approve-or-decline", expiresAt: "2026-11-17T08:00:00Z" }), NOW)).toMatchObject({ expiry: "Expired", expired: true });
  });
});

describe("the exact text, in A7·a's parts", () => {
  const text = (body: string, args: Record<string, unknown> = {}) =>
    approvalCard(pending({ write: { connector: "gmail", operation: "send", summary: "Send an email", effect: "irreversible", arguments: args }, asks: { tier: "asks-first", kind: "send", to: ["ana@acme.example"], text: body, voice: "assistant", always: false } }), NOW);

  it("draws an email's subject first, the body, then the signature apart", () => {
    const card = text("Hi Ana,\n\nFriday at 7:30 works.\n\nSent by Nexus, Maya's assistant", { subject: " Dinner Friday " });
    expect(card).toMatchObject({ subject: "Dinner Friday", text: "Hi Ana,\n\nFriday at 7:30 works.", end: "Sent by Nexus, Maya's assistant" });
  });

  it("keeps Sent using Nexus apart, and a text with no ending whole", () => {
    expect(text("Friday at 7:30 works. — Sent using Nexus").end).toBe("Sent using Nexus");
    const plain = text("Friday at 7:30 works.");
    expect(plain.text).toBe("Friday at 7:30 works.");
    expect(plain.subject).toBeUndefined();
    expect(plain.end).toBeUndefined();
  });
});

describe("how a decision ends", () => {
  it("ends in place with the agent's words and its link", () => {
    const phase = phaseAfter({ kind: "decided", decided: { card: CARD, decision: "approve", outcome: "written", text: "Done: Open an issue.", result: { url: "https://github.com/acme/api/issues/9" }, always: true } });
    expect(phase.kind).toBe("ended");
    if (phase.kind !== "ended") return;
    expect(endedView(phase.decided)).toEqual({
      text: "Done: Open an issue.",
      url: "https://github.com/acme/api/issues/9",
      always: "I won’t ask for this kind again. Change it in Account › Assistant › Approvals.",
      tone: "done",
    });
  });

  it("shows a written result with a title as its own row with Open (A7·f)", () => {
    const ended = endedView({ card: CARD, decision: "approve", outcome: "written", text: "Done: Added it to your calendar.", result: { title: " Dinner at Nopa ", url: "https://calendar.google.com/e/1" } });
    expect(ended).toMatchObject({ title: "Dinner at Nopa", url: "https://calendar.google.com/e/1", tone: "done" });
    expect(endedView({ card: CARD, decision: "approve", outcome: "failed", text: "No.", result: { title: "Dinner" } }).title).toBeUndefined();
  });

  it("marks a failed write, and a declined one stays quiet", () => {
    expect(endedView({ card: CARD, decision: "approve", outcome: "failed", text: "GitHub refused it." }).tone).toBe("danger");
    expect(endedView({ card: CARD, decision: "decline", outcome: "declined", text: "Declined. Nothing was written." })).toEqual({ text: "Declined. Nothing was written.", tone: "quiet" });
  });

  it("says why the agent API refused it, and what a failed send means", () => {
    expect(phaseAfter({ kind: "failed", failure: { status: 410, error: "expired" } })).toEqual({ kind: "refused", text: "This approval expired. Nothing was written. Ask again." });
    expect(phaseAfter({ kind: "failed", failure: { status: 409, error: "already_decided" } })).toEqual({ kind: "refused", text: "This write was already approved or declined." });
    expect(UNSENT).toEqual({ kind: "refused", text: "Nexus could not take your answer. Try again in a moment." });
  });
});

describe("quick replies (A1·D)", () => {
  it("come from the reply's last message, only when it is an answer", () => {
    const asks = { kind: "answer", text: "Book it?", suggestions: ["Book it", "Other times", "Not now", "Later"] } as unknown as AgentMessage;
    expect(lastSuggestions([asks])).toEqual(["Book it", "Other times", "Not now"]);
    expect(lastSuggestions([asks, pending()])).toEqual([]);
    expect(lastSuggestions([])).toEqual([]);
  });
});

describe("the errand link card (A1·5)", () => {
  const errand = { id: "i4", goal: "Dinner Friday", status: "running", steps: 2, at: "2026-11-18T08:02:00.000Z" };
  const opened = { kind: "notice", topic: "errands", state: "opened", text: OPENED_ERRAND, errand } as unknown as AgentMessage;

  it("shows the errand the notice opened, with its line and spoken label", () => {
    expect(errandCard(opened)).toEqual({ id: "i4", status: "running", name: "Dinner Friday", line: "2 steps so far", label: "Open the errand Dinner Friday, Running, 2 steps so far" });
  });

  it("shows the errand as the list has it now, and why a failed one stopped", () => {
    const now = { ...errand, status: "failed" as const, detail: "Nopa is full", title: "Dinner" };
    expect(errandCard(opened, () => now)).toMatchObject({ status: "failed", name: "Dinner", line: "Nopa is full" });
  });

  it("is no card for any other message", () => {
    expect(errandCard({ kind: "notice", text: "Hello" } as AgentMessage)).toBeUndefined();
    expect(errandCard(pending())).toBeUndefined();
  });
});

describe("the session link card (A1·5)", () => {
  const withSessions = (sessions: unknown): AgentMessage => ({ kind: "answer", text: "I started a session.", sessions } as unknown as AgentMessage);

  it("opens a page of Nexus in the browser", () => {
    const [card] = sessionCards(withSessions([{ where: "synced", state: "done", title: "Codex session", detail: "gpt-5.5", url: "https://nexus.inc/app?next=%2Fsynced%2Ft_1", id: "t_1" }]));
    expect(card).toEqual({ title: "Codex session", state: "done", line: "Done · gpt-5.5", url: "https://nexus.inc/app?next=%2Fsynced%2Ft_1", label: "Open Codex session, Done, gpt-5.5" });
  });

  it("says a desktop draft opens in Nexus for desktop, and has no link", () => {
    const [card] = sessionCards(withSessions([{ where: "desktop", state: "ready", title: "Fix the tests", repo: "acme/cortex-sim", branch: "decoder-gru", url: "nexus://new?prompt=Fix" }]));
    expect(card).toEqual({
      title: "Fix the tests",
      state: "ready",
      line: `Ready to open · on decoder-gru · ${ON_DESKTOP}`,
      repo: "cortex-sim",
      label: `Fix the tests, Ready to open, on decoder-gru, cortex-sim. ${ON_DESKTOP}`,
    });
    expect(card?.url).toBeUndefined();
  });
});
