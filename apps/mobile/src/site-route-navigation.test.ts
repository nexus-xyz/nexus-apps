import type { PageFrame } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { siteFrameRoutes, siteRouteForLink, siteRouteForPath } from "./site-route-navigation";

const origin = "https://site-1.pages.example";
const root = { path: "/", title: "Overview", url: origin + "/v/pag_1/2?sig=owned" };
const details = { path: "/details", title: "Details", url: root.url + "&sitePath=%2Fdetails" };
const frame: PageFrame = {
  url: root.url, pageId: "pag_1", version: 2, format: "html", title: "Example",
  expiresAt: "2026-11-24T20:00:00Z", siteRoutes: [root, details],
};

describe("signed Site route navigation", () => {
  it("retains the exact signed URL and falls back to the version's root", () => {
    const routes = siteFrameRoutes(frame);
    expect(siteRouteForPath(routes, "/details")).toEqual(details);
    expect(siteRouteForPath(routes, "/removed-in-this-version")).toEqual(root);
    expect(siteFrameRoutes({ ...frame, siteRoutes: undefined })).toEqual([]);
  });

  it("matches only listed same-origin links and never adopts untrusted URLs", () => {
    const routes = siteFrameRoutes({ ...frame, siteRoutes: [
      root,
      details,
      { ...details, path: "/spurious", url: "https://other.pages.example/spurious" },
    ] });
    expect(routes).toEqual([root, details]);
    expect(siteRouteForLink(root.url, origin + "/details?overrides=ignored", routes)).toEqual(details);
    expect(siteRouteForLink(root.url, origin + "/missing", routes)).toBeUndefined();
    expect(siteRouteForLink(root.url, details.url, routes)).toBeUndefined();
    expect(siteRouteForLink(root.url, "https://elsewhere.example/details", routes)).toBeUndefined();
  });
});
