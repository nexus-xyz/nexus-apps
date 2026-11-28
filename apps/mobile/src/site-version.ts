import type { PageEvent, PageFormat } from "@nexus/kit";

export const isSiteFormat = (format: PageFormat | undefined) => format === "html" || format === "jsx";

export type SiteVersion = { pageId: string; version: number; draft: boolean };

/** Gateway opens with the saved version, then reports a preview if a draft exists for that version. */
export function nextSiteVersion(current: SiteVersion | undefined, pageId: string, event: PageEvent): SiteVersion | undefined {
  if (event.type !== "version" && event.type !== "preview") return current;
  const data = event.data;
  if (!data || typeof data !== "object" || !("pageId" in data) || data.pageId !== pageId || !("version" in data)
    || typeof data.version !== "number" || !Number.isSafeInteger(data.version) || data.version < 1) return current;
  if (current?.pageId === pageId && data.version < current.version) return current;
  const draft = event.type === "preview";
  if (current?.pageId === pageId && data.version === current.version && draft === current.draft) return current;
  return { pageId, version: data.version, draft };
}
