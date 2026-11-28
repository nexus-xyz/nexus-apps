import type { NotificationItem } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { channelsLine, itemLine, itemTarget, kindLook, unreadBadge } from "./inbox";

const item = (more: Partial<NotificationItem>): NotificationItem => ({
  id: "ntf_1",
  kind: "k",
  group: "usage_and_billing",
  subjectText: "A call was refused",
  subject: { type: "organization", id: "t_1" },
  sources: [],
  count: 1,
  createdAt: "2026-11-03T10:00:00Z",
  updatedAt: "2026-11-03T10:00:00Z",
  ...more,
});

describe("the inbox", () => {
  it("says where it happened, and how many events past one", () => {
    expect(itemLine(item({ kind: "integrations.provider_key_rejected", group: "automations_and_integrations" }))).toBe("Provider keys");
    expect(itemLine(item({ kind: "requests.member_joined", group: "requests_and_approvals", count: 3 }))).toBe("Members · 3 times");
  });

  it("tints what asks for attention, and gives a kind it does not know its group's look", () => {
    expect(kindLook(item({ kind: "billing.payment_failed" }))).toEqual({ icon: "billing", where: "Billing", attention: true });
    expect(kindLook(item({ kind: "billing.plan_changed" })).attention).toBeUndefined();
    expect(kindLook(item({ kind: "something.new", group: "account_and_security" }))).toEqual({ icon: "shield", where: "Account and security" });
  });

  it("lists a group's channels, the inbox always last", () => {
    const s = (channel: "push" | "email", setting: "on" | "off", available = true) => ({ group: "usage_and_billing" as const, channel, setting, available });
    expect(channelsLine([s("push", "on"), s("email", "on")], "usage_and_billing")).toBe("Push · Email · Inbox");
    expect(channelsLine([s("push", "on", false), s("email", "off")], "usage_and_billing")).toBe("Inbox");
  });

  it("opens the newest source's page", () => {
    expect(itemTarget(item({ sources: [{ type: "artifact_access", artifactId: "art_1", requestId: "r" }] }))).toEqual({ pathname: "/artifact/[id]", params: { id: "art_1" } });
    expect(itemTarget(item({ sources: [{ type: "provider_key", provider: "baseten" }] }))).toBe("/keys");
    expect(itemTarget(item({ sources: [{ type: "limit_change", action: "budget.person_limit_changed" }] }))).toBe("/spend");
    expect(itemTarget(item({ sources: [{ type: "skill_promotion", promotionId: "p" }] }))).toBeUndefined();
    expect(itemTarget(item({}))).toBeUndefined();
    expect(itemTarget(item({ sources: [{ type: "approval", approvalId: "apr_1" }] }))).toEqual({ web: "/app/approvals/apr_1" });
  });

  it("counts what is unread for the drawer", () => {
    expect(unreadBadge(0, false)).toBeUndefined();
    expect(unreadBadge(3, false)).toBe("3");
    expect(unreadBadge(20, true)).toBe("20+");
  });
});
