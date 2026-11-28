import { describe, expect, it } from "vitest";
import { createArtifactsClient } from "./artifacts";
import { MalformedNotification, parseAskOwners, parseNotificationDevice, parseNotificationItem, parseNotificationDevices, parseNotificationKinds, parseNotificationPage, parseNotificationPushKey, parseNotificationSettings } from "./notifications";

const ITEM = {
  id: "ntf_1",
  kind: "requests.artifact_access_requested",
  group: "requests_and_approvals",
  subjectText: "Someone asked for access to your artifact",
  subject: { type: "artifact", id: "art_1" },
  sources: [
    { type: "artifact_access", artifactId: "art_1", requestId: "req_1" },
    { type: "artifact_report", artifactId: "art_1", reportId: "rpt_1" },
    { type: "webhook", hookId: "h_1" },
    { type: "billing_change", auditId: "aud_1" },
  ],
  count: 3,
  readAt: "2026-10-26T10:00:00Z",
  createdAt: "2026-10-26T09:00:00Z",
  updatedAt: "2026-10-26T09:30:00Z",
};

describe("the notification inbox's answers", () => {
  it("keeps a joined person's id and drops a member source without one", () => {
    const joined = { ...ITEM, sources: [{ type: "member", userId: "usr_2" }, { type: "member" }] };
    expect(parseNotificationPage({ items: [joined] }).items[0]?.sources).toEqual([{ type: "member", userId: "usr_2" }]);
  });

  it("keeps an access answer with its outcome and drops one with an unknown outcome", () => {
    const answer = { type: "artifact_access_answer", artifactId: "art_1", requestId: "aar_1", outcome: "granted" };
    const item = { ...ITEM, sources: [answer, { ...answer, outcome: "maybe" }] };
    expect(parseNotificationPage({ items: [item] }).items[0]?.sources).toEqual([answer]);
  });

  it("parses a page, keeping the known sources and the cursor", () => {
    const page = parseNotificationPage({ items: [ITEM], nextCursor: "c_2" });
    expect(page.nextCursor).toBe("c_2");
    expect(page.items).toEqual([
      {
        id: "ntf_1",
        kind: "requests.artifact_access_requested",
        group: "requests_and_approvals",
        subjectText: "Someone asked for access to your artifact",
        subject: { type: "artifact", id: "art_1" },
        sources: [
          { type: "artifact_access", artifactId: "art_1", requestId: "req_1" },
          { type: "artifact_report", artifactId: "art_1", reportId: "rpt_1" },
          { type: "billing_change", auditId: "aud_1" },
        ],
        count: 3,
        readAt: "2026-10-26T10:00:00Z",
        createdAt: "2026-10-26T09:00:00Z",
        updatedAt: "2026-10-26T09:30:00Z",
      },
    ]);
    expect(parseNotificationPage({ items: [] })).toEqual({ items: [] });
  });

  it("parses a failed payment's item and drops a billing source without its month", () => {
    const page = parseNotificationPage({
      items: [
        {
          ...ITEM,
          id: "ntf_2",
          kind: "billing.payment_failed",
          group: "usage_and_billing",
          subjectText: "A payment for your Nexus bill failed",
          subject: { type: "organization", id: "ten_1" },
          sources: [{ type: "billing_period", periodId: "ten_1:live:2026-09" }, { type: "billing_period" }],
          count: 1,
        },
      ],
    });
    expect(page.items[0]?.sources).toEqual([{ type: "billing_period", periodId: "ten_1:live:2026-09" }]);
    expect(page.items[0]?.subject).toEqual({ type: "organization", id: "ten_1" });
  });

  it("keeps a skill promotion's source, by its id, and drops one without it", () => {
    const page = parseNotificationPage({
      items: [
        {
          ...ITEM,
          kind: "requests.skill_promotion_requested",
          subject: { type: "team", id: "team_a" },
          sources: [{ type: "skill_promotion", promotionId: "spr_aaaaaaaaaaaa" }, { type: "skill_promotion" }, { type: "skill_promotion", promotionId: 7 }],
        },
        { ...ITEM, id: "ntf_2", kind: "requests.skill_promotion_decided", subject: { type: "user", id: "usr_ada" }, sources: [{ type: "skill_promotion", promotionId: "spr_bbbbbbbbbbbb" }] },
      ],
    });
    expect(page.items.map((item) => item.sources)).toEqual([[{ type: "skill_promotion", promotionId: "spr_aaaaaaaaaaaa" }], [{ type: "skill_promotion", promotionId: "spr_bbbbbbbbbbbb" }]]);
  });

  it("keeps a security change's action and drops a security_change source without one", () => {
    const changed = { ...ITEM, group: "account_and_security", sources: [{ type: "security_change", action: "key.revoked" }, { type: "security_change" }] };
    expect(parseNotificationPage({ items: [changed] }).items[0]?.sources).toEqual([{ type: "security_change", action: "key.revoked" }]);
  });

  it("keeps a rejected key's provider and drops a provider_key source without one", () => {
    const rejected = { ...ITEM, group: "automations_and_integrations", sources: [{ type: "provider_key", provider: "anthropic" }, { type: "provider_key" }] };
    expect(parseNotificationPage({ items: [rejected] }).items[0]?.sources).toEqual([{ type: "provider_key", provider: "anthropic" }]);
  });

  it("keeps a lost GitHub installation's id and drops a github_installation source without one", () => {
    const lost = { ...ITEM, group: "automations_and_integrations", sources: [{ type: "github_installation", installationId: "42" }, { type: "github_installation" }] };
    expect(parseNotificationPage({ items: [lost] }).items[0]?.sources).toEqual([{ type: "github_installation", installationId: "42" }]);
  });

  it("keeps a month report's month and drops a usage_month source without a YYYY-MM month", () => {
    const report = { ...ITEM, sources: [{ type: "usage_month", month: "2026-10" }, { type: "usage_month", month: "October" }, { type: "usage_month" }] };
    expect(parseNotificationPage({ items: [report] }).items[0]?.sources).toEqual([{ type: "usage_month", month: "2026-10" }]);
  });

  it("keeps a month report's summary and drops an empty or non-text one", () => {
    const summarized = { ...ITEM, title: "October 2026 usage: $60.00", detail: "6 model calls. Open Usage for teams and models." };
    expect(parseNotificationItem(summarized)).toMatchObject({ title: summarized.title, detail: summarized.detail });
    const bare = parseNotificationItem({ ...ITEM, title: "", detail: 7 });
    expect("title" in bare || "detail" in bare).toBe(false);
  });
  it("keeps a limit change's audit action and drops a limit_change source without one", () => {
    const changed = { ...ITEM, sources: [{ type: "limit_change", action: "budget.team_changed" }, { type: "limit_change" }] };
    expect(parseNotificationPage({ items: [changed] }).items[0]?.sources).toEqual([{ type: "limit_change", action: "budget.team_changed" }]);
  });
  it("keeps a referral's hash and drops a referral source without one", () => {
    const joined = { ...ITEM, sources: [{ type: "referral", referral: "0123456789abcdef" }, { type: "referral" }] };
    expect(parseNotificationPage({ items: [joined] }).items[0]?.sources).toEqual([{ type: "referral", referral: "0123456789abcdef" }]);
  });
  it("keeps a money state and a member's question, and drops either when its value is not one it knows", () => {
    const sources = [
      { type: "billing_state", state: "usage_limit_reached" },
      { type: "billing_state", state: "auto_topup_off" },
      { type: "billing_state", state: "usage_limit_near" },
      { type: "billing_state", state: "pricing_change" },
      { type: "billing_state", state: "contract_invoice_due" },
      { type: "billing_state", state: "card_expiring" },
      { type: "billing_state", state: "broke" },
      { type: "owner_ask", userId: "usr_ada", block: "past_due" },
      { type: "owner_ask", userId: "usr_ada", block: "ada@example.com" },
      { type: "owner_ask", block: "billing" },
    ];
    expect(parseNotificationPage({ items: [{ ...ITEM, sources }] }).items[0]?.sources).toEqual([
      { type: "billing_state", state: "usage_limit_reached" },
      { type: "billing_state", state: "auto_topup_off" },
      { type: "billing_state", state: "usage_limit_near" },
      { type: "billing_state", state: "pricing_change" },
      { type: "billing_state", state: "contract_invoice_due" },
      { type: "billing_state", state: "card_expiring" },
      { type: "owner_ask", userId: "usr_ada", block: "past_due" },
    ]);
  });
  it("keeps an approval waiting on the person, and drops one without its id", () => {
    const sources = [{ type: "approval", approvalId: "apr_1" }, { type: "approval" }, { type: "approval", approvalId: 7 }];
    expect(parseNotificationPage({ items: [{ ...ITEM, sources }] }).items[0]?.sources).toEqual([{ type: "approval", approvalId: "apr_1" }]);
  });
  it("keeps a cloud session's transition, and drops one without its ids or number", () => {
    const session = { type: "session", machineId: "cm_1", threadId: "t_1", seq: 7 };
    const sources = [session, { type: "session", machineId: "cm_1", threadId: "t_1" }, { type: "session", machineId: "cm_1", seq: 7 }, { ...session, seq: "7" }];
    expect(parseNotificationPage({ items: [{ ...ITEM, sources }] }).items[0]?.sources).toEqual([session]);
  });
  it("reads a question's answer, and refuses any other", () => {
    expect(parseAskOwners({ status: "sent" })).toEqual({ status: "sent" });
    expect(parseAskOwners({ status: "already_asked" })).toEqual({ status: "already_asked" });
    expect(() => parseAskOwners({ status: "maybe" })).toThrow(MalformedNotification);
  });

  it("refuses an item without its group, subject or a positive count", () => {
    expect(() => parseNotificationPage({ items: [{ ...ITEM, group: "news" }] })).toThrow(MalformedNotification);
    expect(() => parseNotificationPage({ items: [{ ...ITEM, subject: undefined }] })).toThrow(MalformedNotification);
    expect(() => parseNotificationPage({ items: [{ ...ITEM, count: 0 }] })).toThrow(MalformedNotification);
    expect(() => parseNotificationPage({ notifications: [] })).toThrow(MalformedNotification);
  });

  it("parses the catalogue", () => {
    const kinds = parseNotificationKinds({
      kinds: [{ name: "usage.hard_stop_reached", group: "usage_and_billing", subjectText: "A spending limit stopped a call", required: true, defaults: ["inbox", "email"] }],
    });
    expect(kinds).toEqual([{ name: "usage.hard_stop_reached", group: "usage_and_billing", subjectText: "A spending limit stopped a call", required: true, defaults: ["inbox", "email"] }]);
    expect(() => parseNotificationKinds({ kinds: [{ name: "x", group: "usage_and_billing", subjectText: "y" }] })).toThrow(MalformedNotification);
  });

  it("sends the filters as query parameters and marks an item read or done", async () => {
    const calls: { url: string; method: string }[] = [];
    const client = createArtifactsClient({
      baseUrl: "https://gw.test/",
      apiKey: "nx_test",
      fetch: async (url, init) => {
        calls.push({ url, method: init?.method ?? "GET" });
        const body = url.endsWith("/read") || url.endsWith("/done") ? ITEM : { items: [ITEM] };
        return new Response(JSON.stringify(body), { status: 200 });
      },
    });
    await client.notifications({ unread: true, group: "usage_and_billing", cursor: "c_2" });
    await client.notifications();
    expect((await client.markNotificationRead("ntf_1")).id).toBe("ntf_1");
    await client.markNotificationDone("ntf_1");
    expect(calls).toEqual([
      { url: "https://gw.test/v1/notifications?cursor=c_2&unread=true&group=usage_and_billing", method: "GET" },
      { url: "https://gw.test/v1/notifications", method: "GET" },
      { url: "https://gw.test/v1/notifications/ntf_1/read", method: "POST" },
      { url: "https://gw.test/v1/notifications/ntf_1/done", method: "POST" },
    ]);
  });

  it("parses the settings and refuses a row without its flags or with an unknown setting", () => {
    const row = { group: "usage_and_billing", channel: "email", setting: "off", default: "on", locked: false, required: true, available: true };
    expect(parseNotificationSettings({ settings: [row] })).toEqual([row]);
    const push = { ...row, channel: "push", setting: "on", default: "off", available: false };
    expect(parseNotificationSettings({ settings: [push] })).toEqual([push]);
    const { available: _, ...older } = row;
    expect(parseNotificationSettings({ settings: [older] })).toEqual([row]);
    expect(() => parseNotificationSettings({ settings: [{ ...row, setting: "digest" }] })).toThrow(MalformedNotification);
    expect(() => parseNotificationSettings({ settings: [{ ...row, locked: undefined }] })).toThrow(MalformedNotification);
    expect(() => parseNotificationSettings({ settings: [{ ...row, channel: "sms" }] })).toThrow(MalformedNotification);
    expect(() => parseNotificationSettings({})).toThrow(MalformedNotification);
  });

  it("reads the push key and refuses an answer without one", () => {
    expect(parseNotificationPushKey({ publicKey: "BP4z" })).toBe("BP4z");
    expect(() => parseNotificationPushKey({ publicKey: "" })).toThrow(MalformedNotification);
    expect(() => parseNotificationPushKey({ error: { code: "not_configured" } })).toThrow(MalformedNotification);
  });

  it("reads the settings and puts the changes as one body", async () => {
    const calls: { url: string; method: string; body?: unknown }[] = [];
    const row = { group: "usage_and_billing", channel: "email", setting: "off", default: "on", locked: false, required: true, available: true };
    const client = createArtifactsClient({
      baseUrl: "https://gw.test/",
      apiKey: "nx_test",
      fetch: async (url, init) => {
        calls.push({ url, method: init?.method ?? "GET", ...(init?.body ? { body: JSON.parse(String(init.body)) } : {}) });
        return new Response(JSON.stringify({ settings: [row] }), { status: 200 });
      },
    });
    expect(await client.notificationSettings()).toEqual([row]);
    await client.setNotificationSettings([{ group: "usage_and_billing", channel: "email", setting: "off" }]);
    expect(calls).toEqual([
      { url: "https://gw.test/v1/notifications/settings", method: "GET" },
      { url: "https://gw.test/v1/notifications/settings", method: "PUT", body: { settings: [{ group: "usage_and_billing", channel: "email", setting: "off" }] } },
    ]);
  });

  it("parses a device and refuses one without its platform or times", () => {
    const device = { id: "ndv_1", platform: "ios", createdAt: "2026-10-30T00:00:00.000Z", lastSeenAt: "2026-10-30T00:00:00.000Z" };
    expect(parseNotificationDevice(device)).toEqual(device);
    expect(parseNotificationDevice({ ...device, revokedAt: "2026-10-31T00:00:00.000Z", revokedReason: "left", token: "x" })).toEqual({ ...device, revokedAt: "2026-10-31T00:00:00.000Z", revokedReason: "left" });
    expect(() => parseNotificationDevice({ ...device, platform: "sms" })).toThrow(MalformedNotification);
    expect(() => parseNotificationDevice({ ...device, createdAt: undefined })).toThrow(MalformedNotification);
    expect(parseNotificationDevices({ devices: [device] })).toEqual([device]);
    expect(() => parseNotificationDevices({})).toThrow(MalformedNotification);
  });

  it("registers, lists and revokes a device", async () => {
    const calls: { url: string; method: string; body?: unknown }[] = [];
    const device = { id: "ndv_1", platform: "web", createdAt: "2026-10-30T00:00:00.000Z", lastSeenAt: "2026-10-30T00:00:00.000Z" };
    const client = createArtifactsClient({
      baseUrl: "https://gw.test/",
      apiKey: "nx_test",
      fetch: async (url, init) => {
        const method = init?.method ?? "GET";
        calls.push({ url, method, ...(init?.body ? { body: JSON.parse(String(init.body)) } : {}) });
        const answer = method === "GET" ? { devices: [device] } : method === "DELETE" ? { ...device, revokedAt: device.createdAt, revokedReason: "signed_out" } : device;
        return new Response(JSON.stringify(answer), { status: 200 });
      },
    });
    expect(await client.registerNotificationDevice("web", "push-token")).toEqual(device);
    expect(await client.notificationDevices()).toEqual([device]);
    expect((await client.revokeNotificationDevice("ndv_1", "signed_out")).revokedReason).toBe("signed_out");
    await client.revokeNotificationDevice("ndv_1");
    expect(calls).toEqual([
      { url: "https://gw.test/v1/notifications/devices", method: "POST", body: { platform: "web", token: "push-token" } },
      { url: "https://gw.test/v1/notifications/devices", method: "GET" },
      { url: "https://gw.test/v1/notifications/devices/ndv_1?reason=signed_out", method: "DELETE" },
      { url: "https://gw.test/v1/notifications/devices/ndv_1", method: "DELETE" },
    ]);
  });

  it("collects a desktop device's deliveries and refuses an item without its fields", async () => {
    const item = { deliveryId: "ndl_1", kind: "billing.payment_failed", eventId: "nev_1", title: "A payment failed", path: "/app?next=%2Fnotifications" };
    const calls: { url: string; method: string }[] = [];
    let answer: unknown = { items: [item] };
    const client = createArtifactsClient({
      baseUrl: "https://gw.test/",
      apiKey: "nx_test",
      fetch: async (url, init) => {
        calls.push({ url, method: init?.method ?? "GET" });
        return new Response(JSON.stringify(answer), { status: 200 });
      },
    });
    expect(await client.collectNotificationDeliveries("ndv_1")).toEqual([item]);
    expect(calls).toEqual([{ url: "https://gw.test/v1/notifications/devices/ndv_1/collect", method: "POST" }]);
    const month = { ...item, kind: "usage.month_report", title: "October: $12.40 spent", body: "3 members, 41 sessions" };
    answer = { items: [month] };
    expect(await client.collectNotificationDeliveries("ndv_1")).toEqual([month]);
    answer = { items: [{ ...item, title: undefined }] };
    await expect(client.collectNotificationDeliveries("ndv_1")).rejects.toThrow(MalformedNotification);
    answer = { items: [{ ...item, body: 7 }] };
    await expect(client.collectNotificationDeliveries("ndv_1")).rejects.toThrow(MalformedNotification);
    answer = {};
    await expect(client.collectNotificationDeliveries("ndv_1")).rejects.toThrow(MalformedNotification);
  });
});
