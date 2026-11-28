/**
 * Account › Assistant on the phone (A4·B, AIPDAsstSettings): one row per section, in the desktop's order, with no
 * icons. A section with a page on the phone opens it, such as Integrations, Passwords and payments (A8) and Memory
 * (A16). Approvals (A9·g, h) opens Nexus UI's Assistant page (`/app/<org>/agent?view=…`) in the in-app browser. A
 * section with no page yet keeps the board's place, dimmed, and says so. No React Native, so vitest runs it.
 */

export type AssistantSection = {
  label: string;
  /** One short line on what it holds, as the board writes it. */
  detail: string;
} & ({ href: string; url?: never; soon?: never } | { url: string; href?: never; soon?: never } | { soon: true; href?: never; url?: never });

/** What a section with no page says in its line. */
export const NOT_YET = "Not available yet";

/** The sections as this sign-in shows them. `uiLink`: a path of Nexus UI for this organization; without it, its web sections wait too. */
export function assistantSections(uiLink?: (path: string) => string): AssistantSection[] {
  const soon = (label: string): AssistantSection => ({ label, detail: NOT_YET, soon: true });
  const web = (label: string, detail: string, view: string): AssistantSection => (uiLink ? { label, detail, url: uiLink(`/agent?view=${view}`) } : soon(label));
  return [
    soon("Personality"),
    { label: "Integrations", detail: "Connections · Tools", href: "/integrations" },
    soon("Channels"),
    { label: "Proactivity", detail: "Dial · quiet hours", href: "/proactivity" },
    web("Approvals", "Three tiers", "approvals"),
    { label: "Passwords and payments", detail: "Logins · card", href: "/vault" },
    { label: "Memory", detail: "Things it knows · learning", href: "/assistant-memory" },
    { label: "Privacy", detail: "What it can see, and everything it did", href: "/assistant-privacy" },
  ];
}

/** The page's footer: where the web sections open. */
export function assistantFooter(sections: AssistantSection[]): string | undefined {
  return sections.some((section) => section.url) ? "Approvals opens in Nexus on the web." : undefined;
}
