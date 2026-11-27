import { describe, expect, it } from "vitest";
import {
  holdsSecret,
  vaultActivityOf,
  vaultApprovalOf,
  vaultAskRequestOf,
  vaultIndexOf,
  vaultCardChangeOf,
  vaultChangedOf,
  vaultGrantLabel,
  vaultLimit,
  vaultLoginChangeOf,
  vaultLogWhen,
  vaultMoney,
  vaultRead,
  vaultSpent,
  vaultViewOf,
  vaultWhen,
  type AgentVaultView,
} from "./agent-vault";

const VIEW: AgentVaultView = {
  manager: { kind: "1password", vault: "Maya’s Personal vault" },
  logins: [
    { id: "united", name: "United", site: "united.com", grant: { kind: "each_use" }, lastUsed: "2026-11-17T07:41:00.000Z" },
    { id: "marriott", name: "Marriott Bonvoy", site: "marriott.com", grant: { kind: "until", until: "2026-11-22T23:59:00.000Z" }, lastUsed: null },
  ],
  card: { last4: "4021", state: "active", spent: "142.00", limit: "500.00", period: "month" },
  merchants: [
    { id: "doordash", merchant: "DoorDash", amount: "60", per: "order", approval: "asks_first" },
    { id: "united", merchant: "United", amount: null, per: "booking", approval: "face_id" },
    { id: "amazon", merchant: "Amazon", amount: null, per: "order", approval: "off" },
  ],
  applePay: true,
  asks: [{ id: "g1", login: { id: "united", name: "United", account: "MileagePlus ••• 772" }, manager: "1password", site: "united.com", task: "Check in for UA 1532", until: "2026-11-22T23:59:00.000Z", expiresAt: "2026-11-17T08:11:00.000Z" }],
};

describe("passwords and payments (A8)", () => {
  it("reads the view strictly, and drops fields it does not know", () => {
    expect(vaultViewOf(VIEW)).toEqual(VIEW);
    expect(vaultViewOf({ ...VIEW, manager: null, card: null, logins: [], merchants: [], asks: [] })).toMatchObject({ manager: null, card: null });
    for (const bad of [undefined, {}, { ...VIEW, applePay: "yes" }, { ...VIEW, card: { ...VIEW.card, spent: 142 } }, { ...VIEW, manager: { kind: "lastpass", vault: "x" } }]) {
      expect(vaultViewOf(bad)).toBeUndefined();
    }
  });

  it("refuses any body that carries a secret's shape", () => {
    expect(holdsSecret(VIEW)).toBe(false);
    expect(holdsSecret({ password: "x" })).toBe(true);
    expect(holdsSecret({ card: { cardNumber: "x" } })).toBe(true);
    expect(holdsSecret(["4111 1111 1111 1111"])).toBe(true);
    expect(holdsSecret("4111-1111-1111-1111")).toBe(true);
    expect(vaultViewOf({ ...VIEW, logins: [{ ...VIEW.logins[0], password: "hunter2" }] })).toBeUndefined();
    expect(vaultViewOf({ ...VIEW, logins: [{ ...VIEW.logins[0], name: "Card 4111111111111111" }] })).toBeUndefined();
    // A masked account keeps four digits at most.
    expect(vaultViewOf({ ...VIEW, asks: [{ ...VIEW.asks[0], login: { ...VIEW.asks[0]!.login, account: "MileagePlus 88017729" } }] })).toBeUndefined();
  });

  it("reads off for a 404 or a 503 vault_off, and a failure otherwise", () => {
    expect(vaultRead(200, VIEW)).toEqual({ kind: "view", view: VIEW });
    expect(vaultRead(404, {})).toEqual({ kind: "off" });
    expect(vaultRead(503, { error: "vault_off" })).toEqual({ kind: "off" });
    expect(vaultRead(503, { error: "vault_unavailable" })).toEqual({ kind: "failed", status: 503 });
    expect(vaultRead(200, { manager: 1 })).toEqual({ kind: "failed", status: 502 });
  });

  it("reads a write's answer and the activity", () => {
    expect(vaultChangedOf({ vault: VIEW, stopped: 0, decided: "approved" })).toEqual({ vault: VIEW, stopped: 0, decided: "approved" });
    expect(vaultChangedOf({ vault: VIEW, decided: "maybe" })).toBeUndefined();
    const entry = { at: "2026-11-17T07:41:00.000Z", what: "Checked in UA 1532 · seat 14C", with: "United login · 1Password", approved: "Face ID · iPhone", result: "Done", outcome: "done" };
    expect(vaultActivityOf({ entries: [entry, { ...entry, with: null, approved: null, outcome: "refused" }] })).toHaveLength(2);
    expect(vaultActivityOf({ entries: [{ ...entry, outcome: "lost" }] })).toBeUndefined();
  });

  it("reads each write's body strictly", () => {
    expect(vaultLoginChangeOf({ grant: "none" })).toEqual({ grant: "none" });
    expect(vaultLoginChangeOf({ grant: { until: "2026-11-22T23:59:00.000Z" } })).toEqual({ grant: { until: "2026-11-22T23:59:00.000Z" } });
    for (const bad of [{}, { grant: "always" }, { grant: { until: "soon" } }, { grant: "none", x: 1 }]) expect(vaultLoginChangeOf(bad)).toBeUndefined();
    expect(vaultCardChangeOf({ frozen: true })).toEqual({ frozen: true });
    expect(vaultCardChangeOf({ frozen: "yes" })).toBeUndefined();
    expect(vaultApprovalOf({ for: "once" })).toEqual({ for: "once" });
    expect(vaultApprovalOf({ for: "ever" })).toBeUndefined();
  });

  it("says each row as A8 draws it", () => {
    const now = Date.parse("2026-11-17T12:00:00.000Z");
    expect(vaultGrantLabel({ kind: "each_use" })).toBe("Each use · Face ID");
    expect(vaultGrantLabel({ kind: "until", until: "2026-11-08T12:00:00.000Z" })).toBe("Until Sun 8 Nov");
    expect(vaultWhen("2026-11-17T07:41:00.000Z", now)).toBe("Today 07:41");
    expect(vaultWhen("2026-11-17T07:41:00.000Z", now, "America/Los_Angeles")).toBe("Mon");
    expect(vaultWhen("2026-11-12T07:41:00.000Z", now)).toBe("Thu");
    expect(vaultWhen("2026-10-12T07:41:00.000Z", now)).toBe("12 Oct");
    expect(vaultWhen(null, now)).toBe("never");
    expect(vaultLogWhen("2026-11-17T07:41:00.000Z", now)).toBe("07:41");
    expect(vaultLogWhen("2026-11-12T19:02:00.000Z", now)).toBe("Thu 19:02");
    expect(vaultMoney("38.60")).toBe("$38.60");
    expect(vaultMoney("1500.00")).toBe("$1,500");
    expect(vaultSpent(VIEW.card!)).toBe("$142 of $500 this month");
    expect(VIEW.merchants.map(vaultLimit)).toEqual(["$60 an order", "Each booking", "Off"]);
  });
});

