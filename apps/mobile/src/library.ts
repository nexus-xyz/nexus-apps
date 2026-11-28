import type { PageComment, PageCreatedVia, PageFormat } from "@nexus/kit";

/**
 * Artifacts on the phone (brand v5's P20, P75): what kind each is, the library's type pills, a doc's provenance line,
 * and a page's comments as threads. No React Native here, so the repository's vitest runs it.
 */

/** A Markdown page reads as a Doc; an HTML or React page runs as a Site; a whiteboard is a Board; a shared chat is a Chat. */
export type Kind = "doc" | "sheet" | "site" | "board" | "chat";
export const kindOf = (format: PageFormat | undefined): Kind => (format === "md" ? "doc" : format === "sheet" ? "sheet" : format === "board" ? "board" : "site");
export const KIND_LABEL: Record<Kind, string> = { doc: "Doc", sheet: "Sheet", site: "Site", board: "Board", chat: "Chat" };

/** The library's pills (P20·A): All, then a pill per kind the phone has. Slides join when the Gateway has them. */
export const TYPES = [
  { type: "all", label: "All" },
  { type: "doc", label: "Docs" },
  { type: "sheet", label: "Sheets" },
  { type: "site", label: "Sites" },
  { type: "board", label: "Boards" },
] as const;
export type TypeFilter = (typeof TYPES)[number]["type"];
export const ofType = <T extends { kind: Kind }>(rows: T[], type: TypeFilter) => (type === "all" ? rows : rows.filter((r) => r.kind === type));

/** Whose artifacts the library lists, from the menu on its account line. */
export const OWNERS = [
  { owner: "mine", label: "Yours" },
  { owner: "shared_with_me", label: "Shared with me" },
  { owner: "shared_by_me", label: "Shared by me" },
] as const;
export type Owner = (typeof OWNERS)[number]["owner"];

/** How a version was written, as the Gateway records it: "written in a chat", "restored from v6". */
export function howWritten(via: PageCreatedVia | undefined, restoredFrom?: number): string | undefined {
  if (!via) return undefined;
  const how: Record<PageCreatedVia, string> = {
    chat_tool: "written in a chat",
    api: "written through the API",
    cli: "written from the CLI",
    restore: restoredFrom ? `restored from v${restoredFrom}` : "restored",
    remix: "remixed",
  };
  return how[via];
}

/**
 * A doc's line under its bar (P20 Doc): "Doc · v7 · written in a chat". The design names the agent and its model; the
 * Gateway keeps only how a version was written, so the line says that.
 */
export function provenance(kind: Kind, version: number, via: PageCreatedVia | undefined, restoredFrom?: number): string {
  return [KIND_LABEL[kind], `v${version}`, howWritten(via, restoredFrom)].filter(Boolean).join(" · ");
}

/** A site's deploy, as its list says it: "Restored from v6", with a capital, since it starts the row's second line. */
export function deployDetail(via: PageCreatedVia | undefined, restoredFrom?: number): string | undefined {
  const how = howWritten(via, restoredFrom);
  return how && how[0]!.toUpperCase() + how.slice(1);
}

/** A thread: its first comment, its replies in order, and when it was resolved. */
export type Thread = { root: PageComment; replies: PageComment[] };

/** Comments as threads, oldest reply first, the open threads before the resolved ones and each set newest first. */
export function threadsOf(comments: PageComment[]): Thread[] {
  const roots = comments.filter((c) => !c.threadId);
  const byTime = (a: PageComment, b: PageComment) => a.createdAt.localeCompare(b.createdAt);
  return roots
    .map((root) => ({ root, replies: comments.filter((c) => c.threadId === root.id).sort(byTime) }))
    .sort((a, b) => Number(!!a.root.resolvedAt) - Number(!!b.root.resolvedAt) || b.root.createdAt.localeCompare(a.root.createdAt));
}

/** How many threads are open: what the bar's comment button counts. */
export const openThreads = (comments: PageComment[]) => comments.filter((c) => !c.threadId && !c.resolvedAt).length;

/** A person's initials for their comment's avatar: "Ana Ruiz" → "AR", "ana@acme.example" → "A". */
export function initialsOf(name: string | undefined): string {
  if (!name) return "?";
  const words = name.replace(/@.*/, "").split(/[\s._-]+/).filter(Boolean);
  return (words.length > 1 ? words[0]![0]! + words[1]![0]! : (words[0]?.[0] ?? "?")).toUpperCase();
}
