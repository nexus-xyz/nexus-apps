import { describe, expect, it } from "vitest";
import { accountServices, accountsLine, byOffer, canDo, canDoLine, connectionStatus, connectorAction, integrationsOn, whoLine } from "./integrations";

describe("byOffer", () => {
  it("keeps the connectors this Gateway does not offer, for the In preview section", () => {
    const { offered, preview } = byOffer([
      { name: "github", offered: true },
      { name: "jira", offered: false },
      { name: "linear" },
      { name: "notion", offered: false },
    ]);
    expect(offered.map((c) => c.name)).toEqual(["github", "linear"]);
    expect(preview.map((c) => c.name)).toEqual(["jira", "notion"]);
  });
});

describe("whoLine", () => {
  it("says who may use it, as P11·C writes it", () => {
    expect(whoLine({ availability: "enabled_for_all", teams: null }, true, "Acme")).toBe("On for all of Acme");
    expect(whoLine({ availability: "available", teams: null }, true, "Acme")).toBe("People connect their own");
    expect(whoLine({ availability: "disabled", teams: null }, false, "Acme")).toBe("Off");
    expect(whoLine({ availability: "enabled_for_all", teams: ["neuro"] }, true, "Acme", () => "Neuro platform")).toBe("Only Neuro platform");
  });

  it("falls back to whether it is on for you, and to nothing when that is unknown", () => {
    expect(whoLine(undefined, true, "Acme")).toBe("On for you");
    expect(whoLine(undefined, false, "Acme")).toBe("Off for you");
    expect(whoLine(undefined, null, "Acme")).toBeUndefined();
  });
});

describe("connectorAction", () => {
  const drive = { name: "drive", usable: true, connects: ["user" as const], oauth: true, form: null };

  it("asks you to sign in again before anything else", () => {
    expect(connectorAction(drive, [{ connector: "drive", status: "needs_reconnect" }])).toBe("Sign in again");
  });

  it("asks you to connect only one you may use, connect yourself, and have not", () => {
    expect(connectorAction(drive, [])).toBe("Connect");
    expect(connectorAction(drive, [{ connector: "drive", status: "active" }])).toBeUndefined();
    expect(connectorAction({ ...drive, usable: false }, [])).toBeUndefined();
    expect(connectorAction({ ...drive, connects: [] }, [])).toBeUndefined();
  });

  it("names a connection's status", () => {
    expect(connectionStatus({ status: "active" })).toBe("Connected");
    expect(connectionStatus({ status: "needs_reconnect" })).toBe("Sign in again");
    expect(connectionStatus({ status: "active", suspended: true })).toBe("Kept, not used");
  });
});

describe("Assistant A6 on the phone", () => {
  const writes = (enabled: boolean) => ({ enabled, default: false, operations: [{ connector: "linear", operation: "issue_create" }] });

  it("is off unless the build says on", () => {
    expect(integrationsOn(undefined)).toBe(false);
    expect(integrationsOn("1")).toBe(false);
    expect(integrationsOn(" ON ")).toBe(true);
  });

  it("says what Nexus can do in Nexus UI's words, never guessing", () => {
    expect(canDo("linear", undefined, { enabled: true })).toBe("unknown");
    expect(canDo("linear", writes(false), { enabled: true })).toBe("reads");
    expect(canDo("gmail", writes(true), { enabled: true })).toBe("reads");
    expect(canDo("linear", writes(true), undefined)).toBe("unknown");
    expect(canDo("linear", writes(true), { enabled: false })).toBe("writes-ask");
    expect(canDo("linear", writes(true), { enabled: true })).toBe("writes");
    expect(canDoLine("reads", "Acme")).toBe("Reads only");
    expect(canDoLine("writes-ask", "Acme")).toBe("Reads · writes after you approve");
    expect(canDoLine("writes", "Acme")).toBe("Reads · makes the writes Acme allows");
    expect(canDoLine("writes", "Personal")).toBe("Reads · makes the writes you allow");
    expect(canDoLine("unknown", "Acme")).toBe("Reads · could not check its writes");
  });

  it("counts the accounts and the ones that need you", () => {
    expect(accountsLine([])).toBe("0 accounts");
    expect(accountsLine([{ status: "active" }])).toBe("1 account");
    expect(accountsLine([{ status: "active" }, { status: "needs_reconnect" }, { status: "needs_reconnect", suspended: true }])).toBe("3 accounts · 1 needs you");
    expect(accountsLine([{ status: "needs_reconnect" }, { status: "needs_reconnect" }])).toBe("2 accounts · 2 need you");
  });

  it("makes one row per service, the one that needs you first, with its accounts inside", () => {
    const rows = accountServices(
      [
        { connector: "gmail", status: "active" },
        { connector: "gmail", status: "active" },
        { connector: "outlook", status: "needs_reconnect" },
        { connector: "dropbox", status: "active" },
      ],
      (name) => name.charAt(0).toUpperCase() + name.slice(1),
    );
    expect(rows).toEqual([
      { connector: "outlook", count: 1, value: "Sign in again" },
      { connector: "dropbox", count: 1 },
      { connector: "gmail", count: 2, value: "2 accounts" },
    ]);
  });
});
