import { describe, expect, it } from "vitest";
import { invitationsClosed, invitationsCount, invitationsNotice, REFERRAL_CONDITION, REFERRAL_EMAIL_CONDITION, referralInvitations, referralReward, referralRewardText, referralTerms, referralTermsFor, referredErased, rewardsGrantedText } from "./referrals";

const active = { enabled: true, rewardsActive: true, rewardUsdDecimal: "1.000000" };

describe("referralReward", () => {
  it("is the Gateway's amount while Nexus pays rewards", () => {
    expect(referralReward(active)).toBe("$1");
    expect(referralReward({ ...active, rewardUsdDecimal: "2.500000" })).toBe("$2.50");
  });

  it("is null whenever a reward is not certain, so nothing promises money", () => {
    expect(referralReward({ ...active, rewardsActive: false })).toBeNull();
    expect(referralReward({ ...active, rewardUsdDecimal: "0.000000" })).toBeNull();
    expect(referralReward({ ...active, enabled: false })).toBeNull();
    expect(referralReward({ enabled: true }), "an older Gateway sends neither field").toBeNull();
  });
});

describe("referralTerms", () => {
  it("states the amount and the personal-account condition in every place", () => {
    const terms = referralTerms("$1");
    for (const text of [terms.body, terms.step.body, terms.condition]) expect(text).toContain(REFERRAL_CONDITION);
    expect(terms.body).toContain("$1");
    expect(terms.step.title).toBe("You get $1");
  });

  it("names no amount and no credit to earn while Nexus pays none", () => {
    const terms = referralTerms(null);
    const all = [terms.title, terms.body, terms.step.title, terms.step.body, terms.condition].join(" ");
    expect(all).not.toMatch(/\$/);
    expect(all).not.toMatch(/earn/i);
    expect(terms.body).toContain("does not pay referral rewards now");
  });
});

describe("rewardsGrantedText", () => {
  it("counts a staff account's rewards with no limit", () => {
    expect(rewardsGrantedText(1)).toBe("1 rewarded referral");
    expect(rewardsGrantedText(12)).toBe("12 rewarded referrals");
  });

  it("states the $10 purchase that earns the reward", () => {
    expect(referralTerms("$1").body).toContain("buys at least $10 of Nexus credits");
  });
});

describe("referralTermsFor", () => {
  it("is the reward's terms below the lifetime limit", () => {
    expect(referralTermsFor("$1", { granted: 9, limit: 10 })).toEqual(referralTerms("$1"));
    expect(referralTermsFor(null, undefined)).toEqual(referralTerms(null));
  });

  it("promises no money at the limit, even while Nexus pays others", () => {
    for (const granted of [10, 11]) {
      const terms = referralTermsFor("$1", { granted, limit: 10 });
      const all = [terms.title, terms.body, terms.step.title, terms.step.body, terms.condition].join(" ");
      expect(terms.title).toBe("Lifetime reward limit reached");
      expect(terms.body).toContain("Further referrals do not earn credit");
      expect(all).not.toMatch(/\$/);
      expect(all).toContain("10");
    }
  });

  it("says a reward needs the person's confirmed email while Nexus pays one", () => {
    expect(referralTerms("$1").condition).toContain(REFERRAL_EMAIL_CONDITION);
    expect(referralTerms(null).condition).not.toContain(REFERRAL_EMAIL_CONDITION);
  });
});

describe("referredErased", () => {
  it("reads the Gateway's flag, so a live person with no email is not erased", () => {
    expect(referredErased({ email: null, erased: true })).toBe(true);
    expect(referredErased({ email: null, erased: false })).toBe(false);
    expect(referredErased({ email: "b***@example.com", erased: false })).toBe(false);
  });

  it("falls back to a null email on an older Gateway", () => {
    expect(referredErased({ email: null })).toBe(true);
    expect(referredErased({ email: "b***@example.com" })).toBe(false);
  });
});

