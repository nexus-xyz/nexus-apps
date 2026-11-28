import { describe, expect, it } from "vitest";
import { pausedLine, pauseViewOf, privacyViewOf, revokedText, revokeResultOf, revokeText, type AgentPrivacyView } from "./agent-privacy";

const VIEW: AgentPrivacyView = {
  pause: { paused: false },
  links: [
    { id: "agl_slack", agent: "slack", label: "Slack", createdAt: "2026-11-01T00:00:00.000Z" },
    { id: "agl_session", agent: "nexus-assistant", label: "Nexus Assistant writes", createdAt: "2026-11-17T13:00:00.000Z", lastUsedAt: "2026-11-17T13:05:00.000Z" },
  ],
  accounts: [
    { connector: "gmail", account: "ann@gmail.test", scope: "user", reads: ["Search your mail"], writes: ["Send a message"], never: ["Delete a message"] },
    { connector: "linear", account: "Acme", scope: "org", reads: ["List issues"], writes: [], never: [] },
  ],
};

describe("the privacy readers (A11)", () => {
  it("reads the pause strictly", () => {
    expect(pauseViewOf({ paused: true, pausedAt: "2026-11-17T14:02:00.000Z" })).toEqual({ paused: true, pausedAt: "2026-11-17T14:02:00.000Z" });
    expect(pauseViewOf({ paused: false, pausedAt: "2026-11-17T14:02:00.000Z" })).toEqual({ paused: false });
    expect(pauseViewOf({ paused: "yes" })).toBeUndefined();
  });

  it("reads the view, and refuses one with a part that is not what the agent API sends", () => {
    expect(privacyViewOf(JSON.parse(JSON.stringify(VIEW)))).toEqual(VIEW);
    expect(privacyViewOf({ ...VIEW, accounts: [{ ...VIEW.accounts[0], scope: "team" }] })).toBeUndefined();
    expect(privacyViewOf({ ...VIEW, links: [{ id: "agl_1" }] })).toBeUndefined();
    expect(privacyViewOf({ ...VIEW, linksError: "Gateway failed" })?.linksError).toBe("Gateway failed");
  });

  it("reads what Revoke all access did", () => {
    const result = { links: { ended: [VIEW.links[0]], failed: [{ id: "agl_session", error: "Gateway failed" }] }, accounts: { disconnected: [{ connector: "gmail", account: "ann@gmail.test" }], failed: [] } };
    expect(revokeResultOf(result)).toEqual(result);
    expect(revokeResultOf({ links: {} })).toBeUndefined();
    expect(revokedText(revokeResultOf(result)!)).toBe("Ended 1 agent link and disconnected 1 account. 1 did not end. Try again.");
  });
});

describe("the privacy words (A11·b)", () => {
  it("says exactly what Revoke all access ends, and that the organization's accounts stay", () => {
    expect(revokeText(VIEW)).toBe(
      "Ends your 2 agent links: Slack and Nexus Assistant writes. Disconnects your account: Gmail (ann@gmail.test). Your organization's account stays connected. You can reconnect one at a time.",
    );
    expect(revokeText({ links: [], accounts: [] })).toBe("You have no agent links to end. You have no accounts of your own to disconnect. You can reconnect one at a time.");
  });

  it("names when the pause began", () => {
    expect(pausedLine({ paused: true, pausedAt: "2026-11-17T14:02:00.000Z" }, () => "14:02")).toBe("Paused at 14:02 · nothing runs until you resume");
    expect(pausedLine({ paused: true }, () => "never")).toBe("Paused · nothing runs until you resume");
  });
});
