import { vaultViewOf, type AgentVaultView } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { APPROVE_ON_DESKTOP, cardText, decidedText, deviceLine, failedText, LIVES_ON_DESKTOP, loginSite, managerLine, pageOf, sourceLine, sourcesOf, UNLINKED } from "./vault-words";

// Passwords and payments on the phone (A8): what the page shows for each read, and what each write says.

const VIEW: AgentVaultView = {
  manager: { kind: "1password", vault: "Maya’s Personal vault" },
  logins: [
    { id: "united", name: "United", site: "united.com", grant: { kind: "each_use" }, lastUsed: null },
    { id: "pge", name: "PG&E", site: "pge.com", grant: null, lastUsed: null },
  ],
  card: { last4: "4021", state: "active", spent: "142", limit: "500", period: "month" },
  merchants: [],
  applePay: true,
  asks: [],
};

describe("Passwords and payments on the phone", () => {
  it("shows nothing linked where the agent API has no vault, and a failure as one", () => {
    expect(pageOf({ kind: "off" })).toEqual({ view: UNLINKED, unlinked: true });
    expect(pageOf({ kind: "view", view: VIEW })).toEqual({ view: VIEW, unlinked: false });
    expect(pageOf({ kind: "failed", status: 502 })).toBe("failed");
  });

  it("never reads a body shaped like a secret: the kit's reader refuses it", () => {
    expect(vaultViewOf({ ...VIEW, logins: [{ ...VIEW.logins[0], password: "hunter2" }] })).toBeUndefined();
    expect(vaultViewOf({ ...VIEW, manager: { kind: "1password", vault: "4111 1111 1111 1111" } })).toBeUndefined();
  });

  it("names the password manager and the logins it may use, or says none is linked", () => {
    expect(managerLine(VIEW, false)).toEqual({ title: "1Password · Maya’s Personal vault", line: "Linked on your desktop. Nexus can ask to use 1 login. It never reads, stores or repeats them." });
    expect(managerLine(UNLINKED, true).title).toBe("No password manager is linked");
    expect(managerLine(UNLINKED, true).line).toContain("Link 1Password or turn on the Nexus vault in the Nexus desktop app.");
    expect(managerLine(UNLINKED, false).line).toContain("until you link one");
  });

  it("names the desktop, its sources, and each login's site, masked account and source", () => {
    const desktop: AgentVaultView = {
      ...VIEW,
      device: "Maya’s MacBook Pro",
      sources: [
        { kind: "1password", vault: "Personal", total: 12 },
        { kind: "nexus", vault: "Nexus vault", total: 2 },
      ],
      logins: [
        { id: "united", name: "United", site: "united.com", account: "m•••@gmail.com", source: "1password", grant: { kind: "each_use" }, lastUsed: null },
        { id: "pge", name: "PG&E", site: "pge.com", source: "nexus", grant: null, lastUsed: null, flag: "weak" },
      ],
    };
    expect(managerLine(desktop, false)).toEqual({
      title: "1Password · Personal · 12 logins and Nexus vault · 2 logins",
      line: "Linked on Maya’s MacBook Pro. Nexus can ask to use 1 login. It never reads, stores or repeats them.",
    });
    expect(loginSite(desktop.logins[0]!)).toBe("united.com · m•••@gmail.com · 1Password");
    expect(loginSite(desktop.logins[1]!)).toBe("pge.com · Nexus vault");
    expect(sourcesOf(VIEW)).toEqual([VIEW.manager]);
    expect(deviceLine(VIEW)).toBe("Linked on your desktop.");
    expect(sourceLine({ kind: "nexus", vault: "Nexus vault", total: 1 })).toBe("Nexus vault · 1 login");
  });

  it("says logins and grants live on the desktop, never on the phone", () => {
    expect(LIVES_ON_DESKTOP).toContain("Nexus desktop app");
    expect(APPROVE_ON_DESKTOP).toBe("Approve it in the Nexus desktop app, with Touch ID.");
    expect(vaultViewOf({ ...VIEW, device: "Mac", logins: [{ ...VIEW.logins[0], account: "4111 1111 1111 1111" }] })).toBeUndefined();
  });

  it("says what each write did, in Nexus UI's words", () => {
    expect(cardText({ vault: { ...VIEW, card: { ...VIEW.card!, state: "frozen" } } })).toBe("The card is frozen. Nothing can be paid with it.");
    const ask = { manager: "1password" as const, site: "united.com" };
    expect(decidedText(ask, { vault: VIEW, decided: "approved" })).toBe("Approved. 1Password fills it in on united.com.");
    expect(decidedText({ ...ask, manager: "nexus" }, { vault: VIEW })).toBe("Approved. The Nexus vault fills it in on united.com.");
    expect(decidedText(ask, { vault: VIEW, decided: "confirm_on_device" })).toBe("Confirm with Face ID on your phone to finish.");
    expect(failedText({ kind: "failed", status: 410, reason: "gone" })).toBe("That request is no longer waiting. Ask Nexus again.");
    expect(failedText({ kind: "failed", status: 503, reason: "off" })).toBe("No password manager or card is linked to Nexus here yet.");
  });
});
