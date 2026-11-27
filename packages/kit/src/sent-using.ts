/**
 * "Sent using Nexus": the line that ends a message Nexus sends as the person, by messaging connector. The
 * agents core adds it (`tiers.ts`), and the person turns it off in Assistant › Approvals. Each platform writes the
 * link its own way. A plain-text body carries the address, and on X the line goes only when the post still fits.
 */

/** Where "Sent using Nexus" links. */
export const NEXUS_URL = "https://nexus.inc";

/** How a platform ends a message sent as the person: the line, what joins it to the text, and the platform's limit. */
export interface MessageFooter {
  line: string;
  join: string;
  /** The most the whole text may weigh (`postWeight`). Over it, the message goes without the footer. */
  limit?: number;
}

/** The footer of a message sent as the person, by connector id. */
export const MESSAGE_FOOTERS: Readonly<Record<string, MessageFooter>> = {
  slack: { line: `_Sent using <${NEXUS_URL}|Nexus>_`, join: "\n\n" },
  teams: { line: `Sent using Nexus (${NEXUS_URL})`, join: "\n\n" },
  gmail: { line: `Sent using Nexus (${NEXUS_URL})`, join: "\n\n" },
  outlook: { line: `Sent using Nexus (${NEXUS_URL})`, join: "\n\n" },
  x: { line: "— Sent using Nexus", join: " ", limit: 280 },
};

/**
 * A post's weight as X counts it: most characters count 2 and Latin ones 1, against a limit of 280. A link counts 23,
 * or its own length when that is more, so the count never runs short.
 */
export function postWeight(text: string): number {
  let weight = 0;
  for (const part of text.split(/(https?:\/\/\S+)/)) {
    if (/^https?:\/\//.test(part)) {
      weight += Math.max(23, [...part].length);
      continue;
    }
    for (const char of part) {
      const code = char.codePointAt(0) ?? 0;
      weight += code <= 4351 || (code >= 8192 && code <= 8205) || (code >= 8208 && code <= 8223) || (code >= 8242 && code <= 8247) ? 1 : 2;
    }
  }
  return weight;
}
