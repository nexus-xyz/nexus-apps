import type { Artifact, ArtifactsClient } from "@nexus/kit";
import { describe, expect, it, vi } from "vitest";
import { absoluteLink, accessLine, addEmails, audienceChoices, audienceLabel, expiryChoices, inTreeOrder, parseEmails, personState, shareAs, shareButton, shareRequest, sharingSettings, whoHasAccess } from "./share";

describe("share", () => {
  it("keeps an absolute link and resolves a path against Nexus UI", () => {
    expect(absoluteLink("https://nexus.xyz/a/tok", "https://x.dev")).toBe("https://nexus.xyz/a/tok");
    expect(absoluteLink("/app/acme/artifacts/art_1", "https://nexus.xyz/")).toBe("https://nexus.xyz/app/acme/artifacts/art_1");
  });

  it("says who an artifact is shared with", () => {
    expect(audienceLabel({ kind: "org", teams: [], people: [] })).toBe("Everyone in the organization");
    expect(audienceLabel({ kind: "teams", teams: ["design"], people: [] })).toBe("Team design");
    expect(audienceLabel({ kind: "external", teams: [], people: ["a@x.com", "b@y.com"] })).toBe("2 people outside");
  });
});

describe("audienceChoices", () => {
  const policy = (patch: Parameters<typeof sharingSettings>[0] = {}) => sharingSettings(patch);

  it("offers every audience under the defaults, the organization first", () => {
    const choices = audienceChoices({ policy: policy(), role: "member" });
    expect(choices.map((c) => c.kind)).toEqual(["org", "teams", "people", "external", "public"]);
    expect(choices.every((c) => !c.refusal)).toBe(true);
  });

  it("names the organization, and groups the audiences inside it before those outside", () => {
    const choices = audienceChoices({ policy: policy(), role: "member", orgName: "Acme" });
    expect(choices[0].label).toBe("Everyone in Acme");
    expect(choices.map((c) => c.outside)).toEqual([false, false, false, true, true]);
  });

  it("leaves out the organization in a personal one, where it means only you", () => {
    expect(audienceChoices({ policy: policy(), role: "owner", personal: true }).map((c) => c.kind)).toEqual(["teams", "people", "external", "public"]);
  });

  it("refuses what the organization's settings forbid, with the reason", () => {
    const named = audienceChoices({ policy: policy({ "artifacts.external": "named" }), role: "member" });
    expect(named.find((c) => c.kind === "external")?.refusal).toBeUndefined();
    expect(named.find((c) => c.kind === "public")?.refusal).toMatch(/named people only/);

    const off = audienceChoices({ policy: policy({ "artifacts.external": "off" }), role: "owner" });
    expect(off.filter((c) => c.refusal).map((c) => c.kind)).toEqual(["external", "public"]);

    const admins = audienceChoices({ policy: policy({ "artifacts.external.who": "admins" }), role: "member" });
    expect(admins.find((c) => c.kind === "external")?.refusal).toMatch(/owners and admins/);
    expect(audienceChoices({ policy: policy({ "artifacts.external.who": "admins" }), role: "admin" }).every((c) => !c.refusal)).toBe(true);
  });

  it("refuses a page's public link when pages may not have one", () => {
    const page = audienceChoices({ policy: policy({ "artifacts.pages.public": false }), role: "owner", page: true });
    expect(page.find((c) => c.kind === "public")?.refusal).toMatch(/public links for pages/);
    expect(audienceChoices({ policy: policy({ "artifacts.pages.public": false }), role: "owner" }).find((c) => c.kind === "public")?.refusal).toBeUndefined();
  });
});

