import { describe, expect, it } from "vitest";
import { isSiteFormat, nextSiteVersion } from "./site-version";

const id = "pag_site";
const event = (type: string, version: number, pageId = id) => ({ type, data: { pageId, version } });

describe("a read-only Site's saved version and draft", () => {
  it("uses separate state for the saved version and a reported draft on top", () => {
    const saved = nextSiteVersion(undefined, id, event("version", 7));
    expect(saved).toEqual({ pageId: id, version: 7, draft: false });
    const preview = nextSiteVersion(saved, id, event("preview", 7));
    expect(preview).toEqual({ pageId: id, version: 7, draft: true });

    const published = nextSiteVersion(preview, id, event("version", 8));
    expect(published).toEqual({ pageId: id, version: 8, draft: false });
    expect(nextSiteVersion(published, id, event("preview", 7))).toBe(published);
  });

  it("ignores other pages and presence, and clears a stale draft on the next Gateway snapshot", () => {
    const prior = nextSiteVersion(undefined, id, event("preview", 7));
    expect(nextSiteVersion(prior, id, event("preview", 10, "pag_other"))).toBe(prior);
    expect(nextSiteVersion(prior, id, event("presence", 7))).toBe(prior);
    expect(nextSiteVersion(prior, id, event("version", -1))).toBe(prior);
    expect(nextSiteVersion(prior, id, event("version", 7))).toEqual({ pageId: id, version: 7, draft: false });
  });

  it("limits saved-only reading to HTML and JSX Sites", () => {
    expect(isSiteFormat("html")).toBe(true);
    expect(isSiteFormat("jsx")).toBe(true);
    expect(isSiteFormat("board")).toBe(false);
    expect(isSiteFormat("md")).toBe(false);
  });
});
