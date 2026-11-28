import { CREDITS_SHORT_TITLE, PAY_NOTICE_FALLBACK_TITLE, PAY_NOTICE_TITLE, type NexusPaysReason, type SubsidyView } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { explainRefusal, freeStartPays, payNotice, planLabel, planOf, storedDetails, usageLine, usageOf, usedUpNotice, type Plan } from "./plan";

const free: Plan = { kind: "free" };
const payg: Plan = { kind: "payg", credits: true, afterTheFact: false };
const member: Plan = { kind: "unknown" };
const pro: Plan = { kind: "paid", name: "Pro" };
const subsidy = (remainingUsd: number, usableForModelCalls = true): SubsidyView => ({
  grantedUsd: 1,
  spentUsd: 1 - remainingUsd,
  reservedUsd: 0,
  remainingUsd,
  fundedBy: "nexus",
  usableForModelCalls,
});

describe("plan", () => {
  it("names how the organization pays, and nothing while it is not known", () => {
    expect(planLabel(free)).toBe("Free");
    expect(planLabel(payg)).toBe("Pay as you go");
    expect(planLabel(member)).toBeUndefined();
    // Only where Gateway sells plans (`NEXUS_PLANS`): pricing v2.0 sells none.
    expect(planLabel(pro)).toBe("Pro");
  });

  it("reads a live plan first, and a plan that is over as none", () => {
    const plan = { plan: "max_100", status: "active" };
    expect(planOf(true, { plan, credits: null })).toEqual({ kind: "paid", name: "Max 100" });
    expect(planOf(true, { plan: { ...plan, status: "canceled" }, credits: null })).toEqual({ kind: "free" });
    expect(usedUpNotice(pro)).toMatchObject({ title: "Pro's Nexus usage is used up", open: { href: "/billing" } });
  });

  it("says what is left of the free start, from the twins first", () => {
    expect(usageLine(usageOf({ subsidy: subsidy(0.4) })!)).toBe("$0.40 of $1 free start left");
    expect(usageLine(usageOf({ subsidy: { ...subsidy(0), grantedUsdDecimal: "1.000000", remainingUsdDecimal: "0.004000" } })!)).toBe("$0.004000 of $1 free start left");
    expect(usageLine(usageOf({ subsidy: subsidy(0) })!)).toBe("Free $1 used up");
    // A usage cached before the micro-dollar amounts has only the numbers.
    expect(usageLine({ leftUsd: 0.4, grantedUsd: 1 })).toBe("$0.40 of $1 free start left");
  });

  it("shows billing status to an owner, and tells a member to ask about access", () => {
    // The title is the glossary's, the same as Nexus UI's.
    expect(usedUpNotice(free)).toEqual({
      title: "Your free start is used up",
      body: "Your own provider keys still work.",
      open: { href: "/billing", label: "Open billing" },
    });
    expect(usedUpNotice(free).title).toBe("Your free start is used up");
    expect(usedUpNotice(payg)).toMatchObject({ title: "Your Nexus credits are used up", open: { href: "/billing" } });
    expect(usedUpNotice(member)).toMatchObject({
      body: "Ask an owner about model access, or add your own provider key under Settings → API keys.",
      open: { href: "/keys" },
    });
  });

  it("says above the composer why Nexus cannot pay, one notice per reason", () => {
    expect(payNotice({ kind: "refused", reason: "free_start_expired", granted: 2_000_000 }, free)).toEqual({
      title: "Your free $2 expired",
      body: "Models Nexus runs are paused. Your own keys keep working.",
      open: { href: "/billing", label: "Open billing" },
    });
    expect(payNotice({ kind: "refused", reason: "usage_limit_reached", changesAt: "2026-12-01T00:00:00.000Z" }, payg).body).toBe(
      "Models Nexus runs are paused until 1 Dec, 00:00 UTC, or until an owner raises the limit. Your own keys keep working.",
    );
    expect(payNotice({ kind: "refused", reason: "charge_failed" }, member)).toMatchObject({
      title: "A payment didn't go through",
      body: "Models Nexus runs are paused until a payment goes through. Ask an owner about model access, or add your own provider key under Settings → API keys.",
      open: { href: "/keys" },
    });
  });

  it("explains the refusals a person can act on, without the CLI's commands", () => {
    const exhausted = explainRefusal(
      {
        code: "subsidy_exhausted",
        message: "this call may cost up to $0.031, and $0.004 of the Nexus credit is left: add your own key for this provider (`nexus keys add <provider> <key>`) or ask for fewer tokens",
      },
      free,
    );
    expect(exhausted).toEqual({
      title: "Your free start doesn't cover this answer",
      body: "Try a shorter answer or use your own provider key.",
      usedUp: true,
      open: { href: "/billing", label: "Open billing" },
    });
    // A stored refusal's text with no code reads the same.
    expect(explainRefusal({ message: "this call may cost up to $0.031, and $0 of the Nexus credit is left: …" }, free)?.usedUp).toBe(true);
    expect(explainRefusal({ code: "subsidy_exhausted", message: "" }, member)?.open.href).toBe("/keys");
    // A pricier model on the free start is not a used-up free start. It names the model refused and the
    // models the free start pays for, and offers Auto.
    const cheaper = { code: "subsidy_exhausted", message: "the free start pays for cheaper models only: pick a cheaper model" };
    expect(explainRefusal(cheaper, free, { model: "Claude Sonnet 5.5", covered: ["Claude Haiku 5.5", "GPT-6 Luna", "GLM 5.3"] })).toEqual({
      title: "Your free start doesn't pay for Claude Sonnet 5.5",
      body: "Pick Auto, Claude Haiku 5.5 or GPT-6 Luna.",
      usedUp: false,
      cheaperModel: true,
      open: { href: "/billing", label: "Open billing" },
    });
    // Without the catalog's prices (an older Gateway, or a member), it still says what to do.
    expect(explainRefusal(cheaper, free)).toMatchObject({
      title: "Your free start covers cheaper models only",
      body: "Pick Auto.",
      cheaperModel: true,
    });
    expect(explainRefusal(cheaper, member, { covered: ["Claude Haiku 5.5"] })?.body).toBe(
      "Pick Auto or Claude Haiku 5.5. Or ask an owner about model access, or add your own provider key under Settings → API keys.",
    );
    expect(explainRefusal({ code: "credit_exhausted", message: "" }, payg)).toMatchObject({ title: CREDITS_SHORT_TITLE, open: { href: "/billing" } });
    // an internal pull request's words: Gateway refuses a call that MAY cost more than what is left, so money can be left.
    const limit = "this call may cost up to $0.35 with Nexus's rate, and $0.20 of this month's usage limit of $25.00 is left: an owner raises the limit";
    expect(explainRefusal({ code: "usage_limit_reached", message: limit }, payg)?.title).toBe("This call may cost up to $0.35, and $0.20 of this month's usage limit is left");
    expect(explainRefusal({ code: "usage_limit_reached", message: limit.replace("$0.20", "$0.00") }, payg)?.title).toBe("This month's usage limit is used up");
    expect(explainRefusal({ code: "usage_limit_reached", message: "" }, { kind: "payg", credits: false, afterTheFact: true })).toMatchObject({
      title: "This month's usage limit doesn't cover this call",
      usedUp: true,
      open: { href: "/billing" },
    });
    expect(explainRefusal({ code: "usage_limit_reached", message: "" }, member)?.body).toMatch(/^Ask an owner to raise the limit, or add your own provider key/);
    expect(explainRefusal({ code: "payment_failed", message: "" }, payg)).toMatchObject({ title: "A payment didn't go through", usedUp: false, open: { href: "/billing" } });
    // A dispute needs another card: the composer's title, the same on every app.
    const disputed = "a payment on the card was disputed, so calls the card pays stopped: an owner adds another card in Billing";
    expect(explainRefusal({ code: "payment_failed", message: disputed }, payg)).toMatchObject({ title: "A payment on the card was disputed", usedUp: false, open: { href: "/billing" } });
    expect(explainRefusal({ code: "payment_failed", message: disputed }, member)?.ask).toBe("payment_failed");
    expect(
      explainRefusal({ code: "provider_key_required", message: "this organization's Nexus credit is used up: model calls need your own Anthropic key; …" }, free),
    ).toMatchObject({ title: "Your free start is used up", usedUp: true });
    expect(
      explainRefusal({ code: "provider_key_required", message: "this organization has $0.15 of Nexus credit, but this Gateway does not pay OpenAI calls with it yet: …" }, free),
    ).toMatchObject({ title: "Nexus usage doesn't pay for OpenAI models yet", usedUp: false, open: { href: "/keys" } });
    expect(explainRefusal({ code: "provider_key_required", message: "no Anthropic key for this organization: an owner or admin adds one with `nexus keys add anthropic <key>`" }, free)?.title).toBe(
      "No Anthropic key for this organization",
    );
    expect(explainRefusal({ code: "payment_past_due", message: "" }, free)?.usedUp).toBe(false);
    expect(explainRefusal({ code: "policy_denied", message: "model not allowed" }, free)).toBeUndefined();
  });

  it("says a usage limit is used up only when nothing of it is left", () => {
    const limit = "this call may cost up to $0.35 with Nexus's rate, and $0.20 of this month's usage limit of $25.00 is left: an owner raises the limit";
    // Money is left: the call may cost more. Not used up, so the composer's notice is not hidden.
    expect(explainRefusal({ code: "usage_limit_reached", message: limit }, payg)?.usedUp).toBe(false);
    expect(explainRefusal({ code: "usage_limit_reached", message: limit.replace("$0.20", "$0.00") }, payg)?.usedUp).toBe(true);
    // Gateway's details first; a sub-cent remainder is nothing left, as the message's "$0.00" says.
    expect(explainRefusal({ code: "usage_limit_reached", message: "", details: { remainingUsd: "0.200000" } }, payg)?.usedUp).toBe(false);
    expect(explainRefusal({ code: "usage_limit_reached", message: limit, details: { remainingUsd: "0.004000" } }, payg)).toMatchObject({
      title: PAY_NOTICE_TITLE.usage_limit_reached,
      usedUp: true,
    });
  });

  it("reads a failed payment's reason from Gateway's details first, and its message only for a stored answer", () => {
    // Live: `details.reason` decides, whatever the message says.
    expect(explainRefusal({ code: "payment_failed", message: "a charge for usage failed", details: { reason: "disputed" } }, payg)).toMatchObject({
      title: PAY_NOTICE_TITLE.card_disputed,
      body: "Model calls on Nexus are paused until an owner adds another card. Your own provider keys still work.",
    });
    const disputed = "a payment on the card was disputed, so calls the card pays stopped";
    expect(explainRefusal({ code: "payment_failed", message: disputed, details: { reason: "declined" } }, payg)?.title).toBe(PAY_NOTICE_TITLE.charge_failed);
    // Stored: no details, so the message tells a dispute.
    expect(explainRefusal({ code: "payment_failed", message: disputed }, payg)?.title).toBe(PAY_NOTICE_TITLE.card_disputed);
  });

  it("reads a reloaded chat refusal's stored details, and its message only when it has none", () => {
    const message = "a charge for usage failed, and $12.30 of usage is unpaid, so calls the card pays stopped";
    // As the chat builds it from a stored answer's `error` (`chat-conversation.tsx`'s `explained`).
    const reloaded = (error: { code: string; message: string; status: number; details?: unknown }) =>
      explainRefusal({ code: error.code, message: error.message, details: storedDetails(error) }, payg);
    expect(reloaded({ code: "payment_failed", message, status: 402, details: { reason: "authentication_required", next: ["billing", "own_key"], unpaidUsd: "12.300000" } })?.title).toBe(
      PAY_NOTICE_TITLE.card_needs_confirming,
    );
    expect(reloaded({ code: "payment_failed", message, status: 402, details: { reason: "disputed" } })?.title).toBe(PAY_NOTICE_TITLE.card_disputed);
    // No details (an older Gateway): today's text, from the message.
    expect(reloaded({ code: "payment_failed", message, status: 402 })).toMatchObject({
      title: PAY_NOTICE_TITLE.charge_failed,
      body: "Model calls on Nexus are paused. Your own provider keys still work.",
    });
    expect(reloaded({ code: "payment_failed", message: "a payment on the card was disputed, so calls the card pays stopped", status: 402 })?.title).toBe(PAY_NOTICE_TITLE.card_disputed);
    // Details that are not an object read as none.
    for (const details of [null, "disputed", ["disputed"]]) expect(storedDetails({ details })).toBeUndefined();
    expect(storedDetails(undefined)).toBeUndefined();
  });

  it("asks to confirm a payment the bank wants confirmed (3-D Secure), as the composer's notice does", () => {
    const message = "a charge for usage failed, and $12.30 of usage is unpaid, so calls the card pays stopped";
    const owner = explainRefusal({ code: "payment_failed", message, details: { reason: "authentication_required" } }, payg);
    expect(owner).toMatchObject({ title: "The bank wants you to confirm a payment", usedUp: false, open: { href: "/billing" } });
    expect(owner?.title).toBe(payNotice({ kind: "refused", reason: "card_needs_confirming" }, payg).title);
    expect(explainRefusal({ code: "payment_failed", message, details: { reason: "authentication_required" } }, member)).toMatchObject({
      body: "Ask an owner to confirm the payment, or add your own provider key under Settings → API keys.",
      ask: "payment_failed",
    });
    // A reason this build does not know reads as a failed payment.
    expect(explainRefusal({ code: "payment_failed", message, details: { reason: "card_expired" } }, payg)?.title).toBe(PAY_NOTICE_TITLE.charge_failed);
  });

  it("says a reason Nexus cannot pay that this build does not know without 'undefined'", () => {
    const reason = "bank_on_fire" as NexusPaysReason;
    for (const plan of [payg, member]) {
      const notice = payNotice({ kind: "refused", reason }, plan);
      expect(notice.title).toBe(PAY_NOTICE_FALLBACK_TITLE);
      expect(JSON.stringify(notice)).not.toContain("undefined");
    }
    expect(payNotice({ kind: "refused", reason }, payg).body).toBe("Models Nexus runs are paused. Your own keys keep working.");
    expect(payNotice({ kind: "refused", reason }, member).ask).toBe("billing");
  });

  it("asks an owner for a member, and opens Billing for an owner, on every block only an owner fixes", () => {
    const blocks = { usage_limit_reached: "usage_limit", credit_exhausted: "credits_out", subsidy_exhausted: "credits_out", payment_failed: "payment_failed", payment_past_due: "past_due" } as const;
    for (const [code, block] of Object.entries(blocks)) {
      expect(explainRefusal({ code, message: "" }, member)).toMatchObject({ ask: block, open: { href: "/keys" } });
      const owner = explainRefusal({ code, message: "" }, payg)!;
      expect(owner.open.href).toBe("/billing");
      expect(owner.ask).toBeUndefined();
    }
    expect(usedUpNotice(member).ask).toBe("billing");
    expect(usedUpNotice(payg).ask).toBeUndefined();
  });

  it("says a payment is still going through, in the composer and in a refused answer", () => {
    expect(payNotice({ kind: "refused", reason: "charge_processing" }, payg)).toEqual({
      title: "A payment is still going through",
      body: "Models Nexus runs on the card are paused until it's paid. A card payment takes seconds, and a bank payment can take a few days. Credits and your own keys still work.",
      open: { href: "/billing", label: "Open billing" },
    });
    const stored = { message: "a charge of $10.80 for usage is still going through, and until it is paid the card pays for at most $20.00 of usage" };
    expect(explainRefusal(stored, payg)).toMatchObject({ title: "A payment is still going through", usedUp: false, open: { href: "/billing" } });
    expect(explainRefusal({ code: "charge_processing", message: "" }, member)).toMatchObject({ ask: "billing", open: { href: "/keys" } });
  });

  it("never directs a person to buy model access in a notice", () => {
    const codes = ["subsidy_exhausted", "credit_exhausted", "usage_limit_reached", "payment_failed", "payment_past_due"];
    const cheaper = explainRefusal.bind(null, { message: "the free start pays for cheaper models only" });
    for (const plan of [free, payg, member]) {
      const named = cheaper(plan, { model: "Claude Sonnet 5.5", covered: ["Claude Haiku 5.5"] })!.body;
      for (const body of [usedUpNotice(plan).body, named, ...codes.map((code) => explainRefusal({ code, message: "" }, plan)!.body)]) {
        expect(body).not.toMatch(/\$|upgrade|purchase|App Store|Play|on the web|desktop|switch to pay as you go|add credits|pay after the fact|update the card|once it is paid/i);
      }
    }
  });

  it("marks the models the free start pays for: Auto, and any whose listed output price is at most the cap", () => {
    const priced = (id: string, output: number) => ({ id, pricing: { input: 0, output, unit: "USD per million tokens" } });
    expect(freeStartPays({ id: "nexus/auto" }, 5)).toBe(true);
    expect(freeStartPays(priced("anthropic/claude-haiku-4.5", 5), 5)).toBe(true);
    expect(freeStartPays(priced("anthropic/claude-sonnet-5.5", 10), 5)).toBe(false);
    // A catalog cached by an older build has no prices: not marked either way.
    expect(freeStartPays({ id: "anthropic/claude-haiku-5.5" }, 5)).toBeUndefined();
  });
});
