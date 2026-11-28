import type { ModuleManifest, RunSummary, RunsPage, RunsQuery } from "@nexus/kit";

// The phone's copy of Nexus UI's `panels/run-pages.ts`, without React Query.

/** Runs per page of a paged report: the Review module's default page size. */
export const RUN_PAGE_SIZE = 50;

/** What `report` asks for: one page on a paged capability (`paged_reports`), the whole list on any other. */
export const pageQuery = (manifest: Pick<ModuleManifest, "paged_reports">, after?: string): RunsQuery =>
  manifest.paged_reports ? { limit: RUN_PAGE_SIZE, ...(after ? { after } : {}) } : {};

/** The loaded pages as one list, newest first. A run on two pages is listed once, as the earlier page has it. */
export function pageRuns(pages: RunsPage[]): RunSummary[] {
  const seen = new Set<string>();
  return pages
    .flatMap((page) => page.runs)
    .filter((run) => {
      if (seen.has(run.id)) return false;
      seen.add(run.id);
      return true;
    });
}

/** The cursor of the page after the last one loaded; undefined when there is none. */
export const nextCursor = (pages: RunsPage[] | null) => pages?.at(-1)?.next_cursor ?? undefined;

/**
 * The pages after a fresh first page. Alone, it replaces the first. With more pages loaded, the old first page's runs
 * stay after the fresh ones: new runs push older runs off the first page, and the second page starts after the old
 * first page's cursor, so dropping them would leave a gap.
 */
export function withFirstPage(pages: RunsPage[] | null, first: RunsPage): RunsPage[] {
  if (!pages || pages.length < 2) return [first];
  const [old, ...rest] = pages;
  return [{ runs: pageRuns([first, old!]), next_cursor: old!.next_cursor }, ...rest];
}
