import { describe, expect, it } from "vitest";
import { createArtifactsClient } from "./artifacts";
import type {
  SkillLibraryList,
  SkillPromotion,
  SkillPromotionDetail,
  SkillPromotionErrorCode,
  SkillPromotionPrevious,
} from "./skills";

/** A file as a careless reader loses it: a byte-order mark, CRLF line ends and no final newline. */
const AWKWARD = new Uint8Array([0xef, 0xbb, 0xbf, ...new TextEncoder().encode("---\r\nname: a\r\n---\r\n\r\nBody")]);

describe("the skills calls on the artifacts client", () => {
  it("exports a version as the bytes Gateway stored, not as decoded text", async () => {
    const seen: { url: string; init?: RequestInit }[] = [];
    const client = createArtifactsClient({
      baseUrl: "https://gateway.test/",
      apiKey: "nx_key",
      fetch: async (url, init) => {
        seen.push({ url, init });
        return new Response(AWKWARD, { headers: { "content-type": "text/markdown; charset=utf-8" } });
      },
    });
    const bytes = await client.exportSkill("skl_1", 2);
    expect(new Uint8Array(bytes)).toEqual(AWKWARD);
    expect(seen[0]?.url).toBe("https://gateway.test/v1/skills/skl_1/versions/2/export");
    expect(new Headers(seen[0]?.init?.headers).get("authorization")).toBe("Bearer nx_key");
  });

  it("writes the next version on a base, and creates and deletes by the skill's route", async () => {
    const calls: { method?: string; url: string; body?: unknown }[] = [];
    const client = createArtifactsClient({
      baseUrl: "https://gateway.test",
      fetch: async (url, init) => {
        calls.push({ method: init?.method, url, body: init?.body ? JSON.parse(String(init.body)) : undefined });
        return new Response("{}", { headers: { "content-type": "application/json" } });
      },
    });
    await client.createSkill({ content: "---\n" });
    await client.writeSkillVersion("skl_1", { content: "next", baseVersion: 2 });
    await client.skillVersion("skl_1", "latest");
    await client.deleteSkill("skl/1");
    expect(calls).toEqual([
      { method: "POST", url: "https://gateway.test/v1/skills", body: { content: "---\n" } },
      { method: "POST", url: "https://gateway.test/v1/skills/skl_1/versions", body: { content: "next", baseVersion: 2 } },
      { method: "GET", url: "https://gateway.test/v1/skills/skl_1/versions/latest", body: undefined },
      { method: "DELETE", url: "https://gateway.test/v1/skills/skl%2F1", body: undefined },
    ]);
  });

  it("encodes the skill id in every route that has one", async () => {
    const urls: string[] = [];
    const client = createArtifactsClient({
      baseUrl: "https://gateway.test",
      fetch: async (url) => {
        urls.push(url);
        return new Response("{}", { headers: { "content-type": "application/json" } });
      },
    });
    await client.skill("skl/1");
    await client.writeSkillVersion("skl/1", { content: "next" });
    await client.skillVersion("skl/1", 2);
    await client.exportSkill("skl/1", 2);
    await client.deleteSkill("skl/1");
    expect(urls).toEqual([
      "https://gateway.test/v1/skills/skl%2F1",
      "https://gateway.test/v1/skills/skl%2F1/versions",
      "https://gateway.test/v1/skills/skl%2F1/versions/2",
      "https://gateway.test/v1/skills/skl%2F1/versions/2/export",
      "https://gateway.test/v1/skills/skl%2F1",
    ]);
  });

  it("asks for, lists, reads, decides and withdraws a promotion by its routes, and keys every write", async () => {
    const calls: { method?: string; url: string; body?: unknown; key?: string | null }[] = [];
    const client = createArtifactsClient({
      baseUrl: "https://gateway.test",
      apiKey: "nx_key",
      fetch: async (url, init) => {
        calls.push({
          method: init?.method,
          url,
          body: init?.body ? JSON.parse(String(init.body)) : undefined,
          key: new Headers(init?.headers).get("idempotency-key"),
        });
        return new Response("{}", { headers: { "content-type": "application/json" } });
      },
    });
    await client.requestSkillPromotion("skl/1", "latest", { scope: "team", team: "web", note: "Ready." });
    await client.requestSkillPromotion("skl_1", 2, { scope: "org" });
    await client.skillPromotions();
    await client.skillPromotions({ state: "held", scope: "team", team: "web team", mine: true, limit: 20, cursor: "abc=" });
    await client.skillPromotions({ mine: false, state: undefined });
    await client.skillPromotion("spr/1");
    await client.decideSkillPromotion("spr_1", { decision: "refused", reason: "Too broad.", reviewed: { content: true } });
    await client.withdrawSkillPromotion("spr_1");
    await client.withdrawSkillPromotion("spr_2", { reason: "Retired.", allVersions: true });
    expect(calls.map(({ key, ...call }) => call)).toEqual([
      { method: "POST", url: "https://gateway.test/v1/skills/skl%2F1/versions/latest/promotions", body: { scope: "team", team: "web", note: "Ready." } },
      { method: "POST", url: "https://gateway.test/v1/skills/skl_1/versions/2/promotions", body: { scope: "org" } },
      { method: "GET", url: "https://gateway.test/v1/skill-promotions", body: undefined },
      {
        method: "GET",
        url: "https://gateway.test/v1/skill-promotions?state=held&scope=team&team=web+team&mine=true&limit=20&cursor=abc%3D",
        body: undefined,
      },
      { method: "GET", url: "https://gateway.test/v1/skill-promotions", body: undefined },
      { method: "GET", url: "https://gateway.test/v1/skill-promotions/spr%2F1", body: undefined },
      {
        method: "POST",
        url: "https://gateway.test/v1/skill-promotions/spr_1/decision",
        body: { decision: "refused", reason: "Too broad.", reviewed: { content: true } },
      },
      { method: "POST", url: "https://gateway.test/v1/skill-promotions/spr_1/withdraw", body: {} },
      { method: "POST", url: "https://gateway.test/v1/skill-promotions/spr_2/withdraw", body: { reason: "Retired.", allVersions: true } },
    ]);
    // A write carries a key of its own, so a retry replays the first answer instead of writing twice.
    const keys = calls.filter((call) => call.method === "POST").map((call) => call.key);
    expect(keys.every((key) => key?.startsWith("ui_"))).toBe(true);
    expect(new Set(keys).size).toBe(keys.length);
    expect(calls.filter((call) => call.method === "GET").every((call) => call.key === null)).toBe(true);
  });

  it("lists the library by its route, and reads a library skill by the routes of a skill", async () => {
    const library: SkillLibraryList = {
      skills: [
        {
          id: "skl_org",
          name: "release-notes",
          description: "Does release-notes work for the team.",
          scope: "org",
          version: 2,
          approvedAt: "2026-10-28T10:30:00.000Z",
        },
        {
          id: "skl_web",
          name: "triage",
          description: "Does triage work for the team.",
          scope: "team",
          teamId: "team_web",
          version: 1,
          approvedAt: "2026-10-28T10:31:00.000Z",
        },
      ],
    };
    const calls: { method?: string; url: string; key: string | null }[] = [];
    const client = createArtifactsClient({
      baseUrl: "https://gateway.test",
      apiKey: "nx_key",
      fetch: async (url, init) => {
        calls.push({ method: init?.method, url, key: new Headers(init?.headers).get("idempotency-key") });
        return new Response(JSON.stringify(library), { headers: { "content-type": "application/json" } });
      },
    });
    expect(await client.skillLibrary()).toEqual(library);
    await client.skill("skl_org");
    await client.skillVersion("skl_org", "latest");
    await client.exportSkill("skl_web", 1);
    expect(calls).toEqual([
      { method: "GET", url: "https://gateway.test/v1/skill-library", key: null },
      { method: "GET", url: "https://gateway.test/v1/skills/skl_org", key: null },
      { method: "GET", url: "https://gateway.test/v1/skills/skl_org/versions/latest", key: null },
      { method: "GET", url: "https://gateway.test/v1/skills/skl_web/versions/1/export", key: null },
    ]);
  });

  it("throws Gateway's typed error with its code and details", async () => {
    const client = createArtifactsClient({
      baseUrl: "https://gateway.test",
      fetch: async () =>
        new Response(
          JSON.stringify({
            error: {
              message: "nobody approves their own request",
              type: "permission_error",
              code: "self_approval",
              nexus: { promotionId: "spr_1", state: "held" },
            },
          }),
          { status: 403, headers: { "content-type": "application/json" } },
        ),
    });
    const error = await client.decideSkillPromotion("spr_1", { decision: "approved" }).catch((caught: unknown) => caught);
    expect(error).toMatchObject({
      status: 403,
      code: "self_approval",
      message: "nobody approves their own request",
      details: { promotionId: "spr_1", state: "held" },
    });
  });

  it("reads a previous version without its provenance, and with it for an approver", async () => {
    const promotion: SkillPromotion = {
      id: "spr_2",
      state: "held",
      scope: "org",
      name: "release-notes",
      description: "Does release-notes work for the team.",
      contentSha256: "b".repeat(64),
      bytes: 90,
      sourceSkillId: "skl_1",
      sourceVersion: 2,
      sourceWrittenVia: "web",
      sourceWrittenAt: "2026-10-28T10:00:00.000Z",
      requestedBy: "usr_mo",
      requestedVia: "web",
      requestedAt: "2026-10-28T11:00:00.000Z",
      selfApproved: false,
    };
    // What someone who loads the library reads: no `promotion`, so no requester, note, approver, reason or review.
    const reader: SkillPromotionPrevious = {
      id: "spr_1",
      name: "release-notes",
      description: "Does release-notes work for the team.",
      contentSha256: "a".repeat(64),
      libraryVersion: 1,
      decidedAt: "2026-10-28T10:30:00.000Z",
      content: "---\nname: release-notes\n---\n",
    };
    const approver: SkillPromotionPrevious = { ...reader, promotion: { ...promotion, id: "spr_1", state: "approved", decidedBy: "usr_ada", decisionReason: "Reads well." } };
    const detail = (previous: SkillPromotionPrevious): SkillPromotionDetail => ({ ...promotion, content: "…", previous });
    for (const previous of [reader, approver]) {
      const client = createArtifactsClient({
        baseUrl: "https://gateway.test",
        apiKey: "nx_key",
        fetch: async () => new Response(JSON.stringify(detail(previous)), { headers: { "content-type": "application/json" } }),
      });
      expect((await client.skillPromotion("spr_2")).previous).toEqual(previous);
    }
    expect("promotion" in reader).toBe(false);
    // The typed errors a client may branch on include the cursor code every list shares.
    const codes: SkillPromotionErrorCode[] = ["invalid_cursor", "invalid_promotion", "person_session_required"];
    expect(codes).toContain("invalid_cursor");
  });
});
