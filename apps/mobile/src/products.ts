import type { ModuleManifest } from "@nexus/kit";

// Copies of Nexus UI's panel helpers (`panels/runs.tsx`, `panels/pages-panel.tsx`, beside Next-only
// imports), as spend.ts copies `spendSource`; move them into @nexus/kit if a third client needs them.

/** "Nexus Intelligence" → "Intelligence", and `core` is the Gateway, as desktop's sidebar names them (catalog.ts). */
export const productName = (product: ModuleManifest["product"]) => (product.slug === "core" ? "Gateway" : product.name.replace(/^Nexus\s+/, ""));

/** A repository URL as `owner/name`, or the input as typed when it is something else. */
export const repoName = (value: string) => /^https:\/\/github\.com\/([^/\s]+\/[^/\s#?]+?)(?:\.git)?\/?$/.exec(value.trim())?.[1] ?? value;

/** A GitHub repository's URL, without `.git` or a trailing slash; undefined for any other input. */
export function githubRepo(repo: string): string | undefined {
  const match = /^https:\/\/github\.com\/[^/\s]+\/[^/\s#?]+?(?:\.git)?\/?$/.exec(repo.trim());
  return match ? repo.trim().replace(/\.git$|\/$/g, "") : undefined;
}

/** A share from 0 to 1 as a percentage: one decimal under 10%, so a large repository's small share is not "0%". */
export function percent(share: number): string {
  const value = share * 100;
  return `${value < 10 ? value.toFixed(1) : Math.round(value)}%`;
}

/** How long a run took: `42s`, `4m 12s`, `1h 3m`. */
export function duration(startedAt: string, finishedAt?: string): string {
  const seconds = Math.max(0, Math.round(((finishedAt ? Date.parse(finishedAt) : Date.now()) - Date.parse(startedAt)) / 1000));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
}
