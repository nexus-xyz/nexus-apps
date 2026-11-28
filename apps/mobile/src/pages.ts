/**
 * Where a page's WebView may go. A page is untrusted HTML on its sandbox host, as in Nexus UI's frame. On
 * the phone it stays on that host: only the frame URL's own origin loads. A web link opens in the in-app browser. Any
 * other scheme (the app's own `nexus://`, `tel:`, `file:`, `data:`) is dropped, so a page cannot navigate the app.
 */
export function pageNavigation(frameUrl: string, url: string): "load" | "browser" | "drop" {
  const origin = originOf(frameUrl);
  if (origin && originOf(url) === origin) return "load";
  return /^https?:\/\//i.test(url) ? "browser" : "drop";
}

/** Scheme and authority, lowercased. The whole authority, so `https://host@elsewhere/` is not `https://host`. */
const originOf = (url: string) => /^https?:\/\/[^/?#\\]+/i.exec(url)?.[0].toLowerCase();

/** What a reader outside the organization sees instead of a shared page's author, as in Nexus UI. */
export const UNVERIFIED_LABEL = "Content is user-generated and unverified.";

/** A page's format, as Nexus UI names it: a `jsx` page is React. */
export const formatLabel = (format: string | undefined) =>
  format === "md" ? "Markdown" : format === "jsx" ? "React" : format === "sheet" ? "Sheet" : format === "board" ? "Board" : "HTML";
