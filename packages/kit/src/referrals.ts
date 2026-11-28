/**
 * The referral offer's words, one source for Nexus UI and the phone; the desktop app copies them
 * (`desktop-gpui/src/pages/referrals.rs`). The amount comes from the Gateway (`rewardUsdDecimal`), never from the
 * client. While Nexus pays no reward (`rewardsActive` is not true), the words promise no money.
 */
import type { ReferralList, ReferralRewardProgress, ReferredPerson } from "./api";
import { BILLING_TERMS } from "./billing-words";
import { budgetText, parseMicros } from "./money";

/**
 * The condition, in the same words on every surface: the Gateway rewards only a purchase of at least $10 in the referred
 * person's personal organization (`referrals::mark_paid_in`).
 */
export const REFERRAL_CONDITION = "buys at least $10 of Nexus credits in their personal account";

/** What a referral earns, in the glossary's words: never "Nexus credit". */
const REWARD_NAME = BILLING_TERMS.promotionalCredit.toLowerCase();

/** What Nexus says while it pays no reward. */
export const NO_REWARD = "Nexus does not pay referral rewards now.";

/**
 * The reward's other condition: the Gateway keys a reward on the referred person's mailbox and voids one
 * with no email (`no_mailbox`).
 */
export const REFERRAL_EMAIL_CONDITION = "That person needs a confirmed email address on their Nexus account.";

/** What the list says for a live person Nexus has no email for yet. */
export const EMAIL_PENDING = "Email pending";

/** The reward for one referral as text (`"$1"`), or null while Nexus pays none: then nothing may promise money. */
export function referralReward(list: Pick<ReferralList, "enabled" | "rewardsActive" | "rewardUsdDecimal">): string | null {
  if (list.enabled === false || list.rewardsActive !== true || !list.rewardUsdDecimal) return null;
  const micros = parseMicros(list.rewardUsdDecimal);
  return micros !== null && micros > 0 ? budgetText(micros) : null;
}

/** The offer: the hero's title and body, the third step of How it works, and the condition at the foot. */
export interface ReferralTerms {
  title: string;
  body: string;
  step: { title: string; body: string };
  condition: string;
}

export function referralTerms(reward: string | null): ReferralTerms {
  if (reward === null) {
    return {
      title: "Invite people to Nexus",
      body: `${NO_REWARD} Your link still works, and the people who join through it show here.`,
      step: { title: "No reward now", body: NO_REWARD },
      condition: `${NO_REWARD} This page shows the reward when Nexus pays one.`,
    };
  }
  return {
    title: `Invite people, earn ${REWARD_NAME}`,
    body: `Get ${reward} of ${REWARD_NAME} when a person who joined through your link ${REFERRAL_CONDITION}.`,
    step: { title: `You get ${reward}`, body: `After that person ${REFERRAL_CONDITION}.` },
    condition: `You get ${reward} of ${REWARD_NAME} when a person who joined through your link ${REFERRAL_CONDITION}. ${REFERRAL_EMAIL_CONDITION}`,
  };
}

/**
 * The offer for this person: the limit's words once they used every lifetime reward, else `referralTerms`. At the limit
 * nothing promises money, even while Nexus still pays other people.
 */
export function referralTermsFor(reward: string | null, progress: ReferralRewardProgress | undefined): ReferralTerms {
  if (!progress || progress.granted < progress.limit) return referralTerms(reward);
  const limit = progress.limit;
  return {
    title: "Lifetime reward limit reached",
    body: `You've reached the lifetime limit of ${limit} rewarded referrals. Further referrals do not earn credit. You can still invite people when invitations are available.`,
    step: { title: "No more rewards", body: `Your account earned all ${limit} rewards it can.` },
    condition: `Each account earns rewards for up to ${limit} referrals. Yours reached that limit.`,
  };
}

/**
 * Whether a referred person erased their account. The Gateway says so (`erased`); a null email alone may be
 * a live person Nexus has no email for. An older Gateway sends no flag: there a null email means erased.
 */
export function referredErased(person: Pick<ReferredPerson, "email" | "erased">): boolean {
  return person.erased ?? person.email === null;
}

/** A staff account's lifetime rewards (`rewardsGranted`), which have no limit: "1 rewarded referral". */
export function rewardsGrantedText(granted: number): string {
  return `${granted} rewarded ${granted === 1 ? "referral" : "referrals"}`;
}

