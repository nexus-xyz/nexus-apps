import type { PageFrame, SiteFrameRoute } from "@nexus/kit";
import { pageNavigation } from "@/pages";

export function siteFrameRoutes(frame: PageFrame | null): SiteFrameRoute[] {
  return frame?.siteRoutes?.filter((route) => pageNavigation(frame.url, route.url) === "load") ?? [];
}

export function siteRouteForPath(routes: readonly SiteFrameRoute[], path: string | undefined): SiteFrameRoute | undefined {
  return routes.find((route) => route.path === path) ?? routes.find((route) => route.path === "/");
}

export function siteRouteForLink(frameUrl: string, target: string, routes: readonly SiteFrameRoute[]): SiteFrameRoute | undefined {
  if (pageNavigation(frameUrl, target) !== "load") return;
  try {
    const path = new URL(target).pathname;
    if (path === new URL(frameUrl).pathname) return;
    return routes.find((route) => route.path === path);
  } catch {
    return;
  }
}