describe("inTreeOrder", () => {
  it("puts each team before the teams under it, whatever order Gateway lists them in", () => {
    const flat = [{ slug: "eng" }, { slug: "ops" }, { slug: "platform", parent: "eng" }, { slug: "core", parent: "platform" }, { slug: "lost", parent: "gone" }];
    expect(inTreeOrder(flat).map((t) => t.slug)).toEqual(["eng", "platform", "core", "ops", "lost"]);
  });

  it("keeps teams the walk never reaches, such as a cycle, at the end", () => {
    const cycle = [{ slug: "eng" }, { slug: "a", parent: "b" }, { slug: "b", parent: "a" }];
    expect(inTreeOrder(cycle).map((t) => t.slug)).toEqual(["eng", "a", "b"]);
  });
});

describe("shareButton", () => {
  const none = { teams: [], people: [], outside: [] };
  const name = (value: string) => ({ platform: "Platform", "a@acme.example": "Ana" })[value] ?? value;

  it("says whom it shares with, by name for one", () => {
    expect(shareButton("teams", { ...none, teams: ["platform"] }, name)).toEqual({ label: "Share with Platform", ready: true });
    expect(shareButton("teams", { ...none, teams: ["platform", "core"] }, name)).toEqual({ label: "Share with 2 teams", ready: true });
    expect(shareButton("people", { ...none, people: ["a@acme.example"] }, name)).toEqual({ label: "Share with Ana", ready: true });
    expect(shareButton("external", { ...none, outside: ["x@partner.com", "y@partner.com"] }, name)).toEqual({ label: "Share with 2 people", ready: true });
    expect(shareButton("public", none, name)).toEqual({ label: "Create the link", ready: true });
  });

  it("counts an address still in the field once it is a whole one", () => {
    expect(shareButton("external", { ...none, outside: ["x@partner.com"], draft: "y@partner.com" }, name)).toEqual({ label: "Share with 2 people", ready: true });
    expect(shareButton("external", { ...none, outside: ["x@partner.com"], draft: "y@part" }, name)).toEqual({ label: "Finish the address", ready: false });
  });

  it("says what is missing until it can share", () => {
    expect(shareButton("teams", none, name)).toEqual({ label: "Choose a team", ready: false });
    expect(shareButton("people", none, name)).toEqual({ label: "Choose a person", ready: false });
    expect(shareButton("external", none, name)).toEqual({ label: "Add an email address", ready: false });
  });
});

describe("addEmails", () => {
  it("adds each address typed or pasted, once, and hands back what is not one", () => {
    expect(addEmails(["a@partner.com"], "B@Client.io, a@partner.com nope")).toEqual({ list: ["a@partner.com", "b@client.io"], invalid: ["nope"] });
    expect(addEmails([], " ")).toEqual({ list: [], invalid: [] });
  });
});

describe("shareRequest", () => {
  const none = { teams: [], people: [], outside: [] };

  it("sends the organization and a public link at once, with the expiry chosen", () => {
    expect(shareRequest("org", none)).toEqual({ audience: "org" });
    expect(shareRequest("public", { ...none, expiresIn: "7d" })).toEqual({ audience: "public", expiresIn: "7d" });
    expect(shareRequest("public", { ...none, expiresIn: "" })).toEqual({ audience: "public" });
  });

  it("waits for a team, a person or a valid email", () => {
    expect(shareRequest("teams", none)).toBeUndefined();
    expect(shareRequest("teams", { ...none, teams: ["design"] })).toEqual({ audience: "teams", teams: ["design"] });
    expect(shareRequest("people", { ...none, people: ["b@acme.example"] })).toEqual({ audience: "people", people: ["b@acme.example"] });
    expect(shareRequest("external", none)).toBeUndefined();
    expect(shareRequest("external", { ...none, outside: ["a@partner.com"], draft: "nope" })).toBeUndefined();
    expect(shareRequest("external", { ...none, outside: ["a@partner.com"], draft: "B@client.io a@partner.com" })).toEqual({ audience: "external", people: ["a@partner.com", "b@client.io"] });
  });

  it("parses typed emails as Nexus UI does", () => {
    expect(parseEmails(" a@x.com; bad, A@x.com ")).toEqual({ emails: ["a@x.com"], invalid: ["bad"] });
  });

  it("keeps a public link's expiry within the organization's maximum", () => {
    expect(expiryChoices({ maxDays: null, required: false }).map((c) => c.value)).toEqual(["", "1d", "7d", "30d", "90d", "365d"]);
    expect(expiryChoices({ maxDays: 14, required: true }).map((c) => c.value)).toEqual(["1d", "7d", "14d"]);
  });
});

