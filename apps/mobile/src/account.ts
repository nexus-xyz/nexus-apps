import { EMAIL_PENDING, invitationsClosed, invitationsCount, invitationsNotice, type ReferralList, type ReferredPerson, referralInvitations, referralReward, referralTermsFor, referredErased, rewardsGrantedText } from "@nexus/kit";

/**
 * Global and Account settings on the phone (brand v5's P96, P68): the words a key's row, the referral page and the
 * accounts list say. No React Native here, so the repository's vitest (`vitest.config.ts` at the root) runs it.
 */

const DAY = 86_400_000;
const date = (at: number, year: boolean) => new Date(at).toLocaleDateString("en-GB", { day: "numeric", month: "short", ...(year ? { year: "numeric" } : {}) });

/**
 * When a key stops working, as P68·K says it: "Never", "in 6 days" inside a month, the date past that ("1 Feb 2027"),
 * and "Expired 1 Oct" once it has.
 */
export function keyExpiry(expiresAt: string | undefined, now = Date.now()): string {
  if (!expiresAt) return "Never";
  const at = new Date(expiresAt).getTime();
  if (!Number.isFinite(at)) return "Never";
  const sameYear = new Date(at).getFullYear() === new Date(now).getFullYear();
  if (at <= now) return `Expired ${date(at, !sameYear)}`;
  const days = Math.ceil((at - now) / DAY);
  if (days <= 1) return "tomorrow";
  if (days <= 30) return `in ${days} days`;
  return date(at, true);
}

/** A key's second line: its start, so you know it in a list, then when it stops working: "nx_8f2k… · in 6 days". */
export const keyLine = (prefix: string, expiresAt: string | undefined, now = Date.now()) => `${prefix}… · ${keyExpiry(expiresAt, now)}`;

/** The link to share: `/r/<code>` on Nexus UI, as Nexus UI builds it (`referrals.ts`). */
export const referralLink = (ui: string, code: string) => `${ui.replace(/\/+$/, "")}/r/${encodeURIComponent(code)}`;

/**
 * A referred person as the list shows them: the Gateway's masked email with the board's dots, Erased once they erased
 * their account, or Email pending for a live person Nexus has no email for yet.
 */
export const referredEmail = (person: Pick<ReferredPerson, "email" | "erased">) =>
  person.email ? person.email.replace(/\*+/g, "•••") : referredErased(person) ? "Erased" : EMAIL_PENDING;

/** "4 joined", or "50+ joined" while more pages are left: the Gateway gives no total. */
export const joinedLine = (loaded: number, more: boolean) => `${more ? `${loaded}+` : loaded} joined`;


/**
 * The offer and sharing follow program state, independently of invitation allowance. The reward's words and amount
 * come from the kit (`referralTerms`): no amount while Nexus pays none. While sign-up is invite-only, a link
 * that admits nobody is hidden and the kit's notice takes the title: `usedUp` when the person used every
 * invitation, which the screen draws in the danger tone. `invites` is the count and the meter's share, as Nexus UI's.
 */
export function referralOffer(
  list: Pick<ReferralList, "enabled" | "code" | "rewardProgress" | "rewardsGranted" | "invitesLeft" | "invitesAllowed" | "rewardsActive" | "rewardUsdDecimal">,
  ui: string | null | undefined,
): { title: string; body: string; progress?: string; link?: string; invites?: { text: string; left: string | null; share: number | null }; usedUp: boolean } {
  if (list.enabled === false) return { title: "Referrals are paused", body: "Your referral history is still available. Sharing links and new rewards are paused.", usedUp: false };
  const invites = referralInvitations(list);
  const notice = invitationsNotice(invites);
  const count = invitationsCount(invites);
  const progress = list.rewardProgress;
  // The kit's words, the limit's at the limit, as Nexus UI and the desktop app say them.
  const terms = referralTermsFor(referralReward(list), progress);
  const offer = notice ?? { title: terms.title, body: terms.body };
  return {
    ...offer,
    // A staff account has no limit: only its count.
    progress: progress ? `${progress.granted} of ${progress.limit} lifetime rewards` : list.rewardsGranted !== undefined ? rewardsGrantedText(list.rewardsGranted) : "Reward progress unavailable on this Gateway",
    link: ui && !invitationsClosed(invites) ? referralLink(ui, list.code) : undefined,
    invites: count ? { text: count.text, left: count.left, share: count.share } : undefined,
    usedUp: invites.kind === "used_up",
  };
}
