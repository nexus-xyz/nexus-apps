import { describe, expect, it } from "vitest";
import { createArtifactsClient } from "./artifacts";
import { createCoreClient } from "./core-client";
import { Etags, needsIfMatch, readPathOf } from "./if-match";
import type { NotificationSettingChange } from "./notifications";

describe("needsIfMatch", () => {
  it("names only the fifteen edits", () => {
    expect(needsIfMatch("PATCH", "/v1/chat/sessions/s1")).toBe(true);
    expect(needsIfMatch("PUT", "/v1/policies/grants/robustness/review")).toBe(true);
    expect(needsIfMatch("PUT", "/v1/policies/grants/robustness")).toBe(false);
    expect(needsIfMatch("DELETE", "/v1/teams/web%2Fa")).toBe(true);
    expect(needsIfMatch("PUT", "/v1/people/me/budget")).toBe(true);
    expect(needsIfMatch("put", "/v1/orgs/settings?x=1")).toBe(true);
    // A sub-resource, another verb, or an empty segment is not one of them.
    expect(needsIfMatch("POST", "/v1/chat/sessions/s1/messages")).toBe(false);
    expect(needsIfMatch("DELETE", "/v1/chat/sessions/s1/share")).toBe(false);
    expect(needsIfMatch("GET", "/v1/orgs/settings")).toBe(false);
    expect(needsIfMatch("POST", "/v1/teams")).toBe(false);
    expect(needsIfMatch("PATCH", "/v1/teams/")).toBe(false);
    expect(needsIfMatch("PUT", "/v1/billing/auto-topup")).toBe(false);
  });
});

describe("Etags", () => {
  it("gives every connector grant set the version of the grant list", () => {
    const etags = new Etags();
    expect(readPathOf("/v1/policies/grants/robustness/review")).toBe("/v1/policies/grants");
    expect(readPathOf("/v1/orgs/settings?x=1")).toBe("/v1/orgs/settings");
    etags.learn("GET", "/v1/policies/grants", 200, '"3"');
    expect(etags.get("/v1/policies/grants/robustness/review")).toBe('"3"');
    // A set of one module answers the version every module's next set needs.
    etags.learn("PUT", "/v1/policies/grants/robustness/review", 200, '"4"');
    expect(etags.get("/v1/policies/grants/intelligence/wiki")).toBe('"4"');
    etags.learn("PUT", "/v1/policies/grants/intelligence/wiki", 412, '"5"');
    expect(etags.get("/v1/policies/grants/robustness/review")).toBeUndefined();
  });

  it("keeps the last version of an editable path, and forgets one it can no longer trust", () => {
    const etags = new Etags();
    etags.learn("GET", "/v1/orgs/settings", 200, '"3"');
    expect(etags.get("/v1/orgs/settings?fresh=1")).toBe('"3"');
    // A successful edit answers the new version.
    etags.learn("PUT", "/v1/orgs/settings", 200, '"4"');
    expect(etags.get("/v1/orgs/settings")).toBe('"4"');
    // An edit that answers none leaves the known version stale: the next edit reads again.
    etags.learn("PUT", "/v1/orgs/settings", 200, null);
    expect(etags.get("/v1/orgs/settings")).toBeUndefined();
    // A refused version is forgotten; a failure other than 412 changes nothing.
    etags.learn("GET", "/v1/orgs/settings", 200, '"5"');
    etags.learn("PUT", "/v1/orgs/settings", 500, null);
    expect(etags.get("/v1/orgs/settings")).toBe('"5"');
    etags.learn("PUT", "/v1/orgs/settings", 412, '"6"');
    expect(etags.get("/v1/orgs/settings")).toBeUndefined();
    // A read with no `ETag` is known as unversioned, and an edit keeps it so.
    etags.learn("GET", "/v1/presence/settings", 200, null);
    etags.learn("PUT", "/v1/presence/settings", 200, null);
    expect(etags.get("/v1/presence/settings")).toBeNull();
    // A deleted resource has no version, nor has one after a write under it.
    etags.learn("GET", "/v1/teams/web", 200, '"1"');
    etags.learn("DELETE", "/v1/teams/web", 200, null);
    expect(etags.get("/v1/teams/web")).toBeUndefined();
    etags.learn("GET", "/v1/chat/sessions/s1", 200, '"2"');
    etags.learn("GET", "/v1/chat/sessions/s1/messages", 200, null);
    expect(etags.get("/v1/chat/sessions/s1")).toBe('"2"');
    etags.learn("POST", "/v1/chat/sessions/s1/messages", 200, null);
    expect(etags.get("/v1/chat/sessions/s1")).toBeUndefined();
    // A path no edit needs is not kept.
    etags.learn("GET", "/v1/skills/x", 200, '"9"');
    expect(etags.get("/v1/skills/x")).toBeUndefined();
  });
});

