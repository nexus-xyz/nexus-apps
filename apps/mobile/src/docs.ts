import { absoluteLink } from "./share";

/** Where a docs page's Markdown is: `/docs/<path>.md`, `/docs.md` for the home (Nexus UI's `docs-md` route). */
export const docsSource = (ui: string, path?: string) =>
  path ? `${ui.replace(/\/$/, "")}/docs/${path.split("/").map(encodeURIComponent).join("/")}.md` : `${ui.replace(/\/$/, "")}/docs.md`;

/**
 * Where a link on a docs page goes: another docs page opens in the reader (`/docs?path=…`); any other page of the site
 * (`/app`, `/llms.txt`) on the site, in the in-app browser. Undefined for the rest (a full URL, `#anchor`), which the
 * Markdown renderer handles as in an answer.
 */
export function docsLink(href: string, ui: string): string | undefined {
  const page = /^\/docs(?:\/([^#?]*?))?(?:\.md)?\/?(?:[#?].*)?$/.exec(href);
  if (page) return page[1] ? `/docs?path=${encodeURIComponent(page[1])}` : "/docs";
  return /^\/(?!\/)/.test(href) ? absoluteLink(href, ui) : undefined;
}

/** A page's first `# Title`, for the screen's title, and the rest as its body. */
export function splitTitle(markdown: string): { title?: string; body: string } {
  const match = /^\s*#\s+(.+)\n+/.exec(markdown);
  return match ? { title: match[1]!.trim(), body: markdown.slice(match[0].length) } : { body: markdown };
}
