import { describe, expect, it } from "vitest";
import { agentSessionAuditOn, appsOn, wikiOn } from "./features";

describe("wikiOn", () => {
  it("is on only when the Gateway says so", () => {
    expect(wikiOn({ features: { wiki: true } })).toBe(true);
    expect(wikiOn({ features: { wiki: false } })).toBe(false);
    expect(wikiOn({})).toBe(false);
    expect(wikiOn(undefined)).toBe(false);
    expect(wikiOn(null)).toBe(false);
  });
});

describe("appsOn", () => {
  it("is on only when the Gateway says so", () => {
    expect(appsOn({ features: { wiki: false, apps: true } })).toBe(true);
    expect(appsOn({ features: { wiki: true, apps: false } })).toBe(false);
    expect(appsOn({ features: { wiki: true } })).toBe(false);
    expect(appsOn(undefined)).toBe(false);
  });
});

describe("agentSessionAuditOn", () => {
  it("is on only when the Gateway says so", () => {
    expect(agentSessionAuditOn({ features: { wiki: false, agentSessionAudit: true } })).toBe(true);
    expect(agentSessionAuditOn({ features: { wiki: true, agentSessionAudit: false } })).toBe(false);
    expect(agentSessionAuditOn({ features: { wiki: true } })).toBe(false);
    expect(agentSessionAuditOn(undefined)).toBe(false);
  });
});