describe("the desktop's vault index", () => {
  const INDEX = {
    device: "Maya’s MacBook Pro",
    sources: [{ kind: "nexus", vault: "Nexus vault", total: 2 }, { kind: "1password", vault: "Personal", total: 1 }],
    logins: [
      { id: "united", name: "United", site: "united.com", account: "m•••@gmail.com", source: "1password", grant: { kind: "each_use" }, lastUsed: null },
      { id: "pge", name: "PG&E", site: "pge.com", source: "nexus", grant: null, lastUsed: "2026-11-17T07:41:00.000Z", flag: "weak" },
    ],
    activity: [{ at: "2026-11-17T07:41:00.000Z", what: "Added the PG&E login", with: "Nexus vault", approved: null, result: "Done", outcome: "done" }],
  };

  it("reads an index strictly, and an empty one after Delete all", () => {
    expect(vaultIndexOf(INDEX)).toEqual(INDEX);
    expect(vaultIndexOf({ ...INDEX, sources: [], logins: [], activity: [] })).toEqual({ ...INDEX, sources: [], logins: [], activity: [] });
    const twice = { ...INDEX, logins: [INDEX.logins[0], INDEX.logins[0]] };
    const sourceless = { ...INDEX, logins: [{ ...INDEX.logins[1], source: undefined }] };
    for (const bad of [undefined, [], { ...INDEX, device: "" }, { ...INDEX, extra: 1 }, twice, sourceless, { ...INDEX, logins: [{ ...INDEX.logins[0], note: "x" }] }]) {
      expect(vaultIndexOf(bad)).toBeUndefined();
    }
  });

  it("refuses an index that holds a secret anywhere", () => {
    expect(vaultIndexOf({ ...INDEX, logins: [{ ...INDEX.logins[0], password: "hunter2" }] })).toBeUndefined();
    expect(vaultIndexOf({ ...INDEX, logins: [{ ...INDEX.logins[0], account: "4111 1111 1111 1111" }] })).toBeUndefined();
    expect(vaultIndexOf({ ...INDEX, logins: [{ ...INDEX.logins[0], account: "member 88017729" }] })).toBeUndefined();
    expect(vaultIndexOf({ ...INDEX, activity: [{ ...INDEX.activity[0], what: "Paid with 4111111111111111" }] })).toBeUndefined();
    expect(vaultIndexOf({ ...INDEX, sources: [{ kind: "nexus", vault: "x", secret: "y" }] })).toBeUndefined();
  });

  it("reads the view's new fields, and still reads an older view", () => {
    const view = { manager: INDEX.sources[1], sources: INDEX.sources, device: INDEX.device, logins: INDEX.logins, card: null, merchants: [], applePay: false, asks: [] };
    expect(vaultViewOf(view)).toEqual(view);
    expect(vaultViewOf({ ...view, sources: [{ kind: "lastpass", vault: "x" }] })).toBeUndefined();
    expect(vaultViewOf({ ...view, logins: [{ ...INDEX.logins[0], source: "lastpass" }] })).toBeUndefined();
    expect(vaultViewOf(VIEW)).toEqual(VIEW);
  });

  it("reads an ask's body strictly", () => {
    expect(vaultAskRequestOf({ login: "united", task: "Check in" })).toEqual({ login: "united", task: "Check in" });
    expect(vaultAskRequestOf({ login: "united", task: "Check in", until: "2026-11-22T23:59:00.000Z" })).toMatchObject({ until: "2026-11-22T23:59:00.000Z" });
    for (const bad of [{}, { login: "a b", task: "x" }, { login: "united", task: "" }, { login: "united", task: "x", until: "soon" }, { login: "united", task: "x", password: "y" }]) {
      expect(vaultAskRequestOf(bad)).toBeUndefined();
    }
  });
});
