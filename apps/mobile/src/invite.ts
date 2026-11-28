/**
 * How the Referrals card invites: the message every invitation carries, and whether the card offers
 * *Text a friend*. That button opens the phone's own Messages composer with no recipient, so the person picks the
 * contacts there and Nexus never reads them. A phone that cannot text (an iPad without Messages, the iOS simulator)
 * keeps *Share link* alone.
 */
export type Invite = { message: string; text: boolean };

export function invite(link: string | undefined, canText: boolean): Invite | null {
  if (!link) return null;
  return { message: `Join me on Nexus: ${link}`, text: canText };
}
