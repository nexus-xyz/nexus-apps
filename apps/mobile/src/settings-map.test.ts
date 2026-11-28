import { readdirSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { findSettings, hubFor, partTitle, SETTINGS, settingsFor } from "./settings-map";

const rows = (parts: ReturnType<typeof settingsFor>) => parts.map((p) => [p.key, p.rows.map((r) => r.label)]);

// An owner's permissions, as far as the map reads them, and a member's (`nexus-base` `permissions.rs`: PEOPLE).
const OWNER = ["org.read", "org.billing", "referrals.read", "provider_keys.manage", "spend.read_all", "spend.export"];
const MEMBER = ["org.read", "members.read", "referrals.read", "spend.read_own", "policy.read", "teams.use", "audit.read_own"];

describe("settingsFor, organized as the desktop's", () => {
  it("shows You, then the organization, then the app, in the desktop's order", () => {
    expect(rows(settingsFor({ personal: false, permissions: OWNER }))).toEqual([
      ["you", ["Organizations", "Profile", "Preferences", "Assistant", "Agents and plans", "API keys", "Connections", "Privacy", "Referrals"]],
      ["org", ["General", "People", "Model access", "Usage", "Billing", "Repositories", "Skills", "Audit log"]],
      ["app", ["Archived sessions", "Docs", "Send feedback", "About"]],
    ]);
  });

  it("keeps the list short: about 20 rows, where the old map had 30", () => {
    expect(settingsFor({ personal: false, permissions: OWNER }).flatMap((p) => p.rows)).toHaveLength(21);
  });

  it("shows a member no page they have no access to, and keeps those they can read", () => {
    const parts = settingsFor({ personal: false, permissions: MEMBER });
    expect(rows(parts).find(([key]) => key === "org")?.[1]).toEqual(["General", "People", "Model access", "Usage", "Repositories", "Skills", "Audit log"]);
    expect(hubFor("models", parts)?.pages?.map((p) => p.label)).toEqual(["Models", "Policy", "Gateway"]);
    expect(hubFor("usage", parts)?.pages?.map((p) => p.label)).toEqual(["Budgets", "Analytics"]);
    expect(findSettings("billing", parts, "Acme")).toEqual([]);
  });

  it("shows nothing that needs a permission while the permissions are not known", () => {
    const labels = settingsFor({ personal: false }).flatMap((p) => p.rows.flatMap((r) => (r.pages ?? [r]).map((page) => page.label)));
    expect(labels).not.toContain("Billing");
    expect(labels).not.toContain("Providers");
    expect(labels).not.toContain("Referrals");
  });

  it("hides People in Personal: no one else to add or group", () => {
    const parts = settingsFor({ personal: true });
    expect(rows(parts).flatMap(([, labels]) => labels)).not.toContain("People");
    expect(hubFor("people", parts)).toBeNull();
  });

  it("drops a hidden page, and a row left with one page links straight to it", () => {
    const org = settingsFor({ personal: false, hidden: ["/billing", "/data"], permissions: OWNER }).find((p) => p.key === "org")!;
    expect(org.rows.map((r) => r.label)).not.toContain("Billing");
    const general = org.rows.find((r) => r.label === "General")!;
    expect(general.href).toBe("/general");
    expect(general.pages).toBeUndefined();
  });
});

describe("a row that holds several pages", () => {
  it("lists them, each with what it is for", () => {
    const people = hubFor("people", settingsFor({ personal: false, permissions: OWNER }));
    expect(people?.label).toBe("People");
    expect(people?.pages?.map((p) => [p.label, !!p.about])).toEqual([["Members", true], ["Teams", true]]);
    expect(hubFor("models", settingsFor({ personal: false, permissions: OWNER }))?.pages?.map((p) => p.label)).toEqual(["Models", "Policy", "Providers", "Gateway"]);
    expect(hubFor("usage", settingsFor({ personal: false, permissions: OWNER }))?.pages?.map((p) => p.label)).toEqual(["Usage", "Budgets", "Analytics", "Observability"]);
  });

  it("opens a page of its own for each row, and says what it covers", () => {
    for (const row of SETTINGS.flatMap((p) => p.rows).filter((r) => r.hub)) {
      expect(row.href).toBe(`/settings-group/${row.hub}`);
      expect(row.about).toBeTruthy();
    }
    expect(hubFor("people", settingsFor({ personal: false }))?.about).toBe("Members and teams");
  });
});

describe("partTitle", () => {
  it("names the organization, or Personal", () => {
    expect(partTitle("org", "Acme", false)).toBe("Acme");
    expect(partTitle("org", "", false)).toBe("Organization");
    expect(partTitle("org", "Acme", true)).toBe("Personal");
    expect(partTitle("you", "Acme", false)).toBe("You");
  });
});

describe("findSettings", () => {
  const parts = settingsFor({ personal: false, permissions: OWNER });

  it("finds a page inside a row, with where it lives", () => {
    expect(findSettings("budget", parts, "Acme")).toEqual([expect.objectContaining({ label: "Budgets", where: "Acme › Usage" })]);
    expect(findSettings("anthropic key", parts, "Acme")).toEqual([expect.objectContaining({ label: "Providers", where: "Acme › Model access" })]);
    expect(findSettings("theme", parts, "Acme")).toEqual([expect.objectContaining({ label: "Appearance", where: "You › Preferences" })]);
    expect(findSettings("scan qr", parts, "Acme")).toEqual([expect.objectContaining({ label: "Computers", href: "/computers", where: "You › Connections" })]);
    expect(findSettings("mcp", parts, "Acme")).toEqual([expect.objectContaining({ label: "Integrations", href: "/integrations", where: "You › Connections" })]);
  });

  it("finds a page by another word for it, after the label matches", () => {
    expect(findSettings("spend", parts, "Acme").map((p) => p.label)).toEqual(["Usage", "Budgets"]);
    expect(findSettings("crash", parts, "Acme")).toEqual([expect.objectContaining({ label: "Privacy", where: "You" })]);
  });

  it("finds the organization picker by what people call it", () => {
    expect(findSettings("switch org", parts, "Acme")).toEqual([expect.objectContaining({ label: "Organizations", href: "/accounts" })]);
  });

  it("finds Account › Assistant by its sections, and leaves it out of a build without the agent API", () => {
    expect(findSettings("always allow", parts, "Acme")).toEqual([expect.objectContaining({ label: "Assistant", href: "/assistant-settings", where: "You" })]);
    const without = settingsFor({ personal: false, hidden: ["/assistant-settings"], permissions: OWNER });
    expect(rows(without)[0]?.[1]).not.toContain("Assistant");
  });

  it("needs every word, and finds nothing for an empty query", () => {
    expect(findSettings("api keys", parts, "Acme").map((p) => p.label)).toEqual(["API keys"]);
    expect(findSettings("api budget", parts, "Acme")).toEqual([]);
    expect(findSettings("  ", parts, "Acme")).toEqual([]);
  });
});

describe("every page the map links to", () => {
  it("is a screen of the app", () => {
    const main = new URL("./app/(app)/(main)/", import.meta.url);
    const screens = new Set(readdirSync(main, { recursive: true }).map((file) => `/${String(file).replace(/\.tsx$/, "").replace(/\/index$/, "")}`));
    const hrefs = SETTINGS.flatMap((p) => p.rows.flatMap((r) => (r.pages ? r.pages : [r]))).map((page) => page.href);
    expect(hrefs.filter((href) => !screens.has(href))).toEqual([]);
    expect(screens.has("/settings-group/[key]")).toBe(true);
  });
});
