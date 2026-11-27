import { expect, it } from "vitest";
import { createCoreClient } from "./core-client";

// An organization's new logo goes to Gateway as the image's bytes, typed by the caller, never as JSON.
it("uploads a logo as its bytes with the caller's type", async () => {
  const seen: { method?: string; url: string; type: string | null; body: unknown }[] = [];
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", fetch: async (url, init) => {
    seen.push({ method: init?.method, url: String(url), type: new Headers(init?.headers).get("content-type"), body: init?.body });
    return new Response(JSON.stringify({ id: "ten_1", name: "Acme", imageUrl: "https://storage.googleapis.com/b/org-logos/ten_1/abc" }), { status: 200 });
  } });
  const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47]);
  await expect(core.setOrgLogo(png, "image/png")).resolves.toMatchObject({ imageUrl: "https://storage.googleapis.com/b/org-logos/ten_1/abc" });
  expect(seen).toEqual([{ method: "POST", url: "http://gateway.invalid/v1/orgs/logo", type: "image/png", body: png }]);
});

it("still sends any other body as JSON", async () => {
  const seen: { type: string | null; body: unknown }[] = [];
  const core = createCoreClient({ baseUrl: "http://gateway.invalid", fetch: async (_url, init) => {
    seen.push({ type: new Headers(init?.headers).get("content-type"), body: init?.body });
    return new Response(JSON.stringify({ id: "ten_1", name: "Acme" }), { status: 200 });
  } });
  await core.setOrgProfile({ imageUrl: null });
  expect(seen).toEqual([{ type: "application/json", body: JSON.stringify({ imageUrl: null }) }]);
});
