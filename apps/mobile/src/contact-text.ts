import type { AgentText } from "@nexus/kit";

/**
 * The text card's logic, with no native import so vitest runs it: which of the device's contacts
 * match the name Nexus was given, the phone numbers to choose from, and the text the person sends. Contacts are read on
 * the phone and never leave it.
 */

/** A device contact as `expo-contacts` gives it: the name fields and the phones it holds. */
export type DeviceContact = {
  id: string;
  fullName?: string | null;
  givenName?: string | null;
  familyName?: string | null;
  phones?: readonly { label?: string | null; number?: string | null }[] | null;
};

/** One phone number to text, with its label as people read it ("mobile", "work"). */
export type PhoneChoice = { label: string; number: string };

/** A contact that matches, with the numbers it can be texted on. */
export type ContactMatch = { id: string; name: string; phones: PhoneChoice[] };

/** The most contacts a card lists to choose from. */
export const MAX_MATCHES = 5;

/** A name folded for comparing: lower case, no accents, single spaces. */
export function folded(name: string): string {
  return name
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/\s+/g, " ")
    .trim();
}

/** iOS keeps its built-in labels as `_$!<Mobile>!$_`; people read `mobile`. */
export function phoneLabel(label: string | null | undefined): string {
  const bare = (label ?? "").replace(/^_\$!</, "").replace(/>!\$_$/, "").trim();
  return bare ? bare.toLowerCase() : "phone";
}

/** A contact's name as people read it: the full name, else the given and family names. */
export function contactName(contact: DeviceContact): string {
  const full = contact.fullName?.trim();
  if (full) return full;
  return [contact.givenName, contact.familyName].map((part) => part?.trim()).filter(Boolean).join(" ");
}

/** A contact's numbers, each once (by its digits), in the order the device keeps them. */
export function phoneChoices(contact: DeviceContact): PhoneChoice[] {
  const choices: PhoneChoice[] = [];
  for (const phone of contact.phones ?? []) {
    const number = phone.number?.trim();
    const digits = number?.replace(/\D/g, "");
    if (!number || !digits || choices.some((seen) => seen.number.replace(/\D/g, "") === digits)) continue;
    choices.push({ label: phoneLabel(phone.label), number });
  }
  return choices;
}

/**
 * How well a contact's name fits `to`: 3 for the whole name, 2 for the first name, 1 when every word of `to` starts a
 * word of the name, 0 for no match.
 */
export function nameScore(name: string, to: string): number {
  const have = folded(name);
  const want = folded(to);
  if (!have || !want) return 0;
  if (have === want) return 3;
  const words = have.split(" ");
  if (words[0] === want) return 2;
  return want.split(" ").every((part) => words.some((word) => word.startsWith(part))) ? 1 : 0;
}

/**
 * The contacts that match `to`, best first: each with a name and at least one number, at most `MAX_MATCHES`. A tie
 * keeps the device's order.
 */
export function contactMatches(contacts: readonly DeviceContact[], to: string): ContactMatch[] {
  return contacts
    .map((contact, at) => ({ contact, at, name: contactName(contact), phones: phoneChoices(contact) }))
    .map((row) => ({ ...row, score: nameScore(row.name, to) }))
    .filter((row) => row.score > 0 && row.phones.length > 0)
    .sort((a, b) => b.score - a.score || a.at - b.at)
    .slice(0, MAX_MATCHES)
    .map(({ contact, name, phones }) => ({ id: contact.id, name, phones }));
}

/**
 * The text the person sends: the card's own, and for an invitation their referral link after it, once. `linkMissing`:
 * an invitation with no link to add (the program is paused, or every invitation is used), so the card says so.
 */
export function textToSend(card: AgentText, link: string | undefined): { text: string; linkMissing: boolean } {
  if (!card.invite) return { text: card.text, linkMissing: false };
  if (!link) return { text: card.text, linkMissing: true };
  if (card.text.includes(link)) return { text: card.text, linkMissing: false };
  return { text: `${card.text}\n\n${link}`, linkMissing: false };
}

/** The card's first line: what it prepares, with the verb first. */
export function textCardTitle(card: AgentText): string {
  return card.invite ? `Invite ${card.to} to Nexus` : `Text ${card.to}`;
}

/** What the card says once Messages closes. iOS reports sent or cancelled; Android never reports, so it says opened. */
export function afterMessages(result: "sent" | "cancelled" | "unknown"): string | undefined {
  if (result === "sent") return "Sent from Messages.";
  if (result === "unknown") return "Opened in Messages.";
  return undefined;
}
