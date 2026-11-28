/**
 * The Assistant's menu, without React Native: the one control at the right of its bar (the Assistant canvas's phone
 * header). It replaces the contact card. The thread's own actions come first: History when the agent API keeps shared
 * conversations, and New conversation once the thread has messages. Then the Assistant's pages in a build with errands
 * (Errands, Projects). Assistant settings, the Assistant group of Settings › Capabilities, is always last. Memory,
 * Discover and Privacy live there, as the canvas puts them.
 *
 * It opens below the ⋯ as the app's own menu (`dropdown.tsx`, `assistant-menu-view.tsx`): Liquid Glass on iOS 26,
 * Material 3's menu on Android, each group a section with a divider.
 */

export type MenuAction = "history" | "new";
export type MenuIcon = "history" | "compose" | "pulse" | "repositories" | "settings";

export type MenuRow = { label: string; icon: MenuIcon } & ({ action: MenuAction; href?: never } | { href: string; action?: never });

/** The menu's groups of rows; an empty group is left out. */
export function menuRows({ history, conversation, errands }: { history: boolean; conversation: boolean; errands: boolean }): MenuRow[][] {
  const thread: MenuRow[] = [
    ...(history ? [{ label: "History", icon: "history", action: "history" } as const] : []),
    ...(conversation ? [{ label: "New conversation", icon: "compose", action: "new" } as const] : []),
  ];
  const pages: MenuRow[] = errands
    ? [
        { label: "Errands", icon: "pulse", href: "/errands" },
        { label: "Projects", icon: "repositories", href: "/assistant-projects" },
      ]
    : [];
  const settings: MenuRow[] = [{ label: "Assistant settings", icon: "settings", href: "/assistant-settings" }];
  return [thread, pages, settings].filter((group) => group.length > 0);
}

/** What picking a row does: the thread's action, or the page it opens. */
export function pickRow(row: MenuRow, onAction: (action: MenuAction) => void, open: (href: string) => void): void {
  if (row.action) onAction(row.action);
  else open(row.href as string);
}

/** History and New conversation wait while an answer runs; the pages never do. */
export const rowDisabled = (row: MenuRow, busy: boolean): boolean => busy && row.action !== undefined;
