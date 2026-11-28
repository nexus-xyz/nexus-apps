import { describe, expect, it } from "vitest";
import { referralOffer, joinedLine, keyExpiry, keyLine, referralLink, referredEmail } from "./account";

const NOW = Date.parse("2026-11-05T12:00:00Z");

describe("a key's expiry", () => {
  it("says never, a count of days inside a month, then the date", () => {
    expect(keyExpiry(undefined, NOW)).toBe("Never");
    expect(keyExpiry("2026-11-11T12:00:00Z", NOW)).toBe("in 6 days");
    expect(keyExpiry("2026-11-06T06:00:00Z", NOW)).toBe("tomorrow");
    expect(keyExpiry("2027-02-01T12:00:00", NOW)).toBe("1 Feb 2027");
  });

  it("says when it expired, with the year only when it is another", () => {
    expect(keyExpiry("2026-10-01T12:00:00", NOW)).toBe("Expired 1 Oct");
    expect(keyExpiry("2025-10-01T12:00:00", NOW)).toBe("Expired 1 Oct 2025");
  });

  it("leads with the key's start", () => {
    expect(keyLine("nx_8f2k", undefined, NOW)).toBe("nx_8f2k… · Never");
  });
});

describe("referrals", () => {
  it("builds the link on Nexus UI", () => {
    expect(referralLink("https://nexus.inc/", "k3f7 q2")).toBe("https://nexus.inc/r/k3f7%20q2");
  });

  it("dots a masked email, says Erased once the person is gone, and Email pending for a live one with none", () => {
    expect(referredEmail({ email: "j***@lumen.example", erased: false })).toBe("j•••@lumen.example");
    expect(referredEmail({ email: null, erased: true })).toBe("Erased");
    expect(referredEmail({ email: null, erased: false })).toBe("Email pending");
    expect(referredEmail({ email: null }), "an older Gateway").toBe("Erased");
  });

  it("counts the people who joined", () => {
    expect(joinedLine(4, false)).toBe("4 joined");
    expect(joinedLine(50, true)).toBe("50+ joined");
  });
});

describe("referral program state", () => {
  const list = {code:"invite",referred:[],invitesLeft:3,rewardProgress:{granted:9,limit:10},rewardsActive:true,rewardUsdDecimal:"1.000000"};
  it("hides the offer and link while paused, preserving access to history", () => {
    const offer = referralOffer({...list, enabled:false}, "https://nexus.test");
    expect(offer.title).toBe("Referrals are paused");
    expect(offer.link).toBeUndefined();
    expect(offer.body).toContain("history");
  });
  it("shows progress, the Gateway's amount and the personal-account condition", () => {
    const offer = referralOffer(list, "https://nexus.test");
    expect(offer.progress).toBe("9 of 10 lifetime rewards");
    expect(offer.body).toBe("Get $1 of promotional credit when a person who joined through your link buys at least $10 of Nexus credits in their personal account.");
    expect(referralOffer({code:"staff",rewardsGranted:12,rewardsActive:true,rewardUsdDecimal:"1.000000"}, "https://nexus.test").progress).toBe("12 rewarded referrals");
    expect(offer.link).toBe("https://nexus.test/r/invite");
  });
  it("promises no money while Nexus pays no reward", () => {
    for (const quiet of [{...list,rewardsActive:false}, {...list,rewardUsdDecimal:"0.000000"}, {code:"old",rewardProgress:list.rewardProgress}]) {
      const offer = referralOffer(quiet, "https://nexus.test");
      expect(offer.title).toBe("Invite people to Nexus");
      expect(offer.body).not.toContain("$");
      expect(offer.body).toContain("does not pay referral rewards now");
      expect(offer.link).toBe(`https://nexus.test/r/${quiet.code}`);
    }
  });
  it.each([10, 11])("stops promising rewards at %i lifetime grants while still permitting invitations", granted => {
    const capped = {...list,rewardProgress:{granted,limit:10}};
    const offer = referralOffer(capped, "https://nexus.test");
    expect(offer.title).toBe("Lifetime reward limit reached");
    expect(offer.body).toContain("Further referrals do not earn credit");
    expect(offer.link).toBeDefined();
  });
  it("counts invitations only while sign-up is invite-only", () => {
    expect(referralOffer({code:"open"}, "https://nexus.test").invites).toBeUndefined();
    expect(referralOffer(list, "https://nexus.test").invites).toEqual({ text: "3 invitations left", left: null, share: null });
    expect(referralOffer({...list,invitesAllowed:10}, "https://nexus.test").invites).toEqual({ text: "7 of 10 invitations used", left: "3 left", share: 0.7 });
  });
  it("says every invitation is used, in the danger tone, and hides the link", () => {
    const offer = referralOffer({...list,invitesLeft:0,invitesAllowed:6}, "https://nexus.test");
    expect(offer.usedUp).toBe(true);
    expect(offer.title).toBe("You used all your invitations");
    expect(offer.body).toContain("All 6 invitations Nexus approved for you are used");
    expect(offer.invites).toEqual({ text: "6 of 6 invitations used", left: "None left", share: 1 });
    expect(offer.link).toBeUndefined();
    const never = referralOffer({...list,invitesLeft:0,invitesAllowed:0}, "https://nexus.test");
    expect([never.usedUp, never.title, never.invites]).toEqual([false, "Invitations come by approval", undefined]);
  });
  it("respects invitation allowance independently of rewards and handles older Gateways", () => {
    expect(referralOffer({...list,invitesLeft:0}, "https://nexus.test").link).toBeUndefined();
    expect(referralOffer({code:"old"}, "https://nexus.test").link).toBe("https://nexus.test/r/old");
    expect(referralOffer({code:"old"}, "https://nexus.test").progress).toBe("Reward progress unavailable on this Gateway");
  });
});