describe("referralRewardText", () => {
  const list = { ...active, rewardProgress: { granted: 0, limit: 10 } };

  it("reads the Gateway's state, with a final Not rewarded", () => {
    expect(referralRewardText({ reward: "rewarded" }, list)).toBe("Rewarded");
    expect(referralRewardText({ reward: "not_rewarded", rewarded: false }, list)).toBe("Not rewarded");
    expect(referralRewardText({ reward: "pending", rewarded: false }, list)).toBe("Not yet rewarded");
  });

  it("never says a reward is coming while Nexus pays none", () => {
    expect(referralRewardText({ reward: "pending" }, { ...list, rewardsActive: false })).toBe("No reward now");
    expect(referralRewardText({ reward: "pending" }, { ...list, enabled: false })).toBe("No reward now");
  });

  it("falls back to rewarded on an older Gateway", () => {
    const old = { enabled: true, rewardProgress: { granted: 10, limit: 10 } };
    expect(referralRewardText({ rewarded: true }, old)).toBe("Rewarded");
    expect(referralRewardText({ rewarded: false }, old)).toBe("Not rewarded");
    expect(referralRewardText({ rewarded: false }, { enabled: true })).toBe("No reward now");
    expect(referralRewardText({ rewarded: null }, old)).toBe("Reward status unavailable");
  });
});

describe("referralInvitations", () => {
  it("is open while the Gateway sends no invitesLeft, or null", () => {
    expect(referralInvitations({})).toEqual({ kind: "open" });
    expect(referralInvitations({ invitesLeft: null as unknown as number, invitesAllowed: 5 })).toEqual({ kind: "open" });
  });

  it("tells a person who used every invitation from one Nexus never approved", () => {
    expect(referralInvitations({ invitesLeft: 0, invitesAllowed: 6 })).toEqual({ kind: "used_up", allowed: 6 });
    expect(referralInvitations({ invitesLeft: 0, invitesAllowed: 0 })).toEqual({ kind: "not_approved" });
    // A Gateway older than invitesAllowed cannot tell the two apart: the page claims nothing was used.
    expect(referralInvitations({ invitesLeft: 0 })).toEqual({ kind: "not_approved" });
    // A number that cannot be a count admits nobody.
    expect(referralInvitations({ invitesLeft: -1 })).toEqual({ kind: "not_approved" });
    expect(referralInvitations({ invitesLeft: Number.NaN, invitesAllowed: 4 })).toEqual({ kind: "used_up", allowed: 4 });
    expect(invitationsClosed({ kind: "used_up", allowed: 6 }) && invitationsClosed({ kind: "not_approved" })).toBe(true);
  });

  it("counts what is used and left while invitations remain", () => {
    expect(referralInvitations({ invitesLeft: 3, invitesAllowed: 10 })).toEqual({ kind: "approved", left: 3, allowed: 10 });
    expect(invitationsCount({ kind: "approved", left: 3, allowed: 10 })).toEqual({ text: "7 of 10 invitations used", left: "3 left", share: 0.7, usedUp: false });
    expect(invitationsCount({ kind: "approved", left: 3 })).toEqual({ text: "3 invitations left", left: null, share: null, usedUp: false });
    expect(invitationsClosed({ kind: "approved", left: 3, allowed: 10 })).toBe(false);
  });

  it("says the invitations ran out, with a full meter", () => {
    const usedUp = { kind: "used_up", allowed: 1 } as const;
    expect(invitationsCount(usedUp)).toEqual({ text: "1 of 1 invitation used", left: "None left", share: 1, usedUp: true });
    expect(invitationsNotice(usedUp)?.title).toBe("You used all your invitations");
    expect(invitationsNotice({ kind: "used_up", allowed: 30 })?.body).toContain("All 30 invitations Nexus approved for you are used");
    expect(invitationsNotice({ kind: "not_approved" })?.title).toBe("Invitations come by approval");
    expect(invitationsNotice({ kind: "approved", left: 1, allowed: 2 })).toBeNull();
  });
});
