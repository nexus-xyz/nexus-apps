import { VAULT_MANAGER_NAMES, type AgentVaultChanged, type AgentVaultRead, type AgentVaultView, type AgentVaultWrite, type VaultAsk, type VaultLogin, type VaultManager } from "@nexus/kit";

/**
 * Passwords and payments on the phone (A8), without React Native: what the page shows for each read of the
 * agent API, and what it says after each write. The wire types, the strict readers and the row words are the kit's
 * (`agent-vault.ts`). Each reader there refuses a body shaped like a secret, so the phone never holds or draws one.
 * The words are Nexus UI's (`agent-vault.tsx` there).
 */

/** What Nexus shows with no vault at all: no password manager, no card. An agent API with the vault off reads as this. */
export const UNLINKED: AgentVaultView = { manager: null, logins: [], card: null, merchants: [], applePay: false, asks: [] };

/** The page for a read: the vault, the unlinked state (with `unlinked` set), or a failure. */
export function pageOf(read: AgentVaultRead): { view: AgentVaultView; unlinked: boolean } | "failed" {
  if (read.kind === "off") return { view: UNLINKED, unlinked: true };
  return read.kind === "view" ? { view: read.view, unlinked: false } : "failed";
}

/** Where the logins live, as the page says it. */
export const LIVES_ON_DESKTOP = "Your logins live in the Nexus desktop app, on your Mac. The phone never holds a password.";
/** Where a grant changes: the desktop owns grants, and each index it sends replaces them. */
export const CHANGE_ON_DESKTOP = "Change grants in the Nexus desktop app.";
/** What a waiting grant says about approving it: Touch ID is on the Mac. */
export const APPROVE_ON_DESKTOP = "Approve it in the Nexus desktop app, with Touch ID.";

/** A source as its line says it: "1Password · Personal · 12 logins". */
export function sourceLine(source: VaultManager): string {
  const name = VAULT_MANAGER_NAMES[source.kind];
  const vault = source.vault === name ? "" : ` · ${source.vault}`;
  const total = source.total === undefined ? "" : ` · ${source.total === 1 ? "1 login" : `${source.total} logins`}`;
  return `${name}${vault}${total}`;
}

/** The sources a view names: its `sources`, else its `manager` alone (an older agent API), else none. */
export const sourcesOf = (view: AgentVaultView): VaultManager[] => view.sources ?? (view.manager ? [view.manager] : []);

/** Where the logins are, in one line: "Linked on Maya’s MacBook Pro." or "Linked on your desktop." */
export const deviceLine = (view: AgentVaultView): string => (view.device ? `Linked on ${view.device}.` : "Linked on your desktop.");

/** A login's site, with its masked account and its source when the index names them: "united.com · m•••@gmail.com · 1Password". */
export const loginSite = (login: VaultLogin): string => [login.site, login.account, login.source ? VAULT_MANAGER_NAMES[login.source] : undefined].filter(Boolean).join(" · ");

/** The sources' title and the line under it, or what the page says when none is linked. */
export function managerLine(view: AgentVaultView, unlinked: boolean): { title: string; line: string } {
  const sources = sourcesOf(view);
  if (!sources.length) {
    const why = unlinked ? "Nexus can’t reach your desktop’s vault here yet, so it can’t use any login." : "Nexus can’t use a login until you link one.";
    return { title: "No password manager is linked", line: `${why} Link 1Password or turn on the Nexus vault in the Nexus desktop app. It never reads, stores or repeats a password.` };
  }
  const usable = view.logins.filter((login) => login.grant).length;
  return {
    title: sources.map(sourceLine).join(" and "),
    line: `${deviceLine(view)} Nexus can ask to use ${usable === 1 ? "1 login" : `${usable} logins`}. It never reads, stores or repeats them.`,
  };
}

/** What the card's Freeze or Unfreeze says. */
export const cardText = (changed: AgentVaultChanged): string =>
  changed.vault.card?.state === "frozen" ? "The card is frozen. Nothing can be paid with it." : "The card works again.";

/** Who fills a login in: the Nexus vault, or the linked password manager by name. */
export const fillerOf = (ask: Pick<VaultAsk, "manager">): string => (ask.manager === "nexus" ? "The Nexus vault" : VAULT_MANAGER_NAMES[ask.manager]);

/** What the grant sheet says once decided. */
export function decidedText(ask: Pick<VaultAsk, "manager" | "site">, changed: AgentVaultChanged): string {
  if (changed.decided === "confirm_on_device") return "Confirm with Face ID on your phone to finish.";
  if (changed.decided === "declined") return "Declined. Nexus won’t use it.";
  return `Approved. ${fillerOf(ask)} fills it in on ${ask.site}.`;
}

/** What a failed write says. */
export function failedText(write: Extract<AgentVaultWrite, { kind: "failed" }>): string {
  if (write.reason === "off") return "No password manager or card is linked to Nexus here yet.";
  if (write.reason === "gone") return "That request is no longer waiting. Ask Nexus again.";
  return "That did not save. Try again in a moment.";
}