/** A stand-in Gateway: each resource at a version, its `GET` and its edits tagged, a stale `If-Match` refused. */
function gateway(versions: Record<string, number>) {
  const seen: { method: string; path: string; ifMatch: string | null }[] = [];
  const fetch = async (url: Request | string | URL, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    const path = new URL(String(url)).pathname;
    const ifMatch = new Headers(init?.headers).get("if-match");
    seen.push({ method, path, ifMatch });
    const version = versions[path];
    const tag: Record<string, string> = version === undefined ? {} : { etag: `"${version}"` };
    // Every answer has an empty `settings`, which the notification settings' parser needs.
    if (method === "GET") return Response.json({ settings: [] }, { headers: tag });
    if (ifMatch && version !== undefined && ifMatch !== `"${version}"`) {
      return Response.json({ error: { message: "the resource changed", code: "version_mismatch" } }, { status: 412 });
    }
    if (version === undefined) return Response.json({ settings: [] });
    versions[path] = version + 1;
    return Response.json({ settings: [] }, { headers: { etag: `"${version + 1}"` } });
  };
  return { seen, fetch };
}

describe("a client with ifMatch", () => {
  it("sends the version it read, and reads first an edit it never read", async () => {
    const { seen, fetch } = gateway({ "/v1/orgs/settings": 3, "/v1/chat/sessions/s1": 7 });
    const core = createCoreClient({ baseUrl: "http://gateway.invalid", ifMatch: true, fetch });
    await core.orgSettings();
    await core.setOrgSettings({ chatRetentionDays: 30 });
    await core.setOrgSettings({ chatRetentionDays: 60 });
    await core.updateChatSession("s1", { title: "Plans" });
    expect(seen).toEqual([
      { method: "GET", path: "/v1/orgs/settings", ifMatch: null },
      { method: "PUT", path: "/v1/orgs/settings", ifMatch: '"3"' },
      // The first edit's answer is the version now.
      { method: "PUT", path: "/v1/orgs/settings", ifMatch: '"4"' },
      { method: "GET", path: "/v1/chat/sessions/s1", ifMatch: null },
      { method: "PATCH", path: "/v1/chat/sessions/s1", ifMatch: '"7"' },
    ]);
  });

  it("edits an unversioned resource without If-Match and without reading it again", async () => {
    const { seen, fetch } = gateway({});
    const core = createCoreClient({ baseUrl: "http://gateway.invalid", ifMatch: true, fetch });
    await core.setOrgSettings({ chatRetentionDays: 30 });
    await core.setOrgSettings({ chatRetentionDays: 60 });
    expect(seen.map(({ method, ifMatch }) => `${method} ${ifMatch}`)).toEqual(["GET null", "PUT null", "PUT null"]);
  });

  it("refuses an edit over a change made elsewhere, then reads again", async () => {
    const versions = { "/v1/orgs/settings": 3 };
    const { seen, fetch } = gateway(versions);
    const core = createCoreClient({ baseUrl: "http://gateway.invalid", ifMatch: true, fetch });
    await core.orgSettings();
    versions["/v1/orgs/settings"] = 4; // Another device saved the settings.
    await expect(core.setOrgSettings({ chatRetentionDays: 30 })).rejects.toMatchObject({ status: 412, code: "version_mismatch" });
    await core.setOrgSettings({ chatRetentionDays: 30 });
    expect(seen.map(({ method, ifMatch }) => `${method} ${ifMatch}`)).toEqual(["GET null", 'PUT "3"', "GET null", 'PUT "4"']);
  });

  it("reads a session again after a message bumped it", async () => {
    const { seen, fetch } = gateway({ "/v1/chat/sessions/s1": 7 });
    const core = createCoreClient({ baseUrl: "http://gateway.invalid", ifMatch: true, fetch });
    await core.chatSession("s1");
    // The stand-in answers JSON, not a stream, so the call rejects; the message was sent all the same.
    await core.streamChatMessage("s1", { content: "Hi" }, () => {}).catch(() => undefined);
    await core.updateChatSession("s1", { archived: true });
    expect(seen.map(({ method, path, ifMatch }) => `${method} ${path} ${ifMatch}`)).toEqual([
      "GET /v1/chat/sessions/s1 null",
      "POST /v1/chat/sessions/s1/messages null",
      "GET /v1/chat/sessions/s1 null",
      'PATCH /v1/chat/sessions/s1 "7"',
    ]);
  });

  it("sends the version on the artifacts client's notification settings, and learns from a refusal", async () => {
    const versions = { "/v1/notifications/settings": 2 };
    const { seen, fetch } = gateway(versions);
    const artifacts = createArtifactsClient({ baseUrl: "http://gateway.invalid", ifMatch: true, fetch });
    const change: NotificationSettingChange[] = [{ group: "usage_and_billing", channel: "email", setting: "off" }];
    await artifacts.setNotificationSettings(change);
    versions["/v1/notifications/settings"] = 9; // Another device changed a switch.
    await expect(artifacts.setNotificationSettings(change)).rejects.toMatchObject({ status: 412 });
    await artifacts.setNotificationSettings(change);
    expect(seen.map(({ method, ifMatch }) => `${method} ${ifMatch}`)).toEqual(["GET null", 'PUT "2"', 'PUT "3"', "GET null", 'PUT "9"']);
  });

  it("changes nothing for a client without it", async () => {
    const { seen, fetch } = gateway({ "/v1/orgs/settings": 3 });
    const core = createCoreClient({ baseUrl: "http://gateway.invalid", fetch });
    await core.setOrgSettings({ chatRetentionDays: 30 });
    expect(seen).toEqual([{ method: "PUT", path: "/v1/orgs/settings", ifMatch: null }]);
  });
});