const artifact = (patch: Partial<Artifact>): Artifact =>
  ({ id: "art_1", version: 1, status: "active", audience: { kind: "org", teams: [], people: [] }, createdAt: "2026-10-27T10:00:00Z", ...patch }) as Artifact;

describe("shareAs", () => {
  it("shares a chat in one step that replaces what was shared before", async () => {
    const made = artifact({ id: "art_2", version: 2 });
    const client = { shareChatSession: vi.fn().mockResolvedValue(made), sharePage: vi.fn(), revokeArtifact: vi.fn() };
    await expect(shareAs(client, { kind: "chat", id: "ses_1" }, { audience: "teams", teams: ["design"] }, [artifact({})])).resolves.toBe(made);
    expect(client.shareChatSession).toHaveBeenCalledWith("ses_1", { audience: "teams", teams: ["design"], replace: true });
    expect(client.revokeArtifact).not.toHaveBeenCalled();
  });

  it("shares a page, then revokes its earlier shares, so one audience stays open", async () => {
    const made = artifact({ id: "art_3" });
    const client = { shareChatSession: vi.fn(), sharePage: vi.fn().mockResolvedValue(made), revokeArtifact: vi.fn().mockResolvedValue({}) };
    await shareAs(client, { kind: "page", id: "pg_1" }, { audience: "public", expiresIn: "7d" }, [artifact({ id: "art_1" }), artifact({ id: "art_3" })]);
    expect(client.sharePage).toHaveBeenCalledWith("pg_1", { audience: "public", expiresIn: "7d" });
    expect(client.revokeArtifact.mock.calls).toEqual([["art_1"]]);
  });
});

describe("whoHasAccess", () => {
  it("lists the active shares of yours from the chat or page, newest first", async () => {
    const older = artifact({ id: "art_1", createdAt: "2026-10-26T10:00:00Z" });
    const newer = artifact({ id: "art_2", createdAt: "2026-10-27T10:00:00Z" });
    const client = { artifacts: vi.fn().mockResolvedValue({ artifacts: [older, newer] }) } as unknown as Pick<ArtifactsClient, "artifacts">;
    await expect(whoHasAccess(client, { kind: "chat", id: "ses_1" })).resolves.toEqual([newer, older]);
    expect(client.artifacts).toHaveBeenCalledWith({ scope: "shared_by_me", session: "ses_1", status: "active", limit: 50 });
  });

  it("reads a page's shares by the page's id, which Gateway files them under", async () => {
    const client = { artifacts: vi.fn().mockResolvedValue({ artifacts: [] }) } as unknown as Pick<ArtifactsClient, "artifacts">;
    await whoHasAccess(client, { kind: "page", id: "pg_1" });
    expect(client.artifacts).toHaveBeenCalledWith({ scope: "shared_by_me", session: "pg_1", status: "active", limit: 50 });
  });
});

describe("access", () => {
  it("says how often a share outside was opened, and each person's state", () => {
    expect(accessLine({ audience: "external", opens: 1, reports: 0 })).toBe("Opened once");
    expect(accessLine({ audience: "external", opens: 0, reports: 0 })).toBe("Not opened yet");
    expect(accessLine({ audience: "public", opens: 3, reports: 2 })).toBe("Opened 3 times · 2 reports");
    expect(personState({ status: "requested", opens: 0 })).toBe("Asked for access");
    expect(personState({ status: "denied", opens: 0 })).toBe("Denied");
    expect(personState({ status: "granted", opens: 0 })).toBe("Not opened yet");
    expect(personState({ status: "granted", opens: 2 })).toBe("Opened 2 times");
  });
});