/** True once the person used all of their lifetime reward slots. */
export function referralLimitReached(list: Pick<ReferralList, "rewardProgress">): boolean {
  return !!list.rewardProgress && list.rewardProgress.granted >= list.rewardProgress.limit;
}

/**
 * One referral's reward, in a word or two. "Not rewarded" is final. "Not yet rewarded" only while Nexus pays rewards,
 * so a referral never waits for a reward that no sweep grants; otherwise "No reward now". A Gateway older than
 * `reward` falls back to `rewarded`.
 */
export function referralRewardText(
  person: Pick<ReferredPerson, "reward" | "rewarded">,
  list: Pick<ReferralList, "enabled" | "rewardsActive" | "rewardUsdDecimal" | "rewardProgress">,
): string {
  const state = person.reward ?? (person.rewarded === true ? "rewarded" : person.rewarded === false ? (referralLimitReached(list) ? "not_rewarded" : "pending") : undefined);
  if (state === "rewarded") return "Rewarded";
  if (state === "not_rewarded") return "Not rewarded";
  if (state === "pending") return referralReward(list) === null ? "No reward now" : "Not yet rewarded";
  return "Reward status unavailable";
}

/**
 * What a person's link can do while sign-up is invite-only. `open`: sign-up is open, so the
 * link admits anyone. `approved`: it admits `left` more, of `allowed` in all (absent on a Gateway older than
 * `invitesAllowed`). `used_up`: it admits nobody, because the person used all `allowed` invitations. `not_approved`:
 * it admits nobody, because Nexus approved none, or the Gateway is too old to say which of the two.
 */
export type ReferralInvitations =
  | { kind: "open" }
  | { kind: "approved"; left: number; allowed?: number }
  | { kind: "used_up"; allowed: number }
  | { kind: "not_approved" };

export function referralInvitations(list: Pick<ReferralList, "invitesLeft" | "invitesAllowed">): ReferralInvitations {
  // Gateway omits `invitesLeft` while sign-up is open; a `null` means the same, never "closed".
  const left = list.invitesLeft;
  if (left == null) return { kind: "open" };
  const allowed = typeof list.invitesAllowed === "number" && list.invitesAllowed > 0 ? list.invitesAllowed : undefined;
  if (left > 0) return allowed === undefined ? { kind: "approved", left } : { kind: "approved", left, allowed: Math.max(allowed, left) };
  return allowed === undefined ? { kind: "not_approved" } : { kind: "used_up", allowed };
}

/** True when the link admits nobody: the page then shows no link and no share buttons. */
export function invitationsClosed(invites: ReferralInvitations): boolean {
  return invites.kind === "used_up" || invites.kind === "not_approved";
}

const invitationWord = (n: number) => (n === 1 ? "invitation" : "invitations");

/** The notice that replaces the link while it admits nobody, or null while it admits someone. */
export function invitationsNotice(invites: ReferralInvitations): { title: string; body: string } | null {
  if (invites.kind === "used_up") {
    return {
      title: "You used all your invitations",
      body: `All ${invites.allowed} ${invitationWord(invites.allowed)} Nexus approved for you are used. Your link admits nobody new until Nexus approves more.`,
    };
  }
  if (invites.kind === "not_approved") {
    return {
      title: "Invitations come by approval",
      body: "Nexus is invite-only for now. Your link admits nobody until Nexus approves invitations for you.",
    };
  }
  return null;
}

/**
 * The count beside the meter, which fills with the invitations used and is full once they run out: "7 of 10
 * invitations used" and "3 left", or "6 of 6 invitations used" and "None left". A Gateway that sends no total gives
 * "3 invitations left" and no meter (`share` null). Null while sign-up is open or the person is not approved.
 */
export function invitationsCount(invites: ReferralInvitations): { text: string; left: string | null; share: number | null; usedUp: boolean } | null {
  if (invites.kind === "used_up") return { text: `${invites.allowed} of ${invites.allowed} ${invitationWord(invites.allowed)} used`, left: "None left", share: 1, usedUp: true };
  if (invites.kind !== "approved") return null;
  if (invites.allowed === undefined) return { text: `${invites.left} ${invitationWord(invites.left)} left`, left: null, share: null, usedUp: false };
  const used = invites.allowed - invites.left;
  return { text: `${used} of ${invites.allowed} ${invitationWord(invites.allowed)} used`, left: `${invites.left} left`, share: used / invites.allowed, usedUp: false };
}
